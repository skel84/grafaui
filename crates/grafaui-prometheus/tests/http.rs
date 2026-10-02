//! Local HTTP server exercises encoded wire requests, errors and in-flight
//! cancellation. No cluster or Prometheus process is needed for these tests.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;

use grafaui_model::{Dashboard, data::QueryContext, time::TimeWindow};
use grafaui_prometheus::{
    Client, Variables,
    lifecycle::{Cancellation, Run},
};

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    loop {
        let mut block = [0; 1024];
        let count = stream.read(&mut block).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&block[..count]);
        if let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let length = headers
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

fn context() -> QueryContext {
    QueryContext {
        window: TimeWindow::new(1000, 900, 90),
        variables: vec![],
    }
}

#[test]
fn dependent_variables_and_encoded_instant_range_requests_use_one_prefixed_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::new(
        &format!("http://{}/prometheus", listener.local_addr().unwrap()),
        15.,
    )
    .unwrap();
    let (sent, received) = mpsc::channel();
    let server = thread::spawn(move || {
        for body in [
            r#"{"status":"success","data":["node-exporter"]}"#,
            r#"{"status":"success","data":["10.0.0.1:9100"]}"#,
            r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"instance":"10.0.0.1:9100"},"value":[1000,"2"]}]}}"#,
            r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"instance":"10.0.0.1:9100"},"values":[[990,"3"]]}]}}"#,
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            sent.send(read_request(&mut stream)).unwrap();
            respond(&mut stream, "200 OK", body);
        }
    });
    let dashboard = Dashboard::parse(r#"{"templating":{"list":[
        {"name":"job","type":"query","query":{"query":"label_values(node_uname_info, job)"}},
        {"name":"node","type":"query","query":"label_values(node_uname_info{job=\"$job\"}, instance)"}]},
        "panels":[{"type":"timeseries","targets":[
        {"expr":"up{instance=\"$node\"}","refId":"A","instant":true,"range":false,"legendFormat":"{{instance}}"},
        {"expr":"rate(x[$__rate_interval])","refId":"B","range":true,"step":30}]}]}"#).unwrap();
    let cancellation = Cancellation::default();
    let (variables, _) = client
        .resolve_variables(&dashboard.variables, &[], context().window, &cancellation)
        .unwrap();
    assert_eq!(variables.selection(0), "node-exporter");
    assert_eq!(variables.selection(1), "10.0.0.1:9100");
    let (frame, _) = client
        .query_panel(dashboard.panel(0), &context(), &variables, &cancellation)
        .unwrap();
    assert_eq!(frame.series.len(), 2);
    assert_eq!(frame.series[0].query, "A");
    assert_eq!(frame.series[1].query, "B");
    assert_eq!(frame.series[0].name, "10.0.0.1:9100");
    assert!(
        frame.series[0].values[..frame.times.len() - 1]
            .iter()
            .all(|v| v.is_nan())
    );
    server.join().unwrap();
    let requests: Vec<_> = received.iter().collect();
    assert!(requests[0].starts_with("GET /prometheus/api/v1/label/job/values?"));
    assert!(requests[0].contains("match%5B%5D=node_uname_info"));
    assert!(requests[1].contains("job%3D%22node-exporter%22"));
    assert!(requests[2].starts_with("POST /prometheus/api/v1/query "));
    assert!(requests[2].contains("query=up%7Binstance%3D%2210.0.0.1%3A9100%22%7D"));
    assert!(requests[2].contains("time=1000"));
    assert!(requests[3].starts_with("POST /prometheus/api/v1/query_range "));
    assert!(requests[3].contains("step=30"));
    assert!(requests[3].contains("%5B60s%5D"));
}

#[test]
fn absent_cluster_label_warns_but_dependent_variables_and_panels_still_query() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap()), 30.).unwrap();
    let (sent, received) = mpsc::channel();
    let server = thread::spawn(move || {
        for body in [
            r#"{"status":"success","data":[]}"#,
            r#"{"status":"success","data":["node-exporter"]}"#,
            r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1000,"3"]}]}}"#,
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            sent.send(read_request(&mut stream)).unwrap();
            respond(&mut stream, "200 OK", body);
        }
    });
    // Mirrors Kubernetes / Views / Global: cluster is optional on exported
    // metrics, and a multi-value job is used in an exact equality matcher.
    let dashboard = Dashboard::parse(r#"{"templating":{"list":[
        {"name":"cluster","type":"query","query":"label_values(kube_node_info,cluster)","current":{"text":"old-cluster","value":"old-cluster"}},
        {"name":"job","type":"query","multi":true,"query":"label_values(node_cpu_seconds_total{cluster=\"$cluster\"},job)"}]},
        "panels":[{"type":"stat","targets":[{"expr":"count(node_cpu_seconds_total{cluster=\"$cluster\",job=\"$job\"})","refId":"A","instant":true,"range":false}]}]}"#).unwrap();
    let cancellation = Cancellation::default();
    let (variables, warnings) = client
        .resolve_variables(&dashboard.variables, &[], context().window, &cancellation)
        .unwrap();
    assert!(variables.options(0).is_empty());
    assert_eq!(variables.selection(0), "");
    assert_eq!(variables.selection(1), "node-exporter");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("variable cluster returned no values"));
    let (frame, _) = client
        .query_panel(dashboard.panel(0), &context(), &variables, &cancellation)
        .unwrap();
    assert_eq!(frame.series[0].values, [3.]);
    server.join().unwrap();
    let requests: Vec<_> = received.iter().collect();
    assert!(requests[0].starts_with("GET /api/v1/label/cluster/values?"));
    assert!(requests[1].starts_with("GET /api/v1/label/job/values?"));
    assert!(requests[1].contains("cluster%3D%22%22"));
    assert!(requests[2].contains("cluster%3D%22%22%2Cjob%3D%22node-exporter%22"));
    assert!(!requests.iter().any(|r| r.contains("old-cluster")));
}

#[test]
fn failed_variable_request_remains_an_error_instead_of_an_empty_selection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap()), 15.).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        respond(
            &mut stream,
            "503 Service Unavailable",
            r#"{"status":"error","errorType":"unavailable","error":"temporarily unavailable"}"#,
        );
    });
    let dashboard = Dashboard::parse(r#"{"templating":{"list":[{"name":"cluster","type":"query","query":"label_values(kube_node_info,cluster)"}]}}"#).unwrap();
    let error = client
        .resolve_variables(
            &dashboard.variables,
            &[],
            context().window,
            &Cancellation::default(),
        )
        .unwrap_err();
    assert!(error.contains("variable cluster: HTTP 503"));
    server.join().unwrap();
}

#[test]
fn failed_real_request_returns_http_and_prometheus_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap()), 15.).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        respond(
            &mut stream,
            "422 Unprocessable Entity",
            r#"{"status":"error","errorType":"execution","error":"bad query"}"#,
        );
    });
    let dashboard = Dashboard::parse(
        r#"{"panels":[{"type":"stat","targets":[{"expr":"up","refId":"Z","instant":true}]}]}"#,
    )
    .unwrap();
    let error = client
        .query_panel(
            dashboard.panel(0),
            &context(),
            &Variables::default(),
            &Cancellation::default(),
        )
        .unwrap_err();
    assert!(error.contains("Z: HTTP 422"));
    assert!(error.contains("execution: bad query"));
    server.join().unwrap();
}

#[test]
fn refresh_during_http_discards_old_response_and_stops_following_targets() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap()), 15.).unwrap();
    let arrived = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let server_arrived = arrived.clone();
    let server_release = release.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        server_arrived.wait();
        server_release.wait();
        respond(
            &mut stream,
            "200 OK",
            r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1000,"42"]}]}}"#,
        );
        listener.set_nonblocking(true).unwrap();
        assert!(
            listener.accept().is_err(),
            "a cancelled run must not request its next target"
        );
    });
    let dashboard = Dashboard::parse(r#"{"panels":[{"type":"stat","targets":[{"expr":"up","refId":"A","instant":true},{"expr":"up","refId":"B","instant":true}]}]}"#).unwrap();
    let old = Run::new(1);
    let token = old.cancellation();
    let worker = thread::spawn(move || {
        client.query_panel(
            dashboard.panel(0),
            &context(),
            &Variables::default(),
            &token,
        )
    });
    arrived.wait();
    drop(old);
    let new = Run::new(2);
    release.wait();
    assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
    assert!(!new.accepts(1));
    assert!(new.accepts(2));
    server.join().unwrap();
}

#[test]
fn invalid_or_credential_bearing_urls_are_rejected_without_echoing_them() {
    for url in [
        "file:///tmp/test",
        "http://user:secret@localhost:9090",
        "http://localhost:9090?token=secret",
    ] {
        let error = Client::new(url, 15.).err().unwrap();
        assert!(!error.contains("secret"));
    }
}
