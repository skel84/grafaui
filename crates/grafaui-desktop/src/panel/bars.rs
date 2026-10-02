//! `barchart`, `histogram` and `candlestick`, on Kit's BarChart and
//! CandlestickChart.

use gpui_kit::base::plot::shape::BarAlignment;
use gpui_kit::component::chart::{BarChart, CandlestickChart};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, SharedString};

use super::{PanelView, SeriesStat, Tick, no_data};

/// One bar per series, at its reduced value.
pub(super) fn render_categories(
    view: &PanelView,
    options: &grafaui_model::spec::BarChartOptions,
    cx: &App,
) -> AnyElement {
    if view.data.series.is_empty() {
        return no_data(cx);
    }
    let field = view.spec.field.clone();
    let bars: Vec<(Tick, SeriesStat)> = view
        .data
        .series
        .iter()
        .enumerate()
        .map(|(index, s)| {
            (
                Tick {
                    index,
                    time: index as f64,
                    label: s.name.clone(),
                },
                s.clone(),
            )
        })
        .collect();
    // Series names can repeat, so bands are keyed by position.
    BarChart::new(bars)
        .id(view.element_id("bars"))
        .alignment(if options.horizontal {
            BarAlignment::Left
        } else {
            BarAlignment::Bottom
        })
        .band(|(tick, _): &(Tick, SeriesStat)| tick.clone())
        .value(|(_, s): &(Tick, SeriesStat)| if s.value.is_finite() { s.value } else { 0. })
        .fill(|(_, s): &(Tick, SeriesStat), _, _, _| s.color)
        .grid_dashed(true)
        .value_axis(true)
        .tooltip_value(|(_, s): &(Tick, SeriesStat), _| s.text.clone())
        .value_tick_format(move |v| SharedString::from(field.format(v)))
        .into_any_element()
}

#[derive(Clone)]
struct Bucket {
    x: Tick,
    count: f64,
}

const BUCKETS: usize = 16;

/// Every sample of every series, counted into equal buckets.
pub(super) fn render_histogram(view: &PanelView, cx: &App) -> AnyElement {
    let values: Vec<f64> = view
        .data
        .frame
        .series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .filter(|v| v.is_finite())
        .collect();
    let (Some(min), Some(max)) = (
        values.iter().copied().reduce(f64::min),
        values.iter().copied().reduce(f64::max),
    ) else {
        return no_data(cx);
    };
    let width = ((max - min) / BUCKETS as f64).max(f64::EPSILON);
    let mut buckets: Vec<Bucket> = (0..BUCKETS)
        .map(|index| Bucket {
            x: Tick {
                index,
                time: index as f64,
                label: view.spec.field.format(min + width * index as f64).into(),
            },
            count: 0.,
        })
        .collect();
    for value in values {
        let index = (((value - min) / width) as usize).min(BUCKETS - 1);
        buckets[index].count += 1.;
    }
    let color = view
        .data
        .series
        .first()
        .map(|s| s.color)
        .unwrap_or(gpui_kit::transparent_black());
    BarChart::new(buckets)
        .id(view.element_id("histogram"))
        .band(|b: &Bucket| b.x.clone())
        .value(|b: &Bucket| b.count)
        .band_tick_count(6)
        .padding_inner(0.08)
        .grid_dashed(true)
        .value_axis(true)
        .fill(move |_, _, _, _| color)
        .into_any_element()
}

#[derive(Clone)]
struct Candle {
    x: Tick,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

/// Candles from the first series, three samples each.
pub(super) fn render_candles(view: &PanelView, cx: &App) -> AnyElement {
    let Some(series) = view.data.frame.series.first() else {
        return no_data(cx);
    };
    let candles: Vec<Candle> = series
        .values
        .chunks(3)
        .zip(view.data.rows.chunks(3))
        .filter_map(|(values, rows)| {
            Some(Candle {
                x: rows.first()?.x.clone(),
                open: *values.first()?,
                close: *values.last()?,
                high: values.iter().copied().fold(f64::MIN, f64::max),
                low: values.iter().copied().fold(f64::MAX, f64::min),
            })
        })
        .collect();
    CandlestickChart::new(candles)
        .id(view.element_id("candles"))
        .x(|c: &Candle| c.x.clone())
        .open(|c: &Candle| c.open)
        .high(|c: &Candle| c.high)
        .low(|c: &Candle| c.low)
        .close(|c: &Candle| c.close)
        .tick_margin(5)
        .into_any_element()
}
