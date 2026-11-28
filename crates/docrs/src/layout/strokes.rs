//! The border lines a table row part draws, and a page's lines joined into
//! runs.

use std::cmp::Ordering;

use super::flow::{EPSILON, Point, RowSlice};
use super::model::{Border, Table, rank};

/// A border line on a grid line, from `from` to `to` (left to right or top
/// down), centred on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Stroke {
    pub from: Point,
    pub to: Point,
    pub border: Border,
}

impl Stroke {
    fn horizontal(&self) -> bool {
        (self.from.y - self.to.y).abs() <= EPSILON
    }

    /// The grid line's coordinate, and the stroke's extent along it.
    fn span(&self) -> (f64, f64, f64) {
        if self.horizontal() {
            (self.from.y, self.from.x, self.to.x)
        } else {
            (self.from.x, self.from.y, self.to.y)
        }
    }

    fn on(horizontal: bool, at: f64, from: f64, to: f64, border: Border) -> Stroke {
        let point = |along: f64| {
            if horizontal {
                Point { x: along, y: at }
            } else {
                Point { x: at, y: along }
            }
        };
        Stroke {
            from: point(from),
            to: point(to),
            border,
        }
    }
}

/// The borders of row `row`'s part `slice` with its top-left at `origin`:
/// the edge above it and below it (a page break cuts a row between the
/// two), and the vertical edges beside its cells.
pub(super) fn row_strokes(
    origin: Point,
    table: &Table,
    row: usize,
    slice: &RowSlice,
) -> Vec<Stroke> {
    let x = |line: usize| origin.x + table.grid[line];
    let columns = table.grid.len().saturating_sub(1);
    let mut out = Vec::new();
    let bottom = origin.y + slice.height;
    for (line, y, cut) in [
        (row, origin.y, !slice.first),
        (row + 1, bottom, !slice.last),
    ] {
        let edge = |column| {
            if cut {
                table.edges.seam(line, column)
            } else {
                table.edges.horizontal(line, column)
            }
        };
        let mut column = 0;
        while column < columns {
            let Some(border) = edge(column) else {
                column += 1;
                continue;
            };
            let start = column;
            while column < columns && edge(column) == Some(border) {
                column += 1;
            }
            out.push(Stroke::on(true, y, x(start), x(column), border));
        }
    }
    for line in 0..=columns {
        if let Some(border) = table.edges.vertical(row, line) {
            out.push(Stroke::on(false, x(line), origin.y, bottom, border));
        }
    }
    out
}

/// A page's strokes as maximal runs: collinear strokes that touch join,
/// and where two overlap with different borders Word's ranking picks one.
#[allow(dead_code, reason = "the PDF painter strokes each page's runs")]
pub(super) fn runs(mut strokes: Vec<Stroke>) -> Vec<Stroke> {
    strokes.sort_by(|a, b| {
        (!a.horizontal(), a.span().0)
            .partial_cmp(&(!b.horizontal(), b.span().0))
            .unwrap_or(Ordering::Equal)
    });
    let mut out = Vec::new();
    let mut start = 0;
    while start < strokes.len() {
        let (horizontal, at) = (strokes[start].horizontal(), strokes[start].span().0);
        let end = start
            + strokes[start..]
                .iter()
                .take_while(|s| s.horizontal() == horizontal && (s.span().0 - at).abs() <= EPSILON)
                .count();
        out.extend(line_runs(&strokes[start..end], horizontal, at));
        start = end;
    }
    out
}

/// The runs of strokes on one grid line.
fn line_runs(strokes: &[Stroke], horizontal: bool, at: f64) -> Vec<Stroke> {
    let mut cuts: Vec<f64> = strokes
        .iter()
        .flat_map(|s| {
            let (_, from, to) = s.span();
            [from, to]
        })
        .collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
    let mut out: Vec<Stroke> = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let winner = strokes
            .iter()
            .filter(|s| {
                let (_, a, b) = s.span();
                a <= from + EPSILON && b >= to - EPSILON
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
            Some(last) if last.border == border && (last.span().2 - from).abs() <= EPSILON => {
                *last = Stroke::on(horizontal, at, last.span().1, to, border);
            }
            _ => out.push(Stroke::on(horizontal, at, from, to, border)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::model::{BorderStyle, Colour};
    use super::*;

    fn border(width: f64) -> Border {
        Border {
            style: BorderStyle::Single,
            width,
            space: 0.0,
            colour: Colour::Auto,
        }
    }

    fn across(y: f64, from: f64, to: f64, width: f64) -> Stroke {
        Stroke::on(true, y, from, to, border(width))
    }

    fn spans(strokes: &[Stroke]) -> Vec<(f64, f64, f64)> {
        strokes
            .iter()
            .map(|s| (s.span().1, s.span().2, s.border.width))
            .collect()
    }

    #[test]
    fn touching_strokes_join_and_a_row_s_bottom_meets_the_next_row_s_top_once() {
        let strokes = vec![
            across(100.0, 72.0, 172.0, 0.5),
            across(100.0, 172.0, 272.0, 0.5),
            // The next row's top: the same line again.
            across(100.0, 72.0, 272.0, 0.5),
            Stroke::on(false, 72.0, 80.0, 100.0, border(0.5)),
            Stroke::on(false, 72.0, 100.0, 120.0, border(0.5)),
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
}
