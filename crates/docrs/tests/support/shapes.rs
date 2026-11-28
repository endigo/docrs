//! The filled rectangles and stroked lines a PDF's pages draw, read back
//! by the test reader.

use crate::reader::{Pdf, Value, tokenize_content};

/// A filled rectangle or a stroked line as drawn, in points from the
/// page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub colour: [u8; 3],
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub stroked: bool,
    /// Drawn after the page's first text.
    pub over_text: bool,
    /// A stroke's dash: its first length and its period.
    pub dash: Option<(f64, f64)>,
}

impl Shape {
    pub fn horizontal(&self) -> bool {
        self.right - self.left > self.bottom - self.top
    }

    pub fn thickness(&self) -> f64 {
        (self.right - self.left).min(self.bottom - self.top)
    }
}

/// Each page's shapes, in drawing order.
pub fn shapes(bytes: &[u8]) -> Vec<Vec<Shape>> {
    let pdf = Pdf::parse(bytes).unwrap();
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    pdf.pages()
        .unwrap()
        .into_iter()
        .zip(heights)
        .map(|(page, height)| {
            let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
            let mut out = Vec::new();
            let (mut fill, mut stroke, mut width, mut dash) = ([0; 3], [0; 3], 1.0, None);
            let mut saved = Vec::new();
            let (mut rects, mut path) = (Vec::new(), Vec::new());
            let mut over_text = false;
            for op in tokenize_content(&content).unwrap() {
                let n = |at: usize| op.operands[at].number().unwrap();
                let rgb = || [0, 1, 2].map(|at| (n(at) * 255.0).round() as u8);
                let shape = |colour, (left, top, right, bottom), stroked| Shape {
                    colour,
                    left,
                    top,
                    right,
                    bottom,
                    stroked,
                    over_text,
                    dash: dash.filter(|_| stroked),
                };
                match op.operator.as_str() {
                    "q" => saved.push((fill, stroke, width, dash)),
                    "Q" => (fill, stroke, width, dash) = saved.pop().unwrap(),
                    "d" => {
                        let lengths: Vec<f64> = match &op.operands[0] {
                            Value::Array(items) => items.iter().filter_map(Value::number).collect(),
                            _ => Vec::new(),
                        };
                        dash = lengths.first().map(|first| (*first, lengths.iter().sum()));
                    }
                    "rg" => fill = rgb(),
                    "RG" => stroke = rgb(),
                    "w" => width = n(0),
                    "BT" => over_text = true,
                    "re" => rects.push((n(0), height - n(1) - n(3), n(0) + n(2), height - n(1))),
                    "f" => out.extend(rects.drain(..).map(|r| shape(fill, r, false))),
                    "m" | "l" => path.push((n(0), height - n(1))),
                    "S" => {
                        let [(x0, y0), (x1, y1)] = [path[0], path[path.len() - 1]];
                        let half = width / 2.0;
                        let bounds = if y0 == y1 {
                            (x0.min(x1), y0 - half, x0.max(x1), y0 + half)
                        } else {
                            (x0 - half, y0.min(y1), x0 + half, y0.max(y1))
                        };
                        out.push(shape(stroke, bounds, true));
                        path.clear();
                    }
                    _ => {}
                }
            }
            out
        })
        .collect()
}

pub fn coloured(shapes: &[Shape], hex: u32) -> Vec<Shape> {
    let colour = [16, 8, 0].map(|shift| (hex >> shift) as u8);
    shapes
        .iter()
        .filter(|s| s.colour == colour)
        .copied()
        .collect()
}
