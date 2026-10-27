//! Numbering contracts (upstream `numbering.py`): report shape, application,
//! canonical authoring, restarts, refusal atomicity. Expected values are
//! frozen from the pinned oracle.

mod support;

use docrs::anchor::BlockTarget;
use docrs::{Document, ErrorKind, FindOptions, Limits, RefusalKind};
use support::*;

const W_DECL: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn refusal(err: &docrs::Error) -> Option<RefusalKind> {
    err.refusal_kind()
}

#[test]
fn reports_definitions_and_numbered_paragraphs() {
    let doc = open_fixture("generated/feature-isolated/numbering-custom.docx");
    let report = doc.list_numbering().unwrap();
    let custom = report.definitions.iter().find(|d| d.num_id == 42).unwrap();
    assert_eq!(custom.abstract_num_id, 90);
    let levels: Vec<(i64, Option<&str>)> = custom
        .levels
        .iter()
        .map(|l| (l.level, l.num_fmt.as_deref()))
        .collect();
    assert_eq!(levels, vec![(0, Some("decimal")), (1, Some("lowerLetter"))]);
    let numbered: Vec<(i64, i64, &str)> = report
        .numbered_paragraphs
        .iter()
        .map(|p| (p.num_id, p.level, p.text.as_str()))
        .collect();
    assert_eq!(
        numbered,
        vec![
            (42, 0, "First numbered item"),
            (42, 0, "Second numbered item"),
            (42, 1, "Nested lettered item"),
        ]
    );
    let json = report.to_json();
    assert_eq!(json["schema"], "paper_numbering");
    assert_eq!(json["version"], 2);
    assert_eq!(json["definitions"].as_array().unwrap().len(), 10);
    assert_eq!(json["numbered_paragraphs"][0]["index"], 1);
    assert!(json["numbered_paragraphs"][0]["table_cell"].is_null());
    assert_eq!(json, doc.list_numbering().unwrap().to_json());
}

#[test]
fn reads_the_libreoffice_remapped_definition() {
    let doc = open_fixture("libreoffice/feature-isolated/numbering-custom.docx");
    let ids: Vec<(i64, String)> = doc
        .list_numbering()
        .unwrap()
        .numbered_paragraphs
        .into_iter()
        .map(|p| (p.num_id, p.text))
        .collect();
    assert_eq!(
        ids,
        vec![
            (7, "First numbered item".to_string()),
            (7, "Second numbered item".to_string()),
            (7, "Nested lettered item".to_string()),
        ]
    );
}

#[test]
fn applies_an_existing_definition_and_reopens() {
    let mut doc = open_fixture("generated/feature-isolated/numbering-custom.docx");
    doc.apply_numbering(
        BlockTarget::Text("Introduction to the hand-numbered"),
        42,
        1,
    )
    .unwrap();
    let (out, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    let numbered = reopened.list_numbering().unwrap().numbered_paragraphs;
    assert_eq!(numbered[0].text, "Introduction to the hand-numbered list.");
    assert_eq!((numbered[0].num_id, numbered[0].level), (42, 1));
}

#[test]
fn refuses_undefined_definitions_and_levels_atomically() {
    let mut doc = open_fixture("generated/feature-isolated/numbering-custom.docx");
    let before = snapshot(&doc);
    let err = doc
        .apply_numbering(BlockTarget::Text("Introduction"), 999, 0)
        .unwrap_err();
    assert_eq!(refusal(&err), Some(RefusalKind::TargetNotFound));
    let err = doc
        .apply_numbering(BlockTarget::Text("Introduction"), 42, 7)
        .unwrap_err();
    assert!(err.message().contains("level 7"));
    assert_eq!(snapshot(&doc), before);
}

fn bare() -> Document {
    Document::open_bytes(
        &minimal_package_with(&format!(
            r#"<w:document {W_DECL}><w:body><w:p><w:r><w:t>Alpha paragraph</w:t></w:r></w:p></w:body></w:document>"#
        )),
        &Limits::default(),
    )
    .unwrap()
}

#[test]
fn creates_a_numbering_part_in_a_definitionless_document() {
    let mut doc = bare();
    let before = snapshot(&doc);
    let err = doc
        .apply_numbering(BlockTarget::Text("Alpha"), 1, 0)
        .unwrap_err();
    assert!(err.message().contains("does not exist"));
    assert_eq!(snapshot(&doc), before);
    let num_id = doc.ensure_bullet_definition().unwrap();
    assert_eq!(num_id, 1);
    assert_eq!(doc.ensure_bullet_definition().unwrap(), 1);
    doc.apply_numbering(BlockTarget::Text("Alpha"), num_id, 0)
        .unwrap();
    let (out, report) = doc.patch_bytes().unwrap();
    let mut added = report.added_parts.clone();
    added.sort();
    assert_eq!(
        added,
        vec![
            "word/_rels/document.xml.rels".to_string(),
            "word/numbering.xml".to_string()
        ]
    );
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    assert!(part_xml(&reopened, "word/numbering.xml").contains(r#"w:val="bullet""#));
    let report = reopened.list_numbering().unwrap();
    assert_eq!(report.numbered_paragraphs.len(), 1);
    assert_eq!(report.definitions[0].levels.len(), 3);
}

#[test]
fn ensure_is_idempotent_and_kinds_differ() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    let bullet = doc.ensure_bullet_definition().unwrap();
    assert_eq!(bullet, doc.ensure_bullet_definition().unwrap());
    let decimal = doc.ensure_decimal_definition().unwrap();
    assert_ne!(decimal, bullet);
    assert_eq!(decimal, doc.ensure_decimal_definition().unwrap());
    // Frozen from the oracle: minimal.docx defines numIds 1..=9.
    assert_eq!(bullet, 10);
    assert_eq!(decimal, 11);
}

#[test]
fn restarts_numbering_with_a_fresh_definition() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    let num_id = doc.ensure_decimal_definition().unwrap();
    doc.apply_numbering(BlockTarget::Text("First body paragraph"), num_id, 0)
        .unwrap();
    let restarted = doc.restart_numbering(num_id).unwrap();
    assert_ne!(restarted, num_id);
    doc.apply_numbering(BlockTarget::Text("Second body paragraph"), restarted, 0)
        .unwrap();
    let xml = part_xml(&doc, "word/numbering.xml");
    let needle = format!(r#"<w:num w:numId="{restarted}">"#);
    let at = xml.find(&needle).unwrap();
    assert!(xml[at..].starts_with(&format!(
        r#"{needle}<w:abstractNumId w:val="9"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>"#
    )));
}

#[test]
fn refuses_restarting_missing_or_customized_definitions() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    let before = snapshot(&doc);
    let err = doc.restart_numbering(999).unwrap_err();
    assert_eq!(refusal(&err), Some(RefusalKind::TargetNotFound));
    assert_eq!(snapshot(&doc), before);
    let num_id = doc.ensure_decimal_definition().unwrap();
    let restarted = doc.restart_numbering(num_id).unwrap();
    let edited = snapshot(&doc);
    let err = doc.restart_numbering(restarted).unwrap_err();
    assert_eq!(refusal(&err), Some(RefusalKind::UnsupportedStructure));
    assert!(err.message().contains("level overrides"));
    assert_eq!(snapshot(&doc), edited);
}

#[test]
fn applies_list_styles_by_ui_name() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    let before = snapshot(&doc);
    let err = doc
        .apply_list_style(
            BlockTarget::Text("Second body paragraph"),
            "No Such List Style",
        )
        .unwrap_err();
    assert_eq!(refusal(&err), Some(RefusalKind::TargetNotFound));
    assert_eq!(snapshot(&doc), before);
    doc.apply_list_style(BlockTarget::Text("Second body paragraph"), "Heading 2")
        .unwrap();
    let block = doc
        .blocks(docrs::View::Current)
        .unwrap()
        .into_iter()
        .find(|b| b.text.starts_with("Second body paragraph"))
        .unwrap();
    assert_eq!(block.style_id.as_deref(), Some("Heading2"));
}

#[test]
fn reports_numbered_table_cells_with_addresses() {
    let mut doc = Document::open_bytes(
        &minimal_package_with(&format!(
            r#"<w:document {W_DECL}><w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>numbered cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#
        )),
        &Limits::default(),
    )
    .unwrap();
    let num_id = doc.ensure_bullet_definition().unwrap();
    doc.apply_numbering(BlockTarget::Text("numbered cell"), num_id, 0)
        .unwrap();
    let report = doc.list_numbering().unwrap();
    assert_eq!(report.numbered_paragraphs.len(), 1);
    assert_eq!(report.numbered_paragraphs[0].text, "numbered cell");
    assert_eq!(report.numbered_paragraphs[0].table_cell, Some((0, 0, 0)));
}

#[test]
fn protection_and_stale_targets_refuse_without_change() {
    let mut doc = open_fixture("generated/feature-isolated/protected-readonly.docx");
    let before = snapshot(&doc);
    let err = doc.ensure_bullet_definition().unwrap_err();
    assert_eq!(refusal(&err), Some(RefusalKind::DocumentProtected));
    assert_eq!(snapshot(&doc), before);

    let mut doc = open_fixture("generated/feature-isolated/numbering-custom.docx");
    let span = doc
        .find_one("Introduction", &FindOptions::default())
        .unwrap();
    let other = open_fixture("generated/feature-isolated/numbering-custom.docx");
    let foreign = other
        .find_one("Introduction", &FindOptions::default())
        .unwrap();
    let err = doc
        .apply_numbering(BlockTarget::Span(&foreign), 42, 0)
        .unwrap_err();
    assert_eq!(refusal(&err), Some(RefusalKind::BoundaryViolation));
    doc.apply_numbering(BlockTarget::Span(&span), 42, 0)
        .unwrap();
    // The live span still addresses the same paragraph (only pPr changed).
    doc.apply_numbering(BlockTarget::Span(&span), 42, 1)
        .unwrap();
    let historical = doc
        .find_one(
            "Introduction",
            &FindOptions {
                view: docrs::View::Original,
                ..FindOptions::default()
            },
        )
        .unwrap();
    let err = doc
        .apply_numbering(BlockTarget::Span(&historical), 42, 0)
        .unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::UnsupportedStructure)
    );
}
