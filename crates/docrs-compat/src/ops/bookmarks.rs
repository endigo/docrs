//! Ops adapters for upstream `docx.bookmarks`.

use docrs::{Document, Error, Result};
use serde_json::Value;

use super::{OpFn, Step};

fn name_arg<'a>(step: &'a Step<'_>, index: usize) -> Result<&'a str> {
    step.param("name", index)
        .as_str()
        .ok_or_else(|| Error::invalid_argument("bad_case", "name is required"))
}

fn list_bookmarks(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(Value::Array(
        doc.bookmarks()?.iter().map(|b| b.to_json()).collect(),
    ))
}

fn create_bookmark(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let mut span = super::span(doc, step.param("span", 0))?;
    let name = name_arg(step, 1)?;
    Ok(doc.create_bookmark(&mut span, name)?.to_json())
}

fn delete_bookmark(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let name = name_arg(step, 0)?;
    doc.delete_bookmark(name)?;
    Ok(Value::Null)
}

/// `ext.p1_bookmark_then_footnote`: one span object bookmarked, then
/// footnoted (the span must stay live across the first commit).
fn bookmark_then_footnote(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let mut span = super::span(doc, step.arg(0))?;
    let name = name_arg(step, 1)?;
    let info = doc.create_bookmark(&mut span, name)?;
    let text = step.arg(2).as_str().unwrap_or("");
    let id = doc.add_footnote(&mut span, text)?;
    Ok(serde_json::json!([info.to_json(), id]))
}

/// Adapter for an upstream `bookmarks.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "bookmarks.list_bookmarks" => Some(list_bookmarks),
        "bookmarks.create_bookmark" => Some(create_bookmark),
        "bookmarks.delete_bookmark" => Some(delete_bookmark),
        "ext.p1_bookmark_then_footnote" => Some(bookmark_then_footnote),
        _ => None,
    }
}
