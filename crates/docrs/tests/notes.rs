//! Footnotes and endnotes (upstream `notes.py`): part creation, ids, marks,
//! stacking order, refusal atomicity and span reuse. Expected values are
//! frozen from the pinned oracle (see `tests/ops/m4-notes-*.json`).

mod support;

use docrs::{Document, FindOptions, Limits, RefusalKind};
use support::*;

fn open_fixture(rel: &str) -> Document {
    Document::open_bytes(&fixture(rel), &Limits::default()).unwrap()
}

fn snapshot(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

fn xml(doc: &Document, part: &str) -> String {
    String::from_utf8(docrs::xml::serialize(doc.package().xml(part).unwrap())).unwrap()
}

fn find(doc: &Document, text: &str) -> docrs::Span {
    doc.find_one(text, &FindOptions::default()).unwrap()
}

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";

#[test]
fn creates_note_parts_bodies_and_marks() {
    let mut doc = open_fixture(MINIMAL);
    let mut span = find(&doc, "perfectly ordinary");
    assert_eq!(doc.add_footnote(&mut span, "A & B <C>").unwrap(), 1);
    assert_eq!(doc.add_endnote(&mut span, "An endnote.").unwrap(), 1);
    assert_eq!(
        xml(&doc, "word/footnotes.xml"),
        concat!(
            r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>"#,
            r#"<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#,
            r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> A &amp; B &lt;C&gt;</w:t></w:r></w:p></w:footnote>"#,
            r#"</w:footnotes>"#
        )
    );
    assert!(xml(&doc, "word/document.xml").contains(concat!(
        r#"<w:r><w:t>perfectly ordinary</w:t></w:r>"#,
        r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteReference w:id="1"/></w:r>"#,
        r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:endnoteReference w:id="1"/></w:r>"#,
        r#"<w:r><w:t xml:space="preserve"> text.</w:t></w:r>"#
    )));
    let (bytes, report) = doc.patch_bytes().unwrap();
    let mut added = report.added_parts.clone();
    added.sort();
    assert_eq!(added, vec!["word/endnotes.xml", "word/footnotes.xml"]);
    let reopened = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    let stories: Vec<String> = reopened.stories().into_iter().map(|s| s.name).collect();
    assert!(stories.contains(&"word/footnotes.xml".to_string()));
    assert!(stories.contains(&"word/endnotes.xml".to_string()));
}

#[test]
fn stacked_marks_keep_insertion_order() {
    let mut doc = open_fixture(MINIMAL);
    let mut span = find(&doc, "perfectly ordinary");
    let first = doc.add_footnote(&mut span, "First").unwrap();
    let second = doc.add_footnote(&mut span, "Second").unwrap();
    assert_eq!((first, second), (1, 2));
    let body = xml(&doc, "word/document.xml");
    let a = body.find(r#"<w:footnoteReference w:id="1"/>"#).unwrap();
    let b = body.find(r#"<w:footnoteReference w:id="2"/>"#).unwrap();
    assert!(a < b);
}

#[test]
fn ids_follow_existing_notes() {
    let mut doc = open_fixture("generated/feature-isolated/footnotes-endnotes.docx");
    let mut span = find(&doc, "This sentence carries a footnote reference.");
    assert_eq!(doc.add_footnote(&mut span, "stacked").unwrap(), 2);
    let body = xml(&doc, "word/document.xml");
    let existing = body.find(r#"<w:footnoteReference w:id="1"/>"#).unwrap();
    let added = body.find(r#"<w:footnoteReference w:id="2"/>"#).unwrap();
    assert!(existing < added, "new marks follow existing note marks");
}

#[test]
fn refusals_leave_the_package_unchanged() {
    let mut doc = open_fixture("generated/feature-isolated/header-footer-sections.docx");
    let before = snapshot(&doc);
    let mut header = doc
        .find_one(
            "Header for section one",
            &FindOptions {
                story: Some("word/header1.xml".into()),
                ..FindOptions::default()
            },
        )
        .unwrap();
    assert_eq!(
        doc.add_footnote(&mut header, "nope").unwrap_err().reason(),
        "note_outside_body"
    );
    let mut body = find(&doc, "Section one");
    assert_eq!(
        doc.add_footnote(&mut body, "").unwrap_err().code(),
        "invalid_argument"
    );
    assert_eq!(
        doc.add_endnote(&mut body, "a\tb").unwrap_err().code(),
        "invalid_argument"
    );
    assert_eq!(snapshot(&doc), before);

    let mut boxed = open_fixture("generated/feature-isolated/textbox.docx");
    let before = snapshot(&boxed);
    let mut span = find(&boxed, "living inside the text box");
    assert_eq!(
        boxed.add_footnote(&mut span, "x").unwrap_err().reason(),
        "note_in_text_box"
    );
    assert_eq!(snapshot(&boxed), before);

    let mut fields = open_fixture("generated/feature-isolated/fields.docx");
    let before = snapshot(&fields);
    let mut span = find(&fields, "June 1, 2026");
    assert_eq!(
        fields.add_footnote(&mut span, "x").unwrap_err().reason(),
        "span_in_field_result"
    );
    assert_eq!(snapshot(&fields), before);

    let mut other = find(&open_fixture(MINIMAL), "perfectly ordinary");
    let mut doc = open_fixture(MINIMAL);
    assert_eq!(
        doc.add_footnote(&mut other, "x")
            .unwrap_err()
            .refusal_kind(),
        Some(RefusalKind::BoundaryViolation)
    );
}
