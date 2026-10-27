//! `ext.p7_*` adapters: sections, headers, footers, settings and block
//! containers (oracle helpers in `tools/oracle/p7_sections.py`).

use docrs::authoring::SectionStart;
use docrs::sections::{Orientation, SectionChange, SectionSetup};
use docrs::{Document, Result};
use serde_json::{Value, json};

use super::super::authoring::image_bytes;
use crate::ops::inherited::{block_summary, container, hdrftr, index_arg, paragraph, pick};
use crate::ops::{OpFn, Step, bad};

fn setup_json(setup: &SectionSetup) -> Value {
    json!({
        "start_type": setup.start_type.as_str(),
        "orientation": setup.orientation.as_str(),
        "page_width": setup.page_width,
        "page_height": setup.page_height,
        "left_margin": setup.left_margin,
        "right_margin": setup.right_margin,
        "top_margin": setup.top_margin,
        "bottom_margin": setup.bottom_margin,
        "gutter": setup.gutter,
        "header_distance": setup.header_distance,
        "footer_distance": setup.footer_distance,
        "different_first_page_header_footer": setup.different_first_page_header_footer,
    })
}

fn sections(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let mut out = Vec::new();
    for section in doc.sections()? {
        out.push(setup_json(&section.setup(doc)?));
    }
    Ok(Value::Array(out))
}

fn section_set(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let section = pick(
        &doc.sections()?,
        index_arg(step.param("index", 0), "section")?,
        "section",
    )?;
    let changes = step
        .param("changes", 1)
        .as_array()
        .ok_or_else(|| bad("changes must be a list"))?;
    let mut out = Vec::new();
    for change in changes {
        let name = change[0].as_str().ok_or_else(|| bad("section property"))?;
        let value = &change[1];
        out.push(match name {
            "start_type" => {
                SectionChange::StartType(value.as_str().map(SectionStart::parse).transpose()?)
            }
            "orientation" => {
                SectionChange::Orientation(value.as_str().map(Orientation::parse).transpose()?)
            }
            "page_width" => SectionChange::PageWidth(value.as_i64()),
            "page_height" => SectionChange::PageHeight(value.as_i64()),
            "left_margin" => SectionChange::LeftMargin(value.as_i64()),
            "right_margin" => SectionChange::RightMargin(value.as_i64()),
            "top_margin" => SectionChange::TopMargin(value.as_i64()),
            "bottom_margin" => SectionChange::BottomMargin(value.as_i64()),
            "gutter" => SectionChange::Gutter(value.as_i64()),
            "header_distance" => SectionChange::HeaderDistance(value.as_i64()),
            "footer_distance" => SectionChange::FooterDistance(value.as_i64()),
            "different_first_page_header_footer" => {
                SectionChange::DifferentFirstPageHeaderFooter(value.as_bool().unwrap_or(false))
            }
            other => return Err(bad(format!("unknown section property {other}"))),
        });
    }
    section.set(doc, &out)?;
    Ok(setup_json(&section.setup(doc)?))
}

fn section_content(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let section = pick(
        &doc.sections()?,
        index_arg(step.param("index", 0), "section")?,
        "section",
    )?;
    let items = section.inner_content(doc)?;
    items
        .iter()
        .map(|item| block_summary(doc, item))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn hdrftr_linked(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let hf = hdrftr(doc, step.param("index", 0), step.param("part", 1))?;
    Ok(json!(hf.is_linked_to_previous(doc)?))
}

fn hdrftr_set_linked(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let hf = hdrftr(doc, step.param("index", 0), step.param("part", 1))?;
    let value = step
        .param("value", 2)
        .as_bool()
        .ok_or_else(|| bad("value must be a bool"))?;
    hf.set_linked_to_previous(doc, value)?;
    Ok(json!(hf.is_linked_to_previous(doc)?))
}

fn container_content(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = container(doc, step.param("container", 0))?;
    let items = target.inner_content(doc)?;
    items
        .iter()
        .map(|item| block_summary(doc, item))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn container_add_paragraph(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = container(doc, step.param("container", 0))?;
    let text = step.param("text", 1).as_str().unwrap_or("");
    let paragraph = target.add_paragraph(doc, text, step.param("style", 2).as_str())?;
    block_summary(doc, &docrs::authoring::BlockItem::Paragraph(paragraph))
}

fn container_add_table(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let key = step.param("container", 0);
    let rows = step.param("rows", 1).as_u64().ok_or_else(|| bad("rows"))? as usize;
    let cols = step.param("cols", 2).as_u64().ok_or_else(|| bad("cols"))? as usize;
    let width = step.param("width", 3);
    let table = if width.is_null() {
        // `_Cell.add_table(rows, cols)`
        let cell = key["cell"]
            .as_array()
            .ok_or_else(|| bad("width-less add_table needs a cell"))?;
        let table = crate::ops::inherited::table(doc, &cell[0])?;
        let row = index_arg(&cell[1], "row")? as usize;
        let col = index_arg(&cell[2], "col")? as usize;
        let cell = table.cell(doc, row, col)?;
        cell.add_table(doc, rows, cols)?
    } else {
        let target = container(doc, key)?;
        let width = width.as_i64().ok_or_else(|| bad("width"))?;
        target.add_table(doc, rows, cols, width)?
    };
    block_summary(doc, &docrs::authoring::BlockItem::Table(table))
}

fn paragraph_set_text(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    target.set_text(doc, step.param("text", 1).as_str())?;
    block_summary(doc, &docrs::authoring::BlockItem::Paragraph(target))
}

fn paragraph_set_style(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    target.set_style(doc, step.param("style", 1).as_str())?;
    block_summary(doc, &docrs::authoring::BlockItem::Paragraph(target))
}

fn paragraph_add_picture(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let bytes = image_bytes(step.param("image", 1))?;
    let run = target.add_run(doc, None, None)?;
    let shape = run.add_picture(
        doc,
        &bytes,
        step.param("width", 2).as_i64(),
        step.param("height", 3).as_i64(),
    )?;
    Ok(json!({"width": shape.width, "height": shape.height}))
}

fn odd_even(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    Ok(json!(doc.odd_and_even_pages_header_footer()?))
}

fn set_odd_even(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let value = step
        .param("value", 0)
        .as_bool()
        .ok_or_else(|| bad("value must be a bool"))?;
    doc.set_odd_and_even_pages_header_footer(value)?;
    Ok(json!(doc.odd_and_even_pages_header_footer()?))
}

/// Adapter for an `ext.p7_*` sections name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p7_sections" => sections,
        "ext.p7_section_set" => section_set,
        "ext.p7_section_content" => section_content,
        "ext.p7_hdrftr_linked" => hdrftr_linked,
        "ext.p7_hdrftr_set_linked" => hdrftr_set_linked,
        "ext.p7_container_content" => container_content,
        "ext.p7_container_add_paragraph" => container_add_paragraph,
        "ext.p7_container_add_table" => container_add_table,
        "ext.p7_paragraph_set_text" => paragraph_set_text,
        "ext.p7_paragraph_set_style" => paragraph_set_style,
        "ext.p7_paragraph_add_picture" => paragraph_add_picture,
        "ext.p7_odd_even" => odd_even,
        "ext.p7_set_odd_even" => set_odd_even,
        _ => return None,
    })
}
