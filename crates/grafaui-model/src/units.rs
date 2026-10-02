//! Formats values the way Grafana's common units do.
//!
//! Covers the units dashboards use most. Like Grafana, any other unit id is
//! shown as a suffix after the plain number.

/// Formats `value` in `unit` with `decimals` places, or a precision picked
/// from the magnitude when `decimals` is `None`.
use crate::time;

pub fn format(value: f64, unit: Option<&str>, decimals: Option<u32>) -> String {
    if value.is_nan() {
        return "No data".into();
    }
    let unit = unit.unwrap_or("short");
    match unit {
        "" | "short" => scaled(value, 1000., &["", " K", " Mil", " Bil", " Tri"], decimals),
        "none" => number(value, decimals),
        "percent" => format!("{}%", number(value, decimals)),
        "percentunit" => format!("{}%", number(value * 100., decimals)),
        "bytes" => scaled(
            value,
            1024.,
            &[" B", " KiB", " MiB", " GiB", " TiB", " PiB"],
            decimals,
        ),
        "decbytes" => scaled(
            value,
            1000.,
            &[" B", " kB", " MB", " GB", " TB", " PB"],
            decimals,
        ),
        "bits" => scaled(value, 1000., &[" b", " kb", " Mb", " Gb", " Tb"], decimals),
        "bps" => scaled(
            value,
            1000.,
            &[" bps", " kbps", " Mbps", " Gbps", " Tbps"],
            decimals,
        ),
        "Bps" | "binBps" => scaled(
            value,
            1024.,
            &[" B/s", " KiB/s", " MiB/s", " GiB/s", " TiB/s"],
            decimals,
        ),
        "decBps" => scaled(value, 1000., &[" B/s", " kB/s", " MB/s", " GB/s"], decimals),
        "s" => duration(value, decimals),
        "ms" => duration(value / 1e3, decimals),
        "µs" | "us" => duration(value / 1e6, decimals),
        "ns" => duration(value / 1e9, decimals),
        "reqps" => suffixed(value, " req/s", decimals),
        "rps" => suffixed(value, " rps", decimals),
        "ops" => suffixed(value, " ops/s", decimals),
        "iops" => suffixed(value, " io/s", decimals),
        "wps" => suffixed(value, " wps", decimals),
        "cps" => suffixed(value, " c/s", decimals),
        "pps" => scaled(value, 1000., &[" p/s", " Kp/s", " Mp/s", " Gp/s"], decimals),
        "decbits" => scaled(value, 1000., &[" b", " kb", " Mb", " Gb", " Tb"], decimals),
        "kbytes" => scaled(
            value * 1024.,
            1024.,
            &[" B", " KiB", " MiB", " GiB", " TiB", " PiB"],
            decimals,
        ),
        "mbytes" => scaled(
            value * 1024. * 1024.,
            1024.,
            &[" B", " KiB", " MiB", " GiB", " TiB", " PiB"],
            decimals,
        ),
        "rotrpm" => suffixed(value, " rpm", decimals),
        "bool" => (if value != 0. { "True" } else { "False" }).into(),
        "bool_yes_no" => (if value != 0. { "Yes" } else { "No" }).into(),
        "bool_on_off" => (if value != 0. { "On" } else { "Off" }).into(),
        "hertz" => scaled(value, 1000., &[" Hz", " kHz", " MHz", " GHz"], decimals),
        "celsius" => format!("{}°C", number(value, decimals)),
        "fahrenheit" => format!("{}°F", number(value, decimals)),
        "watt" => scaled(value, 1000., &[" W", " kW", " MW"], decimals),
        "locale" => locale(value, decimals),
        "string" => number(value, decimals),
        "decmbytes" => scaled(
            value * 1e6,
            1000.,
            &[" B", " kB", " MB", " GB", " TB", " PB"],
            decimals,
        ),
        "binbps" => scaled(
            value,
            1024.,
            &[" b/s", " Kib/s", " Mib/s", " Gib/s", " Tib/s"],
            decimals,
        ),
        "eps" => suffixed(value, " evt/s", decimals),
        "opm" => suffixed(value, " ops/min", decimals),
        "rpm" | "reqpm" => suffixed(value, " req/min", decimals),
        "dtdurations" | "dtdhms" | "dtdurationms" => {
            let seconds = if unit == "dtdurationms" {
                value / 1e3
            } else {
                value
            };
            long_duration(seconds)
        }
        "clocks" | "clockms" => {
            let seconds = (if unit == "clockms" {
                value / 1e3
            } else {
                value
            })
            .max(0.) as i64;
            format!(
                "{:02}:{:02}:{:02}",
                seconds / 3600,
                seconds % 3600 / 60,
                seconds % 60
            )
        }
        "dateTimeAsIso"
        | "dateTimeAsIsoNoDateIfToday"
        | "dateTimeAsUS"
        | "dateTimeAsUSNoDateIfToday"
        | "dateTimeAsLocal"
        | "dateTimeAsLocalNoDateIfToday"
        | "dateTimeAsSystem" => time::date_time((value / 1e3) as i64),
        "dateTimeFromNow" => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0., |d| d.as_secs_f64());
            format!(
                "{} ago",
                long_duration(now - value / 1e3)
                    .split(", ")
                    .next()
                    .unwrap_or_default()
            )
        }
        "currencyUSD" => format!("${}", number(value, decimals)),
        "currencyEUR" => format!("€{}", number(value, decimals)),
        _ => match unit.strip_prefix("suffix:") {
            Some(suffix) => format!("{}{suffix}", number(value, decimals)),
            None => format!("{} {unit}", number(value, decimals)),
        },
    }
}

fn number(value: f64, decimals: Option<u32>) -> String {
    let places = decimals.unwrap_or_else(|| auto_decimals(value)) as usize;
    let text = format!("{value:.places$}");
    if decimals.is_none() && text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

fn auto_decimals(value: f64) -> u32 {
    let magnitude = value.abs();
    if magnitude == 0. || magnitude >= 100. {
        0
    } else if magnitude >= 10. {
        1
    } else if magnitude >= 1. {
        2
    } else {
        3
    }
}

fn scaled(value: f64, step: f64, suffixes: &[&str], decimals: Option<u32>) -> String {
    let mut scaled = value;
    let mut index = 0;
    while scaled.abs() >= step && index + 1 < suffixes.len() {
        scaled /= step;
        index += 1;
    }
    format!("{}{}", number(scaled, decimals), suffixes[index])
}

fn suffixed(value: f64, suffix: &str, decimals: Option<u32>) -> String {
    format!("{}{suffix}", number(value, decimals))
}

/// Digits grouped by thousands: `1,234,567`.
fn locale(value: f64, decimals: Option<u32>) -> String {
    let text = number(value.abs(), decimals);
    let (whole, fraction) = text
        .split_once('.')
        .map_or((text.as_str(), None), |(w, f)| (w, Some(f)));
    let mut grouped = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if value < 0. { "-" } else { "" };
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// `3 days, 4 hours`: the two largest whole units.
fn long_duration(seconds: f64) -> String {
    let mut left = seconds.max(0.) as u64;
    let mut parts = Vec::new();
    for (size, name) in [
        (31_536_000, "year"),
        (86_400, "day"),
        (3600, "hour"),
        (60, "minute"),
        (1, "second"),
    ] {
        let count = left / size;
        if count > 0 && parts.len() < 2 {
            parts.push(format!(
                "{count} {name}{}",
                if count == 1 { "" } else { "s" }
            ));
            left %= size;
        }
    }
    if parts.is_empty() {
        "0 seconds".into()
    } else {
        parts.join(", ")
    }
}

fn duration(seconds: f64, decimals: Option<u32>) -> String {
    let magnitude = seconds.abs();
    let (value, suffix) = if magnitude == 0. {
        (0., " s")
    } else if magnitude < 1e-6 {
        (seconds * 1e9, " ns")
    } else if magnitude < 1e-3 {
        (seconds * 1e6, " µs")
    } else if magnitude < 1. {
        (seconds * 1e3, " ms")
    } else if magnitude < 60. {
        (seconds, " s")
    } else if magnitude < 3600. {
        (seconds / 60., " min")
    } else if magnitude < 86400. {
        (seconds / 3600., " hour")
    } else {
        (seconds / 86400., " day")
    };
    format!("{}{suffix}", number(value, decimals))
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn formats_common_units() {
        assert_eq!(format(42.123, Some("percent"), None), "42.1%");
        assert_eq!(format(0.5, Some("percentunit"), Some(0)), "50%");
        assert_eq!(format(1536., Some("bytes"), None), "1.5 KiB");
        assert_eq!(format(2_500_000., Some("decbytes"), Some(1)), "2.5 MB");
        assert_eq!(format(0.25, Some("s"), None), "250 ms");
        assert_eq!(format(90., Some("s"), None), "1.5 min");
        assert_eq!(format(1234567., Some("locale"), Some(0)), "1,234,567");
        assert_eq!(format(93784., Some("dtdurations"), None), "1 day, 2 hours");
        assert_eq!(format(3725., Some("clocks"), None), "01:02:05");
        assert_eq!(
            format(1.7e12, Some("dateTimeAsIso"), None),
            "2023-11-14 22:13:20"
        );
        assert_eq!(format(1234., Some("short"), None), "1.23 K");
        assert_eq!(format(12., Some("reqps"), None), "12 req/s");
        assert_eq!(format(3., Some("suffix: pods"), None), "3 pods");
        assert_eq!(format(7., Some("weird-unit"), None), "7 weird-unit");
        assert_eq!(format(f64::NAN, None, None), "No data");
    }
}
