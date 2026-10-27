//! What a conversion will take from a reference document, read before any
//! conversion runs: the style each role resolves to, the branding lifted
//! from sample content, the section whose page setup, headers and footers
//! the output keeps, cover page, compatibility mode and lists, with the
//! problems a practice can fix in Word, in plain language.
//!
//! The roles, lists, lifting, cover page and page-number outcome come from a
//! trial conversion of a small probe document inside a transaction that is
//! discarded, so they are exactly what a real conversion would do.

use serde_json::{Value, json};

use super::ast;
use super::frontmatter::Metadata;
use super::render;
use super::stories::stories;
use super::{MarkdownOptions, MarkdownReport, Placement};
use crate::authoring::section_properties;
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Result};

/// Every block role a conversion uses, once.
const PROBE: &str = "# Heading 1\n\n## Heading 2\n\n### Heading 3\n\nFirst paragraph.\n\nBody text.\n\n\
- Bullet\n  - Nested bullet\n\n1. Number\n   1. Nested number\n\n> Quote\n\n\
| Column | Column |\n| --- | --- |\n| Cell | Cell |\n";

/// Word versions by `w:compatSetting compatibilityMode`.
fn word_version(mode: i64) -> &'static str {
    match mode {
        11 => "Word 2003",
        12 => "Word 2007",
        14 => "Word 2010",
        15 => "Word 2013 and later",
        _ => "an unknown Word version",
    }
}

fn twips(tree: &XmlTree, node: Option<NodeId>, attr: &str) -> Option<i64> {
    node.and_then(|n| w_attr(tree, n, attr))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as i64)
}

fn story_summary(tree: &XmlTree) -> Value {
    let root = tree.root();
    let mut images = 0;
    let mut tables = 0;
    let mut fields: Vec<String> = Vec::new();
    let mut text = String::new();
    for node in tree.descendants(root) {
        let local = tree.local_name(node).unwrap_or("");
        match local {
            "drawing" | "pict" if tree.ns_uri(node) == Some(W) => images += 1,
            "tbl" if tree.ns_uri(node) == Some(W) => tables += 1,
            "instrText" if tree.ns_uri(node) == Some(W) => {
                if let Some(name) = tree.text_content(node).split_whitespace().next() {
                    fields.push(name.to_ascii_uppercase());
                }
            }
            "fldSimple" if tree.ns_uri(node) == Some(W) => {
                if let Some(name) =
                    w_attr(tree, node, "instr").and_then(|i| i.split_whitespace().next())
                {
                    fields.push(name.to_ascii_uppercase());
                }
            }
            "t" if tree.ns_uri(node) == Some(W) => text.push_str(&tree.text_content(node)),
            _ => {}
        }
    }
    fields.sort();
    fields.dedup();
    json!({
        "images": images,
        "tables": tables,
        "fields": fields,
        "has_text": !text.trim().is_empty(),
    })
}

/// The header and footer of each type the output's section shows (the
/// template's page 1 on page 1, its body pages' after), summarised.
fn headers_footers(txn: &Txn<'_>, main: &str) -> Result<Value> {
    let mut out = json!({ "header": {}, "footer": {} });
    for story in stories(txn, main)? {
        let mut summary = match &story.part {
            Some(part) => story_summary(txn.tree(part)?),
            None => json!({ "missing_part": true }),
        };
        summary["inherited"] = json!(story.inherited);
        out[story.story][story.kind] = summary;
    }
    Ok(out)
}

/// Inspect `document` as a reference for a conversion with `options` (the
/// placement, style map, page numbers and cover page options matter).
/// Read-only: the trial conversion is discarded.
pub fn inspect(document: &Document, options: &MarkdownOptions) -> Result<Value> {
    let mut problems: Vec<Value> = Vec::new();
    let mut problem = |code: &str, message: String| {
        problems.push(json!({ "code": code, "message": message }));
    };
    let protection = crate::protection::status(document)?;
    if protection.blocks_paper_edits() {
        problem(
            "protected",
            "The template has Restrict Editing turned on; a conversion refuses unless protection is acknowledged.".into(),
        );
    }
    let package = document.package();
    let main = package.main_part_name().to_string();
    let txn = Txn::new(document);
    let tree = txn.tree(&main)?;
    let toc_title = render::reference_toc_title(&txn, &main)?;

    let sections = section_properties(tree)?;
    let section = match sections.last() {
        None => {
            problem(
                "no_section",
                "The template has no page setup (section properties); Word's defaults apply and tables use a 6-inch width.".into(),
            );
            Value::Null
        }
        Some(last) => {
            let pg_sz = w_child(tree, *last, "pgSz");
            let pg_mar = w_child(tree, *last, "pgMar");
            let width = twips(tree, pg_sz, "w").unwrap_or(12240);
            let height = twips(tree, pg_sz, "h").unwrap_or(15840);
            let landscape = w_attr(tree, pg_sz.unwrap_or(*last), "orient") == Some("landscape")
                || width > height;
            let margin = |side: &str| twips(tree, pg_mar, side);
            let left = margin("left").filter(|v| *v != 0).unwrap_or(1440);
            let right = margin("right").filter(|v| *v != 0).unwrap_or(1440);
            let title_page = super::stories::output_section(tree)?.title_page;
            if sections.len() > 1 {
                problem(
                    "multiple_sections",
                    format!(
                        "The template has {} sections. The output keeps the last one's page setup; its page 1 shows the first section's page 1 header and footer and its other pages the last section's, inherited from earlier sections where it lacks them. The earlier sections' layout is dropped with the sample content.",
                        sections.len()
                    ),
                );
            }
            json!({
                "count": sections.len(),
                "used": "last",
                "page": {
                    "width": width,
                    "height": height,
                    "orientation": if landscape { "landscape" } else { "portrait" },
                    "margins": {
                        "top": margin("top"), "right": margin("right"), "bottom": margin("bottom"),
                        "left": margin("left"), "header": margin("header"), "footer": margin("footer"),
                    },
                },
                "text_width": width - left - right,
                "title_page": title_page,
                "headers_footers": headers_footers(&txn, &main)?,
            })
        }
    };

    let mut compatibility = Value::Null;
    let mut even_and_odd = false;
    if let Some(settings) = txn.part_related_by(&main, rt::SETTINGS)? {
        let settings = txn.tree(&settings)?;
        even_and_odd = w_child(settings, settings.root(), "evenAndOddHeaders").is_some();
        let mode = w_child(settings, settings.root(), "compat")
            .into_iter()
            .flat_map(|compat| w_children(settings, compat, "compatSetting"))
            .find(|s| w_attr(settings, *s, "name") == Some("compatibilityMode"))
            .and_then(|s| w_attr(settings, s, "val"))
            .and_then(|v| v.parse::<i64>().ok());
        let mode = mode.unwrap_or(if w_child(settings, settings.root(), "compat").is_some() {
            12
        } else {
            15
        });
        if mode < 15 {
            problem(
                "old_compatibility_mode",
                format!(
                    "The template is in compatibility mode for {}; the output keeps it, so Word lays it out as that version did (save the template in a current format to change this).",
                    word_version(mode)
                ),
            );
        }
        compatibility = json!({ "mode": mode, "word": word_version(mode) });
    }

    let sample = super::styles::Sample::read(&txn)?;
    drop(txn);
    let parsed = ast::parse(PROBE, false)?;
    let trial_options = MarkdownOptions {
        placement: match options.placement {
            Placement::Placeholder(_) => Placement::Append,
            ref other => other.clone(),
        },
        toc: Some(false),
        title_block: false,
        set_properties: false,
        strict: false,
        ..options.clone()
    };
    let mut report = MarkdownReport::default();
    let mut trial = Txn::new(document);
    let outcome = render::render(
        &mut trial,
        &parsed,
        &Metadata::default(),
        &trial_options,
        false,
        &mut report,
    );
    drop(trial);
    if let Err(err) = &outcome {
        problem(
            "conversion_refused",
            format!(
                "A conversion into this template would refuse: {}",
                err.message()
            ),
        );
    }
    for warning in &report.warnings {
        if warning.code == "picture_larger_than_page" {
            problem(warning.code, warning.message.clone());
        }
    }
    for name in &report.created_styles {
        problem(
            "style_added",
            format!(
                "The template has no \"{name}\" style; the conversion adds one (from the sample content's look where there is one, else plain). Add or restyle it in the template to control its look."
            ),
        );
    }
    if report.page_numbers.as_deref() == Some("template_footer_kept") {
        let footer_fields = section
            .pointer("/headers_footers/footer/default/fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !footer_fields.iter().any(|f| f == "PAGE") {
            problem(
                "footer_without_page_number",
                "Page numbers were asked for, but the template's footer has no page number field and is kept unchanged.".into(),
            );
        }
    }
    if report.cover_page.as_deref() == Some("content_after_cover") {
        problem(
            "cover_page",
            "The template's first page is a cover; the content starts on page 2.".into(),
        );
    }
    let body_style = report
        .styles
        .get("body")
        .filter(|body| !body.created)
        .and_then(|body| body.style_id.as_deref());
    let styles = match package.part_related_by(&main, rt::STYLES)? {
        Some(name) => Some(package.xml(&name)?),
        None => None,
    };
    let lifted = sample.summary(styles, body_style);
    Ok(json!({
        "schema_version": 1,
        "roles": report.styles,
        "created_styles": report.created_styles,
        "lifted": lifted,
        "section": section,
        "even_and_odd_headers": even_and_odd,
        "compatibility": compatibility,
        "cover_page": report.cover_page,
        "page_numbers": report.page_numbers,
        "toc_title": toc_title,
        "protected": protection.blocks_paper_edits(),
        "problems": problems,
    }))
}
