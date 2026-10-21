//! Revision enumeration and resolution (upstream `revision.py`, MIT; see
//! `NOTICE.md`).
//!
//! [`Document::revisions`] enumerates every tracked change across every
//! story part: insertions, deletions, moves, run/paragraph/paragraph-mark
//! format changes, table-row markers and the exotic remainder (table/cell
//! property changes, section and numbering changes, custom-XML ranges),
//! which are counted but never resolved.
//!
//! Resolution follows upstream exactly: accepting applies a change
//! (insertions unwrap, deletions disappear), rejecting undoes it (insertions
//! disappear, deleted text returns as live `w:t`); moves resolve as paired
//! units; paragraph-mark revisions join or remove paragraphs; row markers keep
//! or remove rows (a table's last row takes the table with it). Every
//! dependency a resolution can touch is validated first, and all writes are
//! staged in one [`Txn`], so a refusal leaves the document and every handle
//! unchanged. Discarded content takes its comments (anchors, bodies,
//! `commentsExtended`, `commentsIds`, `commentsExtensible` rows), note bodies
//! and now-unreferenced relationships with it.
//!
//! A [`Revisions`] snapshot and its [`Revision`] items carry freshness
//! evidence (document identity plus the identity signature of every
//! enumerable revision); any change to the revision set makes them stale.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::normalize::content_hash;
use crate::package::ct;
use crate::package::rels::{RELS_NS, rels_member_for, rt};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::story::{self, BlockKind, Story, View};
use crate::txn::Txn;
use crate::unicode::py_strip;
use crate::wml::{MC, W, first_choice_children, leading_text, node_children, w_attr};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, DocumentId, Error, RefusalKind, Result};

pub(crate) const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const O_NS: &str = "urn:schemas-microsoft-com:office:office";
pub(crate) const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

/// Revision types `accept`/`reject` can resolve.
pub const RESOLVABLE_TYPES: &[&str] = &[
    "insertion",
    "deletion",
    "format_change",
    "row_insertion",
    "row_deletion",
    "move_from",
    "move_to",
];

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Upstream `_REVISION_TYPES` (tag → type), by `w:` local name.
fn base_revision_type(local: &str) -> Option<&'static str> {
    Some(match local {
        "ins" => "insertion",
        "del" => "deletion",
        "moveFrom" => "move_from",
        "moveTo" => "move_to",
        "rPrChange" | "pPrChange" => "format_change",
        "tblPrChange" | "tblPrExChange" | "tblGridChange" | "trPrChange" | "tcPrChange" => {
            "table_property_change"
        }
        "cellIns" | "cellDel" | "cellMerge" => "cell_revision",
        "sectPrChange" => "section_property_change",
        "numberingChange" => "numbering_change",
        "customXmlInsRangeStart"
        | "customXmlDelRangeStart"
        | "customXmlMoveFromRangeStart"
        | "customXmlMoveToRangeStart" => "custom_xml_revision",
        _ => return None,
    })
}

/// Upstream `_MARKUP_SCAN_TAGS` beyond the enumerated types.
const EXTRA_SCAN_TAGS: &[&str] = &[
    "moveFromRangeStart",
    "moveFromRangeEnd",
    "moveToRangeStart",
    "moveToRangeEnd",
    "customXmlInsRangeEnd",
    "customXmlDelRangeEnd",
    "customXmlMoveFromRangeEnd",
    "customXmlMoveToRangeEnd",
];

/// `w:` local name of an element (None for other namespaces / non-elements).
pub(crate) fn w_local(tree: &XmlTree, id: NodeId) -> Option<&str> {
    if tree.ns_uri(id) == Some(W) {
        tree.local_name(id)
    } else {
        None
    }
}

fn is_tag(tree: &XmlTree, id: NodeId, local: &str) -> bool {
    w_local(tree, id) == Some(local)
}

fn is_revision_tag(tree: &XmlTree, id: NodeId) -> bool {
    w_local(tree, id).and_then(base_revision_type).is_some()
}

fn is_scan_tag(tree: &XmlTree, id: NodeId) -> bool {
    w_local(tree, id).is_some_and(|local| {
        base_revision_type(local).is_some() || EXTRA_SCAN_TAGS.contains(&local)
    })
}

/// Upstream `_revision_type_of`.
fn revision_type_of(tree: &XmlTree, node: NodeId) -> &'static str {
    if tree
        .parent(node)
        .is_some_and(|parent| is_tag(tree, parent, "trPr"))
    {
        return if is_tag(tree, node, "ins") {
            "row_insertion"
        } else {
            "row_deletion"
        };
    }
    w_local(tree, node)
        .and_then(base_revision_type)
        .unwrap_or("unknown")
}

/// lxml `element.iter(*tags)`: descendant-or-self `w:` elements named in `tags`.
pub(crate) fn iter_w<'t>(
    tree: &'t XmlTree,
    root: NodeId,
    tags: &'t [&'t str],
) -> impl Iterator<Item = NodeId> + 't {
    tree.descendants(root)
        .filter(move |node| w_local(tree, *node).is_some_and(|local| tags.contains(&local)))
}

/// lxml `len(element)`: element, comment and PI children.
fn lxml_len(tree: &XmlTree, id: NodeId) -> usize {
    node_children(tree, id).count()
}

/// Raw child index of the `k`-th lxml child of `parent` (or the end).
fn raw_index(tree: &XmlTree, parent: NodeId, k: usize) -> usize {
    let children = tree.children(parent);
    let mut seen = 0;
    for (index, child) in children.iter().enumerate() {
        if !matches!(tree.data(*child), NodeData::Text(_)) {
            if seen == k {
                return index;
            }
            seen += 1;
        }
    }
    children.len()
}

/// lxml `getnext()`: the next element/comment/PI sibling.
fn lxml_next(tree: &XmlTree, id: NodeId) -> Option<NodeId> {
    let parent = tree.parent(id)?;
    let children = tree.children(parent);
    let index = children.iter().position(|child| *child == id)?;
    children[index + 1..]
        .iter()
        .copied()
        .find(|child| !matches!(tree.data(*child), NodeData::Text(_)))
}

/// Python `int(str)` for decimal ids.
pub(crate) fn py_int(value: &str) -> Option<i64> {
    let trimmed = py_strip(value);
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    if !cleaned.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    cleaned.parse::<i64>().ok().map(|n| n * sign)
}

/// Python `repr` of a string (simple quoting rules).
pub(crate) fn py_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr` of a list of strings.
pub(crate) fn py_list(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|value| py_repr(value)).collect();
    format!("[{}]", items.join(", "))
}

pub(crate) fn py_opt_repr(value: Option<&str>) -> String {
    value.map(py_repr).unwrap_or_else(|| "None".to_string())
}

// ---------------------------------------------------------------------------
// Python datetime.fromisoformat / isoformat
// ---------------------------------------------------------------------------

/// A parsed revision date, rendered like Python `datetime.isoformat()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RevisionDate {
    /// Year.
    pub year: i32,
    /// Month (1-12).
    pub month: u32,
    /// Day.
    pub day: u32,
    /// Hour.
    pub hour: u32,
    /// Minute.
    pub minute: u32,
    /// Second.
    pub second: u32,
    /// Microsecond.
    pub microsecond: u32,
    /// UTC offset in microseconds (`None` for a naive value).
    pub offset_us: Option<i64>,
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `_parse_hh_mm_ss_ff`: `HH[:?MM[:?SS[{.,}f+]]]`.
#[allow(clippy::needless_range_loop)]
fn parse_hh_mm_ss_ff(text: &str) -> Option<[u32; 4]> {
    let bytes: Vec<char> = text.chars().collect();
    let len = bytes.len();
    let mut comps = [0u32; 4];
    let mut pos = 0;
    let mut has_sep = false;
    for comp in 0..3 {
        if len - pos < 2 {
            return None;
        }
        let piece: String = bytes[pos..pos + 2].iter().collect();
        comps[comp] = digits(&piece)?;
        pos += 2;
        let next = bytes.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if has_sep && next != Some(':') {
            return None;
        }
        if has_sep {
            pos += 1;
        }
    }
    if pos < len {
        if bytes[pos] != '.' && bytes[pos] != ',' {
            return None;
        }
        pos += 1;
        let remainder = len - pos;
        if remainder == 0 {
            return None;
        }
        let take = remainder.min(6);
        let piece: String = bytes[pos..pos + take].iter().collect();
        let mut value = digits(&piece)?;
        for _ in take..6 {
            value *= 10;
        }
        comps[3] = value;
        if !bytes[pos + take..].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    Some(comps)
}

impl RevisionDate {
    /// Python `datetime.fromisoformat` (3.11+ grammar: extended or basic
    /// calendar dates, any single-character separator, `HH[:MM[:SS[.f]]]`
    /// and `±HH[:MM[:SS[.f]]]` / `Z` offsets). Week dates are not
    /// recognised (they yield `None`).
    pub fn from_isoformat(text: &str) -> Option<RevisionDate> {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() < 7 {
            return None;
        }
        let (date_len, extended) = if chars.len() >= 10 && chars[4] == '-' {
            (10, true)
        } else {
            (8, false)
        };
        if chars.len() < date_len {
            return None;
        }
        let date: String = chars[..date_len].iter().collect();
        let (year, month, day) = if extended {
            if date.as_bytes()[7] != b'-' {
                return None;
            }
            (
                digits(&date[0..4])?,
                digits(&date[5..7])?,
                digits(&date[8..10])?,
            )
        } else {
            (
                digits(&date[0..4])?,
                digits(&date[4..6])?,
                digits(&date[6..8])?,
            )
        };
        let year = year as i32;
        if year < 1 || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return None;
        }
        let mut out = RevisionDate {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_us: None,
        };
        if chars.len() == date_len {
            return Some(out);
        }
        let time: String = chars[date_len + 1..].iter().collect();
        if time.is_empty() {
            return Some(out);
        }
        if time.chars().count() < 2 {
            return None;
        }
        let tz_pos = time
            .find('-')
            .or_else(|| time.find('+'))
            .or_else(|| time.find('Z'));
        let time_part = match tz_pos {
            Some(position) => &time[..position],
            None => &time[..],
        };
        let comps = parse_hh_mm_ss_ff(time_part)?;
        if comps[0] > 23 || comps[1] > 59 || comps[2] > 59 {
            return None;
        }
        out.hour = comps[0];
        out.minute = comps[1];
        out.second = comps[2];
        out.microsecond = comps[3];
        if let Some(position) = tz_pos {
            let zone = &time[position + 1..];
            if position + 1 == time.len() && time.ends_with('Z') {
                out.offset_us = Some(0);
            } else {
                if matches!(zone.chars().count(), 0 | 1 | 3) {
                    return None;
                }
                let tz = parse_hh_mm_ss_ff(zone)?;
                if tz[1] > 59 || tz[2] > 59 {
                    return None;
                }
                let magnitude = (tz[0] as i64 * 3600 + tz[1] as i64 * 60 + tz[2] as i64)
                    * 1_000_000
                    + tz[3] as i64;
                if magnitude >= 86_400 * 1_000_000 {
                    return None;
                }
                let sign = if time.as_bytes()[position] == b'-' {
                    -1
                } else {
                    1
                };
                out.offset_us = Some(sign * magnitude);
            }
        }
        Some(out)
    }

    /// Python `datetime.isoformat()`.
    pub fn isoformat(&self) -> String {
        let mut out = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        if self.microsecond != 0 {
            out.push_str(&format!(".{:06}", self.microsecond));
        }
        if let Some(offset) = self.offset_us {
            let sign = if offset < 0 { '-' } else { '+' };
            let magnitude = offset.unsigned_abs();
            let micros = magnitude % 1_000_000;
            let secs = magnitude / 1_000_000;
            out.push_str(&format!(
                "{sign}{:02}:{:02}",
                secs / 3600,
                (secs % 3600) / 60
            ));
            if secs % 60 != 0 || micros != 0 {
                out.push_str(&format!(":{:02}", secs % 60));
            }
            if micros != 0 {
                out.push_str(&format!(".{micros:06}"));
            }
        }
        out
    }
}

/// Upstream `_parse_date`.
fn parse_date(value: Option<&str>) -> Option<RevisionDate> {
    let value = value.filter(|v| !v.is_empty())?;
    RevisionDate::from_isoformat(&value.replace('Z', "+00:00"))
}

// ---------------------------------------------------------------------------
// Trees and node references
// ---------------------------------------------------------------------------

/// Read access to (possibly staged) part trees.
pub(crate) trait Trees {
    fn tree_of(&self, part: &str) -> Result<&XmlTree>;
}

impl Trees for Document {
    fn tree_of(&self, part: &str) -> Result<&XmlTree> {
        self.package().xml(part)
    }
}

impl Trees for Txn<'_> {
    fn tree_of(&self, part: &str) -> Result<&XmlTree> {
        self.tree(part)
    }
}

/// One element in one part.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeRef {
    pub part: String,
    pub id: NodeId,
}

impl NodeRef {
    fn new(part: &str, id: NodeId) -> Self {
        NodeRef {
            part: part.to_string(),
            id,
        }
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Inert location evidence for a revision (`index` is `-1` for story-level
/// section properties outside every block).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct RevisionAnchor {
    /// Story name.
    pub story: String,
    /// Block index (or -1).
    pub index: i64,
    /// Content hash of the block text (current view).
    pub content_hash: String,
}

/// One tracked change, addressable — and resolvable when supported.
#[derive(Debug, Clone)]
pub struct Revision {
    /// `insertion`, `deletion`, `move_from`, `move_to`, `format_change`,
    /// `row_insertion`, `row_deletion`, or an exotic (unresolvable) type.
    pub revision_type: &'static str,
    /// `w:author` (empty when absent).
    pub author: String,
    /// Parsed `w:date`.
    pub date: Option<RevisionDate>,
    /// Text the revision carries (or applies to, for property changes).
    pub text: String,
    /// Story name.
    pub story: String,
    /// Location evidence.
    pub anchor: RevisionAnchor,
    /// A paragraph-mark revision (`w:pPr/w:rPr/…`).
    pub is_paragraph_mark: bool,
    pub(crate) node: NodeRef,
    pub(crate) document: DocumentId,
    pub(crate) signature: Arc<Vec<NodeRef>>,
}

impl Revision {
    /// Whether `accept`/`reject` can resolve this type.
    pub fn is_resolvable(&self) -> bool {
        RESOLVABLE_TYPES.contains(&self.revision_type)
    }

    /// Upstream `Revision.to_dict()`.
    pub fn to_json(&self) -> Value {
        json!({
            "revision_type": self.revision_type,
            "author": self.author,
            "date": self.date.map(|d| d.isoformat()),
            "text": self.text,
            "story": self.story,
            "anchor": self.anchor,
            "anchor_role": "legacy_inert_location_evidence",
            "is_paragraph_mark": self.is_paragraph_mark,
        })
    }

    /// Apply this change (moves resolve as a pair). Refuses unresolvable
    /// types, a stale revision, a protected document, and any resolution
    /// whose dependencies cannot be cleaned up safely; refusals change
    /// nothing.
    pub fn accept(&self, document: &mut Document) -> Result<()> {
        resolve_individual(document, self, true)
    }

    /// Undo this change, restoring the pre-change content.
    pub fn reject(&self, document: &mut Document) -> Result<()> {
        resolve_individual(document, self, false)
    }

    fn refuse_unresolvable(&self, verb: &str) -> Result<()> {
        if self.is_resolvable() {
            return Ok(());
        }
        Err(unsupported(
            "unresolvable_revision_type",
            format!(
                "cannot {verb} a {} revision: this revision type is enumerated but not resolvable by \
                 paper-docx (resolve it in Word, or a later paper-docx version)",
                py_repr(self.revision_type)
            ),
        ))
    }
}

/// All tracked changes in a document, across every story part.
#[derive(Debug, Clone)]
pub struct Revisions {
    document: DocumentId,
    items: Vec<Revision>,
    signature: Arc<Vec<NodeRef>>,
}

impl Revisions {
    /// Number of revisions.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// No revisions.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Revisions in enumeration order.
    pub fn iter(&self) -> std::slice::Iter<'_, Revision> {
        self.items.iter()
    }

    /// Revision `index`.
    pub fn get(&self, index: usize) -> Option<&Revision> {
        self.items.get(index)
    }

    /// The revisions as a slice.
    pub fn as_slice(&self) -> &[Revision] {
        &self.items
    }

    /// `{revision_type: count}` for enumerated-but-unresolvable revisions.
    pub fn remaining_unsupported(&self) -> BTreeMap<String, usize> {
        let mut census = BTreeMap::new();
        for revision in &self.items {
            if !revision.is_resolvable() {
                *census
                    .entry(revision.revision_type.to_string())
                    .or_default() += 1;
            }
        }
        census
    }

    /// Upstream `Revisions.to_dict()`.
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "paper_revisions",
            "version": 4,
            "revisions": self.items.iter().map(Revision::to_json).collect::<Vec<_>>(),
            "remaining_unsupported": self.remaining_unsupported(),
        })
    }

    /// Apply every selected revision (optionally only `author`'s); returns
    /// the count. The whole selection is validated first.
    pub fn accept_all(&mut self, document: &mut Document, author: Option<&str>) -> Result<usize> {
        resolve_all(document, self, true, author)
    }

    /// Undo every selected revision (optionally only `author`'s).
    pub fn reject_all(&mut self, document: &mut Document, author: Option<&str>) -> Result<usize> {
        resolve_all(document, self, false, author)
    }
}

impl<'a> IntoIterator for &'a Revisions {
    type Item = &'a Revision;
    type IntoIter = std::slice::Iter<'a, Revision>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl std::ops::Index<usize> for Revisions {
    type Output = Revision;

    fn index(&self, index: usize) -> &Revision {
        &self.items[index]
    }
}

impl Document {
    /// A fresh snapshot of every tracked change (upstream `Document.revisions`).
    pub fn revisions(&self) -> Result<Revisions> {
        snapshot(self)
    }
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

struct Raw {
    node: NodeRef,
    revision_type: &'static str,
    author: String,
    date: Option<RevisionDate>,
    text: String,
    story: String,
    anchor: RevisionAnchor,
    is_paragraph_mark: bool,
}

fn node_text(tree: &XmlTree, node: NodeId) -> String {
    iter_w(tree, node, &["t", "delText"])
        .map(|text| leading_text(tree, text))
        .collect()
}

fn applies_to_text(tree: &XmlTree, node: NodeId) -> String {
    let Some(parent) = tree.parent(node) else {
        return String::new();
    };
    if is_tag(tree, parent, "rPr") {
        let mut holder = tree.parent(parent);
        if let Some(h) = holder
            && is_tag(tree, h, "pPr")
        {
            holder = tree.parent(h);
        }
        return holder.map(|h| node_text(tree, h)).unwrap_or_default();
    }
    if is_tag(tree, parent, "pPr") {
        return tree
            .parent(parent)
            .map(|p| node_text(tree, p))
            .unwrap_or_default();
    }
    String::new()
}

/// Upstream `_is_paragraph_mark_revision`.
fn is_paragraph_mark(tree: &XmlTree, node: NodeId) -> bool {
    tree.parent(node).is_some_and(|parent| {
        is_tag(tree, parent, "rPr")
            && tree
                .parent(parent)
                .is_some_and(|grand| is_tag(tree, grand, "pPr"))
    })
}

/// Upstream `_iter_revision_nodes` (pre-order over first-choice children).
fn iter_revision_nodes(tree: &XmlTree, element: NodeId, skip_text_boxes: bool) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = first_choice_children(tree, element)
        .into_iter()
        .rev()
        .collect();
    while let Some(child) = stack.pop() {
        if skip_text_boxes && is_tag(tree, child, "txbxContent") {
            continue;
        }
        if is_revision_tag(tree, child) {
            out.push(child);
        }
        if tree.element(child).is_some() {
            stack.extend(first_choice_children(tree, child).into_iter().rev());
        }
    }
    out
}

fn raw_revision(
    tree: &XmlTree,
    part: &str,
    story: &str,
    node: NodeId,
    anchor: RevisionAnchor,
) -> Raw {
    let revision_type = revision_type_of(tree, node);
    let mut text = node_text(tree, node);
    if text.is_empty() && (is_tag(tree, node, "rPrChange") || is_tag(tree, node, "pPrChange")) {
        text = applies_to_text(tree, node);
    }
    Raw {
        node: NodeRef::new(part, node),
        revision_type,
        author: w_attr(tree, node, "author").unwrap_or("").to_string(),
        date: parse_date(w_attr(tree, node, "date")),
        text,
        story: story.to_string(),
        anchor,
        is_paragraph_mark: is_paragraph_mark(tree, node),
    }
}

/// Upstream `_enumerate_revisions`.
fn enumerate(trees: &impl Trees, stories: &[Story], document: DocumentId) -> Result<Vec<Raw>> {
    let mut out = Vec::new();
    for story in stories {
        let tree = trees.tree_of(&story.part)?;
        for block in story::story_blocks(
            tree,
            &story.name,
            &story.part,
            Some(document),
            View::Current,
        )? {
            let skip = block.kind == BlockKind::Paragraph;
            for node in iter_revision_nodes(tree, block.element, skip) {
                let anchor = RevisionAnchor {
                    story: block.anchor.story.clone(),
                    index: block.anchor.index as i64,
                    content_hash: block.anchor.content_hash.clone(),
                };
                out.push(raw_revision(tree, &story.part, &story.name, node, anchor));
            }
        }
        for sect_pr in iter_w(tree, tree.root(), &["sectPr"]).collect::<Vec<_>>() {
            if tree.parent(sect_pr).is_some_and(|p| is_tag(tree, p, "pPr")) {
                continue;
            }
            for node in iter_revision_nodes(tree, sect_pr, false) {
                let anchor = RevisionAnchor {
                    story: story.name.clone(),
                    index: -1,
                    content_hash: content_hash(""),
                };
                let mut raw = raw_revision(tree, &story.part, &story.name, node, anchor);
                raw.text = String::new();
                raw.is_paragraph_mark = false;
                out.push(raw);
            }
        }
    }
    Ok(out)
}

fn signature_of(
    trees: &impl Trees,
    stories: &[Story],
    document: DocumentId,
) -> Result<Vec<NodeRef>> {
    Ok(enumerate(trees, stories, document)?
        .into_iter()
        .map(|raw| raw.node)
        .collect())
}

fn snapshot(document: &Document) -> Result<Revisions> {
    let stories = story::stories(document);
    let raws = enumerate(document, &stories, document.id())?;
    let signature = Arc::new(raws.iter().map(|raw| raw.node.clone()).collect::<Vec<_>>());
    let items = raws
        .into_iter()
        .map(|raw| Revision {
            revision_type: raw.revision_type,
            author: raw.author,
            date: raw.date,
            text: raw.text,
            story: raw.story,
            anchor: raw.anchor,
            is_paragraph_mark: raw.is_paragraph_mark,
            node: raw.node,
            document: document.id(),
            signature: signature.clone(),
        })
        .collect();
    Ok(Revisions {
        document: document.id(),
        items,
        signature,
    })
}

/// Upstream `_remaining_markup`: `{local-name: count}` of all revision markup
/// left anywhere in the story parts (fallback branches included). Empty after
/// an unfiltered `accept_all`/`reject_all`.
pub fn remaining_markup(document: &Document) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    for story in story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        for node in tree.descendants(tree.root()) {
            if is_scan_tag(tree, node) {
                *counts
                    .entry(w_local(tree, node).unwrap_or("").to_string())
                    .or_default() += 1;
            }
        }
    }
    Ok(counts)
}

// ---------------------------------------------------------------------------
// Resolution context
// ---------------------------------------------------------------------------

pub(crate) struct Ctx<'a, 'd> {
    pub txn: Txn<'d>,
    pub stories: &'a [Story],
    pub main: String,
    pub document: DocumentId,
}

impl<'a, 'd> Ctx<'a, 'd> {
    pub(crate) fn new(document: &'d Document, stories: &'a [Story]) -> Self {
        Ctx {
            txn: Txn::new(document),
            stories,
            main: document.package().main_part_name().to_string(),
            document: document.id(),
        }
    }

    pub(crate) fn tree(&self, part: &str) -> Result<&XmlTree> {
        self.txn.tree(part)
    }

    pub(crate) fn tree_mut(&mut self, part: &str) -> Result<&mut XmlTree> {
        self.txn.tree_mut(part)
    }

    fn is_story_part(&self, part: &str) -> bool {
        self.stories.iter().any(|story| story.part == part)
    }

    /// Upstream `_is_in_story`.
    fn is_in_story(&self, node: &NodeRef) -> Result<bool> {
        if !self.is_story_part(&node.part) {
            return Ok(false);
        }
        Ok(self.tree(&node.part)?.is_attached(node.id))
    }

    fn attached(&self, node: &NodeRef) -> Result<bool> {
        let tree = self.tree(&node.part)?;
        Ok(tree.contains(node.id) && tree.parent(node.id).is_some())
    }
}

/// Upstream `_related_xml_root`: the single related live-XML part or a
/// refusal for an ambiguous/opaque one.
pub(crate) fn related_xml_part(
    txn: &Txn<'_>,
    main: &str,
    rel_type: &str,
    label: &str,
    purpose: &str,
) -> Result<Option<String>> {
    let part = match txn.part_related_by(main, rel_type) {
        Ok(part) => part,
        Err(err) if err.reason() == "ambiguous_relationship" => {
            return Err(unsupported(
                "ambiguous_relationship",
                format!(
                    "multiple {label} relationships make {purpose} ambiguous; nothing was changed"
                ),
            ));
        }
        Err(err) => return Err(err),
    };
    let Some(part) = part else { return Ok(None) };
    let live = txn
        .content_type(&part)
        .is_some_and(|ct| ct::EAGER_XML.contains(&ct.as_str()));
    if !live {
        return Err(unsupported(
            "part_not_live_xml",
            format!("{label} part is not loaded as live XML; nothing was changed"),
        ));
    }
    Ok(Some(part))
}

// ---------------------------------------------------------------------------
// Moves
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MoveSite {
    story: String,
    name: String,
    from: bool,
    start: NodeRef,
    end: Option<NodeRef>,
    wrappers: Vec<NodeRef>,
    mark_stamps: Vec<NodeRef>,
}

impl MoveSite {
    fn elements(&self) -> Vec<NodeRef> {
        let mut out = vec![self.start.clone()];
        out.extend(self.end.clone());
        out.extend(self.wrappers.iter().cloned());
        out.extend(self.mark_stamps.iter().cloned());
        out
    }
}

#[derive(Debug, Clone)]
struct MoveUnit {
    name: String,
    from_site: MoveSite,
    to_site: MoveSite,
}

impl MoveUnit {
    fn elements(&self) -> Vec<NodeRef> {
        let mut out = self.from_site.elements();
        out.extend(self.to_site.elements());
        out
    }

    fn contains(&self, node: &NodeRef) -> bool {
        self.elements().iter().any(|element| element == node)
    }
}

type Orphans = Vec<(NodeRef, String)>;

fn scan_story_moves(
    tree: &XmlTree,
    story: &Story,
    sites: &mut Vec<MoveSite>,
    orphans: &mut Orphans,
) {
    let mut open: [Option<MoveSite>; 2] = [None, None];
    let mut stack: Vec<NodeId> = first_choice_children(tree, tree.root())
        .into_iter()
        .rev()
        .collect();
    while let Some(child) = stack.pop() {
        let role = match w_local(tree, child) {
            Some("moveFromRangeStart") => Some((0, 's')),
            Some("moveToRangeStart") => Some((1, 's')),
            Some("moveFromRangeEnd") => Some((0, 'e')),
            Some("moveToRangeEnd") => Some((1, 'e')),
            Some("moveFrom") => Some((0, 'c')),
            Some("moveTo") => Some((1, 'c')),
            _ => None,
        };
        if let Some((direction, kind)) = role {
            let node = NodeRef::new(&story.part, child);
            match kind {
                's' => {
                    let name = w_attr(tree, child, "name");
                    if let Some(site) = open[direction].take() {
                        orphans.push((node, "overlapping move ranges".to_string()));
                        orphans.extend(
                            site.elements()
                                .into_iter()
                                .map(|e| (e, "overlapping move ranges".to_string())),
                        );
                    } else if name.is_none_or(str::is_empty) {
                        orphans.push((node, "move range without a w:name".to_string()));
                    } else {
                        open[direction] = Some(MoveSite {
                            story: story.name.clone(),
                            name: name.unwrap_or("").to_string(),
                            from: direction == 0,
                            start: node,
                            end: None,
                            wrappers: Vec::new(),
                            mark_stamps: Vec::new(),
                        });
                    }
                }
                'e' => {
                    let matches = open[direction].as_ref().is_some_and(|site| {
                        w_attr(tree, child, "id") == w_attr(tree, site.start.id, "id")
                    });
                    if matches {
                        let mut site = open[direction].take().unwrap_or_else(|| unreachable!());
                        site.end = Some(node);
                        sites.push(site);
                    } else {
                        orphans.push((node, "move range end without its start".to_string()));
                    }
                }
                _ => match open[direction].as_mut() {
                    None => orphans.push((node, "move content outside any move range".to_string())),
                    Some(site) => {
                        if is_paragraph_mark(tree, child) {
                            site.mark_stamps.push(node);
                        } else {
                            site.wrappers.push(node);
                        }
                    }
                },
            }
        }
        if tree.element(child).is_some() {
            stack.extend(first_choice_children(tree, child).into_iter().rev());
        }
    }
    for site in open.into_iter().flatten() {
        orphans.push((site.start, "move range start without its end".to_string()));
    }
}

/// Upstream `_move_units`.
fn move_units(trees: &impl Trees, stories: &[Story]) -> Result<(Vec<MoveUnit>, Orphans)> {
    let mut sites = Vec::new();
    let mut orphans = Vec::new();
    for story in stories {
        scan_story_moves(trees.tree_of(&story.part)?, story, &mut sites, &mut orphans);
    }
    let mut by_name: BTreeMap<String, (Vec<MoveSite>, Vec<MoveSite>)> = BTreeMap::new();
    for site in sites {
        let entry = by_name.entry(site.name.clone()).or_default();
        if site.from {
            entry.0.push(site);
        } else {
            entry.1.push(site);
        }
    }
    let mut units = Vec::new();
    for (name, (froms, tos)) in by_name {
        if froms.len() != 1 || tos.len() != 1 {
            let reason = format!(
                "move {} has {} source and {} destination range(s); expected exactly one of each",
                py_repr(&name),
                froms.len(),
                tos.len()
            );
            for site in froms.iter().chain(tos.iter()) {
                orphans.extend(site.elements().into_iter().map(|e| (e, reason.clone())));
            }
            continue;
        }
        if froms[0].story != tos[0].story {
            let reason = format!(
                "move {} crosses stories ({} -> {}); cross-story moves are not resolvable",
                py_repr(&name),
                froms[0].story,
                tos[0].story
            );
            for site in froms.iter().chain(tos.iter()) {
                orphans.extend(site.elements().into_iter().map(|e| (e, reason.clone())));
            }
            continue;
        }
        let mut froms = froms;
        let mut tos = tos;
        units.push(MoveUnit {
            name,
            from_site: froms.remove(0),
            to_site: tos.remove(0),
        });
    }
    Ok((units, orphans))
}

/// Upstream `_validate_moves`.
fn validate_moves(ctx: &Ctx<'_, '_>, selected: &[Selected], author: Option<&str>) -> Result<()> {
    let move_nodes: Vec<&NodeRef> = selected
        .iter()
        .filter(|s| s.is_move())
        .map(|s| &s.node)
        .collect();
    if move_nodes.is_empty() {
        return Ok(());
    }
    let (units, orphans) = move_units(&ctx.txn, ctx.stories)?;
    let mut touched: Vec<usize> = Vec::new();
    for node in move_nodes {
        if let Some(index) = units.iter().position(|unit| unit.contains(node)) {
            if !touched.contains(&index) {
                touched.push(index);
            }
            continue;
        }
        let reason = orphans
            .iter()
            .find(|(element, _)| element == node)
            .map(|(_, reason)| reason.clone())
            .unwrap_or_else(|| "its range markers were not found".to_string());
        return Err(unsupported(
            "unresolvable_move",
            format!(
                "selected revisions include an unresolvable tracked move: {reason}; nothing was changed. \
                 Resolve it in Word instead"
            ),
        ));
    }
    let Some(author) = author else { return Ok(()) };
    for index in touched {
        let unit = &units[index];
        let mut authors: Vec<String> = Vec::new();
        let mut add = |value: String| {
            if !authors.contains(&value) {
                authors.push(value);
            }
        };
        for element in unit
            .from_site
            .wrappers
            .iter()
            .chain(&unit.from_site.mark_stamps)
            .chain(&unit.to_site.wrappers)
            .chain(&unit.to_site.mark_stamps)
        {
            let tree = ctx.tree(&element.part)?;
            add(w_attr(tree, element.id, "author").unwrap_or("").to_string());
        }
        for element in [
            Some(&unit.from_site.start),
            unit.from_site.end.as_ref(),
            Some(&unit.to_site.start),
            unit.to_site.end.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let tree = ctx.tree(&element.part)?;
            if let Some(value) = w_attr(tree, element.id, "author") {
                add(value.to_string());
            }
        }
        if authors.len() != 1 || authors[0] != author {
            authors.sort();
            return Err(unsupported(
                "mixed_author_move",
                format!(
                    "cannot resolve move {} with author={}: its compound move unit's authors differ ({}); \
                     nothing was changed",
                    py_repr(&unit.name),
                    py_repr(author),
                    py_list(&authors)
                ),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Selected {
    node: NodeRef,
    revision_type: &'static str,
    is_paragraph_mark: bool,
}

impl Selected {
    fn is_move(&self) -> bool {
        matches!(self.revision_type, "move_from" | "move_to")
    }
}

/// Upstream `_comment_id` / `_note_id` (malformed → refusal).
fn marker_id(tree: &XmlTree, node: NodeId, note: bool) -> Result<i64> {
    let raw = w_attr(tree, node, "id");
    match raw.and_then(py_int) {
        Some(value) => Ok(value),
        None => {
            let name = tree.local_name(node).unwrap_or("");
            Err(unsupported(
                if note {
                    "malformed_note_id"
                } else {
                    "malformed_comment_id"
                },
                if note {
                    format!(
                        "malformed {name} w:id={}; nothing was changed",
                        py_opt_repr(raw)
                    )
                } else {
                    format!(
                        "malformed {name} comment marker w:id={}; nothing was changed",
                        py_opt_repr(raw)
                    )
                },
            ))
        }
    }
}

fn comment_id(tree: &XmlTree, node: NodeId) -> Result<i64> {
    marker_id(tree, node, false)
}

fn note_id(tree: &XmlTree, node: NodeId) -> Result<i64> {
    marker_id(tree, node, true)
}

const COMMENT_MARKERS: &[&str] = &["commentRangeStart", "commentRangeEnd", "commentReference"];

/// Upstream `_validate_comment_marker_ids`.
fn validate_comment_marker_ids(ctx: &Ctx<'_, '_>) -> Result<()> {
    for story in ctx.stories {
        let tree = ctx.tree(&story.part)?;
        for marker in iter_w(tree, tree.root(), COMMENT_MARKERS) {
            comment_id(tree, marker)?;
        }
        for comment in iter_w(tree, tree.root(), &["comment"]) {
            comment_id(tree, comment)?;
        }
    }
    if let Some(part) = related_xml_part(
        &ctx.txn,
        &ctx.main,
        rt::COMMENTS,
        "comments",
        "revision cleanup",
    )? {
        let tree = ctx.tree(&part)?;
        for comment in iter_w(tree, tree.root(), &["comment"]) {
            comment_id(tree, comment)?;
        }
    }
    related_xml_part(
        &ctx.txn,
        &ctx.main,
        crate::commentops::COMMENTS_EXTENDED_RELATIONSHIP_TYPE,
        "commentsExtended",
        "revision cleanup",
    )?;
    Ok(())
}

fn paragraph_of_mark(tree: &XmlTree, node: NodeId) -> Option<NodeId> {
    let r_pr = tree.parent(node)?;
    let p_pr = tree.parent(r_pr)?;
    let paragraph = tree.parent(p_pr)?;
    is_tag(tree, paragraph, "p").then_some(paragraph)
}

fn paragraph_has_content(tree: &XmlTree, paragraph: NodeId) -> bool {
    node_children(tree, paragraph).any(|child| !is_tag(tree, child, "pPr"))
}

fn is_last_block_in_container(tree: &XmlTree, paragraph: NodeId) -> bool {
    let Some(parent) = tree.parent(paragraph) else {
        return true;
    };
    !tree
        .child_elements(parent)
        .any(|c| c != paragraph && (is_tag(tree, c, "p") || is_tag(tree, c, "tbl")))
}

const TRANSPARENT_BLOCK_MARKERS: &[&str] = &[
    "bookmarkStart",
    "bookmarkEnd",
    "commentRangeStart",
    "commentRangeEnd",
    "permStart",
    "permEnd",
    "proofErr",
    "moveFromRangeStart",
    "moveFromRangeEnd",
    "moveToRangeStart",
    "moveToRangeEnd",
    "customXmlInsRangeStart",
    "customXmlInsRangeEnd",
    "customXmlDelRangeStart",
    "customXmlDelRangeEnd",
    "customXmlMoveFromRangeStart",
    "customXmlMoveFromRangeEnd",
    "customXmlMoveToRangeStart",
    "customXmlMoveToRangeEnd",
];

/// Upstream `_next_paragraph_sibling`.
fn next_paragraph_sibling(tree: &XmlTree, paragraph: NodeId) -> Result<Option<NodeId>> {
    let mut node = lxml_next(tree, paragraph);
    while let Some(current) = node {
        let local = w_local(tree, current);
        match local {
            Some("p") => return Ok(Some(current)),
            Some("tbl" | "sdt" | "sectPr") => return Ok(None),
            Some(l) if TRANSPARENT_BLOCK_MARKERS.contains(&l) => {
                node = lxml_next(tree, current);
                continue;
            }
            _ => {}
        }
        let name = tree.local_name(current).unwrap_or("#node").to_string();
        return Err(unsupported(
            "unsafe_paragraph_join",
            format!(
                "cannot resolve a paragraph-mark revision across the {} block; its content boundary is not a \
                 safe paragraph join. Nothing was changed",
                py_repr(&name)
            ),
        ));
    }
    Ok(None)
}

fn paragraph_mark_applies(tree: &XmlTree, node: NodeId, accept: bool) -> bool {
    let is_deletion = is_tag(tree, node, "del") || is_tag(tree, node, "moveFrom");
    (accept && is_deletion) || (!accept && !is_deletion)
}

/// Upstream `_validate_paragraph_mark_joins`.
fn validate_paragraph_mark_joins(
    ctx: &Ctx<'_, '_>,
    selected: &[Selected],
    accept: bool,
) -> Result<()> {
    let mut marks: Vec<NodeRef> = selected
        .iter()
        .filter(|s| s.is_paragraph_mark)
        .map(|s| s.node.clone())
        .collect();
    let moves: Vec<&NodeRef> = selected
        .iter()
        .filter(|s| s.is_move())
        .map(|s| &s.node)
        .collect();
    if !moves.is_empty() {
        let (units, _) = move_units(&ctx.txn, ctx.stories)?;
        for unit in &units {
            if moves.iter().any(|node| unit.contains(node)) {
                marks.extend(unit.from_site.mark_stamps.iter().cloned());
                marks.extend(unit.to_site.mark_stamps.iter().cloned());
            }
        }
    }
    let mut seen = HashSet::new();
    for mark in marks {
        if !seen.insert(mark.clone()) {
            continue;
        }
        let tree = ctx.tree(&mark.part)?;
        if let Some(paragraph) = paragraph_of_mark(tree, mark.id)
            && paragraph_mark_applies(tree, mark.id, accept)
            && paragraph_has_content(tree, paragraph)
        {
            next_paragraph_sibling(tree, paragraph)?;
        }
    }
    Ok(())
}

/// Upstream `_resolution_discard_roots`.
fn discard_roots(ctx: &Ctx<'_, '_>, selected: &[Selected], accept: bool) -> Result<Vec<NodeRef>> {
    let (units, _) = move_units(&ctx.txn, ctx.stories)?;
    let mut roots: Vec<NodeRef> = Vec::new();
    let add = |root: NodeRef, roots: &mut Vec<NodeRef>| {
        if !roots.contains(&root) {
            roots.push(root);
        }
    };
    for revision in selected {
        let node = &revision.node;
        let tree = ctx.tree(&node.part)?;
        if revision.is_paragraph_mark
            && paragraph_mark_applies(tree, node.id, accept)
            && let Some(paragraph) = paragraph_of_mark(tree, node.id)
        {
            if paragraph_has_content(tree, paragraph) {
                if next_paragraph_sibling(tree, paragraph)?.is_some()
                    && let Some(p_pr) = tree.first_child_named(paragraph, W, "pPr")
                {
                    add(NodeRef::new(&node.part, p_pr), &mut roots);
                }
            } else if !is_last_block_in_container(tree, paragraph) {
                add(NodeRef::new(&node.part, paragraph), &mut roots);
            }
        }
        if revision.is_move() {
            if let Some(unit) = units.iter().find(|unit| unit.contains(node)) {
                let drop = if accept {
                    &unit.from_site
                } else {
                    &unit.to_site
                };
                for wrapper in &drop.wrappers {
                    add(wrapper.clone(), &mut roots);
                }
            }
            continue;
        }
        if matches!(revision.revision_type, "row_insertion" | "row_deletion") {
            let keeps_row = (is_tag(tree, node.id, "ins") && accept)
                || (is_tag(tree, node.id, "del") && !accept);
            if keeps_row {
                continue;
            }
            let row = tree.parent(node.id).and_then(|tr_pr| tree.parent(tr_pr));
            let table = row.and_then(|row| tree.parent(row));
            match (row, table) {
                (Some(row), Some(table))
                    if is_tag(tree, row, "tr") && is_tag(tree, table, "tbl") =>
                {
                    let rows = tree
                        .child_elements(table)
                        .filter(|c| is_tag(tree, *c, "tr"))
                        .count();
                    let root = if rows == 1 { table } else { row };
                    add(NodeRef::new(&node.part, root), &mut roots);
                }
                _ => {
                    return Err(unsupported(
                        "malformed_row_revision",
                        "malformed tracked-row revision cannot be resolved; nothing was changed",
                    ));
                }
            }
            continue;
        }
        let removes_content =
            (is_tag(tree, node.id, "ins") && !accept) || (is_tag(tree, node.id, "del") && accept);
        if removes_content && !revision.is_paragraph_mark {
            add(node.clone(), &mut roots);
        }
    }
    Ok(roots)
}

/// Upstream `_validate_destructive_closure`.
fn validate_destructive_closure(
    ctx: &Ctx<'_, '_>,
    roots: &[NodeRef],
    selected: &[Selected],
    accept: bool,
    author: Option<&str>,
    individual: bool,
) -> Result<()> {
    if author.is_none() && !individual {
        return Ok(());
    }
    let mut accounted: HashSet<NodeRef> = selected.iter().map(|s| s.node.clone()).collect();
    let moves: Vec<&NodeRef> = selected
        .iter()
        .filter(|s| s.is_move())
        .map(|s| &s.node)
        .collect();
    if !moves.is_empty() {
        let (units, _) = move_units(&ctx.txn, ctx.stories)?;
        for unit in &units {
            if moves.iter().any(|node| unit.contains(node)) {
                accounted.extend(unit.elements());
            }
        }
    }
    let mut authors: Vec<String> = Vec::new();
    let mut types: Vec<String> = Vec::new();
    for root in roots {
        let tree = ctx.tree(&root.part)?;
        for node in tree.descendants(root.id) {
            if !is_scan_tag(tree, node) || accounted.contains(&NodeRef::new(&root.part, node)) {
                continue;
            }
            let node_author = w_attr(tree, node, "author").unwrap_or("");
            let shown = if node_author.is_empty() {
                "<missing author>".to_string()
            } else {
                node_author.to_string()
            };
            if !authors.contains(&shown) {
                authors.push(shown);
            }
            let kind = if is_revision_tag(tree, node) {
                revision_type_of(tree, node).to_string()
            } else {
                w_local(tree, node).unwrap_or("").to_string()
            };
            if !types.contains(&kind) {
                types.push(kind);
            }
        }
    }
    if types.is_empty() {
        return Ok(());
    }
    authors.sort();
    types.sort();
    if individual {
        let verb = if accept { "accept" } else { "reject" };
        return Err(unsupported(
            "consumes_unselected_revisions",
            format!(
                "cannot {verb} a {} revision individually: its destructive resolution would also consume nested \
                 unselected revision markup (types {}, authors {}); nothing was changed",
                py_repr(selected[0].revision_type),
                py_list(&types),
                py_list(&authors)
            ),
        ));
    }
    Err(unsupported(
        "consumes_unselected_revisions",
        format!(
            "cannot resolve revisions with author={}: a selected destructive revision would also consume \
             unselected revision markup (authors {}); nothing was changed",
            py_opt_repr(author),
            py_list(&authors)
        ),
    ))
}

fn fallback_markup(ctx: &Ctx<'_, '_>) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    for story in ctx.stories {
        let tree = ctx.tree(&story.part)?;
        for fallback in tree.descendants(tree.root()) {
            if !tree.is(fallback, MC, "Fallback") {
                continue;
            }
            for node in tree.descendants(fallback) {
                if is_scan_tag(tree, node) {
                    *counts
                        .entry(w_local(tree, node).unwrap_or("").to_string())
                        .or_default() += 1;
                }
            }
        }
    }
    Ok(counts)
}

fn py_dict(counts: &BTreeMap<String, usize>) -> String {
    let items: Vec<String> = counts
        .iter()
        .map(|(key, value)| format!("{}: {value}", py_repr(key)))
        .collect();
    format!("{{{}}}", items.join(", "))
}

/// Upstream `_refuse_unaccounted_markup`.
fn refuse_unaccounted_markup(ctx: &Ctx<'_, '_>) -> Result<()> {
    let fallback = fallback_markup(ctx)?;
    if !fallback.is_empty() {
        return Err(unsupported(
            "fallback_revision_markup",
            format!(
                "revision markup lives inside mc:AlternateContent fallback branches ({}); this package resolves \
                 the primary content only, so the saved file would still carry it. Nothing was changed — \
                 resolve this document in Word",
                py_dict(&fallback)
            ),
        ));
    }
    let (units, orphans) = move_units(&ctx.txn, ctx.stories)?;
    if !orphans.is_empty() {
        let mut reasons: Vec<String> = orphans.iter().map(|(_, reason)| reason.clone()).collect();
        reasons.sort();
        reasons.dedup();
        return Err(unsupported(
            "unpaired_move_markup",
            format!(
                "the document carries move markup this package cannot pair ({}); nothing was changed. Resolve \
                 it in Word instead",
                reasons.join("; ")
            ),
        ));
    }
    let mut marker_only: Vec<String> = units
        .iter()
        .filter(|unit| {
            unit.from_site.wrappers.is_empty()
                && unit.from_site.mark_stamps.is_empty()
                && unit.to_site.wrappers.is_empty()
                && unit.to_site.mark_stamps.is_empty()
        })
        .map(|unit| unit.name.clone())
        .collect();
    marker_only.sort();
    if !marker_only.is_empty() {
        return Err(unsupported(
            "marker_only_move",
            format!(
                "the document carries complete marker-only move units with no resolvable move content ({}); \
                 nothing was changed. Resolve it in Word instead",
                py_list(&marker_only)
            ),
        ));
    }
    let mut accounted: HashSet<NodeRef> = enumerate(&ctx.txn, ctx.stories, ctx.document)?
        .into_iter()
        .map(|raw| raw.node)
        .collect();
    for unit in &units {
        accounted.extend(unit.elements());
    }
    let mut unaccounted = BTreeMap::new();
    for story in ctx.stories {
        let tree = ctx.tree(&story.part)?;
        for node in tree.descendants(tree.root()) {
            if is_scan_tag(tree, node) && !accounted.contains(&NodeRef::new(&story.part, node)) {
                *unaccounted
                    .entry(w_local(tree, node).unwrap_or("").to_string())
                    .or_default() += 1;
            }
        }
    }
    if !unaccounted.is_empty() {
        return Err(unsupported(
            "unenumerable_revision_markup",
            format!(
                "the document carries revision markup this package cannot enumerate ({}); nothing was changed. \
                 Resolve it in Word instead",
                py_dict(&unaccounted)
            ),
        ));
    }
    Ok(())
}

fn comment_ids_inside(tree: &XmlTree, node: NodeId) -> Result<Vec<i64>> {
    iter_w(tree, node, &["commentReference"])
        .map(|reference| comment_id(tree, reference))
        .collect()
}

fn comment_range_ids_inside(tree: &XmlTree, node: NodeId) -> Result<Vec<i64>> {
    iter_w(tree, node, &["commentRangeStart", "commentRangeEnd"])
        .map(|marker| comment_id(tree, marker))
        .collect()
}

fn note_ids_inside(tree: &XmlTree, node: NodeId, reference_tag: &'static str) -> Result<Vec<i64>> {
    let tags: &[&str] = if reference_tag == "footnoteReference" {
        &["footnoteReference"]
    } else {
        &["endnoteReference"]
    };
    iter_w(tree, node, tags)
        .map(|reference| note_id(tree, reference))
        .collect()
}

fn is_relationship_attr(tree: &XmlTree, attr: &crate::xml::Attr) -> bool {
    match attr.ns.map(|ns| tree.uri(ns)) {
        Some(R_NS) => true,
        Some(O_NS) => &*attr.name.local == "relid",
        _ => false,
    }
}

pub(crate) fn relationship_values(tree: &XmlTree, node: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    for descendant in tree.descendants(node) {
        if let Some(element) = tree.element(descendant) {
            for attr in &element.attrs {
                if is_relationship_attr(tree, attr) {
                    out.push(attr.value.clone());
                }
            }
        }
    }
    out
}

const NOTE_KINDS: &[(&str, &str, &str, &str)] = &[
    ("footnoteReference", "footnote", rt::FOOTNOTES, "footnotes"),
    ("endnoteReference", "endnote", rt::ENDNOTES, "endnotes"),
];

/// Upstream `_validate_note_cleanup_graph`.
fn validate_note_cleanup_graph(ctx: &Ctx<'_, '_>) -> Result<()> {
    let mut roots: Vec<String> = ctx.stories.iter().map(|story| story.part.clone()).collect();
    if let Some(part) = related_xml_part(
        &ctx.txn,
        &ctx.main,
        rt::COMMENTS,
        "comments",
        "revision cleanup",
    )? && !roots.contains(&part)
    {
        roots.push(part);
    }
    for (_, body_tag, rel_type, label) in NOTE_KINDS {
        let Some(part) =
            related_xml_part(&ctx.txn, &ctx.main, rel_type, label, "revision cleanup")?
        else {
            continue;
        };
        if !roots.contains(&part) {
            roots.push(part.clone());
        }
        let tree = ctx.tree(&part)?;
        let tags = [*body_tag];
        for note in iter_w(tree, tree.root(), &tags) {
            note_id(tree, note)?;
        }
    }
    for part in &roots {
        let tree = ctx.tree(part)?;
        for marker in iter_w(tree, tree.root(), COMMENT_MARKERS) {
            comment_id(tree, marker)?;
        }
        for comment in iter_w(tree, tree.root(), &["comment"]) {
            comment_id(tree, comment)?;
        }
        for (reference_tag, ..) in NOTE_KINDS {
            let tags = [*reference_tag];
            for reference in iter_w(tree, tree.root(), &tags) {
                note_id(tree, reference)?;
            }
        }
    }
    Ok(())
}

/// Upstream `_validate_cleanup_dependencies`.
fn validate_cleanup_dependencies(ctx: &Ctx<'_, '_>, roots: &[NodeRef]) -> Result<()> {
    let mut has_note_references = false;
    let mut comment_ids: Vec<i64> = Vec::new();
    let mut range_ids: Vec<i64> = Vec::new();
    for root in roots {
        let tree = ctx.tree(&root.part)?;
        comment_ids.extend(comment_ids_inside(tree, root.id)?);
        range_ids.extend(comment_range_ids_inside(tree, root.id)?);
        for reference_tag in ["footnoteReference", "endnoteReference"] {
            if !note_ids_inside(tree, root.id, reference_tag)?.is_empty() {
                has_note_references = true;
            }
        }
    }
    let mut partial: Vec<i64> = range_ids
        .iter()
        .copied()
        .filter(|id| !comment_ids.contains(id))
        .collect();
    partial.sort();
    partial.dedup();
    if !partial.is_empty() {
        let shown: Vec<String> = partial.iter().map(i64::to_string).collect();
        return Err(unsupported(
            "partial_comment_cleanup",
            format!(
                "revision cleanup would remove comment range markers while leaving their reference marks live \
                 (comment ids [{}]); nothing was changed",
                shown.join(", ")
            ),
        ));
    }
    if !comment_ids.is_empty() {
        let comments = related_xml_part(
            &ctx.txn,
            &ctx.main,
            rt::COMMENTS,
            "comments",
            "revision cleanup",
        )?;
        validate_comment_cleanup_graph(ctx, &comment_ids, roots, comments.as_deref())?;
        if let Some(part) = &comments {
            let tree = ctx.tree(part)?;
            if iter_w(
                tree,
                tree.root(),
                &["footnoteReference", "endnoteReference"],
            )
            .next()
            .is_some()
            {
                has_note_references = true;
            }
        }
    }
    if has_note_references {
        validate_note_cleanup_graph(ctx)?;
    }
    Ok(())
}

/// Upstream `_validate_comment_cleanup_graph`.
fn validate_comment_cleanup_graph(
    ctx: &Ctx<'_, '_>,
    comment_ids: &[i64],
    roots: &[NodeRef],
    comments_part: Option<&str>,
) -> Result<()> {
    let is_discarded = |part: &str, tree: &XmlTree, node: NodeId| {
        std::iter::once(node)
            .chain(tree.ancestors(node))
            .any(|candidate| {
                roots
                    .iter()
                    .any(|root| root.part == part && root.id == candidate)
            })
    };
    let mut ids: Vec<i64> = comment_ids.to_vec();
    ids.sort();
    ids.dedup();
    for id in ids {
        let mut references: Vec<(String, NodeId)> = Vec::new();
        let mut starts = 0usize;
        let mut ends = 0usize;
        for story in ctx.stories {
            let tree = ctx.tree(&story.part)?;
            for marker in iter_w(tree, tree.root(), COMMENT_MARKERS) {
                if comment_id(tree, marker)? != id {
                    continue;
                }
                match w_local(tree, marker) {
                    Some("commentReference") => references.push((story.part.clone(), marker)),
                    Some("commentRangeStart") => starts += 1,
                    _ => ends += 1,
                }
            }
        }
        let single_discarded = references.len() == 1 && {
            let (part, node) = &references[0];
            is_discarded(part, ctx.tree(part)?, *node)
        };
        if !single_discarded {
            return Err(unsupported(
                "ambiguous_comment_cleanup",
                format!(
                    "comment {id} has {} reference marks; revision cleanup requires exactly one reference inside \
                     the discarded content. Nothing was changed",
                    references.len()
                ),
            ));
        }
        let bodies = match comments_part {
            None => 0,
            Some(part) => {
                let tree = ctx.tree(part)?;
                let mut count = 0;
                for comment in iter_w(tree, tree.root(), &["comment"]) {
                    if comment_id(tree, comment)? == id {
                        count += 1;
                    }
                }
                count
            }
        };
        if bodies != 1 {
            return Err(unsupported(
                "ambiguous_comment_cleanup",
                format!(
                    "comment {id} has {bodies} comment bodies; revision cleanup requires exactly one. Nothing was \
                     changed"
                ),
            ));
        }
        if starts != ends || starts > 1 {
            return Err(unsupported(
                "ambiguous_comment_cleanup",
                format!("comment {id} has an ambiguous range-marker graph; nothing was changed"),
            ));
        }
    }
    Ok(())
}

/// Upstream `_preflight_resolution`.
fn preflight(
    ctx: &Ctx<'_, '_>,
    selected: &[Selected],
    accept: bool,
    author: Option<&str>,
    require_clean: bool,
    individual: bool,
) -> Result<()> {
    validate_comment_marker_ids(ctx)?;
    validate_moves(ctx, selected, author)?;
    if require_clean {
        refuse_unaccounted_markup(ctx)?;
    }
    validate_paragraph_mark_joins(ctx, selected, accept)?;
    let roots = discard_roots(ctx, selected, accept)?;
    validate_destructive_closure(ctx, &roots, selected, accept, author, individual)?;
    validate_cleanup_dependencies(ctx, &roots)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

fn part_rel_ids(ctx: &Ctx<'_, '_>, part: &str) -> Result<Vec<String>> {
    Ok(ctx
        .txn
        .relationships(part)?
        .into_iter()
        .map(|rel| rel.id)
        .collect())
}

/// Remove relationship `r_id` from `part`'s relationships part.
pub(crate) fn drop_rel(txn: &mut Txn<'_>, part: &str, r_id: &str) -> Result<()> {
    let rels_member = rels_member_for(&format!("/{}", part.trim_start_matches('/')));
    let Some(member) = txn.resolve(&rels_member) else {
        return Ok(());
    };
    let tree = txn.tree_mut(&member)?;
    let root = tree.root();
    let doomed: Vec<NodeId> = tree
        .child_elements(root)
        .filter(|rel| {
            tree.is(*rel, RELS_NS, "Relationship") && tree.attr(*rel, None, "Id") == Some(r_id)
        })
        .collect();
    for rel in doomed {
        tree.detach(rel)?;
    }
    Ok(())
}

fn drop_unreferenced_relationships(
    ctx: &mut Ctx<'_, '_>,
    part: &str,
    candidates: &[String],
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    let referenced: HashSet<String> = {
        let tree = ctx.tree(part)?;
        relationship_values(tree, tree.root()).into_iter().collect()
    };
    let mut doomed: Vec<String> = candidates
        .iter()
        .filter(|r_id| !referenced.contains(*r_id))
        .cloned()
        .collect();
    doomed.sort();
    doomed.dedup();
    let existing = part_rel_ids(ctx, part)?;
    for r_id in doomed {
        if existing.contains(&r_id) {
            drop_rel(&mut ctx.txn, part, &r_id)?;
        }
    }
    Ok(())
}

fn note_reference_remains(ctx: &Ctx<'_, '_>, reference_tag: &'static str, id: i64) -> Result<bool> {
    let tags = [reference_tag];
    for story in ctx.stories {
        let tree = ctx.tree(&story.part)?;
        for reference in iter_w(tree, tree.root(), &tags) {
            if note_id(tree, reference)? == id {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn cleanup_note_bodies(ctx: &mut Ctx<'_, '_>, footnotes: &[i64], endnotes: &[i64]) -> Result<()> {
    for (index, (reference_tag, body_tag, rel_type, label)) in NOTE_KINDS.iter().enumerate() {
        let ids = if index == 0 { footnotes } else { endnotes };
        let mut removable = Vec::new();
        for id in ids {
            if !note_reference_remains(ctx, reference_tag, *id)? && !removable.contains(id) {
                removable.push(*id);
            }
        }
        if removable.is_empty() {
            continue;
        }
        let part = match ctx.txn.part_related_by(&ctx.main, rel_type) {
            Ok(Some(part)) => part,
            Ok(None) => continue,
            Err(err) => return Err(err),
        };
        if !ctx
            .txn
            .content_type(&part)
            .is_some_and(|ct| ct::EAGER_XML.contains(&ct.as_str()))
        {
            return Err(unsupported(
                "part_not_live_xml",
                format!(
                    "{label} part is not loaded as live XML; cleanup cannot continue without leaving orphaned \
                     note bodies"
                ),
            ));
        }
        let doomed: Vec<NodeId> = {
            let tree = ctx.tree(&part)?;
            let mut doomed = Vec::new();
            for note in tree.child_elements(tree.root()).collect::<Vec<_>>() {
                if is_tag(tree, note, body_tag)
                    && w_attr(tree, note, "type").is_none()
                    && removable.contains(&note_id(tree, note)?)
                {
                    doomed.push(note);
                }
            }
            doomed
        };
        for note in doomed {
            discard_element(ctx, &NodeRef::new(&part, note))?;
        }
    }
    Ok(())
}

fn cleanup_comments_extended(ctx: &mut Ctx<'_, '_>, paragraph_ids: &[String]) -> Result<()> {
    if paragraph_ids.is_empty() {
        return Ok(());
    }
    let part = match ctx.txn.part_related_by(
        &ctx.main,
        crate::commentops::COMMENTS_EXTENDED_RELATIONSHIP_TYPE,
    ) {
        Ok(Some(part)) => part,
        Ok(None) => return Ok(()),
        Err(err) => return Err(err),
    };
    if !ctx
        .txn
        .content_type(&part)
        .is_some_and(|ct| ct::EAGER_XML.contains(&ct.as_str()))
    {
        return Err(unsupported(
            "part_not_live_xml",
            "commentsExtended is not loaded as live XML",
        ));
    }
    let doomed: Vec<NodeId> = {
        let tree = ctx.tree(&part)?;
        tree.child_elements(tree.root())
            .filter(|entry| {
                tree.is(*entry, W15, "commentEx")
                    && tree
                        .attr(*entry, Some(W15), "paraId")
                        .is_some_and(|value| paragraph_ids.iter().any(|id| id == value))
            })
            .collect()
    };
    let tree = ctx.tree_mut(&part)?;
    for entry in doomed {
        tree.detach(entry)?;
    }
    Ok(())
}

fn cleanup_comment_anchors(ctx: &mut Ctx<'_, '_>, comment_ids: &[i64]) -> Result<()> {
    if comment_ids.is_empty() {
        return Ok(());
    }
    let stories: &[Story] = ctx.stories;
    for story in stories {
        let doomed: Vec<NodeId> = {
            let tree = ctx.tree(&story.part)?;
            let mut doomed = Vec::new();
            for marker in iter_w(tree, tree.root(), &["commentRangeStart", "commentRangeEnd"]) {
                if comment_ids.contains(&comment_id(tree, marker)?) {
                    doomed.push(marker);
                }
            }
            doomed
        };
        if doomed.is_empty() {
            continue;
        }
        let tree = ctx.tree_mut(&story.part)?;
        for marker in doomed {
            tree.detach(marker)?;
        }
    }
    let part = match ctx.txn.part_related_by(&ctx.main, rt::COMMENTS) {
        Ok(Some(part)) => part,
        Ok(None) => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut paragraph_ids: Vec<String> = Vec::new();
    let doomed: Vec<NodeId> = {
        let tree = ctx.tree(&part)?;
        let mut doomed = Vec::new();
        for comment in tree.child_elements(tree.root()).collect::<Vec<_>>() {
            if !comment_ids.contains(&comment_id(tree, comment)?) {
                continue;
            }
            if let Some(last) = crate::commentops::last_paragraph(tree, comment)
                && let Some(para_id) = tree.attr(last, Some(crate::wml::W14), "paraId")
                && !paragraph_ids.iter().any(|p| p == para_id)
            {
                paragraph_ids.push(para_id.to_string());
            }
            doomed.push(comment);
        }
        doomed
    };
    for comment in doomed {
        discard_element(ctx, &NodeRef::new(&part, comment))?;
    }
    cleanup_comments_extended(ctx, &paragraph_ids)?;
    crate::commentops::remove_comment_identity_rows(
        &mut ctx.txn,
        &ctx.main.clone(),
        &paragraph_ids,
    )?;
    Ok(())
}

/// Upstream `_discard_element`: detach a subtree and prune what only it used.
fn discard_element(ctx: &mut Ctx<'_, '_>, node: &NodeRef) -> Result<()> {
    if !ctx.attached(node)? {
        return Err(unsupported(
            "detached_revision_subtree",
            "cannot discard a detached revision subtree; nothing was changed",
        ));
    }
    let (comment_ids, footnotes, endnotes, relationship_ids) = {
        let tree = ctx.tree(&node.part)?;
        let available = part_rel_ids(ctx, &node.part)?;
        let rels: Vec<String> = relationship_values(tree, node.id)
            .into_iter()
            .filter(|value| available.contains(value))
            .collect();
        (
            comment_ids_inside(tree, node.id)?,
            note_ids_inside(tree, node.id, "footnoteReference")?,
            note_ids_inside(tree, node.id, "endnoteReference")?,
            rels,
        )
    };
    ctx.tree_mut(&node.part)?.detach(node.id)?;
    drop_unreferenced_relationships(ctx, &node.part, &relationship_ids)?;
    cleanup_note_bodies(ctx, &footnotes, &endnotes)?;
    cleanup_comment_anchors(ctx, &comment_ids)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolution primitives
// ---------------------------------------------------------------------------

fn unwrap(tree: &mut XmlTree, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(node)
        .ok_or_else(|| Error::internal("detached_wrapper", "unwrap of a detached wrapper"))?;
    let start = tree.index_in_parent(node).unwrap_or(0);
    let children: Vec<NodeId> = tree.children(node).to_vec();
    for (index, child) in (start..).zip(children) {
        tree.detach(child)?;
        tree.insert(parent, index, child)?;
    }
    tree.detach(node)
}

/// lxml `element.tag = w:local` (rebuilt in place: same position, attributes
/// and children).
fn retag(tree: &mut XmlTree, node: NodeId, local: &str) -> Result<()> {
    let parent = tree
        .parent(node)
        .ok_or_else(|| Error::internal("detached_retag", "retag of a detached element"))?;
    let index = tree.index_in_parent(node).unwrap_or(0);
    let attrs = tree
        .element(node)
        .map(|e| e.attrs.clone())
        .unwrap_or_default();
    let replacement = tree.create_element(node, W, local)?;
    tree.insert(parent, index, replacement)?;
    for attr in attrs {
        let ns = attr.ns.map(|ns| tree.uri(ns).to_string());
        tree.set_attr(replacement, ns.as_deref(), &attr.name.local, &attr.value)?;
    }
    let children: Vec<NodeId> = tree.children(node).to_vec();
    for child in children {
        tree.detach(child)?;
        let end = tree.children(replacement).len();
        tree.insert(replacement, end, child)?;
    }
    tree.detach(node)
}

fn resolve_deletion_reject(tree: &mut XmlTree, node: NodeId) -> Result<()> {
    let del_texts: Vec<NodeId> = iter_w(tree, node, &["delText"]).collect();
    for text in del_texts {
        retag(tree, text, "t")?;
    }
    let del_instr: Vec<NodeId> = iter_w(tree, node, &["delInstrText"]).collect();
    for text in del_instr {
        retag(tree, text, "instrText")?;
    }
    unwrap(tree, node)
}

fn resolve_format_change(tree: &mut XmlTree, node: NodeId, accept: bool) -> Result<()> {
    let parent = tree
        .parent(node)
        .ok_or_else(|| Error::internal("detached_change", "format change is detached"))?;
    if accept {
        tree.detach(node)?;
        if lxml_len(tree, parent) == 0 {
            tree.detach(parent)?;
        }
        return Ok(());
    }
    let is_run_change = is_tag(tree, node, "rPrChange");
    let stored = tree.first_child_named(node, W, if is_run_change { "rPr" } else { "pPr" });
    let kept: Vec<NodeId> = node_children(tree, parent)
        .filter(|child| {
            let local = w_local(tree, *child);
            if is_run_change {
                matches!(local, Some("ins" | "del" | "moveFrom" | "moveTo"))
            } else {
                matches!(local, Some("rPr" | "sectPr"))
            }
        })
        .collect();
    let restored: Vec<NodeId> = match stored {
        Some(stored) => node_children(tree, stored)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|child| tree.deep_clone(child))
            .collect(),
        None => Vec::new(),
    };
    let ordered: Vec<NodeId> = if is_run_change {
        kept.iter().chain(restored.iter()).copied().collect()
    } else {
        restored.iter().chain(kept.iter()).copied().collect()
    };
    for child in tree.children(parent).to_vec() {
        if child != node {
            tree.detach(child)?;
        }
    }
    for child in ordered {
        let end = tree.children(parent).len();
        tree.insert(parent, end, child)?;
    }
    tree.detach(node)?;
    if lxml_len(tree, parent) == 0 {
        tree.detach(parent)?;
    }
    Ok(())
}

fn resolve_paragraph_mark(tree: &mut XmlTree, node: NodeId, accept: bool) -> Result<()> {
    let paragraph = paragraph_of_mark(tree, node);
    let applying = paragraph_mark_applies(tree, node, accept);
    let mut following = None;
    if let Some(paragraph) = paragraph
        && applying
        && paragraph_has_content(tree, paragraph)
    {
        following = next_paragraph_sibling(tree, paragraph)?;
    }
    let r_pr = tree
        .parent(node)
        .ok_or_else(|| Error::internal("detached_mark", "paragraph mark is detached"))?;
    tree.detach(node)?;
    if lxml_len(tree, r_pr) == 0 {
        tree.detach(r_pr)?;
    }
    let Some(paragraph) = paragraph else {
        return Ok(());
    };
    if !applying {
        return Ok(());
    }
    if paragraph_has_content(tree, paragraph) {
        let Some(following) = following else {
            return Ok(());
        };
        let insert_at = usize::from(tree.first_child_named(following, W, "pPr").is_some());
        let content: Vec<NodeId> = tree
            .children(paragraph)
            .iter()
            .copied()
            .filter(|child| !is_tag(tree, *child, "pPr"))
            .collect();
        let start = raw_index(tree, following, insert_at);
        for (index, child) in (start..).zip(content) {
            tree.detach(child)?;
            tree.insert(following, index, child)?;
        }
        tree.detach(paragraph)?;
        return Ok(());
    }
    if !is_last_block_in_container(tree, paragraph) && tree.parent(paragraph).is_some() {
        tree.detach(paragraph)?;
    }
    Ok(())
}

fn resolve_row_revision(ctx: &mut Ctx<'_, '_>, node: &NodeRef, accept: bool) -> Result<()> {
    let (tr_pr, row, keeps_row) = {
        let tree = ctx.tree(&node.part)?;
        let tr_pr = tree
            .parent(node.id)
            .ok_or_else(|| Error::internal("detached_row_marker", "row marker is detached"))?;
        let row = tree
            .parent(tr_pr)
            .ok_or_else(|| Error::internal("detached_row", "row properties are detached"))?;
        let keeps =
            (is_tag(tree, node.id, "ins") && accept) || (is_tag(tree, node.id, "del") && !accept);
        (tr_pr, row, keeps)
    };
    if keeps_row {
        let tree = ctx.tree_mut(&node.part)?;
        tree.detach(node.id)?;
        if lxml_len(tree, tr_pr) == 0 {
            tree.detach(tr_pr)?;
        }
        return Ok(());
    }
    let table = ctx
        .tree(&node.part)?
        .parent(row)
        .ok_or_else(|| Error::internal("detached_table", "row is detached"))?;
    let only_row = {
        let tree = ctx.tree(&node.part)?;
        tree.child_elements(table)
            .filter(|c| is_tag(tree, *c, "tr"))
            .count()
            == 1
    };
    if only_row {
        let tree = ctx.tree_mut(&node.part)?;
        let parent = tree
            .parent(table)
            .ok_or_else(|| Error::internal("detached_table", "table is detached"))?;
        let siblings = tree
            .child_elements(parent)
            .any(|c| c != table && (is_tag(tree, c, "p") || is_tag(tree, c, "tbl")));
        if !siblings {
            let paragraph = crate::oxml::w_el(tree, parent, "p")?;
            let index = tree.index_in_parent(table).unwrap_or(0);
            tree.insert(parent, index, paragraph)?;
        }
        return discard_element(ctx, &NodeRef::new(&node.part, table));
    }
    discard_element(ctx, &NodeRef::new(&node.part, row))
}

fn resolve_move(ctx: &mut Ctx<'_, '_>, node: &NodeRef, accept: bool) -> Result<()> {
    let (units, orphans) = move_units(&ctx.txn, ctx.stories)?;
    if let Some(unit) = units.into_iter().find(|unit| unit.contains(node)) {
        return resolve_move_unit(ctx, &unit, accept);
    }
    if let Some((_, reason)) = orphans.iter().find(|(element, _)| element == node) {
        return Err(unsupported(
            "unresolvable_move",
            format!(
                "cannot resolve this tracked move: {reason}; nothing was changed. Resolve it in Word instead"
            ),
        ));
    }
    Err(unsupported(
        "unresolvable_move",
        "cannot resolve this tracked move: its range markers were not found; nothing was changed. Resolve it in \
         Word instead",
    ))
}

fn resolve_move_unit(ctx: &mut Ctx<'_, '_>, unit: &MoveUnit, accept: bool) -> Result<()> {
    let (keep, drop) = if accept {
        (&unit.to_site, &unit.from_site)
    } else {
        (&unit.from_site, &unit.to_site)
    };
    for wrapper in &keep.wrappers {
        if ctx.attached(wrapper)? {
            unwrap(ctx.tree_mut(&wrapper.part)?, wrapper.id)?;
        }
    }
    for wrapper in &drop.wrappers {
        if ctx.attached(wrapper)? {
            discard_element(ctx, wrapper)?;
        }
    }
    for site in [&unit.from_site, &unit.to_site] {
        for marker in std::iter::once(&site.start).chain(site.end.iter()) {
            if ctx.attached(marker)? {
                ctx.tree_mut(&marker.part)?.detach(marker.id)?;
            }
        }
    }
    for site in [&unit.from_site, &unit.to_site] {
        for stamp in &site.mark_stamps {
            if ctx.attached(stamp)? {
                resolve_paragraph_mark(ctx.tree_mut(&stamp.part)?, stamp.id, accept)?;
            }
        }
    }
    Ok(())
}

fn stale_error() -> Error {
    unsupported(
        "stale_revision",
        "cannot resolve a stale Revision; reacquire it from document.revisions. Nothing was changed",
    )
}

/// Upstream `_resolve_one`.
fn resolve_one(ctx: &mut Ctx<'_, '_>, node: &NodeRef, accept: bool) -> Result<()> {
    if !ctx.attached(node)? || !ctx.is_in_story(node)? {
        return Err(stale_error());
    }
    let (revision_type, mark, tag) = {
        let tree = ctx.tree(&node.part)?;
        (
            revision_type_of(tree, node.id),
            is_paragraph_mark(tree, node.id),
            w_local(tree, node.id).unwrap_or("").to_string(),
        )
    };
    match revision_type {
        "move_from" | "move_to" => return resolve_move(ctx, node, accept),
        "format_change" => {
            return resolve_format_change(ctx.tree_mut(&node.part)?, node.id, accept);
        }
        "row_insertion" | "row_deletion" => return resolve_row_revision(ctx, node, accept),
        _ => {}
    }
    if mark {
        return resolve_paragraph_mark(ctx.tree_mut(&node.part)?, node.id, accept);
    }
    let removes_content = (tag == "ins" && !accept) || (tag == "del" && accept);
    if removes_content {
        return discard_element(ctx, node);
    }
    let tree = ctx.tree_mut(&node.part)?;
    if tag == "ins" {
        unwrap(tree, node.id)
    } else {
        resolve_deletion_reject(tree, node.id)
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

fn foreign(what: &str) -> Error {
    Error::refusal(
        RefusalKind::BoundaryViolation,
        "foreign_revision",
        format!(
            "{what} belongs to a different document; reacquire document.revisions from the document passed"
        ),
    )
}

fn resolve_individual(document: &mut Document, revision: &Revision, accept: bool) -> Result<()> {
    let verb = if accept { "accept" } else { "reject" };
    revision.refuse_unresolvable(verb)?;
    if revision.document != document.id() {
        return Err(foreign("revision"));
    }
    let stories = story::stories(document);
    let staged = {
        let mut ctx = Ctx::new(document, &stories);
        let current = signature_of(&ctx.txn, &stories, ctx.document)?;
        if *revision.signature != current || !ctx.is_in_story(&revision.node)? {
            return Err(unsupported(
                "stale_revision",
                format!(
                    "cannot {verb} a stale Revision; reacquire it from document.revisions. Nothing was changed"
                ),
            ));
        }
        refuse_if_protected(document, "resolve a revision", OperationClass::BodyContent)?;
        let selected = [Selected {
            node: revision.node.clone(),
            revision_type: revision.revision_type,
            is_paragraph_mark: revision.is_paragraph_mark,
        }];
        preflight(&ctx, &selected, accept, None, false, true)?;
        resolve_one(&mut ctx, &revision.node, accept)?;
        ctx.txn.finish()?
    };
    document.commit(staged)
}

fn resolve_all(
    document: &mut Document,
    revisions: &mut Revisions,
    accept: bool,
    author: Option<&str>,
) -> Result<usize> {
    if revisions.document != document.id() {
        return Err(foreign("revisions snapshot"));
    }
    let stories = story::stories(document);
    let count;
    let staged = {
        let mut ctx = Ctx::new(document, &stories);
        let current = signature_of(&ctx.txn, &stories, ctx.document)?;
        if *revisions.signature != current {
            return Err(unsupported(
                "stale_revisions",
                "cannot resolve a stale Revisions snapshot; reacquire document.revisions. Nothing was changed",
            ));
        }
        refuse_if_protected(document, "resolve revisions", OperationClass::BodyContent)?;
        let chosen: Vec<&Revision> = revisions
            .items
            .iter()
            .filter(|r| author.is_none_or(|a| r.author == a))
            .collect();
        let mut unresolvable: Vec<String> = chosen
            .iter()
            .filter(|r| !r.is_resolvable())
            .map(|r| r.revision_type.to_string())
            .collect();
        unresolvable.sort();
        unresolvable.dedup();
        if !unresolvable.is_empty() {
            return Err(unsupported(
                "unresolvable_revision_type",
                format!(
                    "selected revisions include {} which this package can enumerate but not resolve; nothing was \
                     changed. Filter by author, resolve individual insertions/deletions, or resolve the rest in Word",
                    py_list(&unresolvable)
                ),
            ));
        }
        let selected: Vec<Selected> = chosen
            .iter()
            .map(|r| Selected {
                node: r.node.clone(),
                revision_type: r.revision_type,
                is_paragraph_mark: r.is_paragraph_mark,
            })
            .collect();
        preflight(&ctx, &selected, accept, author, author.is_none(), false)?;
        let mut ordered: Vec<&Selected> = selected.iter().collect();
        ordered.sort_by_key(|s| {
            if s.is_move() {
                0
            } else if s.is_paragraph_mark {
                2
            } else {
                1
            }
        });
        let mut handled: HashSet<NodeRef> = HashSet::new();
        for revision in ordered {
            if handled.contains(&revision.node) {
                continue;
            }
            if !ctx.is_in_story(&revision.node)? {
                handled.insert(revision.node.clone());
                continue;
            }
            resolve_one(&mut ctx, &revision.node, accept)?;
            handled.insert(revision.node.clone());
        }
        count = selected.len();
        ctx.txn.finish()?
    };
    document.commit(staged)?;
    *revisions = snapshot(document)?;
    Ok(count)
}

/// Map from a revision's element to its index in a snapshot (test support).
#[doc(hidden)]
pub fn revision_index(revisions: &Revisions) -> HashMap<(String, u32), usize> {
    revisions
        .items
        .iter()
        .enumerate()
        .map(|(index, r)| ((r.node.part.clone(), r.node.id.index()), index))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isoformat_round_trips_python_forms() {
        let cases = [
            (
                "2024-01-02T03:04:05+00:00",
                Some("2024-01-02T03:04:05+00:00"),
            ),
            ("2024-01-02T03:04:05", Some("2024-01-02T03:04:05")),
            ("2024-01-02", Some("2024-01-02T00:00:00")),
            (
                "2024-01-02T03:04:05.5+05:30",
                Some("2024-01-02T03:04:05.500000+05:30"),
            ),
            ("2024-01-02 03:04", Some("2024-01-02T03:04:00")),
            ("20240102T030405", Some("2024-01-02T03:04:05")),
            ("2024-02-30T00:00:00", None),
            ("garbage", None),
            ("2024-01-02T25:00:00", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                RevisionDate::from_isoformat(input)
                    .map(|d| d.isoformat())
                    .as_deref(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn python_reprs() {
        assert_eq!(py_list(&["a".into(), "it's".into()]), "['a', \"it's\"]");
        assert_eq!(py_int(" 01 "), Some(1));
        assert_eq!(py_int("x"), None);
    }
}
