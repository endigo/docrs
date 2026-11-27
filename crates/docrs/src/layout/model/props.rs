use super::colour::{Colour, Palette, Rgb, highlight};
use super::look::{Border, ParaBorders, Underline, VertAlign, read_border, read_underline};
use super::{Align, LineRule, TabKind};
use crate::units::TWIPS_PER_PT;
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

const TWIP: f64 = 1.0 / TWIPS_PER_PT as f64;

/// Word clamps lengths to ±22 inches.
const MAX_LENGTH: f64 = 22.0 * 72.0;
/// Word clamps font sizes to 1–1,638 points.
const FONT_SIZES: (f64, f64) = (1.0, 1638.0);
/// Line spacing in 240ths of a line or twips: 132 lines, or 22 inches.
const MAX_LINE: f64 = 31680.0;

#[derive(Debug, Clone, Default)]
pub(super) struct PPr {
    pub(super) before: Option<f64>,
    pub(super) after: Option<f64>,
    pub(super) before_auto: Option<bool>,
    pub(super) after_auto: Option<bool>,
    pub(super) line: Option<(f64, LineRule)>,
    pub(super) left: Option<f64>,
    pub(super) right: Option<f64>,
    pub(super) first: Option<f64>,
    pub(super) keep_next: Option<bool>,
    pub(super) keep_lines: Option<bool>,
    pub(super) page_break_before: Option<bool>,
    pub(super) widow_control: Option<bool>,
    pub(super) contextual: Option<bool>,
    pub(super) align: Option<Align>,
    pub(super) tabs: Vec<(f64, TabKind)>,
    pub(super) num: Option<(String, Option<u8>)>,
    pub(super) ilvl: Option<u8>,
    /// `Some(None)`: no fill.
    pub(super) shd: Option<Option<Rgb>>,
    /// `ParaBorders::SIDES`; `Some(None)`: `none` or `nil`.
    pub(super) borders: [Option<Option<Border>>; 5],
}

impl PPr {
    pub(super) fn merge(&mut self, other: &PPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f.clone(); } )* };
        }
        take!(
            before,
            after,
            before_auto,
            after_auto,
            line,
            left,
            right,
            first,
            keep_next,
            keep_lines,
            page_break_before,
            widow_control,
            contextual,
            align,
            num,
            ilvl,
            shd
        );
        merge_sides(&mut self.borders, &other.borders);
        for (pos, kind) in &other.tabs {
            self.tabs.retain(|(p, _)| (p - pos).abs() > 0.01);
            if *kind != TabKind::Clear {
                self.tabs.push((*pos, *kind));
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum FontRef {
    Name(String),
    Theme(bool),
}

#[derive(Debug, Clone, Default)]
pub(super) struct RPr {
    pub(super) ascii: Option<FontRef>,
    pub(super) hansi: Option<FontRef>,
    pub(super) size: Option<f64>,
    pub(super) bold: Option<bool>,
    pub(super) italic: Option<bool>,
    pub(super) caps: Option<bool>,
    pub(super) small_caps: Option<bool>,
    pub(super) vanish: Option<bool>,
    pub(super) spacing: Option<f64>,
    pub(super) scale: Option<f64>,
    pub(super) vert: Option<VertAlign>,
    /// Kerning from this size (points) up; 0 is off.
    pub(super) kern: Option<f64>,
    pub(super) colour: Option<Colour>,
    /// `Some(None)`: `none`.
    pub(super) underline: Option<Option<Underline>>,
    pub(super) strike: Option<bool>,
    pub(super) dstrike: Option<bool>,
    /// `Some(None)`: `none`.
    pub(super) highlight: Option<Option<Rgb>>,
    /// `Some(None)`: no fill.
    pub(super) shd: Option<Option<Rgb>>,
    /// Points raised (`w:position`).
    pub(super) position: Option<f64>,
}

impl RPr {
    pub(super) fn merge(&mut self, other: &RPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f.clone(); } )* };
        }
        take!(
            ascii, hansi, size, bold, italic, caps, small_caps, vanish, spacing, scale, vert, kern,
            colour, underline, strike, dstrike, highlight, shd, position
        );
    }

    /// Style layering: toggles XOR with `other`'s (a `false` changes
    /// nothing), the rest is replaced.
    pub(super) fn layer(&mut self, other: &RPr) {
        let xor = |mine: Option<bool>, theirs: Option<bool>| match (mine, theirs) {
            (Some(a), Some(b)) => Some(a ^ b),
            (a, b) => b.or(a),
        };
        let toggles = [
            xor(self.bold, other.bold),
            xor(self.italic, other.italic),
            xor(self.caps, other.caps),
            xor(self.small_caps, other.small_caps),
            xor(self.vanish, other.vanish),
            xor(self.strike, other.strike),
        ];
        self.merge(other);
        [
            self.bold,
            self.italic,
            self.caps,
            self.small_caps,
            self.vanish,
            self.strike,
        ] = toggles;
    }
}

/// Each side `other` sets replaces `mine`'s.
pub(super) fn merge_sides<const N: usize>(
    mine: &mut [Option<Option<Border>>; N],
    other: &[Option<Option<Border>>; N],
) {
    for (slot, side) in mine.iter_mut().zip(other) {
        if side.is_some() {
            *slot = *side;
        }
    }
}

/// A border element's side by its name, or its logical name.
pub(super) fn side(tree: &XmlTree, borders: NodeId, [name, logical]: [&str; 2]) -> Option<NodeId> {
    w_child(tree, borders, name).or_else(|| w_child(tree, borders, logical))
}

pub(super) fn on_off(tree: &XmlTree, node: NodeId) -> bool {
    !matches!(
        w_attr(tree, node, "val"),
        Some("0" | "false" | "off" | "none")
    )
}

pub(super) fn number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub(super) fn length(points: f64) -> f64 {
    points.clamp(-MAX_LENGTH, MAX_LENGTH)
}

pub(super) fn twips(tree: &XmlTree, node: NodeId, name: &str) -> Option<f64> {
    w_attr(tree, node, name)
        .and_then(number)
        .map(|v| length(v * TWIP))
}

fn half_points(tree: &XmlTree, node: NodeId, min: f64) -> Option<f64> {
    w_attr(tree, node, "val")
        .and_then(number)
        .map(|half| (half / 2.0).clamp(min, FONT_SIZES.1))
}

pub(super) fn read_ppr(tree: &XmlTree, ppr: Option<NodeId>, palette: &Palette) -> PPr {
    let mut out = PPr::default();
    let Some(ppr) = ppr else {
        return out;
    };
    for child in tree.child_elements(ppr) {
        let Some(local) = tree.local_name(child) else {
            continue;
        };
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        match local {
            "spacing" => {
                out.before = twips(tree, child, "before");
                out.after = twips(tree, child, "after");
                out.before_auto = w_attr(tree, child, "beforeAutospacing")
                    .map(|v| !matches!(v, "0" | "false" | "off"));
                out.after_auto = w_attr(tree, child, "afterAutospacing")
                    .map(|v| !matches!(v, "0" | "false" | "off"));
                if let Some(line) = w_attr(tree, child, "line")
                    .and_then(number)
                    .map(|line| line.clamp(-MAX_LINE, MAX_LINE))
                {
                    let rule = match w_attr(tree, child, "lineRule") {
                        Some("exact") => LineRule::Exact,
                        Some("atLeast") => LineRule::AtLeast,
                        _ => LineRule::Auto,
                    };
                    out.line = Some((line, rule));
                }
            }
            "ind" => {
                out.left = twips(tree, child, "left").or_else(|| twips(tree, child, "start"));
                out.right = twips(tree, child, "right").or_else(|| twips(tree, child, "end"));
                if let Some(hanging) = twips(tree, child, "hanging") {
                    out.first = Some(-hanging);
                } else if let Some(first) = twips(tree, child, "firstLine") {
                    out.first = Some(first);
                }
            }
            "keepNext" => out.keep_next = Some(on_off(tree, child)),
            "keepLines" => out.keep_lines = Some(on_off(tree, child)),
            "pageBreakBefore" => out.page_break_before = Some(on_off(tree, child)),
            "widowControl" => out.widow_control = Some(on_off(tree, child)),
            "contextualSpacing" => out.contextual = Some(on_off(tree, child)),
            "jc" => out.align = Some(Align::read(w_attr(tree, child, "val"))),
            "tabs" => {
                for tab in w_children(tree, child, "tab") {
                    let Some(pos) = twips(tree, tab, "pos") else {
                        continue;
                    };
                    let kind = match w_attr(tree, tab, "val") {
                        Some("right" | "end") => TabKind::Right,
                        Some("center") => TabKind::Center,
                        Some("decimal") => TabKind::Decimal,
                        Some("clear") => TabKind::Clear,
                        _ => TabKind::Left,
                    };
                    out.tabs.push((pos, kind));
                }
            }
            "numPr" => {
                let ilvl = w_child(tree, child, "ilvl")
                    .and_then(|n| w_attr(tree, n, "val"))
                    .and_then(|v| v.parse().ok());
                if let Some(id) = w_child(tree, child, "numId").and_then(|n| w_attr(tree, n, "val"))
                {
                    out.num = Some((id.to_string(), ilvl));
                }
                out.ilvl = ilvl;
            }
            "shd" => out.shd = Some(palette.shading(tree, child)),
            "pBdr" => {
                for (slot, names) in out.borders.iter_mut().zip(ParaBorders::SIDES) {
                    *slot = side(tree, child, names).map(|s| read_border(tree, s, palette));
                }
            }
            _ => {}
        }
    }
    out
}

pub(super) fn read_rpr(tree: &XmlTree, rpr: Option<NodeId>, palette: &Palette) -> RPr {
    let mut out = RPr::default();
    let Some(rpr) = rpr else {
        return out;
    };
    for child in tree.child_elements(rpr) {
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        match tree.local_name(child).unwrap_or("") {
            "rFonts" => {
                let theme = |name: &str| {
                    w_attr(tree, child, name).map(|v| FontRef::Theme(v.starts_with("major")))
                };
                let named =
                    |name: &str| w_attr(tree, child, name).map(|v| FontRef::Name(v.to_string()));
                out.ascii = theme("asciiTheme").or_else(|| named("ascii"));
                out.hansi = theme("hAnsiTheme").or_else(|| named("hAnsi"));
            }
            "sz" => out.size = half_points(tree, child, FONT_SIZES.0),
            "b" => out.bold = Some(on_off(tree, child)),
            "i" => out.italic = Some(on_off(tree, child)),
            "caps" => out.caps = Some(on_off(tree, child)),
            "smallCaps" => out.small_caps = Some(on_off(tree, child)),
            "vanish" => out.vanish = Some(on_off(tree, child)),
            "spacing" => out.spacing = twips(tree, child, "val"),
            "kern" => out.kern = half_points(tree, child, 0.0),
            "w" => {
                out.scale = w_attr(tree, child, "val")
                    .and_then(|v| number(v.trim_end_matches('%')))
                    .map(|scale| scale.clamp(1.0, 600.0))
            }
            "vertAlign" => {
                out.vert = Some(match w_attr(tree, child, "val") {
                    Some("superscript") => VertAlign::Superscript,
                    Some("subscript") => VertAlign::Subscript,
                    _ => VertAlign::Baseline,
                })
            }
            "color" => {
                out.colour = palette.colour(
                    tree,
                    child,
                    ["val", "themeColor", "themeTint", "themeShade"],
                )
            }
            "u" => out.underline = Some(read_underline(tree, child, palette)),
            "strike" => out.strike = Some(on_off(tree, child)),
            "dstrike" => out.dstrike = Some(on_off(tree, child)),
            "highlight" => out.highlight = w_attr(tree, child, "val").map(highlight),
            "shd" => out.shd = Some(palette.shading(tree, child)),
            "position" => {
                out.position = w_attr(tree, child, "val")
                    .and_then(number)
                    .map(|half| (half / 2.0).clamp(-FONT_SIZES.1, FONT_SIZES.1))
            }
            _ => {}
        }
    }
    out
}
