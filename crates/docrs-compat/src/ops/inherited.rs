//! Ops adapters for the remaining inherited python-docx public API (work
//! package P7, M7). Oracle helpers live in `tools/oracle/ext_p7.py` (which
//! loads `tools/oracle/p7_*.py`); every `ext.p7_*` name registered there has
//! an adapter here with the same name. The shared encoders below mirror
//! `tools/oracle/p7_common.py`.
//!
//! Submodules by area: sections / headers / footers / settings, core
//! properties, styles (writing), text (paragraph format, tab stops, fonts,
//! inline shapes, comment ranges) and tables.

use docrs::authoring::{
    BlockContainer, BlockItem, FONT_FLAGS, FontChange, FontValues, LineSpacing, LineSpacingRule,
    Paragraph, ParagraphFormat, ParagraphFormatValues, Run, Table, Underline,
};
use docrs::sections::HeaderFooter;
use docrs::{Document, Error, Result};
use serde_json::{Map, Value, json};

use super::OpFn;

mod coreprops;
mod sections;
mod styles;
mod tables;
mod text;

/// Adapter for an `ext.p7_*` name.
pub fn lookup(name: &str) -> Option<OpFn> {
    sections::lookup(name)
        .or_else(|| coreprops::lookup(name))
        .or_else(|| styles::lookup(name))
        .or_else(|| text::lookup(name))
        .or_else(|| tables::lookup(name))
}

/// A malformed case argument.
pub(crate) fn bad(message: impl Into<String>) -> Error {
    Error::invalid_argument("bad_case", message)
}

/// Python list indexing (negative from the end).
pub(crate) fn pick<T: Clone>(items: &[T], index: i64, what: &str) -> Result<T> {
    let len = items.len() as i64;
    let real = if index < 0 { len + index } else { index };
    if real < 0 || real >= len {
        return Err(bad(format!("{what} index {index} out of range")));
    }
    Ok(items[real as usize].clone())
}

pub(crate) fn index_arg(value: &Value, what: &str) -> Result<i64> {
    value
        .as_i64()
        .ok_or_else(|| bad(format!("bad {what} index {value}")))
}

/// A section's header or footer by upstream property name.
pub(crate) fn hdrftr(doc: &Document, section: &Value, part: &Value) -> Result<HeaderFooter> {
    let sections = doc.sections()?;
    let section = pick(&sections, index_arg(section, "section")?, "section")?;
    Ok(match part.as_str().unwrap_or("") {
        "header" => section.header(),
        "footer" => section.footer(),
        "first_page_header" => section.first_page_header(),
        "first_page_footer" => section.first_page_footer(),
        "even_page_header" => section.even_page_header(),
        "even_page_footer" => section.even_page_footer(),
        other => return Err(bad(format!("unknown header/footer {other}"))),
    })
}

/// `p7_common.container` (a header/footer gets or adds its definition).
pub(crate) fn container(doc: &mut Document, key: &Value) -> Result<BlockContainer> {
    if key.is_null() || key.as_str() == Some("body") {
        return doc.body();
    }
    if !key["section"].is_null() {
        return hdrftr(doc, &key["section"], &key["part"])?.definition(doc);
    }
    if let Some(cell) = key["cell"].as_array() {
        let table = table(doc, &cell[0])?;
        let row = index_arg(&cell[1], "row")?;
        let col = index_arg(&cell[2], "col")?;
        if row < 0 || col < 0 {
            return Err(bad("negative cell index"));
        }
        return table.cell(doc, row as usize, col as usize)?.container(doc);
    }
    Err(bad(format!("bad container locator {key}")))
}

/// `p7_common.table`.
pub(crate) fn table(doc: &mut Document, key: &Value) -> Result<Table> {
    if let Some(index) = key.as_i64() {
        return pick(&doc.tables()?, index, "table");
    }
    let container = container(doc, &key["in"])?;
    pick(
        &container.tables(doc)?,
        index_arg(&key["index"], "table")?,
        "table",
    )
}

/// `p7_common.paragraph`.
pub(crate) fn paragraph(doc: &mut Document, key: &Value) -> Result<Paragraph> {
    if key.is_object() {
        let container = container(doc, &key["in"])?;
        return pick(
            &container.paragraphs(doc)?,
            index_arg(&key["index"], "paragraph")?,
            "paragraph",
        );
    }
    let paragraphs = doc.paragraphs()?;
    if let Some(index) = key.as_i64() {
        return pick(&paragraphs, index, "paragraph");
    }
    let wanted = key
        .as_str()
        .ok_or_else(|| bad(format!("bad paragraph locator {key}")))?;
    for paragraph in paragraphs {
        if paragraph.text(doc)? == wanted {
            return Ok(paragraph);
        }
    }
    Err(bad(format!("no paragraph with text {wanted:?}")))
}

/// `p7_common.run`.
pub(crate) fn run(doc: &mut Document, para: &Value, index: &Value) -> Result<Run> {
    let paragraph = paragraph(doc, para)?;
    pick(&paragraph.runs(doc)?, index_arg(index, "run")?, "run")
}

/// `_block_summary` of `p7_sections.py`.
pub(crate) fn block_summary(doc: &Document, item: &BlockItem) -> Result<Value> {
    Ok(match item {
        BlockItem::Paragraph(p) => {
            json!({"kind": "p", "text": p.text(doc)?, "style_id": p.style_id(doc)?})
        }
        BlockItem::Table(t) => {
            json!({"kind": "tbl", "rows": t.row_count(doc)?, "cols": t.column_count(doc)?})
        }
    })
}

/// Absolute path of `{"$p7_test_file": name}` in the pinned checkout.
pub(crate) fn test_file(value: &Value) -> Result<std::path::PathBuf> {
    let name = value["$p7_test_file"]
        .as_str()
        .ok_or_else(|| bad(format!("expected $p7_test_file, got {value}")))?;
    Ok(crate::oracle::repo_root()
        .join(".reference/paper-docx/features/steps/test_files")
        .join(name))
}

/// `p7_common.font_values`.
pub(crate) fn font_json(font: &FontValues) -> Value {
    let flags: Map<String, Value> = font
        .flags
        .iter()
        .map(|(name, value)| (name.clone(), json!(value)))
        .collect();
    let underline = match &font.underline {
        None => Value::Null,
        Some(Underline::Bool(flag)) => json!(flag),
        Some(Underline::Style(token)) => json!(token),
    };
    json!({
        "flags": flags,
        "name": font.name,
        "size": font.size,
        "underline": underline,
        "color": {
            "type": font.color.color_type,
            "rgb": font.color.rgb,
            "theme_color": font.color.theme_color,
        },
        "highlight": font.highlight,
        "subscript": font.subscript,
        "superscript": font.superscript,
    })
}

/// `p7_common.parfmt_values`.
pub(crate) fn parfmt_json(fmt: &ParagraphFormatValues) -> Value {
    let line_spacing = match fmt.line_spacing {
        None => Value::Null,
        Some(LineSpacing::Multiple(lines)) => json!(lines),
        Some(LineSpacing::Exactly(emu)) => json!(emu),
    };
    json!({
        "alignment": fmt.alignment,
        "left_indent": fmt.left_indent,
        "right_indent": fmt.right_indent,
        "first_line_indent": fmt.first_line_indent,
        "space_before": fmt.space_before,
        "space_after": fmt.space_after,
        "line_spacing": line_spacing,
        "line_spacing_rule": fmt.line_spacing_rule.map(LineSpacingRule::name),
        "keep_together": fmt.keep_together,
        "keep_with_next": fmt.keep_with_next,
        "page_break_before": fmt.page_break_before,
        "widow_control": fmt.widow_control,
    })
}

/// `p7_common.apply_parfmt` arguments → [`ParagraphFormat`].
pub(crate) fn parfmt_props(value: &Value) -> Result<ParagraphFormat> {
    let props = value
        .as_object()
        .ok_or_else(|| bad("props must be an object"))?;
    let mut format = ParagraphFormat::default();
    let tri = |value: &Value| Some(value.as_bool());
    for (name, value) in props {
        match name.as_str() {
            "alignment" => format.alignment = Some(value.as_str().map(str::to_string)),
            "left_indent" => format.left_indent = Some(value.as_i64()),
            "right_indent" => format.right_indent = Some(value.as_i64()),
            "first_line_indent" => format.first_line_indent = Some(value.as_i64()),
            "space_before" => format.space_before = Some(value.as_i64()),
            "space_after" => format.space_after = Some(value.as_i64()),
            "line_spacing" => {
                format.line_spacing = Some(if value.is_null() {
                    None
                } else if let Some(lines) = value["multiple"].as_f64() {
                    Some(LineSpacing::Multiple(lines))
                } else {
                    Some(LineSpacing::Exactly(
                        value["exact"].as_i64().ok_or_else(|| bad("line_spacing"))?,
                    ))
                })
            }
            "line_spacing_rule" => {
                format.line_spacing_rule = Some(match value.as_str() {
                    None => None,
                    Some(name) => Some(LineSpacingRule::from_name(name)?),
                })
            }
            "keep_together" => format.keep_together = tri(value),
            "keep_with_next" => format.keep_with_next = tri(value),
            "page_break_before" => format.page_break_before = tri(value),
            "widow_control" => format.widow_control = tri(value),
            other => return Err(bad(format!("unknown paragraph format property {other}"))),
        }
    }
    Ok(format)
}

fn hex_color(value: &str) -> Result<(u8, u8, u8)> {
    let byte = |i: usize| {
        value
            .get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .ok_or_else(|| Error::invalid_argument("invalid_rgb", format!("bad color {value}")))
    };
    if value.len() != 6 {
        return Err(Error::invalid_argument(
            "invalid_rgb",
            format!("bad color {value}"),
        ));
    }
    Ok((byte(0)?, byte(2)?, byte(4)?))
}

/// `p7_common.apply_font` arguments (`[[name, value], …]`) → changes.
pub(crate) fn font_changes(value: &Value) -> Result<Vec<FontChange>> {
    let changes = value
        .as_array()
        .ok_or_else(|| bad("changes must be a list"))?;
    let mut out = Vec::new();
    for change in changes {
        let name = change[0].as_str().ok_or_else(|| bad("font change name"))?;
        let value = &change[1];
        out.push(match name {
            "name" => FontChange::Name(value.as_str().map(str::to_string)),
            "size" => FontChange::Size(value.as_i64()),
            "underline" => FontChange::Underline(match value {
                Value::Bool(flag) => Some(Underline::Bool(*flag)),
                Value::String(token) => Some(Underline::Style(token.clone())),
                _ => None,
            }),
            "color" => FontChange::Color(value.as_str().map(hex_color).transpose()?),
            "theme_color" => FontChange::ThemeColor(value.as_str().map(str::to_string)),
            "highlight" => FontChange::Highlight(value.as_str().map(str::to_string)),
            "subscript" => FontChange::Subscript(value.as_bool()),
            "superscript" => FontChange::Superscript(value.as_bool()),
            flag if FONT_FLAGS.iter().any(|(n, _)| *n == flag) => {
                FontChange::Flag(flag.to_string(), value.as_bool())
            }
            other => return Err(bad(format!("unknown font property {other}"))),
        });
    }
    Ok(out)
}

/// Upstream exception class for a Rust error reason that stands for a
/// non-refusal python-docx exception (`p7_common.error_class`); `None` when
/// the error should surface as an error step.
pub(crate) fn upstream_class(err: &Error) -> Option<&'static str> {
    Some(match err.reason() {
        "invalid_span" => "InvalidSpanError",
        "index_out_of_range" => "IndexError",
        "missing_key" => "KeyError",
        "no_picture_element" => "AttributeError",
        _ => return None,
    })
}

/// Run `f`; an error with an upstream exception class becomes
/// `{"error": class}` (as `p7_common.error_class`).
pub(crate) fn error_class(result: Result<Value>) -> Result<Value> {
    match result {
        Err(err) => match upstream_class(&err) {
            Some(class) => Ok(json!({"error": class})),
            None => Err(err),
        },
        ok => ok,
    }
}
