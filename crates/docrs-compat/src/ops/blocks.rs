//! Ops adapters for upstream `docx.blocks`.

use docrs::blocks::{
    ListBlock, ParagraphRange, RichBlock, RichParagraph, SectionOptions, TableBlock, TextRun,
};
use docrs::clock::Timestamp;
use docrs::{Document, Error, Result};
use serde_json::Value;

use super::{OpFn, Step, Target, strings, target};

fn bad(message: impl Into<String>) -> Error {
    Error::invalid_argument("bad_case", message)
}

/// Decode `{"$date": "..."}` (or null).
pub(crate) fn date(value: &Value) -> Result<Option<Timestamp>> {
    match value {
        Value::Null => Ok(None),
        other => match other["$date"].as_str() {
            Some(text) => Timestamp::parse(text).map(Some),
            None => Err(Error::invalid_argument(
                "invalid_date",
                format!("date must be a datetime or None, got {other}"),
            )),
        },
    }
}

fn opt_str(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        other => Err(bad(format!("expected a string, got {other}"))),
    }
}

fn bool_of(value: &Value) -> bool {
    value.as_bool().unwrap_or(false)
}

fn text_run(value: &Value) -> Result<TextRun> {
    if value["$new"] != "blocks.TextRun" {
        return Err(bad(format!("expected blocks.TextRun, got {value}")));
    }
    let fields = &value["fields"];
    Ok(TextRun {
        text: fields["text"]
            .as_str()
            .ok_or_else(|| bad("TextRun.text is required"))?
            .to_string(),
        bold: bool_of(&fields["bold"]),
        italic: bool_of(&fields["italic"]),
    })
}

/// Decode one `$new` block.
fn rich_block(value: &Value) -> Result<RichBlock> {
    let fields = &value["fields"];
    match value["$new"].as_str() {
        Some("blocks.RichParagraph") => Ok(RichBlock::Paragraph(RichParagraph {
            runs: fields["runs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(text_run)
                .collect::<Result<_>>()?,
            style: opt_str(&fields["style"])?,
        })),
        Some("blocks.ListBlock") => Ok(RichBlock::List(ListBlock {
            items: strings(&fields["items"]),
            kind: fields["kind"].as_str().unwrap_or("bullet").to_string(),
            level: fields["level"].as_i64().unwrap_or(0),
        })),
        Some("blocks.TableBlock") => Ok(RichBlock::Table(TableBlock {
            rows: fields["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(strings)
                .collect(),
        })),
        _ => Err(Error::invalid_argument(
            "invalid_block",
            format!("expected RichParagraph, ListBlock, or TableBlock, got {value}"),
        )),
    }
}

fn insert_section_after(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let anchor = target(doc, step.param("anchor", 0))?;
    let heading = step
        .kwarg("heading")
        .as_str()
        .ok_or_else(|| bad("heading is required"))?
        .to_string();
    let paragraphs = strings(step.kwarg("paragraphs"));
    let heading_style = if step.kwargs.contains_key("heading_style") {
        opt_str(step.kwarg("heading_style"))?
    } else {
        Some("Heading2".to_string())
    };
    let body_style = opt_str(step.kwarg("body_style"))?;
    let author = opt_str(step.kwarg("author"))?;
    let options = SectionOptions {
        heading_style: heading_style.as_deref(),
        body_style: body_style.as_deref(),
        tracked: bool_of(step.kwarg("tracked")),
        author: author.as_deref(),
        date: date(step.kwarg("date"))?,
    };
    doc.insert_section_after(anchor.as_target(), &heading, &paragraphs, &options)
        .map(|result| result.to_json())
}

fn optional_target(doc: &Document, value: &Value) -> Result<Option<Target>> {
    match value {
        Value::Null => Ok(None),
        other => target(doc, other).map(Some),
    }
}

fn count(step: &Step<'_>) -> i64 {
    step.kwarg("count").as_i64().unwrap_or(1)
}

fn author(step: &Step<'_>) -> Result<String> {
    Ok(opt_str(step.kwarg("author"))?.unwrap_or_default())
}

fn tracked_delete(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let start = target(doc, step.param("start_anchor", 0))?;
    let end = optional_target(doc, step.kwarg("end_anchor"))?;
    let range = ParagraphRange {
        start: start.as_target(),
        end: end.as_ref().map(Target::as_target),
        count: count(step),
    };
    let author = author(step)?;
    let date = date(step.kwarg("date"))?;
    doc.tracked_delete_paragraphs(range, &author, date)
        .map(|result| result.to_json())
}

fn tracked_replace(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let start = target(doc, step.param("start_anchor", 0))?;
    let replacement = strings(step.param("replacement_paragraphs", 1));
    let end = optional_target(doc, step.kwarg("end_anchor"))?;
    let range = ParagraphRange {
        start: start.as_target(),
        end: end.as_ref().map(Target::as_target),
        count: count(step),
    };
    let body_style = opt_str(step.kwarg("body_style"))?;
    let author = author(step)?;
    let date = date(step.kwarg("date"))?;
    doc.tracked_replace_paragraphs(range, &replacement, body_style.as_deref(), &author, date)
        .map(|result| result.to_json())
}

fn insert_blocks_after(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let anchor = target(doc, step.param("anchor", 0))?;
    let blocks = step
        .kwarg("blocks")
        .as_array()
        .into_iter()
        .flatten()
        .map(rich_block)
        .collect::<Result<Vec<_>>>()?;
    let author = opt_str(step.kwarg("author"))?;
    let date = date(step.kwarg("date"))?;
    doc.insert_blocks_after(
        anchor.as_target(),
        &blocks,
        bool_of(step.kwarg("tracked")),
        author.as_deref(),
        date,
    )
    .map(|result| result.to_json())
}

/// Adapter for an upstream `blocks.*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    match name {
        "blocks.insert_section_after" => Some(insert_section_after),
        "blocks.tracked_delete_paragraphs" => Some(tracked_delete),
        "blocks.tracked_replace_paragraphs" => Some(tracked_replace),
        "blocks.insert_blocks_after" => Some(insert_blocks_after),
        _ => None,
    }
}
