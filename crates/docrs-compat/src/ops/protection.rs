//! Ops adapters for upstream `docx.protection`.

use docrs::{Document, Error, Result};
use serde_json::Value;

use super::{OpFn, Step};

fn status_json(status: docrs::protection::ProtectionStatus) -> Value {
    serde_json::json!({
        "edit": status.edit,
        "formatting": status.formatting,
        "enforced": status.enforced,
        "acknowledged": status.acknowledged,
    })
}

fn set_protection(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let edit = step
        .param("edit", 0)
        .as_str()
        .ok_or_else(|| Error::invalid_argument("bad_case", "edit is required"))?;
    doc.set_protection(edit).map(status_json)
}

fn protection_status(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    doc.protection_status().map(status_json)
}

fn acknowledge_protection(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    doc.acknowledge_protection().map(status_json)
}

/// Adapter for an upstream `protection.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "protection.set_protection" => Some(set_protection),
        "protection.protection_status" => Some(protection_status),
        "protection.acknowledge_protection" => Some(acknowledge_protection),
        _ => None,
    }
}
