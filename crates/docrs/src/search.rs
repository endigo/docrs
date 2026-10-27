//! Exact and normalized text search over visibility-complete traversal
//! (upstream `search.py`, read side).
//!
//! Search assembles text across fragmented runs and paragraph boundaries
//! (each boundary is one `"\n"`). A [`Span`] maps a visible-text interval
//! back to the concrete text atoms that hold it and carries *local freshness
//! evidence*: the atoms' node identities and structural generations, their
//! edit-sensitive context (run, paragraph, revision/control/hyperlink/field
//! scopes and attributes) and the atom sequence of the captured interval.
//! Offsets are Unicode scalar positions, matching Python string indexing.
//!
//! Unknown visible run content becomes a barrier (`U+FFFC`) so a match can
//! never silently jump across an unmodeled object.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::document::DocumentId;
use crate::normalize::{content_hash, normalized_with_map, source_aligned};
use crate::story::{self, Anchor, BlockKind, View, is_direct_run_child, project_run_child};
use crate::unicode::ScalarIndex;
use crate::wml::{W, first_choice_children, is_w, leading_text, w_attr, w_child};
use crate::xml::{NodeData, NodeId, XmlTree, serialize_node};
use crate::{Document, Error, Limits, RefusalKind, Result};

/// Match policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchPolicy {
    /// Literal text (default).
    Exact,
    /// Folded case, typography and whitespace.
    Normalized,
}

impl MatchPolicy {
    /// Upstream token.
    pub fn as_str(self) -> &'static str {
        match self {
            MatchPolicy::Exact => "exact",
            MatchPolicy::Normalized => "normalized",
        }
    }

    /// Parse an upstream token.
    pub fn parse(token: &str) -> Result<MatchPolicy> {
        match token {
            "exact" => Ok(MatchPolicy::Exact),
            "normalized" => Ok(MatchPolicy::Normalized),
            _ => Err(Error::invalid_argument(
                "invalid_match_policy",
                format!("match must be one of ('exact', 'normalized'), got {token:?}"),
            )),
        }
    }
}

/// Search options (upstream `find_text` keywords).
#[derive(Debug, Clone)]
pub struct FindOptions {
    /// Match policy (default exact).
    pub policy: MatchPolicy,
    /// Text layer (default current).
    pub view: View,
    /// Restrict to one story name.
    pub story: Option<String>,
    /// 1-based selection by document position (exclusive with `near`).
    pub nth: Option<usize>,
    /// Rank matches by distance to this context text.
    pub near: Option<String>,
}

impl Default for FindOptions {
    fn default() -> Self {
        FindOptions {
            policy: MatchPolicy::Exact,
            view: View::Current,
            story: None,
            nth: None,
            near: None,
        }
    }
}

/// One text node in search space with its structural context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Atom {
    pub element: NodeId,
    /// Clark tag of the run child (or `#comment` / `#pi`).
    pub tag: String,
    pub block_index: usize,
    pub paragraph: Option<NodeId>,
    pub run: Option<NodeId>,
    pub sdt: Option<NodeId>,
    pub in_insert: bool,
    pub in_delete: bool,
    pub in_text_box: bool,
    pub hyperlink: Option<NodeId>,
    pub in_field: bool,
    pub fixed_text: Option<String>,
    pub barrier: bool,
}

impl Atom {
    pub fn is_synthetic(&self) -> bool {
        self.fixed_text.is_some()
    }

    pub fn is_tag(&self, local: &str) -> bool {
        self.tag
            .strip_prefix('{')
            .and_then(|rest| rest.split_once('}'))
            == Some((W, local))
    }

    pub fn text(&self, tree: &XmlTree) -> String {
        match &self.fixed_text {
            Some(text) => text.clone(),
            None => leading_text(tree, self.element),
        }
    }
}

fn clark_tag(tree: &XmlTree, id: NodeId) -> String {
    match tree.data(id) {
        NodeData::Element(_) => tree.clark(id).unwrap_or_default(),
        NodeData::Comment(_) => "#comment".into(),
        NodeData::Pi { .. } => "#pi".into(),
        NodeData::Text(_) => "#text".into(),
    }
}

#[derive(Clone, Copy)]
struct WalkContext {
    paragraph: Option<NodeId>,
    run: Option<NodeId>,
    sdt: Option<NodeId>,
    ins: bool,
    del: bool,
    hyperlink: Option<NodeId>,
    in_box: bool,
    in_field: bool,
}

/// Upstream `_collect_block_atoms` (field depth shared across a story).
pub(crate) fn collect_block_atoms(
    tree: &XmlTree,
    block_index: usize,
    element: NodeId,
    skip_text_boxes: bool,
    in_txbx: bool,
    field_depth: &mut u64,
    out: &mut Vec<Atom>,
) {
    let start = WalkContext {
        paragraph: is_w(tree, element, "p").then_some(element),
        run: None,
        sdt: None,
        ins: false,
        del: false,
        hyperlink: None,
        in_box: in_txbx,
        in_field: false,
    };
    let mut stack: Vec<(NodeId, WalkContext)> = first_choice_children(tree, element)
        .into_iter()
        .rev()
        .map(|child| (child, start))
        .collect();
    while let Some((child, ctx)) = stack.pop() {
        let w_local = if tree.ns_uri(child) == Some(W) {
            tree.local_name(child).unwrap_or("")
        } else {
            ""
        };
        if w_local == "txbxContent" && skip_text_boxes {
            continue;
        }
        if w_local == "fldChar" {
            match w_attr(tree, child, "fldCharType") {
                Some("begin") => *field_depth += 1,
                Some("end") if *field_depth > 0 => *field_depth -= 1,
                _ => {}
            }
            continue;
        }
        if is_direct_run_child(tree, child) {
            let projection = project_run_child(tree, child);
            if projection.text.is_empty() && !projection.barrier {
                continue;
            }
            let is_text_tag = matches!(w_local, "t" | "delText" | "instrText");
            out.push(Atom {
                element: child,
                tag: clark_tag(tree, child),
                block_index,
                paragraph: ctx.paragraph,
                run: ctx.run,
                sdt: ctx.sdt,
                in_insert: ctx.ins,
                in_delete: ctx.del,
                in_text_box: ctx.in_box,
                hyperlink: ctx.hyperlink,
                in_field: ctx.in_field || *field_depth > 0,
                fixed_text: if is_text_tag {
                    None
                } else {
                    Some(projection.text)
                },
                barrier: projection.barrier,
            });
            continue;
        }
        if tree.element(child).is_none() {
            continue;
        }
        let next = WalkContext {
            paragraph: if w_local == "p" {
                Some(child)
            } else {
                ctx.paragraph
            },
            run: if w_local == "r" { Some(child) } else { ctx.run },
            sdt: if w_local == "sdt" {
                Some(child)
            } else {
                ctx.sdt
            },
            ins: ctx.ins || matches!(w_local, "ins" | "moveTo"),
            del: ctx.del || matches!(w_local, "del" | "moveFrom"),
            hyperlink: if w_local == "hyperlink" {
                Some(child)
            } else {
                ctx.hyperlink
            },
            in_box: ctx.in_box || w_local == "txbxContent",
            in_field: ctx.in_field || w_local == "fldSimple",
        };
        for grandchild in first_choice_children(tree, child).into_iter().rev() {
            stack.push((grandchild, next));
        }
    }
}

/// Every atom of one story, in document order (upstream `_story_atoms`).
pub(crate) fn story_atoms(tree: &XmlTree, story: &str) -> Result<Vec<Atom>> {
    let mut atoms = Vec::new();
    let mut field_depth = 0u64;
    for block in story::iter_block_elements(tree, story)? {
        collect_block_atoms(
            tree,
            block.index,
            block.element,
            block.kind == BlockKind::Paragraph,
            block.in_txbx,
            &mut field_depth,
            &mut atoms,
        );
    }
    Ok(atoms)
}

/// Upstream `_include_atom`.
pub(crate) fn include_atom(atom: &Atom, view: View) -> bool {
    if atom.is_synthetic() {
        return match view {
            View::Current => !atom.in_delete,
            View::Original => !atom.in_insert,
            View::All => true,
        };
    }
    match view {
        View::Current => atom.is_tag("t") && !atom.in_delete,
        View::Original => !atom.in_insert && (atom.is_tag("t") || atom.is_tag("delText")),
        View::All => true,
    }
}

/// Assembled story text with a per-scalar `(atom index, offset)` map;
/// paragraph separators have offset `-1`.
pub(crate) fn assemble(tree: &XmlTree, atoms: &[&Atom]) -> (String, Vec<(usize, isize)>) {
    let mut text = String::new();
    let mut map = Vec::new();
    let mut previous: Option<&Atom> = None;
    for (index, atom) in atoms.iter().copied().enumerate() {
        if let Some(previous) = previous
            && atom.paragraph != previous.paragraph
        {
            text.push('\n');
            map.push((index, -1));
        }
        let piece = atom.text(tree);
        for offset in 0..piece.chars().count() {
            map.push((index, offset as isize));
        }
        text.push_str(&piece);
        previous = Some(atom);
    }
    (text, map)
}

fn match_space(value: &str, policy: MatchPolicy, limits: &Limits) -> Result<(String, Vec<usize>)> {
    match policy {
        MatchPolicy::Exact => Ok((value.to_string(), (0..value.chars().count()).collect())),
        MatchPolicy::Normalized => {
            let (text, map) = normalized_with_map(value);
            Limits::check(
                "max_normalized_chars",
                limits.max_normalized_chars,
                map.len() as u64,
            )?;
            Ok((text, map))
        }
    }
}

/// Scalar-indexed substring.
pub(crate) fn slice_chars(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

static NEXT_SPAN_TOKEN: AtomicU64 = AtomicU64::new(1);

/// A fresh consumption token for an independently created span.
pub(crate) fn new_span_token() -> u64 {
    NEXT_SPAN_TOKEN.fetch_add(1, Ordering::Relaxed)
}

/// Alias of [`new_span_token`] (name used by `tableops`).
pub(crate) fn fresh_span_token() -> u64 {
    new_span_token()
}

/// One edit-sensitive ancestor: identity, tag, sorted attributes and (for
/// content controls) the serialized `w:sdtPr`.
type Scope = (NodeId, String, Vec<(String, String)>, String);

/// Edit-sensitive structural context of one atom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Signature {
    tag: String,
    struct_gen: u32,
    paragraph: Option<NodeId>,
    run: Option<NodeId>,
    in_insert: bool,
    in_delete: bool,
    in_text_box: bool,
    in_field: bool,
    fixed_text: Option<String>,
    barrier: bool,
    scopes: Vec<Scope>,
}

const CONTEXT_SCOPE_TAGS: &[&str] = &[
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    "sdt",
    "hyperlink",
    "fldSimple",
    "txbxContent",
];

pub(crate) fn signature(tree: &XmlTree, atom: &Atom) -> Signature {
    let mut scopes = Vec::new();
    for ancestor in tree.ancestors(atom.element) {
        if tree.ns_uri(ancestor) != Some(W) {
            continue;
        }
        let local = tree.local_name(ancestor).unwrap_or("");
        if !CONTEXT_SCOPE_TAGS.contains(&local) {
            continue;
        }
        let mut attrs: Vec<(String, String)> = tree
            .element(ancestor)
            .map(|element| {
                element
                    .attrs
                    .iter()
                    .map(|attr| {
                        let name = match attr.ns {
                            Some(ns) => format!("{{{}}}{}", tree.uri(ns), attr.name.local),
                            None => attr.name.local.to_string(),
                        };
                        (name, attr.value.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        attrs.sort();
        let extra = if local == "sdt" {
            w_child(tree, ancestor, "sdtPr")
                .map(|pr| serialize_node(tree, pr))
                .unwrap_or_default()
        } else {
            String::new()
        };
        scopes.push((ancestor, local.to_string(), attrs, extra));
    }
    scopes.reverse();
    Signature {
        tag: atom.tag.clone(),
        struct_gen: tree.struct_gen(atom.element),
        paragraph: atom.paragraph,
        run: atom.run,
        in_insert: atom.in_insert,
        in_delete: atom.in_delete,
        in_text_box: atom.in_text_box,
        in_field: atom.in_field,
        fixed_text: atom.fixed_text.clone(),
        barrier: atom.barrier,
        scopes,
    }
}

/// A located visible-text interval (an opaque, session-local handle).
///
/// Cloning a span aliases its consumption token: once any alias is consumed
/// by a successful text-changing replacement, every alias is stale. An
/// independently found span has its own token and must pass freshness checks
/// after any edit.
#[derive(Debug, Clone)]
pub struct Span {
    pub(crate) document: DocumentId,
    pub(crate) token: u64,
    pub(crate) story: String,
    pub(crate) part: String,
    pub(crate) text: String,
    pub(crate) anchor: Anchor,
    pub(crate) in_insert: bool,
    pub(crate) in_delete: bool,
    pub(crate) in_content_control: bool,
    pub(crate) in_text_box: bool,
    pub(crate) in_field: bool,
    pub(crate) crosses_paragraphs: bool,
    pub(crate) atoms: Vec<Atom>,
    pub(crate) start_offset: usize,
    pub(crate) end_offset: usize,
    pub(crate) raw_start: usize,
    pub(crate) match_start: usize,
    pub(crate) match_policy: Option<MatchPolicy>,
    pub(crate) view: View,
    /// [`XmlTree::incarnation`] of the story part when the span was found.
    pub(crate) incarnation: u32,
    pub(crate) signatures: Vec<Signature>,
    pub(crate) atom_sequence: Vec<NodeId>,
    /// `None`: the sequence covers every atom (search-produced spans).
    pub(crate) sequence_view: Option<View>,
}

impl Span {
    /// A span over `atoms` holding `text`, with the context flags, signatures
    /// and anchor derived from the atoms; the location fields (offsets, atom
    /// sequence, policy, view) default and are set by the caller.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_atoms(
        tree: &XmlTree,
        document: DocumentId,
        token: u64,
        story: &str,
        part: &str,
        anchor_index: usize,
        text: String,
        atoms: Vec<Atom>,
    ) -> Span {
        Span {
            incarnation: tree.incarnation(),
            document,
            token,
            story: story.to_string(),
            part: part.to_string(),
            anchor: Anchor {
                story: story.to_string(),
                index: anchor_index,
                content_hash: content_hash(&text),
            },
            in_insert: atoms.iter().any(|a| a.in_insert),
            in_delete: atoms.iter().any(|a| a.in_delete || a.is_tag("delText")),
            in_content_control: atoms.iter().any(|a| a.sdt.is_some()),
            in_text_box: atoms.iter().any(|a| a.in_text_box),
            in_field: atoms.iter().any(|a| a.in_field),
            crosses_paragraphs: false,
            signatures: atoms.iter().map(|a| signature(tree, a)).collect(),
            atom_sequence: atoms.iter().map(|a| a.element).collect(),
            sequence_view: None,
            atoms,
            start_offset: 0,
            end_offset: 0,
            raw_start: 0,
            match_start: 0,
            match_policy: None,
            view: View::Current,
            text,
        }
    }

    /// The located text (scalar-exact source characters).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Story name.
    pub fn story(&self) -> &str {
        &self.story
    }

    /// Inert location evidence.
    pub fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    /// Any atom inside a pending insertion.
    pub fn in_insert(&self) -> bool {
        self.in_insert
    }

    /// Any atom inside a pending deletion (or deleted text).
    pub fn in_delete(&self) -> bool {
        self.in_delete
    }

    /// Any atom inside a content control.
    pub fn in_content_control(&self) -> bool {
        self.in_content_control
    }

    /// Any atom inside a text box.
    pub fn in_text_box(&self) -> bool {
        self.in_text_box
    }

    /// Any atom inside a field (instruction or result).
    pub fn in_field(&self) -> bool {
        self.in_field
    }

    /// The span crosses a paragraph boundary.
    pub fn crosses_paragraphs(&self) -> bool {
        self.crosses_paragraphs
    }

    /// Policy that selected this span.
    pub fn match_policy(&self) -> Option<MatchPolicy> {
        self.match_policy
    }

    /// View the span was captured under.
    pub fn view(&self) -> View {
        self.view
    }

    /// Scalar offset of the span in its story's view text.
    pub fn raw_start(&self) -> usize {
        self.raw_start
    }

    /// Scalar offset of the match in policy space.
    pub fn match_start(&self) -> usize {
        self.match_start
    }

    /// Owning document.
    pub fn document_id(&self) -> DocumentId {
        self.document
    }

    /// JSON evidence (for diagnostics and parity cases).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "story": self.story,
            "text": self.text,
            "anchor": self.anchor,
            "in_insert": self.in_insert,
            "in_delete": self.in_delete,
            "in_content_control": self.in_content_control,
            "in_text_box": self.in_text_box,
            "in_field": self.in_field,
            "crosses_paragraphs": self.crosses_paragraphs,
            "match_policy": self.match_policy,
            "raw_start": self.raw_start,
            "match_start": self.match_start,
        })
    }

    /// `(atom index or None for a paragraph separator, text)` pieces
    /// reconstituting the span text from the current tree.
    pub(crate) fn pieces(&self, tree: &XmlTree) -> Vec<(Option<usize>, String)> {
        let mut out = Vec::new();
        let last = self.atoms.len().saturating_sub(1);
        for (index, atom) in self.atoms.iter().enumerate() {
            if index > 0 && atom.paragraph != self.atoms[index - 1].paragraph {
                out.push((None, "\n".to_string()));
            }
            let text = atom.text(tree);
            let length = text.chars().count();
            let start = if index == 0 {
                self.start_offset.min(length)
            } else {
                0
            };
            let end = if index == last {
                self.end_offset.min(length)
            } else {
                length
            };
            out.push((Some(index), slice_chars(&text, start, end.max(start))));
        }
        out
    }

    pub(crate) fn current_slice(&self, tree: &XmlTree) -> String {
        self.pieces(tree)
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }
}

/// The spans of one story, plus the story's policy-space text and map (for
/// `near` ranking).
#[allow(clippy::too_many_arguments)]
fn spans_for_story(
    document: &Document,
    tree: &XmlTree,
    story: &story::Story,
    atoms: &[&Atom],
    all_atoms: &[Atom],
    needle_match: &str,
    view: View,
    policy: MatchPolicy,
) -> Result<(Vec<Span>, String, Vec<usize>)> {
    let limits = document.package().limits();
    let (raw_text, char_map) = assemble(tree, atoms);
    let (match_text, match_map) = match_space(&raw_text, policy, limits)?;
    let match_index = ScalarIndex::new(&match_text);
    let needle_len = needle_match.chars().count();
    let positions: HashMap<NodeId, usize> = all_atoms
        .iter()
        .enumerate()
        .map(|(i, a)| (a.element, i))
        .collect();
    let raw_chars: Vec<char> = raw_text.chars().collect();
    let mut spans = Vec::new();
    let mut search_from = 0usize;
    while search_from <= match_index.len() {
        let from_byte = match_index.byte(search_from);
        let Some(found_byte) = match_text[from_byte..].find(needle_match) else {
            break;
        };
        let found_at = match_index.scalar(from_byte + found_byte).ok_or_else(|| {
            Error::internal(
                "search_boundary",
                "match did not start on a character boundary",
            )
        })?;
        let match_end = found_at + needle_len;
        if !source_aligned(found_at, match_end, &match_map) {
            search_from = found_at + 1;
            continue;
        }
        search_from = match_end;
        let mut raw_start = match_map[found_at];
        let mut raw_end = match_map[match_end - 1] + 1;
        if policy == MatchPolicy::Exact
            && (char_map[raw_start].1 == -1 || char_map[raw_end - 1].1 == -1)
        {
            continue;
        }
        while raw_start < raw_end && char_map[raw_start].1 == -1 {
            raw_start += 1;
        }
        while raw_end > raw_start && char_map[raw_end - 1].1 == -1 {
            raw_end -= 1;
        }
        if raw_start >= raw_end {
            continue;
        }
        let (start_atom, start_offset) = char_map[raw_start];
        let (end_atom, end_offset) = char_map[raw_end - 1];
        let crosses = (raw_start..raw_end).any(|i| char_map[i].1 == -1)
            || atoms[start_atom].paragraph != atoms[end_atom].paragraph;
        let span_atoms: Vec<Atom> = atoms[start_atom..=end_atom]
            .iter()
            .map(|a| (*a).clone())
            .collect();
        let text: String = raw_chars[raw_start..raw_end].iter().collect();
        let sequence_start = positions[&span_atoms[0].element];
        let sequence_end = positions[&span_atoms[span_atoms.len() - 1].element];
        let anchor_index = span_atoms[0].block_index;
        spans.push(Span {
            crosses_paragraphs: crosses,
            atom_sequence: all_atoms[sequence_start..=sequence_end]
                .iter()
                .map(|a| a.element)
                .collect(),
            start_offset: start_offset as usize,
            end_offset: end_offset as usize + 1,
            raw_start,
            match_start: found_at,
            match_policy: Some(policy),
            view,
            ..Span::from_atoms(
                tree,
                document.id(),
                new_span_token(),
                &story.name,
                &story.part,
                anchor_index,
                text,
                span_atoms,
            )
        });
    }
    Ok((spans, match_text, match_map))
}

/// Every span of `needle` (upstream `find_text`).
pub fn find_text(document: &Document, needle: &str, options: &FindOptions) -> Result<Vec<Span>> {
    if options.near.is_some() && options.nth.is_some() {
        return Err(Error::invalid_argument(
            "near_and_nth",
            "near and nth are mutually exclusive",
        ));
    }
    let limits = document.package().limits();
    let (needle_match, _) = match_space(needle, options.policy, limits)?;
    if needle_match.is_empty()
        || (options.policy == MatchPolicy::Normalized && needle_match.trim_matches(' ').is_empty())
    {
        return Ok(Vec::new());
    }
    let near_match = match &options.near {
        Some(near) => Some(match_space(near, options.policy, limits)?.0),
        None => None,
    };
    let mut ranked: Vec<(Span, Option<usize>)> = Vec::new();
    for story in story::stories(document) {
        if options
            .story
            .as_deref()
            .is_some_and(|wanted| wanted != story.name)
        {
            continue;
        }
        let tree = document.package().xml(&story.part)?;
        let all_atoms = story_atoms(tree, &story.name)?;
        let atoms: Vec<&Atom> = all_atoms
            .iter()
            .filter(|a| include_atom(a, options.view))
            .collect();
        if atoms.is_empty() {
            continue;
        }
        let (spans, match_text, match_map) = spans_for_story(
            document,
            tree,
            &story,
            &atoms,
            &all_atoms,
            &needle_match,
            options.view,
            options.policy,
        )?;
        if spans.is_empty() {
            continue;
        }
        let mut near_positions = Vec::new();
        if let Some(near_match) = near_match.as_deref().filter(|n| !n.is_empty()) {
            let index = ScalarIndex::new(&match_text);
            let near_len = near_match.chars().count();
            let mut from = 0usize;
            while from <= index.len() {
                let byte = index.byte(from);
                let Some(found) = match_text[byte..].find(near_match) else {
                    break;
                };
                let start = index.scalar(byte + found).unwrap_or(from);
                if source_aligned(start, start + near_len, &match_map) {
                    near_positions.push(start);
                }
                from = start + 1;
            }
        }
        for span in spans {
            let distance = if options.near.is_some() {
                near_positions
                    .iter()
                    .map(|p| p.abs_diff(span.match_start))
                    .min()
            } else {
                None
            };
            ranked.push((span, distance));
        }
    }
    // Stable: unranked keep document order after ranked ones.
    ranked.sort_by_key(|(_, distance)| (distance.is_none(), distance.unwrap_or(0)));
    let mut spans: Vec<Span> = ranked.into_iter().map(|(span, _)| span).collect();
    if let Some(nth) = options.nth {
        if nth < 1 || nth > spans.len() {
            return Ok(Vec::new());
        }
        spans = vec![spans.swap_remove(nth - 1)];
    }
    Ok(spans)
}

/// The single span matching `needle`, or a typed refusal (upstream
/// `find_one`).
pub fn find_one(document: &Document, needle: &str, options: &FindOptions) -> Result<Span> {
    if options.near.is_some() {
        return Err(Error::invalid_argument(
            "near_unsupported",
            "find_one does not accept near; use find_text",
        ));
    }
    let mut matches = find_text(document, needle, options)?;
    match matches.len() {
        0 => Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "no_match",
            format!("no match for {needle:?} in any story part"),
        )),
        1 => Ok(matches.remove(0)),
        count => {
            let locations: Vec<String> = matches
                .iter()
                .take(5)
                .map(|s| format!("{}#{}", s.story, s.anchor.index))
                .collect();
            Err(Error::refusal(
                RefusalKind::AmbiguousTarget,
                "multiple_matches",
                format!(
                    "{count} matches for {needle:?} (at {}{}); disambiguate with nth, story, a more specific \
                     exact target, or inspect find_text with near",
                    locations.join(", "),
                    if count > 5 { ", …" } else { "" }
                ),
            ))
        }
    }
}
