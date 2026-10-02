//! `gauge` and `bargauge`.
//!
//! Kit's ProgressCircle is icon-sized, so a gauge is two PieChart rings: a
//! 270° arc (slices for value, track and a transparent gap at the bottom)
//! and a thin threshold ring outside it. Bar gauges compose Kit's progress
//! primitive with value-scale gradients or LCD segments.

use gpui_kit::base::Progress;
use gpui_kit::component::chart::PieChart;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, FontWeight, Hsla, Pixels, Size, Window, div, px, transparent_black,
};
use grafaui_model::spec::{BarGaugeOptions, FieldSpec};

use super::{PanelView, no_data};
use crate::ui::{dp, hsla};

/// The arc starts at 225° (lower left) and sweeps 270° clockwise.
const START: f32 = 225.;
const SWEEP: f32 = 270.;
const MAX_GAUGES: usize = 8;

#[derive(Clone, Copy)]
struct Slice {
    degrees: f32,
    color: Hsla,
}

pub(super) fn render(
    view: &PanelView,
    body: Size<Pixels>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let series = &view.data.series;
    if series.is_empty() {
        return no_data(cx);
    }
    let count = series.len().min(MAX_GAUGES);
    let tile = Size::new(body.width / count as f32, body.height);
    let named = count > 1;
    let label = if named {
        crate::ui::dp_px(18., window)
    } else {
        px(0.)
    };
    let radius = ((tile.width.min(tile.height - label)) / 2. - px(4.)).max(px(10.));
    h_flex()
        .size_full()
        .children((0..count).map(|index| {
            let stat = &series[index];
            let field = &stat.field;
            let (min, max) = range(view, field);
            let fraction = fraction(stat.value, min, max);
            let value_size = (radius * 0.42).clamp(px(10.), px(48.));
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .relative()
                        .w(radius * 2.)
                        .h(radius * 2.)
                        .child(ring(
                            view.element_id(&format!("thresholds-{index}")),
                            threshold_slices(field, min, max),
                            radius,
                            radius * 0.93,
                        ))
                        .child(ring(
                            view.element_id(&format!("gauge-{index}")),
                            value_slices(fraction, stat.color, cx.theme().border),
                            radius * 0.89,
                            radius * 0.68,
                        ))
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(value_size)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(stat.color)
                                .child(stat.text.clone()),
                        ),
                )
                .when(named, |this| {
                    this.child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size(dp(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(stat.name.clone()),
                    )
                })
        }))
        .into_any_element()
}

fn ring(
    id: gpui_kit::SharedString,
    slices: Vec<Slice>,
    outer: Pixels,
    inner: Pixels,
) -> impl IntoElement {
    div().absolute().inset_0().child(
        PieChart::new(slices)
            .id(id)
            .interactive(false)
            .value(|s: &Slice| s.degrees)
            .color(|s: &Slice| s.color)
            .outer_radius(outer.as_f32())
            .inner_radius(inner.as_f32()),
    )
}

/// The value's share of the arc, then the track, around the bottom gap.
fn value_slices(fraction: f32, color: Hsla, track: Hsla) -> Vec<Slice> {
    let end = START + SWEEP * fraction;
    slices(&[end], |offset| {
        if offset < SWEEP * fraction {
            color
        } else {
            track
        }
    })
}

/// One stretch of the arc per threshold step.
fn threshold_slices(field: &FieldSpec, min: f64, max: f64) -> Vec<Slice> {
    let breaks: Vec<f32> = field
        .steps
        .iter()
        .filter(|s| s.value.is_finite())
        .map(|s| {
            let value = if field.percent_steps {
                min + (max - min) * s.value / 100.
            } else {
                s.value
            };
            START + SWEEP * fraction(value, min, max)
        })
        .collect();
    slices(&breaks, |offset| {
        let value = min + (max - min) * f64::from(offset / SWEEP);
        hsla(field.threshold_color_in_range(value, (min, max)))
    })
}

/// Cuts the circle at the arc's ends and at `breaks` (angles from the
/// top, possibly past 360°), coloring each piece by `color(offset)` where
/// `offset` is its middle's angle from the arc's start.
fn slices(breaks: &[f32], color: impl Fn(f32) -> Hsla) -> Vec<Slice> {
    let mut cuts: Vec<f32> = vec![0., START + SWEEP - 360., START, 360.];
    cuts.extend(breaks.iter().map(|b| b.rem_euclid(360.)));
    cuts.sort_by(f32::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    cuts.windows(2)
        .map(|pair| {
            let middle = (pair[0] + pair[1]) / 2.;
            let offset = (middle - START).rem_euclid(360.);
            Slice {
                degrees: pair[1] - pair[0],
                color: if offset > SWEEP {
                    transparent_black()
                } else {
                    color(offset)
                },
            }
        })
        .collect()
}

/// The value range: the field's min and max, else 0 to 100 for percent,
/// else 0 to the largest sample.
fn range(view: &PanelView, field: &FieldSpec) -> (f64, f64) {
    let min = field.min.unwrap_or(0.);
    let max = field.max.unwrap_or_else(|| match field.unit.as_deref() {
        Some("percent") => 100.,
        Some("percentunit") => 1.,
        _ => view
            .data
            .frame
            .series
            .iter()
            .flat_map(|s| s.values.iter().copied())
            .filter(|v| v.is_finite())
            .fold(min + 1., f64::max),
    });
    (min, if max > min { max } else { min + 1. })
}

fn fraction(value: f64, min: f64, max: f64) -> f32 {
    if !value.is_finite() {
        return 0.;
    }
    ((value - min) / (max - min)).clamp(0., 1.) as f32
}

pub(super) fn render_bars(
    view: &PanelView,
    options: &BarGaugeOptions,
    body: Size<Pixels>,
    cx: &App,
) -> AnyElement {
    let series = &view.data.series;
    if series.is_empty() {
        return no_data(cx);
    }
    let row = (body.height / series.len() as f32).min(px(44.));
    let bar = (row * 0.55).clamp(px(4.), px(22.));
    let theme = cx.theme();
    v_flex()
        .size_full()
        .justify_center()
        .children(series.iter().enumerate().map(|(index, stat)| {
            let (min, max) = range(view, &stat.field);
            let colors = super::color_scale::Scale::new(&stat.field, (min, max), stat.color);
            h_flex()
                .h(row)
                .gap(dp(8.))
                .child(
                    div()
                        .w(body.width * 0.28)
                        .flex_none()
                        .truncate()
                        .text_size(dp(12.))
                        .text_color(theme.muted_foreground)
                        .child(stat.name.clone()),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Progress::new(view.element_id(&format!("bar-{index}")))
                            .value(fraction(stat.value, min, max) * 100.)
                            .accessibility_label(stat.name.clone())
                            .w_full()
                            .h(bar)
                            .child(colors.bar(
                                stat.value,
                                options.display,
                                theme.muted,
                                options.show_unfilled,
                            )),
                    ),
                )
                .child(
                    div()
                        .w(dp(72.))
                        .flex_none()
                        .text_right()
                        .text_size(dp(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(stat.color)
                        .child(stat.text.clone()),
                )
        }))
        .into_any_element()
}
