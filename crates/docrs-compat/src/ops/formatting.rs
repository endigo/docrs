//! Ops adapters for upstream `docx.formatting` and the python-docx `styles`
//! read API (reported through `ext.p3_*` helpers in `tools/oracle/ext_p3.py`).

use docrs::anchor::BlockTarget;
use docrs::formatting::FormatTarget;
use docrs::styles::{LatentStyle, Style, StyleType, Styles, internal_to_ui, ui_to_internal};
use docrs::{BlockKind, Document, Error, FindOptions, Result, View};
use serde_json::{Value, json};

use super::{OpFn, Step, span};

fn bad_case(message: impl Into<String>) -> Error {
    Error::invalid_argument("bad_case", message)
}

fn format_of(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let value = step.param("target", 0);
    if let Some(text) = value.get("$paragraph").and_then(Value::as_str) {
        let mut spec = value.clone();
        spec["$span"] = Value::String(text.to_string());
        let found = span(doc, &spec)?;
        return Ok(doc
            .format_of(FormatTarget::Paragraph(BlockTarget::Span(&found)))?
            .to_json());
    }
    if let Some(text) = value.get("$run").and_then(Value::as_str) {
        let mut spec = value.clone();
        spec["$span"] = Value::String(text.to_string());
        let found = span(doc, &spec)?;
        return Ok(doc.format_of(FormatTarget::RunAt(&found))?.to_json());
    }
    if value.get("$span").is_some() {
        let found = span(doc, value)?;
        return Ok(doc.format_of(FormatTarget::Span(&found))?.to_json());
    }
    // Upstream raises TypeError for anything that is not a Run, Paragraph
    // or Span; the Rust API cannot express such a target at all.
    Err(Error::invalid_argument(
        "unsupported_format_target",
        format!("format_of resolves Run, Paragraph or Span targets, not {value}"),
    ))
}

fn style_type(value: &Value) -> Result<StyleType> {
    StyleType::from_xml(
        value
            .as_str()
            .ok_or_else(|| bad_case(format!("style type must be a token, got {value}")))?,
    )
}

fn summary(styles: &Styles, style: Option<&Style>) -> Value {
    let Some(style) = style else {
        return Value::Null;
    };
    let base = match style.style_type {
        StyleType::Numbering => None,
        _ => styles
            .base_style(style)
            .and_then(|base| base.style_id.clone()),
    };
    let next = match style.style_type {
        StyleType::Paragraph | StyleType::Table => {
            styles.next_paragraph_style(style).style_id.clone()
        }
        _ => None,
    };
    json!({
        "style_id": style.style_id,
        "name": style.name,
        "type": style.style_type.as_xml(),
        "builtin": style.builtin,
        "hidden": style.hidden,
        "locked": style.locked,
        "priority": style.priority,
        "quick_style": style.quick_style,
        "unhide_when_used": style.unhide_when_used,
        "base_style": base,
        "next_paragraph_style": next,
    })
}

fn latent_summary(latent: &LatentStyle) -> Value {
    json!({
        "name": latent.name,
        "hidden": latent.hidden,
        "locked": latent.locked,
        "priority": latent.priority,
        "quick_style": latent.quick_style,
        "unhide_when_used": latent.unhide_when_used,
    })
}

fn latent_report(styles: &Styles) -> Value {
    let Some(latent) = styles.latent_styles() else {
        return Value::Null;
    };
    json!({
        "count": latent.len(),
        "default_priority": latent.default_priority,
        "default_to_hidden": latent.default_to_hidden,
        "default_to_locked": latent.default_to_locked,
        "default_to_quick_style": latent.default_to_quick_style,
        "default_to_unhide_when_used": latent.default_to_unhide_when_used,
        "load_count": latent.load_count,
        "exceptions": latent.exceptions.iter().map(latent_summary).collect::<Vec<_>>(),
    })
}

fn p3_styles(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let styles = doc.styles()?;
    let mut defaults = serde_json::Map::new();
    for kind in StyleType::ALL {
        defaults.insert(
            kind.as_xml().to_string(),
            json!(styles.default_style(kind).and_then(|s| s.style_id.clone())),
        );
    }
    Ok(json!({
        "count": styles.len(),
        "styles": styles.iter().map(|style| summary(&styles, Some(style))).collect::<Vec<_>>(),
        "defaults": defaults,
        "latent": latent_report(&styles),
    }))
}

fn text_arg<'a>(step: &'a Step<'_>, name: &str, index: usize) -> Result<&'a str> {
    step.param(name, index)
        .as_str()
        .ok_or_else(|| bad_case(format!("{name} must be a string")))
}

fn p3_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = doc.styles()?;
    let key = text_arg(step, "key", 0)?;
    Ok(summary(&styles, styles.get(key)))
}

fn p3_style_contains(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    Ok(json!(doc.styles()?.contains(text_arg(step, "name", 0)?)))
}

fn p3_style_default(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = doc.styles()?;
    let kind = style_type(step.param("style_type", 0))?;
    Ok(summary(&styles, styles.default_style(kind)))
}

fn p3_style_get_by_id(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = doc.styles()?;
    let id = step.param("style_id", 0).as_str();
    let kind = style_type(step.param("style_type", 1))?;
    Ok(summary(&styles, styles.get_by_id(id, kind)))
}

fn p3_style_get_style_id(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = doc.styles()?;
    let name = text_arg(step, "name", 0)?;
    let kind = style_type(step.param("style_type", 1))?;
    Ok(json!(styles.get_style_id(name, kind)?))
}

fn p3_latent(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(latent_report(&doc.styles()?))
}

fn p3_latent_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = doc.styles()?;
    let name = text_arg(step, "name", 0)?;
    Ok(styles
        .latent_styles()
        .and_then(|latent| latent.get(name))
        .map(latent_summary)
        .unwrap_or(Value::Null))
}

fn p3_babel(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let names = super::strings(step.param("names", 0));
    Ok(json!(
        names
            .iter()
            .map(|name| json!({
                "name": name,
                "ui2internal": ui_to_internal(name),
                "internal2ui": internal_to_ui(name),
            }))
            .collect::<Vec<_>>()
    ))
}

fn p3_format_value(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = text_arg(step, "key", 1)?.to_string();
    let target = step.param("target", 0).clone();
    let args = [target];
    let inner = Step {
        op: "formatting.format_of",
        args: &args,
        kwargs: &serde_json::Map::new(),
    };
    let report = format_of(doc, &inner)?;
    Ok(report["properties"][key.as_str()]["value"].clone())
}

fn sweep_error(err: Error) -> Result<Value> {
    match err.code() {
        code @ ("ambiguous_target"
        | "target_not_found"
        | "unsupported_structure"
        | "boundary_violation"
        | "invalid_argument") => Ok(json!({"error": code})),
        _ => Err(err),
    }
}

fn p3_format_blocks(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let mut out = Vec::new();
    for block in doc.blocks(View::Current)? {
        if block.kind != BlockKind::Paragraph {
            continue;
        }
        match doc.format_of(FormatTarget::Paragraph(BlockTarget::Block(&block))) {
            Ok(format) => out.push(format.to_json()),
            Err(err) => out.push(sweep_error(err)?),
        }
    }
    Ok(Value::Array(out))
}

fn p3_format_spans(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let mut out = Vec::new();
    for block in doc.blocks(View::Current)? {
        if block.kind != BlockKind::Paragraph || block.text.is_empty() {
            continue;
        }
        let spans = doc.find_text(&block.text, &FindOptions::default())?;
        if spans.len() != 1 {
            continue;
        }
        match doc.format_of(FormatTarget::Span(&spans[0])) {
            Ok(format) => out.push(json!({"text": block.text, "format": format.to_json()})),
            Err(err) => {
                let mut value = sweep_error(err)?;
                value["text"] = json!(block.text);
                out.push(value);
            }
        }
    }
    Ok(Value::Array(out))
}

/// Adapter for an upstream `formatting.*` name or a p3 style helper.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "formatting.format_of" => Some(format_of),
        "ext.p3_styles" => Some(p3_styles),
        "ext.p3_style" => Some(p3_style),
        "ext.p3_style_contains" => Some(p3_style_contains),
        "ext.p3_style_default" => Some(p3_style_default),
        "ext.p3_style_get_by_id" => Some(p3_style_get_by_id),
        "ext.p3_style_get_style_id" => Some(p3_style_get_style_id),
        "ext.p3_latent" => Some(p3_latent),
        "ext.p3_latent_style" => Some(p3_latent_style),
        "ext.p3_babel" => Some(p3_babel),
        "ext.p3_format_value" => Some(p3_format_value),
        "ext.p3_format_blocks" => Some(p3_format_blocks),
        "ext.p3_format_spans" => Some(p3_format_spans),
        _ => None,
    }
}
