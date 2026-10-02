//! `timeseries` and legacy `graph`: Kit charts for ordinary series, and a
//! composed Kit plot for limits, independent axes and per-series styling.
//!
//! LineChart takes a single series, so lines are areas with the panel's
//! fill opacity, transparent when it is zero. Stacked series are accumulated in
//! [`super::derive`] by group and axis, then drawn between their actual bases.
//!
//! The composed plot uses zero as the fill baseline. The AreaChart fallback
//! draws negative values as lines because its fill reaches the plot bottom.

use std::rc::Rc;

use gpui_kit::component::chart::{AreaChart, BarChart};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Pixels, SharedString, Size, div, linear_color_stop, linear_gradient, px,
};
use grafaui_model::spec::{
    AxisPlacement, AxisScale, Calc, Curve, DrawStyle, FieldSpec, GradientMode, LineStyle,
    StackMode, ThresholdStyle, TimeSeriesOptions,
};

use super::{ChartRow, PanelView, SeriesStat, no_data};
use crate::ui::dp;

/// Below this body height the legend gives its room to the chart.
const LEGEND_MIN_BODY: f32 = 110.;

pub(super) fn render(
    view: &PanelView,
    options: &TimeSeriesOptions,
    body: Size<Pixels>,
    cx: &App,
) -> AnyElement {
    let data = &view.data;
    if data.series.is_empty() {
        return no_data(cx);
    }
    let field = Rc::new(view.spec.field.clone());
    let ticks = ((body.width / px(110.)) as usize).clamp(2, 8);
    // Kit's area fallback assumes evenly spaced samples and can bridge NaN.
    // The composed plot splits gaps and projects real timestamps.
    let irregular = data
        .frame
        .times
        .windows(3)
        .any(|t| ((t[1] - t[0]) - (t[2] - t[1])).abs() > 0.001);
    let gaps = data
        .frame
        .series
        .iter()
        .any(|s| s.values.iter().any(|v| !v.is_finite()));
    let custom = gaps
        || irregular
        || options.stacking.mode != StackMode::None
        || data.series.iter().any(|s| {
            s.stacking.mode != StackMode::None
                || s.style.stacking.is_some()
                || s.field.scale != AxisScale::Linear
        })
        || !options.thresholds.is_empty()
        || data.series.iter().any(|s| {
            s.right_axis
                || s.field.threshold_style != ThresholdStyle::Off
                || s.field.axis == AxisPlacement::Hidden
                || !s.field.axis_visible
                || s.style.draw.is_some()
                || s.style.show_points.is_some()
                || s.style.line_style.is_some()
                || s.style.gradient.is_some()
        })
        || options.show_points
        || options.draw == DrawStyle::Points
        || options.line_style != LineStyle::Solid
        || options.gradient == GradientMode::Scheme
        || (options.draw == DrawStyle::Bars && data.series.len() > 1);
    let chart = if custom {
        super::timeseries_plot::TimePlot::new(view, options).into_any_element()
    } else if options.draw == DrawStyle::Bars && data.series.len() == 1 {
        bars(view, &field, ticks)
    } else {
        area(view, options, &field, ticks)
    };
    v_flex()
        .size_full()
        .gap(dp(4.))
        .child(div().flex_1().min_h_0().child(chart))
        .when(
            options.legend && body.height > px(LEGEND_MIN_BODY),
            |this| {
                if options.legend_calcs.is_empty() {
                    this.child(legend(&data.series, cx))
                } else {
                    this.child(legend_table(view, &options.legend_calcs, body, cx))
                }
            },
        )
        .into_any_element()
}

fn area(
    view: &PanelView,
    options: &TimeSeriesOptions,
    field: &Rc<FieldSpec>,
    ticks: usize,
) -> AnyElement {
    let data = &view.data;
    let tick_format = field.clone();
    let tip_format = field.clone();
    let mut chart = AreaChart::new(data.rows.clone())
        .id(view.element_id("chart"))
        .x(|row: &ChartRow| row.x.clone())
        .x_tick_count(ticks)
        .grid_dashed(true)
        .y_axis(true)
        .y_tick_format(move |v| SharedString::from(tick_format.format(v)))
        .tooltip_title(|row: &ChartRow| row.x.label.clone());
    if let (Some(min), Some(max)) = (field.min, field.max) {
        chart = chart.y_domain(min, max);
    }
    let order: Vec<usize> = (0..data.series.len()).collect();
    let added = order.clone();
    chart = chart
        .tooltip_value(move |row: &ChartRow, i, _| tip_format.format(row.raw[added[i]]).into());
    let fill = options.fill_opacity;
    let below_zero = data.rows.iter().any(|row| row.ys.iter().any(|&y| y < 0.));
    for index in order {
        let series = &data.series[index];
        let fill = if below_zero {
            0.
        } else {
            series.fill.unwrap_or(fill)
        };
        chart = chart
            .y(move |row: &ChartRow| row.ys[index])
            .name(series.name.clone())
            .stroke(series.color)
            .fill(if options.gradient != GradientMode::None && fill > 0. {
                // Grafana's "opacity" gradient: full at the top, clear at
                // the bottom.
                linear_gradient(
                    180.,
                    linear_color_stop(series.color.opacity(fill), 0.),
                    linear_color_stop(series.color.opacity(0.), 1.),
                )
            } else {
                series.color.opacity(fill).into()
            });
        chart = match options.curve {
            Curve::Linear => chart.linear(),
            Curve::Smooth => chart.natural(),
            Curve::Step => chart.step_after(),
        };
    }
    chart.into_any_element()
}

fn bars(view: &PanelView, field: &Rc<FieldSpec>, ticks: usize) -> AnyElement {
    let color = view.data.series[0].color;
    let format = field.clone();
    BarChart::new(view.data.rows.clone())
        .id(view.element_id("bars"))
        .band(|row: &ChartRow| row.x.clone())
        .value(|row: &ChartRow| row.ys[0])
        .band_tick_count(ticks)
        .grid_dashed(true)
        .value_axis(true)
        .fill(move |_, _, _, _| color)
        .value_tick_format(move |v| SharedString::from(format.format(v)))
        .into_any_element()
}

pub(super) fn legend(series: &[SeriesStat], cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .flex_none()
        .flex_wrap()
        .max_h(dp(38.))
        .overflow_hidden()
        .gap_x(dp(12.))
        .gap_y(dp(2.))
        .children(series.iter().filter(|s| s.in_legend).map(|s| {
            h_flex()
                .gap(dp(5.))
                .text_size(dp(11.5))
                .text_color(muted)
                .child(div().w(dp(12.)).h(dp(3.)).rounded(px(1.)).bg(s.color))
                .child(s.name.clone())
        }))
}

/// Grafana's table legend: a row per series with the chosen calculations,
/// at most a third of the body tall and scrolling past that.
fn legend_table(
    view: &PanelView,
    calcs: &[Calc],
    body: Size<Pixels>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let value_column = |text: SharedString| {
        div()
            .w(dp(72.))
            .flex_none()
            .text_right()
            .truncate()
            .child(text)
    };
    let header = h_flex()
        .gap(dp(8.))
        .pb(dp(2.))
        .border_b_1()
        .border_color(theme.border)
        .text_color(muted)
        .child(div().flex_1())
        .children(calcs.iter().map(|c| value_column(c.label().into())));
    let rows = view.data.series.iter().filter(|s| s.in_legend).map(|s| {
        h_flex()
            .gap(dp(8.))
            .h(dp(18.))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(dp(5.))
                    .child(
                        div()
                            .flex_none()
                            .w(dp(12.))
                            .h(dp(3.))
                            .rounded(px(1.))
                            .bg(s.color),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(s.name.clone()),
                    ),
            )
            .children(s.legend.iter().map(|v| value_column(v.clone())))
    });
    v_flex()
        .flex_none()
        .text_size(dp(11.5))
        .child(header)
        .child(
            v_flex()
                .id(view.element_id("legend"))
                .max_h(body.height / 3.)
                .overflow_y_scroll()
                .children(rows),
        )
}
