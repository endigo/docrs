//! A member removed and added again under the same name is a new member:
//! handles captured into the old one are stale, even when the new content
//! lines up with the old node for node.

mod support;

use docrs::anchor::BlockTarget;
use docrs::package::rels::rt;
use docrs::txn::Txn;
use docrs::{Document, FindOptions, Limits, RefusalKind, ReplaceOptions, View};
use support::*;

const HEADER: &str = "word/header1.xml";
const HEADER_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
const HEADER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t>Header text</w:t></w:r></w:p></w:hdr>"#;
const DOC_WITH_HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p><w:r><w:t>Body text</w:t></w:r></w:p><w:sectPr><w:headerReference w:type="default" r:id="rId1"/></w:sectPr></w:body></w:document>"#;

/// A package whose main part relates to one header.
fn package_with_header() -> Vec<u8> {
    let ct = CT_XML.replace(
        "</Types>",
        &format!("<Override PartName=\"/{HEADER}\" ContentType=\"{HEADER_CT}\"/></Types>"),
    );
    let doc_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{}\" Target=\"header1.xml\"/></Relationships>",
        rt::HEADER
    );
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", DOC_WITH_HEADER.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        RawMember::new(HEADER, HEADER_XML.as_bytes()),
    ])
}

/// Remove the header and add it again, in one commit, with a copy of its
/// own tree: the same nodes, ids and text under the same name.
fn readd_header_unchanged(doc: &mut Document) {
    let tree = doc.package().xml(HEADER).unwrap().clone();
    let staged = {
        let mut txn = Txn::new(doc);
        txn.remove_part(HEADER).unwrap();
        txn.add_xml_part(HEADER, HEADER_CT, tree).unwrap();
        txn.finish().unwrap()
    };
    doc.commit_for_tests(staged).unwrap();
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

#[test]
fn readding_a_member_starts_a_new_incarnation() {
    let mut doc = Document::open_bytes(&package_with_header(), &Limits::default()).unwrap();
    assert_eq!(doc.package().part(HEADER).unwrap().incarnation(), 0);
    assert_eq!(doc.package().xml(HEADER).unwrap().incarnation(), 0);
    readd_header_unchanged(&mut doc);
    assert_eq!(doc.package().part(HEADER).unwrap().incarnation(), 1);
    assert_eq!(doc.package().xml(HEADER).unwrap().incarnation(), 1);
    assert_eq!(
        doc.package()
            .xml("word/document.xml")
            .unwrap()
            .incarnation(),
        0
    );
    let reopened = Document::open_bytes(&bytes(&doc), &Limits::default()).unwrap();
    assert_eq!(reopened.package().xml(HEADER).unwrap().incarnation(), 0);
}

#[test]
fn spans_into_the_old_member_are_stale() {
    let mut doc = Document::open_bytes(&package_with_header(), &Limits::default()).unwrap();
    let span = doc
        .find_text("Header text", &FindOptions::default())
        .unwrap()
        .into_iter()
        .find(|span| span.story().contains("header"))
        .expect("the header is a story");
    readd_header_unchanged(&mut doc);
    let before = bytes(&doc);
    let err = doc
        .replace(&span, "Changed", &ReplaceOptions::default())
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    assert_eq!(err.reason(), "story_replaced", "{err}");
    assert_eq!(bytes(&doc), before);
    let fresh = doc
        .find_text("Header text", &FindOptions::default())
        .unwrap()
        .into_iter()
        .find(|span| span.story().contains("header"))
        .unwrap();
    doc.replace(&fresh, "Changed", &ReplaceOptions::default())
        .unwrap();
    assert!(
        String::from_utf8_lossy(&doc.package().part(HEADER).unwrap().current_bytes())
            .contains("Changed")
    );
}

#[test]
fn blocks_into_the_old_member_are_stale() {
    let mut doc = Document::open_bytes(&package_with_header(), &Limits::default()).unwrap();
    let block = doc
        .blocks(View::Current)
        .unwrap()
        .into_iter()
        .find(|block| block.part() == HEADER)
        .expect("the header paragraph is a block");
    readd_header_unchanged(&mut doc);
    let before = bytes(&doc);
    let err =
        docrs::fields::add_page_number_field(&mut doc, BlockTarget::Block(&block)).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    assert_eq!(err.reason(), "story_replaced", "{err}");
    assert_eq!(bytes(&doc), before);
    let fresh = doc
        .blocks(View::Current)
        .unwrap()
        .into_iter()
        .find(|block| block.part() == HEADER)
        .unwrap();
    docrs::fields::add_page_number_field(&mut doc, BlockTarget::Block(&fresh)).unwrap();
}
