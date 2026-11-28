//! The border lines a table row part draws.

use super::flow::{EPSILON, Point, RowSlice};
use super::model::{Border, Table};

/// A border line on a grid line, from `from` to `to` (left to right or top
/// down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stroke {
    pub from: Point,
    pub to: Point,
    pub border: Border,
    /// Inside the row above the line: the table's bottom, and a page cut's
    /// (rows are tall enough for the border below their last one).
    pub up: bool,
}

impl Stroke {
    /// On the horizontal (else vertical) grid line at `at`, from `from` to
    /// `to` along it.
    pub(crate) fn on(
        horizontal: bool,
        at: f64,
        (from, to): (f64, f64),
        border: Border,
        up: bool,
    ) -> Stroke {
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
            up,
        }
    }
}

/// The borders of row `row`'s part `slice` with its top-left at `origin`:
/// the edge above it and below it (where a page break cuts the row, the
/// table's top and bottom), and the vertical edges beside its cells.
pub(crate) fn row_strokes(
    origin: Point,
    table: &Table,
    row: usize,
    slice: &RowSlice,
) -> Vec<Stroke> {
    let x = |line: usize| origin.x + table.grid[line];
    let columns = table.grid.len().saturating_sub(1);
    let mut out = Vec::new();
    let bottom = origin.y + slice.height;
    for (line, y, cut, top) in [
        (row, origin.y, !slice.first, true),
        (row + 1, bottom, !slice.last, false),
    ] {
        let up = !top && (cut || line == table.rows.len());
        let edge = |column| {
            if cut {
                table.edges.cut(row, column, top)
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
            out.push(Stroke::on(true, y, (x(start), x(column)), border, up));
        }
    }
    // A part with no height has no sides; nor would they read as vertical.
    for line in (0..=columns).filter(|_| slice.height > EPSILON) {
        if let Some(border) = table.edges.vertical(row, line) {
            out.push(Stroke::on(
                false,
                x(line),
                (origin.y, bottom),
                border,
                false,
            ));
        }
    }
    out
}
