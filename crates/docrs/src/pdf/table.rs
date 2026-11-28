//! Table cells' fills and border lines. Word for Mac snaps both to its
//! 1/300-inch device grid and draws each line on one side of its grid line:
//! below it, right of it, and above the table's bottom and a page cut.

use std::cmp::Ordering;

use super::boxes::{self, DEVICE, Edge, Rect};
use crate::Result;
use crate::layout::flow::{Point, RowSlice};
use crate::layout::model::{Table, rank};
use crate::layout::strokes::Stroke;

/// Coordinates closer than this are the same grid line.
const SAME: f64 = 0.01;

fn device(value: f64) -> f64 {
    (value / DEVICE).round() * DEVICE
}

/// The fills of row `row`'s part `slice`, its top-left at `origin`.
pub(super) fn fills(
    out: &mut Vec<u8>,
    origin: Point,
    (table, row): (&Table, usize),
    slice: &RowSlice,
    page_height: f64,
) -> Result<()> {
    let (top, bottom) = (device(origin.y), device(origin.y + slice.height));
    for (cell, left) in table.rows[row].cells.iter().zip(table.cell_lefts(row)) {
        let Some(fill) = cell.fill else {
            continue;
        };
        let left = origin.x + left;
        let rect = Rect {
            left: device(left),
            top,
            right: device(left + cell.width),
            bottom,
        };
        out.extend_from_slice(b"q ");
        boxes::colour(out, fill)?;
        out.extend_from_slice(b"rg ");
        boxes::rectangle(out, rect, page_height)?;
        out.extend_from_slice(b"f Q\n");
    }
    Ok(())
}

/// A page's border lines, each maximal run once. The sides are drawn
/// first, so the horizontal lines draw the corners they cross, as in Word.
/// `strokes` as runs onto `out`, failing once `out` passes `cap` bytes, so
/// a page's border bytes are capped as they are written.
pub(super) fn borders(
    out: &mut Vec<u8>,
    strokes: Vec<Stroke>,
    page_height: f64,
    cap: Cap,
) -> Result<()> {
    let mut runs = runs(strokes);
    runs.sort_by_key(horizontal);
    for stroke in runs {
        cap.check(out.len())?;
        let (at, from, to) = span(&stroke);
        let edge = Edge {
            side: !horizontal(&stroke),
            at: device(at),
            inward: if stroke.up { -1.0 } else { 1.0 },
            span: (device(from), device(to)),
        };
        let thick = boxes::drawn(stroke.border.width);
        // A double or triple line's lines and gaps are each drawn as thick.
        let bands: Vec<(f64, f64)> = if boxes::dash(stroke.border.style).is_some() {
            vec![(0.0, thick)]
        } else {
            (0..boxes::solid_lines(stroke.border.style).len())
                .map(|line| line as f64 * 2.0 * thick)
                .map(|from| (from, from + thick))
                .collect()
        };
        boxes::draw(out, &stroke.border, edge, &bands, page_height)?;
    }
    cap.check(out.len())
}

/// The content budget left for one buffer: `max` bytes in all, `taken`
/// already counted elsewhere.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cap {
    pub(super) max: u64,
    pub(super) taken: u64,
}

impl Cap {
    fn check(self, len: usize) -> Result<()> {
        let drawn = self.taken.saturating_add(len as u64);
        if drawn > self.max {
            return Err(crate::Error::limit(
                "max_pdf_content_bytes",
                self.max,
                drawn,
            ));
        }
        Ok(())
    }
}

fn horizontal(stroke: &Stroke) -> bool {
    (stroke.from.y - stroke.to.y).abs() <= SAME
}

/// The grid line's coordinate, and the stroke's extent along it.
fn span(stroke: &Stroke) -> (f64, f64, f64) {
    if horizontal(stroke) {
        (stroke.from.y, stroke.from.x, stroke.to.x)
    } else {
        (stroke.from.x, stroke.from.y, stroke.to.y)
    }
}

/// Strokes as maximal runs: collinear strokes on the same side of their
/// line that touch join, and where two overlap with different borders
/// Word's ranking picks one.
fn runs(mut strokes: Vec<Stroke>) -> Vec<Stroke> {
    let key = |s: &Stroke| (!horizontal(s), s.up, span(s).0);
    strokes.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal));
    let mut out = Vec::new();
    let mut start = 0;
    while start < strokes.len() {
        let first = key(&strokes[start]);
        let end = start
            + strokes[start..]
                .iter()
                .map(key)
                .take_while(|k| k.0 == first.0 && k.1 == first.1 && (k.2 - first.2).abs() <= SAME)
                .count();
        out.extend(line_runs(&strokes[start..end]));
        start = end;
    }
    out
}

/// The runs of strokes on one grid line, all drawn on the same side of it.
fn line_runs(strokes: &[Stroke]) -> Vec<Stroke> {
    let (across, up) = (horizontal(&strokes[0]), strokes[0].up);
    let at = span(&strokes[0]).0;
    let mut cuts: Vec<f64> = strokes
        .iter()
        .flat_map(|s| {
            let (_, from, to) = span(s);
            [from, to]
        })
        .collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() <= SAME);
    let mut out: Vec<Stroke> = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let winner = strokes
            .iter()
            .filter(|s| {
                let (_, a, b) = span(s);
                a <= from + SAME && b >= to - SAME
            })
            .map(|s| s.border)
            .reduce(|best, next| {
                if rank(&next, &best).is_gt() {
                    next
                } else {
                    best
                }
            });
        let Some(border) = winner else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.border == border && (span(last).2 - from).abs() <= SAME => {
                *last = Stroke::on(across, at, (span(last).1, to), border, up);
            }
            _ => out.push(Stroke::on(across, at, (from, to), border, up)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::model::{Border, BorderStyle, Colour};

    fn border(width: f64) -> Border {
        Border {
            style: BorderStyle::Single,
            width,
            space: 0.0,
            colour: Colour::Auto,
        }
    }

    fn across(y: f64, from: f64, to: f64, width: f64) -> Stroke {
        Stroke::on(true, y, (from, to), border(width), false)
    }

    fn spans(strokes: &[Stroke]) -> Vec<(f64, f64, f64)> {
        strokes
            .iter()
            .map(|s| (span(s).1, span(s).2, s.border.width))
            .collect()
    }

    #[test]
    fn touching_strokes_join_and_a_row_s_bottom_meets_the_next_row_s_top_once() {
        let strokes = vec![
            across(100.0, 72.0, 172.0, 0.5),
            across(100.0, 172.0, 272.0, 0.5),
            // The next row's top: the same line again.
            across(100.0, 72.0, 272.0, 0.5),
            Stroke::on(false, 72.0, (80.0, 100.0), border(0.5), false),
            Stroke::on(false, 72.0, (100.0, 120.0), border(0.5), false),
        ];
        let runs = runs(strokes);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(spans(&runs[..1]), [(72.0, 272.0, 0.5)]);
        assert_eq!(
            (runs[1].from, runs[1].to),
            (Point { x: 72.0, y: 80.0 }, Point { x: 72.0, y: 120.0 })
        );
    }

    #[test]
    fn where_strokes_overlap_the_stronger_border_is_drawn() {
        // A repeated header's bottom over the next row's thicker top.
        let runs = runs(vec![
            across(200.0, 72.0, 272.0, 0.5),
            across(200.0, 172.0, 372.0, 1.5),
        ]);
        assert_eq!(spans(&runs), [(72.0, 172.0, 0.5), (172.0, 372.0, 1.5)]);
    }

    #[test]
    fn strokes_apart_stay_apart() {
        let runs = runs(vec![
            across(10.0, 0.0, 10.0, 0.5),
            across(10.0, 20.0, 30.0, 0.5),
        ]);
        assert_eq!(spans(&runs), [(0.0, 10.0, 0.5), (20.0, 30.0, 0.5)]);
    }

    #[test]
    fn a_tables_bottom_and_the_next_tables_top_on_one_line_are_both_drawn() {
        let bottom = Stroke::on(true, 10.0, (0.0, 30.0), border(0.5), true);
        let runs = runs(vec![across(10.0, 0.0, 30.0, 1.5), bottom]);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert!(runs.iter().any(|run| run.up && run.border.width == 0.5));
    }
}
