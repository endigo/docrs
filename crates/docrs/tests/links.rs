//! Hyperlinks (upstream `links.py`): wrapping, relationships on the owning
//! story part, the Hyperlink character style, refusal atomicity and span
//! ownership/freshness. Expected values are frozen from the pinned oracle
//! (see `tests/ops/m4-links-*.json`).

mod support;

use docrs::{Document, FindOptions, Limits, RefusalKind, ReplaceOptions};
use support::*;

const DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const URL: &str = "https://example.com/a";
const HYPERLINK_RT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

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

fn xml(doc: &Document, part: &str) -> String {
    String::from_utf8(docrs::xml::serialize(doc.package().xml(part).unwrap())).unwrap()
}

fn find(doc: &Document, text: &str) -> docrs::Span {
    doc.find_one(text, &FindOptions::default()).unwrap()
}

fn hyperlink_rels(doc: &Document, source: &str) -> Vec<(String, String)> {
    doc.package()
        .relationships_at(source)
        .unwrap()
        .into_iter()
        .filter(|rel| rel.rel_type == HYPERLINK_RT && rel.external)
        .map(|rel| (rel.id, rel.target))
        .collect()
}

#[test]
fn wraps_a_phrase_and_defines_the_style() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    assert!(!xml(&doc, "word/styles.xml").contains(r#"w:styleId="Hyperlink""#));
    let span = find(&doc, "perfectly ordinary");
    let link = doc.add_hyperlink(&span, URL).unwrap();
    assert_eq!(
        (link.address.as_str(), link.text.as_str()),
        (URL, "perfectly ordinary")
    );
    let rels = hyperlink_rels(&doc, "/word/document.xml");
    assert_eq!(rels.len(), 1);
    let body = xml(&doc, "word/document.xml");
    assert!(body.contains(&format!(
        r#"<w:r><w:t xml:space="preserve">First body paragraph with </w:t></w:r><w:hyperlink r:id="{}" w:history="1"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>perfectly ordinary</w:t></w:r></w:hyperlink><w:r><w:t xml:space="preserve"> text.</w:t></w:r>"#,
        rels[0].0
    )), "{body}");
    assert!(xml(&doc, "word/styles.xml").ends_with(
        r#"<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/></w:style></w:styles>"#
    ));
    let (_, report) = doc.patch_bytes().unwrap();
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert_eq!(
        changed,
        vec![
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/styles.xml"
        ]
    );
    // The span now lies in a hyperlink: it is no longer a valid anchor.
    let err = doc.add_hyperlink(&span, URL).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    let again = find(&doc, "perfectly ordinary");
    assert_eq!(
        doc.add_hyperlink(&again, URL).unwrap_err().reason(),
        "already_hyperlinked"
    );
}

#[test]
fn reuses_a_matching_relationship() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    doc.add_hyperlink(&find(&doc, "perfectly ordinary"), URL)
        .unwrap();
    doc.add_hyperlink(&find(&doc, "First body paragraph"), URL)
        .unwrap();
    doc.add_hyperlink(&find(&doc, "Second body"), "https://example.com/b")
        .unwrap();
    let rels = hyperlink_rels(&doc, "/word/document.xml");
    assert_eq!(rels.len(), 2);
}

#[test]
fn creates_styles_part_and_header_relationships_when_absent() {
    let mut doc = doc_with(r#"<w:p><w:r><w:t>plain words</w:t></w:r></w:p>"#);
    doc.add_hyperlink(&find(&doc, "words"), URL).unwrap();
    let (_, report) = doc.patch_bytes().unwrap();
    let mut added = report.added_parts.clone();
    added.sort();
    assert_eq!(
        added,
        vec!["word/_rels/document.xml.rels", "word/styles.xml"]
    );

    let mut header = open_fixture("generated/feature-isolated/header-footer-sections.docx");
    let span = header
        .find_one(
            "Header for section one",
            &FindOptions {
                story: Some("word/header1.xml".into()),
                ..FindOptions::default()
            },
        )
        .unwrap();
    header
        .add_hyperlink(&span, "https://example.com/header")
        .unwrap();
    assert_eq!(
        hyperlink_rels(&header, "/word/header1.xml"),
        vec![("rId1".to_string(), "https://example.com/header".to_string())]
    );
    let (_, report) = header.patch_bytes().unwrap();
    assert_eq!(
        report.added_parts,
        vec!["word/_rels/header1.xml.rels".to_string()]
    );
}

#[test]
fn refusals_leave_the_package_unchanged() {
    let mut doc = doc_with(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">hello </w:t></w:r><w:bookmarkStart w:id="1" w:name="Term"/><w:r><w:t>world</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#,
        r#"<w:p><w:r><w:t>one</w:t></w:r></w:p><w:p><w:r><w:t>two</w:t></w:r></w:p>"#,
        r#"<w:p><w:sdt><w:sdtPr><w:dataBinding w:xpath="/x" w:storeItemID="{1}"/></w:sdtPr><w:sdtContent><w:r><w:t>BoundLink</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
        r#"<w:p><w:sdt><w:sdtPr><w:lock w:val="contentLocked"/></w:sdtPr><w:sdtContent><w:r><w:t>LockedLink</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
        r#"<w:p><w:sdt><w:sdtPr><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>PlainTextLink</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
        r#"<w:p><w:fldSimple w:instr=" DATE "><w:r><w:t>June 1, 2026</w:t></w:r></w:fldSimple></w:p>"#,
    ));
    let before = snapshot(&doc);
    for (text, reason) in [
        ("hello world", "intervening_markup"),
        ("one\ntwo", "crosses_paragraphs"),
        ("BoundLink", "data_bound_control"),
        ("LockedLink", "locked_control"),
        ("PlainTextLink", "plain_text_control"),
        ("June 1, 2026", "span_in_field_result"),
    ] {
        let err = doc.add_hyperlink(&find(&doc, text), URL).unwrap_err();
        assert_eq!(err.reason(), reason, "{text}");
        assert_eq!(snapshot(&doc), before, "{text}");
    }
    assert_eq!(
        doc.add_hyperlink(&find(&doc, "hello"), "")
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
}

#[test]
fn foreign_and_stale_spans_are_refused() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    let other = open_fixture("generated/minimal-clean/minimal.docx");
    let foreign = find(&other, "perfectly ordinary");
    assert_eq!(
        doc.add_hyperlink(&foreign, URL).unwrap_err().refusal_kind(),
        Some(RefusalKind::BoundaryViolation)
    );
    let stale = find(&doc, "perfectly ordinary");
    let replacing = find(&doc, "perfectly ordinary");
    doc.replace(&replacing, "changed", &ReplaceOptions::default())
        .unwrap();
    let before = snapshot(&doc);
    assert_eq!(
        doc.add_hyperlink(&stale, URL).unwrap_err().refusal_kind(),
        Some(RefusalKind::TargetNotFound)
    );
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn script_addresses_and_control_characters_are_refused() {
    let mut doc = open_fixture("generated/minimal-clean/minimal.docx");
    let before = snapshot(&doc);
    let span = find(&doc, "perfectly ordinary");
    for (address, reason) in [
        ("javascript:alert(1)", "script_address"),
        (" JavaScript:alert(1)", "script_address"),
        ("VBScript:msgbox", "script_address"),
        ("java\tscript:alert(1)", "address_control_character"),
        ("https://example.com/\u{0}", "address_control_character"),
    ] {
        let err = doc.add_hyperlink(&span, address).unwrap_err();
        assert_eq!(err.kind(), docrs::ErrorKind::InvalidArgument, "{address}");
        assert_eq!(err.reason(), reason, "{address}");
    }
    assert_eq!(snapshot(&doc), before);
    // Other schemes are the caller's to vet.
    doc.add_hyperlink(&span, "file:///shared/terms.pdf")
        .unwrap();
}
