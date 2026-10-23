//! Ops adapters for upstream `docx.controls` (plus the p1 `ext.*` helpers
//! that drive `Control` methods).

use docrs::clock::Timestamp;
use docrs::controls::{Control, ControlValue};
use docrs::{Document, Error, Result};
use serde_json::{Value, json};

use super::{OpFn, Step};

/// Decode a control value: string, bool, `{"$date": iso}` (aware
/// datetime) or `{"$day": "YYYY-MM-DD"}` (date).
pub fn control_value(value: &Value) -> Result<ControlValue> {
    match value {
        Value::String(text) => Ok(ControlValue::Text(text.clone())),
        Value::Bool(flag) => Ok(ControlValue::Checked(*flag)),
        Value::Object(map) if map.contains_key("$date") => Ok(ControlValue::Instant(
            Timestamp::parse(map["$date"].as_str().unwrap_or(""))?,
        )),
        Value::Object(map) if map.contains_key("$day") => {
            let text = map["$day"].as_str().unwrap_or("");
            let parts: Vec<&str> = text.split('-').collect();
            let field = |i: usize| parts.get(i).and_then(|p| p.parse::<i64>().ok());
            match (field(0), field(1), field(2)) {
                (Some(year), Some(month), Some(day)) => Ok(ControlValue::Date {
                    year: year as i32,
                    month: month as u32,
                    day: day as u32,
                }),
                _ => Err(Error::invalid_argument(
                    "bad_case",
                    format!("bad $day {text}"),
                )),
            }
        }
        other => Err(Error::invalid_argument(
            "bad_case",
            format!("unsupported control value {other}"),
        )),
    }
}

fn opt(value: &Value) -> Option<&str> {
    value.as_str()
}

fn list_controls(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(Value::Array(
        doc.list_controls()?
            .iter()
            .map(|info| info.to_json())
            .collect(),
    ))
}

fn get_control(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    doc.control(opt(step.kwarg("tag")), opt(step.kwarg("alias")))?;
    Ok(json!({"type": "Control"}))
}

fn set_control_value(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let value = control_value(step.param("value", 0))?;
    doc.set_control_value(value, opt(step.kwarg("tag")), opt(step.kwarg("alias")))?;
    Ok(Value::Null)
}

fn select(doc: &Document, step: &Step<'_>) -> Result<Control> {
    match step.kwarg("index").as_u64() {
        Some(index) => doc
            .controls()?
            .into_iter()
            .nth(index as usize)
            .ok_or_else(|| Error::invalid_argument("bad_case", "control index out of range")),
        None => doc.control(opt(step.kwarg("tag")), opt(step.kwarg("alias"))),
    }
}

/// `ext.p1_control_state`: every `Control` read property plus `info()`.
fn control_state(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let control = select(doc, step)?;
    let value = match control.value(doc)? {
        ControlValue::Checked(flag) => Value::Bool(flag),
        ControlValue::Text(text) => Value::String(text),
        _ => Value::Null,
    };
    Ok(json!({
        "tag": control.tag(doc)?,
        "alias": control.alias(doc)?,
        "control_type": control.control_type(doc)?,
        "is_data_bound": control.is_data_bound(doc)?,
        "showing_placeholder": control.showing_placeholder(doc)?,
        "value": value,
        "info": control.info(doc)?.to_json(),
    }))
}

fn control_set_value(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let control = select(doc, step)?;
    let value = control_value(step.param("value", 0))?;
    control.set_value(doc, value)?;
    Ok(Value::Null)
}

/// `ext.p1_control_set_twice`: one handle, two writes (handle freshness
/// across a commit).
fn control_set_twice(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let control = select(doc, step)?;
    control.set_value(doc, control_value(step.arg(0))?)?;
    control.set_value(doc, control_value(step.arg(1))?)?;
    Ok(Value::Null)
}

/// Adapter for an upstream `controls.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "controls.list_controls" => Some(list_controls),
        "controls.get_control" => Some(get_control),
        "controls.set_control_value" => Some(set_control_value),
        "ext.p1_control_state" => Some(control_state),
        "ext.p1_control_set_value" => Some(control_set_value),
        "ext.p1_control_set_twice" => Some(control_set_twice),
        _ => None,
    }
}
