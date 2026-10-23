//! Compare as a writer: Restrict Editing on the original, `author`
//! validation and the transactional commit of the redline.

use docrs::clock::Timestamp;
use docrs::compare::{CompareOptions, Granularity, MAX_AUTHOR_CHARS, compare};
use docrs::{Document, ErrorKind, Limits, RefusalKind};

fn options(author: &str) -> CompareOptions {
    CompareOptions {
        author: author.into(),
        date: Some(Timestamp::parse("2024-01-02T03:04:05Z").unwrap()),
        granularity: Granularity::Word,
        materialize: None,
        acknowledge_protection: false,
    }
}

/// An original and a revision of it (one word changed), both under `edit`
/// protection when given.
fn pair(edit: Option<&str>) -> (Vec<u8>, Vec<u8>) {
    let mut doc = Document::new();
    doc.add_paragraph("the quick brown fox", None).unwrap();
    if let Some(edit) = edit {
        doc.set_protection(edit).unwrap();
    }
    let original = doc.patch_bytes().unwrap().0;
    let mut revised = Document::open_bytes(&original, &Limits::default()).unwrap();
    revised.acknowledge_protection().unwrap();
    let paragraph = revised.paragraphs().unwrap().remove(0);
    paragraph.runs(&revised).unwrap()[0]
        .set_text(&mut revised, "the quick red fox")
        .unwrap();
    (original, revised.patch_bytes().unwrap().0)
}

#[test]
fn a_protected_original_refuses_unless_the_caller_acknowledges() {
    let (original, revised) = pair(Some("readOnly"));
    let err = compare(
        &original,
        &revised,
        &options("Reviewer"),
        &Limits::default(),
    )
    .err()
    .expect("readOnly original must refuse");
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::DocumentProtected)
    );
    assert_eq!(err.reason(), "restrict_editing");

    let acknowledged = CompareOptions {
        acknowledge_protection: true,
        ..options("Reviewer")
    };
    let result = compare(&original, &revised, &acknowledged, &Limits::default()).unwrap();
    assert!(result.revision_count > 0);
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.kind == "document_protection_present")
    );
    assert!(result.document.protection_acknowledged());
    assert!(result.document.protection_status().unwrap().enforced);
}

#[test]
fn a_permitted_restriction_redlines_without_acknowledging_for_the_caller() {
    let (original, revised) = pair(Some("trackedChanges"));
    let result = compare(
        &original,
        &revised,
        &options("Reviewer"),
        &Limits::default(),
    )
    .unwrap();
    assert!(result.revision_count > 0);
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.kind == "document_protection_present")
    );
    assert!(!result.document.protection_acknowledged());
}

#[test]
fn author_is_validated_before_anything_is_written() {
    let (original, revised) = pair(None);
    for (author, reason) in [
        ("", "author_required"),
        ("two\nlines", "control_characters"),
        ("tab\there", "control_characters"),
        ("bell\u{7}", "invalid_xml_characters"),
        ("next\u{85}line", "control_characters"),
        ("nul\u{0}", "invalid_xml_characters"),
    ] {
        let err = compare(&original, &revised, &options(author), &Limits::default())
            .err()
            .unwrap_or_else(|| panic!("{author:?} must be refused"));
        assert_eq!(err.kind(), ErrorKind::InvalidArgument, "{author:?}");
        assert_eq!(err.reason(), reason, "{author:?}");
    }
    let longest = "a".repeat(MAX_AUTHOR_CHARS);
    compare(&original, &revised, &options(&longest), &Limits::default()).unwrap();
    let too_long = "é".repeat(MAX_AUTHOR_CHARS + 1);
    let err = compare(&original, &revised, &options(&too_long), &Limits::default())
        .err()
        .unwrap();
    assert_eq!(err.reason(), "author_too_long");
    let result = compare(&original, &revised, &options("A & <B>"), &Limits::default()).unwrap();
    let bytes = result.document.patch_bytes().unwrap().0;
    let reopened = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    assert!(!reopened.revisions().unwrap().is_empty());
}

#[test]
fn identical_inputs_leave_the_original_untouched() {
    let (original, _) = pair(None);
    let result = compare(
        &original,
        &original,
        &options("Reviewer"),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(result.revision_count, 0);
    assert!(!result.document.package().is_modified());
    assert_eq!(result.document.patch_bytes().unwrap().0, original);
}
