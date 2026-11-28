//! What text and paragraphs look like, as read: layout measures none of it
//! beyond what `Metric` and the pieces' widths already hold.

use std::hash::{Hash, Hasher};

use super::colour::{Colour, Palette, Rgb};
use super::props::number;
use crate::wml::w_attr;
use crate::xml::{NodeId, XmlTree};

/// A run's appearance, shared by index (`look` on pieces). Experimental.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Appearance {
    pub metric: u32,
    /// Into `Model.faces`: the face of characters below U+0080, and of
    /// the rest (`w:ascii` and `w:hAnsi`).
    pub ascii: u32,
    pub hansi: u32,
    /// The drawn size in points: 2/3 of the font size for super- and
    /// subscript.
    pub size: f64,
    /// Points the baseline is raised (negative: lowered).
    pub rise: f64,
    pub colour: Colour,
    pub bold: bool,
    pub italic: bool,
    pub underline: Option<Underline>,
    pub strike: Strike,
    pub highlight: Option<Rgb>,
    pub shade: Option<Rgb>,
    /// The pieces keep their source characters; the painter applies caps.
    pub caps: Caps,
    /// Extra advance after each character, in points.
    pub spacing: f64,
    /// Horizontal scale, 1 for 100 %.
    pub scale: f64,
    /// Index into `Model.links`.
    pub link: Option<u32>,
}

impl Eq for Appearance {}

impl Appearance {
    /// The face that measures and draws `ch`.
    pub fn face(&self, ch: char) -> u32 {
        if is_ascii_slot(ch) {
            self.ascii
        } else {
            self.hansi
        }
    }
}

/// Characters a run sets in its `w:ascii` font; the rest take `w:hAnsi`.
pub(super) fn is_ascii_slot(ch: char) -> bool {
    (ch as u32) < 0x80
}

impl Hash for Appearance {
    fn hash<H: Hasher>(&self, state: &mut H) {
        [self.size, self.rise, self.spacing, self.scale]
            .map(f64::to_bits)
            .hash(state);
        (
            self.metric,
            self.ascii,
            self.hansi,
            self.colour,
            self.bold,
            self.italic,
            self.underline,
        )
            .hash(state);
        (
            self.strike,
            self.highlight,
            self.shade,
            self.caps,
            self.link,
        )
            .hash(state);
    }
}

/// Experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Underline {
    pub style: UnderlineStyle,
    /// `Auto`: the text's colour.
    pub colour: Colour,
}

/// `w:u` values other than `none`. Experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnderlineStyle {
    Single,
    /// Under words only, not the spaces between them.
    Words,
    Double,
    Thick,
    Dotted,
    DottedHeavy,
    Dash,
    DashedHeavy,
    DashLong,
    DashLongHeavy,
    DotDash,
    DashDotHeavy,
    DotDotDash,
    DashDotDotHeavy,
    Wave,
    WavyHeavy,
    WavyDouble,
}

impl UnderlineStyle {
    const NAMES: [(&str, UnderlineStyle); 17] = [
        ("single", UnderlineStyle::Single),
        ("words", UnderlineStyle::Words),
        ("double", UnderlineStyle::Double),
        ("thick", UnderlineStyle::Thick),
        ("dotted", UnderlineStyle::Dotted),
        ("dottedHeavy", UnderlineStyle::DottedHeavy),
        ("dash", UnderlineStyle::Dash),
        ("dashedHeavy", UnderlineStyle::DashedHeavy),
        ("dashLong", UnderlineStyle::DashLong),
        ("dashLongHeavy", UnderlineStyle::DashLongHeavy),
        ("dotDash", UnderlineStyle::DotDash),
        ("dashDotHeavy", UnderlineStyle::DashDotHeavy),
        ("dotDotDash", UnderlineStyle::DotDotDash),
        ("dashDotDotHeavy", UnderlineStyle::DashDotDotHeavy),
        ("wave", UnderlineStyle::Wave),
        ("wavyHeavy", UnderlineStyle::WavyHeavy),
        ("wavyDouble", UnderlineStyle::WavyDouble),
    ];

    /// The `w:u` value.
    pub fn name(self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|(_, style)| *style == self)
            .map_or("single", |(name, _)| name)
    }
}

/// Experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Strike {
    #[default]
    None,
    Single,
    Double,
}

impl Strike {
    pub fn name(self) -> &'static str {
        match self {
            Strike::None => "none",
            Strike::Single => "single",
            Strike::Double => "double",
        }
    }
}

/// Experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Caps {
    #[default]
    None,
    All,
    /// Lower case drawn as capitals, and spaces set, at 80 % of the size
    /// rounded to a half point.
    Small,
}

impl Caps {
    pub(super) fn of(caps: bool, small_caps: bool) -> Caps {
        if caps {
            Caps::All
        } else if small_caps {
            Caps::Small
        } else {
            Caps::None
        }
    }

    /// The character drawn for `ch` in a run of `size` points, and its size
    /// (Word for Mac, measured: 9 pt small capitals at 11 pt).
    pub fn apply(self, ch: char, size: f64) -> (char, f64) {
        let upper = || ch.to_uppercase().next().unwrap_or(ch);
        match self {
            Caps::All => (upper(), size),
            Caps::Small if ch.is_lowercase() || ch == ' ' => {
                (upper(), (size * 0.8 * 2.0).round() / 2.0)
            }
            _ => (ch, size),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Caps::None => "none",
            Caps::All => "all",
            Caps::Small => "small",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VertAlign {
    #[default]
    Baseline,
    Superscript,
    Subscript,
}

/// A hyperlink's target. Experimental.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Link {
    Uri(String),
    /// A bookmark in this document.
    Anchor(String),
}

impl Link {
    /// A link to `uri`: `#name` is the bookmark `name`, as Word reads it.
    pub(crate) fn to(uri: &str) -> Link {
        match uri.trim().strip_prefix('#') {
            Some(bookmark) => Link::Anchor(bookmark.to_string()),
            None => Link::Uri(uri.to_string()),
        }
    }
}

/// A paragraph or cell border line. Experimental.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Border {
    pub style: BorderStyle,
    /// Points.
    pub width: f64,
    /// Points between the border and the text.
    pub space: f64,
    pub colour: Colour,
}

impl Border {
    /// The lines and the gaps between them: Word sets each `width` wide.
    pub(crate) fn thickness(&self) -> f64 {
        let lines = match self.style {
            BorderStyle::Double => 3.0,
            BorderStyle::Triple => 5.0,
            _ => 1.0,
        };
        self.width * lines
    }

    /// The room the border takes beside the text: its thickness and space.
    pub(crate) fn extent(&self) -> f64 {
        self.thickness() + self.space
    }
}

/// `ST_Border` line styles; the picture borders are `Art`. Experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BorderStyle {
    Single,
    Thick,
    Double,
    Dotted,
    Dashed,
    DotDash,
    DotDotDash,
    Triple,
    ThinThickSmallGap,
    ThickThinSmallGap,
    ThinThickThinSmallGap,
    ThinThickMediumGap,
    ThickThinMediumGap,
    ThinThickThinMediumGap,
    ThinThickLargeGap,
    ThickThinLargeGap,
    ThinThickThinLargeGap,
    Wave,
    DoubleWave,
    DashSmallGap,
    DashDotStroked,
    ThreeDEmboss,
    ThreeDEngrave,
    Outset,
    Inset,
    Art,
}

impl BorderStyle {
    /// Drawn as the style says; the PDF draws any other as a single line.
    #[cfg(feature = "pdf")]
    pub(crate) fn drawn(self) -> bool {
        matches!(
            self,
            BorderStyle::Single
                | BorderStyle::Thick
                | BorderStyle::Double
                | BorderStyle::Triple
                | BorderStyle::Dotted
                | BorderStyle::Dashed
                | BorderStyle::DashSmallGap
                | BorderStyle::DotDash
                | BorderStyle::DotDotDash
        )
    }

    const NAMES: [(&str, BorderStyle); 25] = [
        ("single", BorderStyle::Single),
        ("thick", BorderStyle::Thick),
        ("double", BorderStyle::Double),
        ("dotted", BorderStyle::Dotted),
        ("dashed", BorderStyle::Dashed),
        ("dotDash", BorderStyle::DotDash),
        ("dotDotDash", BorderStyle::DotDotDash),
        ("triple", BorderStyle::Triple),
        ("thinThickSmallGap", BorderStyle::ThinThickSmallGap),
        ("thickThinSmallGap", BorderStyle::ThickThinSmallGap),
        ("thinThickThinSmallGap", BorderStyle::ThinThickThinSmallGap),
        ("thinThickMediumGap", BorderStyle::ThinThickMediumGap),
        ("thickThinMediumGap", BorderStyle::ThickThinMediumGap),
        (
            "thinThickThinMediumGap",
            BorderStyle::ThinThickThinMediumGap,
        ),
        ("thinThickLargeGap", BorderStyle::ThinThickLargeGap),
        ("thickThinLargeGap", BorderStyle::ThickThinLargeGap),
        ("thinThickThinLargeGap", BorderStyle::ThinThickThinLargeGap),
        ("wave", BorderStyle::Wave),
        ("doubleWave", BorderStyle::DoubleWave),
        ("dashSmallGap", BorderStyle::DashSmallGap),
        ("dashDotStroked", BorderStyle::DashDotStroked),
        ("threeDEmboss", BorderStyle::ThreeDEmboss),
        ("threeDEngrave", BorderStyle::ThreeDEngrave),
        ("outset", BorderStyle::Outset),
        ("inset", BorderStyle::Inset),
    ];
}

impl BorderStyle {
    /// The style `name`s; `None` for `none` and `nil`, and a name not in
    /// `ST_Border` a picture border.
    pub(super) fn read(name: &str) -> Option<BorderStyle> {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, style)| *style)
            .or_else(|| (!matches!(name, "none" | "nil")).then_some(BorderStyle::Art))
    }

    /// The `ST_Border` value; `art` for the picture borders.
    pub fn name(self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|(_, style)| *style == self)
            .map_or("art", |(name, _)| name)
    }

    /// Its rank among equal-weight borders: lower wins.
    pub(super) fn precedence(self) -> usize {
        Self::NAMES
            .iter()
            .position(|(_, style)| *style == self)
            .unwrap_or(Self::NAMES.len())
    }

    /// The factor of its width in a border's weight; dotted and dashed
    /// borders weigh 1 whatever their width (MS-OI29500 §2.1.169).
    pub(super) fn number(self) -> Option<u32> {
        Some(match self {
            BorderStyle::Dotted | BorderStyle::Dashed => return None,
            BorderStyle::Single | BorderStyle::Art => 1,
            BorderStyle::Thick => 2,
            BorderStyle::Double => 3,
            other => other.precedence() as u32 + 3,
        })
    }
}

/// Paragraph borders (`w:pBdr`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParaBorders {
    pub top: Option<Border>,
    pub left: Option<Border>,
    pub bottom: Option<Border>,
    pub right: Option<Border>,
    /// Between paragraphs with the same borders.
    pub between: Option<Border>,
}

impl ParaBorders {
    pub(super) const SIDES: [[&str; 2]; 5] = [
        ["top", "top"],
        ["left", "start"],
        ["bottom", "bottom"],
        ["right", "end"],
        ["between", "between"],
    ];

    pub(super) fn from_sides(sides: [Option<Border>; 5]) -> Option<Box<Self>> {
        let [top, left, bottom, right, between] = sides;
        sides.iter().any(Option::is_some).then(|| {
            Box::new(ParaBorders {
                top,
                left,
                bottom,
                right,
                between,
            })
        })
    }
}

/// A cell edge as one level of the table cascade sets it: `None` defers to
/// the next level, `Nil` ends the lookup with no line. Against the adjacent
/// cell's edge both yield.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Edge {
    Line(Border),
    None,
    Nil,
}

/// The sides of `w:tblBorders` and `w:tcBorders`, by `Side`.
pub(super) const EDGE_SIDES: [[&str; 2]; 6] = [
    ["top", "top"],
    ["left", "start"],
    ["bottom", "bottom"],
    ["right", "end"],
    ["insideH", "insideH"],
    ["insideV", "insideV"],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

impl VAlign {
    /// The share of a cell's spare height above its content.
    pub(crate) fn share(self) -> f64 {
        match self {
            VAlign::Top => 0.0,
            VAlign::Center => 0.5,
            VAlign::Bottom => 1.0,
        }
    }

    pub(super) fn read(value: Option<&str>) -> Option<VAlign> {
        match value? {
            "top" => Some(VAlign::Top),
            "center" => Some(VAlign::Center),
            "bottom" => Some(VAlign::Bottom),
            _ => None,
        }
    }
}

/// `w:u`; `None` is `none`.
pub(super) fn read_underline(tree: &XmlTree, u: NodeId, palette: &Palette) -> Option<Underline> {
    let style = match w_attr(tree, u, "val") {
        Some("none") => return None,
        Some(name) => UnderlineStyle::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(UnderlineStyle::Single, |(_, style)| *style),
        None => UnderlineStyle::Single,
    };
    let colour = palette
        .colour(tree, u, ["color", "themeColor", "themeTint", "themeShade"])
        .unwrap_or_default();
    Some(Underline { style, colour })
}

/// One side of a cell or table border element.
pub(super) fn read_edge(tree: &XmlTree, side: NodeId, palette: &Palette) -> Edge {
    match read_border(tree, side, palette) {
        Some(border) => Edge::Line(border),
        None if w_attr(tree, side, "val") == Some("nil") => Edge::Nil,
        None => Edge::None,
    }
}

/// One side of a border element: `None` is `none` or `nil`.
pub(super) fn read_border(tree: &XmlTree, side: NodeId, palette: &Palette) -> Option<Border> {
    let style = BorderStyle::read(w_attr(tree, side, "val")?)?;
    let sz = w_attr(tree, side, "sz").and_then(number);
    // Line widths are eighths of a point (¼–12 pt); picture borders' are points.
    let width = match style {
        BorderStyle::Art => sz.map_or(1.0, |pt| pt.clamp(1.0, 31.0)),
        _ => sz.map_or(0.25, |eighths| eighths.clamp(2.0, 96.0) / 8.0),
    };
    Some(Border {
        style,
        width,
        space: w_attr(tree, side, "space")
            .and_then(number)
            .map_or(0.0, |pt| pt.clamp(0.0, 31.0)),
        colour: palette
            .colour(
                tree,
                side,
                ["color", "themeColor", "themeTint", "themeShade"],
            )
            .unwrap_or_default(),
    })
}
