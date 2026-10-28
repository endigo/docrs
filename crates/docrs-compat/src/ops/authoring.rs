//! Ops adapters for the inherited python-docx authoring API and image
//! characterization (oracle helpers `ext.p4_*` in `tools/oracle/ext_p4.py`;
//! the locators are documented there).

use docrs::authoring::{BreakType, FontChange, Paragraph, Run, SectionStart, Table, Underline};
use docrs::image::{Image, upstream_exception};
use docrs::{Document, Error, Result};
use serde_json::{Value, json};

use super::inherited::{paragraph as para, parfmt_props, pick};
use super::{OpFn, Step, bad};

/// Decode `{"$p4_image": hex}` / `{"$p4_image_file": name}`.
pub fn image_bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(hex) = value["$p4_image"].as_str() {
        return (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("zz"), 16))
            .collect::<std::result::Result<Vec<u8>, _>>()
            .map_err(|_| bad("bad $p4_image hex"));
    }
    if let Some(name) = value["$p4_image_file"].as_str() {
        let path = crate::oracle::repo_root()
            .join(".reference/paper-docx/features/steps/test_files")
            .join(name);
        return std::fs::read(&path).map_err(|e| bad(format!("{}: {e}", path.display())));
    }
    Err(bad(format!("expected an image argument, got {value}")))
}

fn run(doc: &mut Document, para_key: &Value, run_key: &Value) -> Result<Run> {
    let runs = para(doc, para_key)?.runs(doc)?;
    pick(
        &runs,
        run_key.as_i64().ok_or_else(|| bad("bad run index"))?,
        "run",
    )
}

fn table(doc: &Document, key: &Value) -> Result<Table> {
    let tables = doc.tables()?;
    pick(
        &tables,
        key.as_i64().ok_or_else(|| bad("bad table index"))?,
        "table",
    )
}

fn para_summary(doc: &Document, paragraph: &Paragraph) -> Result<Value> {
    Ok(json!({"text": paragraph.text(doc)?, "style_id": paragraph.style_id(doc)?}))
}

fn table_summary(doc: &Document, table: &Table) -> Result<Value> {
    Ok(json!({
        "rows": table.row_count(doc)?,
        "cols": table.column_count(doc)?,
        "style_id": table.style_id(doc)?,
    }))
}

/// Upstream exception class name for an image-related error.
pub fn error_class(err: &Error) -> String {
    if let Some(class) = upstream_exception(err.reason()) {
        return class.to_string();
    }
    match err.reason() {
        "no_picture" => "ValueError".into(),
        "missing_image_relationship" | "missing_image_part" => "KeyError".into(),
        other => format!("rust:{other}"),
    }
}

pub fn image_summary(image: &Image) -> Value {
    image.to_json()
}

fn characterize(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let bytes = image_bytes(step.param("image", 0))?;
    Ok(match Image::from_blob(&bytes) {
        Ok(image) => image_summary(&image),
        Err(err) => json!({"error": error_class(&err)}),
    })
}

fn scaled(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let bytes = image_bytes(step.param("image", 0))?;
    let width = step.param("width", 1).as_i64();
    let height = step.param("height", 2).as_i64();
    Ok(
        match Image::from_blob(&bytes).and_then(|image| image.scaled_dimensions(width, height)) {
            Ok((cx, cy)) => json!([cx, cy]),
            Err(err) => json!({"error": error_class(&err)}),
        },
    )
}

fn add_paragraph(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let text = step.param("text", 0).as_str().unwrap_or("");
    let paragraph = doc.add_paragraph(text, step.param("style", 1).as_str())?;
    para_summary(doc, &paragraph)
}

fn add_heading(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let text = step.param("text", 0).as_str().unwrap_or("");
    let level = step.param("level", 1).as_i64().unwrap_or(1);
    let paragraph = doc.add_heading(text, level)?;
    para_summary(doc, &paragraph)
}

fn add_page_break(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let paragraph = doc.add_page_break()?;
    para_summary(doc, &paragraph)
}

fn add_table(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let rows = step.param("rows", 0).as_u64().ok_or_else(|| bad("rows"))? as usize;
    let cols = step.param("cols", 1).as_u64().ok_or_else(|| bad("cols"))? as usize;
    let table = doc.add_table(rows, cols, step.param("style", 2).as_str())?;
    table_summary(doc, &table)
}

fn shape_json(shape: &docrs::authoring::InlineShape) -> Value {
    json!({"width": shape.width, "height": shape.height})
}

fn add_picture(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let bytes = image_bytes(step.param("image", 0))?;
    let shape = doc.add_picture(
        &bytes,
        step.param("width", 1).as_i64(),
        step.param("height", 2).as_i64(),
    )?;
    Ok(shape_json(&shape))
}

fn add_section(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let start = SectionStart::parse(step.param("start_type", 0).as_str().unwrap_or("nextPage"))?;
    let section = doc.add_section(start)?;
    Ok(json!({"start_type": section.start_type.as_str()}))
}

fn add_run(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    let run = paragraph.add_run(
        doc,
        step.param("text", 1).as_str(),
        step.param("style", 2).as_str(),
    )?;
    Ok(json!({"text": run.text(doc)?}))
}

fn insert_paragraph_before(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    let new = paragraph.insert_paragraph_before(
        doc,
        step.param("text", 1).as_str(),
        step.param("style", 2).as_str(),
    )?;
    para_summary(doc, &new)
}

fn set_paragraph_text(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    paragraph.set_text(doc, step.param("text", 1).as_str())?;
    para_summary(doc, &paragraph)
}

fn clear_paragraph(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    paragraph.clear(doc)?;
    para_summary(doc, &paragraph)
}

fn set_paragraph_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    paragraph.set_style(doc, step.param("style", 1).as_str())?;
    para_summary(doc, &paragraph)
}

fn set_alignment(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    paragraph.set_alignment(doc, step.param("value", 1).as_str())?;
    Ok(Value::Null)
}

fn paragraph_format(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let paragraph = para(doc, step.param("para", 0))?;
    let format = parfmt_props(step.param("props", 1))?;
    paragraph.set_format(doc, &format)?;
    Ok(Value::Null)
}

fn run_text_value(doc: &Document, run: &Run) -> Result<Value> {
    Ok(json!({"text": run.text(doc)?}))
}

fn set_run_text(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    run.set_text(doc, step.param("text", 2).as_str().unwrap_or(""))?;
    run_text_value(doc, &run)
}

fn run_add_text(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    run.add_text(doc, step.param("text", 2).as_str().unwrap_or(""))?;
    run_text_value(doc, &run)
}

fn run_add_tab(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    run.add_tab(doc)?;
    run_text_value(doc, &run)
}

fn run_add_break(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    let kind = match step.param("kind", 2).as_str().unwrap_or("line") {
        "line" => BreakType::Line,
        "page" => BreakType::Page,
        "column" => BreakType::Column,
        "line_clear_left" => BreakType::LineClearLeft,
        "line_clear_right" => BreakType::LineClearRight,
        "line_clear_all" => BreakType::LineClearAll,
        other => return Err(bad(format!("unknown break kind {other}"))),
    };
    run.add_break(doc, kind)?;
    run_text_value(doc, &run)
}

fn run_clear(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    run.clear(doc)?;
    run_text_value(doc, &run)
}

fn set_run_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    run.set_style(doc, step.param("style", 2).as_str())?;
    Ok(Value::Null)
}

fn hex_color(value: &str) -> Result<(u8, u8, u8)> {
    let byte = |i: usize| {
        value
            .get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .ok_or_else(|| bad(format!("bad color {value}")))
    };
    Ok((byte(0)?, byte(2)?, byte(4)?))
}

fn run_font(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    let changes = step
        .param("changes", 2)
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
            "highlight" => FontChange::Highlight(value.as_str().map(str::to_string)),
            "subscript" => FontChange::Subscript(value.as_bool()),
            "superscript" => FontChange::Superscript(value.as_bool()),
            flag => FontChange::Flag(flag.to_string(), value.as_bool()),
        });
    }
    run.set_font(doc, &out)?;
    Ok(Value::Null)
}

fn run_add_picture(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let run = run(doc, step.param("para", 0), step.param("run", 1))?;
    let bytes = image_bytes(step.param("image", 2))?;
    let shape = run.add_picture(
        doc,
        &bytes,
        step.param("width", 3).as_i64(),
        step.param("height", 4).as_i64(),
    )?;
    Ok(shape_json(&shape))
}

fn index(value: &Value, what: &str) -> Result<usize> {
    value
        .as_u64()
        .map(|v| v as usize)
        .ok_or_else(|| bad(format!("bad {what}")))
}

fn cell_set_text(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let table = table(doc, step.param("table", 0))?;
    let cell = table.cell(
        doc,
        index(step.param("row", 1), "row")?,
        index(step.param("col", 2), "col")?,
    )?;
    cell.set_text(doc, step.param("text", 3).as_str().unwrap_or(""))?;
    Ok(json!({"text": cell.text(doc)?}))
}

fn table_add_row(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let table = table(doc, step.param("table", 0))?;
    table.add_row(doc)?;
    table_summary(doc, &table)
}

fn table_add_column(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let table = table(doc, step.param("table", 0))?;
    let width = step
        .param("width", 1)
        .as_i64()
        .ok_or_else(|| bad("width"))?;
    table.add_column(doc, width)?;
    table_summary(doc, &table)
}

fn table_set_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let table = table(doc, step.param("table", 0))?;
    table.set_style(doc, step.param("style", 1).as_str())?;
    table_summary(doc, &table)
}

/// Adapter for an `ext.p4_*` authoring or image name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p4_characterize" => characterize,
        "ext.p4_scaled" => scaled,
        "ext.p4_add_paragraph" => add_paragraph,
        "ext.p4_add_heading" => add_heading,
        "ext.p4_add_page_break" => add_page_break,
        "ext.p4_add_table" => add_table,
        "ext.p4_add_picture" => add_picture,
        "ext.p4_add_section" => add_section,
        "ext.p4_add_run" => add_run,
        "ext.p4_insert_paragraph_before" => insert_paragraph_before,
        "ext.p4_set_paragraph_text" => set_paragraph_text,
        "ext.p4_clear_paragraph" => clear_paragraph,
        "ext.p4_set_paragraph_style" => set_paragraph_style,
        "ext.p4_set_alignment" => set_alignment,
        "ext.p4_paragraph_format" => paragraph_format,
        "ext.p4_set_run_text" => set_run_text,
        "ext.p4_run_add_text" => run_add_text,
        "ext.p4_run_add_tab" => run_add_tab,
        "ext.p4_run_add_break" => run_add_break,
        "ext.p4_run_clear" => run_clear,
        "ext.p4_set_run_style" => set_run_style,
        "ext.p4_run_font" => run_font,
        "ext.p4_run_add_picture" => run_add_picture,
        "ext.p4_cell_set_text" => cell_set_text,
        "ext.p4_table_add_row" => table_add_row,
        "ext.p4_table_add_column" => table_add_column,
        "ext.p4_table_set_style" => table_set_style,
        _ => return None,
    })
}
