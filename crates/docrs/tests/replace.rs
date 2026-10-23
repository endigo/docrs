//! M3 safe-editor contracts: the five acceptance checks for untracked
//! replacement, handle freshness, consumption, aliasing and late failures.

mod support;

use std::fs;

use docrs::package::diff::diff_packages;
use docrs::replace::fault::{self, ReplacePoint};
use docrs::{
    Document, ErrorKind, FindOptions, Limits, RefusalKind, ReplaceAllOptions, ReplaceOptions, View,
};
use support::*;

const DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn bytes_with(body: &str) -> Vec<u8> {
    minimal_package_with(&format!(
        r#"<w:document {DECLS}><w:body>{body}</w:body></w:document>"#
    ))
}

fn r(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn rb(props: &str, text: &str) -> String {
    format!(r#"<w:r><w:rPr>{props}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn open(bytes: &[u8]) -> Document {
    Document::open_bytes(bytes, &Limits::default()).unwrap()
}

fn snapshot(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

fn body_text(doc: &Document) -> String {
    doc.blocks(View::Current)
        .unwrap()
        .iter()
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find(doc: &Document, needle: &str) -> docrs::Span {
    doc.find_one(needle, &FindOptions::default()).unwrap()
}

fn plain() -> ReplaceOptions {
    ReplaceOptions::default()
}

/// Checks 1–3: save → reopen shows only the intended correction and the
/// package diff is exactly the edited story part.
#[test]
fn split_run_phrase_replacement_round_trips_with_exact_budget() {
    let input = fixture("generated/feature-isolated/fragmented-runs.docx");
    let mut doc = open(&input);
    let before = body_text(&doc);
    let word = before
        .split_whitespace()
        .find(|w| w.len() > 4 && doc.find_text(w, &FindOptions::default()).unwrap().len() == 1)
        .expect("a unique word")
        .to_string();
    let span = find(&doc, &word);
    let report = doc.replace(&span, &format!("{word}Z"), &plain()).unwrap();
    assert!(report.changed);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    let patch = doc.save(&out).unwrap();
    assert_eq!(patch.changed_parts, vec!["word/document.xml".to_string()]);
    let output = fs::read(&out).unwrap();
    let diff = diff_packages(&input, &output, &Limits::default()).unwrap();
    assert_eq!(diff.semantic_changed_parts(), vec!["word/document.xml"]);
    assert!(diff.added.is_empty() && diff.removed.is_empty());
    let reopened = Document::open(&out).unwrap();
    assert_eq!(
        body_text(&reopened),
        before.replacen(&word, &format!("{word}Z"), 1)
    );
}

/// Check 5: every refusal leaves the in-memory package byte-identical, the
/// span reusable and an existing output file untouched.
#[test]
fn refusals_are_atomic() {
    let input = bytes_with(&format!(
        r#"<w:p>{}</w:p><w:p>{}</w:p><w:p><w:hyperlink r:id="rId1">{}</w:hyperlink>{}</w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> DATE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p><w:p><w:ins w:id="1" w:author="A">{}</w:ins></w:p>"#,
        r("alpha beta"),
        r("gamma"),
        r("linked"),
        r(" after"),
        r("today"),
        r("pending")
    ));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    fs::write(&out, b"sentinel").unwrap();
    let mut doc = open(&input);
    let cases: Vec<(&str, &str, ErrorKind)> = vec![
        (
            "beta\ngamma",
            "x",
            ErrorKind::Refusal(RefusalKind::BoundaryViolation),
        ),
        (
            "linked after",
            "linked later",
            ErrorKind::Refusal(RefusalKind::BoundaryViolation),
        ),
        (
            "today",
            "tomorrow",
            ErrorKind::Refusal(RefusalKind::UnsupportedStructure),
        ),
        (
            "pending",
            "done",
            ErrorKind::Refusal(RefusalKind::UnsupportedStructure),
        ),
        ("alpha", "line\nbreak", ErrorKind::InvalidArgument),
    ];
    for (needle, with, kind) in cases {
        let before = snapshot(&doc);
        let span = find(&doc, needle);
        let err = doc.replace(&span, with, &plain()).unwrap_err();
        assert_eq!(err.kind(), kind, "{needle}: {err}");
        assert_eq!(
            snapshot(&doc),
            before,
            "{needle}: refusal changed the package"
        );
        assert_eq!(fs::read(&out).unwrap(), b"sentinel");
        assert!(doc.find_one(needle, &FindOptions::default()).is_ok());
    }
    let span = find(&doc, "alpha");
    doc.replace(&span, "ALPHA", &plain()).unwrap();
    assert!(body_text(&doc).starts_with("ALPHA beta"));
}

#[test]
fn ambiguous_and_missing_targets_refuse_before_editing() {
    let mut doc = open(&bytes_with(&format!(
        "<w:p>{}</w:p>",
        r("the cat and the dog")
    )));
    let before = snapshot(&doc);
    assert_eq!(
        doc.find_one("the", &FindOptions::default())
            .unwrap_err()
            .kind(),
        ErrorKind::Refusal(RefusalKind::AmbiguousTarget)
    );
    assert_eq!(snapshot(&doc), before);
    let second = doc
        .find_one(
            "the",
            &FindOptions {
                nth: Some(2),
                ..FindOptions::default()
            },
        )
        .unwrap();
    doc.replace(&second, "a", &plain()).unwrap();
    assert_eq!(body_text(&doc), "the cat and a dog");
}

#[test]
fn successful_edit_consumes_span_and_aliases() {
    let mut doc = open(&bytes_with(&format!("<w:p>{}</w:p>", r("draft text"))));
    let span = find(&doc, "draft");
    let alias = span.clone();
    doc.replace(&span, "final", &plain()).unwrap();
    for handle in [&span, &alias] {
        let err = doc.replace(handle, "other", &plain()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Refusal(RefusalKind::TargetNotFound));
        assert_eq!(err.reason(), "span_consumed");
    }
    assert_eq!(body_text(&doc), "final text");
}

#[test]
fn identical_text_is_an_unchanged_report_and_keeps_the_span() {
    let input = bytes_with(&format!("<w:p>{}</w:p>", r("same text")));
    let mut doc = open(&input);
    let span = find(&doc, "same");
    let report = doc.replace(&span, "same", &plain()).unwrap();
    assert!(!report.changed);
    assert!(report.preserved_formatting_regions);
    assert_eq!(snapshot(&doc), input, "no-op must not touch the package");
    doc.replace(&span, "new", &plain()).unwrap();
    assert_eq!(body_text(&doc), "new text");
}

#[test]
fn unrelated_edits_keep_independent_spans_fresh_and_related_edits_stale_them() {
    let mut doc = open(&bytes_with(&format!(
        "<w:p>{}</w:p><w:p>{}</w:p>",
        r("first paragraph"),
        r("second paragraph")
    )));
    let first = find(&doc, "first");
    let second = find(&doc, "second");
    let overlapping = find(&doc, "second paragraph");
    doc.replace(&first, "1st", &plain()).unwrap();
    doc.replace(&second, "2nd", &plain()).unwrap();
    let err = doc.replace(&overlapping, "x", &plain()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Refusal(RefusalKind::TargetNotFound));
    assert_eq!(body_text(&doc), "1st paragraph\n2nd paragraph");
}

#[test]
fn foreign_spans_refuse_as_boundary_violations() {
    let input = bytes_with(&format!("<w:p>{}</w:p>", r("shared words")));
    let a = open(&input);
    let mut b = open(&input);
    let span = find(&a, "shared");
    let before = snapshot(&b);
    let err = b.replace(&span, "other", &plain()).unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::BoundaryViolation)
    );
    assert_eq!(snapshot(&b), before);
}

#[test]
fn historical_view_targets_are_inspection_only() {
    let mut doc = open(&bytes_with(&format!("<w:p>{}</w:p>", r("plain words"))));
    let span = doc
        .find_one(
            "plain",
            &FindOptions {
                view: View::Original,
                ..FindOptions::default()
            },
        )
        .unwrap();
    let before = snapshot(&doc);
    let err = doc.replace(&span, "x", &plain()).unwrap_err();
    assert_eq!(err.reason(), "historical_view_target");
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn injected_late_failures_leave_document_and_span_unchanged() {
    let input = bytes_with(&format!("<w:p>{}</w:p>", r("stable words here")));
    let mut doc = open(&input);
    let span = find(&doc, "stable");
    for point in [ReplacePoint::AfterStage, ReplacePoint::BeforeCommit] {
        fault::fail_at(point);
        let err = doc.replace(&span, "changed", &plain()).unwrap_err();
        fault::clear();
        assert_eq!(err.kind(), ErrorKind::Internal, "{point:?}");
        assert_eq!(snapshot(&doc), input, "{point:?}");
    }
    doc.replace(&span, "changed", &plain()).unwrap();
    assert_eq!(body_text(&doc), "changed words here");

    let mut doc = open(&input);
    fault::fail_at(ReplacePoint::BeforeCommit);
    assert!(
        doc.replace_all("e", "E", &ReplaceAllOptions::default())
            .is_err()
    );
    fault::clear();
    assert_eq!(snapshot(&doc), input);
}

#[test]
fn formatting_and_affix_localization() {
    let mut doc = open(&bytes_with(&format!(
        "<w:p>{}{}</w:p><w:p>{}{}</w:p>",
        rb("<w:b/>", "Omeg"),
        rb("<w:i/>", "a point"),
        rb("<w:b/>", "Pay"),
        rb("<w:b/>", "ment due")
    )));
    // Changing text that spans two different direct formats collapses into
    // the starting run's formatting, and says so.
    let span = find(&doc, "Omega point");
    let report = doc.replace(&span, "Xmega poinX", &plain()).unwrap();
    assert!(!report.preserved_formatting_regions);
    let span = find(&doc, "Payment due");
    let report = doc.replace(&span, "Payment overdue", &plain()).unwrap();
    assert!(report.preserved_formatting_regions);
    assert_eq!(body_text(&doc), "Xmega poinX\nPayment overdue");
    let mut doc = open(&bytes_with(&format!("<w:p>{}</w:p>", r("aa"))));
    let span = find(&doc, "aa");
    let err = doc.replace(&span, "aaa", &plain()).unwrap_err();
    assert_eq!(err.reason(), "ambiguous_affix_alignment");
}

#[test]
fn tab_is_kept_when_the_change_lies_beside_it() {
    let mut doc = open(&bytes_with(
        "<w:p><w:r><w:t>Section 3.</w:t><w:tab/><w:t>Termination</w:t></w:r></w:p>",
    ));
    let span = find(&doc, "Section 3.\tTermination");
    doc.replace(&span, "Section 4. Termination", &plain())
        .unwrap();
    assert_eq!(body_text(&doc), "Section 4.\tTermination");
}

#[test]
fn bookmarks_and_positional_markers_are_respected() {
    let mut doc = open(&bytes_with(&format!(
        r#"<w:p>{}<w:bookmarkStart w:id="1" w:name="Clause"/>{}<w:bookmarkEnd w:id="1"/>{}</w:p>"#,
        r("See "),
        r("clause"),
        r(" here")
    )));
    let before = snapshot(&doc);
    let span = find(&doc, "See clause here");
    assert_eq!(
        doc.replace(&span, "See here", &plain())
            .unwrap_err()
            .reason(),
        "ambiguous_affix_alignment"
    );
    assert_eq!(
        doc.replace(&span, "Saw here", &plain())
            .unwrap_err()
            .reason(),
        "hollows_bookmark"
    );
    let mut wide = open(&bytes_with(&format!(
        r#"<w:p>{}<w:bookmarkStart w:id="2" w:name="Wide"/>{}{}<w:bookmarkEnd w:id="2"/></w:p>"#,
        r("See "),
        r("clause"),
        r(" here")
    )));
    let wide_before = snapshot(&wide);
    let span = find(&wide, "See clause");
    assert_eq!(
        wide.replace(&span, "Sea clausX", &plain())
            .unwrap_err()
            .reason(),
        "crosses_bookmark"
    );
    assert_eq!(snapshot(&wide), wide_before);
    assert_eq!(snapshot(&doc), before);
    let span = find(&doc, "clause");
    doc.replace(&span, "section", &plain()).unwrap();
    assert_eq!(body_text(&doc), "See section here");
}

#[test]
fn replace_all_reports_refusals_and_commits_the_rest_atomically() {
    let mut doc = open(&bytes_with(&format!(
        r#"<w:p>{}</w:p><w:p><w:ins w:id="1" w:author="A">{}</w:ins></w:p><w:p>{}</w:p>"#,
        r("term one"),
        r("term two"),
        r("term three")
    )));
    let report = doc
        .replace_all("term", "word", &ReplaceAllOptions::default())
        .unwrap();
    assert_eq!(report.replaced_count, 2);
    assert_eq!(report.refused.len(), 1);
    assert_eq!(report.refused[0].error, "UnsupportedStructureError");
    assert_eq!(body_text(&doc), "word one\nterm two\nword three");
}

#[test]
fn protection_gate_follows_the_matrix() {
    let settings = |edit: &str| {
        format!(
            r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:documentProtection w:edit="{edit}" w:enforcement="1"/></w:settings>"#
        )
    };
    for (edit, refuses) in [
        ("readOnly", true),
        ("comments", true),
        ("forms", true),
        ("trackedChanges", false),
        ("bogus", true),
        ("none", false),
    ] {
        let bytes = package_with_settings(
            &format!(
                r#"<w:document {DECLS}><w:body><w:p>{}</w:p></w:body></w:document>"#,
                r("locked text")
            ),
            &settings(edit),
        );
        let mut doc = open(&bytes);
        let span = find(&doc, "locked");
        let result = doc.replace(&span, "open", &plain());
        assert_eq!(result.is_err(), refuses, "{edit}");
        if refuses {
            assert_eq!(
                result.unwrap_err().kind(),
                ErrorKind::Refusal(RefusalKind::DocumentProtected)
            );
            assert_eq!(snapshot(&doc), bytes);
            doc.acknowledge_protection().unwrap();
            let span = find(&doc, "locked");
            doc.replace(&span, "open", &plain()).unwrap();
            assert!(doc.protection_status().unwrap().enforced);
        }
    }
}
