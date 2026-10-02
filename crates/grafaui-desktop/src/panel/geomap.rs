//! Geographic presentation drawn with Kit plot paths, circles and hover overlays.
use super::{PanelView, no_data};
use crate::ui::{self, dp, dp_px};
use gpui_kit::component::plot::{
    IntoPlot, PathCaches, Plot, ShapeKey, TooltipState,
    label::{PlotLabel, Text},
    tooltip::Tooltip,
};
use gpui_kit::component::{ActiveTheme, Sizable, button::Button, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, ElementId, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ScrollDelta, ScrollWheelEvent, Window, div, fill, point, px, size,
};
use grafaui_model::geomap::{self, LayerKind, MapData, Options};
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct ViewState {
    center: (f64, f64),
    zoom: f64,
    drag: Option<Point<Pixels>>,
}
impl ViewState {
    pub(super) fn new(options: &Options) -> Self {
        Self {
            center: geomap::mercator(options.latitude, options.longitude),
            zoom: options.zoom,
            drag: None,
        }
    }
    fn zoom(&mut self, change: f64) {
        self.zoom = (self.zoom + change).clamp(0., 12.);
    }
    fn pan(&mut self, delta: Point<Pixels>, tile_size: f64) {
        let world = tile_size * 2_f64.powf(self.zoom);
        self.center.0 = (self.center.0 - f64::from(delta.x.as_f32()) / world).rem_euclid(1.);
        self.center.1 = (self.center.1 - f64::from(delta.y.as_f32()) / world).clamp(0., 1.);
    }
    fn fit(&mut self, data: &MapData, width: f64, height: f64, tile: f64) {
        let points: Vec<_> = data
            .layers
            .iter()
            .flatten()
            .map(|p| geomap::mercator(p.latitude, p.longitude))
            .collect();
        if points.is_empty() {
            return;
        }
        let min_x = points.iter().map(|p| p.0).reduce(f64::min).unwrap();
        let max_x = points.iter().map(|p| p.0).reduce(f64::max).unwrap();
        let min_y = points.iter().map(|p| p.1).reduce(f64::min).unwrap();
        let max_y = points.iter().map(|p| p.1).reduce(f64::max).unwrap();
        self.center = ((min_x + max_x) / 2., (min_y + max_y) / 2.);
        self.zoom = (width / (tile * (max_x - min_x).max(0.001)))
            .min(height / (tile * (max_y - min_y).max(0.001)))
            .log2()
            .clamp(0., 12.);
    }
}

pub(super) fn render(
    view: &PanelView,
    options: &Options,
    window: &Window,
    cx: &mut Context<PanelView>,
) -> AnyElement {
    let Some(data) = view
        .data
        .geomap
        .as_ref()
        .filter(|d| d.layers.iter().any(|l| !l.is_empty()))
    else {
        return no_data(cx);
    };
    let mut initial = ViewState::new(options);
    if options.fit {
        let body = view.body_size(window);
        initial.fit(
            data,
            body.width.as_f32() as f64 * 0.8,
            body.height.as_f32() as f64 * 0.8,
            dp_px(256., window).as_f32() as f64,
        );
    }
    let state = view.geo_view.as_ref().unwrap_or(&initial);
    let plot = GeoPlot {
        id: view.element_id("geo-plot").into(),
        data: data.clone(),
        options: options.clone(),
        center: state.center,
        zoom: state.zoom,
        world: 256. * 2_f64.powf(state.zoom),
        width: 1.,
        height: 1.,
        radius_scale: 1.,
    };
    let captured_state = state.clone();
    let zoom_state = state.clone();
    let wheel_state = state.clone();
    let map = div()
        .id(view.element_id("geo-map"))
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(cx.theme().background)
        .child(plot)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if this.geo_view.is_none() {
                    this.geo_view = Some(captured_state.clone());
                }
                if let Some(state) = this.geo_view.as_mut() {
                    state.drag = Some(event.position);
                }
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
            if let Some(state) = this.geo_view.as_mut() {
                if event.dragging() {
                    if let Some(previous) = state.drag.replace(event.position) {
                        state.pan(
                            event.position - previous,
                            dp_px(256., window).as_f32() as f64,
                        );
                        cx.notify();
                    }
                } else {
                    state.drag = None;
                }
            }
        }))
        .when(options.wheel_zoom, |this| {
            this.on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                let state = this.geo_view.get_or_insert_with(|| wheel_state.clone());
                {
                    let delta = match event.delta {
                        ScrollDelta::Pixels(p) => p.y.as_f32(),
                        ScrollDelta::Lines(p) => p.y * 16.,
                    };
                    state.zoom(f64::from(delta) * 0.005);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
        })
        .when(options.show_zoom, |this| {
            this.child(
                v_flex()
                    .absolute()
                    .top(dp(8.))
                    .left(dp(8.))
                    .gap(dp(2.))
                    .children(
                        [(1., "+", "Zoom in"), (-1., "−", "Zoom out")]
                            .into_iter()
                            .map(|(delta, label, tip)| {
                                let initial = zoom_state.clone();
                                Button::new(view.element_id(tip))
                                    .small()
                                    .label(label)
                                    .tooltip(tip)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.geo_view
                                            .get_or_insert_with(|| initial.clone())
                                            .zoom(delta);
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                            }),
                    )
                    .child(
                        Button::new(view.element_id("geo-reset"))
                            .small()
                            .label("↺")
                            .tooltip("Reset map view")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.geo_view = None;
                                cx.stop_propagation();
                                cx.notify();
                            })),
                    ),
            )
        });
    v_flex()
        .size_full()
        .gap(dp(4.))
        .child(div().flex_1().min_h_0().child(map))
        .children(options.layers.iter().filter(|l| l.legend).map(|layer| {
            let field = view
                .spec
                .field
                .for_series(layer.color_field.as_deref().unwrap_or(""));
            h_flex()
                .flex_none()
                .flex_wrap()
                .gap(dp(8.))
                .text_size(dp(10.))
                .text_color(cx.theme().muted_foreground)
                .child(layer.name.clone())
                .children(field.steps.iter().map(|step| {
                    h_flex()
                        .gap(dp(4.))
                        .child(div().size(dp(8.)).rounded_full().bg(ui::hsla(step.color)))
                        .child(if step.value.is_finite() {
                            format!("≥ {}", field.format(step.value))
                        } else {
                            field
                                .min
                                .map(|v| field.format(v))
                                .unwrap_or_else(|| "Low".into())
                        })
                }))
        }))
        .into_any_element()
}

#[derive(IntoPlot)]
struct GeoPlot {
    id: ElementId,
    data: Rc<MapData>,
    options: Options,
    center: (f64, f64),
    zoom: f64,
    world: f64,
    width: f32,
    height: f32,
    radius_scale: f32,
}
impl GeoPlot {
    fn position(&self, latitude: f64, longitude: f64) -> (f32, f32) {
        let (x, y) = geomap::mercator(latitude, longitude);
        (
            (((x - self.center.0 + 0.5).rem_euclid(1.) - 0.5) * self.world) as f32
                + self.width / 2.,
            ((y - self.center.1) * self.world) as f32 + self.height / 2.,
        )
    }
    fn nearest(&self, cursor: Point<Pixels>) -> Option<usize> {
        let mut best = None;
        let mut distance = f32::INFINITY;
        let mut index = 0;
        for (layer, points) in self.options.layers.iter().zip(&self.data.layers) {
            for p in points {
                let (x, y) = self.position(p.latitude, p.longitude);
                let d = (x - cursor.x.as_f32()).hypot(y - cursor.y.as_f32());
                if self.options.tooltip
                    && layer.tooltip
                    && p.value.is_finite()
                    && d < p.radius.max(12.) * self.radius_scale
                    && d < distance
                    && x >= 0.
                    && x < self.width
                    && y >= 0.
                    && y < self.height
                {
                    best = Some(index);
                    distance = d;
                }
                index += 1;
            }
        }
        best
    }
}
impl Plot for GeoPlot {
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut App,
    ) -> Vec<AnyElement> {
        self.radius_scale = dp_px(1., window).as_f32();
        self.width = bounds.size.width.as_f32();
        self.height = bounds.size.height.as_f32();
        self.world = dp_px(256., window).as_f32() as f64 * 2_f64.powf(self.zoom);
        Vec::new()
    }
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let land = cx.theme().secondary;
        let border = cx.theme().border;
        let caches = PathCaches::for_paint("geography", window, cx);
        window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
            caches.update(cx, |caches, _| {
                for (i, ring) in geomap::world_outline()
                    .iter()
                    .enumerate()
                    .filter(|_| self.options.basemap)
                {
                    let shift = (self.center.0 - ring[0].0).round();
                    let points: Vec<_> = ring
                        .iter()
                        .map(|&(x, y)| {
                            (
                                ((x + shift - self.center.0) * self.world) as f32 + self.width / 2.,
                                ((y - self.center.1) * self.world) as f32 + self.height / 2.,
                            )
                        })
                        .collect();
                    if points.iter().all(|p| p.0 < 0.)
                        || points.iter().all(|p| p.0 > self.width)
                        || points.iter().all(|p| p.1 < 0.)
                        || points.iter().all(|p| p.1 > self.height)
                    {
                        continue;
                    }
                    for (stroke, slot) in [(false, 2 * i), (true, 2 * i + 1)] {
                        let mut key = ShapeKey::new(stroke);
                        for &(x, y) in &points {
                            key.f32(x).f32(y);
                        }
                        if let Some(path) =
                            caches.slot(slot).get(key.finish(), bounds.origin, || {
                                let mut path = if stroke {
                                    gpui_kit::PathBuilder::stroke(dp_px(0.8, window))
                                } else {
                                    gpui_kit::PathBuilder::fill()
                                };
                                path.move_to(point(px(points[0].0), px(points[0].1)));
                                for &(x, y) in &points[1..] {
                                    path.line_to(point(px(x), px(y)));
                                }
                                path.close();
                                path.build().ok()
                            })
                        {
                            window.paint_path(path, if stroke { border } else { land });
                        }
                    }
                }
            });
            // A coordinate grid keeps the offline outline useful at city-level zoom.
            let desired = 360. * f64::from(dp_px(100., window).as_f32()) / self.world;
            let step = [1., 2., 5., 10., 15., 30., 60., 90.]
                .into_iter()
                .find(|n| *n >= desired)
                .unwrap_or(90.);
            let grid_color = cx.theme().border.opacity(0.35);
            let label_color = cx.theme().muted_foreground;
            let mut labels = Vec::new();
            for i in 0..(360. / step) as i32 {
                let longitude = -180. + f64::from(i) * step;
                let (x, _) = self.position(0., longitude);
                if x >= 0. && x < self.width {
                    window.paint_quad(fill(
                        Bounds::new(
                            bounds.origin + point(px(x), px(0.)),
                            size(px(0.5), px(self.height)),
                        ),
                        grid_color,
                    ));
                    labels.push(
                        Text::new(
                            format!(
                                "{}°{}",
                                longitude.abs(),
                                if longitude < 0. { "W" } else { "E" }
                            ),
                            point(px(x + 3.), px(self.height - 14.)),
                            label_color,
                        )
                        .font_size(dp_px(9., window)),
                    );
                }
            }
            for i in (-80. / step).ceil() as i32..=(80. / step).floor() as i32 {
                let latitude = f64::from(i) * step;
                let (_, y) = self.position(latitude, 0.);
                if y >= 0. && y < self.height {
                    window.paint_quad(fill(
                        Bounds::new(
                            bounds.origin + point(px(0.), px(y)),
                            size(px(self.width), px(0.5)),
                        ),
                        grid_color,
                    ));
                    labels.push(
                        Text::new(
                            format!(
                                "{}°{}",
                                latitude.abs(),
                                if latitude < 0. { "S" } else { "N" }
                            ),
                            point(px(3.), px(y + 2.)),
                            label_color,
                        )
                        .font_size(dp_px(9., window)),
                    );
                }
            }
            PlotLabel::new(labels).paint(&bounds, window, cx);
            for (layer, locations) in self.options.layers.iter().zip(&self.data.layers) {
                for location in locations {
                    if !location.value.is_finite()
                        || (layer.kind == LayerKind::Heat && location.value <= 0.)
                    {
                        continue;
                    }
                    let (x, y) = self.position(location.latitude, location.longitude);
                    let radius = dp_px(location.radius, window).as_f32();
                    if x + radius < 0.
                        || x - radius > self.width
                        || y + radius < 0.
                        || y - radius > self.height
                    {
                        continue;
                    }
                    let color = ui::hsla(location.color);
                    if layer.kind == LayerKind::Heat {
                        // Concentric, transparent circles give a simple blurred heat kernel.
                        let blur = dp_px(layer.blur, window).as_f32();
                        for band in (0..8).rev() {
                            let r = radius + blur * band as f32 / 7.;
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        bounds.origin + point(px(x - r), px(y - r)),
                                        size(px(2. * r), px(2. * r)),
                                    ),
                                    color.opacity(layer.opacity * 0.14 * location.weight.sqrt()),
                                )
                                .corner_radii(px(r)),
                            );
                        }
                    } else {
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    bounds.origin + point(px(x - radius), px(y - radius)),
                                    size(px(2. * radius), px(2. * radius)),
                                ),
                                color.opacity(layer.opacity),
                            )
                            .corner_radii(px(radius)),
                        );
                        if self.zoom >= 3. {
                            PlotLabel::new(vec![
                                Text::new(
                                    location.label.clone(),
                                    point(px(x + radius + 3.), px(y - 5.)),
                                    cx.theme().foreground,
                                )
                                .font_size(dp_px(10., window)),
                            ])
                            .paint(&bounds, window, cx);
                        }
                    }
                }
            }
        });
    }
    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        _: Bounds<Pixels>,
        _: &App,
    ) -> Option<TooltipState> {
        self.nearest(position)
            .map(|i| TooltipState::new(i, position, Vec::new()))
    }
    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<AnyElement> {
        let p = self.data.layers.iter().flatten().nth(state.index)?;
        let mut tip = Tooltip::new(cursor, bounds.size).title(p.label.clone());
        for (name, value) in &p.details {
            tip = tip.plain_row(name.clone(), value.clone());
        }
        Some(
            tip.plain_row("Latitude", format!("{:.4}°", p.latitude))
                .plain_row("Longitude", format!("{:.4}°", p.longitude))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_pan_wraps_longitude_clamps_latitude_and_preserves_zoom() {
        let mut state = ViewState::new(&Options::default());
        state.pan(point(px(256.), px(0.)), 256.);
        assert_eq!(state.center, (0., 0.5));
        state.pan(point(px(0.), px(10000.)), 256.);
        assert_eq!(state.center.1, 0.);
        state.zoom(100.);
        assert_eq!(state.zoom, 12.);
        state.zoom(-100.);
        assert_eq!(state.zoom, 0.);
    }
}
