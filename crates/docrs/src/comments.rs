//! Document comments: the inherited python-docx API (`comments.py`,
//! `parts/comments.py`, `oxml/comments.py`; MIT, see `NOTICE.md`) and
//! upstream `Span.comment`.
//!
//! [`Document::comments`] is a read snapshot of the `w:comment` children of
//! the related comments part; it never creates the part (upstream
//! materializes an empty `word/comments.xml` on first read). [`Comment`]
//! handles carry freshness evidence: mutations re-validate that the comment
//! element is still a child of the document's live comments part.
//! Authoring creates `word/comments.xml` (from upstream's
//! `default-comments.xml` template) and the modern identity parts on first
//! use, all inside one transaction.

use serde::Serialize;

use crate::anchor::{BlockTarget, require_current_view};
use crate::clock::Timestamp;
use crate::commentops::{
    self, comment_element, ensure_comment_identity, mark_comment_range, migrate_comment_extension,
    preflight_comment_add, preflight_comment_range, preflight_comments_extended_write,
    validate_comment_arguments, validate_xml_characters,
};
use crate::package::{ct, rels::rt};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::revision::{RevisionDate, py_int, w_local};
use crate::search::Span;
use crate::txn::{RelTarget, Txn};
use crate::unicode::is_py_space;
use crate::wml::{W, leading_text, node_children, w_attr};
use crate::xml::{NodeId, XML_NS, XmlTree};
use crate::{Document, DocumentId, Error, RefusalKind, Result};

/// Upstream `templates/default-comments.xml` (MIT; see `NOTICE.md`).
const DEFAULT_COMMENTS: &[u8] = include_bytes!("../assets/default-comments.xml");

const COMMENTS_PARTNAME: &str = "word/comments.xml";

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

/// A comment (a read snapshot plus a live handle for mutations).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comment {
    /// `w:id`.
    pub comment_id: i64,
    /// `w:author` (empty when absent).
    pub author: String,
    /// `w:initials` (`None` when absent).
    pub initials: Option<String>,
    /// `w:date`, parsed like python-docx `ST_DateTime`.
    #[serde(serialize_with = "serialize_date")]
    pub timestamp: Option<RevisionDate>,
    /// Paragraph texts joined with `\n` (python-docx `Comment.text`).
    pub text: String,
    /// Direct paragraph texts.
    pub paragraphs: Vec<String>,
    #[serde(skip)]
    pub(crate) document: DocumentId,
    #[serde(skip)]
    pub(crate) part: String,
    #[serde(skip)]
    pub(crate) element: NodeId,
    #[serde(skip)]
    pub(crate) raw_id: String,
}

fn serialize_date<S: serde::Serializer>(
    value: &Option<RevisionDate>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    match value {
        Some(date) => s.serialize_str(&date.isoformat()),
        None => s.serialize_none(),
    }
}

impl Comment {
    /// The inspection payload used by the parity harness.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "comment_id": self.comment_id,
            "author": self.author,
            "initials": self.initials,
            "timestamp": self.timestamp.map(|d| d.isoformat()),
            "text": self.text,
        })
    }
}

/// The comments of a document, in part order.
#[derive(Debug, Clone, Default)]
pub struct Comments {
    items: Vec<Comment>,
}

impl Comments {
    /// Number of comments.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// No comments.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Comments in part order.
    pub fn iter(&self) -> std::slice::Iter<'_, Comment> {
        self.items.iter()
    }

    /// The first comment whose `w:id` attribute is exactly `comment_id`
    /// (python-docx `Comments.get` compares the attribute text).
    pub fn get(&self, comment_id: i64) -> Option<&Comment> {
        let wanted = comment_id.to_string();
        self.items.iter().find(|c| c.raw_id == wanted)
    }
}

impl<'a> IntoIterator for &'a Comments {
    type Item = &'a Comment;
    type IntoIter = std::slice::Iter<'a, Comment>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

/// python-docx `ST_DateTime.convert_from_xml` (never fails: unparseable
/// values become the Unix epoch, UTC).
pub(crate) fn st_datetime(raw: &str) -> RevisionDate {
    let epoch = RevisionDate {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_us: Some(0),
    };
    if let Some(body) = raw.strip_suffix('Z') {
        let plain = body.len() >= 19
            && body.as_bytes().get(10) == Some(&b'T')
            && !body.contains(['+', 'Z'])
            && body[10..].matches('-').count() == 0;
        if plain && let Some(mut date) = RevisionDate::from_isoformat(body) {
            date.offset_us = Some(0);
            return date;
        }
        return epoch;
    }
    RevisionDate::from_isoformat(raw).unwrap_or(epoch)
}

/// python-docx `CT_R.text`.
fn run_text(tree: &XmlTree, run: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(run) {
        match w_local(tree, child) {
            Some("t") => out.push_str(&leading_text(tree, child)),
            Some("tab" | "ptab") => out.push('\t'),
            Some("cr") => out.push('\n'),
            Some("br") => {
                if w_attr(tree, child, "type").unwrap_or("textWrapping") == "textWrapping" {
                    out.push('\n');
                }
            }
            Some("noBreakHyphen") => out.push('-'),
            _ => {}
        }
    }
    out
}

/// python-docx `CT_P.text` (`w:r | w:hyperlink` children).
pub(crate) fn paragraph_text(tree: &XmlTree, paragraph: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(paragraph) {
        match w_local(tree, child) {
            Some("r") => out.push_str(&run_text(tree, child)),
            Some("hyperlink") => {
                for run in tree.child_elements(child) {
                    if w_local(tree, run) == Some("r") {
                        out.push_str(&run_text(tree, run));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn comment_from(
    tree: &XmlTree,
    part: &str,
    element: NodeId,
    document: DocumentId,
) -> Result<Comment> {
    let raw = w_attr(tree, element, "id");
    let comment_id = raw.and_then(py_int).ok_or_else(|| {
        unsupported(
            "malformed_comment_id",
            format!(
                "malformed comment w:id={}; nothing was changed",
                crate::revision::py_opt_repr(raw)
            ),
        )
    })?;
    let paragraphs: Vec<String> = tree
        .child_elements(element)
        .filter(|child| w_local(tree, *child) == Some("p"))
        .map(|p| paragraph_text(tree, p))
        .collect();
    Ok(Comment {
        comment_id,
        author: w_attr(tree, element, "author").unwrap_or("").to_string(),
        initials: w_attr(tree, element, "initials").map(str::to_string),
        timestamp: w_attr(tree, element, "date").map(st_datetime),
        text: paragraphs.join("\n"),
        paragraphs,
        document,
        part: part.to_string(),
        element,
        raw_id: raw.unwrap_or("").to_string(),
    })
}

/// The comments of `part` as seen through `txn`.
pub(crate) fn comments_in(txn: &Txn<'_>, part: &str) -> Result<Comments> {
    let tree = txn.tree(part)?;
    let document = txn.document().id();
    let mut items = Vec::new();
    for element in tree.child_elements(tree.root()) {
        if w_local(tree, element) == Some("comment") {
            items.push(comment_from(tree, part, element, document)?);
        }
    }
    Ok(Comments { items })
}

/// The comment at `element` of `part` (after a commit).
pub(crate) fn comment_at(document: &Document, part: &str, element: NodeId) -> Result<Comment> {
    comment_from(document.package().xml(part)?, part, element, document.id())
}

/// Every comment of the document (empty when it has no comments part).
pub fn comments(document: &Document) -> Result<Comments> {
    let txn = Txn::new(document);
    match commentops::comments_part(&txn, "comment inspection")? {
        Some(part) => {
            let live = txn
                .content_type(&part)
                .is_some_and(|ct| ct::EAGER_XML.contains(&ct.as_str()));
            if !live {
                return Ok(Comments::default());
            }
            comments_in(&txn, &part)
        }
        None => Ok(Comments::default()),
    }
}

/// `DocumentPart._comments_part`: the related comments part, or a new
/// `word/comments.xml` from the default template.
fn ensure_comments_part(txn: &mut Txn<'_>) -> Result<String> {
    if let Some(part) = commentops::comments_part(txn, "comment authoring")? {
        return Ok(part);
    }
    if txn.resolve(COMMENTS_PARTNAME).is_some() {
        return Err(unsupported(
            "comments_part_name_taken",
            "a package part already occupies /word/comments.xml without the document comments relationship; \
             nothing was changed",
        ));
    }
    let main = txn.document().package().main_part_name().to_string();
    txn.add_xml_part(
        COMMENTS_PARTNAME,
        ct::WML_COMMENTS,
        crate::parts::template(DEFAULT_COMMENTS)?,
    )?;
    txn.relate_to(&main, rt::COMMENTS, RelTarget::Part(COMMENTS_PARTNAME))?;
    Ok(COMMENTS_PARTNAME.to_string())
}

/// `CT_Comments._next_available_comment_id`.
fn next_comment_id(tree: &XmlTree) -> Result<i64> {
    let mut used = Vec::new();
    for comment in tree.child_elements(tree.root()) {
        if w_local(tree, comment) != Some("comment") {
            continue;
        }
        if let Some(raw) = w_attr(tree, comment, "id") {
            used.push(py_int(raw).ok_or_else(|| {
                Error::invalid_argument(
                    "malformed_comment_id",
                    format!("invalid literal for int(): {raw:?}"),
                )
            })?);
        }
    }
    // `w:id` is a signed 32-bit decimal; past it, reuse the lowest gap.
    let next = used.iter().copied().max().unwrap_or(-1).checked_add(1);
    if let Some(next) = next.filter(|next| *next <= i64::from(i32::MAX)) {
        return Ok(next);
    }
    let mut sorted = used.clone();
    sorted.sort_unstable();
    for (expected, actual) in sorted.iter().enumerate() {
        if expected as i64 != *actual {
            return Ok(expected as i64);
        }
    }
    let next = used.len() as i64;
    if next > i64::from(i32::MAX) {
        return Err(Error::invalid_argument(
            "comment_id_exhausted",
            "every comment id up to 2147483647 is taken; nothing was changed",
        ));
    }
    Ok(next)
}

fn append(tree: &mut XmlTree, parent: NodeId, child: NodeId) -> Result<()> {
    let end = tree.children(parent).len();
    tree.insert(parent, end, child)
}

/// `_RunContentAppender.append_to_run_from_text`.
fn append_run_text(tree: &mut XmlTree, run: NodeId, text: &str) -> Result<()> {
    let mut buffer = String::new();
    let flush = |tree: &mut XmlTree, buffer: &mut String| -> Result<()> {
        if !buffer.is_empty() {
            crate::oxml::add_t(tree, run, buffer)?;
            buffer.clear();
        }
        Ok(())
    };
    for c in text.chars() {
        match c {
            '\t' => {
                flush(tree, &mut buffer)?;
                let tab = tree.create_element(run, W, "tab")?;
                append(tree, run, tab)?;
            }
            '\r' | '\n' => {
                flush(tree, &mut buffer)?;
                let br = tree.create_element(run, W, "br")?;
                append(tree, run, br)?;
            }
            c => buffer.push(c),
        }
    }
    flush(tree, &mut buffer)
}

/// `Paragraph.add_run(text)`.
fn add_run(tree: &mut XmlTree, paragraph: NodeId, text: &str) -> Result<()> {
    let run = tree.create_element(paragraph, W, "r")?;
    append(tree, paragraph, run)?;
    if !text.is_empty() {
        append_run_text(tree, run, text)?;
    }
    Ok(())
}

/// `CT_Comments.add_comment` plus the attribute writes of
/// `Comments.add_comment`.
fn new_comment_element(
    tree: &mut XmlTree,
    id: i64,
    author: &str,
    initials: Option<&str>,
    date: &str,
) -> Result<NodeId> {
    let root = tree.root();
    let comment = tree.create_element(root, W, "comment")?;
    append(tree, root, comment)?;
    tree.set_attr(comment, Some(W), "id", &id.to_string())?;
    tree.set_attr(comment, Some(W), "author", author)?;
    if let Some(initials) = initials {
        tree.set_attr(comment, Some(W), "initials", initials)?;
    }
    tree.set_attr(comment, Some(W), "date", date)?;
    let p = tree.create_element(comment, W, "p")?;
    append(tree, comment, p)?;
    let p_pr = tree.create_element(p, W, "pPr")?;
    append(tree, p, p_pr)?;
    let style = tree.create_element(p_pr, W, "pStyle")?;
    append(tree, p_pr, style)?;
    tree.set_attr(style, Some(W), "val", "CommentText")?;
    let run = tree.create_element(p, W, "r")?;
    append(tree, p, run)?;
    let r_pr = tree.create_element(run, W, "rPr")?;
    append(tree, run, r_pr)?;
    let r_style = tree.create_element(r_pr, W, "rStyle")?;
    append(tree, r_pr, r_style)?;
    tree.set_attr(r_style, Some(W), "val", "CommentReference")?;
    let annotation = tree.create_element(run, W, "annotationRef")?;
    append(tree, run, annotation)?;
    Ok(comment)
}

/// Append a comment paragraph (`Comment.add_paragraph(text)` with the
/// default `CommentText` style) and migrate thread state.
fn stage_comment_paragraph(
    txn: &mut Txn<'_>,
    part: &str,
    comment: NodeId,
    text: &str,
) -> Result<NodeId> {
    let previous_last = commentops::last_paragraph(txn.tree(part)?, comment);
    let tree = txn.tree_mut(part)?;
    let p = tree.create_element(comment, W, "p")?;
    append(tree, comment, p)?;
    if !text.is_empty() {
        add_run(tree, p, text)?;
    }
    crate::oxml::set_paragraph_style(tree, p, Some("CommentText"))?;
    if let Some(previous) = previous_last {
        migrate_comment_extension(txn, part, comment, previous)?;
    }
    Ok(p)
}

/// Stage upstream `Comments.add_comment(text, author, initials)`: the new
/// comment (part created on demand) with its identity rows. `date` is the
/// `ST_DateTime` value (`None` → now). Returns (part, element, id).
pub(crate) fn stage_add_comment(
    txn: &mut Txn<'_>,
    text: &str,
    author: &str,
    initials: Option<&str>,
    date: Option<Timestamp>,
) -> Result<(String, NodeId, i64)> {
    let part = ensure_comments_part(txn)?;
    validate_xml_characters(author, "author")?;
    if let Some(initials) = initials {
        validate_xml_characters(initials, "initials")?;
    }
    validate_xml_characters(text, "text")?;
    let date = date.unwrap_or_else(Timestamp::now).to_xml();
    let id = next_comment_id(txn.tree(&part)?)?;
    let comment = new_comment_element(txn.tree_mut(&part)?, id, author, initials, &date)?;
    if !text.is_empty() {
        let mut lines = text.split('\n');
        let first = lines.next().unwrap_or("");
        let first_p = {
            let tree = txn.tree(&part)?;
            tree.child_elements(comment)
                .find(|c| w_local(tree, *c) == Some("p"))
                .ok_or_else(|| {
                    Error::internal("comment_paragraph", "new comment lacks a paragraph")
                })?
        };
        if !first.is_empty() {
            add_run(txn.tree_mut(&part)?, first_p, first)?;
        }
        for line in lines {
            stage_comment_paragraph(txn, &part, comment, line)?;
        }
    }
    ensure_comment_identity(txn, &part, comment)?;
    Ok((part, comment, id))
}

fn main_part(document: &Document) -> String {
    document.package().main_part_name().to_string()
}

fn main_story(document: &Document) -> Option<String> {
    let main = main_part(document);
    crate::story::stories(document)
        .into_iter()
        .find(|story| story.part == main)
        .map(|story| story.name)
}

/// Distinct runs of a span's atoms, in order.
fn span_runs(span: &Span) -> Vec<Option<NodeId>> {
    let mut runs: Vec<Option<NodeId>> = Vec::new();
    for atom in &span.atoms {
        if !runs.contains(&atom.run) {
            runs.push(atom.run);
        }
    }
    runs
}

/// Upstream `Comments.add_comment(text, author, initials)`: a comment with
/// no anchor in the document text. `initials: None` omits `w:initials`.
pub fn add_unanchored_comment(
    document: &mut Document,
    text: &str,
    author: &str,
    initials: Option<&str>,
    date: Option<Timestamp>,
) -> Result<Comment> {
    let (staged, part, element) = {
        let mut txn = Txn::new(document);
        refuse_if_protected(document, "add a comment", OperationClass::Comment)?;
        preflight_comment_add(&txn)?;
        let (part, element, _) = stage_add_comment(&mut txn, text, author, initials, date)?;
        (txn.finish()?, part, element)
    };
    document.commit(staged)?;
    comment_at(document, &part, element)
}

/// Upstream `Document.add_comment(runs, text, author, initials)`, with the
/// anchor runs given as the runs holding `runs`' first and last text atoms
/// (whole runs are anchored, as upstream does for `Run` arguments).
pub fn add_comment(
    document: &mut Document,
    runs: &Span,
    text: &str,
    author: &str,
    initials: Option<&str>,
    date: Option<Timestamp>,
) -> Result<Comment> {
    let main = main_part(document);
    if runs.document != document.id() || runs.part != main {
        return Err(unsupported(
            "foreign_comment_runs",
            "comment range runs do not belong to this document; nothing was changed",
        ));
    }
    require_current_view(BlockTarget::Span(runs))?;
    let tree = document.package().xml(&main)?;
    crate::replace::validate_fresh(document, tree, runs, None)?;
    let selected = span_runs(runs);
    let (Some(Some(first)), Some(Some(last))) =
        (selected.first().copied(), selected.last().copied())
    else {
        return Err(unsupported(
            "span_outside_runs",
            "span text is not inside runs; cannot anchor a comment",
        ));
    };
    preflight_comment_range(document, tree, first, last, "add a comment")?;
    let (staged, part, element) = {
        let mut txn = Txn::new(document);
        preflight_comment_add(&txn)?;
        let (part, element, id) = stage_add_comment(&mut txn, text, author, initials, date)?;
        mark_comment_range(txn.tree_mut(&main)?, first, last, id)?;
        (txn.finish()?, part, element)
    };
    document.commit(staged)?;
    comment_at(document, &part, element)
}

fn live_comment(document: &Document, comment: &Comment, operation: &str) -> Result<()> {
    let txn = Txn::new(document);
    comment_element(&txn, comment)?;
    refuse_if_protected(document, operation, OperationClass::Comment)
}

/// Upstream `Comment.add_paragraph(text)` (default `CommentText` style):
/// re-keys thread and resolution state onto the new last paragraph.
pub fn add_comment_paragraph(
    document: &mut Document,
    comment: &Comment,
    text: &str,
) -> Result<Comment> {
    live_comment(document, comment, "edit a comment")?;
    validate_xml_characters(text, "text")?;
    let staged = {
        let mut txn = Txn::new(document);
        preflight_comment_add(&txn)?;
        preflight_comments_extended_write(&txn)?;
        stage_comment_paragraph(&mut txn, &comment.part, comment.element, text)?;
        txn.finish()?
    };
    document.commit(staged)?;
    comment_at(document, &comment.part, comment.element)
}

/// Upstream `Comment.add_table(rows, cols, width)` (`width` in EMU).
pub fn add_comment_table(
    document: &mut Document,
    comment: &Comment,
    rows: usize,
    cols: usize,
    width_emu: i64,
) -> Result<Comment> {
    live_comment(document, comment, "edit a comment")?;
    let staged = {
        let mut txn = Txn::new(document);
        preflight_comment_add(&txn)?;
        preflight_comments_extended_write(&txn)?;
        let previous = commentops::last_paragraph(txn.tree(&comment.part)?, comment.element);
        {
            let tree = txn.tree_mut(&comment.part)?;
            let cells = vec![vec![String::new(); cols]; rows];
            let table = crate::oxml::new_table(tree, comment.element, &cells, width_emu)?;
            append(tree, comment.element, table)?;
        }
        if let Some(previous) = previous {
            migrate_comment_extension(&mut txn, &comment.part, comment.element, previous)?;
        }
        txn.finish()?
    };
    document.commit(staged)?;
    comment_at(document, &comment.part, comment.element)
}

/// Upstream `Comment.author = value`.
pub fn set_comment_author(
    document: &mut Document,
    comment: &Comment,
    author: &str,
) -> Result<Comment> {
    set_comment_attr(document, comment, "author", Some(author))
}

/// Upstream `Comment.initials = value` (`None` removes `w:initials`).
pub fn set_comment_initials(
    document: &mut Document,
    comment: &Comment,
    initials: Option<&str>,
) -> Result<Comment> {
    set_comment_attr(document, comment, "initials", initials)
}

fn set_comment_attr(
    document: &mut Document,
    comment: &Comment,
    name: &str,
    value: Option<&str>,
) -> Result<Comment> {
    live_comment(document, comment, "edit a comment")?;
    if let Some(value) = value {
        validate_xml_characters(value, name)?;
    }
    let staged = {
        let mut txn = Txn::new(document);
        let tree = txn.tree_mut(&comment.part)?;
        match value {
            Some(value) => tree.set_attr(comment.element, Some(W), name, value)?,
            None => {
                tree.remove_attr(comment.element, Some(W), name);
            }
        }
        txn.finish()?
    };
    document.commit(staged)?;
    comment_at(document, &comment.part, comment.element)
}

// ---------------------------------------------------------------------------
// Span.comment
// ---------------------------------------------------------------------------

fn atom_text(tree: &XmlTree, atom: &crate::search::Atom) -> String {
    atom.text(tree)
}

fn slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Upstream `_set_preserved_text`.
fn set_preserved_text(tree: &mut XmlTree, element: NodeId, text: &str) -> Result<()> {
    crate::wml::set_leading_text(tree, element, text)?;
    let edge = text.chars().next().is_some_and(is_py_space)
        || text.chars().last().is_some_and(is_py_space);
    if edge {
        tree.set_attr(element, Some(XML_NS), "space", "preserve")?;
    } else {
        tree.remove_attr(element, Some(XML_NS), "space");
    }
    Ok(())
}

fn new_run_like(tree: &mut XmlTree, run: NodeId) -> Result<NodeId> {
    let new_run = tree.create_element(run, W, "r")?;
    if let Some(r_pr) = tree.first_child_named(run, W, "rPr") {
        let copy = tree.deep_clone(r_pr);
        append(tree, new_run, copy)?;
    }
    Ok(new_run)
}

fn split_before(tree: &mut XmlTree, run: NodeId, element: NodeId, prefix: &str) -> Result<()> {
    let mut preceding = Vec::new();
    for child in node_children(tree, run) {
        if child == element {
            break;
        }
        if w_local(tree, child) != Some("rPr") {
            preceding.push(child);
        }
    }
    if preceding.is_empty() && prefix.is_empty() {
        return Ok(());
    }
    let new_run = new_run_like(tree, run)?;
    let parent = tree
        .parent(run)
        .ok_or_else(|| Error::internal("detached_run", "run is detached"))?;
    let index = tree.index_in_parent(run).unwrap_or(0);
    tree.insert(parent, index, new_run)?;
    for child in preceding {
        tree.detach(child)?;
        append(tree, new_run, child)?;
    }
    if !prefix.is_empty() {
        crate::oxml::add_t(tree, new_run, prefix)?;
    }
    Ok(())
}

fn split_after(tree: &mut XmlTree, run: NodeId, element: NodeId, tail: &str) -> Result<()> {
    let mut following = Vec::new();
    let mut seen = false;
    for child in node_children(tree, run) {
        if seen && w_local(tree, child) != Some("rPr") {
            following.push(child);
        }
        if child == element {
            seen = true;
        }
    }
    if following.is_empty() && tail.is_empty() {
        return Ok(());
    }
    let new_run = new_run_like(tree, run)?;
    let parent = tree
        .parent(run)
        .ok_or_else(|| Error::internal("detached_run", "run is detached"))?;
    let index = tree.index_in_parent(run).unwrap_or(0) + 1;
    tree.insert(parent, index, new_run)?;
    if !tail.is_empty() {
        crate::oxml::add_t(tree, new_run, tail)?;
    }
    for child in following {
        tree.detach(child)?;
        append(tree, new_run, child)?;
    }
    Ok(())
}

/// Upstream `Span._isolate_edge_runs`.
fn isolate_edge_runs(tree: &mut XmlTree, span: &Span) -> Result<()> {
    let mut start_offset = span.start_offset;
    let mut end_offset = span.end_offset;
    let first = &span.atoms[0];
    if let Some(run) = first.run {
        let text = atom_text(tree, first);
        let prefix = if start_offset > 0 {
            slice(&text, 0, start_offset)
        } else {
            String::new()
        };
        split_before(tree, run, first.element, &prefix)?;
        if !prefix.is_empty() {
            let length = text.chars().count();
            set_preserved_text(tree, first.element, &slice(&text, start_offset, length))?;
            if span.atoms.len() == 1 {
                end_offset -= start_offset;
            }
            start_offset = 0;
        }
    }
    let _ = start_offset;
    let last = &span.atoms[span.atoms.len() - 1];
    if let Some(run) = last.run {
        let text = atom_text(tree, last);
        let length = text.chars().count();
        let tail = if end_offset < length {
            slice(&text, end_offset, length)
        } else {
            String::new()
        };
        split_after(tree, run, last.element, &tail)?;
        if !tail.is_empty() {
            set_preserved_text(tree, last.element, &slice(&text, 0, end_offset))?;
        }
    }
    Ok(())
}

impl Span {
    /// Anchor a new comment to exactly this span's text (upstream
    /// `Span.comment`): boundary runs are split so the range marks wrap the
    /// span text; `word/comments.xml` is created on first use. Only the main
    /// document story carries comments. `date: None` stamps now.
    ///
    /// When a leading prefix is split off the first run the span itself
    /// becomes stale (Rust spans are immutable values; upstream rewrites
    /// the span's offsets in place); re-find the text to reuse it.
    pub fn comment(
        &self,
        document: &mut Document,
        text: &str,
        author: &str,
        initials: Option<&str>,
        date: Option<Timestamp>,
    ) -> Result<Comment> {
        validate_comment_arguments(text, author, initials)?;
        if self.document != document.id() {
            return Err(Error::refusal(
                RefusalKind::BoundaryViolation,
                "foreign_span",
                "span belongs to a different document; re-find the text in the document passed to this operation",
            ));
        }
        refuse_if_protected(document, "anchor a comment", OperationClass::Comment)?;
        require_current_view(BlockTarget::Span(self))?;
        let main_story_name = main_story(document).unwrap_or_default();
        if self.story != main_story_name {
            return Err(unsupported(
                "comment_outside_main_story",
                format!(
                    "comments anchor in the main document story (span is in {})",
                    self.story
                ),
            ));
        }
        let main = main_part(document);
        let tree = document.package().xml(&main)?;
        crate::replace::validate_fresh(document, tree, self, None)?;
        if self.atoms.iter().any(|atom| atom.run.is_none()) {
            return Err(unsupported(
                "span_outside_runs",
                "span text is not inside runs; cannot anchor a comment",
            ));
        }
        let (staged, part, element) = {
            let mut txn = Txn::new(document);
            preflight_comment_add(&txn)?;
            let runs: Vec<NodeId> = span_runs(self).into_iter().flatten().collect();
            preflight_comment_range(
                document,
                tree,
                runs[0],
                runs[runs.len() - 1],
                "anchor a comment",
            )?;
            isolate_edge_runs(txn.tree_mut(&main)?, self)?;
            let (part, element, id) =
                stage_add_comment(&mut txn, text, author, Some(initials.unwrap_or("")), date)?;
            mark_comment_range(txn.tree_mut(&main)?, runs[0], runs[runs.len() - 1], id)?;
            (txn.finish()?, part, element)
        };
        document.commit(staged)?;
        comment_at(document, &part, element)
    }
}

impl Document {
    /// Every comment (upstream `Document.comments`; never creates the part).
    pub fn comments(&self) -> Result<Comments> {
        comments(self)
    }

    /// Upstream `Document.add_comment` (see [`add_comment`]).
    pub fn add_comment(
        &mut self,
        runs: &Span,
        text: &str,
        author: &str,
        initials: Option<&str>,
        date: Option<Timestamp>,
    ) -> Result<Comment> {
        add_comment(self, runs, text, author, initials, date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn st_datetime_matches_python_docx() {
        assert_eq!(
            st_datetime("2024-01-02T03:04:05Z").isoformat(),
            "2024-01-02T03:04:05+00:00"
        );
        assert_eq!(
            st_datetime("2024-01-02T03:04:05.25Z").isoformat(),
            "2024-01-02T03:04:05.250000+00:00"
        );
        assert_eq!(
            st_datetime("nonsense").isoformat(),
            "1970-01-01T00:00:00+00:00"
        );
        assert_eq!(
            st_datetime("2024-01-02T03:04:05").isoformat(),
            "2024-01-02T03:04:05"
        );
    }

    #[test]
    fn iter_w_is_descendant_or_self() {
        let tree = crate::xml::parse_str(&format!("<w:p xmlns:w=\"{W}\"><w:r/></w:p>")).unwrap();
        assert_eq!(
            crate::revision::iter_w(&tree, tree.root(), &["p", "r"]).count(),
            2
        );
    }
}
