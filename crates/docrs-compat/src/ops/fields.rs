//! Ops adapters for upstream `docx.fields` (and `_fieldcode` through the
//! `ext.p2_fieldcode` oracle helper).

use docrs::fields::fieldcode::{self, FieldOperand};
use docrs::{Document, Result};
use serde_json::{Value, json};

use super::{OpFn, Step, bad, strings, target};

fn page_number(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    doc.add_page_number_field(paragraph.as_target())?;
    Ok(Value::Null)
}

fn page_count(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    doc.add_page_count_field(paragraph.as_target())?;
    Ok(Value::Null)
}

fn date_field(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    let format = step.kwarg("date_format").as_str().map(str::to_string);
    doc.add_date_field(paragraph.as_target(), format.as_deref())?;
    Ok(Value::Null)
}

fn reference(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    let bookmark = step
        .kwarg("bookmark")
        .as_str()
        .ok_or_else(|| bad("bookmark is required"))?
        .to_string();
    let kind = step.kwarg("kind").as_str().unwrap_or("text").to_string();
    doc.add_reference_field(paragraph.as_target(), &bookmark, &kind)?;
    Ok(Value::Null)
}

fn caption(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = target(doc, step.param("paragraph", 0))?;
    let label = step.kwarg("label").as_str().unwrap_or("Figure").to_string();
    let description = step.kwarg("description").as_str().unwrap_or("").to_string();
    doc.add_caption(paragraph.as_target(), &label, &description)?;
    Ok(Value::Null)
}

fn toc(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let anchor = target(doc, step.param("anchor", 0))?;
    let levels = match step.kwarg("levels") {
        Value::Null => (1, 3),
        Value::Array(items) if items.len() == 2 => (
            items[0].as_i64().unwrap_or(0),
            items[1].as_i64().unwrap_or(0),
        ),
        other => {
            return Err(bad(format!("levels must be a pair, got {other}")));
        }
    };
    doc.insert_toc_after(anchor.as_target(), levels)?;
    Ok(Value::Null)
}

fn operand_json(operand: &FieldOperand) -> Value {
    json!({"value": operand.value, "start": operand.start, "end": operand.end, "quoted": operand.quoted})
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let items = strings(pair);
            (items.len() == 2).then(|| (items[0].clone(), items[1].clone()))
        })
        .collect()
}

fn borrowed(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect()
}

fn fieldcode_op(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let function = step.arg(0).as_str().unwrap_or("");
    let instruction = step.arg(1).as_str().unwrap_or("").to_string();
    match function {
        "bookmark_operands" => {
            let names = strings(step.arg(2));
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            Ok(Value::Array(
                fieldcode::bookmark_operands(&instruction, &refs)?
                    .iter()
                    .map(operand_json)
                    .collect(),
            ))
        }
        "command_operand" => {
            let command = step.arg(2).as_str().unwrap_or("");
            Ok(fieldcode::command_operand(&instruction, command)?
                .as_ref()
                .map(operand_json)
                .unwrap_or(Value::Null))
        }
        "rewrite_bookmark_operands" => {
            let renames = pairs(step.arg(2));
            fieldcode::rewrite_bookmark_operands(&instruction, &borrowed(&renames)).map(Value::from)
        }
        "rewrite_command_operand" => {
            let command = step.arg(2).as_str().unwrap_or("");
            let renames = pairs(step.arg(3));
            fieldcode::rewrite_command_operand(&instruction, command, &borrowed(&renames))
                .map(Value::from)
        }
        other => Err(bad(format!("unknown fieldcode function {other:?}"))),
    }
}

/// Adapter for an upstream `fields.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "fields.add_page_number_field" => Some(page_number),
        "fields.add_page_count_field" => Some(page_count),
        "fields.add_date_field" => Some(date_field),
        "fields.add_reference_field" => Some(reference),
        "fields.add_caption" => Some(caption),
        "fields.insert_toc_after" => Some(toc),
        "ext.p2_fieldcode" => Some(fieldcode_op),
        _ => None,
    }
}
