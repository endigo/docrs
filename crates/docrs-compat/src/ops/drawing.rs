//! Ops adapters for upstream `docx.drawing.Drawing` (oracle helpers
//! `ext.p4_has_picture`, `ext.p4_drawing_image`, `ext.p4_replace_picture`).

use docrs::drawing::Drawing;
use docrs::{Document, Error, Result};
use serde_json::{Value, json};

use super::authoring::{error_class, image_bytes, image_summary};
use super::{OpFn, Step};

fn drawing(doc: &Document, key: &Value) -> Result<Drawing> {
    let bad = |message: String| Error::invalid_argument("bad_case", message);
    let (drawings, index) = match key {
        Value::Number(n) => (doc.drawings()?, n.as_i64().unwrap_or(0)),
        Value::Object(map) => {
            let part = map["part"]
                .as_str()
                .ok_or_else(|| bad("drawing part".into()))?;
            (
                doc.drawings_in(part)?,
                map.get("index").and_then(Value::as_i64).unwrap_or(0),
            )
        }
        other => return Err(bad(format!("bad drawing locator {other}"))),
    };
    let len = drawings.len() as i64;
    let real = if index < 0 { len + index } else { index };
    if real < 0 || real >= len {
        return Err(bad(format!("drawing index {index} out of range")));
    }
    Ok(drawings[real as usize].clone())
}

fn has_picture(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    Ok(json!(
        drawing(doc, step.param("drawing", 0))?.has_picture(doc)?
    ))
}

fn drawing_image(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = drawing(doc, step.param("drawing", 0))?;
    Ok(match target.image(doc) {
        Ok(image) => image_summary(&image),
        Err(err) => json!({"error": error_class(&err)}),
    })
}

fn replace_picture(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = drawing(doc, step.param("drawing", 0))?;
    let bytes = image_bytes(step.param("image", 1))?;
    target.replace_picture(doc, &bytes)?;
    Ok(Value::Null)
}

/// Adapter for an `ext.p4_*` drawing name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p4_has_picture" => has_picture,
        "ext.p4_drawing_image" => drawing_image,
        "ext.p4_replace_picture" => replace_picture,
        _ => return None,
    })
}
