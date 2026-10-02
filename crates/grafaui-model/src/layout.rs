//! Places panels on Grafana's grid: 24 columns, rows of 30 px with 8 px
//! gaps, split into sections at each row panel.
//!
//! A section's panels keep their columns, and their `y` is counted from the
//! section's top, so collapsing a row hides its section without moving the
//! panels of the others.

use crate::schema::{GridPos, LegacyRow, RawPanel};
use crate::spec::{self, PanelSpec};

pub const COLUMNS: u32 = 24;
/// Height of one grid row, in pixels at the default text size.
pub const ROW_HEIGHT: f32 = 30.;
/// Gap between panels, in pixels at the default text size.
pub const GAP: f32 = 8.;

#[derive(Clone, Debug)]
pub struct Section {
    /// `None` for the panels above the first row.
    pub row: Option<RowHeader>,
    pub panels: Vec<Placed>,
    /// Grid rows the section spans when expanded.
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct RowHeader {
    pub title: String,
    pub collapsed: bool,
    pub repeat: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Index into [`crate::Dashboard::panels`].
    pub key: usize,
    pub pos: GridPos,
}

/// Splits the current flat panel list (schema 16 and later) into sections.
pub(crate) fn sections(raw: &[RawPanel], specs: &mut Vec<PanelSpec>) -> Vec<Section> {
    let mut sections = vec![Builder::new(None, 0)];
    for panel in raw {
        if panel.kind == "row" {
            let top = panel.grid_pos.map_or(0, |p| p.y + 1);
            let mut section = Builder::new(
                Some(RowHeader {
                    title: panel.title.clone(),
                    collapsed: panel.collapsed,
                    repeat: panel.repeat.clone().filter(|r| !r.is_empty()),
                }),
                top,
            );
            // A folded row carries its panels inside it.
            for child in &panel.panels {
                section.push(child, specs);
            }
            sections.push(section);
        } else {
            sections
                .last_mut()
                .expect("there is always a section")
                .push(panel, specs);
        }
    }
    finish(sections)
}

/// Lays out dashboards from before schema 16, whose rows hold panels sized
/// by `span` (1–12) and a pixel `height` per row.
pub(crate) fn legacy_sections(rows: &[LegacyRow], specs: &mut Vec<PanelSpec>) -> Vec<Section> {
    let titled = rows.len() > 1;
    let sections = rows
        .iter()
        .map(|row| {
            let header = (titled || row.show_title).then(|| RowHeader {
                title: row.title.clone(),
                collapsed: row.collapse,
                repeat: None,
            });
            let height = pixels(&row.height).map_or(8, grid_rows);
            let mut section = Builder::new(header, 0);
            let (mut x, mut y) = (0, 0);
            for panel in &row.panels {
                let w = panel
                    .span
                    .map_or(12, |span| (span * 2.).round() as u32)
                    .clamp(1, COLUMNS);
                let h = pixels(&panel.height).map_or(height, grid_rows);
                if x + w > COLUMNS {
                    x = 0;
                    y += height;
                }
                let mut panel = panel.clone();
                panel.grid_pos = Some(GridPos { x, y, w, h });
                section.push(&panel, specs);
                x += w;
            }
            section
        })
        .collect();
    finish(sections)
}

fn pixels(value: &serde_json::Value) -> Option<f32> {
    value
        .as_f64()
        .map(|v| v as f32)
        .or_else(|| value.as_str()?.trim_end_matches("px").trim().parse().ok())
}

/// Grid rows for a legacy height in pixels. At least two: old dashboards
/// sometimes set `"1px"`, which left room for the title only.
fn grid_rows(pixels: f32) -> u32 {
    ((pixels + GAP) / (ROW_HEIGHT + GAP)).ceil().max(2.) as u32
}

struct Builder {
    row: Option<RowHeader>,
    top: u32,
    panels: Vec<Placed>,
    /// Where a panel without `gridPos` goes next.
    cursor: (u32, u32),
}

impl Builder {
    fn new(row: Option<RowHeader>, top: u32) -> Self {
        Self {
            row,
            top,
            panels: Vec::new(),
            cursor: (0, 0),
        }
    }

    fn push(&mut self, raw: &RawPanel, specs: &mut Vec<PanelSpec>) {
        let key = specs.len();
        specs.push(spec::panel(raw, key));
        let pos = match raw.grid_pos {
            Some(pos) => {
                let x = pos.x.min(COLUMNS - 1);
                GridPos {
                    x,
                    y: pos.y.saturating_sub(self.top),
                    w: pos.w.clamp(1, COLUMNS - x),
                    h: pos.h.max(1),
                }
            }
            None => self.flow(),
        };
        self.panels.push(Placed { key, pos });
    }

    fn flow(&mut self) -> GridPos {
        let bottom = self
            .panels
            .iter()
            .map(|p| p.pos.y + p.pos.h)
            .max()
            .unwrap_or(0);
        let (mut x, mut y) = self.cursor;
        if x + 12 > COLUMNS {
            x = 0;
            y = bottom;
        }
        self.cursor = (x + 12, y);
        GridPos { x, y, w: 12, h: 8 }
    }
}

fn finish(sections: Vec<Builder>) -> Vec<Section> {
    sections
        .into_iter()
        .filter(|s| s.row.is_some() || !s.panels.is_empty())
        .map(|mut s| {
            s.panels.sort_by_key(|p| (p.pos.y, p.pos.x));
            compact(&mut s.panels);
            s.panels.sort_by_key(|p| (p.pos.y, p.pos.x));
            let height = s
                .panels
                .iter()
                .map(|p| p.pos.y + p.pos.h)
                .max()
                .unwrap_or(0);
            Section {
                row: s.row,
                panels: s.panels,
                height,
            }
        })
        .collect()
}

/// Replaces each repeated panel with a copy per value of its variable
/// (`values` gives them, empty for an unknown variable): side by side,
/// wrapping at `maxPerRow`, or stacked. The panels below move down.
pub(crate) fn expand_repeats(
    sections: &mut [Section],
    specs: &mut Vec<PanelSpec>,
    values: impl Fn(&str) -> Vec<String>,
) {
    for section in sections {
        let mut i = 0;
        while i < section.panels.len() {
            let placed = section.panels[i];
            let Some(repeat) = specs[placed.key].repeat.clone() else {
                i += 1;
                continue;
            };
            let values = values(&repeat.variable);
            if values.is_empty() {
                i += 1;
                continue;
            }
            let count = values.len() as u32;
            let pos = placed.pos;
            let (per_row, w) = if repeat.horizontal {
                let per_row = repeat.max_per_row.unwrap_or(count).clamp(1, count);
                (per_row, (COLUMNS / per_row).max(1))
            } else {
                (1, pos.w)
            };
            let rows = count.div_ceil(per_row);
            // Make room below.
            let extra = (rows - 1) * pos.h;
            for other in &mut section.panels {
                if other.pos.y >= pos.y + pos.h {
                    other.pos.y += extra;
                }
            }
            let mut copies = Vec::new();
            for (n, value) in values.into_iter().enumerate() {
                let n = n as u32;
                let key = if n == 0 {
                    placed.key
                } else {
                    specs.push(specs[placed.key].clone());
                    specs.len() - 1
                };
                let spec = &mut specs[key];
                spec.key = key;
                spec.repeat = None;
                spec.scoped = vec![(repeat.variable.clone(), value)];
                let (x, y) = if repeat.horizontal {
                    ((n % per_row) * w, pos.y + n / per_row * pos.h)
                } else {
                    (pos.x, pos.y + n * pos.h)
                };
                copies.push(Placed {
                    key,
                    pos: GridPos { x, y, w, h: pos.h },
                });
            }
            let added = copies.len();
            section.panels.splice(i..=i, copies);
            i += added;
        }
        section.panels.sort_by_key(|p| (p.pos.y, p.pos.x));
        compact(&mut section.panels);
        section.panels.sort_by_key(|p| (p.pos.y, p.pos.x));
        section.height = section
            .panels
            .iter()
            .map(|p| p.pos.y + p.pos.h)
            .max()
            .unwrap_or(0);
    }
}

/// Moves each panel up until it meets one above it, as Grafana's grid does.
/// Panels in a folded row keep the positions they had when it was last
/// open, often far down the page; this closes those gaps too.
///
/// `panels` must be sorted by `(y, x)`.
fn compact(panels: &mut [Placed]) {
    for i in 0..panels.len() {
        let pos = panels[i].pos;
        let overlaps = |other: &GridPos| other.x < pos.x + pos.w && pos.x < other.x + other.w;
        panels[i].pos.y = panels[..i]
            .iter()
            .map(|p| p.pos)
            .filter(|other| overlaps(other))
            .map(|other| other.y + other.h)
            .max()
            .unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(json: &str) -> (Vec<Section>, Vec<PanelSpec>) {
        let raw: Vec<RawPanel> = serde_json::from_str(json).unwrap();
        let mut specs = Vec::new();
        (sections(&raw, &mut specs), specs)
    }

    #[test]
    fn splits_at_rows_and_counts_from_each_top() {
        let (sections, specs) = layout(
            r#"[
              {"type":"stat","gridPos":{"x":0,"y":0,"w":6,"h":4}},
              {"type":"row","title":"Open","gridPos":{"x":0,"y":4,"w":24,"h":1}},
              {"type":"graph","gridPos":{"x":12,"y":5,"w":12,"h":8}},
              {"type":"row","title":"Folded","collapsed":true,"gridPos":{"x":0,"y":13,"w":24,"h":1},
               "panels":[{"type":"gauge","gridPos":{"x":0,"y":14,"w":8,"h":6}}]}
            ]"#,
        );
        assert_eq!(specs.len(), 3);
        assert_eq!(sections.len(), 3);
        assert!(sections[0].row.is_none());
        assert_eq!(
            sections[1].panels[0].pos,
            GridPos {
                x: 12,
                y: 0,
                w: 12,
                h: 8
            }
        );
        assert_eq!(sections[1].height, 8);
        let folded = &sections[2];
        assert!(folded.row.as_ref().unwrap().collapsed);
        assert_eq!(folded.panels[0].pos.y, 0);
    }

    #[test]
    fn clamps_and_flows_bad_positions() {
        let (sections, _) = layout(
            r#"[{"type":"stat","gridPos":{"x":30,"y":0,"w":12,"h":0}},{"type":"stat"},{"type":"stat"},{"type":"stat"}]"#,
        );
        let positions: Vec<_> = sections[0].panels.iter().map(|p| p.pos).collect();
        assert_eq!(
            positions[0],
            GridPos {
                x: 0,
                y: 0,
                w: 12,
                h: 8
            }
        );
        assert_eq!(
            positions[1],
            GridPos {
                x: 12,
                y: 0,
                w: 12,
                h: 8
            }
        );
        // Clamped onto the second panel, so it moves below it.
        assert_eq!(
            positions[2],
            GridPos {
                x: 0,
                y: 8,
                w: 12,
                h: 8
            }
        );
        assert_eq!(
            positions[3],
            GridPos {
                x: 23,
                y: 8,
                w: 1,
                h: 1
            }
        );
    }

    #[test]
    fn lays_out_legacy_rows_by_span() {
        let rows: Vec<LegacyRow> = serde_json::from_str(
            r#"[{"title":"A","height":"250px","panels":[{"type":"graph","span":6},{"type":"graph","span":8},{"type":"text","span":4}]},
                {"title":"B","panels":[{"type":"singlestat","span":3}]}]"#,
        )
        .unwrap();
        let mut specs = Vec::new();
        let sections = legacy_sections(&rows, &mut specs);
        assert_eq!(sections.len(), 2);
        let a: Vec<_> = sections[0].panels.iter().map(|p| p.pos).collect();
        assert_eq!(
            a[0],
            GridPos {
                x: 0,
                y: 0,
                w: 12,
                h: 7
            }
        );
        // Nothing is above the text panel, so it rises beside the first.
        assert_eq!(
            a[1],
            GridPos {
                x: 16,
                y: 0,
                w: 8,
                h: 7
            }
        );
        assert_eq!(
            a[2],
            GridPos {
                x: 0,
                y: 7,
                w: 16,
                h: 7
            }
        );
        assert_eq!(sections[1].row.as_ref().unwrap().title, "B");
    }

    #[test]
    fn folded_rows_close_stale_gaps() {
        let (sections, _) = layout(
            r#"[
                {"type":"row","collapsed":true,"gridPos":{"x":0,"y":0,"w":24,"h":1},"panels":[
                    {"type":"timeseries","gridPos":{"x":0,"y":710,"w":12,"h":10}},
                    {"type":"timeseries","gridPos":{"x":12,"y":710,"w":12,"h":10}},
                    {"type":"timeseries","gridPos":{"x":0,"y":910,"w":24,"h":10}}]}]"#,
        );
        let ys: Vec<u32> = sections[0].panels.iter().map(|p| p.pos.y).collect();
        assert_eq!(ys, vec![0, 0, 10]);
        assert_eq!(sections[0].height, 20);
    }
}
