//! One dashboard panel: its frame (title, description, partial badge) and
//! a body drawn by the module for its visualization.
//!
//! Display data is derived in [`PanelView::set_frame`] when the data
//! changes, never in `render`.

mod bars;
mod color_scale;
mod gauge;
mod geomap;
mod heatmap;
mod pie;
mod placeholder;
mod stat;
mod table;
mod text;
mod timeseries;
mod timeseries_plot;

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, Icon, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, Hsla, IntoElement, Pixels, Render, SharedString, Size,
    Window, div, px, size,
};
use grafaui_model::data::Frame;
use grafaui_model::schema::GridPos;
use grafaui_model::spec::{AxisPlacement, Calc, FieldSpec, PanelSpec, Viz};
use grafaui_model::time;
use grafaui_model::transform::{self, Table};

use crate::grid;
use crate::ui::{self, Tone, dp, dp_px};

/// Below this width the header shrinks the partial tag to an icon.
const NARROW: f32 = 260.;

/// Height of the title row.
const HEADER: f32 = 28.;
/// Padding around the body.
const INSET: f32 = 8.;

pub(crate) struct PanelView {
    spec: Rc<PanelSpec>,
    pos: GridPos,
    /// The spec's title with variables filled in.
    title: SharedString,
    data: PanelData,
    geo_view: Option<geomap::ViewState>,
    loading: bool,
    error: Option<String>,
    warnings: Vec<String>,
}

/// What the body draws, derived once per frame of data.
pub(crate) struct PanelData {
    pub frame: Frame,
    pub series: Vec<SeriesStat>,
    /// One row per sample for the time charts; stacked when the panel
    /// stacks.
    pub rows: Vec<ChartRow>,
    /// Table panels: the table after transformations.
    pub table: Option<Table>,
    pub table_columns: Vec<table::Column>,
    pub heatmap: Option<Rc<grafaui_model::heatmap::Grid>>,
    pub geomap: Option<Rc<grafaui_model::geomap::MapData>>,
    /// What the panel ignores: the spec's list plus what only drawing
    /// finds out.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SeriesStat {
    pub name: SharedString,
    pub value: f64,
    pub text: SharedString,
    pub color: Hsla,
    /// Its own fill opacity, from an override.
    pub fill: Option<f32>,
    pub in_legend: bool,
    /// The legend table's values, one per
    /// [`grafaui_model::spec::TimeSeriesOptions::legend_calcs`].
    pub legend: Vec<SharedString>,
    pub field: Rc<FieldSpec>,
    pub right_axis: bool,
    pub style: grafaui_model::overrides::SeriesStyle,
    pub color_range: (f64, f64),
    pub stacking: grafaui_model::chart::Stacking,
}

/// A sample time on a chart's x-axis. Equal by position, so two samples in
/// the same minute stay apart.
#[derive(Clone, Debug)]
pub(crate) struct Tick {
    index: usize,
    time: f64,
    label: SharedString,
}

impl PartialEq for Tick {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl Eq for Tick {}

impl std::hash::Hash for Tick {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state);
    }
}

impl From<Tick> for SharedString {
    fn from(tick: Tick) -> Self {
        tick.label
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ChartRow {
    pub x: Tick,
    pub ys: Rc<[f64]>,
    pub bases: Rc<[f64]>,
    /// Each series' own value, which the tooltip shows when `ys` is stacked.
    pub raw: Rc<[f64]>,
}

impl PanelView {
    pub(crate) fn new(
        spec: Rc<PanelSpec>,
        pos: GridPos,
        title: String,
        frame: Frame,
        span: u64,
    ) -> Self {
        let data = derive(&spec, frame, span);
        Self {
            spec,
            pos,
            title: title.into(),
            data,
            geo_view: None,
            loading: false,
            error: None,
            warnings: Vec::new(),
        }
    }

    /// New data, and the title again with the variables' current values.
    pub(crate) fn set_frame(
        &mut self,
        title: String,
        frame: Frame,
        span: u64,
        cx: &mut Context<Self>,
    ) {
        self.title = title.into();
        self.data = derive(&self.spec, frame, span);
        self.loading = false;
        self.error = None;
        self.warnings.clear();
        cx.notify();
    }

    pub(crate) fn set_loading(&mut self, title: String, cx: &mut Context<Self>) {
        self.title = title.into();
        self.loading = true;
        self.error = None;
        self.warnings.clear();
        cx.notify();
    }

    pub(crate) fn set_result(
        &mut self,
        title: String,
        result: Result<(Frame, Vec<String>), String>,
        span: u64,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok((frame, warnings)) => {
                self.set_frame(title, frame, span, cx);
                self.warnings = warnings;
            }
            Err(error) => {
                self.loading = false;
                self.error = Some(error);
                self.data = derive(&self.spec, Frame::default(), span);
                cx.notify();
            }
        }
    }

    pub(crate) fn spec(&self) -> &PanelSpec {
        &self.spec
    }

    pub(crate) fn notes(&self) -> &[String] {
        &self.data.notes
    }

    /// An element id unique to this panel.
    fn element_id(&self, part: &str) -> SharedString {
        format!("panel-{}-{part}", self.spec.key).into()
    }

    /// The body's size, from the grid position and the window width.
    fn body_size(&self, window: &Window) -> Size<Pixels> {
        let outer = grid::panel_size(self.pos, window);
        size(
            (outer.width - dp_px(2. * INSET, window)).max(px(1.)),
            (outer.height - dp_px(HEADER + INSET, window)).max(px(1.)),
        )
    }

    /// `narrow` panels show the partial tag as its icon alone, leaving the
    /// title room.
    fn render_header(&self, narrow: bool, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let description = self.spec.description.clone();
        let notes = self.data.notes.clone();
        h_flex()
            .h(dp(HEADER))
            .flex_none()
            .px(dp(INSET))
            .gap(dp(6.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(dp(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(if self.title.is_empty() {
                        SharedString::from("")
                    } else {
                        self.title.clone()
                    }),
            )
            .when_some(description, |this, description| {
                this.child(
                    div()
                        .id(self.element_id("description"))
                        .flex_none()
                        .child(
                            Icon::new(IconName::Info)
                                .size(dp(13.))
                                .text_color(theme.muted_foreground),
                        )
                        .tooltip(move |window, cx| {
                            Tooltip::new(description.clone()).build(window, cx)
                        }),
                )
            })
            .child(div().flex_1())
            .when(self.loading, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Loading…"),
                )
            })
            .when(!self.warnings.is_empty(), |this| {
                let warning = self.warnings.join("\n");
                this.child(
                    div()
                        .id(self.element_id("warning"))
                        .child(ui::tag(Tone::Warn, None, "Warning", cx))
                        .tooltip(move |window, cx| Tooltip::new(warning.clone()).build(window, cx)),
                )
            })
            .when(
                !notes.is_empty() && !matches!(self.spec.viz, Viz::Unsupported),
                |this| {
                    let tip: SharedString = format!("Not drawn:\n• {}", notes.join("\n• ")).into();
                    this.child(
                        div()
                            .id(self.element_id("partial"))
                            .flex_none()
                            .when_else(
                                narrow,
                                |this| {
                                    this.child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(dp(13.))
                                            .text_color(Tone::Warn.color(cx)),
                                    )
                                },
                                |this| {
                                    this.child(ui::tag(
                                        Tone::Warn,
                                        Some(IconName::TriangleAlert),
                                        "Partial",
                                        cx,
                                    ))
                                },
                            )
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
                    )
                },
            )
    }

    fn render_body(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(error) = &self.error {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(div().w_full().min_w_0().p_2().child(error.clone()))
                .into_any_element();
        }
        if self.loading
            && self.data.series.is_empty()
            && !matches!(self.spec.viz, Viz::Text(_) | Viz::Unsupported)
        {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(div().w_full().min_w_0().child("Loading Prometheus data…"))
                .into_any_element();
        }
        let body = self.body_size(window);
        match &self.spec.viz {
            Viz::TimeSeries(options) => timeseries::render(self, options, body, cx),
            Viz::Stat(options) => stat::render(self, options, body, window, cx),
            Viz::Gauge(_) => gauge::render(self, body, window, cx),
            Viz::BarGauge(options) => gauge::render_bars(self, options, body, cx),
            Viz::Pie(options) => pie::render(self, options, body, cx),
            Viz::BarChart(options) => bars::render_categories(self, options, cx),
            Viz::Histogram => bars::render_histogram(self, cx),
            Viz::Heatmap(options) => heatmap::render(self, options, cx),
            Viz::Geomap(options) => geomap::render(self, options, window, cx),
            Viz::Candlestick => bars::render_candles(self, cx),
            Viz::Table => table::render(self, cx),
            Viz::Text(options) => text::render(self, options, cx),
            Viz::Unsupported => placeholder::render(self, cx),
        }
    }
}

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id(self.element_id("frame"))
            .size_full()
            .overflow_hidden()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .bg(grid::panel_background(cx))
            .child(self.render_header(self.body_size(window).width < px(NARROW), cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(dp(INSET))
                    .pb(dp(INSET))
                    .child(self.render_body(window, cx)),
            )
    }
}

fn derive(spec: &PanelSpec, frame: Frame, span: u64) -> PanelData {
    let calc = match &spec.viz {
        Viz::Stat(o) => o.calc,
        Viz::Gauge(o) => o.calc,
        Viz::BarGauge(o) => o.calc,
        Viz::Pie(o) => o.calc,
        _ => Calc::LastNotNull,
    };
    let table = matches!(spec.viz, Viz::Table).then(|| {
        let queries: Vec<&str> = spec.queries.iter().map(|q| q.ref_id.as_str()).collect();
        transform::table(&spec.transforms, &frame.series, &queries)
    });
    let table_columns = table
        .as_ref()
        .map(|t| table::columns(t, &spec.field))
        .unwrap_or_default();
    // Transformations, then overrides: they filter, rename and hide series.
    // Negative-Y flips only what is drawn; the legend and tooltip keep the
    // real values.
    let mut frame = if matches!(spec.viz, Viz::Table) {
        frame
    } else {
        transform::apply(&spec.transforms, frame)
    };
    let mut styles = Vec::with_capacity(frame.series.len());
    let mut fields = Vec::with_capacity(frame.series.len());
    frame.series.retain_mut(|s| {
        let context = grafaui_model::spec::FieldContext::new(&s.name)
            .query(&s.query)
            .values(&s.values);
        let style = spec.field.style_for_field(&context);
        if style.hidden {
            return false;
        }
        fields.push(match &spec.viz {
            Viz::TimeSeries(options) => spec.field.for_time_series_field(&context, options),
            _ => spec.field.for_field(&context).into_owned(),
        });
        if let Some(name) = &style.display_name {
            s.name.clone_from(name);
        }
        styles.push(style);
        true
    });
    let left_unit = fields
        .iter()
        .find(|f| f.axis != AxisPlacement::Right)
        .and_then(|f| f.unit.clone());
    let right_axes: Vec<_> = fields
        .iter()
        .map(|f| {
            f.axis == AxisPlacement::Right || (f.axis == AxisPlacement::Auto && f.unit != left_unit)
        })
        .collect();
    let legend_calcs: &[Calc] = match &spec.viz {
        Viz::TimeSeries(o) => &o.legend_calcs,
        _ => &[],
    };
    let color_ranges = [false, true].map(|right| {
        let time_series = matches!(spec.viz, Viz::TimeSeries(_));
        let mut values = frame
            .series
            .iter()
            .enumerate()
            .filter(|(i, _)| !time_series || right_axes[*i] == right)
            .flat_map(|(_, s)| s.values.iter().copied())
            .filter(|v| v.is_finite());
        let Some(first) = values.next() else {
            return (0., 100.);
        };
        values.fold((first, first), |(min, max), v| (min.min(v), max.max(v)))
    });
    let series: Vec<SeriesStat> = frame
        .series
        .iter()
        .zip(&styles)
        .zip(&fields)
        .enumerate()
        .map(|(index, ((s, style), field))| {
            let value = calc.reduce(&s.values);
            let mut range = color_ranges[usize::from(right_axes[index])];
            if matches!(spec.viz, Viz::BarGauge(_) | Viz::Gauge(_)) {
                range = match field.unit.as_deref() {
                    Some("percent") => (0., 100.),
                    Some("percentunit") => (0., 1.),
                    _ => (0., range.1.max(1.)),
                };
            }
            let color_range = field.color_range(range);
            let color_value = if matches!(spec.viz, Viz::TimeSeries(_)) {
                field.color_calc.reduce(&s.values)
            } else {
                value
            };
            let color = field.series_color_in_range(index, color_value, color_range);
            SeriesStat {
                name: s.name.clone().into(),
                value,
                text: field.display(value).into(),
                color: ui::hsla(color),
                fill: style.fill_opacity,
                in_legend: !style.hidden_in_legend,
                legend: legend_calcs
                    .iter()
                    .map(|c| field.format(c.reduce(&s.values)).into())
                    .collect(),
                field: Rc::new(field.clone()),
                right_axis: right_axes[index],
                style: style.clone(),
                color_range,
                stacking: style.stacking.clone().unwrap_or_else(|| match &spec.viz {
                    Viz::TimeSeries(o) => o.stacking.clone(),
                    _ => Default::default(),
                }),
            }
        })
        .collect();
    let stacking: Vec<_> = series
        .iter()
        .map(|s: &SeriesStat| s.stacking.clone())
        .collect();
    let percent_axes = [false, true].map(|right| {
        series.iter().any(|s| {
            s.right_axis == right && s.stacking.mode == grafaui_model::chart::StackMode::Percent
        })
    });
    let rows = frame
        .times
        .iter()
        .enumerate()
        .map(|(index, &t)| {
            let raw: Rc<[f64]> = frame
                .series
                .iter()
                .map(|s| s.values.get(index).copied().unwrap_or(f64::NAN))
                .collect();
            let drawn = raw
                .iter()
                .zip(&styles)
                .map(|(&v, style)| if style.negative_y { -v } else { v });
            let values: Vec<_> = drawn.collect();
            let (mut ys, mut bases) = grafaui_model::chart::stack(&values, &stacking, &right_axes);
            // Percent stacks use a 0..100 display domain. Unstacked fractional
            // percentages on that axis still need their ordinary unit conversion.
            for (i, series) in series.iter().enumerate() {
                if percent_axes[usize::from(series.right_axis)]
                    && series.stacking.mode != grafaui_model::chart::StackMode::Percent
                    && series.field.unit.as_deref() == Some("percentunit")
                {
                    ys[i] *= 100.;
                    bases[i] *= 100.;
                }
            }
            ChartRow {
                x: Tick {
                    index,
                    time: t,
                    label: time::tick_label(t.floor() as i64, span).into(),
                },
                ys: ys.into(),
                bases: bases.into(),
                raw,
            }
        })
        .collect();
    let mut notes = spec.ignored.clone();
    let heatmap = match &spec.viz {
        Viz::Heatmap(options) => Some(Rc::new(grafaui_model::heatmap::Grid::from_frame(
            &frame, options,
        ))),
        _ => None,
    };
    let geomap = match &spec.viz {
        Viz::Geomap(options) => Some(Rc::new(grafaui_model::geomap::MapData::from_frame(
            &frame,
            options,
            &spec.field,
        ))),
        _ => None,
    };
    notes.extend(render_notes(spec, &frame));
    PanelData {
        frame,
        series,
        rows,
        table,
        table_columns,
        heatmap,
        geomap,
        notes,
    }
}

/// Limits that depend on the data rather than on the panel's settings.
fn render_notes(spec: &PanelSpec, _frame: &Frame) -> Vec<String> {
    let mut notes = Vec::new();
    match &spec.viz {
        Viz::BarGauge(o) if o.vertical => notes.push("vertical bars (drawn horizontal)".into()),
        _ => {}
    }
    notes
}

/// Body text for a panel with nothing to show.
fn no_data(cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(cx.theme().muted_foreground)
        .child("No data")
        .into_any_element()
}
