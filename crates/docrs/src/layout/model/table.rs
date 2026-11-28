use std::rc::Rc;

use super::cascade::{Bands, Part, Parts, Position, Region, TableLook, parts_of};
use super::colour::{Palette, Rgb};
use super::edges::{Edges, GridCell};
use super::look::{EDGE_SIDES, Edge, VAlign, read_edge};
use super::props::{side, twips};
use super::styles::{TableStyle, band_size};
use super::{Builder, Cell, Row, Table, TableAlign};
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

/// Word's table grid holds at most 63 columns.
const MAX_GRID_COLUMNS: usize = 63;

/// A `w:tcPr`'s look; `Some(None)` is no fill.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CellProps {
    fill: Option<Option<Rgb>>,
    valign: Option<VAlign>,
    /// By `EDGE_SIDES`.
    borders: [Option<Edge>; 6],
}

impl CellProps {
    pub(super) fn merge(&mut self, other: &CellProps) {
        self.fill = other.fill.or(self.fill);
        self.valign = other.valign.or(self.valign);
        set(&mut self.borders, other.borders);
    }

    pub(super) fn clear_borders(&mut self) {
        self.borders = [None; 6];
    }
}

/// Each value `top` sets replaces `base`'s.
fn set<T: Copy, const N: usize>(base: &mut [Option<T>; N], top: [Option<T>; N]) {
    for (slot, value) in base.iter_mut().zip(top) {
        if value.is_some() {
            *slot = value;
        }
    }
}

pub(super) fn read_cell_props(
    tree: &XmlTree,
    tc_pr: Option<NodeId>,
    palette: &Palette,
) -> CellProps {
    let Some(tc_pr) = tc_pr else {
        return CellProps::default();
    };
    CellProps {
        fill: w_child(tree, tc_pr, "shd").map(|shd| palette.shading(tree, shd)),
        valign: w_child(tree, tc_pr, "vAlign").and_then(|v| VAlign::read(w_attr(tree, v, "val"))),
        borders: read_edges(tree, w_child(tree, tc_pr, "tcBorders"), palette),
    }
}

/// `w:tblBorders` or `w:tcBorders` sides, by `EDGE_SIDES`.
pub(super) fn read_edges(
    tree: &XmlTree,
    borders: Option<NodeId>,
    palette: &Palette,
) -> [Option<Edge>; 6] {
    let mut out = [None; 6];
    if let Some(borders) = borders {
        for (slot, names) in out.iter_mut().zip(EDGE_SIDES) {
            *slot = side(tree, borders, names).map(|s| read_edge(tree, s, palette));
        }
    }
    out
}

/// What a cell's paragraphs take from its table.
#[derive(Clone)]
pub(crate) struct TableCtx {
    pub(super) style: Rc<TableStyle>,
    pub(crate) parts: Parts,
}

impl TableCtx {
    pub(crate) fn new(format: &TableFormat, parts: Parts) -> TableCtx {
        TableCtx {
            style: format.style.clone(),
            parts,
        }
    }
}

/// A table's style, by name and resolved, and its `w:tblLook`.
pub(crate) struct TableFormat {
    #[cfg(feature = "pdf")]
    pub(crate) id: Option<String>,
    pub(super) style: Rc<TableStyle>,
    pub(crate) look: TableLook,
}

/// For each of a cell's edges (top, left, bottom, right), the side of
/// `EDGE_SIDES` a format covering `region` gives it: the region's own
/// edge, else its inside one.
fn sides_in(region: Region, row: usize, column: usize) -> [usize; 4] {
    [
        if row == region.top { 0 } else { 4 },
        if column == region.left { 1 } else { 5 },
        if row == region.bottom { 2 } else { 4 },
        if column == region.right { 3 } else { 5 },
    ]
}

/// A cell's candidate edges from cascade levels, nearest first: the first
/// line or `nil` wins, and `none` defers to the next level (MS-OI29500
/// §2.1.169).
fn candidates(levels: &[([Option<Edge>; 6], [usize; 4])]) -> [Edge; 4] {
    std::array::from_fn(|edge| {
        levels
            .iter()
            .find_map(|(sides, map)| sides[map[edge]].filter(|side| *side != Edge::None))
            .unwrap_or(Edge::None)
    })
}

/// A row exception (`w:tblPrEx`): table properties for one row. Word for
/// Mac 16.115 draws no fill from its `w:shd`.
struct Exception {
    margins: [Option<f64>; 4],
    borders: [Option<Edge>; 6],
}

/// What reading a table's rows shares: the table-wide properties, and the
/// grid as the rows extend it.
struct TableReader {
    format: TableFormat,
    palette: Palette,
    margins: [Option<f64>; 4],
    borders: [Option<Edge>; 6],
    fill: Option<Option<Rgb>>,
    bands: Bands,
    /// Column widths.
    grid: Vec<f64>,
    rows: usize,
    /// The fill and alignment of each grid column's open vertical merge.
    merges: Vec<Option<(Option<Rgb>, VAlign)>>,
}

impl TableReader {
    fn new(builder: &mut Builder<'_>, tree: &XmlTree, tbl: NodeId) -> TableReader {
        let palette = builder.fmt.theme.palette.clone();
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let pr = |name: &str| tbl_pr.and_then(|p| w_child(tree, p, name));
        let format = builder.fmt.table(tree, tbl);
        let style = format.style.clone();
        let mut table_margins = style.margins;
        // Word for Mac 16.115: a table on the default table style has 5.4
        // points each side unless that style sets them; one on a style of
        // its own, or in a document with no default table style, has none.
        let default = builder.fmt.styles.default_table.as_deref();
        let own_style = pr("tblStyle")
            .and_then(|s| w_attr(tree, s, "val"))
            .is_some_and(|id| default != Some(id));
        if !own_style && default.is_some() {
            for slot in &mut table_margins[2..] {
                slot.get_or_insert(5.4);
            }
        }
        set(&mut table_margins, margins(tree, pr("tblCellMar")));
        let mut borders = style.borders;
        set(&mut borders, read_edges(tree, pr("tblBorders"), &palette));
        TableReader {
            margins: table_margins,
            borders,
            fill: pr("shd")
                .map(|shd| palette.shading(tree, shd))
                .or(style.fill),
            bands: (
                band_size(tree, tbl_pr, "tblStyleRowBandSize").unwrap_or(style.bands.0),
                band_size(tree, tbl_pr, "tblStyleColBandSize").unwrap_or(style.bands.1),
            ),
            grid: w_child(tree, tbl, "tblGrid")
                .map(|g| {
                    w_children(tree, g, "gridCol")
                        .map(|c| twips(tree, c, "w").unwrap_or(0.0))
                        .take(MAX_GRID_COLUMNS)
                        .collect()
                })
                .unwrap_or_default(),
            rows: w_children(tree, tbl, "tr").count(),
            merges: vec![None; MAX_GRID_COLUMNS],
            format,
            palette,
        }
    }

    fn row(
        &mut self,
        builder: &mut Builder<'_>,
        tree: &XmlTree,
        tr: NodeId,
        index: usize,
    ) -> (Row, Vec<GridCell>) {
        let tr_pr = w_child(tree, tr, "trPr");
        let prop = |name: &str| tr_pr.and_then(|p| w_child(tree, p, name));
        let exception = w_child(tree, tr, "tblPrEx");
        let ex = |name: &str| exception.and_then(|e| w_child(tree, e, name));
        let mut exception = Exception {
            margins: self.margins,
            borders: read_edges(tree, ex("tblBorders"), &self.palette),
        };
        set(&mut exception.margins, margins(tree, ex("tblCellMar")));
        let tcs: Vec<NodeId> = w_children(tree, tr, "tc").collect();
        let mut column = prop("gridBefore")
            .and_then(|g| w_attr(tree, g, "val"))
            .and_then(|v| v.parse::<usize>().ok())
            .map_or(0, |before| before.min(MAX_GRID_COLUMNS - 1));
        let mut cells = Vec::with_capacity(tcs.len());
        let mut grid_row = Vec::with_capacity(tcs.len());
        for (cell_index, &tc) in tcs.iter().enumerate() {
            let at = Position {
                row: index,
                rows: self.rows,
                column: cell_index,
                columns: tcs.len(),
            };
            let (cell, grid_cell) = self.cell(builder, tree, tc, at, column, &exception);
            column += grid_cell.span;
            cells.push(cell);
            grid_row.push(grid_cell);
        }
        let flag = |name: &str| prop(name).is_some_and(|n| super::on_off(tree, n));
        let row = Row {
            cells,
            height: prop("trHeight").and_then(|h| {
                twips(tree, h, "val").map(|v| (v, w_attr(tree, h, "hRule") == Some("exact")))
            }),
            border_top: 0.0,
            border_bottom: 0.0,
            cant_split: flag("cantSplit"),
            header: flag("tblHeader"),
            node: Some(tr),
        };
        (row, grid_row)
    }

    fn cell(
        &mut self,
        builder: &mut Builder<'_>,
        tree: &XmlTree,
        tc: NodeId,
        at: Position,
        column: usize,
        exception: &Exception,
    ) -> (Cell, GridCell) {
        let tc_pr = w_child(tree, tc, "tcPr");
        let prop = |name: &str| tc_pr.and_then(|p| w_child(tree, p, name));
        let span = prop("gridSpan")
            .and_then(|s| w_attr(tree, s, "val"))
            .and_then(|v| v.parse::<usize>().ok())
            .map_or(1, |span| span.clamp(1, MAX_GRID_COLUMNS));
        let width = self.width(column, span, prop("tcW"), tree);
        let vmerge = match prop("vMerge") {
            None => 0,
            Some(v) if w_attr(tree, v, "val") == Some("restart") => 1,
            Some(_) => 2,
        };
        let regions = self.format.look.parts(at, self.bands);
        let own = read_cell_props(tree, tc_pr, &self.palette);
        let (fill, valign) = self.look(&regions, &own, column, vmerge);
        let (edges, cut) = self.edges(&regions, &own, exception, at);
        let grid_cell = GridCell {
            column,
            span,
            merged_up: vmerge == 2,
            edges,
            cut,
        };
        let mut cell_margins = exception.margins;
        set(&mut cell_margins, margins(tree, prop("tcMar")));
        let ctx = TableCtx::new(&self.format, parts_of(&regions));
        let mut blocks = Vec::new();
        builder.blocks(tree, tc, Some(&ctx), &mut blocks);
        let cell = Cell {
            width,
            margins: cell_margins.map(|margin| margin.unwrap_or(0.0)),
            blocks,
            vmerge,
            column,
            fill,
            valign,
        };
        (cell, grid_cell)
    }

    /// A cell's width from the grid, else its own; the grid grows to hold it.
    fn width(&mut self, column: usize, span: usize, tc_w: Option<NodeId>, tree: &XmlTree) -> f64 {
        let width = if self.grid.len() >= column + span {
            self.grid[column..column + span].iter().sum()
        } else {
            tc_w.and_then(|w| twips(tree, w, "w")).unwrap_or(72.0)
        };
        while self.grid.len() < (column + span).min(MAX_GRID_COLUMNS) {
            self.grid.push(width / span as f64);
        }
        width
    }

    /// A cell's fill and vertical alignment: its own, else its formats'
    /// (the last to apply first), the style's and the table's;
    /// a merge's continuation keeps its first cell's.
    fn look(
        &mut self,
        regions: &[(Part, Region)],
        own: &CellProps,
        column: usize,
        vmerge: u8,
    ) -> (Option<Rgb>, VAlign) {
        let style = &self.format.style;
        let mut props = CellProps {
            fill: self.fill,
            ..CellProps::default()
        };
        props.merge(&style.cell);
        for look in regions.iter().filter_map(|(part, _)| style.part(*part)) {
            props.merge(&look.cell);
        }
        props.merge(own);
        let look = (props.fill.flatten(), props.valign.unwrap_or_default());
        let open = &mut self.merges[column.min(MAX_GRID_COLUMNS - 1)];
        match vmerge {
            1 => *open = Some(look),
            2 => return open.unwrap_or(look),
            _ => *open = None,
        }
        look
    }

    /// A cell's candidate edges: its own, its formats' (the last to apply
    /// first), the row's and the table's; and its top and bottom where a
    /// page break cuts it, which take the table's top and bottom borders
    /// (Word for Mac 16.115).
    fn edges(
        &self,
        regions: &[(Part, Region)],
        own: &CellProps,
        exception: &Exception,
        at: Position,
    ) -> ([Edge; 4], [Edge; 2]) {
        let style = &self.format.style;
        let whole = Region {
            top: 0,
            bottom: at.rows - 1,
            left: 0,
            right: at.columns - 1,
        };
        let table_sides = sides_in(whole, at.row, at.column);
        let mut levels = vec![(own.borders, [0, 1, 2, 3])];
        levels.extend(regions.iter().rev().filter_map(|(part, region)| {
            let look = style.part(*part)?;
            Some((look.cell.borders, sides_in(*region, at.row, at.column)))
        }));
        levels.push((exception.borders, table_sides));
        levels.push((self.borders, table_sides));
        let edges = candidates(&levels);
        let outer = [0, table_sides[1], 2, table_sides[3]];
        let count = levels.len();
        for level in &mut levels[count - 2..] {
            level.1 = outer;
        }
        let [top, _, bottom, _] = candidates(&levels);
        (edges, [top, bottom])
    }
}

/// The children of `tbl` before each row, and after the last.
fn between_rows(tree: &XmlTree, tbl: NodeId) -> Vec<Vec<NodeId>> {
    let mut out = vec![Vec::new()];
    for child in tree.child_elements(tbl) {
        if tree.is(child, W, "tr") {
            out.push(Vec::new());
        } else if let Some(gap) = out.last_mut() {
            gap.push(child);
        }
    }
    out
}

impl Builder<'_> {
    pub(super) fn table(&mut self, tree: &XmlTree, tbl: NodeId) -> Table {
        let mut reader = TableReader::new(self, tree, tbl);
        let between = between_rows(tree, tbl);
        let (mut rows, grid_rows): (Vec<Row>, Vec<Vec<GridCell>>) = w_children(tree, tbl, "tr")
            .enumerate()
            .map(|(index, tr)| {
                for &node in &between[index] {
                    self.loose_bookmarks(tree, node);
                }
                for node in tree.child_elements(tr).filter(|&n| !tree.is(n, W, "tc")) {
                    self.loose_bookmarks(tree, node);
                }
                reader.row(self, tree, tr, index)
            })
            .unzip();
        for &node in between.last().into_iter().flatten() {
            self.loose_bookmarks(tree, node);
        }
        let grid = reader.grid;
        let edges = Edges::resolve(&grid_rows, grid.len());
        border_heights(&mut rows, &edges, grid.len());
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let pr = |name: &str| tbl_pr.and_then(|p| w_child(tree, p, name));
        let align = match pr("jc").and_then(|jc| w_attr(tree, jc, "val")) {
            Some("center") => TableAlign::Center,
            Some("right" | "end") => TableAlign::Right,
            _ => TableAlign::Left,
        };
        let mut indent = pr("tblInd")
            .filter(|ind| w_attr(tree, *ind, "type").is_none_or(|t| t == "dxa"))
            .and_then(|ind| twips(tree, ind, "w"))
            .unwrap_or(0.0);
        if self.outdent_tables && self.depth == 1 && align == TableAlign::Left {
            indent -= rows
                .first()
                .and_then(|row| row.cells.first())
                .map_or(0.0, |cell| cell.margins[2]);
        }
        let mut x = 0.0;
        let lines = std::iter::once(0.0)
            .chain(grid.iter().map(|width| {
                x += width;
                x
            }))
            .collect();
        Table {
            story: self.story,
            rows,
            grid: lines,
            indent,
            align,
            edges,
        }
    }
}

/// Word adds the widest border above each row, and below the last, to its
/// height.
fn border_heights(rows: &mut [Row], edges: &Edges, columns: usize) {
    let widest = |line: usize| {
        (0..columns)
            .filter_map(|column| edges.horizontal(line, column))
            .map(|border| border.width)
            .fold(0.0, f64::max)
    };
    let count = rows.len();
    for (index, row) in rows.iter_mut().enumerate() {
        row.border_top = widest(index);
        if index + 1 == count {
            row.border_bottom = widest(count);
        }
    }
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
