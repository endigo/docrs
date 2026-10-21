//! Atomic save and late-failure behavior (upstream
//! `test_audit_late_failure_atomicity.py` save cases, `patch_save` docs).

mod support;

use std::fs;

use docrs::package::save::fault::{self, SavePoint};
use docrs::xml::NodeData;
use docrs::{Document, ErrorKind, Limits};
use support::*;

fn edited_document() -> Document {
    let mut doc = Document::open_bytes(
        &fixture("generated/minimal-clean/minimal.docx"),
        &Limits::default(),
    )
    .unwrap();
    let mut tree = doc.package().xml("word/document.xml").unwrap().clone();
    let node = tree
        .descendants(tree.root())
        .find(|n| matches!(tree.data(*n), NodeData::Text(t) if !t.trim().is_empty()))
        .unwrap();
    tree.set_text(node, "Edited text").unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", tree)
        .unwrap();
    doc
}

fn leftovers(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".partial"))
        .collect()
}

#[test]
fn injected_failures_preserve_existing_output_and_clean_up() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    let sentinel = b"SENTINEL: must survive every failed save".to_vec();
    let doc = edited_document();
    for point in [
        SavePoint::AfterTempCreate,
        SavePoint::AfterWrite,
        SavePoint::BeforeRename,
    ] {
        fs::write(&out, &sentinel).unwrap();
        fault::fail_at(point);
        let err = doc.save(&out).unwrap_err();
        fault::clear();
        assert_eq!(err.kind(), ErrorKind::Io, "{point:?}");
        assert_eq!(
            fs::read(&out).unwrap(),
            sentinel,
            "{point:?}: destination changed"
        );
        assert!(
            leftovers(dir.path()).is_empty(),
            "{point:?}: temp file left behind"
        );
        // The failed save did not undo the in-memory edit.
        let (bytes, report) = doc.patch_bytes().unwrap();
        assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
        assert!(Document::open_bytes(&bytes, &Limits::default()).is_ok());
    }
    let report = doc.save(&out).unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    let reopened = Document::open(&out).unwrap();
    let tree = reopened.package().xml("word/document.xml").unwrap();
    assert!(tree.text_content(tree.root()).contains("Edited text"));
}

#[test]
fn same_path_save_reads_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.docx");
    fs::write(&path, fixture("generated/minimal-clean/minimal.docx")).unwrap();
    let mut doc = Document::open(&path).unwrap();
    let mut tree = doc.package().xml("word/document.xml").unwrap().clone();
    let node = tree
        .descendants(tree.root())
        .find(|n| matches!(tree.data(*n), NodeData::Text(t) if !t.trim().is_empty()))
        .unwrap();
    tree.set_text(node, "In place").unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", tree)
        .unwrap();
    doc.save(&path).unwrap();
    let reopened = Document::open(&path).unwrap();
    let tree = reopened.package().xml("word/document.xml").unwrap();
    assert!(tree.text_content(tree.root()).contains("In place"));
}

#[test]
fn no_op_save_writes_verbatim_copy() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("copy.docx");
    let original = fixture("generated/gauntlet/gauntlet.docx");
    let doc = Document::open_bytes(&original, &Limits::default()).unwrap();
    let report = doc.save(&out).unwrap();
    assert!(report.verbatim_copy);
    assert_eq!(fs::read(&out).unwrap(), original);
}

#[test]
fn creates_missing_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("a/b/c.docx");
    edited_document().save(&out).unwrap();
    assert!(Document::open(&out).is_ok());
}

#[cfg(unix)]
#[test]
fn permissions_and_symlinks_are_preserved() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.docx");
    fs::write(&target, b"old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    let link = dir.path().join("link.docx");
    symlink(&target, &link).unwrap();

    edited_document().save(&link).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "link was replaced"
    );
    assert_ne!(fs::read(&target).unwrap(), b"old");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(Document::open(&link).is_ok());
    assert!(leftovers(dir.path()).is_empty());

    // A dangling link creates its target.
    let dangling = dir.path().join("dangling.docx");
    let missing = dir.path().join("missing.docx");
    symlink(&missing, &dangling).unwrap();
    edited_document().save(&dangling).unwrap();
    assert!(missing.exists());
    assert!(
        fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn failed_validation_never_touches_disk() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    fs::write(&out, b"keep").unwrap();
    let mut doc = edited_document();
    // An output budget smaller than the package fails before any write.
    let mut tight = doc.package().limits().clone();
    tight.max_output_bytes = 64;
    let reopened_bytes = doc.patch_bytes().unwrap().0;
    doc = Document::open_bytes(&reopened_bytes, &tight).unwrap();
    let mut tree = doc.package().xml("word/document.xml").unwrap().clone();
    let node = tree
        .descendants(tree.root())
        .find(|n| matches!(tree.data(*n), NodeData::Text(t) if !t.trim().is_empty()))
        .unwrap();
    tree.set_text(node, "again").unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", tree)
        .unwrap();
    let err = doc.save(&out).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ResourceLimit);
    assert_eq!(fs::read(&out).unwrap(), b"keep");
    assert!(leftovers(dir.path()).is_empty());
}
