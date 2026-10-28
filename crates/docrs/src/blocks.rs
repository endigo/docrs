//! Anchor-relative block operations and clause-level redlines (upstream
//! `blocks.py`, MIT; see `NOTICE.md`).
//!
//! Operations compose whole paragraphs: insert a section or a typed block
//! list after an anchor, tracked-delete a paragraph range, tracked-replace a
//! range. Every selected paragraph must share one parent element; ranges
//! crossing story regions, tables or content controls refuse.
//!
//! Tracked deletion moves the paragraph's own runs into `w:del` (retagging
//! `w:t` → `w:delText`), keeping each run's `rPr`; tracked insert/delete also
//! stamp the paragraph mark (`w:pPr/w:rPr/w:ins|w:del`), so accepting a
//! deletion removes the paragraph and rejecting an insertion removes it.
//!
//! Every operation validates first and stages its writes in a [`Txn`].

use std::collections::BTreeMap;

use serde::Serialize;

use crate::anchor::{
    BlockTarget, Located, locate_paragraph, require_current_view, require_owner,
    resolve_anchor_paragraph,
};
use crate::clock::Timestamp;
use crate::image::py_round;
use crate::numbering::{
    ListKind, StylesView, add_next, ensure_definition_in, set_p_style, validated_style_id,
};
use crate::oxml::{self, add_t, append, get_or_add_ppr, set_w, w_el};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::validate_writable_text;
use crate::revision::w_local;
use crate::story::{count_fldchar_delta, iter_block_elements};
use crate::styles::py_int;
use crate::tracked::{named_bookmarks_in, new_revision, next_revision_id};
use crate::txn::Txn;
use crate::wml::{is_w, iter_nodes, leading_text, node_children, w_attr, w_child, w_children};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// Outcome of a block-level edit (upstream `BlockEditResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockEditResult {
    /// Story edited.
    pub story: String,
    /// Inserted blocks (paragraphs, tables and table padding).
    pub inserted_blocks: usize,
    /// Paragraphs marked deleted.
    pub deleted_blocks: usize,
    /// Visible text of each deleted paragraph.
    pub deleted_text: Vec<String>,
    /// Revision ids stamped, in order.
    pub revision_ids: Vec<i64>,
}

impl BlockEditResult {
    /// Upstream `to_dict()` (`schema: paper_block_edit`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_block_edit",
            "version": 1,
            "story": self.story,
            "inserted_blocks": self.inserted_blocks,
            "deleted_blocks": self.deleted_blocks,
            "deleted_text": self.deleted_text,
            "revision_ids": self.revision_ids,
        })
    }
}

/// One run of a rich paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextRun {
    /// Run text.
    pub text: String,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
}

/// A paragraph built from styled runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RichParagraph {
    /// Runs in order.
    pub runs: Vec<TextRun>,
    /// Paragraph style id, validated against the document.
    pub style: Option<String>,
}

/// A real bullet/decimal list backed by a canonical numbering definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListBlock {
    /// Item texts.
    pub items: Vec<String>,
    /// `"bullet"` or `"decimal"`.
    pub kind: String,
    /// Level 0..=2.
    pub level: i64,
}

impl Default for ListBlock {
    fn default() -> Self {
        ListBlock {
            items: Vec::new(),
            kind: "bullet".to_string(),
            level: 0,
        }
    }
}

/// A rectangular table of plain-text cells.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableBlock {
    /// Rows of cell texts.
    pub rows: Vec<Vec<String>>,
}

/// One typed block for [`insert_blocks_after`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RichBlock {
    /// A styled paragraph.
    Paragraph(RichParagraph),
    /// A list.
    List(ListBlock),
    /// A table.
    Table(TableBlock),
}

/// Options for [`insert_section_after`].
#[derive(Debug, Clone)]
pub struct SectionOptions<'a> {
    /// Heading style id (default `Heading2`).
    pub heading_style: Option<&'a str>,
    /// Body paragraph style id.
    pub body_style: Option<&'a str>,
    /// Insert as tracked changes.
    pub tracked: bool,
    /// Revision author (required when tracked).
    pub author: Option<&'a str>,
    /// Revision date (`None` = now).
    pub date: Option<Timestamp>,
}

impl Default for SectionOptions<'_> {
    fn default() -> Self {
        SectionOptions {
            heading_style: Some("Heading2"),
            body_style: None,
            tracked: false,
            author: None,
            date: None,
        }
    }
}

/// A paragraph range: `start` through `end`, or `count` sibling paragraphs.
#[derive(Debug, Clone, Copy)]
pub struct ParagraphRange<'a> {
    /// First paragraph.
    pub start: BlockTarget<'a>,
    /// Last paragraph (overrides `count`).
    pub end: Option<BlockTarget<'a>>,
    /// Number of sibling paragraphs when `end` is `None` (>= 1).
    pub count: i64,
}

impl<'a> ParagraphRange<'a> {
    /// The single paragraph at `start`.
    pub fn one(start: BlockTarget<'a>) -> Self {
        ParagraphRange {
            start,
            end: None,
            count: 1,
        }
    }
}

fn value_error(reason: &'static str, message: impl Into<String>) -> Error {
    Error::invalid_argument(reason, message)
}

fn is_xml_representable(c: char) -> bool {
    matches!(c as u32, 0x09 | 0x0A | 0x0D | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// Upstream `search._validate_xml_characters`.
pub(crate) fn validate_xml_characters(value: &str, argument: &str) -> Result<()> {
    let mut invalid: Vec<String> = value
        .chars()
        .filter(|c| !is_xml_representable(*c))
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    invalid.sort();
    invalid.dedup();
    if !invalid.is_empty() {
        return Err(value_error(
            "invalid_xml_characters",
            format!("{argument} contains character(s) XML 1.0 cannot represent: {invalid:?}"),
        ));
    }
    Ok(())
}

/// Upstream `_validate_tracked_identity` (the date is typed in Rust).
fn validate_tracked_identity(author: Option<&str>) -> Result<()> {
    if let Some(author) = author {
        validate_xml_characters(author, "author")?;
    }
    Ok(())
}

/// Upstream `_refuse_cell_anchor`.
fn refuse_cell_anchor(tree: &XmlTree, paragraph: NodeId) -> Result<()> {
    if tree
        .parent(paragraph)
        .is_some_and(|parent| is_w(tree, parent, "tc"))
    {
        return Err(Error::unsupported(
            "cell_anchor",
            "anchor resolves inside a table cell; block insertion targets body-level paragraphs — anchor on a \
             paragraph outside the table",
        ));
    }
    Ok(())
}

/// Upstream `_field_open_flags`: complex-field state before and after the
/// block containing `element` (a top-level content control is measured
/// among the body children).
pub(crate) fn field_open_flags(
    tree: &XmlTree,
    story: &str,
    element: NodeId,
) -> Result<(bool, bool)> {
    if let Some(body) = w_child(tree, tree.root(), "body")
        && is_w(tree, element, "sdt")
        && tree.parent(element) == Some(body)
    {
        let mut depth = 0i64;
        for child in node_children(tree, body) {
            if child == element {
                return Ok((depth > 0, depth + count_fldchar_delta(tree, element) > 0));
            }
            depth = (depth + count_fldchar_delta(tree, child)).max(0);
        }
    }
    let mut depth = 0i64;
    for block in iter_block_elements(tree, story)? {
        let contains = block.element == element
            || tree
                .descendants(block.element)
                .any(|node| node == element && is_w(tree, node, "p"));
        let delta = count_fldchar_delta(tree, block.element);
        if contains {
            return Ok((depth > 0, depth + delta > 0));
        }
        depth = (depth + delta).max(0);
    }
    Ok((false, false))
}

/// Upstream `_refuse_paragraph_in_open_field`.
pub(crate) fn refuse_paragraph_in_open_field(
    tree: &XmlTree,
    story: &str,
    paragraph: NodeId,
    for_insertion: bool,
) -> Result<()> {
    let (open_before, open_after) = field_open_flags(tree, story, paragraph)?;
    let blocked = if for_insertion {
        open_after
    } else {
        open_before || open_after
    };
    if blocked {
        return Err(Error::unsupported(
            "inside_open_field",
            "target lies inside a field result that spans paragraphs (a TOC or similar); Word regenerates field \
             results on update, so content written there would silently vanish",
        ));
    }
    Ok(())
}

const TRANSPARENT_PARAGRAPH_CHILDREN: &[&str] = &[
    "proofErr",
    "bookmarkStart",
    "bookmarkEnd",
    "commentRangeStart",
    "commentRangeEnd",
];

const SAFE_RUN_CHILDREN: &[&str] = &[
    "rPr",
    "t",
    "tab",
    "br",
    "cr",
    "commentReference",
    "lastRenderedPageBreak",
];

fn display_tag(tree: &XmlTree, node: NodeId) -> String {
    match tree.data(node) {
        NodeData::Element(_) => tree.local_name(node).unwrap_or("").to_string(),
        NodeData::Comment(_) => "comment".to_string(),
        NodeData::Pi { .. } => "processing instruction".to_string(),
        NodeData::Text(_) => "text".to_string(),
    }
}

/// Upstream `_validate_deletable_paragraph`.
fn validate_deletable_paragraph(tree: &XmlTree, paragraph: NodeId) -> Result<()> {
    let hollowable = named_bookmarks_in(tree, paragraph);
    if !hollowable.is_empty() {
        return Err(Error::unsupported(
            "named_bookmark",
            format!(
                "paragraph carries named bookmark(s) {hollowable:?} — the targets of REF/PAGEREF/TOC references; \
                 deleting it would hollow them. Remove the bookmark deliberately first (only point bookmarks like \
                 _GoBack are transparent)"
            ),
        ));
    }
    for child in node_children(tree, paragraph) {
        let local = w_local(tree, child);
        if local == Some("pPr")
            || local.is_some_and(|l| TRANSPARENT_PARAGRAPH_CHILDREN.contains(&l))
        {
            continue;
        }
        if local != Some("r") {
            return Err(Error::unsupported(
                "non_run_content",
                format!(
                    "tracked paragraph delete supports plain runs only; found {:?} content (hyperlinks, controls \
                     and existing revisions are refused, not corrupted)",
                    display_tag(tree, child)
                ),
            ));
        }
        for run_child in node_children(tree, child) {
            if !w_local(tree, run_child).is_some_and(|l| SAFE_RUN_CHILDREN.contains(&l)) {
                return Err(Error::unsupported(
                    "non_text_run",
                    format!(
                        "tracked paragraph delete supports text-only runs; found {:?} inside a run",
                        display_tag(tree, run_child)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Upstream `_paragraph_visible_text`.
fn paragraph_visible_text(tree: &XmlTree, paragraph: NodeId) -> String {
    let mut out = String::new();
    for node in iter_nodes(tree, paragraph) {
        match w_local(tree, node) {
            Some("t") => out.push_str(&leading_text(tree, node)),
            Some("tab") => out.push('\t'),
            Some("br" | "cr") => out.push('\n'),
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct Stamp<'a> {
    author: &'a str,
    date: Timestamp,
}

/// `CT_RunTrackChange.new(tag, id, author, date)`.
fn new_track_change(
    tree: &mut XmlTree,
    scope: NodeId,
    tag: &str,
    revision_id: i64,
    stamp: Stamp<'_>,
) -> Result<NodeId> {
    new_revision(
        tree,
        scope,
        tag,
        revision_id,
        stamp.author,
        &stamp.date.to_xml(),
    )
}

/// Upstream `_stamp_paragraph_mark`.
fn stamp_paragraph_mark(
    tree: &mut XmlTree,
    paragraph: NodeId,
    tag: &str,
    revision_id: i64,
    stamp: Stamp<'_>,
) -> Result<()> {
    crate::tracked::stamp_paragraph_mark(
        tree,
        paragraph,
        tag,
        revision_id,
        stamp.author,
        &stamp.date.to_xml(),
    )
}

/// Detach `node` and its tail text (lxml moves an element with its tail).
fn detach_with_tail(tree: &mut XmlTree, node: NodeId) -> Result<Vec<NodeId>> {
    let mut moved = vec![node];
    if let Some(parent) = tree.parent(node) {
        let children = tree.children(parent);
        let index = tree.index_in_parent(node).unwrap_or(0);
        moved.extend(
            children[index + 1..]
                .iter()
                .copied()
                .take_while(|c| matches!(tree.data(*c), NodeData::Text(_))),
        );
    }
    for item in &moved {
        tree.detach(*item)?;
    }
    Ok(moved)
}

/// Upstream `_wrap_paragraph_content_as_insertion`.
fn wrap_paragraph_content_as_insertion(
    tree: &mut XmlTree,
    paragraph: NodeId,
    revision_id: i64,
    stamp: Stamp<'_>,
) -> Result<()> {
    let ins = new_track_change(tree, paragraph, "ins", revision_id, stamp)?;
    append(tree, paragraph, ins)?;
    let children: Vec<NodeId> = node_children(tree, paragraph).collect();
    for child in children {
        if child == ins || w_local(tree, child) == Some("pPr") {
            continue;
        }
        for node in detach_with_tail(tree, child)? {
            append(tree, ins, node)?;
        }
    }
    Ok(())
}

/// Upstream `_mark_paragraph_deleted`; returns the deleted visible text.
fn mark_paragraph_deleted(
    tree: &mut XmlTree,
    paragraph: NodeId,
    revision_id: i64,
    stamp: Stamp<'_>,
) -> Result<String> {
    let text = paragraph_visible_text(tree, paragraph);
    let children: Vec<NodeId> = node_children(tree, paragraph).collect();
    let mut placement = None;
    let mut moved = Vec::new();
    let mut proof_errs = Vec::new();
    let mut remaining = Vec::new();
    for (position, child) in children.iter().enumerate() {
        match w_local(tree, *child) {
            Some("pPr") => remaining.push(*child),
            Some("proofErr") => proof_errs.push(*child),
            Some(local) if TRANSPARENT_PARAGRAPH_CHILDREN.contains(&local) => {
                remaining.push(*child)
            }
            _ => {
                placement.get_or_insert(position);
                moved.push(*child);
            }
        }
    }
    for node in proof_errs {
        detach_with_tail(tree, node)?;
    }
    let mut moved_nodes = Vec::new();
    for node in moved {
        moved_nodes.extend(detach_with_tail(tree, node)?);
    }
    let deletion = new_track_change(tree, paragraph, "del", revision_id, stamp)?;
    match placement {
        Some(placement) if placement < remaining.len() => {
            let before = remaining[placement];
            let index = tree.index_in_parent(before).unwrap_or(0);
            tree.insert(paragraph, index, deletion)?;
        }
        _ => append(tree, paragraph, deletion)?,
    }
    for node in moved_nodes {
        append(tree, deletion, node)?;
    }
    let texts: Vec<NodeId> = tree
        .descendants(deletion)
        .filter(|node| w_local(tree, *node) == Some("t"))
        .collect();
    for t in texts {
        tree.set_local_name(t, "delText")?;
    }
    stamp_paragraph_mark(tree, paragraph, "del", revision_id, stamp)?;
    Ok(text)
}

/// Upstream `_insert_after`: each node after the previous (lxml `addnext`).
pub(crate) fn insert_after(tree: &mut XmlTree, anchor: NodeId, nodes: &[NodeId]) -> Result<()> {
    let mut point = anchor;
    for node in nodes {
        add_next(tree, point, *node)?;
        point = *node;
    }
    Ok(())
}

/// Upstream `_new_paragraph`.
fn new_paragraph(
    tree: &mut XmlTree,
    scope: NodeId,
    text: &str,
    style: Option<&str>,
) -> Result<NodeId> {
    let paragraph = w_el(tree, scope, "p")?;
    if let Some(style) = style.filter(|s| !s.is_empty()) {
        set_p_style(tree, paragraph, Some(style))?;
    }
    let run = w_el(tree, paragraph, "r")?;
    add_t(tree, run, text)?;
    append(tree, paragraph, run)?;
    Ok(paragraph)
}

fn stamp_inserted(
    tree: &mut XmlTree,
    paragraphs: &[NodeId],
    next_id: &mut i64,
    stamp: Stamp<'_>,
    revision_ids: &mut Vec<i64>,
) -> Result<()> {
    for paragraph in paragraphs {
        wrap_paragraph_content_as_insertion(tree, *paragraph, *next_id, stamp)?;
        stamp_paragraph_mark(tree, *paragraph, "ins", *next_id, stamp)?;
        revision_ids.push(*next_id);
        *next_id += 1;
    }
    Ok(())
}

fn validate_texts(texts: &[String], argument: &str) -> Result<()> {
    for (index, text) in texts.iter().enumerate() {
        validate_writable_text(text, &format!("{argument}[{index}]"))?;
    }
    Ok(())
}

fn finish_styles(txn: &mut Txn<'_>, styles_missing: bool) -> Result<()> {
    if styles_missing {
        crate::parts::ensure_styles_part(txn)?;
    }
    Ok(())
}

/// Insert a heading plus body paragraphs after `anchor` (upstream
/// `insert_section_after`). With `tracked` each paragraph is a real Word
/// insertion attributed to `author`.
pub fn insert_section_after(
    document: &mut Document,
    anchor: BlockTarget<'_>,
    heading: &str,
    paragraphs: &[String],
    options: &SectionOptions<'_>,
) -> Result<BlockEditResult> {
    if options.tracked && options.author.is_none_or(str::is_empty) {
        return Err(value_error(
            "author_required",
            "author is required when tracked=True",
        ));
    }
    if options.tracked {
        validate_tracked_identity(options.author)?;
    }
    validate_writable_text(heading, "heading")?;
    validate_texts(paragraphs, "paragraphs")?;
    let mut txn = Txn::new(document);
    let (heading_style, body_style, styles_missing) =
        if options.heading_style.is_none() && options.body_style.is_none() {
            (None, None, false)
        } else {
            let styles = StylesView::from_txn(&txn)?;
            (
                validated_style_id(&styles, options.heading_style, "heading_style")?,
                validated_style_id(&styles, options.body_style, "body_style")?,
                styles.missing,
            )
        };
    let located = resolve_anchor_paragraph(document, anchor)?;
    check_insertion_anchor(document, &located)?;
    let stamp = Stamp {
        author: options.author.unwrap_or(""),
        date: options.date.unwrap_or_else(Timestamp::now),
    };
    let first_revision = if options.tracked {
        Some(next_revision_id(document, &BTreeMap::new())?)
    } else {
        None
    };
    finish_styles(&mut txn, styles_missing)?;
    let tree = txn.tree_mut(&located.part)?;
    let mut nodes = vec![new_paragraph(
        tree,
        located.element,
        heading,
        heading_style.as_deref(),
    )?];
    for text in paragraphs {
        nodes.push(new_paragraph(
            tree,
            located.element,
            text,
            body_style.as_deref(),
        )?);
    }
    let mut revision_ids = Vec::new();
    if let Some(mut next_id) = first_revision {
        stamp_inserted(tree, &nodes, &mut next_id, stamp, &mut revision_ids)?;
    }
    insert_after(tree, located.element, &nodes)?;
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(BlockEditResult {
        story: located.story,
        inserted_blocks: nodes.len(),
        deleted_blocks: 0,
        deleted_text: Vec::new(),
        revision_ids,
    })
}

pub(crate) fn check_insertion_anchor(document: &Document, located: &Located) -> Result<()> {
    let tree = document.package().xml(&located.part)?;
    refuse_cell_anchor(tree, located.element)?;
    refuse_paragraph_in_open_field(tree, &located.story, located.element, true)
}

/// Upstream `_select_paragraph_range`: (story, part, selected paragraphs).
fn select_paragraph_range(
    document: &Document,
    range: &ParagraphRange<'_>,
) -> Result<(String, String, Vec<NodeId>)> {
    if range.count < 1 {
        return Err(value_error("invalid_count", "count must be >= 1"));
    }
    require_owner(document, range.start, "start_anchor")?;
    if let Some(end) = range.end {
        require_owner(document, end, "end_anchor")?;
    }
    let start = locate_paragraph(document, range.start)?;
    let end_location = match range.end {
        Some(end) => Some(locate_paragraph(document, end)?),
        None => None,
    };
    require_current_view(range.start)?;
    if let Some(end) = range.end {
        require_current_view(end)?;
    }
    refuse_if_protected(
        document,
        "insert or remove paragraphs",
        OperationClass::BodyContent,
    )?;
    let tree = document.package().xml(&start.part)?;
    refuse_paragraph_in_open_field(tree, &start.story, start.element, false)?;
    let parent = tree
        .parent(start.element)
        .ok_or_else(|| Error::internal("range_parent", "start paragraph is detached"))?;
    let siblings: Vec<NodeId> = w_children(tree, parent, "p").collect();
    let start_index = siblings
        .iter()
        .position(|p| *p == start.element)
        .ok_or_else(|| Error::internal("range_start", "start paragraph not among siblings"))?;
    let end_index = match end_location {
        Some(end) => {
            if end.story != start.story {
                return Err(Error::refusal(
                    RefusalKind::BoundaryViolation,
                    "cross_story_range",
                    "start and end anchors live in different story parts",
                ));
            }
            if tree.parent(end.element) != Some(parent) {
                return Err(Error::refusal(
                    RefusalKind::BoundaryViolation,
                    "cross_parent_range",
                    "start and end anchors do not share one parent; edits spanning story regions, table boundaries, \
                     or content-control boundaries are refused",
                ));
            }
            let end_index = siblings
                .iter()
                .position(|p| *p == end.element)
                .ok_or_else(|| Error::internal("range_end", "end paragraph not among siblings"))?;
            if end_index < start_index {
                return Err(Error::refusal(
                    RefusalKind::TargetNotFound,
                    "end_before_start",
                    "end anchor appears before start anchor",
                ));
            }
            end_index
        }
        None => {
            let end_index = start_index + range.count as usize - 1;
            if end_index >= siblings.len() {
                return Err(Error::refusal(
                    RefusalKind::TargetNotFound,
                    "count_past_end",
                    format!(
                        "count={} extends past the last paragraph of {}",
                        range.count, start.story
                    ),
                ));
            }
            end_index
        }
    };
    Ok((
        start.story,
        start.part,
        siblings[start_index..=end_index].to_vec(),
    ))
}

fn require_author(author: &str) -> Result<()> {
    if author.is_empty() {
        return Err(value_error("author_required", "author is required"));
    }
    validate_tracked_identity(Some(author))
}

/// Mark a paragraph range deleted with real tracked changes (upstream
/// `tracked_delete_paragraphs`).
pub fn tracked_delete_paragraphs(
    document: &mut Document,
    range: ParagraphRange<'_>,
    author: &str,
    date: Option<Timestamp>,
) -> Result<BlockEditResult> {
    tracked_range_edit(document, range, None, None, author, date)
}

/// Tracked-delete a paragraph range and tracked-insert `replacement`
/// paragraphs after it (upstream `tracked_replace_paragraphs`).
pub fn tracked_replace_paragraphs(
    document: &mut Document,
    range: ParagraphRange<'_>,
    replacement: &[String],
    body_style: Option<&str>,
    author: &str,
    date: Option<Timestamp>,
) -> Result<BlockEditResult> {
    tracked_range_edit(document, range, Some(replacement), body_style, author, date)
}

fn tracked_range_edit(
    document: &mut Document,
    range: ParagraphRange<'_>,
    replacement: Option<&[String]>,
    body_style: Option<&str>,
    author: &str,
    date: Option<Timestamp>,
) -> Result<BlockEditResult> {
    require_author(author)?;
    let mut txn = Txn::new(document);
    let mut styles_missing = false;
    let mut body_style_id = None;
    if let Some(replacement) = replacement {
        validate_texts(replacement, "replacement_paragraphs")?;
        if body_style.is_some() {
            let styles = StylesView::from_txn(&txn)?;
            body_style_id = validated_style_id(&styles, body_style, "body_style")?;
            styles_missing = styles.missing;
        }
    }
    let (story, part, selected) = select_paragraph_range(document, &range)?;
    {
        let tree = document.package().xml(&part)?;
        for paragraph in &selected {
            validate_deletable_paragraph(tree, *paragraph)?;
        }
    }
    let stamp = Stamp {
        author,
        date: date.unwrap_or_else(Timestamp::now),
    };
    let mut next_id = next_revision_id(document, &BTreeMap::new())?;
    finish_styles(&mut txn, styles_missing)?;
    let tree = txn.tree_mut(&part)?;
    let mut deleted_text = Vec::new();
    let mut revision_ids = Vec::new();
    for paragraph in &selected {
        deleted_text.push(mark_paragraph_deleted(tree, *paragraph, next_id, stamp)?);
        revision_ids.push(next_id);
        next_id += 1;
    }
    let mut inserted = Vec::new();
    if let Some(replacement) = replacement {
        let last = *selected
            .last()
            .ok_or_else(|| Error::internal("range_empty", "empty selection"))?;
        for text in replacement {
            inserted.push(new_paragraph(tree, last, text, body_style_id.as_deref())?);
        }
        stamp_inserted(tree, &inserted, &mut next_id, stamp, &mut revision_ids)?;
        insert_after(tree, last, &inserted)?;
    }
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(BlockEditResult {
        story,
        inserted_blocks: inserted.len(),
        deleted_blocks: selected.len(),
        deleted_text,
        revision_ids,
    })
}

/// Upstream `_validate_rich_blocks`.
fn validate_rich_blocks(
    styles: Option<&StylesView<'_>>,
    blocks: &[RichBlock],
    tracked: bool,
) -> Result<()> {
    if blocks.is_empty() {
        return Err(value_error("empty_blocks", "blocks must not be empty"));
    }
    for (position, block) in blocks.iter().enumerate() {
        let label = format!("blocks[{position}]");
        match block {
            RichBlock::Paragraph(rich) => {
                if rich.runs.is_empty() {
                    return Err(value_error(
                        "empty_runs",
                        format!("{label}: RichParagraph needs at least one run"),
                    ));
                }
                for run in &rich.runs {
                    validate_writable_text(&run.text, &format!("{label} run text"))?;
                }
                if let Some(style) = rich.style.as_deref() {
                    let view = styles
                        .ok_or_else(|| Error::internal("styles_view", "styles view not loaded"))?;
                    validated_style_id(view, Some(style), &format!("{label}.style"))?;
                }
            }
            RichBlock::List(list) => {
                if !matches!(list.kind.as_str(), "bullet" | "decimal") {
                    return Err(value_error(
                        "invalid_list_kind",
                        format!("{label}: kind must be 'bullet' or 'decimal'"),
                    ));
                }
                if !(0..=2).contains(&list.level) {
                    return Err(value_error(
                        "invalid_list_level",
                        format!("{label}: level must be 0..2"),
                    ));
                }
                if list.items.is_empty() {
                    return Err(value_error(
                        "empty_list",
                        format!("{label}: ListBlock needs at least one item"),
                    ));
                }
                for item in &list.items {
                    validate_writable_text(item, &format!("{label} item"))?;
                }
            }
            RichBlock::Table(table) => {
                if tracked {
                    return Err(Error::unsupported(
                        "tracked_table",
                        "tracked table insertion is not supported (Word marks table revisions row-by-row; that \
                         vocabulary is a later phase) — insert the table untracked or as text",
                    ));
                }
                if table.rows.is_empty() || table.rows.iter().any(Vec::is_empty) {
                    return Err(value_error(
                        "empty_table",
                        format!("{label}: TableBlock rows must be non-empty"),
                    ));
                }
                let width = table.rows[0].len();
                if table.rows.iter().any(|row| row.len() != width) {
                    return Err(value_error(
                        "non_rectangular_table",
                        format!("{label}: TableBlock rows must be rectangular"),
                    ));
                }
                for row in &table.rows {
                    for cell in row {
                        validate_writable_text(cell, &format!("{label} cell"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// python-docx `ST_TwipsMeasure` → EMU.
fn twips_measure_emu(value: &str) -> Result<i64> {
    if value.contains(['i', 'm', 'p']) {
        let split = value.len().saturating_sub(2);
        let (quantity, units) = (
            value.get(..split).unwrap_or(""),
            value.get(split..).unwrap_or(""),
        );
        let multiplier = match units {
            "mm" => 36_000.0,
            "cm" => 360_000.0,
            "in" => 914_400.0,
            "pt" => 12_700.0,
            "pc" | "pi" => 152_400.0,
            _ => {
                return Err(value_error(
                    "malformed_measure",
                    format!("unknown measurement unit in {value:?}"),
                ));
            }
        };
        let quantity: f64 = quantity
            .trim_matches(crate::unicode::is_py_space)
            .parse()
            .map_err(|_| {
                value_error(
                    "malformed_measure",
                    format!("could not convert {value:?} to float"),
                )
            })?;
        return Ok(py_round(quantity * multiplier));
    }
    let twips = py_int(value).ok_or_else(|| {
        value_error(
            "malformed_measure",
            format!("invalid twips measure {value:?}"),
        )
    })?;
    Ok(twips * 635)
}

/// Upstream `_new_table` width rule: last section's page width minus its
/// left and right margins, else 6 inches.
fn section_text_width(document: &Document) -> Result<i64> {
    const SIX_INCHES: i64 = 5_486_400;
    let package = document.package();
    let tree = package.xml(package.main_part_name())?;
    let root = tree.root();
    let mut sect_prs = Vec::new();
    for body in w_children(tree, root, "body") {
        for child in tree.child_elements(body) {
            if is_w(tree, child, "p") {
                for ppr in w_children(tree, child, "pPr") {
                    sect_prs.extend(w_children(tree, ppr, "sectPr"));
                }
            } else if is_w(tree, child, "sectPr") {
                sect_prs.push(child);
            }
        }
    }
    let Some(sect_pr) = sect_prs.last().copied() else {
        return Ok(SIX_INCHES);
    };
    let attr = |child: &str, name: &str| {
        w_child(tree, sect_pr, child).and_then(|element| w_attr(tree, element, name))
    };
    let (Some(width), Some(left), Some(right)) = (
        attr("pgSz", "w"),
        attr("pgMar", "left"),
        attr("pgMar", "right"),
    ) else {
        // Upstream converts in order and stops at the first missing value.
        for value in [
            attr("pgSz", "w"),
            attr("pgMar", "left"),
            attr("pgMar", "right"),
        ]
        .into_iter()
        .flatten()
        {
            twips_measure_emu(value)?;
        }
        return Ok(SIX_INCHES);
    };
    Ok(twips_measure_emu(width)? - twips_measure_emu(left)? - twips_measure_emu(right)?)
}

/// Upstream `_new_rich_paragraph`.
fn new_rich_paragraph(tree: &mut XmlTree, scope: NodeId, rich: &RichParagraph) -> Result<NodeId> {
    let paragraph = w_el(tree, scope, "p")?;
    if let Some(style) = rich.style.as_deref().filter(|s| !s.is_empty()) {
        set_p_style(tree, paragraph, Some(style))?;
    }
    for spec in &rich.runs {
        let run = w_el(tree, paragraph, "r")?;
        if spec.bold || spec.italic {
            let rpr = w_el(tree, run, "rPr")?;
            if spec.bold {
                let b = w_el(tree, rpr, "b")?;
                append(tree, rpr, b)?;
            }
            if spec.italic {
                let i = w_el(tree, rpr, "i")?;
                append(tree, rpr, i)?;
            }
            append(tree, run, rpr)?;
        }
        add_t(tree, run, &spec.text)?;
        append(tree, paragraph, run)?;
    }
    Ok(paragraph)
}

/// Insert a typed block list after `anchor` (upstream
/// `insert_blocks_after`): rich paragraphs, real lists (canonical numbering
/// created on demand) and rectangular tables.
pub fn insert_blocks_after(
    document: &mut Document,
    anchor: BlockTarget<'_>,
    blocks: &[RichBlock],
    tracked: bool,
    author: Option<&str>,
    date: Option<Timestamp>,
) -> Result<BlockEditResult> {
    if tracked && author.is_none_or(str::is_empty) {
        return Err(value_error(
            "author_required",
            "author is required when tracked=True",
        ));
    }
    if tracked {
        validate_tracked_identity(author)?;
    }
    let mut txn = Txn::new(document);
    let needs_styles = blocks
        .iter()
        .any(|block| matches!(block, RichBlock::Paragraph(p) if p.style.is_some()));
    let styles_missing = {
        let mut styles = None;
        if needs_styles && !blocks.is_empty() {
            // Upstream touches `document.styles` only when validating a style.
            styles = Some(StylesView::from_txn(&txn)?);
        }
        validate_rich_blocks(styles.as_ref(), blocks, tracked)?;
        styles.is_some_and(|view| view.missing)
    };
    let located = resolve_anchor_paragraph(document, anchor)?;
    check_insertion_anchor(document, &located)?;
    let stamp = Stamp {
        author: author.unwrap_or(""),
        date: date.unwrap_or_else(Timestamp::now),
    };
    let width = if blocks.iter().any(|b| matches!(b, RichBlock::Table(_))) {
        section_text_width(document)?
    } else {
        0
    };
    let first_revision = if tracked {
        Some(next_revision_id(document, &BTreeMap::new())?)
    } else {
        None
    };
    finish_styles(&mut txn, styles_missing)?;
    // Numbering definitions first (they live in another part).
    let mut list_num_ids = Vec::new();
    for block in blocks {
        if let RichBlock::List(list) = block {
            let kind = if list.kind == "bullet" {
                ListKind::Bullet
            } else {
                ListKind::Decimal
            };
            list_num_ids.push(ensure_definition_in(&mut txn, kind)?);
        }
    }
    let tree = txn.tree_mut(&located.part)?;
    let scope = located.element;
    let mut nodes = Vec::new();
    let mut paragraph_nodes = Vec::new();
    let mut lists = list_num_ids.into_iter();
    for block in blocks {
        match block {
            RichBlock::Paragraph(rich) => {
                let node = new_rich_paragraph(tree, scope, rich)?;
                nodes.push(node);
                paragraph_nodes.push(node);
            }
            RichBlock::List(list) => {
                let num_id = lists
                    .next()
                    .ok_or_else(|| Error::internal("list_numbering", "missing list numId"))?;
                for item in &list.items {
                    let paragraph = new_paragraph(tree, scope, item, None)?;
                    let ppr = get_or_add_ppr(tree, paragraph)?;
                    let num_pr = w_el(tree, ppr, "numPr")?;
                    let ilvl = w_el(tree, num_pr, "ilvl")?;
                    set_w(tree, ilvl, "val", &list.level.to_string())?;
                    append(tree, num_pr, ilvl)?;
                    let num = w_el(tree, num_pr, "numId")?;
                    set_w(tree, num, "val", &num_id.to_string())?;
                    append(tree, num_pr, num)?;
                    append(tree, ppr, num_pr)?;
                    nodes.push(paragraph);
                    paragraph_nodes.push(paragraph);
                }
            }
            RichBlock::Table(table) => {
                nodes.push(oxml::new_table(tree, scope, &table.rows, width)?);
            }
        }
    }
    // Word fuses adjacent sibling tables and refuses a story ending in a
    // table: pad with an empty paragraph where needed.
    let following_is_table_or_end = {
        let parent = tree.parent(located.element);
        let next = parent.and_then(|parent| {
            let children = tree.children(parent);
            let index = tree.index_in_parent(located.element)?;
            children[index + 1..]
                .iter()
                .copied()
                .find(|c| !matches!(tree.data(*c), NodeData::Text(_)))
        });
        next.is_none_or(|node| is_w(tree, node, "tbl"))
    };
    let mut padded = Vec::new();
    for (position, node) in nodes.iter().enumerate() {
        padded.push(*node);
        if !is_w(tree, *node, "tbl") {
            continue;
        }
        let pad = match nodes.get(position + 1) {
            Some(next) => is_w(tree, *next, "tbl"),
            None => following_is_table_or_end,
        };
        if pad {
            padded.push(w_el(tree, scope, "p")?);
        }
    }
    let mut revision_ids = Vec::new();
    if let Some(mut next_id) = first_revision {
        stamp_inserted(
            tree,
            &paragraph_nodes,
            &mut next_id,
            stamp,
            &mut revision_ids,
        )?;
    }
    insert_after(tree, located.element, &padded)?;
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(BlockEditResult {
        story: located.story,
        inserted_blocks: padded.len(),
        deleted_blocks: 0,
        deleted_text: Vec::new(),
        revision_ids,
    })
}

impl Document {
    /// See [`insert_section_after`].
    pub fn insert_section_after(
        &mut self,
        anchor: BlockTarget<'_>,
        heading: &str,
        paragraphs: &[String],
        options: &SectionOptions<'_>,
    ) -> Result<BlockEditResult> {
        insert_section_after(self, anchor, heading, paragraphs, options)
    }

    /// See [`insert_blocks_after`].
    pub fn insert_blocks_after(
        &mut self,
        anchor: BlockTarget<'_>,
        blocks: &[RichBlock],
        tracked: bool,
        author: Option<&str>,
        date: Option<Timestamp>,
    ) -> Result<BlockEditResult> {
        insert_blocks_after(self, anchor, blocks, tracked, author, date)
    }

    /// See [`tracked_delete_paragraphs`].
    pub fn tracked_delete_paragraphs(
        &mut self,
        range: ParagraphRange<'_>,
        author: &str,
        date: Option<Timestamp>,
    ) -> Result<BlockEditResult> {
        tracked_delete_paragraphs(self, range, author, date)
    }

    /// See [`tracked_replace_paragraphs`].
    pub fn tracked_replace_paragraphs(
        &mut self,
        range: ParagraphRange<'_>,
        replacement: &[String],
        body_style: Option<&str>,
        author: &str,
        date: Option<Timestamp>,
    ) -> Result<BlockEditResult> {
        tracked_replace_paragraphs(self, range, replacement, body_style, author, date)
    }
}
