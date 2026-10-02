//! `piechart`: Kit's PieChart, a donut when the panel asks for one, with a
//! legend of values and shares beside or below it.

use gpui_kit::component::chart::PieChart;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Pixels, SharedString, Size, div, px};
use grafaui_model::spec::PieOptions;

use super::{PanelView, SeriesStat, no_data};
use crate::ui::dp;

pub(super) fn render(
    view: &PanelView,
    options: &PieOptions,
    body: Size<Pixels>,
    cx: &App,
) -> AnyElement {
    let series: Vec<SeriesStat> = view
        .data
        .series
        .iter()
        .filter(|s| s.value.is_finite() && s.value > 0.)
        .cloned()
        .collect();
    if series.is_empty() {
        return no_data(cx);
    }
    let total: f64 = series.iter().map(|s| s.value).sum();
    let beside = body.width > body.height * 1.4;
    let chart_side = if beside {
        body.height.min(body.width * 0.6)
    } else {
        (body.height * 0.7).min(body.width)
    };
    let radius = (chart_side / 2. - px(6.)).max(px(8.));
    let pie = PieChart::new(series.clone())
        .id(view.element_id("pie"))
        .value(|s: &SeriesStat| s.value as f32)
        .color(|s: &SeriesStat| s.color)
        .outer_radius(radius.as_f32())
        .inner_radius(if options.donut {
            radius.as_f32() * 0.6
        } else {
            0.
        })
        .pad_angle(0.01)
        .tooltip_name(|s: &SeriesStat| s.name.clone())
        .tooltip_value(|s: &SeriesStat, _, share| format!("{} ({share:.1}%)", s.text).into());
    let legend = v_flex()
        .min_w_0()
        .gap(dp(3.))
        .justify_center()
        .children(series.iter().map(|s| {
            let share: SharedString = format!("{:.0}%", s.value / total * 100.).into();
            h_flex()
                .gap(dp(6.))
                .text_size(dp(12.))
                .child(div().flex_none().size(dp(9.)).rounded(px(2.)).bg(s.color))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(s.name.clone()),
                )
                .child(div().flex_none().child(s.text.clone()))
                .child(
                    div()
                        .flex_none()
                        .text_color(cx.theme().muted_foreground)
                        .child(share),
                )
        }));
    div()
        .size_full()
        .flex()
        .when_else(beside, |this| this.flex_row(), |this| this.flex_col())
        .gap(dp(10.))
        .items_center()
        .child(div().flex_none().w(chart_side).h(chart_side).child(pie))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(legend),
        )
        .into_any_element()
}
