use std::collections::HashMap;

use super::colour::{Palette, Rgb, SCHEME_SLOTS};
use crate::layout::fonts::FontKind;
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

pub(super) struct Theme {
    pub(super) major: Option<String>,
    pub(super) minor: Option<String>,
    pub(super) palette: Palette,
}

pub(super) fn read_theme(tree: Option<&XmlTree>) -> Theme {
    let mut theme = Theme {
        major: None,
        minor: None,
        palette: Palette::default(),
    };
    if let Some(tree) = tree {
        for node in tree.descendants(tree.root()) {
            if tree.is(node, A, "clrScheme") {
                read_scheme(tree, node, &mut theme.palette);
            }
            for (local, slot) in [
                ("majorFont", &mut theme.major),
                ("minorFont", &mut theme.minor),
            ] {
                if tree.is(node, A, local) && slot.is_none() {
                    *slot = tree
                        .first_child_named(node, A, "latin")
                        .and_then(|l| tree.attr(l, None, "typeface"))
                        .filter(|t| !t.is_empty())
                        .map(str::to_string);
                }
            }
        }
    }
    theme
}

/// The first colour scheme's slots: `a:srgbClr`, or `a:sysClr`'s cached
/// `lastClr`.
fn read_scheme(tree: &XmlTree, scheme: NodeId, palette: &mut Palette) {
    for (slot, name) in palette.0.iter_mut().zip(SCHEME_SLOTS) {
        if slot.is_some() {
            continue;
        }
        let colour = tree.first_child_named(scheme, A, name).and_then(|c| {
            tree.child_elements(c)
                .find_map(|value| match tree.local_name(value) {
                    Some("srgbClr") => tree.attr(value, None, "val"),
                    Some("sysClr") => tree.attr(value, None, "lastClr"),
                    _ => None,
                })
        });
        *slot = colour.and_then(Rgb::hex);
    }
}

/// Font kinds named by the font table (`w:family`).
pub(super) fn read_font_kinds(tree: Option<&XmlTree>) -> HashMap<String, FontKind> {
    let mut out = HashMap::new();
    if let Some(tree) = tree {
        for font in w_children(tree, tree.root(), "font") {
            let Some(name) = w_attr(tree, font, "name") else {
                continue;
            };
            let fixed =
                w_child(tree, font, "pitch").and_then(|p| w_attr(tree, p, "val")) == Some("fixed");
            let kind = match w_child(tree, font, "family").and_then(|f| w_attr(tree, f, "val")) {
                _ if fixed => FontKind::Mono,
                Some("modern") => FontKind::Mono,
                Some("roman") => FontKind::Serif,
                _ => FontKind::Sans,
            };
            out.insert(name.to_lowercase(), kind);
        }
    }
    out
}
