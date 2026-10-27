//! Field authoring contracts (upstream `fields.py`, `_fieldcode.py`):
//! placeholders, update-on-open, TOC structure, self-consistency with the
//! in-field guard, refusal atomicity. Expected values are frozen from the
//! pinned oracle.

mod support;

use docrs::anchor::BlockTarget;
use docrs::fields::fieldcode;
use docrs::{Document, FindOptions, Limits, RefusalKind, View};
use support::*;

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";

#[test]
fn authors_page_and_count_fields_in_a_header() {
    let mut doc = open_fixture("generated/feature-isolated/header-footer-sections.docx");
    doc.add_page_number_field(BlockTarget::Text("Header for section one"))
        .unwrap();
    doc.add_page_count_field(BlockTarget::Text("Header for section one"))
        .unwrap();
    let (out, report) = doc.patch_bytes().unwrap();
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert!(changed.contains(&"word/settings.xml".to_string()));
    assert!(!changed.contains(&"word/document.xml".to_string()));
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    let header = reopened
        .find_one("Header for section one", &FindOptions::default())
        .unwrap();
    let part = part_xml(&reopened, header.story());
    assert!(part.contains(r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#));
    assert!(part_xml(&reopened, "word/settings.xml").contains(r#"<w:updateFields w:val="true"/>"#));
}

#[test]
fn authors_a_date_field_without_computing_a_date() {
    let mut doc = open_fixture(MINIMAL);
    doc.add_date_field(
        BlockTarget::Text("Second body paragraph"),
        Some("MMMM d, \"yyyy"),
    )
    .unwrap();
    let body = part_xml(&doc, "word/document.xml");
    assert!(body.contains(r#"w:instr=" DATE \@ &quot;MMMM d, yyyy&quot; ""#));
    assert!(body.contains("<w:t>(date)</w:t>"));
    let span = doc.find_one("(date)", &FindOptions::default()).unwrap();
    assert!(span.in_field());
    let err = doc
        .replace(&span, "July 8, 2026", &Default::default())
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
}

#[test]
fn authors_cross_references_in_three_kinds() {
    let mut doc = open_fixture("generated/feature-isolated/bookmarks.docx");
    let target = BlockTarget::Text("Paragraph after the bookmarks.");
    for kind in ["text", "page", "number"] {
        doc.add_reference_field(target, "DefinedTerm", kind)
            .unwrap();
    }
    let body = part_xml(&doc, "word/document.xml");
    assert!(body.contains(r#"w:instr=" REF DefinedTerm \h ""#));
    assert!(body.contains(r#"w:instr=" PAGEREF DefinedTerm \h ""#));
    assert!(body.contains(r#"w:instr=" REF DefinedTerm \r \h ""#));
    assert!(body.contains("<w:t>(reference)</w:t>"));
}

#[test]
fn refuses_unknown_bookmarks_and_kinds_atomically() {
    let mut doc = open_fixture(MINIMAL);
    let before = snapshot(&doc);
    let err = doc
        .add_reference_field(BlockTarget::Text("Second body paragraph"), "Ghost", "text")
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    assert!(err.message().contains("no bookmark"));
    let err = doc
        .add_reference_field(
            BlockTarget::Text("Second body paragraph"),
            "Ghost",
            "chapter",
        )
        .unwrap_err();
    assert!(err.message().contains("kind"));
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn inserts_a_toc_complex_field_marked_dirty() {
    let mut doc = open_fixture(MINIMAL);
    doc.insert_toc_after(BlockTarget::Text("Minimal Clean Document"), (1, 3))
        .unwrap();
    let (out, report) = doc.patch_bytes().unwrap();
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert_eq!(
        changed,
        vec![
            "word/document.xml".to_string(),
            "word/settings.xml".to_string()
        ]
    );
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    let body = part_xml(&reopened, "word/document.xml");
    assert!(body.contains(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin" w:dirty="true"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Table of contents placeholder — update fields to build it.</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    assert!(part_xml(&reopened, "word/settings.xml").contains("w:updateFields"));
    let mut reopened = reopened;
    let span = reopened
        .find_one("Table of contents placeholder", &FindOptions::default())
        .unwrap();
    assert!(span.in_field());
    assert!(
        reopened
            .replace(&span, "hand-edited toc", &Default::default())
            .is_err()
    );
}

#[test]
fn toc_refuses_bad_levels_history_and_non_body_anchors() {
    let mut doc = open_fixture(MINIMAL);
    let before = snapshot(&doc);
    let err = doc
        .insert_toc_after(BlockTarget::Text("Minimal Clean Document"), (3, 1))
        .unwrap_err();
    assert!(err.message().contains("levels"));
    for view in [View::Original, View::All] {
        let block = doc
            .blocks(view)
            .unwrap()
            .into_iter()
            .find(|b| b.text == "Minimal Clean Document")
            .unwrap();
        let err = doc
            .insert_toc_after(BlockTarget::Block(&block), (1, 3))
            .unwrap_err();
        assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
        let span = doc
            .find_one(
                "Minimal Clean Document",
                &FindOptions {
                    view,
                    ..FindOptions::default()
                },
            )
            .unwrap();
        assert!(
            doc.insert_toc_after(BlockTarget::Span(&span), (1, 3))
                .is_err()
        );
    }
    assert_eq!(snapshot(&doc), before);

    let mut headers = open_fixture("generated/feature-isolated/header-footer-sections.docx");
    let err = headers
        .insert_toc_after(BlockTarget::Text("Header for section one"), (1, 3))
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
}

#[test]
fn refuses_field_authoring_on_protected_documents() {
    let mut doc = open_fixture("generated/feature-isolated/protected-readonly.docx");
    let before = snapshot(&doc);
    let err = doc
        .add_page_number_field(BlockTarget::Text("This template is locked"))
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::DocumentProtected));
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn captions_set_the_style_and_a_seq_field() {
    let mut doc = open_fixture(MINIMAL);
    doc.add_caption(
        BlockTarget::Text("Second body paragraph"),
        "Table 2-b",
        "Totals",
    )
    .unwrap();
    let body = part_xml(&doc, "word/document.xml");
    assert!(body.contains(r#"<w:pStyle w:val="Caption"/>"#));
    assert!(body.contains(r#"<w:t xml:space="preserve">Table 2-b </w:t>"#));
    assert!(body.contains(r#"w:instr=" SEQ Table2b \* ARABIC ""#));
    assert!(body.contains("<w:t>: Totals</w:t>"));
    let err = doc
        .add_caption(BlockTarget::Text("Second body paragraph"), "", "")
        .unwrap_err();
    assert!(err.message().contains("label"));
}

#[test]
fn creates_the_settings_part_when_missing() {
    let mut doc = Document::open_bytes(
        &minimal_package_with(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Alpha</w:t></w:r></w:p></w:body></w:document>"#,
        ),
        &Limits::default(),
    )
    .unwrap();
    doc.add_page_number_field(BlockTarget::Text("Alpha"))
        .unwrap();
    let (_, report) = doc.patch_bytes().unwrap();
    let mut added = report.added_parts.clone();
    added.sort();
    assert_eq!(
        added,
        vec![
            "word/_rels/document.xml.rels".to_string(),
            "word/settings.xml".to_string()
        ]
    );
    assert!(part_xml(&doc, "word/settings.xml").contains(r#"w:updateFields w:val="true""#));
}

#[test]
fn fieldcode_parses_and_rewrites_like_upstream() {
    let operands = fieldcode::bookmark_operands(" REF Clause1 \\h ", &[]).unwrap();
    assert_eq!(operands.len(), 1);
    assert_eq!(
        (
            operands[0].value.as_str(),
            operands[0].start,
            operands[0].end
        ),
        ("Clause1", 5, 12)
    );
    let quoted = fieldcode::bookmark_operands(" HYPERLINK \\l \"Sec 2\" \\o tip ", &[]).unwrap();
    assert_eq!(quoted[0].value, "Sec 2");
    assert!(quoted[0].quoted);
    let formula =
        fieldcode::bookmark_operands(" = Total * 2 + SUM(Total) + \"Total\" ", &["total"]).unwrap();
    let starts: Vec<usize> = formula.iter().map(|o| o.start).collect();
    assert_eq!(starts, vec![3, 19]);
    assert!(
        fieldcode::bookmark_operands(" EMBED Excel.Sheet ", &["EMBED"])
            .unwrap()
            .is_empty()
    );
    assert!(fieldcode::bookmark_operands(" REF \"unterminated ", &[]).is_err());
    assert_eq!(
        fieldcode::rewrite_bookmark_operands(" REF Old \\h ", &[("old", "New Name")]).unwrap(),
        " REF \"New Name\" \\h "
    );
    assert_eq!(
        fieldcode::rewrite_command_operand(" STYLEREF Old \\h ", "STYLEREF", &[("old", "Renamed")])
            .unwrap(),
        " STYLEREF Renamed \\h "
    );
}
