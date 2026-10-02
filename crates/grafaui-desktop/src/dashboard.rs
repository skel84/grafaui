//! The window: title bar, variables and time controls, and the grid of
//! sections and panels.

use std::ops::Range;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app_view::DashboardChoices;
use crate::data_worker::{self, Event};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme, Icon, IndexPath, Sizable, TitleBar, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyView, Context, Entity, FontWeight, IntoElement, Pixels, Render, ScrollHandle,
    SharedString, StyleRefinement, Subscription, Task, Window, div, px,
};
use grafaui_model::data::{DataSource, FakeSource, QueryContext};
use grafaui_model::time::{self, TimeWindow};
use grafaui_model::{Dashboard, Section};
use grafaui_prometheus::lifecycle::Run;

use crate::grid;
use crate::panel::PanelView;
use crate::report;
use crate::ui::{self, Tone, dp};

/// The time ranges the picker offers.
const RANGES: &[&str] = &[
    "now-5m", "now-15m", "now-1h", "now-3h", "now-6h", "now-12h", "now-24h", "now-2d", "now-7d",
    "now-30d",
];

type Choices = Vec<SharedString>;

/// Height of a row header, in default-size pixels.
const ROW_HEADER: f32 = 30.;

#[derive(Clone)]
pub(crate) enum Source {
    Fake(FakeSource),
    Prometheus(grafaui_prometheus::Client),
}

pub(crate) struct DashboardView {
    dashboard: Rc<Dashboard>,
    origin: SharedString,
    source: Source,
    picker: Entity<SelectState<DashboardChoices>>,
    navigation: Option<Result<String, String>>,
    revision: u64,
    run: Option<Run>,
    task: Option<Task<()>>,
    status: String,
    completed: usize,
    errors: usize,
    variable_error: Option<String>,
    /// By panel key.
    panels: Vec<Entity<PanelView>>,
    /// By section.
    collapsed: Vec<bool>,
    /// A select per shown variable, with its index in the dashboard's
    /// variables (hidden ones have none).
    variables: Vec<(usize, Entity<SelectState<Choices>>)>,
    /// Each variable's current value, by variable.
    values: Vec<String>,
    ranges: Vec<String>,
    range: usize,
    range_select: Entity<SelectState<Choices>>,
    now: i64,
    scroll: ScrollHandle,
    /// `GRAFAUI_PERF=1`: timing a scroll through the dashboard.
    perf: Option<crate::perf::Perf>,
    _subscriptions: Vec<Subscription>,
}

impl DashboardView {
    pub(crate) fn new(
        dashboard: Dashboard,
        origin: String,
        source: Source,
        picker: Entity<SelectState<DashboardChoices>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dashboard = Rc::new(dashboard);
        let mut ranges: Vec<String> = RANGES.iter().map(|r| r.to_string()).collect();
        let from = dashboard.time_from.clone();
        let range = match ranges.iter().position(|r| *r == from) {
            Some(index) => index,
            None if time::parse_relative(&from).is_some() => {
                ranges.push(from);
                ranges.sort_by_key(|r| time::parse_relative(r));
                ranges
                    .iter()
                    .position(|r| *r == dashboard.time_from)
                    .unwrap_or(0)
            }
            // Absolute ranges show as the last six hours.
            None => ranges.iter().position(|r| r == "now-6h").unwrap_or(0),
        };
        let labels: Choices = ranges
            .iter()
            .map(|r| time::describe(time::parse_relative(r).unwrap_or(21_600)).into())
            .collect();
        let range_select =
            cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(range)), window, cx));
        let mut subscriptions = vec![cx.subscribe_in(
            &range_select,
            window,
            |this, state, _: &SelectEvent<Choices>, window, cx| {
                if let Some(index) = state.read(cx).selected_index(cx) {
                    this.range = index.row;
                    this.query(window, cx);
                }
            },
        )];

        let live = matches!(source, Source::Prometheus(_));
        let values: Vec<String> = dashboard
            .variables
            .iter()
            .map(|v| {
                if live {
                    v.selected.join(" + ")
                } else {
                    v.current.clone()
                }
            })
            .collect();
        let variables: Vec<_> = dashboard
            .variables
            .iter()
            .enumerate()
            .filter(|(_, variable)| !variable.hidden)
            .map(|(index, variable)| {
                let options: Choices = if live {
                    variable
                        .saved_options
                        .iter()
                        .cloned()
                        .map(Into::into)
                        .collect()
                } else {
                    variable.options.iter().cloned().map(Into::into).collect()
                };
                let selected = variable
                    .options
                    .iter()
                    .position(|o| *o == variable.current)
                    .unwrap_or(0);
                let state = cx.new(|cx| {
                    SelectState::new(options, Some(IndexPath::new(selected)), window, cx)
                });
                (index, state)
            })
            .collect();
        for &(index, ref state) in &variables {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, _, event: &SelectEvent<Choices>, window, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else {
                        return;
                    };
                    this.values[index] = value.to_string();
                    this.query(window, cx);
                },
            ));
        }

        let collapsed = dashboard
            .sections
            .iter()
            // For screenshots: `GRAFAUI_EXPAND=1` opens every row.
            .map(|s| {
                s.row.as_ref().is_some_and(|r| r.collapsed)
                    && std::env::var_os("GRAFAUI_EXPAND").is_none()
            })
            .collect();
        let mut this = Self {
            dashboard: dashboard.clone(),
            origin: origin.into(),
            source,
            picker,
            navigation: None,
            revision: 0,
            run: None,
            task: None,
            status: if live {
                "Connecting to Prometheus…".into()
            } else {
                "Fake data".into()
            },
            completed: 0,
            errors: 0,
            variable_error: None,
            panels: Vec::new(),
            collapsed,
            variables,
            values,
            ranges,
            range,
            range_select,
            now: unix_now(),
            scroll: ScrollHandle::new(),
            perf: crate::perf::enabled().then(crate::perf::Perf::new),
            _subscriptions: subscriptions,
        };
        // For screenshots: `GRAFAUI_SCROLL=<px>` opens scrolled down.
        if let Some(y) = std::env::var("GRAFAUI_SCROLL")
            .ok()
            .and_then(|y| y.parse::<f32>().ok())
        {
            this.scroll.set_offset(gpui_kit::point(px(0.), px(-y)));
        }
        let context = this.context();
        let positions = dashboard
            .sections
            .iter()
            .flat_map(|s| s.panels.iter())
            .map(|p| (p.key, p.pos))
            .collect::<std::collections::HashMap<_, _>>();
        this.panels = dashboard
            .panels
            .iter()
            .map(|spec| {
                let frame = match &this.source {
                    Source::Fake(source) => source.query(spec, &context),
                    Source::Prometheus(_) => Default::default(),
                };
                let spec = Rc::new(spec.clone());
                let pos = positions[&spec.key];
                let span = context.window.span;
                let title = context.for_panel(&spec).interpolate(&spec.title);
                cx.new(|_| PanelView::new(spec, pos, title, frame, span))
            })
            .collect();
        if live {
            this.query(window, cx);
        }
        this
    }

    pub(crate) fn cancel_requests(&mut self) {
        self.run = None;
        self.task = None;
    }

    pub(crate) fn set_navigation(
        &mut self,
        navigation: Option<Result<String, String>>,
        cx: &mut Context<Self>,
    ) {
        self.navigation = navigation;
        cx.notify();
    }

    fn context(&self) -> QueryContext {
        QueryContext {
            window: TimeWindow::relative(&self.ranges[self.range], self.now),
            variables: self
                .dashboard
                .variables
                .iter()
                .zip(&self.values)
                .map(|(v, value)| (v.name.clone(), value.clone()))
                .collect(),
        }
    }

    /// Runs every panel's queries again.
    fn query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.now = unix_now();
        let context = self.context();
        if let Source::Prometheus(client) = &self.source {
            let client = client.clone();
            self.revision += 1;
            let revision = self.revision;
            // Drop cancels queued work and the old foreground receiver.
            self.run = Some(Run::new(revision));
            self.task = None;
            self.completed = 0;
            self.errors = 0;
            self.variable_error = None;
            self.status = "Loading variables…".into();
            for panel in &self.panels {
                let title = context
                    .for_panel(panel.read(cx).spec())
                    .interpolate(&panel.read(cx).spec().title);
                panel.update(cx, |p, cx| p.set_loading(title, cx));
            }
            let (sender, receiver) = async_channel::unbounded();
            let cancellation = self.run.as_ref().unwrap().cancellation();
            let dashboard = self.dashboard.as_ref().clone();
            let selections = self.values.clone();
            cx.background_executor()
                .spawn(async move {
                    data_worker::run(client, dashboard, selections, context, cancellation, sender);
                })
                .detach();
            self.task = Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = receiver.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| {
                            if !this.run.as_ref().is_some_and(|run| run.accepts(revision)) {
                                return;
                            }
                            this.receive(event, window, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            cx.notify();
            return;
        }
        let Source::Fake(source) = &self.source else {
            unreachable!()
        };
        for panel in &self.panels {
            let frame = source.query(panel.read(cx).spec(), &context);
            let spec = panel.read(cx).spec();
            let title = context.for_panel(spec).interpolate(&spec.title);
            panel.update(cx, |panel, cx| {
                panel.set_frame(title, frame, context.window.span, cx)
            });
        }
        cx.notify();
    }

    fn receive(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Variables(variables, warnings) => {
                for index in 0..self.values.len() {
                    self.values[index] = variables.selection(index);
                }
                for (index, state) in &self.variables {
                    let mut options: Choices = variables
                        .options(*index)
                        .iter()
                        .cloned()
                        .map(Into::into)
                        .collect();
                    let selected: SharedString = self.values[*index].clone().into();
                    if !options.contains(&selected) {
                        options.insert(0, selected.clone());
                    }
                    state.update(cx, |state, cx| {
                        state.set_items(options, window, cx);
                        state.set_selected_value(&selected, window, cx);
                    });
                }
                self.status = "Loading panels…".into();
                if !warnings.is_empty() {
                    self.variable_error = Some(warnings.join("; "));
                }
            }
            Event::Panel { key, result, span } => {
                self.completed += 1;
                if result.is_err() {
                    self.errors += 1;
                }
                let context = self.context();
                let panel = &self.panels[key];
                let title = context
                    .for_panel(panel.read(cx).spec())
                    .interpolate(&panel.read(cx).spec().title);
                panel.update(cx, |panel, cx| panel.set_result(title, result, span, cx));
                self.status = format!(
                    "{}/{} panels · {} errors",
                    self.completed,
                    self.panels.len(),
                    self.errors
                );
            }
            Event::Failed(error) => {
                self.status = "Variable query failed".into();
                self.variable_error = Some(error.clone());
                let context = self.context();
                for panel in &self.panels {
                    let title = context
                        .for_panel(panel.read(cx).spec())
                        .interpolate(&panel.read(cx).spec().title);
                    panel.update(cx, |p, cx| {
                        p.set_result(title, Err(error.clone()), context.window.span, cx)
                    });
                }
            }
            Event::Finished => {
                self.status = if self.errors == 0 {
                    "Connected".into()
                } else {
                    format!(
                        "{} panel error{}",
                        self.errors,
                        if self.errors == 1 { "" } else { "s" }
                    )
                };
            }
        }
        cx.notify();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.now = unix_now();
        self.query(window, cx);
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (full, partial, none) = self.dashboard.support_counts();
        TitleBar::new()
            .h(dp(40.))
            .when(cfg!(target_os = "macos"), |bar| bar.pl(dp(84.)))
            .child(
                h_flex()
                    .gap(dp(8.))
                    .min_w_0()
                    .flex_1()
                    .child(
                        Icon::new(IconName::LayoutDashboard)
                            .size(dp(16.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Select::new(&self.picker)
                            .id("dashboard-picker")
                            .small()
                            .w(dp(360.))
                            .menu_width(dp(460.))
                            .menu_max_h(dp(480.))
                            .accessibility_label("Dashboard")
                            .search_placeholder("Search dashboards…")
                            .placeholder("Choose a dashboard"),
                    )
                    .children(
                        self.dashboard
                            .tags
                            .iter()
                            .take(4)
                            .map(|tag| ui::tag(Tone::Muted, None, tag.clone(), cx)),
                    ),
            )
            .child(
                h_flex()
                    .gap(dp(6.))
                    .pr(dp(10.))
                    .flex_none()
                    .child(
                        Select::new(&self.range_select)
                            .id("time-range")
                            .small()
                            .w(dp(150.))
                            .icon(IconName::Calendar),
                    )
                    .child(
                        Button::new("refresh")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .tooltip("Refresh dashboard data")
                            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
                    )
                    .child(
                        div()
                            .text_size(dp(11.5))
                            .text_color(theme.muted_foreground)
                            .child(self.origin.clone()),
                    )
                    .child(
                        Button::new("compatibility")
                            .ghost()
                            .small()
                            .tooltip("Which panels gpui-kit draws")
                            .child(
                                h_flex()
                                    .gap(dp(4.))
                                    .child(ui::tag(
                                        Tone::Good,
                                        Some(IconName::CircleCheck),
                                        full.to_string(),
                                        cx,
                                    ))
                                    .when(partial > 0, |this| {
                                        this.child(ui::tag(
                                            Tone::Warn,
                                            Some(IconName::TriangleAlert),
                                            partial.to_string(),
                                            cx,
                                        ))
                                    })
                                    .when(none > 0, |this| {
                                        this.child(ui::tag(
                                            Tone::Crit,
                                            Some(IconName::CircleX),
                                            none.to_string(),
                                            cx,
                                        ))
                                    }),
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_report(window, cx)),
                            ),
                    ),
            )
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .flex_wrap()
            .gap(dp(8.))
            .px(dp(grid::PAD))
            .py(dp(8.))
            .border_b_1()
            .border_color(theme.border)
            .children(
                self.variables
                    .iter()
                    .map(|(index, state)| (index, &self.dashboard.variables[*index], state))
                    .map(|(&index, variable, state)| {
                        h_flex()
                            .child(
                                div()
                                    .h(dp(26.))
                                    .px(dp(8.))
                                    .flex()
                                    .items_center()
                                    .rounded_l(px(4.))
                                    .border_1()
                                    .border_r_0()
                                    .border_color(theme.border)
                                    .bg(theme.secondary)
                                    .text_size(dp(12.))
                                    .child(SharedString::from(variable.label.clone())),
                            )
                            .child(
                                Select::new(state)
                                    .id(("variable", index))
                                    .small()
                                    .w(dp(150.))
                                    .menu_width(dp(220.)),
                            )
                    }),
            )
    }

    /// The part of the scrolled content worth drawing: the viewport and half
    /// a viewport either side, in content coordinates.
    ///
    /// GPUI's view cache is keyed by bounds, which scrolling changes, so every
    /// panel in the tree is laid out and painted again on each scroll step.
    /// Panels outside this range are left out and only their slot is kept.
    fn drawn_range(&self, window: &Window) -> Range<Pixels> {
        let height = match self.scroll.bounds().size.height {
            h if h > px(0.) => h,
            _ => window.viewport_size().height,
        };
        let top = -self.scroll.offset().y;
        (top - height / 2.)..(top + height * 1.5)
    }

    /// `top` is where the section starts in the scrolled content.
    fn render_section(
        &self,
        index: usize,
        section: &Section,
        top: Pixels,
        drawn: &Range<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.collapsed[index];
        let grid_top = match section.row {
            Some(_) => top + ui::dp_px(ROW_HEADER + 8., window),
            None => top,
        };
        v_flex()
            .w_full()
            .gap(dp(8.))
            .when_some(section.row.as_ref(), |this, row| {
                this.child(self.render_row(index, row, section.panels.len(), collapsed, cx))
            })
            .when(!collapsed && !section.panels.is_empty(), |this| {
                this.child(self.render_grid(section, grid_top, drawn, window))
            })
            .into_any_element()
    }

    /// The section's height as `render_section` lays it out.
    fn section_height(&self, index: usize, section: &Section, window: &Window) -> Pixels {
        let header = section.row.is_some().then(|| ui::dp_px(ROW_HEADER, window));
        let grid = (!self.collapsed[index] && !section.panels.is_empty()).then(|| {
            if grid::is_narrow(window) {
                let heights: Pixels = section
                    .panels
                    .iter()
                    .map(|p| grid::rows_height(p.pos.h, window))
                    .sum();
                heights + ui::dp_px(8., window) * (section.panels.len() - 1) as f32
            } else {
                grid::rows_height(section.height, window)
            }
        });
        match (header, grid) {
            (Some(h), Some(g)) => h + ui::dp_px(8., window) + g,
            (h, g) => h.or(g).unwrap_or_default(),
        }
    }

    fn render_row(
        &self,
        index: usize,
        row: &grafaui_model::RowHeader,
        count: usize,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id(("row", index))
            .h(dp(ROW_HEADER))
            .gap(dp(6.))
            .cursor_pointer()
            .text_size(dp(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(dp(14.))
                .text_color(theme.muted_foreground),
            )
            .child(SharedString::from(if row.title.is_empty() {
                "Row".to_owned()
            } else {
                self.context().interpolate(&row.title)
            }))
            .when(collapsed, |this| {
                this.child(
                    div()
                        .text_size(dp(12.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "({count} panel{})",
                            if count == 1 { "" } else { "s" }
                        )),
                )
            })
            .when_some(row.repeat.clone(), |this, by| {
                this.child(ui::tag(
                    Tone::Warn,
                    None,
                    format!("repeats by ${by} (shown once)"),
                    cx,
                ))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.collapsed[index] = !this.collapsed[index];
                cx.notify();
            }))
    }

    /// `top` is where the grid starts in the scrolled content; panels that
    /// don't reach into `drawn` keep an empty slot.
    fn render_grid(
        &self,
        section: &Section,
        top: Pixels,
        drawn: &Range<Pixels>,
        window: &Window,
    ) -> AnyElement {
        let view = |key: usize, y: Pixels, height: Pixels| {
            (y < drawn.end && y + height > drawn.start).then(|| {
                AnyView::from(self.panels[key].clone())
                    .cached(StyleRefinement::default().size_full())
            })
        };
        if grid::is_narrow(window) {
            let gap = ui::dp_px(8., window);
            let mut y = top;
            return v_flex()
                .w_full()
                .gap(gap)
                .children(section.panels.iter().map(|placed| {
                    let height = grid::rows_height(placed.pos.h, window);
                    let slot = div()
                        .w_full()
                        .h(height)
                        .children(view(placed.key, y, height));
                    y += height + gap;
                    slot
                }))
                .into_any_element();
        }
        div()
            .relative()
            .w_full()
            .h(grid::rows_height(section.height, window))
            .children(section.panels.iter().map(|placed| {
                let (left, y) = grid::panel_origin(placed.pos, window);
                let size = grid::panel_size(placed.pos, window);
                div()
                    .absolute()
                    .left(left)
                    .top(y)
                    .w(size.width)
                    .h(size.height)
                    .children(view(placed.key, top + y, size.height))
            }))
            .into_any_element()
    }

    fn open_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = report::entries(&self.panels, cx);
        window.open_sheet(cx, move |sheet, _, cx| {
            sheet
                .title("Panel compatibility")
                .size(dp(520.))
                .child(report::render(&entries, cx))
        });
    }
}

impl Render for DashboardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = self
            .perf
            .as_mut()
            .map(|perf| perf.frame_start(&self.scroll, window));
        let drawn = self.drawn_range(window);
        let gap = ui::dp_px(8., window);
        let mut top = ui::dp_px(grid::PAD, window);
        let dashboard = self.dashboard.clone();
        let mut sections = Vec::with_capacity(dashboard.sections.len());
        for (index, section) in dashboard.sections.iter().enumerate() {
            sections.push(self.render_section(index, section, top, &drawn, window, cx));
            top += self.section_height(index, section, window) + gap;
        }
        let element = v_flex()
            .size_full()
            .bg(grid::page_background(cx))
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .when_some(self.navigation.clone(), |this, navigation| {
                let (message, color) = match navigation {
                    Ok(message) => (message, cx.theme().muted_foreground),
                    Err(message) => (message, cx.theme().danger),
                };
                this.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(color)
                        .child(message),
                )
            })
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_3()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match &self.source {
                        Source::Fake(_) => "Fake data".to_owned(),
                        Source::Prometheus(client) => client.label(),
                    })
                    .children(
                        matches!(self.source, Source::Prometheus(_)).then(|| self.status.clone()),
                    ),
            )
            .when_some(self.variable_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .when(!self.variables.is_empty(), |this| {
                this.child(self.render_controls(cx))
            })
            .child(
                div()
                    .id("dashboard-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        v_flex()
                            .w_full()
                            .p(dp(grid::PAD))
                            .gap(dp(8.))
                            .children(sections),
                    ),
            );
        if let (Some(perf), Some(started)) = (self.perf.as_mut(), started) {
            perf.frame_end(started, window, cx);
        }
        element
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
