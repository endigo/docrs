//! Safe untracked replacement (upstream `search.py` write side, M3 subset).
//!
//! Every replacement is *validate fully, then stage, then commit*:
//!
//! 1. argument, protection and freshness checks run against the live tree;
//! 2. the edit is planned completely (unique affix localization, writable
//!    segments, destination evidence, positional-marker, bookmark, control,
//!    placeholder and revision checks) without touching anything;
//! 3. the plan is applied to a clone of the story tree (node IDs retained);
//! 4. the clone replaces the live tree and the span's consumption token is
//!    recorded — together, after nothing else can fail.
//!
//! A refusal or failure at any earlier step leaves the document, every
//! handle and every file exactly as they were. Identical-text replacement is
//! an explicit unchanged report that consumes nothing.
//!
//! Supported (M3): untracked edits inside plain base-text runs of one owner.
//! Tracked replacement is planned for M5 and refuses explicitly.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use serde::Serialize;

use crate::blocks::validate_xml_characters;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::revision::py_int;
use crate::search::{Atom, MatchPolicy, Span, find_text, include_atom, signature, story_atoms};
use crate::story::{self, Anchor, View};
pub(crate) use crate::tracked::next_revision_id;
use crate::tracked::{
    add_tracked_run, hollowed_bookmarks, insert_after, new_revision, pieces_in_range,
    set_preserved_text,
};
use crate::unicode::is_py_space;
use crate::wml::{self, W14, is_w, iter_nodes, leading_text, w_attr, w_child};
use crate::xml::{self, NodeData, NodeId, XML_NS, XmlTree};
use crate::{Document, Error, ErrorKind, FindOptions, RefusalKind, Result};

/// Options for [`Document::replace`].
#[derive(Debug, Clone, Default)]
pub struct ReplaceOptions {
    /// Author a tracked change (`w:del` / `w:ins`).
    pub tracked: bool,
    /// Author for tracked changes (required when `tracked`).
    pub author: Option<String>,
    /// Revision date (`None`: now).
    pub date: Option<crate::clock::Timestamp>,
    /// Edit text owned by one existing insertion without changing it.
    pub preserve_revision: bool,
}

/// Evidence for one replacement (upstream `ReplaceResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplaceReport {
    /// Story name.
    pub story: String,
    /// Text removed (whole span for untracked edits; changed region when
    /// narrowed across a break or boundary).
    pub deleted_text: String,
    /// Text written.
    pub inserted_text: String,
    /// Tracked change authored.
    pub tracked: bool,
    /// New revision ids.
    pub revision_ids: Vec<i64>,
    /// Existing insertion ids preserved.
    pub preserved_revision_ids: Vec<i64>,
    /// Every changed formatting region was kept (no intentional collapse).
    pub preserved_formatting_regions: bool,
    /// Whether anything changed (false for identical text).
    pub changed: bool,
}

impl ReplaceReport {
    /// JSON (`schema: paper_replace`, `version: 3` plus `changed`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_replace",
            "version": 3,
            "story": self.story,
            "deleted_text": self.deleted_text,
            "inserted_text": self.inserted_text,
            "tracked": self.tracked,
            "revision_ids": self.revision_ids,
            "preserved_revision_ids": self.preserved_revision_ids,
            "preserved_formatting_regions": self.preserved_formatting_regions,
            "changed": self.changed,
        })
    }
}

/// One match `replace_all` refused (the rest proceeded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusedMatch {
    /// Story name.
    pub story: String,
    /// Location evidence of the refused span.
    pub anchor: Anchor,
    /// Refusal code.
    pub code: &'static str,
    /// Upstream exception class name.
    pub error: &'static str,
    /// Stable sub-code.
    pub reason: &'static str,
    /// Human context.
    pub message: String,
}

/// Outcome of [`Document::replace_all`] (upstream `ReplaceAllResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplaceAllReport {
    /// Successful replacements.
    pub replaced_count: usize,
    /// Per-match evidence, in application order.
    pub results: Vec<ReplaceReport>,
    /// Per-match refusals.
    pub refused: Vec<RefusedMatch>,
}

impl ReplaceAllReport {
    /// JSON (`schema: paper_replace_all`, `version: 3`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_replace_all",
            "version": 3,
            "replaced_count": self.replaced_count,
            "results": self.results.iter().map(ReplaceReport::to_json).collect::<Vec<_>>(),
            "refused": self.refused,
        })
    }
}

/// Failure-injection points for replacement (`fault-injection` feature).
#[cfg(feature = "fault-injection")]
pub mod fault {
    use std::cell::Cell;

    /// Where an injected failure fires.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ReplacePoint {
        /// After the plan was applied to the staged tree.
        AfterStage,
        /// Immediately before the staged trees replace the live ones.
        BeforeCommit,
    }

    thread_local! {
        static FAIL_AT: Cell<Option<ReplacePoint>> = const { Cell::new(None) };
    }

    /// Arm a one-shot failure.
    pub fn fail_at(point: ReplacePoint) {
        FAIL_AT.with(|cell| cell.set(Some(point)));
    }

    /// Disarm.
    pub fn clear() {
        FAIL_AT.with(|cell| cell.set(None));
    }

    pub(crate) fn check(point: ReplacePoint) -> crate::Result<()> {
        FAIL_AT.with(|cell| {
            if cell.get() == Some(point) {
                cell.set(None);
                Err(crate::Error::internal(
                    "injected_failure",
                    format!("injected failure at {point:?}"),
                ))
            } else {
                Ok(())
            }
        })
    }
}

#[cfg(feature = "fault-injection")]
fn inject_after_stage() -> Result<()> {
    fault::check(fault::ReplacePoint::AfterStage)
}
#[cfg(feature = "fault-injection")]
fn inject_before_commit() -> Result<()> {
    fault::check(fault::ReplacePoint::BeforeCommit)
}
#[cfg(not(feature = "fault-injection"))]
fn inject_after_stage() -> Result<()> {
    Ok(())
}
#[cfg(not(feature = "fault-injection"))]
fn inject_before_commit() -> Result<()> {
    Ok(())
}

fn boundary(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::BoundaryViolation, reason, message)
}

fn stale(reason: &'static str, message: impl Into<String>) -> Error {
    Error::not_found(reason, message)
}

fn ambiguous_affix() -> Error {
    Error::unsupported(
        "ambiguous_affix_alignment",
        "exact affix alignment is ambiguous; re-find and replace only the exact substring you intend to \
         change (for example, 'payment' with 'settlement')",
    )
}

#[derive(Debug, Clone)]
struct BookmarkRange {
    name: String,
    text_elements: Vec<NodeId>,
}

#[derive(Debug, Clone, Default)]
struct BookmarkCensus {
    ranges: Vec<BookmarkRange>,
    by_element: HashMap<NodeId, Vec<usize>>,
}

fn bookmark_census(tree: &XmlTree) -> BookmarkCensus {
    let mut active: Vec<(Option<String>, String, Vec<NodeId>)> = Vec::new();
    let mut ranges = Vec::new();
    for node in iter_nodes(tree, tree.root()) {
        if is_w(tree, node, "bookmarkStart") {
            let id = w_attr(tree, node, "id").map(str::to_string);
            let name = w_attr(tree, node, "name").unwrap_or("").to_string();
            active.retain(|(existing, _, _)| *existing != id);
            active.push((id, name, Vec::new()));
        } else if (is_w(tree, node, "t") || is_w(tree, node, "delText"))
            && !leading_text(tree, node).is_empty()
        {
            for (_, _, elements) in active.iter_mut() {
                elements.push(node);
            }
        } else if is_w(tree, node, "bookmarkEnd") {
            let id = w_attr(tree, node, "id").map(str::to_string);
            if let Some(position) = active.iter().position(|(existing, _, _)| *existing == id) {
                let (_, name, elements) = active.remove(position);
                if name != "_GoBack" && !elements.is_empty() {
                    ranges.push(BookmarkRange {
                        name,
                        text_elements: elements,
                    });
                }
            }
        }
    }
    let mut by_element: HashMap<NodeId, Vec<usize>> = HashMap::new();
    for (index, range) in ranges.iter().enumerate() {
        for element in &range.text_elements {
            by_element.entry(*element).or_default().push(index);
        }
    }
    BookmarkCensus { ranges, by_element }
}

/// One story census shared by a synchronous batch.
pub struct Census {
    atoms: Vec<Atom>,
    by_element: HashMap<NodeId, usize>,
    bookmarks: BookmarkCensus,
}

impl Census {
    fn build(tree: &XmlTree, story: &str) -> Result<Census> {
        let atoms = story_atoms(tree, story)?;
        let by_element = atoms
            .iter()
            .enumerate()
            .map(|(index, atom)| (atom.element, index))
            .collect();
        Ok(Census {
            atoms,
            by_element,
            bookmarks: bookmark_census(tree),
        })
    }
}

pub(crate) fn validate_fresh(
    document: &Document,
    tree: &XmlTree,
    span: &Span,
    census: Option<&Census>,
) -> Result<()> {
    if document.is_consumed(span.token) {
        return Err(stale(
            "span_consumed",
            "span was consumed by a successful replacement; re-find the text",
        ));
    }
    if span.current_slice(tree) != span.text {
        return Err(stale(
            "span_text_changed",
            format!(
                "span is stale: expected {:?} at its anchor ({}#{}) but the document has changed",
                span.text, span.anchor.story, span.anchor.index
            ),
        ));
    }
    if !story::stories(document)
        .iter()
        .any(|story| story.name == span.story)
    {
        return Err(stale(
            "story_removed",
            "span is stale: its story part was removed from the document",
        ));
    }
    if tree.incarnation() != span.incarnation {
        return Err(stale(
            "story_replaced",
            "span is stale: its story part was removed and added again; re-find the text",
        ));
    }
    let owned;
    let (atoms, by_element): (&[Atom], &HashMap<NodeId, usize>) = match census {
        Some(census) => (&census.atoms, &census.by_element),
        None => {
            let atoms = story_atoms(tree, &span.story)?;
            let map: HashMap<NodeId, usize> = atoms
                .iter()
                .enumerate()
                .map(|(i, a)| (a.element, i))
                .collect();
            owned = (atoms, map);
            (&owned.0, &owned.1)
        }
    };
    for (captured, expected) in span.atoms.iter().zip(span.signatures.iter()) {
        let current = by_element
            .get(&captured.element)
            .map(|index| &atoms[*index]);
        if current.is_none_or(|atom| signature(tree, atom) != *expected) {
            return Err(stale(
                "span_context_changed",
                "span is stale: its field, hyperlink, content-control, revision, or containing structure has \
                 changed or was removed",
            ));
        }
    }
    let sequence: Vec<NodeId> = match span.sequence_view {
        None => atoms.iter().map(|a| a.element).collect(),
        Some(view) => atoms
            .iter()
            .filter(|a| include_atom(a, view))
            .map(|a| a.element)
            .collect(),
    };
    let start = span
        .atom_sequence
        .first()
        .and_then(|first| sequence.iter().position(|e| e == first));
    let end = span
        .atom_sequence
        .last()
        .and_then(|last| sequence.iter().position(|e| e == last));
    let current: &[NodeId] = match (start, end) {
        (Some(start), Some(end)) if start <= end => &sequence[start..=end],
        _ => &[],
    };
    if current != span.atom_sequence.as_slice() {
        return Err(stale(
            "span_sequence_changed",
            "span is stale: the text-atom sequence inside its captured interval has changed",
        ));
    }
    Ok(())
}

/// Upstream `_validate_writable_text`.
pub(crate) fn validate_writable_text(value: &str, argument: &str) -> Result<()> {
    validate_xml_characters(value, argument)?;
    let mut found: Vec<char> = value
        .chars()
        .filter(|c| matches!(c, '\n' | '\r' | '\t' | '\u{b}' | '\u{c}'))
        .collect();
    found.sort();
    found.dedup();
    if !found.is_empty() {
        return Err(Error::invalid_argument(
            "control_characters",
            format!(
                "{argument} contains control character(s) {found:?}: Word does not render them as breaks \
                 inside w:t — pass separate paragraphs instead"
            ),
        ));
    }
    Ok(())
}

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Canonical form of an element subtree (stands in for lxml `c14n`).
pub(crate) fn canonical(tree: &XmlTree, node: NodeId) -> String {
    let mut out = String::new();
    let mut scope: Vec<(Option<String>, Option<String>)> = tree
        .in_scope(node)
        .into_iter()
        .map(|(prefix, uri)| {
            (
                prefix.map(|p| p.to_string()),
                uri.map(|u| tree.uri(u).to_string()),
            )
        })
        .collect();
    scope.sort();
    out.push_str(&format!("{scope:?}"));
    canonical_rec(tree, node, &mut out);
    out
}

/// Pre-order with an explicit stack (`None` closes an element), so subtree
/// depth costs no call stack.
fn canonical_rec(tree: &XmlTree, node: NodeId, out: &mut String) {
    let mut stack = vec![Some(node)];
    while let Some(step) = stack.pop() {
        let Some(node) = step else {
            out.push_str("</>");
            continue;
        };
        match tree.data(node) {
            NodeData::Text(text) => out.push_str(&format!("T{text:?}")),
            NodeData::Comment(_) => {}
            NodeData::Pi { target, data } => out.push_str(&format!("P{target:?}{data:?}")),
            NodeData::Element(element) => {
                let mut attrs: Vec<(String, String, String)> = element
                    .attrs
                    .iter()
                    .map(|attr| {
                        (
                            attr.ns
                                .map(|ns| tree.uri(ns).to_string())
                                .unwrap_or_default(),
                            attr.name.lexical(),
                            attr.value.clone(),
                        )
                    })
                    .collect();
                attrs.sort();
                let mut decls: Vec<(Option<String>, Option<String>)> = element
                    .ns_decls
                    .iter()
                    .filter(|decl| {
                        tree.parent(node).is_none_or(|parent| {
                            tree.lookup_prefix(parent, decl.prefix.as_deref()) != Some(decl.uri)
                        })
                    })
                    .map(|decl| {
                        (
                            decl.prefix.as_deref().map(str::to_string),
                            decl.uri.map(|u| tree.uri(u).to_string()),
                        )
                    })
                    .collect();
                decls.sort();
                out.push_str(&format!("<{}{decls:?}{attrs:?}>", element.name.lexical()));
                stack.push(None);
                stack.extend(tree.children(node).iter().rev().map(|child| Some(*child)));
            }
        }
    }
}

type Evidence = (String, Vec<NodeId>);

fn destination_evidence(tree: &XmlTree, atom: &Atom) -> Result<Evidence> {
    let (Some(run), Some(paragraph)) = (atom.run, atom.paragraph) else {
        return Err(Error::unsupported(
            "not_run_text",
            "the changed interval is not ordinary run text; target a smaller plain-text span",
        ));
    };
    let rpr = w_child(tree, run, "rPr")
        .map(|rpr| canonical(tree, rpr))
        .unwrap_or_default();
    let mut ancestry = Vec::new();
    let mut current = tree.parent(run);
    while let Some(node) = current {
        if node == paragraph {
            break;
        }
        ancestry.push(node);
        current = tree.parent(node);
    }
    if current != Some(paragraph) {
        return Err(Error::unsupported(
            "detached_from_paragraph",
            "the changed interval is detached from its owning paragraph; re-find the text",
        ));
    }
    Ok((rpr, ancestry))
}

fn same_destination(left: &Evidence, right: &Evidence) -> bool {
    left.0 == right.0 && left.1 == right.1
}

fn has_multiple_direct_rpr(tree: &XmlTree, atoms: &[&Atom]) -> Result<bool> {
    if atoms.iter().any(|atom| atom.is_synthetic()) {
        return Ok(false);
    }
    let first = destination_evidence(tree, atoms[0])?.0;
    for atom in &atoms[1..] {
        if destination_evidence(tree, atom)?.0 != first {
            return Ok(true);
        }
    }
    Ok(false)
}

fn maximal_affix_decompositions(
    old: &[char],
    new: &[char],
    aligned: impl Fn(usize, usize) -> bool,
) -> Vec<(usize, usize)> {
    let limit = old.len().min(new.len());
    let mut prefix_limit = 0;
    while prefix_limit < limit && aligned(prefix_limit, prefix_limit) {
        prefix_limit += 1;
    }
    let mut suffix_limit = 0;
    while suffix_limit < limit
        && aligned(old.len() - suffix_limit - 1, new.len() - suffix_limit - 1)
    {
        suffix_limit += 1;
    }
    let preserved = (prefix_limit + suffix_limit).min(limit);
    let first_prefix = preserved.saturating_sub(suffix_limit);
    let last_prefix = prefix_limit.min(preserved);
    (first_prefix..=last_prefix)
        .map(|prefix| (prefix, preserved - prefix))
        .collect()
}

pub(crate) fn refuse_intervening_positional_nodes(tree: &XmlTree, atoms: &[&Atom]) -> Result<()> {
    if atoms.len() < 2 {
        return Ok(());
    }
    let Some(paragraph) = atoms[0].paragraph else {
        return Ok(());
    };
    if atoms.iter().any(|atom| atom.paragraph != Some(paragraph)) {
        return Ok(());
    }
    let stream: Vec<NodeId> = iter_nodes(tree, paragraph).collect();
    let positions: HashMap<NodeId, usize> =
        stream.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut bookmark_names: HashMap<Option<String>, String> = HashMap::new();
    for node in &stream {
        if is_w(tree, *node, "bookmarkStart") {
            bookmark_names.insert(
                w_attr(tree, *node, "id").map(str::to_string),
                w_attr(tree, *node, "name")
                    .filter(|n| !n.is_empty())
                    .unwrap_or("<unnamed>")
                    .to_string(),
            );
        }
    }
    let changed: HashSet<NodeId> = atoms.iter().map(|atom| atom.element).collect();
    let mut changed_ancestors: HashSet<NodeId> = HashSet::new();
    for element in &changed {
        changed_ancestors.extend(tree.ancestors(*element));
    }
    let (Some(first), Some(last)) = (
        positions.get(&atoms[0].element),
        positions.get(&atoms[atoms.len() - 1].element),
    ) else {
        return Ok(());
    };
    if first + 1 >= *last {
        return Ok(());
    }
    for node in &stream[first + 1..*last] {
        if changed.contains(node) || changed_ancestors.contains(node) {
            continue;
        }
        if is_w(tree, *node, "rPr") || tree.ancestors(*node).any(|a| is_w(tree, a, "rPr")) {
            continue;
        }
        if is_w(tree, *node, "bookmarkStart") || is_w(tree, *node, "bookmarkEnd") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let name = bookmark_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "<unnamed>".into());
            return Err(Error::unsupported(
                "crosses_bookmark",
                format!(
                    "the changed interval crosses bookmark {name:?}; target a smaller span wholly inside or outside that bookmark"
                ),
            ));
        }
        return Err(Error::unsupported(
            "crosses_positional_marker",
            "the changed interval crosses a positional marker or non-text run node whose anchoring cannot be \
             preserved; target a smaller span on one side",
        ));
    }
    Ok(())
}

/// Destination evidence of every atom, refused when the atoms sit under
/// different inline wrapper owners.
fn inline_ancestry_evidence(tree: &XmlTree, atoms: &[&Atom]) -> Result<Vec<Evidence>> {
    let evidence: Vec<Evidence> = atoms
        .iter()
        .map(|atom| destination_evidence(tree, atom))
        .collect::<Result<_>>()?;
    if evidence[1..].iter().any(|item| item.1 != evidence[0].1) {
        return Err(Error::unsupported(
            "crosses_inline_wrappers",
            "the changed interval crosses separate inline wrapper owners; target and replace a smaller span \
             within one wrapper, or edit each wrapper separately",
        ));
    }
    Ok(evidence)
}

pub(crate) fn validate_one_inline_ancestry(tree: &XmlTree, atoms: &[&Atom]) -> Result<()> {
    inline_ancestry_evidence(tree, atoms).map(|_| ())
}

fn control_type(tree: &XmlTree, sdt_pr: Option<NodeId>) -> &'static str {
    let Some(pr) = sdt_pr else { return "rich_text" };
    if tree.first_child_named(pr, W14, "checkbox").is_some() {
        return "checkbox";
    }
    for (local, kind) in [
        ("dropDownList", "dropdown"),
        ("comboBox", "combo"),
        ("date", "date"),
        ("picture", "picture"),
        ("docPartObj", "building_block"),
        ("group", "group"),
        ("text", "text"),
    ] {
        if w_child(tree, pr, local).is_some() {
            return kind;
        }
    }
    "rich_text"
}

fn validate_control(tree: &XmlTree, sdt: NodeId) -> Result<()> {
    let sdt_pr = w_child(tree, sdt, "sdtPr");
    if sdt_pr.is_some_and(|pr| w_child(tree, pr, "dataBinding").is_some()) {
        return Err(Error::unsupported(
            "data_bound_control",
            "span lies in a data-bound control whose surface text Word re-syncs from custom XML; use the binding source instead",
        ));
    }
    let kind = control_type(tree, sdt_pr);
    if kind != "text" && kind != "rich_text" {
        return Err(Error::unsupported(
            "typed_control",
            format!(
                "span lies in a {kind} control whose display text is tied to machine-readable control state; set the control value instead"
            ),
        ));
    }
    if let Some(pr) = sdt_pr {
        let locks: Vec<NodeId> = wml::w_children(tree, pr, "lock").collect();
        if locks.len() > 1 {
            return Err(Error::unsupported(
                "multiple_locks",
                "control has multiple w:lock declarations; nothing was changed",
            ));
        }
        if locks.first().is_some_and(|lock| {
            matches!(
                w_attr(tree, *lock, "val"),
                Some("contentLocked" | "sdtContentLocked")
            )
        }) {
            return Err(Error::unsupported(
                "locked_control",
                "control content is locked; nothing was changed",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_replaceable(
    tree: &XmlTree,
    span: &Span,
    text_boundaries: bool,
) -> Result<()> {
    for atom in &span.atoms {
        if text_boundaries && atom.is_synthetic() {
            let detail = if atom.barrier {
                "unmodeled visible run content"
            } else {
                "a tab or line break, or a no-break hyphen"
            };
            return Err(Error::unsupported(
                if atom.barrier {
                    "crosses_unmodeled_content"
                } else {
                    "crosses_break"
                },
                format!(
                    "span crosses {detail}; replace the text segments on either side individually"
                ),
            ));
        }
        if atom.is_tag("delText") || atom.in_delete {
            return Err(Error::unsupported(
                "tracked_deletion",
                "span includes tracked-deleted text; resolve the revision first or target visible text only",
            ));
        }
        if atom.is_tag("instrText") {
            return Err(Error::unsupported(
                "field_instruction",
                "span includes field-instruction text; editing field code internals is not supported",
            ));
        }
        if atom.in_field {
            return Err(Error::unsupported(
                "field_result",
                "span lies inside a field result (TOC entry, page number, date, cross-reference, …); Word \
                 regenerates field results on update, so the edit would silently vanish",
            ));
        }
    }
    if text_boundaries && span.crosses_paragraphs {
        return Err(boundary(
            "crosses_paragraph",
            "span crosses a paragraph boundary; character-level replace is same-paragraph only",
        ));
    }
    let scopes: HashSet<Option<NodeId>> = span.atoms.iter().map(|atom| atom.sdt).collect();
    if scopes.len() > 1 {
        return Err(boundary(
            "crosses_content_control",
            "span crosses a content-control boundary; edit inside or outside the control, not across it",
        ));
    }
    let mut controls: Vec<NodeId> = Vec::new();
    for atom in &span.atoms {
        for ancestor in tree.ancestors(atom.element) {
            if is_w(tree, ancestor, "sdt") && !controls.contains(&ancestor) {
                controls.push(ancestor);
            }
        }
    }
    for control in controls {
        validate_control(tree, control)?;
    }
    let links: HashSet<Option<NodeId>> = span.atoms.iter().map(|atom| atom.hyperlink).collect();
    if links.len() > 1 {
        return Err(boundary(
            "crosses_hyperlink",
            "span crosses a hyperlink boundary; edit the linked text and the surrounding text separately",
        ));
    }
    Ok(())
}

fn enclosing_insertion(tree: &XmlTree, element: NodeId) -> Option<NodeId> {
    tree.ancestors(element)
        .find(|node| is_w(tree, *node, "ins") || is_w(tree, *node, "moveTo"))
}

fn revision_ancestors(tree: &XmlTree, element: NodeId) -> Vec<NodeId> {
    tree.ancestors(element)
        .filter(|node| {
            ["ins", "del", "moveFrom", "moveTo"]
                .iter()
                .any(|local| is_w(tree, *node, local))
        })
        .collect()
}

fn preserved_insertion_ids(tree: &XmlTree, span: &Span, authorize: bool) -> Result<Vec<i64>> {
    if !authorize {
        return Ok(Vec::new());
    }
    let wrappers: Vec<Option<NodeId>> = span
        .atoms
        .iter()
        .map(|atom| enclosing_insertion(tree, atom.element))
        .collect();
    if wrappers.iter().all(Option::is_none) {
        return Ok(Vec::new());
    }
    if span.view != View::Current {
        return Err(Error::unsupported(
            "preserve_requires_current_view",
            "preserve_revision requires a span selected with view='current'",
        ));
    }
    if wrappers.iter().any(Option::is_none) {
        return Err(Error::unsupported(
            "mixed_insertion_ownership",
            "span mixes base text and insertion-owned text; preserve one existing insertion at a time",
        ));
    }
    let wrapper = wrappers[0].unwrap_or(span.atoms[0].element);
    if !is_w(tree, wrapper, "ins") {
        return Err(Error::unsupported(
            "tracked_move",
            "tracked moves cannot be edited in place; resolve the move first",
        ));
    }
    if wrappers[1..]
        .iter()
        .any(|candidate| *candidate != Some(wrapper))
    {
        return Err(Error::unsupported(
            "multiple_insertions",
            "span crosses multiple tracked insertion wrappers; preserve one existing insertion at a time",
        ));
    }
    for element in &span.atom_sequence {
        let ancestors = revision_ancestors(tree, *element);
        if ancestors.len() != 1 || ancestors[0] != wrapper {
            return Err(Error::unsupported(
                "mixed_revision_history",
                "span contains nested or mixed revision history that cannot be edited while preserving one insertion",
            ));
        }
    }
    match w_attr(tree, wrapper, "id").and_then(py_int) {
        Some(id) => Ok(vec![id]),
        None => Err(Error::unsupported(
            "revision_id",
            "the preserved insertion has a missing or non-decimal w:id",
        )),
    }
}

pub(crate) fn placeholder_controls(tree: &XmlTree, atoms: &[Atom]) -> Vec<NodeId> {
    let mut controls = Vec::new();
    for atom in atoms {
        let Some(sdt) = atom.sdt else { continue };
        if controls.contains(&sdt) {
            continue;
        }
        if w_child(tree, sdt, "sdtPr")
            .is_some_and(|pr| w_child(tree, pr, "showingPlcHdr").is_some())
        {
            controls.push(sdt);
        }
    }
    controls
}

fn placeholder_prompt_text(tree: &XmlTree, sdt: NodeId) -> String {
    let Some(content) = w_child(tree, sdt, "sdtContent") else {
        return String::new();
    };
    tree.descendants(content)
        .filter(|node| is_w(tree, *node, "t"))
        .map(|node| leading_text(tree, node))
        .collect()
}

fn in_nested_sdt(tree: &XmlTree, node: NodeId, outer: NodeId) -> bool {
    for ancestor in tree.ancestors(node) {
        if ancestor == outer {
            return false;
        }
        if is_w(tree, ancestor, "sdt") {
            return true;
        }
    }
    false
}

fn clear_placeholder_state(tree: &mut XmlTree, sdt: NodeId) -> Result<()> {
    if let Some(pr) = w_child(tree, sdt, "sdtPr")
        && let Some(showing) = w_child(tree, pr, "showingPlcHdr")
    {
        tree.detach(showing)?;
    }
    let styles: Vec<NodeId> = tree
        .descendants(sdt)
        .filter(|node| {
            is_w(tree, *node, "rStyle") && w_attr(tree, *node, "val") == Some("PlaceholderText")
        })
        .filter(|node| !in_nested_sdt(tree, *node, sdt))
        .collect();
    for style in styles {
        tree.detach(style)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct TextAssignment {
    element: NodeId,
    before: String,
    after: String,
    after_space: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Plan {
    report: ReplaceReport,
    assignments: Vec<TextAssignment>,
    placeholders: Vec<NodeId>,
    tracked: Option<TrackedPlan>,
    consume: bool,
}

impl Plan {
    fn mutates(&self) -> bool {
        !self.assignments.is_empty() || !self.placeholders.is_empty() || self.tracked.is_some()
    }
}

/// Upstream `_tracked_layering_context`: the enclosing insertion of each
/// atom and whether the span extends the author's own insertion.
#[derive(Debug, Clone)]
pub(crate) struct Layering {
    enclosing: Vec<Option<NodeId>>,
    extends_own_insertion: bool,
}

/// A fully validated tracked replacement, applied mechanically.
#[derive(Debug, Clone)]
struct TrackedPlan {
    first_element: NodeId,
    first_run: NodeId,
    first_text: String,
    middle_elements: Vec<NodeId>,
    last_element: Option<NodeId>,
    last_text: Option<String>,
    same_run_tail: Option<String>,
    rpr: Option<NodeId>,
    ins_rpr: Option<NodeId>,
    deleted_pieces: Vec<(Option<NodeId>, String)>,
    new_mid: String,
    del_id: Option<i64>,
    ins_id: Option<i64>,
    author: String,
    date: String,
    own_insertions: Vec<NodeId>,
}

fn piece_lengths(pieces: &[(Option<usize>, String)]) -> Vec<(Option<usize>, usize)> {
    pieces
        .iter()
        .map(|(index, text)| (*index, text.chars().count()))
        .collect()
}

/// Each piece as `(atom index, its interval in the span text, base)`, where
/// `base` is the interval start's offset inside the atom's own text
/// (`span_start` for the first atom, 0 otherwise).
fn piece_ranges(
    pieces: &[(Option<usize>, usize)],
    span_start: usize,
) -> impl Iterator<Item = (Option<usize>, Range<usize>, usize)> + '_ {
    pieces
        .iter()
        .scan(0usize, move |position, (atom_index, length)| {
            let start = *position;
            *position += length;
            let base = if *atom_index == Some(0) {
                span_start
            } else {
                0
            };
            Some((*atom_index, start..*position, base))
        })
}

pub(crate) struct Outcome {
    pub(crate) replacement: String,
    pub(crate) segments: Vec<(usize, usize, usize)>,
    /// Scalar offsets of the changed interval in the span text.
    pub(crate) changed_start: usize,
    pub(crate) changed_end: usize,
}

fn writable_segments(
    span: &Span,
    pieces: &[(Option<usize>, usize)],
    start_offset: usize,
    end_offset: usize,
) -> Result<Vec<(usize, usize, usize)>> {
    let mut out = Vec::new();
    for (atom_index, piece, base) in piece_ranges(pieces, span.start_offset) {
        let start = start_offset.max(piece.start);
        let end = end_offset.min(piece.end);
        if end > start {
            let Some(atom_index) = atom_index else {
                return Err(boundary(
                    "crosses_paragraph",
                    "the changed interval crosses a paragraph boundary; replace one paragraph at a time",
                ));
            };
            out.push((
                atom_index,
                base + start - piece.start,
                base + end - piece.start,
            ));
        }
    }
    if out.is_empty() {
        return Err(Error::unsupported(
            "no_writable_region",
            "the changed interval has no writable text region; re-find and replace only the exact substring you intend to change",
        ));
    }
    Ok(out)
}

pub(crate) fn ordinary_outcome(tree: &XmlTree, span: &Span, new_text: &str) -> Result<Outcome> {
    let old = chars(&span.text);
    let new = chars(new_text);
    let decompositions = maximal_affix_decompositions(&old, &new, |o, n| old[o] == new[n]);
    if decompositions.len() != 1 {
        return Err(ambiguous_affix());
    }
    let (prefix, suffix) = decompositions[0];
    let changed_start = prefix;
    let mut changed_end = old.len() - suffix;
    let mut replacement = collect(&new[prefix..new.len() - suffix]);
    let pieces = piece_lengths(&span.pieces(tree));
    let segments = if changed_start == changed_end {
        let mut candidates: Vec<(usize, usize)> = Vec::new();
        for (atom_index, piece, base) in piece_ranges(&pieces, span.start_offset) {
            if let Some(atom_index) = atom_index
                && piece.start <= changed_start
                && changed_start <= piece.end
            {
                let atom = &span.atoms[atom_index];
                if !atom.is_synthetic()
                    && candidates
                        .iter()
                        .all(|(existing, _)| *existing != atom_index)
                {
                    candidates.push((atom_index, base + changed_start - piece.start));
                }
            }
        }
        if candidates.is_empty() {
            return Err(Error::unsupported(
                "no_insertion_region",
                "the insertion point has no writable text region; re-find text on one side of the insertion point",
            ));
        }
        let candidate_atoms: Vec<&Atom> = candidates
            .iter()
            .map(|(index, _)| &span.atoms[*index])
            .collect();
        let first = destination_evidence(tree, candidate_atoms[0])?;
        for atom in &candidate_atoms[1..] {
            if !same_destination(&first, &destination_evidence(tree, atom)?) {
                return Err(Error::unsupported(
                    "ambiguous_insertion_destination",
                    "the insertion point has competing formatting or structural destinations; re-find a smaller span \
                     with one uniform destination or construct the intended formatting and structure explicitly",
                ));
            }
        }
        refuse_intervening_positional_nodes(tree, &candidate_atoms)?;
        vec![(candidates[0].0, candidates[0].1, candidates[0].1)]
    } else {
        let mut segments = writable_segments(span, &pieces, changed_start, changed_end)?;
        let changed_atoms: Vec<&Atom> = segments
            .iter()
            .map(|(index, _, _)| &span.atoms[*index])
            .collect();
        if !replacement.is_empty() && has_multiple_direct_rpr(tree, &changed_atoms)? {
            let final_index = segments[segments.len() - 1].0;
            if let Some((_, piece, _)) = piece_ranges(&pieces, span.start_offset)
                .find(|(atom_index, _, _)| *atom_index == Some(final_index))
            {
                changed_end = piece.end;
            }
            let suffix_to_keep = old.len() - changed_end;
            replacement = collect(&new[prefix..new.len() - suffix_to_keep]);
            segments = writable_segments(span, &pieces, changed_start, changed_end)?;
        }
        segments
    };
    Ok(Outcome {
        replacement,
        segments,
        changed_start,
        changed_end,
    })
}

fn hollowed_bookmarks_after(
    tree: &XmlTree,
    assignments: &[TextAssignment],
    atoms: &[Atom],
    census: Option<&BookmarkCensus>,
) -> Vec<String> {
    if atoms.first().and_then(|a| a.paragraph).is_none() {
        return Vec::new();
    }
    let computed;
    let census = match census {
        Some(census) => census,
        None => {
            computed = bookmark_census(tree);
            &computed
        }
    };
    let planned: HashMap<NodeId, &str> = assignments
        .iter()
        .map(|a| (a.element, a.after.as_str()))
        .collect();
    let mut candidates: HashSet<usize> = HashSet::new();
    for assignment in assignments {
        if !assignment.before.is_empty()
            && assignment.after.is_empty()
            && let Some(ranges) = census.by_element.get(&assignment.element)
        {
            candidates.extend(ranges.iter().copied());
        }
    }
    census
        .ranges
        .iter()
        .enumerate()
        .filter(|(index, _)| candidates.contains(index))
        .filter(|(_, range)| {
            range
                .text_elements
                .iter()
                .all(|element| match planned.get(element) {
                    Some(after) => after.is_empty(),
                    None => leading_text(tree, *element).is_empty(),
                })
        })
        .map(|(_, range)| range.name.clone())
        .collect()
}

fn regional_text_assignments(
    tree: &XmlTree,
    span: &Span,
    new_text: &str,
    census: Option<&Census>,
) -> Result<(Vec<TextAssignment>, bool)> {
    if span.text == new_text {
        return Ok((Vec::new(), true));
    }
    let outcome = ordinary_outcome(tree, span, new_text)?;
    let changed_atoms: Vec<&Atom> = outcome
        .segments
        .iter()
        .map(|(index, _, _)| &span.atoms[*index])
        .collect();
    if changed_atoms.iter().any(|atom| atom.is_synthetic()) {
        return Err(Error::unsupported(
            "crosses_break",
            "the changed interval crosses a tab or line break, a no-break hyphen, or other non-text run content; \
             replace the text segments on either side individually",
        ));
    }
    let evidence = inline_ancestry_evidence(tree, &changed_atoms)?;
    let preserved = evidence[1..]
        .iter()
        .all(|item| same_destination(&evidence[0], item));
    let mut assignments = Vec::new();
    for (segment_index, (atom_index, start, end)) in outcome.segments.iter().enumerate() {
        let atom = &span.atoms[*atom_index];
        let text = chars(&atom.text(tree));
        let inserted = if segment_index == 0 {
            outcome.replacement.as_str()
        } else {
            ""
        };
        let after = format!(
            "{}{inserted}{}",
            collect(&text[..*start]),
            collect(&text[*end..])
        );
        let before = collect(&text);
        if after == before {
            continue;
        }
        let current_space = tree
            .attr(atom.element, Some(XML_NS), "space")
            .map(str::to_string);
        let edge_space = after.chars().next().is_some_and(is_py_space)
            || after.chars().last().is_some_and(is_py_space);
        assignments.push(TextAssignment {
            element: atom.element,
            before,
            after,
            after_space: if edge_space {
                Some("preserve".into())
            } else {
                current_space
            },
        });
    }
    let hollowed = hollowed_bookmarks_after(
        tree,
        &assignments,
        &span.atoms,
        census.map(|c| &c.bookmarks),
    );
    if !hollowed.is_empty() {
        return Err(hollows_bookmark_error(&hollowed));
    }
    refuse_intervening_positional_nodes(tree, &changed_atoms)?;
    Ok((assignments, preserved))
}

fn synthetic_positions(span: &Span, pieces: &[(Option<usize>, usize)]) -> HashSet<usize> {
    piece_ranges(pieces, span.start_offset)
        .filter(|(atom_index, _, _)| {
            atom_index.is_none_or(|index| span.atoms[index].is_synthetic())
        })
        .flat_map(|(_, piece, _)| piece)
        .collect()
}

/// Upstream `Span._narrow_to_change`.
pub(crate) fn narrow_to_change(
    tree: &XmlTree,
    span: &Span,
    new_text: &str,
) -> Result<Option<(Span, String)>> {
    let pieces = piece_lengths(&span.pieces(tree));
    let synthetic = synthetic_positions(span, &pieces);
    let old = chars(&span.text);
    let new = chars(new_text);
    let aligned =
        |o: usize, n: usize| old[o] == new[n] || (synthetic.contains(&o) && is_py_space(new[n]));
    let decompositions = maximal_affix_decompositions(&old, &new, aligned);
    if decompositions.len() != 1 {
        return Err(ambiguous_affix());
    }
    let (prefix, suffix) = decompositions[0];
    if prefix == 0 && suffix == 0 {
        return Ok(None);
    }
    let mut changed_old = collect(&old[prefix..old.len() - suffix]);
    let mut changed_new = collect(&new[prefix..new.len() - suffix]);
    if changed_old.is_empty() && changed_new.is_empty() {
        return Ok(None);
    }
    let target_start = prefix;
    let target_end = old.len() - suffix;
    let (start_index, start_offset, end_index, mut end_offset);
    if target_start == target_end {
        let mut candidates: Vec<(usize, usize)> = Vec::new();
        for (atom_index, piece, base) in piece_ranges(&pieces, span.start_offset) {
            if let Some(atom_index) = atom_index
                && piece.start <= target_start
                && target_start <= piece.end
                && !span.atoms[atom_index].is_synthetic()
            {
                candidates.push((atom_index, base + target_start - piece.start));
            }
        }
        let (Some(first), Some(last)) = (candidates.first(), candidates.last()) else {
            return Ok(None);
        };
        start_index = first.0;
        start_offset = first.1;
        end_index = last.0;
        end_offset = last.1;
    } else {
        let mut found_start: Option<(usize, usize)> = None;
        let mut found_end: Option<(usize, usize)> = None;
        for (atom_index, piece, base) in piece_ranges(&pieces, span.start_offset) {
            if found_start.is_none() && piece.end > target_start {
                let Some(index) = atom_index else {
                    return Ok(None);
                };
                found_start = Some((index, base + target_start - piece.start));
            }
            if piece.end >= target_end {
                match atom_index {
                    None => {
                        if found_end.is_none() {
                            return Ok(None);
                        }
                    }
                    Some(index) => found_end = Some((index, base + target_end - piece.start)),
                }
                break;
            }
        }
        let (Some(start), Some(end)) = (found_start, found_end) else {
            return Ok(None);
        };
        start_index = start.0;
        start_offset = start.1;
        end_index = end.0;
        end_offset = end.1;
    }
    let sub_atoms: Vec<Atom> = span.atoms[start_index..=end_index].to_vec();
    let sub_refs: Vec<&Atom> = sub_atoms.iter().collect();
    if !changed_new.is_empty()
        && !changed_old.is_empty()
        && has_multiple_direct_rpr(tree, &sub_refs)?
    {
        let mut expanded_end = 0;
        for (atom_index, piece, _) in piece_ranges(&pieces, span.start_offset) {
            expanded_end = piece.end;
            if atom_index == Some(end_index) {
                break;
            }
        }
        let suffix_to_keep = old.len() - expanded_end;
        changed_old = collect(&old[prefix..expanded_end]);
        changed_new = collect(&new[prefix..new.len() - suffix_to_keep]);
        end_offset = if end_index == span.atoms.len() - 1 {
            span.end_offset
        } else {
            sub_atoms[sub_atoms.len() - 1].text(tree).chars().count()
        };
        if prefix == 0 && expanded_end == old.len() {
            return Ok(None);
        }
    }
    let sequence_start = span
        .atom_sequence
        .iter()
        .position(|e| *e == sub_atoms[0].element);
    let sequence_end = span
        .atom_sequence
        .iter()
        .position(|e| *e == sub_atoms[sub_atoms.len() - 1].element);
    let atom_sequence = match (sequence_start, sequence_end) {
        (Some(start), Some(end)) if start <= end => span.atom_sequence[start..=end].to_vec(),
        _ => sub_atoms.iter().map(|a| a.element).collect(),
    };
    let first_paragraph = sub_atoms[0].paragraph;
    let crosses_paragraphs = sub_atoms.iter().any(|a| a.paragraph != first_paragraph);
    let sub = Span {
        incarnation: span.incarnation,
        crosses_paragraphs,
        atom_sequence,
        sequence_view: span.sequence_view,
        start_offset,
        end_offset,
        view: span.view,
        ..Span::from_atoms(
            tree,
            span.document,
            span.token,
            &span.story,
            &span.part,
            span.anchor.index,
            changed_old,
            sub_atoms,
        )
    };
    Ok(Some((sub, changed_new)))
}

/// Upstream `Span._replace` (untracked), as a pure plan.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_replace(
    document: &Document,
    tree: &XmlTree,
    span: &Span,
    new_text: &str,
    options: &ReplaceOptions,
    census: Option<&Census>,
    next_revision_id: i64,
    layering: Option<&Layering>,
) -> Result<Plan> {
    if options.tracked && options.preserve_revision {
        return Err(Error::invalid_argument(
            "tracked_with_preserve",
            "tracked=True cannot be combined with preserve_revision=True",
        ));
    }
    if options.tracked {
        let Some(author) = options.author.as_deref().filter(|a| !a.is_empty()) else {
            return Err(Error::invalid_argument(
                "author_required",
                "author is required when tracked=True",
            ));
        };
        validate_xml_characters(author, "author")?;
    }
    validate_writable_text(new_text, "new_text")?;
    refuse_if_protected(document, "replace text", OperationClass::BodyContent)?;
    if span.view != View::Current {
        return Err(Error::unsupported(
            "historical_view_target",
            format!(
                "span was captured under view={:?}, which is inspection-only; re-find the text with view='current' to edit it",
                span.view.as_str()
            ),
        ));
    }
    validate_fresh(document, tree, span, census)?;
    let computed_layering;
    let layering = if options.tracked {
        let layering = match layering {
            Some(layering) => layering,
            None => {
                computed_layering =
                    tracked_layering_context(tree, span, options.author.as_deref().unwrap_or(""))?;
                &computed_layering
            }
        };
        if span.text == new_text {
            return Err(stale(
                "replacement_equals_text",
                "replacement equals the existing text; nothing to change",
            ));
        }
        Some(layering)
    } else {
        None
    };
    if options.tracked || span.atoms.iter().any(Atom::is_synthetic) || span.crosses_paragraphs {
        validate_replaceable(tree, span, false)?;
        if let Some((sub, sub_new)) = narrow_to_change(tree, span, new_text)? {
            let mut plan = plan_replace(
                document,
                tree,
                &sub,
                &sub_new,
                options,
                census,
                next_revision_id,
                layering,
            )?;
            plan.consume = true;
            return Ok(plan);
        }
    }
    let preserved = preserved_insertion_ids(tree, span, options.preserve_revision)?;
    let preservation_noop = !preserved.is_empty() && new_text == span.text;
    validate_replaceable(tree, span, true)?;
    if options.tracked {
        let hollowed = hollowed_bookmarks(tree, &span.atoms, span.end_offset);
        if !hollowed.is_empty() {
            return Err(hollows_bookmark_error(&hollowed));
        }
    }
    if !options.tracked
        && span.atoms.iter().any(|atom| atom.in_insert)
        && !options.preserve_revision
    {
        return Err(Error::unsupported(
            "pending_insertion",
            "span intersects a pending tracked insertion; an untracked edit there would silently rewrite text the \
             revision history attributes to its author — accept or reject the revision first, or use tracked \
             replacement",
        ));
    }
    let placeholders = placeholder_controls(tree, &span.atoms);
    if options.tracked && !placeholders.is_empty() {
        return Err(Error::unsupported(
            "tracked_placeholder",
            "span lies in a form control still showing PLACEHOLDER prompt text; a tracked replacement would \
             fabricate a deletion of text that was never real content — fill the control untracked (or set the \
             control value) first",
        ));
    }
    for sdt in &placeholders {
        let prompt = placeholder_prompt_text(tree, *sdt);
        if span.text != prompt {
            return Err(Error::unsupported(
                "partial_placeholder",
                format!(
                    "span covers only part of a placeholder prompt; replacing it would promote the leftover prompt \
                     text to real content — replace the whole prompt ({prompt:?}) or set the control value"
                ),
            ));
        }
    }
    let report = |changed: bool, preserved_formatting: bool| ReplaceReport {
        story: span.story.clone(),
        deleted_text: span.text.clone(),
        inserted_text: new_text.to_string(),
        tracked: false,
        revision_ids: Vec::new(),
        preserved_revision_ids: preserved.clone(),
        preserved_formatting_regions: preserved_formatting,
        changed,
    };
    if preservation_noop {
        return Ok(Plan {
            report: report(false, true),
            assignments: Vec::new(),
            placeholders: Vec::new(),
            tracked: None,
            consume: false,
        });
    }
    if let Some(layering) = layering {
        let date = options.date.unwrap_or_else(crate::clock::Timestamp::now);
        return plan_tracked(
            tree,
            span,
            new_text,
            options.author.as_deref().unwrap_or(""),
            date,
            layering,
            next_revision_id,
        );
    }
    let (assignments, preserved_formatting) =
        regional_text_assignments(tree, span, new_text, census)?;
    let changed = !assignments.is_empty();
    Ok(Plan {
        report: report(changed, preserved_formatting),
        placeholders: if changed { placeholders } else { Vec::new() },
        tracked: None,
        consume: changed,
        assignments,
    })
}

fn hollows_bookmark_error(hollowed: &[String]) -> Error {
    Error::unsupported(
        "hollows_bookmark",
        format!(
            "replacing this span would hollow out bookmark(s) {hollowed:?} — the targets of REF/PAGEREF \
             cross-references and TOC entries; narrow the span to inside the bookmark, or remove the \
             bookmark deliberately first"
        ),
    )
}

/// Upstream `_tracked_layering_context`.
fn tracked_layering_context(tree: &XmlTree, span: &Span, author: &str) -> Result<Layering> {
    let enclosing: Vec<Option<NodeId>> = span
        .atoms
        .iter()
        .map(|a| enclosing_insertion(tree, a.element))
        .collect();
    let scopes: HashSet<Option<NodeId>> = enclosing.iter().copied().collect();
    if scopes.len() <= 1 {
        return Ok(Layering {
            enclosing,
            extends_own_insertion: false,
        });
    }
    let all_own = enclosing.iter().flatten().all(|element| {
        is_w(tree, *element, "ins") && w_attr(tree, *element, "author").unwrap_or("") == author
    });
    let mut inside_seen = false;
    let mut contiguous_tail = true;
    for element in &enclosing {
        if element.is_some() {
            inside_seen = true;
        } else if inside_seen {
            contiguous_tail = false;
            break;
        }
    }
    if !(all_own && enclosing[0].is_none() && contiguous_tail) {
        return Err(Error::unsupported(
            "tracked_layering",
            "span overlaps a pending tracked insertion it cannot layer over (different author, a tracked move, or \
             base text following the insertion); accept or reject the existing revision first, or target text \
             inside or outside it",
        ));
    }
    Ok(Layering {
        enclosing,
        extends_own_insertion: true,
    })
}

/// Upstream `Span._tracked_replace`, as a plan.
#[allow(clippy::too_many_arguments)]
fn plan_tracked(
    tree: &XmlTree,
    span: &Span,
    new_text: &str,
    author: &str,
    date: crate::clock::Timestamp,
    layering: &Layering,
    next_revision_id: i64,
) -> Result<Plan> {
    if span.story == "word/comments.xml" {
        return Err(Error::unsupported(
            "tracked_in_comment",
            "tracked changes inside comment text are not representable; edit the comment untracked or reply instead",
        ));
    }
    if span.atoms.iter().any(|atom| atom.run.is_none()) {
        return Err(Error::unsupported(
            "not_in_run",
            "matched text is not inside a run; cannot anchor a revision",
        ));
    }
    let outcome = ordinary_outcome(tree, span, new_text)?;
    let text = chars(&span.text);
    let prefix_len = outcome.changed_start;
    let suffix_len = text.len() - outcome.changed_end;
    let first = &span.atoms[0];
    let last = &span.atoms[span.atoms.len() - 1];
    let old_mid = collect(&text[outcome.changed_start..outcome.changed_end]);
    let new_mid = outcome.replacement.clone();
    let changed_atoms: Vec<&Atom> = outcome
        .segments
        .iter()
        .map(|(index, _, _)| &span.atoms[*index])
        .collect();
    refuse_intervening_positional_nodes(tree, &changed_atoms)?;
    validate_one_inline_ancestry(tree, &changed_atoms)?;
    let first_run = first
        .run
        .ok_or_else(|| Error::unsupported("not_in_run", "matched text is not inside a run"))?;
    let rpr = w_child(tree, first_run, "rPr");
    let span_pieces: Vec<(Option<NodeId>, String)> = span
        .pieces(tree)
        .into_iter()
        .filter_map(|(index, piece)| index.map(|index| (span.atoms[index].run, piece)))
        .collect();
    let mut deleted_pieces = pieces_in_range(&span_pieces, prefix_len, text.len() - suffix_len);
    let ins_source_run = changed_atoms.first().and_then(|a| a.run).or(first.run);
    let ins_rpr = ins_source_run.and_then(|run| w_child(tree, run, "rPr"));
    let first_full = chars(&first.text(tree));
    let last_full = chars(&last.text(tree));
    let before = collect(&first_full[..span.start_offset.min(first_full.len())]);
    let after = collect(&last_full[span.end_offset.min(last_full.len())..]);
    let prefix = collect(&text[..prefix_len]);
    let suffix = if suffix_len > 0 {
        collect(&text[text.len() - suffix_len..])
    } else {
        String::new()
    };
    let first_is_last = span.atoms.len() == 1;
    let same_run = first.run == last.run;
    let tail_in_new_run = first_is_last || same_run;
    let middle_elements: Vec<NodeId> = if span.atoms.len() > 2 {
        span.atoms[1..span.atoms.len() - 1]
            .iter()
            .map(|a| a.element)
            .collect()
    } else {
        Vec::new()
    };
    let (last_element, last_text) = if !tail_in_new_run {
        (Some(last.element), Some(format!("{suffix}{after}")))
    } else if !first_is_last {
        (Some(last.element), Some(String::new()))
    } else {
        (None, None)
    };
    if layering.extends_own_insertion {
        deleted_pieces.retain(|(source_run, _)| {
            source_run.is_none_or(|run| enclosing_insertion(tree, run).is_none())
        });
    }
    let mut next_id = next_revision_id;
    let mut revision_ids = Vec::new();
    let del_id = if !old_mid.is_empty() && !deleted_pieces.is_empty() {
        revision_ids.push(next_id);
        next_id += 1;
        Some(next_id - 1)
    } else {
        None
    };
    let ins_id = if !new_mid.is_empty() {
        revision_ids.push(next_id);
        Some(next_id)
    } else {
        None
    };
    let mut own_insertions: Vec<NodeId> = Vec::new();
    if layering.extends_own_insertion {
        for element in layering.enclosing.iter().flatten() {
            if !own_insertions.contains(element) {
                own_insertions.push(*element);
            }
        }
    }
    Ok(Plan {
        report: ReplaceReport {
            story: span.story.clone(),
            deleted_text: old_mid.clone(),
            inserted_text: new_mid.clone(),
            tracked: true,
            revision_ids,
            preserved_revision_ids: Vec::new(),
            preserved_formatting_regions: false,
            changed: true,
        },
        assignments: Vec::new(),
        placeholders: Vec::new(),
        tracked: Some(TrackedPlan {
            first_element: first.element,
            first_run,
            first_text: format!("{before}{prefix}"),
            middle_elements,
            last_element,
            last_text,
            same_run_tail: if tail_in_new_run {
                Some(format!("{suffix}{after}"))
            } else {
                None
            },
            rpr,
            ins_rpr,
            deleted_pieces,
            new_mid,
            del_id,
            ins_id,
            author: author.to_string(),
            date: date.to_xml(),
            own_insertions,
        }),
        consume: true,
    })
}

fn apply_tracked(tree: &mut XmlTree, plan: &TrackedPlan) -> Result<()> {
    set_preserved_text(tree, plan.first_element, &plan.first_text)?;
    for element in &plan.middle_elements {
        set_preserved_text(tree, *element, "")?;
    }
    if let (Some(element), Some(text)) = (plan.last_element, plan.last_text.as_deref()) {
        set_preserved_text(tree, element, text)?;
    }
    let mut past = false;
    let mut trailing = Vec::new();
    for child in tree.children(plan.first_run).to_vec() {
        if past && !is_w(tree, child, "rPr") && !matches!(tree.data(child), NodeData::Text(_)) {
            trailing.push(child);
        }
        if child == plan.first_element {
            past = true;
        }
    }
    let moved_run = if trailing.is_empty() {
        None
    } else {
        let run = crate::oxml::w_el(tree, plan.first_run, "r")?;
        if let Some(rpr) = plan.rpr {
            let copy = tree.deep_clone(rpr);
            crate::oxml::append(tree, run, copy)?;
        }
        for element in trailing {
            tree.detach(element)?;
            crate::oxml::append(tree, run, element)?;
        }
        Some(run)
    };
    let mut point = plan.first_run;
    if let Some(id) = plan.del_id {
        let deletion = new_revision(tree, plan.first_run, "del", id, &plan.author, &plan.date)?;
        for (source_run, piece) in &plan.deleted_pieces {
            let source_rpr = source_run.and_then(|run| w_child(tree, run, "rPr"));
            add_tracked_run(tree, deletion, piece, source_rpr, true)?;
        }
        insert_after(tree, point, deletion)?;
        point = deletion;
    }
    if let Some(id) = plan.ins_id {
        let insertion = new_revision(tree, plan.first_run, "ins", id, &plan.author, &plan.date)?;
        add_tracked_run(tree, insertion, &plan.new_mid, plan.ins_rpr, false)?;
        insert_after(tree, point, insertion)?;
        point = insertion;
    }
    if let Some(tail) = plan.same_run_tail.as_deref().filter(|t| !t.is_empty()) {
        let run = crate::oxml::w_el(tree, plan.first_run, "r")?;
        if let Some(rpr) = plan.rpr {
            let copy = tree.deep_clone(rpr);
            crate::oxml::append(tree, run, copy)?;
        }
        crate::oxml::add_t(tree, run, tail)?;
        insert_after(tree, point, run)?;
        point = run;
    }
    if let Some(run) = moved_run {
        insert_after(tree, point, run)?;
    }
    for own in &plan.own_insertions {
        if !tree.is_attached(*own) {
            continue;
        }
        let has_text = !tree.text_content(*own).is_empty();
        let has_object = tree
            .descendants(*own)
            .any(|n| is_w(tree, n, "drawing") || is_w(tree, n, "object"));
        if !has_text && !has_object {
            tree.detach(*own)?;
        }
    }
    Ok(())
}

pub(crate) fn apply_plan(tree: &mut XmlTree, plan: &Plan) -> Result<()> {
    if let Some(tracked) = &plan.tracked {
        return apply_tracked(tree, tracked);
    }
    for assignment in &plan.assignments {
        wml::set_leading_text(tree, assignment.element, &assignment.after)?;
        match &assignment.after_space {
            None => {
                tree.remove_attr(assignment.element, Some(XML_NS), "space");
            }
            Some(value) => tree.set_attr(assignment.element, Some(XML_NS), "space", value)?,
        }
    }
    for sdt in &plan.placeholders {
        clear_placeholder_state(tree, *sdt)?;
    }
    Ok(())
}

fn require_owner(document: &Document, span: &Span) -> Result<()> {
    if span.document != document.id() {
        return Err(boundary(
            "foreign_span",
            "span belongs to a different document; re-find the text in the document passed to this operation",
        ));
    }
    Ok(())
}

/// Replace one span (see [`Document::replace`]).
pub(crate) fn replace(
    document: &mut Document,
    span: &Span,
    new_text: &str,
    options: &ReplaceOptions,
) -> Result<ReplaceReport> {
    require_owner(document, span)?;
    let tree = document.package().xml(&span.part)?;
    let next_id = if options.tracked {
        next_revision_id(document, &BTreeMap::new())?
    } else {
        1
    };
    let plan = plan_replace(document, tree, span, new_text, options, None, next_id, None)?;
    if !plan.mutates() {
        return Ok(plan.report);
    }
    let mut staged = tree.clone();
    apply_plan(&mut staged, &plan)?;
    validate_staged(&staged, &span.part)?;
    inject_after_stage()?;
    inject_before_commit()?;
    document.package_mut().commit_tree(&span.part, staged)?;
    if plan.consume {
        document.consume(span.token);
    }
    Ok(plan.report)
}

/// The staged tree must serialize to XML this crate re-reads identically.
fn validate_staged(staged: &XmlTree, part: &str) -> Result<()> {
    let bytes = xml::serialize(staged);
    let reparsed = xml::parse(&bytes, &crate::Limits::trusted()).map_err(|err| {
        Error::internal(
            "staged_reparse",
            format!("staged {part} does not reparse: {err}"),
        )
    })?;
    if !xml::trees_equivalent(staged, &reparsed) {
        return Err(Error::internal(
            "staged_roundtrip",
            format!("staged {part} does not round-trip"),
        ));
    }
    Ok(())
}

/// Options for [`Document::replace_all`].
#[derive(Debug, Clone)]
pub struct ReplaceAllOptions {
    /// Match policy.
    pub policy: MatchPolicy,
    /// View (only `Current` targets are editable).
    pub view: View,
    /// Restrict to one story.
    pub story: Option<String>,
    /// Per-match replacement options.
    pub replace: ReplaceOptions,
}

impl Default for ReplaceAllOptions {
    fn default() -> Self {
        ReplaceAllOptions {
            policy: MatchPolicy::Exact,
            view: View::Current,
            story: None,
            replace: ReplaceOptions::default(),
        }
    }
}

/// Replace every match in one atomic pass (see [`Document::replace_all`]).
pub(crate) fn replace_all(
    document: &mut Document,
    needle: &str,
    new_text: &str,
    options: &ReplaceAllOptions,
) -> Result<ReplaceAllReport> {
    if options.replace.tracked && options.replace.preserve_revision {
        return Err(Error::invalid_argument(
            "tracked_with_preserve",
            "tracked=True cannot be combined with preserve_revision=True",
        ));
    }
    if options.replace.tracked && options.replace.author.as_deref().is_none_or(str::is_empty) {
        return Err(Error::invalid_argument(
            "author_required",
            "author is required when tracked=True",
        ));
    }
    validate_writable_text(new_text, "new_text")?;
    refuse_if_protected(document, "replace text", OperationClass::BodyContent)?;
    let find_options = FindOptions {
        policy: options.policy,
        view: options.view,
        story: options.story.clone(),
        nth: None,
        near: None,
    };
    let spans: Vec<Span> = find_text(document, needle, &find_options)?
        .into_iter()
        .filter(|span| span.text != new_text)
        .collect();
    let mut by_story: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for span in spans {
        by_story.entry(span.story.clone()).or_default().push(span);
    }
    let mut staged: BTreeMap<String, XmlTree> = BTreeMap::new();
    let mut next_id = if options.replace.tracked {
        next_revision_id(document, &BTreeMap::new())?
    } else {
        1
    };
    let mut results = Vec::new();
    let mut refused = Vec::new();
    for (story_name, mut story_spans) in by_story {
        let part = story_spans[0].part.clone();
        let live = document.package().xml(&part)?;
        let census = Census::build(live, &story_name)?;
        let mut working = live.clone();
        story_spans.sort_by_key(|span| std::cmp::Reverse(span.raw_start));
        let mut touched = false;
        for span in &story_spans {
            match plan_replace(
                document,
                &working,
                span,
                new_text,
                &options.replace,
                Some(&census),
                next_id,
                None,
            ) {
                Ok(plan) => {
                    if plan.mutates() {
                        apply_plan(&mut working, &plan)?;
                        touched = true;
                    }
                    if let Some(highest) = plan.report.revision_ids.iter().max() {
                        next_id = next_id.max(highest + 1);
                    }
                    results.push(plan.report);
                }
                Err(err) if err.kind() == ErrorKind::Refusal(RefusalKind::TargetNotFound) => {
                    return Err(err);
                }
                Err(err) if err.is_refusal() => {
                    let kind = err
                        .refusal_kind()
                        .unwrap_or(RefusalKind::UnsupportedStructure);
                    refused.push(RefusedMatch {
                        story: span.story.clone(),
                        anchor: span.anchor.clone(),
                        code: err.code(),
                        error: kind.upstream_class(),
                        reason: err.reason(),
                        message: err.message().to_string(),
                    });
                }
                Err(err) => return Err(err),
            }
        }
        if touched {
            validate_staged(&working, &part)?;
            staged.insert(part, working);
        }
    }
    inject_after_stage()?;
    inject_before_commit()?;
    for (part, tree) in staged {
        document.package_mut().commit_tree(&part, tree)?;
    }
    Ok(ReplaceAllReport {
        replaced_count: results.len(),
        results,
        refused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affix_decompositions_match_reference() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        let (old, new) = (c("payment"), c("settlement"));
        let d = maximal_affix_decompositions(&old, &new, |o, n| old[o] == new[n]);
        assert_eq!(d, vec![(0, 4)]);
        let (old, new) = (c("aa"), c("aaa"));
        let d = maximal_affix_decompositions(&old, &new, |o, n| old[o] == new[n]);
        assert_eq!(d, vec![(0, 2), (1, 1), (2, 0)]);
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int("x"), None);
    }
}
