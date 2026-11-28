//! Pictures on the page: an inline one where layout put its box, an
//! anchored one where its position puts it, behind or in front of the text.

use std::collections::HashSet;

use super::image::{self, ImageResources};
use super::preflight::unsupported_image;
use super::writer::{Dict, Writer, write_name, write_number};
use super::{PdfLimits, Tally};
use crate::layout::model::{Anchor, Object, ObjectKind, Page, Picture, Place, Position, Relative};
use crate::txn::Txn;
use crate::{Error, Result};

/// A rectangle in points from the page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What an anchor on a line is placed against.
pub(super) struct Spot<'a> {
    pub page: &'a Page,
    /// The page shows an odd number: its inside margin is on the left.
    pub odd: bool,
    pub para_top: f64,
    pub line_top: f64,
    pub line_height: f64,
    /// The x of the anchor's character.
    pub character: f64,
}

/// An anchored object drawn on the open page.
struct Layer {
    furniture: bool,
    behind: bool,
    order: u32,
    content: Vec<u8>,
}

pub(super) struct Pictures {
    images: ImageResources,
    limits: PdfLimits,
    /// Parts already counted in a warning.
    warned: HashSet<String>,
    layers: Vec<Layer>,
    /// The bytes of `layers`' content.
    layered: usize,
    pub(super) tally: Tally,
}

/// A row-vector affine matrix, as `cm` takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Matrix([f64; 6]);

impl Matrix {
    fn translate(x: f64, y: f64) -> Matrix {
        Matrix([1.0, 0.0, 0.0, 1.0, x, y])
    }

    fn scale(x: f64, y: f64) -> Matrix {
        Matrix([x, 0.0, 0.0, y, 0.0, 0.0])
    }

    /// Clockwise as seen on the page; PDF's y axis points up.
    fn clockwise(degrees: f64) -> Matrix {
        let (sin, cos) = (-degrees.to_radians()).sin_cos();
        Matrix([cos, sin, -sin, cos, 0.0, 0.0])
    }

    /// `self`, then `next`.
    fn then(self, next: Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = next.0;
        Matrix([
            a * a2 + b * c2,
            a * b2 + b * d2,
            c * a2 + d * c2,
            c * b2 + d * d2,
            e * a2 + f * c2 + e2,
            e * b2 + f * d2 + f2,
        ])
    }

    fn write(self, out: &mut Vec<u8>) -> Result<()> {
        for value in self.0 {
            write_number(out, value)?;
            out.push(b' ');
        }
        out.extend_from_slice(b"cm\n");
        Ok(())
    }
}

impl Pictures {
    pub(super) fn new(limits: PdfLimits) -> Self {
        Pictures {
            images: ImageResources::default(),
            limits,
            warned: HashSet::new(),
            layers: Vec::new(),
            layered: 0,
            tally: Tally::default(),
        }
    }

    /// An anchored object, into its layer on the open page.
    pub(super) fn anchored(
        &mut self,
        writer: &mut Writer,
        txn: &Txn<'_>,
        object: &Object,
        anchor: &Anchor,
        spot: &Spot<'_>,
        furniture: bool,
    ) -> Result<()> {
        let rect = place(anchor, object, spot);
        let mut content = Vec::new();
        self.draw(writer, txn, object, rect, spot.page.height, &mut content)?;
        if !content.is_empty() {
            self.layered += content.len();
            self.layers.push(Layer {
                furniture,
                behind: anchor.behind,
                order: anchor.order,
                content,
            });
        }
        Ok(())
    }

    /// The bytes of the open page's layers.
    pub(super) fn layered(&self) -> usize {
        self.layered
    }

    /// The page's content stream: the headers' and footers' layer under
    /// the body's, each its behind-text objects, its text, then its
    /// in-front objects, by `relativeHeight`.
    pub(super) fn page_stream(&mut self, furniture: &[u8], body: &[u8]) -> Vec<u8> {
        let mut layers = std::mem::take(&mut self.layers);
        self.layered = 0;
        layers.sort_by_key(|layer| layer.order);
        let mut stream = Vec::new();
        for (in_furniture, text) in [(true, furniture), (false, body)] {
            for behind in [true, false] {
                if !behind {
                    stream.extend_from_slice(text);
                }
                for layer in layers
                    .iter()
                    .filter(|l| l.furniture == in_furniture && l.behind == behind)
                {
                    stream.extend_from_slice(&layer.content);
                }
            }
        }
        stream
    }

    /// The `/XObject` resources, when there are images.
    pub(super) fn resources(&self) -> Option<Dict> {
        let dict = self.images.resources();
        (!dict.is_empty()).then_some(dict)
    }

    /// `object` at `rect` into `out`, on a page `page_height` tall.
    pub(super) fn draw(
        &mut self,
        writer: &mut Writer,
        txn: &Txn<'_>,
        object: &Object,
        rect: Rect,
        page_height: f64,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let ObjectKind::Picture(picture) = &object.kind else {
            return Ok(());
        };
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return Ok(());
        }
        let frame = frame(picture, rect, page_height);
        match self.xobject(writer, txn, picture)? {
            Some(name) => drawn(out, frame, picture, rect, &name),
            None => placeholder(out, frame, rect),
        }
    }

    /// The XObject name of the picture's part, written the first time it
    /// is used; `None` when it cannot be drawn.
    fn xobject(
        &mut self,
        writer: &mut Writer,
        txn: &Txn<'_>,
        picture: &Picture,
    ) -> Result<Option<String>> {
        let Some(part) = &picture.part else {
            return Ok(None);
        };
        let Some(data) = txn.binary(part) else {
            return Ok(None);
        };
        match self.images.add(writer, part, &data, &self.limits) {
            Ok(placed) => {
                let (name, warnings) = (placed.name.clone(), placed.warnings.clone());
                if self.warned.insert(part.clone()) {
                    for code in warnings {
                        self.tally.note(code, "image", None, None);
                    }
                }
                Ok(Some(name))
            }
            Err(err) if undrawable(&err) => {
                let content_type = txn.content_type(part).unwrap_or_default();
                // Preflight already counts the formats it knows by type.
                if unsupported_image(&content_type, part).is_none()
                    && self.warned.insert(part.clone())
                {
                    let subject = if err.reason() == image::UNSUPPORTED {
                        "image"
                    } else {
                        "unreadable image"
                    };
                    self.tally.note(image::UNSUPPORTED, subject, None, None);
                }
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }
}

/// An inline object's extent in its box, whose bottom-left corner is at
/// `x`, `baseline`.
pub(super) fn inline_rect(object: &Object, x: f64, baseline: f64) -> Rect {
    let [left, _, _, bottom] = object.effect;
    Rect {
        x: x + left,
        y: baseline - bottom - object.height,
        width: object.width,
        height: object.height,
    }
}

/// A picture the PDF shows as a placeholder; every other error refuses.
fn undrawable(err: &Error) -> bool {
    matches!(err.reason(), image::UNSUPPORTED | image::MALFORMED)
}

/// The matrix from the picture's own space, `rect`'s size with its
/// origin at the bottom-left, to the page: flipped, then rotated
/// clockwise about the centre.
fn frame(picture: &Picture, rect: Rect, page_height: f64) -> Matrix {
    let centre = (
        rect.x + rect.width / 2.0,
        page_height - rect.y - rect.height / 2.0,
    );
    let flip = |on: bool| if on { -1.0 } else { 1.0 };
    Matrix::translate(-rect.width / 2.0, -rect.height / 2.0)
        .then(Matrix::scale(flip(picture.flip_h), flip(picture.flip_v)))
        .then(Matrix::clockwise(picture.rotation))
        .then(Matrix::translate(centre.0, centre.1))
}

/// A crop side's bounds: all of the image cut off, or ten times its size
/// added.
const CROP_SIDE: (f64, f64) = (-10.0, 1.0);

/// The least share of the image a crop shows and still draws: one in ten
/// thousand. Less, and the image stretched over the picture would be
/// beyond what a content stream writes.
const MIN_SHOWN: f64 = 1e-4;

/// The image stretched over the uncropped picture and clipped to `rect`.
fn drawn(
    out: &mut Vec<u8>,
    frame: Matrix,
    picture: &Picture,
    rect: Rect,
    name: &str,
) -> Result<()> {
    let [left, top, right, bottom] = picture
        .crop
        .map(|side| side.clamp(CROP_SIDE.0, CROP_SIDE.1));
    let (shown_x, shown_y) = (1.0 - left - right, 1.0 - top - bottom);
    if shown_x < MIN_SHOWN || shown_y < MIN_SHOWN {
        return Ok(());
    }
    let (width, height) = (rect.width / shown_x, rect.height / shown_y);
    out.extend_from_slice(b"q\n");
    frame.write(out)?;
    if picture.crop.iter().any(|&side| side > 0.0) {
        rectangle(out, rect)?;
        out.extend_from_slice(b"W n\n");
    }
    Matrix([width, 0.0, 0.0, height, -left * width, -bottom * height]).write(out)?;
    write_name(out, name)?;
    out.extend_from_slice(b" Do\nQ\n");
    Ok(())
}

/// A grey box with a cross, where a picture cannot be drawn.
fn placeholder(out: &mut Vec<u8>, frame: Matrix, rect: Rect) -> Result<()> {
    out.extend_from_slice(b"q\n");
    frame.write(out)?;
    out.extend_from_slice(b"0.9 g 0.6 G 0.5 w\n");
    rectangle(out, rect)?;
    out.extend_from_slice(b"B\n0 0 m ");
    for value in [rect.width, rect.height] {
        write_number(out, value)?;
        out.push(b' ');
    }
    out.extend_from_slice(b"l ");
    write_number(out, rect.width)?;
    out.extend_from_slice(b" 0 m 0 ");
    write_number(out, rect.height)?;
    out.extend_from_slice(b" l S\nQ\n");
    Ok(())
}

/// `0 0 width height re`.
fn rectangle(out: &mut Vec<u8>, rect: Rect) -> Result<()> {
    out.extend_from_slice(b"0 0 ");
    for value in [rect.width, rect.height] {
        write_number(out, value)?;
        out.push(b' ');
    }
    out.extend_from_slice(b"re\n");
    Ok(())
}

/// Where an anchor's position puts its extent.
fn place(anchor: &Anchor, object: &Object, spot: &Spot<'_>) -> Rect {
    let odd = spot.odd;
    let (x, width) = horizontal(anchor.x.from, odd, spot);
    let (y, height) = vertical(anchor.y.from, odd, spot);
    Rect {
        x: offset(anchor.x, x, width, object.width, odd),
        y: offset(anchor.y, y, height, object.height, odd),
        width: object.width,
        height: object.height,
    }
}

/// The frame's start and size across the page.
fn horizontal(from: Relative, odd: bool, spot: &Spot<'_>) -> (f64, f64) {
    let page = spot.page;
    let width = page.width;
    let left = page.left + page.gutter;
    let right = width - page.right;
    let left_margin = (0.0, left);
    let right_margin = (right, width - right);
    match from {
        Relative::Page => (0.0, width),
        Relative::Character => (spot.character, 0.0),
        Relative::LeftMargin => left_margin,
        Relative::RightMargin => right_margin,
        Relative::InsideMargin if odd => left_margin,
        Relative::OutsideMargin if !odd => left_margin,
        Relative::InsideMargin | Relative::OutsideMargin => right_margin,
        _ => (left, page.text_width),
    }
}

/// The frame's start and size down the page.
fn vertical(from: Relative, odd: bool, spot: &Spot<'_>) -> (f64, f64) {
    let page = spot.page;
    let height = page.height;
    let top_margin = (0.0, page.top);
    let bottom_margin = (height - page.bottom, page.bottom);
    match from {
        Relative::Page => (0.0, height),
        Relative::Paragraph => (spot.para_top, 0.0),
        Relative::Line => (spot.line_top, spot.line_height),
        Relative::TopMargin => top_margin,
        Relative::BottomMargin => bottom_margin,
        Relative::InsideMargin if odd => top_margin,
        Relative::OutsideMargin if !odd => top_margin,
        Relative::InsideMargin | Relative::OutsideMargin => bottom_margin,
        _ => (page.top, height - page.top - page.bottom),
    }
}

/// The object's start in a frame from `start`, `size` long.
fn offset(position: Position, start: f64, size: f64, extent: f64, odd: bool) -> f64 {
    let end = start + size - extent;
    match position.place {
        Place::Offset(points) => start + points,
        Place::Share(share) => start + share * size,
        Place::Start => start,
        Place::Center => start + (size - extent) / 2.0,
        Place::End => end,
        Place::Inside if odd => start,
        Place::Outside if !odd => start,
        Place::Inside | Place::Outside => end,
    }
}
