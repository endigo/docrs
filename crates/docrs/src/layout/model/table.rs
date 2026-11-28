use super::colour::{Palette, Rgb};
use super::look::{Border, BorderSource, CellBorders, VAlign, read_border};
use super::props::{length, merge_sides, number, on_off, side, twips};
use super::{Builder, Cell, Row, Table};
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

/// Word's table grid holds at most 63 columns.
const MAX_GRID_COLUMNS: usize = 63;

/// A `w:tcPr`'s look; `Some(None)` is no fill, or a `none` border.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CellProps {
    fill: Option<Option<Rgb>>,
    valign: Option<VAlign>,
    borders: [Option<Option<Border>>; 6],
}

impl CellProps {
    pub(super) fn merge(&mut self, other: &CellProps) {
        self.fill = other.fill.or(self.fill);
        self.valign = other.valign.or(self.valign);
        merge_sides(&mut self.borders, &other.borders);
    }

    /// The cell's own properties over its table style's.
    fn over(&self, style: &CellProps) -> (Option<Rgb>, VAlign, CellBorders) {
        let sourced = |direct: Option<Option<Border>>, style: Option<Option<Border>>| match direct {
            Some(own) => Some((own, BorderSource::Direct)),
            None => style.map(|b| (b, BorderSource::Style)),
        };
        let sides = std::array::from_fn(|i| sourced(self.borders[i], style.borders[i]));
        (
            self.fill.or(style.fill).flatten(),
            self.valign.or(style.valign).unwrap_or_default(),
            CellBorders::from_sides(sides),
        )
    }
}

pub(super) fn read_cell_props(
    tree: &XmlTree,
    tc_pr: Option<NodeId>,
    palette: &Palette,
) -> CellProps {
    let mut out = CellProps::default();
    let Some(tc_pr) = tc_pr else {
        return out;
    };
    out.fill = w_child(tree, tc_pr, "shd").map(|shd| palette.shading(tree, shd));
    out.valign = w_child(tree, tc_pr, "vAlign").and_then(|v| VAlign::read(w_attr(tree, v, "val")));
    if let Some(borders) = w_child(tree, tc_pr, "tcBorders") {
        for (slot, names) in out.borders.iter_mut().zip(CellBorders::SIDES) {
            *slot = side(tree, borders, names).map(|s| read_border(tree, s, palette));
        }
    }
    out
}

#[derive(Clone)]
pub(crate) struct TableCtx {
    pub(crate) style: Option<String>,
    pub(crate) first_row: bool,
}

impl Builder<'_> {
    pub(super) fn table(&mut self, tree: &XmlTree, tbl: NodeId) -> Table {
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let (style, look_first_row) = self.fmt.table(tree, tbl);
        let (mut table_margins, mut table_borders) = self.fmt.styles.table_look(style.as_deref());
        let (default_margins, _) = self
            .fmt
            .styles
            .table_look(self.fmt.styles.default_table.as_deref());
        // Word's 5.4-point side margins come from Table Normal: a document
        // without a style that sets them has none.
        for (slot, value) in table_margins.iter_mut().zip(default_margins) {
            if slot.is_none() {
                *slot = value;
            }
        }
        let style_cell = self.fmt.styles.cell_props(style.as_deref());
        let direct = margins(tree, tbl_pr.and_then(|p| w_child(tree, p, "tblCellMar")));
        for (slot, value) in table_margins.iter_mut().zip(direct) {
            if value.is_some() {
                *slot = value;
            }
        }
        let direct = borders(tree, tbl_pr.and_then(|p| w_child(tree, p, "tblBorders")));
        for (slot, value) in table_borders.iter_mut().zip(direct) {
            if value.is_some() {
                *slot = value;
            }
        }
        let [table_top, table_bottom, inside] = table_borders.map(|b| b.unwrap_or(0.0));
        let row_count = w_children(tree, tbl, "tr").count();
        // Explicit bottom borders of the previous row's cells.
        let mut above_next = 0.0f64;
        let grid: Vec<f64> = w_child(tree, tbl, "tblGrid")
            .map(|g| {
                w_children(tree, g, "gridCol")
                    .map(|c| twips(tree, c, "w").unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();
        let mut rows = Vec::new();
        for (row_index, tr) in w_children(tree, tbl, "tr").enumerate() {
            let tr_pr = w_child(tree, tr, "trPr");
            let height = tr_pr
                .and_then(|p| w_child(tree, p, "trHeight"))
                .and_then(|h| {
                    twips(tree, h, "val").map(|v| (v, w_attr(tree, h, "hRule") == Some("exact")))
                });
            let flag = |name: &str| {
                tr_pr
                    .and_then(|p| w_child(tree, p, name))
                    .is_some_and(|n| on_off(tree, n))
            };
            let ctx = TableCtx {
                style: style.clone(),
                first_row: row_index == 0 && look_first_row,
            };
            let mut cells = Vec::new();
            let mut column = 0;
            let last = row_index + 1 == row_count;
            let mut border_top = above_next;
            let mut border_bottom = 0.0f64;
            above_next = 0.0;
            for tc in w_children(tree, tr, "tc") {
                let tc_pr = w_child(tree, tc, "tcPr");
                let [top, bottom, _] =
                    borders(tree, tc_pr.and_then(|p| w_child(tree, p, "tcBorders")));
                border_top =
                    border_top.max(top.unwrap_or(if row_index == 0 { table_top } else { inside }));
                if last {
                    border_bottom = border_bottom.max(bottom.unwrap_or(table_bottom));
                } else if let Some(bottom) = bottom {
                    above_next = above_next.max(bottom);
                }
                let span = tc_pr
                    .and_then(|p| w_child(tree, p, "gridSpan"))
                    .and_then(|s| w_attr(tree, s, "val"))
                    .and_then(|v| v.parse::<usize>().ok())
                    .map_or(1, |span| span.clamp(1, MAX_GRID_COLUMNS));
                let width: f64 = if grid.len() >= column + span {
                    grid[column..column + span].iter().sum()
                } else {
                    tc_pr
                        .and_then(|p| w_child(tree, p, "tcW"))
                        .and_then(|w| twips(tree, w, "w"))
                        .unwrap_or(72.0)
                };
                column += span;
                let mut cell_margins = table_margins;
                let own = margins(tree, tc_pr.and_then(|p| w_child(tree, p, "tcMar")));
                for (slot, value) in cell_margins.iter_mut().zip(own) {
                    if value.is_some() {
                        *slot = value;
                    }
                }
                let vmerge = match tc_pr.and_then(|p| w_child(tree, p, "vMerge")) {
                    None => 0,
                    Some(v) if w_attr(tree, v, "val") == Some("restart") => 1,
                    Some(_) => 2,
                };
                let (fill, valign, cell_borders) =
                    read_cell_props(tree, tc_pr, &self.fmt.theme.palette).over(&style_cell);
                let mut blocks = Vec::new();
                self.blocks(tree, tc, Some(&ctx), &mut blocks);
                cells.push(Cell {
                    width,
                    margins: cell_margins.map(|margin| margin.unwrap_or(0.0)),
                    blocks,
                    vmerge,
                    fill,
                    valign,
                    borders: cell_borders,
                });
            }
            rows.push(Row {
                cells,
                height,
                border_top,
                border_bottom,
                cant_split: flag("cantSplit"),
                header: flag("tblHeader"),
                node: Some(tr),
            });
        }
        Table {
            story: self.story,
            rows,
        }
    }
}

/// Border widths in points from `w:tblBorders` or `w:tcBorders`: top,
/// bottom and inside horizontal (`none` and `nil` are 0).
pub(super) fn borders(tree: &XmlTree, node: Option<NodeId>) -> [Option<f64>; 3] {
    let mut out = [None; 3];
    if let Some(node) = node {
        for (slot, name) in out.iter_mut().zip(["top", "bottom", "insideH"]) {
            *slot = w_child(tree, node, name).map(|side| match w_attr(tree, side, "val") {
                Some("none" | "nil") | None => 0.0,
                Some(_) => w_attr(tree, side, "sz")
                    .and_then(number)
                    .map_or(0.25, |eighths| length(eighths / 8.0)),
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
