//! Transformations: the panel's `transformations`, applied to what its
//! queries return before it is drawn.
//!
//! Renames, filters, ordering and calculated fields run in dashboard order.
//! Tables also join query columns and sort rows. Other transformations
//! land in the panel's ignored list.

use regex::Regex;
use serde_json::Value;

use crate::data::{Frame, Series};
use crate::overrides;
use crate::spec::Calc;

#[derive(Clone, Debug)]
pub enum Transform {
    /// `renameByRegex`: a JS-style replacement (`$1`) over field names.
    RenameByRegex {
        regex: Regex,
        replace: String,
    },
    /// `organize`: hide, rename and order fields.
    Organize {
        exclude: Vec<String>,
        rename: Vec<(String, String)>,
        order: Vec<(String, i64)>,
    },
    /// `merge`, `seriesToColumns`, `joinByField`: one table row per label
    /// set, with a value column per query.
    Join,
    GroupBy {
        fields: Vec<GroupField>,
    },
    /// `filterFieldsByName`: keep only these fields.
    Filter {
        names: Vec<String>,
        pattern: Option<Regex>,
    },
    /// `sortBy`: order table rows by a column.
    SortBy {
        field: String,
        desc: bool,
    },
    Calculate {
        alias: String,
        calculation: Calculation,
        replace: bool,
    },
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GroupField {
    pub name: String,
    pub group: bool,
    pub aggregations: Vec<GroupAggregation>,
}
impl GroupField {
    pub fn new(name: impl Into<String>, group: bool, aggregations: Vec<GroupAggregation>) -> Self {
        Self {
            name: name.into(),
            group,
            aggregations,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum GroupAggregation {
    Reduce(Calc),
    UniqueValues,
}
impl GroupAggregation {
    fn parse(name: &str) -> Option<Self> {
        if name == "uniqueValues" {
            Some(Self::UniqueValues)
        } else {
            Calc::parse(name).map(Self::Reduce)
        }
    }
}
fn group_by(options: &Value) -> Option<Transform> {
    let mut fields = Vec::new();
    for (name, value) in options.get("fields")?.as_object()? {
        let operation = value
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match operation {
            "groupby" => fields.push(GroupField::new(name, true, Vec::new())),
            "aggregate" => {
                let aggregations = value
                    .get("aggregations")?
                    .as_array()?
                    .iter()
                    .map(|a| GroupAggregation::parse(a.as_str()?))
                    .collect::<Option<Vec<_>>>()?;
                fields.push(GroupField::new(name, false, aggregations));
            }
            "" => {}
            _ => return None,
        }
    }
    Some(Transform::GroupBy { fields })
}

#[derive(Clone, Debug)]
pub enum Operand {
    Field(String),
    Fixed(f64),
}

#[derive(Clone, Copy, Debug)]
pub enum Operator {
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Clone, Debug)]
pub enum Calculation {
    Reduce {
        include: Vec<String>,
        reducer: Calc,
    },
    Binary {
        left: Operand,
        right: Operand,
        operator: Operator,
    },
}

impl Calculation {
    fn evaluate(&self, names: &[String], values: &[f64]) -> f64 {
        match self {
            Self::Reduce { include, reducer } => reducer.reduce(
                &names
                    .iter()
                    .zip(values)
                    .filter(|(name, _)| include.is_empty() || include.contains(name))
                    .map(|(_, value)| *value)
                    .collect::<Vec<_>>(),
            ),
            Self::Binary {
                left,
                right,
                operator,
            } => {
                let operand = |value: &Operand| match value {
                    Operand::Fixed(v) => *v,
                    Operand::Field(name) => names
                        .iter()
                        .position(|n| n == name)
                        .map_or(f64::NAN, |i| values[i]),
                };
                let (left, right) = (operand(left), operand(right));
                if !left.is_finite() || !right.is_finite() {
                    return f64::NAN;
                }
                let value = match operator {
                    Operator::Add => left + right,
                    Operator::Subtract => left - right,
                    Operator::Multiply => left * right,
                    Operator::Divide => left / right,
                };
                if value.is_finite() { value } else { f64::NAN }
            }
        }
    }
}

fn calculate(options: &Value) -> Option<Transform> {
    let operand = |value: &Value| -> Option<Operand> {
        if let Some(name) = value.as_str() {
            return Some(Operand::Field(name.into()));
        }
        if let Some(fixed) = value.get("fixed") {
            let number = fixed.as_f64().or_else(|| fixed.as_str()?.parse().ok())?;
            return number.is_finite().then_some(Operand::Fixed(number));
        }
        if value.pointer("/matcher/id")?.as_str()? == "byName" {
            Some(Operand::Field(
                value.pointer("/matcher/options")?.as_str()?.to_owned(),
            ))
        } else {
            None
        }
    };
    let calculation = match options.get("mode")?.as_str()? {
        "reduceRow" => {
            let reduce = options.get("reduce")?;
            let reducer = match reduce.get("reducer")?.as_str()? {
                "mean" => Calc::Mean,
                "sum" => Calc::Sum,
                _ => return None,
            };
            let include = reduce
                .get("include")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            Calculation::Reduce { include, reducer }
        }
        "binary" => {
            let binary = options.get("binary")?;
            let operator = match binary.get("operator")?.as_str()? {
                "+" => Operator::Add,
                "-" => Operator::Subtract,
                "*" => Operator::Multiply,
                "/" => Operator::Divide,
                _ => return None,
            };
            Calculation::Binary {
                left: operand(binary.get("left")?)?,
                right: operand(binary.get("right")?)?,
                operator,
            }
        }
        _ => return None,
    };
    let alias = options
        .get("alias")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| match &calculation {
            Calculation::Reduce {
                reducer: Calc::Mean,
                ..
            } => "Mean".into(),
            Calculation::Reduce { .. } => "Sum".into(),
            Calculation::Binary { .. } => "Calculation".into(),
        });
    Some(Transform::Calculate {
        alias,
        calculation,
        replace: options
            .get("replaceFields")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Reads the transformations; ones the app can't apply go to `ignored`.
pub(crate) fn parse(raw: &[Value], table: bool, ignored: &mut Vec<String>) -> Vec<Transform> {
    let mut out = Vec::new();
    for raw in raw {
        if raw.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = raw.get("id").and_then(Value::as_str).unwrap_or_default();
        let options = raw.get("options").unwrap_or(&Value::Null);
        let text = |key: &str| options.get(key).and_then(Value::as_str).unwrap_or_default();
        let names = |key: &str| -> Vec<(String, Value)> {
            options
                .get(key)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let parsed = match id {
            "calculateField" => calculate(options),
            "groupBy" if table => group_by(options),
            "renameByRegex" => {
                overrides::regex(text("regex"), false).map(|regex| Transform::RenameByRegex {
                    regex,
                    replace: js_replacement(text("renamePattern")),
                })
            }
            "organize" => Some(Transform::Organize {
                exclude: names("excludeByName")
                    .into_iter()
                    .filter(|(_, v)| v.as_bool() == Some(true))
                    .map(|(k, _)| k)
                    .collect(),
                rename: names("renameByName")
                    .into_iter()
                    .filter_map(|(k, v)| {
                        Some((k, v.as_str().filter(|n| !n.is_empty())?.to_owned()))
                    })
                    .collect(),
                order: names("indexByName")
                    .into_iter()
                    .filter_map(|(k, v)| Some((k, v.as_i64()?)))
                    .collect(),
            }),
            "joinByField" if table => {
                // The current join combines reduced values by their complete
                // label sets. It cannot implement Grafana's frame-row joins.
                if let Some(field) = options.get("byField").and_then(Value::as_str) {
                    ignored.push(format!("joinByField on {field}"));
                }
                if text("mode") == "inner" {
                    ignored.push("joinByField inner join".into());
                }
                Some(Transform::Join)
            }
            "merge" | "seriesToColumns" | "joinByLabels" if table => Some(Transform::Join),
            // Charts already draw each series by its name.
            "merge" | "seriesToColumns" | "joinByField" | "labelsToFields" if !table => continue,
            // Tables already have a column per label.
            "labelsToFields" => continue,
            "filterFieldsByName" => {
                let include = options.get("include").unwrap_or(&Value::Null);
                let names = include
                    .get("names")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let pattern = include
                    .get("pattern")
                    .and_then(Value::as_str)
                    .and_then(|p| overrides::regex(p, false));
                Some(Transform::Filter { names, pattern })
            }
            "sortBy" if table => options.pointer("/sort/0").and_then(|sort| {
                Some(Transform::SortBy {
                    field: sort.get("field")?.as_str()?.to_owned(),
                    desc: sort.get("desc").and_then(Value::as_bool).unwrap_or(false),
                })
            }),
            // Charts and stats don't depend on row order.
            "sortBy" => continue,
            _ => None,
        };
        match parsed {
            Some(transform) => out.push(transform),
            None => {
                let note = format!("transformation {id}");
                if !ignored.contains(&note) {
                    ignored.push(note);
                }
            }
        }
    }
    out
}

/// `$1` → `${1}`, so a following letter isn't read as part of the group.
fn js_replacement(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            let mut group = String::new();
            while let Some(d) = chars.next_if(char::is_ascii_digit) {
                group.push(d);
            }
            out.push_str(&format!("${{{group}}}"));
        } else {
            out.push(c);
        }
    }
    out
}

/// A field (series or column) name after the renames.
pub fn rename(transforms: &[Transform], name: &str) -> String {
    let mut name = name.to_owned();
    for transform in transforms {
        match transform {
            Transform::RenameByRegex { regex, replace } => {
                name = regex.replace(&name, replace.as_str()).into_owned();
            }
            Transform::Organize { rename, .. } => {
                if let Some((_, to)) = rename.iter().find(|(from, _)| *from == name) {
                    name.clone_from(to);
                }
            }
            _ => {}
        }
    }
    name
}

/// Whether field `name` survives the filters and organize's exclusions.
/// Checked before renames, against the name as the query returns it.
pub fn keeps(transforms: &[Transform], name: &str) -> bool {
    transforms.iter().all(|transform| match transform {
        Transform::Filter { names, pattern } => {
            names.iter().any(|n| n == name) || pattern.as_ref().is_some_and(|p| p.is_match(name))
        }
        Transform::Organize { exclude, .. } => !exclude.iter().any(|e| e == name),
        _ => true,
    })
}

/// Apply series transformations in dashboard order, before field overrides.
pub fn apply(transforms: &[Transform], mut frame: Frame) -> Frame {
    for transform in transforms {
        let one = std::slice::from_ref(transform);
        match transform {
            Transform::Calculate {
                alias,
                calculation,
                replace,
            } => {
                let names: Vec<_> = frame.series.iter().map(|s| s.name.clone()).collect();
                let values = (0..frame.times.len())
                    .map(|i| {
                        let row: Vec<_> = frame
                            .series
                            .iter()
                            .map(|s| s.values.get(i).copied().unwrap_or(f64::NAN))
                            .collect();
                        calculation.evaluate(&names, &row)
                    })
                    .collect();
                if *replace {
                    frame.series.clear();
                }
                frame.series.push(Series {
                    name: alias.clone(),
                    query: "calculated".into(),
                    field: Some(alias.clone()),
                    labels: Vec::new(),
                    values,
                });
            }
            _ => {
                frame.series.retain_mut(|series| {
                    if !keeps(one, &series.name) {
                        return false;
                    }
                    series.name = rename(one, &series.name);
                    true
                });
                if let Transform::Organize { order, .. } = transform {
                    // Ordering keys refer to the names before this organize's renames.
                    frame.series.sort_by_key(|s| {
                        order
                            .iter()
                            .find(|(name, _)| rename(one, name) == s.name)
                            .map_or(i64::MAX, |(_, index)| *index)
                    });
                }
            }
        }
    }
    frame
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Text(String),
    Number(f64),
}

#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Table {
    /// Names as shown, after renames.
    pub columns: Vec<String>,
    /// Names before renames, which field overrides may match instead.
    pub sources: Vec<String>,
    /// Original query per column; shared columns from joins have no single query.
    pub query_refs: Vec<Option<String>>,
    pub rows: Vec<Vec<Cell>>,
}

/// An explicitly named query column can be present even with one query.
fn references_query_column(transforms: &[Transform], query: &str) -> bool {
    let name = format!("Value #{query}");
    transforms.iter().any(|t| match t {
        Transform::Organize {
            exclude,
            rename,
            order,
        } => {
            exclude.contains(&name)
                || rename
                    .iter()
                    .any(|(n, _)| *n == name || n.starts_with(&format!("{name} (")))
                || order
                    .iter()
                    .any(|(n, _)| *n == name || n.starts_with(&format!("{name} (")))
        }
        Transform::Filter { names, .. } => names.contains(&name),
        Transform::SortBy { field, .. } => *field == name,
        Transform::GroupBy { fields } => fields
            .iter()
            .any(|f| f.name == name || f.name.starts_with(&format!("{name} ("))),
        _ => false,
    })
}

pub(crate) fn value_column(
    transforms: &[Transform],
    field: Option<&str>,
    query: &str,
    query_count: usize,
    joined: bool,
) -> String {
    match field {
        Some(field) => field.to_owned(),
        None if joined && query_count > 1 || references_query_column(transforms, query) => {
            format!("Value #{query}")
        }
        None => "Value".into(),
    }
}

/// The table a table panel shows: a column per label and a value column
/// (one per query when joined), one row per series or label set.
pub fn table(transforms: &[Transform], series: &[Series], queries: &[&str]) -> Table {
    let mut keys: Vec<String> = Vec::new();
    for s in series {
        for (key, _) in &s.labels {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    // SQL results name their value columns; so does a join of several
    // queries, as "Value #A".
    let mut value_columns: Vec<String> = Vec::new();
    for s in series {
        if let Some(field) = &s.field
            && !value_columns.contains(field)
        {
            value_columns.push(field.clone());
        }
    }
    let joined = !value_columns.is_empty()
        || queries.len() > 1
            && (transforms.iter().any(|t| matches!(t, Transform::Join))
                || queries
                    .iter()
                    .any(|q| references_query_column(transforms, q)));
    let column_of = |s: &Series| -> String {
        value_column(
            transforms,
            s.field.as_deref(),
            &s.query,
            queries.len(),
            joined,
        )
    };
    for s in series {
        let column = column_of(s);
        if !value_columns.contains(&column) {
            value_columns.push(column);
        }
    }
    let mut columns: Vec<String> = if keys.is_empty() && !joined {
        vec!["Series".into()]
    } else {
        keys.clone()
    };
    columns.extend(value_columns.iter().cloned());
    let label_cells = |s: &Series| -> Vec<Cell> {
        if keys.is_empty() && !joined {
            return vec![Cell::Text(s.name.clone())];
        }
        keys.iter()
            .map(|key| {
                Cell::Text(
                    s.labels
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default(),
                )
            })
            .collect()
    };
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    if joined {
        let mut index: Vec<Vec<Cell>> = Vec::new();
        for s in series {
            let labels = label_cells(s);
            let at = match index.iter().position(|l| *l == labels) {
                Some(at) => at,
                None => {
                    index.push(labels.clone());
                    let mut row = labels;
                    row.extend(value_columns.iter().map(|_| Cell::Number(f64::NAN)));
                    rows.push(row);
                    rows.len() - 1
                }
            };
            if let Some(q) = value_columns.iter().position(|c| *c == column_of(s)) {
                let width = columns.len() - value_columns.len();
                rows[at][width + q] = Cell::Number(Calc::LastNotNull.reduce(&s.values));
            }
        }
    } else {
        for s in series {
            let mut row = label_cells(s);
            row.push(Cell::Number(Calc::LastNotNull.reduce(&s.values)));
            rows.push(row);
        }
    }
    let query_refs = columns
        .iter()
        .map(|column| {
            let mut origins = series
                .iter()
                .filter(|s| {
                    column_of(s) == *column
                        || s.labels.iter().any(|(key, _)| key == column)
                        || column == "Series"
                })
                .map(|s| &s.query);
            let first = origins.next()?;
            origins.all(|query| query == first).then(|| first.clone())
        })
        .collect();
    let mut table = Table {
        sources: columns.clone(),
        query_refs,
        columns,
        rows,
    };
    for transform in transforms {
        let one = std::slice::from_ref(transform);
        match transform {
            Transform::GroupBy { fields } => table.group(fields),
            Transform::Calculate {
                alias,
                calculation,
                replace,
            } => {
                // Text columns are not numeric inputs to row reductions.
                let values: Vec<_> = table
                    .rows
                    .iter()
                    .map(|row| {
                        let numbers: Vec<_> = row
                            .iter()
                            .map(|cell| match cell {
                                Cell::Number(v) => *v,
                                Cell::Text(_) => f64::NAN,
                            })
                            .collect();
                        Cell::Number(calculation.evaluate(&table.columns, &numbers))
                    })
                    .collect();
                let mut origins = table.query_refs.iter().filter_map(Option::as_ref);
                let query = origins
                    .next()
                    .filter(|first| origins.all(|other| other == *first))
                    .cloned();
                if *replace {
                    table.select(&[]);
                }
                table.columns.push(alias.clone());
                table.sources.push(alias.clone());
                table.query_refs.push(query);
                for (row, value) in table.rows.iter_mut().zip(values) {
                    row.push(value);
                }
            }
            Transform::SortBy { field, desc } => {
                if let Some(col) = table
                    .columns
                    .iter()
                    .position(|c| c == field)
                    .or_else(|| table.sources.iter().position(|c| c == field))
                {
                    table.rows.sort_by(|a, b| {
                        let order = match (&a[col], &b[col]) {
                            (Cell::Number(x), Cell::Number(y)) => x.total_cmp(y),
                            (Cell::Text(x), Cell::Text(y)) => x.cmp(y),
                            _ => std::cmp::Ordering::Equal,
                        };
                        if *desc { order.reverse() } else { order }
                    });
                }
            }
            _ => {
                let mut kept: Vec<_> = (0..table.columns.len())
                    .filter(|&i| keeps(one, &table.columns[i]))
                    .collect();
                if let Transform::Organize { order, .. } = transform {
                    kept.sort_by_key(|&i| {
                        order
                            .iter()
                            .find(|(n, _)| *n == table.columns[i])
                            .map_or(i64::MAX, |(_, o)| *o)
                    });
                }
                table.select(&kept);
                table.columns = table.columns.iter().map(|c| rename(one, c)).collect();
            }
        }
    }
    table
}

impl Table {
    /// Match overrides against a transformed column without losing its origin.
    pub fn field_context(&self, column: usize) -> crate::overrides::FieldContext<'_> {
        use crate::overrides::{FieldContext, FieldType};
        let kind = if self.sources[column] == "Time" {
            FieldType::Time
        } else if self
            .rows
            .iter()
            .any(|row| matches!(row[column], Cell::Number(_)))
        {
            FieldType::Number
        } else {
            FieldType::String
        };
        let mut context = FieldContext::new(&self.columns[column])
            .source(&self.sources[column])
            .kind(kind);
        if let Some(query) = self.query_refs[column].as_deref() {
            context = context.query(query);
        }
        context
    }

    fn group(&mut self, fields: &[GroupField]) {
        let keys: Vec<_> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, name)| fields.iter().any(|f| f.group && f.name == **name))
            .map(|(i, _)| i)
            .collect();
        let aggregates: Vec<_> = self
            .columns
            .iter()
            .enumerate()
            .flat_map(|(i, name)| {
                fields
                    .iter()
                    .filter(move |f| !f.group && f.name == *name)
                    .flat_map(move |f| f.aggregations.iter().map(move |a| (i, *a)))
            })
            .collect();
        let mut groups: Vec<(Vec<Cell>, Vec<usize>)> = Vec::new();
        let equal = |a: &Cell, b: &Cell| match (a, b) {
            (Cell::Number(a), Cell::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
            _ => a == b,
        };
        for (i, row) in self.rows.iter().enumerate() {
            let key: Vec<_> = keys.iter().map(|&i| row[i].clone()).collect();
            if let Some((_, indices)) = groups
                .iter_mut()
                .find(|(k, _)| k.iter().zip(&key).all(|(a, b)| equal(a, b)))
            {
                indices.push(i);
            } else {
                groups.push((key, vec![i]));
            }
        }
        let mut columns: Vec<_> = keys.iter().map(|&i| self.columns[i].clone()).collect();
        let mut sources: Vec<_> = keys.iter().map(|&i| self.sources[i].clone()).collect();
        let mut query_refs: Vec<_> = keys.iter().map(|&i| self.query_refs[i].clone()).collect();
        for &(i, aggregate) in &aggregates {
            let label = match aggregate {
                GroupAggregation::UniqueValues => "uniqueValues",
                GroupAggregation::Reduce(c) => match c {
                    Calc::Last => "last",
                    Calc::LastNotNull => "lastNotNull",
                    Calc::First => "first",
                    Calc::Mean => "mean",
                    Calc::Sum => "sum",
                    Calc::Min => "min",
                    Calc::Max => "max",
                    Calc::Count => "count",
                    Calc::Range => "range",
                    Calc::Delta => "delta",
                },
            };
            columns.push(format!("{} ({label})", self.columns[i]));
            sources.push(self.sources[i].clone());
            query_refs.push(self.query_refs[i].clone());
        }
        let rows = groups
            .into_iter()
            .map(|(mut key, indices)| {
                for &(i, aggregate) in &aggregates {
                    let cells: Vec<_> = indices.iter().map(|&row| &self.rows[row][i]).collect();
                    let value = match aggregate {
                        GroupAggregation::UniqueValues => {
                            let mut unique = Vec::new();
                            for cell in cells {
                                if !unique.iter().any(|c| equal(c, cell)) {
                                    unique.push(cell.clone());
                                }
                            }
                            Cell::Text(
                                unique
                                    .iter()
                                    .map(|c| match c {
                                        Cell::Text(s) => s.clone(),
                                        Cell::Number(v) => v.to_string(),
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            )
                        }
                        GroupAggregation::Reduce(Calc::Last | Calc::LastNotNull | Calc::First)
                            if cells.iter().any(|c| matches!(c, Cell::Text(_))) =>
                        {
                            let present =
                                |c: &&Cell| !matches!(c, Cell::Number(v) if !v.is_finite());
                            let cell = match aggregate {
                                GroupAggregation::Reduce(Calc::First) => {
                                    cells.iter().find(|c| present(c))
                                }
                                GroupAggregation::Reduce(Calc::LastNotNull) => {
                                    cells.iter().rev().find(|c| present(c))
                                }
                                _ => cells.last(),
                            };
                            cell.map(|c| (*c).clone())
                                .unwrap_or(Cell::Text(String::new()))
                        }
                        GroupAggregation::Reduce(Calc::Count) => Cell::Number(
                            cells
                                .iter()
                                .filter(|c| !matches!(c, Cell::Number(v) if !v.is_finite()))
                                .count() as f64,
                        ),
                        GroupAggregation::Reduce(calc) => Cell::Number(
                            calc.reduce(
                                &cells
                                    .iter()
                                    .map(|c| match c {
                                        Cell::Number(v) => *v,
                                        _ => f64::NAN,
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                        ),
                    };
                    key.push(value);
                }
                key
            })
            .collect();
        self.columns = columns;
        self.sources = sources;
        self.query_refs = query_refs;
        self.rows = rows;
    }
    fn select(&mut self, indices: &[usize]) {
        self.columns = indices.iter().map(|&i| self.columns[i].clone()).collect();
        self.sources = indices.iter().map(|&i| self.sources[i].clone()).collect();
        self.query_refs = indices
            .iter()
            .map(|&i| self.query_refs[i].clone())
            .collect();
        for row in &mut self.rows {
            *row = indices.iter().map(|&i| row[i].clone()).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(json: &str, table: bool) -> (Vec<Transform>, Vec<String>) {
        let raw: Vec<Value> = serde_json::from_str(json).unwrap();
        let mut ignored = Vec::new();
        (parse(&raw, table, &mut ignored), ignored)
    }

    fn series(query: &str, pod: &str, value: f64) -> Series {
        Series {
            name: format!("{{pod=\"{pod}\"}}"),
            query: query.to_owned(),
            field: None,
            labels: vec![("pod".into(), pod.into())],
            values: vec![value],
        }
    }

    #[test]
    fn renames_by_regex_and_name() {
        let (t, ignored) = parsed(
            r#"[{"id":"renameByRegex","options":{"regex":"(.*)_bytes","renamePattern":"$1 size"}},
                {"id":"organize","options":{"renameByName":{"heap size":"Heap"}}},
                {"id":"calculateField","options":{}}]"#,
            false,
        );
        assert_eq!(rename(&t, "heap_bytes"), "Heap");
        assert_eq!(rename(&t, "stack_bytes"), "stack size");
        assert_eq!(ignored, ["transformation calculateField"]);
    }

    #[test]
    fn reads_regexes_as_javascript() {
        let (t, ignored) = parsed(
            r#"[{"id":"renameByRegex","options":{"regex":"/.* (.*)/","renamePattern":"$1"}},
                {"id":"sortBy","options":{"sort":[{"field":"Time"}]}}]"#,
            false,
        );
        assert!(ignored.is_empty(), "{ignored:?}");
        assert_eq!(rename(&t, "avg example.com"), "example.com");
        // A brace that isn't a quantifier is literal.
        let (t, _) = parsed(
            r#"[{"id":"renameByRegex","options":{"regex":"value {path=\"(.*)\"}","renamePattern":"$1"}}]"#,
            false,
        );
        assert_eq!(rename(&t, "value {path=\"/api\"}"), "/api");
    }

    #[test]
    fn joins_queries_into_columns() {
        let (t, ignored) = parsed(
            r#"[{"id":"merge","options":{}},
                {"id":"organize","options":{"excludeByName":{"Time":true},
                  "renameByName":{"Value #A":"CPU","Value #B":"Memory"},
                  "indexByName":{"Value #B":0,"pod":1,"Value #A":2}}},
                {"id":"sortBy","options":{"sort":[{"field":"CPU","desc":true}]}}]"#,
            true,
        );
        assert!(ignored.is_empty(), "{ignored:?}");
        let data = [
            series("A", "a", 1.),
            series("A", "b", 3.),
            series("B", "a", 10.),
            series("B", "b", 30.),
        ];
        let table = table(&t, &data, &["A", "B"]);
        assert_eq!(table.columns, ["Memory", "pod", "CPU"]);
        assert_eq!(table.query_refs, [Some("B".into()), None, Some("A".into())]);
        assert_eq!(
            table.rows[0],
            [Cell::Number(30.), Cell::Text("b".into()), Cell::Number(3.)]
        );
        assert_eq!(table.rows.len(), 2);
    }

    #[test]
    fn single_query_scoped_value_name_survives_organize() {
        let (transforms, _) = parsed(
            r#"[{"id":"organize","options":{"renameByName":{"Value #A":"Views"}}}]"#,
            true,
        );
        let data = [series("A", "dashboard", 42.)];
        let table = table(&transforms, &data, &["A"]);
        assert_eq!(table.columns, ["pod", "Views"]);
        assert_eq!(table.sources, ["pod", "Value #A"]);
        assert_eq!(table.query_refs, [Some("A".into()), Some("A".into())]);
        assert_eq!(table.rows[0][1], Cell::Number(42.));
    }
    #[test]
    fn row_reductions_follow_renames_filters_and_order() {
        let (transforms, ignored) = parsed(
            r#"[
            {"id":"organize","options":{"renameByName":{"a":"Linux","b":"Windows"}}},
            {"id":"calculateField","options":{"alias":"Real","mode":"reduceRow","reduce":{"include":["Linux","Windows"],"reducer":"mean"}}},
            {"id":"organize","options":{"excludeByName":{"Linux":true,"Windows":true},"indexByName":{"Real":0,"Limits":1}}}
        ]"#,
            false,
        );
        assert!(ignored.is_empty());
        let mut data = [
            series("A", "a", 2.),
            series("B", "a", 4.),
            series("C", "a", 10.),
        ];
        for (series, name) in data.iter_mut().zip(["a", "b", "Limits"]) {
            series.name = name.into();
        }
        data[0].values = vec![2., f64::NAN];
        data[1].values = vec![4., 8.];
        let frame = apply(
            &transforms,
            Frame {
                times: vec![0., 10.],
                series: data.into(),
            },
        );
        assert_eq!(
            frame
                .series
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Real", "Limits"]
        );
        assert_eq!(frame.series[0].values, [3., 8.]);
    }

    #[test]
    fn sum_and_replace_fields_produce_one_series() {
        let (transforms, ignored) = parsed(
            r#"[{"id":"calculateField","options":{"alias":"Total","mode":"reduceRow","reduce":{"reducer":"sum"},"replaceFields":true}}]"#,
            false,
        );
        assert!(ignored.is_empty());
        let frame = apply(
            &transforms,
            Frame {
                times: vec![0.],
                series: vec![series("A", "a", 2.), series("B", "b", 4.)],
            },
        );
        assert_eq!(frame.series.len(), 1);
        assert_eq!(frame.series[0].name, "Total");
        assert_eq!(frame.series[0].values, [6.]);
    }

    #[test]
    fn table_binary_calculation_uses_renamed_columns() {
        let (transforms, ignored) = parsed(
            r#"[
            {"id":"merge"},
            {"id":"organize","options":{"renameByName":{"Value #A":"Bytes","Value #B":"Rows"}}},
            {"id":"calculateField","options":{"alias":"BytePerRow","mode":"binary","binary":{"left":"Bytes","operator":"/","right":"Rows"}}}
        ]"#,
            true,
        );
        assert!(ignored.is_empty());
        let data = [
            series("A", "one", 100.),
            series("B", "one", 4.),
            series("A", "two", 100.),
            series("B", "two", 0.),
        ];
        let table = table(&transforms, &data, &["A", "B"]);
        assert_eq!(table.columns, ["pod", "Bytes", "Rows", "BytePerRow"]);
        assert_eq!(table.rows[0][3], Cell::Number(25.));
        assert!(matches!(table.rows[1][3], Cell::Number(v) if v.is_nan()));
        assert_eq!(table.sources, ["pod", "Value #A", "Value #B", "BytePerRow"]);
    }

    #[test]
    fn modern_binary_operands_support_named_fields_and_fixed_values() {
        let (transforms, ignored) = parsed(
            r#"[{"id":"calculateField","options":{"alias":"Scaled","mode":"binary","replaceFields":true,"binary":{"left":{"matcher":{"id":"byName","options":"CPU"}},"operator":"*","right":{"fixed":"100"}}}}]"#,
            false,
        );
        assert!(ignored.is_empty());
        let mut s = series("A", "one", 0.25);
        s.name = "CPU".into();
        let frame = apply(
            &transforms,
            Frame {
                times: vec![0.],
                series: vec![s],
            },
        );
        assert_eq!(frame.series[0].values, [25.]);
    }

    #[test]
    fn unsupported_calculation_modes_and_reducers_are_reported() {
        for options in [
            r#"{"mode":"window"}"#,
            r#"{"mode":"reduceRow","reduce":{"reducer":"variance"}}"#,
        ] {
            let (transforms, ignored) = parsed(
                &format!(r#"[{{"id":"calculateField","options":{options}}}]"#),
                false,
            );
            assert!(transforms.is_empty());
            assert_eq!(ignored, ["transformation calculateField"]);
        }
    }
    #[test]
    fn group_by_reduces_and_deduplicates_preserving_sources() {
        let (transforms, ignored) = parsed(
            r#"[{"id":"groupBy","options":{"fields":{"pod":{"operation":"groupby"},"Value":{"operation":"aggregate","aggregations":["sum","mean","lastNotNull"]},"instance":{"operation":"aggregate","aggregations":["uniqueValues"]}}}}]"#,
            true,
        );
        assert!(ignored.is_empty());
        let Transform::GroupBy { fields } = &transforms[0] else {
            panic!()
        };
        let mut table = Table {
            columns: vec!["pod".into(), "Value".into(), "instance".into()],
            sources: vec!["pod".into(), "Value".into(), "instance".into()],
            query_refs: vec![Some("A".into()); 3],
            rows: vec![
                vec![
                    Cell::Text("a".into()),
                    Cell::Number(2.),
                    Cell::Text("host1".into()),
                ],
                vec![
                    Cell::Text("a".into()),
                    Cell::Number(4.),
                    Cell::Text("host2".into()),
                ],
                vec![
                    Cell::Text("b".into()),
                    Cell::Number(8.),
                    Cell::Text("host1".into()),
                ],
                vec![
                    Cell::Text("a".into()),
                    Cell::Number(f64::NAN),
                    Cell::Text("host1".into()),
                ],
            ],
        };
        table.group(fields);
        assert_eq!(
            table.columns,
            [
                "pod",
                "Value (sum)",
                "Value (mean)",
                "Value (lastNotNull)",
                "instance (uniqueValues)"
            ]
        );
        assert_eq!(
            table.sources,
            ["pod", "Value", "Value", "Value", "instance"]
        );
        assert_eq!(table.rows.len(), 2);
        assert_eq!(
            table.rows[0],
            vec![
                Cell::Text("a".into()),
                Cell::Number(6.),
                Cell::Number(3.),
                Cell::Number(4.),
                Cell::Text("host1, host2".into())
            ]
        );
    }

    #[test]
    fn group_by_query_columns_can_be_renamed_after_aggregation() {
        let (transforms, ignored) = parsed(
            r#"[{"id":"groupBy","options":{"fields":{"pod":{"operation":"groupby"},"Value #A":{"operation":"aggregate","aggregations":["lastNotNull"]},"Value #B":{"operation":"aggregate","aggregations":["sum"]}}}},{"id":"organize","options":{"renameByName":{"Value #A (lastNotNull)":"Used","Value #B (sum)":"Total"}}}]"#,
            true,
        );
        assert!(ignored.is_empty());
        let table = table(
            &transforms,
            &[series("A", "p", 2.), series("B", "p", 8.)],
            &["A", "B"],
        );
        assert_eq!(table.columns, ["pod", "Used", "Total"]);
        assert_eq!(table.query_refs, [None, Some("A".into()), Some("B".into())]);
        assert_eq!(
            table.rows[0],
            [Cell::Text("p".into()), Cell::Number(2.), Cell::Number(8.)]
        );
    }

    #[test]
    fn unknown_group_by_reducers_remain_reported() {
        let (transforms, ignored) = parsed(
            r#"[{"id":"groupBy","options":{"fields":{"Value":{"operation":"aggregate","aggregations":["unsupported"]}}}}]"#,
            true,
        );
        assert!(transforms.is_empty());
        assert_eq!(ignored, ["transformation groupBy"]);
    }
    #[test]
    fn named_field_and_inner_joins_are_not_reported_as_fully_supported() {
        let (transforms, ignored) = parsed(
            r#"[{"id":"joinByField","options":{"byField":"CustomerID","mode":"inner"}}]"#,
            true,
        );
        assert_eq!(transforms.len(), 1);
        assert_eq!(
            ignored,
            ["joinByField on CustomerID", "joinByField inner join"]
        );
    }
}
