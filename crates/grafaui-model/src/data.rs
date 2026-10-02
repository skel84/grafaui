//! Query results. [`DataSource`] is the seam a real source (Prometheus)
//! will plug into later; [`FakeSource`] makes up plausible series.
//!
//! Fake series are deterministic: the same dashboard, panel, query, time
//! range and variable values always give the same numbers.

use crate::spec::{FieldSpec, PanelSpec, Viz};
use crate::sql;
use crate::time::TimeWindow;

#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// Unix seconds, including fractional seconds, shared by every series.
    /// Missing values at a timestamp are represented by NaN, never zero.
    pub times: Vec<f64>,
    pub series: Vec<Series>,
}

#[derive(Clone, Debug)]
pub struct Series {
    pub name: String,
    /// The `refId` of the query that returned it.
    pub query: String,
    /// The value column it fills when a SQL query returns several.
    pub field: Option<String>,
    pub labels: Vec<(String, String)>,
    pub values: Vec<f64>,
}

/// What a query is evaluated against.
#[derive(Clone, Debug)]
pub struct QueryContext {
    pub window: TimeWindow,
    /// Each template variable's current value.
    pub variables: Vec<(String, String)>,
}

impl QueryContext {
    /// This context with the panel's own variables (a repeated copy's
    /// value) in front.
    pub fn for_panel(&self, panel: &PanelSpec) -> std::borrow::Cow<'_, QueryContext> {
        if panel.scoped.is_empty() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut context = self.clone();
        for (name, value) in &panel.scoped {
            match context.variables.iter_mut().find(|(n, _)| n == name) {
                Some(slot) => slot.1.clone_from(value),
                None => context.variables.push((name.clone(), value.clone())),
            }
        }
        std::borrow::Cow::Owned(context)
    }

    /// Replaces `$name`, `${name}` and `[[name]]` with variable values.
    pub fn interpolate(&self, text: &str) -> String {
        let mut out = text.to_owned();
        // Longest names first, so `$node` doesn't eat `$node_name`.
        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        for (name, value) in variables {
            for pattern in [
                format!("${{{name}}}"),
                format!("[[{name}]]"),
                format!("${name}"),
            ] {
                out = out.replace(&pattern, value);
            }
        }
        out
    }
}

pub trait DataSource {
    fn query(&self, panel: &PanelSpec, context: &QueryContext) -> Frame;
}

/// Makes up series shaped by each panel's unit and queries.
#[derive(Clone, Debug, Default)]
pub struct FakeSource {
    /// Mixed into every series, so two dashboards with equal panel ids
    /// differ. The dashboard uid is a good choice.
    pub seed: u64,
}

impl FakeSource {
    pub fn new(seed: &str) -> Self {
        Self { seed: hash(seed) }
    }
}

impl DataSource for FakeSource {
    fn query(&self, panel: &PanelSpec, context: &QueryContext) -> Frame {
        let context = &*context.for_panel(panel);
        let window = &context.window;
        let times = window
            .times()
            .into_iter()
            .map(|t| t as f64)
            .collect::<Vec<_>>();
        let vars: String = context
            .variables
            .iter()
            .map(|(name, value)| format!("{name}={value};"))
            .collect();
        let panel_seed = self.seed ^ hash(&format!("{}|{}|{vars}", panel.key, panel.title));
        if matches!(panel.viz, Viz::Geomap(_)) {
            return fake_geomap(panel, times, panel_seed);
        }
        if let Viz::Heatmap(options) = &panel.viz {
            return fake_heatmap(panel, options, times, panel_seed);
        }
        let mut series = Vec::new();
        for (index, query) in panel.queries.iter().enumerate() {
            let legend = query.legend.as_deref().map(|l| context.interpolate(l));
            let text = query.text.as_deref().map(|t| context.interpolate(t));
            let seed = panel_seed ^ hash(&query.ref_id).rotate_left(index as u32);
            let sql = text
                .as_deref()
                .filter(|t| legend.is_none() && sql::is_sql(t))
                .and_then(sql::shape);
            let named = match &sql {
                Some(sql) => sql_names(sql, panel, seed),
                None => names(
                    legend.as_deref(),
                    text.as_deref(),
                    &query.ref_id,
                    panel,
                    seed,
                )
                .into_iter()
                .map(|(labels, name)| (labels, name, None))
                .collect(),
            };
            for (labels, name, field) in named {
                let n = series.len();
                let context = crate::spec::FieldContext::new(&name).query(&query.ref_id);
                let config = match &panel.viz {
                    Viz::TimeSeries(options) => {
                        panel.field.for_time_series_field(&context, options)
                    }
                    Viz::Table => {
                        let joined = panel.queries.len() > 1
                            && panel
                                .transforms
                                .iter()
                                .any(|t| matches!(t, crate::transform::Transform::Join));
                        let source = crate::transform::value_column(
                            &panel.transforms,
                            field.as_deref(),
                            &query.ref_id,
                            panel.queries.len(),
                            joined,
                        );
                        let shown = crate::transform::rename(&panel.transforms, &source);
                        panel
                            .field
                            .for_field(
                                &crate::spec::FieldContext::new(&shown)
                                    .source(&source)
                                    .query(&query.ref_id),
                            )
                            .into_owned()
                    }
                    _ => panel
                        .field
                        .for_field(
                            &crate::spec::FieldContext::new(field.as_deref().unwrap_or(&name))
                                .query(&query.ref_id),
                        )
                        .into_owned(),
                };
                let shape = Shape::for_field(panel, &config, text.as_deref());
                let mut values = shape.generate(seed ^ hash(&name), n, times.len());
                if shape.epoch {
                    let end = times.last().copied().unwrap_or_default();
                    values.iter_mut().for_each(|v| *v = (end - *v) * 1e3);
                }
                series.push(Series {
                    name,
                    query: query.ref_id.clone(),
                    field,
                    labels,
                    values,
                });
            }
        }
        if series.is_empty() {
            // Panels without queries (or with only hidden ones) still show
            // something in the mock.
            let name = if panel.title.is_empty() {
                "Series A".to_owned()
            } else {
                context.interpolate(&panel.title)
            };
            let count = match panel.viz {
                Viz::Pie(_) | Viz::BarGauge(_) | Viz::BarChart(_) => 4,
                _ => 1,
            };
            for n in 0..count {
                let name = if count == 1 {
                    name.clone()
                } else {
                    format!("{name} {}", n + 1)
                };
                let values =
                    Shape::for_query(panel, None).generate(panel_seed ^ n as u64, n, times.len());
                series.push(Series {
                    name,
                    query: "A".into(),
                    field: None,
                    labels: Vec::new(),
                    values,
                });
            }
        }
        Frame { times, series }
    }
}

/// Geographic queries return named coordinate columns and real city locations.
fn fake_geomap(panel: &PanelSpec, times: Vec<f64>, seed: u64) -> Frame {
    const CITIES: &[(&str, &str, f64, f64)] = &[
        ("北京", "北京", 39.904, 116.407),
        ("上海", "上海", 31.230, 121.474),
        ("广州", "广东", 23.129, 113.264),
        ("深圳", "广东", 22.543, 114.058),
        ("成都", "四川", 30.572, 104.067),
        ("重庆", "重庆", 29.564, 106.551),
        ("武汉", "湖北", 30.592, 114.305),
        ("西安", "陕西", 34.342, 108.940),
        ("杭州", "浙江", 30.274, 120.155),
        ("南京", "江苏", 32.060, 118.797),
        ("郑州", "河南", 34.747, 113.625),
        ("济南", "山东", 36.651, 117.120),
        ("青岛", "山东", 36.067, 120.383),
        ("天津", "天津", 39.084, 117.201),
        ("长沙", "湖南", 28.228, 112.939),
        ("昆明", "云南", 25.038, 102.718),
        ("哈尔滨", "黑龙江", 45.804, 126.535),
        ("沈阳", "辽宁", 41.806, 123.432),
    ];
    const WORLD_CITIES: &[(&str, &str, f64, f64)] = &[
        ("New York", "US", 40.713, -74.006),
        ("Chicago", "US", 41.878, -87.630),
        ("Denver", "US", 39.739, -104.990),
        ("Seattle", "US", 47.606, -122.332),
        ("San Francisco", "US", 37.775, -122.419),
        ("Houston", "US", 29.760, -95.370),
        ("London", "UK", 51.507, -0.128),
        ("Paris", "France", 48.857, 2.352),
        ("Rome", "Italy", 41.903, 12.496),
        ("Berlin", "Germany", 52.520, 13.405),
        ("Tokyo", "Japan", 35.676, 139.650),
        ("Singapore", "Singapore", 1.352, 103.820),
        ("Beijing", "China", 39.904, 116.407),
        ("Mumbai", "India", 19.076, 72.878),
        ("Sydney", "Australia", -33.869, 151.209),
        ("Nairobi", "Kenya", -1.292, 36.822),
        ("Cape Town", "South Africa", -33.925, 18.424),
        ("São Paulo", "Brazil", -23.551, -46.633),
    ];
    let Viz::Geomap(options) = &panel.viz else {
        unreachable!()
    };
    let chinese = panel
        .title
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    let cities = if chinese { CITIES } else { WORLD_CITIES };
    let mut rng = SplitMix(seed);
    let mut series = Vec::new();
    let fallback = crate::spec::Query {
        ref_id: "A".into(),
        text: None,
        legend: None,
        request: Default::default(),
    };
    let queries = if panel.queries.is_empty() {
        std::slice::from_ref(&fallback)
    } else {
        &panel.queries
    };
    for query in queries {
        let mut coordinates = vec![
            ("latitude".to_owned(), true),
            ("longitude".to_owned(), false),
        ];
        for layer in &options.layers {
            for (name, latitude) in [
                (layer.latitude.as_ref(), true),
                (layer.longitude.as_ref(), false),
            ] {
                if let Some(name) = name
                    && !coordinates.iter().any(|(n, _)| n == name)
                {
                    coordinates.push((name.clone(), latitude));
                }
            }
        }
        let shape = query.text.as_deref().and_then(sql::shape);
        let mut value_fields: Vec<String> = shape
            .as_ref()
            .map(|s| s.values().map(str::to_owned).collect())
            .unwrap_or_default();
        for layer in &options.layers {
            for name in [&layer.color_field, &layer.size_field]
                .into_iter()
                .flatten()
            {
                if !value_fields.contains(name) {
                    value_fields.push(name.clone());
                }
            }
        }
        value_fields.retain(|name| !coordinates.iter().any(|(n, _)| n == name));
        if value_fields.is_empty() {
            value_fields.push("Value".into());
        }
        for &(city, region, lat, lon) in cities {
            let labels = if chinese {
                vec![("城市".into(), city.into()), ("省份".into(), region.into())]
            } else {
                vec![
                    ("City".into(), city.into()),
                    ("Region".into(), region.into()),
                ]
            };
            for (name, value) in coordinates
                .iter()
                .map(|(name, latitude)| (name.as_str(), if *latitude { lat } else { lon }))
                .chain(value_fields.iter().map(|name| {
                    let config = panel
                        .field
                        .for_field(&crate::spec::FieldContext::new(name).query(&query.ref_id));
                    let max = config.max.unwrap_or(1000.);
                    let min = config.min.unwrap_or(0.);
                    let value = min + (max - min) * (0.03 + rng.unit() * 0.97);
                    (
                        name.as_str(),
                        if config.unit.is_none() && max >= 10. {
                            value.round()
                        } else {
                            value
                        },
                    )
                }))
            {
                series.push(Series {
                    name: format!("{city} {name}"),
                    query: query.ref_id.clone(),
                    field: Some(name.into()),
                    labels: labels.clone(),
                    values: vec![value; times.len()],
                });
            }
        }
    }
    Frame { times, series }
}

/// Heatmaps receive observations or bucket counts, never metric-valued
/// series masquerading as cell intensity. Prometheus rows are cumulative.
fn fake_heatmap(
    panel: &PanelSpec,
    options: &crate::heatmap::Options,
    times: Vec<f64>,
    seed: u64,
) -> Frame {
    use crate::heatmap::Source;
    let mut rng = SplitMix(seed);
    let scale = match options.unit.as_deref() {
        Some("bytes" | "decbytes") => 2_000_000.,
        Some("s") => 0.25,
        Some("ms") => 250.,
        _ => 20.,
    };
    let query = panel
        .queries
        .first()
        .map(|q| q.ref_id.clone())
        .unwrap_or_else(|| "A".into());
    let mut series = Vec::new();
    if options.source == Source::Samples {
        for n in 0..48 {
            let values = times
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let phase = i as f64 / 13. + n as f64 / 7.;
                    scale
                        * ((phase.sin() * 0.45 + (i as f64 / 29.).cos() * 0.25)
                            + (rng.unit() * 2. - 1.) * 0.75)
                            .exp()
                })
                .collect();
            series.push(Series {
                name: format!("Observation {}", n + 1),
                query: query.clone(),
                field: None,
                labels: Vec::new(),
                values,
            });
        }
    } else if panel
        .queries
        .iter()
        .any(|q| q.text.as_deref().is_some_and(|q| q.contains("_bucket")))
    {
        let rows = 15;
        let mut cumulative = vec![0.; times.len()];
        for n in 0..rows {
            let upper = if n + 1 == rows {
                f64::INFINITY
            } else {
                scale * 2_f64.powi(n - 7)
            };
            let values = times
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let center = 7. + (i as f64 / 14.).sin() * 1.8;
                    let mass = (-((n as f64 - center) / 2.).powi(2) / 2.).exp();
                    let count =
                        (mass * (30. + (i as f64 / 9.).cos() * 12.) * (0.7 + rng.unit() * 0.6))
                            .round();
                    cumulative[i] += count;
                    cumulative[i]
                })
                .collect();
            let bound = if upper.is_infinite() {
                "+Inf".into()
            } else {
                upper.to_string()
            };
            series.push(Series {
                name: bound.clone(),
                query: query.clone(),
                field: None,
                labels: vec![("le".into(), bound)],
                values,
            });
        }
    } else {
        // SQL CASE expressions in the fixtures return ordered categories.
        let category = regex::Regex::new(r"(?i)(?:THEN|ELSE)\s*'([^']+)'").unwrap();
        let mut labels: Vec<String> = panel
            .queries
            .iter()
            .filter_map(|q| q.text.as_deref())
            .flat_map(|q| {
                category
                    .captures_iter(q)
                    .map(|c| c[1].to_owned())
                    .collect::<Vec<_>>()
            })
            .collect();
        if labels.is_empty() {
            labels = vec![
                "0–10".into(),
                "10–25".into(),
                "25–50".into(),
                "50–100".into(),
                "100–250".into(),
                "250+".into(),
            ];
        }
        for (n, label) in labels.iter().enumerate() {
            let values = times
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let center = labels.len() as f64 * (0.4 + (i as f64 / 12.).sin() * 0.15);
                    ((-((n as f64 - center) / 1.5).powi(2) / 2.).exp() * (30. + rng.unit() * 50.))
                        .round()
                })
                .collect();
            series.push(Series {
                name: format!("count {label}"),
                query: query.clone(),
                field: None,
                labels: vec![("bucket".into(), label.clone())],
                values,
            });
        }
    }
    Frame { times, series }
}

/// The series a SQL query returns: for each label set, a series per value
/// column, named by the labels (and the column, when there are several).
fn sql_names(
    sql: &sql::Shape,
    panel: &PanelSpec,
    seed: u64,
) -> Vec<(Vec<(String, String)>, String, Option<String>)> {
    let keys: Vec<&str> = sql.labels().collect();
    let constant = |key: &str| {
        sql.columns
            .iter()
            .find(|c| c.name == key)
            .and_then(|c| c.constant.clone())
    };
    let values: Vec<&str> = sql.values().collect();
    let values = if values.is_empty() {
        vec!["value"]
    } else {
        values
    };
    let count = if keys.iter().all(|k| constant(k).is_some()) {
        1
    } else {
        let base = match panel.viz {
            Viz::Table => 5,
            Viz::Pie(_) | Viz::BarGauge(_) | Viz::BarChart(_) => 4,
            _ => 2,
        };
        base + (seed % 3) as usize
    };
    let mut out = Vec::new();
    for i in 0..count {
        let labels: Vec<(String, String)> = keys
            .iter()
            .map(|key| {
                (
                    (*key).to_owned(),
                    constant(key).unwrap_or_else(|| label_value(key, i, seed)),
                )
            })
            .collect();
        let label_text = labels
            .iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for value in &values {
            let name = match (labels.is_empty(), values.len()) {
                (true, _) => (*value).to_owned(),
                (false, 1) => label_text.clone(),
                (false, _) => format!("{label_text} {value}"),
            };
            out.push((labels.clone(), name, Some((*value).to_owned())));
        }
    }
    out
}

/// The series a query returns: label sets and display names.
fn names(
    legend: Option<&str>,
    text: Option<&str>,
    ref_id: &str,
    panel: &PanelSpec,
    seed: u64,
) -> Vec<(Vec<(String, String)>, String)> {
    // Instant table queries expose the metadata consumed by groupBy even
    // when the dashboard leaves its legend empty or asks Grafana for __auto.
    let grouped: Vec<_> = panel
        .transforms
        .iter()
        .filter_map(|t| match t {
            crate::transform::Transform::GroupBy { fields } => Some(fields),
            _ => None,
        })
        .flatten()
        .collect();
    let legend = legend.filter(|l| grouped.is_empty() || (!l.trim().is_empty() && *l != "__auto"));
    let mut keys = legend.map(placeholders).unwrap_or_default();
    if keys.is_empty() && legend.is_none() {
        keys = text.map(grouping).unwrap_or_default();
    }
    if keys.is_empty() && legend.is_none() {
        keys = text.map(regex_selected).unwrap_or_default();
    }
    for field in &grouped {
        if field.name != "Value"
            && !field.name.starts_with("Value #")
            && field.name != "Time"
            && !keys.contains(&field.name)
        {
            keys.push(field.name.clone());
        }
    }
    if keys.is_empty() {
        let name = legend
            .map(str::to_owned)
            .or_else(|| text.and_then(metric_name))
            .unwrap_or_else(|| format!("Series {ref_id}"));
        return vec![(Vec::new(), name)];
    }
    // A legend of just one label, with overrides for series by name: the
    // overrides' names are the label's values, so colors and the like show.
    // Table overrides name columns, not the label values of their rows.
    let named = if matches!(panel.viz, Viz::Table) {
        Vec::new()
    } else {
        crate::overrides::names(&panel.field.overrides)
    };
    if keys.len() == 1
        && !named.is_empty()
        && legend.is_some_and(|l| l.trim().starts_with("{{") && l.trim().ends_with("}}"))
    {
        return named
            .into_iter()
            .map(|name| (vec![(keys[0].clone(), name.clone())], name))
            .collect();
    }
    let base = match panel.viz {
        Viz::Pie(_) | Viz::BarGauge(_) => 4,
        _ => 2,
    };
    let count = if grouped.is_empty() {
        base + (seed % 3) as usize
    } else {
        6
    };
    (0..count)
        .map(|i| {
            let labels: Vec<_> = keys
                .iter()
                .map(|key| {
                    let (row, label_seed) = if grouped.is_empty() {
                        (i, seed)
                    } else if grouped.iter().any(|f| f.group && f.name == *key) {
                        (i / 2, 0)
                    } else {
                        (i, 0)
                    };
                    (key.clone(), label_value(key, row, label_seed))
                })
                .collect();
            let name = match legend {
                Some(legend) => fill_legend(legend, &labels),
                None => format!(
                    "{{{}}}",
                    labels
                        .iter()
                        .map(|(k, v)| format!("{k}=\"{v}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            (labels, name)
        })
        .collect()
}

/// `legend` with each `{{label}}` (spaces allowed inside) replaced by its
/// value.
fn fill_legend(legend: &str, labels: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut rest = legend;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start..].find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        let key = rest[start + 2..start + end].trim();
        match labels.iter().find(|(k, _)| k == key) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

/// Label names in `{{label}}` placeholders.
fn placeholders(legend: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = legend;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start..].find("}}") else {
            break;
        };
        let key = rest[start + 2..start + end].trim().to_owned();
        if !key.is_empty() && !keys.contains(&key) {
            keys.push(key);
        }
        rest = &rest[start + end + 2..];
    }
    keys
}

/// Labels in the PromQL `by (a, b)` clause that shapes the result. A
/// grouping inside another aggregation, as in `count(count(x) by (cpu))`,
/// is folded away by it and doesn't count.
fn grouping(query: &str) -> Vec<String> {
    /// Calls that fold their argument's series into one.
    const FOLDING: &[&str] = &[
        "sum",
        "avg",
        "count",
        "min",
        "max",
        "group",
        "stddev",
        "stdvar",
        "quantile",
        "scalar",
        "absent",
        "count_values",
    ];
    // The function name of each open parenthesis.
    let mut calls: Vec<&str> = Vec::new();
    let mut quote = None;
    for (at, c) in query.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '(') => {
                let before = query[..at].trim_end();
                let name_at = before
                    .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                    .map_or(0, |i| i + 1);
                let name = &before[name_at..];
                if name == "by" && !calls.iter().any(|call| FOLDING.contains(call)) {
                    let Some(close) = query[at..].find(')') else {
                        break;
                    };
                    return query[at + 1..at + close]
                        .split(',')
                        .map(|label| label.trim().to_owned())
                        .filter(|label| !label.is_empty())
                        .collect();
                }
                calls.push(name);
            }
            (None, ')') => {
                calls.pop();
            }
            _ => {}
        }
    }
    Vec::new()
}

/// Labels a query without aggregation selects by regex, as in
/// `probe_success{instance=~"$target"}`: it returns a series per value.
fn regex_selected(query: &str) -> Vec<String> {
    const FOLDING: &[&str] = &[
        "sum", "avg", "count", "min", "max", "group", "topk", "bottomk", "quantile", "scalar",
    ];
    let folded = FOLDING.iter().any(|f| {
        query.match_indices(f).any(|(at, _)| {
            let before = query[..at].chars().next_back();
            let after = query[at + f.len()..].trim_start();
            before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                && (after.starts_with('(')
                    || after.starts_with("by")
                    || after.starts_with("without"))
        })
    });
    if folded {
        return Vec::new();
    }
    let mut keys = Vec::new();
    for (at, _) in query.match_indices("=~") {
        let before = query[..at].trim_end();
        let start = before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let key = &before[start..];
        if !key.is_empty() && !keys.iter().any(|k| k == key) {
            keys.push(key.to_owned());
        }
    }
    keys.truncate(1);
    keys
}

/// The first metric in a PromQL query, skipping functions and keywords.
fn metric_name(query: &str) -> Option<String> {
    const KEYWORDS: &[&str] = &[
        "by",
        "without",
        "on",
        "ignoring",
        "group_left",
        "group_right",
        "and",
        "or",
        "unless",
        "bool",
        "offset",
    ];
    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '{' || c == '[' || c == '"' {
            // Skip label matchers, ranges and strings.
            let close = match c {
                '{' => '}',
                '[' => ']',
                _ => '"',
            };
            i += query[i + 1..]
                .find(close)
                .map_or(bytes.len(), |end| end + 2);
            continue;
        }
        if c.is_ascii_digit() {
            // Numbers and durations such as `5m` are not metrics.
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' || c == ':' {
            let start = i;
            while i < bytes.len()
                && ((bytes[i] as char).is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b':'))
            {
                i += 1;
            }
            let word = &query[start..i];
            let is_call = query[i..].trim_start().starts_with('(');
            if !is_call && !KEYWORDS.contains(&word) && !word.starts_with('$') {
                return Some(word.to_owned());
            }
            continue;
        }
        i += 1;
    }
    None
}

fn label_value(key: &str, i: usize, seed: u64) -> String {
    // Rotated by the seed, unique within a pool, numbered past its end.
    let pick = |pool: &[&str]| {
        let value = pool[(i + seed as usize % pool.len()) % pool.len()];
        match i / pool.len() {
            0 => value.to_owned(),
            lap => format!("{value}-{}", lap + 1),
        }
    };
    let key = key.to_lowercase();
    match key.as_str() {
        "instance" | "target" => format!("10.0.1.{}:9100", 11 + i),
        "node" | "nodename" | "hostname" | "host" => format!("worker-{}", i + 1),
        "pod" => format!(
            "api-7f9c6d-{}",
            pick(&["x2k4p", "b7mzq", "lq9ts", "c3hdw", "p8nrv"])
        ),
        "namespace" => pick(&[
            "default",
            "kube-system",
            "monitoring",
            "ingress-nginx",
            "payments",
        ]),
        "job" => pick(&["node-exporter", "kubelet", "apiserver", "cadvisor"]),
        "mode" => pick(&["user", "system", "iowait", "irq", "softirq", "steal"]),
        "cpu" | "core" => i.to_string(),
        "device" => pick(&["nvme0n1", "sda", "eth0", "ens5", "dm-0"]),
        "mountpoint" => pick(&["/", "/var", "/home", "/boot", "/data", "/tmp"]),
        "container" => pick(&["app", "sidecar", "envoy", "init", "exporter"]),
        "code" | "status" | "status_code" => pick(&["200", "404", "500", "301", "503"]),
        "method" => pick(&["GET", "POST", "PUT", "DELETE"]),
        "quantile" => pick(&["0.5", "0.9", "0.99", "0.999"]),
        "service" | "app" | "application" => {
            pick(&["checkout", "frontend", "orders", "search", "auth"])
        }
        "level" | "severity" | "detected_level" => pick(&["info", "warn", "error", "debug"]),
        "health_status" | "health" => pick(&["Healthy", "Progressing", "Degraded", "Missing"]),
        "sync_status" => pick(&["Synced", "OutOfSync", "Unknown"]),
        "phase" => pick(&["Running", "Pending", "Succeeded", "Failed"]),
        "state" => pick(&["active", "idle", "waiting"]),
        "region" => pick(&["us-east-1", "eu-west-1", "ap-southeast-2"]),
        "zone" | "availability_zone" => pick(&["us-east-1a", "us-east-1b", "us-east-1c"]),
        "database" | "datname" | "db" => pick(&["app", "analytics", "postgres", "billing"]),
        "table" => pick(&["events", "users", "orders", "sessions"]),
        "queue" => pick(&["orders", "emails", "webhooks", "default"]),
        "topic" => pick(&["events", "audit", "clicks", "payments"]),
        "handler" | "path" | "route" | "uri" | "url" => pick(&[
            "/api/v1/orders",
            "/api/v1/users",
            "/healthz",
            "/login",
            "/metrics",
        ]),
        "le" | "bucket" => pick(&["0.1", "0.25", "0.5", "1", "+Inf"]),
        "version" => pick(&["v1.4.2", "v1.5.0", "v1.5.1"]),
        "type" | "kind" => pick(&["alpha", "beta", "gamma", "delta"]),
        "name" => pick(&["alpha", "bravo", "charlie", "delta", "echo"]),
        "cluster" => pick(&["prod-eu", "prod-us", "staging"]),
        "instanceid" => format!("i-0{:x}", 0x3fa9c2e1u32.wrapping_add(i as u32 * 7919)),
        "functionname" => pick(&[
            "checkout-handler",
            "thumbnailer",
            "auth-authorizer",
            "nightly-report",
        ]),
        "loadbalancer" | "targetgroup" => format!("app/prod-alb/{:x}", 0x9e3c71u32 + i as u32),
        "cacheclusterid" | "dbinstanceidentifier" => {
            pick(&["prod-primary", "prod-replica-1", "prod-replica-2"])
        }
        "loggroupname" => pick(&["/aws/lambda/checkout", "/aws/ecs/api", "/aws/rds/prod"]),
        "queuename" => pick(&["orders", "emails", "webhooks"]),
        "integration" | "receiver" => pick(&["slack", "pagerduty", "email", "webhook"]),
        "shard" | "shard_num" | "replica_num" => i.to_string(),
        "user" | "username" | "initial_user" => {
            pick(&["default", "analyst", "etl", "grafana", "admin"])
        }
        "ip_proto" | "proto" | "protocol" => pick(&["tcp", "udp", "icmp", "gre"]),
        "domain" | "server_name" | "vhost" => pick(&[
            "example.com",
            "api.example.com",
            "cdn.example.com",
            "shop.example.com",
        ]),
        "engine" => pick(&[
            "MergeTree",
            "ReplicatedMergeTree",
            "ReplacingMergeTree",
            "Distributed",
        ]),
        "query_kind" | "query_type" => pick(&["Select", "Insert", "Alter", "Create"]),
        "disk" | "disk_name" => pick(&["default", "s3", "cold", "hot"]),
        "src_ip" | "dst_ip" | "ip" | "client_ip" | "remote_addr" => {
            format!("10.0.{}.{}", 2 + i / 250, 10 + i % 250)
        }
        "country" | "country_code" => pick(&["US", "DE", "JP", "BR", "IN", "FR"]),
        "browser" | "user_agent" => pick(&["Chrome", "Firefox", "Safari", "Edge"]),
        // Named after something familiar.
        k if k.contains("database") || k.ends_with("_db") => {
            pick(&["app", "analytics", "postgres", "billing"])
        }
        k if k.contains("table") => pick(&["events", "users", "orders", "sessions", "metrics"]),
        k if k.contains("user") => pick(&["default", "analyst", "etl", "grafana", "admin"]),
        k if k.contains("host") || k.contains("server") => format!("ch-{}", i + 1),
        k if k.contains("status") || k.contains("state") => pick(&["ok", "warning", "error"]),
        k if k.contains("type") || k.contains("kind") => pick(&["alpha", "beta", "gamma", "delta"]),
        k if k.contains("name") => pick(&["alpha", "bravo", "charlie", "delta", "echo"]),
        _ => format!("{key}-{}", i + 1),
    }
}

/// The range and movement of a panel's values, picked from its unit and a
/// few hints in its title and queries.
#[derive(Clone, Copy, Debug)]
struct Shape {
    low: f64,
    high: f64,
    floor: f64,
    ceiling: f64,
    /// Counts: rounded to whole numbers.
    whole: bool,
    /// Timestamps: values are seconds before the range's end, sent as Unix
    /// milliseconds.
    epoch: bool,
}

impl Shape {
    /// The shape of what `query` returns in `panel`: hints come from the
    /// panel's title and that query.
    fn for_query(panel: &PanelSpec, query: Option<&str>) -> Self {
        Self::for_field(panel, &panel.field, query)
    }

    fn for_field(panel: &PanelSpec, field: &FieldSpec, query: Option<&str>) -> Self {
        let text = format!("{} {}", panel.title, query.unwrap_or_default()).to_lowercase();
        let unit = field.unit.as_deref().unwrap_or_default();
        if unit.starts_with("dateTime") {
            // Seconds before the end of the range; turned into Unix
            // milliseconds once the times are known.
            return Self {
                low: 600.,
                high: 3e5,
                floor: 0.,
                ceiling: f64::INFINITY,
                whole: true,
                epoch: true,
            };
        }
        let duration = matches!(unit, "s" | "dtdurations" | "dtdhms" | "clocks");
        if duration
            && (text.contains("uptime")
                || text.contains("boot_time")
                || text.contains("start_time"))
        {
            // Between two days and two months, in seconds.
            return Self {
                low: 1.7e5,
                high: 5.2e6,
                floor: 0.,
                ceiling: f64::INFINITY,
                whole: true,
                epoch: false,
            };
        }
        let binary = [
            "probe_success",
            "up{",
            "up ",
            "_up{",
            "probe_http_ssl",
            "_healthy",
            "_ready",
        ]
        .iter()
        .any(|m| text.contains(m) || text == "up");
        if binary
            && field.unit.as_deref().is_none_or(|u| {
                matches!(
                    u,
                    "short" | "none" | "" | "bool" | "bool_yes_no" | "bool_on_off"
                )
            })
        {
            return Self {
                low: 0.9,
                high: 1.4,
                floor: 0.,
                ceiling: 1.,
                whole: true,
                epoch: false,
            };
        }
        let counted = query.is_some_and(|q| q.trim_start().starts_with("count("));
        if counted
            && field
                .unit
                .as_deref()
                .is_none_or(|u| matches!(u, "short" | "none" | ""))
        {
            return Self {
                low: 2.,
                high: 48.,
                floor: 0.,
                ceiling: f64::INFINITY,
                whole: true,
                epoch: false,
            };
        }
        let (low, high, floor, ceiling) = match field.unit.as_deref().unwrap_or("short") {
            "percent" => (15., 75., 0., 100.),
            "percentunit" => (0.15, 0.75, 0., 1.),
            "bytes" | "decbytes" => (1.5e9, 12e9, 0., f64::INFINITY),
            "bits" | "bps" => (4e6, 9e8, 0., f64::INFINITY),
            "Bps" | "binBps" | "decBps" => (5e5, 1.2e8, 0., f64::INFINITY),
            "s" => (0.04, 0.6, 0., f64::INFINITY),
            "ms" => (20., 450., 0., f64::INFINITY),
            "µs" | "us" => (200., 9000., 0., f64::INFINITY),
            "ns" => (2e4, 9e5, 0., f64::INFINITY),
            "reqps" | "rps" | "ops" | "iops" | "wps" | "cps" => (40., 900., 0., f64::INFINITY),
            "celsius" => (38., 72., -40., 150.),
            "hertz" => (1.2e9, 3.4e9, 0., f64::INFINITY),
            _ => (8., 95., 0., f64::INFINITY),
        };
        match (field.min, field.max) {
            (Some(min), Some(max)) if max > min => {
                let span = max - min;
                Self {
                    low: min + span * 0.15,
                    high: min + span * 0.85,
                    floor: min,
                    ceiling: max,
                    whole: false,
                    epoch: false,
                }
            }
            _ => Self {
                low,
                high,
                floor,
                ceiling,
                whole: false,
                epoch: false,
            },
        }
    }

    /// `points` values for series number `index`: a level, a slow wave
    /// and mean-reverting noise.
    fn generate(&self, seed: u64, index: usize, points: usize) -> Vec<f64> {
        let mut rng = SplitMix(seed);
        let span = self.high - self.low;
        let level = self.low + span * (0.2 + 0.6 * rng.unit()) / (1. + index as f64 * 0.15);
        let amplitude = span * (0.05 + 0.15 * rng.unit());
        let period = points as f64 * (0.4 + 0.8 * rng.unit());
        let phase = rng.unit() * std::f64::consts::TAU;
        let mut drift = 0.;
        (0..points)
            .map(|t| {
                drift = drift * 0.85 + (rng.unit() - 0.5) * span * 0.08;
                let wave = (t as f64 / period * std::f64::consts::TAU + phase).sin();
                let value = (level + amplitude * wave + drift).clamp(self.floor, self.ceiling);
                if self.whole { value.round() } else { value }
            })
            .collect()
    }
}

/// Small deterministic generator; quality is irrelevant for fake charts.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// FNV-1a.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_labels_and_metric_names() {
        assert_eq!(
            placeholders("{{instance}} - {{ mode }}"),
            ["instance", "mode"]
        );
        let labels = [("integration".to_owned(), "slack".to_owned())];
        assert_eq!(
            fill_legend("sent {{ integration}} {{other}}", &labels),
            "sent slack {{other}}"
        );
        assert_eq!(
            grouping("sum by (namespace, pod) (rate(x[5m]))"),
            vec!["namespace".to_owned(), "pod".to_owned()]
        );
        assert_eq!(
            grouping(r#"sum(rate(node_cpu_seconds_total{mode!="idle"}[5m])) by (cpu)"#),
            vec!["cpu".to_owned()]
        );
        assert!(grouping("count(count(node_cpu_seconds_total{job=\"node\"}) by (cpu))").is_empty());
        assert!(grouping("scalar(node_load1) * 100 / count(count(x) by (cpu))").is_empty());
        assert_eq!(
            grouping("topk(5, sum by (pod) (rate(x[5m])))"),
            vec!["pod".to_owned()]
        );
        assert_eq!(
            grouping("sum by (namespace, pod) (rate(x[5m]))"),
            ["namespace", "pod"]
        );
        assert_eq!(
            metric_name(r#"sum(rate(node_cpu_seconds_total{mode!="idle"}[5m])) by (cpu)"#)
                .as_deref(),
            Some("node_cpu_seconds_total")
        );
        assert_eq!(metric_name("up").as_deref(), Some("up"));
    }

    #[test]
    fn label_values_never_repeat() {
        for seed in 0..5 {
            let values: Vec<_> = (0..12)
                .map(|i| label_value("mountpoint", i, seed))
                .collect();
            let mut unique = values.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), values.len(), "{values:?}");
        }
    }

    #[test]
    fn interpolates_variables() {
        let context = QueryContext {
            window: TimeWindow::new(0, 3600, 10),
            variables: vec![
                ("node".into(), "worker-1".into()),
                ("node_name".into(), "w".into()),
            ],
        };
        assert_eq!(
            context.interpolate("$node / ${node} / [[node]] / $node_name"),
            "worker-1 / worker-1 / worker-1 / w"
        );
    }

    #[test]
    fn fake_values_follow_legacy_right_axis_and_renamed_table_columns() {
        let source = FakeSource::new("field-config-regression");
        let context = QueryContext {
            window: TimeWindow::new(0, 3600, 20),
            variables: Vec::new(),
        };
        let dashboard =
            crate::Dashboard::parse(include_str!("../../../fixtures/real/11074.json")).unwrap();
        let mut checked = 0;
        for panel in &dashboard.panels {
            let Viz::TimeSeries(options) = &panel.viz else {
                continue;
            };
            let frame = source.query(panel, &context);
            for series in &frame.series {
                let field = panel.field.for_time_series(&series.name, options);
                if field.axis == crate::spec::AxisPlacement::Right
                    && field.unit.as_deref() == Some("percent")
                {
                    assert!(
                        series.values.iter().all(|v| (0. ..=100.).contains(v)),
                        "{}: {:?}",
                        series.name,
                        series.values
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked >= 3);
        let dashboard =
            crate::Dashboard::parse(include_str!("../../../fixtures/real/20398.json")).unwrap();
        let panel = dashboard
            .panels
            .iter()
            .find(|p| p.title == "Node Summary")
            .unwrap();
        let frame = source.query(panel, &context);
        let table = crate::transform::table(
            &panel.transforms,
            &frame.series,
            &panel
                .queries
                .iter()
                .map(|q| q.ref_id.as_str())
                .collect::<Vec<_>>(),
        );
        for name in ["CPU Allocation", "Memory Allocation"] {
            let i = table.columns.iter().position(|c| c == name).unwrap();
            assert!(table.rows.iter().all(
                |row| matches!(row[i], crate::transform::Cell::Number(v) if (0. ..=1.).contains(&v))
            ));
        }
    }
    #[test]
    fn geomap_fake_fields_follow_layer_aliases_and_query_overrides() {
        let dashboard = crate::Dashboard::parse(r#"{"panels":[{"type":"geomap","title":"World locations","targets":[{"refId":"B"}],"fieldConfig":{"overrides":[{"matcher":{"id":"byFrameRefID","options":"B"},"properties":[{"id":"min","value":10},{"id":"max","value":20}]}]},"options":{"layers":[{"type":"markers","location":{"mode":"coords","latitude":"y","longitude":"x"},"config":{"style":{"color":{"field":"Price"},"size":{"field":"Count"}}}}]}}]}"#).unwrap();
        let panel = &dashboard.panels[0];
        let context = QueryContext {
            window: TimeWindow::new(0, 60, 10),
            variables: Vec::new(),
        };
        let frame = FakeSource::new("map-aliases").query(panel, &context);
        for name in ["y", "x", "Price", "Count"] {
            assert!(
                frame
                    .series
                    .iter()
                    .any(|s| s.field.as_deref() == Some(name)),
                "missing {name}"
            );
        }
        assert!(
            frame
                .series
                .iter()
                .filter(|s| matches!(s.field.as_deref(), Some("Price" | "Count")))
                .flat_map(|s| &s.values)
                .all(|v| (10. ..=20.).contains(v))
        );
        let Viz::Geomap(options) = &panel.viz else {
            panic!("geomap")
        };
        let data = crate::geomap::MapData::from_frame(&frame, options, &panel.field);
        assert_eq!(data.layers[0].len(), 18);
        assert!(data.layers[0].iter().any(|p| p.longitude < -70.));
        assert!(data.layers[0].iter().any(|p| p.latitude < 0.));
        let mut no_queries = panel.clone();
        no_queries.queries.clear();
        assert!(
            !FakeSource::new("fallback-map")
                .query(&no_queries, &context)
                .series
                .is_empty()
        );
    }
}
