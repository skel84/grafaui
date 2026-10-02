//! What the app draws for each panel: one [`Viz`] per supported Grafana
//! type, with the options it honours, plus a list of what it ignores.
//!
//! Legacy types are converted here: `graph` becomes a time series,
//! `singlestat` a stat or gauge, `table-old` a table.

pub use crate::chart::{AxisScale, StackMode, Stacking};
pub use crate::overrides::{FieldContext, FieldType};

use serde_json::Value;

use crate::color::{self, Rgba};
use crate::overrides::{self, Override, SeriesStyle};
use crate::schema::{FieldDefaults, RawPanel, Target, Thresholds};
use crate::transform::{self, Transform};
use crate::units;

#[derive(Clone, Debug)]
pub struct PanelSpec {
    /// Index of the panel in [`crate::Dashboard::panels`], unique even when
    /// the JSON repeats or omits ids.
    pub key: usize,
    pub id: Option<u32>,
    /// The `type` from the JSON, as written.
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub viz: Viz,
    pub queries: Vec<Query>,
    /// Request settings retained independently of visualization options.
    pub request: PanelRequest,
    pub field: FieldSpec,
    pub transforms: Vec<Transform>,
    /// Repeated once per value of a variable.
    pub repeat: Option<Repeat>,
    /// Variables set for this panel alone: a repeated copy's value.
    pub scoped: Vec<(String, String)>,
    /// Settings this panel uses that the app doesn't draw. Empty for a
    /// fully supported panel.
    pub ignored: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub variable: String,
    /// Copies go side by side (else stacked).
    pub horizontal: bool,
    pub max_per_row: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Full,
    Partial,
    Placeholder,
}

impl PanelSpec {
    pub fn support(&self) -> Support {
        if matches!(self.viz, Viz::Unsupported) {
            Support::Placeholder
        } else if self.ignored.is_empty() {
            Support::Full
        } else {
            Support::Partial
        }
    }
}

#[derive(Clone, Debug)]
pub struct Query {
    pub ref_id: String,
    pub text: Option<String>,
    pub legend: Option<String>,
    pub request: TargetRequest,
}

#[derive(Clone, Debug, Default)]
pub struct TargetRequest {
    pub expr: Option<String>,
    pub instant: Option<bool>,
    pub range: Option<bool>,
    pub format: Option<String>,
    pub interval: Option<String>,
    pub interval_factor: Option<f64>,
    pub step: Option<f64>,
    pub datasource: Value,
    pub exemplar: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PanelRequest {
    pub interval: Option<String>,
    pub max_data_points: Option<usize>,
    pub time_from: Option<String>,
    pub time_shift: Option<String>,
    pub datasource: Value,
}

#[derive(Clone, Debug)]
pub enum Viz {
    TimeSeries(TimeSeriesOptions),
    Stat(StatOptions),
    Gauge(GaugeOptions),
    BarGauge(BarGaugeOptions),
    Pie(PieOptions),
    BarChart(BarChartOptions),
    Histogram,
    Heatmap(crate::heatmap::Options),
    Geomap(crate::geomap::Options),
    Table,
    Text(TextOptions),
    Candlestick,
    /// Drawn as a placeholder card.
    Unsupported,
}

/// Categorical bars, using Kit's vertical or horizontal alignment.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct BarChartOptions {
    pub horizontal: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawStyle {
    Line,
    Bars,
    Points,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    Smooth,
    Step,
}

#[derive(Clone, Debug)]
pub struct TimeSeriesOptions {
    pub draw: DrawStyle,
    pub curve: Curve,
    /// 0–1.
    pub fill_opacity: f32,
    pub gradient: GradientMode,
    pub stacking: Stacking,
    pub show_points: bool,
    pub legend: bool,
    /// Values the legend shows per series, as a table; empty for a plain
    /// list.
    pub legend_calcs: Vec<Calc>,
    /// Legacy graph limits; modern limits come from each field's steps.
    pub thresholds: Vec<GraphThreshold>,
    pub right_axis: Option<AxisSpec>,
    pub line_style: LineStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientMode {
    #[default]
    None,
    Opacity,
    Hue,
    Scheme,
}

impl GradientMode {
    pub(crate) fn parse(mode: &str) -> Option<Self> {
        Some(match mode {
            "none" => Self::None,
            "opacity" => Self::Opacity,
            "hue" => Self::Hue,
            "scheme" => Self::Scheme,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed(Vec<f32>),
}

impl LineStyle {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        Some(match str_at(value, &["fill"]).unwrap_or("solid") {
            "solid" => Self::Solid,
            "dash" => Self::Dashed(
                value
                    .get("dash")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_f64)
                            .filter(|v| v.is_finite() && *v > 0.)
                            .map(|v| v.clamp(0.5, 1000.) as f32)
                            .collect::<Vec<_>>()
                    })
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| vec![10., 10.]),
            ),
            "dot" => Self::Dashed(vec![1., 3.]),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AxisPlacement {
    #[default]
    Auto,
    Left,
    Right,
    Hidden,
}

impl AxisPlacement {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auto" => Self::Auto,
            "left" => Self::Left,
            "right" => Self::Right,
            "hidden" => Self::Hidden,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AxisSpec {
    pub unit: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub visible: bool,
    pub scale: AxisScale,
}

impl AxisSpec {
    pub fn new(unit: Option<String>) -> Self {
        Self {
            unit,
            min: None,
            max: None,
            visible: true,
            scale: AxisScale::Linear,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThresholdStyle {
    #[default]
    Off,
    Line,
    Area,
    LineAndArea,
    Dashed,
    DashedAndArea,
}

impl ThresholdStyle {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "off" => Self::Off,
            "line" => Self::Line,
            "area" => Self::Area,
            "line+area" => Self::LineAndArea,
            "dashed" => Self::Dashed,
            "dashed+area" => Self::DashedAndArea,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GraphThreshold {
    pub value: f64,
    /// The far edge of a band, or infinity at the end of the scale.
    pub end: f64,
    pub line: Option<Rgba>,
    pub fill: Option<Rgba>,
    pub right_axis: bool,
}

impl GraphThreshold {
    pub fn new(value: f64, end: f64) -> Self {
        Self {
            value,
            end,
            line: None,
            fill: None,
            right_axis: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CellDisplay {
    #[default]
    Auto,
    ColorText,
    ColorBackground {
        gradient: bool,
    },
    Gauge {
        mode: CellGauge,
        value: GaugeValue,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellGauge {
    Basic,
    Gradient,
    Lcd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaugeValue {
    Text,
    Color,
    Hidden,
}

impl CellDisplay {
    pub(crate) fn parse(raw: &Value) -> Option<Self> {
        let kind = raw.as_str().or_else(|| str_at(raw, &["type"]))?;
        Some(match kind {
            "auto" => Self::Auto,
            "color-text" => Self::ColorText,
            "color-background" => Self::ColorBackground {
                gradient: str_at(raw, &["mode"]) == Some("gradient"),
            },
            "basic" | "gradient-gauge" | "lcd-gauge" | "gauge" => Self::Gauge {
                mode: match (kind, str_at(raw, &["mode"])) {
                    ("gradient-gauge", _) | (_, Some("gradient")) => CellGauge::Gradient,
                    ("lcd-gauge", _) | (_, Some("lcd")) => CellGauge::Lcd,
                    _ => CellGauge::Basic,
                },
                value: match str_at(raw, &["valueDisplayMode"]) {
                    Some("hidden") => GaugeValue::Hidden,
                    Some("color") => GaugeValue::Color,
                    _ => GaugeValue::Text,
                },
            },
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calc {
    Last,
    LastNotNull,
    First,
    Mean,
    Max,
    Min,
    Sum,
    Count,
    Range,
    Delta,
}

impl Calc {
    /// The column heading Grafana's legend table gives this calculation.
    pub fn label(self) -> &'static str {
        match self {
            Self::Last => "Last",
            Self::LastNotNull => "Last *",
            Self::First => "First",
            Self::Mean => "Mean",
            Self::Max => "Max",
            Self::Min => "Min",
            Self::Sum => "Total",
            Self::Count => "Count",
            Self::Range => "Range",
            Self::Delta => "Delta",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "last" | "current" => Self::Last,
            "lastNotNull" => Self::LastNotNull,
            "first" | "firstNotNull" => Self::First,
            "mean" | "avg" => Self::Mean,
            "max" => Self::Max,
            "min" => Self::Min,
            "sum" | "total" => Self::Sum,
            "count" => Self::Count,
            "range" => Self::Range,
            "delta" | "diff" => Self::Delta,
            _ => return None,
        })
    }

    pub fn reduce(self, values: &[f64]) -> f64 {
        let finite = || values.iter().copied().filter(|v| v.is_finite());
        let result = match self {
            Self::Last => values.last().copied(),
            Self::LastNotNull => finite().next_back(),
            Self::First => finite().next(),
            Self::Mean => {
                let count = finite().count();
                (count > 0).then(|| finite().sum::<f64>() / count as f64)
            }
            Self::Max => finite().reduce(f64::max),
            Self::Min => finite().reduce(f64::min),
            Self::Sum => finite().reduce(|a, b| a + b),
            Self::Count => Some(finite().count() as f64),
            Self::Range => finite()
                .reduce(f64::max)
                .zip(finite().reduce(f64::min))
                .map(|(max, min)| max - min),
            Self::Delta => finite()
                .next_back()
                .zip(finite().next())
                .map(|(l, f)| l - f),
        };
        result.unwrap_or(f64::NAN)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatColor {
    Value,
    Background,
    None,
}

#[derive(Clone, Debug)]
pub struct StatOptions {
    pub calc: Calc,
    pub color: StatColor,
    pub sparkline: bool,
}

#[derive(Clone, Debug)]
pub struct GaugeOptions {
    pub calc: Calc,
}

#[derive(Clone, Debug)]
pub struct BarGaugeOptions {
    pub calc: Calc,
    pub vertical: bool,
    pub display: CellGauge,
    pub show_unfilled: bool,
}

#[derive(Clone, Debug)]
pub struct PieOptions {
    pub calc: Calc,
    pub donut: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMode {
    Markdown,
    Html,
    Code,
}

#[derive(Clone, Debug)]
pub struct TextOptions {
    pub mode: TextMode,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// Series take the classic palette in order.
    Palette,
    /// Every series takes the fixed color.
    Fixed(Rgba),
    /// Values take the color of the threshold step they reach.
    Thresholds,
    Continuous(color::ColorScheme),
}

impl ColorMode {
    pub(crate) fn parse(mode: &str, fixed: Option<&str>) -> Option<Self> {
        Some(match mode {
            "fixed" | "shades" => Self::Fixed(fixed.and_then(color::parse).unwrap_or(color::GREEN)),
            "thresholds" => Self::Thresholds,
            "palette-classic" | "palette-classic-by-name" => Self::Palette,
            other => Self::Continuous(color::ColorScheme::parse(other)?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    /// `f64::NEG_INFINITY` for the base step.
    pub value: f64,
    pub color: Rgba,
}

#[derive(Clone, Debug)]
pub struct FieldSpec {
    pub unit: Option<String>,
    pub decimals: Option<u32>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Ascending by value; never empty.
    pub steps: Vec<Step>,
    /// Steps are percentages of min–max rather than absolute values.
    pub percent_steps: bool,
    pub color: ColorMode,
    pub color_calc: Calc,
    pub mappings: Vec<Mapping>,
    pub overrides: Vec<Override>,
    pub axis: AxisPlacement,
    pub axis_visible: bool,
    pub scale: AxisScale,
    pub threshold_style: ThresholdStyle,
    pub cell_display: CellDisplay,
}

/// A value mapping: values it matches show its text (and color) instead.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    pub matches: MappingMatch,
    pub text: Option<String>,
    pub color: Option<Rgba>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MappingMatch {
    Value(f64),
    Range(Option<f64>, Option<f64>),
    /// No value: null or NaN.
    Missing,
}

impl Mapping {
    fn matches(&self, value: f64) -> bool {
        match self.matches {
            MappingMatch::Value(v) => value == v,
            MappingMatch::Range(from, to) => {
                !value.is_nan() && from.is_none_or(|f| value >= f) && to.is_none_or(|t| value <= t)
            }
            MappingMatch::Missing => value.is_nan(),
        }
    }
}

impl FieldSpec {
    /// The value as an axis or tooltip shows it.
    pub fn format(&self, value: f64) -> String {
        units::format(value, self.unit.as_deref(), self.decimals)
    }

    /// The value as a stat, gauge or table cell shows it: mapped text when a
    /// mapping matches, else [`Self::format`].
    pub fn display(&self, value: f64) -> String {
        self.mapping(value)
            .and_then(|m| m.text.clone())
            .unwrap_or_else(|| self.format(value))
    }

    fn mapping(&self, value: f64) -> Option<&Mapping> {
        self.mappings.iter().find(|m| m.matches(value))
    }

    /// How series `name` is drawn, from the overrides that match it.
    pub fn style(&self, name: &str) -> SeriesStyle {
        overrides::style(&self.overrides, name)
    }

    pub fn style_with_values(&self, name: &str, values: &[f64]) -> SeriesStyle {
        overrides::style_with_values(&self.overrides, name, Some(values))
    }

    pub fn style_for_field(&self, field: &FieldContext<'_>) -> SeriesStyle {
        overrides::style_for_field(&self.overrides, field)
    }

    pub fn for_field(&self, field: &FieldContext<'_>) -> std::borrow::Cow<'_, FieldSpec> {
        self.with_patches(overrides::patches_for_field(&self.overrides, *field).collect())
    }

    /// The field config of series (or table column) `name`: this one with
    /// the unit, decimals, range, thresholds and mappings its overrides set.
    pub fn for_series(&self, name: &str) -> std::borrow::Cow<'_, FieldSpec> {
        self.for_values(name, None)
    }

    pub fn for_series_with_values(
        &self,
        name: &str,
        values: &[f64],
    ) -> std::borrow::Cow<'_, FieldSpec> {
        self.for_values(name, Some(values))
    }

    fn for_values(&self, name: &str, values: Option<&[f64]>) -> std::borrow::Cow<'_, FieldSpec> {
        let patches: Vec<_> =
            overrides::patches_with_values(&self.overrides, name, values).collect();
        self.with_patches(patches)
    }

    /// Resolve both identities of a renamed table column in override order.
    pub fn for_table_column(
        &self,
        shown: &str,
        source: &str,
        values: Option<&[f64]>,
    ) -> std::borrow::Cow<'_, FieldSpec> {
        self.with_patches(
            overrides::column_patches(&self.overrides, shown, source, values).collect(),
        )
    }

    fn with_patches(
        &self,
        patches: Vec<&overrides::FieldPatch>,
    ) -> std::borrow::Cow<'_, FieldSpec> {
        if patches.is_empty() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut field = self.clone();
        for patch in patches {
            if patch.unit.is_some() {
                field.unit.clone_from(&patch.unit);
            }
            field.decimals = patch.decimals.or(field.decimals);
            field.min = patch.min.or(field.min);
            field.max = patch.max.or(field.max);
            field.axis = patch.axis.unwrap_or(field.axis);
            field.scale = patch.scale.unwrap_or(field.scale);
            field.threshold_style = patch.threshold_style.unwrap_or(field.threshold_style);
            field.cell_display = patch.cell_display.unwrap_or(field.cell_display);
            field.color = patch.color.unwrap_or(field.color);
            field.color_calc = patch.color_calc.unwrap_or(field.color_calc);
            if let Some((steps, percent)) = &patch.steps {
                field.steps.clone_from(steps);
                field.percent_steps = *percent;
                if field.color == ColorMode::Palette {
                    field.color = ColorMode::Thresholds;
                }
            }
            if let Some(mappings) = &patch.mappings {
                field.mappings.clone_from(mappings);
            }
        }
        std::borrow::Cow::Owned(field)
    }

    /// A legacy graph's second axis supplies its series' unit and bounds.
    pub fn for_time_series(&self, name: &str, options: &TimeSeriesOptions) -> FieldSpec {
        self.time_series_field(name, None, options)
    }

    pub fn for_time_series_with_values(
        &self,
        name: &str,
        values: &[f64],
        options: &TimeSeriesOptions,
    ) -> FieldSpec {
        self.time_series_field(name, Some(values), options)
    }

    fn time_series_field(
        &self,
        name: &str,
        values: Option<&[f64]>,
        options: &TimeSeriesOptions,
    ) -> FieldSpec {
        let context = FieldContext::new(name);
        let context = values.map_or(context, |v| context.values(v));
        self.for_time_series_field(&context, options)
    }

    pub fn for_time_series_field(
        &self,
        context: &FieldContext<'_>,
        options: &TimeSeriesOptions,
    ) -> FieldSpec {
        let mut field = self.for_field(context).into_owned();
        if field.axis == AxisPlacement::Right
            && let Some(axis) = &options.right_axis
        {
            field.unit.clone_from(&axis.unit);
            field.min = axis.min;
            field.max = axis.max;
            field.axis_visible = axis.visible;
            field.scale = axis.scale;
        }
        field
    }

    /// The color of the threshold step `value` reaches.
    pub fn threshold_color(&self, value: f64) -> Rgba {
        self.threshold_color_in_range(value, (0., 100.))
    }

    pub fn threshold_color_in_range(&self, value: f64, range: (f64, f64)) -> Rgba {
        let value = if self.percent_steps {
            let (min, max) = self.color_range(range);
            if max > min {
                (value - min) / (max - min) * 100.
            } else {
                value
            }
        } else {
            value
        };
        self.steps
            .iter()
            .rev()
            .find(|step| value >= step.value)
            .unwrap_or(&self.steps[0])
            .color
    }

    /// The color of series `index`, whose reduced value is `value`.
    pub fn series_color(&self, index: usize, value: f64) -> Rgba {
        self.series_color_in_range(index, value, (0., 100.))
    }

    /// Explicit bounds take priority over the range of the actual data.
    pub fn color_range(&self, fallback: (f64, f64)) -> (f64, f64) {
        let min = self.min.filter(|v| v.is_finite()).unwrap_or(fallback.0);
        let max = self.max.filter(|v| v.is_finite()).unwrap_or(fallback.1);
        (min, if max > min { max } else { min + 1. })
    }

    pub fn series_color_in_range(&self, index: usize, value: f64, range: (f64, f64)) -> Rgba {
        if let Some(color) = self.mapping(value).and_then(|m| m.color) {
            return color;
        }
        match self.color {
            ColorMode::Palette => color::classic(index),
            ColorMode::Fixed(color) => color,
            ColorMode::Thresholds => self.threshold_color_in_range(value, range),
            ColorMode::Continuous(scheme) => {
                let (min, max) = self.color_range(range);
                scheme.sample((value - min) / (max - min))
            }
        }
    }

    /// Color stops along a value scale. Thresholds have hard transitions;
    /// palettes interpolate between these stops.
    pub fn gradient_stops(&self, range: (f64, f64)) -> Vec<(f64, Rgba)> {
        let (min, max) = self.color_range(range);
        match self.color {
            ColorMode::Thresholds => self
                .steps
                .iter()
                .map(|step| {
                    let value = if self.percent_steps && step.value.is_finite() {
                        min + (max - min) * step.value / 100.
                    } else {
                        step.value
                    };
                    (value, step.color)
                })
                .collect(),
            ColorMode::Fixed(color) => vec![(min, color), (max, color)],
            mode => {
                let colors = match mode {
                    ColorMode::Continuous(scheme) => scheme.colors(),
                    _ => &color::CLASSIC,
                };
                colors
                    .iter()
                    .enumerate()
                    .map(|(i, &color)| {
                        (
                            min + (max - min) * i as f64 / (colors.len() - 1) as f64,
                            color,
                        )
                    })
                    .collect()
            }
        }
    }
}

/// Normalizes one non-row panel.
pub(crate) fn panel(raw: &RawPanel, key: usize) -> PanelSpec {
    let mut ignored = Vec::new();
    let mut field = field_spec(&raw.field_config.defaults, raw.kind.as_str(), &mut ignored);
    field.overrides = overrides::parse(&raw.field_config.overrides, &mut ignored);
    let viz = viz(raw, &mut field, &mut ignored);
    let transforms = transform::parse(
        &raw.transformations,
        matches!(viz, Viz::Table),
        &mut ignored,
    );
    if matches!(viz, Viz::Unsupported) {
        ignored.clear();
    }
    PanelSpec {
        key,
        id: raw.id,
        kind: raw.kind.clone(),
        title: raw.title.clone(),
        description: raw.description.clone().filter(|d| !d.trim().is_empty()),
        viz,
        queries: raw
            .targets
            .iter()
            .enumerate()
            .filter(|(_, target)| !target.hide)
            .map(|(index, target)| query(target, index))
            .collect(),
        request: PanelRequest {
            interval: raw
                .extra
                .get("interval")
                .and_then(Value::as_str)
                .map(str::to_owned),
            max_data_points: raw
                .extra
                .get("maxDataPoints")
                .and_then(Value::as_u64)
                .map(|n| n as usize),
            time_from: raw
                .extra
                .get("timeFrom")
                .and_then(Value::as_str)
                .map(str::to_owned),
            time_shift: raw
                .extra
                .get("timeShift")
                .and_then(Value::as_str)
                .map(str::to_owned),
            datasource: raw.extra.get("datasource").cloned().unwrap_or_default(),
        },
        field,
        transforms,
        repeat: raw
            .repeat
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(|variable| {
                let extra = raw.extra_value();
                Repeat {
                    variable: variable.to_owned(),
                    horizontal: str_at(&extra, &["repeatDirection"]) != Some("v"),
                    max_per_row: num_at(&extra, &["maxPerRow"]).map(|n| n as u32),
                }
            }),
        scoped: Vec::new(),
        ignored,
    }
}

fn query(target: &Target, index: usize) -> Query {
    Query {
        ref_id: target
            .ref_id
            .clone()
            .unwrap_or_else(|| ((b'A' + (index % 26) as u8) as char).to_string()),
        text: target.query(),
        legend: target
            .legend_format
            .clone()
            .filter(|l| !l.is_empty() && l != "__auto")
            .or_else(|| target.cloudwatch_legend()),
        request: TargetRequest {
            expr: target.expr.clone(),
            instant: target.extra.get("instant").and_then(Value::as_bool),
            range: target.extra.get("range").and_then(Value::as_bool),
            format: target
                .extra
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_owned),
            interval: target
                .extra
                .get("interval")
                .and_then(Value::as_str)
                .map(str::to_owned),
            interval_factor: target.extra.get("intervalFactor").and_then(Value::as_f64),
            step: target.extra.get("step").and_then(Value::as_f64),
            datasource: target.extra.get("datasource").cloned().unwrap_or_default(),
            exemplar: target
                .extra
                .get("exemplar")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    }
}

fn viz(raw: &RawPanel, field: &mut FieldSpec, ignored: &mut Vec<String>) -> Viz {
    let options = &raw.options;
    let custom = &raw.field_config.defaults.custom;
    match raw.kind.as_str() {
        "timeseries" | "trend" => Viz::TimeSeries(time_series(options, custom, ignored)),
        "graph" => Viz::TimeSeries(legacy_graph(raw, field, ignored)),
        "stat" => Viz::Stat(StatOptions {
            calc: calc(options),
            color: match str_at(options, &["colorMode"]) {
                Some("background" | "background_solid") => StatColor::Background,
                Some("none") => StatColor::None,
                _ => StatColor::Value,
            },
            sparkline: str_at(options, &["graphMode"]) != Some("none"),
        }),
        "singlestat" => legacy_singlestat(raw, field),
        "gauge" => Viz::Gauge(GaugeOptions {
            calc: calc(options),
        }),
        "bargauge" => Viz::BarGauge(BarGaugeOptions {
            calc: calc(options),
            vertical: str_at(options, &["orientation"]) == Some("vertical"),
            display: match str_at(options, &["displayMode"]) {
                Some("lcd") => CellGauge::Lcd,
                Some("gradient") => CellGauge::Gradient,
                _ => CellGauge::Basic,
            },
            show_unfilled: bool_at(options, &["showUnfilled"]).unwrap_or(true),
        }),
        "piechart" | "grafana-piechart-panel" => Viz::Pie(PieOptions {
            calc: calc(options),
            donut: str_at(options, &["pieType"]) == Some("donut")
                || str_at(&raw.extra_value(), &["pieType"]) == Some("donut"),
        }),
        "barchart" => {
            if str_at(options, &["colorByField"]).is_some_and(|name| !name.is_empty()) {
                ignored.push("barchart colorByField".into());
            }
            Viz::BarChart(BarChartOptions {
                horizontal: str_at(options, &["orientation"]) == Some("horizontal"),
            })
        }
        "histogram" => Viz::Histogram,
        "heatmap" => Viz::Heatmap(crate::heatmap::parse(raw, ignored)),
        "geomap" => Viz::Geomap(crate::geomap::parse(&raw.options, ignored)),
        "table" | "table-old" => Viz::Table,
        "text" => text(raw),
        "candlestick" => Viz::Candlestick,
        _ => Viz::Unsupported,
    }
}

fn time_series(options: &Value, custom: &Value, _ignored: &mut Vec<String>) -> TimeSeriesOptions {
    let stacking = custom
        .get("stacking")
        .and_then(Stacking::parse)
        .unwrap_or_default();
    TimeSeriesOptions {
        draw: match str_at(custom, &["drawStyle"]) {
            Some("bars") => DrawStyle::Bars,
            Some("points") => DrawStyle::Points,
            _ => DrawStyle::Line,
        },
        curve: match str_at(custom, &["lineInterpolation"]) {
            Some("smooth") => Curve::Smooth,
            Some("stepBefore" | "stepAfter") => Curve::Step,
            _ => Curve::Linear,
        },
        fill_opacity: (num_at(custom, &["fillOpacity"]).unwrap_or(0.) / 100.).clamp(0., 1.) as f32,
        gradient: str_at(custom, &["gradientMode"])
            .and_then(GradientMode::parse)
            .unwrap_or_default(),
        stacking,
        show_points: str_at(custom, &["showPoints"]) == Some("always"),
        legend: legend_shown(options),
        legend_calcs: legend_calcs(options),
        thresholds: Vec::new(),
        right_axis: None,
        line_style: custom
            .get("lineStyle")
            .and_then(LineStyle::parse)
            .unwrap_or_default(),
    }
}

fn legend_shown(options: &Value) -> bool {
    bool_at(options, &["legend", "showLegend"]).unwrap_or(true)
        && str_at(options, &["legend", "displayMode"]) != Some("hidden")
}

fn legend_calcs(options: &Value) -> Vec<Calc> {
    options
        .pointer("/legend/calcs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| Calc::parse(c.as_str()?))
        .collect()
}

fn legacy_graph(
    raw: &RawPanel,
    field: &mut FieldSpec,
    ignored: &mut Vec<String>,
) -> TimeSeriesOptions {
    let extra = raw.extra_value();
    if field.unit.is_none() {
        field.unit = extra
            .pointer("/yaxes/0/format")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    field.min = field
        .min
        .or_else(|| legacy_number(extra.pointer("/yaxes/0/min")));
    field.max = field
        .max
        .or_else(|| legacy_number(extra.pointer("/yaxes/0/max")));
    field.scale = legacy_number(extra.pointer("/yaxes/0/logBase"))
        .filter(|n| *n > 1.)
        .map_or(AxisScale::Linear, AxisScale::Log);
    field.axis_visible = extra
        .pointer("/yaxes/0/show")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if let Some(raw) = extra.get("seriesOverrides").and_then(Value::as_array) {
        let stacked = bool_at(&extra, &["stack"]) == Some(true);
        field
            .overrides
            .extend(overrides::parse_legacy(raw, stacked, ignored));
    }
    let mut thresholds: Vec<GraphThreshold> = extra
        .get("thresholds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let value = legacy_number(t.get("value"))?;
            let color = match str_at(t, &["colorMode"]) {
                Some("warning") => color::parse("yellow").unwrap(),
                Some("ok") => color::GREEN,
                _ => color::parse("red").unwrap(),
            };
            Some(GraphThreshold {
                value,
                end: if str_at(t, &["op"]) == Some("lt") {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                },
                line: (bool_at(t, &["line"]) == Some(true)).then(|| {
                    str_at(t, &["lineColor"])
                        .and_then(color::parse)
                        .unwrap_or(color)
                }),
                fill: (bool_at(t, &["fill"]) == Some(true)).then(|| {
                    str_at(t, &["fillColor"])
                        .and_then(color::parse)
                        .unwrap_or(Rgba((color.0 & 0xffffff00) | 40))
                }),
                right_axis: str_at(t, &["yaxis"]) == Some("right"),
            })
        })
        .collect();
    thresholds.sort_by(|a, b| a.value.total_cmp(&b.value));
    let above: Vec<_> = thresholds
        .iter()
        .map(|t| t.end.is_sign_positive())
        .collect();
    // Adjacent limits of the same direction form bands rather than
    // repeatedly tinting the same region.
    for i in 0..thresholds.len() {
        let axis = thresholds[i].right_axis;
        let neighbor = if above[i] {
            (i + 1..thresholds.len()).find(|&j| thresholds[j].right_axis == axis && above[j])
        } else {
            (0..i)
                .rev()
                .find(|&j| thresholds[j].right_axis == axis && !above[j])
        };
        if let Some(j) = neighbor {
            thresholds[i].end = thresholds[j].value;
        }
    }
    // The old legend's value columns, in Grafana's order.
    let legend_calcs = if bool_at(&extra, &["legend", "values"]) == Some(true) {
        [
            ("min", Calc::Min),
            ("max", Calc::Max),
            ("avg", Calc::Mean),
            ("current", Calc::Last),
            ("total", Calc::Sum),
        ]
        .into_iter()
        .filter(|(key, _)| bool_at(&extra, &["legend", key]) == Some(true))
        .map(|(_, calc)| calc)
        .collect()
    } else {
        Vec::new()
    };
    let bars = bool_at(&extra, &["bars"]) == Some(true);
    let lines = bool_at(&extra, &["lines"]).unwrap_or(true);
    let points = bool_at(&extra, &["points"]) == Some(true);
    TimeSeriesOptions {
        draw: if bars {
            DrawStyle::Bars
        } else if !lines && points {
            DrawStyle::Points
        } else {
            DrawStyle::Line
        },
        curve: if bool_at(&extra, &["steppedLine"]) == Some(true) {
            Curve::Step
        } else {
            Curve::Linear
        },
        fill_opacity: (num_at(&extra, &["fill"]).unwrap_or(1.) / 10.).clamp(0., 1.) as f32,
        gradient: if num_at(&extra, &["fillGradient"]).is_some_and(|g| g > 0.) {
            GradientMode::Opacity
        } else {
            GradientMode::None
        },
        stacking: Stacking::new(
            if bool_at(&extra, &["stack"]) != Some(true) {
                StackMode::None
            } else if bool_at(&extra, &["percentage"]) == Some(true) {
                StackMode::Percent
            } else {
                StackMode::Normal
            },
            "A",
        ),
        show_points: points,
        legend: bool_at(&extra, &["legend", "show"]).unwrap_or(true),
        legend_calcs,
        thresholds,
        right_axis: extra.pointer("/yaxes/1").map(|axis| AxisSpec {
            unit: str_at(axis, &["format"]).map(str::to_owned),
            min: legacy_number(axis.get("min")),
            max: legacy_number(axis.get("max")),
            visible: bool_at(axis, &["show"]).unwrap_or(true),
            scale: legacy_number(axis.get("logBase"))
                .filter(|n| *n > 1.)
                .map_or(AxisScale::Linear, AxisScale::Log),
        }),
        line_style: if bool_at(&extra, &["dashes"]) == Some(true) {
            LineStyle::Dashed(vec![
                num_at(&extra, &["dashLength"]).unwrap_or(10.).max(0.5) as f32,
                num_at(&extra, &["spaceLength"]).unwrap_or(10.).max(0.5) as f32,
            ])
        } else {
            LineStyle::Solid
        },
    }
}

fn legacy_singlestat(raw: &RawPanel, field: &mut FieldSpec) -> Viz {
    let extra = raw.extra_value();
    if field.unit.is_none() {
        field.unit = str_at(&extra, &["format"]).map(str::to_owned);
    }
    let thresholds: Vec<f64> = str_at(&extra, &["thresholds"])
        .unwrap_or_default()
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    let colors: Vec<Rgba> = extra
        .get("colors")
        .and_then(Value::as_array)
        .map(|colors| {
            colors
                .iter()
                .filter_map(|c| color::parse(c.as_str()?))
                .collect()
        })
        .unwrap_or_default();
    if !thresholds.is_empty() && colors.len() > thresholds.len() {
        field.steps = std::iter::once(f64::NEG_INFINITY)
            .chain(thresholds)
            .zip(colors)
            .map(|(value, color)| Step { value, color })
            .collect();
        field.color = ColorMode::Thresholds;
    }
    let calc = str_at(&extra, &["valueName"])
        .and_then(Calc::parse)
        .unwrap_or(Calc::Mean);
    if bool_at(&extra, &["gauge", "show"]) == Some(true) {
        field.min = field.min.or(num_at(&extra, &["gauge", "minValue"]));
        field.max = field.max.or(num_at(&extra, &["gauge", "maxValue"]));
        return Viz::Gauge(GaugeOptions { calc });
    }
    Viz::Stat(StatOptions {
        calc,
        color: if bool_at(&extra, &["colorBackground"]) == Some(true) {
            StatColor::Background
        } else if bool_at(&extra, &["colorValue"]) == Some(true) {
            StatColor::Value
        } else {
            StatColor::None
        },
        sparkline: bool_at(&extra, &["sparkline", "show"]) == Some(true),
    })
}

fn text(raw: &RawPanel) -> Viz {
    let extra = raw.extra_value();
    let mode = str_at(&raw.options, &["mode"])
        .or_else(|| str_at(&extra, &["mode"]))
        .unwrap_or("markdown");
    let content = str_at(&raw.options, &["content"])
        .or_else(|| str_at(&extra, &["content"]))
        .unwrap_or_default()
        .to_owned();
    let mode = match mode {
        "html" => TextMode::Html,
        "code" => TextMode::Code,
        _ => TextMode::Markdown,
    };
    Viz::Text(TextOptions { mode, content })
}

fn field_spec(defaults: &FieldDefaults, kind: &str, ignored: &mut Vec<String>) -> FieldSpec {
    let (steps, percent_steps) = steps(defaults.thresholds.as_ref());
    let mode = defaults.color.as_ref().and_then(|c| c.mode.as_deref());
    // Grafana colors stats and gauges by threshold unless told otherwise.
    let by_threshold = matches!(kind, "stat" | "gauge" | "bargauge" | "singlestat");
    let color = match mode {
        Some(mode) => ColorMode::parse(
            mode,
            defaults
                .color
                .as_ref()
                .and_then(|c| c.fixed_color.as_deref()),
        )
        .unwrap_or_else(|| {
            ignored.push(format!("{mode} color scheme"));
            ColorMode::Palette
        }),
        None if by_threshold => ColorMode::Thresholds,
        None => ColorMode::Palette,
    };
    let mappings = mappings(&defaults.mappings, ignored);
    FieldSpec {
        unit: defaults.unit.clone().filter(|u| !u.is_empty()),
        decimals: defaults.decimals,
        min: defaults.min,
        max: defaults.max,
        steps,
        percent_steps,
        color,
        color_calc: defaults
            .color
            .as_ref()
            .and_then(|c| c.series_by.as_deref())
            .and_then(Calc::parse)
            .unwrap_or(Calc::Last),
        mappings,
        overrides: Vec::new(),
        axis: str_at(&defaults.custom, &["axisPlacement"])
            .and_then(AxisPlacement::parse)
            .unwrap_or_default(),
        axis_visible: true,
        scale: AxisScale::parse(
            &defaults
                .custom
                .get("scaleDistribution")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .unwrap_or_else(|| {
            ignored.push("axis scale distribution".into());
            AxisScale::Linear
        }),
        threshold_style: str_at(&defaults.custom, &["thresholdsStyle", "mode"])
            .and_then(ThresholdStyle::parse)
            .unwrap_or_default(),
        cell_display: defaults
            .custom
            .get("cellOptions")
            .or_else(|| defaults.custom.get("displayMode"))
            .and_then(CellDisplay::parse)
            .unwrap_or_default(),
    }
}

fn legacy_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|v| v.is_finite())
}

/// Threshold steps, ascending and never empty, and whether they are
/// percentages.
pub(crate) fn steps(thresholds: Option<&Thresholds>) -> (Vec<Step>, bool) {
    let mut steps: Vec<Step> = thresholds
        .map(|t| {
            t.steps
                .iter()
                .map(|step| Step {
                    value: step.value.unwrap_or(f64::NEG_INFINITY),
                    color: color::parse(&step.color).unwrap_or(color::GREEN),
                })
                .collect()
        })
        .unwrap_or_default();
    steps.sort_by(|a, b| a.value.total_cmp(&b.value));
    if steps.is_empty() {
        steps.push(Step {
            value: f64::NEG_INFINITY,
            color: color::GREEN,
        });
    }
    let percent = thresholds.and_then(|t| t.mode.as_deref()) == Some("percentage");
    (steps, percent)
}

/// Reads value, range and null mappings; others go to `ignored`.
pub(crate) fn mappings(raw: &[Value], ignored: &mut Vec<String>) -> Vec<Mapping> {
    let result = |r: &Value| {
        (
            r.get("text").and_then(Value::as_str).map(str::to_owned),
            r.get("color")
                .and_then(Value::as_str)
                .and_then(color::parse),
        )
    };
    let mut out = Vec::new();
    for mapping in raw {
        let options = mapping.get("options").unwrap_or(&Value::Null);
        match mapping.get("type").and_then(Value::as_str) {
            Some("value") => {
                for (key, r) in options.as_object().into_iter().flatten() {
                    if let Ok(value) = key.parse() {
                        let (text, color) = result(r);
                        out.push(Mapping {
                            matches: MappingMatch::Value(value),
                            text,
                            color,
                        });
                    }
                }
            }
            Some("range") => {
                let (text, color) = result(options.get("result").unwrap_or(&Value::Null));
                let bound = |key| options.get(key).and_then(Value::as_f64);
                out.push(Mapping {
                    matches: MappingMatch::Range(bound("from"), bound("to")),
                    text,
                    color,
                });
            }
            Some("special")
                if matches!(
                    str_at(options, &["match"]),
                    Some("null" | "nan" | "null+nan")
                ) =>
            {
                let (text, color) = result(options.get("result").unwrap_or(&Value::Null));
                out.push(Mapping {
                    matches: MappingMatch::Missing,
                    text,
                    color,
                });
            }
            // Before Grafana 8: type 1 maps a value, type 2 a range, with
            // the numbers saved as strings.
            None if mapping.get("type").and_then(Value::as_u64).is_some() => {
                let number = |key| {
                    let value = mapping.get(key)?;
                    value
                        .as_f64()
                        .or_else(|| value.as_str()?.trim().parse().ok())
                };
                let text = mapping
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let matches = match mapping.get("type").and_then(Value::as_u64) {
                    Some(1) if mapping.get("value").and_then(Value::as_str) == Some("null") => {
                        Some(MappingMatch::Missing)
                    }
                    Some(1) => number("value").map(MappingMatch::Value),
                    Some(2) => Some(MappingMatch::Range(number("from"), number("to"))),
                    _ => None,
                };
                if let Some(matches) = matches {
                    out.push(Mapping {
                        matches,
                        text,
                        color: None,
                    });
                }
            }
            _ => {
                let note = "value mappings by regex or special value".to_owned();
                if !ignored.contains(&note) {
                    ignored.push(note);
                }
            }
        }
    }
    out
}

fn calc(options: &Value) -> Calc {
    options
        .pointer("/reduceOptions/calcs/0")
        .and_then(Value::as_str)
        .and_then(Calc::parse)
        .unwrap_or(Calc::LastNotNull)
}

fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| value.get(*key))
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    at(value, path)?.as_str()
}

fn num_at(value: &Value, path: &[&str]) -> Option<f64> {
    let value = at(value, path)?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    at(value, path)?.as_bool()
}

impl RawPanel {
    fn extra_value(&self) -> Value {
        Value::Object(self.extra.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: &str) -> PanelSpec {
        panel(&serde_json::from_str(json).unwrap(), 0)
    }

    #[test]
    fn reduces() {
        let values = [1., f64::NAN, 4., 3.];
        assert_eq!(Calc::LastNotNull.reduce(&values), 3.);
        assert!(Calc::Last.reduce(&[1., f64::NAN]).is_nan());
        assert_eq!(Calc::Mean.reduce(&values), 8. / 3.);
        assert_eq!(Calc::Range.reduce(&values), 3.);
        assert!(Calc::Max.reduce(&[]).is_nan());
        assert!(Calc::Sum.reduce(&[f64::NAN, f64::NAN]).is_nan());
        assert!(Calc::Sum.reduce(&[]).is_nan());
        assert_eq!(Calc::Sum.reduce(&[0., f64::NAN]), 0.);
    }

    #[test]
    fn color_schemes_and_gradients_parse_and_keep_override_order() {
        for (mode, scheme) in [
            ("continuous-GrYlRd", color::ColorScheme::GreenYellowRed),
            ("continuous-RdYlGr", color::ColorScheme::RedYellowGreen),
            ("continuous-BlPu", color::ColorScheme::BluePurple),
            ("continuous-YlRd", color::ColorScheme::YellowRed),
        ] {
            let p = spec(&format!(
                r#"{{"type":"timeseries","fieldConfig":{{"defaults":{{"min":10,"max":110,"color":{{"mode":"{mode}","seriesBy":"max"}},"custom":{{"gradientMode":"scheme"}}}},"overrides":[
                {{"matcher":{{"id":"byName","options":"a"}},"properties":[{{"id":"color","value":{{"mode":"fixed","fixedColor":"blue"}}}}]}},
                {{"matcher":{{"id":"byName","options":"a"}},"properties":[{{"id":"color","value":{{"mode":"{mode}","seriesBy":"mean"}}}},{{"id":"custom.gradientMode","value":"opacity"}}]}},
                {{"matcher":{{"id":"byName","options":"a"}},"properties":[{{"id":"color","value":{{}}}}]}}
            ]}}}}"#
            ));
            assert_eq!(p.field.color, ColorMode::Continuous(scheme));
            assert_eq!(p.field.color_calc, Calc::Max);
            assert_eq!(p.field.for_series("a").color, ColorMode::Continuous(scheme));
            assert_eq!(p.field.for_series("a").color_calc, Calc::Mean);
            assert_eq!(p.field.style("a").gradient, Some(GradientMode::Opacity));
            assert_eq!(
                p.field.series_color_in_range(0, 60., (0., 1000.)),
                scheme.sample(0.5)
            );
            let Viz::TimeSeries(o) = p.viz else { panic!() };
            assert_eq!(o.gradient, GradientMode::Scheme);
            assert!(p.ignored.is_empty(), "{:?}", p.ignored);
        }
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"defaults":{"color":{"mode":"unknown-scheme"}},"overrides":[{"matcher":{"id":"byName","options":"a"},"properties":[{"id":"color","value":{"mode":"unknown-scheme"}}]}]}}"#,
        );
        assert_eq!(
            p.ignored,
            ["unknown-scheme color scheme", "override color scheme"]
        );
    }

    #[test]
    fn bar_gauge_styles_and_unfilled_tracks_parse() {
        for (mode, display) in [
            ("basic", CellGauge::Basic),
            ("gradient", CellGauge::Gradient),
            ("lcd", CellGauge::Lcd),
        ] {
            let p = spec(&format!(
                r#"{{"type":"bargauge","options":{{"displayMode":"{mode}","showUnfilled":false}}}}"#
            ));
            let Viz::BarGauge(o) = p.viz else { panic!() };
            assert_eq!(o.display, display);
            assert!(!o.show_unfilled);
            assert!(p.ignored.is_empty());
        }
    }

    #[test]
    fn modern_threshold_styles_and_percentage_steps() {
        for mode in ["line", "area", "line+area", "dashed", "dashed+area", "off"] {
            let p = spec(&format!(
                r#"{{"type":"timeseries","fieldConfig":{{"defaults":{{
                "min":10,"max":210,"thresholds":{{"mode":"percentage","steps":[{{"color":"green","value":null}},{{"color":"red","value":80}}]}},
                "custom":{{"thresholdsStyle":{{"mode":"{mode}"}}}}
            }}}}}}"#
            ));
            assert_eq!(
                p.field.threshold_style,
                ThresholdStyle::parse(mode).unwrap()
            );
            assert!(p.field.percent_steps);
            assert_eq!(p.field.steps[1].value, 80.);
            assert!(p.ignored.is_empty(), "{:?}", p.ignored);
        }
    }

    #[test]
    fn legacy_limits_keep_colors_direction_and_axis() {
        let p = spec(
            r#"{"type":"graph","thresholds":[
            {"value":90,"op":"gt","fill":true,"line":true,"colorMode":"critical"},
            {"value":80,"op":"gt","fill":true,"line":true,"fillColor":"rgba(10,20,30,0.2)","lineColor":"blue"},
            {"value":20,"op":"lt","fill":true,"yaxis":"right"},
            {"op":"gt","line":true}
        ]}"#,
        );
        let Viz::TimeSeries(o) = p.viz else { panic!() };
        assert_eq!(o.thresholds.len(), 3);
        assert_eq!(o.thresholds[0].end, f64::NEG_INFINITY);
        assert!(o.thresholds[0].right_axis);
        assert_eq!(o.thresholds[1].value, 80.);
        assert_eq!(o.thresholds[1].end, 90.);
        assert_eq!(o.thresholds[1].line, color::parse("blue"));
        assert_eq!(o.thresholds[1].fill, color::parse("rgba(10,20,30,0.2)"));
        assert_eq!(o.thresholds[2].end, f64::INFINITY);
        assert!(p.ignored.is_empty());
    }

    #[test]
    fn right_axis_defaults_and_overrides_keep_independent_units() {
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"defaults":{"unit":"bytes","custom":{"axisPlacement":"left"}},
            "overrides":[{"matcher":{"id":"byName","options":"load"},"properties":[
                {"id":"custom.axisPlacement","value":"right"},{"id":"unit","value":"percent"},{"id":"max","value":100}
            ]}]}}"#,
        );
        assert_eq!(p.field.axis, AxisPlacement::Left);
        let right = p.field.for_series("load");
        assert_eq!(right.axis, AxisPlacement::Right);
        assert_eq!(right.unit.as_deref(), Some("percent"));
        assert_eq!(right.max, Some(100.));
        assert!(p.ignored.is_empty());
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"defaults":{"custom":{"axisPlacement":"right"}}}}"#,
        );
        assert_eq!(p.field.axis, AxisPlacement::Right);
    }

    #[test]
    fn legacy_axes_accept_string_bounds_and_series_assignment() {
        let p = spec(
            r#"{"type":"graph","yaxes":[{"format":"bytes","min":"5","max":"500"},{"format":"percent","min":0,"max":"100","show":true}],
            "seriesOverrides":[{"alias":"used","yaxis":2}]}"#,
        );
        assert_eq!(p.field.min, Some(5.));
        assert_eq!(p.field.max, Some(500.));
        assert_eq!(p.field.for_series("used").axis, AxisPlacement::Right);
        let Viz::TimeSeries(o) = p.viz else { panic!() };
        let right = o.right_axis.unwrap();
        assert_eq!(right.unit.as_deref(), Some("percent"));
        assert_eq!(right.max, Some(100.));
        assert!(p.ignored.is_empty());
    }

    #[test]
    fn table_cell_modes_parse_defaults_and_column_overrides() {
        let p = spec(
            r#"{"type":"table","fieldConfig":{"defaults":{"custom":{"cellOptions":{"type":"color-background","mode":"gradient"}}},
            "overrides":[
                {"matcher":{"id":"byName","options":"value"},"properties":[{"id":"custom.cellOptions","value":{"type":"gauge","mode":"gradient","valueDisplayMode":"color"}},{"id":"min","value":0},{"id":"max","value":1}]},
                {"matcher":{"id":"byName","options":"status"},"properties":[{"id":"custom.displayMode","value":"color-text"},{"id":"color","value":{"mode":"fixed","fixedColor":"red"}}]},
                {"matcher":{"id":"byName","options":"count"},"properties":[{"id":"custom.displayMode","value":"lcd-gauge"}]}
            ]}}"#,
        );
        assert_eq!(
            p.field.cell_display,
            CellDisplay::ColorBackground { gradient: true }
        );
        let gauge = p.field.for_series("value");
        assert_eq!(
            gauge.cell_display,
            CellDisplay::Gauge {
                mode: CellGauge::Gradient,
                value: GaugeValue::Color
            }
        );
        assert_eq!((gauge.min, gauge.max), (Some(0.), Some(1.)));
        assert_eq!(
            p.field.for_series("status").cell_display,
            CellDisplay::ColorText
        );
        assert_eq!(
            p.field.for_series("status").series_color(0, 42.),
            color::parse("red").unwrap()
        );
        assert_eq!(
            p.field.for_series("count").cell_display,
            CellDisplay::Gauge {
                mode: CellGauge::Lcd,
                value: GaugeValue::Text
            }
        );
        assert!(p.ignored.is_empty());
        assert_eq!(
            CellDisplay::parse(&serde_json::json!("basic")),
            Some(CellDisplay::Gauge {
                mode: CellGauge::Basic,
                value: GaugeValue::Text
            })
        );
        assert_eq!(
            CellDisplay::parse(&serde_json::json!({"type":"gauge","valueDisplayMode":"hidden"})),
            Some(CellDisplay::Gauge {
                mode: CellGauge::Basic,
                value: GaugeValue::Hidden
            })
        );
    }

    #[test]
    fn value_matchers_patch_fields_in_override_order() {
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"overrides":[
            {"matcher":{"id":"byName","options":"x"},"properties":[{"id":"unit","value":"bytes"}]},
            {"matcher":{"id":"byValue","options":{"reducer":"max","op":"gte","value":10}},"properties":[{"id":"unit","value":"percent"},{"id":"custom.axisPlacement","value":"right"}]}
        ]}}"#,
        );
        assert_eq!(
            p.field.for_series_with_values("x", &[9.]).unit.as_deref(),
            Some("bytes")
        );
        let right = p.field.for_series_with_values("x", &[10.]);
        assert_eq!(right.axis, AxisPlacement::Right);
        assert_eq!(right.unit.as_deref(), Some("percent"));
    }

    #[test]
    fn renamed_column_overrides_keep_their_original_order() {
        let p = spec(
            r#"{"type":"table","fieldConfig":{"overrides":[
            {"matcher":{"id":"byName","options":"CPU Allocation"},"properties":[{"id":"custom.displayMode","value":"color-text"},{"id":"unit","value":"percentunit"}]},
            {"matcher":{"id":"byName","options":"Value #A"},"properties":[{"id":"custom.cellOptions","value":{"type":"gauge"}}]}
        ]}}"#,
        );
        let field = p
            .field
            .for_table_column("CPU Allocation", "Value #A", Some(&[0.5]));
        assert_eq!(field.unit.as_deref(), Some("percentunit"));
        assert_eq!(
            field.cell_display,
            CellDisplay::Gauge {
                mode: CellGauge::Basic,
                value: GaugeValue::Text
            }
        );
    }

    #[test]
    fn thresholds_pick_the_highest_step_reached() {
        let p = spec(
            r#"{"type":"stat","fieldConfig":{"defaults":{"thresholds":{"steps":[
                {"color":"green","value":null},{"color":"red","value":80},{"color":"yellow","value":50}]}}}}"#,
        );
        assert_eq!(p.field.threshold_color(10.), color::parse("green").unwrap());
        assert_eq!(
            p.field.threshold_color(60.),
            color::parse("yellow").unwrap()
        );
        assert_eq!(p.field.threshold_color(95.), color::parse("red").unwrap());
        assert_eq!(p.support(), Support::Full);
    }

    #[test]
    fn converts_legacy_singlestat_to_gauge() {
        let p = spec(
            r##"{"type":"singlestat","format":"percent","valueName":"current",
                "thresholds":"70,90","colors":["#299c46","rgba(237,129,40,0.89)","#d44a3a"],
                "gauge":{"show":true,"minValue":0,"maxValue":100}}"##,
        );
        let Viz::Gauge(GaugeOptions { calc }) = p.viz else {
            panic!("expected a gauge, got {:?}", p.viz)
        };
        assert_eq!(calc, Calc::Last);
        assert_eq!(p.field.unit.as_deref(), Some("percent"));
        assert_eq!(p.field.steps.len(), 3);
        assert_eq!(p.field.max, Some(100.));
    }

    #[test]
    fn lists_what_a_panel_ignores() {
        let p = spec(
            r#"{"type":"timeseries","transformations":[{}],
                "fieldConfig":{"defaults":{"custom":{"stacking":{"mode":"percent"}}},"overrides":[{}]}}"#,
        );
        assert_eq!(p.support(), Support::Partial);
        assert_eq!(p.ignored.len(), 2, "{:?}", p.ignored);
    }

    #[test]
    fn unknown_types_become_placeholders() {
        let p = spec(r#"{"type":"unknown-plugin","transformations":[{}]}"#);
        assert!(matches!(p.viz, Viz::Unsupported));
        assert_eq!(p.support(), Support::Placeholder);
        assert!(p.ignored.is_empty());
    }
    #[test]
    fn parses_stack_groups_percent_and_unstacked_overrides() {
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"defaults":{"custom":{"stacking":{"mode":"percent","group":"cpu"}}},"overrides":[
            {"matcher":{"id":"byName","options":"total"},"properties":[{"id":"custom.stacking","value":{"mode":"normal","group":false}}]},
            {"matcher":{"id":"byName","options":"other"},"properties":[{"id":"custom.stacking","value":{"mode":"normal","group":"B"}}]}
        ]}}"#,
        );
        let Viz::TimeSeries(o) = &p.viz else { panic!() };
        assert_eq!(o.stacking, Stacking::new(StackMode::Percent, "cpu"));
        assert_eq!(
            p.field.style("total").stacking.unwrap().mode,
            StackMode::None
        );
        assert_eq!(
            p.field.style("other").stacking.unwrap(),
            Stacking::new(StackMode::Normal, "B")
        );
        assert!(p.ignored.is_empty(), "{:?}", p.ignored);
        let legacy = spec(
            r#"{"type":"graph","stack":true,"percentage":true,"seriesOverrides":[{"alias":"limit","stack":false}]}"#,
        );
        let Viz::TimeSeries(o) = &legacy.viz else {
            panic!()
        };
        assert_eq!(o.stacking.mode, StackMode::Percent);
        assert_eq!(
            legacy.field.style("limit").stacking.unwrap().mode,
            StackMode::None
        );
    }

    #[test]
    fn parses_log_scales_on_defaults_overrides_and_legacy_axes() {
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"defaults":{"custom":{"scaleDistribution":{"type":"log","log":2}}},"overrides":[{"matcher":{"id":"byName","options":"right"},"properties":[{"id":"custom.axisPlacement","value":"right"},{"id":"custom.scaleDistribution","value":{"type":"log","log":10}}]}]}}"#,
        );
        assert_eq!(p.field.scale, AxisScale::Log(2.));
        assert_eq!(p.field.for_series("right").scale, AxisScale::Log(10.));
        assert!(p.ignored.is_empty());
        let p = spec(
            r#"{"type":"graph","yaxes":[{"logBase":2},{"logBase":10}],"seriesOverrides":[{"alias":"right","yaxis":2}]}"#,
        );
        let Viz::TimeSeries(o) = &p.viz else { panic!() };
        assert_eq!(p.field.scale, AxisScale::Log(2.));
        assert_eq!(
            p.field.for_time_series("right", o).scale,
            AxisScale::Log(10.)
        );
    }
    #[test]
    fn categorical_bar_orientation_parses_without_an_unsupported_note() {
        for (orientation, horizontal) in
            [("horizontal", true), ("vertical", false), ("auto", false)]
        {
            let p = spec(&format!(
                r#"{{"type":"barchart","options":{{"orientation":"{orientation}"}}}}"#
            ));
            let Viz::BarChart(options) = p.viz else {
                panic!("categorical bar chart")
            };
            assert_eq!(options.horizontal, horizontal);
            assert!(p.ignored.is_empty());
        }
        assert!(matches!(
            spec(r#"{"type":"barchart"}"#).viz,
            Viz::BarChart(BarChartOptions { horizontal: false })
        ));
    }

    #[test]
    fn query_matchers_patch_units_and_axes_on_equal_names_independently() {
        let p = spec(
            r#"{"type":"timeseries","fieldConfig":{"defaults":{"unit":"bytes"},"overrides":[
            {"matcher":{"id":"byFrameRefID","options":"B"},"properties":[{"id":"unit","value":"cps"},{"id":"custom.axisPlacement","value":"right"},{"id":"max","value":20}]},
            {"matcher":{"id":"byType","options":"number"},"properties":[{"id":"decimals","value":2}]}
        ]}}"#,
        );
        assert!(p.ignored.is_empty());
        let Viz::TimeSeries(options) = &p.viz else {
            panic!("time series")
        };
        let a = p
            .field
            .for_time_series_field(&FieldContext::new("same").query("A"), options);
        let b = p
            .field
            .for_time_series_field(&FieldContext::new("same").query("B"), options);
        assert_eq!(a.unit.as_deref(), Some("bytes"));
        assert_eq!(b.unit.as_deref(), Some("cps"));
        assert_eq!(b.max, Some(20.));
        assert_eq!(a.decimals, Some(2));
        assert_eq!(b.axis, AxisPlacement::Right);
    }
}
