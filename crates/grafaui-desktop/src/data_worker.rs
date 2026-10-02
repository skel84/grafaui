//! Background orchestration only; entity updates live in DashboardView.
use std::sync::atomic::{AtomicUsize, Ordering};

use async_channel::Sender;
use grafaui_model::{
    Dashboard,
    data::{Frame, QueryContext},
};
use grafaui_prometheus::{Client, Variables, lifecycle::Cancellation, query::panel_window};

pub(crate) enum Event {
    Variables(Variables, Vec<String>),
    Panel {
        key: usize,
        result: Result<(Frame, Vec<String>), String>,
        span: u64,
    },
    Failed(String),
    Finished,
}

pub(crate) fn run(
    client: Client,
    dashboard: Dashboard,
    selections: Vec<String>,
    mut context: QueryContext,
    cancellation: Cancellation,
    sender: Sender<Event>,
) {
    let (variables, warnings) = match client.resolve_variables(
        &dashboard.variables,
        &selections,
        context.window,
        &cancellation,
    ) {
        Ok(value) => value,
        Err(error) => {
            let _ = sender.send_blocking(Event::Failed(error));
            return;
        }
    };
    context.variables = variables.context_values();
    if cancellation.is_cancelled()
        || sender
            .send_blocking(Event::Variables(variables.clone(), warnings))
            .is_err()
    {
        return;
    }
    let next = AtomicUsize::new(0);
    // Small bounded worker pool: unrelated panels appear as soon as they finish.
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                loop {
                    if cancellation.is_cancelled() {
                        return;
                    }
                    let key = next.fetch_add(1, Ordering::Relaxed);
                    let Some(panel) = dashboard.panels.get(key) else {
                        return;
                    };
                    let span = panel_window(panel, &context, &variables)
                        .map(|w| w.span)
                        .unwrap_or(context.window.span);
                    let result = client.query_panel(panel, &context, &variables, &cancellation);
                    if cancellation.is_cancelled()
                        || sender
                            .send_blocking(Event::Panel { key, result, span })
                            .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    if !cancellation.is_cancelled() {
        let _ = sender.send_blocking(Event::Finished);
    }
}
