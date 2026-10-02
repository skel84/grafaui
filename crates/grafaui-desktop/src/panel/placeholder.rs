//! The card for panel types gpui-kit has no widget for. It keeps the
//! panel's place and size and says what would be drawn there.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, Icon, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, FontWeight, SharedString, div, px};

use super::PanelView;
use crate::ui::{MONO_FONT, dp};

const MAX_QUERIES: usize = 3;

pub(super) fn render(view: &PanelView, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let spec = view.spec();
    let queries: Vec<SharedString> = spec
        .queries
        .iter()
        .filter_map(|q| q.text.clone())
        .take(MAX_QUERIES)
        .map(Into::into)
        .collect();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(dp(4.))
        .px(dp(12.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border.opacity(0.6))
        .bg(theme.muted.opacity(0.25))
        .child(
            Icon::new(IconName::Frame)
                .size(dp(20.))
                .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .font_family(MONO_FONT)
                .text_size(dp(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(if spec.kind.is_empty() {
                    "(no type)".to_owned()
                } else {
                    spec.kind.clone()
                })),
        )
        .child(
            div()
                .text_size(dp(12.))
                .text_color(theme.muted_foreground)
                .child("No gpui-kit widget for this panel type yet"),
        )
        .children(queries.into_iter().map(|q| {
            div()
                .max_w_full()
                .truncate()
                .font_family(MONO_FONT)
                .text_size(dp(11.))
                .text_color(theme.muted_foreground.opacity(0.8))
                .child(q)
        }))
        .into_any_element()
}
