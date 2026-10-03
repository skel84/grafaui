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
    api::ApiRequest,
    lifecycle::Cancellation,
    plan::VariablePlan,
    query::{self, Variables},
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
        request: &ApiRequest,
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
        let request = if post {
            let url = self
                .base
                .join(&format!("api/v1/{}", request.path))
                .map_err(|_| "invalid API path")?;
            self.http
                .post(url)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(request.form())
        } else {
            let url = self
                .base
                .join(&request.get_target()?)
                .map_err(|_| "invalid API path")?;
            self.http.get(url)
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
                .request(&request.api(), true, cancellation)
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
        let mut plan = VariablePlan::new(definitions, selections, window, self.scrape_interval);
        loop {
            cancellation.check()?;
            let Some(request) = plan.next_request()? else {
                return Ok(plan.finish());
            };
            let value = self
                .request(&request, false, cancellation)
                .map_err(|e| plan.failed(&e))?;
            plan.answer(&value)?;
        }
    }
}
