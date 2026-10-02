//! Kit's Table over transformed data, with per-column field formatting and
//! Grafana's colored cells and gauge modes.
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, SharedString, div, linear_color_stop, linear_gradient};
use grafaui_model::spec::{CellDisplay, ColorMode, FieldSpec, GaugeValue};
use grafaui_model::transform::Cell;
use std::rc::Rc;

use super::{PanelView, no_data};
use crate::ui::{self, dp};

pub(crate) struct Column {
    pub index: usize,
    pub field: Rc<FieldSpec>,
}

/// Resolve column overrides and ranges when data changes, including
/// value matchers and the query's original name before a transformation.
pub(super) fn columns(
    table: &grafaui_model::transform::Table,
    defaults: &FieldSpec,
) -> Vec<Column> {
    table
        .columns
        .iter()
        .zip(&table.sources)
        .enumerate()
        .filter_map(|(i, _)| {
            let numeric = table
                .rows
                .iter()
                .any(|row| matches!(row.get(i), Some(Cell::Number(_))));
            let values: Vec<_> = table
                .rows
                .iter()
                .filter_map(|row| match row.get(i) {
                    Some(Cell::Number(v)) => Some(*v),
                    _ => None,
                })
                .collect();
            let mut context = table.field_context(i);
            if numeric {
                context = context.values(&values);
            }
            if defaults.style_for_field(&context).hidden {
                return None;
            }
            let mut field = defaults.for_field(&context).into_owned();
            let initial = if matches!(field.color, ColorMode::Continuous(_)) {
                (f64::INFINITY, f64::NEG_INFINITY)
            } else {
                (0., 0.)
            };
            let (min, max) = values
                .into_iter()
                .filter(|v| v.is_finite())
                .fold(initial, |(min, max), v| (min.min(v), max.max(v)));
            let min = if min.is_finite() { min } else { 0. };
            field.min = field.min.or(Some(min));
            field.max = field.max.or(Some(if max > min { max } else { min + 1. }));
            Some(Column {
                index: i,
                field: Rc::new(field),
            })
        })
        .collect()
}

pub(super) fn render(view: &PanelView, cx: &App) -> AnyElement {
    let Some(table) = view.data.table.as_ref().filter(|t| !t.rows.is_empty()) else {
        return no_data(cx);
    };
    let columns = &view.data.table_columns;
    if columns.is_empty() {
        return no_data(cx);
    }
    let header = TableHeader::new().child(TableRow::new().children(columns.iter().map(|c| {
        TableHead::new().child(cell_text(SharedString::from(
            table.columns[c.index].clone(),
        )))
    })));
    let body = TableBody::new().children(table.rows.iter().map(|row| {
        TableRow::new().children(columns.iter().map(|column| match &row[column.index] {
            Cell::Text(text) => TableCell::new().child(cell_text(text.clone().into())),
            Cell::Number(value) => numeric_cell(*value, &column.field, cx),
        }))
    }));
    div()
        .id(view.element_id("table"))
        .size_full()
        .overflow_y_scroll()
        .child(Table::new().small().child(header).child(body))
        .into_any_element()
}

fn numeric_cell(value: f64, field: &FieldSpec, cx: &App) -> TableCell {
    let color = ui::hsla(field.series_color(0, value));
    let text = SharedString::from(field.display(value));
    match field.cell_display {
        CellDisplay::Auto => TableCell::new().child(cell_text(text)),
        CellDisplay::ColorText => TableCell::new().child(cell_text(text).text_color(color)),
        CellDisplay::ColorBackground { gradient } => {
            // Tinting preserves theme foreground contrast, including mapped
            // text. Data colors belong to the dashboard, not the UI theme.
            let background = if gradient {
                linear_gradient(
                    90.,
                    linear_color_stop(color.opacity(0.12), 0.),
                    linear_color_stop(color.opacity(0.42), 1.),
                )
            } else {
                color.opacity(0.3).into()
            };
            TableCell::new().bg(background).child(cell_text(text))
        }
        CellDisplay::Gauge {
            mode,
            value: display,
        } => {
            let min = field.min.unwrap_or(0.);
            let max = field.max.unwrap_or(100.);
            let track = super::color_scale::Scale::new(field, (min, max), color).bar(
                value,
                mode,
                cx.theme().muted,
                true,
            );
            TableCell::new().child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap(dp(6.))
                    .child(div().flex_1().min_w(dp(36.)).h(dp(12.)).child(track))
                    .when(display != GaugeValue::Hidden, |this| {
                        this.child(
                            cell_text(text)
                                .flex_none()
                                .when(display == GaugeValue::Color, |this| this.text_color(color)),
                        )
                    }),
            )
        }
    }
}

fn cell_text(text: SharedString) -> gpui_kit::Div {
    div().min_w_0().max_w_full().truncate().child(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafaui_model::data::{DataSource, FakeSource, QueryContext};
    use grafaui_model::spec::CellGauge;

    #[test]
    fn lcd_override_reaches_the_single_query_fixture_column() {
        let dashboard =
            grafaui_model::Dashboard::parse(include_str!("../../../../fixtures/real/15083.json"))
                .unwrap();
        let panel = dashboard
            .panels
            .iter()
            .find(|p| p.title == "Top 10 dashboards")
            .unwrap();
        let context = QueryContext {
            window: grafaui_model::time::TimeWindow::new(0, 3600, 20),
            variables: Vec::new(),
        };
        let frame = FakeSource::new("table-cells").query(panel, &context);
        let table = grafaui_model::transform::table(&panel.transforms, &frame.series, &["A"]);
        let columns = columns(&table, &panel.field);
        let views = columns
            .iter()
            .find(|c| table.columns[c.index] == "Views")
            .unwrap();
        assert_eq!(
            views.field.cell_display,
            CellDisplay::Gauge {
                mode: CellGauge::Lcd,
                value: GaugeValue::Text
            }
        );
        assert!(
            table.rows.len() > 1,
            "column overrides must not become fake row names"
        );
    }
    #[test]
    fn query_type_and_visibility_overrides_follow_transformed_columns() {
        use grafaui_model::data::Series;
        let dashboard = grafaui_model::Dashboard::parse(r#"{"panels":[{"type":"table","fieldConfig":{"overrides":[
          {"matcher":{"id":"byType","options":"string"},"properties":[{"id":"custom.hideFrom.viz","value":true}]},
          {"matcher":{"id":"byName","options":"Value #A"},"properties":[{"id":"custom.hideFrom.viz","value":true}]},
          {"matcher":{"id":"byName","options":"CPU"},"properties":[{"id":"custom.hideFrom.viz","value":false}]},
          {"matcher":{"id":"byFrameRefID","options":"B"},"properties":[{"id":"unit","value":"bytes"},{"id":"custom.cellOptions","value":{"type":"color-text"}}]}
        ]},"transformations":[{"id":"organize","options":{"renameByName":{"Value #A":"CPU","Value #B":"Memory"},"indexByName":{"Value #B":0}}}]}]}"#).unwrap();
        let panel = &dashboard.panels[0];
        let series: Vec<_> = [("A", 3.), ("B", 1024.)]
            .into_iter()
            .map(|(query, value)| Series {
                name: "same".into(),
                query: query.into(),
                field: None,
                labels: vec![("pod".into(), "web".into())],
                values: vec![value],
            })
            .collect();
        let table = grafaui_model::transform::table(&panel.transforms, &series, &["A", "B"]);
        let shown = columns(&table, &panel.field);
        assert_eq!(
            shown
                .iter()
                .map(|c| table.columns[c.index].as_str())
                .collect::<Vec<_>>(),
            ["Memory", "CPU"]
        );
        assert_eq!(shown[0].field.unit.as_deref(), Some("bytes"));
        assert_eq!(shown[0].field.cell_display, CellDisplay::ColorText);
        assert_eq!(shown[1].field.unit, None);
    }
}
