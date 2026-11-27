use std::collections::HashMap;
use std::rc::Rc;

use super::colour::Palette;
#[cfg(feature = "pdf")]
use super::marks::TableMarks;
use super::props::{PPr, RPr, read_ppr, read_rpr};
use super::table::{CellProps, borders, margins, read_cell_props};
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::XmlTree;

pub(super) struct StyleDef {
    based_on: Option<String>,
    ppr: PPr,
    rpr: RPr,
    /// Table styles: whole-table cell margins and borders, first-row look.
    cell_margins: [Option<f64>; 4],
    borders: [Option<f64>; 3],
    pub(super) first_row: Option<(PPr, RPr)>,
    #[cfg(feature = "pdf")]
    marks: TableMarks,
    /// Table styles: whole-table cell properties (`w:tcPr`).
    cell: CellProps,
    /// Position in the styles part.
    order: usize,
    /// A table style with table properties of its own (`w:tblPr` content
    /// or conditional formatting).
    table_props: bool,
}

pub(super) struct Styles {
    pub(super) defs: HashMap<String, StyleDef>,
    pub(super) default_para: Option<String>,
    pub(super) default_table: Option<String>,
    pub(super) default_ppr: PPr,
    pub(super) default_rpr: RPr,
    chains: HashMap<String, Rc<(PPr, RPr)>>,
}

impl Styles {
    pub(super) fn read(tree: Option<&XmlTree>, palette: &Palette) -> Styles {
        let mut styles = Styles {
            defs: HashMap::new(),
            default_para: None,
            default_table: None,
            default_ppr: PPr::default(),
            default_rpr: RPr::default(),
            chains: HashMap::new(),
        };
        let Some(tree) = tree else {
            return styles;
        };
        let root = tree.root();
        if let Some(defaults) = w_child(tree, root, "docDefaults") {
            let rpr = w_child(tree, defaults, "rPrDefault").and_then(|d| w_child(tree, d, "rPr"));
            let ppr = w_child(tree, defaults, "pPrDefault").and_then(|d| w_child(tree, d, "pPr"));
            styles.default_rpr = read_rpr(tree, rpr, palette);
            styles.default_ppr = read_ppr(tree, ppr, palette);
        }
        for (order, style) in w_children(tree, root, "style").enumerate() {
            let Some(id) = w_attr(tree, style, "styleId") else {
                continue;
            };
            let kind = w_attr(tree, style, "type").unwrap_or("paragraph");
            let default = matches!(w_attr(tree, style, "default"), Some("1" | "true" | "on"));
            if default && kind == "paragraph" && styles.default_para.is_none() {
                styles.default_para = Some(id.to_string());
            }
            if default && kind == "table" && styles.default_table.is_none() {
                styles.default_table = Some(id.to_string());
            }
            let tbl_ppr = w_child(tree, style, "tblPr");
            let first_row = w_children(tree, style, "tblStylePr")
                .find(|s| w_attr(tree, *s, "type") == Some("firstRow"))
                .map(|s| {
                    (
                        read_ppr(tree, w_child(tree, s, "pPr"), palette),
                        read_rpr(tree, w_child(tree, s, "rPr"), palette),
                    )
                });
            styles.defs.insert(
                id.to_string(),
                StyleDef {
                    based_on: w_child(tree, style, "basedOn")
                        .and_then(|b| w_attr(tree, b, "val"))
                        .map(str::to_string),
                    ppr: read_ppr(tree, w_child(tree, style, "pPr"), palette),
                    rpr: read_rpr(tree, w_child(tree, style, "rPr"), palette),
                    cell_margins: margins(
                        tree,
                        tbl_ppr.and_then(|t| w_child(tree, t, "tblCellMar")),
                    ),
                    borders: borders(tree, tbl_ppr.and_then(|t| w_child(tree, t, "tblBorders"))),
                    first_row,
                    #[cfg(feature = "pdf")]
                    marks: TableMarks::read(tree, style),
                    cell: read_cell_props(tree, w_child(tree, style, "tcPr"), palette),
                    order,
                    table_props: tbl_ppr.is_some_and(|t| tree.child_elements(t).next().is_some())
                        || w_child(tree, style, "tblStylePr").is_some(),
                },
            );
        }
        styles
    }

    /// A style's properties with its `basedOn` chain, nearest first: Word
    /// resets a toggle a style sets instead of XORing it (MS-OI29500
    /// §2.1.258).
    pub(super) fn chain(&mut self, id: &str) -> Rc<(PPr, RPr)> {
        if let Some(found) = self.chains.get(id) {
            return found.clone();
        }
        let mut ppr = PPr::default();
        let mut rpr = RPr::default();
        for def in self.lineage(id).into_iter().rev() {
            ppr.merge(&def.ppr);
            rpr.merge(&def.rpr);
        }
        let out = Rc::new((ppr, rpr));
        self.chains.insert(id.to_string(), out.clone());
        out
    }

    /// A table style's cell properties with its `basedOn` chain.
    pub(super) fn cell_props(&self, id: Option<&str>) -> CellProps {
        let mut out = CellProps::default();
        let Some(id) = id else {
            return out;
        };
        for def in self.lineage(id).into_iter().rev() {
            out.merge(&def.cell);
        }
        out
    }

    /// The marks of a table style and its `basedOn` chain.
    #[cfg(feature = "pdf")]
    pub(super) fn table_marks(&self, id: Option<&str>) -> TableMarks {
        id.map_or_else(TableMarks::default, |id| {
            self.lineage(id)
                .into_iter()
                .fold(TableMarks::default(), |marks, def| marks.or(def.marks))
        })
    }

    /// The style and its bases, nearest first.
    fn lineage(&self, id: &str) -> Vec<&StyleDef> {
        let mut ids: Vec<&str> = Vec::new();
        let mut current = Some(id);
        while let Some(cur) = current {
            if ids.contains(&cur) || ids.len() > 32 {
                break;
            }
            ids.push(cur);
            current = self.defs.get(cur).and_then(|d| d.based_on.as_deref());
        }
        ids.iter().filter_map(|id| self.defs.get(*id)).collect()
    }

    /// A table style's cell margins and borders with its `basedOn` chain.
    pub(super) fn table_look(&self, id: Option<&str>) -> ([Option<f64>; 4], [Option<f64>; 3]) {
        let mut margins = [None; 4];
        let mut borders = [None; 3];
        let mut current = id.map(str::to_string);
        let mut seen = 0;
        while let Some(cur) = current {
            let Some(def) = self.defs.get(&cur) else {
                break;
            };
            for (slot, value) in margins.iter_mut().zip(def.cell_margins) {
                if slot.is_none() {
                    *slot = value;
                }
            }
            for (slot, value) in borders.iter_mut().zip(def.borders) {
                if slot.is_none() {
                    *slot = value;
                }
            }
            // Word reads a table style with properties of its own before a
            // base style defined after it: the base adds nothing, and the
            // cell margins it leaves unset are 0.
            let later = def
                .based_on
                .as_ref()
                .and_then(|b| self.defs.get(b))
                .is_some_and(|base| base.order > def.order);
            if later && def.table_props {
                for slot in &mut margins {
                    slot.get_or_insert(0.0);
                }
                break;
            }
            current = def.based_on.clone();
            seen += 1;
            if seen > 32 {
                break;
            }
        }
        (margins, borders)
    }
}
