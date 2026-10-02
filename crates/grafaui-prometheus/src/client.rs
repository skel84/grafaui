use std::io::Read;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use grafaui_model::{
    PanelSpec, Variable,
    data::{Frame, QueryContext},
    time::TimeWindow,
};
use reqwest::{Url, blocking::Client as HttpClient};
use serde_json::Value;

use crate::{
    Result,
    lifecycle::Cancellation,
    query::{self, VariableQuery, Variables},
    response,
};

#[derive(Clone)]
pub struct Client {
    http: HttpClient,
    base: Url,
    scrape_interval: f64,
    slots: Arc<(Mutex<usize>, Condvar)>,
}

struct Permit(Arc<(Mutex<usize>, Condvar)>);
impl Drop for Permit {
    fn drop(&mut self) {
        let (lock, ready) = &*self.0;
        *lock.lock().unwrap() -= 1;
        ready.notify_one();
    }
}

impl Client {
    pub fn new(url: &str, scrape_interval: f64) -> Result<Self> {
        let mut base = Url::parse(url).map_err(|_| "invalid Prometheus URL".to_owned())?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err("Prometheus URL must use http or https".into());
        }
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("URL credentials, query strings and fragments are unsupported".into());
        }
        if scrape_interval <= 0. || !scrape_interval.is_finite() {
            return Err("scrape interval must be positive".into());
        }
        base.set_path(&format!("{}/", base.path().trim_end_matches('/')));
        let http = HttpClient::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| "could not create Prometheus HTTP client".to_owned())?;
        Ok(Self {
            http,
            base,
            scrape_interval,
            slots: Arc::new((Mutex::new(0), Condvar::new())),
        })
    }

    pub fn label(&self) -> String {
        format!("Prometheus · {}", self.base)
    }

    fn request(
        &self,
        path: &str,
        params: &[(String, String)],
        post: bool,
        cancellation: &Cancellation,
    ) -> Result<Value> {
        cancellation.check()?;
        // Bound concurrency across old and new dashboard revisions as well.
        let (lock, ready) = &*self.slots;
        let mut active = lock.lock().unwrap();
        while *active >= 4 {
            cancellation.check()?;
            active = ready
                .wait_timeout(active, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        cancellation.check()?;
        *active += 1;
        drop(active);
        let _permit = Permit(self.slots.clone());
        let url = self
            .base
            .join(&format!("api/v1/{path}"))
            .map_err(|_| "invalid API path")?;
        let request = if post {
            self.http.post(url).form(params)
        } else {
            self.http.get(url).query(params)
        };
        let mut response = request.send().map_err(|e| {
            // Don't include the URL (or potential credentials) in error output.
            if e.is_timeout() {
                "Prometheus request timed out".to_owned()
            } else if e.is_connect() {
                "could not connect to Prometheus".to_owned()
            } else {
                "Prometheus HTTP request failed".to_owned()
            }
        })?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "could not read Prometheus response")?;
        cancellation.check()?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err("Prometheus response exceeds 64 MiB".into());
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| format!("Prometheus returned HTTP {status} with invalid JSON"))?;
        if !status.is_success() {
            let message = response::envelope(&value)
                .err()
                .unwrap_or_else(|| "request failed".into());
            return Err(format!("HTTP {status}: {message}"));
        }
        Ok(value)
    }

    pub fn query_panel(
        &self,
        panel: &PanelSpec,
        context: &QueryContext,
        variables: &Variables,
        cancellation: &Cancellation,
    ) -> Result<(Frame, Vec<String>)> {
        let requests = query::requests(panel, context, variables, self.scrape_interval)?;
        let mut frames = Vec::new();
        let mut warnings = Vec::new();
        for request in requests {
            let value = self
                .request(request.path(), &request.params(), true, cancellation)
                .map_err(|e| format!("{}: {e}", request.ref_id()))?;
            let (frame, notes) = response::convert(&value, &request)
                .map_err(|e| format!("{}: {e}", request.ref_id()))?;
            frames.push(frame);
            warnings.extend(notes);
        }
        if panel.queries.iter().any(|q| q.request.exemplar) {
            warnings.push("Exemplars are not requested by this connection".into());
        }
        if panel.queries.iter().any(|q| {
            q.request.format.as_deref() == Some("table")
                && q.request
                    .range
                    .unwrap_or(!q.request.instant.unwrap_or(false))
        }) {
            warnings.push("Range table targets currently reduce each series to one row".into());
        }
        cancellation.check()?;
        Ok((response::merge(frames), warnings))
    }

    /// Resolve in dashboard order so dependent variables see the preceding
    /// variable's new selection. Runs again on refresh/range/selection changes.
    pub fn resolve_variables(
        &self,
        definitions: &[Variable],
        selections: &[String],
        window: TimeWindow,
        cancellation: &Cancellation,
    ) -> Result<(Variables, Vec<String>)> {
        let mut variables = Variables::new(definitions, selections);
        let mut warnings = Vec::new();
        for (index, definition) in definitions.iter().enumerate() {
            cancellation.check()?;
            query::check_datasource(&definition.datasource)
                .map_err(|e| format!("variable {}: {e}", definition.name))?;
            let options = match definition.kind.as_str() {
                "datasource" => {
                    if definition.query.as_deref() != Some("prometheus") {
                        return Err(format!(
                            "variable {}: only the Prometheus datasource is available",
                            definition.name
                        ));
                    }
                    vec!["Prometheus".into()]
                }
                "constant" | "textbox" => {
                    let value = definition
                        .selected
                        .first()
                        .cloned()
                        .or_else(|| definition.query.clone())
                        .unwrap_or_else(|| definition.current.clone());
                    vec![value]
                }
                "custom" | "interval" => {
                    if !definition.saved_options.is_empty() {
                        definition.saved_options.clone()
                    } else {
                        definition
                            .query
                            .as_deref()
                            .unwrap_or("")
                            .split(',')
                            .map(|v| v.trim().to_owned())
                            .collect()
                    }
                }
                "query" => {
                    let text = definition
                        .query
                        .as_deref()
                        .ok_or_else(|| format!("variable {} has no query", definition.name))?;
                    let text = variables.interpolate(text, &[])?;
                    let interval = (window.span as f64 / 600.).ceil().max(1.);
                    let text = query::macros(
                        &text,
                        interval,
                        (interval + self.scrape_interval).max(4. * self.scrape_interval),
                        window,
                    )?;
                    let query = query::variable_query(&text)
                        .map_err(|e| format!("variable {}: {e}", definition.name))?;
                    let mut params = vec![
                        (
                            "start".into(),
                            (window.end - window.span as i64).to_string(),
                        ),
                        ("end".into(), window.end.to_string()),
                    ];
                    let path;
                    match &query {
                        VariableQuery::LabelValues { selector, label } => {
                            // Request distinct values directly. Fetching every
                            // series for broad container selectors can exceed
                            // the response limit just to populate a dropdown.
                            path = format!("label/{label}/values");
                            if let Some(selector) = selector {
                                params.push(("match[]".into(), selector.clone()));
                            }
                        }
                        VariableQuery::LabelNames => path = "labels".into(),
                        VariableQuery::Metrics(_) => path = "label/__name__/values".into(),
                        VariableQuery::QueryResult(expression) => {
                            path = "query".into();
                            params = vec![
                                ("query".into(), expression.clone()),
                                ("time".into(), window.end.to_string()),
                                ("timeout".into(), "15s".into()),
                            ];
                        }
                    }
                    let value = self
                        .request(&path, &params, false, cancellation)
                        .map_err(|e| format!("variable {}: {e}", definition.name))?;
                    let (data, notes) = response::envelope(&value)?;
                    warnings.extend(notes);
                    match query {
                        VariableQuery::QueryResult(_) => {
                            if data.get("resultType").and_then(Value::as_str) != Some("vector") {
                                return Err(
                                    "query_result variables require a vector response".into()
                                );
                            }
                            data.get("result").and_then(Value::as_array).ok_or("invalid query_result response")?.iter().map(|entry| {
                                let labels = entry.get("metric").and_then(Value::as_object).ok_or("query_result missing metric")?.iter()
                                    .map(|(k, v)| Ok((k.clone(), v.as_str().ok_or("invalid metric label")?.to_owned()))).collect::<Result<Vec<_>>>()?;
                                let sample = entry.get("value").and_then(Value::as_array).ok_or("query_result missing sample (native histograms unsupported)")?;
                                Ok(format!("{} {} {}", response::metric_name(&labels), sample.get(1).and_then(Value::as_str).ok_or("invalid sample")?, sample.first().ok_or("missing timestamp")?))
                            }).collect::<Result<Vec<_>>>()?
                        }
                        VariableQuery::Metrics(pattern) => {
                            let regex = regex::Regex::new(&pattern)
                                .map_err(|e| format!("metrics regex: {e}"))?;
                            data.as_array()
                                .ok_or("invalid label values response")?
                                .iter()
                                .filter_map(Value::as_str)
                                .filter(|s| regex.is_match(s))
                                .map(str::to_owned)
                                .collect()
                        }
                        _ => data
                            .as_array()
                            .ok_or("invalid label values response")?
                            .iter()
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_owned)
                                    .ok_or("invalid label value".into())
                            })
                            .collect::<Result<Vec<_>>>()?,
                    }
                }
                kind => {
                    return Err(format!(
                        "variable {}: unsupported variable type {kind}",
                        definition.name
                    ));
                }
            };
            if let Some(warning) = variables.resolve(index, options)? {
                warnings.push(warning);
            }
        }
        Ok((variables, warnings))
    }
}
