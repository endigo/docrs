//! M5 revision contracts (oracle-independent; expected values frozen from
//! the pinned upstream oracle): enumeration, accept/reject algebra, paired
//! moves, closure refusals, refusal atomicity and handle freshness.

mod support;

use docrs::revision::remaining_markup;
use docrs::{Document, ErrorKind, FindOptions, Limits, RefusalKind, ReplaceOptions, View};
use support::*;

const W_DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;

fn with_body(body: &str) -> Document {
    let bytes = minimal_package_with(&format!(
        r#"<w:document {W_DECLS}><w:body>{body}</w:body></w:document>"#
    ));
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

fn texts(doc: &Document) -> Vec<String> {
    doc.blocks(View::Current)
        .unwrap()
        .into_iter()
        .filter(|b| b.story == "word/document.xml")
        .map(|b| b.text)
        .collect()
}

fn refusal(err: &docrs::Error) -> RefusalKind {
    match err.kind() {
        ErrorKind::Refusal(kind) => kind,
        other => panic!("expected a refusal, got {other:?}: {err}"),
    }
}

const FIXTURES_WITH_REVISIONS: &[(&str, usize)] = &[
    ("generated/feature-isolated/tracked-ins-del.docx", 3),
    ("generated/feature-isolated/tracked-moves.docx", 2),
    ("generated/feature-isolated/format-changes.docx", 2),
    ("generated/feature-isolated/format-changes-rich.docx", 3),
    ("generated/feature-isolated/row-revisions.docx", 10),
    ("generated/feature-isolated/paragraph-merge.docx", 2),
    ("generated/gauntlet/gauntlet.docx", 22),
    ("generated/redline/multiround.docx", 20),
    ("libreoffice/feature-isolated/tracked-ins-del.docx", 3),
];

#[test]
fn every_fixture_resolves_to_zero_markup_both_ways() {
    for (rel, count) in FIXTURES_WITH_REVISIONS {
        for accept in [true, false] {
            let mut doc = open_fixture(rel);
            let mut revisions = doc.revisions().unwrap();
            assert_eq!(revisions.len(), *count, "{rel}");
            assert!(revisions.remaining_unsupported().is_empty(), "{rel}");
            let resolved = if accept {
                revisions.accept_all(&mut doc, None).unwrap()
            } else {
                revisions.reject_all(&mut doc, None).unwrap()
            };
            assert_eq!(resolved, *count, "{rel}");
            assert!(
                revisions.is_empty(),
                "snapshot refreshes after resolution: {rel}"
            );
            let reopened = reopen(&doc);
            assert!(
                remaining_markup(&reopened).unwrap().is_empty(),
                "{rel} accept={accept}"
            );
            assert!(reopened.revisions().unwrap().is_empty(), "{rel}");
        }
    }
}

#[test]
fn enumeration_matches_the_oracle_payload() {
    let doc = open_fixture("generated/feature-isolated/tracked-ins-del.docx");
    let payload = doc.revisions().unwrap().to_json();
    assert_eq!(payload["schema"], "paper_revisions");
    assert_eq!(payload["version"], 4);
    assert_eq!(payload["remaining_unsupported"], serde_json::json!({}));
    let kinds: Vec<(String, String, String, String)> = payload["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["revision_type"].as_str().unwrap().to_string(),
                r["author"].as_str().unwrap().to_string(),
                r["text"].as_str().unwrap().to_string(),
                r["date"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                "deletion".into(),
                "Bob Reviewer".into(),
                "forty-two".into(),
                "2026-06-02T14:45:00+00:00".into()
            ),
            (
                "insertion".into(),
                "Alice Editor".into(),
                "forty-seven".into(),
                "2026-06-01T09:30:00+00:00".into()
            ),
            (
                "insertion".into(),
                "Bob Reviewer".into(),
                "This whole sentence was inserted with tracking on.".into(),
                "2026-06-02T14:45:00+00:00".into()
            ),
        ]
    );
    assert_eq!(
        payload["revisions"][0]["anchor_role"],
        "legacy_inert_location_evidence"
    );
    assert_eq!(
        payload["revisions"][0]["anchor"]["story"],
        "word/document.xml"
    );
}

#[test]
fn author_filter_and_individual_resolution() {
    let mut doc = open_fixture("generated/feature-isolated/tracked-ins-del.docx");
    let mut snapshot = doc.revisions().unwrap();
    assert_eq!(
        snapshot.accept_all(&mut doc, Some("Alice Editor")).unwrap(),
        1
    );
    let authors: Vec<String> = snapshot.iter().map(|r| r.author.clone()).collect();
    assert_eq!(authors, vec!["Bob Reviewer", "Bob Reviewer"]);
    let deletion = snapshot
        .iter()
        .find(|r| r.revision_type == "deletion")
        .unwrap()
        .clone();
    deletion.reject(&mut doc).unwrap();
    assert!(texts(&doc)[1].contains("forty-two"));
}

#[test]
fn multiround_reject_all_restores_the_original() {
    let mut doc = open_fixture("generated/redline/multiround.docx");
    assert_eq!(
        doc.revisions().unwrap().reject_all(&mut doc, None).unwrap(),
        20
    );
    let reopened = reopen(&doc);
    assert_eq!(
        texts(&reopened),
        vec![
            "Engagement letter, revised across two rounds.",
            "This agreement is made between the parties.",
            "The indemnity clause shall survive termination of this agreement.",
            "Middle paragraph between the move sites.",
            "Payment is due within thirty days of invoice.",
            "Delivery follows the schedule in Exhibit A.",
            "Item\nAmount\nOld charge\n$50",
            "This sentence continues ",
            "onto the following line.",
            "A tracked split divides this once-single sentence.",
            "Signed at .",
            "Closing paragraph after all tracked activity.",
        ]
    );
    let document_xml = String::from_utf8(docrs::xml::serialize(
        reopened.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap();
    assert!(
        !document_xml.contains("commentRangeStart") && !document_xml.contains("commentReference")
    );
    // The rejected insertion's comment body went with it.
    assert!(reopened.comments().unwrap().is_empty());
}

#[test]
fn moves_resolve_as_pairs_from_either_site() {
    let moved = "The indemnity clause relocated by tracked move.";
    let mut doc = open_fixture("generated/feature-isolated/tracked-moves.docx");
    let from = doc
        .revisions()
        .unwrap()
        .iter()
        .find(|r| r.revision_type == "move_from")
        .unwrap()
        .clone();
    from.accept(&mut doc).unwrap();
    let after = texts(&doc);
    assert_eq!(after.iter().filter(|t| t.contains(moved)).count(), 1);
    let middle = after
        .iter()
        .position(|t| t == "Paragraph between the move ends.")
        .unwrap();
    assert!(after.iter().position(|t| t == moved).unwrap() > middle);
    assert!(remaining_markup(&doc).unwrap().is_empty());

    let mut doc = open_fixture("generated/feature-isolated/tracked-moves.docx");
    let to = doc
        .revisions()
        .unwrap()
        .iter()
        .find(|r| r.revision_type == "move_to")
        .unwrap()
        .clone();
    to.reject(&mut doc).unwrap();
    let after = texts(&doc);
    let middle = after
        .iter()
        .position(|t| t == "Paragraph between the move ends.")
        .unwrap();
    assert!(after.iter().position(|t| t == moved).unwrap() < middle);
}

#[test]
fn row_revisions_keep_or_remove_rows() {
    let mut doc = open_fixture("generated/feature-isolated/row-revisions.docx");
    doc.revisions().unwrap().accept_all(&mut doc, None).unwrap();
    assert_eq!(texts(&doc)[1], "Item\nAmount\nFiling fee\n$100");
    let mut doc = open_fixture("generated/feature-isolated/row-revisions.docx");
    doc.revisions().unwrap().reject_all(&mut doc, None).unwrap();
    assert_eq!(texts(&doc)[1], "Item\nAmount\nOld charge\n$50");
    let types: Vec<&str> = open_fixture("generated/feature-isolated/row-revisions.docx")
        .revisions()
        .unwrap()
        .iter()
        .filter(|r| r.revision_type.starts_with("row_"))
        .map(|r| r.revision_type)
        .collect();
    assert_eq!(types, vec!["row_insertion", "row_deletion"]);
}

#[test]
fn closure_refusals_are_atomic() {
    let mut doc = open_fixture("generated/feature-isolated/row-revisions.docx");
    let before = doc.patch_bytes().unwrap().0;
    let row_ins = doc
        .revisions()
        .unwrap()
        .iter()
        .find(|r| r.revision_type == "row_insertion")
        .unwrap()
        .clone();
    let err = row_ins.reject(&mut doc).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::UnsupportedStructure);
    assert!(err.message().contains("nested unselected"), "{err}");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
    row_ins.accept(&mut doc).unwrap();
    assert_eq!(doc.revisions().unwrap().len(), 9);

    let mut doc = with_body(
        r#"<w:p><w:del w:id="20" w:author="Alice"><w:r><w:delText>outer</w:delText></w:r><w:ins w:id="21" w:author="Bob"><w:r><w:t>nested</w:t></w:r></w:ins></w:del></w:p>"#,
    );
    let before = doc.patch_bytes().unwrap().0;
    let mut snapshot = doc.revisions().unwrap();
    let err = snapshot.accept_all(&mut doc, Some("Alice")).unwrap_err();
    assert!(err.message().contains("'Bob'"), "{err}");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
    assert_eq!(
        snapshot.len(),
        2,
        "a refused resolution leaves the snapshot fresh"
    );
    assert_eq!(snapshot.accept_all(&mut doc, None).unwrap(), 2);
}

#[test]
fn exotic_types_are_counted_and_refused_by_name() {
    let mut doc = with_body(
        r#"<w:tbl><w:tblPr><w:tblPrChange w:id="900" w:author="A"><w:tblPr/></w:tblPrChange></w:tblPr><w:tblGrid><w:gridCol/></w:tblGrid><w:tr><w:tc><w:tcPr><w:cellMerge w:id="902" w:author="A"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:sectPr><w:sectPrChange w:id="903" w:author="A"><w:sectPr/></w:sectPrChange></w:sectPr>"#,
    );
    let before = doc.patch_bytes().unwrap().0;
    let mut snapshot = doc.revisions().unwrap();
    let census: Vec<(String, usize)> = snapshot.remaining_unsupported().into_iter().collect();
    assert_eq!(
        census,
        vec![
            ("cell_revision".into(), 1),
            ("section_property_change".into(), 1),
            ("table_property_change".into(), 1)
        ]
    );
    let section = snapshot
        .iter()
        .find(|r| r.revision_type == "section_property_change")
        .unwrap();
    assert_eq!(section.anchor.index, -1);
    let err = section.accept(&mut doc).unwrap_err();
    assert!(err.message().contains("'section_property_change'"), "{err}");
    let err = snapshot.accept_all(&mut doc, None).unwrap_err();
    assert_eq!(err.reason(), "unresolvable_revision_type");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn stale_snapshots_and_revisions_refuse_without_change() {
    let mut doc = with_body(
        r#"<w:p><w:ins w:id="10" w:author="Audit"><w:r><w:t>first</w:t></w:r></w:ins><w:ins w:id="11" w:author="Audit"><w:r><w:t>second</w:t></w:r></w:ins></w:p>"#,
    );
    let mut snapshot = doc.revisions().unwrap();
    let first = snapshot[0].clone();
    let second = snapshot[1].clone();
    first.accept(&mut doc).unwrap();
    let before = doc.patch_bytes().unwrap().0;
    for err in [
        first.accept(&mut doc).unwrap_err(),
        second.accept(&mut doc).unwrap_err(),
        snapshot.clone().accept_all(&mut doc, None).unwrap_err(),
        snapshot.reject_all(&mut doc, None).unwrap_err(),
    ] {
        assert_eq!(refusal(&err), RefusalKind::UnsupportedStructure);
        assert!(err.message().contains("stale"), "{err}");
        assert_eq!(doc.patch_bytes().unwrap().0, before);
    }
    assert_eq!(
        doc.revisions().unwrap().accept_all(&mut doc, None).unwrap(),
        1
    );
}

#[test]
fn foreign_revisions_and_snapshots_are_boundary_violations() {
    let mut a = open_fixture("generated/feature-isolated/tracked-ins-del.docx");
    let mut b = open_fixture("generated/feature-isolated/tracked-ins-del.docx");
    let before = b.patch_bytes().unwrap().0;
    let mut snapshot = a.revisions().unwrap();
    let err = snapshot[0].accept(&mut b).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::BoundaryViolation);
    let err = snapshot.accept_all(&mut b, None).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::BoundaryViolation);
    assert_eq!(b.patch_bytes().unwrap().0, before);
    assert_eq!(snapshot.accept_all(&mut a, None).unwrap(), 3);
}

#[test]
fn pending_revisions_survive_save_and_reopen_with_authorship() {
    let mut doc = open_fixture("generated/redline/multiround.docx");
    let first = doc.revisions().unwrap()[0].clone();
    first.accept(&mut doc).unwrap();
    let live: Vec<serde_json::Value> = doc
        .revisions()
        .unwrap()
        .iter()
        .map(|r| r.to_json())
        .collect();
    let reopened = reopen(&doc);
    let after: Vec<serde_json::Value> = reopened
        .revisions()
        .unwrap()
        .iter()
        .map(|r| r.to_json())
        .collect();
    assert_eq!(live.len(), 19);
    assert_eq!(live, after);
}

#[test]
fn tracked_edit_algebra() {
    let mut tracked = open_fixture("generated/feature-isolated/fragmented-runs.docx");
    let span = tracked
        .find_one("$75–100/hr", &FindOptions::default())
        .unwrap();
    let options = ReplaceOptions {
        tracked: true,
        author: Some("Carol QA".into()),
        date: Some(docrs::clock::Timestamp::parse("2026-07-07T12:00:00Z").unwrap()),
        preserve_revision: false,
    };
    tracked.replace(&span, "$85–110/hr", &options).unwrap();
    let pristine = texts(&open_fixture(
        "generated/feature-isolated/fragmented-runs.docx",
    ));
    let mut rejected =
        Document::open_bytes(&tracked.patch_bytes().unwrap().0, &Limits::default()).unwrap();
    tracked
        .revisions()
        .unwrap()
        .accept_all(&mut tracked, None)
        .unwrap();
    let mut plain = open_fixture("generated/feature-isolated/fragmented-runs.docx");
    let span = plain
        .find_one("$75–100/hr", &FindOptions::default())
        .unwrap();
    plain
        .replace(&span, "$85–110/hr", &ReplaceOptions::default())
        .unwrap();
    assert_eq!(texts(&reopen(&tracked)), texts(&reopen(&plain)));
    rejected
        .revisions()
        .unwrap()
        .reject_all(&mut rejected, None)
        .unwrap();
    assert_eq!(texts(&reopen(&rejected)), pristine);
}

#[test]
fn large_documents_enumerate_and_resolve_promptly() {
    let mut doc = open_fixture("generated/large/large-5000-paragraphs.docx");
    let started = std::time::Instant::now();
    let mut revisions = doc.revisions().unwrap();
    assert!(revisions.is_empty());
    assert_eq!(revisions.accept_all(&mut doc, None).unwrap(), 0);
    assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    assert!(doc.patch_bytes().unwrap().1.verbatim_copy);
}

#[test]
fn protection_gates_resolution() {
    let mut doc = open_fixture("generated/feature-isolated/protected-readonly.docx");
    let err = doc
        .revisions()
        .unwrap()
        .accept_all(&mut doc, None)
        .unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::DocumentProtected);
    doc.acknowledge_protection().unwrap();
    assert_eq!(
        doc.revisions().unwrap().accept_all(&mut doc, None).unwrap(),
        0
    );
}

#[test]
fn discard_drops_only_unreferenced_relationships() {
    let ct = CT_XML.replace(
        "</Types>",
        r#"<Default Extension="bmp" ContentType="image/bmp"/></Types>"#,
    );
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.bmp"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.bmp"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://audit.example.invalid/" TargetMode="External"/></Relationships>"#;
    let blip = |id: &str| {
        format!(
            r#"<w:r><w:drawing><a:blip xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" r:embed="{id}"/></w:drawing></w:r>"#
        )
    };
    let body = format!(
        r#"<w:p>{}<w:ins w:id="20" w:author="Audit">{}{}<w:hyperlink r:id="rId3"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:ins></w:p>"#,
        blip("rId1"),
        blip("rId1"),
        blip("rId2")
    );
    let document = format!(r#"<w:document {W_DECLS}><w:body>{body}</w:body></w:document>"#);
    let bytes = build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/media/image1.bmp", b"BM"),
        RawMember::new("word/media/image2.bmp", b"BM"),
    ]);
    let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    assert_eq!(
        doc.revisions().unwrap().reject_all(&mut doc, None).unwrap(),
        1
    );
    let ids: Vec<String> = doc
        .package()
        .relationships_at("/word/document.xml")
        .unwrap()
        .into_iter()
        .map(|rel| rel.id)
        .collect();
    assert_eq!(ids, vec!["rId1"]);
    let (_, report) = doc.patch_bytes().unwrap();
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert_eq!(
        changed,
        vec!["word/_rels/document.xml.rels", "word/document.xml"]
    );
}

#[test]
fn paragraph_mark_resolution_joins_and_splits() {
    let mut doc = open_fixture("generated/feature-isolated/paragraph-merge.docx");
    let mark = doc
        .revisions()
        .unwrap()
        .iter()
        .find(|r| r.is_paragraph_mark && r.revision_type == "deletion")
        .unwrap()
        .clone();
    mark.accept(&mut doc).unwrap();
    assert!(
        texts(&doc)
            .iter()
            .any(|t| t == "This sentence continues onto the following line.")
    );
    let mut doc = open_fixture("generated/feature-isolated/paragraph-merge.docx");
    let mark = doc
        .revisions()
        .unwrap()
        .iter()
        .find(|r| r.is_paragraph_mark && r.revision_type == "insertion")
        .unwrap()
        .clone();
    mark.reject(&mut doc).unwrap();
    assert!(
        texts(&doc)
            .iter()
            .any(|t| t == "A tracked split divides this once-single sentence.")
    );
}

#[test]
fn format_change_reject_restores_stored_properties() {
    let mut doc = open_fixture("generated/feature-isolated/format-changes-rich.docx");
    let change = doc
        .revisions()
        .unwrap()
        .iter()
        .find(|r| !r.is_paragraph_mark && r.text.contains("Delivery"))
        .unwrap()
        .clone();
    change.reject(&mut doc).unwrap();
    let xml = String::from_utf8(docrs::xml::serialize(
        doc.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap();
    let run = &xml[..xml.find("Delivery").unwrap()];
    let r_pr = &run[run.rfind("<w:rPr>").unwrap()..];
    assert!(
        r_pr.contains("<w:i/>")
            && r_pr.contains(r#"<w:sz w:val="28"/>"#)
            && !r_pr.contains("<w:b/>"),
        "{r_pr}"
    );
    let marks = remaining_markup(&doc).unwrap();
    assert_eq!(marks.get("rPrChange"), Some(&1));
    assert_eq!(marks.get("pPrChange"), Some(&1));
}

#[test]
fn a_failed_snapshot_refresh_leaves_document_and_snapshot_unchanged() {
    use docrs::revision::fault::{self, RevisionPoint};
    for accept in [true, false] {
        let mut doc = open_fixture("generated/feature-isolated/tracked-ins-del.docx");
        let before = doc.patch_bytes().unwrap().0;
        let mut revisions = doc.revisions().unwrap();
        fault::fail_at(RevisionPoint::Refresh);
        let result = if accept {
            revisions.accept_all(&mut doc, None)
        } else {
            revisions.reject_all(&mut doc, None)
        };
        fault::clear();
        let err = result.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal, "{err}");
        assert_eq!(doc.patch_bytes().unwrap().0, before, "accept={accept}");
        assert_eq!(revisions.len(), 3);
        let resolved = if accept {
            revisions.accept_all(&mut doc, None)
        } else {
            revisions.reject_all(&mut doc, None)
        };
        assert_eq!(resolved.unwrap(), 3);
        assert!(revisions.is_empty());
        assert!(remaining_markup(&doc).unwrap().is_empty());
    }
}
