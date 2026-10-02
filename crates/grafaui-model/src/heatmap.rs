//! Heatmap options and bucket grids, independent of the desktop renderer.
use crate::color::{self, Rgba};
use crate::data::Frame;
use crate::schema::RawPanel;
use crate::units;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BucketSizing {
    Auto,
    Count(usize),
    Size(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Samples,
    Rows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Greens,
    Oranges,
    Spectral,
    RedYellowGreen,
    Turbo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeatColor {
    Scheme(Palette),
    Opacity(Rgba),
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Options {
    pub source: Source,
    pub x_buckets: BucketSizing,
    pub y_buckets: BucketSizing,
    pub y_log: Option<f64>,
    pub unit: Option<String>,
    pub decimals: Option<u32>,
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    pub y_visible: bool,
    pub x_visible: bool,
    pub y_right: bool,
    pub y_reverse: bool,
    pub color: HeatColor,
    pub reverse_color: bool,
    pub exponent: f64,
    pub color_steps: usize,
    pub color_min: Option<f64>,
    pub color_max: Option<f64>,
    pub filter_le: Option<f64>,
    pub filter_ge: Option<f64>,
    pub gap: f32,
    pub legend: bool,
    pub tooltip: bool,
    pub histogram: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            source: Source::Samples,
            x_buckets: BucketSizing::Auto,
            y_buckets: BucketSizing::Auto,
            y_log: None,
            unit: None,
            decimals: None,
            y_min: None,
            y_max: None,
            y_visible: true,
            x_visible: true,
            y_right: false,
            y_reverse: false,
            color: HeatColor::Scheme(Palette::Spectral),
            reverse_color: false,
            exponent: 0.5,
            color_steps: 128,
            color_min: None,
            color_max: None,
            filter_le: Some(0.),
            filter_ge: None,
            gap: 1.,
            legend: true,
            tooltip: true,
            histogram: false,
        }
    }
}

impl Options {
    pub fn visible(&self, count: f64) -> bool {
        count.is_finite()
            && self.filter_le.is_none_or(|v| count > v)
            && self.filter_ge.is_none_or(|v| count < v)
    }

    pub fn format(&self, value: f64) -> String {
        units::format(value, self.unit.as_deref(), self.decimals)
    }

    /// Count colors are independent of the units of the bucket bounds.
    pub fn color_at(&self, fraction: f64) -> Rgba {
        let fraction = if fraction.is_nan() {
            0.
        } else {
            fraction.clamp(0., 1.)
        };
        let steps = self.color_steps.max(2) - 1;
        let fraction = (fraction * steps as f64).round() / steps as f64;
        match self.color {
            HeatColor::Opacity(color) => {
                let alpha =
                    (fraction.powf(self.exponent) * f64::from(color.alpha())).round() as u32;
                Rgba((color.0 & 0xffffff00) | alpha)
            }
            HeatColor::Scheme(palette) => palette.sample(if self.reverse_color {
                1. - fraction
            } else {
                fraction
            }),
        }
    }
}

impl Palette {
    fn parse(name: &str) -> Option<Self> {
        Some(match name.strip_prefix("interpolate").unwrap_or(name) {
            "Greens" => Self::Greens,
            "Oranges" => Self::Oranges,
            "Spectral" => Self::Spectral,
            "RdYlGn" => Self::RedYellowGreen,
            "Turbo" => Self::Turbo,
            _ => return None,
        })
    }

    pub fn sample(self, fraction: f64) -> Rgba {
        // ColorBrewer stops in Grafana's dark-theme order. D3's Turbo uses
        // its published polynomial; other schemes interpolate these stops.
        const GREENS: &[Rgba] = &[
            Rgba::rgb(0x00441b),
            Rgba::rgb(0x006d2c),
            Rgba::rgb(0x238b45),
            Rgba::rgb(0x41ab5d),
            Rgba::rgb(0x74c476),
            Rgba::rgb(0xa1d99b),
            Rgba::rgb(0xc7e9c0),
            Rgba::rgb(0xe5f5e0),
            Rgba::rgb(0xf7fcf5),
        ];
        const ORANGES: &[Rgba] = &[
            Rgba::rgb(0x7f2704),
            Rgba::rgb(0xa63603),
            Rgba::rgb(0xd94801),
            Rgba::rgb(0xf16913),
            Rgba::rgb(0xfd8d3c),
            Rgba::rgb(0xfdae6b),
            Rgba::rgb(0xfdd0a2),
            Rgba::rgb(0xfee6ce),
            Rgba::rgb(0xfff5eb),
        ];
        const SPECTRAL: &[Rgba] = &[
            Rgba::rgb(0x5e4fa2),
            Rgba::rgb(0x3288bd),
            Rgba::rgb(0x66c2a5),
            Rgba::rgb(0xabdda4),
            Rgba::rgb(0xe6f598),
            Rgba::rgb(0xffffbf),
            Rgba::rgb(0xfee08b),
            Rgba::rgb(0xfdae61),
            Rgba::rgb(0xf46d43),
            Rgba::rgb(0xd53e4f),
            Rgba::rgb(0x9e0142),
        ];
        const RD_YL_GN: &[Rgba] = &[
            Rgba::rgb(0x006837),
            Rgba::rgb(0x1a9850),
            Rgba::rgb(0x66bd63),
            Rgba::rgb(0xa6d96a),
            Rgba::rgb(0xd9ef8b),
            Rgba::rgb(0xffffbf),
            Rgba::rgb(0xfee08b),
            Rgba::rgb(0xfdae61),
            Rgba::rgb(0xf46d43),
            Rgba::rgb(0xd73027),
            Rgba::rgb(0xa50026),
        ];
        let t = if fraction.is_nan() {
            0.
        } else {
            fraction.clamp(0., 1.)
        };
        if self == Self::Turbo {
            let r = 34.61
                + t * (1172.33 - t * (10793.56 - t * (33300.12 - t * (38394.49 - t * 14825.05))));
            let g =
                23.31 + t * (557.33 + t * (1225.33 - t * (3574.96 - t * (1073.77 + t * 707.56))));
            let b =
                27.2 + t * (3211.1 - t * (15327.97 - t * (27814. - t * (22569.18 - t * 6838.66))));
            let channel = |v: f64| v.round().clamp(0., 255.) as u32;
            return Rgba::rgb(channel(r) << 16 | channel(g) << 8 | channel(b));
        }
        color::interpolate(
            match self {
                Self::Greens => GREENS,
                Self::Oranges => ORANGES,
                Self::Spectral => SPECTRAL,
                Self::RedYellowGreen => RD_YL_GN,
                _ => unreachable!(),
            },
            t,
        )
    }
}

pub(crate) fn parse(raw: &RawPanel, ignored: &mut Vec<String>) -> Options {
    let mut out = Options::default();
    let modern = &raw.options;
    let legacy = Value::Object(raw.extra.clone());
    let get = |path: &str, old: &str| modern.pointer(path).or_else(|| legacy.pointer(old));
    let number = |path, old| get(path, old).and_then(num);
    let boolean = |path, old| get(path, old).and_then(Value::as_bool);
    let text = |path, old| get(path, old).and_then(Value::as_str);
    let bucket_query = raw.targets.iter().any(|q| {
        q.extra.get("format").and_then(Value::as_str) == Some("heatmap")
            || q.expr.as_deref().is_some_and(|e| e.contains("_bucket"))
    });
    out.source = match modern.get("calculate").and_then(Value::as_bool) {
        Some(true) => Source::Samples,
        Some(false) => Source::Rows,
        None if bucket_query || text("/dataFormat", "/dataFormat") == Some("tsbuckets") => {
            Source::Rows
        }
        _ => Source::Samples,
    };
    out.x_buckets = sizing(modern.pointer("/calculation/xBuckets"), true);
    out.y_buckets = sizing(modern.pointer("/calculation/yBuckets"), false);
    let log = number("/calculation/yBuckets/scale/log", "/yAxis/logBase");
    if modern
        .pointer("/calculation/yBuckets/scale/type")
        .and_then(Value::as_str)
        == Some("log")
        || log.is_some_and(|l| l > 1.)
    {
        out.y_log = Some(log.filter(|l| *l > 1.).unwrap_or(2.));
    }
    out.unit = text("/yAxis/unit", "/yAxis/format").map(str::to_owned);
    out.decimals = number("/yAxis/decimals", "/yAxis/decimals")
        .filter(|v| *v >= 0.)
        .map(|v| v.min(12.) as u32);
    out.y_min = number("/yAxis/min", "/yAxis/min");
    out.y_max = number("/yAxis/max", "/yAxis/max");
    out.y_visible = boolean("/yAxis/show", "/yAxis/show").unwrap_or(true)
        && text("/yAxis/axisPlacement", "/yAxis/axisPlacement") != Some("hidden");
    out.x_visible = boolean("/xAxis/show", "/xAxis/show").unwrap_or(true);
    out.y_right = text("/yAxis/axisPlacement", "/yAxis/axisPlacement") == Some("right");
    out.y_reverse = boolean("/yAxis/reverse", "/yAxis/reverse").unwrap_or(false);
    let fill = text("/color/fill", "/color/cardColor")
        .and_then(color::parse)
        .unwrap_or(color::GREEN);
    out.color = if text("/color/mode", "/color/mode") == Some("opacity") {
        HeatColor::Opacity(fill)
    } else {
        let scheme = text("/color/scheme", "/color/colorScheme").unwrap_or("Spectral");
        HeatColor::Scheme(Palette::parse(scheme).unwrap_or_else(|| {
            ignored.push(format!("heatmap color scheme {scheme}"));
            Palette::Spectral
        }))
    };
    out.reverse_color = boolean("/color/reverse", "/color/reverse").unwrap_or(false);
    out.exponent = if matches!(
        text("/color/scale", "/color/colorScale"),
        Some("exponential" | "sqrt")
    ) {
        number("/color/exponent", "/color/exponent")
            .unwrap_or(0.5)
            .clamp(0.05, 10.)
    } else {
        1.
    };
    out.color_steps = number("/color/steps", "/color/steps")
        .unwrap_or(128.)
        .clamp(2., 256.) as usize;
    out.color_min = number("/color/min", "/color/min");
    out.color_max = number("/color/max", "/color/max");
    out.filter_le = number("/filterValues/le", "/filterValues/le").or_else(|| {
        boolean("/hideZeroBuckets", "/hideZeroBuckets")
            .unwrap_or(true)
            .then_some(0.)
    });
    out.filter_ge = number("/filterValues/ge", "/filterValues/ge");
    out.gap = number("/cellGap", "/cards/cardPadding")
        .unwrap_or(1.)
        .clamp(0., 10.) as f32;
    out.legend = boolean("/legend/show", "/legend/show").unwrap_or(true);
    out.tooltip = boolean("/tooltip/show", "/tooltip/show").unwrap_or(true)
        && text("/tooltip/mode", "/tooltip/mode") != Some("none");
    out.histogram = boolean("/tooltip/yHistogram", "/tooltip/showHistogram").unwrap_or(false);
    out
}

fn num(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|v| v.is_finite())
}

fn sizing(value: Option<&Value>, time: bool) -> BucketSizing {
    let Some(value) = value else {
        return BucketSizing::Auto;
    };
    let amount = value
        .get("value")
        .and_then(|v| {
            if time {
                v.as_str().and_then(duration).or_else(|| num(v))
            } else {
                num(v)
            }
        })
        .filter(|v| *v > 0.);
    match (value.get("mode").and_then(Value::as_str), amount) {
        (Some("count"), Some(v)) => BucketSizing::Count(v.clamp(1., 128.) as usize),
        (Some("size"), Some(v)) => BucketSizing::Size(v),
        _ => BucketSizing::Auto,
    }
}

fn duration(text: &str) -> Option<f64> {
    let at = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let amount: f64 = text[..at].parse().ok()?;
    Some(
        amount
            * match text[at..].trim() {
                "" | "s" => 1.,
                "ms" => 0.001,
                "m" | "min" => 60.,
                "h" => 3600.,
                "d" => 86400.,
                _ => return None,
            },
    )
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Bucket {
    pub min: f64,
    pub max: f64,
    pub label: Option<String>,
}

impl Bucket {
    pub fn new(min: f64, max: f64) -> Self {
        Self {
            min,
            max,
            label: None,
        }
    }
    pub fn label(&self, options: &Options) -> String {
        self.label.clone().unwrap_or_else(|| {
            format!(
                "{} – {}",
                options.format(self.min),
                if self.max.is_infinite() {
                    "+Inf".into()
                } else {
                    options.format(self.max)
                }
            )
        })
    }
}

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Grid {
    /// Time edges, one longer than each count row.
    pub times: Vec<i64>,
    pub buckets: Vec<Bucket>,
    /// Bucket rows, low to high; missing counts stay NaN.
    pub counts: Vec<Vec<f64>>,
}

impl Grid {
    pub fn count_range(&self, options: &Options) -> (f64, f64) {
        let mut counts = self
            .counts
            .iter()
            .flatten()
            .copied()
            .filter(|v| v.is_finite());
        let first = counts.next().unwrap_or(0.);
        let (min, max) = counts.fold((first, first), |(min, max), v| (min.min(v), max.max(v)));
        let min = options.color_min.unwrap_or(min.min(0.));
        let max = options.color_max.unwrap_or(max);
        (min, if max > min { max } else { min + 1. })
    }

    pub fn from_frame(frame: &Frame, options: &Options) -> Self {
        if frame.times.is_empty() || frame.series.is_empty() {
            return Self::default();
        }
        // Heatmap bucket edges have whole-second resolution.
        let sample_times: Vec<i64> = frame.times.iter().map(|t| t.floor() as i64).collect();
        let step = sample_times
            .windows(2)
            .map(|t| t[1] - t[0])
            .find(|t| *t > 0)
            .unwrap_or(1);
        let min_time = sample_times[0];
        let max_time = sample_times.last().copied().unwrap() + step;
        let size = match options.x_buckets {
            BucketSizing::Size(v) => v.max(1.).ceil() as i64,
            BucketSizing::Count(n) => ((max_time - min_time) as f64 / n.max(1) as f64)
                .ceil()
                .max(1.) as i64,
            BucketSizing::Auto => step,
        }
        .max(((max_time - min_time) as f64 / 512.).ceil().max(1.) as i64);
        let start = min_time.div_euclid(size) * size;
        let columns = ((max_time - start) as f64 / size as f64).ceil() as usize;
        let times = (0..=columns).map(|i| start + size * i as i64).collect();
        let column = |i: usize| ((sample_times[i] - start) / size) as usize;
        let mut grid = Self {
            times,
            ..Self::default()
        };
        if options.source == Source::Rows {
            let bounds: Vec<_> = frame
                .series
                .iter()
                .map(|s| {
                    s.labels.iter().find(|(k, _)| k == "le").and_then(|(_, v)| {
                        v.parse::<f64>()
                            .ok()
                            .or_else(|| (v == "+Inf").then_some(f64::INFINITY))
                    })
                })
                .collect();
            if bounds.iter().all(Option::is_some) {
                let mut upper: Vec<_> = bounds
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|v| *v > 0.)
                    .collect();
                upper.sort_by(f64::total_cmp);
                upper.dedup();
                let mut previous = vec![0_f64; frame.times.len()];
                let mut lower = 0.;
                for upper in upper {
                    let mut cumulative = vec![f64::NAN; frame.times.len()];
                    for (s, _) in frame
                        .series
                        .iter()
                        .zip(&bounds)
                        .filter(|(_, bound)| **bound == Some(upper))
                    {
                        for (i, &v) in s.values.iter().take(cumulative.len()).enumerate() {
                            if v.is_finite() {
                                cumulative[i] = if cumulative[i].is_finite() {
                                    cumulative[i] + v
                                } else {
                                    v
                                };
                            }
                        }
                    }
                    let mut counts = vec![f64::NAN; columns];
                    for i in 0..frame.times.len() {
                        if cumulative[i].is_finite() && previous[i].is_finite() {
                            let count = (cumulative[i] - previous[i]).max(0.);
                            let target = &mut counts[column(i)];
                            *target = if target.is_finite() {
                                *target + count
                            } else {
                                count
                            };
                        }
                    }
                    grid.buckets.push(Bucket::new(lower, upper));
                    grid.counts.push(counts);
                    lower = upper;
                    previous = cumulative;
                }
            } else {
                for (i, s) in frame.series.iter().enumerate() {
                    let mut bucket = Bucket::new(i as f64, (i + 1) as f64);
                    bucket.label = Some(s.name.clone());
                    let mut counts = vec![f64::NAN; columns];
                    for (i, &v) in s.values.iter().take(frame.times.len()).enumerate() {
                        if v.is_finite() {
                            let target = &mut counts[column(i)];
                            *target = if target.is_finite() { *target + v } else { v };
                        }
                    }
                    grid.buckets.push(bucket);
                    grid.counts.push(counts);
                }
            }
        } else {
            let values: Vec<_> = frame
                .series
                .iter()
                .flat_map(|s| &s.values)
                .copied()
                .filter(|v| v.is_finite() && (options.y_log.is_none() || *v > 0.))
                .collect();
            if values.is_empty() {
                return Self::default();
            }
            let min = options
                .y_min
                .unwrap_or(values.iter().copied().reduce(f64::min).unwrap());
            let min = if options.y_log.is_some() && min <= 0. {
                values.iter().copied().reduce(f64::min).unwrap()
            } else {
                min
            };
            let max = options
                .y_max
                .unwrap_or(values.iter().copied().reduce(f64::max).unwrap());
            let max = if max > min {
                max
            } else {
                min + min.abs().max(1.)
            };
            let base = options
                .y_log
                .filter(|b| b.is_finite() && *b > 1.)
                .unwrap_or(2.);
            let project = |v: f64| {
                if options.y_log.is_some() {
                    v.log(base)
                } else {
                    v
                }
            };
            let invert = |v: f64| {
                if options.y_log.is_some() {
                    base.powf(v)
                } else {
                    v
                }
            };
            let (low, high) = (project(min), project(max));
            let rows = match options.y_buckets {
                BucketSizing::Count(n) => n.clamp(1, 128),
                BucketSizing::Size(v) if v > 0. => {
                    ((high - low) / v).ceil().clamp(1., 128.) as usize
                }
                _ => 25,
            };
            let width = match options.y_buckets {
                BucketSizing::Size(v) if v > 0. => v.max((high - low) / 128.),
                _ => (high - low) / rows as f64,
            };
            for i in 0..rows {
                grid.buckets.push(Bucket::new(
                    invert(low + width * i as f64),
                    invert(low + width * (i + 1) as f64),
                ));
                grid.counts.push(vec![0.; columns]);
            }
            for s in &frame.series {
                for (i, &v) in s.values.iter().take(frame.times.len()).enumerate() {
                    if v.is_finite() && v >= min && v <= max && (options.y_log.is_none() || v > 0.)
                    {
                        let row = (((project(v) - low) / width).floor() as usize).min(rows - 1);
                        grid.counts[row][column(i)] += 1.;
                    }
                }
            }
        }
        grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Series;

    fn options(json: &str) -> (Options, Vec<String>) {
        let raw: RawPanel = serde_json::from_str(json).unwrap();
        let mut ignored = Vec::new();
        (parse(&raw, &mut ignored), ignored)
    }
    fn series(bound: &str, values: Vec<f64>) -> Series {
        Series {
            name: bound.into(),
            query: "A".into(),
            field: None,
            labels: vec![("le".into(), bound.into())],
            values,
        }
    }

    #[test]
    fn parses_legacy_histogram_and_opacity_options() {
        let (o, ignored) = options(
            r##"{"type":"heatmap","dataFormat":"tsbuckets","yAxis":{"format":"ms","logBase":1},"color":{"mode":"opacity","cardColor":"#73BF69","colorScale":"sqrt","exponent":0.4},"hideZeroBuckets":false,"legend":{"show":false},"tooltip":{"showHistogram":true}}"##,
        );
        assert!(ignored.is_empty());
        assert_eq!(o.source, Source::Rows);
        assert_eq!(o.unit.as_deref(), Some("ms"));
        assert!(o.y_log.is_none());
        assert_eq!(o.color, HeatColor::Opacity(Rgba::rgb(0x73bf69)));
        assert_eq!(o.exponent, 0.4);
        assert!(o.visible(0.));
        assert!(!o.legend);
        assert!(o.histogram);
        assert_eq!(o.color_at(0.).alpha(), 0);
        assert_eq!(o.color_at(1.).alpha(), 255);
    }

    #[test]
    fn parses_modern_calculated_buckets_and_palette() {
        let (o, ignored) = options(
            r#"{"type":"heatmap","options":{"calculate":true,"calculation":{"xBuckets":{"mode":"size","value":"1min"},"yBuckets":{"mode":"count","value":"30","scale":{"type":"log","log":2}}},"color":{"scheme":"RdYlGn","reverse":true},"filterValues":{"le":0.01},"cellGap":2,"yAxis":{"unit":"s","axisPlacement":"right","reverse":true},"tooltip":{"mode":"none"}}}"#,
        );
        assert!(ignored.is_empty());
        assert_eq!(o.source, Source::Samples);
        assert_eq!(o.x_buckets, BucketSizing::Size(60.));
        assert_eq!(o.y_buckets, BucketSizing::Count(30));
        assert_eq!(o.y_log, Some(2.));
        assert_eq!(o.color, HeatColor::Scheme(Palette::RedYellowGreen));
        assert_eq!(o.color_at(0.), Palette::RedYellowGreen.sample(1.));
        assert!(o.y_reverse && o.y_right && !o.tooltip);
        assert!(!o.visible(0.01));
        assert!(o.visible(0.02));
    }

    #[test]
    fn cumulative_histograms_are_sorted_summed_and_deaccumulated() {
        let frame = Frame {
            times: vec![0., 10.],
            series: vec![
                series("+Inf", vec![12., 18.]),
                series("2", vec![8., 12.]),
                series("1", vec![2., 3.]),
                series("1", vec![3., 4.]),
            ],
        };
        let mut o = Options {
            source: Source::Rows,
            ..Options::default()
        };
        let grid = Grid::from_frame(&frame, &o);
        assert_eq!(grid.counts, [vec![5., 7.], vec![3., 5.], vec![4., 6.]]);
        assert_eq!(grid.buckets[1], Bucket::new(1., 2.));
        assert_eq!(grid.buckets.last().unwrap().max, f64::INFINITY);
        o.x_buckets = BucketSizing::Size(20.);
        assert_eq!(
            Grid::from_frame(&frame, &o).counts,
            [vec![12.], vec![8.], vec![10.]]
        );
    }

    #[test]
    fn missing_histogram_buckets_remain_missing() {
        let frame = Frame {
            times: vec![0., 10.],
            series: vec![series("1", vec![2., f64::NAN]), series("2", vec![5., 8.])],
        };
        let o = Options {
            source: Source::Rows,
            ..Options::default()
        };
        let grid = Grid::from_frame(&frame, &o);
        assert_eq!(grid.counts[1][0], 3.);
        assert!(grid.counts[0][1].is_nan() && grid.counts[1][1].is_nan());
        assert!(!o.visible(grid.counts[1][1]));
    }

    #[test]
    fn samples_use_fixed_width_buckets_and_include_the_upper_edge() {
        let frame = Frame {
            times: vec![0., 10., 20.],
            series: vec![
                series("", vec![0., 2., 5.]),
                series("", vec![1., 3., f64::NAN]),
            ],
        };
        let o = Options {
            y_buckets: BucketSizing::Size(2.),
            ..Options::default()
        };
        let grid = Grid::from_frame(&frame, &o);
        assert_eq!(
            grid.buckets,
            [
                Bucket::new(0., 2.),
                Bucket::new(2., 4.),
                Bucket::new(4., 6.)
            ]
        );
        assert_eq!(
            grid.counts,
            [vec![2., 0., 0.], vec![0., 2., 0.], vec![0., 0., 1.]]
        );
    }

    #[test]
    fn logarithmic_buckets_ignore_nonpositive_and_missing_samples() {
        let frame = Frame {
            times: vec![0., 10., 20.],
            series: vec![
                series("", vec![1., 2., 8.]),
                series("", vec![0., -1., f64::NAN]),
            ],
        };
        let o = Options {
            y_log: Some(2.),
            y_buckets: BucketSizing::Size(1.),
            ..Options::default()
        };
        let grid = Grid::from_frame(&frame, &o);
        assert_eq!(
            grid.buckets,
            [
                Bucket::new(1., 2.),
                Bucket::new(2., 4.),
                Bucket::new(4., 8.)
            ]
        );
        assert_eq!(
            grid.counts,
            [vec![1., 0., 0.], vec![0., 1., 0.], vec![0., 0., 1.]]
        );
    }

    #[test]
    fn bucket_categories_keep_their_transformed_labels() {
        let frame = Frame {
            times: vec![0., 10.],
            series: vec![Series {
                labels: vec![],
                ..series("10", vec![4., 7.])
            }],
        };
        let o = Options {
            source: Source::Rows,
            ..Options::default()
        };
        let grid = Grid::from_frame(&frame, &o);
        assert_eq!(grid.buckets[0].label(&o), "10");
        assert_eq!(grid.counts[0], [4., 7.]);
    }

    #[test]
    fn unknown_palettes_remain_visible_in_support_report() {
        let (_, ignored) =
            options(r#"{"type":"heatmap","options":{"color":{"scheme":"unknown"}}}"#);
        assert_eq!(ignored, ["heatmap color scheme unknown"]);
    }
}
