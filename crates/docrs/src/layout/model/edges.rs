//! Table borders on the grid: each cell's candidate edges, then Word's
//! conflict resolution between neighbours (MS-OI29500 §2.1.169).

use std::cmp::Ordering;

use super::colour::{Colour, Rgb};
use super::look::{Border, Edge};

/// A cell's candidate edges and where it sits on the grid.
#[derive(Debug, Clone, Copy)]
pub(super) struct GridCell {
    pub(super) column: usize,
    pub(super) span: usize,
    /// Continues the vertical merge of the cell above.
    pub(super) merged_up: bool,
    /// Top, left, bottom, right.
    pub(super) edges: [Edge; 4],
}

/// A table's borders after conflicts. Experimental.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edges {
    columns: usize,
    /// Grid lines above each row and below the last, `columns` segments
    /// each, and whether a vertical merge runs through the segment.
    horizontal: Vec<Option<Border>>,
    merged: Vec<bool>,
    /// Per row, the grid lines left of each column and right of the last.
    vertical: Vec<Option<Border>>,
}

impl Edges {
    /// Grid line `line` (0: the table's top) over grid column `column`.
    pub fn horizontal(&self, line: usize, column: usize) -> Option<Border> {
        let index = line * self.columns + column;
        if self.merged.get(index).copied().unwrap_or(false) {
            return None;
        }
        self.seam(line, column)
    }

    /// The same where a page break cuts the table: a vertical merge
    /// through the segment is closed there.
    pub fn seam(&self, line: usize, column: usize) -> Option<Border> {
        self.horizontal
            .get(line * self.columns + column)
            .copied()
            .flatten()
    }

    /// Vertical grid line `line` (0: the table's left) beside row `row`.
    pub fn vertical(&self, row: usize, line: usize) -> Option<Border> {
        self.vertical
            .get(row * (self.columns + 1) + line)
            .copied()
            .flatten()
    }

    pub(super) fn resolve(rows: &[Vec<GridCell>], columns: usize) -> Edges {
        let at = |row: usize, column: usize| {
            rows.get(row)?
                .iter()
                .find(|cell| (cell.column..cell.column + cell.span).contains(&column))
        };
        let mut horizontal = Vec::with_capacity((rows.len() + 1) * columns);
        let mut merged = Vec::with_capacity(horizontal.capacity());
        for line in 0..=rows.len() {
            for column in 0..columns {
                let above = line.checked_sub(1).and_then(|row| at(row, column));
                let below = at(line, column);
                merged.push(above.is_some() && below.is_some_and(|below| below.merged_up));
                horizontal.push(conflict(
                    above.map(|c| c.edges[2]),
                    below.map(|c| c.edges[0]),
                ));
            }
        }
        let mut vertical = Vec::with_capacity(rows.len() * (columns + 1));
        for row in 0..rows.len() {
            for line in 0..=columns {
                let left = line.checked_sub(1).and_then(|column| at(row, column));
                let right = at(row, line);
                vertical.push(match (left, right) {
                    (Some(left), Some(right)) if left.column == right.column => None,
                    (left, right) => conflict(left.map(|c| c.edges[3]), right.map(|c| c.edges[1])),
                });
            }
        }
        Edges {
            columns,
            horizontal,
            merged,
            vertical,
        }
    }
}

/// The border drawn between two candidates, the first in reading order
/// winning ties: `none` yields, `nil` hides both.
fn conflict(first: Option<Edge>, second: Option<Edge>) -> Option<Border> {
    match (first.unwrap_or(Edge::None), second.unwrap_or(Edge::None)) {
        (Edge::Nil, _) | (_, Edge::Nil) | (Edge::None, Edge::None) => None,
        (Edge::Line(only), Edge::None) | (Edge::None, Edge::Line(only)) => Some(only),
        (Edge::Line(a), Edge::Line(b)) => Some(if rank(&b, &a).is_gt() { b } else { a }),
    }
}

/// Word's ranking: the heavier border, then the earlier style, then the
/// darker colour.
pub(crate) fn rank(a: &Border, b: &Border) -> Ordering {
    weight(a)
        .cmp(&weight(b))
        .then(b.style.precedence().cmp(&a.style.precedence()))
        .then(brightness(b.colour).cmp(&brightness(a.colour)))
}

/// Width in eighths of a point times the style's number.
fn weight(border: &Border) -> u32 {
    border
        .style
        .number()
        .map_or(1, |number| (border.width * 8.0).round() as u32 * number)
}

/// Compared in turn: R + B + 2G, then B + 2G, then G.
fn brightness(colour: Colour) -> (u32, u32, u32) {
    let Rgb { r, g, b } = match colour {
        Colour::Rgb(rgb) => rgb,
        _ => Rgb { r: 0, g: 0, b: 0 },
    };
    let (r, g, b) = (u32::from(r), u32::from(g), u32::from(b));
    (r + b + 2 * g, b + 2 * g, g)
}

#[cfg(test)]
mod tests {
    use super::super::look::BorderStyle;
    use super::*;

    fn line(style: BorderStyle, eighths: f64, colour: &str) -> Edge {
        Edge::Line(Border {
            style,
            width: eighths / 8.0,
            space: 0.0,
            colour: Rgb::hex(colour).map_or(Colour::Auto, Colour::Rgb),
        })
    }

    fn winner(a: Edge, b: Edge) -> Option<(BorderStyle, f64, Colour)> {
        let both = [conflict(Some(a), Some(b)), conflict(Some(b), Some(a))];
        assert_eq!(both[0], both[1], "the order does not matter");
        both[0].map(|border| (border.style, border.width * 8.0, border.colour))
    }

    #[test]
    fn the_heavier_border_wins_then_the_earlier_style_then_the_darker_colour() {
        use BorderStyle::*;
        let red = Colour::Rgb(Rgb::hex("FF0000").unwrap());
        let cases = [
            // Weight is eighths times the style's number: 12 beats 4.
            (
                line(Single, 4.0, "000000"),
                line(Single, 12.0, "000000"),
                (Single, 12.0),
            ),
            // Double 4 weighs 12, single 8 weighs 8.
            (
                line(Double, 4.0, "000000"),
                line(Single, 8.0, "000000"),
                (Double, 4.0),
            ),
            // Equal weights (12): single comes before double.
            (
                line(Double, 4.0, "000000"),
                line(Single, 12.0, "000000"),
                (Single, 12.0),
            ),
            // Dotted weighs 1 at any width.
            (
                line(Dotted, 24.0, "000000"),
                line(Single, 2.0, "000000"),
                (Single, 2.0),
            ),
            // Equal weight and style: thick 4 and single 8 weigh 8; single first.
            (
                line(Thick, 4.0, "000000"),
                line(Single, 8.0, "000000"),
                (Single, 8.0),
            ),
        ];
        for (a, b, (style, eighths)) in cases {
            let (got_style, got_eighths, _) = winner(a, b).unwrap();
            assert_eq!((got_style, got_eighths), (style, eighths), "{a:?} {b:?}");
        }
        // Red and blue tie on R + B + 2G; red is darker on B + 2G.
        assert_eq!(
            winner(line(Single, 8.0, "0000FF"), line(Single, 8.0, "FF0000")).map(|w| w.2),
            Some(red)
        );
        assert_eq!(
            winner(line(Single, 8.0, "00FF00"), line(Single, 8.0, "FF00FF")).map(|w| w.2),
            Some(Colour::Rgb(Rgb::hex("FF00FF").unwrap())),
            "green counts twice"
        );
        assert_eq!(
            winner(
                line(Single, 8.0, "FFFFFF"),
                Edge::Line(Border {
                    colour: Colour::Auto,
                    ..match line(Single, 8.0, "000000") {
                        Edge::Line(border) => border,
                        _ => unreachable!(),
                    }
                })
            )
            .map(|w| w.2),
            Some(Colour::Auto),
            "automatic is black"
        );
    }

    #[test]
    fn none_yields_and_nil_hides() {
        let single = line(BorderStyle::Single, 4.0, "000000");
        assert!(winner(Edge::None, single).is_some());
        assert_eq!(winner(Edge::Nil, single), None);
        assert_eq!(winner(Edge::None, Edge::None), None);
        assert_eq!(conflict(None, Some(single)), conflict(Some(single), None));
    }

    #[test]
    fn spans_and_merges_remove_inner_edges() {
        let single = line(BorderStyle::Single, 4.0, "000000");
        let cell = |column, span, merged_up| GridCell {
            column,
            span,
            merged_up,
            edges: [single; 4],
        };
        // Row 0: a cell over columns 0–1, then column 2. Row 1: column 0
        // continues nothing, columns 1–2 merge with nothing; column 2
        // continues the cell above.
        let rows = [
            vec![cell(0, 2, false), cell(2, 1, false)],
            vec![cell(0, 2, false), cell(2, 1, true)],
        ];
        let edges = Edges::resolve(&rows, 3);
        assert!(edges.vertical(0, 1).is_none(), "inside the span");
        assert!(edges.vertical(0, 2).is_some());
        assert!(
            edges.horizontal(1, 2).is_none(),
            "inside the vertical merge"
        );
        assert!(
            edges.seam(1, 2).is_some(),
            "closed where a page break cuts it"
        );
        assert!(edges.horizontal(1, 0).is_some());
        assert!(
            (0..3).all(|c| edges.horizontal(0, c).is_some() && edges.horizontal(2, c).is_some())
        );
        assert!(edges.vertical(1, 0).is_some() && edges.vertical(1, 3).is_some());
    }
}
