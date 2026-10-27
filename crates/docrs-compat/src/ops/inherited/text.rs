//! `ext.p7_*` adapters: paragraph format, tab stops, fonts, inline shapes,
//! comment ranges and pictures from a path (oracle helpers in
//! `tools/oracle/p7_text.py`).

use docrs::authoring::{
    Length, ParagraphFragment, ParagraphItem, RgbColor, RunItem, TabStop, TabStopValues,
};
use docrs::{Document, Result};
use serde_json::{Value, json};

use crate::ops::inherited::{
    error_class, font_changes, font_json, index_arg, paragraph, parfmt_json, parfmt_props, pick,
    run, test_file,
};
use crate::ops::{OpFn, Step, bad};

fn parfmt(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    Ok(parfmt_json(&target.format(doc)?))
}

fn set_parfmt(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let format = parfmt_props(step.param("props", 1))?;
    target.set_format(doc, &format)?;
    Ok(parfmt_json(&target.format(doc)?))
}

fn font(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = run(doc, step.param("para", 0), step.param("run", 1))?;
    Ok(font_json(&target.font(doc)?))
}

fn set_font(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = run(doc, step.param("para", 0), step.param("run", 1))?;
    let changes = font_changes(step.param("changes", 2))?;
    target.set_font(doc, &changes)?;
    Ok(font_json(&target.font(doc)?))
}

fn tab_json(values: &TabStopValues) -> Value {
    json!({"position": values.position, "alignment": values.alignment, "leader": values.leader})
}

fn tabs_json(doc: &Document, tabs: &[TabStop]) -> Result<Value> {
    tabs.iter()
        .map(|tab| tab.values(doc).map(|v| tab_json(&v)))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn tab_stops(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let tabs = target.tab_stops(doc)?;
    tabs_json(doc, &tabs)
}

fn add_tab_stop(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let position = step
        .param("position", 1)
        .as_i64()
        .ok_or_else(|| bad("position"))?;
    let alignment = step.param("alignment", 2).as_str().unwrap_or("left");
    let leader = step.param("leader", 3).as_str().unwrap_or("none");
    let tab = target.add_tab_stop(doc, position, alignment, leader)?;
    let tabs = target.tab_stops(doc)?;
    Ok(json!({"added": tab_json(&tab.values(doc)?), "tabs": tabs_json(doc, &tabs)?}))
}

fn clear_tab_stops(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    target.clear_tab_stops(doc)?;
    let tabs = target.tab_stops(doc)?;
    tabs_json(doc, &tabs)
}

fn delete_tab_stop(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let index = index_arg(step.param("index", 1), "tab")?;
    error_class((|| {
        target.delete_tab_stop(doc, index)?;
        let tabs = target.tab_stops(doc)?;
        tabs_json(doc, &tabs)
    })())
}

fn set_tab_stop(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let tabs = target.tab_stops(doc)?;
    let tab = pick(&tabs, index_arg(step.param("index", 1), "tab")?, "tab")?;
    let value = step.param("value", 3);
    let tab = match step.param("prop", 2).as_str().unwrap_or("") {
        "position" => tab.set_position(doc, value.as_i64().ok_or_else(|| bad("position"))?)?,
        "alignment" => {
            tab.set_alignment(doc, value.as_str().ok_or_else(|| bad("alignment"))?)?;
            tab
        }
        "leader" => {
            tab.set_leader(doc, value.as_str())?;
            tab
        }
        other => return Err(bad(format!("unknown tab stop property {other}"))),
    };
    let tabs = target.tab_stops(doc)?;
    Ok(json!({"tab": tab_json(&tab.values(doc)?), "tabs": tabs_json(doc, &tabs)?}))
}

fn shape_json(doc: &Document, shape: &docrs::authoring::InlineShapeRef) -> Result<Value> {
    Ok(json!({
        "width": shape.width(doc)?,
        "height": shape.height(doc)?,
        "type": shape.shape_type(doc)?.name(),
    }))
}

fn inline_shapes(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    let shapes = doc.inline_shapes()?;
    shapes
        .iter()
        .map(|shape| shape_json(doc, shape))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn set_inline_shape(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let shapes = doc.inline_shapes()?;
    let shape = pick(
        &shapes,
        index_arg(step.param("index", 0), "shape")?,
        "inline shape",
    )?;
    let width = step.param("width", 1).as_i64();
    let height = step.param("height", 2).as_i64();
    error_class((|| {
        if let Some(cx) = width {
            shape.set_width(doc, cx)?;
        }
        if let Some(cy) = height {
            shape.set_height(doc, cy)?;
        }
        shape_json(doc, &shape)
    })())
}

fn mark_comment_range(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let first = run(doc, step.param("para", 0), step.param("run", 1))?;
    let last = run(doc, step.param("last_para", 2), step.param("last_run", 3))?;
    let id = step
        .param("comment_id", 4)
        .as_i64()
        .ok_or_else(|| bad("comment_id"))?;
    first.mark_comment_range(doc, &last, id)?;
    Ok(Value::Null)
}

fn add_picture_path(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let path = test_file(step.param("path", 0))?;
    let shape = doc.add_picture_file(
        &path,
        step.param("width", 1).as_i64(),
        step.param("height", 2).as_i64(),
    )?;
    Ok(json!({"width": shape.width, "height": shape.height}))
}

fn run_add_picture_path(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = run(doc, step.param("para", 0), step.param("run", 1))?;
    let path = test_file(step.param("path", 2))?;
    let shape = target.add_picture_file(
        doc,
        &path,
        step.param("width", 3).as_i64(),
        step.param("height", 4).as_i64(),
    )?;
    Ok(json!({"width": shape.width, "height": shape.height}))
}

fn fragment_json(result: Result<Option<ParagraphFragment>>) -> Result<Value> {
    match result {
        Ok(None) => Ok(Value::Null),
        Ok(Some(f)) => Ok(json!({"text": f.text, "runs": f.runs})),
        Err(err)
            if matches!(
                err.reason(),
                "not_first_page_break" | "no_rendered_page_break"
            ) =>
        {
            Ok(json!({"error": "ValueError"}))
        }
        Err(err) => Err(err),
    }
}

fn paragraph_content(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let mut items = Vec::new();
    for item in target.inner_content(doc)? {
        items.push(match item {
            ParagraphItem::Hyperlink(link) => json!({"hyperlink": {
                "address": link.address(doc)?,
                "fragment": link.fragment(doc)?,
                "url": link.url(doc)?,
                "text": link.text(doc)?,
                "runs": link.runs(doc)?.iter().map(|r| r.text(doc)).collect::<Result<Vec<_>>>()?,
                "contains_page_break": link.contains_page_break(doc)?,
            }}),
            ParagraphItem::Run(run) => {
                let content: Vec<Value> = run
                    .inner_content(doc)?
                    .into_iter()
                    .map(|item| match item {
                        RunItem::Text(text) => json!({"text": text}),
                        RunItem::Drawing(_) => json!({"drawing": true}),
                        RunItem::RenderedPageBreak(_) => json!({"page_break": true}),
                    })
                    .collect();
                json!({"run": {
                    "text": run.text(doc)?,
                    "contains_page_break": run.contains_page_break(doc)?,
                    "content": content,
                }})
            }
        });
    }
    let mut breaks = Vec::new();
    for page_break in target.rendered_page_breaks(doc)? {
        breaks.push(json!({
            "preceding": fragment_json(page_break.preceding_paragraph_fragment(doc))?,
            "following": fragment_json(page_break.following_paragraph_fragment(doc))?,
        }));
    }
    Ok(json!({
        "contains_page_break": target.contains_page_break(doc)?,
        "hyperlinks": target.hyperlinks(doc)?.len(),
        "items": items,
        "page_breaks": breaks,
    }))
}

fn set_hyperlink_address(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let index = step
        .param("index", 1)
        .as_u64()
        .ok_or_else(|| bad("index"))? as usize;
    let value = step
        .param("value", 2)
        .as_str()
        .ok_or_else(|| bad("value"))?;
    let link = target
        .hyperlinks(doc)?
        .into_iter()
        .nth(index)
        .ok_or_else(|| bad("hyperlink index out of range"))?;
    link.set_address(doc, value)?;
    Ok(
        json!({"address": link.address(doc)?, "fragment": link.fragment(doc)?, "url": link.url(doc)?}),
    )
}

fn image_from_file(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let path = test_file(step.param("path", 0))?;
    Ok(docrs::image::Image::from_file(&path)?.to_json())
}

fn style_names(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let runs = target
        .runs(doc)?
        .iter()
        .map(|run| run.style(doc).map(|s| s.and_then(|s| s.name)))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"paragraph": target.style(doc)?.and_then(|s| s.name), "runs": runs}))
}

fn table_style_name(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = crate::ops::inherited::table(doc, step.param("table", 0))?;
    Ok(json!(target.style(doc)?.and_then(|s| s.name)))
}

fn length(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let value = step.param("value", 1);
    let number = value.as_f64().ok_or_else(|| bad("value"))?;
    let length = match step.param("unit", 0).as_str().unwrap_or("") {
        "Emu" => Length::from_emu(value.as_i64().ok_or_else(|| bad("Emu needs an int"))?),
        "Inches" => Length::from_inches(number),
        "Cm" => Length::from_cm(number),
        "Mm" => Length::from_mm(number),
        "Pt" => Length::from_pt(number),
        "Twips" => Length::from_twips(number),
        "Length" => Length(value.as_i64().ok_or_else(|| bad("Length needs an int"))?),
        other => return Err(bad(format!("unknown unit {other}"))),
    };
    // Floats as IEEE-754 bit patterns (`p7_text._bits`).
    let bits = |value: f64| value.to_bits() as i64;
    Ok(json!({
        "emu": length.emu(),
        "cm": bits(length.cm()),
        "inches": bits(length.inches()),
        "mm": bits(length.mm()),
        "pt": bits(length.pt()),
        "twips": length.twips(),
    }))
}

fn rgb(_doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let text = step.param("text", 0).as_str().ok_or_else(|| bad("text"))?;
    Ok(json!(RgbColor::from_string(text)?.to_string()))
}

fn clear_body(doc: &mut Document, _step: &Step<'_>) -> Result<Value> {
    doc.clear_body_content()?;
    Ok(json!(doc.paragraphs()?.len() + doc.tables()?.len()))
}

fn paragraph_add_run(doc: &mut Document, step: &Step<'_>) -> Result<Value> {
    let target = paragraph(doc, step.param("para", 0))?;
    let run = target.add_run(
        doc,
        step.param("text", 1).as_str(),
        step.param("style", 2).as_str(),
    )?;
    Ok(json!({"text": run.text(doc)?}))
}

/// Adapter for an `ext.p7_*` text name.
pub fn lookup(name: &str) -> Option<OpFn> {
    Some(match name {
        "ext.p7_parfmt" => parfmt,
        "ext.p7_set_parfmt" => set_parfmt,
        "ext.p7_font" => font,
        "ext.p7_set_font" => set_font,
        "ext.p7_tab_stops" => tab_stops,
        "ext.p7_add_tab_stop" => add_tab_stop,
        "ext.p7_clear_tab_stops" => clear_tab_stops,
        "ext.p7_delete_tab_stop" => delete_tab_stop,
        "ext.p7_set_tab_stop" => set_tab_stop,
        "ext.p7_inline_shapes" => inline_shapes,
        "ext.p7_set_inline_shape" => set_inline_shape,
        "ext.p7_mark_comment_range" => mark_comment_range,
        "ext.p7_add_picture_path" => add_picture_path,
        "ext.p7_run_add_picture_path" => run_add_picture_path,
        "ext.p7_paragraph_add_run" => paragraph_add_run,
        "ext.p7_length" => length,
        "ext.p7_rgb" => rgb,
        "ext.p7_clear_body" => clear_body,
        "ext.p7_paragraph_content" => paragraph_content,
        "ext.p7_style_names" => style_names,
        "ext.p7_set_hyperlink_address" => set_hyperlink_address,
        "ext.p7_image_from_file" => image_from_file,
        "ext.p7_table_style_name" => table_style_name,
        _ => return None,
    })
}
