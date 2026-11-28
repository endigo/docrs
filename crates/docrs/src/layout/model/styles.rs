use std::collections::HashMap;
use std::rc::Rc;

use super::cascade::{Bands, Part, PartLook, Parts};
use super::colour::{Palette, Rgb};
use super::look::Edge;
#[cfg(feature = "pdf")]
use super::marks::TableMarks;
use super::props::{PPr, RPr, read_ppr, read_rpr};
use super::table::{CellProps, margins, read_cell_props, read_edges};
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

pub(super) struct StyleDef {
    based_on: Option<String>,
    ppr: PPr,
    rpr: RPr,
    /// Table styles: whole-table cell margins, borders, fill and band
    /// sizes, and the conditional formats.
    cell_margins: [Option<f64>; 4],
    borders: [Option<Edge>; 6],
    fill: Option<Option<Rgb>>,
    bands: (Option<usize>, Option<usize>),
    parts: Vec<(Part, PartLook)>,
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
    tables: HashMap<Option<String>, Rc<TableStyle>>,
}

/// A table style resolved through its `basedOn` chain.
#[derive(Debug, Default)]
pub(crate) struct TableStyle {
    pub(super) ppr: PPr,
    pub(super) rpr: RPr,
    /// Whole-table cell properties; Word reads no borders there
    /// (MS-OI29500 §2.1.255).
    pub(super) cell: CellProps,
    /// By `Part`, base styles' first.
    parts: [Option<PartLook>; 12],
    pub(super) margins: [Option<f64>; 4],
    pub(super) borders: [Option<Edge>; 6],
    pub(super) fill: Option<Option<Rgb>>,
    pub(super) bands: Bands,
}

impl TableStyle {
    pub(super) fn part(&self, part: Part) -> Option<&PartLook> {
        self.parts[part as usize].as_ref()
    }

    /// Paragraph and run properties of a cell `parts` reach: the whole
    /// table's, then each format's in order.
    pub(super) fn props(&self, parts: Parts) -> (PPr, RPr) {
        let mut ppr = self.ppr.clone();
        let mut rpr = self.rpr.clone();
        for part in parts.iter().filter_map(|part| self.part(part)) {
            ppr.merge(&part.ppr);
            rpr.merge(&part.rpr);
        }
        (ppr, rpr)
    }
}

/// A band size is an `ST_DecimalNumber`, a 32-bit integer.
pub(super) fn band_size(tree: &XmlTree, tbl_pr: Option<NodeId>, name: &str) -> Option<usize> {
    tbl_pr
        .and_then(|p| w_child(tree, p, name))
        .and_then(|b| w_attr(tree, b, "val"))
        .and_then(|v| v.parse::<usize>().ok())
        .map(|size| size.clamp(1, i32::MAX as usize))
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
            tables: HashMap::new(),
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
            let mut whole = PartLook {
                ppr: read_ppr(tree, w_child(tree, style, "pPr"), palette),
                rpr: read_rpr(tree, w_child(tree, style, "rPr"), palette),
                cell: read_cell_props(tree, w_child(tree, style, "tcPr"), palette),
            };
            let mut parts = Vec::new();
            for conditional in w_children(tree, style, "tblStylePr") {
                let look = PartLook {
                    ppr: read_ppr(tree, w_child(tree, conditional, "pPr"), palette),
                    rpr: read_rpr(tree, w_child(tree, conditional, "rPr"), palette),
                    cell: read_cell_props(tree, w_child(tree, conditional, "tcPr"), palette),
                };
                match w_attr(tree, conditional, "type") {
                    Some("wholeTable") => whole.merge(&look),
                    Some(name) => parts.extend(Part::read(name).map(|part| (part, look))),
                    None => {}
                }
            }
            styles.defs.insert(
                id.to_string(),
                StyleDef {
                    based_on: w_child(tree, style, "basedOn")
                        .and_then(|b| w_attr(tree, b, "val"))
                        .map(str::to_string),
                    ppr: whole.ppr,
                    rpr: whole.rpr,
                    cell_margins: margins(
                        tree,
                        tbl_ppr.and_then(|t| w_child(tree, t, "tblCellMar")),
                    ),
                    borders: read_edges(
                        tree,
                        tbl_ppr.and_then(|t| w_child(tree, t, "tblBorders")),
                        palette,
                    ),
                    fill: tbl_ppr
                        .and_then(|t| w_child(tree, t, "shd"))
                        .map(|shd| palette.shading(tree, shd)),
                    bands: (
                        band_size(tree, tbl_ppr, "tblStyleRowBandSize"),
                        band_size(tree, tbl_ppr, "tblStyleColBandSize"),
                    ),
                    parts,
                    #[cfg(feature = "pdf")]
                    marks: TableMarks::read(tree, style),
                    cell: whole.cell,
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

    /// A table style with its `basedOn` chain; an empty one for `None`.
    pub(super) fn table_style(&mut self, id: Option<&str>) -> Rc<TableStyle> {
        let key = id.map(str::to_string);
        if let Some(found) = self.tables.get(&key) {
            return found.clone();
        }
        let mut style = TableStyle {
            bands: (1, 1),
            ..TableStyle::default()
        };
        if let Some(id) = id {
            let chain = self.chain(id);
            (style.ppr, style.rpr) = (chain.0.clone(), chain.1.clone());
            let lineage = self.lineage(id);
            for def in lineage.iter().rev() {
                style.cell.merge(&def.cell);
                for (part, look) in &def.parts {
                    style.parts[*part as usize]
                        .get_or_insert_with(PartLook::default)
                        .merge(look);
                }
            }
            style.cell.clear_borders();
            style.fill = lineage.iter().find_map(|def| def.fill);
            let size = |pick: fn(&StyleDef) -> Option<usize>| {
                lineage.iter().find_map(|def| pick(def)).unwrap_or(1)
            };
            style.bands = (size(|def| def.bands.0), size(|def| def.bands.1));
            (style.margins, style.borders) = self.table_look(id);
        }
        let style = Rc::new(style);
        self.tables.insert(key, style.clone());
        style
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

    /// The nearest style of `id`'s chain whose own properties `pick`.
    pub(super) fn set_by<'s>(
        &'s self,
        id: &'s str,
        pick: fn(&PPr, &RPr) -> bool,
    ) -> Option<&'s str> {
        self.lineage_ids(id).into_iter().find(|id| {
            self.defs
                .get(*id)
                .is_some_and(|def| pick(&def.ppr, &def.rpr))
        })
    }

    /// The style and its bases, nearest first.
    fn lineage(&self, id: &str) -> Vec<&StyleDef> {
        self.lineage_ids(id)
            .iter()
            .filter_map(|id| self.defs.get(*id))
            .collect()
    }

    fn lineage_ids<'s>(&'s self, id: &'s str) -> Vec<&'s str> {
        let mut ids: Vec<&str> = Vec::new();
        let mut current = Some(id);
        while let Some(cur) = current {
            if ids.contains(&cur) || ids.len() > 32 {
                break;
            }
            ids.push(cur);
            current = self.defs.get(cur).and_then(|d| d.based_on.as_deref());
        }
        ids
    }

    /// A table style's cell margins and borders with its `basedOn` chain.
    fn table_look(&self, id: &str) -> ([Option<f64>; 4], [Option<Edge>; 6]) {
        let mut margins = [None; 4];
        let mut borders = [None; 6];
        let mut current = Some(id.to_string());
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
