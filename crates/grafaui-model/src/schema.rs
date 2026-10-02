//! Grafana dashboard JSON as it is stored, read leniently.
//!
//! Every field defaults when missing, and fields this crate doesn't model
//! stay in `extra` as raw JSON, so any dashboard that Grafana exports parses.
//! [`crate::spec`] turns these raw types into what the app draws.

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

/// A number that may be saved as a number, a numeric string, `""` or
/// `null`. Anything that isn't a number reads as `None`.
fn lenient_number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    Ok(lenient_number(deserializer)?
        .filter(|n| *n >= 0.)
        .map(|n| n as u32))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawDashboard {
    pub uid: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub schema_version: u32,
    pub panels: Vec<RawPanel>,
    /// Dashboards before schema version 16 kept panels inside `rows`.
    pub rows: Vec<LegacyRow>,
    pub templating: Templating,
    pub time: TimeRange,
    pub refresh: Value,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawPanel {
    pub id: Option<u32>,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub grid_pos: Option<GridPos>,
    pub targets: Vec<Target>,
    pub field_config: FieldConfig,
    pub options: Value,
    pub transformations: Vec<Value>,
    pub links: Vec<Value>,
    pub repeat: Option<String>,
    /// Only on `row` panels: whether the row is folded, and if so the
    /// panels folded inside it.
    pub collapsed: bool,
    pub panels: Vec<RawPanel>,
    /// Legacy `span` (1–12) from the old `rows` layout.
    #[serde(deserialize_with = "lenient_number")]
    pub span: Option<f64>,
    pub height: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GridPos {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Target {
    pub ref_id: Option<String>,
    pub expr: Option<String>,
    pub legend_format: Option<String>,
    pub hide: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Target {
    /// The query text whatever the data source calls it.
    pub fn query(&self) -> Option<String> {
        if let Some(expr) = self.expr.as_ref().filter(|e| !e.is_empty()) {
            return Some(expr.clone());
        }
        ["query", "rawSql", "target", "expression"]
            .iter()
            .find_map(|key| self.extra.get(*key)?.as_str().map(str::to_owned))
            .filter(|q| !q.is_empty())
    }

    /// A CloudWatch target's legend: its alias or label with the metric and
    /// statistic filled in, else the metric name, plus a `{{dimension}}`
    /// for each wildcard dimension so the mock returns a series per value.
    pub fn cloudwatch_legend(&self) -> Option<String> {
        let metric = self
            .extra
            .get("metricName")?
            .as_str()
            .filter(|m| !m.is_empty())?;
        let stat = self
            .extra
            .get("statistic")
            .and_then(Value::as_str)
            .or_else(|| self.extra.get("statistics")?.get(0)?.as_str())
            .unwrap_or("Average");
        let template = ["label", "alias"]
            .iter()
            .find_map(|key| self.extra.get(*key)?.as_str().filter(|t| !t.is_empty()));
        let mut legend = match template {
            Some(template) => template
                .replace("{{metric}}", metric)
                .replace("{{stat}}", stat)
                .replace("${PROP('MetricName')}", metric)
                .replace("${PROP('Stat')}", stat),
            None => metric.to_owned(),
        };
        let wildcards = self
            .extra
            .get("dimensions")
            .and_then(Value::as_object)
            .into_iter()
            .flatten();
        for (key, value) in wildcards {
            let wildcard =
                value.as_str() == Some("*") || value.get(0).and_then(Value::as_str) == Some("*");
            if wildcard && !legend.contains(&format!("{{{{{key}}}}}")) {
                legend.push_str(&format!(" {{{{{key}}}}}"));
            }
        }
        Some(legend)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct FieldConfig {
    pub defaults: FieldDefaults,
    pub overrides: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct FieldDefaults {
    pub unit: Option<String>,
    #[serde(deserialize_with = "lenient_count")]
    pub decimals: Option<u32>,
    #[serde(deserialize_with = "lenient_number")]
    pub min: Option<f64>,
    #[serde(deserialize_with = "lenient_number")]
    pub max: Option<f64>,
    pub thresholds: Option<Thresholds>,
    pub color: Option<ColorConfig>,
    pub custom: Value,
    pub mappings: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    pub mode: Option<String>,
    pub steps: Vec<ThresholdStep>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ThresholdStep {
    pub color: String,
    /// `null` for the base step.
    #[serde(deserialize_with = "lenient_number")]
    pub value: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ColorConfig {
    pub mode: Option<String>,
    pub fixed_color: Option<String>,
    pub series_by: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct LegacyRow {
    pub title: String,
    pub collapse: bool,
    #[serde(rename = "showTitle")]
    pub show_title: bool,
    pub height: Value,
    pub panels: Vec<RawPanel>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Templating {
    pub list: Vec<Variable>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    /// 0 shows the variable, 1 hides its label, 2 hides it entirely.
    pub hide: u8,
    pub current: Value,
    pub options: Vec<Value>,
    pub query: Value,
    pub multi: bool,
    pub include_all: bool,
    pub all_value: Option<String>,
    pub regex: String,
    pub sort: u8,
    pub datasource: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct TimeRange {
    pub from: String,
    pub to: String,
}

impl Default for TimeRange {
    fn default() -> Self {
        Self {
            from: "now-6h".into(),
            to: "now".into(),
        }
    }
}
