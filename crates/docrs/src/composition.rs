//! Cross-document composition (upstream `docx.composition`, paper-docx
//! 0.2.1, MIT; behavior re-implemented in Rust).
//!
//! [`insert_blocks_from`] copies a block range of a source document's body
//! after a destination anchor; [`append_document`] appends a whole source
//! body (optionally with its letterhead). Both reconcile styles
//! (`match_by_name` or `import_renamed`), remap numbering to fresh
//! definitions, copy images as parts with fresh relationships, recreate
//! external hyperlinks, rename colliding bookmarks (remapping REF-style
//! fields), reallocate content-control ids and pad adjacent tables, and
//! return a [`CompositionReport`].
//!
//! Every refusal condition is checked before the first staged write; all
//! writes go to one [`Txn`] committed as the last step, so a refusal leaves
//! the destination unchanged. The source is only read (`&Document`).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use serde::Serialize;

use crate::anchor::{self, BlockTarget, Located};
use crate::blocks::field_open_flags;
use crate::bookmarks::pyclass::{is_py_alnum, is_py_alpha};
use crate::fields::fieldcode;
use crate::image::Image;
use crate::numbering::{add_next, add_previous};
use crate::oxml::{append, set_w, w_el};
use crate::package::rels::{Relationship, base_uri, rt, source_part_name};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::revision::is_scan_tag;
use crate::sections::{HeaderFooterRole, HeaderFooterType, add_definition, insert_settings_child};
use crate::story;
use crate::txn::{RelTarget, Txn};
use crate::unicode::casefold;
use crate::wml::{
    A, R, W, iter_nodes, leading_text, node_children, set_leading_text, w_attr, w_child,
};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, Error, Result};

const VML_NS: &str = "urn:schemas-microsoft-com:vml";
const OFFICE_NS: &str = "urn:schemas-microsoft-com:office:office";
const MAIN_STORY: &str = "word/document.xml";

const BODY_BLOCK_TAGS: &[&str] = &["p", "tbl", "sdt"];
const STYLE_REF_TAGS: &[&str] = &["pStyle", "rStyle", "tblStyle"];
const STYLE_CHAIN_TAGS: &[&str] = &["basedOn", "link", "next"];

/// Python `repr()` of a string (quote choice and escapes of control
/// characters), used in finding details that upstream formats with `!r`.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn is_w_any(tree: &XmlTree, node: NodeId, names: &[&str]) -> bool {
    tree.ns_uri(node) == Some(W)
        && tree
            .local_name(node)
            .is_some_and(|local| names.contains(&local))
}

fn is_w(tree: &XmlTree, node: NodeId, local: &str) -> bool {
    tree.is(node, W, local)
}

fn descendants_named(tree: &XmlTree, root: NodeId, ns: &str, local: &str) -> Vec<NodeId> {
    tree.descendants(root)
        .filter(|node| tree.is(*node, ns, local))
        .collect()
}

fn w_descendants(tree: &XmlTree, root: NodeId, local: &str) -> Vec<NodeId> {
    descendants_named(tree, root, W, local)
}

fn w_direct(tree: &XmlTree, parent: NodeId, local: &str) -> Vec<NodeId> {
    tree.child_elements(parent)
        .filter(|child| is_w(tree, *child, local))
        .collect()
}

fn py_int(raw: &str) -> Option<i64> {
    crate::bookmarks::py_int(raw)
}

fn char_slice(chars: &[char], start: usize, end: usize) -> String {
    let start = start.min(chars.len());
    let end = end.clamp(start, chars.len());
    chars[start..end].iter().collect()
}

/// How source styles reconcile with the destination (`styles=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StylesMode {
    /// A destination style with the same name and type wins.
    #[default]
    MatchByName,
    /// A colliding but different definition is cloned under a fresh id and
    /// name.
    ImportRenamed,
}

impl StylesMode {
    /// Parse upstream's token (`"match_by_name"` / `"import_renamed"`).
    pub fn parse(token: &str) -> Result<StylesMode> {
        match token {
            "match_by_name" => Ok(StylesMode::MatchByName),
            "import_renamed" => Ok(StylesMode::ImportRenamed),
            other => Err(Error::invalid_argument(
                "invalid_styles_mode",
                format!(
                    "styles must be 'match_by_name' or 'import_renamed', got {}",
                    py_repr(other)
                ),
            )),
        }
    }
}

/// How appended content starts (`section=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectionStart {
    /// A page-break paragraph precedes the appended content.
    #[default]
    NewPage,
    /// The content follows flush.
    Continuous,
}

impl SectionStart {
    /// Parse upstream's token (`"new_page"` / `"continuous"`).
    pub fn parse(token: &str) -> Result<SectionStart> {
        match token {
            "new_page" => Ok(SectionStart::NewPage),
            "continuous" => Ok(SectionStart::Continuous),
            other => Err(Error::invalid_argument(
                "invalid_section",
                format!(
                    "section must be 'new_page' or 'continuous', got {}",
                    py_repr(other)
                ),
            )),
        }
    }
}

/// Whose headers and footers the result keeps (`headers=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadersFrom {
    /// Keep the destination's letterhead.
    #[default]
    Destination,
    /// Copy the source's last-section letterhead over the destination's
    /// last section.
    Source,
}

impl HeadersFrom {
    /// Parse upstream's token (`"destination"` / `"source"`).
    pub fn parse(token: &str) -> Result<HeadersFrom> {
        match token {
            "destination" => Ok(HeadersFrom::Destination),
            "source" => Ok(HeadersFrom::Source),
            other => Err(Error::invalid_argument(
                "invalid_headers",
                format!(
                    "headers must be 'destination' or 'source', got {}",
                    py_repr(other)
                ),
            )),
        }
    }
}

/// Options of [`insert_blocks_from`] (upstream keyword arguments).
#[derive(Debug, Clone, Copy)]
pub struct InsertBlocksOptions<'a> {
    /// Source paragraph whose top-level block ends the range.
    pub end_anchor: Option<BlockTarget<'a>>,
    /// Blocks to copy when there is no end anchor (validated `>= 1` even
    /// with an end anchor).
    pub count: i64,
    /// Style reconciliation.
    pub styles: StylesMode,
    /// Include the start anchor's block.
    pub include_start: bool,
    /// Include the end anchor's block (requires an end anchor when false).
    pub include_end: bool,
}

impl Default for InsertBlocksOptions<'_> {
    fn default() -> Self {
        InsertBlocksOptions {
            end_anchor: None,
            count: 1,
            styles: StylesMode::MatchByName,
            include_start: true,
            include_end: true,
        }
    }
}

/// Options of [`append_document`].
#[derive(Debug, Clone, Copy, Default)]
pub struct AppendOptions {
    /// Page break before the appended content, or flush.
    pub section: SectionStart,
    /// Style reconciliation.
    pub styles: StylesMode,
    /// Letterhead source.
    pub headers: HeadersFrom,
}

/// A report-only finding (upstream `CompositionFinding`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompositionFinding {
    /// Finding kind (`bookmark_partially_in_range`,
    /// `letterhead_first_page_skipped`).
    pub kind: String,
    /// Human-readable detail.
    pub detail: String,
}

/// Everything one composition call did (upstream `CompositionReport`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CompositionReport {
    /// Body blocks inserted (padding paragraphs included).
    pub inserted_blocks: usize,
    /// Source style id → destination style id.
    pub style_map: BTreeMap<String, String>,
    /// Destination ids of imported style definitions, in import order.
    pub imported_styles: Vec<String>,
    /// Source style name → fresh destination name (`import_renamed`).
    pub renamed_styles: BTreeMap<String, String>,
    /// Source `w:numId` → destination `w:numId`.
    pub numbering_map: BTreeMap<i64, i64>,
    /// Image part names referenced by copied content (one per copied
    /// relationship, deduplicated parts repeat).
    pub media_copied: Vec<String>,
    /// Source bookmark name → fresh destination name.
    pub bookmarks_renamed: BTreeMap<String, String>,
    /// Report-only findings, in order.
    pub findings: Vec<CompositionFinding>,
    /// The declared changed-part budget of the body composition.
    pub declared_parts: Vec<String>,
}

impl CompositionReport {
    /// Upstream `to_dict()` (`schema: paper_composition`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        let mut imported = self.imported_styles.clone();
        imported.sort();
        let mut media = self.media_copied.clone();
        media.sort();
        let mut declared = self.declared_parts.clone();
        declared.sort();
        declared.dedup();
        let numbering: serde_json::Map<String, serde_json::Value> = self
            .numbering_map
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        serde_json::json!({
            "schema": "paper_composition",
            "version": 1,
            "inserted_blocks": self.inserted_blocks,
            "style_map": self.style_map,
            "imported_styles": imported,
            "renamed_styles": self.renamed_styles,
            "numbering_map": numbering,
            "media_copied": media,
            "bookmarks_renamed": self.bookmarks_renamed,
            "findings": self.findings,
            "declared_parts": declared,
        })
    }
}

/// Insertion-ordered string map with Python `dict` semantics.
#[derive(Debug, Clone, Default)]
struct OrderedMap {
    entries: Vec<(String, String)>,
}

impl OrderedMap {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn insert(&mut self, key: &str, value: &str) {
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value.to_string(),
            None => self.entries.push((key.to_string(), value.to_string())),
        }
    }

    fn remove(&mut self, key: &str) {
        self.entries.retain(|(k, _)| k != key);
    }

    fn update(&mut self, other: &OrderedMap) {
        for (k, v) in &other.entries {
            self.insert(k, v);
        }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn pairs(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    fn to_btree(&self) -> BTreeMap<String, String> {
        self.entries.iter().cloned().collect()
    }
}

/// The report while it is being built (dict insertion order matters for
/// case-insensitive rename lookups).
#[derive(Debug, Default)]
struct Progress {
    inserted_blocks: usize,
    style_map: OrderedMap,
    imported_styles: Vec<String>,
    renamed_styles: OrderedMap,
    numbering_map: Vec<(i64, i64)>,
    media_copied: Vec<String>,
    bookmarks_renamed: OrderedMap,
    findings: Vec<CompositionFinding>,
    declared_parts: Vec<String>,
}

impl Progress {
    fn numbering(&self, source_id: i64) -> Option<i64> {
        self.numbering_map
            .iter()
            .find(|(k, _)| *k == source_id)
            .map(|(_, v)| *v)
    }

    fn into_report(self) -> CompositionReport {
        CompositionReport {
            inserted_blocks: self.inserted_blocks,
            style_map: self.style_map.to_btree(),
            imported_styles: self.imported_styles,
            renamed_styles: self.renamed_styles.to_btree(),
            numbering_map: self.numbering_map.into_iter().collect(),
            media_copied: self.media_copied,
            bookmarks_renamed: self.bookmarks_renamed.to_btree(),
            findings: self.findings,
            declared_parts: self.declared_parts,
        }
    }
}

struct Source<'s> {
    doc: &'s Document,
    main: String,
    styles_part: Option<String>,
    /// python-docx's default styles, read when the source has no styles
    /// part (upstream would create that part on the source).
    default_styles: Option<XmlTree>,
    /// Relationships by source part, parsed once.
    rels: RefCell<HashMap<String, Rc<Vec<Relationship>>>>,
}

impl<'s> Source<'s> {
    fn new(doc: &'s Document) -> Result<Source<'s>> {
        let main = doc.package().main_part_name().to_string();
        let styles_part = crate::styles::styles_part(doc)?;
        let default_styles = match styles_part {
            Some(_) => None,
            None => Some(crate::styles::default_styles_tree()?),
        };
        Ok(Source {
            doc,
            main,
            styles_part,
            default_styles,
            rels: RefCell::new(HashMap::new()),
        })
    }

    fn tree(&self, part: &str) -> Result<&'s XmlTree> {
        self.doc.package().xml(part)
    }

    fn styles(&self) -> Result<&XmlTree> {
        match (&self.styles_part, &self.default_styles) {
            (Some(part), _) => self.doc.package().xml(part),
            (None, Some(tree)) => Ok(tree),
            (None, None) => Err(Error::internal("source_styles", "no source styles")),
        }
    }

    fn rels(&self, part: &str) -> Result<Rc<Vec<Relationship>>> {
        if let Some(rels) = self.rels.borrow().get(part) {
            return Ok(Rc::clone(rels));
        }
        let rels = Rc::new(
            self.doc
                .package()
                .relationships_at(&source_part_name(part))?,
        );
        self.rels
            .borrow_mut()
            .insert(part.to_string(), Rc::clone(&rels));
        Ok(rels)
    }

    fn target_member(&self, part: &str, rel: &Relationship) -> Option<String> {
        let target = rel.target_part_name(base_uri(&source_part_name(part)));
        self.doc
            .package()
            .resolve_member(&target)
            .map(str::to_string)
    }
}

/// python-docx `rels.get(rId)` (the last record wins for duplicates).
fn rel_by_id<'r>(rels: &'r [Relationship], r_id: &str) -> Option<&'r Relationship> {
    rels.iter().rev().find(|rel| rel.id == r_id)
}

fn body_of(tree: &XmlTree) -> Result<NodeId> {
    w_child(tree, tree.root(), "body")
        .ok_or_else(|| Error::unsupported("no_body", "the main document part has no w:body"))
}

/// Body children lxml iterates (elements, comments, PIs), `w:sectPr`
/// excluded.
fn body_children(tree: &XmlTree, body: NodeId) -> Vec<NodeId> {
    node_children(tree, body)
        .filter(|child| !is_w(tree, *child, "sectPr"))
        .collect()
}

/// python-docx `document.sections`: `./w:body/w:p/w:pPr/w:sectPr |
/// ./w:body/w:sectPr` in document order.
fn section_properties(tree: &XmlTree) -> Result<Vec<NodeId>> {
    let body = body_of(tree)?;
    let mut out = Vec::new();
    for child in tree.child_elements(body) {
        if is_w(tree, child, "p") {
            for ppr in w_direct(tree, child, "pPr") {
                out.extend(w_direct(tree, ppr, "sectPr"));
            }
        } else if is_w(tree, child, "sectPr") {
            out.push(child);
        }
    }
    Ok(out)
}

fn refused_reason(tree: &XmlTree, node: NodeId) -> Option<&'static str> {
    if tree.ns_uri(node) != Some(W) {
        return None;
    }
    Some(match tree.local_name(node)? {
        "object" => "an embedded OLE object",
        "altChunk" => "an altChunk import",
        "customXml" => "customXml content whose backing part cannot be carried",
        "dataBinding" => "a data-bound content control whose custom XML binding cannot be carried",
        "footnoteReference" => "a footnote reference (its note cannot be carried)",
        "endnoteReference" => "an endnote reference (its note cannot be carried)",
        "commentRangeStart" | "commentReference" => {
            "a comment anchor (composition cannot carry comments)"
        }
        "subDoc" => "a subdocument reference",
        _ => return None,
    })
}

/// Upstream `_refuse_unsupported_content`.
fn refuse_unsupported_content(tree: &XmlTree, elements: &[NodeId]) -> Result<()> {
    let refuse = |reason: &str| {
        Error::unsupported(
            "unsupported_content",
            format!(
                "the source range contains {reason}; composition cannot carry it (a declared limit)"
            ),
        )
    };
    for &element in elements {
        if !is_w_any(tree, element, BODY_BLOCK_TAGS) {
            let reason = match refused_reason(tree, element) {
                Some(reason) => reason.to_string(),
                None => {
                    let name = match tree.data(element) {
                        NodeData::Element(_) => tree.local_name(element).unwrap_or("").to_string(),
                        NodeData::Comment(_) => "comment".to_string(),
                        NodeData::Pi { .. } => "processing instruction".to_string(),
                        NodeData::Text(_) => "text".to_string(),
                    };
                    format!("unsupported top-level body content {}", py_repr(&name))
                }
            };
            return Err(refuse(&reason));
        }
        for node in iter_nodes(tree, element) {
            if is_scan_tag(tree, node) {
                return Err(Error::unsupported(
                    "tracked_revision_markup",
                    "the source range carries tracked-revision markup; accept or reject the source revisions \
                     first, then compose",
                ));
            }
            if let Some(reason) = refused_reason(tree, node) {
                return Err(refuse(reason));
            }
        }
    }
    Ok(())
}

/// Upstream `_refuse_malformed_numeric_ids` over `(tree, root)` pairs.
fn refuse_malformed_numeric_ids(roots: &[(&XmlTree, NodeId)]) -> Result<()> {
    for &(tree, root) in roots {
        for marker in tree.descendants(root) {
            if !is_w_any(tree, marker, &["bookmarkStart", "bookmarkEnd"]) {
                continue;
            }
            let raw = w_attr(tree, marker, "id");
            let value = raw.and_then(py_int).unwrap_or(-1);
            if value < 0 {
                return Err(Error::unsupported(
                    "malformed_bookmark_id",
                    format!(
                        "composition found a bookmark marker with a missing or non-numeric w:id {}; nothing was \
                         changed",
                        raw.map_or("None".to_string(), py_repr)
                    ),
                ));
            }
        }
        for sdt_pr in w_descendants(tree, root, "sdtPr") {
            let Some(raw) = w_child(tree, sdt_pr, "id").and_then(|id| w_attr(tree, id, "val"))
            else {
                continue;
            };
            if py_int(raw).unwrap_or(-1) < 0 {
                return Err(Error::unsupported(
                    "malformed_control_id",
                    format!(
                        "composition found a content control with a non-numeric w:id {}; nothing was changed",
                        py_repr(raw)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Upstream `bookmarks._iter_field_instructions`: (concatenated
/// instruction, `w:instrText` nodes) per complex field closed under `root`.
pub(crate) fn iter_field_instructions(tree: &XmlTree, root: NodeId) -> Vec<(String, Vec<NodeId>)> {
    let mut stack: Vec<(Vec<NodeId>, bool)> = Vec::new();
    let mut out = Vec::new();
    for node in iter_nodes(tree, root) {
        if is_w(tree, node, "fldChar") {
            match w_attr(tree, node, "fldCharType") {
                Some("begin") => stack.push((Vec::new(), true)),
                Some("separate") => {
                    if let Some(top) = stack.last_mut() {
                        top.1 = false;
                    }
                }
                Some("end") => {
                    if let Some((nodes, _)) = stack.pop() {
                        let text: String = nodes.iter().map(|n| leading_text(tree, *n)).collect();
                        out.push((text, nodes));
                    }
                }
                _ => {}
            }
        } else if is_w(tree, node, "instrText")
            && let Some(frame) = stack.iter_mut().rev().find(|frame| frame.1)
        {
            frame.0.push(node);
        }
    }
    out
}

fn simple_field_instructions(tree: &XmlTree, root: NodeId) -> Vec<(NodeId, String)> {
    w_descendants(tree, root, "fldSimple")
        .into_iter()
        .filter_map(|field| {
            w_attr(tree, field, "instr")
                .filter(|instr| !instr.is_empty())
                .map(|instr| (field, instr.to_string()))
        })
        .collect()
}

fn contains_fields(tree: &XmlTree, elements: &[NodeId]) -> bool {
    elements.iter().any(|element| {
        tree.descendants(*element)
            .any(|node| is_w_any(tree, node, &["fldSimple", "fldChar"]))
    })
}

/// Upstream `_preflight_bookmark_references`.
fn preflight_bookmark_references(
    source: &Source<'_>,
    tree: &XmlTree,
    elements: &[NodeId],
) -> Result<()> {
    let bookmarks = crate::bookmarks::list_bookmarks(source.doc)?;
    let mut by_fold: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for bookmark in &bookmarks {
        let names = by_fold.entry(casefold(&bookmark.name)).or_default();
        if !names.contains(&bookmark.name) {
            names.push(bookmark.name.clone());
        }
    }
    let ambiguous: Vec<Vec<String>> = by_fold
        .into_values()
        .filter(|names| names.len() > 1)
        .map(|mut names| {
            names.sort();
            names
        })
        .collect();
    if !ambiguous.is_empty() {
        return Err(Error::unsupported(
            "ambiguous_source_bookmarks",
            format!("source bookmark names are case-ambiguous: {ambiguous:?}; nothing was changed"),
        ));
    }
    let mut starts: HashMap<Option<String>, String> = HashMap::new();
    let mut ends: HashSet<Option<String>> = HashSet::new();
    for &element in elements {
        for start in w_descendants(tree, element, "bookmarkStart") {
            starts.insert(
                w_attr(tree, start, "id").map(str::to_string),
                w_attr(tree, start, "name").unwrap_or("").to_string(),
            );
        }
        for end in w_descendants(tree, element, "bookmarkEnd") {
            ends.insert(w_attr(tree, end, "id").map(str::to_string));
        }
    }
    let copied: HashSet<String> = starts
        .iter()
        .filter(|(id, _)| ends.contains(*id))
        .map(|(_, name)| casefold(name))
        .collect();
    let known: Vec<&str> = bookmarks.iter().map(|b| b.name.as_str()).collect();
    let validate = |instruction: &str| -> Result<()> {
        for operand in fieldcode::bookmark_operands(instruction, &known)? {
            if !copied.contains(&casefold(&operand.value)) {
                return Err(Error::unsupported(
                    "bookmark_reference_outside_range",
                    format!(
                        "the source range references bookmark {}, but its definition is outside the copied range. \
                         Nothing was changed",
                        py_repr(&operand.value)
                    ),
                ));
            }
        }
        Ok(())
    };
    for &element in elements {
        for (instruction, _) in iter_field_instructions(tree, element) {
            validate(&instruction)?;
        }
        for (_, instruction) in simple_field_instructions(tree, element) {
            validate(&instruction)?;
        }
        for hyperlink in w_descendants(tree, element, "hyperlink") {
            if let Some(anchor) = w_attr(tree, hyperlink, "anchor").filter(|a| !a.is_empty())
                && !copied.contains(&casefold(anchor))
            {
                return Err(Error::unsupported(
                    "bookmark_link_outside_range",
                    format!(
                        "the source range links to bookmark {}, but its definition is outside the copied range. \
                         Nothing was changed",
                        py_repr(anchor)
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn refuse_relationship(
    tree: &XmlTree,
    node: NodeId,
    attribute: &str,
    r_id: &str,
    rel: Option<&Relationship>,
    expected: Option<&str>,
) -> Error {
    let kind = match rel {
        None => expected.unwrap_or("unresolved").to_string(),
        Some(_) if expected == Some("linked image") => "linked image".to_string(),
        Some(rel) => rel.rel_type.rsplit('/').next().unwrap_or("").to_string(),
    };
    Error::unsupported(
        "unsupported_relationship",
        format!(
            "the source range contains an unsupported {kind} relationship ({}@{attribute}={}); composition cannot \
             remap it. Nothing was changed",
            tree.local_name(node).unwrap_or(""),
            py_repr(r_id)
        ),
    )
}

/// Upstream `_preflight_relationships`: every relationship-valued attribute
/// must be a remappable embedded image or external hyperlink.
fn preflight_relationships(
    source: &Source<'_>,
    part: &str,
    tree: &XmlTree,
    elements: &[NodeId],
) -> Result<()> {
    let rels = source.rels(part)?;
    for &element in elements {
        for node in tree.descendants(element) {
            let Some(data) = tree.element(node) else {
                continue;
            };
            for attr in &data.attrs {
                let ns = attr.ns.map(|ns| tree.uri(ns));
                let local = &*attr.name.local;
                if !(ns == Some(R) || (ns == Some(OFFICE_NS) && local == "relid")) {
                    continue;
                }
                let r_id = attr.value.as_str();
                let rel = rel_by_id(&rels, r_id);
                let is_r = ns == Some(R);
                let image_ok = || rel.is_some_and(|rel| rel.rel_type == rt::IMAGE && !rel.external);
                if tree.is(node, A, "blip") && is_r && local == "embed" {
                    if image_ok() {
                        continue;
                    }
                    return Err(refuse_relationship(
                        tree,
                        node,
                        local,
                        r_id,
                        rel,
                        Some("embedded image"),
                    ));
                }
                if tree.is(node, VML_NS, "imagedata") && is_r && local == "id" {
                    if image_ok() {
                        continue;
                    }
                    return Err(refuse_relationship(
                        tree,
                        node,
                        local,
                        r_id,
                        rel,
                        Some("embedded image"),
                    ));
                }
                if is_w(tree, node, "hyperlink") && is_r && local == "id" {
                    if rel.is_some_and(|rel| rel.rel_type == rt::HYPERLINK && rel.external) {
                        continue;
                    }
                    return Err(refuse_relationship(
                        tree,
                        node,
                        local,
                        r_id,
                        rel,
                        Some("external hyperlink"),
                    ));
                }
                let expected = (is_r && local == "link").then_some("linked image");
                return Err(refuse_relationship(tree, node, local, r_id, rel, expected));
            }
        }
    }
    Ok(())
}

/// `(node, attribute, rId)` of every image reference under `elements`
/// that names a relationship.
fn image_references(tree: &XmlTree, elements: &[NodeId]) -> Vec<(NodeId, &'static str, String)> {
    let mut out = Vec::new();
    for &element in elements {
        for node in tree.descendants(element) {
            let attr = if tree.is(node, A, "blip") {
                "embed"
            } else if tree.is(node, VML_NS, "imagedata") {
                "id"
            } else {
                continue;
            };
            if let Some(r_id) = tree.attr(node, Some(R), attr).filter(|v| !v.is_empty()) {
                out.push((node, attr, r_id.to_string()));
            }
        }
    }
    out
}

fn image_relationship_changed(r_id: &str) -> Error {
    Error::unsupported(
        "image_relationship_changed",
        format!(
            "image relationship {} changed after composition preflight. Nothing was changed",
            py_repr(r_id)
        ),
    )
}

/// Upstream `_refuse_unloadable_media`: every image must be one this
/// package can re-embed.
fn refuse_unloadable_media(
    source: &Source<'_>,
    part: &str,
    tree: &XmlTree,
    elements: &[NodeId],
) -> Result<()> {
    let rels = source.rels(part)?;
    for (_, _, r_id) in image_references(tree, elements) {
        let r_id = r_id.as_str();
        let rel = rel_by_id(&rels, r_id).filter(|rel| !rel.external);
        let Some(rel) = rel else {
            return Err(image_relationship_changed(r_id));
        };
        let blob = image_blob(source, part, rel).ok_or_else(|| image_relationship_changed(r_id))?;
        if let Err(err) = Image::from_vec(blob) {
            if err.reason() == "unrecognized_image" {
                return Err(Error::unsupported(
                    "unembeddable_image",
                    "the source range contains an image format this package cannot re-embed (e.g. EMF/WMF); \
                     convert or remove it first. Nothing was changed",
                ));
            }
            return Err(err);
        }
    }
    Ok(())
}

fn image_blob(source: &Source<'_>, part: &str, rel: &Relationship) -> Option<Vec<u8>> {
    let member = source.target_member(part, rel)?;
    let part = source.doc.package().part(&member)?;
    Some(part.current_bytes().into_owned())
}

/// Upstream `_style_definitions`: `w:style` children by id and by
/// (casefolded name, type).
struct StyleDefs {
    by_id: HashMap<String, NodeId>,
    by_name: HashMap<(String, String), NodeId>,
}

fn style_definitions(tree: &XmlTree) -> StyleDefs {
    let mut by_id = HashMap::new();
    let mut by_name = HashMap::new();
    for style in w_direct(tree, tree.root(), "style") {
        if let Some(id) = w_attr(tree, style, "styleId").filter(|id| !id.is_empty()) {
            by_id.insert(id.to_string(), style);
        }
        let name = w_child(tree, style, "name").and_then(|n| w_attr(tree, n, "val"));
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            let kind = w_attr(tree, style, "type")
                .filter(|t| !t.is_empty())
                .unwrap_or("paragraph");
            by_name.insert((casefold(name), kind.to_string()), style);
        }
    }
    StyleDefs { by_id, by_name }
}

fn referenced_style_ids(tree: &XmlTree, elements: &[NodeId]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for &element in elements {
        for tag in STYLE_REF_TAGS {
            for node in w_descendants(tree, element, tag) {
                if let Some(value) = w_attr(tree, node, "val").filter(|v| !v.is_empty())
                    && !seen.iter().any(|s| s == value)
                {
                    seen.push(value.to_string());
                }
            }
        }
    }
    seen
}

fn styleref_style_ids(
    tree: &XmlTree,
    elements: &[NodeId],
    styles: &XmlTree,
    defs: &StyleDefs,
) -> Result<Vec<String>> {
    let mut instructions = Vec::new();
    for &element in elements {
        instructions.extend(
            iter_field_instructions(tree, element)
                .into_iter()
                .map(|(text, _)| text),
        );
        instructions.extend(
            simple_field_instructions(tree, element)
                .into_iter()
                .map(|(_, text)| text),
        );
    }
    let mut ids: Vec<String> = Vec::new();
    for instruction in instructions {
        let Some(operand) = fieldcode::command_operand(&instruction, "STYLEREF")? else {
            continue;
        };
        let key = (casefold(&operand.value), "paragraph".to_string());
        let Some(definition) = defs.by_name.get(&key) else {
            continue;
        };
        if let Some(id) = w_attr(styles, *definition, "styleId").filter(|id| !id.is_empty())
            && !ids.iter().any(|existing| existing == id)
        {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

/// Upstream `_expand_style_chain`: breadth-first over basedOn/link/next.
fn expand_style_chain(
    styles: &XmlTree,
    by_id: &HashMap<String, NodeId>,
    wanted: &[String],
) -> Result<Vec<String>> {
    let mut ordered: Vec<String> = Vec::new();
    let mut queue: VecDeque<(String, Option<String>, Option<&str>)> =
        wanted.iter().map(|id| (id.clone(), None, None)).collect();
    while let Some((style_id, parent, tag)) = queue.pop_front() {
        if ordered.contains(&style_id) {
            continue;
        }
        let Some(&definition) = by_id.get(&style_id) else {
            let owner = match (parent, tag) {
                (Some(parent), Some(tag)) => {
                    format!("source style {} through w:{tag}", py_repr(&parent))
                }
                _ => "the source content".to_string(),
            };
            return Err(Error::unsupported(
                "undefined_source_style",
                format!(
                    "{owner} references undefined source style {}; nothing was changed",
                    py_repr(&style_id)
                ),
            ));
        };
        ordered.push(style_id.clone());
        for tag in STYLE_CHAIN_TAGS {
            for chained in w_direct(styles, definition, tag) {
                if let Some(value) = w_attr(styles, chained, "val").filter(|v| !v.is_empty()) {
                    queue.push_back((value.to_string(), Some(style_id.clone()), Some(tag)));
                }
            }
        }
    }
    Ok(ordered)
}

/// Upstream `_normalized_style_bytes`: the definition without `styleId` and
/// rsid attributes, as a canonical string (prefixes, namespaces, attribute
/// order, text, comments and PIs; blank text between elements ignored, as
/// python-docx parses with `remove_blank_text`).
fn normalized_style(tree: &XmlTree, node: NodeId) -> String {
    fn walk(tree: &XmlTree, node: NodeId, root: bool, out: &mut String) {
        match tree.data(node) {
            NodeData::Element(element) => {
                out.push('<');
                out.push_str(&element.name.lexical());
                out.push('{');
                out.push_str(element.ns.map(|ns| tree.uri(ns)).unwrap_or(""));
                out.push('}');
                for attr in &element.attrs {
                    let ns = attr.ns.map(|ns| tree.uri(ns)).unwrap_or("");
                    let clark = if ns.is_empty() {
                        attr.name.local.to_string()
                    } else {
                        format!("{{{ns}}}{}", attr.name.local)
                    };
                    if (root && clark.ends_with("}styleId")) || (root && clark.contains("rsid")) {
                        continue;
                    }
                    out.push(' ');
                    out.push_str(&attr.name.lexical());
                    out.push('{');
                    out.push_str(ns);
                    out.push_str("}=");
                    out.push_str(&format!("{:?}", attr.value));
                }
                out.push('>');
                let has_elements = tree.child_elements(node).next().is_some();
                for child in tree.children(node) {
                    if let NodeData::Text(text) = tree.data(*child)
                        && has_elements
                        && text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
                    {
                        continue;
                    }
                    walk(tree, *child, false, out);
                }
                out.push_str("</>");
            }
            NodeData::Text(text) => out.push_str(&format!("T{text:?}")),
            NodeData::Comment(text) => out.push_str(&format!("C{text:?}")),
            NodeData::Pi { target, data } => out.push_str(&format!("P{target:?}{data:?}")),
        }
    }
    let mut out = String::new();
    walk(tree, node, true, &mut out);
    out
}

fn fresh_style_id(taken: &HashSet<String>, base: &str) -> String {
    let mut candidate = format!("{base}Imported");
    let mut counter = 1;
    while taken.contains(&candidate) {
        counter += 1;
        candidate = format!("{base}Imported{counter}");
    }
    candidate
}

fn fresh_style_name(by_name: &HashMap<(String, String), NodeId>, base: &str, kind: &str) -> String {
    let mut candidate = format!("{base} (imported)");
    let mut counter = 1;
    while by_name.contains_key(&(casefold(&candidate), kind.to_string())) {
        counter += 1;
        candidate = format!("{base} (imported {counter})");
    }
    candidate
}

#[derive(Debug, Clone)]
struct NumberingRemap {
    source_num_id: i64,
    destination_num_id: i64,
    destination_abstract_id: i64,
    source_num: NodeId,
    source_abstract: NodeId,
}

#[derive(Debug, Clone, Default)]
struct NumberingPlan {
    remaps: Vec<NumberingRemap>,
}

fn parse_numbering_id(raw: Option<&str>, prefix: &str) -> Result<i64> {
    let value = raw.and_then(py_int).unwrap_or(-1);
    if value < 0 {
        return Err(Error::unsupported(
            "malformed_numbering_id",
            format!(
                "{prefix} missing or non-numeric id {}. Nothing was changed",
                raw.map_or("None".to_string(), py_repr)
            ),
        ));
    }
    Ok(value)
}

fn referenced_numbering_ids(elements: &[(&XmlTree, NodeId)]) -> Result<Vec<i64>> {
    let mut referenced = Vec::new();
    for &(tree, element) in elements {
        for num_id in w_descendants(tree, element, "numId") {
            let value = parse_numbering_id(
                w_attr(tree, num_id, "val"),
                "source content has a w:numId with",
            )?;
            if value > 0 && !referenced.contains(&value) {
                referenced.push(value);
            }
        }
    }
    Ok(referenced)
}

/// Ordered int-keyed index (Python dict of int → element).
type Indexed = Vec<(i64, NodeId)>;

fn lookup_indexed(index: &Indexed, key: i64) -> Option<NodeId> {
    index.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn index_numbering_elements(
    tree: &XmlTree,
    elements: Vec<NodeId>,
    attribute: &str,
    label: &str,
    element_name: &str,
) -> Result<Indexed> {
    let mut indexed: Indexed = Vec::new();
    for element in elements {
        let value = parse_numbering_id(
            w_attr(tree, element, attribute),
            &format!("{label} has a {element_name} with"),
        )?;
        if lookup_indexed(&indexed, value).is_some() {
            return Err(Error::unsupported(
                "duplicate_numbering_id",
                format!(
                    "{label} contains duplicate {element_name} id {value}. Nothing was changed"
                ),
            ));
        }
        indexed.push((value, element));
    }
    Ok(indexed)
}

struct NumberingGraph {
    nums: Indexed,
    abstracts: Indexed,
    abstract_refs: Vec<(i64, i64)>,
}

/// Upstream `_validated_numbering_graph`.
fn validated_numbering_graph(tree: &XmlTree, label: &str) -> Result<NumberingGraph> {
    let root = tree.root();
    let pictures = index_numbering_elements(
        tree,
        w_direct(tree, root, "numPicBullet"),
        "numPicBulletId",
        label,
        "w:numPicBullet",
    )?;
    let abstracts = index_numbering_elements(
        tree,
        w_direct(tree, root, "abstractNum"),
        "abstractNumId",
        label,
        "w:abstractNum",
    )?;
    let nums =
        index_numbering_elements(tree, w_direct(tree, root, "num"), "numId", label, "w:num")?;
    let mut abstract_refs = Vec::new();
    for &(num_id, num) in &nums {
        let refs = w_direct(tree, num, "abstractNumId");
        if refs.len() != 1 {
            return Err(Error::unsupported(
                "malformed_numbering_reference",
                format!(
                    "{label} w:num {num_id} must contain exactly one w:abstractNumId. Nothing was changed"
                ),
            ));
        }
        let abstract_id = parse_numbering_id(
            w_attr(tree, refs[0], "val"),
            &format!("{label} w:num {num_id} has a w:abstractNumId with"),
        )?;
        if lookup_indexed(&abstracts, abstract_id).is_none() {
            return Err(Error::unsupported(
                "missing_abstract_numbering",
                format!(
                    "{label} w:num {num_id} references missing abstract numbering definition {abstract_id}. Nothing \
                     was changed"
                ),
            ));
        }
        abstract_refs.push((num_id, abstract_id));
    }
    for &(abstract_id, abstract_num) in &abstracts {
        for picture_ref in w_descendants(tree, abstract_num, "lvlPicBulletId") {
            let picture_id = parse_numbering_id(
                w_attr(tree, picture_ref, "val"),
                &format!("{label} w:abstractNum {abstract_id} has a w:lvlPicBulletId with"),
            )?;
            if lookup_indexed(&pictures, picture_id).is_none() {
                return Err(Error::unsupported(
                    "missing_picture_bullet",
                    format!(
                        "{label} w:abstractNum {abstract_id} references missing picture-bullet definition \
                         {picture_id}. Nothing was changed"
                    ),
                ));
            }
        }
    }
    Ok(NumberingGraph {
        nums,
        abstracts,
        abstract_refs,
    })
}

/// Upstream `_fresh_bookmark_name`: a Word-legal name of at most 40
/// characters not used (casefolded) in `existing`.
fn fresh_bookmark_name(existing: &HashSet<String>, base: &str) -> String {
    let mut stem: String = base
        .chars()
        .map(|c| if c == '_' || is_py_alnum(c) { c } else { '_' })
        .collect();
    if stem.chars().next().is_none_or(|c| !is_py_alpha(c)) {
        stem = format!("B_{stem}");
    }
    let stem_chars: Vec<char> = stem.chars().collect();
    let mut counter = 1;
    loop {
        let suffix = if counter == 1 {
            "_imported".to_string()
        } else {
            format!("_imported{counter}")
        };
        let keep = 40usize.saturating_sub(suffix.chars().count());
        let candidate = format!("{}{suffix}", char_slice(&stem_chars, 0, keep));
        if !existing.contains(&casefold(&candidate)) {
            return candidate;
        }
        counter += 1;
    }
}

/// Write a rewritten instruction back across its `w:instrText` nodes.
fn redistribute(tree: &mut XmlTree, nodes: &[NodeId], instruction: &str) -> Result<()> {
    let chars: Vec<char> = instruction.chars().collect();
    let mut offset = 0usize;
    for (position, node) in nodes.iter().enumerate() {
        if position == nodes.len() - 1 {
            set_leading_text(tree, *node, &char_slice(&chars, offset, chars.len()))?;
            break;
        }
        let width = leading_text(tree, *node).chars().count();
        set_leading_text(tree, *node, &char_slice(&chars, offset, offset + width))?;
        offset += width;
    }
    Ok(())
}

/// A composition in progress: one transaction over the destination.
struct Composer<'d, 's> {
    txn: Txn<'d>,
    source: Source<'s>,
    progress: Progress,
    /// Story parts created by this transaction (letterhead definitions).
    created: Vec<String>,
}

impl<'d, 's> Composer<'d, 's> {
    fn main(&self) -> String {
        self.txn.document().package().main_part_name().to_string()
    }

    /// Story parts of the destination as they will be after commit.
    fn story_parts(&self) -> Vec<String> {
        let mut parts: Vec<String> = story::stories(self.txn.document())
            .into_iter()
            .map(|story| story.part)
            .collect();
        parts.extend(self.created.iter().cloned());
        parts
    }

    fn story_roots(&self) -> Result<Vec<(&XmlTree, NodeId)>> {
        let mut out = Vec::new();
        for part in self.story_parts() {
            let tree = self.txn.tree(&part)?;
            out.push((tree, tree.root()));
        }
        Ok(out)
    }

    fn refuse_malformed_numeric_ids(&self, tree: &XmlTree, elements: &[NodeId]) -> Result<()> {
        let mut roots = self.story_roots()?;
        roots.extend(elements.iter().map(|element| (tree, *element)));
        refuse_malformed_numeric_ids(&roots)
    }

    /// Upstream `_chained_source_definitions`: the source style ids
    /// `elements` reach, in chain order, with their definitions.
    fn chained_source_definitions(
        &self,
        tree: &XmlTree,
        elements: &[NodeId],
    ) -> Result<(Vec<String>, Vec<NodeId>)> {
        let styles = self.source.styles()?;
        let defs = style_definitions(styles);
        let mut referenced = referenced_style_ids(tree, elements);
        referenced.extend(styleref_style_ids(tree, elements, styles, &defs)?);
        let ids = expand_style_chain(styles, &defs.by_id, &referenced)?;
        let definitions = ids.iter().map(|id| defs.by_id[id]).collect();
        Ok((ids, definitions))
    }

    /// Upstream `_preflight_numbering`: validate both graphs and allocate
    /// every remap.
    fn preflight_numbering(&self, elements: &[(&XmlTree, NodeId)]) -> Result<NumberingPlan> {
        let referenced = referenced_numbering_ids(elements)?;
        if referenced.is_empty() {
            return Ok(NumberingPlan::default());
        }
        let source_part = self
            .source
            .doc
            .package()
            .part_related_by(&self.source.main, rt::NUMBERING)?
            .ok_or_else(|| {
                Error::unsupported(
                    "source_numbering_missing",
                    "the source content references numbering but the source document has no numbering part. \
                     Nothing was changed",
                )
            })?;
        let destination_part = self.txn.part_related_by(&self.main(), rt::NUMBERING)?.ok_or_else(|| {
            Error::unsupported(
                "destination_numbering_missing",
                "the destination document has no numbering part; create a list definition first \
                 (ensure_bullet_definition / ensure_decimal_definition)",
            )
        })?;
        let source_tree = self.source.tree(&source_part)?;
        let source_graph = validated_numbering_graph(source_tree, "source numbering")?;
        let destination_graph =
            validated_numbering_graph(self.txn.tree(&destination_part)?, "destination numbering")?;
        let first_num_id = destination_graph
            .nums
            .iter()
            .map(|(k, _)| *k)
            .max()
            .unwrap_or(0)
            + 1;
        let first_abstract_id = destination_graph
            .abstracts
            .iter()
            .map(|(k, _)| *k)
            .max()
            .unwrap_or(-1)
            + 1;
        let mut remaps = Vec::new();
        for (offset, source_num_id) in (0i64..).zip(referenced) {
            let Some(source_num) = lookup_indexed(&source_graph.nums, source_num_id) else {
                return Err(Error::unsupported(
                    "undefined_source_numbering",
                    format!(
                        "source numbering id {source_num_id} has no w:num definition. Nothing was changed"
                    ),
                ));
            };
            let abstract_id = source_graph
                .abstract_refs
                .iter()
                .find(|(num, _)| *num == source_num_id)
                .map(|(_, a)| *a)
                .ok_or_else(|| {
                    Error::internal("numbering_graph", "validated num lost its abstract")
                })?;
            let source_abstract = lookup_indexed(&source_graph.abstracts, abstract_id)
                .ok_or_else(|| Error::internal("numbering_graph", "validated abstract vanished"))?;
            if source_tree
                .descendants(source_abstract)
                .any(|node| is_w_any(source_tree, node, &["numStyleLink", "styleLink"]))
            {
                return Err(Error::unsupported(
                    "linked_numbering_style",
                    format!(
                        "source numbering id {source_num_id} depends on a linked numbering style that composition \
                         cannot reconcile. Nothing was changed"
                    ),
                ));
            }
            if !w_descendants(source_tree, source_abstract, "lvlPicBulletId").is_empty() {
                return Err(Error::unsupported(
                    "picture_bullet",
                    format!(
                        "source numbering id {source_num_id} uses a picture bullet; composition cannot carry its \
                         numbering-part relationships. Nothing was changed"
                    ),
                ));
            }
            remaps.push(NumberingRemap {
                source_num_id,
                destination_num_id: first_num_id + offset,
                destination_abstract_id: first_abstract_id + offset,
                source_num,
                source_abstract,
            });
        }
        Ok(NumberingPlan { remaps })
    }

    /// Deep copies of source `elements` as detached nodes of the
    /// destination `part`, scoped for insertion under `scope`.
    fn import(
        &mut self,
        part: &str,
        scope: NodeId,
        tree: &XmlTree,
        elements: &[NodeId],
    ) -> Result<Vec<NodeId>> {
        let destination = self.txn.tree_mut(part)?;
        elements
            .iter()
            .map(|element| destination.import_subtree(tree, *element, scope))
            .collect()
    }

    /// Upstream `_reconcile_styles` over the source style ids `wanted` (what
    /// `chained_source_definitions` found for the copied elements): returns
    /// the imported definitions (in the destination styles part).
    fn reconcile_styles(
        &mut self,
        part: &str,
        clones: &[NodeId],
        wanted: &[String],
        mode: StylesMode,
    ) -> Result<(String, Vec<NodeId>)> {
        let styles_part = crate::parts::ensure_styles_part(&mut self.txn)?;
        let source_styles = self.source.styles()?;
        let source_defs = style_definitions(source_styles);
        let (plan, renamed_now) = {
            let destination_tree = self.txn.tree(&styles_part)?;
            let destination_defs = style_definitions(destination_tree);
            let mut style_map = OrderedMap::default();
            let mut to_import: Vec<(String, NodeId, Option<String>)> = Vec::new();
            let mut renamed = OrderedMap::default();
            let mut taken: HashSet<String> = destination_defs.by_id.keys().cloned().collect();
            for style_id in wanted {
                if let Some(mapped) = self.progress.style_map.get(style_id) {
                    style_map.insert(style_id, mapped);
                    continue;
                }
                let definition = source_defs.by_id[style_id];
                let name = match w_child(source_styles, definition, "name") {
                    None => style_id.clone(),
                    Some(name) => w_attr(source_styles, name, "val")
                        .ok_or_else(|| {
                            Error::unsupported(
                                "style_name_without_value",
                                format!(
                                    "source style {} has a w:name without w:val",
                                    py_repr(style_id)
                                ),
                            )
                        })?
                        .to_string(),
                };
                let kind = w_attr(source_styles, definition, "type")
                    .filter(|t| !t.is_empty())
                    .unwrap_or("paragraph")
                    .to_string();
                if let Some(&existing) = destination_defs
                    .by_name
                    .get(&(casefold(&name), kind.clone()))
                {
                    let existing_id = || {
                        w_attr(destination_tree, existing, "styleId")
                            .map(str::to_string)
                            .ok_or_else(|| {
                                Error::unsupported(
                                    "destination_style_without_id",
                                    format!(
                                        "destination style {} has no w:styleId",
                                        py_repr(&name)
                                    ),
                                )
                            })
                    };
                    if mode == StylesMode::MatchByName
                        || normalized_style(destination_tree, existing)
                            == normalized_style(source_styles, definition)
                    {
                        style_map.insert(style_id, &existing_id()?);
                        continue;
                    }
                    let new_id = fresh_style_id(&taken, style_id);
                    taken.insert(new_id.clone());
                    let new_name = fresh_style_name(&destination_defs.by_name, &name, &kind);
                    style_map.insert(style_id, &new_id);
                    renamed.insert(&name, &new_name);
                    to_import.push((new_id, definition, Some(new_name)));
                    continue;
                }
                let new_id = if taken.contains(style_id) {
                    fresh_style_id(&taken, style_id)
                } else {
                    style_id.clone()
                };
                taken.insert(new_id.clone());
                style_map.insert(style_id, &new_id);
                to_import.push((new_id, definition, None));
            }
            ((style_map, to_import), renamed)
        };
        let (style_map, to_import) = plan;
        self.progress.renamed_styles.update(&renamed_now);
        let mut imported = Vec::new();
        {
            let tree = self.txn.tree_mut(&styles_part)?;
            let root = tree.root();
            for (new_id, definition, new_name) in &to_import {
                let clone = tree.import_subtree(source_styles, *definition, root)?;
                set_w(tree, clone, "styleId", new_id)?;
                if let Some(new_name) = new_name
                    && let Some(name) = w_child(tree, clone, "name")
                {
                    set_w(tree, name, "val", new_name)?;
                }
                for tag in STYLE_CHAIN_TAGS {
                    for chained in w_direct(tree, clone, tag) {
                        match w_attr(tree, chained, "val").and_then(|v| style_map.get(v)) {
                            Some(mapped) => {
                                let mapped = mapped.to_string();
                                set_w(tree, chained, "val", &mapped)?;
                            }
                            None => tree.detach(chained)?,
                        }
                    }
                }
                append(tree, root, clone)?;
                self.progress.imported_styles.push(new_id.clone());
                imported.push(clone);
            }
        }
        {
            let tree = self.txn.tree_mut(part)?;
            for &clone in clones {
                for tag in STYLE_REF_TAGS {
                    for node in w_descendants(tree, clone, tag) {
                        let mapped = w_attr(tree, node, "val")
                            .and_then(|value| {
                                style_map.get(value).filter(|mapped| *mapped != value)
                            })
                            .map(str::to_string);
                        if let Some(mapped) = mapped {
                            set_w(tree, node, "val", &mapped)?;
                        }
                    }
                }
            }
        }
        self.progress.style_map.update(&style_map);
        if !self.progress.renamed_styles.is_empty() {
            let renames = self.progress.renamed_styles.clone();
            let tree = self.txn.tree_mut(part)?;
            remap_styleref_fields(tree, clones, &renames)?;
        }
        Ok((styles_part, imported))
    }

    /// Upstream `_remap_numbering` over clone sets of possibly different
    /// destination parts.
    fn remap_numbering(
        &mut self,
        sets: &[(String, Vec<NodeId>)],
        plan: &NumberingPlan,
    ) -> Result<()> {
        let mut referenced: Vec<i64> = Vec::new();
        for (part, nodes) in sets {
            let tree = self.txn.tree(part)?;
            for &node in nodes {
                for num_id in w_descendants(tree, node, "numId") {
                    if let Some(value) = w_attr(tree, num_id, "val")
                        .filter(|v| !v.is_empty())
                        .and_then(py_int)
                        && value > 0
                        && !referenced.contains(&value)
                    {
                        referenced.push(value);
                    }
                }
            }
        }
        if referenced.is_empty() {
            return Ok(());
        }
        let numbering_part = self
            .txn
            .part_related_by(&self.main(), rt::NUMBERING)?
            .ok_or_else(|| {
                Error::internal("numbering_part", "validated numbering part vanished")
            })?;
        let source_part = self
            .source
            .doc
            .package()
            .part_related_by(&self.source.main, rt::NUMBERING)?
            .ok_or_else(|| {
                Error::internal("numbering_part", "validated source numbering vanished")
            })?;
        let source_tree = self.source.tree(&source_part)?;
        let mut numbering_map: Vec<(i64, i64)> = Vec::new();
        for remap in &plan.remaps {
            if !referenced.contains(&remap.source_num_id) {
                continue;
            }
            if let Some(existing) = self.progress.numbering(remap.source_num_id) {
                numbering_map.push((remap.source_num_id, existing));
                continue;
            }
            let tree = self.txn.tree_mut(&numbering_part)?;
            let root = tree.root();
            let abstract_clone = tree.import_subtree(source_tree, remap.source_abstract, root)?;
            set_w(
                tree,
                abstract_clone,
                "abstractNumId",
                &remap.destination_abstract_id.to_string(),
            )?;
            match w_child(tree, root, "num") {
                Some(first_num) => add_previous(tree, first_num, abstract_clone)?,
                None => append(tree, root, abstract_clone)?,
            }
            let num_clone = tree.import_subtree(source_tree, remap.source_num, root)?;
            set_w(
                tree,
                num_clone,
                "numId",
                &remap.destination_num_id.to_string(),
            )?;
            if let Some(reference) = w_child(tree, num_clone, "abstractNumId") {
                set_w(
                    tree,
                    reference,
                    "val",
                    &remap.destination_abstract_id.to_string(),
                )?;
            }
            match w_child(tree, root, "numIdMacAtCleanup") {
                Some(cleanup) => add_previous(tree, cleanup, num_clone)?,
                None => append(tree, root, num_clone)?,
            }
            numbering_map.push((remap.source_num_id, remap.destination_num_id));
        }
        for (part, nodes) in sets {
            let tree = self.txn.tree_mut(part)?;
            for &node in nodes {
                for num_id in w_descendants(tree, node, "numId") {
                    let mapped = w_attr(tree, num_id, "val")
                        .filter(|v| !v.is_empty())
                        .and_then(py_int)
                        .and_then(|value| {
                            numbering_map
                                .iter()
                                .find(|(k, _)| *k == value)
                                .map(|(_, v)| *v)
                        });
                    if let Some(mapped) = mapped {
                        set_w(tree, num_id, "val", &mapped.to_string())?;
                    }
                }
            }
        }
        for (k, v) in numbering_map {
            match self
                .progress
                .numbering_map
                .iter_mut()
                .find(|(key, _)| *key == k)
            {
                Some(entry) => entry.1 = v,
                None => self.progress.numbering_map.push((k, v)),
            }
        }
        Ok(())
    }

    /// Upstream `_copy_media`: images become (deduplicated) destination
    /// image parts related from `part`.
    fn copy_media(&mut self, part: &str, source_part: &str, clones: &[NodeId]) -> Result<()> {
        let rels = self.source.rels(source_part)?;
        let references = image_references(self.txn.tree(part)?, clones);
        let mut rel_map: HashMap<String, String> = HashMap::new();
        for (node, attr, r_id) in references {
            if !rel_map.contains_key(&r_id) {
                let rel = rel_by_id(&rels, &r_id)
                    .filter(|rel| !rel.external && rel.rel_type == rt::IMAGE)
                    .ok_or_else(|| image_relationship_changed(&r_id))?;
                let blob = image_blob(&self.source, source_part, rel)
                    .ok_or_else(|| image_relationship_changed(&r_id))?;
                let image = Image::from_vec(blob)?;
                let image_part =
                    crate::authoring::get_or_add_image_part(&mut self.txn, &image, None)?.0;
                let new_r_id = self
                    .txn
                    .relate_to(part, rt::IMAGE, RelTarget::Part(&image_part))?;
                rel_map.insert(r_id.clone(), new_r_id);
                self.progress.media_copied.push(image_part);
            }
            let tree = self.txn.tree_mut(part)?;
            tree.set_attr(node, Some(R), attr, &rel_map[&r_id])?;
        }
        Ok(())
    }

    /// Upstream `_recreate_hyperlinks`.
    fn recreate_hyperlinks(
        &mut self,
        part: &str,
        source_part: &str,
        clones: &[NodeId],
    ) -> Result<()> {
        let rels = self.source.rels(source_part)?;
        let links: Vec<(NodeId, String)> = {
            let tree = self.txn.tree(part)?;
            clones
                .iter()
                .flat_map(|clone| w_descendants(tree, *clone, "hyperlink"))
                .filter_map(|link| {
                    let r_id = tree.attr(link, Some(R), "id").filter(|v| !v.is_empty())?;
                    Some((link, r_id.to_string()))
                })
                .collect()
        };
        for (hyperlink, r_id) in links {
            let rel = rel_by_id(&rels, &r_id)
                .filter(|rel| rel.external && rel.rel_type == rt::HYPERLINK)
                .ok_or_else(|| {
                    Error::unsupported(
                        "hyperlink_relationship_changed",
                        format!(
                            "hyperlink relationship {} changed after composition preflight. Nothing was changed",
                            py_repr(&r_id)
                        ),
                    )
                })?;
            let new_r_id =
                self.txn
                    .relate_to(part, rt::HYPERLINK, RelTarget::External(&rel.target))?;
            self.txn
                .tree_mut(part)?
                .set_attr(hyperlink, Some(R), "id", &new_r_id)?;
        }
        Ok(())
    }

    /// Reconcile the imported `clones` of `part` with the destination:
    /// styles, numbering, media, hyperlinks, bookmarks and control ids.
    fn carry(
        &mut self,
        part: &str,
        source_part: &str,
        clones: &[NodeId],
        wanted: &[String],
        plan: &NumberingPlan,
        mode: StylesMode,
    ) -> Result<()> {
        let (styles_part, imported) = self.reconcile_styles(part, clones, wanted, mode)?;
        self.remap_numbering(
            &[(part.to_string(), clones.to_vec()), (styles_part, imported)],
            plan,
        )?;
        self.copy_media(part, source_part, clones)?;
        self.recreate_hyperlinks(part, source_part, clones)?;
        self.reconcile_bookmarks(part, clones)?;
        self.reallocate_sdt_ids(part, clones)
    }

    /// Upstream `_reconcile_bookmarks`.
    fn reconcile_bookmarks(&mut self, part: &str, clones: &[NodeId]) -> Result<()> {
        let mut existing: HashSet<String> = HashSet::new();
        let mut max_id = 0i64;
        for (tree, root) in self.story_roots()? {
            for node in tree.descendants(root) {
                if is_w(tree, node, "bookmarkStart") {
                    if let Some(name) = w_attr(tree, node, "name").filter(|n| !n.is_empty()) {
                        existing.insert(casefold(name));
                    }
                    max_id = max_id.max(w_attr(tree, node, "id").and_then(py_int).unwrap_or(0));
                } else if is_w(tree, node, "bookmarkEnd") {
                    max_id = max_id.max(w_attr(tree, node, "id").and_then(py_int).unwrap_or(0));
                }
            }
        }
        let mut renames = OrderedMap::default();
        let mut id_map: HashMap<Option<String>, String> = HashMap::new();
        let mut next_id = max_id + 1;
        let mut findings = Vec::new();
        let tree = self.txn.tree_mut(part)?;
        for &clone in clones {
            for start in w_descendants(tree, clone, "bookmarkStart") {
                let name = w_attr(tree, start, "name").unwrap_or("").to_string();
                let old_id = w_attr(tree, start, "id").map(str::to_string);
                if name == "_GoBack" {
                    let end = w_descendants(tree, clone, "bookmarkEnd")
                        .into_iter()
                        .find(|end| w_attr(tree, *end, "id").map(str::to_string) == old_id);
                    if let Some(end) = end {
                        tree.detach(end)?;
                    }
                    tree.detach(start)?;
                    continue;
                }
                id_map.insert(old_id, next_id.to_string());
                set_w(tree, start, "id", &next_id.to_string())?;
                next_id += 1;
                if existing.contains(&casefold(&name)) {
                    let new_name = fresh_bookmark_name(&existing, &name);
                    renames.insert(&name, &new_name);
                    set_w(tree, start, "name", &new_name)?;
                    existing.insert(casefold(&new_name));
                } else {
                    existing.insert(casefold(&name));
                }
            }
            for end in w_descendants(tree, clone, "bookmarkEnd") {
                let old_id = w_attr(tree, end, "id").map(str::to_string);
                match id_map.get(&old_id) {
                    Some(new_id) => {
                        let new_id = new_id.clone();
                        set_w(tree, end, "id", &new_id)?;
                    }
                    None => {
                        tree.detach(end)?;
                        findings.push(CompositionFinding {
                            kind: "bookmark_partially_in_range".into(),
                            detail: "a bookmark end whose start lies outside the copied range was dropped".into(),
                        });
                    }
                }
            }
        }
        let ends_present: HashSet<Option<String>> = clones
            .iter()
            .flat_map(|clone| w_descendants(tree, *clone, "bookmarkEnd"))
            .map(|end| w_attr(tree, end, "id").map(str::to_string))
            .collect();
        for &clone in clones {
            for start in w_descendants(tree, clone, "bookmarkStart") {
                if !ends_present.contains(&w_attr(tree, start, "id").map(str::to_string)) {
                    let name = w_attr(tree, start, "name").unwrap_or("").to_string();
                    tree.detach(start)?;
                    renames.remove(&name);
                    findings.push(CompositionFinding {
                        kind: "bookmark_partially_in_range".into(),
                        detail: format!(
                            "bookmark {} starts in the copied range but ends outside it; the start marker was dropped",
                            py_repr(&name)
                        ),
                    });
                }
            }
        }
        self.progress.findings.extend(findings);
        self.progress.bookmarks_renamed.update(&renames);
        if !self.progress.bookmarks_renamed.is_empty() {
            let renames = self.progress.bookmarks_renamed.clone();
            let tree = self.txn.tree_mut(part)?;
            remap_field_refs(tree, clones, &renames)?;
        }
        Ok(())
    }

    /// Upstream `_reallocate_sdt_ids`.
    fn reallocate_sdt_ids(&mut self, part: &str, clones: &[NodeId]) -> Result<()> {
        let mut existing: HashSet<i64> = HashSet::new();
        for (tree, root) in self.story_roots()? {
            for sdt_pr in w_descendants(tree, root, "sdtPr") {
                if let Some(value) = w_child(tree, sdt_pr, "id")
                    .and_then(|id| w_attr(tree, id, "val"))
                    .filter(|v| !v.is_empty())
                    .and_then(py_int)
                {
                    existing.insert(value);
                }
            }
        }
        let mut next_id = existing.iter().copied().max().unwrap_or(0) + 1;
        let tree = self.txn.tree_mut(part)?;
        for &clone in clones {
            for sdt_pr in w_descendants(tree, clone, "sdtPr") {
                let Some(id_element) = w_child(tree, sdt_pr, "id") else {
                    continue;
                };
                let Some(value) = w_attr(tree, id_element, "val")
                    .filter(|v| !v.is_empty())
                    .and_then(py_int)
                else {
                    continue;
                };
                if existing.contains(&value) {
                    set_w(tree, id_element, "val", &next_id.to_string())?;
                    existing.insert(next_id);
                    next_id += 1;
                } else {
                    existing.insert(value);
                    next_id = next_id.max(value + 1);
                }
            }
        }
        Ok(())
    }

    /// Upstream `_compose` (body range after `anchor` in the main story).
    fn compose(
        &mut self,
        range: &[NodeId],
        anchor_element: NodeId,
        anchor_story: &str,
        mode: StylesMode,
    ) -> Result<()> {
        let source_main = self.source.main.clone();
        let source_tree = self.source.tree(&source_main)?;
        let has_fields = contains_fields(source_tree, range);
        refuse_unsupported_content(source_tree, range)?;
        if anchor_story != MAIN_STORY {
            return Err(Error::unsupported(
                "destination_not_main_body",
                format!(
                    "composition inserts into the main document body only (anchor is in {anchor_story})"
                ),
            ));
        }
        let main = self.main();
        {
            let tree = self.txn.tree(&main)?;
            if is_w(tree, anchor_element, "p")
                && tree
                    .parent(anchor_element)
                    .is_some_and(|parent| is_w(tree, parent, "tc"))
            {
                return Err(Error::unsupported(
                    "cell_anchor",
                    "anchor resolves inside a table cell; block insertion targets body-level paragraphs — anchor \
                     on a paragraph outside the table",
                ));
            }
            let (_, open_after) = field_open_flags(tree, anchor_story, anchor_element)?;
            if open_after {
                return Err(Error::unsupported(
                    "inside_open_field",
                    "the insertion boundary after the destination block remains inside an open field result; Word \
                     may regenerate that result and remove composed content. Use a current-view destination after \
                     the matching field end, or deliberately close or unlink the field before retrying",
                ));
            }
        }
        self.refuse_malformed_numeric_ids(source_tree, range)?;
        preflight_bookmark_references(&self.source, source_tree, range)?;
        preflight_relationships(&self.source, &source_main, source_tree, range)?;
        let (wanted, chained) = self.chained_source_definitions(source_tree, range)?;
        let plan = {
            let source_styles = self.source.styles()?;
            let mut elements: Vec<(&XmlTree, NodeId)> =
                range.iter().map(|e| (source_tree, *e)).collect();
            elements.extend(chained.iter().map(|e| (source_styles, *e)));
            self.preflight_numbering(&elements)?
        };
        refuse_unloadable_media(&self.source, &source_main, source_tree, range)?;

        let scope = self
            .txn
            .tree(&main)?
            .parent(anchor_element)
            .ok_or_else(|| Error::internal("anchor_detached", "destination anchor is detached"))?;
        let mut clones = self.import(&main, scope, source_tree, range)?;
        self.carry(&main, &source_main, &clones, &wanted, &plan, mode)?;
        {
            let tree = self.txn.tree_mut(&main)?;
            pad_adjacent_tables(tree, anchor_element, &mut clones)?;
            crate::blocks::insert_after(tree, anchor_element, &clones)?;
        }
        if has_fields {
            crate::fields::set_update_fields_on_open(&mut self.txn)?;
        }
        self.progress.inserted_blocks = clones.len();
        let mut declared: Vec<String> = [
            "word/document.xml",
            "word/styles.xml",
            "word/numbering.xml",
            "word/_rels/document.xml.rels",
            "[Content_Types].xml",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        declared.extend(self.progress.media_copied.iter().cloned());
        self.progress.declared_parts = declared;
        Ok(())
    }

    /// The destination definition for a header/footer slot of the last
    /// section, created from the template when the slot is linked (python-docx
    /// `is_linked_to_previous = False`).
    fn ensure_definition(
        &mut self,
        sect_pr: NodeId,
        role: HeaderFooterRole,
        kind: HeaderFooterType,
    ) -> Result<String> {
        let main = self.main();
        let existing = reference_of(self.txn.tree(&main)?, sect_pr, role, kind);
        if let Some(r_id) = existing {
            return self.txn.part_related_by_id(&main, &r_id)?.ok_or_else(|| {
                Error::unsupported(
                    "missing_header_footer_part",
                    format!(
                        "the destination {} reference {r_id} has no part",
                        role.reference()
                    ),
                )
            });
        }
        let name = add_definition(&mut self.txn, sect_pr, role, kind)?;
        self.created.push(name.clone());
        Ok(name)
    }

    /// Upstream `_copy_letterhead`.
    fn copy_letterhead(&mut self, dest_section_count: usize, mode: StylesMode) -> Result<()> {
        let main = self.main();
        let dest_sect = *section_properties(self.txn.tree(&main)?)?
            .last()
            .ok_or_else(|| {
                Error::unsupported(
                    "no_section_properties",
                    "the destination has no section properties",
                )
            })?;
        let source_main = self.source.main.clone();
        let source_tree = self.source.tree(&source_main)?;
        let source_sections = section_properties(source_tree)?;
        let src_sect = *source_sections.last().ok_or_else(|| {
            Error::unsupported(
                "no_section_properties",
                "the source has no section properties",
            )
        })?;
        let source_even = match self
            .source
            .doc
            .package()
            .part_related_by(&source_main, rt::SETTINGS)?
        {
            Some(part) => even_and_odd(self.source.tree(&part)?)?,
            None => false,
        };
        let settings = crate::parts::ensure_settings_part(&mut self.txn)?;
        let dest_even = even_and_odd(self.txn.tree(&settings)?)?;
        if source_even != dest_even && dest_section_count > 1 {
            return Err(Error::unsupported(
                "even_odd_document_wide",
                "even/odd headers are a document-wide setting; destination has more than one section. Nothing was \
                 changed",
            ));
        }
        set_even_and_odd(self.txn.tree_mut(&settings)?, source_even)?;
        if title_pg(source_tree, src_sect)? {
            self.progress.findings.push(CompositionFinding {
                kind: "letterhead_first_page_skipped".into(),
                detail: "first-page headers apply to the destination section's first page, not the appended \
                         content; the source first-page letterhead was not copied"
                    .into(),
            });
        }
        {
            let tree = self.txn.tree_mut(&main)?;
            for title in w_direct(tree, dest_sect, "titlePg") {
                tree.detach(title)?;
            }
        }
        let mut letterhead_has_fields = false;
        for (role, kind) in SLOTS {
            let Some(source_part) =
                self.defined_header_footer(source_tree, &source_sections, role, kind)?
            else {
                continue;
            };
            let dest_part = self.ensure_definition(dest_sect, role, kind)?;
            let src_tree = self.source.tree(&source_part)?;
            let children: Vec<NodeId> = node_children(src_tree, src_tree.root()).collect();
            refuse_unsupported_content(src_tree, &children)?;
            self.refuse_malformed_numeric_ids(src_tree, &children)?;
            preflight_relationships(&self.source, &source_part, src_tree, &children)?;
            refuse_unloadable_media(&self.source, &source_part, src_tree, &children)?;
            let (wanted, chained) = self.chained_source_definitions(src_tree, &children)?;
            let plan = {
                let source_styles = self.source.styles()?;
                let mut elements: Vec<(&XmlTree, NodeId)> =
                    children.iter().map(|e| (src_tree, *e)).collect();
                elements.extend(chained.iter().map(|e| (source_styles, *e)));
                self.preflight_numbering(&elements)?
            };
            let dest_root = {
                let tree = self.txn.tree_mut(&dest_part)?;
                let root = tree.root();
                clear_children(tree, root)?;
                root
            };
            let clones = self.import(&dest_part, dest_root, src_tree, &children)?;
            if contains_fields(self.txn.tree(&dest_part)?, &clones) {
                letterhead_has_fields = true;
            }
            self.carry(&dest_part, &source_part, &clones, &wanted, &plan, mode)?;
            let tree = self.txn.tree_mut(&dest_part)?;
            for clone in clones {
                append(tree, dest_root, clone)?;
            }
        }
        if letterhead_has_fields {
            crate::fields::set_update_fields_on_open(&mut self.txn)?;
        }
        Ok(())
    }

    /// Upstream `_defined_header_footer`: the definition Word shows for the
    /// last source section's slot (walking linked sections backwards).
    fn defined_header_footer(
        &self,
        tree: &XmlTree,
        sections: &[NodeId],
        role: HeaderFooterRole,
        kind: HeaderFooterType,
    ) -> Result<Option<String>> {
        for &sect_pr in sections.iter().rev() {
            let Some(r_id) = reference_of(tree, sect_pr, role, kind) else {
                continue;
            };
            let rels = self.source.rels(&self.source.main)?;
            let member = rel_by_id(&rels, &r_id)
                .filter(|rel| !rel.external)
                .and_then(|rel| self.source.target_member(&self.source.main, rel))
                .ok_or_else(|| {
                    Error::unsupported(
                        "missing_header_footer_part",
                        format!(
                            "the source {} reference {r_id} has no part",
                            role.reference()
                        ),
                    )
                })?;
            return Ok(Some(member));
        }
        Ok(None)
    }
}

/// The letterhead slots `_copy_letterhead` carries, in upstream order.
const SLOTS: [(HeaderFooterRole, HeaderFooterType); 4] = [
    (HeaderFooterRole::Header, HeaderFooterType::Primary),
    (HeaderFooterRole::Footer, HeaderFooterType::Primary),
    (HeaderFooterRole::Header, HeaderFooterType::EvenPage),
    (HeaderFooterRole::Footer, HeaderFooterType::EvenPage),
];

/// `rId` of the first `w:headerReference`/`w:footerReference` of the slot's
/// type directly under `sect_pr`.
fn reference_of(
    tree: &XmlTree,
    sect_pr: NodeId,
    role: HeaderFooterRole,
    kind: HeaderFooterType,
) -> Option<String> {
    crate::sections::reference(tree, sect_pr, role, kind)
        .and_then(|reference| tree.attr(reference, Some(R), "id").map(str::to_string))
}

/// `ST_OnOff` with python-docx's `CT_OnOff` default (`True` when absent).
fn on_off_val(tree: &XmlTree, element: NodeId) -> Result<bool> {
    match w_attr(tree, element, "val") {
        None => Ok(true),
        Some("1" | "true" | "on") => Ok(true),
        Some("0" | "false" | "off") => Ok(false),
        Some(other) => Err(Error::unsupported(
            "invalid_on_off",
            format!("invalid ST_OnOff value {}", py_repr(other)),
        )),
    }
}

/// `Settings.odd_and_even_pages_header_footer`.
fn even_and_odd(tree: &XmlTree) -> Result<bool> {
    match w_child(tree, tree.root(), "evenAndOddHeaders") {
        Some(element) => on_off_val(tree, element),
        None => Ok(false),
    }
}

/// `Settings.odd_and_even_pages_header_footer = value`.
fn set_even_and_odd(tree: &mut XmlTree, value: bool) -> Result<()> {
    let root = tree.root();
    if !value {
        for element in w_direct(tree, root, "evenAndOddHeaders") {
            tree.detach(element)?;
        }
        return Ok(());
    }
    let element = match w_child(tree, root, "evenAndOddHeaders") {
        Some(existing) => existing,
        None => {
            let element = w_el(tree, root, "evenAndOddHeaders")?;
            insert_settings_child(tree, root, element, "evenAndOddHeaders")?;
            element
        }
    };
    // `val = True` equals the attribute default, which python-docx removes.
    tree.remove_attr(element, Some(W), "val");
    Ok(())
}

/// `CT_SectPr.titlePg_val`.
fn title_pg(tree: &XmlTree, sect_pr: NodeId) -> Result<bool> {
    match w_child(tree, sect_pr, "titlePg") {
        Some(element) => on_off_val(tree, element),
        None => Ok(false),
    }
}

/// `for child in list(root): root.remove(child)` with lxml semantics: every
/// child node and its tail go; the leading text stays.
fn clear_children(tree: &mut XmlTree, root: NodeId) -> Result<()> {
    let children: Vec<NodeId> = tree.children(root).to_vec();
    let leading = children
        .iter()
        .take_while(|child| matches!(tree.data(**child), NodeData::Text(_)))
        .count();
    for child in children.into_iter().skip(leading) {
        tree.detach(child)?;
    }
    Ok(())
}

/// Upstream `_pad_adjacent_tables`.
fn pad_adjacent_tables(tree: &mut XmlTree, anchor: NodeId, clones: &mut Vec<NodeId>) -> Result<()> {
    let scope = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("anchor_detached", "destination anchor is detached"))?;
    if clones
        .first()
        .is_some_and(|first| is_w(tree, *first, "tbl"))
        && is_w(tree, anchor, "tbl")
    {
        let paragraph = w_el(tree, scope, "p")?;
        clones.insert(0, paragraph);
    }
    if clones.last().is_some_and(|last| is_w(tree, *last, "tbl")) {
        let siblings = tree.children(scope);
        let index = tree
            .index_in_parent(anchor)
            .ok_or_else(|| Error::internal("anchor_detached", "destination anchor is detached"))?;
        let following = siblings[index + 1..]
            .iter()
            .copied()
            .find(|node| !matches!(tree.data(*node), NodeData::Text(_)));
        if following.is_some_and(|node| is_w(tree, node, "tbl")) {
            let paragraph = w_el(tree, scope, "p")?;
            clones.push(paragraph);
        }
    }
    Ok(())
}

/// Upstream `_remap_styleref_fields`.
fn remap_styleref_fields(
    tree: &mut XmlTree,
    clones: &[NodeId],
    renames: &OrderedMap,
) -> Result<()> {
    let pairs = renames.pairs();
    for &clone in clones {
        for (instruction, nodes) in iter_field_instructions(tree, clone) {
            let rewritten = fieldcode::rewrite_command_operand(&instruction, "STYLEREF", &pairs)?;
            if rewritten != instruction {
                redistribute(tree, &nodes, &rewritten)?;
            }
        }
        for (field, instruction) in simple_field_instructions(tree, clone) {
            let rewritten = fieldcode::rewrite_command_operand(&instruction, "STYLEREF", &pairs)?;
            set_w(tree, field, "instr", &rewritten)?;
        }
    }
    Ok(())
}

/// Upstream `_remap_field_refs`: REF-style instructions and hyperlink
/// anchors inside the copied range that point at renamed bookmarks.
fn remap_field_refs(tree: &mut XmlTree, clones: &[NodeId], renames: &OrderedMap) -> Result<()> {
    let pairs = renames.pairs();
    for &clone in clones {
        for (instruction, nodes) in iter_field_instructions(tree, clone) {
            let rewritten = fieldcode::rewrite_bookmark_operands(&instruction, &pairs)?;
            if rewritten != instruction {
                redistribute(tree, &nodes, &rewritten)?;
            }
        }
        for (field, instruction) in simple_field_instructions(tree, clone) {
            let rewritten = fieldcode::rewrite_bookmark_operands(&instruction, &pairs)?;
            if rewritten != instruction {
                set_w(tree, field, "instr", &rewritten)?;
            }
        }
        for link in w_descendants(tree, clone, "hyperlink") {
            let Some(anchor) = w_attr(tree, link, "anchor").filter(|a| !a.is_empty()) else {
                continue;
            };
            let folded = casefold(anchor);
            let replacement = pairs
                .iter()
                .rev()
                .find(|(old, _)| casefold(old) == folded)
                .map(|(_, new)| new.to_string());
            if let Some(replacement) = replacement {
                set_w(tree, link, "anchor", &replacement)?;
            }
        }
    }
    Ok(())
}

/// Upstream `_body_block_for_paragraph`: the top-level body block (a
/// paragraph or content control) containing `paragraph`.
fn body_block_for_paragraph(tree: &XmlTree, body: NodeId, paragraph: NodeId) -> Option<NodeId> {
    let mut current = paragraph;
    while let Some(parent) = tree.parent(current) {
        if parent == body {
            break;
        }
        current = parent;
    }
    if tree.parent(current) != Some(body) {
        return None;
    }
    (is_w(tree, current, "p") || is_w(tree, current, "sdt")).then_some(current)
}

/// Upstream `_source_range`: the physical slice of source body children
/// between the adjusted endpoints.
fn source_range(
    source: &Document,
    start: &Located,
    end: Option<&Located>,
    count: i64,
    include_start: bool,
    include_end: bool,
) -> Result<Vec<NodeId>> {
    if start.story != MAIN_STORY {
        return Err(Error::unsupported(
            "source_not_main_body",
            format!(
                "composition copies from the main document body only (start anchor is in {})",
                start.story
            ),
        ));
    }
    let tree = source.package().xml(&start.part)?;
    let body = body_of(tree)?;
    let children = body_children(tree, body);
    let blocks: Vec<NodeId> = children
        .iter()
        .copied()
        .filter(|child| is_w_any(tree, *child, BODY_BLOCK_TAGS))
        .collect();
    let start_block = body_block_for_paragraph(tree, body, start.element);
    let Some(mut start_index) =
        start_block.and_then(|block| blocks.iter().position(|b| *b == block))
    else {
        return Err(Error::unsupported(
            "source_anchor_not_top_level",
            "the start anchor is not a top-level body block (text boxes and table cells cannot anchor a composition \
             range)",
        ));
    };
    let end_index = match end {
        Some(end) => {
            let end_block = body_block_for_paragraph(tree, body, end.element);
            let position = end_block.and_then(|block| blocks.iter().position(|b| *b == block));
            let Some(mut end_index) = position.filter(|_| end.story == start.story) else {
                return Err(Error::unsupported(
                    "end_anchor_not_top_level",
                    "the end anchor must be a top-level body block of the same source document",
                ));
            };
            if end_index < start_index {
                return Err(Error::not_found(
                    "end_before_start",
                    "end anchor precedes start anchor in the source body",
                ));
            }
            if !include_start {
                start_index += 1;
            }
            if !include_end {
                if end_index == 0 {
                    return Err(Error::not_found(
                        "empty_range",
                        "the adjusted source range is empty",
                    ));
                }
                end_index -= 1;
            }
            if start_index > end_index {
                return Err(Error::not_found(
                    "empty_range",
                    "the adjusted source range is empty",
                ));
            }
            end_index
        }
        None => {
            if !include_start {
                start_index += 1;
            }
            let end_index = start_index as i64 + count - 1;
            if end_index >= blocks.len() as i64 {
                return Err(Error::not_found(
                    "range_past_end",
                    format!(
                        "the source body has only {} blocks from the start anchor; {count} requested",
                        blocks.len() as i64 - start_index as i64
                    ),
                ));
            }
            end_index as usize
        }
    };
    let first = children
        .iter()
        .position(|c| *c == blocks[start_index])
        .ok_or_else(|| Error::internal("source_range", "block not among body children"))?;
    let last = children
        .iter()
        .position(|c| *c == blocks[end_index])
        .ok_or_else(|| Error::internal("source_range", "block not among body children"))?;
    Ok(children[first..=last].to_vec())
}

/// Copy a block range from `source` after `anchor` (upstream
/// `insert_blocks_from`).
///
/// `start_anchor` and `options.end_anchor` identify source paragraphs whose
/// top-level body blocks (a direct body paragraph or a top-level content
/// control) are the range endpoints; any inspection view is accepted. The
/// destination `anchor` must be a current-view paragraph target in the main
/// body. Refuses a protected destination, an empty adjusted range, and
/// content composition cannot carry (revisions, comments, notes, OLE
/// objects, custom XML, unloadable images, unremappable relationships,
/// references to bookmarks outside the range, undefined styles or
/// numbering).
pub fn insert_blocks_from(
    document: &mut Document,
    source: &Document,
    start_anchor: BlockTarget<'_>,
    anchor: BlockTarget<'_>,
    options: &InsertBlocksOptions<'_>,
) -> Result<CompositionReport> {
    if options.end_anchor.is_none() && !options.include_end {
        return Err(Error::invalid_argument(
            "include_end_requires_end_anchor",
            "include_end=False requires end_anchor",
        ));
    }
    if options.count < 1 {
        return Err(Error::invalid_argument(
            "count_below_one",
            "count must be >= 1",
        ));
    }
    let start = anchor::locate_paragraph(source, start_anchor)?;
    let end = match options.end_anchor {
        Some(end) => Some(anchor::locate_paragraph(source, end)?),
        None => None,
    };
    let destination = anchor::locate_paragraph(document, anchor)?;
    anchor::require_current_view(anchor)?;
    refuse_if_protected(
        document,
        "compose content into the document",
        OperationClass::BodyContent,
    )?;
    let range = source_range(
        source,
        &start,
        end.as_ref(),
        options.count,
        options.include_start,
        options.include_end,
    )?;
    let (staged, progress) = {
        let mut composer = Composer {
            txn: Txn::new(document),
            source: Source::new(source)?,
            progress: Progress::default(),
            created: Vec::new(),
        };
        composer.compose(
            &range,
            destination.element,
            &destination.story,
            options.styles,
        )?;
        (composer.txn.finish()?, composer.progress)
    };
    document.commit(staged)?;
    Ok(progress.into_report())
}

/// Append `source`'s whole body to `document` (upstream `append_document`).
///
/// `SectionStart::NewPage` prefixes a page-break paragraph; no new
/// `w:sectPr` is authored. `HeadersFrom::Source` overwrites the
/// destination's last-section default and even-page headers and footers
/// with the source's and sets the document-wide even/odd setting (refused
/// when it would change and the destination has several sections).
pub fn append_document(
    document: &mut Document,
    source: &Document,
    options: &AppendOptions,
) -> Result<CompositionReport> {
    refuse_if_protected(document, "append a document", OperationClass::BodyContent)?;
    let source_main = source.package().main_part_name().to_string();
    let source_tree = source.package().xml(&source_main)?;
    let range = body_children(source_tree, body_of(source_tree)?);
    if range.is_empty() {
        return Err(Error::not_found(
            "source_body_empty",
            "the source document body has no blocks",
        ));
    }
    let main = document.package().main_part_name().to_string();
    let (anchor_element, section_count) = {
        let tree = document.package().xml(&main)?;
        let body = body_of(tree)?;
        let blocks: Vec<NodeId> = node_children(tree, body)
            .filter(|child| is_w_any(tree, *child, BODY_BLOCK_TAGS))
            .collect();
        let Some(&last) = blocks.last() else {
            return Err(Error::not_found(
                "destination_body_empty",
                "the destination document body has no blocks",
            ));
        };
        (last, section_properties(tree)?.len())
    };
    let (staged, progress) = {
        let mut composer = Composer {
            txn: Txn::new(document),
            source: Source::new(source)?,
            progress: Progress::default(),
            created: Vec::new(),
        };
        composer.compose(&range, anchor_element, MAIN_STORY, options.styles)?;
        if options.section == SectionStart::NewPage {
            let tree = composer.txn.tree_mut(&main)?;
            let scope = tree.parent(anchor_element).ok_or_else(|| {
                Error::internal("anchor_detached", "destination anchor is detached")
            })?;
            let paragraph = w_el(tree, scope, "p")?;
            let run = w_el(tree, paragraph, "r")?;
            let page_break = w_el(tree, run, "br")?;
            set_w(tree, page_break, "type", "page")?;
            append(tree, run, page_break)?;
            append(tree, paragraph, run)?;
            add_next(tree, anchor_element, paragraph)?;
        }
        if options.headers == HeadersFrom::Source {
            composer.copy_letterhead(section_count, options.styles)?;
        }
        (composer.txn.finish()?, composer.progress)
    };
    document.commit(staged)?;
    Ok(progress.into_report())
}

/// Upstream's private `_copy_letterhead` alone, with a fresh report (its
/// tests call it directly to reach letterheads inherited from earlier source
/// sections, which `append_document` refuses through the body preflight).
/// A test hook, not part of the stable API.
#[doc(hidden)]
pub fn copy_letterhead(
    document: &mut Document,
    source: &Document,
    styles: StylesMode,
) -> Result<CompositionReport> {
    let main = document.package().main_part_name().to_string();
    let section_count = section_properties(document.package().xml(&main)?)?.len();
    let (staged, progress) = {
        let mut composer = Composer {
            txn: Txn::new(document),
            source: Source::new(source)?,
            progress: Progress::default(),
            created: Vec::new(),
        };
        composer.copy_letterhead(section_count, styles)?;
        (composer.txn.finish()?, composer.progress)
    };
    document.commit(staged)?;
    Ok(progress.into_report())
}

impl Document {
    /// Copy a block range from `source` after `anchor` (see
    /// [`insert_blocks_from`]).
    pub fn insert_blocks_from(
        &mut self,
        source: &Document,
        start_anchor: BlockTarget<'_>,
        anchor: BlockTarget<'_>,
        options: &InsertBlocksOptions<'_>,
    ) -> Result<CompositionReport> {
        insert_blocks_from(self, source, start_anchor, anchor, options)
    }

    /// Append `source`'s body (see [`append_document`]).
    pub fn append_document(
        &mut self,
        source: &Document,
        options: &AppendOptions,
    ) -> Result<CompositionReport> {
        append_document(self, source, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_bookmark_names_follow_word_grammar() {
        let existing: HashSet<String> = ["term".to_string()].into_iter().collect();
        assert_eq!(fresh_bookmark_name(&existing, "Term"), "Term_imported");
        let name = "A".repeat(40);
        let fresh = fresh_bookmark_name(&existing, &name);
        assert_eq!(fresh.chars().count(), 40);
        assert!(fresh.ends_with("_imported"));
        assert_eq!(fresh_bookmark_name(&existing, "1 x"), "B_1_x_imported");
        let taken: HashSet<String> = ["b_imported".to_string()].into_iter().collect();
        assert_eq!(fresh_bookmark_name(&taken, "B"), "B_imported2");
    }

    #[test]
    fn python_repr_quotes_like_python() {
        assert_eq!(py_repr("Term"), "'Term'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a\nb"), "'a\\nb'");
    }

    #[test]
    fn report_shape_sorts_and_dedups() {
        let report = CompositionReport {
            imported_styles: vec!["b".into(), "a".into()],
            numbering_map: [(10, 12), (2, 11)].into_iter().collect(),
            declared_parts: vec!["x".into(), "x".into(), "a".into()],
            ..CompositionReport::default()
        };
        let json = report.to_json();
        assert_eq!(json["schema"], "paper_composition");
        assert_eq!(json["version"], 1);
        assert_eq!(json["imported_styles"], serde_json::json!(["a", "b"]));
        assert_eq!(json["numbering_map"]["10"], 12);
        assert_eq!(json["declared_parts"], serde_json::json!(["a", "x"]));
    }
}
