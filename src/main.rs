//! Grafaui: a Grafana dashboard drawn natively with GPUI Kit.

use std::path::PathBuf;

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::eyre;
use grafaui_desktop::DashboardCatalog;

/// Show a Grafana dashboard JSON with fake data or a Prometheus endpoint.
#[derive(Parser)]
#[command(name = "grafaui", version, about)]
struct Cli {
    /// Dashboard JSON exported from Grafana (or returned by its API)
    file: Option<PathBuf>,

    /// Open a built-in example instead: kitchen-sink, legacy or node-exporter
    #[arg(long, conflicts_with = "file")]
    fixture: Option<String>,

    /// Discover dashboard JSON files recursively here instead of repository fixtures
    #[arg(long)]
    dashboard_dir: Option<PathBuf>,

    /// Prometheus base URL, including any path prefix
    #[arg(long, conflicts_with = "fake")]
    prometheus_url: Option<String>,

    /// Use deterministic fake data (also the default without a URL)
    #[arg(long)]
    fake: bool,

    /// Prometheus scrape interval in seconds, used for $__rate_interval
    #[arg(long, default_value_t = 15.0, requires = "prometheus_url")]
    scrape_interval: f64,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let mut catalog = DashboardCatalog::discover(cli.dashboard_dir.as_deref())
        .map_err(color_eyre::eyre::Report::msg)?;
    for warning in catalog.warnings() {
        eprintln!("Dashboard discovery: {warning}");
    }
    let selected = match (cli.file, cli.fixture) {
        (Some(path), _) => catalog
            .include_file(&path)
            .map_err(color_eyre::eyre::Report::msg)?,
        (None, name) => {
            let name = name.unwrap_or_else(|| "kitchen-sink".into());
            let id = DashboardCatalog::fixture_id(&name);
            if catalog.find(&id).is_none() {
                return Err(eyre!(
                    "no fixture named {name}; try kitchen-sink, legacy, node-exporter"
                ));
            }
            id
        }
    };
    let mode = match cli.prometheus_url {
        Some(url) => grafaui_desktop::DataMode::Prometheus {
            url,
            scrape_interval: cli.scrape_interval,
        },
        None => grafaui_desktop::DataMode::Fake,
    };
    grafaui_desktop::run_catalog(catalog, selected, mode)
}
