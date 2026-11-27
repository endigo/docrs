use super::props::{length, number, on_off, twips};
use super::{Builder, Cell, Row, Table};
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

/// Word's table grid holds at most 63 columns.
const MAX_GRID_COLUMNS: usize = 63;

#[derive(Clone)]
pub(super) struct TableCtx {
    pub(super) style: Option<String>,
    pub(super) first_row: bool,
}

impl Builder<'_> {
    pub(super) fn table(&mut self, tree: &XmlTree, tbl: NodeId) -> Table {
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let style = tbl_pr
            .and_then(|p| w_child(tree, p, "tblStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string)
            .or_else(|| self.styles.default_table.clone());
        let look_first_row = tbl_pr
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
        let (mut table_margins, mut table_borders) = self.styles.table_look(style.as_deref());
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
                    .filter(|span| *span <= MAX_GRID_COLUMNS)
                    .unwrap_or(1)
                    .max(1);
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
                let mut blocks = Vec::new();
                self.blocks(tree, tc, Some(&ctx), &mut blocks);
                cells.push(Cell {
                    width,
                    margins: [
                        cell_margins[0].unwrap_or(0.0),
                        cell_margins[1].unwrap_or(0.0),
                        cell_margins[2].unwrap_or(5.4),
                        cell_margins[3].unwrap_or(5.4),
                    ],
                    blocks,
                    vmerge,
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
        Table { rows }
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
                    .and_then(|eighths| length(eighths / 8.0))
                    .unwrap_or(0.25),
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
