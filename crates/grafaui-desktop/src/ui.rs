//! Small shared building blocks: scaled sizes, colors, tags.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Div, FontWeight, Hsla, Pixels, Rems, Rgba as GpuiRgba, SharedString, Window, px, rems,
};
use grafaui_model::color::Rgba;

/// Monospace face for queries.
pub(crate) const MONO_FONT: &str = "Menlo";

/// The theme's base text size at the default text size, in pixels.
pub(crate) const BASE_TEXT: f32 = 14.;

/// `n` pixels at the default text size, scaling with the window's rem size.
pub(crate) fn dp(n: f32) -> Rems {
    rems(n / BASE_TEXT)
}

/// `dp(n)` in pixels, for arithmetic and APIs that take `Pixels`.
pub(crate) fn dp_px(n: f32, window: &Window) -> Pixels {
    window.rem_size() * (n / BASE_TEXT)
}

/// A model color as a GPUI color.
pub(crate) fn hsla(color: Rgba) -> Hsla {
    GpuiRgba {
        r: color.red() as f32 / 255.,
        g: color.green() as f32 / 255.,
        b: color.blue() as f32 / 255.,
        a: color.alpha() as f32 / 255.,
    }
    .into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Good,
    Warn,
    Crit,
    Muted,
}

impl Tone {
    pub(crate) fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Tone::Good => theme.success,
            Tone::Warn => theme.warning,
            Tone::Crit => theme.danger,
            Tone::Muted => theme.muted_foreground,
        }
    }
}

/// A small rounded label in a tone.
pub(crate) fn tag(
    tone: Tone,
    icon: Option<IconName>,
    text: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let fg = tone.color(cx);
    h_flex()
        .flex_none()
        .gap(dp(4.))
        .h(dp(18.))
        .px(dp(6.))
        .rounded(px(4.))
        .bg(fg.opacity(0.14))
        .text_color(fg)
        .text_size(dp(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .when_some(icon, |this, icon| {
            this.child(Icon::new(icon).size(dp(12.)).text_color(fg))
        })
        .child(text.into())
}
