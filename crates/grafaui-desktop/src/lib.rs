//! Grafaui's window: a Grafana dashboard drawn with GPUI Kit widgets, on
//! fake or Prometheus data.

mod app_view;
mod catalog;
mod dashboard;
mod data_worker;
mod grid;
mod panel;
mod perf;
mod report;
mod ui;

use app_view::AppView;
pub use catalog::{DashboardCatalog, DashboardEntry};
use dashboard::Source;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, point, px, size,
};

/// The single endpoint used for all Prometheus datasource names/UIDs in an
/// imported dashboard. Credentials and multiple endpoints are out of scope.
#[derive(Clone, Debug, Default)]
pub enum DataMode {
    #[default]
    Fake,
    Prometheus {
        url: String,
        scrape_interval: f64,
    },
}

actions!(grafaui, [Quit]);

/// Parses `json` and opens a window showing it. `origin` names where the
/// dashboard came from, for the title bar.
pub fn run(json: &str, origin: String, mode: DataMode) -> color_eyre::Result<()> {
    let catalog = DashboardCatalog::single(json, origin).map_err(color_eyre::eyre::Report::msg)?;
    run_catalog(catalog, "opened-dashboard".into(), mode)
}

/// Opens a local dashboard collection with a searchable picker. Switching
/// dashboards retains the chosen data mode and Prometheus endpoint.
pub fn run_catalog(
    catalog: DashboardCatalog,
    selected: String,
    mode: DataMode,
) -> color_eyre::Result<()> {
    perf::start();
    let (dashboard, origin) = catalog
        .open(&selected, matches!(mode, DataMode::Prometheus { .. }))
        .map_err(color_eyre::eyre::Report::msg)?;
    let catalog = std::sync::Arc::new(catalog);
    let source = match mode {
        DataMode::Fake => Source::Fake(grafaui_model::data::FakeSource::new(
            dashboard.uid.as_deref().unwrap_or(&dashboard.title),
        )),
        DataMode::Prometheus {
            url,
            scrape_interval,
        } => Source::Prometheus(
            grafaui_prometheus::Client::new(&url, scrape_interval)
                .map_err(color_eyre::eyre::Report::msg)?,
        ),
    };
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            // Grafana dashboards are usually read in the dark theme.
            Theme::change(ThemeMode::Dark, None, cx);
            cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
                window_min_size: Some(size(px(480.), px(400.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{} — Grafaui", dashboard.title).into()),
                    traffic_light_position: Some(point(px(14.), px(13.))),
                    ..TitleBar::title_bar_options()
                }),
                ..TitleBar::window_options()
            };
            let dashboard = dashboard.clone();
            let origin = origin.clone();
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    AppView::new(
                        catalog.clone(),
                        selected.clone(),
                        dashboard,
                        origin,
                        source.clone(),
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to open the window");
            cx.activate(true);
        });
    Ok(())
}
