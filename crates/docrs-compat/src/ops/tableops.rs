//! Ops adapters for upstream `docx.tableops`.
//!
//! `{"$table": "near text", "match"?}` decodes through `find_table`, as the
//! oracle does; a returned table encodes as `{"type": "Table", "rows":
//! [[cell text…]…]}` (see `tools/oracle/ext_p3.py`).

use docrs::replace::ReplaceReport;
use docrs::tableops::{Table, UpdateCellOptions};
use docrs::{Document, Error, MatchPolicy, RefusalKind, Result};
use serde_json::{Value, json};

use super::{OpFn, Step, bad, date};

fn policy(value: &Value) -> Result<MatchPolicy> {
    match value {
        Value::Null => Ok(MatchPolicy::Exact),
        Value::String(token) => MatchPolicy::parse(token),
        other => Err(bad(format!("match must be a string, got {other}"))),
    }
}

fn table(doc: &Document, value: &Value) -> Result<Table> {
    let near = value["$table"]
        .as_str()
        .ok_or_else(|| bad(format!("expected $table, got {value}")))?;
    doc.find_table(near, policy(&value["match"])?)
}

fn table_json(doc: &Document, table: &Table) -> Result<Value> {
    Ok(json!({"type": "Table", "rows": doc.table_grid(table)?}))
}

/// Upstream `to_dict()` of a `ReplaceResult` (no Rust-only `changed`).
fn report_json(report: &ReplaceReport) -> Value {
    let mut value = report.to_json();
    if let Some(map) = value.as_object_mut() {
        map.remove("changed");
    }
    value
}

/// A 0-based index argument; negative indices are out of range upstream.
fn index(value: &Value, what: &str) -> Result<std::result::Result<usize, i64>> {
    let number = value
        .as_i64()
        .ok_or_else(|| bad(format!("{what} must be an integer, got {value}")))?;
    Ok(if number < 0 {
        Err(number)
    } else {
        Ok(number as usize)
    })
}

fn negative_row(row: i64, doc: &Document, table: &Table) -> Result<Error> {
    let count = doc.table_grid(table)?.len() as i64;
    Ok(Error::refusal(
        RefusalKind::TargetNotFound,
        "row_out_of_range",
        format!("row {row} does not exist (0..{})", count - 1),
    ))
}

fn find_table(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let near = step
        .param("near_text", 0)
        .as_str()
        .ok_or_else(|| bad("near_text is required"))?;
    let found = doc.find_table(near, policy(step.param("match", 1))?)?;
    table_json(doc, &found)
}

fn update_cell(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let handle = table(doc, step.param("table", 0))?;
    let new_text = step
        .param("new_text", 3)
        .as_str()
        .ok_or_else(|| bad("new_text is required"))?;
    let options = UpdateCellOptions {
        tracked: step.kwarg("tracked").as_bool().unwrap_or(false),
        author: step.kwarg("author").as_str().map(str::to_string),
        date: date(step.kwarg("date"))?,
    };
    let row = index(step.param("row", 1), "row")?;
    let column = index(step.param("column", 2), "column")?;
    match (row, column) {
        (Ok(row), Ok(column)) => {
            let report = doc.update_cell(&handle, row, column, new_text, &options)?;
            Ok(report_json(&report))
        }
        (Err(row), _) => Err(negative_row(row, doc, &handle)?),
        (Ok(row), Err(column)) => Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "column_has_no_cell",
            format!("layout-grid column {column} has no cell in row {row}"),
        )),
    }
}

fn insert_row_after(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let handle = table(doc, step.param("table", 0))?;
    let values = super::strings(step.param("values", 2));
    let copy_format_from = match step.kwarg("copy_format_from") {
        Value::Null => None,
        value => match index(value, "copy_format_from")? {
            Ok(index) => Some(index),
            Err(negative) => {
                return Err(Error::refusal(
                    RefusalKind::TargetNotFound,
                    "copy_format_row_out_of_range",
                    format!("copy_format_from row {negative} does not exist"),
                ));
            }
        },
    };
    match index(step.param("row", 1), "row")? {
        Ok(row) => {
            doc.insert_row_after(&handle, row, &values, copy_format_from)?;
            Ok(Value::Null)
        }
        Err(row) => Err(negative_row(row, doc, &handle)?),
    }
}

fn delete_row(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let handle = table(doc, step.param("table", 0))?;
    match index(step.param("row", 1), "row")? {
        Ok(row) => {
            doc.delete_row(&handle, row)?;
            Ok(Value::Null)
        }
        Err(row) => Err(negative_row(row, doc, &handle)?),
    }
}

fn table_grid(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let handle = table(doc, step.param("table", 0))?;
    Ok(json!(doc.table_grid(&handle)?))
}

/// Adapter for an upstream `tableops.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "tableops.find_table" => Some(find_table),
        "tableops.update_cell" => Some(update_cell),
        "tableops.insert_row_after" => Some(insert_row_after),
        "tableops.delete_row" => Some(delete_row),
        "ext.p3_table_grid" => Some(table_grid),
        _ => None,
    }
}
