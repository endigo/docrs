//! Effective paragraph, run and table formatting, resolved once for layout
//! and the PDF preflight alike.

use super::colour::Palette;
#[cfg(feature = "pdf")]
use super::marks::TableMarks;
use super::numbering::Numbering;
use super::props::{PPr, RPr, read_ppr, read_rpr};
use super::styles::Styles;
use super::table::TableCtx;
use super::theme::read_theme;
use crate::wml::{w_attr, w_child};
use crate::xml::{NodeId, XmlTree};

pub(crate) struct Formatting {
    pub(super) styles: Styles,
    pub(super) numbering: Numbering,
    /// The theme's colours, for the properties read on the way.
    pub(super) palette: Palette,
}

/// A paragraph's style, its direct properties, and its effective paragraph
/// and run properties (before the runs' own).
pub(crate) struct ParaFormat {
    pub(crate) style: String,
    pub(crate) direct: PPr,
    pub(crate) ppr: PPr,
    pub(crate) rpr: RPr,
}

impl Formatting {
    pub(crate) fn read(
        styles: Option<&XmlTree>,
        numbering: Option<&XmlTree>,
        theme: Option<&XmlTree>,
    ) -> Formatting {
        let palette = read_theme(theme).palette;
        Formatting {
            styles: Styles::read(styles, &palette),
            numbering: Numbering::read(numbering, &palette),
            palette,
        }
    }

    pub(crate) fn paragraph(
        &mut self,
        tree: &XmlTree,
        p: NodeId,
        table: Option<&TableCtx>,
    ) -> ParaFormat {
        let ppr_node = w_child(tree, p, "pPr");
        let direct = read_ppr(tree, ppr_node, &self.palette);
        let style = ppr_node
            .and_then(|pp| w_child(tree, pp, "pStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string)
            .or_else(|| self.styles.default_para.clone())
            .unwrap_or_default();
        let (ppr, rpr) = self.para_props(&style, &direct, table);
        ParaFormat {
            style,
            direct,
            ppr,
            rpr,
        }
    }

    fn para_props(&mut self, style: &str, direct: &PPr, table: Option<&TableCtx>) -> (PPr, RPr) {
        let mut ppr = self.styles.default_ppr.clone();
        let mut rpr = self.styles.default_rpr.clone();
        if let Some(table) = table
            && let Some(id) = &table.style
        {
            let chain = self.styles.chain(id);
            ppr.merge(&chain.0);
            rpr.layer(&chain.1);
            if table.first_row
                && let Some((p, r)) = self.styles.defs.get(id).and_then(|d| d.first_row.as_ref())
            {
                ppr.merge(p);
                rpr.layer(r);
            }
        }
        let chain = self.styles.chain(style);
        ppr.merge(&chain.0);
        rpr.layer(&chain.1);
        // Numbering's indents sit between the style and direct formatting.
        let num = direct.num.clone().or_else(|| ppr.num.clone());
        if let Some((id, ilvl)) = num {
            let ilvl = direct.ilvl.or(ilvl).or(ppr.ilvl).unwrap_or(0);
            if let Some(level) = self.numbering.level(&id, ilvl) {
                ppr.merge(&level.ppr);
            }
        }
        ppr.merge(direct);
        (ppr, rpr)
    }

    /// A run's effective properties over its paragraph's.
    pub(crate) fn run(&mut self, tree: &XmlTree, r: NodeId, para_rpr: &RPr) -> RPr {
        let rpr_node = w_child(tree, r, "rPr");
        let mut rpr = para_rpr.clone();
        if let Some(char_style) = rpr_node
            .and_then(|n| w_child(tree, n, "rStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
        {
            let chain = self.styles.chain(char_style);
            rpr.layer(&chain.1);
        }
        rpr.merge(&read_rpr(tree, rpr_node, &self.palette));
        rpr
    }

    /// A table's style (its own, else the default) and whether its first
    /// row takes the style's first-row look.
    pub(crate) fn table(&self, tree: &XmlTree, tbl: NodeId) -> (Option<String>, bool) {
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let style = tbl_pr
            .and_then(|p| w_child(tree, p, "tblStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string)
            .or_else(|| self.styles.default_table.clone());
        let first_row = tbl_pr
            .and_then(|p| w_child(tree, p, "tblLook"))
            .map(|l| {
                w_attr(tree, l, "firstRow")
                    .map(|v| v == "1" || v == "true")
                    .unwrap_or_else(|| {
                        w_attr(tree, l, "val")
                            .and_then(|v| u32::from_str_radix(v, 16).ok())
                            .is_some_and(|v| v & 0x20 != 0)
                    })
            })
            .unwrap_or(true);
        (style, first_row)
    }

    /// What a table style and its `basedOn` chain draw around the cells.
    #[cfg(feature = "pdf")]
    pub(crate) fn table_marks(&self, style: Option<&str>) -> TableMarks {
        self.styles.table_marks(style)
    }
}
