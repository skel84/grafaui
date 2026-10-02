//! `GRAFAUI_PERF=1`: measures startup and scrolling, prints a report and
//! quits.
//!
//! Startup is timed from `run` to the first `render` and to the first
//! next-frame callback (GPUI's frame loop, not the moment pixels reach the
//! screen). Then the dashboard scrolls to the bottom and back at a fixed
//! step per frame. Each step is taken at the top of `render`, so the frame
//! that is built is the one at the new offset.
//!
//! The time between frames covers everything: building, layout, paint,
//! waiting for a drawable and encoding (it can't go below the display's
//! refresh interval). `render()` is the root view only: cached panels render
//! later, during prepaint. Build with `--features grafaui-desktop/profiler`
//! for GPUI's own `Window::draw` time, which includes them.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::{App, Pixels, ScrollHandle, Window, point, px};

static START: OnceLock<Instant> = OnceLock::new();

/// Call first thing; startup times count from here.
pub(crate) fn start() {
    START.get_or_init(Instant::now);
}

pub(crate) fn enabled() -> bool {
    std::env::var_os("GRAFAUI_PERF").is_some()
}

fn since_start() -> Duration {
    START.get().map_or(Duration::ZERO, Instant::elapsed)
}

/// Pixels scrolled per frame.
const STEP: f32 = 60.;
/// Frames rendered before scrolling starts, so the first queries settle.
const WARMUP: usize = 10;

#[derive(Default)]
pub(crate) struct Perf {
    frames: usize,
    last_frame: Option<Instant>,
    /// Between successive renders while scrolling.
    intervals: Vec<Duration>,
    /// Inside the root `render`.
    renders: Vec<Duration>,
    down: bool,
    /// Back at the top: the frame being built is the last one.
    done: bool,
    reported: bool,
    max: Pixels,
    /// GPUI's histograms when scrolling started.
    #[cfg(feature = "profiler")]
    baseline: Option<gpui_kit::profiler::FrameDurationSnapshot>,
}

impl Perf {
    pub(crate) fn new() -> Self {
        Self {
            down: true,
            ..Self::default()
        }
    }

    /// Call at the top of `render`, before building anything: takes the
    /// next scroll step and returns when the render started.
    pub(crate) fn frame_start(&mut self, scroll: &ScrollHandle, window: &mut Window) -> Instant {
        let now = Instant::now();
        if self.frames == 0 {
            eprintln!(
                "perf: first render          {:>8.1} ms after start",
                ms(since_start())
            );
            window.on_next_frame(|_, _| {
                eprintln!(
                    "perf: first frame callback  {:>8.1} ms after start",
                    ms(since_start())
                );
            });
        }
        #[cfg(feature = "profiler")]
        if self.frames == WARMUP {
            self.baseline = Some(window.frame_duration_snapshot());
        }
        if self.frames > WARMUP && !self.done {
            if let Some(last) = self.last_frame {
                self.intervals.push(now - last);
            }
            self.step(scroll);
        }
        self.last_frame = Some(now);
        self.frames += 1;
        now
    }

    fn step(&mut self, scroll: &ScrollHandle) {
        let offset = -scroll.offset().y;
        let max = scroll.max_offset().y;
        self.max = self.max.max(max);
        let next = if self.down {
            offset + px(STEP)
        } else {
            offset - px(STEP)
        };
        if self.down && next >= max {
            self.down = false;
        } else if !self.down && next <= Pixels::ZERO {
            self.done = true;
        }
        scroll.set_offset(point(px(0.), -next.clamp(Pixels::ZERO, max)));
    }

    /// Call at the end of `render`: records its time and asks for the next
    /// frame, or reports and quits once back at the top.
    pub(crate) fn frame_end(&mut self, started: Instant, window: &mut Window, cx: &mut App) {
        if self.reported {
            return;
        }
        if self.frames > WARMUP + 1 {
            self.renders.push(started.elapsed());
        }
        if self.done {
            self.reported = true;
            self.report(window);
            cx.quit();
            return;
        }
        window.request_animation_frame();
    }

    fn report(&self, window: &Window) {
        eprintln!(
            "perf: scrolled 0 → {:.0} px → 0 in {STEP} px steps",
            f32::from(self.max)
        );
        if !self.intervals.is_empty() {
            stats("frame interval", &self.intervals);
            stats("root render()", &self.renders);
        }
        #[cfg(feature = "profiler")]
        self.report_profiler(window);
        #[cfg(not(feature = "profiler"))]
        let _ = window;
        eprintln!("perf: rss {}", rss());
    }

    /// GPUI's `Window::draw` (building, layout, prepaint and paint on the
    /// CPU) and the interval between presented frames, scrolling only.
    #[cfg(feature = "profiler")]
    fn report_profiler(&self, window: &Window) {
        let end = window.frame_duration_snapshot();
        let since = |mut histogram: hdrhistogram::Histogram<u64>,
                     baseline: Option<&hdrhistogram::Histogram<u64>>| {
            if let Some(baseline) = baseline {
                histogram.subtract(baseline).ok();
            }
            histogram
        };
        let baseline = self.baseline.as_ref();
        let draw = since(
            end.draw_duration_histogram,
            baseline.map(|b| &b.draw_duration_histogram),
        );
        let present = since(
            end.present_interval_histogram,
            baseline.map(|b| &b.present_interval_histogram),
        );
        histogram("Window::draw", &draw);
        histogram("present interval", &present);
    }
}

fn stats(name: &str, values: &[Duration]) {
    let mut sorted: Vec<f64> = values.iter().map(|d| ms(*d)).collect();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let mean = sorted.iter().sum::<f64>() / sorted.len().max(1) as f64;
    eprintln!(
        "perf: {name:<17} n={:<4} mean {mean:>6.2}  p50 {:>6.2}  p95 {:>6.2}  max {:>6.2} ms",
        sorted.len(),
        at(0.5),
        at(0.95),
        at(1.0),
    );
}

/// An hdrhistogram of nanoseconds, in the same shape as [`stats`].
#[cfg(feature = "profiler")]
fn histogram(name: &str, histogram: &hdrhistogram::Histogram<u64>) {
    let ms = |ns: f64| ns / 1e6;
    eprintln!(
        "perf: {name:<17} n={:<4} mean {:>6.2}  p50 {:>6.2}  p95 {:>6.2}  max {:>6.2} ms",
        histogram.len(),
        ms(histogram.mean()),
        ms(histogram.value_at_quantile(0.5) as f64),
        ms(histogram.value_at_quantile(0.95) as f64),
        ms(histogram.max() as f64),
    );
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

/// Resident memory, from `ps`.
fn rss() -> String {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or_else(|| "unknown".into(), |kb| format!("{:.0} MB", kb / 1024.))
}
