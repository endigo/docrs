//! Ops adapters for upstream `docx.commentops`, `docx.comments` and
//! `Span.comment` (oracle helpers `ext.p5_*` in `tools/oracle/ext_p5.py`).

use docrs::clock::Timestamp;
use docrs::commentops;
use docrs::comments::{self, Comment};
use docrs::{Document, Error, Result};
use serde_json::{Value, json};

use super::{OpFn, Step};

fn supplied(step: &Step<'_>, name: &str, index: usize) -> bool {
    step.kwargs.contains_key(name) || step.args.len() > index
}

fn text(step: &Step<'_>, name: &str, index: usize, default: &str) -> Result<String> {
    if !supplied(step, name, index) {
        return Ok(default.to_string());
    }
    step.param(name, index)
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::invalid_argument("bad_case", format!("{name} must be a string")))
}

fn opt_text(step: &Step<'_>, name: &str, index: usize, default: Option<&str>) -> Option<String> {
    if !supplied(step, name, index) {
        return default.map(str::to_string);
    }
    step.param(name, index).as_str().map(str::to_string)
}

/// Decode a `$date` argument (`None` when absent).
pub(crate) fn date(step: &Step<'_>, name: &str, index: usize) -> Result<Option<Timestamp>> {
    let value = step.param(name, index);
    if value.is_null() {
        return Ok(None);
    }
    let raw = value["$date"].as_str().ok_or_else(|| {
        Error::invalid_argument("bad_case", format!("expected $date, got {value}"))
    })?;
    Timestamp::parse(raw).map(Some)
}

fn comment(doc: &Document, step: &Step<'_>) -> Result<Comment> {
    let id = step
        .param("comment_id", 0)
        .as_i64()
        .ok_or_else(|| Error::invalid_argument("bad_case", "comment_id is required"))?;
    doc.comments()?
        .iter()
        .find(|c| c.comment_id == id)
        .cloned()
        .ok_or_else(|| Error::invalid_argument("bad_case", format!("no comment {id}")))
}

fn list(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(Value::Array(
        doc.comments()?.iter().map(Comment::to_json).collect(),
    ))
}

fn add_comment(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let span = super::span(doc, step.param("span", 0))?;
    let initials = opt_text(step, "initials", 3, Some(""));
    let comment = doc.add_comment(
        &span,
        &text(step, "text", 1, "")?,
        &text(step, "author", 2, "")?,
        initials.as_deref(),
        date(step, "date", 4)?,
    )?;
    Ok(comment.to_json())
}

fn comments_add(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let initials = opt_text(step, "initials", 2, Some(""));
    let comment = comments::add_unanchored_comment(
        doc,
        &text(step, "text", 0, "")?,
        &text(step, "author", 1, "")?,
        initials.as_deref(),
        date(step, "date", 3)?,
    )?;
    Ok(comment.to_json())
}

fn span_comment(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let span = super::span(doc, step.param("span", 0))?;
    let initials = opt_text(step, "initials", 3, None);
    let comment = span.comment(
        doc,
        &text(step, "text", 1, "")?,
        &text(step, "author", 2, "")?,
        initials.as_deref(),
        date(step, "date", 4)?,
    )?;
    Ok(comment.to_json())
}

fn reply(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let parent = comment(doc, step)?;
    let initials = opt_text(step, "initials", 3, None);
    let child = commentops::reply(
        doc,
        &parent,
        &text(step, "text", 1, "")?,
        &text(step, "author", 2, "")?,
        initials.as_deref(),
        date(step, "date", 4)?,
    )?;
    Ok(child.to_json())
}

fn resolve(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    let resolved = step.param("resolved", 1).as_bool().unwrap_or(true);
    commentops::resolve(doc, &target, resolved)?;
    Ok(Value::Null)
}

fn is_resolved(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    Ok(json!(commentops::is_resolved(doc, &target)?))
}

fn parent_of(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    Ok(json!(commentops::parent_of(doc, &target)?))
}

fn anchored_text(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    Ok(json!(commentops::anchored_text(doc, &target)?))
}

fn delete_comment(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    commentops::delete_comment(doc, &target)?;
    Ok(Value::Null)
}

fn comment_thread(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(serde_json::to_value(commentops::comment_thread(doc)?).unwrap_or(Value::Null))
}

fn add_paragraph(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    let updated = comments::add_comment_paragraph(doc, &target, &text(step, "text", 1, "")?)?;
    Ok(updated.to_json())
}

fn add_table(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    let number = |name: &str, index: usize| {
        step.param(name, index)
            .as_i64()
            .ok_or_else(|| Error::invalid_argument("bad_case", format!("{name} is required")))
    };
    let updated = comments::add_comment_table(
        doc,
        &target,
        number("rows", 1)? as usize,
        number("cols", 2)? as usize,
        number("width_emu", 3)?,
    )?;
    Ok(updated.to_json())
}

fn set_author(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    let updated = comments::set_comment_author(doc, &target, &text(step, "author", 1, "")?)?;
    Ok(updated.to_json())
}

fn set_initials(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = comment(doc, step)?;
    let initials = opt_text(step, "initials", 1, None);
    let updated = comments::set_comment_initials(doc, &target, initials.as_deref())?;
    Ok(updated.to_json())
}

/// Adapter for an upstream `commentops.*` name or its `ext.p5_*` helper.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "commentops.comment_thread" => Some(comment_thread),
        "ext.p5_comments" => Some(list),
        "ext.p5_add_comment" => Some(add_comment),
        "ext.p5_comments_add" => Some(comments_add),
        "ext.p5_span_comment" => Some(span_comment),
        "ext.p5_reply" => Some(reply),
        "ext.p5_resolve" => Some(resolve),
        "ext.p5_is_resolved" => Some(is_resolved),
        "ext.p5_parent_of" => Some(parent_of),
        "ext.p5_anchored_text" => Some(anchored_text),
        "ext.p5_delete_comment" => Some(delete_comment),
        "ext.p5_comment_add_paragraph" => Some(add_paragraph),
        "ext.p5_comment_add_table" => Some(add_table),
        "ext.p5_comment_set_author" => Some(set_author),
        "ext.p5_comment_set_initials" => Some(set_initials),
        _ => None,
    }
}
