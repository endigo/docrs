//! Compare: a native tracked-changes redline (upstream `_compare.py`).
//!
//! `compare(original, revised)` returns the original with `w:ins`/`w:del`
//! markup that transforms it into the revised document. The algebra is the
//! contract — `accept_all(compare(A, B)) == B`, `reject_all(compare(A, B)) ==
//! A`, `compare(A, A)` emits nothing, and fixed inputs (with a fixed date)
//! produce identical output. Everything compare cannot express as a clean
//! redline refuses with `unsupported_structure` (declared limits: insertions
//! and deletions only; no move or format synthesis; no cross-story moves;
//! whole-block add/remove only beside story-container blocks; changed merged
//! rows, objects, fields, controls, package-part changes and formatting
//! differences refuse; block and sequence budgets refuse before quadratic
//! work). The original is gated like any writer: Restrict Editing that
//! forbids body edits refuses unless [`CompareOptions::acknowledge_protection`]
//! is set.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;

use crate::clock::Timestamp;
use crate::difflib::{SequenceMatcher, Tag};
use crate::normalize::content_hash;
use crate::oxml::{self, insert_before_successors, w_el};
use crate::package::ct;
use crate::package::is_xml_part_name;
use crate::package::patch::{effective_content_types, parts_semantically_equal};
use crate::package::rels::{RELS_NS, base_uri, rels_member_for, resolve_target};
use crate::package::zip_read::read_archive;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::{ReplaceOptions, apply_plan, plan_replace, validate_writable_text};
use crate::search::{Atom, Span, include_atom, new_span_token, signature, story_atoms};
use crate::story::{self, Anchor, BlockKind, View, iter_block_elements, subtree_text, table_text};
use crate::tracked::{insert_after, insert_before, next_revision_id};
use crate::txn::Txn;
use crate::unicode::{casefold, is_py_space};
use crate::wml::{W, is_w, iter_nodes, leading_text, node_children, w_attr, w_child, w_children};
use crate::xml::{self, NodeData, NodeId, XML_NS, XmlTree};
use crate::{Document, Error, Limits, RefusalKind, Result};

/// Per-story block budget.
pub const MAX_BLOCKS: usize = 10_000;
/// Block/row sequence-matching budget (cells).
pub const MAX_SEQUENCE_CELLS: usize = 1_000_000;
/// Token sequence-matching budget (cells).
pub const MAX_TEXT_SEQUENCE_CELLS: usize = 1_000_000;

/// Comparison granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// Token-level edits inside matched paragraphs (default).
    Word,
    /// Whole-block replacements.
    Block,
}

/// How pending revisions in the inputs are materialized first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Materialize {
    /// Accept them on a working copy.
    Accept,
    /// Reject them on a working copy.
    Reject,
}

/// Compare options.
#[derive(Debug, Clone)]
pub struct CompareOptions {
    /// Revision author (required).
    pub author: String,
    /// Revision date (`None`: now).
    pub date: Option<Timestamp>,
    /// Granularity.
    pub granularity: Granularity,
    /// Resolve pending input revisions first.
    pub materialize: Option<Materialize>,
    /// Redline an original under Restrict Editing that forbids body edits
    /// (see [`crate::Document::acknowledge_protection`]); the redline keeps
    /// the acknowledgment.
    pub acknowledge_protection: bool,
}

/// Longest `author` compare writes into `w:author`, in characters.
pub const MAX_AUTHOR_CHARS: usize = 255;

/// A difference compare reports instead of redlining.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompareFinding {
    /// Finding kind.
    pub kind: String,
    /// Story or part.
    pub story: String,
    /// Detail.
    pub detail: String,
}

/// The redline plus what compare reported.
pub struct CompareResult {
    /// The redlined document (the original with tracked changes).
    pub document: Document,
    /// Reported findings.
    pub findings: Vec<CompareFinding>,
    /// Revisions in the redline.
    pub revision_count: usize,
    /// Stories compared.
    pub stories: Vec<String>,
}

impl CompareResult {
    /// Upstream `to_dict()` (`schema: paper_compare`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_compare",
            "version": 1,
            "revision_count": self.revision_count,
            "stories": self.stories,
            "findings": self.findings,
        })
    }
}

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

/// Tags upstream `Document.revisions` enumerates (`_REVISION_TYPES`).
const REVISION_TAGS: &[&str] = &[
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    "rPrChange",
    "pPrChange",
    "tblPrChange",
    "tblPrExChange",
    "tblGridChange",
    "trPrChange",
    "tcPrChange",
    "cellIns",
    "cellDel",
    "cellMerge",
    "sectPrChange",
    "numberingChange",
    "customXmlInsRangeStart",
    "customXmlDelRangeStart",
    "customXmlMoveFromRangeStart",
    "customXmlMoveToRangeStart",
];

/// Upstream `_MARKUP_SCAN_TAGS`.
const MARKUP_SCAN_EXTRA: &[&str] = &[
    "moveFromRangeStart",
    "moveFromRangeEnd",
    "moveToRangeStart",
    "moveToRangeEnd",
    "customXmlInsRangeEnd",
    "customXmlDelRangeEnd",
    "customXmlMoveFromRangeEnd",
    "customXmlMoveToRangeEnd",
];

fn is_markup(tree: &XmlTree, node: NodeId) -> bool {
    tree.ns_uri(node) == Some(W)
        && tree.local_name(node).is_some_and(|local| {
            REVISION_TAGS.contains(&local) || MARKUP_SCAN_EXTRA.contains(&local)
        })
}

struct RawPackage {
    parts: BTreeMap<String, Vec<u8>>,
    order: Vec<String>,
    story_parts: BTreeSet<String>,
    has_revisions: bool,
}

fn read_raw_package(bytes: &[u8], label: &str, limits: &Limits) -> Result<RawPackage> {
    let contents = read_archive(bytes, limits)?;
    let order: Vec<String> = contents.members.iter().map(|m| m.name.clone()).collect();
    let parts: BTreeMap<String, Vec<u8>> = contents
        .members
        .into_iter()
        .map(|m| (m.name, m.data))
        .collect();
    let unsafe_graph = |detail: String| {
        unsupported(
            "raw_graph",
            format!("the {label} package graph cannot be validated safely: {detail}"),
        )
    };
    let reachable = reachable_raw_parts(&parts, label)?;
    let unreachable: Vec<&String> = parts
        .keys()
        .filter(|name| !reachable.contains(*name))
        .collect();
    if !unreachable.is_empty() {
        return Err(unsupported(
            "unreachable_parts",
            format!(
                "the {label} contains unreachable package part(s) {unreachable:?}; compare cannot preserve parts omitted by the OPC relationship graph"
            ),
        ));
    }
    let Some(content_types) = parts.get("[Content_Types].xml") else {
        return Err(unsupported(
            "no_content_types",
            format!("the {label} package has no [Content_Types].xml part"),
        ));
    };
    let order_refs: Vec<&str> = order.iter().map(String::as_str).collect();
    let (defaults, raw_overrides) = effective_content_types(content_types, &order_refs, limits)
        .ok_or_else(|| unsafe_graph("[Content_Types].xml does not parse".into()))?;
    let overrides: HashMap<String, String> = raw_overrides
        .into_iter()
        .map(|(k, v)| (casefold(&k), v))
        .collect();
    let content_type = |name: &str| -> String {
        if let Some(found) = overrides.get(&casefold(&format!("/{name}"))) {
            return found.clone();
        }
        let ext = if name.contains('.') {
            name.rsplit('.').next().unwrap_or("").to_lowercase()
        } else {
            String::new()
        };
        defaults.get(&ext).cloned().unwrap_or_default()
    };
    let supported = [
        ct::WML_DOCUMENT_MAIN,
        ct::WML_ENDNOTES,
        ct::WML_FOOTER,
        ct::WML_FOOTNOTES,
        ct::WML_HEADER,
    ];
    let mut story_parts = BTreeSet::new();
    let mut revision_parts = BTreeSet::new();
    for name in &reachable {
        let ct_value = content_type(name);
        if supported.contains(&ct_value.as_str()) {
            story_parts.insert(name.clone());
            revision_parts.insert(name.clone());
        } else if ct_value == ct::WML_COMMENTS {
            revision_parts.insert(name.clone());
        }
    }
    let mut has_revisions = false;
    for name in &revision_parts {
        let tree = xml::parse(&parts[name], limits).map_err(|err| unsafe_graph(err.to_string()))?;
        if tree
            .descendants(tree.root())
            .any(|node| is_markup(&tree, node))
        {
            has_revisions = true;
            break;
        }
    }
    Ok(RawPackage {
        parts,
        order,
        story_parts,
        has_revisions,
    })
}

fn reachable_raw_parts(parts: &BTreeMap<String, Vec<u8>>, label: &str) -> Result<BTreeSet<String>> {
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    reachable.insert("[Content_Types].xml".into());
    let by_fold: HashMap<String, &String> =
        parts.keys().map(|name| (casefold(name), name)).collect();
    let mut pending = vec!["/".to_string()];
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(source) = pending.pop() {
        if !visited.insert(source.clone()) {
            continue;
        }
        let rels_name = rels_member_for(&source);
        let Some(actual_rels) = by_fold.get(&casefold(&rels_name)).copied() else {
            continue;
        };
        reachable.insert(actual_rels.clone());
        let tree = xml::parse(&parts[actual_rels], &Limits::default()).map_err(|err| {
            unsupported(
                "raw_graph",
                format!("the {label} package graph cannot be validated safely: {err}"),
            )
        })?;
        let root = tree.root();
        if !tree.is(root, RELS_NS, "Relationships") {
            return Err(unsupported(
                "raw_graph",
                format!("relationship part {actual_rels:?} has unexpected root"),
            ));
        }
        let mut ids: HashSet<String> = HashSet::new();
        for rel in tree.child_elements(root) {
            if !tree.is(rel, RELS_NS, "Relationship") {
                continue;
            }
            let Some(id) = tree.attr(rel, None, "Id") else {
                return Err(unsupported(
                    "raw_graph",
                    format!(
                        "relationship part {rels_name:?} contains a relationship with a missing Id"
                    ),
                ));
            };
            let normalized = id.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized.is_empty() {
                return Err(unsupported(
                    "raw_graph",
                    format!(
                        "relationship part {rels_name:?} contains a relationship with an empty Id"
                    ),
                ));
            }
            if !ids.insert(normalized) {
                return Err(unsupported(
                    "raw_graph",
                    format!(
                        "relationship part {rels_name:?} contains duplicate relationship Id {id:?}"
                    ),
                ));
            }
            if tree.attr(rel, None, "TargetMode").unwrap_or("Internal") == "External" {
                continue;
            }
            let Some(target) = tree.attr(rel, None, "Target").filter(|t| !t.is_empty()) else {
                return Err(unsupported(
                    "raw_graph",
                    format!(
                        "relationship part {rels_name:?} contains an internal relationship without a target"
                    ),
                ));
            };
            let target_name = resolve_target(base_uri(&source), target);
            let member = target_name.trim_start_matches('/');
            let Some(actual) = by_fold.get(&casefold(member)).copied() else {
                return Err(unsupported(
                    "raw_graph",
                    format!("relationship part {actual_rels:?} targets missing part {member:?}"),
                ));
            };
            if reachable.insert(actual.clone()) {
                pending.push(format!("/{actual}"));
            }
        }
    }
    Ok(reachable)
}

fn refuse_unsupported_package_differences(
    original: &RawPackage,
    revised: &RawPackage,
    limits: &Limits,
) -> Result<()> {
    let added: Vec<&String> = revised
        .parts
        .keys()
        .filter(|n| !original.parts.contains_key(*n))
        .collect();
    let removed: Vec<&String> = original
        .parts
        .keys()
        .filter(|n| !revised.parts.contains_key(*n))
        .collect();
    if !added.is_empty() || !removed.is_empty() {
        return Err(unsupported(
            "package_parts_differ",
            format!(
                "compare cannot redline package-part addition or removal (only-original: {removed:?}, only-revised: {added:?})"
            ),
        ));
    }
    let a_order: Vec<&str> = original.order.iter().map(String::as_str).collect();
    let b_order: Vec<&str> = revised.order.iter().map(String::as_str).collect();
    for (name, before) in &original.parts {
        let after = &revised.parts[name];
        if before == after {
            continue;
        }
        if original.story_parts.contains(name) && revised.story_parts.contains(name) {
            continue;
        }
        if is_xml_part_name(name)
            && parts_semantically_equal(name, before, after, &a_order, &b_order, limits)
        {
            continue;
        }
        return Err(unsupported(
            "package_part_changed",
            format!(
                "compare cannot redline a change in package part {name:?}; make that change separately or compare documents whose non-story parts are equivalent"
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum CNode {
    Elem {
        ns: Option<String>,
        local: String,
        lexical: String,
        decls: Vec<(Option<String>, Option<String>)>,
        attrs: Vec<(String, Option<String>, String, String)>,
        children: Vec<CNode>,
    },
    Text(String),
    Other(String),
}

impl CNode {
    fn is(&self, local_name: &str) -> bool {
        matches!(self, CNode::Elem { ns, local, .. } if ns.as_deref() == Some(W) && local == local_name)
    }
}

fn to_cnode(tree: &XmlTree, node: NodeId) -> CNode {
    match tree.data(node) {
        NodeData::Text(text) => CNode::Text(text.clone()),
        NodeData::Comment(text) => CNode::Other(format!("<!--{text}-->")),
        NodeData::Pi { target, data } => CNode::Other(format!("<?{target} {data}?>")),
        NodeData::Element(element) => CNode::Elem {
            ns: element.ns.map(|ns| tree.uri(ns).to_string()),
            local: element.name.local.to_string(),
            lexical: element.name.lexical(),
            decls: element
                .ns_decls
                .iter()
                .map(|d| {
                    (
                        d.prefix.as_deref().map(str::to_string),
                        d.uri.map(|u| tree.uri(u).to_string()),
                    )
                })
                .collect(),
            attrs: element
                .attrs
                .iter()
                .map(|a| {
                    (
                        a.name.lexical(),
                        a.ns.map(|ns| tree.uri(ns).to_string()),
                        a.name.local.to_string(),
                        a.value.clone(),
                    )
                })
                .collect(),
            children: tree
                .children(node)
                .iter()
                .map(|c| to_cnode(tree, *c))
                .collect(),
        },
    }
}

fn cnode_text(node: &CNode) -> String {
    match node {
        CNode::Elem { children, .. } => children
            .iter()
            .filter_map(|c| {
                if let CNode::Text(t) = c {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .take(1)
            .collect(),
        _ => String::new(),
    }
}

/// lxml `.text`: leading character data.
fn leading(children: &[CNode]) -> String {
    let mut out = String::new();
    for child in children {
        match child {
            CNode::Text(text) => out.push_str(text),
            _ => break,
        }
    }
    out
}

fn serialize_cnode(node: &CNode, out: &mut String) {
    match node {
        CNode::Text(text) => {
            out.push_str(
                &text
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;"),
            );
        }
        CNode::Other(text) => out.push_str(text),
        CNode::Elem {
            lexical,
            decls,
            attrs,
            children,
            ..
        } => {
            out.push('<');
            out.push_str(lexical);
            for (prefix, uri) in decls {
                out.push_str(&format!(
                    " xmlns{}=\"{}\"",
                    prefix
                        .as_deref()
                        .map(|p| format!(":{p}"))
                        .unwrap_or_default(),
                    uri.as_deref().unwrap_or("")
                ));
            }
            for (name, _, _, value) in attrs {
                out.push_str(&format!(" {name}=\"{value}\""));
            }
            if children.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                for child in children {
                    serialize_cnode(child, out);
                }
                out.push_str(&format!("</{lexical}>"));
            }
        }
    }
}

fn rpr_bytes(children: &[CNode]) -> String {
    let mut out = String::new();
    if let Some(rpr) = children.iter().find(|c| c.is("rPr")) {
        serialize_cnode(rpr, &mut out);
    }
    out
}

fn run_shape(node: &CNode) -> Option<String> {
    let CNode::Elem { children, .. } = node else {
        return None;
    };
    let elements: Vec<&CNode> = children
        .iter()
        .filter(|c| !matches!(c, CNode::Text(_)))
        .collect();
    if !elements.iter().any(|c| c.is("t")) {
        return None;
    }
    let safe = ["rPr", "t", "tab", "br", "cr", "noBreakHyphen"];
    if elements.iter().any(|c| !safe.iter().any(|s| c.is(s))) {
        return None;
    }
    Some(rpr_bytes(children))
}

fn same_attrs(left: &CNode, right: &CNode) -> bool {
    let (CNode::Elem { attrs: a, .. }, CNode::Elem { attrs: b, .. }) = (left, right) else {
        return false;
    };
    let key = |attrs: &Vec<(String, Option<String>, String, String)>| {
        let mut v: Vec<(Option<String>, String, String)> = attrs
            .iter()
            .map(|(_, ns, l, v)| (ns.clone(), l.clone(), v.clone()))
            .collect();
        v.sort();
        v
    };
    key(a) == key(b)
}

fn mergeable(left: &CNode, right: &CNode) -> bool {
    if !left.is("r") || !right.is("r") || !same_attrs(left, right) {
        return false;
    }
    let shape = run_shape(left);
    shape.is_some() && shape == run_shape(right)
}

fn canonicalize(node: &mut CNode) {
    fn strip(node: &mut CNode) {
        if let CNode::Elem { children, .. } = node {
            children.retain(|c| !c.is("proofErr"));
            for child in children.iter_mut() {
                strip(child);
            }
            children.retain(|c| {
                if (c.is("pPr") || c.is("rPr"))
                    && let CNode::Elem {
                        attrs, children, ..
                    } = c
                {
                    return !(attrs.is_empty() && children.is_empty());
                }
                true
            });
        }
    }
    strip(node);
    fn merge(node: &mut CNode) {
        if let CNode::Elem { children, .. } = node {
            let mut index = 0;
            while index + 1 < children.len() {
                // lxml indexes element children only; text nodes are tails.
                let next =
                    (index + 1..children.len()).find(|i| !matches!(children[*i], CNode::Text(_)));
                let Some(next) = next else { break };
                if matches!(children[index], CNode::Text(_)) {
                    index += 1;
                    continue;
                }
                if !mergeable(&children[index], &children[next]) {
                    index = next;
                    continue;
                }
                let right = children.remove(next);
                if let (
                    CNode::Elem {
                        children: left_children,
                        ..
                    },
                    CNode::Elem {
                        children: right_children,
                        ..
                    },
                ) = (&mut children[index], right)
                {
                    left_children.extend(right_children.into_iter().filter(|c| !c.is("rPr")));
                }
            }
            for child in children.iter_mut() {
                merge(child);
            }
            if node.is("r")
                && let CNode::Elem { children, .. } = node
            {
                let mut index = 0;
                while index + 1 < children.len() {
                    if children[index].is("t") && children[index + 1].is("t") {
                        let right = children.remove(index + 1);
                        let joined =
                            format!("{}{}", cnode_text(&children[index]), cnode_text(&right));
                        if let CNode::Elem {
                            children: t_children,
                            ..
                        } = &mut children[index]
                        {
                            t_children.retain(|c| !matches!(c, CNode::Text(_)));
                            t_children.insert(0, CNode::Text(joined));
                        }
                    } else {
                        index += 1;
                    }
                }
            }
        }
    }
    merge(node);
    fn spaces(node: &mut CNode) {
        let is_t = node.is("t");
        if let CNode::Elem {
            attrs, children, ..
        } = node
        {
            if is_t {
                let value = leading(children);
                let edge = value.chars().next().is_some_and(is_py_space)
                    || value.chars().last().is_some_and(is_py_space);
                attrs.retain(|(_, ns, local, _)| {
                    !(ns.as_deref() == Some(XML_NS) && local == "space")
                });
                if edge {
                    attrs.push((
                        "xml:space".into(),
                        Some(XML_NS.into()),
                        "space".into(),
                        "preserve".into(),
                    ));
                }
            }
            for child in children.iter_mut() {
                spaces(child);
            }
        }
    }
    spaces(node);
}

fn used_namespaces(node: &CNode, out: &mut Vec<(String, String)>) {
    if let CNode::Elem {
        lexical,
        ns,
        attrs,
        children,
        ..
    } = node
    {
        if let (Some((prefix, _)), Some(uri)) = (lexical.split_once(':'), ns)
            && !out.iter().any(|(p, _)| p == prefix)
        {
            out.push((prefix.to_string(), uri.clone()));
        }
        for (name, ns, _, _) in attrs {
            if let (Some((prefix, _)), Some(uri)) = (name.split_once(':'), ns)
                && prefix != "xml"
                && !out.iter().any(|(p, _)| p == prefix)
            {
                out.push((prefix.to_string(), uri.clone()));
            }
        }
        for child in children {
            used_namespaces(child, out);
        }
    }
}

fn canonical_cnode(mut node: CNode) -> String {
    canonicalize(&mut node);
    let mut used = Vec::new();
    used_namespaces(&node, &mut used);
    let mut out = format!("{used:?}");
    serialize_cnode(&node, &mut out);
    out
}

/// Canonical story form for semantically inert run splitting.
fn canonical_story_bytes(tree: &XmlTree, node: NodeId) -> String {
    canonical_cnode(to_cnode(tree, node))
}

fn textless_story_bytes(tree: &XmlTree, node: NodeId) -> String {
    fn blank(node: &mut CNode) {
        let is_text = ["t", "delText", "instrText", "delInstrText"]
            .iter()
            .any(|t| node.is(t));
        if let CNode::Elem {
            attrs, children, ..
        } = node
        {
            if is_text {
                let rest: Vec<CNode> = children
                    .iter()
                    .skip_while(|c| matches!(c, CNode::Text(_)))
                    .cloned()
                    .collect();
                *children = rest;
                attrs.retain(|(_, ns, local, _)| {
                    !(ns.as_deref() == Some(XML_NS) && local == "space")
                });
            }
            for child in children.iter_mut() {
                blank(child);
            }
        }
    }
    let mut node = to_cnode(tree, node);
    blank(&mut node);
    canonical_cnode(node)
}

struct Ctx<'d> {
    document: &'d Document,
    author: String,
    stamp: String,
    stamp_ts: Timestamp,
    granularity: Granularity,
    story: String,
    part: String,
    numbering_ids: HashSet<i64>,
    id_cell: i64,
    finished: BTreeMap<String, XmlTree>,
}

type StoryBlock = (BlockKind, NodeId, String);

fn story_blocks(tree: &XmlTree, story: &str) -> Result<Vec<StoryBlock>> {
    Ok(iter_block_elements(tree, story)?
        .into_iter()
        .map(|block| {
            let text = match block.kind {
                BlockKind::Table => table_text(tree, block.element, View::Current),
                BlockKind::Paragraph => {
                    subtree_text(
                        tree,
                        block.element,
                        View::Current,
                        true,
                        block.in_sdt,
                        block.in_txbx,
                    )
                    .text
                }
            };
            (block.kind, block.element, text)
        })
        .collect())
}

fn kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Paragraph => "paragraph",
        BlockKind::Table => "table",
    }
}

fn aligned_opcodes(
    keys_o: &[String],
    keys_r: &[String],
    subject: &str,
) -> Result<Vec<crate::difflib::Opcode>> {
    let limit = keys_o.len().min(keys_r.len());
    let mut prefix = 0;
    while prefix < limit && keys_o[prefix] == keys_r[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < limit - prefix
        && keys_o[keys_o.len() - 1 - suffix] == keys_r[keys_r.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let middle_o = keys_o.len() - prefix - suffix;
    let middle_r = keys_r.len() - prefix - suffix;
    if middle_o.saturating_mul(middle_r) > MAX_SEQUENCE_CELLS {
        return Err(unsupported(
            "sequence_budget",
            format!(
                "{subject} exceeds the sequence-matching budget ({middle_o} x {middle_r} > {MAX_SEQUENCE_CELLS} cells); split the documents"
            ),
        ));
    }
    let mut out = Vec::new();
    if prefix > 0 {
        out.push((Tag::Equal, 0, prefix, 0, prefix));
    }
    if middle_o > 0 || middle_r > 0 {
        let a = &keys_o[prefix..keys_o.len() - suffix];
        let b = &keys_r[prefix..keys_r.len() - suffix];
        for (tag, i1, i2, j1, j2) in SequenceMatcher::with_autojunk(a, b, false).opcodes() {
            out.push((tag, i1 + prefix, i2 + prefix, j1 + prefix, j2 + prefix));
        }
    }
    if suffix > 0 {
        out.push((
            Tag::Equal,
            keys_o.len() - suffix,
            keys_o.len(),
            keys_r.len() - suffix,
            keys_r.len(),
        ));
    }
    Ok(out)
}

impl Ctx<'_> {
    fn overrides<'t>(&'t self, working: &'t XmlTree) -> BTreeMap<String, XmlTree> {
        let mut map = self.finished.clone();
        map.insert(self.part.clone(), working.clone());
        map
    }

    /// Upstream `_next_id`: max of the shared counter and the attached scan.
    fn next_id(&mut self, working: &XmlTree) -> Result<i64> {
        let scanned = next_revision_id(self.document, &self.overrides(working))?;
        let value = self.id_cell.max(scanned);
        self.id_cell = value + 1;
        Ok(value)
    }
}

fn new_revision(
    tree: &mut XmlTree,
    scope: NodeId,
    local: &str,
    id: i64,
    author: &str,
    date: &str,
) -> Result<NodeId> {
    let element = w_el(tree, scope, local)?;
    oxml::set_w(tree, element, "id", &id.to_string())?;
    oxml::set_w(tree, element, "author", author)?;
    oxml::set_w(tree, element, "date", date)?;
    Ok(element)
}

/// Upstream `blocks._stamp_paragraph_mark`.
fn stamp_paragraph_mark(
    tree: &mut XmlTree,
    paragraph: NodeId,
    local: &str,
    id: i64,
    author: &str,
    date: &str,
) -> Result<()> {
    let ppr = oxml::get_or_add_ppr(tree, paragraph)?;
    let rpr = match w_child(tree, ppr, "rPr") {
        Some(existing) => existing,
        None => {
            let rpr = w_el(tree, ppr, "rPr")?;
            insert_before_successors(tree, ppr, rpr, &["sectPr", "pPrChange"])?;
            rpr
        }
    };
    let mark = new_revision(tree, rpr, local, id, author, date)?;
    tree.insert(rpr, 0, mark)
}

/// Upstream `blocks._wrap_paragraph_content_as_insertion`.
fn wrap_paragraph_content_as_insertion(
    tree: &mut XmlTree,
    paragraph: NodeId,
    id: i64,
    author: &str,
    date: &str,
) -> Result<()> {
    let insertion = new_revision(tree, paragraph, "ins", id, author, date)?;
    for child in node_children(tree, paragraph).collect::<Vec<_>>() {
        if is_w(tree, child, "pPr") {
            continue;
        }
        tree.detach(child)?;
        oxml::append(tree, insertion, child)?;
    }
    oxml::append(tree, paragraph, insertion)
}

fn retag_deleted_text(tree: &mut XmlTree, node: NodeId) -> Result<()> {
    let nodes: Vec<NodeId> = tree.descendants(node).collect();
    for item in nodes {
        if is_w(tree, item, "t") {
            tree.set_local_name(item, "delText")?;
        } else if is_w(tree, item, "instrText") {
            tree.set_local_name(item, "delInstrText")?;
        }
    }
    Ok(())
}

fn fldchar_unbalanced(tree: &XmlTree, element: NodeId) -> bool {
    let mut depth = 0i64;
    for node in tree.descendants(element) {
        if !is_w(tree, node, "fldChar") {
            continue;
        }
        match w_attr(tree, node, "fldCharType") {
            Some("begin") => depth += 1,
            Some("end") => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    depth != 0
}

/// Upstream `blocks._named_bookmarks_in`.
fn named_bookmarks_in(tree: &XmlTree, paragraph: NodeId) -> Vec<String> {
    let stream: Vec<NodeId> = iter_nodes(tree, paragraph).collect();
    let mut starts: HashMap<Option<String>, (usize, String)> = HashMap::new();
    let mut named: Vec<(Option<String>, String)> = Vec::new();
    for (position, node) in stream.iter().enumerate() {
        if is_w(tree, *node, "bookmarkStart") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let name = w_attr(tree, *node, "name").unwrap_or("").to_string();
            starts.insert(id.clone(), (position, name.clone()));
            if name != "_GoBack" {
                named.push((id, name));
            }
        } else if is_w(tree, *node, "bookmarkEnd") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let Some((start, name)) = starts.remove(&id) else {
                continue;
            };
            if name == "_GoBack" {
                continue;
            }
            let has_text = stream[start + 1..position]
                .iter()
                .any(|inner| is_w(tree, *inner, "t") && !leading_text(tree, *inner).is_empty());
            if !has_text {
                named.retain(|(i, _)| *i != id);
            }
        }
    }
    named.into_iter().map(|(_, name)| name).collect()
}

fn refuse_unrepresentable_children(
    ctx: &Ctx<'_>,
    tree: &XmlTree,
    paragraph: NodeId,
    verb: &str,
) -> Result<()> {
    for child in tree.child_elements(paragraph) {
        for local in ["sdt", "fldSimple"] {
            if is_w(tree, child, local) {
                return Err(unsupported(
                    "unrepresentable_child",
                    format!(
                        "compare cannot {verb} a paragraph containing <w:{local}> in {}; restructure the change or resolve it manually (a declared limit)",
                        ctx.story
                    ),
                ));
            }
        }
    }
    if fldchar_unbalanced(tree, paragraph) {
        return Err(unsupported(
            "unbalanced_field",
            format!(
                "compare cannot {verb} one paragraph of a multi-paragraph complex field in {} — the begin/end markers would unbalance (a declared limit)",
                ctx.story
            ),
        ));
    }
    let mut named = named_bookmarks_in(tree, paragraph);
    if !named.is_empty() {
        named.sort();
        return Err(unsupported(
            "named_bookmark",
            format!(
                "compare cannot {verb} a paragraph carrying named bookmark(s) {named:?} in {} — cross-references would dangle (a declared limit)",
                ctx.story
            ),
        ));
    }
    Ok(())
}

fn mark_paragraph_deleted(ctx: &mut Ctx<'_>, tree: &mut XmlTree, paragraph: NodeId) -> Result<()> {
    refuse_unrepresentable_children(ctx, tree, paragraph, "delete")?;
    let revision_id = ctx.next_id(tree)?;
    let mut current: Option<NodeId> = None;
    for child in node_children(tree, paragraph).collect::<Vec<_>>() {
        let transparent = [
            "bookmarkStart",
            "bookmarkEnd",
            "commentRangeStart",
            "commentRangeEnd",
        ]
        .iter()
        .any(|t| is_w(tree, child, t));
        if is_w(tree, child, "pPr") || transparent {
            current = None;
            continue;
        }
        if is_w(tree, child, "proofErr") {
            tree.detach(child)?;
            continue;
        }
        if is_w(tree, child, "hyperlink") {
            wrap_children_deleted(ctx, tree, child, revision_id)?;
            current = None;
            continue;
        }
        let deletion = match current {
            Some(existing) => existing,
            None => {
                let deletion =
                    new_revision(tree, paragraph, "del", revision_id, &ctx.author, &ctx.stamp)?;
                let index = tree.index_in_parent(child).unwrap_or(0);
                tree.insert(paragraph, index, deletion)?;
                current = Some(deletion);
                deletion
            }
        };
        retag_deleted_text(tree, child)?;
        tree.detach(child)?;
        oxml::append(tree, deletion, child)?;
    }
    let mark_id = ctx.next_id(tree)?;
    stamp_paragraph_mark(tree, paragraph, "del", mark_id, &ctx.author, &ctx.stamp)
}

fn wrap_children_deleted(
    ctx: &Ctx<'_>,
    tree: &mut XmlTree,
    container: NodeId,
    revision_id: i64,
) -> Result<()> {
    let mut wrapper: Option<NodeId> = None;
    for child in node_children(tree, container).collect::<Vec<_>>() {
        if !is_w(tree, child, "r") {
            wrapper = None;
            continue;
        }
        let target = match wrapper {
            Some(existing) => existing,
            None => {
                let deletion =
                    new_revision(tree, container, "del", revision_id, &ctx.author, &ctx.stamp)?;
                let index = tree.index_in_parent(child).unwrap_or(0);
                tree.insert(container, index, deletion)?;
                wrapper = Some(deletion);
                deletion
            }
        };
        retag_deleted_text(tree, child)?;
        tree.detach(child)?;
        oxml::append(tree, target, child)?;
    }
    Ok(())
}

fn delete_block(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    kind: BlockKind,
    element: NodeId,
) -> Result<()> {
    if kind == BlockKind::Table {
        for row in w_children(tree, element, "tr").collect::<Vec<_>>() {
            mark_row_deleted(ctx, tree, row)?;
        }
        return Ok(());
    }
    mark_paragraph_deleted(ctx, tree, element)
}

fn sanitize_source(ctx: &Ctx<'_>, source: &XmlTree, element: NodeId) -> Result<()> {
    let refused: &[(&str, &str)] = &[
        ("drawing", "an image or drawing"),
        ("object", "an embedded object"),
        ("pict", "legacy VML content"),
        ("hyperlink", "a hyperlink"),
        ("sdt", "a content control"),
        ("sectPr", "a section break"),
        ("fldSimple", "a field"),
        ("fldChar", "a complex field"),
    ];
    let descendants: Vec<NodeId> = source.descendants(element).skip(1).collect();
    for (tag, description) in refused {
        if descendants.iter().any(|n| is_w(source, *n, tag)) {
            return Err(unsupported(
                "insert_unrepresentable",
                format!(
                    "compare cannot insert {description} in {} without changing its package relationships or behavior",
                    ctx.story
                ),
            ));
        }
    }
    for num_pr in descendants.iter().filter(|n| is_w(source, **n, "numPr")) {
        let value = w_child(source, *num_pr, "numId").and_then(|n| w_attr(source, n, "val"));
        let numeric = value.and_then(|v| v.trim().parse::<i64>().ok());
        if !numeric.is_some_and(|n| ctx.numbering_ids.contains(&n)) {
            return Err(unsupported(
                "undefined_numbering",
                format!(
                    "compare cannot insert content referencing undefined numbering id {value:?} in {}",
                    ctx.story
                ),
            ));
        }
    }
    for tag in [
        "bookmarkStart",
        "bookmarkEnd",
        "commentRangeStart",
        "commentRangeEnd",
        "commentReference",
    ] {
        if descendants.iter().any(|n| is_w(source, *n, tag)) {
            return Err(unsupported(
                "insert_anchor",
                format!(
                    "compare cannot insert bookmark or comment anchors in {}; their ids and targets cannot be transferred",
                    ctx.story
                ),
            ));
        }
    }
    if fldchar_unbalanced(source, element) {
        return Err(unsupported(
            "unbalanced_field",
            format!(
                "compare cannot insert one paragraph of a multi-paragraph complex field in {} — the begin/end markers would unbalance (a declared limit)",
                ctx.story
            ),
        ));
    }
    Ok(())
}

fn cloned_as_insertion(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    source: &XmlTree,
    kind: BlockKind,
    element: NodeId,
    scope: NodeId,
) -> Result<NodeId> {
    sanitize_source(ctx, source, element)?;
    let clone = tree.import_subtree(source, element, scope)?;
    if kind == BlockKind::Table {
        for row in w_children(tree, clone, "tr").collect::<Vec<_>>() {
            mark_cloned_row_inserted(ctx, tree, row)?;
        }
        return Ok(clone);
    }
    let insert_id = ctx.next_id(tree)?;
    wrap_paragraph_content_as_insertion(tree, clone, insert_id, &ctx.author, &ctx.stamp)?;
    let mark_id = ctx.next_id(tree)?;
    stamp_paragraph_mark(tree, clone, "ins", mark_id, &ctx.author, &ctx.stamp)?;
    Ok(clone)
}

fn require_container_anchor(ctx: &Ctx<'_>, tree: &XmlTree, reference: NodeId) -> Result<()> {
    if tree
        .parent(reference)
        .is_none_or(|parent| is_w(tree, parent, "txbxContent"))
    {
        return Err(unsupported(
            "text_box_block",
            format!(
                "compare cannot add or remove whole blocks next to text-box content in {} (a declared limit)",
                ctx.story
            ),
        ));
    }
    if ctx.story.starts_with("word/footnotes") || ctx.story.starts_with("word/endnotes") {
        return Err(unsupported(
            "note_block",
            format!(
                "compare cannot add or remove whole blocks in {}; note-content edits must stay within existing notes (a declared limit)",
                ctx.story
            ),
        ));
    }
    Ok(())
}

fn insert_blocks(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    revised: &XmlTree,
    blocks_o: &[StoryBlock],
    insert_index: usize,
    new_blocks: &[StoryBlock],
) -> Result<()> {
    let scope = if insert_index < blocks_o.len() {
        blocks_o[insert_index].1
    } else if let Some(last) = blocks_o.last() {
        last.1
    } else {
        tree.root()
    };
    let scope_parent = tree.parent(scope).unwrap_or(scope);
    let mut clones = Vec::new();
    for (kind, element, _) in new_blocks {
        clones.push(cloned_as_insertion(
            ctx,
            tree,
            revised,
            *kind,
            *element,
            scope_parent,
        )?);
    }
    if insert_index < blocks_o.len() {
        let reference = blocks_o[insert_index].1;
        require_container_anchor(ctx, tree, reference)?;
        for clone in clones {
            insert_before(tree, reference, clone)?;
        }
        return Ok(());
    }
    let Some(last) = blocks_o.last() else {
        return Err(unsupported(
            "empty_story",
            format!("compare cannot insert into the empty story {}", ctx.story),
        ));
    };
    let mut reference = last.1;
    require_container_anchor(ctx, tree, reference)?;
    for clone in clones {
        insert_after(tree, reference, clone)?;
        reference = clone;
    }
    Ok(())
}

fn mark_row_deleted(ctx: &mut Ctx<'_>, tree: &mut XmlTree, row: NodeId) -> Result<()> {
    let revision_id = ctx.next_id(tree)?;
    let tr_pr = match w_child(tree, row, "trPr") {
        Some(existing) => existing,
        None => {
            let tr_pr = w_el(tree, row, "trPr")?;
            tree.insert(row, 0, tr_pr)?;
            tr_pr
        }
    };
    let marker = new_revision(tree, tr_pr, "del", revision_id, &ctx.author, &ctx.stamp)?;
    oxml::append(tree, tr_pr, marker)?;
    for cell in w_children(tree, row, "tc").collect::<Vec<_>>() {
        for paragraph in w_children(tree, cell, "p").collect::<Vec<_>>() {
            mark_paragraph_deleted(ctx, tree, paragraph)?;
        }
    }
    Ok(())
}

fn mark_cloned_row_inserted(ctx: &mut Ctx<'_>, tree: &mut XmlTree, row: NodeId) -> Result<()> {
    let revision_id = ctx.next_id(tree)?;
    let tr_pr = match w_child(tree, row, "trPr") {
        Some(existing) => existing,
        None => {
            let tr_pr = w_el(tree, row, "trPr")?;
            tree.insert(row, 0, tr_pr)?;
            tr_pr
        }
    };
    let marker = new_revision(tree, tr_pr, "ins", revision_id, &ctx.author, &ctx.stamp)?;
    oxml::append(tree, tr_pr, marker)?;
    for cell in w_children(tree, row, "tc").collect::<Vec<_>>() {
        for paragraph in w_children(tree, cell, "p").collect::<Vec<_>>() {
            let insert_id = ctx.next_id(tree)?;
            wrap_paragraph_content_as_insertion(
                tree,
                paragraph,
                insert_id,
                &ctx.author,
                &ctx.stamp,
            )?;
            let mark_id = ctx.next_id(tree)?;
            stamp_paragraph_mark(tree, paragraph, "ins", mark_id, &ctx.author, &ctx.stamp)?;
        }
    }
    Ok(())
}

fn insert_rows(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    revised: &XmlTree,
    rows_o: &[NodeId],
    index: usize,
    new_rows: &[NodeId],
    after: Option<NodeId>,
) -> Result<()> {
    let scope = rows_o
        .first()
        .and_then(|r| tree.parent(*r))
        .unwrap_or(tree.root());
    let mut clones = Vec::new();
    for row in new_rows {
        sanitize_source(ctx, revised, *row)?;
        let clone = tree.import_subtree(revised, *row, scope)?;
        mark_cloned_row_inserted(ctx, tree, clone)?;
        clones.push(clone);
    }
    if let Some(after) = after {
        let mut reference = after;
        for clone in clones {
            insert_after(tree, reference, clone)?;
            reference = clone;
        }
        return Ok(());
    }
    if index < rows_o.len() {
        for clone in clones {
            insert_before(tree, rows_o[index], clone)?;
        }
        return Ok(());
    }
    let mut reference = rows_o[rows_o.len() - 1];
    for clone in clones {
        insert_after(tree, reference, clone)?;
        reference = clone;
    }
    Ok(())
}

fn row_text(tree: &XmlTree, row: NodeId) -> String {
    w_children(tree, row, "tc")
        .map(|cell| {
            tree.descendants(cell)
                .filter(|n| is_w(tree, *n, "t"))
                .map(|n| leading_text(tree, n))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\u{0}")
}

fn refuse_merged_rows(ctx: &Ctx<'_>, tree: &XmlTree, rows: &[NodeId]) -> Result<()> {
    for row in rows {
        if tree
            .descendants(*row)
            .skip(1)
            .any(|n| is_w(tree, n, "gridSpan") || is_w(tree, n, "vMerge"))
        {
            return Err(unsupported(
                "merged_rows",
                format!(
                    "compare cannot redline changed table rows containing merged cells in {} (a declared limit)",
                    ctx.story
                ),
            ));
        }
    }
    Ok(())
}

fn compare_table(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    revised: &XmlTree,
    table_o: NodeId,
    table_r: NodeId,
) -> Result<()> {
    let rows_o: Vec<NodeId> = w_children(tree, table_o, "tr").collect();
    let rows_r: Vec<NodeId> = w_children(revised, table_r, "tr").collect();
    let keys_o: Vec<String> = rows_o.iter().map(|r| row_text(tree, *r)).collect();
    let keys_r: Vec<String> = rows_r.iter().map(|r| row_text(revised, *r)).collect();
    for (tag, i1, i2, j1, j2) in
        aligned_opcodes(&keys_o, &keys_r, &format!("table in {}", ctx.story))?
    {
        match tag {
            Tag::Equal => {}
            Tag::Delete => {
                refuse_merged_rows(ctx, tree, &rows_o[i1..i2])?;
                for row in &rows_o[i1..i2] {
                    mark_row_deleted(ctx, tree, *row)?;
                }
            }
            Tag::Insert => {
                refuse_merged_rows(ctx, revised, &rows_r[j1..j2])?;
                insert_rows(ctx, tree, revised, &rows_o, i1, &rows_r[j1..j2], None)?;
            }
            Tag::Replace => {
                let (old_rows, new_rows) = (&rows_o[i1..i2], &rows_r[j1..j2]);
                refuse_merged_rows(ctx, tree, old_rows)?;
                refuse_merged_rows(ctx, revised, new_rows)?;
                if old_rows.len() == 1 && new_rows.len() == 1 {
                    if !replace_row_cells(ctx, tree, revised, old_rows[0], new_rows[0])? {
                        mark_row_deleted(ctx, tree, old_rows[0])?;
                        insert_rows(ctx, tree, revised, &rows_o, i2, new_rows, Some(old_rows[0]))?;
                    }
                    continue;
                }
                for row in old_rows {
                    mark_row_deleted(ctx, tree, *row)?;
                }
                insert_rows(
                    ctx,
                    tree,
                    revised,
                    &rows_o,
                    i2,
                    new_rows,
                    Some(old_rows[old_rows.len() - 1]),
                )?;
            }
        }
    }
    Ok(())
}

fn visible_paragraph_text(tree: &XmlTree, paragraph: NodeId) -> String {
    subtree_text(tree, paragraph, View::Current, true, false, false).text
}

fn replace_row_cells(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    revised: &XmlTree,
    row_o: NodeId,
    row_r: NodeId,
) -> Result<bool> {
    let cells_o: Vec<NodeId> = w_children(tree, row_o, "tc").collect();
    let cells_r: Vec<NodeId> = w_children(revised, row_r, "tc").collect();
    if cells_o.len() != cells_r.len() || ctx.granularity != Granularity::Word {
        return Ok(false);
    }
    let nested = |t: &XmlTree, row: NodeId| t.descendants(row).skip(1).any(|n| is_w(t, n, "tbl"));
    if nested(tree, row_o) || nested(revised, row_r) {
        return Ok(false);
    }
    let mut plan = Vec::new();
    for (cell_o, cell_r) in cells_o.iter().zip(cells_r.iter()) {
        let paragraphs_o: Vec<NodeId> = w_children(tree, *cell_o, "p").collect();
        let paragraphs_r: Vec<NodeId> = w_children(revised, *cell_r, "p").collect();
        if paragraphs_o.len() != 1 || paragraphs_r.len() != 1 {
            let texts_o: Vec<String> = paragraphs_o
                .iter()
                .map(|p| visible_paragraph_text(tree, *p))
                .collect();
            let texts_r: Vec<String> = paragraphs_r
                .iter()
                .map(|p| visible_paragraph_text(revised, *p))
                .collect();
            if texts_o == texts_r {
                continue;
            }
            return Ok(false);
        }
        let text_o = visible_paragraph_text(tree, paragraphs_o[0]);
        let text_r = visible_paragraph_text(revised, paragraphs_r[0]);
        if text_o != text_r {
            plan.push((paragraphs_o[0], text_o, text_r));
        }
    }
    for (paragraph, text_o, text_r) in plan {
        replace_paragraph_text(ctx, tree, paragraph, &text_o, &text_r)?;
    }
    Ok(true)
}

/// Upstream `_token_regions`: `\S+|\s+` tokens diffed without autojunk.
fn token_regions(old: &str, new: &str) -> Result<Vec<(usize, usize, String)>> {
    let tokens = |text: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in text.chars() {
            let space = is_py_space(c);
            match out.last_mut() {
                Some(last) if last.chars().next().is_some_and(|f| is_py_space(f) == space) => {
                    last.push(c)
                }
                _ => out.push(c.to_string()),
            }
        }
        out
    };
    let tokens_o = tokens(old);
    let tokens_r = tokens(new);
    let cells = tokens_o.len().saturating_mul(tokens_r.len());
    if cells > MAX_TEXT_SEQUENCE_CELLS {
        return Err(unsupported(
            "token_budget",
            format!(
                "paragraph token diff exceeds the sequence-matching budget ({cells} > {MAX_TEXT_SEQUENCE_CELLS} token cells); use granularity='block' or split the documents"
            ),
        ));
    }
    let mut offsets = vec![0usize];
    for token in &tokens_o {
        offsets.push(offsets[offsets.len() - 1] + token.chars().count());
    }
    let mut regions = Vec::new();
    for (tag, i1, i2, j1, j2) in
        SequenceMatcher::with_autojunk(&tokens_o, &tokens_r, false).opcodes()
    {
        if tag == Tag::Equal {
            continue;
        }
        regions.push((offsets[i1], offsets[i2], tokens_r[j1..j2].concat()));
    }
    Ok(regions)
}

/// Upstream `_paragraph_span`.
fn paragraph_span(
    ctx: &Ctx<'_>,
    tree: &XmlTree,
    paragraph: NodeId,
    start: usize,
    end: usize,
) -> Result<Option<Span>> {
    let atoms: Vec<Atom> = story_atoms(tree, &ctx.story)?
        .into_iter()
        .filter(|atom| atom.paragraph == Some(paragraph) && include_atom(atom, View::Current))
        .collect();
    if atoms.is_empty() || end < start {
        return Ok(None);
    }
    let mut positions: Vec<(usize, usize)> = Vec::new();
    let mut visible = String::new();
    for (index, atom) in atoms.iter().enumerate() {
        if atom.barrier {
            continue;
        }
        let text = atom.text(tree);
        for offset in 0..text.chars().count() {
            positions.push((index, offset));
        }
        visible.push_str(&text);
    }
    if end > positions.len() {
        return Ok(None);
    }
    let (start_atom, start_offset, end_atom, end_offset) = if start == end {
        let mut candidates = Vec::new();
        let mut position = 0;
        for (index, atom) in atoms.iter().enumerate() {
            if atom.barrier {
                continue;
            }
            let piece_end = position + atom.text(tree).chars().count();
            if position <= start && start <= piece_end && !atom.is_synthetic() {
                candidates.push((index, start - position));
            }
            position = piece_end;
        }
        let (Some(first), Some(last)) = (candidates.first(), candidates.last()) else {
            return Ok(None);
        };
        (first.0, first.1, last.0, last.1)
    } else {
        let (sa, so) = positions[start];
        let (ea, eo) = positions[end - 1];
        (sa, so, ea, eo + 1)
    };
    let span_atoms: Vec<Atom> = atoms[start_atom..=end_atom].to_vec();
    if span_atoms.iter().any(|a| a.barrier) {
        return Ok(None);
    }
    let text: String = visible.chars().skip(start).take(end - start).collect();
    Ok(Some(Span {
        incarnation: tree.incarnation(),
        document: ctx.document.id(),
        token: new_span_token(),
        story: ctx.story.clone(),
        part: ctx.part.clone(),
        anchor: Anchor {
            story: ctx.story.clone(),
            index: span_atoms[0].block_index,
            content_hash: content_hash(&text),
        },
        in_insert: span_atoms.iter().any(|a| a.in_insert),
        in_delete: span_atoms
            .iter()
            .any(|a| a.in_delete || a.is_tag("delText")),
        in_content_control: span_atoms.iter().any(|a| a.sdt.is_some()),
        in_text_box: span_atoms.iter().any(|a| a.in_text_box),
        in_field: span_atoms.iter().any(|a| a.in_field),
        crosses_paragraphs: false,
        signatures: span_atoms.iter().map(|a| signature(tree, a)).collect(),
        atom_sequence: span_atoms.iter().map(|a| a.element).collect(),
        sequence_view: Some(View::Current),
        atoms: span_atoms,
        start_offset,
        end_offset,
        raw_start: 0,
        match_start: 0,
        match_policy: None,
        view: View::Current,
        text,
    }))
}

fn replace_paragraph_text(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    paragraph: NodeId,
    text_o: &str,
    text_r: &str,
) -> Result<()> {
    let regions = token_regions(text_o, text_r)?;
    for (start, end, replacement) in regions.into_iter().rev() {
        let outcome = (|| -> Result<()> {
            let Some(span) = paragraph_span(ctx, tree, paragraph, start, end)? else {
                return Err(unsupported(
                    "region_unmappable",
                    "region unmappable to atoms",
                ));
            };
            let options = ReplaceOptions {
                tracked: true,
                author: Some(ctx.author.clone()),
                date: Some(ctx.stamp_ts),
                preserve_revision: false,
            };
            let next = next_revision_id(ctx.document, &ctx.overrides(tree))?;
            let plan = plan_replace(
                ctx.document,
                tree,
                &span,
                &replacement,
                &options,
                None,
                next,
                None,
            )?;
            apply_plan(tree, &plan)
        })();
        if let Err(err) = outcome {
            if err.is_refusal() {
                return Err(unsupported(
                    "word_edit_refused",
                    format!(
                        "compare cannot safely redline this paragraph in {}; the text-edit primitive refused: {}",
                        ctx.story,
                        err.message()
                    ),
                ));
            }
            return Err(err);
        }
    }
    Ok(())
}

fn compare_region(
    ctx: &mut Ctx<'_>,
    tree: &mut XmlTree,
    revised: &XmlTree,
    old_run: &[StoryBlock],
    new_run: &[StoryBlock],
) -> Result<()> {
    if ctx.granularity == Granularity::Block
        || old_run.len() != 1
        || new_run.len() != 1
        || old_run[0].0 != new_run[0].0
    {
        insert_blocks(ctx, tree, revised, old_run, 0, new_run)?;
        for (kind, element, _) in old_run {
            delete_block(ctx, tree, *kind, *element)?;
        }
        return Ok(());
    }
    let (kind, element_o, text_o) = &old_run[0];
    let (_, element_r, text_r) = &new_run[0];
    if *kind == BlockKind::Table {
        return compare_table(ctx, tree, revised, *element_o, *element_r);
    }
    if textless_story_bytes(tree, *element_o) != textless_story_bytes(revised, *element_r) {
        return Err(unsupported(
            "text_with_format_change",
            format!(
                "compare cannot combine a text edit with a formatting or structural change in {}; apply the formatting change separately",
                ctx.story
            ),
        ));
    }
    replace_paragraph_text(ctx, tree, *element_o, text_o, text_r)
}

fn compare_story(ctx: &mut Ctx<'_>, tree: &mut XmlTree, revised: &XmlTree) -> Result<()> {
    let blocks_o = story_blocks(tree, &ctx.story)?;
    let blocks_r = story_blocks(revised, &ctx.story)?;
    if blocks_o.len().max(blocks_r.len()) > MAX_BLOCKS {
        return Err(unsupported(
            "block_budget",
            format!(
                "story {} exceeds the compare block budget ({MAX_BLOCKS} blocks); split the documents",
                ctx.story
            ),
        ));
    }
    let keys = |blocks: &[StoryBlock]| {
        blocks
            .iter()
            .map(|(k, _, t)| format!("{}\u{0}{t}", kind_name(*k)))
            .collect::<Vec<_>>()
    };
    let opcodes = aligned_opcodes(
        &keys(&blocks_o),
        &keys(&blocks_r),
        &format!("story {}", ctx.story),
    )?;
    for (tag, i1, i2, j1, j2) in opcodes {
        match tag {
            Tag::Equal => {
                for offset in 0..i2 - i1 {
                    let (_, element_o, text) = &blocks_o[i1 + offset];
                    let (_, element_r, _) = &blocks_r[j1 + offset];
                    if canonical_story_bytes(tree, *element_o)
                        != canonical_story_bytes(revised, *element_r)
                    {
                        let preview: String = text.chars().take(80).collect();
                        return Err(unsupported(
                            "formatting_difference",
                            format!(
                                "compare cannot redline a formatting or structural difference in {}: {preview:?}",
                                ctx.story
                            ),
                        ));
                    }
                }
            }
            Tag::Delete => {
                for (kind, element, _) in &blocks_o[i1..i2] {
                    delete_block(ctx, tree, *kind, *element)?;
                }
            }
            Tag::Insert => insert_blocks(ctx, tree, revised, &blocks_o, i1, &blocks_r[j1..j2])?,
            Tag::Replace => {
                compare_region(ctx, tree, revised, &blocks_o[i1..i2], &blocks_r[j1..j2])?
            }
        }
    }
    Ok(())
}

fn numbering_ids(document: &Document) -> HashSet<i64> {
    // Upstream `_numbering_ids`: definitions from `list_numbering`, and any
    // failure (missing part, unresolvable paragraph numbering) means none.
    match crate::numbering::list_numbering(document) {
        Ok(report) => report.definitions.iter().map(|d| d.num_id).collect(),
        Err(_) => HashSet::new(),
    }
}

fn story_revision_count(document: &Document) -> Result<usize> {
    let mut count = 0;
    for story in story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        count += tree
            .descendants(tree.root())
            .filter(|n| {
                tree.ns_uri(*n) == Some(W)
                    && tree
                        .local_name(*n)
                        .is_some_and(|l| REVISION_TAGS.contains(&l))
            })
            .count();
    }
    Ok(count)
}

/// Hook for materializing input revisions and proving the algebra; filled
/// in by the revision engine (`crate::revision`).
pub(crate) struct Verifier {
    /// Resolve every revision (accept = true / reject = false).
    pub resolve_all: fn(&mut Document, bool) -> Result<()>,
}

/// `author` goes into every `w:author`: required, XML 1.0 text without
/// control characters, at most [`MAX_AUTHOR_CHARS`] characters.
fn validate_author(author: &str) -> Result<()> {
    if author.is_empty() {
        return Err(Error::invalid_argument(
            "author_required",
            "author is required",
        ));
    }
    validate_writable_text(author, "author")?;
    if author.chars().any(char::is_control) {
        return Err(Error::invalid_argument(
            "control_characters",
            "author contains control character(s); w:author is a single line of text",
        ));
    }
    let length = author.chars().count();
    if length > MAX_AUTHOR_CHARS {
        return Err(Error::invalid_argument(
            "author_too_long",
            format!("author is {length} characters; at most {MAX_AUTHOR_CHARS} are allowed"),
        ));
    }
    Ok(())
}

/// Produce the redline. `verifier` resolves revisions for `materialize` and
/// for the algebra proof; without it compare refuses inputs that need
/// materialization and skips nothing silently (`algebra_unverified` is
/// reported as a finding).
pub(crate) fn compare_with(
    original: &[u8],
    revised: &[u8],
    options: &CompareOptions,
    limits: &Limits,
    verifier: Option<&Verifier>,
) -> Result<CompareResult> {
    validate_author(&options.author)?;
    let stamp_ts = options.date.unwrap_or_else(Timestamp::now);
    let raw_original = read_raw_package(original, "original", limits)?;
    let raw_revised = read_raw_package(revised, "revised", limits)?;
    if options.materialize.is_none() {
        for (which, raw) in [("original", &raw_original), ("revised", &raw_revised)] {
            if raw.has_revisions {
                return Err(unsupported(
                    "pending_revisions",
                    format!(
                        "the {which} document carries pending tracked revisions; compare would conflate them with its own redline. Pass materialize='accept' or 'reject' to resolve working copies first (the input files are not modified)"
                    ),
                ));
            }
        }
    }
    refuse_unsupported_package_differences(&raw_original, &raw_revised, limits)?;
    let mut document = Document::open_bytes(original, limits)?;
    if options.acknowledge_protection {
        document.acknowledge_protection()?;
    }
    refuse_if_protected(&document, "compare documents", OperationClass::BodyContent)?;
    let mut revised_doc = Document::open_bytes(revised, limits)?;
    let mut findings = Vec::new();
    for (which, doc) in [("original", &mut document), ("revised", &mut revised_doc)] {
        if story_revision_count(doc)? > 0 {
            match (options.materialize, verifier) {
                (None, _) => {
                    return Err(unsupported(
                        "pending_revisions",
                        format!(
                            "the {which} document carries pending tracked revisions; compare would conflate them with its own redline. Pass materialize='accept' or 'reject' to resolve working copies first (the input files are not modified)"
                        ),
                    ));
                }
                (Some(_), None) => {
                    return Err(unsupported(
                        "materialize_unavailable",
                        "revision materialization is not available in this build; nothing was changed",
                    ));
                }
                (Some(mode), Some(verifier)) => {
                    // The revised input is a throwaway working copy; the
                    // original was gated above and keeps the caller's
                    // acknowledgment.
                    if which == "revised" && doc.protection_status()?.enforced {
                        doc.acknowledge_protection()?;
                    }
                    (verifier.resolve_all)(doc, mode == Materialize::Accept)?;
                }
            }
        }
    }
    let original_reference = document.patch_bytes()?.0;
    let revised_reference = revised_doc.patch_bytes()?.0;
    let status = document.protection_status()?;
    if status.enforced {
        findings.push(CompareFinding {
            kind: "document_protection_present".into(),
            story: "word/settings.xml".into(),
            detail: format!(
                "original enforces {} protection; the redline was produced anyway (a new artifact) and the setting is carried through unmodified",
                status.edit.as_deref().map(|e| format!("'{e}'")).unwrap_or_else(|| "None".into())
            ),
        });
    }
    let stories_o: BTreeMap<String, String> = story::stories(&document)
        .into_iter()
        .map(|s| (s.name, s.part))
        .collect();
    let stories_r: BTreeMap<String, String> = story::stories(&revised_doc)
        .into_iter()
        .map(|s| (s.name, s.part))
        .collect();
    let names_o: BTreeSet<&String> = stories_o
        .keys()
        .filter(|n| n.as_str() != "word/comments.xml")
        .collect();
    let names_r: BTreeSet<&String> = stories_r
        .keys()
        .filter(|n| n.as_str() != "word/comments.xml")
        .collect();
    if names_o != names_r {
        let only_o: Vec<&&String> = names_o.difference(&names_r).collect();
        let only_r: Vec<&&String> = names_r.difference(&names_o).collect();
        return Err(unsupported(
            "story_parts_differ",
            format!(
                "the documents carry different story parts (only-original: {only_o:?}, only-revised: {only_r:?}); compare cannot redline story-part addition or removal"
            ),
        ));
    }
    let raw_o: BTreeSet<String> = raw_original
        .story_parts
        .iter()
        .map(|n| casefold(n))
        .collect();
    let raw_r: BTreeSet<String> = raw_revised
        .story_parts
        .iter()
        .map(|n| casefold(n))
        .collect();
    let loaded_o: BTreeSet<String> = names_o.iter().map(|n| n.to_string()).collect();
    let loaded_r: BTreeSet<String> = names_r.iter().map(|n| n.to_string()).collect();
    if loaded_o != raw_o {
        return Err(unsupported(
            "story_unloadable",
            "the original package contains a story part compare could not load safely",
        ));
    }
    if loaded_r != raw_r {
        return Err(unsupported(
            "story_unloadable",
            "the revised package contains a story part compare could not load safely",
        ));
    }
    let mut ctx = Ctx {
        document: &document,
        author: options.author.clone(),
        stamp: stamp_ts.to_xml(),
        stamp_ts,
        granularity: options.granularity,
        story: String::new(),
        part: String::new(),
        numbering_ids: numbering_ids(&document),
        id_cell: 0,
        finished: BTreeMap::new(),
    };
    let mut compared = Vec::new();
    for story in loaded_o.iter() {
        let part_o = stories_o[story].clone();
        let part_r = stories_r[story].clone();
        ctx.story = story.clone();
        ctx.part = part_o.clone();
        let mut working = document.package().xml(&part_o)?.clone();
        let revised_tree = revised_doc.package().xml(&part_r)?;
        compare_story(&mut ctx, &mut working, revised_tree)?;
        ctx.finished.insert(part_o, working);
        compared.push(story.clone());
    }
    let finished = std::mem::take(&mut ctx.finished);
    drop(ctx);
    let mut txn = Txn::new(&document);
    for (part, tree) in finished {
        let unchanged = xml::trees_equivalent(&tree, document.package().xml(&part)?);
        if !unchanged {
            *txn.tree_mut(&part)? = tree;
        }
    }
    let staged = txn.finish()?;
    document.commit(staged)?;
    match verifier {
        Some(verifier) => verify_algebra(
            &document,
            &original_reference,
            &revised_reference,
            limits,
            verifier,
        )?,
        None => findings.push(CompareFinding {
            kind: "algebra_unverified".into(),
            story: String::new(),
            detail: "accept/reject verification requires the revision engine".into(),
        }),
    }
    let revision_count = document.revisions()?.len();
    Ok(CompareResult {
        document,
        findings,
        revision_count,
        stories: compared,
    })
}

fn verify_algebra(
    redline: &Document,
    original_reference: &[u8],
    revised_reference: &[u8],
    limits: &Limits,
    verifier: &Verifier,
) -> Result<()> {
    let redline_bytes = redline.patch_bytes()?.0;
    for (accept, expected, verb) in [
        (true, revised_reference, "accept"),
        (false, original_reference, "reject"),
    ] {
        let mut candidate = Document::open_bytes(&redline_bytes, limits)?;
        if candidate.protection_status()?.enforced {
            candidate.acknowledge_protection()?;
        }
        (verifier.resolve_all)(&mut candidate, accept)?;
        let leftover = remaining_markup(&candidate)?;
        if !leftover.is_empty() {
            return Err(unsupported(
                "algebra_unproved",
                format!(
                    "compare could not prove its {verb} contract; revision markup remains after verification: {leftover:?}"
                ),
            ));
        }
        assert_packages_match(&candidate.patch_bytes()?.0, expected, verb, limits)?;
    }
    Ok(())
}

fn remaining_markup(document: &Document) -> Result<BTreeMap<String, usize>> {
    let mut out = BTreeMap::new();
    for story in story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        for node in tree.descendants(tree.root()) {
            if is_markup(tree, node) {
                *out.entry(tree.local_name(node).unwrap_or("").to_string())
                    .or_insert(0) += 1;
            }
        }
    }
    Ok(out)
}

fn assert_packages_match(
    actual: &[u8],
    expected: &[u8],
    outcome: &str,
    limits: &Limits,
) -> Result<()> {
    let fail = |detail: String| {
        unsupported(
            "algebra_unproved",
            format!("compare could not prove its {outcome} contract; {detail}"),
        )
    };
    let a = Document::open_bytes(actual, limits)?;
    let b = Document::open_bytes(expected, limits)?;
    let sa: BTreeMap<String, String> = story::stories(&a)
        .into_iter()
        .map(|s| (s.name, s.part))
        .collect();
    let sb: BTreeMap<String, String> = story::stories(&b)
        .into_iter()
        .map(|s| (s.name, s.part))
        .collect();
    if sa.keys().collect::<Vec<_>>() != sb.keys().collect::<Vec<_>>() {
        return Err(fail("story parts differ".into()));
    }
    for (name, part) in &sa {
        let ta = a.package().xml(part)?;
        let tb = b.package().xml(&sb[name])?;
        if canonical_story_bytes(ta, ta.root()) != canonical_story_bytes(tb, tb.root()) {
            return Err(fail(format!("story {name:?} does not match")));
        }
    }
    let za = read_archive(actual, limits)?;
    let zb = read_archive(expected, limits)?;
    let names_a: BTreeSet<&str> = za.members.iter().map(|m| m.name.as_str()).collect();
    let names_b: BTreeSet<&str> = zb.members.iter().map(|m| m.name.as_str()).collect();
    if names_a != names_b {
        return Err(fail("package parts differ".into()));
    }
    let order_a: Vec<&str> = za.members.iter().map(|m| m.name.as_str()).collect();
    let order_b: Vec<&str> = zb.members.iter().map(|m| m.name.as_str()).collect();
    let story_members: BTreeSet<&str> = sa.values().map(String::as_str).collect();
    for member in &za.members {
        if story_members.contains(member.name.as_str()) {
            continue;
        }
        let other = zb.get(&member.name).unwrap_or(&[]);
        if member.data == other {
            continue;
        }
        if is_xml_part_name(&member.name)
            && parts_semantically_equal(
                &member.name,
                &member.data,
                other,
                &order_a,
                &order_b,
                limits,
            )
        {
            continue;
        }
        return Err(fail(format!(
            "package part {:?} does not match",
            member.name
        )));
    }
    Ok(())
}

/// Compare `original` and `revised` package bytes (see module docs).
pub fn compare(
    original: &[u8],
    revised: &[u8],
    options: &CompareOptions,
    limits: &Limits,
) -> Result<CompareResult> {
    compare_with(
        original,
        revised,
        options,
        limits,
        crate::compare::default_verifier().as_ref(),
    )
}

/// The verifier backed by the revision engine.
pub(crate) fn default_verifier() -> Option<Verifier> {
    fn resolve_all(document: &mut Document, accept: bool) -> Result<()> {
        let mut revisions = document.revisions()?;
        if accept {
            revisions.accept_all(document, None)?;
        } else {
            revisions.reject_all(document, None)?;
        }
        Ok(())
    }
    Some(Verifier { resolve_all })
}
