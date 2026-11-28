//! Drawings: an inline one takes its place in a line, an anchored one is
//! placed by its position and takes no part in text flow.

use std::collections::HashMap;

use super::props::{length, number};
use crate::units::EMU_PER_PT;
use crate::wml::{A, PIC, R, WP, first_choice_children};
use crate::xml::{NodeId, XmlTree};

const PICTURE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const WP14: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing";
/// Word names its watermark shapes with these prefixes.
pub(crate) const WATERMARK_PREFIXES: &[&str] =
    &["PowerPlusWaterMarkObject", "WordPictureWatermark"];

/// `a:srcRect` and `wp14:pctPos*Offset` count in thousandths of a percent.
const PERCENT_UNIT: f64 = 100_000.0;
/// `a:xfrm rot` counts in 60,000ths of a degree.
const ROTATION_UNIT: f64 = 60_000.0;

/// What a drawing draws, at its `wp:extent` in points.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub width: f64,
    pub height: f64,
    /// `wp:effectExtent` (left, top, right, bottom): an inline box's room
    /// past the extent, for effects and rotation.
    pub effect: [f64; 4],
    pub kind: ObjectKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectKind {
    Picture(Picture),
    /// A VML horizontal rule.
    Rule,
    /// A chart, SmartArt, shape or watermark, which preflight reports:
    /// blank space.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    /// The media part; `None` when the blip's relationship names none in
    /// the package (a linked picture).
    pub part: Option<String>,
    /// `a:srcRect` (left, top, right, bottom): the share of the image cut
    /// off each side; negative adds space.
    pub crop: [f64; 4],
    /// Degrees clockwise about the centre.
    pub rotation: f64,
    pub flip_h: bool,
    pub flip_v: bool,
}

/// An anchored drawing (`wp:anchor`).
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    /// Into `Model.objects`.
    pub object: u32,
    /// The pieces before it in its paragraph.
    pub piece: u32,
    pub x: Position,
    pub y: Position,
    /// `behindDoc`: under the text.
    pub behind: bool,
    /// `relativeHeight`: a higher one is drawn over a lower one.
    pub order: u32,
}

/// `wp:positionH` or `wp:positionV`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub from: Relative,
    pub place: Place,
}

/// `relativeFrom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relative {
    Page,
    Margin,
    Column,
    Character,
    Paragraph,
    Line,
    LeftMargin,
    RightMargin,
    TopMargin,
    BottomMargin,
    InsideMargin,
    OutsideMargin,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// Points from the frame's start.
    Offset(f64),
    /// A share of the frame's size from its start.
    Share(f64),
    /// Left or top.
    Start,
    Center,
    /// Right or bottom.
    End,
    Inside,
    Outside,
}

/// A `w:drawing`'s object, and where it is placed when it is anchored.
pub(super) struct Drawing {
    pub object: Object,
    pub anchor: Option<Placement>,
}

pub(super) struct Placement {
    pub x: Position,
    pub y: Position,
    pub behind: bool,
    pub order: u32,
}

/// Reads `drawing` (`w:drawing`); `parts` maps its story's relationship
/// ids to the parts they name.
pub(super) fn read(
    tree: &XmlTree,
    drawing: NodeId,
    parts: &HashMap<String, String>,
) -> Option<Drawing> {
    let frame = tree
        .child_elements(drawing)
        .find(|&n| tree.is(n, WP, "inline") || tree.is(n, WP, "anchor"))?;
    let child = |name: &str| tree.first_child_named(frame, WP, name);
    let extent = child("extent")?;
    let emu = |node: NodeId, name: &str| {
        tree.attr(node, None, name)
            .and_then(number)
            .map_or(0.0, |emu| length(emu / EMU_PER_PT as f64))
    };
    let effect =
        child("effectExtent").map_or([0.0; 4], |e| ["l", "t", "r", "b"].map(|side| emu(e, side)));
    let watermark = child("docPr")
        .and_then(|d| tree.attr(d, None, "name"))
        .is_some_and(is_watermark);
    let kind = match picture(tree, frame, parts) {
        Some(picture) if !watermark => ObjectKind::Picture(picture),
        _ => ObjectKind::Unsupported,
    };
    let object = Object {
        width: emu(extent, "cx"),
        height: emu(extent, "cy"),
        effect,
        kind,
    };
    let anchor = tree.is(frame, WP, "anchor").then(|| placement(tree, frame));
    Some(Drawing { object, anchor })
}

pub(crate) fn is_watermark(name: &str) -> bool {
    WATERMARK_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn picture(tree: &XmlTree, frame: NodeId, parts: &HashMap<String, String>) -> Option<Picture> {
    let data = tree
        .descendants(frame)
        .find(|&n| tree.is(n, A, "graphicData"))
        .filter(|&n| tree.attr(n, None, "uri") == Some(PICTURE_URI))?;
    let pic = tree.first_child_named(data, PIC, "pic")?;
    let find = |within: NodeId, ns: &str, name: &str| {
        tree.descendants(within).find(|&n| tree.is(n, ns, name))
    };
    let fill = tree.first_child_named(pic, PIC, "blipFill");
    let part = fill
        .and_then(|f| tree.first_child_named(f, A, "blip"))
        .and_then(|blip| tree.attr(blip, Some(R), "embed"))
        .and_then(|id| parts.get(id))
        .cloned();
    let crop = fill
        .and_then(|f| tree.first_child_named(f, A, "srcRect"))
        .map_or([0.0; 4], |rect| {
            ["l", "t", "r", "b"].map(|side| {
                tree.attr(rect, None, side)
                    .and_then(number)
                    .map_or(0.0, |v| v / PERCENT_UNIT)
            })
        });
    let xfrm = tree
        .first_child_named(pic, PIC, "spPr")
        .and_then(|sp| find(sp, A, "xfrm"));
    let flag = |name: &str| {
        xfrm.and_then(|x| tree.attr(x, None, name))
            .is_some_and(|v| matches!(v, "1" | "true"))
    };
    let rotation = xfrm
        .and_then(|x| tree.attr(x, None, "rot"))
        .and_then(number)
        .map_or(0.0, |rot| (rot / ROTATION_UNIT).rem_euclid(360.0));
    Some(Picture {
        part,
        crop,
        rotation,
        flip_h: flag("flipH"),
        flip_v: flag("flipV"),
    })
}

fn placement(tree: &XmlTree, anchor: NodeId) -> Placement {
    let attr = |name: &str| tree.attr(anchor, None, name);
    let on = |name: &str| attr(name).is_some_and(|v| matches!(v, "1" | "true"));
    let (x, y) = match tree.first_child_named(anchor, WP, "simplePos") {
        Some(simple) if on("simplePos") => {
            let at = |name: &str| Position {
                from: Relative::Page,
                place: Place::Offset(
                    tree.attr(simple, None, name)
                        .and_then(number)
                        .map_or(0.0, |emu| length(emu / EMU_PER_PT as f64)),
                ),
            };
            (at("x"), at("y"))
        }
        _ => (
            position(tree, anchor, "positionH", Relative::Column),
            position(tree, anchor, "positionV", Relative::Paragraph),
        ),
    };
    Placement {
        x,
        y,
        behind: on("behindDoc"),
        order: attr("relativeHeight")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    }
}

fn position(tree: &XmlTree, anchor: NodeId, name: &str, default: Relative) -> Position {
    let Some(node) = tree.first_child_named(anchor, WP, name) else {
        return Position {
            from: default,
            place: Place::Offset(0.0),
        };
    };
    let from = tree
        .attr(node, None, "relativeFrom")
        .map_or(default, |value| relative(value, default));
    let place = first_choice_children(tree, node)
        .into_iter()
        .find_map(|child| {
            let text = tree.text_content(child);
            let text = text.trim();
            match (tree.ns_uri(child), tree.local_name(child)) {
                (Some(WP), Some("posOffset")) => {
                    number(text).map(|emu| Place::Offset(length(emu / EMU_PER_PT as f64)))
                }
                (Some(WP), Some("align")) => Some(align(text)),
                (Some(WP14), Some("pctPosHOffset" | "pctPosVOffset")) => {
                    number(text).map(|share| Place::Share(share / PERCENT_UNIT))
                }
                _ => None,
            }
        })
        .unwrap_or(Place::Offset(0.0));
    Position { from, place }
}

fn relative(value: &str, default: Relative) -> Relative {
    match value {
        "page" => Relative::Page,
        "margin" => Relative::Margin,
        "column" => Relative::Column,
        "character" => Relative::Character,
        "paragraph" => Relative::Paragraph,
        "line" => Relative::Line,
        "leftMargin" => Relative::LeftMargin,
        "rightMargin" => Relative::RightMargin,
        "topMargin" => Relative::TopMargin,
        "bottomMargin" => Relative::BottomMargin,
        "insideMargin" => Relative::InsideMargin,
        "outsideMargin" => Relative::OutsideMargin,
        _ => default,
    }
}

fn align(value: &str) -> Place {
    match value {
        "center" => Place::Center,
        "right" | "bottom" => Place::End,
        "inside" => Place::Inside,
        "outside" => Place::Outside,
        _ => Place::Start,
    }
}
