//! Exercise the exact production conversion and variable/request paths against
//! a live endpoint, without opening a window. Errors yield a failing exit code.
use grafaui_model::{Dashboard, data::QueryContext, time::TimeWindow};
use grafaui_prometheus::{Client, lifecycle::Cancellation};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .ok_or("usage: check URL DASHBOARD.json [from] [scrape-interval-seconds]")?;
    let path = args.next().ok_or("missing dashboard path")?;
    let from = args.next().unwrap_or_else(|| "now-1h".into());
    let scrape_interval = args
        .next()
        .map(|s| {
            s.parse::<f64>()
                .map_err(|_| "invalid scrape interval".to_owned())
        })
        .transpose()?
        .unwrap_or(15.);
    let dashboard =
        Dashboard::parse_unexpanded(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let client = Client::new(&url, scrape_interval)?;
    let cancellation = Cancellation::default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut context = QueryContext {
        window: TimeWindow::relative(&from, now),
        variables: vec![],
    };
    let (variables, warnings) =
        client.resolve_variables(&dashboard.variables, &[], context.window, &cancellation)?;
    context.variables = variables.context_values();
    for (name, value) in &context.variables {
        println!("${name} = {value}");
    }
    for warning in warnings {
        println!("warning: {warning}");
    }
    let mut errors = 0;
    let mut nonempty = 0;
    let mut empty = 0;
    for panel in &dashboard.panels {
        match client.query_panel(panel, &context, &variables, &cancellation) {
            Ok((frame, warnings)) => {
                let finite = frame
                    .series
                    .iter()
                    .flat_map(|s| &s.values)
                    .filter(|v| v.is_finite())
                    .count();
                if finite > 0 {
                    nonempty += 1;
                } else {
                    empty += 1;
                }
                println!(
                    "{}: {} series, {} timestamps, {finite} finite samples — {}",
                    panel.key,
                    frame.series.len(),
                    frame.times.len(),
                    panel.title
                );
                for warning in warnings {
                    println!("  warning: {warning}");
                }
            }
            Err(error) => {
                errors += 1;
                println!("{}: ERROR {error} — {}", panel.key, panel.title);
            }
        }
    }
    println!(
        "{} panels: {nonempty} with data, {empty} empty, {errors} errors",
        dashboard.panels.len()
    );
    if errors == 0 {
        Ok(())
    } else {
        Err(format!("{errors} panel requests failed"))
    }
}
