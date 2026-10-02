//! Dashboard time ranges such as `now-6h`, and the sample times inside them.

/// Samples fake data is generated at.
pub const POINTS: usize = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeWindow {
    /// Unix seconds of the last sample.
    pub end: i64,
    pub span: u64,
    pub points: usize,
}

impl TimeWindow {
    pub fn new(end: i64, span: u64, points: usize) -> Self {
        Self {
            end,
            span: span.max(1),
            points: points.max(2),
        }
    }

    /// A window ending at `now` for a relative range (`now-6h` to `now`).
    /// Absolute and unreadable ranges fall back to six hours.
    pub fn relative(from: &str, now: i64) -> Self {
        Self::new(now, parse_relative(from).unwrap_or(6 * 3600), POINTS)
    }

    pub fn step(&self) -> i64 {
        (self.span / (self.points as u64 - 1)).max(1) as i64
    }

    pub fn times(&self) -> Vec<i64> {
        let step = self.step();
        let start = self.end - step * (self.points as i64 - 1);
        (0..self.points as i64).map(|i| start + i * step).collect()
    }
}

/// Seconds in `now-<n><unit>`, with an optional `/<unit>` rounding suffix
/// ignored.
pub fn parse_relative(from: &str) -> Option<u64> {
    let spec = from.trim().strip_prefix("now-")?;
    let spec = spec.split('/').next()?;
    let digits = spec.find(|c: char| !c.is_ascii_digit())?;
    let (count, unit) = spec.split_at(digits);
    let count: u64 = count.parse().ok()?;
    let seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        "M" => 2_592_000,
        "y" => 31_536_000,
        _ => return None,
    };
    Some(count * seconds)
}

/// Short label for a span, as Grafana's time picker shows it.
pub fn describe(span: u64) -> String {
    const UNITS: &[(u64, &str)] = &[
        (31_536_000, "year"),
        (604_800, "week"),
        (86_400, "day"),
        (3600, "hour"),
        (60, "minute"),
        (1, "second"),
    ];
    let (size, name) = UNITS
        .iter()
        .find(|(size, _)| span >= *size && span.is_multiple_of(*size))
        .copied()
        .unwrap_or((1, "second"));
    let count = span / size;
    let plural = if count == 1 { "" } else { "s" };
    format!("Last {count} {name}{plural}")
}

/// `HH:MM` in UTC for axis ticks, with the date for spans over two days.
pub fn tick_label(time: i64, span: u64) -> String {
    let day_seconds = time.rem_euclid(86_400);
    let (hour, minute) = (day_seconds / 3600, day_seconds % 3600 / 60);
    if span > 2 * 86_400 {
        let (_, month, day) = civil(time.div_euclid(86_400));
        format!("{month:02}/{day:02} {hour:02}:{minute:02}")
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

/// `YYYY-MM-DD HH:MM:SS` in UTC for a Unix time in seconds.
pub fn date_time(time: i64) -> String {
    let (year, month, day) = civil(time.div_euclid(86_400));
    let seconds = time.rem_euclid(86_400);
    format!(
        "{year}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_ranges() {
        assert_eq!(parse_relative("now-6h"), Some(21_600));
        assert_eq!(parse_relative("now-7d/d"), Some(604_800));
        assert_eq!(parse_relative("now-15m"), Some(900));
        assert_eq!(parse_relative("2024-01-01T00:00:00Z"), None);
        assert_eq!(describe(21_600), "Last 6 hours");
        assert_eq!(describe(86_400), "Last 1 day");
    }

    #[test]
    fn spaces_samples_evenly_up_to_the_end() {
        let times = TimeWindow::new(1000, 900, 4).times();
        assert_eq!(times, [100, 400, 700, 1000]);
        assert_eq!(tick_label(1_700_000_000, 3600), "22:13");
        assert_eq!(tick_label(1_700_000_000, 7 * 86_400), "11/14 22:13");
    }
}
