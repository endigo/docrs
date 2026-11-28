//! Paragraph borders and shading. As in Word for Mac's PDFs, fills and
//! solid lines are filled rectangles; broken lines are stroked with a dash.

use super::writer::write_number;
use crate::Result;
use crate::layout::flow::ParaBox;
use crate::layout::model::{Border, BorderStyle, Colour, Rgb};

/// Word for Mac draws a line in whole 1/300ths of an inch, rounded down,
/// with its inner edge where the border's width puts it (measured).
pub(super) const DEVICE: f64 = 0.24;

const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

/// Points from the page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// One edge of a box: the outer edge `at` (an x for a side, else a y),
/// `inward` the sign toward the text, and the `span` it runs along.
#[derive(Clone, Copy)]
pub(super) struct Edge {
    pub side: bool,
    pub at: f64,
    pub inward: f64,
    pub span: (f64, f64),
}

impl Edge {
    /// The band `from`..`to` points in from the outer edge.
    fn band(self, from: f64, to: f64) -> Rect {
        let (a, b) = (self.at + self.inward * from, self.at + self.inward * to);
        let (low, high) = (a.min(b), a.max(b));
        if self.side {
            Rect {
                left: low,
                top: self.span.0,
                right: high,
                bottom: self.span.1,
            }
        } else {
            Rect {
                left: self.span.0,
                top: low,
                right: self.span.1,
                bottom: high,
            }
        }
    }
}

/// The fill behind `boxed`'s text, below and above its lines.
pub(super) fn fill(out: &mut Vec<u8>, boxed: &ParaBox, page_height: f64) -> Result<()> {
    let Some(fill) = boxed.fill else {
        return Ok(());
    };
    let inset = |line: Option<Border>| line.map_or(0.0, |b| b.thickness());
    let rect = Rect {
        left: fill.left,
        top: boxed.top + inset(boxed.top_line),
        right: fill.right,
        bottom: fill.bottom - inset(boxed.bottom_line),
    };
    out.extend_from_slice(b"q ");
    colour(out, fill.colour)?;
    out.extend_from_slice(b"rg ");
    rectangle(out, rect, page_height)?;
    out.extend_from_slice(b"f Q\n");
    Ok(())
}

/// `boxed`'s border lines.
pub(super) fn lines(out: &mut Vec<u8>, boxed: &ParaBox, page_height: f64) -> Result<()> {
    let across = (boxed.left, boxed.right);
    let edges = [
        (boxed.top_line, false, boxed.top, 1.0, across),
        (boxed.bottom_line, false, boxed.bottom, -1.0, across),
        (boxed.left_line, true, boxed.left, 1.0, boxed.sides),
        (boxed.right_line, true, boxed.right, -1.0, boxed.sides),
    ];
    for (line, side, at, inward, span) in edges {
        if let Some(border) = line {
            let edge = Edge {
                side,
                at,
                inward,
                span,
            };
            let thick = drawn(border.width);
            let bands: Vec<(f64, f64)> = if dash(border.style).is_some() {
                vec![(border.width - thick, border.width)]
            } else {
                solid_lines(border.style)
                    .iter()
                    .map(|end| (end * border.width - thick, end * border.width))
                    .collect()
            };
            draw(out, &border, edge, &bands, page_height)?;
        }
    }
    Ok(())
}

/// The thickness Word draws a `width` line with.
pub(super) fn drawn(width: f64) -> f64 {
    ((width / DEVICE + 1e-6).floor() * DEVICE).max(DEVICE)
}

/// Where each line of a solid style ends, in widths from the outer edge.
pub(super) fn solid_lines(style: BorderStyle) -> &'static [f64] {
    match style {
        BorderStyle::Double => &[1.0, 3.0],
        BorderStyle::Triple => &[1.0, 3.0, 5.0],
        _ => &[1.0],
    }
}

/// A broken style's dash pattern, in drawn widths (Word for Mac's PDFs).
pub(super) fn dash(style: BorderStyle) -> Option<&'static [f64]> {
    Some(match style {
        BorderStyle::Dotted => &[1.0, 1.0],
        BorderStyle::Dashed => &[4.0, 4.0],
        BorderStyle::DashSmallGap => &[4.0, 1.0],
        BorderStyle::DotDash => &[7.0, 3.0, 3.0, 3.0],
        BorderStyle::DotDotDash => &[6.0, 2.0, 2.0, 2.0, 2.0, 2.0],
        _ => return None,
    })
}

/// `border` along `edge`, its lines filling `bands` (from the outer edge);
/// a broken style is stroked along the middle of the first band.
pub(super) fn draw(
    out: &mut Vec<u8>,
    border: &Border,
    edge: Edge,
    bands: &[(f64, f64)],
    page_height: f64,
) -> Result<()> {
    let rgb = match border.colour {
        Colour::Rgb(rgb) => rgb,
        _ => BLACK,
    };
    let thick = drawn(border.width);
    out.extend_from_slice(b"q ");
    colour(out, rgb)?;
    if let (Some(pattern), Some(&(from, to))) = (dash(border.style), bands.first()) {
        let band = edge.band(from, to);
        out.extend_from_slice(b"RG ");
        numbers(out, &[thick])?;
        out.extend_from_slice(b"w [");
        let lengths: Vec<f64> = pattern.iter().map(|unit| unit * thick).collect();
        numbers(out, &lengths)?;
        out.extend_from_slice(b"] 0 d ");
        let (from, to) = if edge.side {
            let x = (band.left + band.right) / 2.0;
            ((x, band.top), (x, band.bottom))
        } else {
            let y = (band.top + band.bottom) / 2.0;
            ((band.left, y), (band.right, y))
        };
        numbers(out, &[from.0, page_height - from.1])?;
        out.extend_from_slice(b"m ");
        numbers(out, &[to.0, page_height - to.1])?;
        out.extend_from_slice(b"l S Q\n");
        return Ok(());
    }
    out.extend_from_slice(b"rg ");
    for &(from, to) in bands {
        rectangle(out, edge.band(from, to), page_height)?;
    }
    out.extend_from_slice(b"f Q\n");
    Ok(())
}

pub(super) fn rectangle(out: &mut Vec<u8>, rect: Rect, page_height: f64) -> Result<()> {
    numbers(
        out,
        &[
            rect.left,
            page_height - rect.bottom,
            rect.right - rect.left,
            rect.bottom - rect.top,
        ],
    )?;
    out.extend_from_slice(b"re ");
    Ok(())
}

pub(super) fn colour(out: &mut Vec<u8>, rgb: Rgb) -> Result<()> {
    numbers(out, &[rgb.r, rgb.g, rgb.b].map(|v| f64::from(v) / 255.0))
}

fn numbers(out: &mut Vec<u8>, values: &[f64]) -> Result<()> {
    for &value in values {
        write_number(out, value)?;
        out.push(b' ');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_drawn_in_whole_device_units_as_word_draws_them() {
        // Word for Mac's PDFs: ¾, 1, 2¼ and 3 pt lines.
        for (width, word) in [(0.75, 0.72), (1.0, 0.96), (2.25, 2.16), (3.0, 2.88)] {
            assert!((drawn(width) - word).abs() < 1e-9, "{width}");
        }
        assert_eq!(drawn(0.1), DEVICE);
    }

    #[test]
    fn broken_lines_have_words_dashes() {
        // Word for Mac's PDFs, 1 pt lines drawn 0.96 thick.
        for (style, word) in [
            (BorderStyle::Dotted, &[0.96, 0.96][..]),
            (BorderStyle::Dashed, &[3.84, 3.84]),
            (BorderStyle::DashSmallGap, &[3.84, 0.96]),
            (BorderStyle::DotDash, &[6.72, 2.88, 2.88, 2.88]),
            (
                BorderStyle::DotDotDash,
                &[5.76, 1.92, 1.92, 1.92, 1.92, 1.92],
            ),
        ] {
            let ours: Vec<f64> = dash(style)
                .unwrap()
                .iter()
                .map(|u| u * drawn(1.0))
                .collect();
            assert!(
                ours.iter().zip(word).all(|(a, b)| (a - b).abs() < 1e-9),
                "{style:?}"
            );
            assert_eq!(ours.len(), word.len());
        }
    }

    #[test]
    fn a_line_keeps_its_inner_edge_and_a_double_one_its_gap() {
        let edge = Edge {
            side: false,
            at: 100.0,
            inward: -1.0,
            span: (10.0, 20.0),
        };
        let band = edge.band(0.75 - drawn(0.75), 0.75);
        assert!((band.top - 99.25).abs() < 1e-9 && (band.bottom - 99.97).abs() < 1e-9);
        assert_eq!(solid_lines(BorderStyle::Double), [1.0, 3.0]);
    }
}
