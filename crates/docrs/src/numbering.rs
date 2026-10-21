//! Numbering enumeration, application and minimal authoring (upstream
//! `numbering.py`, MIT; see `NOTICE.md`).
//!
//! Applies *existing* numbering definitions and list styles to paragraphs and
//! reports what a document defines and uses. Exactly two canonical
//! definitions — one bullet, one decimal — can be created on demand
//! ([`ensure_bullet_definition`] / [`ensure_decimal_definition`], idempotent),
//! plus level-0 restarts ([`restart_numbering`]). Everything else refuses.
//!
//! Every mutation validates first and stages its writes in a [`Txn`]; a
//! refusal leaves the document unchanged.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use crate::anchor::{BlockTarget, locate_paragraph, require_current_view};
use crate::oxml::{PPR_SEQ, get_or_add, get_or_add_ppr, set_w};
use crate::package::ct;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::story::{self, BlockKind, View, iter_block_elements, subtree_text};
use crate::txn::Txn;
use crate::unicode::{is_py_space, py_strip};
use crate::wml::{W, first_choice_children, is_w, leading_text, w_attr, w_child, w_children};
use crate::xml::{self, NodeData, NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// One level of a numbering definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NumberingLevel {
    /// `w:ilvl`.
    pub level: i64,
    /// `w:numFmt/@w:val`.
    pub num_fmt: Option<String>,
    /// `w:lvlText/@w:val`.
    pub lvl_text: Option<String>,
}

/// A `w:num` with its effective levels (abstract levels plus overrides).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NumberingDefinition {
    /// `w:numId`.
    pub num_id: i64,
    /// Referenced `w:abstractNumId` (`-1` when absent).
    pub abstract_num_id: i64,
    /// Levels sorted by level number.
    pub levels: Vec<NumberingLevel>,
}

/// A paragraph whose effective numbering resolves to a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NumberedParagraph {
    /// Story name.
    pub story: String,
    /// Block index of the paragraph (or of its table).
    pub index: usize,
    /// Effective numId.
    pub num_id: i64,
    /// Effective level.
    pub level: i64,
    /// Current-view text.
    pub text: String,
    /// `(row, grid column, paragraph-in-cell)` for table-cell paragraphs.
    pub table_cell: Option<(usize, i64, usize)>,
}

/// Upstream `NumberingReport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NumberingReport {
    /// Definitions sorted by numId.
    pub definitions: Vec<NumberingDefinition>,
    /// Numbered paragraphs in story order.
    pub numbered_paragraphs: Vec<NumberedParagraph>,
}

impl NumberingReport {
    /// Upstream `to_dict()` (`schema: paper_numbering`, `version: 2`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_numbering",
            "version": 2,
            "definitions": self.definitions,
            "numbered_paragraphs": self.numbered_paragraphs,
        })
    }
}

// ---------------------------------------------------------------------------
// Small Python-compatible helpers shared by the P2 modules
// ---------------------------------------------------------------------------

/// Python `int(text)` for base-10 strings (surrounding whitespace, a sign,
/// single underscores between digits).
pub(crate) fn py_int(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches(is_py_space);
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let mut value: i64 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let digit = c.to_digit(10)?;
        value = value.checked_mul(10)?.checked_add(digit as i64)?;
    }
    Some(if negative { -value } else { value })
}

fn malformed_int(what: &str, value: Option<&str>) -> Error {
    Error::invalid_argument(
        "malformed_integer",
        match value {
            Some(value) => format!("{what} {value:?} is not an integer"),
            None => format!("{what} is missing"),
        },
    )
}

/// `int(value)` with upstream's raw `TypeError`/`ValueError` mapped to
/// `invalid_argument`.
pub(crate) fn require_int(value: Option<&str>, what: &str) -> Result<i64> {
    value
        .and_then(py_int)
        .ok_or_else(|| malformed_int(what, value))
}

/// Non-text child nodes (lxml's element/comment/PI children).
pub(crate) fn lxml_children(tree: &XmlTree, id: NodeId) -> Vec<NodeId> {
    tree.children(id)
        .iter()
        .copied()
        .filter(|child| !matches!(tree.data(*child), NodeData::Text(_)))
        .collect()
}

/// lxml `anchor.addnext(node)`: `node` goes after `anchor` *and its tail*.
pub(crate) fn add_next(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("add_next", "anchor is detached"))?;
    let children = tree.children(parent);
    let mut index = tree
        .index_in_parent(anchor)
        .ok_or_else(|| Error::internal("add_next", "anchor not found"))?
        + 1;
    while index < children.len() && matches!(tree.data(children[index]), NodeData::Text(_)) {
        index += 1;
    }
    tree.insert(parent, index, node)
}

/// lxml `anchor.addprevious(node)`.
pub(crate) fn add_previous(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("add_previous", "anchor is detached"))?;
    let index = tree
        .index_in_parent(anchor)
        .ok_or_else(|| Error::internal("add_previous", "anchor not found"))?;
    tree.insert(parent, index, node)
}

/// Copy the element tree of `fragment` (well-formed XML) into `tree` as new
/// detached nodes scoped for insertion under `scope` (python-docx
/// `parse_xml` of a template). Namespace prefixes come from the destination
/// scope; whitespace-only text between elements is dropped like python-docx
/// `remove_blank_text`.
pub(crate) fn import_fragment(tree: &mut XmlTree, scope: NodeId, fragment: &str) -> Result<NodeId> {
    let source = xml::parse_str(fragment)
        .map_err(|err| Error::internal("fragment_template", err.to_string()))?;
    import_node(tree, scope, &source, source.root())
}

fn import_node(
    tree: &mut XmlTree,
    scope: NodeId,
    source: &XmlTree,
    node: NodeId,
) -> Result<NodeId> {
    let ns = source.ns_uri(node).unwrap_or("").to_string();
    let local = source.local_name(node).unwrap_or("").to_string();
    let element = tree.create_element(scope, &ns, &local)?;
    let attrs: Vec<(Option<String>, String, String)> = source
        .element(node)
        .map(|data| {
            data.attrs
                .iter()
                .map(|attr| {
                    (
                        attr.ns.map(|ns| source.uri(ns).to_string()),
                        attr.name.local.to_string(),
                        attr.value.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for (ns, local, value) in attrs {
        tree.set_attr(element, ns.as_deref(), &local, &value)?;
    }
    let has_elements = source.child_elements(node).next().is_some();
    for child in source.children(node).to_vec() {
        match source.data(child) {
            NodeData::Element(_) => {
                let copy = import_node(tree, element, source, child)?;
                crate::oxml::append(tree, element, copy)?;
            }
            NodeData::Text(text) => {
                if has_elements && py_strip(text).is_empty() {
                    continue;
                }
                let copy = tree.create_text(text.clone());
                crate::oxml::append(tree, element, copy)?;
            }
            _ => {}
        }
    }
    Ok(element)
}

// ---------------------------------------------------------------------------
// Styles (read straight from the styles part; python-docx semantics)
// ---------------------------------------------------------------------------

const DEFAULT_STYLES: &[u8] = include_bytes!("../assets/default-styles.xml");

/// python-docx `BabelFish` UI ↔ internal style-name aliases.
const STYLE_ALIASES: &[(&str, &str)] = &[
    ("Caption", "caption"),
    ("Footer", "footer"),
    ("Header", "header"),
    ("Heading 1", "heading 1"),
    ("Heading 2", "heading 2"),
    ("Heading 3", "heading 3"),
    ("Heading 4", "heading 4"),
    ("Heading 5", "heading 5"),
    ("Heading 6", "heading 6"),
    ("Heading 7", "heading 7"),
    ("Heading 8", "heading 8"),
    ("Heading 9", "heading 9"),
];

fn ui2internal(name: &str) -> &str {
    STYLE_ALIASES
        .iter()
        .find(|(ui, _)| *ui == name)
        .map(|(_, internal)| *internal)
        .unwrap_or(name)
}

fn internal2ui(name: &str) -> &str {
    STYLE_ALIASES
        .iter()
        .find(|(_, internal)| *internal == name)
        .map(|(ui, _)| *ui)
        .unwrap_or(name)
}

/// The styles part as python-docx `document.styles` sees it: the related
/// part, or (when missing) the default template python-docx would create.
pub(crate) struct StylesView<'a> {
    pub(crate) tree: Cow<'a, XmlTree>,
    /// The package has no styles part (upstream would create one on access).
    pub(crate) missing: bool,
}

impl<'a> StylesView<'a> {
    /// Styles as seen through `txn` (staged parts included).
    pub(crate) fn from_txn(txn: &'a Txn<'_>) -> Result<StylesView<'a>> {
        let main = txn.document().package().main_part_name().to_string();
        match txn.part_related_by(&main, rt::STYLES)? {
            Some(part) => Ok(StylesView {
                tree: Cow::Borrowed(txn.tree(&part)?),
                missing: false,
            }),
            None => Ok(StylesView {
                tree: Cow::Owned(crate::parts::template(DEFAULT_STYLES)?),
                missing: true,
            }),
        }
    }

    /// Styles of a document (read-only).
    pub(crate) fn from_document(document: &'a Document) -> Result<StylesView<'a>> {
        let package = document.package();
        match package.part_related_by(package.main_part_name(), rt::STYLES)? {
            Some(part) => Ok(StylesView {
                tree: Cow::Borrowed(package.xml(&part)?),
                missing: false,
            }),
            None => Ok(StylesView {
                tree: Cow::Owned(crate::parts::template(DEFAULT_STYLES)?),
                missing: true,
            }),
        }
    }

    /// Every `w:style` child of the styles root.
    pub(crate) fn styles(&self) -> Vec<NodeId> {
        let tree: &XmlTree = &self.tree;
        w_children(tree, tree.root(), "style").collect()
    }

    /// `w:styleId` values (upstream `{style.style_id for style in styles}`).
    pub(crate) fn style_ids(&self) -> HashSet<String> {
        self.styles()
            .into_iter()
            .filter_map(|style| w_attr(&self.tree, style, "styleId").map(str::to_string))
            .collect()
    }

    fn name_val(&self, style: NodeId) -> Option<&str> {
        w_child(&self.tree, style, "name").and_then(|name| w_attr(&self.tree, name, "val"))
    }

    /// `CT_Styles.get_by_id` (first match).
    fn get_by_id(&self, style_id: &str) -> Option<NodeId> {
        self.styles()
            .into_iter()
            .find(|style| w_attr(&self.tree, *style, "styleId") == Some(style_id))
    }

    /// `CT_Styles.get_by_name`: any `w:name` child with the value.
    fn get_by_name(&self, name: &str) -> Option<NodeId> {
        self.styles().into_iter().find(|style| {
            w_children(&self.tree, *style, "name")
                .any(|child| w_attr(&self.tree, child, "val") == Some(name))
        })
    }

    /// python-docx `styles[key]` (UI name, then deprecated style-id lookup).
    fn lookup(&self, key: &str) -> Option<NodeId> {
        self.get_by_name(ui2internal(key))
            .or_else(|| self.get_by_id(key))
    }

    /// UI names of every style (upstream `{style.name for style in styles}`).
    fn ui_names(&self) -> HashSet<String> {
        self.styles()
            .into_iter()
            .filter_map(|style| {
                self.name_val(style)
                    .map(|name| internal2ui(name).to_string())
            })
            .collect()
    }

    /// `CT_Style.type` parsed (`None` when the attribute is absent).
    fn raw_type(&self, style: NodeId) -> Result<Option<&str>> {
        match w_attr(&self.tree, style, "type") {
            None => Ok(None),
            Some(value @ ("paragraph" | "character" | "table" | "numbering")) => Ok(Some(value)),
            Some(other) => Err(Error::invalid_argument(
                "malformed_style_type",
                format!("WD_STYLE_TYPE has no XML mapping for '{other}'"),
            )),
        }
    }

    /// `CT_Styles.default_for(PARAGRAPH)`: the last explicit paragraph style
    /// with `w:default` on.
    fn default_paragraph_style(&self) -> Result<Option<NodeId>> {
        let mut found = None;
        for style in self.styles() {
            if self.raw_type(style)? != Some("paragraph") {
                continue;
            }
            match w_attr(&self.tree, style, "default") {
                None | Some("0" | "false" | "off") => {}
                Some("1" | "true" | "on") => found = Some(style),
                Some(other) => {
                    return Err(Error::invalid_argument(
                        "malformed_on_off",
                        format!(
                            "value must be one of '1', '0', 'true', 'false', 'on', or 'off', got '{other}'"
                        ),
                    ));
                }
            }
        }
        Ok(found)
    }

    /// python-docx `get_style_id(style_name, PARAGRAPH)`.
    fn paragraph_style_id(&self, style_name: &str) -> Result<Option<String>> {
        let style = self.lookup(style_name).ok_or_else(|| {
            Error::refusal(
                RefusalKind::TargetNotFound,
                "style_not_defined",
                format!("no style with name '{style_name}'"),
            )
        })?;
        let kind = self.raw_type(style)?.unwrap_or("paragraph");
        if kind != "paragraph" {
            return Err(Error::invalid_argument(
                "style_type_mismatch",
                format!("assigned style is type {kind}, need type paragraph"),
            ));
        }
        if self.default_paragraph_style()? == Some(style) {
            return Ok(None);
        }
        Ok(w_attr(&self.tree, style, "styleId").map(str::to_string))
    }
}

/// Upstream `blocks._validated_style_id`.
pub(crate) fn validated_style_id(
    styles: &StylesView<'_>,
    style_id: Option<&str>,
    argument: &str,
) -> Result<Option<String>> {
    let Some(style_id) = style_id else {
        return Ok(None);
    };
    if !styles.style_ids().contains(style_id) {
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "style_not_defined",
            format!("{argument} {style_id:?} is not defined in this document's styles"),
        ));
    }
    Ok(Some(style_id.to_string()))
}

/// `CT_P.style = value` exactly as python-docx does it: reuse the first
/// `w:pStyle` (or add one in schema order); `None` removes every `w:pStyle`.
pub(crate) fn set_p_style(
    tree: &mut XmlTree,
    paragraph: NodeId,
    style: Option<&str>,
) -> Result<()> {
    let ppr = get_or_add_ppr(tree, paragraph)?;
    match style {
        None => crate::oxml::remove_w_children(tree, ppr, "pStyle"),
        Some(style) => {
            let p_style = get_or_add(tree, ppr, "pStyle", PPR_SEQ)?;
            set_w(tree, p_style, "val", style)
        }
    }
}

// ---------------------------------------------------------------------------
// Definitions and the report
// ---------------------------------------------------------------------------

fn level_of(tree: &XmlTree, lvl: NodeId, level: i64) -> NumberingLevel {
    let value = |local: &str| {
        w_child(tree, lvl, local)
            .and_then(|child| w_attr(tree, child, "val"))
            .map(str::to_string)
    };
    NumberingLevel {
        level,
        num_fmt: value("numFmt"),
        lvl_text: value("lvlText"),
    }
}

/// Upstream `_definitions`.
pub(crate) fn definitions(numbering: Option<&XmlTree>) -> Result<Vec<NumberingDefinition>> {
    let Some(tree) = numbering else {
        return Ok(Vec::new());
    };
    let root = tree.root();
    let mut abstract_levels: HashMap<i64, Vec<NumberingLevel>> = HashMap::new();
    for abstract_num in w_children(tree, root, "abstractNum") {
        let abstract_id = require_int(
            w_attr(tree, abstract_num, "abstractNumId"),
            "w:abstractNumId",
        )?;
        let mut levels = Vec::new();
        for lvl in w_children(tree, abstract_num, "lvl") {
            let level = require_int(w_attr(tree, lvl, "ilvl"), "w:ilvl")?;
            levels.push(level_of(tree, lvl, level));
        }
        abstract_levels.insert(abstract_id, levels);
    }
    let mut out = Vec::new();
    for num in w_children(tree, root, "num") {
        let num_id = require_int(w_attr(tree, num, "numId"), "w:numId")?;
        let abstract_id = match w_child(tree, num, "abstractNumId") {
            Some(reference) => {
                require_int(w_attr(tree, reference, "val"), "w:abstractNumId/@w:val")?
            }
            None => -1,
        };
        let mut levels: BTreeMap<i64, NumberingLevel> = BTreeMap::new();
        for level in abstract_levels.get(&abstract_id).into_iter().flatten() {
            levels.insert(level.level, level.clone());
        }
        for level_override in w_children(tree, num, "lvlOverride") {
            let override_level = require_int(
                w_attr(tree, level_override, "ilvl"),
                "w:lvlOverride/@w:ilvl",
            )?;
            let Some(lvl) = w_child(tree, level_override, "lvl") else {
                continue;
            };
            levels.insert(override_level, level_of(tree, lvl, override_level));
        }
        out.push(NumberingDefinition {
            num_id,
            abstract_num_id: abstract_id,
            levels: levels.into_values().collect(),
        });
    }
    out.sort_by_key(|definition| definition.num_id);
    Ok(out)
}

fn live_numbering_tree(document: &Document) -> Result<Option<&XmlTree>> {
    let package = document.package();
    match package.part_related_by(package.main_part_name(), rt::NUMBERING)? {
        Some(part) => Ok(Some(package.xml(&part)?)),
        None => Ok(None),
    }
}

struct StyleIndex {
    by_id: HashMap<String, NodeId>,
    default_id: Option<String>,
}

fn style_index(styles: &StylesView<'_>) -> StyleIndex {
    let tree: &XmlTree = &styles.tree;
    let mut by_id = HashMap::new();
    let mut default_id = None;
    for style in styles.styles() {
        if let Some(id) = w_attr(tree, style, "styleId") {
            by_id.insert(id.to_string(), style);
        }
        if w_attr(tree, style, "type") != Some("paragraph") {
            continue;
        }
        let on = w_attr(tree, style, "default")
            .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "on"))
            .unwrap_or(false);
        if on {
            default_id = w_attr(tree, style, "styleId").map(str::to_string);
        }
    }
    StyleIndex { by_id, default_id }
}

/// Upstream `_effective_paragraph_numbering`: `(numId, ilvl)` after
/// paragraph-style inheritance, `None` for unnumbered or numId 0.
fn effective_numbering(
    styles: &StylesView<'_>,
    index: &StyleIndex,
    tree: &XmlTree,
    paragraph: NodeId,
) -> Result<Option<(i64, i64)>> {
    let p_pr = w_child(tree, paragraph, "pPr");
    let style_id = match p_pr.and_then(|ppr| w_child(tree, ppr, "pStyle")) {
        Some(style) => w_attr(tree, style, "val").map(str::to_string),
        None => index.default_id.clone(),
    };
    let style_tree: &XmlTree = &styles.tree;
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = style_id;
    while let Some(id) = current {
        let Some(style) = index.by_id.get(&id).copied() else {
            break;
        };
        if !seen.insert(id) {
            break;
        }
        chain.push(style);
        current = w_child(style_tree, style, "basedOn")
            .and_then(|based| w_attr(style_tree, based, "val"))
            .map(str::to_string);
    }
    chain.reverse();
    let mut num_id = None;
    let mut level = None;
    let mut layers: Vec<(&XmlTree, Option<NodeId>)> = chain
        .iter()
        .map(|style| (style_tree, w_child(style_tree, *style, "pPr")))
        .collect();
    layers.push((tree, p_pr));
    for (layer_tree, layer) in layers {
        let Some(num_pr) = layer.and_then(|layer| w_child(layer_tree, layer, "numPr")) else {
            continue;
        };
        if let Some(value) =
            w_child(layer_tree, num_pr, "numId").and_then(|e| w_attr(layer_tree, e, "val"))
        {
            num_id = Some(require_int(Some(value), "w:numId/@w:val")?);
        }
        if let Some(value) =
            w_child(layer_tree, num_pr, "ilvl").and_then(|e| w_attr(layer_tree, e, "val"))
        {
            level = Some(require_int(Some(value), "w:ilvl/@w:val")?);
        }
    }
    match num_id {
        None | Some(0) => Ok(None),
        Some(num_id) => Ok(Some((num_id, level.unwrap_or(0)))),
    }
}

/// Upstream `_selected_descendants`.
fn selected_descendants(tree: &XmlTree, root: NodeId, target: &str, out: &mut Vec<NodeId>) {
    for child in first_choice_children(tree, root) {
        if is_w(tree, child, target) {
            out.push(child);
        }
        if !is_w(tree, child, "tbl") {
            selected_descendants(tree, child, target, out);
        }
    }
}

fn selected(tree: &XmlTree, root: NodeId, target: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    selected_descendants(tree, root, target, &mut out);
    out
}

fn table_paragraphs(tree: &XmlTree, table: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    for row in selected(tree, table, "tr") {
        for cell in selected(tree, row, "tc") {
            out.extend(selected(tree, cell, "p"));
        }
    }
    out
}

fn climb_to(tree: &XmlTree, start: Option<NodeId>, target: &str) -> Option<NodeId> {
    let mut current = start;
    while let Some(node) = current {
        if is_w(tree, node, target) || is_w(tree, node, "body") {
            break;
        }
        current = tree.parent(node);
    }
    current.filter(|node| is_w(tree, *node, target))
}

/// Upstream `_table_cell_address`.
fn table_cell_address(tree: &XmlTree, paragraph: NodeId) -> Result<Option<(usize, i64, usize)>> {
    let Some(tc) = climb_to(tree, tree.parent(paragraph), "tc") else {
        return Ok(None);
    };
    let Some(tr) = climb_to(tree, tree.parent(tc), "tr") else {
        return Ok(None);
    };
    let Some(tbl) = climb_to(tree, tree.parent(tr), "tbl") else {
        return Ok(None);
    };
    let unreachable = || {
        Error::invalid_argument(
            "unreachable_table_cell",
            "numbered paragraph is not reachable through its table's selected content",
        )
    };
    let rows = selected(tree, tbl, "tr");
    let cells = selected(tree, tr, "tc");
    let mut column = 0i64;
    if let Some(grid_before) =
        w_child(tree, tr, "trPr").and_then(|pr| w_child(tree, pr, "gridBefore"))
    {
        column = match w_attr(tree, grid_before, "val").filter(|v| !v.is_empty()) {
            Some(value) => require_int(Some(value), "w:gridBefore/@w:val")?,
            None => 0,
        };
    }
    for cell in &cells {
        if *cell == tc {
            break;
        }
        let span = w_child(tree, *cell, "tcPr").and_then(|pr| w_child(tree, pr, "gridSpan"));
        column += match span {
            Some(span) => match w_attr(tree, span, "val").filter(|v| !v.is_empty()) {
                Some(value) => require_int(Some(value), "w:gridSpan/@w:val")?,
                None => 1,
            },
            None => 1,
        };
    }
    let row = rows.iter().position(|r| *r == tr).ok_or_else(unreachable)?;
    let within = selected(tree, tc, "p")
        .iter()
        .position(|p| *p == paragraph)
        .ok_or_else(unreachable)?;
    Ok(Some((row, column, within)))
}

/// Every numbering definition and every numbered paragraph in `document`
/// (upstream `list_numbering`). Refuses when a paragraph resolves to a
/// missing definition or to a level the definition omits.
pub fn list_numbering(document: &Document) -> Result<NumberingReport> {
    let definitions = definitions(live_numbering_tree(document)?)?;
    let by_id: HashMap<i64, &NumberingDefinition> =
        definitions.iter().map(|d| (d.num_id, d)).collect();
    let styles = StylesView::from_document(document)?;
    let index = style_index(&styles);
    let mut numbered = Vec::new();
    for story in story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        for block in iter_block_elements(tree, &story.name)? {
            let paragraphs = match block.kind {
                BlockKind::Paragraph => vec![block.element],
                BlockKind::Table => table_paragraphs(tree, block.element),
            };
            for paragraph in paragraphs {
                let Some((num_id, level)) = effective_numbering(&styles, &index, tree, paragraph)?
                else {
                    continue;
                };
                let Some(definition) = by_id.get(&num_id) else {
                    return Err(Error::refusal(
                        RefusalKind::UnsupportedStructure,
                        "missing_numbering_definition",
                        format!(
                            "paragraph resolves to missing numbering definition numId={num_id}; nothing was changed"
                        ),
                    ));
                };
                if !definition.levels.iter().any(|l| l.level == level) {
                    return Err(Error::refusal(
                        RefusalKind::UnsupportedStructure,
                        "undefined_numbering_level",
                        format!(
                            "paragraph resolves to undefined numbering level {level} for numId={num_id}; nothing was changed"
                        ),
                    ));
                }
                let text = subtree_text(
                    tree,
                    paragraph,
                    View::Current,
                    true,
                    block.in_sdt,
                    block.in_txbx,
                )
                .text;
                numbered.push(NumberedParagraph {
                    story: story.name.clone(),
                    index: block.index,
                    num_id,
                    level,
                    text,
                    table_cell: table_cell_address(tree, paragraph)?,
                });
            }
        }
    }
    Ok(NumberingReport {
        definitions,
        numbered_paragraphs: numbered,
    })
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// `CT_NumPr` child order.
const NUMPR_SEQ: &[&str] = &["ilvl", "numId", "numberingChange", "ins"];

fn locate_target(
    document: &Document,
    paragraph: BlockTarget<'_>,
) -> Result<crate::anchor::Located> {
    let located = locate_paragraph(document, paragraph)?;
    require_current_view(paragraph)?;
    Ok(located)
}

/// Give `paragraph` the existing definition `num_id` at `level` (upstream
/// `apply_numbering`). Never fabricates a definition.
pub fn apply_numbering(
    document: &mut Document,
    paragraph: BlockTarget<'_>,
    num_id: i64,
    level: i64,
) -> Result<()> {
    let located = locate_target(document, paragraph)?;
    refuse_if_protected(document, "apply numbering", OperationClass::BodyContent)?;
    let definitions = definitions(live_numbering_tree(document)?)?;
    let Some(definition) = definitions.iter().find(|d| d.num_id == num_id) else {
        let defined: Vec<i64> = definitions.iter().map(|d| d.num_id).collect();
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "numbering_definition_missing",
            format!(
                "numbering definition numId={num_id} does not exist (defined: {defined:?}); authoring new definitions is not supported"
            ),
        ));
    };
    if !definition.levels.iter().any(|l| l.level == level) {
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "numbering_level_missing",
            format!("numbering definition numId={num_id} does not define level {level}"),
        ));
    }
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&located.part)?;
    let ppr = get_or_add_ppr(tree, located.element)?;
    let num_pr = get_or_add(tree, ppr, "numPr", PPR_SEQ)?;
    let ilvl = get_or_add(tree, num_pr, "ilvl", NUMPR_SEQ)?;
    set_w(tree, ilvl, "val", &level.to_string())?;
    let num = get_or_add(tree, num_pr, "numId", NUMPR_SEQ)?;
    set_w(tree, num, "val", &num_id.to_string())?;
    let staged = txn.finish()?;
    document.commit(staged)
}

/// The numId `style_name` binds through its `basedOn` chain (numId 0 reads
/// as no binding).
fn style_numbering_binding(styles: &StylesView<'_>, style_name: &str) -> Result<Option<i64>> {
    let tree: &XmlTree = &styles.tree;
    let mut style = styles.lookup(style_name);
    let mut seen: HashSet<Option<String>> = HashSet::new();
    while let Some(current) = style {
        if !seen.insert(w_attr(tree, current, "styleId").map(str::to_string)) {
            break;
        }
        let value = w_children(tree, current, "pPr")
            .flat_map(|ppr| w_children(tree, ppr, "numPr").collect::<Vec<_>>())
            .flat_map(|num_pr| w_children(tree, num_pr, "numId").collect::<Vec<_>>())
            .find_map(|num_id| w_attr(tree, num_id, "val"));
        if let Some(value) = value {
            let num_id = require_int(Some(value), "w:numId/@w:val")?;
            return Ok((num_id != 0).then_some(num_id));
        }
        style = w_child(tree, current, "basedOn")
            .and_then(|based| w_attr(tree, based, "val"))
            .and_then(|based| styles.get_by_id(based));
    }
    Ok(None)
}

/// Apply the existing paragraph style `style_name` (a UI name) to
/// `paragraph` (upstream `apply_list_style`). Refuses an undefined style
/// and a style whose numbering binding does not resolve.
pub fn apply_list_style(
    document: &mut Document,
    paragraph: BlockTarget<'_>,
    style_name: &str,
) -> Result<()> {
    let located = locate_target(document, paragraph)?;
    refuse_if_protected(document, "apply a list style", OperationClass::BodyContent)?;
    let mut txn = Txn::new(document);
    let (style_id, missing) = {
        let styles = StylesView::from_txn(&txn)?;
        if !styles.ui_names().contains(style_name) {
            return Err(Error::refusal(
                RefusalKind::TargetNotFound,
                "style_not_defined",
                format!("style {style_name:?} is not defined in this document"),
            ));
        }
        if let Some(bound) = style_numbering_binding(&styles, style_name)? {
            let definitions = definitions(live_numbering_tree(txn.document())?)?;
            if !definitions.iter().any(|d| d.num_id == bound) {
                return Err(Error::refusal(
                    RefusalKind::TargetNotFound,
                    "style_numbering_unresolved",
                    format!(
                        "style {style_name:?} binds numbering definition numId={bound}, which does not resolve in this \
                         document — applying it would render a fake, marker-less list; add a real definition first"
                    ),
                ));
            }
        }
        (styles.paragraph_style_id(style_name)?, styles.missing)
    };
    if missing {
        crate::parts::ensure_styles_part(&mut txn)?;
    }
    let tree = txn.tree_mut(&located.part)?;
    set_p_style(tree, located.element, style_id.as_deref())?;
    let staged = txn.finish()?;
    document.commit(staged)
}

// ---------------------------------------------------------------------------
// Minimal, guarded numbering authoring
// ---------------------------------------------------------------------------

/// Canonical definition kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// `PaperBulletList`.
    Bullet,
    /// `PaperDecimalList`.
    Decimal,
}

impl ListKind {
    fn marker(self) -> &'static str {
        match self {
            ListKind::Bullet => "PaperBulletList",
            ListKind::Decimal => "PaperDecimalList",
        }
    }

    fn template(self) -> &'static str {
        match self {
            ListKind::Bullet => concat!(
                "<w:abstractNum {ids}>",
                "<w:name w:val=\"PaperBulletList\"/>",
                "<w:multiLevelType w:val=\"hybridMultilevel\"/>",
                "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>",
                "<w:lvlText w:val=\"\u{2022}\"/><w:lvlJc w:val=\"left\"/>",
                "<w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr>",
                "<w:rPr><w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\" w:hint=\"default\"/></w:rPr></w:lvl>",
                "<w:lvl w:ilvl=\"1\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>",
                "<w:lvlText w:val=\"o\"/><w:lvlJc w:val=\"left\"/>",
                "<w:pPr><w:ind w:left=\"1440\" w:hanging=\"360\"/></w:pPr>",
                "<w:rPr><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"",
                " w:hint=\"default\"/></w:rPr></w:lvl>",
                "<w:lvl w:ilvl=\"2\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>",
                "<w:lvlText w:val=\"\u{25aa}\"/><w:lvlJc w:val=\"left\"/>",
                "<w:pPr><w:ind w:left=\"2160\" w:hanging=\"360\"/></w:pPr>",
                "<w:rPr><w:rFonts w:ascii=\"Wingdings\" w:hAnsi=\"Wingdings\"",
                " w:hint=\"default\"/></w:rPr></w:lvl>",
                "</w:abstractNum>"
            ),
            ListKind::Decimal => concat!(
                "<w:abstractNum {ids}>",
                "<w:name w:val=\"PaperDecimalList\"/>",
                "<w:multiLevelType w:val=\"hybridMultilevel\"/>",
                "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/>",
                "<w:lvlText w:val=\"%1.\"/><w:lvlJc w:val=\"left\"/>",
                "<w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                "<w:lvl w:ilvl=\"1\"><w:start w:val=\"1\"/><w:numFmt w:val=\"lowerLetter\"/>",
                "<w:lvlText w:val=\"%2.\"/><w:lvlJc w:val=\"left\"/>",
                "<w:pPr><w:ind w:left=\"1440\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                "<w:lvl w:ilvl=\"2\"><w:start w:val=\"1\"/><w:numFmt w:val=\"lowerRoman\"/>",
                "<w:lvlText w:val=\"%3.\"/><w:lvlJc w:val=\"right\"/>",
                "<w:pPr><w:ind w:left=\"2160\" w:hanging=\"180\"/></w:pPr></w:lvl>",
                "</w:abstractNum>"
            ),
        }
    }

    fn canonical_xml(self, abstract_id: &str) -> String {
        self.template().replace(
            "{ids}",
            &format!("xmlns:w=\"{W}\" w:abstractNumId=\"{abstract_id}\""),
        )
    }
}

const EMPTY_NUMBERING: &[u8] =
    b"<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"/>";

/// Prefix- and attribute-order-independent structure signature (upstream
/// `_structural_signature`).
#[derive(Debug, PartialEq, Eq)]
enum Signature {
    Element {
        name: (String, String),
        attrs: Vec<(String, String, String)>,
        text: String,
        children: Vec<Signature>,
    },
    Other(String),
}

fn signature(tree: &XmlTree, node: NodeId, ignore_root_abstract_id: bool) -> Signature {
    let Some(element) = tree.element(node) else {
        return Signature::Other(format!("{:?}", tree.data(node)));
    };
    let mut attrs: Vec<(String, String, String)> = element
        .attrs
        .iter()
        .map(|attr| {
            (
                attr.ns
                    .map(|ns| tree.uri(ns).to_string())
                    .unwrap_or_default(),
                attr.name.local.to_string(),
                attr.value.clone(),
            )
        })
        .filter(|(ns, local, _)| !(ignore_root_abstract_id && ns == W && local == "abstractNumId"))
        .collect();
    attrs.sort();
    let mut text = leading_text(tree, node);
    if py_strip(&text).is_empty() {
        text.clear();
    }
    Signature::Element {
        name: (
            tree.ns_uri(node).unwrap_or("").to_string(),
            tree.local_name(node).unwrap_or("").to_string(),
        ),
        attrs,
        text,
        children: lxml_children(tree, node)
            .into_iter()
            .map(|child| signature(tree, child, false))
            .collect(),
    }
}

fn is_canonical_abstract(tree: &XmlTree, abstract_num: NodeId, kind: ListKind) -> Result<bool> {
    let expected = xml::parse_str(&kind.canonical_xml("0"))
        .map_err(|err| Error::internal("canonical_template", err.to_string()))?;
    Ok(signature(tree, abstract_num, true) == signature(&expected, expected.root(), true))
}

fn is_plain_num_reference(tree: &XmlTree, num: NodeId, abstract_id: &str) -> bool {
    let children = lxml_children(tree, num);
    let [reference] = children.as_slice() else {
        return false;
    };
    if !is_w(tree, *reference, "abstractNumId") {
        return false;
    }
    let ref_attrs = &tree
        .element(*reference)
        .map(|e| e.attrs.clone())
        .unwrap_or_default();
    let num_attrs = &tree
        .element(num)
        .map(|e| e.attrs.clone())
        .unwrap_or_default();
    let is_w_attr = |attr: &crate::xml::Attr, local: &str| {
        attr.ns.map(|ns| tree.uri(ns)) == Some(W) && &*attr.name.local == local
    };
    ref_attrs.len() == 1
        && is_w_attr(&ref_attrs[0], "val")
        && ref_attrs[0].value == abstract_id
        && num_attrs.len() == 1
        && is_w_attr(&num_attrs[0], "numId")
}

/// Upstream `_next_free_ids`.
pub(crate) fn next_free_ids(tree: &XmlTree) -> Result<(i64, i64)> {
    let root = tree.root();
    let parse = |value: Option<&str>, what: &str| match value.filter(|v| !v.is_empty()) {
        None => Ok(0),
        Some(value) => require_int(Some(value), what),
    };
    let mut max_abstract = -1;
    for abstract_num in w_children(tree, root, "abstractNum") {
        max_abstract = max_abstract.max(parse(
            w_attr(tree, abstract_num, "abstractNumId"),
            "w:abstractNumId",
        )?);
    }
    let mut max_num = 0;
    for num in w_children(tree, root, "num") {
        max_num = max_num.max(parse(w_attr(tree, num, "numId"), "w:numId")?);
    }
    Ok((max_abstract + 1, max_num + 1))
}

/// The numbering part, created (`word/numbering.xml`, empty root) when the
/// main document has none.
pub(crate) fn get_or_create_numbering(txn: &mut Txn<'_>) -> Result<String> {
    crate::parts::ensure_part(
        txn,
        rt::NUMBERING,
        "word/numbering.xml",
        ct::WML_NUMBERING,
        EMPTY_NUMBERING,
    )
}

/// Upstream `_ensure_definition` inside a transaction.
pub(crate) fn ensure_definition_in(txn: &mut Txn<'_>, kind: ListKind) -> Result<i64> {
    let part = get_or_create_numbering(txn)?;
    {
        let tree = txn.tree(&part)?;
        let root = tree.root();
        for abstract_num in w_children(tree, root, "abstractNum") {
            let named = w_child(tree, abstract_num, "name")
                .and_then(|name| w_attr(tree, name, "val"))
                == Some(kind.marker());
            if !named || !is_canonical_abstract(tree, abstract_num, kind)? {
                continue;
            }
            let Some(abstract_id) = w_attr(tree, abstract_num, "abstractNumId") else {
                continue;
            };
            for num in w_children(tree, root, "num") {
                if is_plain_num_reference(tree, num, abstract_id) {
                    return require_int(w_attr(tree, num, "numId"), "w:numId");
                }
            }
        }
    }
    let (abstract_id, num_id) = next_free_ids(txn.tree(&part)?)?;
    let tree = txn.tree_mut(&part)?;
    let root = tree.root();
    let abstract_num = import_fragment(tree, root, &kind.canonical_xml(&abstract_id.to_string()))?;
    let num = import_fragment(
        tree,
        root,
        &format!(
            "<w:num xmlns:w=\"{W}\" w:numId=\"{num_id}\"><w:abstractNumId w:val=\"{abstract_id}\"/></w:num>"
        ),
    )?;
    // Schema order: abstractNum* before num*.
    match w_child(tree, root, "num") {
        Some(first_num) => add_previous(tree, first_num, abstract_num)?,
        None => crate::oxml::append(tree, root, abstract_num)?,
    }
    match w_children(tree, root, "num").last() {
        Some(last) => add_next(tree, last, num)?,
        None => match w_child(tree, root, "numIdMacAtCleanup") {
            Some(cleanup) => add_previous(tree, cleanup, num)?,
            None => crate::oxml::append(tree, root, num)?,
        },
    }
    Ok(num_id)
}

fn ensure_definition(document: &mut Document, kind: ListKind) -> Result<i64> {
    refuse_if_protected(
        document,
        "author a numbering definition",
        OperationClass::BodyContent,
    )?;
    let mut txn = Txn::new(document);
    let num_id = ensure_definition_in(&mut txn, kind)?;
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(num_id)
}

/// The numId of a canonical bullet-list definition, created (with
/// `word/numbering.xml` if needed) when absent. Reuse requires the exact
/// canonical shape.
pub fn ensure_bullet_definition(document: &mut Document) -> Result<i64> {
    ensure_definition(document, ListKind::Bullet)
}

/// The numId of a canonical decimal-list definition (same rules as
/// [`ensure_bullet_definition`]).
pub fn ensure_decimal_definition(document: &mut Document) -> Result<i64> {
    ensure_definition(document, ListKind::Decimal)
}

/// A new numId continuing `num_id`'s formatting with level 0 restarted at 1
/// (upstream `restart_numbering`). Paragraphs are not re-pointed.
pub fn restart_numbering(document: &mut Document, num_id: i64) -> Result<i64> {
    refuse_if_protected(document, "restart numbering", OperationClass::BodyContent)?;
    let package = document.package();
    let part = package.part_related_by(package.main_part_name(), rt::NUMBERING)?;
    let tree = match &part {
        Some(part) => Some(package.xml(part)?),
        None => None,
    };
    let mut definition = None;
    if let Some(tree) = tree {
        for num in w_children(tree, tree.root(), "num") {
            let raw = w_attr(tree, num, "numId");
            let candidate = match raw {
                None => -1,
                Some(raw) => py_int(raw).ok_or_else(|| {
                    Error::refusal(
                        RefusalKind::UnsupportedStructure,
                        "malformed_num_id",
                        format!(
                            "numbering contains a malformed numId {raw:?}; nothing was changed"
                        ),
                    )
                })?,
            };
            if candidate == num_id {
                definition = Some(num);
                break;
            }
        }
    }
    let (Some(part), Some(tree), Some(definition)) = (part.clone(), tree, definition) else {
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "numbering_definition_missing",
            format!("numbering definition numId={num_id} does not exist; nothing to restart"),
        ));
    };
    let children = lxml_children(tree, definition);
    let abstract_ref = w_child(tree, definition, "abstractNumId");
    let Some(abstract_id) = abstract_ref
        .and_then(|reference| w_attr(tree, reference, "val"))
        .map(str::to_string)
    else {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "no_abstract_num_id",
            format!(
                "numbering definition numId={num_id} has no abstractNumId; nothing was changed"
            ),
        ));
    };
    if children.len() != 1 || Some(children[0]) != abstract_ref {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "numbering_overrides",
            format!(
                "numbering definition numId={num_id} carries level overrides or other customizations that \
                 restart_numbering cannot preserve; nothing was changed"
            ),
        ));
    }
    if !w_children(tree, tree.root(), "abstractNum").any(|abstract_num| {
        w_attr(tree, abstract_num, "abstractNumId") == Some(abstract_id.as_str())
    }) {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "missing_abstract_num",
            format!(
                "numbering definition numId={num_id} references missing abstractNumId={abstract_id:?}; nothing was changed"
            ),
        ));
    }
    let (_, new_num_id) = next_free_ids(tree)?;
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&part)?;
    let root = tree.root();
    let new_num = import_fragment(
        tree,
        root,
        &format!(
            "<w:num xmlns:w=\"{W}\" w:numId=\"{new_num_id}\"><w:abstractNumId w:val=\"{}\"/>\
             <w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"1\"/></w:lvlOverride></w:num>",
            xml_attr_escape(&abstract_id)
        ),
    )?;
    let last = w_children(tree, root, "num")
        .last()
        .ok_or_else(|| Error::internal("restart_numbering", "definition vanished"))?;
    add_next(tree, last, new_num)?;
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(new_num_id)
}

fn xml_attr_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

impl Document {
    /// See [`list_numbering`].
    pub fn list_numbering(&self) -> Result<NumberingReport> {
        list_numbering(self)
    }

    /// See [`apply_numbering`].
    pub fn apply_numbering(
        &mut self,
        paragraph: BlockTarget<'_>,
        num_id: i64,
        level: i64,
    ) -> Result<()> {
        apply_numbering(self, paragraph, num_id, level)
    }

    /// See [`apply_list_style`].
    pub fn apply_list_style(&mut self, paragraph: BlockTarget<'_>, style_name: &str) -> Result<()> {
        apply_list_style(self, paragraph, style_name)
    }

    /// See [`ensure_bullet_definition`].
    pub fn ensure_bullet_definition(&mut self) -> Result<i64> {
        ensure_bullet_definition(self)
    }

    /// See [`ensure_decimal_definition`].
    pub fn ensure_decimal_definition(&mut self) -> Result<i64> {
        ensure_decimal_definition(self)
    }

    /// See [`restart_numbering`].
    pub fn restart_numbering(&mut self, num_id: i64) -> Result<i64> {
        restart_numbering(self, num_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_int_matches_python() {
        assert_eq!(py_int("42"), Some(42));
        assert_eq!(py_int(" -7 "), Some(-7));
        assert_eq!(py_int("+1_000"), Some(1000));
        assert_eq!(py_int("007"), Some(7));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("1.0"), None);
    }
}
