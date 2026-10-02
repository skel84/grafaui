//! `stat` and legacy `singlestat`: a big value per series, colored by
//! threshold, over an optional sparkline (an AreaChart with its axes off).

use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, FontWeight, Hsla, Pixels, Size, Window, div, px, white};
use grafaui_model::spec::{StatColor, StatOptions};

use super::{ChartRow, PanelView, no_data};
use crate::ui::dp_px;

/// Most tiles a stat panel shows.
const MAX_TILES: usize = 12;

pub(super) fn render(
    view: &PanelView,
    options: &StatOptions,
    body: Size<Pixels>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let series = &view.data.series;
    if series.is_empty() {
        return no_data(cx);
    }
    let count = series.len().min(MAX_TILES);
    // Tiles go side by side in a wide body and stacked in a tall one.
    let across = body.width >= body.height;
    let tile = if across {
        Size::new(body.width / count as f32, body.height)
    } else {
        Size::new(body.width, body.height / count as f32)
    };
    // One size for every tile, fitted to the longest value, as Grafana does.
    let chars = series[..count]
        .iter()
        .map(|s| s.text.chars().count())
        .max()
        .unwrap_or(0)
        .max(3) as f32;
    let value_size = (tile.height * 0.42)
        .min(tile.width / (chars * 0.62))
        .clamp(px(12.), px(64.));
    let gap = dp_px(4., window);
    div()
        .size_full()
        .flex()
        .when_else(across, |this| this.flex_row(), |this| this.flex_col())
        .gap(gap)
        .children(
            (0..count)
                .map(|index| render_tile(view, options, index, tile, value_size, count > 1, cx)),
        )
        .into_any_element()
}

fn render_tile(
    view: &PanelView,
    options: &StatOptions,
    index: usize,
    tile: Size<Pixels>,
    value_size: Pixels,
    named: bool,
    cx: &App,
) -> impl IntoElement {
    let stat = &view.data.series[index];
    let theme = cx.theme();
    let (background, value_color) = match options.color {
        StatColor::Background => (Some(stat.color), white()),
        StatColor::Value => (None, stat.color),
        StatColor::None => (None, theme.foreground),
    };
    let sparkline_color = if background.is_some() {
        white().opacity(0.6)
    } else {
        stat.color
    };
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .rounded(px(3.))
        .when_some(background, |this, bg| this.bg(bg))
        .when(options.sparkline && tile.height > px(50.), |this| {
            this.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(tile.height * 0.45)
                    .child(sparkline(view, index, sparkline_color)),
            )
        })
        .child(
            v_flex()
                .relative()
                .size_full()
                .items_center()
                .justify_center()
                .when(named, |this| {
                    this.child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size((value_size * 0.36).max(px(10.)))
                            .text_color(if background.is_some() {
                                white().opacity(0.85)
                            } else {
                                theme.muted_foreground
                            })
                            .child(stat.name.clone()),
                    )
                })
                .child(
                    div()
                        .text_size(value_size)
                        .line_height(value_size * 1.1)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(value_color)
                        .whitespace_nowrap()
                        .child(stat.text.clone()),
                ),
        )
}

fn sparkline(view: &PanelView, index: usize, color: Hsla) -> AnyElement {
    let source = &view.data.frame.series[index].values;
    let rows: Vec<ChartRow> = view
        .data
        .rows
        .iter()
        .zip(source)
        .map(|(row, &value)| {
            let ys: std::rc::Rc<[f64]> = [value].into();
            ChartRow {
                x: row.x.clone(),
                raw: ys.clone(),
                bases: vec![0.].into(),
                ys,
            }
        })
        .collect();
    AreaChart::new(rows)
        .id(view.element_id(&format!("spark-{index}")))
        .interactive(false)
        .x(|row: &ChartRow| row.x.clone())
        .y(|row: &ChartRow| row.ys[0])
        .x_axis(false)
        .y_axis(false)
        .grid(false)
        .stroke(color)
        .fill(color.opacity(0.18))
        .into_any_element()
}
