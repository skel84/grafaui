//! Variable resolution without I/O. [`VariablePlan`] walks the definitions in
//! dashboard order, so a dependent variable sees the preceding one's new
//! selection. It hands out the next [`ApiRequest`] it needs, takes the JSON
//! answer, and ends with the resolved [`Variables`] and warnings. Any transport
//! drives it; `Client::resolve_variables` is a loop over it.
use grafaui_model::{Variable, time::TimeWindow};
use serde_json::Value;

use crate::{
    Result,
    api::ApiRequest,
    query::{self, VariableQuery, Variables},
    response,
};

pub struct VariablePlan {
    definitions: Vec<Variable>,
    window: TimeWindow,
    scrape_interval: f64,
    variables: Variables,
    warnings: Vec<String>,
    /// The next definition to resolve.
    index: usize,
    /// The query waiting for an answer, for the definition at `index`.
    pending: Option<VariableQuery>,
}

impl VariablePlan {
    pub fn new(
        definitions: &[Variable],
        selections: &[String],
        window: TimeWindow,
        scrape_interval: f64,
    ) -> Self {
        Self {
            definitions: definitions.to_vec(),
            window,
            scrape_interval,
            variables: Variables::new(definitions, selections),
            warnings: Vec::new(),
            index: 0,
            pending: None,
        }
    }

    /// Resolves every variable that needs no request, and returns the next
    /// request, or `None` once all are resolved. Call [`Self::answer`] with
    /// the request's JSON before calling this again.
    pub fn next_request(&mut self) -> Result<Option<ApiRequest>> {
        if self.pending.is_some() {
            return Err("the previous variable request has no answer yet".into());
        }
        while let Some(definition) = self.definitions.get(self.index) {
            let name = &definition.name;
            query::check_datasource(&definition.datasource)
                .map_err(|e| format!("variable {name}: {e}"))?;
            let options = match definition.kind.as_str() {
                "datasource" => {
                    if definition.query.as_deref() != Some("prometheus") {
                        return Err(format!(
                            "variable {name}: only the Prometheus datasource is available"
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
                    let (query, request) = self.query_request(definition)?;
                    self.pending = Some(query);
                    return Ok(Some(request));
                }
                kind => {
                    return Err(format!("variable {name}: unsupported variable type {kind}"));
                }
            };
            self.resolve(options)?;
        }
        Ok(None)
    }

    /// Takes the answer to the request [`Self::next_request`] returned.
    pub fn answer(&mut self, value: &Value) -> Result<()> {
        let query = self
            .pending
            .take()
            .ok_or("no variable request is waiting for an answer")?;
        let name = self.definitions[self.index].name.clone();
        let (data, notes) =
            response::envelope(value).map_err(|e| format!("variable {name}: {e}"))?;
        self.warnings.extend(notes);
        let options = Self::options(query, data).map_err(|e| format!("variable {name}: {e}"))?;
        self.resolve(options)
    }

    /// The request failed: the error, naming the variable it was for.
    pub fn failed(&self, error: &str) -> String {
        match self.definitions.get(self.index) {
            Some(definition) => format!("variable {}: {error}", definition.name),
            None => error.to_owned(),
        }
    }

    /// The resolved variables and the warnings collected on the way.
    pub fn finish(self) -> (Variables, Vec<String>) {
        (self.variables, self.warnings)
    }

    fn resolve(&mut self, options: Vec<String>) -> Result<()> {
        if let Some(warning) = self.variables.resolve(self.index, options)? {
            self.warnings.push(warning);
        }
        self.index += 1;
        Ok(())
    }

    fn query_request(&self, definition: &Variable) -> Result<(VariableQuery, ApiRequest)> {
        let window = self.window;
        let text = definition
            .query
            .as_deref()
            .ok_or_else(|| format!("variable {} has no query", definition.name))?;
        let text = self.variables.interpolate(text, &[])?;
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
        let path = match &query {
            VariableQuery::LabelValues { selector, label } => {
                // Request distinct values directly. Fetching every series for
                // broad container selectors can exceed the response limit just
                // to populate a dropdown.
                if let Some(selector) = selector {
                    params.push(("match[]".into(), selector.clone()));
                }
                format!("label/{label}/values")
            }
            VariableQuery::LabelNames => "labels".into(),
            VariableQuery::Metrics(_) => "label/__name__/values".into(),
            VariableQuery::QueryResult(expression) => {
                params = vec![
                    ("query".into(), expression.clone()),
                    ("time".into(), window.end.to_string()),
                    ("timeout".into(), "15s".into()),
                ];
                "query".into()
            }
        };
        Ok((query, ApiRequest::new(path, params)))
    }

    fn options(query: VariableQuery, data: &Value) -> Result<Vec<String>> {
        match query {
            VariableQuery::QueryResult(_) => {
                if data.get("resultType").and_then(Value::as_str) != Some("vector") {
                    return Err("query_result variables require a vector response".into());
                }
                data.get("result")
                    .and_then(Value::as_array)
                    .ok_or("invalid query_result response")?
                    .iter()
                    .map(|entry| {
                        let labels = entry
                            .get("metric")
                            .and_then(Value::as_object)
                            .ok_or("query_result missing metric")?
                            .iter()
                            .map(|(k, v)| {
                                Ok((
                                    k.clone(),
                                    v.as_str().ok_or("invalid metric label")?.to_owned(),
                                ))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let sample = entry
                            .get("value")
                            .and_then(Value::as_array)
                            .ok_or("query_result missing sample (native histograms unsupported)")?;
                        Ok(format!(
                            "{} {} {}",
                            response::metric_name(&labels),
                            sample
                                .get(1)
                                .and_then(Value::as_str)
                                .ok_or("invalid sample")?,
                            sample.first().ok_or("missing timestamp")?
                        ))
                    })
                    .collect()
            }
            VariableQuery::Metrics(pattern) => {
                let regex =
                    regex::Regex::new(&pattern).map_err(|e| format!("metrics regex: {e}"))?;
                Ok(data
                    .as_array()
                    .ok_or("invalid label values response")?
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| regex.is_match(s))
                    .map(str::to_owned)
                    .collect())
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
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafaui_model::Dashboard;
    use serde_json::json;

    fn window() -> TimeWindow {
        TimeWindow::new(1000, 900, 90)
    }

    fn plan(json: &str, selections: &[String]) -> VariablePlan {
        let dashboard = Dashboard::parse(json).unwrap();
        VariablePlan::new(&dashboard.variables, selections, window(), 15.)
    }

    fn values(items: &[&str]) -> Value {
        json!({"status": "success", "data": items})
    }

    fn param<'a>(request: &'a ApiRequest, key: &str) -> Option<&'a str> {
        request
            .params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn dependent_variables_see_the_previous_selection() {
        let mut plan = plan(
            r#"{"templating":{"list":[
            {"name":"job","type":"query","query":"label_values(node_uname_info, job)"},
            {"name":"node","type":"query","query":"label_values(node_uname_info{job=\"$job\"}, instance)"}]}}"#,
            &[],
        );
        let first = plan.next_request().unwrap().unwrap();
        assert_eq!(first.path, "label/job/values");
        assert_eq!(param(&first, "match[]"), Some("node_uname_info"));
        assert_eq!(param(&first, "start"), Some("100"));
        assert_eq!(param(&first, "end"), Some("1000"));
        // A second call before the answer is a mistake, not a repeat.
        assert!(plan.next_request().is_err());
        plan.answer(&values(&["nodes", "other"])).unwrap();

        let second = plan.next_request().unwrap().unwrap();
        assert_eq!(second.path, "label/instance/values");
        assert_eq!(
            param(&second, "match[]"),
            Some(r#"node_uname_info{job="nodes"}"#)
        );
        plan.answer(&values(&["10.0.0.1:9100"])).unwrap();
        assert_eq!(plan.next_request().unwrap(), None);

        let (variables, warnings) = plan.finish();
        assert!(warnings.is_empty());
        assert_eq!(variables.selection(0), "nodes");
        assert_eq!(variables.options(1), ["10.0.0.1:9100"]);
    }

    #[test]
    fn include_all_keeps_all_and_interpolates_every_value() {
        let mut plan = plan(
            r#"{"templating":{"list":[
            {"name":"ns","type":"query","includeAll":true,"multi":true,"query":"label_values(kube_pod_info, namespace)","current":{"value":"$__all"}},
            {"name":"pod","type":"query","query":"query_result(count by (pod) (kube_pod_info{namespace=~\"$ns\"}))"}]}}"#,
            &[],
        );
        plan.next_request().unwrap().unwrap();
        plan.answer(&values(&["default", "kube-system"])).unwrap();
        let second = plan.next_request().unwrap().unwrap();
        assert_eq!(second.path, "query");
        assert_eq!(
            param(&second, "query"),
            Some(r#"count by (pod) (kube_pod_info{namespace=~"(default|kube-system)"})"#)
        );
        assert_eq!(param(&second, "time"), Some("1000"));
        plan.answer(
            &json!({"status":"success","data":{"resultType":"vector","result":[
            {"metric":{"pod":"a"},"value":[1000,"1"]}]}}),
        )
        .unwrap();
        assert_eq!(plan.next_request().unwrap(), None);
        let (variables, _) = plan.finish();
        assert_eq!(variables.selection(0), "All");
        assert_eq!(variables.options(0), ["All", "default", "kube-system"]);
        assert_eq!(variables.options(1), [r#"{pod="a"} 1 1000"#]);
    }

    #[test]
    fn an_empty_answer_warns_and_selects_nothing() {
        let mut plan = plan(
            r#"{"templating":{"list":[
            {"name":"node","type":"query","query":"label_values(up, instance)","current":{"value":"gone"}}]}}"#,
            &[],
        );
        plan.next_request().unwrap().unwrap();
        plan.answer(&values(&[])).unwrap();
        assert_eq!(plan.next_request().unwrap(), None);
        let (variables, warnings) = plan.finish();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("returned no values"), "{warnings:?}");
        assert_eq!(variables.selection(0), "");
    }

    #[test]
    fn static_variables_need_no_request() {
        let mut plan = plan(
            r#"{"templating":{"list":[
            {"name":"cluster","type":"constant","query":"prod"},
            {"name":"env","type":"custom","query":"dev, prod","current":{"value":"prod"}},
            {"name":"ds","type":"datasource","query":"prometheus"}]}}"#,
            &[],
        );
        assert_eq!(plan.next_request().unwrap(), None);
        let (variables, _) = plan.finish();
        assert_eq!(variables.selection(0), "prod");
        assert_eq!(variables.options(1), ["dev", "prod"]);
        assert_eq!(variables.selection(1), "prod");
        assert_eq!(variables.selection(2), "Prometheus");
    }

    #[test]
    fn errors_name_the_variable_and_answers_without_requests_are_refused() {
        let mut plan = plan(
            r#"{"templating":{"list":[{"name":"node","type":"query","query":"label_values(up, instance)"}]}}"#,
            &[],
        );
        assert!(plan.answer(&values(&["x"])).is_err());
        plan.next_request().unwrap().unwrap();
        let error = plan
            .answer(&json!({"status":"error","errorType":"bad_data","error":"nope"}))
            .unwrap_err();
        assert_eq!(error, "variable node: bad_data: nope");
        assert_eq!(plan.failed("HTTP 403"), "variable node: HTTP 403");

        let mut unsupported = super::tests::plan(
            r#"{"templating":{"list":[{"name":"x","type":"adhoc"}]}}"#,
            &[],
        );
        assert!(unsupported.next_request().unwrap_err().contains("adhoc"));
    }
}
