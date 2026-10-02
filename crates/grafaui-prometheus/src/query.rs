use grafaui_model::data::QueryContext;
use grafaui_model::spec::{PanelSpec, Query};
use grafaui_model::{Variable, time::TimeWindow};
use promql_parser::{parser, util::parse_duration};
use regex::{Captures, Regex};
use serde_json::Value;

use crate::Result;

/// A snapshot of variable choices and selections, passed with each request.
#[derive(Clone, Debug, Default)]
pub struct Variables {
    entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
struct Entry {
    definition: Variable,
    options: Vec<String>,
    selected: Vec<String>,
}

impl Variables {
    pub fn new(definitions: &[Variable], selections: &[String]) -> Self {
        Self {
            entries: definitions
                .iter()
                .enumerate()
                .map(|(i, v)| Entry {
                    definition: v.clone(),
                    options: v.saved_options.clone(),
                    selected: selections
                        .get(i)
                        .map(|s| s.split(" + ").map(str::to_owned).collect())
                        .unwrap_or_else(|| v.selected.clone()),
                })
                .collect(),
        }
    }

    pub fn options(&self, index: usize) -> &[String] {
        &self.entries[index].options
    }
    pub fn selection(&self, index: usize) -> String {
        self.entries[index].selected.join(" + ")
    }
    pub fn context_values(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.definition.name.clone(), self.selection(i)))
            .collect()
    }

    pub(crate) fn resolve(&mut self, index: usize, mut options: Vec<String>) -> Result<()> {
        let entry = &mut self.entries[index];
        options.retain(|v| !v.is_empty() && v != "$__all" && v != "All");
        let pattern = entry.definition.regex.trim();
        if !pattern.is_empty() {
            let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
            let pattern = pattern.rsplit_once('/').map(|(p, _)| p).unwrap_or(pattern);
            // Grafana stores JavaScript named captures; Rust spells them (?P<name>).
            let pattern = pattern
                .replace("(?<value>", "(?P<value>")
                .replace("(?<text>", "(?P<text>");
            let regex = Regex::new(&pattern)
                .map_err(|e| format!("variable {} regex: {e}", entry.definition.name))?;
            options = options
                .into_iter()
                .filter_map(|value| {
                    let captures = regex.captures(&value)?;
                    Some(
                        captures
                            .name("value")
                            .or_else(|| captures.get(1))
                            .or_else(|| captures.get(0))?
                            .as_str()
                            .to_owned(),
                    )
                })
                .collect();
        }
        match entry.definition.sort {
            0 => {}
            1 | 2 => options.sort(),
            3 | 4 => options.sort_by(|a, b| {
                a.parse::<f64>()
                    .unwrap_or(f64::NAN)
                    .total_cmp(&b.parse::<f64>().unwrap_or(f64::NAN))
            }),
            5 | 6 => options.sort_by_key(|v| v.to_lowercase()),
            n => {
                return Err(format!(
                    "variable {}: unsupported sort mode {n}",
                    entry.definition.name
                ));
            }
        }
        if matches!(entry.definition.sort, 2 | 4 | 6) {
            options.reverse();
        }
        let mut seen = std::collections::HashSet::new();
        options.retain(|v| seen.insert(v.clone()));
        if entry.definition.include_all {
            options.insert(0, "All".into());
        }
        if options.is_empty() {
            return Err(format!(
                "variable {} returned no choices",
                entry.definition.name
            ));
        }
        let all = entry.selected.iter().any(|v| v == "All" || v == "$__all");
        entry.selected = if all && entry.definition.include_all {
            vec!["All".into()]
        } else {
            entry
                .selected
                .iter()
                .filter(|v| options.contains(v))
                .cloned()
                .collect()
        };
        if entry.selected.is_empty() {
            entry.selected.push(options[0].clone());
        }
        entry.options = options;
        Ok(())
    }

    /// Grafana variable syntax, with PromQL string/regex escaping. Unknown
    /// variables and formatters are errors instead of being sent literally.
    pub fn interpolate(&self, text: &str, scoped: &[(String, String)]) -> Result<String> {
        let regex =
            Regex::new(r"\$\{([\w]+)(?::([\w]+))?\}|\[\[([\w]+)(?::([\w]+))?\]\]|\$([\w]+)")
                .unwrap();
        let mut error = None;
        let output = regex
            .replace_all(text, |c: &Captures| {
                let name = c
                    .get(1)
                    .or_else(|| c.get(3))
                    .or_else(|| c.get(5))
                    .unwrap()
                    .as_str();
                let format = c.get(2).or_else(|| c.get(4)).map(|m| m.as_str());
                // Built-in macros are expanded separately.
                if name.starts_with("__") {
                    return c[0].to_owned();
                }
                let Some(entry) = self.entries.iter().find(|e| e.definition.name == name) else {
                    error = Some(format!("unknown variable ${name}"));
                    return c[0].to_owned();
                };
                let scoped_value = scoped
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| vec![v.clone()]);
                let selected = scoped_value.as_ref().unwrap_or(&entry.selected);
                let all = selected.iter().any(|v| v == "All" || v == "$__all");
                if all
                    && let Some(value) = entry
                        .definition
                        .all_value
                        .as_ref()
                        .filter(|v| !v.is_empty())
                {
                    return value.clone();
                }
                let values: Vec<_> = if all {
                    entry
                        .options
                        .iter()
                        .filter(|v| *v != "All")
                        .cloned()
                        .collect()
                } else {
                    selected.clone()
                };
                let string_escape = |s: &str| {
                    s.replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('\n', "\\n")
                };
                match format {
                    Some("raw") => values.join(","),
                    Some("csv") => values.join(","),
                    Some("pipe") => values.join("|"),
                    Some("regex") => {
                        let escaped: Vec<_> = values
                            .iter()
                            .map(|v| string_escape(&regex::escape(v)))
                            .collect();
                        format!("({})", escaped.join("|"))
                    }
                    None if entry.definition.multi || all => {
                        let escaped: Vec<_> = values
                            .iter()
                            .map(|v| string_escape(&regex::escape(v)))
                            .collect();
                        format!("({})", escaped.join("|"))
                    }
                    None => values.first().map(|v| string_escape(v)).unwrap_or_default(),
                    Some(f) => {
                        error = Some(format!("unsupported variable formatter {f}"));
                        c[0].to_owned()
                    }
                }
            })
            .into_owned();
        error.map_or(Ok(output), Err)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueryKind {
    Instant { time: f64 },
    Range { start: f64, end: f64, step: f64 },
}

#[derive(Clone, Debug)]
pub struct Request {
    query: String,
    ref_id: String,
    legend: Option<String>,
    kind: QueryKind,
}

impl Request {
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn ref_id(&self) -> &str {
        &self.ref_id
    }
    pub fn legend(&self) -> Option<&str> {
        self.legend.as_deref()
    }
    pub fn kind(&self) -> &QueryKind {
        &self.kind
    }
    pub fn path(&self) -> &'static str {
        match self.kind {
            QueryKind::Instant { .. } => "query",
            QueryKind::Range { .. } => "query_range",
        }
    }
    pub fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            ("query".into(), self.query.clone()),
            ("timeout".into(), "15s".into()),
        ];
        match self.kind {
            QueryKind::Instant { time } => params.push(("time".into(), time.to_string())),
            QueryKind::Range { start, end, step } => params.extend([
                ("start".into(), start.to_string()),
                ("end".into(), end.to_string()),
                ("step".into(), step.to_string()),
            ]),
        }
        params
    }
}

pub fn duration(text: &str) -> Result<f64> {
    let text = text.trim().trim_start_matches('>');
    let seconds = text
        .parse::<f64>()
        .ok()
        .or_else(|| parse_duration(text).ok().map(|v| v.as_secs_f64()))
        .ok_or_else(|| format!("unsupported duration {text:?}"))?;
    if seconds > 0. && seconds.is_finite() {
        Ok(seconds)
    } else {
        Err(format!("duration must be positive: {text:?}"))
    }
}

pub fn check_datasource(value: &Value) -> Result<()> {
    if let Some(kind) = value.get("type").and_then(Value::as_str)
        && kind != "prometheus"
    {
        return Err(format!(
            "unsupported datasource type {kind}; this connection is Prometheus"
        ));
    }
    if value
        .as_str()
        .is_some_and(|s| matches!(s, "-- Mixed --" | "__expr__" | "-- Grafana --"))
    {
        return Err("mixed and expression datasources are unsupported".into());
    }
    Ok(())
}

fn optional_duration(
    value: Option<&str>,
    variables: &Variables,
    scoped: &[(String, String)],
) -> Result<Option<f64>> {
    value
        .filter(|s| !s.is_empty())
        .map(|s| variables.interpolate(s, scoped).and_then(|s| duration(&s)))
        .transpose()
}

pub fn panel_window(
    panel: &PanelSpec,
    context: &QueryContext,
    variables: &Variables,
) -> Result<TimeWindow> {
    let mut window = context.window;
    if let Some(span) =
        optional_duration(panel.request.time_from.as_deref(), variables, &panel.scoped)?
    {
        window.span = span.ceil() as u64;
    }
    if let Some(shift) = optional_duration(
        panel.request.time_shift.as_deref(),
        variables,
        &panel.scoped,
    )? {
        window.end -= shift.ceil() as i64;
    }
    Ok(window)
}

/// Construct requests without parsing PromQL itself. The server remains the
/// authority for expression syntax, including newer/experimental functions.
pub fn requests(
    panel: &PanelSpec,
    context: &QueryContext,
    variables: &Variables,
    scrape_interval: f64,
) -> Result<Vec<Request>> {
    check_datasource(&panel.request.datasource)?;
    let window = panel_window(panel, context, variables)?;
    let points = panel
        .request
        .max_data_points
        .unwrap_or(600)
        .clamp(2, 10_000);
    let interval = (window.span as f64 / points as f64).ceil().max(1.).max(
        optional_duration(panel.request.interval.as_deref(), variables, &panel.scoped)?
            .unwrap_or(1.),
    );
    let mut result = Vec::new();
    for query in &panel.queries {
        result.extend(
            target_requests(query, panel, window, variables, interval, scrape_interval)
                .map_err(|e| format!("{}: {e}", query.ref_id))?,
        );
    }
    Ok(result)
}

fn target_requests(
    query: &Query,
    panel: &PanelSpec,
    window: TimeWindow,
    variables: &Variables,
    interval: f64,
    scrape: f64,
) -> Result<Vec<Request>> {
    let settings = &query.request;
    check_datasource(&settings.datasource)?;
    if let Some(format) = settings.format.as_deref()
        && !matches!(format, "time_series" | "table" | "heatmap")
    {
        return Err(format!("unsupported Prometheus format {format}"));
    }
    let expression = settings
        .expr
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or("target has no PromQL expr (SQL, logs and Grafana expressions are unsupported)")?;
    let min_step = optional_duration(settings.interval.as_deref(), variables, &panel.scoped)?
        .unwrap_or(scrape);
    let factor = settings.interval_factor.unwrap_or(1.);
    if !factor.is_finite() || factor < 1. {
        return Err("intervalFactor must be at least 1".into());
    }
    let legacy_step = settings.step.unwrap_or(1.);
    if !legacy_step.is_finite() || legacy_step <= 0. {
        return Err("step must be positive".into());
    }
    let step = (interval * factor).max(min_step).max(legacy_step).ceil();
    let rate = (interval + min_step).max(4. * min_step).ceil();
    let expression = macros(
        &variables.interpolate(expression, &panel.scoped)?,
        interval,
        rate,
        window,
    )?;
    let legend = query
        .legend
        .as_deref()
        .map(|l| variables.interpolate(l, &panel.scoped))
        .transpose()?;
    let instant = settings.instant.unwrap_or(false);
    let range = settings.range.unwrap_or(!instant);
    let mut result = Vec::new();
    if range {
        let end = (window.end as f64 / step).floor() * step;
        let start = ((window.end as f64 - window.span as f64) / step).floor() * step;
        result.push(Request {
            query: expression.clone(),
            ref_id: query.ref_id.clone(),
            legend: legend.clone(),
            kind: QueryKind::Range { start, end, step },
        });
    }
    if instant {
        result.push(Request {
            query: expression,
            ref_id: query.ref_id.clone(),
            legend,
            kind: QueryKind::Instant {
                time: window.end as f64,
            },
        });
    }
    if result.is_empty() {
        return Err("target disables both instant and range requests".into());
    }
    Ok(result)
}

pub fn macros(text: &str, interval: f64, rate: f64, window: TimeWindow) -> Result<String> {
    let mut output = text.to_owned();
    for (name, value) in [
        ("__rate_interval_ms", (rate * 1000.).round().to_string()),
        ("__rate_interval", format!("{}s", rate.ceil())),
        ("__interval_ms", (interval * 1000.).round().to_string()),
        ("__interval", format!("{}s", interval.ceil())),
        ("__range_ms", (window.span * 1000).to_string()),
        ("__range_s", window.span.to_string()),
        ("__range", format!("{}s", window.span)),
        (
            "__from",
            ((window.end - window.span as i64) * 1000).to_string(),
        ),
        ("__to", (window.end * 1000).to_string()),
    ] {
        output = output
            .replace(&format!("${{{name}}}"), &value)
            .replace(&format!("${name}"), &value);
    }
    if Regex::new(r"\$\{?__\w+").unwrap().is_match(&output) {
        return Err("unsupported Grafana macro or macro formatter".into());
    }
    Ok(output)
}

#[derive(Debug, PartialEq)]
pub enum VariableQuery {
    LabelValues {
        selector: Option<String>,
        label: String,
    },
    LabelNames,
    QueryResult(String),
    Metrics(String),
}

pub fn variable_query(text: &str) -> Result<VariableQuery> {
    let text = text.trim();
    if text == "label_names()" {
        return Ok(VariableQuery::LabelNames);
    }
    if let Some(inner) = text
        .strip_prefix("label_values(")
        .and_then(|s| s.strip_suffix(')'))
    {
        // Split the last comma; commas inside a vector selector belong to its matchers.
        let (selector, label) = match inner.rsplit_once(',') {
            Some((s, l)) => (Some(s.trim().to_owned()), l.trim()),
            None => (None, inner.trim()),
        };
        if !Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_]*$")
            .unwrap()
            .is_match(label)
        {
            return Err("invalid label_values label".into());
        }
        if let Some(s) = &selector {
            let expression = parser::parse(s).map_err(|e| format!("label_values selector: {e}"))?;
            match expression {
                parser::Expr::VectorSelector(v) if v.offset.is_none() && v.at.is_none() => {},
                _ => return Err("label_values requires a plain vector selector; use query_result for PromQL expressions".into()),
            }
        }
        return Ok(VariableQuery::LabelValues {
            selector,
            label: label.into(),
        });
    }
    if let Some(inner) = text
        .strip_prefix("query_result(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Ok(VariableQuery::QueryResult(inner.into()));
    }
    if let Some(inner) = text
        .strip_prefix("metrics(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Ok(VariableQuery::Metrics(inner.into()));
    }
    Err(format!(
        "unsupported variable query {text:?}; supported: label_values, label_names, metrics, query_result"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafaui_model::Dashboard;

    fn dashboard(target: &str) -> Dashboard {
        Dashboard::parse(&format!(
            r#"{{"panels":[{{"type":"timeseries","targets":[{target}]}}]}}"#
        ))
        .unwrap()
    }
    pub(crate) fn context() -> QueryContext {
        QueryContext {
            window: TimeWindow::new(1000, 900, 90),
            variables: Vec::new(),
        }
    }

    #[test]
    fn instant_range_both_hidden_and_default_targets() {
        let dashboard = Dashboard::parse(
            r#"{"panels":[{"type":"timeseries","targets":[
            {"expr":"up","refId":"A","instant":true,"range":false},
            {"expr":"up","refId":"B","instant":false,"range":true},
            {"expr":"up","refId":"C","instant":true,"range":true},
            {"expr":"up","refId":"hidden","hide":true},
            {"expr":"up","refId":"E"}]}]}"#,
        )
        .unwrap();
        let requests =
            requests(dashboard.panel(0), &context(), &Variables::default(), 15.).unwrap();
        assert_eq!(
            requests.iter().map(Request::ref_id).collect::<Vec<_>>(),
            ["A", "B", "C", "C", "E"]
        );
        assert_eq!(requests[0].kind(), &QueryKind::Instant { time: 1000. });
        assert_eq!(
            requests[1].kind(),
            &QueryKind::Range {
                start: 90.,
                end: 990.,
                step: 15.
            }
        );
        assert_eq!(requests[0].path(), "query");
        assert_eq!(requests[1].path(), "query_range");
        assert!(requests[1].params().contains(&("step".into(), "15".into())));
    }

    #[test]
    fn minimum_step_point_limit_factor_panel_time_and_macros() {
        let dashboard = Dashboard::parse(r#"{"panels":[{"type":"timeseries","maxDataPoints":10,"interval":"20s","timeFrom":"5m","timeShift":"1h",
            "targets":[{"expr":"rate(up[$__rate_interval]) + $__interval_ms + $__range_s + $__from + $__to","refId":"R","interval":"30s","intervalFactor":2,"step":45}]}]}"#).unwrap();
        let requests =
            requests(dashboard.panel(0), &context(), &Variables::default(), 15.).unwrap();
        assert_eq!(
            requests[0].kind(),
            &QueryKind::Range {
                start: -2940.,
                end: -2640.,
                step: 60.
            }
        );
        assert_eq!(
            requests[0].query(),
            "rate(up[120s]) + 30000 + 300 + -2900000 + -2600000"
        );
        assert_eq!(
            macros(
                "$__interval ${__rate_interval} $__range $__range_ms",
                30.,
                120.,
                TimeWindow::new(1000, 300, 90)
            )
            .unwrap(),
            "30s 120s 300s 300000"
        );
        assert!(macros("$__unknown", 1., 60., context().window).is_err());
        assert_eq!(duration("1h30m").unwrap(), 5400.);
    }

    #[test]
    fn rejects_unsupported_and_disabled_targets_without_fallback() {
        for target in [
            r#"{"rawSql":"select 1"}"#,
            r#"{"expr":"up","instant":false,"range":false}"#,
            r#"{"expr":"up","datasource":{"type":"loki"}}"#,
            r#"{"expr":"up","intervalFactor":0}"#,
            r#"{"expr":"up","format":"logs"}"#,
        ] {
            assert!(
                requests(
                    dashboard(target).panel(0),
                    &context(),
                    &Variables::default(),
                    15.
                )
                .is_err(),
                "{target}"
            );
        }
    }

    #[test]
    fn interpolation_escapes_regex_all_multi_and_scoped_values() {
        let dashboard = Dashboard::parse(
            r#"{"templating":{"list":[
            {"name":"node","type":"query","multi":true,"includeAll":true},
            {"name":"node_name","type":"constant","query":"a\"b"},
            {"name":"any","type":"query","includeAll":true,"allValue":".*"}]}}"#,
        )
        .unwrap();
        let mut variables = Variables::new(
            &dashboard.variables,
            &["All".into(), "a\"b".into(), "All".into()],
        );
        variables
            .resolve(0, vec!["host.a".into(), "host+b".into()])
            .unwrap();
        variables.resolve(1, vec!["a\"b".into()]).unwrap();
        variables.resolve(2, vec!["one".into()]).unwrap();
        assert_eq!(
            variables
                .interpolate(
                    r#"x{instance=~"$node",name="$node_name",any=~"${any}"}"#,
                    &[]
                )
                .unwrap(),
            r#"x{instance=~"(host\\.a|host\\+b)",name="a\"b",any=~".*"}"#
        );
        assert_eq!(
            variables
                .interpolate("${node:raw} [[node:csv]]", &[])
                .unwrap(),
            "host.a,host+b host.a,host+b"
        );
        assert_eq!(
            variables
                .interpolate("$node", &[("node".into(), "host.a".into())])
                .unwrap(),
            r"(host\\.a)"
        );
        assert!(variables.interpolate("$missing", &[]).is_err());
        assert!(variables.interpolate("${node:json}", &[]).is_err());
    }

    #[test]
    fn selector_validation_uses_promql_parser_and_preserves_matcher_commas() {
        assert_eq!(
            variable_query(
                r#"label_values(node_uname_info{job="nodes",instance=~"a,b"}, nodename)"#
            )
            .unwrap(),
            VariableQuery::LabelValues {
                selector: Some(r#"node_uname_info{job="nodes",instance=~"a,b"}"#.into()),
                label: "nodename".into()
            }
        );
        assert!(matches!(
            variable_query("label_values(job)").unwrap(),
            VariableQuery::LabelValues { selector: None, .. }
        ));
        assert!(variable_query("label_values(sum(up), job)").is_err());
        assert!(variable_query("label_values(up offset 1h, job)").is_err());
        assert!(variable_query("label_values(up{bad}, job)").is_err());
        assert_eq!(
            variable_query("query_result(sum(rate(x[5m])))").unwrap(),
            VariableQuery::QueryResult("sum(rate(x[5m]))".into())
        );
    }

    #[test]
    fn variable_regex_sort_and_selection_recovery() {
        let dashboard = Dashboard::parse(r#"{"templating":{"list":[{"name":"node","type":"query","regex":"/host-(?<value>[0-9]+)/","sort":3}]}}"#).unwrap();
        let mut variables = Variables::new(&dashboard.variables, &["gone".into()]);
        variables
            .resolve(
                0,
                vec![
                    "host-10".into(),
                    "host-2".into(),
                    "other".into(),
                    "host-2".into(),
                ],
            )
            .unwrap();
        assert_eq!(variables.options(0), ["2", "10"]);
        assert_eq!(variables.selection(0), "2");
        assert!(variables.resolve(0, vec![]).is_err());
    }
}
