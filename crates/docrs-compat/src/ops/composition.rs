//! Ops adapters for upstream `docx.composition`.
//!
//! The ops runner gives every case one fresh destination document; the
//! source document is an argument:
//!
//! * `{"$source": "<fixture>"}` or `{"$source": "<fixture>", "steps": [...]}`
//!   — the fixture (`new`, `synthetic:<name>` or an upstream fixture path)
//!   opened fresh, then built with ordinary ops steps run on it. Sources are
//!   cached per destination document and spec, so several arguments of one
//!   case address the same source object (as in the oracle).
//! * `{"$source_span": "<text>", "source": <spec>, "view"?, "nth"?, ...}` and
//!   `{"$source_block": "<text>", "source": <spec>, "view"?, "index"?}` —
//!   live source anchors.
//!
//! Synthetic and `new` sources are also written to `target/p6-sources/`
//! so the oracle (`tools/oracle/ext_p6.py`) opens the same bytes; the ops
//! runner runs every Rust case before the oracle batch.

use std::cell::RefCell;

use docrs::composition::{
    AppendOptions, HeadersFrom, InsertBlocksOptions, SectionStart, StylesMode,
};
use docrs::{Document, DocumentId, Error, Limits, Result};
use serde_json::{Value, json};

use super::{OpFn, Step, Target, bad, block, span, target};

struct Entry {
    key: String,
    document: Document,
    snapshot: Vec<u8>,
}

#[derive(Default)]
struct Cache {
    destination: Option<DocumentId>,
    entries: Vec<Entry>,
}

thread_local! {
    static SOURCES: RefCell<Cache> = RefCell::new(Cache::default());
}

fn normalized_spec(value: &Value) -> Result<Value> {
    match value {
        Value::String(fixture) => Ok(json!({"$source": fixture})),
        Value::Object(map) if map.get("$source").is_some_and(Value::is_string) => {
            let mut spec = serde_json::Map::new();
            spec.insert("$source".into(), map["$source"].clone());
            if let Some(steps) = map.get("steps") {
                spec.insert("steps".into(), steps.clone());
            }
            Ok(Value::Object(spec))
        }
        other => Err(bad(format!("cannot decode source {other}"))),
    }
}

fn materialized_dir() -> std::path::PathBuf {
    crate::oracle::repo_root().join("target").join("p6-sources")
}

fn fixture_bytes(fixture: &str) -> Result<Vec<u8>> {
    let (name, bytes) = if fixture == "new" {
        let (bytes, _) = Document::new().patch_bytes()?;
        ("new".to_string(), bytes)
    } else if let Some(name) = fixture.strip_prefix("synthetic:") {
        let bytes = crate::hostile::synthetic_document(name)
            .ok_or_else(|| bad(format!("unknown synthetic document {name}")))?;
        (name.to_string(), bytes.to_vec())
    } else {
        let path = crate::hostile::fixtures_dir().join(fixture);
        return std::fs::read(&path).map_err(|e| bad(format!("{}: {e}", path.display())));
    };
    let dir = materialized_dir();
    std::fs::create_dir_all(&dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let path = dir.join(format!(
        "{}.docx",
        crate::opsrun::case_safe_file_stem(&name)
    ));
    if std::fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::write(&path, &bytes).map_err(|e| bad(format!("{}: {e}", path.display())))?;
    }
    Ok(bytes)
}

fn build_source(spec: &Value) -> Result<Document> {
    let fixture = spec["$source"].as_str().unwrap_or("");
    let mut document = Document::open_bytes(&fixture_bytes(fixture)?, &Limits::default())?;
    let empty = serde_json::Map::new();
    for step in spec["steps"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let name = step["op"].as_str().unwrap_or("");
        let adapter =
            super::lookup(name).ok_or_else(|| bad(format!("no adapter for source step {name}")))?;
        let call = Step {
            args: step["args"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            kwargs: step["kwargs"].as_object().unwrap_or(&empty),
        };
        adapter(&mut document, &call)
            .map_err(|err| Error::internal("source_step_failed", format!("{name}: {err}")))?;
    }
    Ok(document)
}

fn source_index(destination: &Document, value: &Value) -> Result<usize> {
    let spec = normalized_spec(value)?;
    let key = spec.to_string();
    let stale = SOURCES.with(|cache| cache.borrow().destination != Some(destination.id()));
    if stale {
        SOURCES.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.destination = Some(destination.id());
            cache.entries.clear();
        });
    }
    if let Some(index) =
        SOURCES.with(|cache| cache.borrow().entries.iter().position(|e| e.key == key))
    {
        return Ok(index);
    }
    let document = build_source(&spec)?;
    let (snapshot, _) = document.patch_bytes()?;
    SOURCES.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.entries.push(Entry {
            key,
            document,
            snapshot,
        });
        Ok(cache.entries.len() - 1)
    })
}

fn source_spec_of(value: &Value) -> Option<&Value> {
    value.get("$source").map(|_| value)
}

/// Every source spec an argument mentions (so all are built before the
/// cache is borrowed).
fn mentioned_sources(value: &Value) -> Vec<&Value> {
    if let Some(spec) = source_spec_of(value) {
        return vec![spec];
    }
    if value.get("$source_span").is_some() || value.get("$source_block").is_some() {
        return value.get("source").into_iter().collect();
    }
    Vec::new()
}

fn source_target(
    destination: &Document,
    entries: &[Entry],
    keys: &[(String, usize)],
    value: &Value,
) -> Result<Target> {
    let lookup = |spec: &Value| -> Result<&Document> {
        let key = normalized_spec(spec)?.to_string();
        keys.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, index)| &entries[*index].document)
            .ok_or_else(|| bad("source not prepared"))
    };
    if let Some(text) = value["$source_span"].as_str() {
        let mut spec = value.clone();
        spec["$span"] = Value::String(text.to_string());
        return Ok(Target::Span(span(lookup(&value["source"])?, &spec)?));
    }
    if let Some(text) = value["$source_block"].as_str() {
        let mut spec = value.clone();
        spec["$block"] = Value::String(text.to_string());
        return Ok(Target::Block(block(lookup(&value["source"])?, &spec)?));
    }
    target(destination, value)
}

fn bool_kw(step: &Step<'_>, name: &str, default: bool) -> Result<bool> {
    match step.kwarg(name) {
        Value::Null => Ok(default),
        Value::Bool(value) => Ok(*value),
        other => Err(bad(format!("{name} must be a bool, got {other}"))),
    }
}

fn str_kw<'a>(step: &'a Step<'_>, name: &str, default: &'a str) -> Result<&'a str> {
    match step.kwarg(name) {
        Value::Null => Ok(default),
        Value::String(value) => Ok(value),
        other => Err(bad(format!("{name} must be a string, got {other}"))),
    }
}

fn prepare(doc: &Document, values: &[&Value]) -> Result<Vec<(String, usize)>> {
    let mut keys = Vec::new();
    for value in values {
        for spec in mentioned_sources(value) {
            let index = source_index(doc, spec)?;
            keys.push((normalized_spec(spec)?.to_string(), index));
        }
    }
    Ok(keys)
}

fn insert_blocks_from(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = StylesMode::parse(str_kw(step, "styles", "match_by_name")?)?;
    let source_value = step.arg(0);
    let start_value = step.arg(1);
    let anchor_value = step.kwarg("anchor");
    let end_value = step.kwarg("end_anchor");
    let keys = prepare(doc, &[source_value, start_value, anchor_value, end_value])?;
    let count = match step.kwarg("count") {
        Value::Null => 1,
        value => value
            .as_i64()
            .ok_or_else(|| bad("count must be an integer"))?,
    };
    let include_start = bool_kw(step, "include_start", true)?;
    let include_end = bool_kw(step, "include_end", true)?;
    SOURCES.with(|cache| {
        let cache = cache.borrow();
        let source_key = normalized_spec(source_value)?.to_string();
        let source = keys
            .iter()
            .find(|(k, _)| *k == source_key)
            .map(|(_, index)| &cache.entries[*index].document)
            .ok_or_else(|| bad("insert_blocks_from needs a $source"))?;
        let start = source_target(doc, &cache.entries, &keys, start_value)?;
        let end = match end_value {
            Value::Null => None,
            value => Some(source_target(doc, &cache.entries, &keys, value)?),
        };
        let anchor = source_target(doc, &cache.entries, &keys, anchor_value)?;
        let options = InsertBlocksOptions {
            end_anchor: end.as_ref().map(Target::as_target),
            count,
            styles,
            include_start,
            include_end,
        };
        docrs::composition::insert_blocks_from(
            doc,
            source,
            start.as_target(),
            anchor.as_target(),
            &options,
        )
        .map(|report| report.to_json())
    })
}

fn append_document(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = StylesMode::parse(str_kw(step, "styles", "match_by_name")?)?;
    let section = SectionStart::parse(str_kw(step, "section", "new_page")?)?;
    let headers = HeadersFrom::parse(str_kw(step, "headers", "destination")?)?;
    let source_value = step.arg(0);
    let keys = prepare(doc, &[source_value])?;
    SOURCES.with(|cache| {
        let cache = cache.borrow();
        let index = keys
            .first()
            .map(|(_, index)| *index)
            .ok_or_else(|| bad("append_document needs a $source"))?;
        let options = AppendOptions {
            section,
            styles,
            headers,
        };
        docrs::composition::append_document(doc, &cache.entries[index].document, &options)
            .map(|report| report.to_json())
    })
}

/// `ext.p6_copy_letterhead(source, styles=)`: upstream `_copy_letterhead`
/// with a fresh report (success paths only; upstream has no rollback there).
fn copy_letterhead(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let styles = StylesMode::parse(str_kw(step, "styles", "match_by_name")?)?;
    let keys = prepare(doc, &[step.arg(0)])?;
    let index = keys
        .first()
        .map(|(_, i)| *i)
        .ok_or_else(|| bad("needs a $source"))?;
    SOURCES.with(|cache| {
        let cache = cache.borrow();
        docrs::composition::copy_letterhead(doc, &cache.entries[index].document, styles)
            .map(|report| report.to_json())
    })
}

fn source_unchanged(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let keys = prepare(doc, &[step.arg(0)])?;
    let index = keys
        .first()
        .map(|(_, i)| *i)
        .ok_or_else(|| bad("needs a $source"))?;
    SOURCES.with(|cache| {
        let cache = cache.borrow();
        let entry = &cache.entries[index];
        let (bytes, _) = entry.document.patch_bytes()?;
        Ok(Value::Bool(bytes == entry.snapshot))
    })
}

/// Adapter for an upstream `composition.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "composition.insert_blocks_from" => insert_blocks_from,
        "composition.append_document" => append_document,
        "ext.p6_source_unchanged" => source_unchanged,
        "ext.p6_copy_letterhead" => copy_letterhead,
        _ => return None,
    })
}
