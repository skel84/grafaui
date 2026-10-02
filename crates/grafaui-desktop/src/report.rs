//! The compatibility sheet: every panel, whether gpui-kit draws it, and
//! what it leaves out.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, FontWeight, IntoElement, SharedString, div};
use grafaui_model::Support;

use crate::panel::PanelView;
use crate::ui::{self, MONO_FONT, Tone, dp};

#[derive(Clone)]
pub(crate) struct Entry {
    title: SharedString,
    kind: SharedString,
    support: Support,
    notes: Vec<SharedString>,
}

pub(crate) fn entries(panels: &[Entity<PanelView>], cx: &App) -> Vec<Entry> {
    let mut entries: Vec<Entry> = panels
        .iter()
        .map(|panel| {
            let panel = panel.read(cx);
            let spec = panel.spec();
            let support = match spec.support() {
                Support::Full if !panel.notes().is_empty() => Support::Partial,
                support => support,
            };
            Entry {
                title: if spec.title.is_empty() {
                    "(untitled)".into()
                } else {
                    spec.title.clone().into()
                },
                kind: spec.kind.clone().into(),
                support,
                notes: panel.notes().iter().cloned().map(Into::into).collect(),
            }
        })
        .collect();
    // Placeholders first: they are what a port has to build next.
    entries.sort_by_key(|e| match e.support {
        Support::Placeholder => 0,
        Support::Partial => 1,
        Support::Full => 2,
    });
    entries
}

pub(crate) fn render(entries: &[Entry], cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .id("report")
        .size_full()
        .overflow_y_scroll()
        .gap(dp(2.))
        .children(entries.iter().map(|entry| {
            let (tone, icon, label) = match entry.support {
                Support::Full => (Tone::Good, IconName::CircleCheck, "Drawn"),
                Support::Partial => (Tone::Warn, IconName::TriangleAlert, "Partial"),
                Support::Placeholder => (Tone::Crit, IconName::CircleX, "Placeholder"),
            };
            v_flex()
                .py(dp(8.))
                .gap(dp(4.))
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap(dp(8.))
                        .child(ui::tag(tone, Some(icon), label, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(entry.title.clone()),
                        )
                        .child(
                            div()
                                .font_family(MONO_FONT)
                                .text_size(dp(11.5))
                                .text_color(theme.muted_foreground)
                                .child(entry.kind.clone()),
                        ),
                )
                .children(entry.notes.iter().map(|note| {
                    div()
                        .pl(dp(4.))
                        .text_size(dp(12.))
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!("• {note}")))
                }))
        }))
}
