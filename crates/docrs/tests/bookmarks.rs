//! Bookmarks (upstream `bookmarks.py`): listing, exact-span creation with
//! edge-run isolation, reference-aware deletion, refusal atomicity and span
//! freshness. Expected values are frozen from the pinned oracle (see
//! `tests/ops/m4-bookmarks-*.json`).

mod support;

use docrs::{Document, FindOptions, Limits, RefusalKind};
use serde_json::json;
use support::*;

const DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn open_fixture(rel: &str) -> Document {
    Document::open_bytes(&fixture(rel), &Limits::default()).unwrap()
}

fn doc_with(body: &str) -> Document {
    let bytes = minimal_package_with(&format!(
        r#"<w:document {DECLS}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    ));
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

fn snapshot(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

fn body_xml(doc: &Document) -> String {
    String::from_utf8(docrs::xml::serialize(
        doc.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap()
}

fn find(doc: &Document, text: &str) -> docrs::Span {
    doc.find_one(text, &FindOptions::default()).unwrap()
}

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";
const BOOKMARKS: &str = "generated/feature-isolated/bookmarks.docx";

#[test]
fn lists_bookmarks_with_their_wrapped_text() {
    let doc = open_fixture(BOOKMARKS);
    let listed: Vec<_> = doc
        .bookmarks()
        .unwrap()
        .iter()
        .map(|b| b.to_json())
        .collect();
    assert_eq!(
        listed,
        vec![
            json!({"name": "DefinedTerm", "id": 1, "story": "word/document.xml", "text": "the Master Agreement", "is_point": false}),
            json!({"name": "_GoBack", "id": 2, "story": "word/document.xml", "text": "", "is_point": true}),
        ]
    );
}

#[test]
fn current_view_text_and_rich_run_content() {
    let doc = doc_with(concat!(
        r#"<w:p><w:bookmarkStart w:id="1" w:name="CurrentText"/><w:r><w:t>A</w:t></w:r><w:moveFrom w:id="2"><w:r><w:t>B</w:t></w:r></w:moveFrom><w:r><w:t>C</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#,
        r#"<w:p><w:bookmarkStart w:id="12" w:name="RichAnchor"/><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t><w:br/><w:t>C</w:t><w:noBreakHyphen/><w:t>D</w:t></w:r><w:bookmarkEnd w:id="12"/></w:p>"#,
        r#"<w:p><w:bookmarkStart w:id="11" w:name="Across"/><w:r><w:t>one</w:t></w:r></w:p><w:p><w:r><w:t>two</w:t></w:r><w:bookmarkEnd w:id="11"/></w:p>"#,
    ));
    let texts: Vec<(String, String)> = doc
        .bookmarks()
        .unwrap()
        .into_iter()
        .map(|b| (b.name, b.text))
        .collect();
    assert_eq!(
        texts,
        vec![
            ("CurrentText".into(), "AC".into()),
            ("RichAnchor".into(), "A\tB\nC-D".into()),
            ("Across".into(), "one\ntwo".into()),
        ]
    );
}

#[test]
fn creates_a_bookmark_on_an_exact_span() {
    let mut doc = open_fixture(MINIMAL);
    let mut span = find(&doc, "perfectly ordinary");
    let info = doc.create_bookmark(&mut span, "KeyPhrase").unwrap();
    assert_eq!(
        info.to_json(),
        json!({"name": "KeyPhrase", "id": 1, "story": "word/document.xml", "text": "perfectly ordinary", "is_point": false})
    );
    assert!(body_xml(&doc).contains(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">First body paragraph with </w:t></w:r>"#,
        r#"<w:bookmarkStart w:id="1" w:name="KeyPhrase"/><w:r><w:t>perfectly ordinary</w:t></w:r><w:bookmarkEnd w:id="1"/>"#,
        r#"<w:r><w:t xml:space="preserve"> text.</w:t></w:r></w:p>"#
    )));
    let (bytes, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    let reopened = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    assert_eq!(reopened.bookmarks().unwrap()[0].text, "perfectly ordinary");
    assert!(
        reopened
            .find_one(
                "First body paragraph with perfectly ordinary text.",
                &FindOptions::default()
            )
            .is_ok()
    );
    // The span follows the split and stays usable, as upstream's does.
    assert_eq!(doc.add_footnote(&mut span, "A note.").unwrap(), 1);
}

#[test]
fn ids_are_unique_and_names_case_insensitive() {
    let mut doc = open_fixture(BOOKMARKS);
    let mut span = find(&doc, "for definitions");
    let info = doc.create_bookmark(&mut span, "SecondMark").unwrap();
    assert_eq!(info.bookmark_id, 3);
    let before = snapshot(&doc);
    let mut again = find(&doc, "Paragraph after");
    for name in ["DefinedTerm", "definedterm", "SECONDMARK"] {
        let err = doc.create_bookmark(&mut again, name).unwrap_err();
        assert_eq!(err.reason(), "duplicate_bookmark_name");
    }
    for name in [
        "1starts-with-digit",
        "has space",
        "",
        &"x".repeat(41),
        "_hidden",
    ] {
        let err = doc.create_bookmark(&mut again, name).unwrap_err();
        assert_eq!(err.code(), "invalid_argument", "{name:?}");
    }
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn creation_refusals_are_atomic() {
    let mut doc = doc_with(concat!(
        r#"<w:p><w:r><w:t>A</w:t></w:r><w:del w:id="1"><w:r><w:delText>B</w:delText></w:r></w:del><w:r><w:t>C</w:t></w:r></w:p>"#,
        r#"<w:p><w:ins w:id="2"><w:r><w:t xml:space="preserve">pending </w:t></w:r></w:ins><w:r><w:t>settled</w:t></w:r></w:p>"#,
        r#"<w:p><w:fldSimple w:instr=" DATE "><w:r><w:t>June 1, 2026</w:t></w:r></w:fldSimple></w:p>"#,
    ));
    let before = snapshot(&doc);
    for (text, reason) in [
        ("AC", "crosses_revision_content"),
        ("pending settled", "crosses_revision_scopes"),
        ("June 1, 2026", "span_in_field_result"),
    ] {
        let mut span = find(&doc, text);
        let err = doc.create_bookmark(&mut span, "Mark").unwrap_err();
        assert_eq!(err.reason(), reason, "{text}");
        assert_eq!(snapshot(&doc), before, "{text}");
    }
    let mut foreign = find(&open_fixture(MINIMAL), "perfectly ordinary");
    let err = doc.create_bookmark(&mut foreign, "Mark").unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
}

#[test]
fn deletes_markers_and_keeps_text() {
    let mut doc = open_fixture(BOOKMARKS);
    doc.delete_bookmark("definedterm").unwrap();
    let names: Vec<String> = doc
        .bookmarks()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert_eq!(names, vec!["_GoBack".to_string()]);
    assert!(
        doc.find_one(
            "See the Master Agreement for definitions.",
            &FindOptions::default()
        )
        .is_ok()
    );
    assert_eq!(
        doc.delete_bookmark("Nonexistent")
            .unwrap_err()
            .refusal_kind(),
        Some(RefusalKind::TargetNotFound)
    );
}

#[test]
fn deletion_honors_references_and_marker_hygiene() {
    let mark = |id: u32, name: &str| {
        format!(
            r#"<w:p><w:bookmarkStart w:id="{id}" w:name="{name}"/><w:r><w:t>{name} text</w:t></w:r><w:bookmarkEnd w:id="{id}"/></w:p>"#
        )
    };
    let mut doc = doc_with(&[
        mark(1, "Referenced"),
        mark(2, "Implicit"),
        mark(3, "Quoted"),
        mark(4, "Split"),
        mark(5, "Anchor"),
        mark(6, "Embed"),
        r#"<w:p><w:fldSimple w:instr=" REF Referenced \h "/></w:p>"#.into(),
        r#"<w:p><w:fldSimple w:instr=" Implicit \* MERGEFORMAT "/></w:p>"#.into(),
        r#"<w:p><w:fldSimple w:instr=' QUOTE "literal REF Quoted text" '/></w:p>"#.into(),
        r#"<w:p><w:fldSimple w:instr=" EMBED Word.Document.12 "/></w:p>"#.into(),
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> REF </w:instrText></w:r><w:r><w:instrText xml:space="preserve">Split \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#.into(),
        r#"<w:p><w:hyperlink w:anchor="Anchor"><w:r><w:t>jump</w:t></w:r></w:hyperlink></w:p>"#.into(),
    ].concat());
    let before = snapshot(&doc);
    for name in ["Referenced", "Implicit", "Split", "Anchor"] {
        assert_eq!(
            doc.delete_bookmark(name).unwrap_err().reason(),
            "bookmark_referenced",
            "{name}"
        );
        assert_eq!(snapshot(&doc), before);
    }
    doc.delete_bookmark("Quoted").unwrap();
    doc.delete_bookmark("Embed").unwrap();

    let mut dup = doc_with(
        r#"<w:p><w:r><w:t>targets</w:t></w:r><w:bookmarkStart w:id="5" w:name="target"/><w:bookmarkEnd w:id="5"/><w:bookmarkStart w:id="05" w:name="unrelated"/><w:bookmarkEnd w:id="05"/></w:p>"#,
    );
    let before = snapshot(&dup);
    assert_eq!(
        dup.delete_bookmark("target").unwrap_err().reason(),
        "malformed_bookmark_ids"
    );
    assert_eq!(snapshot(&dup), before);

    let mut bad = doc_with(
        r#"<w:p><w:bookmarkStart w:id="1" w:name="Target"/><w:r><w:t>target</w:t></w:r><w:bookmarkEnd w:id="1"/><w:bookmarkStart w:id="bad" w:name="Other"/></w:p>"#,
    );
    assert_eq!(
        bad.delete_bookmark("Target").unwrap_err().reason(),
        "malformed_bookmark_ids"
    );
    assert_eq!(bad.bookmarks().unwrap_err().code(), "invalid_argument");

    let mut both = doc_with(
        r#"<w:p><w:bookmarkStart w:id="1" w:name="Dup"/><w:bookmarkEnd w:id="1"/></w:p><w:p><w:bookmarkStart w:id="2" w:name="Dup"/><w:bookmarkEnd w:id="2"/></w:p><w:p><w:bookmarkStart w:id="3" w:name="Case"/><w:bookmarkEnd w:id="3"/><w:bookmarkStart w:id="4" w:name="CASE"/><w:bookmarkEnd w:id="4"/></w:p>"#,
    );
    both.delete_bookmark("Dup").unwrap();
    assert_eq!(
        both.delete_bookmark("case").unwrap_err().reason(),
        "case_ambiguous_bookmark"
    );
    assert_eq!(both.bookmarks().unwrap().len(), 2);
}

#[test]
fn header_bookmarks_share_the_global_id_space() {
    let mut doc = open_fixture("generated/feature-isolated/header-footer-sections.docx");
    let mut body = find(&doc, "Section one");
    assert_eq!(
        doc.create_bookmark(&mut body, "BodyMark")
            .unwrap()
            .bookmark_id,
        1
    );
    let mut header = doc
        .find_one(
            "Header for section one",
            &FindOptions {
                story: Some("word/header1.xml".into()),
                ..FindOptions::default()
            },
        )
        .unwrap();
    let info = doc.create_bookmark(&mut header, "HeaderMark").unwrap();
    assert_eq!(
        (info.bookmark_id, info.story.as_str()),
        (2, "word/header1.xml")
    );
}
