//! Ground truth: `accept_all` on the multiround redline equals the
//! hand-built accepted fixture.

mod support;

use docrs::xml::{self, trees_equivalent};
use docrs::{Document, Limits, View};
use support::*;

fn body_texts(doc: &Document) -> Vec<(String, String)> {
    doc.blocks(View::Current)
        .unwrap()
        .into_iter()
        .map(|b| (b.story.clone(), b.text.clone()))
        .collect()
}

#[test]
fn accept_all_on_multiround_matches_the_accepted_ground_truth() {
    let mut doc = Document::open_bytes(
        &fixture("generated/redline/multiround.docx"),
        &Limits::default(),
    )
    .unwrap();
    let mut revisions = doc.revisions().unwrap();
    assert_eq!(revisions.len(), 20);
    assert_eq!(revisions.accept_all(&mut doc, None).unwrap(), 20);
    assert!(docrs::revision::remaining_markup(&doc).unwrap().is_empty());
    let (bytes, _) = doc.patch_bytes().unwrap();
    let reopened = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    assert!(reopened.revisions().unwrap().is_empty());
    let truth = Document::open_bytes(
        &fixture("generated/redline/multiround-accepted.docx"),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(body_texts(&reopened), body_texts(&truth));
    // Upstream leaves an empty `w:pPr` where a paragraph-mark stamp was the
    // only paragraph property (parity with the oracle is pinned by the ops
    // sweep); apart from those, the accepted part equals the ground truth.
    let serialized = String::from_utf8(xml::serialize(
        reopened.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap();
    assert_eq!(serialized.matches("<w:pPr/>").count(), 4);
    let ours = xml::parse_str(&serialized.replace("<w:pPr/>", "")).unwrap();
    let theirs = truth.package().xml("word/document.xml").unwrap();
    assert!(
        trees_equivalent(&ours, theirs),
        "word/document.xml differs semantically from multiround-accepted"
    );
}
