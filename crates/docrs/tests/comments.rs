//! M5 comment contracts (oracle-independent; expected values frozen from
//! the pinned upstream oracle): authoring, threads, resolution, deletion,
//! refusal atomicity and comment-handle freshness.

mod support;

use docrs::clock::Timestamp;
use docrs::commentops::{anchored_text, delete_comment, is_resolved, parent_of, reply, resolve};
use docrs::comments::{add_comment, add_comment_paragraph, add_unanchored_comment};
use docrs::xml::serialize;
use docrs::{Document, ErrorKind, FindOptions, Limits, RefusalKind, View};
use support::*;

const W_DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn date() -> Option<Timestamp> {
    Some(Timestamp::parse("2024-01-02T03:04:05Z").unwrap())
}

fn target_doc() -> Document {
    let bytes = minimal_package_with(&format!(
        r#"<w:document {W_DECLS}><w:body><w:p><w:r><w:t>comment target</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">Alpha </w:t></w:r><w:r><w:t>beta gamma</w:t></w:r></w:p></w:body></w:document>"#
    ));
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

fn xml(doc: &Document, part: &str) -> String {
    String::from_utf8(serialize(doc.package().xml(part).unwrap())).unwrap()
}

fn refusal(err: &docrs::Error) -> RefusalKind {
    match err.kind() {
        ErrorKind::Refusal(kind) => kind,
        other => panic!("expected a refusal, got {other:?}: {err}"),
    }
}

fn find(doc: &Document, text: &str) -> docrs::Span {
    doc.find_one(text, &FindOptions::default()).unwrap()
}

#[test]
fn span_comment_creates_parts_and_identity() {
    let mut doc = target_doc();
    let span = find(&doc, "comment target");
    let comment = span
        .comment(&mut doc, "Parent", "Reviewer", None, date())
        .unwrap();
    assert_eq!(comment.comment_id, 0);
    assert_eq!(comment.author, "Reviewer");
    assert_eq!(comment.initials.as_deref(), Some(""));
    assert_eq!(
        comment.timestamp.unwrap().isoformat(),
        "2024-01-02T03:04:05+00:00"
    );
    assert_eq!(comment.text, "Parent");
    let (_, report) = doc.patch_bytes().unwrap();
    let mut added = report.added_parts.clone();
    added.sort();
    assert_eq!(
        added,
        vec![
            "word/_rels/document.xml.rels",
            "word/comments.xml",
            "word/commentsExtensible.xml",
            "word/commentsIds.xml"
        ]
    );
    let comments = xml(&doc, "word/comments.xml");
    assert!(comments.contains(r#"w14:paraId="10000001""#), "{comments}");
    assert!(xml(&doc, "word/commentsIds.xml").contains(r#"w16cid:durableId="10000001""#));
    assert_eq!(anchored_text(&doc, &comment).unwrap(), "comment target");
}

#[test]
fn replies_get_their_own_anchor_triple_and_thread_link() {
    let mut doc = target_doc();
    let parent = find(&doc, "comment target")
        .comment(&mut doc, "Parent", "Reviewer", None, date())
        .unwrap();
    let child = reply(&mut doc, &parent, "Reply", "Second Reviewer", None, date()).unwrap();
    assert_eq!(child.comment_id, 1);
    let body = xml(&doc, "word/document.xml");
    let order: Vec<&str> = [
        r#"<w:commentRangeStart w:id="0"/>"#,
        r#"<w:commentRangeStart w:id="1"/>"#,
        r#"<w:commentRangeEnd w:id="1"/>"#,
        r#"<w:commentRangeEnd w:id="0"/>"#,
        r#"<w:commentReference w:id="0"/>"#,
        r#"<w:commentReference w:id="1"/>"#,
    ]
    .into_iter()
    .collect();
    let positions: Vec<usize> = order
        .iter()
        .map(|m| body.find(m).unwrap_or_else(|| panic!("{m}")))
        .collect();
    let mut sorted = positions.clone();
    sorted.sort();
    assert_eq!(positions, sorted);
    let extended = xml(&doc, "word/commentsExtended.xml");
    assert!(
        extended.contains(r#"w15:paraId="10000002" w15:done="0" w15:paraIdParent="10000001""#),
        "{extended}"
    );
    assert_eq!(parent_of(&doc, &child).unwrap(), Some(0));
    assert_eq!(parent_of(&doc, &parent).unwrap(), None);
    assert_eq!(anchored_text(&doc, &child).unwrap(), "comment target");
    resolve(&mut doc, &parent, true).unwrap();
    let parent = doc.comments().unwrap().get(0).unwrap().clone();
    add_comment_paragraph(&mut doc, &parent, "Additional context").unwrap();
    assert!(is_resolved(&doc, &parent).unwrap());
    assert_eq!(parent_of(&doc, &child).unwrap(), Some(0));
    let thread = doc.comment_thread().unwrap();
    assert_eq!(thread.len(), 2);
    assert!(thread[0].resolved);
    assert_eq!(thread[1].parent_id, Some(0));
}

#[test]
fn deleting_a_parent_removes_the_thread_and_handles_go_stale() {
    let mut doc = target_doc();
    let parent = find(&doc, "comment target")
        .comment(&mut doc, "Parent", "Reviewer", None, date())
        .unwrap();
    let child = reply(&mut doc, &parent, "Reply", "Second Reviewer", None, date()).unwrap();
    delete_comment(&mut doc, &parent).unwrap();
    assert!(doc.comments().unwrap().is_empty());
    assert!(!xml(&doc, "word/document.xml").contains("commentR"));
    assert!(!xml(&doc, "word/commentsIds.xml").contains("w16cid:commentId "));
    let before = doc.patch_bytes().unwrap().0;
    let err = resolve(&mut doc, &child, true).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::TargetNotFound);
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn foreign_comment_handles_are_boundary_violations() {
    let mut a = target_doc();
    let mut b = target_doc();
    let comment = find(&a, "comment target")
        .comment(&mut a, "Parent", "Reviewer", None, date())
        .unwrap();
    find(&b, "comment target")
        .comment(&mut b, "Other", "Reviewer", None, date())
        .unwrap();
    let before = b.patch_bytes().unwrap().0;
    let err = resolve(&mut b, &comment, true).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::BoundaryViolation);
    assert_eq!(b.patch_bytes().unwrap().0, before);
}

#[test]
fn comment_refusals_change_nothing() {
    // A complex-field boundary inside the anchored runs.
    let bytes = minimal_package_with(&format!(
        r#"<w:document {W_DECLS}><w:body><w:p><w:r><w:fldChar w:fldCharType="begin"/><w:t>target</w:t></w:r></w:p></w:body></w:document>"#
    ));
    let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    let before = doc.patch_bytes().unwrap().0;
    let span = find(&doc, "target");
    let err = span
        .comment(&mut doc, "review", "A", None, date())
        .unwrap_err();
    assert!(err.message().contains("field boundary"), "{err}");
    let err = doc
        .add_comment(&span, "review", "A", None, date())
        .unwrap_err();
    assert!(err.message().contains("field boundary"), "{err}");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
    // Programmer errors.
    let mut doc = target_doc();
    let before = doc.patch_bytes().unwrap().0;
    let span = find(&doc, "comment target");
    assert_eq!(
        span.comment(&mut doc, "x", "", None, None)
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        add_unanchored_comment(&mut doc, "bad \u{0} text", "A", None, None)
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidArgument
    );
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn protection_gates_comment_mutators_but_not_reads() {
    let mut doc = Document::open_bytes(
        &fixture("generated/feature-isolated/protected-readonly.docx"),
        &Limits::default(),
    )
    .unwrap();
    let before = doc.patch_bytes().unwrap().0;
    let err = add_unanchored_comment(&mut doc, "blocked", "A", None, None).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::DocumentProtected);
    assert!(doc.comment_thread().unwrap().is_empty());
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn inspection_never_creates_a_comments_part() {
    let doc = target_doc();
    assert!(doc.comment_thread().unwrap().is_empty());
    assert!(doc.comments().unwrap().is_empty());
    assert!(doc.patch_bytes().unwrap().1.verbatim_copy);
}

#[test]
fn fixture_comment_properties() {
    let doc = Document::open_bytes(
        &fixture("generated/feature-isolated/comments.docx"),
        &Limits::default(),
    )
    .unwrap();
    let comments = doc.comments().unwrap();
    let rows: Vec<(i64, String, Option<String>, String, String)> = comments
        .iter()
        .map(|c| {
            (
                c.comment_id,
                c.author.clone(),
                c.initials.clone(),
                c.timestamp.unwrap().isoformat(),
                c.text.clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                0,
                "Alice Editor".into(),
                Some("AE".into()),
                "2026-06-03T10:15:00+00:00".into(),
                "Please double-check this figure.".into()
            ),
            (
                1,
                "Bob Reviewer".into(),
                Some("BR".into()),
                "2026-06-03T10:15:00+00:00".into(),
                "Approved as written.".into()
            ),
        ]
    );
    assert_eq!(
        anchored_text(&doc, comments.get(1).unwrap()).unwrap(),
        "A second paragraph with another comment target."
    );
}

#[test]
fn span_comment_isolates_edge_runs() {
    let mut doc = target_doc();
    let span = find(&doc, "beta gam");
    let comment = span.comment(&mut doc, "Note", "R", None, date()).unwrap();
    assert_eq!(anchored_text(&doc, &comment).unwrap(), "beta gam");
    let body = xml(&doc, "word/document.xml");
    assert!(body.contains(r#"<w:commentRangeStart w:id="0"/><w:r><w:t>beta gam</w:t></w:r><w:commentRangeEnd w:id="0"/>"#), "{body}");
    // Only a tail was split: the span still denotes its text (as upstream).
    let again = span.comment(&mut doc, "Again", "R", None, date()).unwrap();
    assert_eq!(anchored_text(&doc, &again).unwrap(), "beta gam");

    // Splitting off a leading prefix shortens the matched text node; Rust
    // spans are immutable values (upstream rewrites the span's offsets in
    // place), so the old span no longer proves its text and refuses.
    let mut doc = target_doc();
    let span = find(&doc, "pha be");
    let comment = span.comment(&mut doc, "Note", "R", None, date()).unwrap();
    assert_eq!(anchored_text(&doc, &comment).unwrap(), "pha be");
    let before = doc.patch_bytes().unwrap().0;
    let err = span
        .comment(&mut doc, "Again", "R", None, date())
        .unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::TargetNotFound);
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn multiple_comment_relationships_refuse_resolution_atomically() {
    let mut doc = Document::open_bytes(
        &fixture("generated/feature-isolated/comments.docx"),
        &Limits::default(),
    )
    .unwrap();
    let comment = doc.comments().unwrap().get(0).unwrap().clone();
    let rels = "word/_rels/document.xml.rels";
    let original =
        String::from_utf8(doc.package().part(rels).unwrap().current_bytes().to_vec()).unwrap();
    let duplicated = original.replace(
        "</Relationships>",
        r#"<Relationship Id="rId99" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/></Relationships>"#,
    );
    doc.package_mut_for_tests()
        .commit_bytes(rels, duplicated.into_bytes())
        .unwrap();
    let before = doc.patch_bytes().unwrap().0;
    let err = resolve(&mut doc, &comment, true).unwrap_err();
    assert_eq!(refusal(&err), RefusalKind::UnsupportedStructure);
    assert!(
        err.message().contains("multiple comments relationships"),
        "{err}"
    );
    let err = doc.comment_thread().unwrap_err();
    assert!(
        err.message().contains("multiple comments relationships"),
        "{err}"
    );
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn historical_view_spans_cannot_anchor_comments() {
    // "old " is deleted text: visible under view=original, not in the
    // current body.
    let bytes = minimal_package_with(&format!(
        r#"<w:document {W_DECLS}><w:body><w:p><w:r><w:t xml:space="preserve">keep </w:t></w:r><w:del w:id="1" w:author="A" w:date="2024-01-01T00:00:00Z"><w:r><w:delText xml:space="preserve">old </w:delText></w:r></w:del><w:ins w:id="2" w:author="A" w:date="2024-01-01T00:00:00Z"><w:r><w:t xml:space="preserve">new </w:t></w:r></w:ins><w:r><w:t>words</w:t></w:r></w:p></w:body></w:document>"#
    ));
    let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    for (needle, view) in [
        ("old words", View::Original),
        ("keep", View::Original),
        ("new words", View::All),
    ] {
        let span = doc
            .find_one(
                needle,
                &FindOptions {
                    view,
                    ..FindOptions::default()
                },
            )
            .unwrap_or_else(|err| panic!("{needle:?} under {view:?}: {err}"));
        let err = span
            .comment(&mut doc, "note", "Reviewer", None, date())
            .unwrap_err();
        assert_eq!(refusal(&err), RefusalKind::UnsupportedStructure);
        assert_eq!(err.reason(), "historical_view_target", "{needle:?}");
        let err = add_comment(&mut doc, &span, "note", "Reviewer", None, date()).unwrap_err();
        assert_eq!(err.reason(), "historical_view_target", "{needle:?}");
        assert_eq!(doc.patch_bytes().unwrap().0, bytes, "{needle:?}");
    }
    // The same text found under the current view anchors.
    let span = find(&doc, "keep");
    add_comment(&mut doc, &span, "note", "Reviewer", None, date()).unwrap();
}
