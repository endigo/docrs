//! `ext.p7_*` adapters: table, row, column and cell properties and cell
//! merging (oracle helpers in `tools/oracle/p7_tables.py`).

use docrs::authoring::{Cell, Table, TableDirection};
use docrs::{Document, Result};
use serde_json::{Value, json};

use crate::ops::inherited::{error_class, index_arg, pick, table};
use crate::ops::{OpFn, Step, bad};

fn rows_json(doc: &Document, target: &Table) -> Result<Value> {
    let mut rows = Vec::new();
    for row in target.rows(doc)? {
        let cells = row
            .cells(doc)?
            .iter()
            .map(|cell| cell.text(doc))
            .collect::<Result<Vec<_>>>()?;
        rows.push(json!({
            "height": row.height(doc)?,
            "height_rule": row.height_rule(doc)?,
            "grid_cols_before": row.grid_cols_before(doc)?,
            "grid_cols_after": row.grid_cols_after(doc)?,
            "cells": cells,
        }));
    }
    let mut columns = Vec::new();
    for column in target.columns(doc)? {
        let cells = column
            .cells(doc)?
            .iter()
            .map(|cell| cell.text(doc))
            .collect::<Result<Vec<_>>>()?;
        columns.push(json!({"width": column.width(doc)?, "cells": cells}));
    }
    Ok(json!({"rows": rows, "columns": columns}))
}

fn table_json(doc: &Document, target: &Table) -> Result<Value> {
    let mut out = rows_json(doc, target)?;
    let direction = target.table_direction(doc)?.map(|d| match d {
        TableDirection::Ltr => "LTR",
        TableDirection::Rtl => "RTL",
    });
    out["alignment"] = json!(target.alignment(doc)?);
    out["autofit"] = json!(target.autofit(doc)?);
    out["direction"] = json!(direction);
    out["style_id"] = json!(target.style_id(doc)?);
    Ok(out)
}

fn rows(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    rows_json(doc, &target)
}

fn cell_json(doc: &Document, cell: &Cell) -> Result<Value> {
    Ok(json!({
        "text": cell.text(doc)?,
        "width": cell.width(doc)?,
        "grid_span": cell.grid_span(doc)?,
        "vertical_alignment": cell.vertical_alignment(doc)?,
        "paragraphs": cell.paragraphs(doc)?.len(),
        "tables": cell.tables(doc)?.len(),
    }))
}

fn usize_arg(value: &Value, what: &str) -> Result<usize> {
    let index = index_arg(value, what)?;
    usize::try_from(index).map_err(|_| bad(format!("negative {what} index")))
}

fn table_props(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    table_json(doc, &target)
}

fn table_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let changes = step
        .param("changes", 1)
        .as_array()
        .ok_or_else(|| bad("changes must be a list"))?;
    for change in changes {
        let value = &change[1];
        match change[0].as_str().unwrap_or("") {
            "alignment" => target.set_alignment(doc, value.as_str())?,
            "autofit" => target.set_autofit(doc, value.as_bool().ok_or_else(|| bad("autofit"))?)?,
            "direction" => target.set_table_direction(
                doc,
                match value.as_str() {
                    None => None,
                    Some("LTR") => Some(TableDirection::Ltr),
                    Some("RTL") => Some(TableDirection::Rtl),
                    Some(other) => return Err(bad(format!("unknown direction {other}"))),
                },
            )?,
            other => return Err(bad(format!("unknown table property {other}"))),
        }
    }
    table_json(doc, &target)
}

fn row_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let row = pick(
        &target.rows(doc)?,
        index_arg(step.param("row", 1), "row")?,
        "row",
    )?;
    let changes = step
        .param("changes", 2)
        .as_array()
        .ok_or_else(|| bad("changes must be a list"))?;
    for change in changes {
        let value = &change[1];
        match change[0].as_str().unwrap_or("") {
            "height" => row.set_height(doc, value.as_i64())?,
            "height_rule" => row.set_height_rule(doc, value.as_str())?,
            other => return Err(bad(format!("unknown row property {other}"))),
        }
    }
    rows_json(doc, &target)
}

fn column_set_width(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let column = pick(
        &target.columns(doc)?,
        index_arg(step.param("col", 1), "column")?,
        "column",
    )?;
    column.set_width(doc, step.param("width", 2).as_i64())?;
    rows_json(doc, &target)
}

fn cells(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let mut out = Vec::new();
    for row in 0..target.row_count(doc)? {
        let cells = target
            .row_cells(doc, row)?
            .iter()
            .map(|cell| cell_json(doc, cell))
            .collect::<Result<Vec<_>>>()?;
        out.push(Value::Array(cells));
    }
    Ok(Value::Array(out))
}

fn column_cells(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let col = usize_arg(step.param("col", 1), "column")?;
    target
        .column_cells(doc, col)?
        .iter()
        .map(|cell| cell.text(doc).map(Value::String))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn cell_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let cell = target.cell(
        doc,
        usize_arg(step.param("row", 1), "row")?,
        usize_arg(step.param("col", 2), "col")?,
    )?;
    let changes = step
        .param("changes", 3)
        .as_array()
        .ok_or_else(|| bad("changes must be a list"))?;
    for change in changes {
        let value = &change[1];
        match change[0].as_str().unwrap_or("") {
            "width" => cell.set_width(doc, value.as_i64().ok_or_else(|| bad("width"))?)?,
            "vertical_alignment" => cell.set_vertical_alignment(doc, value.as_str())?,
            other => return Err(bad(format!("unknown cell property {other}"))),
        }
    }
    cell_json(doc, &cell)
}

fn merge(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = table(doc, step.param("table", 0))?;
    let a = target.cell(
        doc,
        usize_arg(step.param("row", 1), "row")?,
        usize_arg(step.param("col", 2), "col")?,
    )?;
    let b = target.cell(
        doc,
        usize_arg(step.param("row2", 3), "row")?,
        usize_arg(step.param("col2", 4), "col")?,
    )?;
    error_class((|| {
        let merged = a.merge(doc, &b)?;
        Ok(json!({"merged": cell_json(doc, &merged)?, "table": table_json(doc, &target)?}))
    })())
}

/// Adapter for an `ext.p7_*` tables name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p7_table" => table_props,
        "ext.p7_rows" => rows,
        "ext.p7_table_set" => table_set,
        "ext.p7_row_set" => row_set,
        "ext.p7_column_set_width" => column_set_width,
        "ext.p7_cells" => cells,
        "ext.p7_column_cells" => column_cells,
        "ext.p7_cell_set" => cell_set,
        "ext.p7_merge" => merge,
        _ => return None,
    })
}
