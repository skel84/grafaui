//! Grafana limits and independent value axes composed from Kit 0.7 plot
//! primitives. The same projection drives axes, series, limits and hover.
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::plot::{
    AxisLabelSide, AxisText, Curve as PlotCurve, Grid, IntoPlot, PathCaches, Plot, PlotAxis,
    ShapeKey, TooltipState, axis_gutter,
    label::measure_text_width,
    shape::{Area, Line},
    tooltip::{CrossLine, Dot, Tooltip},
};
use gpui_kit::{
    AnyElement, App, Bounds, ContentMask, ElementId, IntoElement, Pixels, Point, SharedString,
    TextAlign, Window, fill, linear_color_stop, linear_gradient, point, px, size,
};
use grafaui_model::spec::{
    AxisPlacement, AxisScale, Curve, DrawStyle, FieldSpec, GradientMode, GraphThreshold, LineStyle,
    StackMode, ThresholdStyle, TimeSeriesOptions,
};

use super::{ChartRow, PanelView, SeriesStat};
use crate::ui::{self, dp_px};

#[derive(Clone, Copy, Debug)]
struct Domain {
    min: f64,
    max: f64,
    scale: AxisScale,
    percent: bool,
}

impl Domain {
    fn project(self, value: f64, height: f32) -> f32 {
        let (min, max, value) = (
            self.scale.project(self.min),
            self.scale.project(self.max),
            self.scale.project(value),
        );
        (height as f64 * (max - value) / (max - min)) as f32
    }
    fn value(self, fraction: f64) -> f64 {
        self.scale.invert(
            self.scale.project(self.min)
                + (self.scale.project(self.max) - self.scale.project(self.min)) * fraction,
        )
    }
    fn baseline(self) -> f64 {
        if self.scale == AxisScale::Linear {
            0.
        } else {
            self.min
        }
    }
    fn format(self, field: &FieldSpec, value: f64) -> String {
        if self.percent {
            grafaui_model::units::format(value, Some("percent"), field.decimals)
        } else {
            field.format(value)
        }
    }
    fn ticks(self) -> Vec<f64> {
        if let AxisScale::Log(base) = self.scale {
            let low = self.min.log(base).round();
            let high = self.max.log(base).round();
            let step = ((high - low) / 4.).ceil().max(1.);
            let mut ticks = Vec::new();
            let mut exponent = low;
            while exponent <= high && ticks.len() < 16 {
                ticks.push(base.powf(exponent));
                exponent += step;
            }
            if ticks.last().is_none_or(|v| *v < self.max * 0.999) {
                ticks.push(self.max);
            }
            ticks
        } else {
            (0..5).map(|i| self.value(i as f64 / 4.)).collect()
        }
    }
}

#[derive(Clone, Copy)]
struct Geometry {
    left: f32,
    width: f32,
    top: f32,
    height: f32,
}

#[derive(IntoPlot)]
pub(super) struct TimePlot {
    id: ElementId,
    rows: Vec<ChartRow>,
    series: Vec<SeriesStat>,
    options: TimeSeriesOptions,
    domains: [Domain; 2],
    geometry: Geometry,
    font_size: Pixels,
}

impl TimePlot {
    pub(super) fn new(view: &PanelView, options: &TimeSeriesOptions) -> Self {
        let domains = [false, true].map(|right| domain(&view.data.rows, &view.data.series, right));
        Self {
            id: view.element_id("time-plot").into(),
            rows: view.data.rows.clone(),
            series: view.data.series.clone(),
            options: options.clone(),
            domains,
            geometry: Geometry {
                left: 0.,
                width: 1.,
                top: 0.,
                height: 1.,
            },
            font_size: px(10.),
        }
    }

    fn axis_field(&self, right: bool) -> Option<&FieldSpec> {
        self.series
            .iter()
            .find(|s| {
                s.right_axis == right
                    && s.field.axis != AxisPlacement::Hidden
                    && s.field.axis_visible
            })
            .map(|s| s.field.as_ref())
    }

    fn x(&self, index: usize) -> f32 {
        let g = self.geometry;
        g.left
            + if self.rows.len() > 1 && self.rows.last().unwrap().x.time > self.rows[0].x.time {
                let start = self.rows[0].x.time;
                let span = self.rows.last().unwrap().x.time - start;
                g.width * ((self.rows[index].x.time - start) / span) as f32
            } else {
                g.width / 2.
            }
    }

    fn limits(&self) -> Vec<(GraphThreshold, bool)> {
        let mut limits: Vec<(GraphThreshold, bool)> = self
            .options
            .thresholds
            .iter()
            .cloned()
            .map(|t| (t, false))
            .collect();
        for series in &self.series {
            let f = &series.field;
            let style = f.threshold_style;
            if style == ThresholdStyle::Off {
                continue;
            }
            let domain = self.domains[usize::from(series.right_axis)];
            let convert = |v: f64| {
                if f.percent_steps && v.is_finite() {
                    let min = f.min.unwrap_or(domain.min);
                    let max = f.max.unwrap_or(domain.max);
                    min + (max - min) * v / 100.
                } else {
                    v
                }
            };
            for (i, step) in f.steps.iter().enumerate() {
                let mut threshold = GraphThreshold::new(
                    convert(step.value),
                    f.steps
                        .get(i + 1)
                        .map(|s| convert(s.value))
                        .unwrap_or(f64::INFINITY),
                );
                threshold.line = (step.value.is_finite()
                    && matches!(
                        style,
                        ThresholdStyle::Line
                            | ThresholdStyle::LineAndArea
                            | ThresholdStyle::Dashed
                            | ThresholdStyle::DashedAndArea
                    ))
                .then_some(step.color);
                threshold.fill = (step.color.alpha() > 0
                    && matches!(
                        style,
                        ThresholdStyle::Area
                            | ThresholdStyle::LineAndArea
                            | ThresholdStyle::DashedAndArea
                    ))
                .then_some(grafaui_model::color::Rgba(
                    (step.color.0 & 0xffffff00) | (step.color.alpha() as u32 * 28 / 255),
                ));
                threshold.right_axis = series.right_axis;
                let dashed = matches!(
                    style,
                    ThresholdStyle::Dashed | ThresholdStyle::DashedAndArea
                );
                if !limits.iter().any(|(t, d)| {
                    t.value == threshold.value
                        && t.end == threshold.end
                        && t.line == threshold.line
                        && t.fill == threshold.fill
                        && t.right_axis == threshold.right_axis
                        && *d == dashed
                }) {
                    limits.push((threshold, dashed));
                }
            }
        }
        limits
    }
}

fn domain(rows: &[ChartRow], series: &[SeriesStat], right: bool) -> Domain {
    let fields: Vec<_> = series
        .iter()
        .enumerate()
        .filter(|(_, s)| s.right_axis == right)
        .collect();
    let percent = fields
        .iter()
        .any(|(_, s)| s.stacking.mode == StackMode::Percent);
    let scale = if percent {
        AxisScale::Linear
    } else {
        fields
            .first()
            .map_or(AxisScale::Linear, |(_, s)| s.field.scale)
    };
    let values: Vec<_> = rows
        .iter()
        .flat_map(|r| {
            fields
                .iter()
                .flat_map(move |(i, _)| [r.ys[*i], r.bases[*i]])
        })
        .filter(|v| v.is_finite() && (scale == AxisScale::Linear || *v > 0.))
        .collect();
    let mut min = values
        .iter()
        .copied()
        .reduce(f64::min)
        .unwrap_or(if scale == AxisScale::Linear { 0. } else { 1. });
    let mut max = values.iter().copied().reduce(f64::max).unwrap_or(min);
    if percent {
        min = if min < 0. { -100. } else { 0. };
        max = if max > 0. { 100. } else { 0. };
    } else if let AxisScale::Log(base) = scale {
        min = fields
            .iter()
            .find_map(|(_, s)| s.field.min.filter(|v| *v > 0.))
            .unwrap_or(base.powf(min.log(base).floor()));
        max = fields
            .iter()
            .find_map(|(_, s)| s.field.max.filter(|v| *v > min))
            .unwrap_or(base.powf(max.log(base).ceil()));
    } else {
        let margin = (max - min) * 0.05;
        min = fields
            .iter()
            .find_map(|(_, s)| s.field.min)
            .unwrap_or(if min < 0. { min - margin } else { min });
        max = fields
            .iter()
            .find_map(|(_, s)| s.field.max)
            .unwrap_or(max + margin);
    }
    if let AxisScale::Log(base) = scale
        && max <= min
        && fields.iter().all(|(_, s)| s.field.min.is_none())
    {
        min /= base;
        max *= base;
    }
    if max <= min {
        max = if let AxisScale::Log(base) = scale {
            min * base
        } else {
            min + min.abs().max(1.)
        };
    }
    Domain {
        min,
        max,
        scale,
        percent,
    }
}

impl Plot for TimePlot {
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut App,
    ) -> Vec<AnyElement> {
        self.font_size = dp_px(10., window);
        let mut gutter = |right| {
            self.axis_field(right)
                .map(|f| {
                    (0..5)
                        .map(|i| {
                            measure_text_width(
                                &SharedString::from(self.domains[usize::from(right)].format(
                                    f,
                                    self.domains[usize::from(right)].value(i as f64 / 4.),
                                )),
                                self.font_size,
                                window,
                            )
                        })
                        .fold(0., f32::max)
                        + dp_px(8., window).as_f32()
                })
                .unwrap_or(0.)
        };
        let left = gutter(false);
        let right = gutter(true);
        let top = self.font_size.as_f32() / 2.;
        self.geometry = Geometry {
            left,
            width: (bounds.size.width.as_f32() - left - right).max(1.),
            top,
            height: (bounds.size.height.as_f32() - axis_gutter(self.font_size) - top).max(1.),
        };
        vec![]
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let g = self.geometry;
        let plot_bounds = Bounds::new(
            bounds.origin + point(px(g.left), px(g.top)),
            size(px(g.width), px(g.height)),
        );
        let muted = cx.theme().muted_foreground;
        Grid::new()
            .y((0..5).map(|i| px(g.height * i as f32 / 4.)))
            .stroke(cx.theme().border)
            .dash_array(&[px(3.), px(3.)])
            .paint(&plot_bounds, window);
        let limits = self.limits();
        let mask = ContentMask {
            bounds: plot_bounds,
        };
        window.with_content_mask(Some(mask), |window| {
            for (t, _) in &limits {
                if let Some(color) = t.fill {
                    let domain = self.domains[usize::from(t.right_axis)];
                    let a = domain.project(t.value.clamp(domain.min, domain.max), g.height);
                    let b = domain.project(t.end.clamp(domain.min, domain.max), g.height);
                    window.paint_quad(fill(
                        Bounds::new(
                            plot_bounds.origin + point(px(0.), px(a.min(b))),
                            size(px(g.width), px((b - a).abs())),
                        ),
                        ui::hsla(color),
                    ));
                }
            }
            let caches = PathCaches::for_paint("series", window, cx);
            caches.update(cx, |caches, _| {
                let order: Vec<_> = if self.options.stacking.mode != StackMode::None {
                    (0..self.series.len()).rev().collect()
                } else {
                    (0..self.series.len()).collect()
                };
                let mut cache_slot = 0;
                for i in order {
                    let s = &self.series[i];
                    let domain = self.domains[usize::from(s.right_axis)];
                    let points: Vec<_> = self
                        .rows
                        .iter()
                        .enumerate()
                        .map(|(j, row)| (self.x(j) - g.left, domain.project(row.ys[i], g.height)))
                        .collect();
                    let curve = match self.options.curve {
                        Curve::Linear => PlotCurve::Linear,
                        Curve::Smooth => PlotCurve::Natural,
                        Curve::Step => PlotCurve::StepAfter,
                    };
                    let opacity = s.fill.unwrap_or(self.options.fill_opacity);
                    let draw = s.style.draw.unwrap_or(self.options.draw);
                    let points_only = draw == DrawStyle::Points;
                    let gradient = s.style.gradient.unwrap_or(self.options.gradient);
                    let scheme = gradient == GradientMode::Scheme;
                    let scale = super::color_scale::Scale::new(&s.field, s.color_range, s.color);
                    if draw == DrawStyle::Bars {
                        let (slot, slots) = bar_slot(&self.series, self.options.draw, i);
                        let group_width = g.width / self.rows.len().max(1) as f32 * 0.8;
                        let width = (group_width / slots as f32).max(1.);
                        let offset = (slot as f32 + 0.5) * width - group_width / 2.;
                        for (j, &(x, y)) in points.iter().enumerate() {
                            let zero = domain.project(
                                if domain.scale == AxisScale::Linear {
                                    self.rows[j].bases[i]
                                } else {
                                    self.rows[j].bases[i].max(domain.min)
                                },
                                g.height,
                            );
                            if y.is_finite() {
                                let bounds = Bounds::new(
                                    plot_bounds.origin
                                        + point(px(x + offset - width / 2.), px(y.min(zero))),
                                    size(px(width * 0.9), px((zero - y).abs().max(1.))),
                                );
                                if scheme {
                                    for band in scale.bands(domain.min, domain.max) {
                                        let top = domain.project(band.max, g.height);
                                        let bottom = domain.project(band.min, g.height);
                                        window.with_content_mask(
                                            Some(ContentMask {
                                                bounds: Bounds::new(
                                                    plot_bounds.origin + point(px(0.), px(top)),
                                                    size(px(g.width), px(bottom - top)),
                                                ),
                                            }),
                                            |window| {
                                                let height = bounds.size.height.as_f32();
                                                window.paint_quad(fill(
                                                    bounds,
                                                    linear_gradient(
                                                        180.,
                                                        linear_color_stop(
                                                            ui::hsla(band.to),
                                                            (plot_bounds.origin.y.as_f32() + top
                                                                - bounds.origin.y.as_f32())
                                                                / height,
                                                        ),
                                                        linear_color_stop(
                                                            ui::hsla(band.from),
                                                            (plot_bounds.origin.y.as_f32()
                                                                + bottom
                                                                - bounds.origin.y.as_f32())
                                                                / height,
                                                        ),
                                                    ),
                                                ));
                                            },
                                        );
                                    }
                                } else {
                                    window.paint_quad(fill(bounds, s.color));
                                }
                            }
                        }
                        continue;
                    }
                    // Split at missing samples; never bridge a gap with a line.
                    for segment in points
                        .split(|(_, y)| !y.is_finite())
                        .filter(|s| !s.is_empty())
                    {
                        if opacity > 0. && !points_only {
                            if s.stacking.mode != StackMode::None {
                                let lower: Vec<_> = segment
                                    .iter()
                                    .map(|&(x, _)| {
                                        let j = points.iter().position(|p| p.0 == x).unwrap();
                                        (
                                            x,
                                            domain.project(
                                                if domain.scale == AxisScale::Linear {
                                                    self.rows[j].bases[i]
                                                } else {
                                                    self.rows[j].bases[i].max(domain.min)
                                                },
                                                g.height,
                                            ),
                                        )
                                    })
                                    .collect();
                                let mut key = ShapeKey::new(curve);
                                for &(x, y) in segment.iter().chain(&lower) {
                                    key.f32(x).f32(y);
                                }
                                if let Some(path) = caches.slot(2 * cache_slot).get(
                                    key.finish(),
                                    plot_bounds.origin,
                                    || band_path(segment, &lower, curve),
                                ) {
                                    if scheme {
                                        scale.paint_path(
                                            path,
                                            plot_bounds,
                                            (domain.min, domain.max),
                                            domain.scale,
                                            opacity,
                                            window,
                                        );
                                    } else {
                                        let fill_color = if gradient != GradientMode::None {
                                            linear_gradient(
                                                180.,
                                                linear_color_stop(s.color.opacity(opacity), 0.),
                                                linear_color_stop(s.color.opacity(0.), 1.),
                                            )
                                        } else {
                                            gpui_kit::Background::from(s.color.opacity(opacity))
                                        };
                                        window.paint_path(path, fill_color);
                                    }
                                }
                            } else if scheme {
                                let baseline = domain.project(domain.baseline(), g.height);
                                let mut key = ShapeKey::new((curve, baseline.to_bits()));
                                for &(x, y) in segment {
                                    key.f32(x).f32(y);
                                }
                                if let Some(path) = caches.slot(2 * cache_slot).get(
                                    key.finish(),
                                    plot_bounds.origin,
                                    || area_path(segment, curve, baseline),
                                ) {
                                    scale.paint_path(
                                        path,
                                        plot_bounds,
                                        (domain.min, domain.max),
                                        domain.scale,
                                        opacity,
                                        window,
                                    );
                                }
                            } else {
                                let fill_color = if gradient != GradientMode::None {
                                    linear_gradient(
                                        180.,
                                        linear_color_stop(s.color.opacity(opacity), 0.),
                                        linear_color_stop(s.color.opacity(0.), 1.),
                                    )
                                } else {
                                    s.color.opacity(opacity).into()
                                };
                                let area = Area::new()
                                    .data(segment.iter().copied())
                                    .x(|p| Some(p.0))
                                    .y1(|p| Some(p.1))
                                    .y0(domain.project(domain.baseline(), g.height))
                                    .curve(curve)
                                    .fill(fill_color)
                                    .stroke(s.color.opacity(0.));
                                let (fill, stroke) = caches.slot_pair(cache_slot);
                                area.paint_cached(&plot_bounds, fill, stroke, window);
                            }
                        }
                        // The cache includes projected geometry and therefore zoom.
                        let pattern = s
                            .style
                            .line_style
                            .as_ref()
                            .unwrap_or(&self.options.line_style);
                        let width = dp_px(s.style.line_width.unwrap_or(1.), window);
                        let line_color = s.color.opacity(if points_only { 0. } else { 1. });
                        let dashed = matches!(pattern, LineStyle::Dashed(_));
                        if dashed || scheme {
                            let dashes: Vec<_> = match pattern {
                                LineStyle::Dashed(dashes) => {
                                    dashes.iter().map(|d| dp_px(*d, window)).collect()
                                }
                                _ => Vec::new(),
                            };
                            let mut key = ShapeKey::new((curve, width.as_f32().to_bits()));
                            for dash in &dashes {
                                key.f32(dash.as_f32());
                            }
                            for &(x, y) in segment {
                                key.f32(x).f32(y);
                            }
                            if let Some(path) = caches.slot(2 * cache_slot + 2).get(
                                key.finish(),
                                plot_bounds.origin,
                                || stroke_path(segment, curve, width, &dashes),
                            ) {
                                if scheme {
                                    scale.paint_path(
                                        path,
                                        plot_bounds,
                                        (domain.min, domain.max),
                                        domain.scale,
                                        if points_only { 0. } else { 1. },
                                        window,
                                    );
                                } else {
                                    window.paint_path(path, line_color);
                                }
                            }
                        }
                        let line = Line::new()
                            .data(segment.iter().copied())
                            .x(|p| Some(p.0))
                            .y(|p| Some(p.1))
                            .curve(curve)
                            .stroke_width(width)
                            .stroke(if dashed || scheme {
                                line_color.opacity(0.)
                            } else {
                                line_color
                            });
                        let line = if s.style.show_points.unwrap_or(self.options.show_points)
                            || points_only
                        {
                            line.dot()
                                .dot_size(dp_px(s.style.point_size.unwrap_or(4.), window))
                                .dot_fill(s.color)
                        } else {
                            line
                        };
                        // Dashed strokes use their own cache. Dots are cheap quads;
                        // the transparent line uses a different cache slot.
                        line.paint_cached(&plot_bounds, caches.slot(2 * cache_slot + 3), window);
                        cache_slot += 2;
                    }
                }
            });
            for (t, dashed) in &limits {
                let domain = self.domains[usize::from(t.right_axis)];
                if let Some(color) = t
                    .line
                    .filter(|_| t.value >= domain.min && t.value <= domain.max)
                {
                    let y = domain.project(t.value, g.height);
                    let mut path = gpui_kit::PathBuilder::stroke(px(1.));
                    if *dashed {
                        path = path.dash_array(&[px(6.), px(4.)]);
                    }
                    path.move_to(plot_bounds.origin + point(px(0.), px(y)));
                    path.line_to(plot_bounds.origin + point(px(g.width), px(y)));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, ui::hsla(color));
                    }
                }
            }
        });
        let axis_bounds = Bounds::new(bounds.origin + point(px(0.), px(g.top)), bounds.size);
        for right in [false, true] {
            if let Some(field) = self.axis_field(right) {
                let domain = self.domains[usize::from(right)];
                PlotAxis::new()
                    .y(px(if right { g.left + g.width } else { g.left }))
                    .y_label_side(if right {
                        AxisLabelSide::End
                    } else {
                        AxisLabelSide::Start
                    })
                    .y_label(domain.ticks().into_iter().map(|value| {
                        AxisText::new(
                            domain.format(field, value),
                            px(domain.project(value, g.height)),
                            muted,
                        )
                        .font_size(self.font_size)
                        .align(if right {
                            TextAlign::Left
                        } else {
                            TextAlign::Right
                        })
                    }))
                    .paint(&axis_bounds, window, cx);
            }
        }
        let count = ((g.width / 110.) as usize).clamp(2, 8).min(self.rows.len());
        let x_bounds = Bounds::new(
            plot_bounds.origin,
            size(px(g.width), bounds.size.height - px(g.top)),
        );
        PlotAxis::new()
            .x(px(g.height))
            .stroke(cx.theme().border)
            .x_label((0..count).map(|i| {
                let index = if count > 1 {
                    i * (self.rows.len() - 1) / (count - 1)
                } else {
                    0
                };
                AxisText::new(
                    self.rows[index].x.label.clone(),
                    px(self.x(index) - g.left),
                    muted,
                )
                .font_size(self.font_size)
                .align(if i == 0 {
                    TextAlign::Left
                } else if i + 1 == count {
                    TextAlign::Right
                } else {
                    TextAlign::Center
                })
            }))
            .paint(&x_bounds, window, cx);
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        _: Bounds<Pixels>,
        _: &App,
    ) -> Option<TooltipState> {
        let g = self.geometry;
        if self.rows.is_empty()
            || position.x.as_f32() < g.left
            || position.x.as_f32() > g.left + g.width
            || position.y.as_f32() < g.top
            || position.y.as_f32() > g.top + g.height
        {
            return None;
        }
        let index = (0..self.rows.len()).min_by(|&a, &b| {
            (self.x(a) - position.x.as_f32())
                .abs()
                .total_cmp(&(self.x(b) - position.x.as_f32()).abs())
        })?;
        let row = self.rows.get(index)?;
        let dots = self
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| {
                point(
                    px(self.x(index)),
                    px(g.top
                        + self.domains[usize::from(s.right_axis)].project(row.ys[i], g.height)),
                )
            })
            .collect();
        Some(TooltipState::new(
            index,
            point(px(self.x(index)), position.y),
            dots,
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
        let row = self.rows.get(state.index)?;
        let mut tip = Tooltip::new(cursor, bounds.size)
            .title(row.x.label.clone())
            .cross_line(
                CrossLine::new(state.cross_line).span(self.geometry.top, self.geometry.height),
            )
            .dots(
                state
                    .dots
                    .iter()
                    .zip(&self.series)
                    .filter(|(p, _)| p.y.as_f32().is_finite())
                    .map(|(p, s)| Dot::new(*p).fill(s.color).stroke(cx.theme().background)),
            );
        for (i, s) in self.series.iter().enumerate() {
            tip = tip.row(s.color, s.name.clone(), s.field.format(row.raw[i]));
        }
        Some(tip.into_any_element())
    }
}

fn stroke_path(
    points: &[(f32, f32)],
    curve: PlotCurve,
    width: Pixels,
    dashes: &[Pixels],
) -> Option<gpui_kit::Path<Pixels>> {
    let mut path = gpui_kit::PathBuilder::stroke(width);
    // Kit treats an empty dash array as a dash pattern, not a solid line.
    if !dashes.is_empty() {
        path = path.dash_array(dashes);
    }
    trace_path(&mut path, points, curve);
    path.build().ok()
}

fn area_path(
    points: &[(f32, f32)],
    curve: PlotCurve,
    baseline: f32,
) -> Option<gpui_kit::Path<Pixels>> {
    let mut path = gpui_kit::PathBuilder::fill();
    trace_path(&mut path, points, curve);
    path.line_to(point(px(points.last()?.0), px(baseline)));
    path.line_to(point(px(points.first()?.0), px(baseline)));
    path.close();
    path.build().ok()
}

fn band_path(
    top: &[(f32, f32)],
    lower: &[(f32, f32)],
    curve: PlotCurve,
) -> Option<gpui_kit::Path<Pixels>> {
    let mut path = gpui_kit::PathBuilder::fill();
    trace_path(&mut path, top, curve);
    let lower: Vec<_> = lower.iter().rev().copied().collect();
    path.line_to(point(px(lower.first()?.0), px(lower.first()?.1)));
    trace_body(&mut path, &lower, curve, true);
    path.close();
    path.build().ok()
}

fn trace_path(path: &mut gpui_kit::PathBuilder, points: &[(f32, f32)], curve: PlotCurve) {
    let p = |i: usize| point(px(points[i].0), px(points[i].1));
    path.move_to(p(0));
    trace_body(path, points, curve, false);
}

fn trace_body(
    path: &mut gpui_kit::PathBuilder,
    points: &[(f32, f32)],
    curve: PlotCurve,
    reversed: bool,
) {
    let p = |i: usize| point(px(points[i].0), px(points[i].1));
    for i in 1..points.len() {
        match curve {
            PlotCurve::Linear => path.line_to(p(i)),
            PlotCurve::StepAfter => {
                path.line_to(if reversed {
                    point(p(i - 1).x, p(i).y)
                } else {
                    point(p(i).x, p(i - 1).y)
                });
                path.line_to(p(i));
            }
            PlotCurve::Natural => {
                let p0 = p(i.saturating_sub(2));
                let p1 = p(i - 1);
                let p2 = p(i);
                let p3 = p((i + 1).min(points.len() - 1));
                path.cubic_bezier_to(
                    p2,
                    point(p1.x + (p2.x - p0.x) / 6., p1.y + (p2.y - p0.y) / 6.),
                    point(p2.x - (p3.x - p1.x) / 6., p2.y - (p3.y - p1.y) / 6.),
                );
            }
        }
    }
}

/// Only bars in the same stack and on the same axis share a horizontal slot.
fn bar_slot(series: &[super::SeriesStat], default: DrawStyle, index: usize) -> (usize, usize) {
    let mut slots: Vec<usize> = Vec::new();
    let mut selected = 0;
    for (i, s) in series
        .iter()
        .enumerate()
        .filter(|(_, s)| s.style.draw.unwrap_or(default) == DrawStyle::Bars)
    {
        let slot = slots
            .iter()
            .position(|&j| {
                s.stacking.mode != StackMode::None
                    && series[j].stacking == s.stacking
                    && series[j].right_axis == s.right_axis
            })
            .unwrap_or_else(|| {
                slots.push(i);
                slots.len() - 1
            });
        if i == index {
            selected = slot;
        }
    }
    (selected, slots.len().max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn fractional_irregular_timestamps_project_by_time_and_keep_missing_values() {
        let dashboard =
            grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries"}]}"#).unwrap();
        let spec = Rc::new(dashboard.panel(0).clone());
        let view = super::super::PanelView::new(
            spec,
            Default::default(),
            "timestamps".into(),
            grafaui_model::data::Frame {
                times: vec![100.125, 100.625, 102.125],
                series: vec![grafaui_model::data::Series {
                    name: "A".into(),
                    query: "A".into(),
                    field: None,
                    labels: vec![],
                    values: vec![1., f64::NAN, 3.],
                }],
            },
            2,
        );
        let grafaui_model::Viz::TimeSeries(options) = &view.spec.viz else {
            unreachable!()
        };
        let mut plot = TimePlot::new(&view, options);
        plot.geometry.left = 10.;
        plot.geometry.width = 100.;
        assert_eq!(plot.x(0), 10.);
        assert_eq!(plot.x(1), 35.);
        assert_eq!(plot.x(2), 110.);
        assert!(plot.rows[1].raw[0].is_nan());
        assert!(plot.rows[1].ys[0].is_nan());
    }
    #[test]
    fn scheme_strokes_without_dashes_build_a_solid_path() {
        for curve in [PlotCurve::Linear, PlotCurve::Natural, PlotCurve::StepAfter] {
            assert!(
                stroke_path(&[(0., 20.), (10., 40.), (20., 30.)], curve, px(1.), &[]).is_some()
            );
        }
    }
    #[test]
    fn domains_project_independent_units_and_negative_values() {
        let left = Domain {
            min: -100.,
            max: 100.,
            scale: AxisScale::Linear,
            percent: false,
        };
        let right = Domain {
            min: 0.,
            max: 1_000_000.,
            scale: AxisScale::Linear,
            percent: false,
        };
        assert_eq!(left.project(0., 200.), 100.);
        assert_eq!(right.project(500_000., 200.), 100.);
        assert_eq!(right.value(0.5), 500_000.);
    }

    #[test]
    fn series_colors_use_the_selected_reducer_and_latest_override() {
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"min":0,"max":100},"overrides":[
            {"matcher":{"id":"byName","options":"a"},"properties":[{"id":"color","value":{"mode":"fixed","fixedColor":"blue"}}]},
            {"matcher":{"id":"byName","options":"a"},"properties":[{"id":"color","value":{"mode":"continuous-GrYlRd","seriesBy":"mean"}}]}
        ]}}]}"#).unwrap();
        let frame = grafaui_model::data::Frame {
            times: vec![0., 1.],
            series: vec![grafaui_model::data::Series {
                name: "a".into(),
                query: "A".into(),
                field: None,
                labels: Vec::new(),
                values: vec![90., 10.],
            }],
        };
        let data = super::super::derive(&dashboard.panels[0], frame, 1);
        assert_eq!(data.series[0].value, 10.);
        assert_eq!(data.series[0].color_range, (0., 100.));
        assert_eq!(
            data.series[0].color,
            ui::hsla(grafaui_model::color::ColorScheme::GreenYellowRed.sample(0.5))
        );
    }

    #[test]
    fn axis_stacking_tooltip_values_and_renamed_fields_stay_independent() {
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"unit":"bytes","custom":{"stacking":{"mode":"normal"}}},"overrides":[
            {"matcher":{"id":"byName","options":"right"},"properties":[{"id":"custom.axisPlacement","value":"right"},{"id":"unit","value":"percent"},{"id":"displayName","value":"Renamed"}]},
            {"matcher":{"id":"byValue","options":{"reducer":"allIsZero","op":"gte","value":0}},"properties":[{"id":"custom.hideFrom","value":{"viz":true,"legend":true}}]}
        ]}}]}"#).unwrap();
        let spec = &dashboard.panels[0];
        let series = |name: &str, values: Vec<f64>| grafaui_model::data::Series {
            name: name.into(),
            query: "A".into(),
            field: None,
            labels: vec![],
            values,
        };
        let frame = grafaui_model::data::Frame {
            times: vec![0., 1.],
            series: vec![
                series("left", vec![1000., 2000.]),
                series("left2", vec![2000., 3000.]),
                series("right", vec![1., 2.]),
                series("zero", vec![0., 0.]),
            ],
        };
        let data = super::super::derive(spec, frame, 1);
        assert_eq!(data.series.len(), 3);
        assert_eq!(data.series[2].name.as_ref(), "Renamed");
        assert_eq!(data.series[2].field.unit.as_deref(), Some("percent"));
        assert_eq!(&*data.rows[0].ys, &[1000., 3000., 1.]);
        assert_eq!(&*data.rows[0].raw, &[1000., 2000., 1.]);
        assert_eq!(domain(&data.rows, &data.series, false).max, 5250.);
        assert_eq!(domain(&data.rows, &data.series, true).max, 2.1);
    }

    #[test]
    fn percentage_limits_use_field_range_and_preserve_transparent_base() {
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"min":10,"max":210,
            "custom":{"thresholdsStyle":{"mode":"line+area"}},"thresholds":{"mode":"percentage","steps":[{"color":"transparent","value":null},{"color":"red","value":80}]}
        }}}]}"#).unwrap();
        let spec = std::rc::Rc::new(dashboard.panels[0].clone());
        let frame = grafaui_model::data::Frame {
            times: vec![0., 1.],
            series: vec![grafaui_model::data::Series {
                name: "a".into(),
                query: "A".into(),
                field: None,
                labels: vec![],
                values: vec![20., 100.],
            }],
        };
        let view = PanelView::new(
            spec.clone(),
            grafaui_model::schema::GridPos::default(),
            String::new(),
            frame,
            1,
        );
        let grafaui_model::spec::Viz::TimeSeries(options) = &spec.viz else {
            panic!()
        };
        let plot = TimePlot::new(&view, options);
        let limits = plot.limits();
        assert!(limits[0].0.fill.is_none());
        assert_eq!(limits[1].0.value, 170.);
        assert!(limits[1].0.line.is_some() && limits[1].0.fill.is_some());
        assert_eq!(plot.domains[0].project(170., 200.), 40.);
    }
    #[test]
    fn log_axes_space_powers_evenly_and_skip_nonpositive_values() {
        let d = Domain {
            min: 1.,
            max: 1000.,
            scale: AxisScale::Log(10.),
            percent: false,
        };
        assert_eq!(d.project(10., 300.), 200.);
        assert_eq!(d.project(100., 300.), 100.);
        assert!(d.project(0., 300.).is_nan());
        assert!(d.project(-1., 300.).is_nan());
        assert!((d.value(1. / 3.) - 10.).abs() < 1e-8);
        assert_eq!(d.ticks(), [1., 10., 100., 1000.]);
    }
    #[test]
    fn percent_stacks_keep_raw_values_and_independent_bar_slots() {
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"unit":"percentunit","custom":{"drawStyle":"bars","stacking":{"mode":"percent","group":"A"}}},"overrides":[{"matcher":{"id":"byName","options":"limit"},"properties":[{"id":"custom.stacking","value":{"mode":"none","group":"A"}}]}]}}]}"#).unwrap();
        let frame = grafaui_model::data::Frame {
            times: vec![0.],
            series: [("a", 1.), ("b", 3.), ("limit", 0.9)]
                .into_iter()
                .map(|(name, value)| grafaui_model::data::Series {
                    name: name.into(),
                    query: "A".into(),
                    field: None,
                    labels: vec![],
                    values: vec![value],
                })
                .collect(),
        };
        let data = super::super::derive(&dashboard.panels[0], frame, 1);
        assert_eq!(&*data.rows[0].ys, [25., 100., 90.]);
        assert_eq!(&*data.rows[0].bases, [0., 25., 0.]);
        assert_eq!(&*data.rows[0].raw, [1., 3., 0.9]);
        let d = domain(&data.rows, &data.series, false);
        assert_eq!((d.min, d.max), (0., 100.));
        assert_eq!(d.format(&data.series[0].field, 100.), "100%");
        assert_eq!(bar_slot(&data.series, DrawStyle::Bars, 0), (0, 2));
        assert_eq!(bar_slot(&data.series, DrawStyle::Bars, 1), (0, 2));
        assert_eq!(bar_slot(&data.series, DrawStyle::Bars, 2), (1, 2));
    }
    #[test]
    fn constant_log_series_get_room_on_both_sides() {
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"custom":{"scaleDistribution":{"type":"log","log":2}}}}}]}"#).unwrap();
        let frame = grafaui_model::data::Frame {
            times: vec![0., 1.],
            series: vec![grafaui_model::data::Series {
                name: "a".into(),
                query: "A".into(),
                field: None,
                labels: vec![],
                values: vec![1., 1.],
            }],
        };
        let data = super::super::derive(&dashboard.panels[0], frame, 1);
        let d = domain(&data.rows, &data.series, false);
        assert_eq!((d.min, d.max), (0.5, 2.));
        assert_eq!(d.project(1., 200.), 100.);
    }
    #[test]
    fn frame_matchers_separate_equal_named_series_on_two_axes() {
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"timeseries","fieldConfig":{"defaults":{"unit":"bytes"},"overrides":[
          {"matcher":{"id":"byFrameRefID","options":"B"},"properties":[{"id":"unit","value":"percent"},{"id":"custom.axisPlacement","value":"right"}]}
        ]}}]}"#).unwrap();
        let frame = grafaui_model::data::Frame {
            times: vec![0., 1.],
            series: [("A", vec![1024., 2048.]), ("B", vec![50., 80.])]
                .into_iter()
                .map(|(query, values)| grafaui_model::data::Series {
                    query: query.into(),
                    name: "same".into(),
                    field: None,
                    labels: Vec::new(),
                    values,
                })
                .collect(),
        };
        let data = super::super::derive(&dashboard.panels[0], frame, 1);
        assert!(!data.series[0].right_axis);
        assert!(data.series[1].right_axis);
        assert_eq!(data.series[0].field.unit.as_deref(), Some("bytes"));
        assert_eq!(data.series[1].field.unit.as_deref(), Some("percent"));
        assert_eq!(&*data.rows[1].raw, &[2048., 80.]);
        assert!(domain(&data.rows, &data.series, false).max > 2048.);
        assert_eq!(domain(&data.rows, &data.series, true).max, 84.);
    }
}
