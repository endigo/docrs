//! Ops adapters for upstream `docx.notes`.

use docrs::{Document, Error, Result};
use serde_json::{Value, json};

use super::{OpFn, Step};

fn text_arg<'a>(step: &'a Step<'_>) -> Result<&'a str> {
    step.param("text", 1)
        .as_str()
        .ok_or_else(|| Error::invalid_argument("bad_case", "text is required"))
}

fn add_footnote(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let text = text_arg(step)?.to_string();
    let mut span = super::span(doc, step.param("span", 0))?;
    Ok(json!(doc.add_footnote(&mut span, &text)?))
}

fn add_endnote(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let text = text_arg(step)?.to_string();
    let mut span = super::span(doc, step.param("span", 0))?;
    Ok(json!(doc.add_endnote(&mut span, &text)?))
}

/// `ext.p1_stacked_notes`: several notes on one span object, in order
/// (`[kind, text]` pairs; kind `footnote` or `endnote`).
fn stacked_notes(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let mut span = super::span(doc, step.arg(0))?;
    let mut ids = Vec::new();
    for pair in step.arg(1).as_array().cloned().unwrap_or_default() {
        let kind = pair[0].as_str().unwrap_or("footnote");
        let text = pair[1].as_str().unwrap_or("");
        ids.push(if kind == "endnote" {
            doc.add_endnote(&mut span, text)?
        } else {
            doc.add_footnote(&mut span, text)?
        });
    }
    Ok(json!(ids))
}

/// Adapter for an upstream `notes.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "notes.add_footnote" => Some(add_footnote),
        "notes.add_endnote" => Some(add_endnote),
        "ext.p1_stacked_notes" => Some(stacked_notes),
        _ => None,
    }
}
