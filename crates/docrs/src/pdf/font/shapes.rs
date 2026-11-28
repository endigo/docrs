//! The built-in symbol font: a Type 3 font with the shapes of the
//! converter's bullets and ballot boxes, placed and sized as Word's fonts
//! draw them, and a box for any other symbol nothing else draws.

use super::to_unicode;
use crate::pdf::writer::{Dict, Object, Ref, Writer, name, write_number};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Bullet,
    Square,
    Ring,
    Ballot,
    BallotChecked,
    BallotCrossed,
    Unknown,
}

impl Shape {
    /// The converter's shape for `meaning`: its list bullets (Symbol's •,
    /// Wingdings' ▪, Courier New's o) and its task-list boxes.
    pub(crate) fn of(meaning: char) -> Option<Shape> {
        Some(match meaning {
            '\u{2022}' => Shape::Bullet,
            '\u{25AA}' => Shape::Square,
            'o' => Shape::Ring,
            '\u{2610}' => Shape::Ballot,
            '\u{2611}' => Shape::BallotChecked,
            '\u{2612}' => Shape::BallotCrossed,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Shape::Bullet => "bullet",
            Shape::Square => "square",
            Shape::Ring => "ring",
            Shape::Ballot => "ballot",
            Shape::BallotChecked => "ballotcheck",
            Shape::BallotCrossed => "ballotx",
            Shape::Unknown => "unknown",
        }
    }

    /// The advance in thousandths of an em.
    pub(crate) fn width(self) -> f64 {
        match self {
            Shape::Bullet => 460.0,
            Shape::Square => 457.5,
            Shape::Ring => 600.0,
            Shape::Ballot | Shape::BallotChecked | Shape::BallotCrossed => 1000.0,
            Shape::Unknown => 600.0,
        }
    }

    /// The glyph's bounding box and outline in thousandths of an em, from
    /// the glyphs of Word for Mac's fonts: Symbol U+F0B7, Wingdings U+F0A7,
    /// Courier New `o`, MS Gothic U+2610-2612.
    fn outline(self) -> ([f64; 4], Vec<Path>) {
        let ballot = |outer: [f64; 4], inner: [f64; 4]| vec![Path::Rect(outer), Path::Rect(inner)];
        match self {
            Shape::Bullet => (
                [52.0, 103.0, 409.0, 460.0],
                vec![Path::Ellipse([52.0, 103.0, 409.0, 460.0])],
            ),
            Shape::Square => (
                [84.5, 217.0, 373.5, 506.0],
                vec![Path::Rect([84.5, 217.0, 373.5, 506.0])],
            ),
            Shape::Ring => (
                [71.0, -16.0, 529.0, 438.0],
                vec![
                    Path::Ellipse([71.0, -16.0, 529.0, 438.0]),
                    Path::Ellipse([112.0, 25.0, 488.0, 396.0]),
                ],
            ),
            Shape::Ballot => (
                [133.0, 0.0, 871.0, 734.0],
                ballot([133.0, 0.0, 871.0, 734.0], [172.0, 47.0, 828.0, 688.0]),
            ),
            Shape::BallotChecked => {
                let mut paths = ballot([133.0, -8.0, 871.0, 727.0], [172.0, 39.0, 828.0, 680.0]);
                paths.push(Path::Stroke(
                    56.0,
                    vec![(200.0, 350.0), (330.0, 190.0), (900.0, 600.0)],
                ));
                ([78.0, -8.0, 930.0, 727.0], paths)
            }
            Shape::BallotCrossed => {
                let mut paths = ballot([133.0, -12.0, 867.0, 723.0], [172.0, 35.0, 824.0, 676.0]);
                paths.push(Path::Stroke(44.0, vec![(218.0, 70.0), (786.0, 637.0)]));
                paths.push(Path::Stroke(44.0, vec![(218.0, 637.0), (786.0, 70.0)]));
                ([133.0, -12.0, 867.0, 723.0], paths)
            }
            Shape::Unknown => (
                [75.0, 0.0, 525.0, 700.0],
                ballot([75.0, 0.0, 525.0, 700.0], [125.0, 50.0, 475.0, 650.0]),
            ),
        }
    }

    /// The glyph procedure: `d1`, then its closed paths filled even-odd
    /// and its strokes.
    fn procedure(self) -> Result<Vec<u8>> {
        let (bbox, paths) = self.outline();
        let mut out = Vec::new();
        for value in [self.width(), 0.0].into_iter().chain(bbox) {
            write_number(&mut out, value)?;
            out.push(b' ');
        }
        out.extend_from_slice(b"d1\n");
        let (fills, strokes): (Vec<&Path>, Vec<&Path>) = paths
            .iter()
            .partition(|path| !matches!(path, Path::Stroke(..)));
        for path in &fills {
            path.write(&mut out)?;
        }
        if !fills.is_empty() {
            out.extend_from_slice(b"f*\n");
        }
        for path in strokes {
            path.write(&mut out)?;
        }
        Ok(out)
    }
}

enum Path {
    /// `[x0, y0, x1, y1]`.
    Rect([f64; 4]),
    /// Inscribed in `[x0, y0, x1, y1]`.
    Ellipse([f64; 4]),
    /// A polyline of a width, with round joins.
    Stroke(f64, Vec<(f64, f64)>),
}

/// A quarter ellipse's control points lie this far along its radii.
const KAPPA: f64 = 0.552_284_75;

impl Path {
    fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut ops: Vec<(Vec<f64>, &str)> = Vec::new();
        match self {
            Path::Rect([x0, y0, x1, y1]) => ops.push((vec![*x0, *y0, x1 - x0, y1 - y0], "re")),
            Path::Ellipse([x0, y0, x1, y1]) => {
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let (rx, ry) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
                let (kx, ky) = (rx * KAPPA, ry * KAPPA);
                ops.push((vec![cx + rx, cy], "m"));
                for segment in [
                    [rx, ky, kx, ry, 0.0, ry],
                    [-kx, ry, -rx, ky, -rx, 0.0],
                    [-rx, -ky, -kx, -ry, 0.0, -ry],
                    [kx, -ry, rx, -ky, rx, 0.0],
                ] {
                    let point =
                        |at: usize| segment[at] + if at.is_multiple_of(2) { cx } else { cy };
                    ops.push(((0..6).map(point).collect(), "c"));
                }
                ops.push((vec![], "h"));
            }
            Path::Stroke(width, points) => {
                ops.push((vec![*width], "w"));
                ops.push((vec![1.0], "j"));
                for (at, (x, y)) in points.iter().enumerate() {
                    ops.push((vec![*x, *y], if at == 0 { "m" } else { "l" }));
                }
                ops.push((vec![], "S"));
            }
        }
        for (operands, operator) in ops {
            for value in operands {
                write_number(out, value)?;
                out.push(b' ');
            }
            out.extend_from_slice(operator.as_bytes());
            out.push(b'\n');
        }
        Ok(())
    }
}

/// The codes drawn with the built-in font: each shape and the text it
/// extracts as gets one, from 1.
#[derive(Default)]
pub(crate) struct Shapes {
    codes: Vec<(Shape, String)>,
}

impl Shapes {
    pub(crate) fn code(&mut self, shape: Shape, text: &str) -> Result<u8> {
        let at = match self
            .codes
            .iter()
            .position(|(s, t)| *s == shape && t == text)
        {
            Some(at) => at,
            None => {
                self.codes.push((shape, text.to_string()));
                self.codes.len() - 1
            }
        };
        u8::try_from(at + 1).map_err(|_| {
            Error::invalid_argument(
                "pdf_font_too_many_glyphs",
                "the document draws more than 255 distinct symbols with the built-in font",
            )
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    pub(crate) fn write(&self, writer: &mut Writer) -> Result<Ref> {
        let mut shapes: Vec<Shape> = Vec::new();
        for (shape, _) in &self.codes {
            if !shapes.contains(shape) {
                shapes.push(*shape);
            }
        }
        let mut procs = Dict::new();
        let mut bbox = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for shape in &shapes {
            let proc = writer.alloc()?;
            writer.stream(proc, Dict::new(), &shape.procedure()?)?;
            procs.set(shape.name(), proc);
            let (own, _) = shape.outline();
            for at in 0..4 {
                bbox[at] = if at < 2 {
                    bbox[at].min(own[at])
                } else {
                    bbox[at].max(own[at])
                };
            }
        }
        let mut differences = vec![Object::from(1_i64)];
        differences.extend(self.codes.iter().map(|(shape, _)| name(shape.name())));
        let widths: Vec<Object> = self
            .codes
            .iter()
            .map(|(shape, _)| shape.width().into())
            .collect();
        let to_unicode = writer.alloc()?;
        let texts = self
            .codes
            .iter()
            .enumerate()
            .map(|(at, (_, text))| (at as u16 + 1, text.as_str()));
        writer.stream(to_unicode, Dict::new(), &to_unicode::cmap(texts, 1))?;
        let font = writer.alloc()?;
        writer.dict(
            font,
            Dict::new()
                .with("Type", name("Font"))
                .with("Subtype", name("Type3"))
                .with("FontBBox", bbox.map(Object::from).to_vec())
                .with(
                    "FontMatrix",
                    [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]
                        .map(Object::from)
                        .to_vec(),
                )
                .with("CharProcs", procs)
                .with(
                    "Encoding",
                    Dict::new()
                        .with("Type", name("Encoding"))
                        .with("Differences", differences),
                )
                .with("FirstChar", 1_i64)
                .with("LastChar", self.codes.len() as i64)
                .with("Widths", widths)
                .with("ToUnicode", to_unicode)
                .with("Resources", Dict::new()),
        )?;
        Ok(font)
    }
}
