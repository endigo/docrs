#![allow(dead_code)] // shared decoders are used by the per-package adapters

//! Rust side of shared ops cases: `"module.function"` → adapter.
//!
//! Each module file exposes `lookup(name) -> Option<OpFn>` for the upstream
//! names it implements (e.g. `"bookmarks.create_bookmark"`). Arguments use
//! the same `$`-tagged encoding as `tools/oracle/oracle.py` (`$span`,
//! `$paragraph`, `$block`, `$table`, `$date`, `$new`).

pub mod authoring;
pub mod blocks;
pub mod bookmarks;
pub mod commentops;
pub mod composition;
pub mod controls;
pub mod drawing;
pub mod fields;
pub mod formatting;
pub mod inherited;
pub mod links;
pub mod notes;
pub mod numbering;
pub mod protection;
pub mod revision;
pub mod tableops;

use docrs::anchor::BlockTarget;
use docrs::{Block, Document, Error, FindOptions, MatchPolicy, Result, Span, View};
use serde_json::Value;

/// One step of an ops case.
pub struct Step<'a> {
    #[allow(dead_code)]
    pub op: &'a str,
    pub args: &'a [Value],
    pub kwargs: &'a serde_json::Map<String, Value>,
}

impl Step<'_> {
    /// Positional argument `index` (after the implicit document).
    pub fn arg(&self, index: usize) -> &Value {
        self.args.get(index).unwrap_or(&Value::Null)
    }

    /// Keyword argument.
    pub fn kwarg(&self, name: &str) -> &Value {
        self.kwargs.get(name).unwrap_or(&Value::Null)
    }

    /// Keyword or positional.
    pub fn param(&self, name: &str, index: usize) -> &Value {
        match self.kwargs.get(name) {
            Some(value) => value,
            None => self.arg(index),
        }
    }
}

/// Adapter signature.
pub type OpFn = fn(&mut Document, &Step<'_>) -> Result<Value>;

/// Find the adapter for `name`.
pub fn lookup(name: &str) -> Option<OpFn> {
    authoring::lookup(name)
        .or_else(|| blocks::lookup(name))
        .or_else(|| bookmarks::lookup(name))
        .or_else(|| controls::lookup(name))
        .or_else(|| drawing::lookup(name))
        .or_else(|| fields::lookup(name))
        .or_else(|| formatting::lookup(name))
        .or_else(|| links::lookup(name))
        .or_else(|| notes::lookup(name))
        .or_else(|| numbering::lookup(name))
        .or_else(|| protection::lookup(name))
        .or_else(|| revision::lookup(name))
        .or_else(|| commentops::lookup(name))
        .or_else(|| composition::lookup(name))
        .or_else(|| tableops::lookup(name))
        .or_else(|| inherited::lookup(name))
}

/// Decode a `$span` argument.
pub fn span(doc: &Document, value: &Value) -> Result<Span> {
    let text = value["$span"].as_str().ok_or_else(|| {
        Error::invalid_argument("bad_case", format!("expected $span, got {value}"))
    })?;
    let options = FindOptions {
        policy: MatchPolicy::parse(value["match"].as_str().unwrap_or("exact"))?,
        view: View::parse(value["view"].as_str().unwrap_or("current"))?,
        story: value["story"].as_str().map(str::to_string),
        nth: value["nth"].as_u64().map(|n| n as usize),
        near: None,
    };
    doc.find_one(text, &options)
}

/// Decode a `$block` argument (first block whose text equals the value).
pub fn block(doc: &Document, value: &Value) -> Result<Block> {
    let wanted = value["$block"].as_str().ok_or_else(|| {
        Error::invalid_argument("bad_case", format!("expected $block, got {value}"))
    })?;
    let view = View::parse(value["view"].as_str().unwrap_or("current"))?;
    let index = value["index"].as_u64().unwrap_or(0) as usize;
    doc.blocks(view)?
        .into_iter()
        .filter(|block| block.text == wanted)
        .nth(index)
        .ok_or_else(|| {
            Error::invalid_argument("bad_case", format!("no block with text {wanted:?}"))
        })
}

/// An owned [`BlockTarget`].
pub enum Target {
    Text(String),
    Block(Block),
    Span(Span),
}

impl Target {
    pub fn as_target(&self) -> BlockTarget<'_> {
        match self {
            Target::Text(text) => BlockTarget::Text(text),
            Target::Block(block) => BlockTarget::Block(block),
            Target::Span(span) => BlockTarget::Span(span),
        }
    }
}

/// Decode a block target: a plain string, `$block`, `$span` or `$paragraph`
/// (a paragraph is addressed through a span inside it).
pub fn target(doc: &Document, value: &Value) -> Result<Target> {
    if let Some(text) = value.as_str() {
        return Ok(Target::Text(text.to_string()));
    }
    if value.get("$block").is_some() {
        return Ok(Target::Block(block(doc, value)?));
    }
    if value.get("$span").is_some() {
        return Ok(Target::Span(span(doc, value)?));
    }
    if let Some(text) = value["$paragraph"].as_str() {
        let mut spec = value.clone();
        spec["$span"] = Value::String(text.to_string());
        return Ok(Target::Span(span(doc, &spec)?));
    }
    Err(Error::invalid_argument(
        "bad_case",
        format!("cannot decode target {value}"),
    ))
}

/// Strings from a JSON array argument.
pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
