//! The shape of a SQL query's result, read from its text without a full
//! parser: enough for the mock to return the right columns and series.
//!
//! The SELECT list gives the columns, named by alias. The time column is
//! the one built from a time macro or named like time; aggregates are
//! values; with a GROUP BY, the other columns split the result into series.
//! Without one, columns are told apart by name. ClickHouse macros that
//! wrap the select list (`$rate(…)`, `$columns(key, value)`) are unwrapped
//! first.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Time,
    /// A label: splits series, or a text column in a table.
    Text,
    Number,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub kind: Kind,
    /// A string literal's text: the same in every row.
    pub constant: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub columns: Vec<Column>,
}

impl Shape {
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.of(Kind::Text)
    }

    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.of(Kind::Number)
    }

    fn of(&self, kind: Kind) -> impl Iterator<Item = &str> {
        self.columns
            .iter()
            .filter(move |c| c.kind == kind)
            .map(|c| c.name.as_str())
    }
}

/// Functions whose result is one value per group.
const AGGREGATES: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "quantile",
    "median",
    "uniq",
    "any",
    "argmax",
    "argmin",
    "stddev",
    "var",
    "topk",
    "grouparray",
    "anylast",
    "first_value",
    "last_value",
    "percentile",
    "approx_count_distinct",
    "rate",
    "increase",
];

/// Names a time column usually has.
const TIME_NAMES: &[&str] = &[
    "t",
    "time",
    "timestamp",
    "ts",
    "_time",
    "minute",
    "hour",
    "day",
    "date",
    "datetime",
    "event_time",
    "time_sec",
    "interval",
    "bucket",
    "period",
];

/// Words in a column name that suggest a number, for queries without a
/// GROUP BY.
const NUMBER_HINTS: &[&str] = &[
    "count", "total", "sum", "avg", "max", "min", "size", "bytes", "space", "free", "used", "rows",
    "ms", "sec", "duration", "elapsed", "percent", "pct", "ratio", "rate", "num", "qps", "memory",
    "parts", "value", "latency", "cpu", "load", "errors", "requests", "hits", "misses", "usage",
    "speed", "lag", "delay", "uptime", "cnt", "amount", "score", "level_", "p50", "p90", "p95",
    "p99",
];

/// The result shape of `query`, or `None` if it isn't a SELECT.
pub fn shape(query: &str) -> Option<Shape> {
    let query = strip_comments(query);
    let lower = query.to_lowercase();
    // `$columns(key, value)`: a series per key, of value.
    for macro_name in [
        "$columns(",
        "$ratecolumns(",
        "$perseccolumns(",
        "$deltacolumns(",
    ] {
        if let Some(at) = lower.find(macro_name) {
            let args = split_top(inside_parens(&query[at + macro_name.len() - 1..])?);
            let mut columns = Vec::new();
            columns.push(Column {
                name: "t".into(),
                kind: Kind::Time,
                constant: None,
            });
            if let Some(key) = args.first() {
                columns.push(Column {
                    name: item_name(key),
                    kind: Kind::Text,
                    constant: None,
                });
            }
            if let Some(value) = args.get(1) {
                columns.push(Column {
                    name: item_name(value),
                    kind: Kind::Number,
                    constant: None,
                });
            }
            return Some(Shape { columns });
        }
    }
    // `$rate(a, b)` and friends: the arguments are the select list.
    for macro_name in ["$rate(", "$persecond(", "$delta(", "$increase("] {
        if let Some(at) = lower.find(macro_name) {
            let args = split_top(inside_parens(&query[at + macro_name.len() - 1..])?);
            let mut columns = vec![Column {
                name: "t".into(),
                kind: Kind::Time,
                constant: None,
            }];
            columns.extend(args.iter().map(|a| Column {
                name: item_name(a),
                kind: Kind::Number,
                constant: None,
            }));
            return Some(Shape { columns });
        }
    }
    let select = find_top_word(&lower, "select", 0)?;
    let from = find_top_word(&lower, "from", select + 6).unwrap_or(query.len());
    let grouped =
        find_top_word(&lower, "group by", from).is_some() || lower[from..].contains("group by");
    let items = split_top(&query[select + 6..from]);
    let mut columns: Vec<Column> = items
        .iter()
        .map(|item| {
            let name = item_name(item);
            let lower_item = item.to_lowercase();
            let lower_name = name.to_lowercase();
            let expr = expression(item);
            let literal = expr.len() >= 2 && expr.starts_with('\'') && expr.ends_with('\'');
            let constant = literal.then(|| expr[1..expr.len() - 1].to_owned());
            let kind = if constant.is_some() {
                Kind::Text
            } else if is_time(&lower_item, &lower_name) {
                Kind::Time
            } else if is_aggregate(&lower_item) {
                Kind::Number
            } else if grouped {
                Kind::Text
            } else if looks_numeric(&lower_item, &lower_name) {
                Kind::Number
            } else {
                Kind::Text
            };
            Column {
                name,
                kind,
                constant,
            }
        })
        .filter(|c| !c.name.is_empty() && c.name != "*")
        .collect();
    if columns.is_empty() {
        columns.push(Column {
            name: "value".into(),
            kind: Kind::Number,
            constant: None,
        });
    }
    Some(Shape { columns })
}

/// Whether `query` reads as SQL rather than PromQL or LogQL.
pub fn is_sql(query: &str) -> bool {
    let lower = strip_comments(query).trim_start().to_lowercase();
    lower.starts_with("select")
        || lower.starts_with("with ")
        || lower.starts_with('$') && lower.contains("from")
}

fn strip_comments(query: &str) -> String {
    query
        .lines()
        .map(|line| match line.find("--") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_time(item: &str, name: &str) -> bool {
    [
        "$timeseries",
        "$__timeinterval",
        "$__timegroup",
        "$__time(",
        "tostartof",
        "todatetime",
        "$__unixepochgroup",
    ]
    .iter()
    .any(|m| item.contains(m))
        || TIME_NAMES.contains(&name)
}

fn is_aggregate(item: &str) -> bool {
    calls(item).any(|call| AGGREGATES.iter().any(|a| call.starts_with(a)))
}

fn looks_numeric(item: &str, name: &str) -> bool {
    NUMBER_HINTS.iter().any(|hint| name.contains(hint))
        || item.trim().parse::<f64>().is_ok()
        || item.contains(['/', '*', '+'])
        || item.contains(" - ")
}

/// Lower-case names of the functions `item` calls.
fn calls(item: &str) -> impl Iterator<Item = &str> {
    item.match_indices('(').filter_map(|(at, _)| {
        let before = item[..at].trim_end();
        let start = before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        Some(&before[start..]).filter(|name| !name.is_empty())
    })
}

/// A select item without its alias.
fn expression(item: &str) -> &str {
    let item = item.trim();
    let lower = item.to_lowercase();
    match lower.rfind(" as ") {
        Some(at) if lower[..at].matches('(').count() == lower[..at].matches(')').count() => {
            item[..at].trim()
        }
        _ => item,
    }
}

/// The column name of a select item: its alias, else the column it reads.
fn item_name(item: &str) -> String {
    let item = item.trim();
    let lower = item.to_lowercase();
    let unquote = |s: &str| s.trim().trim_matches(['"', '`', '\'', '[', ']']).to_owned();
    // `expr AS alias`, the last one at the top level.
    let mut alias_at = None;
    let mut depth = 0i32;
    for (at, c) in lower.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 && lower[at..].starts_with(" as ") => alias_at = Some(at + 4),
            _ => {}
        }
    }
    if let Some(at) = alias_at {
        return unquote(&item[at..]);
    }
    // `max(x) ms`: a trailing bare alias after a call.
    if let Some((head, tail)) = item.rsplit_once(char::is_whitespace) {
        let tail = tail.trim();
        let bare = tail
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '"' || c == '`');
        let head = head.trim_end();
        let column = !head.is_empty()
            && head
                .chars()
                .all(|c| c.is_alphanumeric() || "_.\"`".contains(c));
        if bare && (head.ends_with(')') || column) {
            return unquote(tail);
        }
    }
    // A plain (maybe qualified) column.
    if item
        .chars()
        .all(|c| c.is_alphanumeric() || "_.\"`".contains(c))
    {
        return unquote(item.rsplit('.').next().unwrap_or(item));
    }
    item.chars().take(32).collect()
}

/// The text between the parenthesis `text` starts with and its match.
fn inside_parens(text: &str) -> Option<&str> {
    let mut depth = 0;
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[1..at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits on commas outside parentheses and quotes.
fn split_top(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0i32;
    let mut quote = None;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(c),
            (None, '(' | '[') => depth += 1,
            (None, ')' | ']') => depth -= 1,
            (None, ',') if depth == 0 => {
                items.push(text[start..at].trim().to_owned());
                start = at + 1;
            }
            _ => {}
        }
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        items.push(last.to_owned());
    }
    items
}

/// The byte offset of `word` at parenthesis depth 0, outside quotes, from
/// `start`. `lower` is the lower-cased query.
fn find_top_word(lower: &str, word: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote = None;
    let bytes = lower.as_bytes();
    for (at, c) in lower.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, _) if at >= start && depth == 0 && lower[at..].starts_with(word) => {
                let before = at.checked_sub(1).map(|i| bytes[i]);
                let after = bytes.get(at + word.len()).copied();
                let boundary =
                    |b: Option<u8>| b.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_'));
                if boundary(before) && boundary(after) {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(query: &str) -> Vec<(String, Kind)> {
        shape(query)
            .unwrap()
            .columns
            .into_iter()
            .map(|c| (c.name, c.kind))
            .collect()
    }

    #[test]
    fn grouped_time_series() {
        let got = kinds(
            "SELECT $timeSeries as t, user, max(query_duration_ms) ms FROM system.query_log \
             WHERE $timeFilter GROUP BY t, user ORDER BY t",
        );
        assert_eq!(
            got,
            [
                ("t".into(), Kind::Time),
                ("user".into(), Kind::Text),
                ("ms".into(), Kind::Number)
            ]
        );
    }

    #[test]
    fn grafana_macros_and_label_after_value() {
        let got = kinds(
            "SELECT $__timeInterval(timestamp) as time, count(*) AS \"Requests\", \
             toString(status) as name FROM logs WHERE $__timeFilter(timestamp) GROUP BY name, time",
        );
        assert_eq!(
            got,
            [
                ("time".into(), Kind::Time),
                ("Requests".into(), Kind::Number),
                ("name".into(), Kind::Text)
            ]
        );
    }

    #[test]
    fn table_without_group_by() {
        let got = kinds(
            "SELECT name as Name, path as Path, free_space as Free, total_space as Total, \
             1 - free_space/total_space as Used FROM system.disks",
        );
        let kinds: Vec<Kind> = got.iter().map(|(_, k)| *k).collect();
        assert_eq!(
            kinds,
            [
                Kind::Text,
                Kind::Text,
                Kind::Number,
                Kind::Number,
                Kind::Number
            ]
        );
        assert_eq!(got[4].0, "Used");
    }

    #[test]
    fn altinity_macros() {
        assert_eq!(
            kinds("$rate(count() c) FROM $table"),
            [("t".into(), Kind::Time), ("c".into(), Kind::Number)]
        );
        assert_eq!(
            kinds("$columns(host, count() c) FROM $table"),
            [
                ("t".into(), Kind::Time),
                ("host".into(), Kind::Text),
                ("c".into(), Kind::Number)
            ]
        );
    }

    #[test]
    fn subqueries_and_promql() {
        let got = kinds(
            "WITH x AS (SELECT a FROM b) SELECT ip_proto, sum(bytes) AS bytes FROM (SELECT * FROM flows) GROUP BY ip_proto",
        );
        assert_eq!(
            got,
            [
                ("ip_proto".into(), Kind::Text),
                ("bytes".into(), Kind::Number)
            ]
        );
        assert!(!is_sql("sum by (pod) (rate(x[5m]))"));
        let got =
            shape("SELECT database Database, '整体' as name, count() c FROM t GROUP BY database")
                .unwrap();
        assert_eq!(got.columns[0].name, "Database");
        assert_eq!(got.columns[1].constant.as_deref(), Some("整体"));
        assert!(is_sql("  select 1"));
    }
}
