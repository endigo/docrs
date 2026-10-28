//! Footnotes and endnotes, including the note body and the reference mark —
//! upstream `notes.py`.
//!
//! Adapted from paper-docx (MIT); see `NOTICE.md`.

use crate::controls::refuse_control_surface;
use crate::links::{
    isolate_edge_runs, next_sibling, require_live_span, span_runs, validate_span_fresh,
};
use crate::oxml::{add_t, append, w_el};
use crate::package::ct;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::validate_writable_text;
use crate::search::Span;
use crate::tracked::insert_after;
use crate::txn::Txn;
use crate::unicode::casefold;
use crate::wml::{W, is_w, w_attr};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

const W_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Footnote,
    Endnote,
}

impl Kind {
    fn local(self) -> &'static str {
        match self {
            Kind::Footnote => "footnote",
            Kind::Endnote => "endnote",
        }
    }

    fn template(self) -> String {
        let kind = self.local();
        format!(
            "<w:{kind}s {W_DECL}><w:{kind} w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:{kind}>\
             <w:{kind} w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{kind}></w:{kind}s>"
        )
    }
}

/// Upstream `_ensure_notes_part`: the related notes part, or a new
/// `word/{footnotes,endnotes}.xml` from the separator template.
fn ensure_notes_part(txn: &mut Txn<'_>, kind: Kind) -> Result<String> {
    let (rel_type, name, content_type) = match kind {
        Kind::Footnote => (rt::FOOTNOTES, "word/footnotes.xml", ct::WML_FOOTNOTES),
        Kind::Endnote => (rt::ENDNOTES, "word/endnotes.xml", ct::WML_ENDNOTES),
    };
    crate::parts::ensure_part(
        txn,
        rel_type,
        name,
        content_type,
        kind.template().as_bytes(),
    )
}

/// The footnotes part and the next free footnote id, inside `txn` (the
/// part is created from the separator template when missing).
#[cfg(feature = "markdown")]
pub(crate) fn footnotes_part_and_next_id(txn: &mut Txn<'_>) -> Result<(String, i64)> {
    let part = ensure_notes_part(txn, Kind::Footnote)?;
    let next = next_note_id(txn.tree(&part)?, Kind::Footnote)?;
    Ok((part, next))
}

/// Upstream `_next_note_id`: one past the highest non-separator id among
/// the root's direct note children.
fn next_note_id(tree: &XmlTree, kind: Kind) -> Result<i64> {
    let mut highest: Option<i64> = None;
    for note in tree
        .child_elements(tree.root())
        .filter(|n| is_w(tree, *n, kind.local()))
    {
        if matches!(
            w_attr(tree, note, "type"),
            Some("separator" | "continuationSeparator")
        ) {
            continue;
        }
        let Some(raw) = w_attr(tree, note, "id") else {
            continue;
        };
        let id = crate::bookmarks::py_int(raw).ok_or_else(|| {
            Error::invalid_argument(
                "non_numeric_note_id",
                format!(
                    "invalid literal for int() with base 10: {}",
                    crate::controls::py_repr(raw)
                ),
            )
        })?;
        highest = Some(highest.map_or(id, |h| h.max(id)));
    }
    Ok(highest.unwrap_or(0) + 1)
}

fn is_note_mark(tree: &XmlTree, node: NodeId) -> bool {
    is_w(tree, node, "r")
        && tree
            .child_elements(node)
            .any(|c| is_w(tree, c, "footnoteReference") || is_w(tree, c, "endnoteReference"))
}

fn append_new(tree: &mut XmlTree, parent: NodeId, local: &str) -> Result<NodeId> {
    let child = w_el(tree, parent, local)?;
    append(tree, parent, child)?;
    Ok(child)
}

fn insert_note(document: &mut Document, span: &mut Span, text: &str, kind: Kind) -> Result<i64> {
    let operation = match kind {
        Kind::Footnote => "add a footnote",
        Kind::Endnote => "add an endnote",
    };
    let (staged, note_id, offsets) = {
        let doc: &Document = document;
        require_live_span(doc, span)?;
        refuse_if_protected(doc, operation, OperationClass::BodyContent)?;
        if text.is_empty() {
            return Err(Error::invalid_argument(
                "empty_note_text",
                "note text must be non-empty",
            ));
        }
        validate_writable_text(text, "text")?;
        validate_span_fresh(doc, span)?;
        let main_story = casefold(doc.package().main_part_name());
        if span.story != main_story {
            return Err(Error::unsupported(
                "note_outside_body",
                format!(
                    "footnotes and endnotes attach in the main document body (span is in {})",
                    span.story
                ),
            ));
        }
        if span.in_text_box {
            return Err(Error::unsupported(
                "note_in_text_box",
                "footnotes and endnotes cannot attach inside a text box",
            ));
        }
        if span.in_field {
            return Err(Error::unsupported(
                "span_in_field_result",
                "the span lies inside a field result; a note planted there is destroyed the next time the field updates",
            ));
        }
        refuse_control_surface(doc.package().xml(&span.part)?, span, "a note mark")?;
        let mut txn = Txn::new(doc);
        let offsets = isolate_edge_runs(txn.tree_mut(&span.part)?, span)?;
        let runs = span_runs(span);
        let Some(last_run) = runs.last().copied() else {
            return Err(Error::refusal(
                RefusalKind::TargetNotFound,
                "no_runs",
                "span has no runs to attach a note to",
            ));
        };
        let part = ensure_notes_part(&mut txn, kind)?;
        let notes = txn.tree_mut(&part)?;
        let note_id = next_note_id(notes, kind)?;
        let root = notes.root();
        let note = notes.create_element(root, W, kind.local())?;
        notes.set_attr(note, Some(W), "id", &note_id.to_string())?;
        let paragraph = append_new(notes, note, "p")?;
        let ref_run = append_new(notes, paragraph, "r")?;
        append_new(notes, ref_run, &format!("{}Ref", kind.local()))?;
        let text_run = append_new(notes, paragraph, "r")?;
        add_t(notes, text_run, &format!(" {text}"))?;
        append(notes, root, note)?;

        let tree = txn.tree_mut(&span.part)?;
        let mark = tree.create_element(last_run, W, "r")?;
        let mut anchor = last_run;
        while let Some(sibling) = next_sibling(tree, anchor).filter(|s| is_note_mark(tree, *s)) {
            anchor = sibling;
        }
        insert_after(tree, anchor, mark)?;
        let rpr = append_new(tree, mark, "rPr")?;
        let align = append_new(tree, rpr, "vertAlign")?;
        tree.set_attr(align, Some(W), "val", "superscript")?;
        let reference = append_new(tree, mark, &format!("{}Reference", kind.local()))?;
        tree.set_attr(reference, Some(W), "id", &note_id.to_string())?;
        (txn.finish()?, note_id, offsets)
    };
    document.commit(staged)?;
    span.start_offset = offsets.0;
    span.end_offset = offsets.1;
    Ok(note_id)
}

/// Attach a footnote to `span` and return its note id (upstream
/// `add_footnote`).
///
/// Creates the footnotes part on first use. The span stays usable for
/// further notes, as upstream's does. Refuses a protected document, a stale
/// or foreign span, a span outside the main body, and one inside a text
/// box, a field result, or a data-bound, locked or plain-text control.
pub fn add_footnote(document: &mut Document, span: &mut Span, text: &str) -> Result<i64> {
    insert_note(document, span, text, Kind::Footnote)
}

/// Attach an endnote to `span` and return its note id (upstream
/// `add_endnote`); same refusals as [`add_footnote`].
pub fn add_endnote(document: &mut Document, span: &mut Span, text: &str) -> Result<i64> {
    insert_note(document, span, text, Kind::Endnote)
}

impl Document {
    /// Attach a footnote (see [`add_footnote`]).
    pub fn add_footnote(&mut self, span: &mut Span, text: &str) -> Result<i64> {
        add_footnote(self, span, text)
    }

    /// Attach an endnote (see [`add_endnote`]).
    pub fn add_endnote(&mut self, span: &mut Span, text: &str) -> Result<i64> {
        add_endnote(self, span, text)
    }
}
