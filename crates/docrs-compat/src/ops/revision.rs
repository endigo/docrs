//! Ops adapters for upstream `docx.revision` (oracle helpers `ext.p5_*` in
//! `tools/oracle/ext_p5.py`).

use docrs::revision::{Revision, Revisions};
use docrs::{Document, Error, Result};
use serde_json::{Value, json};

use super::{OpFn, Step, bad};

fn author(step: &Step<'_>) -> Option<String> {
    step.param("author", 0).as_str().map(str::to_string)
}

/// Upstream `_select` in `ext_p5.py`.
pub(crate) fn select<'a>(revisions: &'a Revisions, spec: &Value) -> Result<&'a Revision> {
    let bad = || Error::invalid_argument("bad_case", format!("no revision matches {spec}"));
    if spec.is_null() {
        return revisions.get(0).ok_or_else(bad);
    }
    if let Some(index) = spec.as_u64() {
        return revisions.get(index as usize).ok_or_else(bad);
    }
    let items: Vec<&Revision> = revisions
        .iter()
        .filter(|r| spec["type"].as_str().is_none_or(|t| r.revision_type == t))
        .filter(|r| spec["text"].as_str().is_none_or(|t| r.text.contains(t)))
        .filter(|r| {
            spec["mark"]
                .as_bool()
                .is_none_or(|m| r.is_paragraph_mark == m)
        })
        .filter(|r| spec["author"].as_str().is_none_or(|a| r.author == a))
        .collect();
    items
        .get(spec["nth"].as_u64().unwrap_or(0) as usize)
        .copied()
        .ok_or_else(bad)
}

fn census(revisions: &Revisions) -> Value {
    json!(revisions.remaining_unsupported())
}

fn revisions(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(doc.revisions()?.to_json())
}

fn remaining_unsupported(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(census(&doc.revisions()?))
}

fn accept_all(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let mut snapshot = doc.revisions()?;
    Ok(json!(snapshot.accept_all(doc, author(step).as_deref())?))
}

fn reject_all(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let mut snapshot = doc.revisions()?;
    Ok(json!(snapshot.reject_all(doc, author(step).as_deref())?))
}

fn accept(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let snapshot = doc.revisions()?;
    let revision = select(&snapshot, step.param("select", 0))?.clone();
    revision.accept(doc)?;
    Ok(Value::Null)
}

fn reject(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let snapshot = doc.revisions()?;
    let revision = select(&snapshot, step.param("select", 0))?.clone();
    revision.reject(doc)?;
    Ok(Value::Null)
}

fn remaining_markup(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(json!(docrs::revision::remaining_markup(doc)?))
}

fn stale(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let mut snapshot = doc.revisions()?;
    let first = step.kwarg("first").as_u64().unwrap_or(0) as usize;
    let index = step.kwarg("index").as_u64().unwrap_or(1) as usize;
    let items: Vec<Revision> = snapshot.iter().cloned().collect();
    let bad = || Error::invalid_argument("bad_case", "revision index out of range");
    items.get(first).ok_or_else(bad)?.accept(doc)?;
    match step.kwarg("action").as_str().unwrap_or("accept_all") {
        "accept_all" => Ok(json!(snapshot.accept_all(doc, None)?)),
        "reject_all" => Ok(json!(snapshot.reject_all(doc, None)?)),
        "accept" => items
            .get(index)
            .ok_or_else(bad)?
            .accept(doc)
            .map(|_| Value::Null),
        "reject" => items
            .get(index)
            .ok_or_else(bad)?
            .reject(doc)
            .map(|_| Value::Null),
        other => Err(Error::invalid_argument(
            "bad_case",
            format!("unknown action {other}"),
        )),
    }
}

fn replace(doc: &mut Document, step: &Step<'_>, tracked: bool) -> Result<Value> {
    let span = super::span(doc, step.param("span", 0))?;
    let text = step
        .param("text", 1)
        .as_str()
        .ok_or_else(|| bad("text is required"))?
        .to_string();
    let options = docrs::ReplaceOptions {
        tracked,
        author: step.param("author", 2).as_str().map(str::to_string),
        date: super::commentops::date(step, "date", 3)?,
        preserve_revision: false,
    };
    doc.replace(&span, &text, &options)?;
    Ok(Value::Null)
}

fn tracked_replace(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    replace(doc, step, true)
}

fn plain_replace(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    replace(doc, step, false)
}

fn accept_all_twice(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let mut snapshot = doc.revisions()?;
    let first = snapshot.accept_all(doc, None)?;
    let second = snapshot.accept_all(doc, None)?;
    Ok(json!([first, second, snapshot.len()]))
}

/// Adapter for an upstream `revision.*` name or its `ext.p5_*` helper.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "ext.p5_revisions" => Some(revisions),
        "ext.p5_remaining_unsupported" => Some(remaining_unsupported),
        "ext.p5_accept_all" => Some(accept_all),
        "ext.p5_reject_all" => Some(reject_all),
        "ext.p5_accept" => Some(accept),
        "ext.p5_reject" => Some(reject),
        "ext.p5_remaining_markup" => Some(remaining_markup),
        "ext.p5_stale" => Some(stale),
        "ext.p5_accept_all_twice" => Some(accept_all_twice),
        "ext.p5_tracked_replace" => Some(tracked_replace),
        "ext.p5_replace" => Some(plain_replace),
        _ => None,
    }
}
