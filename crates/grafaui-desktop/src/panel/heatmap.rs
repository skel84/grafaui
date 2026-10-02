//! Heatmap cells drawn with Kit's plot lifecycle, axes and tooltip overlay.
use super::{PanelView, no_data};
use crate::ui::{self, dp, dp_px};
use gpui_kit::component::plot::{
    AxisLabelSide, AxisText, IntoPlot, Plot, PlotAxis, TooltipState, axis_gutter,
    label::measure_text_width,
    tooltip::{CrossLine, Tooltip},
};
use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, ContentMask, ElementId, Pixels, Point, SharedString, TextAlign,
    Window, div, fill, point, px, relative, size,
};
use grafaui_model::heatmap::{Grid, Options};
use grafaui_model::{time, units};
use std::rc::Rc;

pub(super) fn render(view: &PanelView, options: &Options, cx: &App) -> AnyElement {
    let Some(grid) = view
        .data
        .heatmap
        .as_ref()
        .filter(|g| !g.buckets.is_empty() && g.times.len() > 1)
    else {
        return no_data(cx);
    };
    let range = grid.count_range(options);
    v_flex()
        .size_full()
        .gap(dp(3.))
        .child(div().flex_1().min_h_0().child(HeatPlot::new(
            view.element_id("heatmap").into(),
            grid.clone(),
            options.clone(),
        )))
        .when(options.legend, |this| {
            this.child(
                h_flex()
                    .flex_none()
                    .gap(dp(6.))
                    .text_size(dp(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child(units::format(range.0, Some("short"), Some(0)))
                    .child(
                        h_flex()
                            .w(dp(180.))
                            .max_w_full()
                            .h(dp(8.))
                            .children((0..64).map(|i| {
                                div()
                                    .flex_1()
                                    .h_full()
                                    .bg(ui::hsla(options.color_at(i as f64 / 63.)))
                            })),
                    )
                    .child(units::format(range.1, Some("short"), Some(0)))
                    .child("Count"),
            )
        })
        .into_any_element()
}

#[derive(Clone, Copy)]
struct Geometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

#[derive(IntoPlot)]
struct HeatPlot {
    id: ElementId,
    grid: Rc<Grid>,
    options: Options,
    range: (f64, f64),
    geometry: Geometry,
    font: Pixels,
}

impl HeatPlot {
    fn new(id: ElementId, grid: Rc<Grid>, options: Options) -> Self {
        let range = grid.count_range(&options);
        Self {
            id,
            grid,
            options,
            range,
            geometry: Geometry {
                left: 0.,
                top: 0.,
                width: 1.,
                height: 1.,
            },
            font: px(10.),
        }
    }
    fn columns(&self) -> usize {
        self.grid.times.len().saturating_sub(1)
    }
    fn row_y(&self, row: usize) -> f32 {
        let row = if self.options.y_reverse {
            row
        } else {
            self.grid.buckets.len() - row - 1
        };
        self.geometry.top + row as f32 * self.geometry.height / self.grid.buckets.len() as f32
    }
    fn tick(&self, row: usize) -> SharedString {
        let bucket = &self.grid.buckets[row];
        bucket
            .label
            .clone()
            .unwrap_or_else(|| {
                if bucket.max.is_infinite() {
                    "+Inf".into()
                } else {
                    self.options.format(bucket.max)
                }
            })
            .into()
    }
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let g = self.geometry;
        let x = position.x.as_f32() - g.left;
        let y = position.y.as_f32() - g.top;
        if x < 0.
            || x >= g.width
            || y < 0.
            || y >= g.height
            || self.columns() == 0
            || self.grid.buckets.is_empty()
        {
            return None;
        }
        let col = (x / g.width * self.columns() as f32).floor() as usize;
        let row = (y / g.height * self.grid.buckets.len() as f32).floor() as usize;
        Some((
            if self.options.y_reverse {
                row
            } else {
                self.grid.buckets.len() - row - 1
            },
            col,
        ))
    }
    fn color(&self, value: f64) -> gpui_kit::Hsla {
        ui::hsla(
            self.options
                .color_at((value - self.range.0) / (self.range.1 - self.range.0)),
        )
    }
}

impl Plot for HeatPlot {
    fn id(&self) -> Option<ElementId> {
        self.options.tooltip.then(|| self.id.clone())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut App,
    ) -> Vec<AnyElement> {
        self.font = dp_px(10., window);
        let gutter = if self.options.y_visible {
            (0..self.grid.buckets.len())
                .map(|i| measure_text_width(&self.tick(i), self.font, window))
                .fold(0_f32, f32::max)
                .min(bounds.size.width.as_f32() * 0.3)
                + dp_px(8., window).as_f32()
        } else {
            0.
        };
        let top = self.font.as_f32() / 2.;
        let bottom = if self.options.x_visible {
            axis_gutter(self.font)
        } else {
            0.
        };
        self.geometry = Geometry {
            left: if self.options.y_right { 0. } else { gutter },
            top,
            width: (bounds.size.width.as_f32() - gutter).max(1.),
            height: (bounds.size.height.as_f32() - top - bottom).max(1.),
        };
        Vec::new()
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let g = self.geometry;
        let columns = self.columns();
        if columns == 0 || self.grid.buckets.is_empty() {
            return;
        }
        let plot = Bounds::new(
            bounds.origin + point(px(g.left), px(g.top)),
            size(px(g.width), px(g.height)),
        );
        let width = g.width / columns as f32;
        let height = g.height / self.grid.buckets.len() as f32;
        let gap = dp_px(self.options.gap, window)
            .as_f32()
            .min(width * 0.8)
            .min(height * 0.8);
        window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
            for (row, counts) in self.grid.counts.iter().enumerate() {
                for (col, &count) in counts.iter().enumerate() {
                    if self.options.visible(count) {
                        window.paint_quad(fill(
                            Bounds::new(
                                bounds.origin
                                    + point(
                                        px(g.left + width * col as f32 + gap / 2.),
                                        px(self.row_y(row) + gap / 2.),
                                    ),
                                size(px(width - gap), px(height - gap)),
                            ),
                            self.color(count),
                        ));
                    }
                }
            }
        });
        let muted = cx.theme().muted_foreground;
        if self.options.y_visible {
            let max_ticks = (g.height / (self.font.as_f32() * 1.5)).floor().max(1.) as usize;
            let stride = self.grid.buckets.len().div_ceil(max_ticks).max(1);
            let x = if self.options.y_right { g.width } else { 0. };
            PlotAxis::new()
                .y(px(x))
                .stroke(cx.theme().border)
                .y_label_side(if self.options.y_right {
                    AxisLabelSide::End
                } else {
                    AxisLabelSide::Start
                })
                .y_label((0..self.grid.buckets.len()).step_by(stride).map(|i| {
                    AxisText::new(self.tick(i), px(self.row_y(i) - g.top + height / 2.), muted)
                        .font_size(self.font)
                        .align(if self.options.y_right {
                            TextAlign::Left
                        } else {
                            TextAlign::Right
                        })
                }))
                .paint(&plot, window, cx);
        }
        if self.options.x_visible {
            let count = ((g.width / dp_px(110., window).as_f32()).floor() as usize)
                .clamp(2, 8)
                .min(columns);
            let span = (self.grid.times[columns] - self.grid.times[0]).max(1) as u64;
            PlotAxis::new()
                .x(px(g.height))
                .stroke(cx.theme().border)
                .x_label((0..count).map(|i| {
                    let col = if count > 1 {
                        i * (columns - 1) / (count - 1)
                    } else {
                        0
                    };
                    AxisText::new(
                        time::tick_label(self.grid.times[col], span),
                        px(width * (col as f32 + 0.5)),
                        muted,
                    )
                    .font_size(self.font)
                    .align(if i == 0 {
                        TextAlign::Left
                    } else if i + 1 == count {
                        TextAlign::Right
                    } else {
                        TextAlign::Center
                    })
                }))
                .paint(
                    &Bounds::new(
                        plot.origin,
                        size(plot.size.width, bounds.size.height - px(g.top)),
                    ),
                    window,
                    cx,
                );
        }
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        _: Bounds<Pixels>,
        _: &App,
    ) -> Option<TooltipState> {
        let (row, col) = self.cell_at(position)?;
        let value = self.grid.counts[row][col];
        if !self.options.visible(value) {
            return None;
        }
        let g = self.geometry;
        Some(TooltipState::new(
            row * self.columns() + col,
            point(
                px(g.left + (col as f32 + 0.5) * g.width / self.columns() as f32),
                px(self.row_y(row) + g.height / self.grid.buckets.len() as f32 / 2.),
            ),
            Vec::new(),
        ))
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (row, col) = (state.index / self.columns(), state.index % self.columns());
        let value = *self.grid.counts.get(row)?.get(col)?;
        let mut tip = Tooltip::new(cursor, bounds.size)
            .glide(false)
            // Kit 0.7's structured rows replace its freeform children.
            // Keep all content freeform so the optional histogram is retained.
            .child(div().font_semibold().child(format!(
                "{} – {}",
                time::date_time(self.grid.times[col]),
                time::date_time(self.grid.times[col + 1])
            )))
            .child(
                h_flex()
                    .justify_between()
                    .gap(dp(12.))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("Bucket"),
                    )
                    .child(self.grid.buckets[row].label(&self.options)),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap(dp(12.))
                    .child(
                        h_flex()
                            .gap(dp(6.))
                            .child(div().size(dp(8.)).rounded(dp(2.)).bg(self.color(value)))
                            .child(div().text_color(cx.theme().muted_foreground).child("Count")),
                    )
                    .child(units::format(value, Some("short"), None)),
            )
            .cross_line(
                CrossLine::new(state.cross_line)
                    .band(px(self.geometry.width / self.columns() as f32))
                    .span(self.geometry.top, self.geometry.height),
            );
        if self.options.histogram {
            let max = self
                .grid
                .counts
                .iter()
                .map(|r| r[col])
                .filter(|v| v.is_finite())
                .fold(1_f64, f64::max);
            tip = tip
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child("Bucket counts at this time"),
                )
                .child(
                    h_flex()
                        .items_end()
                        .w(dp(180.))
                        .h(dp(44.))
                        .gap(dp(1.))
                        .children(self.grid.counts.iter().map(|r| {
                            let value = if r[col].is_finite() {
                                r[col].max(0.)
                            } else {
                                0.
                            };
                            div()
                                .flex_1()
                                .h(relative((value / max) as f32))
                                .bg(self.color(value))
                        })),
                );
        }
        Some(tip.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_uses_the_cell_geometry_and_respects_reversed_rows() {
        let mut grid = Grid::default();
        grid.times = vec![0, 60, 120];
        grid.buckets = vec![
            grafaui_model::heatmap::Bucket::new(0., 1.),
            grafaui_model::heatmap::Bucket::new(1., 2.),
        ];
        grid.counts = vec![vec![1., 2.], vec![3., 4.]];
        let mut plot = HeatPlot::new("test".into(), Rc::new(grid), Options::default());
        plot.geometry = Geometry {
            left: 40.,
            top: 5.,
            width: 200.,
            height: 100.,
        };
        assert_eq!(plot.cell_at(point(px(60.), px(15.))), Some((1, 0)));
        assert_eq!(plot.cell_at(point(px(220.), px(90.))), Some((0, 1)));
        assert_eq!(plot.cell_at(point(px(39.), px(15.))), None);
        plot.options.y_reverse = true;
        assert_eq!(plot.cell_at(point(px(60.), px(15.))), Some((0, 0)));
    }
}
