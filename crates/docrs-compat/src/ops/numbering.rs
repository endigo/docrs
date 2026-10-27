//! Ops adapters for upstream `docx.numbering`.

use docrs::{Document, Result};
use serde_json::Value;

use super::{OpFn, Step, bad, target};

fn int(value: &Value, name: &str) -> Result<i64> {
    value
        .as_i64()
        .ok_or_else(|| bad(format!("{name} must be an integer, got {value}")))
}

fn list_numbering(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    doc.list_numbering().map(|report| report.to_json())
}

fn apply_numbering(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    let num_id = int(step.param("num_id", 1), "num_id")?;
    let level = match step.param("level", 2) {
        Value::Null => 0,
        value => int(value, "level")?,
    };
    doc.apply_numbering(paragraph.as_target(), num_id, level)?;
    Ok(Value::Null)
}

fn apply_list_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    let name = step
        .param("style_name", 1)
        .as_str()
        .ok_or_else(|| bad("style_name is required"))?
        .to_string();
    doc.apply_list_style(paragraph.as_target(), &name)?;
    Ok(Value::Null)
}

fn ensure_bullet(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    doc.ensure_bullet_definition().map(Value::from)
}

fn ensure_decimal(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    doc.ensure_decimal_definition().map(Value::from)
}

fn restart(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let num_id = int(step.param("num_id", 0), "num_id")?;
    doc.restart_numbering(num_id).map(Value::from)
}

/// Adapter for an upstream `numbering.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "numbering.list_numbering" => Some(list_numbering),
        "numbering.apply_numbering" => Some(apply_numbering),
        "numbering.apply_list_style" => Some(apply_list_style),
        "numbering.ensure_bullet_definition" => Some(ensure_bullet),
        "numbering.ensure_decimal_definition" => Some(ensure_decimal),
        "numbering.restart_numbering" => Some(restart),
        _ => None,
    }
}
