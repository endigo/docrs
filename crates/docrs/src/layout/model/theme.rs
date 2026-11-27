use std::collections::HashMap;

use super::fonts::FontKind;
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::XmlTree;

struct Theme {
    major: Option<String>,
    minor: Option<String>,
}

fn read_theme(tree: Option<&XmlTree>) -> Theme {
    const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    let mut theme = Theme {
        major: None,
        minor: None,
    };
    if let Some(tree) = tree {
        for node in tree.descendants(tree.root()) {
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

/// Font kinds named by the font table (`w:family`).
fn read_font_kinds(tree: Option<&XmlTree>) -> HashMap<String, FontKind> {
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
