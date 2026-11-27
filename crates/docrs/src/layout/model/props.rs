use super::{LineRule, TabKind};
use crate::units::TWIPS_PER_PT;
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

/// Points per twip.
const TWIP: f64 = 1.0 / TWIPS_PER_PT as f64;

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
    pub(super) justified: Option<bool>,
    pub(super) tabs: Vec<(f64, TabKind)>,
    pub(super) num: Option<(String, Option<u8>)>,
    pub(super) ilvl: Option<u8>,
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
            justified,
            num,
            ilvl
        );
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
    pub(super) vert: Option<bool>,
    /// Kerning from this size (points) up; 0 is off.
    pub(super) kern: Option<f64>,
}

impl RPr {
    pub(super) fn merge(&mut self, other: &RPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f.clone(); } )* };
        }
        take!(
            ascii, hansi, size, bold, italic, caps, small_caps, vanish, spacing, scale, vert, kern
        );
    }

    /// Style layering: toggles XOR with `other`'s, the rest is replaced.
    pub(super) fn layer(&mut self, other: &RPr) {
        let toggles = [
            (self.bold, other.bold),
            (self.italic, other.italic),
            (self.caps, other.caps),
            (self.small_caps, other.small_caps),
        ];
        self.merge(other);
        let xor = |(mine, theirs): (Option<bool>, Option<bool>)| match (mine, theirs) {
            (Some(a), Some(b)) => Some(a ^ b),
            (a, b) => b.or(a),
        };
        self.bold = xor(toggles[0]);
        self.italic = xor(toggles[1]);
        self.caps = xor(toggles[2]);
        self.small_caps = xor(toggles[3]);
    }
}

pub(super) fn on_off(tree: &XmlTree, node: NodeId) -> bool {
    !matches!(
        w_attr(tree, node, "val"),
        Some("0" | "false" | "off" | "none")
    )
}

pub(super) fn twips(tree: &XmlTree, node: NodeId, name: &str) -> Option<f64> {
    w_attr(tree, node, name)
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v * TWIP)
}

pub(super) fn read_ppr(tree: &XmlTree, ppr: Option<NodeId>) -> PPr {
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
                if let Some(line) = w_attr(tree, child, "line").and_then(|v| v.parse::<f64>().ok())
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
            "jc" => {
                out.justified = Some(matches!(
                    w_attr(tree, child, "val"),
                    Some(
                        "both"
                            | "distribute"
                            | "lowKashida"
                            | "mediumKashida"
                            | "highKashida"
                            | "thaiDistribute"
                    )
                ))
            }
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
            _ => {}
        }
    }
    out
}

pub(super) fn read_rpr(tree: &XmlTree, rpr: Option<NodeId>) -> RPr {
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
            "sz" => {
                out.size = w_attr(tree, child, "val")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| v / 2.0)
            }
            "b" => out.bold = Some(on_off(tree, child)),
            "i" => out.italic = Some(on_off(tree, child)),
            "caps" => out.caps = Some(on_off(tree, child)),
            "smallCaps" => out.small_caps = Some(on_off(tree, child)),
            "vanish" => out.vanish = Some(on_off(tree, child)),
            "spacing" => out.spacing = twips(tree, child, "val"),
            "kern" => {
                out.kern = w_attr(tree, child, "val")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|half| half / 2.0)
            }
            "w" => {
                out.scale = w_attr(tree, child, "val")
                    .and_then(|v| v.trim_end_matches('%').parse::<f64>().ok())
            }
            "vertAlign" => {
                out.vert = Some(matches!(
                    w_attr(tree, child, "val"),
                    Some("superscript" | "subscript")
                ))
            }
            _ => {}
        }
    }
    out
}

/// Border widths in points from `w:tblBorders` or `w:tcBorders`: top,
/// bottom and inside horizontal (`none` and `nil` are 0).
pub(super) fn borders(tree: &XmlTree, node: Option<NodeId>) -> [Option<f64>; 3] {
    let mut out = [None; 3];
    if let Some(node) = node {
        for (slot, name) in out.iter_mut().zip(["top", "bottom", "insideH"]) {
            *slot = w_child(tree, node, name).map(|side| match w_attr(tree, side, "val") {
                Some("none" | "nil") | None => 0.0,
                Some(_) => {
                    w_attr(tree, side, "sz")
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(2.0)
                        / 8.0
                }
            });
        }
    }
    out
}

pub(super) fn margins(tree: &XmlTree, mar: Option<NodeId>) -> [Option<f64>; 4] {
    let mut out = [None; 4];
    if let Some(mar) = mar {
        for (index, names) in [
            ["top", "top"],
            ["bottom", "bottom"],
            ["left", "start"],
            ["right", "end"],
        ]
        .iter()
        .enumerate()
        {
            let side = w_child(tree, mar, names[0]).or_else(|| w_child(tree, mar, names[1]));
            out[index] = side.and_then(|s| twips(tree, s, "w"));
        }
    }
    out
}
