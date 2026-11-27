//! What a table style draws, for the PDF preflight.

use super::props::{on_off, shaded};
use crate::wml::W;
use crate::wml::w_attr;
use crate::xml::{NodeId, XmlTree};

/// What a table style draws around and behind its cells. Every
/// conditional format counts whatever the table's `w:tblLook` turns off, so
/// it can over-report, never under-report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TableMarks {
    pub(crate) border: bool,
    pub(crate) shading: bool,
    pub(crate) floating: bool,
    pub(crate) bidi: bool,
}

impl TableMarks {
    /// Read from a style's table and cell properties, conditional ones
    /// included.
    pub(super) fn read(tree: &XmlTree, style: NodeId) -> TableMarks {
        let mut marks = TableMarks::default();
        for node in tree.descendants(style) {
            let in_table_props = tree
                .parent(node)
                .is_some_and(|p| tree.is(p, W, "tblPr") || tree.is(p, W, "tcPr"));
            if !in_table_props {
                continue;
            }
            match tree.local_name(node).unwrap_or("") {
                "tblBorders" | "tcBorders" => marks.border |= any_border(tree, node),
                "shd" => marks.shading |= shaded(tree, node),
                "tblpPr" => marks.floating = true,
                "bidiVisual" => marks.bidi |= on_off(tree, node),
                _ => {}
            }
        }
        marks
    }

    pub(super) fn or(self, other: TableMarks) -> TableMarks {
        TableMarks {
            border: self.border || other.border,
            shading: self.shading || other.shading,
            floating: self.floating || other.floating,
            bidi: self.bidi || other.bidi,
        }
    }
}

/// A border group (`w:tblBorders`, `w:tcBorders`) with an edge that draws a
/// line.
pub(crate) fn any_border(tree: &XmlTree, group: NodeId) -> bool {
    tree.child_elements(group)
        .any(|edge| !matches!(w_attr(tree, edge, "val"), None | Some("none" | "nil")))
}
