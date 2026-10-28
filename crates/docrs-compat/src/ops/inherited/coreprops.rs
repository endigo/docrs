//! `ext.p7_*` adapters: core properties (oracle helpers in
//! `tools/oracle/p7_coreprops.py`).

use docrs::clock::Timestamp;
use docrs::coreprops::{CoreProperties, CoreProperty, CorePropertyChange, CoreValue};
use docrs::{Document, Result};
use serde_json::{Map, Value, json};

use crate::ops::{OpFn, Step, bad, date};

/// `p7_coreprops._props`.
fn props_json(props: &CoreProperties) -> Value {
    let mut out = Map::new();
    for property in CoreProperty::ALL {
        let value = match property {
            CoreProperty::Created | CoreProperty::LastPrinted | CoreProperty::Modified => {
                json!(props.date(property).map(Timestamp::to_xml))
            }
            CoreProperty::Revision => json!(props.revision),
            _ => json!(props.text(property)),
        };
        out.insert(property.name().to_string(), value);
    }
    Value::Object(out)
}

fn core_properties(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(props_json(&doc.core_properties()?))
}

fn set_core_properties(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let changes = step
        .param("changes", 0)
        .as_array()
        .ok_or_else(|| bad("changes must be a list"))?;
    let mut out = Vec::new();
    for change in changes {
        let name = change[0].as_str().ok_or_else(|| bad("property name"))?;
        let property = CoreProperty::from_name(name)?;
        let value = match &change[1] {
            Value::String(text) => CoreValue::Text(text.clone()),
            Value::Number(n) => CoreValue::Int(n.as_i64().ok_or_else(|| bad("integer"))?),
            other if other.get("$date").is_some() => {
                CoreValue::Date(date(other)?.ok_or_else(|| bad("date"))?)
            }
            other => return Err(bad(format!("unsupported core property value {other}"))),
        };
        out.push(CorePropertyChange::new(property, value));
    }
    let now = date(step.param("now", 1))?;
    Ok(props_json(&doc.set_core_properties(&out, now)?))
}

/// Adapter for an `ext.p7_*` core-properties name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p7_core_properties" => core_properties,
        "ext.p7_set_core_properties" => set_core_properties,
        _ => return None,
    })
}
