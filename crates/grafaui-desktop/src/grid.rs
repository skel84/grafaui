//! Grafana's grid in window pixels: 24 columns across the dashboard width,
//! rows of 30 px and 8 px gaps, all scaled with the text size.
//!
//! In a narrow window every panel takes the full width, stacked in reading
//! order, as Grafana does on small screens.

use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, Hsla, Pixels, Size, Window, px, size};
use grafaui_model::layout::{COLUMNS, GAP, ROW_HEIGHT};
use grafaui_model::schema::GridPos;

use crate::ui::dp_px;

/// Space around the grid.
pub(crate) const PAD: f32 = 12.;
/// Below this width, in default-size pixels, panels stack.
const NARROW: f32 = 720.;

pub(crate) fn is_narrow(window: &Window) -> bool {
    window.viewport_size().width < dp_px(NARROW, window)
}

fn inner_width(window: &Window) -> Pixels {
    (window.viewport_size().width - dp_px(2. * PAD, window)).max(px(100.))
}

fn column(window: &Window) -> Pixels {
    (inner_width(window) + dp_px(GAP, window)) / COLUMNS as f32
}

/// Rows of height `h` with the gaps between them.
pub(crate) fn rows_height(h: u32, window: &Window) -> Pixels {
    if h == 0 {
        return px(0.);
    }
    dp_px(h as f32 * (ROW_HEIGHT + GAP) - GAP, window)
}

/// Where a panel sits in its section, in wide layout.
pub(crate) fn panel_origin(pos: GridPos, window: &Window) -> (Pixels, Pixels) {
    (
        column(window) * pos.x as f32,
        dp_px(pos.y as f32 * (ROW_HEIGHT + GAP), window),
    )
}

pub(crate) fn panel_size(pos: GridPos, window: &Window) -> Size<Pixels> {
    let width = if is_narrow(window) {
        inner_width(window)
    } else {
        column(window) * pos.w as f32 - dp_px(GAP, window)
    };
    size(width, rows_height(pos.h, window))
}

pub(crate) fn page_background(cx: &App) -> Hsla {
    cx.theme().background
}

/// Panels sit a step off the page, as Grafana's do.
pub(crate) fn panel_background(cx: &App) -> Hsla {
    let theme = cx.theme();
    if theme.is_dark() {
        theme.background.blend(theme.foreground.opacity(0.035))
    } else {
        theme.background.blend(theme.foreground.opacity(0.02))
    }
}
