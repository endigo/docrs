//! What a table style draws, for the PDF preflight.

use super::look::BorderStyle;
use super::props::on_off;
use crate::wml::W;
use crate::wml::w_attr;
use crate::xml::{NodeId, XmlTree};

/// What of a table style the PDF changes. Every conditional format counts
/// whatever the table's `w:tblLook` turns off, so it can over-report, never
/// under-report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TableMarks {
    /// A border in a style drawn as a single line.
    pub(crate) approximated: bool,
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
                "tblBorders" | "tcBorders" => marks.approximated |= approximated(tree, node),
                "tblpPr" => marks.floating = true,
                "bidiVisual" => marks.bidi |= on_off(tree, node),
                _ => {}
            }
        }
        marks
    }

    pub(super) fn or(self, other: TableMarks) -> TableMarks {
        TableMarks {
            approximated: self.approximated || other.approximated,
            floating: self.floating || other.floating,
            bidi: self.bidi || other.bidi,
        }
    }
}

/// A border group (`w:tblBorders`, `w:tcBorders`) with an edge in a style
/// the PDF draws as a single line.
pub(crate) fn approximated(tree: &XmlTree, group: NodeId) -> bool {
    tree.child_elements(group).any(|edge| {
        w_attr(tree, edge, "val")
            .and_then(BorderStyle::read)
            .is_some_and(|style| !style.drawn())
    })
}
