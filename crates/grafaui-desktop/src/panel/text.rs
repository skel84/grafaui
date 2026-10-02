//! `text`: Kit's TextView, as Markdown, HTML or a code block.

use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, div};
use grafaui_model::spec::{TextMode, TextOptions};

use super::PanelView;

pub(super) fn render(view: &PanelView, options: &TextOptions, _: &App) -> AnyElement {
    let id = view.element_id("text");
    let text = match options.mode {
        TextMode::Markdown => TextView::markdown(id, options.content.clone()),
        TextMode::Html => TextView::html(id, options.content.clone()),
        TextMode::Code => TextView::markdown(id, format!("```\n{}\n```", options.content)),
    };
    div()
        .id(view.element_id("text-scroll"))
        .size_full()
        .overflow_y_scroll()
        .child(text.selectable(true))
        .into_any_element()
}
