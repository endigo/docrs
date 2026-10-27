//! `ext.p7_*` adapters: style writing (oracle helpers in
//! `tools/oracle/p7_styles.py`).

use docrs::styles::{LatentStyle, StyleType, Styles};
use docrs::styles_write::{
    LatentDefaultsChange, LatentStyleChange, StyleChange, StyleRef, latent_style_refs,
};
use docrs::{Document, Result};
use serde_json::{Map, Value, json};

use crate::ops::inherited::{error_class, font_changes, font_json, parfmt_json, parfmt_props};
use crate::ops::{OpFn, Step, bad};

/// `p7_common.error_class` plus the style-specific `AttributeError`.
fn classify(result: Result<Value>) -> Result<Value> {
    match result {
        Err(err) if err.reason() == "style_attribute_unsupported" => {
            Ok(json!({"error": "AttributeError"}))
        }
        other => error_class(other),
    }
}

fn key<'a>(step: &'a Step<'_>) -> Result<&'a str> {
    step.param("key", 0)
        .as_str()
        .ok_or_else(|| bad("style key must be a string"))
}

/// `p7_styles._summary`.
fn summary(doc: &Document, style: &StyleRef) -> Result<Value> {
    let info = style.info(doc)?;
    let mut out = Map::new();
    out.insert("name".into(), json!(info.name));
    out.insert("style_id".into(), json!(info.style_id));
    out.insert("type".into(), json!(info.style_type.as_xml()));
    out.insert("builtin".into(), json!(info.builtin));
    out.insert("hidden".into(), json!(info.hidden));
    out.insert("locked".into(), json!(info.locked));
    out.insert("priority".into(), json!(info.priority));
    out.insert("quick_style".into(), json!(info.quick_style));
    out.insert("unhide_when_used".into(), json!(info.unhide_when_used));
    if info.style_type != StyleType::Numbering {
        let base = style.base_style(doc)?;
        let id = match base {
            Some(base) => base.style_id(doc)?,
            None => None,
        };
        out.insert("base_style".into(), json!(id));
    }
    if matches!(info.style_type, StyleType::Paragraph | StyleType::Table) {
        out.insert(
            "next_paragraph_style".into(),
            json!(style.next_paragraph_style(doc)?.style_id(doc)?),
        );
    }
    Ok(Value::Object(out))
}

fn styles(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let snapshot: Styles = doc.styles()?;
    Ok(json!({
        "count": snapshot.len(),
        "styles": snapshot
            .iter()
            .map(|style| json!([style.name, style.style_id]))
            .collect::<Vec<_>>(),
    }))
}

fn style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    classify(doc.style(key).and_then(|style| summary(doc, &style)))
}

fn style_contains(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let name = step
        .param("name", 0)
        .as_str()
        .ok_or_else(|| bad("name must be a string"))?;
    Ok(json!(doc.styles()?.contains(name)))
}

fn add_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let name = step
        .param("name", 0)
        .as_str()
        .ok_or_else(|| bad("name must be a string"))?;
    let kind = StyleType::from_xml(step.param("style_type", 1).as_str().unwrap_or(""))?;
    let builtin = step.param("builtin", 2).as_bool().unwrap_or(false);
    let style = doc.add_style(name, kind, builtin)?;
    summary(doc, &style)
}

fn delete_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    classify(doc.style(key).and_then(|style| {
        style.delete(doc)?;
        Ok(json!(doc.styles()?.len()))
    }))
}

fn style_id_of(doc: &Document, key: &str) -> Result<Option<String>> {
    doc.style(key)?.style_id(doc)
}

fn style_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    let props = step
        .param("props", 1)
        .as_object()
        .ok_or_else(|| bad("props must be an object"))?;
    let style = doc.style(key)?;
    let order = [
        "name",
        "style_id",
        "hidden",
        "locked",
        "priority",
        "quick_style",
        "unhide_when_used",
        "base_style",
        "next_paragraph_style",
    ];
    if let Some(unknown) = props.keys().find(|k| !order.contains(&k.as_str())) {
        return Err(bad(format!("unknown style property {unknown}")));
    }
    let mut changes = Vec::new();
    for prop in order {
        let Some(value) = props.get(prop) else {
            continue;
        };
        let flag = || value.as_bool().unwrap_or(false);
        changes.push(match prop {
            "name" => StyleChange::Name(value.as_str().map(str::to_string)),
            "style_id" => StyleChange::StyleId(value.as_str().map(str::to_string)),
            "hidden" => StyleChange::Hidden(flag()),
            "locked" => StyleChange::Locked(flag()),
            "priority" => StyleChange::Priority(value.as_i64()),
            "quick_style" => StyleChange::QuickStyle(flag()),
            "unhide_when_used" => StyleChange::UnhideWhenUsed(flag()),
            "base_style" => StyleChange::BaseStyle(match value.as_str() {
                None => None,
                Some(target) => style_id_of(doc, target)?,
            }),
            _ => StyleChange::NextParagraphStyle(match value.as_str() {
                None => None,
                Some(target) => style_id_of(doc, target)?,
            }),
        });
    }
    style.set(doc, &changes)?;
    summary(doc, &style)
}

fn style_font(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    classify(
        doc.style(key)
            .and_then(|style| Ok(font_json(&style.font(doc)?))),
    )
}

fn style_set_font(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    let changes = font_changes(step.param("changes", 1))?;
    classify(doc.style(key).and_then(|style| {
        style.set_font(doc, &changes)?;
        Ok(font_json(&style.font(doc)?))
    }))
}

fn style_parfmt(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    classify(
        doc.style(key)
            .and_then(|style| Ok(parfmt_json(&style.paragraph_format(doc)?))),
    )
}

fn style_set_parfmt(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    let format = parfmt_props(step.param("props", 1))?;
    classify(doc.style(key).and_then(|style| {
        style.set_paragraph_format(doc, &format)?;
        Ok(parfmt_json(&style.paragraph_format(doc)?))
    }))
}

fn latent_json(latent: &LatentStyle) -> Value {
    json!({
        "name": latent.name,
        "hidden": latent.hidden,
        "locked": latent.locked,
        "priority": latent.priority,
        "quick_style": latent.quick_style,
        "unhide_when_used": latent.unhide_when_used,
    })
}

fn latent_styles(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let snapshot = doc.styles()?;
    let Some(latent) = snapshot.latent_styles() else {
        return Ok(json!({
            "default_priority": null,
            "load_count": null,
            "default_to_hidden": false,
            "default_to_locked": false,
            "default_to_quick_style": false,
            "default_to_unhide_when_used": false,
            "count": 0,
            "names": [],
        }));
    };
    Ok(json!({
        "default_priority": latent.default_priority,
        "load_count": latent.load_count,
        "default_to_hidden": latent.default_to_hidden,
        "default_to_locked": latent.default_to_locked,
        "default_to_quick_style": latent.default_to_quick_style,
        "default_to_unhide_when_used": latent.default_to_unhide_when_used,
        "count": latent.len(),
        "names": latent.exceptions.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
    }))
}

fn latent_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    classify(
        doc.latent_style(key)
            .and_then(|latent| Ok(latent_json(&latent.info(doc)?))),
    )
}

fn latent_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let props = step
        .param("props", 0)
        .as_object()
        .ok_or_else(|| bad("props must be an object"))?;
    let order = [
        "default_priority",
        "load_count",
        "default_to_hidden",
        "default_to_locked",
        "default_to_quick_style",
        "default_to_unhide_when_used",
    ];
    if let Some(unknown) = props.keys().find(|k| !order.contains(&k.as_str())) {
        return Err(bad(format!("unknown latent default {unknown}")));
    }
    let mut changes = Vec::new();
    for prop in order {
        let Some(value) = props.get(prop) else {
            continue;
        };
        // `bool(value)` for the flags.
        let flag = value
            .as_bool()
            .unwrap_or_else(|| value.as_i64().unwrap_or(0) != 0);
        changes.push(match prop {
            "default_priority" => LatentDefaultsChange::DefaultPriority(value.as_i64()),
            "load_count" => LatentDefaultsChange::LoadCount(value.as_i64()),
            "default_to_hidden" => LatentDefaultsChange::DefaultToHidden(flag),
            "default_to_locked" => LatentDefaultsChange::DefaultToLocked(flag),
            "default_to_quick_style" => LatentDefaultsChange::DefaultToQuickStyle(flag),
            _ => LatentDefaultsChange::DefaultToUnhideWhenUsed(flag),
        });
    }
    doc.set_latent_defaults(&changes)?;
    latent_styles(doc, step)
}

fn latent_add(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let name = step
        .param("name", 0)
        .as_str()
        .ok_or_else(|| bad("name must be a string"))?;
    let latent = doc.add_latent_style(name)?;
    Ok(latent_json(&latent.info(doc)?))
}

fn latent_delete(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    classify(doc.latent_style(key).and_then(|latent| {
        latent.delete(doc)?;
        Ok(json!(latent_style_refs(doc)?.len()))
    }))
}

fn latent_style_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = key(step)?;
    let props = step
        .param("props", 1)
        .as_object()
        .ok_or_else(|| bad("props must be an object"))?
        .clone();
    classify(doc.latent_style(key).and_then(|latent| {
        let mut changes = Vec::new();
        for prop in [
            "hidden",
            "locked",
            "priority",
            "quick_style",
            "unhide_when_used",
        ] {
            let Some(value) = props.get(prop) else {
                continue;
            };
            changes.push(match prop {
                "hidden" => LatentStyleChange::Hidden(value.as_bool()),
                "locked" => LatentStyleChange::Locked(value.as_bool()),
                "priority" => LatentStyleChange::Priority(value.as_i64()),
                "quick_style" => LatentStyleChange::QuickStyle(value.as_bool()),
                _ => LatentStyleChange::UnhideWhenUsed(value.as_bool()),
            });
        }
        latent.set(doc, &changes)?;
        Ok(latent_json(&latent.info(doc)?))
    }))
}

/// Adapter for an `ext.p7_*` styles name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p7_styles" => styles,
        "ext.p7_style" => style,
        "ext.p7_style_contains" => style_contains,
        "ext.p7_add_style" => add_style,
        "ext.p7_delete_style" => delete_style,
        "ext.p7_style_set" => style_set,
        "ext.p7_style_font" => style_font,
        "ext.p7_style_set_font" => style_set_font,
        "ext.p7_style_parfmt" => style_parfmt,
        "ext.p7_style_set_parfmt" => style_set_parfmt,
        "ext.p7_latent_styles" => latent_styles,
        "ext.p7_latent_style" => latent_style,
        "ext.p7_latent_set" => latent_set,
        "ext.p7_latent_add" => latent_add,
        "ext.p7_latent_delete" => latent_delete,
        "ext.p7_latent_style_set" => latent_style_set,
        _ => return None,
    })
}
