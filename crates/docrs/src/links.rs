//! Hyperlinks on existing text — upstream `links.py`, plus the span edge-run
//! isolation (`Span._isolate_edge_runs`) shared with bookmarks and notes.
//!
//! Adapted from paper-docx (MIT); see `NOTICE.md`.

use serde::Serialize;

use crate::anchor::require_current_view;
use crate::controls::{refuse_control_surface, tail_nodes};
use crate::oxml::add_t;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::validate_fresh;
use crate::search::Span;
use crate::txn::{RelTarget, Txn};
use crate::unicode::is_py_space;
use crate::wml::{W, is_w, leading_text, node_children, set_leading_text, w_attr, w_child};
use crate::xml::{NodeId, XML_NS, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

fn boundary(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::BoundaryViolation, reason, message)
}

/// Upstream `require_span_owner`.
pub(crate) fn require_span_owner(document: &Document, span: &Span) -> Result<()> {
    if span.document != document.id() {
        return Err(boundary(
            "foreign_span",
            "span belongs to a different document; re-find the text in the document passed to this operation",
        ));
    }
    Ok(())
}

/// Ownership, then the current-view requirement for live mutation anchors.
pub(crate) fn require_live_span(document: &Document, span: &Span) -> Result<()> {
    require_span_owner(document, span)?;
    require_current_view(crate::anchor::BlockTarget::Span(span))
}

/// `Span._validate_fresh` against the live tree.
pub(crate) fn validate_span_fresh(document: &Document, span: &Span) -> Result<()> {
    let tree = document.package().xml(&span.part).map_err(|_| {
        Error::refusal(
            RefusalKind::TargetNotFound,
            "story_removed",
            "span is stale: its story part was removed from the document",
        )
    })?;
    validate_fresh(document, tree, span, None)
}

/// Move `node` (with its lxml tail) to the end of `parent`.
pub(crate) fn append_moving(tree: &mut XmlTree, parent: NodeId, node: NodeId) -> Result<()> {
    let tail = tail_nodes(tree, node);
    tree.detach(node)?;
    for text in &tail {
        tree.detach(*text)?;
    }
    for moved in std::iter::once(node).chain(tail) {
        let end = tree.children(parent).len();
        tree.insert(parent, end, moved)?;
    }
    Ok(())
}

/// lxml `anchor.addprevious(node)` for a new detached `node`.
pub(crate) fn add_previous(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "anchor has no parent"))?;
    let index = tree
        .index_in_parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "anchor has no index"))?;
    tree.insert(parent, index, node)
}

/// lxml `anchor.addnext(node)` for a new detached `node`.
pub(crate) fn add_next(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "anchor has no parent"))?;
    let index = tree
        .index_in_parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "anchor has no index"))?;
    tree.insert(parent, index + 1, node)
}

/// lxml `copy.deepcopy(element)` appended to `parent` (the tail is copied
/// too).
fn append_copy(tree: &mut XmlTree, parent: NodeId, element: NodeId) -> Result<()> {
    let tail: Vec<String> = tail_nodes(tree, element)
        .into_iter()
        .filter_map(|t| tree.text(t).map(str::to_string))
        .collect();
    let copy = tree.deep_clone(element);
    let end = tree.children(parent).len();
    tree.insert(parent, end, copy)?;
    for text in tail {
        let node = tree.create_text(text);
        let end = tree.children(parent).len();
        tree.insert(parent, end, node)?;
    }
    Ok(())
}

/// Upstream `_set_preserved_text`.
fn set_preserved_text(tree: &mut XmlTree, element: NodeId, text: &str) -> Result<()> {
    set_leading_text(tree, element, text)?;
    let edge_space = text.chars().next().is_some_and(is_py_space)
        || text.chars().last().is_some_and(is_py_space);
    if edge_space {
        tree.set_attr(element, Some(XML_NS), "space", "preserve")?;
    } else {
        tree.remove_attr(element, Some(XML_NS), "space");
    }
    Ok(())
}

fn new_run_like(tree: &mut XmlTree, run: NodeId) -> Result<NodeId> {
    let new_run = tree.create_element(run, W, "r")?;
    Ok(new_run)
}

fn split_before(tree: &mut XmlTree, run: NodeId, element: NodeId, prefix: &str) -> Result<()> {
    let preceding: Vec<NodeId> = node_children(tree, run)
        .take_while(|child| *child != element)
        .filter(|child| !is_w(tree, *child, "rPr"))
        .collect();
    if preceding.is_empty() && prefix.is_empty() {
        return Ok(());
    }
    let new_run = new_run_like(tree, run)?;
    add_previous(tree, run, new_run)?;
    if let Some(rpr) = w_child(tree, run, "rPr") {
        append_copy(tree, new_run, rpr)?;
    }
    for child in preceding {
        append_moving(tree, new_run, child)?;
    }
    if !prefix.is_empty() {
        add_t(tree, new_run, prefix)?;
    }
    Ok(())
}

fn split_after(tree: &mut XmlTree, run: NodeId, element: NodeId, tail: &str) -> Result<()> {
    let following: Vec<NodeId> = node_children(tree, run)
        .skip_while(|child| *child != element)
        .skip(1)
        .filter(|child| !is_w(tree, *child, "rPr"))
        .collect();
    if following.is_empty() && tail.is_empty() {
        return Ok(());
    }
    let new_run = new_run_like(tree, run)?;
    add_next(tree, run, new_run)?;
    if let Some(rpr) = w_child(tree, run, "rPr") {
        append_copy(tree, new_run, rpr)?;
    }
    if !tail.is_empty() {
        add_t(tree, new_run, tail)?;
    }
    for child in following {
        append_moving(tree, new_run, child)?;
    }
    Ok(())
}

/// Upstream `Span._isolate_edge_runs` on a staged tree: split the boundary
/// runs so the span's runs hold exactly its text. Returns the span's new
/// `(start_offset, end_offset)`.
pub(crate) fn isolate_edge_runs(tree: &mut XmlTree, span: &Span) -> Result<(usize, usize)> {
    let mut start = span.start_offset;
    let mut end = span.end_offset;
    let first = &span.atoms[0];
    if let Some(run) = first.run {
        let text = first.text(tree);
        let prefix: String = text.chars().take(start).collect();
        split_before(tree, run, first.element, &prefix)?;
        if !prefix.is_empty() {
            let rest: String = text.chars().skip(start).collect();
            set_preserved_text(tree, first.element, &rest)?;
            if span.atoms.len() == 1 {
                end -= start;
            }
            start = 0;
        }
    }
    let last = &span.atoms[span.atoms.len() - 1];
    if let Some(run) = last.run {
        let text = last.text(tree);
        let length = text.chars().count();
        let tail: String = if end < length {
            text.chars().skip(end).collect()
        } else {
            String::new()
        };
        split_after(tree, run, last.element, &tail)?;
        if !tail.is_empty() {
            let kept: String = text.chars().take(end).collect();
            set_preserved_text(tree, last.element, &kept)?;
        }
    }
    Ok((start, end))
}

/// The distinct runs of the span's atoms, in order.
pub(crate) fn span_runs(span: &Span) -> Vec<NodeId> {
    let mut runs: Vec<NodeId> = Vec::new();
    for atom in &span.atoms {
        if let Some(run) = atom.run
            && !runs.contains(&run)
        {
            runs.push(run);
        }
    }
    runs
}

/// Next element/comment/PI sibling (lxml `getnext`).
pub(crate) fn next_sibling(tree: &XmlTree, node: NodeId) -> Option<NodeId> {
    let parent = tree.parent(node)?;
    let siblings: Vec<NodeId> = node_children(tree, parent).collect();
    let index = siblings.iter().position(|s| *s == node)?;
    siblings.get(index + 1).copied()
}

/// A created hyperlink: its address and visible text (what upstream's
/// `Hyperlink.address` / `.text` report).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hyperlink {
    /// External target.
    pub address: String,
    /// Visible text (python-docx `CT_R.text` of each direct run).
    pub text: String,
}

impl Hyperlink {
    /// Oracle-compatible JSON (`{"type": "Hyperlink", "address", "text"}`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"type": "Hyperlink", "address": self.address, "text": self.text})
    }
}

/// python-docx `CT_R.text`.
fn run_text(tree: &XmlTree, run: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(run) {
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        match tree.local_name(child).unwrap_or("") {
            "t" => out.push_str(&leading_text(tree, child)),
            "tab" | "ptab" => out.push('\t'),
            "cr" => out.push('\n'),
            "br" => {
                if w_attr(tree, child, "type").unwrap_or("textWrapping") == "textWrapping" {
                    out.push('\n');
                }
            }
            "noBreakHyphen" => out.push('-'),
            _ => {}
        }
    }
    out
}

fn has_style_named(tree: &XmlTree, name: &str) -> bool {
    tree.child_elements(tree.root())
        .filter(|style| is_w(tree, *style, "style"))
        .any(|style| {
            w_child(tree, style, "name").and_then(|n| w_attr(tree, n, "val")) == Some(name)
        })
}

/// python-docx `styles.add_style("Hyperlink", CHARACTER, builtin=True)`
/// when the styles part lacks it (creating the styles part if needed).
fn ensure_hyperlink_style(txn: &mut Txn<'_>) -> Result<()> {
    let styles = crate::parts::ensure_styles_part(txn)?;
    if has_style_named(txn.tree(&styles)?, "Hyperlink") {
        return Ok(());
    }
    let tree = txn.tree_mut(&styles)?;
    let root = tree.root();
    let style = tree.create_element(root, W, "style")?;
    let end = tree.children(root).len();
    tree.insert(root, end, style)?;
    tree.set_attr(style, Some(W), "type", "character")?;
    tree.set_attr(style, Some(W), "styleId", "Hyperlink")?;
    let name = tree.create_element(style, W, "name")?;
    tree.insert(style, 0, name)?;
    tree.set_attr(name, Some(W), "val", "Hyperlink")
}

/// Wrap `span`'s runs in a hyperlink to `address` (upstream
/// `add_hyperlink`).
///
/// The text stays; it takes the `Hyperlink` character style, which is
/// defined when the document lacks it. Refuses an empty address, a script
/// address (`javascript:`, `vbscript:`) or one with control characters, a
/// protected document, a stale or foreign span, a field result, a span
/// crossing paragraphs, an existing hyperlink, intervening markup, and a
/// data-bound, locked or plain-text control surface. Other schemes are the
/// caller's to vet.
pub fn add_hyperlink(document: &mut Document, span: &Span, address: &str) -> Result<Hyperlink> {
    if address.is_empty() {
        return Err(Error::invalid_argument(
            "empty_address",
            "address must be a non-empty URL",
        ));
    }
    if address.chars().any(char::is_control) {
        return Err(Error::invalid_argument(
            "address_control_character",
            "address must not contain control characters",
        ));
    }
    let scheme = address
        .trim_start()
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    if matches!(scheme.as_deref(), Some("javascript" | "vbscript")) {
        return Err(Error::invalid_argument(
            "script_address",
            "address must not be a javascript: or vbscript: URL",
        ));
    }
    let doc: &Document = document;
    require_live_span(doc, span)?;
    refuse_if_protected(doc, "add a hyperlink", OperationClass::BodyContent)?;
    validate_span_fresh(doc, span)?;
    if span.in_field {
        return Err(unsupported(
            "span_in_field_result",
            "the span lies inside a field result; Word regenerates field results on update, so the hyperlink would silently vanish",
        ));
    }
    refuse_control_surface(doc.package().xml(&span.part)?, span, "a hyperlink")?;
    let mut txn = Txn::new(doc);
    ensure_hyperlink_style(&mut txn)?;
    let tree = txn.tree_mut(&span.part)?;
    isolate_edge_runs(tree, span)?;
    let runs = span_runs(span);
    if runs.is_empty() {
        return Err(boundary(
            "no_runs",
            "span has no runs to wrap in a hyperlink; nothing was changed",
        ));
    }
    let parents: Vec<Option<NodeId>> = runs.iter().map(|run| tree.parent(*run)).collect();
    if parents.iter().any(|p| *p != parents[0]) {
        return Err(boundary(
            "crosses_paragraphs",
            "cannot wrap a hyperlink across a paragraph boundary; nothing was changed",
        ));
    }
    if !tree.ancestors(runs[0]).any(|a| is_w(tree, a, "p")) {
        return Err(boundary(
            "not_in_paragraph",
            "hyperlink runs are not inside a paragraph; nothing was changed",
        ));
    }
    for run in &runs {
        for ancestor in tree.ancestors(*run) {
            if is_w(tree, ancestor, "p") {
                break;
            }
            if is_w(tree, ancestor, "hyperlink") {
                return Err(unsupported(
                    "already_hyperlinked",
                    "span is already inside a hyperlink; retarget it with Hyperlink.address. Nothing was changed",
                ));
            }
        }
    }
    if runs.len() > 1 {
        let last = runs[runs.len() - 1];
        let mut sibling = next_sibling(tree, runs[0]);
        while sibling != Some(last) {
            match sibling {
                Some(node) if runs.contains(&node) => sibling = next_sibling(tree, node),
                _ => {
                    return Err(unsupported(
                        "intervening_markup",
                        "cannot wrap a hyperlink around intervening markup; nothing was changed",
                    ));
                }
            }
        }
    }
    let story_part = txn.resolve(&span.part).ok_or_else(|| {
        unsupported(
            "story_without_part",
            format!(
                "span story {} has no package part; nothing was changed",
                span.story
            ),
        )
    })?;
    let r_id = txn.relate_to(&story_part, rt::HYPERLINK, RelTarget::External(address))?;
    let tree = txn.tree_mut(&span.part)?;
    let hyperlink = tree.create_element(runs[0], W, "hyperlink")?;
    add_previous(tree, runs[0], hyperlink)?;
    for run in &runs {
        append_moving(tree, hyperlink, *run)?;
        let rpr = match w_child(tree, *run, "rPr") {
            Some(rpr) => rpr,
            None => {
                let rpr = tree.create_element(*run, W, "rPr")?;
                tree.insert(*run, 0, rpr)?;
                rpr
            }
        };
        let style = match w_child(tree, rpr, "rStyle") {
            Some(style) => style,
            None => {
                let style = tree.create_element(rpr, W, "rStyle")?;
                tree.insert(rpr, 0, style)?;
                style
            }
        };
        tree.set_attr(style, Some(W), "val", "Hyperlink")?;
    }
    // Attributes last: declaring `r` on the new element before the runs
    // move in would change their namespace scope.
    tree.set_attr(hyperlink, Some(R_NS), "id", &r_id)?;
    tree.set_attr(hyperlink, Some(W), "history", "1")?;
    let text: String = tree
        .child_elements(hyperlink)
        .filter(|child| is_w(tree, *child, "r"))
        .map(|run| run_text(tree, run))
        .collect();
    let staged = txn.finish()?;
    document.commit(staged)?;
    document.consume(span.token);
    Ok(Hyperlink {
        address: address.to_string(),
        text,
    })
}

impl Document {
    /// Wrap `span` in a hyperlink to `address` (see [`add_hyperlink`]).
    pub fn add_hyperlink(&mut self, span: &Span, address: &str) -> Result<Hyperlink> {
        add_hyperlink(self, span, address)
    }
}
