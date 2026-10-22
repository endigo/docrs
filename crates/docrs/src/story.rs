//! Visibility-complete story traversal and inspection (upstream `story.py`).
//!
//! * Every story part is walked: body, headers, footers, footnotes, endnotes,
//!   comments. Separator and continuation-separator notes are skipped.
//! * Paragraphs inside table cells belong to the table block; paragraphs in
//!   content controls and text boxes are emitted as their own (flagged)
//!   blocks.
//! * `mc:AlternateContent` contributes exactly one branch (see
//!   [`crate::wml::first_choice_children`]).
//! * Empty paragraphs are emitted; block indices are stable.
//!
//! Walkers are iterative so deep (trusted-profile) documents cannot exhaust
//! the call stack.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::normalize::content_hash;
use crate::unicode::casefold;
use crate::wml::{self, W, first_choice_children, is_w, leading_text, w_attr, w_child, w_children};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// Text layer selected for inspection and search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// Insertions in, deletions out (the document if all changes were accepted).
    Current,
    /// Deletions in, insertions out.
    Original,
    /// Everything.
    All,
}

impl View {
    /// Upstream token.
    pub fn as_str(self) -> &'static str {
        match self {
            View::Current => "current",
            View::Original => "original",
            View::All => "all",
        }
    }

    /// Parse an upstream token.
    pub fn parse(token: &str) -> Result<View> {
        match token {
            "current" => Ok(View::Current),
            "original" => Ok(View::Original),
            "all" => Ok(View::All),
            _ => Err(Error::invalid_argument(
                "invalid_view",
                format!("view must be one of ('current', 'original', 'all'), got {token:?}"),
            )),
        }
    }
}

/// A story part: its (casefolded) story name and the package member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    /// Upstream story name: the casefolded part name.
    pub name: String,
    /// Package member holding the story.
    pub part: String,
}

fn story_rank(name: &str) -> u8 {
    if name == "word/document.xml" {
        0
    } else if name.starts_with("word/header") {
        1
    } else if name.starts_with("word/footer") {
        2
    } else if name == "word/footnotes.xml" {
        3
    } else if name == "word/endnotes.xml" {
        4
    } else {
        5
    }
}

/// Every story part, in traversal order (body, headers, footers, footnotes,
/// endnotes, comments).
pub fn stories(document: &Document) -> Vec<Story> {
    let mut found: Vec<Story> = document
        .package()
        .parts()
        .filter(|part| {
            part.is_reachable()
                && part
                    .content_type()
                    .is_some_and(|ct| crate::package::ct::EAGER_XML.contains(&ct))
        })
        .filter_map(|part| {
            let name = casefold(part.name());
            let is_story = name == "word/document.xml"
                || ((name.starts_with("word/header") || name.starts_with("word/footer"))
                    && name.ends_with(".xml"))
                || matches!(
                    name.as_str(),
                    "word/footnotes.xml" | "word/endnotes.xml" | "word/comments.xml"
                );
            is_story.then(|| Story {
                name,
                part: part.name().to_string(),
            })
        })
        .collect();
    found.sort_by(|a, b| (story_rank(&a.name), &a.name).cmp(&(story_rank(&b.name), &b.name)));
    found
}

/// Inert location evidence (story + block index + content hash).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Anchor {
    /// Story name.
    pub story: String,
    /// Block index within the story.
    pub index: usize,
    /// [`content_hash`] of the located text.
    pub content_hash: String,
}

/// Shape summary of a table block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableShape {
    /// Direct `w:tr` rows.
    pub rows: usize,
    /// Most direct `w:tc` cells in a row.
    pub columns: usize,
    /// Any `w:vMerge`/`w:gridSpan` descendant.
    pub has_merges: bool,
    /// Any nested table.
    pub has_nested_table: bool,
}

/// Block kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// `w:p`.
    Paragraph,
    /// `w:tbl`.
    Table,
}

/// One paragraph or table observed during traversal.
#[derive(Debug, Clone)]
pub struct Block {
    /// Story name.
    pub story: String,
    /// Kind.
    pub kind: BlockKind,
    /// Index within the story.
    pub index: usize,
    /// Location evidence.
    pub anchor: Anchor,
    /// View-filtered text.
    pub text: String,
    /// `w:pPr/w:pStyle/@w:val`.
    pub style_id: Option<String>,
    /// Text from pending insertions contributed.
    pub in_insert: bool,
    /// Text from pending deletions contributed.
    pub in_delete: bool,
    /// Inside (or containing) a content control.
    pub in_content_control: bool,
    /// Inside (or containing) a text box.
    pub in_text_box: bool,
    /// Contains, or lies inside, a field.
    pub has_field: bool,
    /// Table shape for table blocks.
    pub table: Option<TableShape>,
    pub(crate) element: NodeId,
    pub(crate) handle: BlockHandle,
}

/// Private freshness evidence carried by a live [`Block`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockHandle {
    pub document: Option<crate::DocumentId>,
    pub part: String,
    pub view: View,
    pub root: NodeId,
    pub parent: Option<NodeId>,
    pub containers: Vec<NodeId>,
    pub struct_gen: u32,
    pub incarnation: u32,
}

impl Block {
    /// View the block was captured under.
    pub fn view(&self) -> View {
        self.handle.view
    }

    /// Package member holding the block.
    pub fn part(&self) -> &str {
        &self.handle.part
    }
}

/// Ancestors of `element` from its parent up to and including `root`
/// (upstream `_container_elements`).
pub(crate) fn container_elements(tree: &XmlTree, element: NodeId) -> Vec<NodeId> {
    let root = tree.root();
    let mut out = Vec::new();
    for ancestor in tree.ancestors(element) {
        out.push(ancestor);
        if ancestor == root {
            return out;
        }
    }
    out
}

impl Block {
    /// Upstream `Block.to_dict()`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "story": self.story,
            "kind": self.kind,
            "index": self.index,
            "anchor": self.anchor,
            "anchor_role": "legacy_inert_location_evidence",
            "text": self.text,
            "style_id": self.style_id,
            "in_insert": self.in_insert,
            "in_delete": self.in_delete,
            "in_content_control": self.in_content_control,
            "in_text_box": self.in_text_box,
            "has_field": self.has_field,
            "table": self.table,
        })
    }
}

/// Keys reported by [`Outline::blind_region_counts`], in payload order.
pub const BLIND_REGION_KEYS: &[&str] = &[
    "tracked_insertions",
    "tracked_deletions",
    "moves",
    "format_changes",
    "content_controls",
    "text_boxes",
    "fields",
    "math",
    "embedded_objects",
    "alt_chunks",
    "hidden_text",
];

/// Inspection snapshot: story parts, all blocks, blind-region counts.
#[derive(Debug, Clone)]
pub struct Outline {
    /// Story names in traversal order.
    pub story_parts: Vec<String>,
    /// Every block of every story.
    pub blocks: Vec<Block>,
    /// Regions the traversal flags or cannot read.
    pub blind_region_counts: BTreeMap<String, usize>,
}

impl Outline {
    /// Upstream `Outline.to_dict()` (`schema: paper_outline`, `version: 3`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_outline",
            "version": 3,
            "story_parts": self.story_parts,
            "blind_region_counts": self.blind_region_counts,
            "blocks": self.blocks.iter().map(Block::to_json).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Text projection of one run child (upstream `_textatoms.py`)
// ---------------------------------------------------------------------------

/// Object-replacement character separating search text at unmodeled
/// visible run content.
pub const BARRIER_TEXT: &str = "\u{FFFC}";

/// Plain-text contribution of one direct `w:r` child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// Contributed text.
    pub text: String,
    /// Visible but unmodeled.
    pub barrier: bool,
}

/// Upstream `project_run_child`.
pub fn project_run_child(tree: &XmlTree, id: NodeId) -> Projection {
    let plain = |text: String| Projection {
        text,
        barrier: false,
    };
    let barrier = || Projection {
        text: BARRIER_TEXT.to_string(),
        barrier: true,
    };
    if tree.element(id).is_none() || tree.ns_uri(id) != Some(W) {
        return barrier();
    }
    match tree.local_name(id).unwrap_or("") {
        "t" | "delText" | "instrText" => plain(leading_text(tree, id)),
        "tab" | "ptab" => plain("\t".into()),
        "cr" => plain("\n".into()),
        "br" => {
            if w_attr(tree, id, "type").unwrap_or("textWrapping") == "textWrapping" {
                plain("\n".into())
            } else {
                barrier()
            }
        }
        "noBreakHyphen" => plain("-".into()),
        "rPr" | "fldChar" | "lastRenderedPageBreak" => plain(String::new()),
        _ => barrier(),
    }
}

/// Whether `id`'s actual parent is a `w:r`.
pub fn is_direct_run_child(tree: &XmlTree, id: NodeId) -> bool {
    tree.parent(id)
        .is_some_and(|parent| is_w(tree, parent, "r"))
}

// ---------------------------------------------------------------------------
// View-filtered text visitor
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct TextVisitor {
    pub(crate) text: String,
    in_insert: bool,
    in_delete: bool,
    in_content_control: bool,
    in_text_box: bool,
    has_field: bool,
}

#[derive(Clone, Copy)]
struct VisitFlags {
    ins: bool,
    del: bool,
    sdt: bool,
    txbx: bool,
}

pub(crate) fn subtree_text(
    tree: &XmlTree,
    element: NodeId,
    view: View,
    skip_text_boxes: bool,
    in_sdt: bool,
    in_txbx: bool,
) -> TextVisitor {
    let mut visitor = TextVisitor::default();
    let start = VisitFlags {
        ins: false,
        del: false,
        sdt: in_sdt,
        txbx: in_txbx,
    };
    let mut stack: Vec<(NodeId, VisitFlags)> = first_choice_children(tree, element)
        .into_iter()
        .rev()
        .map(|child| (child, start))
        .collect();
    while let Some((node, mut flags)) = stack.pop() {
        let is_w_ns = tree.ns_uri(node) == Some(W);
        let local = if is_w_ns {
            tree.local_name(node).unwrap_or("")
        } else {
            ""
        };
        if local == "txbxContent" && skip_text_boxes {
            continue;
        }
        match local {
            "ins" | "moveTo" => flags.ins = true,
            "del" | "moveFrom" => flags.del = true,
            "sdt" => flags.sdt = true,
            "txbxContent" => flags.txbx = true,
            "fldSimple" => visitor.has_field = true,
            "fldChar" if w_attr(tree, node, "fldCharType") == Some("begin") => {
                visitor.has_field = true
            }
            _ => {}
        }
        if local == "t" {
            if view == View::Current && flags.del {
                continue;
            }
            if view == View::Original && flags.ins {
                continue;
            }
            emit(
                &mut visitor,
                &leading_text(tree, node),
                flags.ins,
                flags.del,
                flags.sdt,
                flags.txbx,
            );
            continue;
        }
        if local == "delText" {
            if view == View::Current {
                continue;
            }
            if view == View::Original && flags.ins {
                continue;
            }
            emit(
                &mut visitor,
                &leading_text(tree, node),
                flags.ins,
                true,
                flags.sdt,
                flags.txbx,
            );
            continue;
        }
        if local == "instrText" {
            continue;
        }
        if is_direct_run_child(tree, node) {
            let projection = project_run_child(tree, node);
            if projection.barrier || projection.text.is_empty() {
                continue;
            }
            if view == View::Current && flags.del {
                continue;
            }
            if view == View::Original && flags.ins {
                continue;
            }
            emit(
                &mut visitor,
                &projection.text,
                flags.ins,
                flags.del,
                flags.sdt,
                flags.txbx,
            );
            continue;
        }
        if tree.element(node).is_some() {
            for child in first_choice_children(tree, node).into_iter().rev() {
                stack.push((child, flags));
            }
        }
    }
    visitor
}

fn emit(visitor: &mut TextVisitor, text: &str, ins: bool, del: bool, sdt: bool, txbx: bool) {
    visitor.text.push_str(text);
    visitor.in_insert |= ins;
    visitor.in_delete |= del;
    visitor.in_content_control |= sdt;
    visitor.in_text_box |= txbx;
}

// ---------------------------------------------------------------------------
// Block iteration
// ---------------------------------------------------------------------------

/// One block element found by the shared walker.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockElement {
    pub kind: BlockKind,
    pub index: usize,
    pub element: NodeId,
    pub in_sdt: bool,
    pub in_txbx: bool,
}

fn block_containers(tree: &XmlTree, story: &str) -> Result<Vec<NodeId>> {
    let root = tree.root();
    let root_local = if tree.ns_uri(root) == Some(W) {
        tree.local_name(root).unwrap_or("")
    } else {
        ""
    };
    Ok(match root_local {
        "document" => w_child(tree, root, "body").into_iter().collect(),
        "hdr" | "ftr" => vec![root],
        "footnotes" | "endnotes" => tree
            .child_elements(root)
            .filter(|note| is_w(tree, *note, "footnote") || is_w(tree, *note, "endnote"))
            .filter(|note| {
                !matches!(
                    w_attr(tree, *note, "type"),
                    Some("separator" | "continuationSeparator")
                )
            })
            .collect(),
        "comments" => w_children(tree, root, "comment").collect(),
        _ => {
            return Err(Error::refusal(
                RefusalKind::UnsupportedStructure,
                "unrecognized_story_root",
                format!(
                    "unrecognized story root {:?} in {story}",
                    tree.clark(root).unwrap_or_default()
                ),
            ));
        }
    })
}

fn text_box_contents(tree: &XmlTree, paragraph: NodeId) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut stack: Vec<NodeId> = first_choice_children(tree, paragraph)
        .into_iter()
        .rev()
        .collect();
    while let Some(node) = stack.pop() {
        if is_w(tree, node, "txbxContent") {
            found.push(node);
        } else if tree.element(node).is_some() {
            stack.extend(first_choice_children(tree, node).into_iter().rev());
        }
    }
    found
}

/// Upstream `_iter_block_elements`: the single definition of block identity.
pub(crate) fn iter_block_elements(tree: &XmlTree, story: &str) -> Result<Vec<BlockElement>> {
    let mut out = Vec::new();
    let mut counter = 0usize;
    for container in block_containers(tree, story)? {
        // Work list of (children, position, in_sdt, in_txbx) frames.
        let mut frames: Vec<(Vec<NodeId>, usize, bool, bool)> =
            vec![(first_choice_children(tree, container), 0, false, false)];
        while let Some(frame) = frames.last_mut() {
            if frame.1 >= frame.0.len() {
                frames.pop();
                continue;
            }
            let child = frame.0[frame.1];
            frame.1 += 1;
            let (in_sdt, in_txbx) = (frame.2, frame.3);
            if is_w(tree, child, "p") {
                out.push(BlockElement {
                    kind: BlockKind::Paragraph,
                    index: counter,
                    element: child,
                    in_sdt,
                    in_txbx,
                });
                counter += 1;
                let boxes = text_box_contents(tree, child);
                // Text boxes are walked depth-first right after their paragraph.
                for txbx in boxes.into_iter().rev() {
                    frames.push((first_choice_children(tree, txbx), 0, in_sdt, true));
                }
            } else if is_w(tree, child, "tbl") {
                out.push(BlockElement {
                    kind: BlockKind::Table,
                    index: counter,
                    element: child,
                    in_sdt,
                    in_txbx,
                });
                counter += 1;
            } else if is_w(tree, child, "sdt")
                && let Some(content) = w_child(tree, child, "sdtContent")
            {
                frames.push((first_choice_children(tree, content), 0, true, in_txbx));
            }
        }
    }
    Ok(out)
}

pub(crate) fn count_fldchar_delta(tree: &XmlTree, element: NodeId) -> i64 {
    let mut delta = 0i64;
    let mut stack = vec![element];
    while let Some(node) = stack.pop() {
        for child in first_choice_children(tree, node) {
            if is_w(tree, child, "fldChar") {
                match w_attr(tree, child, "fldCharType") {
                    Some("begin") => delta += 1,
                    Some("end") => delta -= 1,
                    _ => {}
                }
            }
            if tree.element(child).is_some() {
                stack.push(child);
            }
        }
    }
    delta
}

fn table_shape(tree: &XmlTree, table: NodeId) -> TableShape {
    let rows: Vec<NodeId> = w_children(tree, table, "tr").collect();
    let columns = rows
        .iter()
        .map(|row| w_children(tree, *row, "tc").count())
        .max()
        .unwrap_or(0);
    let mut has_merges = false;
    let mut has_nested = false;
    for node in tree.descendants(table) {
        if node == table {
            continue;
        }
        if is_w(tree, node, "vMerge") || is_w(tree, node, "gridSpan") {
            has_merges = true;
        }
        if is_w(tree, node, "tbl") {
            has_nested = true;
        }
    }
    TableShape {
        rows: rows.len(),
        columns,
        has_merges,
        has_nested_table: has_nested,
    }
}

pub(crate) fn table_text(tree: &XmlTree, table: NodeId, view: View) -> String {
    let mut pieces = Vec::new();
    for row in w_children(tree, table, "tr") {
        for cell in w_children(tree, row, "tc") {
            pieces.push(subtree_text(tree, cell, view, false, false, false).text);
        }
    }
    pieces.join("\n")
}

fn style_id(tree: &XmlTree, paragraph: NodeId) -> Option<String> {
    for ppr in w_children(tree, paragraph, "pPr") {
        for style in w_children(tree, ppr, "pStyle") {
            if let Some(value) = w_attr(tree, style, "val") {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Blocks of one story.
pub(crate) fn story_blocks(
    tree: &XmlTree,
    story: &str,
    part: &str,
    document: Option<crate::DocumentId>,
    view: View,
) -> Result<Vec<Block>> {
    let mut blocks = Vec::new();
    let mut open_depth: i64 = 0;
    for block in iter_block_elements(tree, story)? {
        let (text, style, table, visitor) = match block.kind {
            BlockKind::Table => {
                let visitor = subtree_text(
                    tree,
                    block.element,
                    view,
                    false,
                    block.in_sdt,
                    block.in_txbx,
                );
                (
                    table_text(tree, block.element, view),
                    None,
                    Some(table_shape(tree, block.element)),
                    visitor,
                )
            }
            BlockKind::Paragraph => {
                let visitor =
                    subtree_text(tree, block.element, view, true, block.in_sdt, block.in_txbx);
                (
                    visitor.text.clone(),
                    style_id(tree, block.element),
                    None,
                    visitor,
                )
            }
        };
        let in_open_field = open_depth > 0;
        open_depth = (open_depth + count_fldchar_delta(tree, block.element)).max(0);
        blocks.push(Block {
            story: story.to_string(),
            kind: block.kind,
            index: block.index,
            anchor: Anchor {
                story: story.to_string(),
                index: block.index,
                content_hash: content_hash(&text),
            },
            text,
            style_id: style,
            in_insert: visitor.in_insert,
            in_delete: visitor.in_delete,
            in_content_control: block.in_sdt || visitor.in_content_control,
            in_text_box: block.in_txbx || visitor.in_text_box,
            has_field: visitor.has_field || in_open_field,
            table,
            element: block.element,
            handle: BlockHandle {
                document,
                part: part.to_string(),
                view,
                root: tree.root(),
                parent: tree.parent(block.element),
                containers: container_elements(tree, block.element),
                struct_gen: tree.struct_gen(block.element),
                incarnation: tree.incarnation(),
            },
        });
    }
    Ok(blocks)
}

fn count_blind_regions(tree: &XmlTree, counts: &mut BTreeMap<String, usize>) {
    const SURFACED_GRAPHIC_URIS: &[&str] = &[
        "http://schemas.openxmlformats.org/drawingml/2006/picture",
        "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
    ];
    const FORMAT_CHANGES: &[&str] = &[
        "rPrChange",
        "pPrChange",
        "tblPrChange",
        "tcPrChange",
        "trPrChange",
        "sectPrChange",
        "numberingChange",
        "cellIns",
        "cellDel",
        "cellMerge",
    ];
    let mut bump = |key: &str| *counts.entry(key.to_string()).or_default() += 1;
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        let children = first_choice_children(tree, node);
        for child in &children {
            let child = *child;
            if !matches!(tree.data(child), NodeData::Element(_)) {
                continue;
            }
            let ns = tree.ns_uri(child);
            let local = tree.local_name(child).unwrap_or("");
            let key = match (ns, local) {
                (Some(W), "ins") => Some("tracked_insertions"),
                (Some(W), "del") => Some("tracked_deletions"),
                (Some(W), "moveFrom" | "moveTo") => Some("moves"),
                (Some(W), "sdt") => Some("content_controls"),
                (Some(W), "txbxContent") => Some("text_boxes"),
                (Some(W), "fldSimple") => Some("fields"),
                (Some(wml::M), "oMath" | "oMathPara") => Some("math"),
                (Some(W), "object") => Some("embedded_objects"),
                (Some(W), "altChunk") => Some("alt_chunks"),
                (Some(W), "vanish") => Some("hidden_text"),
                _ => None,
            };
            if let Some(key) = key {
                bump(key);
            } else if ns == Some(W) && FORMAT_CHANGES.contains(&local) {
                bump("format_changes");
            } else if ns == Some(W)
                && local == "fldChar"
                && w_attr(tree, child, "fldCharType") == Some("begin")
            {
                bump("fields");
            } else if ns == Some(wml::A)
                && local == "graphicData"
                && !tree
                    .attr(child, None, "uri")
                    .is_some_and(|uri| SURFACED_GRAPHIC_URIS.contains(&uri))
            {
                bump("embedded_objects");
            }
        }
        for child in children.into_iter().rev() {
            if tree.element(child).is_some() {
                stack.push(child);
            }
        }
    }
}

/// Every block in every story part, in document order.
pub fn iter_blocks(document: &Document, view: View) -> Result<Vec<Block>> {
    let mut blocks = Vec::new();
    for story in stories(document) {
        let tree = document.package().xml(&story.part)?;
        blocks.extend(story_blocks(
            tree,
            &story.name,
            &story.part,
            Some(document.id()),
            view,
        )?);
    }
    Ok(blocks)
}

/// Inspection snapshot (deterministic).
pub fn outline(document: &Document, view: View) -> Result<Outline> {
    let blocks = iter_blocks(document, view)?;
    let mut counts: BTreeMap<String, usize> = BLIND_REGION_KEYS
        .iter()
        .map(|key| (key.to_string(), 0))
        .collect();
    for story in stories(document) {
        let tree = document.package().xml(&story.part)?;
        count_blind_regions(tree, &mut counts);
    }
    Ok(Outline {
        story_parts: stories(document)
            .into_iter()
            .map(|story| story.name)
            .collect(),
        blocks,
        blind_region_counts: counts,
    })
}
