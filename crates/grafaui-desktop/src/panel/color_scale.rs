//! Value-scale colors shared by chart paths, table gauges and bar gauges.
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, ContentMask, Hsla, Path, Pixels, Window, div, linear_color_stop,
    linear_gradient, point, px, relative, size,
};
use grafaui_model::color::{self, Rgba};
use grafaui_model::spec::{CellGauge, ColorMode, FieldSpec};

use crate::ui::{dp, hsla};

pub(super) struct Band {
    pub min: f64,
    pub max: f64,
    pub from: Rgba,
    pub to: Rgba,
}

pub(super) struct Scale<'a> {
    field: &'a FieldSpec,
    range: (f64, f64),
    color: Hsla,
}

impl<'a> Scale<'a> {
    pub(super) fn new(field: &'a FieldSpec, range: (f64, f64), color: Hsla) -> Self {
        Self {
            field,
            range: field.color_range(range),
            color,
        }
    }

    /// Clip to the visible scale, retaining exact threshold boundaries.
    pub(super) fn bands(&self, min: f64, max: f64) -> Vec<Band> {
        Self::bands_from(
            &self.field.gradient_stops(self.range),
            self.field.color == ColorMode::Thresholds,
            min,
            max,
        )
    }

    fn bands_from(stops: &[(f64, Rgba)], discrete: bool, min: f64, max: f64) -> Vec<Band> {
        if max <= min {
            return Vec::new();
        }
        let mut cuts = vec![min, max];
        cuts.extend(
            stops
                .iter()
                .map(|s| s.0)
                .filter(|v| v.is_finite() && *v > min && *v < max),
        );
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        let color = |v| {
            let i = stops.iter().rposition(|s| v >= s.0).unwrap_or(0);
            let (low, from) = stops[i];
            if discrete {
                return from;
            }
            match stops.get(i + 1) {
                Some(&(high, to)) if high > low => {
                    color::interpolate(&[from, to], (v - low) / (high - low))
                }
                _ => from,
            }
        };
        cuts.windows(2)
            .map(|pair| {
                let (min, max) = (pair[0], pair[1]);
                let (from, to) = if discrete {
                    let color = color((min + max) / 2.);
                    (color, color)
                } else {
                    (color(min), color(max))
                };
                Band { min, max, from, to }
            })
            .collect()
    }

    fn bar_bands(&self, value: f64) -> Vec<Band> {
        let (min, max) = self.range;
        let end = value.clamp(min, max);
        match self.field.color {
            ColorMode::Palette | ColorMode::Fixed(_) => Vec::new(),
            ColorMode::Continuous(_) => self.bands(min, end),
            ColorMode::Thresholds => {
                // Bar-gauge gradients blend reached threshold colors;
                // chart scheme gradients keep threshold boundaries sharp.
                let mut stops = vec![(min, self.field.threshold_color_in_range(min, self.range))];
                stops.extend(
                    self.field
                        .gradient_stops(self.range)
                        .into_iter()
                        .filter(|(v, _)| *v > min && *v <= end),
                );
                Self::bands_from(&stops, false, min, end)
            }
        }
    }

    /// GPUI 0.7 gradients have two stops. Paint each neighboring pair under
    /// a mask, with stop positions relative to the cached path's bounds.
    pub(super) fn paint_path(
        &self,
        path: Path<Pixels>,
        plot: Bounds<Pixels>,
        domain: (f64, f64),
        axis: grafaui_model::chart::AxisScale,
        opacity: f32,
        window: &mut Window,
    ) {
        let height = plot.size.height.as_f32();
        let (min, max) = domain;
        let (low, high) = (axis.project(min), axis.project(max));
        let project = |value| {
            plot.origin.y.as_f32()
                + (height as f64 * (high - axis.project(value)) / (high - low)) as f32
        };
        let value = |y: f32| {
            axis.invert(high - f64::from((y - plot.origin.y.as_f32()) / height) * (high - low))
        };
        let top = path.bounds.origin.y.as_f32().max(plot.origin.y.as_f32());
        let bottom = path.bounds.bottom().as_f32().min(plot.bottom().as_f32());
        let path_height = path.bounds.size.height.as_f32().max(0.001);
        for band in self.bands(value(bottom), value(top)) {
            let y0 = project(band.max);
            let y1 = project(band.min);
            let background = linear_gradient(
                180.,
                linear_color_stop(
                    hsla(band.to).opacity(opacity),
                    (y0 - path.bounds.origin.y.as_f32()) / path_height,
                ),
                linear_color_stop(
                    hsla(band.from).opacity(opacity),
                    (y1 - path.bounds.origin.y.as_f32()) / path_height,
                ),
            );
            window.with_content_mask(
                Some(ContentMask {
                    bounds: Bounds::new(
                        point(plot.origin.x, px(y0)),
                        size(plot.size.width, px((y1 - y0).max(0.))),
                    ),
                }),
                |window| window.paint_path(path.clone(), background),
            );
        }
    }

    pub(super) fn bar(
        &self,
        value: f64,
        mode: CellGauge,
        track: Hsla,
        show_unfilled: bool,
    ) -> AnyElement {
        let (min, max) = self.range;
        let fraction = if value.is_finite() {
            ((value - min) / (max - min)).clamp(0., 1.)
        } else {
            0.
        };
        if mode == CellGauge::Lcd {
            return h_flex()
                .w_full()
                .h_full()
                .gap(dp(1.))
                .children((0..40).map(|i| {
                    let active = f64::from(i) < fraction * 40.;
                    let color = if active {
                        if matches!(self.field.color, ColorMode::Palette | ColorMode::Fixed(_)) {
                            self.color
                        } else {
                            hsla(self.field.series_color_in_range(
                                0,
                                min + (max - min) * (f64::from(i) + 0.5) / 40.,
                                self.range,
                            ))
                        }
                    } else {
                        track.opacity(if show_unfilled { 1. } else { 0. })
                    };
                    div().flex_1().min_w_0().h_full().bg(color)
                }))
                .into_any_element();
        }
        let mut bar = div()
            .relative()
            .w_full()
            .h_full()
            .overflow_hidden()
            .when(show_unfilled, |b| b.bg(track));
        if mode == CellGauge::Basic
            || matches!(self.field.color, ColorMode::Palette | ColorMode::Fixed(_))
        {
            bar = bar.child(div().h_full().w(relative(fraction as f32)).bg(self.color));
        } else {
            let end = min + (max - min) * fraction;
            bar = bar.children(self.bar_bands(end).into_iter().map(|b| {
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(((b.min - min) / (max - min)) as f32))
                    .w(relative(((b.max - b.min) / (max - min)) as f32))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(hsla(b.from), 0.),
                        linear_color_stop(hsla(b.to), 1.),
                    ))
            }));
        }
        bar.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafaui_model::Dashboard;

    fn make_field(config: &str) -> FieldSpec {
        Dashboard::parse(&format!(
            r#"{{"panels":[{{"type":"timeseries","fieldConfig":{{"defaults":{config}}}}}]}}"#
        ))
        .unwrap()
        .panels
        .remove(0)
        .field
    }

    #[test]
    fn gradients_follow_value_bounds_and_threshold_steps() {
        let field = make_field(r#"{"min":10,"max":110,"color":{"mode":"continuous-GrYlRd"}}"#);
        let bands = Scale::new(&field, (0., 200.), hsla(color::GREEN)).bands(0., 200.);
        assert_eq!(bands.len(), 4);
        assert_eq!((bands[1].min, bands[1].max), (10., 60.));
        assert_eq!(bands[0].from, color::parse("green").unwrap());
        assert_eq!(bands[3].to, color::parse("red").unwrap());
        let field = make_field(
            r#"{"min":10,"max":110,"color":{"mode":"thresholds"},"thresholds":{"mode":"percentage","steps":[{"value":null,"color":"transparent"},{"value":80,"color":"red"}]}}"#,
        );
        let bands = Scale::new(&field, (0., 200.), hsla(color::GREEN)).bands(0., 110.);
        assert_eq!(bands.len(), 2);
        assert_eq!(bands[0].max, 90.);
        assert_eq!(bands[0].from, Rgba(0));
        assert_eq!(bands[1].from, bands[1].to);
        let bars = Scale::new(&field, (0., 200.), hsla(color::GREEN)).bar_bands(110.);
        assert_ne!(
            bars[0].from, bars[0].to,
            "bar gradients blend the reached threshold colors"
        );
    }
}
