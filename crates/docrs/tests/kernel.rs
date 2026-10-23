//! M1 preservation-kernel contracts over the frozen upstream corpus
//! (upstream `test_kernel.py`, `test_manifest.py`, `test_audit_package.py`).

mod support;

use docrs::package::diff::{diff_packages, sha256_hex};
use docrs::package::is_xml_part_name;
use docrs::xml::{self, NodeData};
use docrs::{Document, ErrorKind, Limits, RefusalKind};
use support::*;

#[test]
fn fixture_manifest_is_intact() {
    let manifest = std::fs::read_to_string(fixtures_dir().join("MANIFEST.sha256")).unwrap();
    let mut checked = 0;
    for line in manifest.lines() {
        let (hash, path) = line.split_once("  ").expect("manifest line");
        let bytes = std::fs::read(fixtures_dir().join(path)).unwrap();
        assert_eq!(
            sha256_hex(&bytes),
            hash,
            "{path} drifted from MANIFEST.sha256"
        );
        checked += 1;
    }
    assert_eq!(checked, 83);
    assert_eq!(all_fixtures().len(), 41);
}

#[test]
fn every_clean_fixture_opens_and_no_op_save_is_byte_identical() {
    for rel in all_fixtures() {
        if is_corrupt_fixture(&rel) {
            continue;
        }
        let bytes = fixture(&rel);
        let doc = Document::open_bytes(&bytes, &Limits::default())
            .unwrap_or_else(|err| panic!("{rel}: {err}"));
        let (out, report) = doc.patch_bytes().unwrap();
        assert!(report.verbatim_copy, "{rel}");
        assert_eq!(out, bytes, "{rel}: no-op save must be byte-identical");
        assert!(
            report.changed_parts.is_empty()
                && report.added_parts.is_empty()
                && report.removed_parts.is_empty()
        );
    }
}

#[test]
fn corrupt_fixtures_refuse_with_their_ground_truth() {
    let broken = Document::open_bytes(
        &fixture("generated/corrupt/broken-rel.docx"),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        broken.kind(),
        ErrorKind::Refusal(RefusalKind::MalformedPackage)
    );
    assert_eq!(broken.reason(), "broken_relationship_target");

    let malformed = Document::open_bytes(
        &fixture("generated/corrupt/malformed-xml.docx"),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        malformed.kind(),
        ErrorKind::Refusal(RefusalKind::MalformedPackage)
    );
    assert_eq!(malformed.reason(), "unparseable_xml_part");
}

/// Every XML member of every fixture survives parse → serialize → parse
/// with identical semantics (names, scopes, attributes, text, comments, PIs).
#[test]
fn every_xml_part_round_trips_semantically() {
    let mut parts = 0;
    for rel in all_fixtures() {
        let contents =
            docrs::package::zip_read::read_archive(&fixture(&rel), &Limits::default()).unwrap();
        for member in &contents.members {
            if !is_xml_part_name(&member.name) {
                continue;
            }
            let tree = match xml::parse(&member.data, &Limits::default()) {
                Ok(tree) => tree,
                Err(_) if is_corrupt_fixture(&rel) => continue,
                Err(err) => panic!("{rel}/{}: {err}", member.name),
            };
            let written = xml::serialize(&tree);
            let reparsed = xml::parse(&written, &Limits::default()).unwrap();
            assert!(
                xml::trees_equivalent(&tree, &reparsed),
                "{rel}/{}",
                member.name
            );
            assert_eq!(xml::serialize(&reparsed), written, "{rel}/{}", member.name);
            parts += 1;
        }
    }
    assert!(parts > 300, "only {parts} XML parts checked");
}

#[test]
fn forced_reserialization_restores_all_parts() {
    for rel in [
        "generated/gauntlet/gauntlet.docx",
        "libreoffice/feature-isolated/comments.docx",
        "google/minimal-clean/google-docs-export.docx",
    ] {
        let bytes = fixture(rel);
        let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
        let names: Vec<String> = doc
            .package()
            .parts()
            .map(|part| part.name().to_string())
            .filter(|name| is_xml_part_name(name))
            .collect();
        for name in &names {
            let tree = doc.package().xml(name).unwrap().clone();
            doc.package_mut_for_tests().commit_tree(name, tree).unwrap();
        }
        let (out, report) = doc.patch_bytes().unwrap();
        assert!(
            report.changed_parts.is_empty(),
            "{rel}: {:?}",
            report.changed_parts
        );
        // Parts whose re-serialization is byte-identical need no restoring.
        assert!(!report.restored_parts.is_empty() && report.restored_parts.len() <= names.len());
        assert!(report.verbatim_copy);
        assert_eq!(out, bytes);
    }
}

fn first_text_node(doc: &Document) -> docrs::xml::NodeId {
    let tree = doc.package().xml("word/document.xml").unwrap();
    tree.descendants(tree.root())
        .find(|node| {
            matches!(tree.data(*node), NodeData::Text(t) if !t.trim().is_empty())
                && tree
                    .parent(*node)
                    .is_some_and(|p| tree.local_name(p) == Some("t"))
        })
        .expect("a w:t text node")
}

#[test]
fn single_part_edit_respects_changed_part_budget() {
    for rel in all_fixtures() {
        if is_corrupt_fixture(&rel) || rel.contains("large") {
            continue;
        }
        let bytes = fixture(&rel);
        let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
        let main = doc.package().main_part_name().to_string();
        let tree = doc.package().xml(&main).unwrap();
        let Some(node) = tree
            .descendants(tree.root())
            .find(|node| matches!(tree.data(*node), NodeData::Text(t) if !t.is_empty()))
        else {
            continue;
        };
        let mut edited = tree.clone();
        let original_text = edited.text(node).unwrap().to_string();
        edited.set_text(node, format!("{original_text}§")).unwrap();
        doc.package_mut_for_tests()
            .commit_tree(&main, edited)
            .unwrap();
        let (out, report) = doc.patch_bytes().unwrap();
        assert_eq!(report.changed_parts, vec![main.clone()], "{rel}");
        assert!(!report.verbatim_copy);
        let diff = diff_packages(&bytes, &out, &Limits::default()).unwrap();
        assert!(diff.added.is_empty() && diff.removed.is_empty(), "{rel}");
        assert_eq!(diff.semantic_changed_parts(), vec![main.as_str()], "{rel}");
        assert_eq!(
            diff.changed.len(),
            1,
            "{rel}: only the edited part may change bytes"
        );
        let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
        for part in doc.package().parts() {
            if part.name() == main {
                continue;
            }
            assert_eq!(
                reopened.package().part(part.name()).unwrap().original(),
                part.original(),
                "{rel}/{}",
                part.name()
            );
        }
        let text = reopened
            .package()
            .xml(&main)
            .unwrap()
            .text_content(reopened.package().xml(&main).unwrap().root());
        assert!(text.contains(&format!("{original_text}§")), "{rel}");
    }
}

#[test]
fn edit_then_revert_is_verbatim() {
    let bytes = fixture("generated/minimal-clean/minimal.docx");
    let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    let node = first_text_node(&doc);
    let tree = doc.package().xml("word/document.xml").unwrap().clone();
    let original = tree.text(node).unwrap().to_string();
    let mut edited = tree.clone();
    edited.set_text(node, "changed").unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", edited.clone())
        .unwrap();
    assert!(!doc.patch_bytes().unwrap().1.verbatim_copy);
    edited.set_text(node, original).unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", edited)
        .unwrap();
    let (out, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.restored_parts, vec!["word/document.xml".to_string()]);
    assert!(report.verbatim_copy);
    assert_eq!(out, bytes);
}

#[test]
fn orphan_members_survive_a_changed_save() {
    // A member no relationship reaches (upstream drops these on save).
    let mut members = minimal_members();
    members.push(RawMember::new("customXml/orphan.bin", b"\x00\x01opaque"));
    let bytes = build_zip(&members);
    let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    let node = first_text_node(&doc);
    let mut tree = doc.package().xml("word/document.xml").unwrap().clone();
    tree.set_text(node, "Edited").unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", tree)
        .unwrap();
    let (out, report) = doc.patch_bytes().unwrap();
    assert!(report.removed_parts.is_empty());
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    assert_eq!(
        reopened
            .package()
            .part("customXml/orphan.bin")
            .unwrap()
            .original()
            .unwrap(),
        b"\x00\x01opaque"
    );
}

#[test]
fn new_document_opens_bundled_template() {
    let doc = Document::new();
    assert_eq!(doc.package().main_part_name(), "word/document.xml");
    let (out, report) = doc.patch_bytes().unwrap();
    assert!(report.verbatim_copy);
    assert_eq!(
        sha256_hex(&out),
        "2094b5bddffe9cf973d61fe03388413804f034160718494a65db7e98da40d35d"
    );
}
