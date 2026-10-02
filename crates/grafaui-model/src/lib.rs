//! Grafana dashboards as the app draws them, with no UI types.
//!
//! [`Dashboard::parse`] reads the JSON Grafana exports, converts legacy
//! panel types and layouts, and places every panel on the grid. Panel data
//! comes from a [`data::DataSource`]; for now only the fake one exists.

pub mod chart;
pub mod color;
pub mod data;
pub mod geomap;
pub mod heatmap;
pub mod layout;
pub mod overrides;
pub mod schema;
pub mod spec;
pub mod sql;
pub mod time;
pub mod transform;
pub mod units;

use serde_json::Value;

pub use layout::{Placed, RowHeader, Section};
pub use spec::{PanelSpec, Support, Viz};

#[derive(Clone, Debug)]
pub struct Dashboard {
    pub uid: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    /// The `from` of the saved time range, such as `now-6h`.
    pub time_from: String,
    pub time_to: String,
    pub variables: Vec<Variable>,
    pub sections: Vec<Section>,
    /// Every panel, rows excluded; [`Placed::key`] indexes it.
    pub panels: Vec<PanelSpec>,
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub label: String,
    pub kind: String,
    pub options: Vec<String>,
    pub current: String,
    /// Not shown in the controls (hidden, or a constant), but still filled
    /// into queries and titles.
    pub hidden: bool,
    pub query: Option<String>,
    pub regex: String,
    pub sort: u8,
    pub multi: bool,
    pub include_all: bool,
    pub all_value: Option<String>,
    pub datasource: Value,
    /// Actual saved values, separate from the fake-mode display choices.
    pub selected: Vec<String>,
    pub saved_options: Vec<String>,
}

impl Dashboard {
    /// Reads a dashboard export. Also accepts the `{"dashboard": …}`
    /// wrapper that Grafana's HTTP API returns.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        Self::parse_with_repeats(json, true)
    }

    /// Real connections resolve query variables after loading. Render repeated
    /// panels once until live layout expansion is implemented, never using the
    /// fake variable choices to generate real requests.
    pub fn parse_unexpanded(json: &str) -> Result<Self, serde_json::Error> {
        Self::parse_with_repeats(json, false)
    }

    fn parse_with_repeats(json: &str, expand: bool) -> Result<Self, serde_json::Error> {
        let mut value: Value = serde_json::from_str(json)?;
        if let Some(inner) = value.get_mut("dashboard").filter(|d| d.is_object()) {
            value = inner.take();
        }
        let raw: schema::RawDashboard = serde_json::from_value(value)?;
        let mut panels = Vec::new();
        let mut sections = if raw.panels.is_empty() && !raw.rows.is_empty() {
            layout::legacy_sections(&raw.rows, &mut panels)
        } else {
            layout::sections(&raw.panels, &mut panels)
        };
        let variables: Vec<Variable> = raw.templating.list.iter().filter_map(variable).collect();
        if expand {
            layout::expand_repeats(&mut sections, &mut panels, |name| {
                let Some(variable) = variables.iter().find(|v| v.name == name) else {
                    return Vec::new();
                };
                // Every value when "All" (or several) are picked, else the one.
                let all = variable.current == "All" || variable.current.contains(" + ");
                let values: Vec<String> = if all {
                    variable
                        .options
                        .iter()
                        .filter(|o| *o != "All" && !o.contains(" + "))
                        .cloned()
                        .collect()
                } else {
                    vec![variable.current.clone()]
                };
                values.into_iter().take(8).collect()
            });
        } else {
            for panel in &mut panels {
                if panel.repeat.is_some() {
                    panel
                        .ignored
                        .push("Live repeat expansion (shown once)".into());
                }
            }
        }
        Ok(Self {
            uid: raw.uid.clone(),
            title: if raw.title.is_empty() {
                "Untitled dashboard".into()
            } else {
                raw.title.clone()
            },
            tags: raw.tags.clone(),
            time_from: raw.time.from.clone(),
            time_to: raw.time.to.clone(),
            variables,
            sections,
            panels,
        })
    }

    pub fn panel(&self, key: usize) -> &PanelSpec {
        &self.panels[key]
    }

    /// Panel counts by support level: (full, partial, placeholder).
    pub fn support_counts(&self) -> (usize, usize, usize) {
        self.panels
            .iter()
            .fold((0, 0, 0), |(full, partial, none), panel| {
                match panel.support() {
                    Support::Full => (full + 1, partial, none),
                    Support::Partial => (full, partial + 1, none),
                    Support::Placeholder => (full, partial, none + 1),
                }
            })
    }
}

/// A variable with its options. Query variables save only their current
/// value, so the mock adds a few made-up options after it.
fn variable(raw: &schema::Variable) -> Option<Variable> {
    if raw.name.is_empty() {
        return None;
    }
    let text = |value: &Value| -> Option<String> {
        let text = value
            .get("text")
            .or_else(|| value.get("value"))
            .unwrap_or(value);
        match text {
            Value::String(s) => Some(s.clone()),
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" + "),
            ),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let mut options: Vec<String> = raw
        .options
        .iter()
        .filter_map(text)
        .filter(|o| !o.is_empty())
        .collect();
    let current = text(&raw.current)
        .filter(|c| !c.is_empty())
        .or_else(|| options.first().cloned())
        .or_else(|| {
            // A constant's or text box's value is its query.
            let fixed = matches!(raw.kind.as_str(), "constant" | "textbox");
            raw.query
                .as_str()
                .filter(|q| fixed && !q.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "All".into());
    if options.is_empty() {
        options.push(current.clone());
        if matches!(raw.kind.as_str(), "query" | "custom" | "datasource") {
            options.extend((1..=3).map(|n| format!("{}-{n}", raw.name)));
        }
    }
    if !options.contains(&current) {
        options.insert(0, current.clone());
    }
    Some(Variable {
        name: raw.name.clone(),
        label: raw
            .label
            .clone()
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| raw.name.clone()),
        kind: raw.kind.clone(),
        options,
        current,
        hidden: raw.hide == 2 || raw.kind == "constant",
        query: raw
            .query
            .as_str()
            .or_else(|| raw.query.get("query").and_then(Value::as_str))
            .map(str::to_owned),
        regex: raw.regex.clone(),
        sort: raw.sort,
        multi: raw.multi,
        include_all: raw.include_all,
        all_value: raw.all_value.clone(),
        datasource: raw.datasource.clone(),
        selected: match raw.current.get("value") {
            Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        },
        saved_options: raw
            .options
            .iter()
            .filter_map(|v| v.get("value").or_else(|| v.get("text")))
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use data::{DataSource, FakeSource, QueryContext};

    #[test]
    fn live_layout_does_not_expand_repeats_with_mock_variable_options() {
        let dashboard = Dashboard::parse_unexpanded(r#"{"templating":{"list":[{"name":"node","type":"query","query":"label_values(up, instance)"}]},
            "panels":[{"type":"timeseries","repeat":"node","targets":[{"expr":"up{instance=\"$node\"}"}]}]}"#).unwrap();
        assert_eq!(dashboard.panels.len(), 1);
        assert!(dashboard.panel(0).scoped.is_empty());
        assert!(
            dashboard
                .panel(0)
                .ignored
                .iter()
                .any(|n| n.contains("Live repeat"))
        );
        assert!(dashboard.variables[0].saved_options.is_empty());
    }

    #[test]
    fn parses_api_wrapper_and_variables() {
        let dashboard = Dashboard::parse(
            r#"{"dashboard":{"title":"T","templating":{"list":[
                {"name":"node","type":"query","current":{"text":"worker-1","value":"worker-1"}},
                {"name":"hidden","type":"query","hide":2},
                {"name":"cluster","type":"constant","query":"prod-eu-1"},
                {"name":"env","type":"custom","current":{"text":"prod"},
                 "options":[{"text":"dev"},{"text":"prod"}]}]},
              "panels":[{"type":"timeseries","targets":[{"expr":"up","legendFormat":"{{instance}}"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(dashboard.title, "T");
        let shown: Vec<_> = dashboard.variables.iter().filter(|v| !v.hidden).collect();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].options[0], "worker-1");
        assert_eq!(shown[1].options, ["dev", "prod"]);
        assert_eq!(shown[1].current, "prod");
        // Hidden ones are kept for interpolation.
        let cluster = dashboard
            .variables
            .iter()
            .find(|v| v.name == "cluster")
            .unwrap();
        assert!(cluster.hidden);
        assert_eq!(cluster.current, "prod-eu-1");
    }

    #[test]
    fn fake_data_is_deterministic_and_bounded() {
        let dashboard = Dashboard::parse(
            r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"unit":"percent"}},
                "targets":[{"expr":"x","legendFormat":"{{mode}}"},{"expr":"rate(y_total[5m])"}]}]}"#,
        )
        .unwrap();
        let context = QueryContext {
            window: time::TimeWindow::new(1_700_000_000, 3600, 30),
            variables: Vec::new(),
        };
        let source = FakeSource::new("uid");
        let a = source.query(dashboard.panel(0), &context);
        let b = source.query(dashboard.panel(0), &context);
        assert!(a.series.len() >= 3);
        assert_eq!(a.series.last().unwrap().name, "y_total");
        assert_eq!(a.series[0].name, a.series[0].labels[0].1);
        for (x, y) in a.series.iter().zip(&b.series) {
            assert_eq!(x.values, y.values);
            assert_eq!(x.values.len(), 30);
            assert!(x.values.iter().all(|v| (0. ..=100.).contains(v)));
        }
    }
}
