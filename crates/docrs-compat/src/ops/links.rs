//! Ops adapters for upstream `docx.links`.

use docrs::{Document, Error, Result};
use serde_json::Value;

use super::{OpFn, Step};

fn add_hyperlink(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let address = step
        .param("address", 1)
        .as_str()
        .ok_or_else(|| Error::invalid_argument("bad_case", "address is required"))?
        .to_string();
    let span = super::span(doc, step.param("span", 0))?;
    Ok(doc.add_hyperlink(&span, &address)?.to_json())
}

/// `ext.p1_link_stale_span`: a span captured before another edit of the
/// same text is refused as stale.
fn link_stale_span(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let span = super::span(doc, step.arg(0))?;
    let other = super::span(doc, step.arg(0))?;
    let replacement = step.arg(1).as_str().unwrap_or("");
    doc.replace(&other, replacement, &docrs::ReplaceOptions::default())?;
    let address = step.arg(2).as_str().unwrap_or("");
    Ok(doc.add_hyperlink(&span, address)?.to_json())
}

/// Adapter for an upstream `links.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "links.add_hyperlink" => Some(add_hyperlink),
        "ext.p1_link_stale_span" => Some(link_stale_span),
        _ => None,
    }
}
