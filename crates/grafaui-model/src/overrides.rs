//! Field overrides: per-series changes to the panel's field config, picked
//! by series name.
//!
//! Styles include color, drawing mode, points, dashes, negative-Y and hiding.
//! Field patches include units, range, thresholds, axis and table cell mode.
//! Matchers select by name, regex or a reduction of the actual values.

use regex::Regex;
use serde_json::Value;

use crate::color::{self, Rgba};
use crate::schema::Thresholds;
use crate::spec::{
    self, AxisPlacement, AxisScale, Calc, CellDisplay, ColorMode, DrawStyle, GradientMode,
    LineStyle, Mapping, StackMode, Stacking, Step, ThresholdStyle,
};

#[derive(Clone, Debug)]
pub struct Override {
    matcher: Matcher,
    style: SeriesStyle,
    patch: Option<FieldPatch>,
    hidden: Option<bool>,
    hidden_in_legend: Option<bool>,
}

/// Grafana field types used by `byType` override matchers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldType {
    #[default]
    Number,
    String,
    Time,
    Boolean,
    Other,
}

impl FieldType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "number" => Some(Self::Number),
            "string" => Some(Self::String),
            "time" => Some(Self::Time),
            "boolean" => Some(Self::Boolean),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// A field's identity after transformations, with its query and value metadata.
/// Resolution is read-only; the panel owns this information.
#[derive(Clone, Copy, Debug)]
pub struct FieldContext<'a> {
    name: &'a str,
    source: Option<&'a str>,
    query: Option<&'a str>,
    kind: FieldType,
    values: Option<&'a [f64]>,
}

impl<'a> FieldContext<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            source: None,
            query: None,
            kind: FieldType::Number,
            values: None,
        }
    }

    pub fn source(mut self, source: &'a str) -> Self {
        self.source = Some(source);
        self
    }
    pub fn query(mut self, query: &'a str) -> Self {
        self.query = Some(query);
        self
    }
    pub fn kind(mut self, kind: FieldType) -> Self {
        self.kind = kind;
        self
    }
    pub fn values(mut self, values: &'a [f64]) -> Self {
        self.values = Some(values);
        self
    }

    fn identities(self) -> impl Iterator<Item = &'a str> {
        std::iter::once(self.name).chain(self.source)
    }
}

/// Field config an override changes for the series it matches.
#[derive(Clone, Debug, Default)]
pub struct FieldPatch {
    pub unit: Option<String>,
    pub decimals: Option<u32>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Steps, and whether they are percentages.
    pub steps: Option<(Vec<Step>, bool)>,
    pub mappings: Option<Vec<Mapping>>,
    pub axis: Option<AxisPlacement>,
    pub threshold_style: Option<ThresholdStyle>,
    pub cell_display: Option<CellDisplay>,
    pub color: Option<ColorMode>,
    pub color_calc: Option<Calc>,
    pub scale: Option<AxisScale>,
}

#[derive(Clone, Debug)]
enum Matcher {
    Name(String),
    Names(Vec<String>),
    Regex(Regex),
    Value(ValueMatcher),
    Query(String),
    Type(FieldType),
}

#[derive(Clone, Debug)]
enum ValueMatcher {
    AllNull,
    AllZero,
    Reduced {
        reducer: Calc,
        op: Comparison,
        value: f64,
    },
}

#[derive(Clone, Copy, Debug)]
enum Comparison {
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
}

impl ValueMatcher {
    fn parse(raw: &Value) -> Option<Self> {
        let reducer = raw.get("reducer")?.as_str()?;
        // Grafana boolean reducers return their boolean directly, ignoring
        // op/value (fixtures use gte 0 even for allIsNull).
        match reducer {
            "allIsNull" => return Some(Self::AllNull),
            "allIsZero" => return Some(Self::AllZero),
            _ => {}
        }
        let reducer = Calc::parse(reducer)?;
        let op = match raw.get("op").and_then(Value::as_str).unwrap_or("eq") {
            "eq" => Comparison::Equal,
            "neq" | "ne" => Comparison::NotEqual,
            "gt" => Comparison::Greater,
            "gte" => Comparison::GreaterEqual,
            "lt" => Comparison::Less,
            "lte" => Comparison::LessEqual,
            _ => return None,
        };
        Some(Self::Reduced {
            reducer,
            op,
            value: raw.get("value")?.as_f64()?,
        })
    }

    fn matches(&self, values: &[f64]) -> bool {
        match self {
            Self::AllNull => values.iter().all(|v| v.is_nan()),
            Self::AllZero => {
                values.iter().any(|v| !v.is_nan())
                    && values.iter().filter(|v| !v.is_nan()).all(|v| *v == 0.)
            }
            Self::Reduced { reducer, op, value } => {
                let reduced = reducer.reduce(values);
                match op {
                    Comparison::Equal => reduced == *value,
                    Comparison::NotEqual => reduced != *value,
                    Comparison::Greater => reduced > *value,
                    Comparison::GreaterEqual => reduced >= *value,
                    Comparison::Less => reduced < *value,
                    Comparison::LessEqual => reduced <= *value,
                }
            }
        }
    }
}

impl Matcher {
    fn parse(raw: &Value) -> Result<Self, String> {
        let id = raw.get("id").and_then(Value::as_str).unwrap_or_default();
        let options = raw.get("options");
        match id {
            "byName" => Ok(Self::Name(
                options
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )),
            "byNames" => Ok(Self::Names(
                options
                    .and_then(|o| o.get("names"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            )),
            "byRegexp" => {
                let pattern = options.and_then(Value::as_str).unwrap_or_default();
                js_regex(pattern).ok_or_else(|| format!("override regex {pattern:?}"))
            }
            "byValue" => options
                .and_then(ValueMatcher::parse)
                .map(Self::Value)
                .ok_or_else(|| "override matcher byValue".to_owned()),
            "byFrameRefID" => options
                .and_then(Value::as_str)
                .map(|q| Self::Query(q.to_owned()))
                .ok_or_else(|| "override matcher byFrameRefID".to_owned()),
            "byType" => options
                .and_then(Value::as_str)
                .and_then(FieldType::parse)
                .map(Self::Type)
                .ok_or_else(|| "override matcher byType".to_owned()),
            other => Err(format!("override matcher {other}")),
        }
    }

    fn matches(&self, field: &FieldContext<'_>) -> bool {
        match self {
            Self::Name(n) => field.identities().any(|name| n == name),
            Self::Names(names) => field
                .identities()
                .any(|name| names.iter().any(|n| n == name)),
            Self::Regex(regex) => field.identities().any(|name| regex.is_match(name)),
            Self::Value(matcher) => field.values.is_some_and(|v| matcher.matches(v)),
            Self::Query(query) => field.query == Some(query.as_str()),
            Self::Type(kind) => *kind == field.kind,
        }
    }
}

/// Grafana's reading of a regex option: `/…/flags` matches anywhere, a bare
/// pattern must match the whole name.
fn js_regex(pattern: &str) -> Option<Matcher> {
    regex(pattern, true).map(Matcher::Regex)
}

/// A JavaScript regex as Grafana writes them: `/…/flags`, or a bare pattern
/// that must match the whole text when `anchored`.
pub(crate) fn regex(pattern: &str, anchored: bool) -> Option<Regex> {
    let source = match pattern.strip_prefix('/').and_then(|p| p.rsplit_once('/')) {
        Some((body, flags)) if flags.contains('i') => format!("(?i){}", literal_braces(body)),
        Some((body, _)) => literal_braces(body),
        None if anchored => format!("^(?:{})$", literal_braces(pattern)),
        None => literal_braces(pattern),
    };
    Regex::new(&source).ok()
}

/// JavaScript reads a brace that doesn't start a `{n}`, `{n,}` or `{n,m}`
/// quantifier as a literal brace; the regex crate rejects it.
fn literal_braces(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i]);
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            '{' => {
                let rest: String = chars[i + 1..].iter().collect();
                let quantifier = rest.split_once('}').is_some_and(|(inner, _)| {
                    let (min, max) = inner.split_once(',').unwrap_or((inner, "0"));
                    !min.is_empty()
                        && min.chars().all(|c| c.is_ascii_digit())
                        && max.chars().all(|c| c.is_ascii_digit())
                });
                if quantifier {
                    let end = i + 1 + rest.find('}').unwrap_or(0);
                    out.extend(&chars[i..=end]);
                    i = end + 1;
                    continue;
                }
                out.push_str("\\{");
            }
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// How one series is drawn, after the overrides that match it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeriesStyle {
    pub color: Option<Rgba>,
    pub display_name: Option<String>,
    /// Drawn below zero, as Grafana does for transmit next to receive.
    pub negative_y: bool,
    /// 0–1.
    pub fill_opacity: Option<f32>,
    /// Left out of the chart (and its legend).
    pub hidden: bool,
    /// Left out of the legend only.
    pub hidden_in_legend: bool,
    pub line_style: Option<LineStyle>,
    pub draw: Option<DrawStyle>,
    pub show_points: Option<bool>,
    pub line_width: Option<f32>,
    pub point_size: Option<f32>,
    pub gradient: Option<GradientMode>,
    pub stacking: Option<Stacking>,
}

impl SeriesStyle {
    fn merge(&mut self, other: &SeriesStyle) {
        self.color = other.color.or(self.color);
        if other.display_name.is_some() {
            self.display_name.clone_from(&other.display_name);
        }
        self.negative_y |= other.negative_y;
        self.fill_opacity = other.fill_opacity.or(self.fill_opacity);
        self.hidden |= other.hidden;
        self.hidden_in_legend |= other.hidden_in_legend;
        if other.line_style.is_some() {
            self.line_style.clone_from(&other.line_style);
        }
        self.draw = other.draw.or(self.draw);
        self.show_points = other.show_points.or(self.show_points);
        self.line_width = other.line_width.or(self.line_width);
        self.point_size = other.point_size.or(self.point_size);
        self.gradient = other.gradient.or(self.gradient);
        if other.stacking.is_some() {
            self.stacking.clone_from(&other.stacking);
        }
    }
}

/// Reads the panel's overrides; what they set that isn't drawn goes to
/// `ignored`, once per kind.
pub(crate) fn parse(raw: &[Value], ignored: &mut Vec<String>) -> Vec<Override> {
    let mut note = |text: String| {
        if !ignored.contains(&text) {
            ignored.push(text);
        }
    };
    let mut overrides = Vec::new();
    for raw in raw {
        let matcher = match Matcher::parse(raw.get("matcher").unwrap_or(&Value::Null)) {
            Ok(matcher) => matcher,
            Err(unsupported) => {
                note(unsupported);
                continue;
            }
        };
        let mut style = SeriesStyle::default();
        let mut hidden = None;
        let mut hidden_in_legend = None;
        let mut patch = FieldPatch::default();
        let mut patched = false;
        let properties = raw.get("properties").and_then(Value::as_array);
        for property in properties.into_iter().flatten() {
            let id = property
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let value = property.get("value").unwrap_or(&Value::Null);
            match id {
                "color" => {
                    if let Some(mode) = value.get("mode").and_then(Value::as_str) {
                        patch.color =
                            ColorMode::parse(mode, value.get("fixedColor").and_then(Value::as_str));
                        if let Some(ColorMode::Fixed(color)) = patch.color {
                            style.color = Some(color);
                        }
                        if patch.color.is_none() {
                            note("override color scheme".into());
                        }
                    }
                    patch.color_calc = value
                        .get("seriesBy")
                        .and_then(Value::as_str)
                        .and_then(Calc::parse);
                    patched |= patch.color.is_some() || patch.color_calc.is_some();
                }
                "displayName" | "displayNameFromDS" => {
                    style.display_name = value.as_str().map(str::to_owned);
                }
                "custom.transform" => match value.as_str() {
                    Some("negative-Y") => style.negative_y = true,
                    _ => note("override transform".into()),
                },
                "custom.fillOpacity" => {
                    style.fill_opacity = value.as_f64().map(|v| (v / 100.).clamp(0., 1.) as f32);
                }
                "custom.hideFrom" => {
                    let flag = |key| value.get(key).and_then(Value::as_bool).unwrap_or(false);
                    style.hidden = flag("viz");
                    style.hidden_in_legend = flag("legend");
                    hidden = Some(style.hidden);
                    hidden_in_legend = Some(style.hidden_in_legend);
                }
                "custom.hideFrom.viz" | "custom.hidden" => hidden = value.as_bool(),
                "custom.hideFrom.legend" => hidden_in_legend = value.as_bool(),
                "custom.lineStyle" => {
                    style.line_style = LineStyle::parse(value);
                    if style.line_style.is_none() {
                        note("dashed lines".into());
                    }
                }
                "custom.drawStyle" => {
                    style.draw = match value.as_str() {
                        Some("line") => Some(DrawStyle::Line),
                        Some("points") => Some(DrawStyle::Points),
                        Some("bars") => Some(DrawStyle::Bars),
                        _ => {
                            note("override property custom.drawStyle".into());
                            None
                        }
                    };
                }
                "custom.showPoints" => {
                    style.show_points = match value.as_str() {
                        Some("always") => Some(true),
                        Some("never" | "auto") => Some(false),
                        _ => None,
                    };
                }
                "custom.lineWidth" => style.line_width = value.as_f64().map(|v| v.max(0.) as f32),
                "custom.pointSize" => style.point_size = value.as_f64().map(|v| v.max(0.) as f32),
                "custom.gradientMode" => {
                    style.gradient = value.as_str().and_then(GradientMode::parse);
                    if style.gradient.is_none() {
                        note("override property custom.gradientMode".into());
                    }
                }
                "custom.stacking" => {
                    style.stacking = Stacking::parse(value);
                    if style.stacking.is_none() {
                        note("per-series stacking".into());
                    }
                }
                "custom.scaleDistribution" => {
                    patch.scale = AxisScale::parse(value);
                    patched = true;
                    if patch.scale.is_none() {
                        note("override axis scale distribution".into());
                    }
                }
                "custom.axisPlacement" => {
                    patch.axis = value.as_str().and_then(AxisPlacement::parse);
                    patched = true;
                }
                "custom.thresholdsStyle" => {
                    patch.threshold_style = value
                        .get("mode")
                        .and_then(Value::as_str)
                        .and_then(ThresholdStyle::parse);
                    patched = true;
                }
                "unit" => {
                    patch.unit = value.as_str().map(str::to_owned);
                    patched = true;
                }
                "decimals" => {
                    patch.decimals = value.as_u64().map(|d| d as u32);
                    patched = true;
                }
                "min" | "max" => {
                    let bound = if id == "min" {
                        &mut patch.min
                    } else {
                        &mut patch.max
                    };
                    *bound = value.as_f64();
                    patched = true;
                }
                "thresholds" => {
                    let thresholds: Option<Thresholds> = serde_json::from_value(value.clone()).ok();
                    patch.steps = Some(spec::steps(thresholds.as_ref()));
                    patched = true;
                }
                "mappings" => {
                    let raw = value.as_array().map(Vec::as_slice).unwrap_or_default();
                    let mut notes = Vec::new();
                    patch.mappings = Some(spec::mappings(raw, &mut notes));
                    notes.into_iter().for_each(&mut note);
                    patched = true;
                }
                "custom.cellOptions" | "custom.displayMode" => {
                    patch.cell_display = CellDisplay::parse(value);
                    if patch.cell_display.is_some() {
                        patched = true;
                    } else {
                        note("table cell display modes".into());
                    }
                }
                // Cosmetic: line and point sizes, table column sizing and
                // alignment, links, filters, tooltips.
                "custom.width"
                | "custom.minWidth"
                | "custom.align"
                | "links"
                | "custom.filterable"
                | "custom.inspect"
                | "description"
                | "custom.hideFrom.tooltip"
                | "noValue"
                | "displayNameFromDS.prefix" => {}
                other => note(format!("override property {other}")),
            }
        }
        let patch = patched.then_some(patch);
        overrides.push(Override {
            matcher,
            style,
            patch,
            hidden,
            hidden_in_legend,
        });
    }
    overrides
}

/// Reads a legacy graph's `seriesOverrides`: an alias (a name, or a
/// `/regex/`) and the old graph's per-series options.
pub(crate) fn parse_legacy(
    raw: &[Value],
    _stacked: bool,
    ignored: &mut Vec<String>,
) -> Vec<Override> {
    let mut note = |text: &str| {
        if !ignored.iter().any(|i| i == text) {
            ignored.push(text.to_owned());
        }
    };
    let mut overrides = Vec::new();
    for raw in raw {
        let Some(alias) = raw
            .get("alias")
            .and_then(Value::as_str)
            .filter(|a| !a.is_empty())
        else {
            continue;
        };
        let matcher = if alias.starts_with('/') {
            match js_regex(alias) {
                Some(matcher) => matcher,
                None => {
                    note(&format!("override regex {alias:?}"));
                    continue;
                }
            }
        } else {
            Matcher::Name(alias.to_owned())
        };
        let mut style = SeriesStyle::default();
        let Some(options) = raw.as_object() else {
            continue;
        };
        let mut patch = FieldPatch::default();
        for (key, value) in options {
            match key.as_str() {
                "transform" if value.as_str() == Some("negative-Y") => style.negative_y = true,
                "transform" => note("override transform"),
                "color" => {
                    style.color = value.as_str().and_then(color::parse);
                    patch.color = style.color.map(ColorMode::Fixed);
                }
                "fill" => {
                    style.fill_opacity = value.as_f64().map(|f| (f / 10.).clamp(0., 1.) as f32)
                }
                "legend" => style.hidden_in_legend = value.as_bool() == Some(false),
                "hiddenSeries" => style.hidden = value.as_bool() == Some(true),
                "dashes" => {
                    style.line_style = Some(if value.as_bool() == Some(true) {
                        let length = |key| {
                            raw.get(key).and_then(Value::as_f64).unwrap_or(10.).max(0.5) as f32
                        };
                        LineStyle::Dashed(vec![length("dashLength"), length("spaceLength")])
                    } else {
                        LineStyle::Solid
                    })
                }
                "yaxis" => {
                    patch.axis = Some(if value.as_f64() == Some(2.) {
                        AxisPlacement::Right
                    } else {
                        AxisPlacement::Left
                    })
                }
                "stack" => {
                    style.stacking = value.as_bool().map(|v| {
                        Stacking::new(
                            if v {
                                StackMode::Normal
                            } else {
                                StackMode::None
                            },
                            "A",
                        )
                    })
                }
                "bars" if value.as_bool() == Some(true) => style.draw = Some(DrawStyle::Bars),
                "lines" if value.as_bool() == Some(false) => style.draw = Some(DrawStyle::Points),
                "points" => style.show_points = value.as_bool(),
                "linewidth" => style.line_width = value.as_f64().map(|v| v.max(0.) as f32),
                "pointradius" => style.point_size = value.as_f64().map(|v| (v * 2.).max(0.) as f32),
                // Cosmetic, or the same as the panel.
                "alias" | "$$hashKey" | "fillGradient" | "zindex" | "dashLength"
                | "spaceLength" | "bars" | "lines" | "steppedLine" | "nullPointMode"
                | "legend_" => {}
                other => note(&format!("override property {other}")),
            }
        }
        // Legacy draw flags are independent booleans. Bars take priority
        // over lines=false, regardless of JSON object key order.
        if raw.get("bars").and_then(Value::as_bool) == Some(true) {
            style.draw = Some(DrawStyle::Bars);
        } else if raw.get("lines").and_then(Value::as_bool) == Some(true) {
            style.draw = Some(DrawStyle::Line);
        }
        overrides.push(Override {
            matcher,
            style,
            patch: (patch.axis.is_some() || patch.color.is_some()).then_some(patch),
            hidden: raw.get("hiddenSeries").and_then(Value::as_bool),
            hidden_in_legend: raw.get("legend").and_then(Value::as_bool).map(|v| !v),
        });
    }
    overrides
}

/// The field patches of the overrides that match series `name`, in order.
pub fn patches<'a>(
    overrides: &'a [Override],
    name: &'a str,
) -> impl Iterator<Item = &'a FieldPatch> {
    patches_with_values(overrides, name, None)
}

pub(crate) fn patches_with_values<'a>(
    overrides: &'a [Override],
    name: &'a str,
    values: Option<&'a [f64]>,
) -> impl Iterator<Item = &'a FieldPatch> {
    let mut field = FieldContext::new(name);
    field.values = values;
    patches_for_field(overrides, field)
}

pub(crate) fn patches_for_field<'a>(
    overrides: &'a [Override],
    field: FieldContext<'a>,
) -> impl Iterator<Item = &'a FieldPatch> {
    overrides
        .iter()
        .filter(move |o| o.patch.is_some() && o.matcher.matches(&field))
        .filter_map(|o| o.patch.as_ref())
}

pub(crate) fn column_patches<'a>(
    overrides: &'a [Override],
    shown: &'a str,
    source: &'a str,
    values: Option<&'a [f64]>,
) -> impl Iterator<Item = &'a FieldPatch> {
    let mut field = FieldContext::new(shown).source(source);
    field.values = values;
    patches_for_field(overrides, field)
}

/// Series names the overrides pick out by exact name, in order. The mock
/// names its series after them where it can, so the overrides apply.
pub fn names(overrides: &[Override]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for o in overrides {
        let found = match &o.matcher {
            Matcher::Name(name) => std::slice::from_ref(name),
            Matcher::Names(list) => list.as_slice(),
            Matcher::Regex(_) | Matcher::Value(_) | Matcher::Query(_) | Matcher::Type(_) => &[],
        };
        for name in found {
            if !name.is_empty() && !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

/// The style of series `name`: every matching override, in order.
pub fn style(overrides: &[Override], name: &str) -> SeriesStyle {
    style_with_values(overrides, name, None)
}

pub(crate) fn style_with_values(
    overrides: &[Override],
    name: &str,
    values: Option<&[f64]>,
) -> SeriesStyle {
    let mut field = FieldContext::new(name);
    field.values = values;
    style_for_field(overrides, &field)
}

pub(crate) fn style_for_field(overrides: &[Override], field: &FieldContext<'_>) -> SeriesStyle {
    let mut style = SeriesStyle::default();
    for o in overrides.iter().filter(|o| o.matcher.matches(field)) {
        style.merge(&o.style);
        if let Some(hidden) = o.hidden {
            style.hidden = hidden;
        }
        if let Some(hidden) = o.hidden_in_legend {
            style.hidden_in_legend = hidden;
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(json: &str) -> (Vec<Override>, Vec<String>) {
        let raw: Vec<Value> = serde_json::from_str(json).unwrap();
        let mut ignored = Vec::new();
        (parse(&raw, &mut ignored), ignored)
    }

    #[test]
    fn regex_override_flips_transmit() {
        let (overrides, ignored) = parsed(
            r#"[{"matcher":{"id":"byRegexp","options":"/.*Tx.*/"},
                 "properties":[{"id":"custom.transform","value":"negative-Y"},
                               {"id":"color","value":{"mode":"fixed","fixedColor":"red"}}]}]"#,
        );
        assert!(ignored.is_empty(), "{ignored:?}");
        let tx = style(&overrides, "Tx eth0");
        assert!(tx.negative_y);
        assert!(tx.color.is_some());
        assert_eq!(style(&overrides, "Rx eth0"), SeriesStyle::default());
    }

    #[test]
    fn bare_regex_matches_whole_name() {
        let (overrides, _) = parsed(
            r#"[{"matcher":{"id":"byRegexp","options":"Total"},
                 "properties":[{"id":"custom.fillOpacity","value":0}]}]"#,
        );
        assert_eq!(style(&overrides, "Total").fill_opacity, Some(0.));
        assert_eq!(style(&overrides, "Total RAM").fill_opacity, None);
    }

    #[test]
    fn legacy_aliases() {
        let raw: Vec<Value> = serde_json::from_str(
            r##"[{"alias":"/.*out.*/","transform":"negative-Y","$$hashKey":"object:1"},
                {"alias":"Total","color":"#890f02","fill":0,"legend":false,"dashes":true}]"##,
        )
        .unwrap();
        let mut ignored = Vec::new();
        let overrides = parse_legacy(&raw, false, &mut ignored);
        assert!(style(&overrides, "eth0 out").negative_y);
        let total = style(&overrides, "Total");
        assert!(total.color.is_some() && total.hidden_in_legend);
        assert_eq!(total.fill_opacity, Some(0.));
        assert_eq!(style(&overrides, "Total RAM"), SeriesStyle::default());
        assert!(ignored.is_empty());
        assert_eq!(total.line_style, Some(LineStyle::Dashed(vec![10., 10.])));
    }

    #[test]
    fn notes_what_is_not_drawn_once() {
        let (overrides, ignored) = parsed(
            r#"[{"matcher":{"id":"byName","options":"a"},"properties":[{"id":"custom.lineStyle","value":{"fill":"dash"}}]},
                {"matcher":{"id":"byName","options":"b"},"properties":[{"id":"custom.lineStyle","value":{"fill":"dash"}}]},
                {"matcher":{"id":"byType","options":"unsupported"},"properties":[]}]"#,
        );
        assert_eq!(overrides.len(), 2);
        assert_eq!(ignored, vec!["override matcher byType".to_owned()]);
    }

    #[test]
    fn value_matchers_evaluate_data_and_boolean_reducers_ignore_comparison() {
        let (overrides, ignored) = parsed(
            r#"[
            {"matcher":{"id":"byValue","options":{"reducer":"allIsNull","op":"gte","value":0}},"properties":[{"id":"custom.hideFrom","value":{"viz":true,"legend":true}}]},
            {"matcher":{"id":"byValue","options":{"reducer":"allIsZero","op":"gte","value":0}},"properties":[{"id":"custom.hideFrom","value":{"viz":true}}]},
            {"matcher":{"id":"byValue","options":{"reducer":"max","op":"gt","value":10}},"properties":[{"id":"color","value":{"mode":"fixed","fixedColor":"red"}}]}
        ]"#,
        );
        assert!(ignored.is_empty());
        assert!(
            !style(&overrides, "empty").hidden,
            "value matchers need data"
        );
        assert!(style_with_values(&overrides, "empty", Some(&[])).hidden);
        assert!(style_with_values(&overrides, "null", Some(&[f64::NAN, f64::NAN])).hidden);
        assert!(style_with_values(&overrides, "zero", Some(&[0., f64::NAN, 0.])).hidden);
        assert!(!style_with_values(&overrides, "nonzero", Some(&[0., 1.])).hidden);
        assert_eq!(
            style_with_values(&overrides, "high", Some(&[5., 11.])).color,
            color::parse("red")
        );
    }

    #[test]
    fn legacy_and_modern_points_and_dash_patterns() {
        let (overrides, ignored) = parsed(
            r#"[{"matcher":{"id":"byName","options":"x"},"properties":[
            {"id":"custom.lineStyle","value":{"fill":"dash","dash":[6,4]}},
            {"id":"custom.showPoints","value":"always"},
            {"id":"custom.drawStyle","value":"points"}
        ]}]"#,
        );
        assert!(ignored.is_empty());
        let x = style(&overrides, "x");
        assert_eq!(x.line_style, Some(LineStyle::Dashed(vec![6., 4.])));
        assert_eq!(x.draw, Some(DrawStyle::Points));
        assert_eq!(x.show_points, Some(true));
        let raw = serde_json::from_str::<Vec<Value>>(r#"[{"alias":"x","lines":false,"points":true,"yaxis":2,"dashes":true,"dashLength":3,"spaceLength":7,"pointradius":2}]"#).unwrap();
        let mut ignored = Vec::new();
        let overrides = parse_legacy(&raw, false, &mut ignored);
        let x = style(&overrides, "x");
        assert_eq!(x.draw, Some(DrawStyle::Points));
        assert_eq!(x.line_style, Some(LineStyle::Dashed(vec![3., 7.])));
        assert_eq!(x.point_size, Some(4.));
        assert!(ignored.is_empty());
        let raw = serde_json::from_str::<Vec<Value>>(
            r#"[{"alias":"rate","bars":true,"lines":false,"yaxis":2}]"#,
        )
        .unwrap();
        let overrides = parse_legacy(&raw, false, &mut ignored);
        assert_eq!(style(&overrides, "rate").draw, Some(DrawStyle::Bars));
    }

    #[test]
    fn invalid_dash_lengths_cannot_stall_path_tessellation() {
        assert_eq!(
            LineStyle::parse(&serde_json::json!({"fill":"dash","dash":[0,-1,6,4]})),
            Some(LineStyle::Dashed(vec![6., 4.]))
        );
        assert_eq!(
            LineStyle::parse(&serde_json::json!({"fill":"dash","dash":[]})),
            Some(LineStyle::Dashed(vec![10., 10.]))
        );
    }
    #[test]
    fn query_and_type_matchers_require_their_field_context() {
        let (overrides, ignored) = parsed(
            r#"[
            {"matcher":{"id":"byFrameRefID","options":"B"},"properties":[{"id":"custom.axisPlacement","value":"right"}]},
            {"matcher":{"id":"byType","options":"string"},"properties":[{"id":"custom.hideFrom.viz","value":true}]}
        ]"#,
        );
        assert!(ignored.is_empty());
        assert_eq!(
            patches_for_field(&overrides, FieldContext::new("same").query("B"))
                .find_map(|p| p.axis),
            Some(AxisPlacement::Right)
        );
        assert_eq!(
            patches_for_field(&overrides, FieldContext::new("same").query("A"))
                .find_map(|p| p.axis),
            None
        );
        assert_eq!(
            patches_for_field(&overrides, FieldContext::new("same")).find_map(|p| p.axis),
            None
        );
        assert!(
            style_for_field(
                &overrides,
                &FieldContext::new("label").kind(FieldType::String)
            )
            .hidden
        );
        assert!(!style_for_field(&overrides, &FieldContext::new("value")).hidden);
    }

    #[test]
    fn later_visibility_overrides_can_unhide_original_and_renamed_fields() {
        let (overrides, ignored) = parsed(
            r#"[
            {"matcher":{"id":"byName","options":"Value #A"},"properties":[{"id":"custom.hideFrom","value":{"viz":true,"legend":true}}]},
            {"matcher":{"id":"byName","options":"Views"},"properties":[{"id":"custom.hideFrom.viz","value":false},{"id":"custom.hideFrom.legend","value":false}]}
        ]"#,
        );
        assert!(ignored.is_empty());
        let hidden = style_for_field(&overrides, &FieldContext::new("Value #A"));
        assert!(hidden.hidden && hidden.hidden_in_legend);
        let visible = style_for_field(&overrides, &FieldContext::new("Views").source("Value #A"));
        assert!(!visible.hidden && !visible.hidden_in_legend);
    }
}
