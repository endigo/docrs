//! Effective paragraph, run and table formatting, resolved once for layout
//! and the PDF preflight alike.

use super::Align;
use super::cascade::TableLook;
#[cfg(feature = "pdf")]
use super::marks::TableMarks;
use super::numbering::Numbering;
use super::props::{PPr, RPr, read_ppr, read_rpr};
use super::styles::Styles;
use super::table::{TableCtx, TableFormat};
use super::theme::{Theme, read_theme};
use crate::wml::{compat_setting, w_attr, w_child};
use crate::xml::{NodeId, XmlTree};

pub(crate) struct Formatting {
    pub(super) styles: Styles,
    pub(super) numbering: Numbering,
    /// Its fonts for layout, its colours for the properties read on the way.
    pub(super) theme: Theme,
    /// `overrideTableStyleFontSizeAndJustification`: the standard's style
    /// order in tables, without Word 2007's exception for Normal.
    table_style_order: bool,
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
        settings: Option<&XmlTree>,
    ) -> Formatting {
        let theme = read_theme(theme);
        Formatting {
            styles: Styles::read(styles, &theme.palette),
            numbering: Numbering::read(numbering, &theme.palette),
            theme,
            table_style_order: settings
                .and_then(|s| compat_setting(s, "overrideTableStyleFontSizeAndJustification"))
                .is_some_and(|v| matches!(v, "1" | "true" | "on")),
        }
    }

    pub(crate) fn paragraph(
        &mut self,
        tree: &XmlTree,
        p: NodeId,
        table: Option<&TableCtx>,
    ) -> ParaFormat {
        let ppr_node = w_child(tree, p, "pPr");
        let direct = read_ppr(tree, ppr_node, &self.theme.palette);
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
        let table = table.map(|table| table.style.props(table.parts));
        if let Some((table_ppr, table_rpr)) = &table {
            ppr.merge(table_ppr);
            rpr.layer(table_rpr);
        }
        let chain = self.styles.chain(style);
        ppr.merge(&chain.0);
        rpr.layer(&chain.1);
        if let Some((table_ppr, table_rpr)) = &table
            && !self.table_style_order
        {
            self.normal_gives_way(style, table_ppr, table_rpr, &mut ppr, &mut rpr);
        }
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

    /// Word 2007's exception: an 11 or 12 point size, or left alignment,
    /// that the paragraph has from the default paragraph style gives way to
    /// the table style's.
    fn normal_gives_way(
        &self,
        style: &str,
        table_ppr: &PPr,
        table_rpr: &RPr,
        ppr: &mut PPr,
        rpr: &mut RPr,
    ) {
        let normal = self.styles.default_para.as_deref();
        let from_normal = |pick: fn(&PPr, &RPr) -> bool| {
            normal.is_some() && self.styles.set_by(style, pick) == normal
        };
        if table_rpr.size.is_some()
            && matches!(rpr.size, Some(11.0 | 12.0))
            && from_normal(|_, rpr| rpr.size.is_some())
        {
            rpr.size = table_rpr.size;
        }
        if table_ppr.align.is_some()
            && ppr.align == Some(Align::Left)
            && from_normal(|ppr, _| ppr.align.is_some())
        {
            ppr.align = table_ppr.align;
        }
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
        rpr.merge(&read_rpr(tree, rpr_node, &self.theme.palette));
        rpr
    }

    /// A table's style (its own, else the default) and its `w:tblLook`.
    pub(crate) fn table(&mut self, tree: &XmlTree, tbl: NodeId) -> TableFormat {
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let id = tbl_pr
            .and_then(|p| w_child(tree, p, "tblStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string)
            .or_else(|| self.styles.default_table.clone());
        TableFormat {
            style: self.styles.table_style(id.as_deref()),
            #[cfg(feature = "pdf")]
            id,
            look: TableLook::read(tree, tbl_pr.and_then(|p| w_child(tree, p, "tblLook"))),
        }
    }

    /// What a table style and its `basedOn` chain draw around the cells.
    #[cfg(feature = "pdf")]
    pub(crate) fn table_marks(&self, style: Option<&str>) -> TableMarks {
        self.styles.table_marks(style)
    }
}
