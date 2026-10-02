//! Every dashboard under `fixtures/` (the built-in ones and the real ones
//! downloaded from grafana.com) parses, places each panel once without
//! overlaps, and answers fake queries without panicking.

use std::path::{Path, PathBuf};

use grafaui_model::data::{DataSource, FakeSource, QueryContext};
use grafaui_model::layout::COLUMNS;
use grafaui_model::time::TimeWindow;
use grafaui_model::transform;
use grafaui_model::{Dashboard, Viz};

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut paths = Vec::new();
    for dir in [root.clone(), root.join("real")] {
        for entry in std::fs::read_dir(&dir).expect("fixtures directory") {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    assert!(paths.len() > 3, "found only {} fixtures", paths.len());
    paths
}

// Keep the original feature regressions scoped to their known input set.
// The general smoke test below covers all downloaded fixtures, including gaps.
fn baseline_fixtures() -> Vec<PathBuf> {
    let sources: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sources/expanded.json")).unwrap();
    fixtures()
        .into_iter()
        .filter(|path| {
            !sources["dashboards"]
                .as_array()
                .unwrap()
                .iter()
                .any(|source| path.ends_with(source["file"].as_str().unwrap()))
        })
        .collect()
}

#[test]
fn every_fixture_lays_out_and_queries() {
    let mut failures = Vec::new();
    for path in fixtures() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let json = std::fs::read_to_string(&path).unwrap();
        let dashboard = match Dashboard::parse(&json) {
            Ok(dashboard) => dashboard,
            Err(error) => {
                failures.push(format!("{name}: parse: {error}"));
                continue;
            }
        };
        // Each panel placed exactly once, inside the grid, without overlaps.
        let mut seen = vec![0; dashboard.panels.len()];
        for section in &dashboard.sections {
            for (i, a) in section.panels.iter().enumerate() {
                seen[a.key] += 1;
                if a.pos.x + a.pos.w > COLUMNS || a.pos.w == 0 || a.pos.h == 0 {
                    failures.push(format!("{name}: panel {} at {:?}", a.key, a.pos));
                }
                for b in &section.panels[i + 1..] {
                    let (a, b) = (a.pos, b.pos);
                    let overlap =
                        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                    if overlap {
                        failures.push(format!("{name}: {a:?} overlaps {b:?}"));
                    }
                }
            }
        }
        for (key, count) in seen.iter().enumerate() {
            if *count != 1 {
                failures.push(format!("{name}: panel {key} placed {count} times"));
            }
        }
        // Fake queries for every panel, and the table for table panels.
        let source = FakeSource::new(&name);
        let context = QueryContext {
            window: TimeWindow::relative("now-6h", 1_759_400_000),
            variables: dashboard
                .variables
                .iter()
                .map(|v| (v.name.clone(), v.current.clone()))
                .collect(),
        };
        for panel in &dashboard.panels {
            let frame = source.query(panel, &context);
            if frame.series.is_empty() && !matches!(panel.viz, Viz::Text(_) | Viz::Unsupported) {
                failures.push(format!("{name}: {:?} returned no series", panel.title));
            }
            if matches!(panel.viz, Viz::Table) {
                let queries: Vec<&str> = panel.queries.iter().map(|q| q.ref_id.as_str()).collect();
                let table = transform::table(&panel.transforms, &frame.series, &queries);
                if table.query_refs.len() != table.columns.len()
                    || table
                        .rows
                        .iter()
                        .any(|row| row.len() != table.columns.len())
                {
                    failures.push(format!("{name}: {:?} has ragged rows", panel.title));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} problems:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn heatmaps_and_calculated_fields_work_with_their_real_fixtures() {
    use grafaui_model::heatmap::Grid;
    use grafaui_model::transform::{Cell, Transform};
    let context = QueryContext {
        window: TimeWindow::relative("now-6h", 1_759_400_000),
        variables: Vec::new(),
    };
    let mut heatmaps = 0;
    let mut calculations = 0;
    for path in baseline_fixtures() {
        let dashboard = Dashboard::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let source = FakeSource::new(&dashboard.title);
        for panel in &dashboard.panels {
            if let Viz::Heatmap(options) = &panel.viz {
                let frame = transform::apply(&panel.transforms, source.query(panel, &context));
                let grid = Grid::from_frame(&frame, options);
                assert!(
                    !grid.buckets.is_empty(),
                    "{}: {}",
                    path.display(),
                    panel.title
                );
                assert!(
                    grid.counts
                        .iter()
                        .all(|row| row.len() + 1 == grid.times.len())
                );
                assert!(
                    grid.counts
                        .iter()
                        .flatten()
                        .all(|v| !v.is_finite() || *v >= 0.)
                );
                assert!(
                    grid.counts
                        .iter()
                        .flatten()
                        .any(|v| v.is_finite() && *v > 0.)
                );
                let again = Grid::from_frame(
                    &transform::apply(&panel.transforms, source.query(panel, &context)),
                    options,
                );
                assert_eq!(grid.buckets, again.buckets);
                assert!(
                    grid.counts
                        .iter()
                        .flatten()
                        .zip(again.counts.iter().flatten())
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
                assert!(
                    panel.ignored.is_empty(),
                    "{}: {:?}",
                    panel.title,
                    panel.ignored
                );
                heatmaps += 1;
            }
            if !panel
                .transforms
                .iter()
                .any(|t| matches!(t, Transform::Calculate { .. }))
            {
                continue;
            }
            let frame = source.query(panel, &context);
            if matches!(panel.viz, Viz::Table) {
                let queries: Vec<_> = panel.queries.iter().map(|q| q.ref_id.as_str()).collect();
                let table = transform::table(&panel.transforms, &frame.series, &queries);
                for alias in panel.transforms.iter().filter_map(|t| match t {
                    Transform::Calculate { alias, .. } => Some(alias),
                    _ => None,
                }) {
                    let column = table
                        .columns
                        .iter()
                        .position(|name| name == alias)
                        .expect("calculated column");
                    assert!(
                        table.rows.iter().any(
                            |r| matches!(r[column], Cell::Number(v) if v.is_finite() && v > 0.)
                        ),
                        "{}",
                        panel.title
                    );
                }
            } else {
                let result = transform::apply(&panel.transforms, frame);
                for alias in panel.transforms.iter().filter_map(|t| match t {
                    Transform::Calculate { alias, .. } => Some(alias),
                    _ => None,
                }) {
                    let series = result
                        .series
                        .iter()
                        .find(|s| &s.name == alias)
                        .expect("calculated series");
                    assert!(
                        series.values.iter().any(|v| v.is_finite() && *v > 0.),
                        "{}: {}",
                        panel.title,
                        alias
                    );
                }
            }
            assert!(
                !panel
                    .ignored
                    .iter()
                    .any(|s| s == "transformation calculateField")
            );
            calculations += 1;
        }
    }
    assert_eq!(heatmaps, 20);
    assert_eq!(calculations, 8);
}

#[test]
fn group_by_and_geomaps_work_with_their_real_fixtures() {
    use grafaui_model::{geomap::MapData, transform::Transform};
    let context = QueryContext {
        window: TimeWindow::relative("now-6h", 1_759_400_000),
        variables: Vec::new(),
    };
    let mut groups = 0;
    let mut maps = 0;
    for path in baseline_fixtures() {
        let dashboard = Dashboard::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let source = FakeSource::new(&dashboard.title);
        for panel in &dashboard.panels {
            if panel
                .transforms
                .iter()
                .any(|t| matches!(t, Transform::GroupBy { .. }))
            {
                let frame = source.query(panel, &context);
                let queries: Vec<_> = panel.queries.iter().map(|q| q.ref_id.as_str()).collect();
                let table = transform::table(&panel.transforms, &frame.series, &queries);
                assert!(
                    !table.columns.is_empty() && !table.rows.is_empty(),
                    "{}: {}",
                    path.display(),
                    panel.title
                );
                assert!(table.rows.iter().all(|r| r.len() == table.columns.len()));
                assert!(!panel.ignored.iter().any(|s| s == "transformation groupBy"));
                if panel.title.contains("Persistent Volumes") {
                    assert!(
                        table.columns.iter().any(|s| s == "Used"),
                        "{:?}",
                        table.columns
                    );
                    assert!(
                        table.columns.iter().any(|s| s == "Total"),
                        "{:?}",
                        table.columns
                    );
                }
                groups += 1;
            }
            if let Viz::Geomap(options) = &panel.viz {
                let frame = source.query(panel, &context);
                let data = MapData::from_frame(&frame, options, &panel.field);
                assert_eq!(data.layers.len(), options.layers.len());
                for layer in &data.layers {
                    assert_eq!(layer.len(), 18, "{}: {}", path.display(), panel.title);
                    assert!(layer.iter().all(|p| p.latitude.abs() < 85.
                        && p.longitude.abs() < 180.
                        && p.value.is_finite()
                        && p.radius.is_finite()));
                    assert!(layer.iter().any(|p| p.label == "北京"));
                }
                maps += 1;
            }
        }
    }
    assert_eq!(groups, 4);
    assert_eq!(maps, 2);
}

#[test]
fn percentage_stacks_and_log_scales_work_with_their_real_fixtures() {
    use grafaui_model::{
        chart::{AxisScale, StackMode, stack},
        spec::AxisPlacement,
    };
    let context = QueryContext {
        window: TimeWindow::relative("now-6h", 1_759_400_000),
        variables: Vec::new(),
    };
    let mut percentages = 0;
    let mut logs = 0;
    for path in baseline_fixtures() {
        let dashboard = Dashboard::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let source = FakeSource::new(&dashboard.title);
        for panel in &dashboard.panels {
            let Viz::TimeSeries(options) = &panel.viz else {
                continue;
            };
            if options.stacking.mode == StackMode::Percent {
                let frame = source.query(panel, &context);
                let values: Vec<_> = frame
                    .series
                    .iter()
                    .map(|s| *s.values.last().unwrap())
                    .collect();
                let definitions: Vec<_> = frame
                    .series
                    .iter()
                    .map(|s| {
                        panel
                            .field
                            .style_with_values(&s.name, &s.values)
                            .stacking
                            .unwrap_or_else(|| options.stacking.clone())
                    })
                    .collect();
                let right: Vec<_> = frame
                    .series
                    .iter()
                    .map(|s| {
                        panel.field.for_time_series(&s.name, options).axis == AxisPlacement::Right
                    })
                    .collect();
                let (tops, _) = stack(&values, &definitions, &right);
                assert!(
                    tops.iter()
                        .zip(&definitions)
                        .any(|(v, d)| d.mode == StackMode::Percent && (*v - 100.).abs() < 1e-8),
                    "{}: {}",
                    path.display(),
                    panel.title
                );
                percentages += 1;
            }
            if let AxisScale::Log(base) = panel.field.scale {
                assert!(base > 1.);
                let frame = source.query(panel, &context);
                assert!(
                    frame
                        .series
                        .iter()
                        .flat_map(|s| &s.values)
                        .any(|v| *v > 0. && panel.field.scale.project(*v).is_finite()),
                    "{}",
                    panel.title
                );
                assert!(!panel.ignored.iter().any(|s| s == "log scale"));
                logs += 1;
            }
        }
    }
    assert_eq!(percentages, 4);
    assert_eq!(logs, 8); // Four modern panels and four legacy graph axes.
}

#[test]
fn expanded_snapshots_exercise_maps_timelines_and_business_tables() {
    let sources: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sources/expanded.json")).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    let snapshots = sources["dashboards"].as_array().unwrap();
    assert_eq!(snapshots.len(), 20);
    for snapshot in snapshots {
        assert!(
            snapshot["source_url"]
                .as_str()
                .unwrap()
                .starts_with("https://")
        );
        assert_eq!(snapshot["sha256"].as_str().unwrap().len(), 64);
        let file = root.join(snapshot["file"].as_str().unwrap());
        let dashboard = Dashboard::parse(&std::fs::read_to_string(file).unwrap()).unwrap();
        for panel in dashboard.panels {
            *kinds.entry(panel.kind).or_default() += 1;
        }
    }
    assert_eq!(kinds.get("geomap"), Some(&9));
    assert_eq!(kinds.get("state-timeline"), Some(&20));
    assert_eq!(kinds.get("status-history"), Some(&4));
    assert_eq!(kinds.get("table"), Some(&39));
}

#[test]
fn common_override_and_horizontal_bar_gaps_are_closed_in_their_fixtures() {
    use grafaui_model::spec::{AxisPlacement, FieldContext};
    for filename in [
        "7645.json",
        "10991.json",
        "11176.json",
        "17284.json",
        "grafana-table-kitchen-sink.json",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/real")
            .join(filename);
        let dashboard = Dashboard::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        for panel in &dashboard.panels {
            for note in &panel.ignored {
                assert!(
                    !matches!(
                        note.as_str(),
                        "override matcher byFrameRefID"
                            | "override matcher byType"
                            | "override property custom.hideFrom.viz"
                            | "horizontal bars"
                    ),
                    "{filename}: {note}"
                );
            }
        }
        if filename == "7645.json" {
            let panel = dashboard
                .panels
                .iter()
                .find(|p| p.title == "Memory Allocations")
                .unwrap();
            let a = panel.field.for_field(&FieldContext::new("same").query("A"));
            let b = panel.field.for_field(&FieldContext::new("same").query("B"));
            assert_eq!(b.axis, AxisPlacement::Right);
            assert_ne!(a.unit, b.unit);
        }
        if filename == "17284.json" {
            assert_eq!(
                dashboard
                    .panels
                    .iter()
                    .filter(|p| matches!(&p.viz,Viz::BarChart(o) if o.horizontal))
                    .count(),
                2
            );
        }
    }
}
