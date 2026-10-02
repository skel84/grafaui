//! Window-level dashboard navigation; each dashboard owns its own request lifecycle.
use std::sync::Arc;

use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{ActiveTheme, IndexPath, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window, div,
};
use grafaui_model::{Dashboard, data::FakeSource};

use crate::catalog::{DashboardCatalog, DashboardEntry};
use crate::dashboard::{DashboardView, Source};

#[derive(Clone)]
pub(crate) struct DashboardChoice {
    id: String,
    title: SharedString,
    origin: SharedString,
    search: String,
}

impl From<&DashboardEntry> for DashboardChoice {
    fn from(entry: &DashboardEntry) -> Self {
        Self {
            id: entry.id().into(),
            title: entry.title().to_owned().into(),
            origin: entry.origin().to_owned().into(),
            search: format!(
                "{} {} {}",
                entry.title(),
                entry.origin(),
                entry.tags().join(" ")
            )
            .to_lowercase(),
        }
    }
}

impl SelectItem for DashboardChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &String {
        &self.id
    }
    fn matches(&self, query: &str) -> bool {
        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| self.search.contains(word))
    }
    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .min_w_0()
            .flex_1()
            .child(div().truncate().child(self.title.clone()))
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.origin.clone()),
            )
    }
}

pub(crate) type DashboardChoices = SearchableVec<DashboardChoice>;

pub(crate) struct AppView {
    catalog: Arc<DashboardCatalog>,
    selected: String,
    picker: Entity<SelectState<DashboardChoices>>,
    dashboard: Entity<DashboardView>,
    source: Source,
    revision: u64,
    open_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl AppView {
    pub(crate) fn new(
        catalog: Arc<DashboardCatalog>,
        selected: String,
        dashboard: Dashboard,
        origin: String,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let choices: Vec<_> = catalog
            .entries()
            .iter()
            .map(DashboardChoice::from)
            .collect();
        let selected_ix = choices
            .iter()
            .position(|choice| choice.id == selected)
            .map(IndexPath::new);
        let picker = cx.new(|cx| {
            SelectState::new(DashboardChoices::from(choices), selected_ix, window, cx)
                .searchable(true)
        });
        let view = cx.new(|cx| {
            DashboardView::new(
                dashboard,
                origin,
                source.clone(),
                picker.clone(),
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &SelectEvent<DashboardChoices>, window, cx| {
                if let SelectEvent::Confirm(Some(id)) = event {
                    this.open(id.clone(), window, cx);
                }
            },
        );
        Self {
            catalog,
            selected,
            picker,
            dashboard: view,
            source,
            revision: 0,
            open_task: None,
            _subscription: subscription,
        }
    }

    fn open(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.revision += 1;
        let revision = self.revision;
        self.open_task = None;
        if id == self.selected {
            self.dashboard
                .update(cx, |dashboard, cx| dashboard.set_navigation(None, cx));
            return;
        }
        let title = self
            .catalog
            .find(&id)
            .map(|entry| entry.title())
            .unwrap_or("dashboard");
        self.dashboard.update(cx, |dashboard, cx| {
            dashboard.set_navigation(Some(Ok(format!("Opening {title}…"))), cx)
        });
        let catalog = self.catalog.clone();
        let live = matches!(self.source, Source::Prometheus(_));
        let load = cx
            .background_executor()
            .spawn(async move { catalog.open(&id, live).map(|loaded| (id, loaded)) });
        self.open_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // A newer selection or a closed window supersedes this load.
                if this.revision != revision {
                    return;
                }
                match result {
                    Ok((id, (dashboard, origin))) => {
                        let source = match &this.source {
                            Source::Fake(_) => Source::Fake(FakeSource::new(
                                dashboard.uid.as_deref().unwrap_or(&dashboard.title),
                            )),
                            Source::Prometheus(client) => Source::Prometheus(client.clone()),
                        };
                        this.dashboard.update(cx, |old, _| old.cancel_requests());
                        window.set_window_title(&format!("{} — Grafaui", dashboard.title));
                        this.dashboard = cx.new(|cx| {
                            DashboardView::new(
                                dashboard,
                                origin,
                                source,
                                this.picker.clone(),
                                window,
                                cx,
                            )
                        });
                        this.selected = id;
                    }
                    Err(error) => {
                        this.picker.update(cx, |picker, cx| {
                            picker.set_selected_value(&this.selected, window, cx)
                        });
                        this.dashboard.update(cx, |dashboard, cx| {
                            dashboard.set_navigation(
                                Some(Err(format!("Could not open dashboard: {error}"))),
                                cx,
                            )
                        });
                    }
                }
                cx.notify();
            });
        }));
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.dashboard.clone()
    }
}
