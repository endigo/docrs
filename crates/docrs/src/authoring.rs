//! The inherited python-docx authoring API (upstream `document.py`,
//! `blkcntnr.py`, `text/paragraph.py`, `text/run.py`, `text/parfmt.py`,
//! `text/font.py`, `table.py`, `parts/story.py`, `package.py`,
//! `oxml/*`; python-docx, MIT, see `NOTICE.md`).
//!
//! Handles ([`Paragraph`], [`Run`], [`Table`], [`Cell`]) are live and local:
//! each carries the document identity, the package member, the element's
//! node id and freshness evidence (root, parent and structural generation).
//! Every operation re-validates its handle, stages its writes in a
//! [`crate::txn::Txn`] and commits last, so a refusal or error leaves the
//! document and every other handle unchanged.
//!
//! XML effects follow upstream exactly, including lxml's text semantics:
//! removing an element removes its tail text, and `insert(0, …)` (used for
//! `w:pPr`, `w:rPr`, `w:tcPr`) lands after an element's leading text.
//! Like upstream, these inherited operations are not gated by document
//! protection (only paper-docx's own operations are).

use crate::image::Image;
use crate::oxml::{
    PPR_SEQ, RPR_SEQ, add_t, append, get_or_add, insert_before_successors, successors,
};
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::{RelTarget, Txn};
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, DocumentId, Error, RefusalKind, Result};

/// DrawingML wordprocessingDrawing (`wp:`).
pub const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
/// DrawingML main (`a:`).
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// DrawingML picture (`pic:`).
pub const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
/// Office relationships (`r:`).
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const PICTURE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";

// ------------------------------------------------------------- handles

/// Freshness evidence shared by every live authoring handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Handle {
    pub document: DocumentId,
    pub part: String,
    pub root: NodeId,
    pub element: NodeId,
    pub parent: Option<NodeId>,
    pub struct_gen: u32,
}

impl Handle {
    pub(crate) fn capture(doc: &Document, part: &str, tree: &XmlTree, element: NodeId) -> Handle {
        Handle {
            document: doc.id(),
            part: part.to_string(),
            root: tree.root(),
            element,
            parent: tree.parent(element),
            struct_gen: tree.struct_gen(element),
        }
    }

    /// Validate against the live document; returns the live tree.
    pub(crate) fn check<'d>(
        &self,
        doc: &'d Document,
        local: &str,
        what: &str,
    ) -> Result<&'d XmlTree> {
        self.check_in(doc, W, local, what)
    }

    /// [`Handle::check`] for an element in namespace `ns`.
    pub(crate) fn check_in<'d>(
        &self,
        doc: &'d Document,
        ns: &str,
        local: &str,
        what: &str,
    ) -> Result<&'d XmlTree> {
        if self.document != doc.id() {
            return Err(Error::refusal(
                RefusalKind::BoundaryViolation,
                "foreign_handle",
                format!(
                    "{what} belongs to a different document; reacquire it from the document passed to this operation"
                ),
            ));
        }
        let stale = || {
            Error::refusal(
                RefusalKind::TargetNotFound,
                "stale_handle",
                format!(
                    "{what} is stale: its element was removed, moved or replaced; reacquire it"
                ),
            )
        };
        if doc.package().part(&self.part).is_none() {
            return Err(stale());
        }
        let tree = doc.package().xml(&self.part)?;
        if tree.root() != self.root
            || !tree.contains(self.element)
            || !tree.is_attached(self.element)
            || tree.parent(self.element) != self.parent
            || tree.struct_gen(self.element) != self.struct_gen
            || !tree.is(self.element, ns, local)
        {
            return Err(stale());
        }
        Ok(tree)
    }
}

/// A live `w:p` handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub(crate) h: Handle,
}

/// A live `w:r` handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub(crate) h: Handle,
}

/// A live `w:tbl` handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub(crate) h: Handle,
}

/// A live `w:tc` handle (upstream `_Cell`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub(crate) h: Handle,
}

/// An inline picture added by [`Document::add_picture`] / [`Run::add_picture`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InlineShape {
    /// `wp:extent/@cx` in EMU.
    pub width: i64,
    /// `wp:extent/@cy` in EMU.
    pub height: i64,
    /// Relationship id the picture embeds.
    pub r_id: String,
    /// The image part (member name).
    pub image_part: String,
}

/// Section start (`WD_SECTION_START`, by XML token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SectionStart {
    /// `continuous`.
    Continuous,
    /// `nextColumn`.
    NewColumn,
    /// `nextPage` (the default; written as no `w:type`).
    NewPage,
    /// `evenPage`.
    EvenPage,
    /// `oddPage`.
    OddPage,
}

impl SectionStart {
    /// XML token.
    pub fn as_str(self) -> &'static str {
        match self {
            SectionStart::Continuous => "continuous",
            SectionStart::NewColumn => "nextColumn",
            SectionStart::NewPage => "nextPage",
            SectionStart::EvenPage => "evenPage",
            SectionStart::OddPage => "oddPage",
        }
    }

    /// From an XML token (`ValueError` upstream for unknown tokens).
    pub fn parse(token: &str) -> Result<SectionStart> {
        Ok(match token {
            "continuous" => SectionStart::Continuous,
            "nextColumn" => SectionStart::NewColumn,
            "nextPage" => SectionStart::NewPage,
            "evenPage" => SectionStart::EvenPage,
            "oddPage" => SectionStart::OddPage,
            other => {
                return Err(Error::invalid_argument(
                    "invalid_section_start",
                    format!("WD_SECTION_START has no XML mapping for {other:?}"),
                ));
            }
        })
    }
}

/// The section added by [`Document::add_section`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Section {
    /// Its start type.
    pub start_type: SectionStart,
}

/// Break kinds `Run::add_break` accepts (`WD_BREAK`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakType {
    /// Line break (`<w:br/>`).
    Line,
    /// Page break.
    Page,
    /// Column break.
    Column,
    /// Line break clearing left.
    LineClearLeft,
    /// Line break clearing right.
    LineClearRight,
    /// Line break clearing both sides.
    LineClearAll,
}

// --------------------------------------------------------- lxml helpers

/// lxml `parent.insert(0, child)`: before the first non-text child, i.e.
/// after the parent's leading text.
pub(crate) fn insert_first(tree: &mut XmlTree, parent: NodeId, child: NodeId) -> Result<()> {
    let index = tree
        .children(parent)
        .iter()
        .position(|node| !matches!(tree.data(*node), NodeData::Text(_)))
        .unwrap_or(tree.children(parent).len());
    tree.insert(parent, index, child)
}

/// lxml `parent.remove(element)`: the element and its tail text go.
pub(crate) fn remove(tree: &mut XmlTree, element: NodeId) -> Result<()> {
    if let Some(parent) = tree.parent(element) {
        let children = tree.children(parent).to_vec();
        if let Some(index) = children.iter().position(|node| *node == element) {
            for node in children[index + 1..].iter() {
                if matches!(tree.data(*node), NodeData::Text(_)) {
                    tree.detach(*node)?;
                } else {
                    break;
                }
            }
        }
    }
    tree.detach(element)
}

/// xmlchemy `_remove_<tag>`: every direct `w:local` child.
pub(crate) fn remove_all(tree: &mut XmlTree, parent: NodeId, local: &str) -> Result<()> {
    let doomed: Vec<NodeId> = w_children(tree, parent, local).collect();
    for node in doomed {
        remove(tree, node)?;
    }
    Ok(())
}

/// `./*[not(self::w:<keep>)]` removal (`clear_content`).
pub(crate) fn clear_except(tree: &mut XmlTree, parent: NodeId, keep: &str) -> Result<()> {
    let doomed: Vec<NodeId> = tree
        .child_elements(parent)
        .filter(|node| !tree.is(*node, W, keep))
        .collect();
    for node in doomed {
        remove(tree, node)?;
    }
    Ok(())
}

fn get_or_add_first(tree: &mut XmlTree, parent: NodeId, local: &str) -> Result<NodeId> {
    if let Some(existing) = w_child(tree, parent, local) {
        return Ok(existing);
    }
    let child = tree.create_element(parent, W, local)?;
    insert_first(tree, parent, child)?;
    Ok(child)
}

fn get_or_add_ppr(tree: &mut XmlTree, p: NodeId) -> Result<NodeId> {
    get_or_add_first(tree, p, "pPr")
}

fn get_or_add_rpr(tree: &mut XmlTree, r: NodeId) -> Result<NodeId> {
    get_or_add_first(tree, r, "rPr")
}

/// `CT_Style` child order (upstream `CT_Style._tag_seq`).
pub(crate) const STYLE_SEQ: &[&str] = &[
    "name",
    "aliases",
    "basedOn",
    "next",
    "link",
    "autoRedefine",
    "hidden",
    "uiPriority",
    "semiHidden",
    "unhideWhenUsed",
    "qFormat",
    "locked",
    "personal",
    "personalCompose",
    "personalReply",
    "rsid",
    "pPr",
    "rPr",
    "tblPr",
    "trPr",
    "tcPr",
    "tblStylePr",
];

/// The element whose `w:pPr` / `w:rPr` a `ParagraphFormat` or `Font`
/// edits: a paragraph or run (`insert(0, …)`, upstream `CT_P` / `CT_R`) or a
/// style definition (schema order, upstream `CT_Style`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    /// `w:p` or `w:r`.
    Text,
    /// `w:style`.
    Style,
}

/// `get_or_add_pPr` for `host`.
pub(crate) fn host_ppr(tree: &mut XmlTree, element: NodeId, host: Host) -> Result<NodeId> {
    match host {
        Host::Text => get_or_add_ppr(tree, element),
        Host::Style => get_or_add(tree, element, "pPr", STYLE_SEQ),
    }
}

/// `get_or_add_rPr` for `host`.
pub(crate) fn host_rpr(tree: &mut XmlTree, element: NodeId, host: Host) -> Result<NodeId> {
    match host {
        Host::Text => get_or_add_rpr(tree, element),
        Host::Style => get_or_add(tree, element, "rPr", STYLE_SEQ),
    }
}

pub(crate) fn new_w(tree: &mut XmlTree, scope: NodeId, local: &str) -> Result<NodeId> {
    tree.create_element(scope, W, local)
}

pub(crate) fn set_w(tree: &mut XmlTree, id: NodeId, local: &str, value: &str) -> Result<()> {
    tree.set_attr(id, Some(W), local, value)
}

/// `insert_element_before(*successors)` for a new `w:local` child.
pub(crate) fn add_w(
    tree: &mut XmlTree,
    parent: NodeId,
    local: &str,
    after: &[&str],
) -> Result<NodeId> {
    let child = new_w(tree, parent, local)?;
    insert_before_successors(tree, parent, child, after)?;
    Ok(child)
}

// ------------------------------------------------------------- text views

fn run_child_text(tree: &XmlTree, node: NodeId) -> Option<String> {
    if tree.ns_uri(node) != Some(W) {
        return None;
    }
    Some(match tree.local_name(node)? {
        "t" => tree.text_content(node),
        "tab" | "ptab" => "\t".to_string(),
        "cr" => "\n".to_string(),
        "noBreakHyphen" => "-".to_string(),
        "br" => match w_attr(tree, node, "type").unwrap_or("textWrapping") {
            "textWrapping" => "\n".to_string(),
            _ => String::new(),
        },
        _ => return None,
    })
}

/// Upstream `CT_R.text`.
pub(crate) fn run_text(tree: &XmlTree, r: NodeId) -> String {
    tree.child_elements(r)
        .filter_map(|child| run_child_text(tree, child))
        .collect()
}

/// Upstream `CT_P.text` (runs and hyperlink runs).
pub(crate) fn paragraph_text(tree: &XmlTree, p: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(p) {
        if tree.is(child, W, "r") {
            out.push_str(&run_text(tree, child));
        } else if tree.is(child, W, "hyperlink") {
            for r in w_children(tree, child, "r") {
                out.push_str(&run_text(tree, r));
            }
        }
    }
    out
}

// --------------------------------------------------------------- lengths

/// `Emu(value).twips`: `int(round(emu / 635.0))`.
fn twips(emu: i64) -> i64 {
    (emu as f64 / 635.0).round_ties_even() as i64
}

pub(crate) fn check_range(value: i64, low: i128, high: i128, what: &str) -> Result<()> {
    if (value as i128) < low || (value as i128) > high {
        return Err(Error::invalid_argument(
            "value_out_of_range",
            format!("{what} value must be in range {low} to {high} inclusive, got {value}"),
        ));
    }
    Ok(())
}

/// `ST_TwipsMeasure.to_xml` (XsdUnsignedLong validation on the EMU value).
pub(crate) fn twips_measure(emu: i64, what: &str) -> Result<String> {
    check_range(emu, 0, u64::MAX as i128, what)?;
    Ok(twips(emu).to_string())
}

/// `ST_SignedTwipsMeasure.to_xml` (XsdInt validation on the EMU value).
pub(crate) fn signed_twips_measure(emu: i64, what: &str) -> Result<String> {
    check_range(emu, i32::MIN as i128, i32::MAX as i128, what)?;
    Ok(twips(emu).to_string())
}

/// `ST_TwipsMeasure.convert_from_xml` → EMU.
pub(crate) fn parse_twips_measure(value: &str) -> Result<i64> {
    if value.contains('i') || value.contains('m') || value.contains('p') {
        return parse_universal_measure(value);
    }
    let twips: i64 = value.trim().parse().map_err(|_| {
        Error::invalid_argument(
            "invalid_measure",
            format!("invalid literal for int(): {value:?}"),
        )
    })?;
    Ok(twips * 635)
}

/// Python `float(text)`: surrounding whitespace, `_` between digits,
/// `inf`/`nan` spellings.
pub(crate) fn py_float(text: &str) -> Option<f64> {
    let trimmed = crate::unicode::py_strip(text);
    if trimmed.is_empty() {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let mut cleaned = String::with_capacity(trimmed.len());
    for (index, ch) in chars.iter().enumerate() {
        if *ch == '_' {
            let digit_before = index > 0 && chars[index - 1].is_ascii_digit();
            let digit_after = chars.get(index + 1).is_some_and(char::is_ascii_digit);
            if !(digit_before && digit_after) {
                return None;
            }
            continue;
        }
        cleaned.push(*ch);
    }
    if !cleaned.is_ascii() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

fn parse_universal_measure(value: &str) -> Result<i64> {
    let bad = || {
        Error::invalid_argument(
            "invalid_measure",
            format!("invalid universal measure {value:?}"),
        )
    };
    let split = value
        .len()
        .checked_sub(2)
        .filter(|i| value.is_char_boundary(*i))
        .ok_or_else(bad)?;
    let (number, units) = value.split_at(split);
    let quantity: f64 = number.trim().parse().map_err(|_| bad())?;
    let multiplier = match units {
        "mm" => 36_000.0,
        "cm" => 360_000.0,
        "in" => 914_400.0,
        "pt" => 12_700.0,
        "pc" | "pi" => 152_400.0,
        _ => return Err(bad()),
    };
    Ok((quantity * multiplier).round_ties_even() as i64)
}

// ---------------------------------------------------------------- styles

/// Style type (`WD_STYLE_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleType {
    /// `paragraph`.
    Paragraph,
    /// `character`.
    Character,
    /// `table`.
    Table,
    /// `numbering`.
    Numbering,
}

impl StyleType {
    fn parse(token: &str) -> Result<StyleType> {
        Ok(match token {
            "paragraph" => StyleType::Paragraph,
            "character" => StyleType::Character,
            "table" => StyleType::Table,
            "numbering" => StyleType::Numbering,
            other => {
                return Err(Error::invalid_argument(
                    "invalid_style_type",
                    format!("WD_STYLE_TYPE has no XML mapping for {other:?}"),
                ));
            }
        })
    }

    fn label(self) -> &'static str {
        match self {
            StyleType::Paragraph => "PARAGRAPH (1)",
            StyleType::Character => "CHARACTER (2)",
            StyleType::Table => "TABLE (3)",
            StyleType::Numbering => "LIST (4)",
        }
    }
}

/// Upstream `BabelFish.ui2internal`.
pub fn ui_to_internal(name: &str) -> &str {
    match name {
        "Caption" => "caption",
        "Footer" => "footer",
        "Header" => "header",
        "Heading 1" => "heading 1",
        "Heading 2" => "heading 2",
        "Heading 3" => "heading 3",
        "Heading 4" => "heading 4",
        "Heading 5" => "heading 5",
        "Heading 6" => "heading 6",
        "Heading 7" => "heading 7",
        "Heading 8" => "heading 8",
        "Heading 9" => "heading 9",
        other => other,
    }
}

pub(crate) fn on_off(value: &str) -> Result<bool> {
    match value {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        other => Err(Error::invalid_argument(
            "invalid_on_off",
            format!(
                "value must be one of '1', '0', 'true', 'false', 'on', or 'off', got '{other}'"
            ),
        )),
    }
}

/// Upstream `StoryPart.get_style_id(style_or_name, style_type)`: `None` for
/// `None` or the default style of the type. Like upstream (which evaluates
/// `DocumentPart.styles` first), the styles part is created from the default
/// template when missing, even for `None`.
pub(crate) fn resolve_style_id(
    txn: &mut Txn<'_>,
    name: Option<&str>,
    kind: StyleType,
) -> Result<Option<String>> {
    let part = crate::parts::ensure_styles_part(txn)?;
    let Some(name) = name else {
        return Ok(None);
    };
    let tree = txn.tree(&part)?;
    let root = tree.root();
    let styles: Vec<NodeId> = w_children(tree, root, "style").collect();
    let internal = ui_to_internal(name);
    let by_name = styles.iter().copied().find(|style| {
        w_children(tree, *style, "name").any(|n| w_attr(tree, n, "val") == Some(internal))
    });
    let found = by_name.or_else(|| {
        styles
            .iter()
            .copied()
            .find(|style| w_attr(tree, *style, "styleId") == Some(name))
    });
    let Some(style) = found else {
        return Err(Error::invalid_argument(
            "unknown_style",
            format!("no style with name '{name}'"),
        ));
    };
    let style_type = match w_attr(tree, style, "type") {
        None => StyleType::Paragraph,
        Some(token) => StyleType::parse(token)?,
    };
    if style_type != kind {
        return Err(Error::invalid_argument(
            "style_type_mismatch",
            format!(
                "assigned style is type {}, need type {}",
                style_type.label(),
                kind.label()
            ),
        ));
    }
    let mut default = None;
    for candidate in &styles {
        let Some(token) = w_attr(tree, *candidate, "type") else {
            continue;
        };
        if StyleType::parse(token)? != kind {
            continue;
        }
        if let Some(flag) = w_attr(tree, *candidate, "default")
            && on_off(flag)?
        {
            default = Some(*candidate);
        }
    }
    if default == Some(style) {
        return Ok(None);
    }
    Ok(w_attr(tree, style, "styleId").map(str::to_string))
}

// ----------------------------------------------------- shared XML effects

/// `CT_R.text = text` (`clear_content` + `_RunContentAppender`).
fn set_run_text(tree: &mut XmlTree, r: NodeId, text: &str) -> Result<()> {
    clear_except(tree, r, "rPr")?;
    append_run_content(tree, r, text)
}

pub(crate) fn append_run_content(tree: &mut XmlTree, r: NodeId, text: &str) -> Result<()> {
    let mut buffer = String::new();
    let flush = |tree: &mut XmlTree, buffer: &mut String| -> Result<()> {
        if !buffer.is_empty() {
            add_t(tree, r, buffer)?;
            buffer.clear();
        }
        Ok(())
    };
    for ch in text.chars() {
        match ch {
            '\t' => {
                flush(tree, &mut buffer)?;
                let tab = new_w(tree, r, "tab")?;
                append(tree, r, tab)?;
            }
            '\r' | '\n' => {
                flush(tree, &mut buffer)?;
                let br = new_w(tree, r, "br")?;
                append(tree, r, br)?;
            }
            other => buffer.push(other),
        }
    }
    flush(tree, &mut buffer)
}

/// `CT_R.style = style_id`.
fn set_run_style(tree: &mut XmlTree, r: NodeId, style_id: Option<&str>) -> Result<()> {
    let rpr = get_or_add_rpr(tree, r)?;
    match (style_id, w_child(tree, rpr, "rStyle")) {
        (None, _) => remove_all(tree, rpr, "rStyle"),
        (Some(id), Some(existing)) => set_w(tree, existing, "val", id),
        (Some(id), None) => {
            let style = add_w(tree, rpr, "rStyle", successors(RPR_SEQ, "rStyle"))?;
            set_w(tree, style, "val", id)
        }
    }
}

/// `CT_P.style = style_id`.
fn set_paragraph_style_id(tree: &mut XmlTree, p: NodeId, style_id: Option<&str>) -> Result<()> {
    let ppr = get_or_add_ppr(tree, p)?;
    match style_id {
        None => remove_all(tree, ppr, "pStyle"),
        Some(id) => {
            let style = get_or_add(tree, ppr, "pStyle", PPR_SEQ)?;
            set_w(tree, style, "val", id)
        }
    }
}

/// Upstream `Paragraph.add_run(text, style)` within a transaction.
fn add_run_in(
    txn: &mut Txn<'_>,
    part: &str,
    p: NodeId,
    text: Option<&str>,
    style: Option<&str>,
) -> Result<NodeId> {
    let style_id = match style {
        Some(name) if !name.is_empty() => {
            Some(resolve_style_id(txn, Some(name), StyleType::Character)?)
        }
        _ => None,
    };
    let tree = txn.tree_mut(part)?;
    let r = new_w(tree, p, "r")?;
    append(tree, p, r)?;
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        set_run_text(tree, r, text)?;
    }
    if let Some(style_id) = style_id {
        set_run_style(tree, r, style_id.as_deref())?;
    }
    Ok(r)
}

/// `paragraph.add_run(text)` + `paragraph.style = style` for a new paragraph.
pub(crate) fn fill_paragraph(
    txn: &mut Txn<'_>,
    part: &str,
    p: NodeId,
    text: Option<&str>,
    style: Option<&str>,
) -> Result<()> {
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        add_run_in(txn, part, p, Some(text), None)?;
    }
    if style.is_some() {
        let style_id = resolve_style_id(txn, style, StyleType::Paragraph)?;
        let tree = txn.tree_mut(part)?;
        set_paragraph_style_id(tree, p, style_id.as_deref())?;
    }
    Ok(())
}

pub(crate) fn body(tree: &XmlTree) -> Result<NodeId> {
    w_child(tree, tree.root(), "body").ok_or_else(|| {
        Error::refusal(
            RefusalKind::UnsupportedStructure,
            "missing_body",
            "the main document has no w:body element",
        )
    })
}

/// `CT_Body.add_p()` (before `w:sectPr`).
fn add_body_paragraph(tree: &mut XmlTree) -> Result<NodeId> {
    let body = body(tree)?;
    let p = new_w(tree, body, "p")?;
    insert_before_successors(tree, body, p, &["sectPr"])?;
    Ok(p)
}

/// Finish the transaction, commit it, then yield `value`.
macro_rules! commit {
    ($doc:expr, $txn:expr, $value:expr) => {{
        let staged = $txn.finish()?;
        $doc.commit(staged)?;
        Ok($value)
    }};
}

/// Capture a handle for `element` from a staged tree.
pub(crate) fn staged_handle(txn: &Txn<'_>, part: &str, element: NodeId) -> Result<Handle> {
    let tree = txn.tree(part)?;
    Ok(Handle::capture(txn.document(), part, tree, element))
}

// ----------------------------------------------------------- pictures

/// Upstream `ImageParts`: image parts reached by `IMAGE` relationships in a
/// depth-first walk of the relationship graph, in first-encounter order.
pub(crate) fn image_parts(txn: &Txn<'_>) -> Result<Vec<String>> {
    fn walk(
        txn: &Txn<'_>,
        source: &str,
        visited: &mut Vec<String>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        let source_name = if source == "/" {
            "/".to_string()
        } else {
            format!("/{source}")
        };
        let base = crate::package::rels::base_uri(&source_name).to_string();
        for rel in txn.relationships(source)? {
            if rel.external {
                continue;
            }
            let Some(member) = txn.resolve(&rel.target_part_name(&base)) else {
                continue;
            };
            if rel.rel_type == rt::IMAGE && !out.contains(&member) {
                out.push(member.clone());
            }
            if visited.contains(&member) {
                continue;
            }
            visited.push(member.clone());
            walk(txn, &member, visited, out)?;
        }
        Ok(())
    }
    let mut visited = Vec::new();
    let mut out = Vec::new();
    walk(txn, "/", &mut visited, &mut out)?;
    Ok(out)
}

/// `PackURI.idx`: trailing number of `[a-zA-Z]+[1-9][0-9]*` at the start of
/// the extension-less file name.
fn partname_idx(member: &str) -> Option<u64> {
    let filename = member.rsplit('/').next().unwrap_or(member);
    let stem = match filename.rfind('.') {
        Some(dot) if filename[..dot].chars().any(|c| c != '.') => &filename[..dot],
        _ => filename,
    };
    let letters = stem.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    if letters == 0 {
        return None;
    }
    let rest = &stem[letters..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.starts_with('0') {
        return None;
    }
    digits.parse().ok()
}

/// `ImageParts._add_image_part(image)`: a new part at the next
/// `word/media/image%d.<ext>`.
pub(crate) fn add_image_part(txn: &mut Txn<'_>, image: &Image) -> Result<String> {
    add_image_part_ext(txn, image, image.ext())
}

/// [`add_image_part`] with an explicit extension (`Image.ext` is the file
/// name's extension for an image loaded from a path).
fn add_image_part_ext(txn: &mut Txn<'_>, image: &Image, ext: &str) -> Result<String> {
    let parts = image_parts(txn)?;
    let used: Vec<Option<u64>> = parts.iter().map(|p| partname_idx(p)).collect();
    let count = parts.len() as u64;
    let n = (1..=count)
        .find(|n| !used.contains(&Some(*n)))
        .unwrap_or(count + 1);
    let name = format!("word/media/image{n}.{ext}");
    if txn.resolve(&name).is_some() {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "image_partname_collision",
            format!(
                "the next image part name {name} is already used by a part that no image relationship reaches"
            ),
        ));
    }
    txn.add_binary_part(&name, image.content_type(), image.blob().to_vec())?;
    Ok(name)
}

/// `ImageParts.get_or_add_image_part`: reuse a part with the same SHA-1.
/// Returns the part and whether it already existed.
pub(crate) fn get_or_add_image_part(
    txn: &mut Txn<'_>,
    image: &Image,
    ext: Option<&str>,
) -> Result<(String, bool)> {
    for part in image_parts(txn)? {
        let matches = txn
            .binary(&part)
            .is_some_and(|bytes| crate::image::sha1_hex(&bytes) == image.sha1());
        if matches {
            return Ok((part, true));
        }
    }
    let part = match ext {
        Some(ext) => add_image_part_ext(txn, image, ext)?,
        None => add_image_part(txn, image)?,
    };
    Ok((part, false))
}

/// `StoryPart.next_id`: one more than the largest all-digit `@id`.
fn next_id(tree: &XmlTree) -> u128 {
    let mut max: Option<u128> = None;
    for node in tree.descendants(tree.root()) {
        let Some(element) = tree.element(node) else {
            continue;
        };
        for attr in &element.attrs {
            if attr.ns.is_none() && &*attr.name.local == "id" {
                let value = attr.value.as_str();
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    let parsed = value.parse::<u128>().unwrap_or(u128::MAX);
                    max = Some(max.map_or(parsed, |m| m.max(parsed)));
                }
            }
        }
    }
    max.map_or(1, |m| m.saturating_add(1))
}

fn el(
    tree: &mut XmlTree,
    scope: NodeId,
    ns: &str,
    local: &str,
    attrs: &[(&str, &str)],
) -> Result<NodeId> {
    let node = tree.create_element(scope, ns, local)?;
    for (name, value) in attrs {
        tree.set_attr(node, None, name, value)?;
    }
    Ok(node)
}

fn child(
    tree: &mut XmlTree,
    parent: NodeId,
    ns: &str,
    local: &str,
    attrs: &[(&str, &str)],
) -> Result<NodeId> {
    let node = el(tree, parent, ns, local, attrs)?;
    append(tree, parent, node)?;
    Ok(node)
}

/// `CT_Inline.new_pic_inline(shape_id, rId, filename, cx, cy)` built under
/// `scope` (detached).
fn new_pic_inline(
    tree: &mut XmlTree,
    scope: NodeId,
    shape_id: u128,
    r_id: &str,
    filename: &str,
    cx: i64,
    cy: i64,
) -> Result<NodeId> {
    let (cx_s, cy_s, id_s) = (cx.to_string(), cy.to_string(), shape_id.to_string());
    let name = format!("Picture {shape_id}");
    let inline = el(tree, scope, WP, "inline", &[])?;
    child(tree, inline, WP, "extent", &[("cx", &cx_s), ("cy", &cy_s)])?;
    child(tree, inline, WP, "docPr", &[("id", &id_s), ("name", &name)])?;
    let frame = child(tree, inline, WP, "cNvGraphicFramePr", &[])?;
    child(
        tree,
        frame,
        A,
        "graphicFrameLocks",
        &[("noChangeAspect", "1")],
    )?;
    let graphic = child(tree, inline, A, "graphic", &[])?;
    let data = child(tree, graphic, A, "graphicData", &[("uri", PICTURE_URI)])?;
    let pic = child(tree, data, PIC, "pic", &[])?;
    let nv = child(tree, pic, PIC, "nvPicPr", &[])?;
    child(tree, nv, PIC, "cNvPr", &[("id", "0"), ("name", filename)])?;
    child(tree, nv, PIC, "cNvPicPr", &[])?;
    let fill = child(tree, pic, PIC, "blipFill", &[])?;
    let blip = child(tree, fill, A, "blip", &[])?;
    tree.set_attr(blip, Some(R), "embed", r_id)?;
    let stretch = child(tree, fill, A, "stretch", &[])?;
    child(tree, stretch, A, "fillRect", &[])?;
    let sp = child(tree, pic, PIC, "spPr", &[])?;
    let xfrm = child(tree, sp, A, "xfrm", &[])?;
    child(tree, xfrm, A, "off", &[("x", "0"), ("y", "0")])?;
    child(tree, xfrm, A, "ext", &[("cx", &cx_s), ("cy", &cy_s)])?;
    child(tree, sp, A, "prstGeom", &[("prst", "rect")])?;
    Ok(inline)
}

/// Upstream `Run.add_picture` (`StoryPart.new_pic_inline` + `add_drawing`).
fn add_picture_in(
    txn: &mut Txn<'_>,
    part: &str,
    r: NodeId,
    image: &[u8],
    width: Option<i64>,
    height: Option<i64>,
) -> Result<InlineShape> {
    add_picture_named(txn, part, r, image, None, width, height)
}

/// [`add_picture_in`] with an explicit file name (`Image.from_file(path)`
/// names the picture after the file's base name).
pub(crate) fn add_picture_named(
    txn: &mut Txn<'_>,
    part: &str,
    r: NodeId,
    image: &[u8],
    filename: Option<&str>,
    width: Option<i64>,
    height: Option<i64>,
) -> Result<InlineShape> {
    let image = Image::from_blob(image)?;
    let (image_part, reused) =
        get_or_add_image_part(txn, &image, filename.map(crate::image::py_splitext_ext))?;
    let r_id = txn.relate_to(part, rt::IMAGE, RelTarget::Part(&image_part))?;
    let (cx, cy) = image.scaled_dimensions(width, height)?;
    let shape_id = next_id(txn.tree(part)?);
    for (value, what) in [(cx, "cx"), (cy, "cy")] {
        check_range(value, 0, 27_273_042_316_900, what)?;
    }
    if shape_id > u128::from(u32::MAX) {
        return Err(Error::invalid_argument(
            "value_out_of_range",
            format!("drawing id value must be in range 0 to 4294967295 inclusive, got {shape_id}"),
        ));
    }
    let tree = txn.tree_mut(part)?;
    let drawing = new_w(tree, r, "drawing")?;
    // Upstream names the picture after the reused part's `Image`, which is
    // `image.<ext>` for a part loaded from the package; a fresh part keeps
    // the file name it was created from.
    let name = match filename {
        Some(name) if !reused => name.to_string(),
        _ => image.filename(),
    };
    let inline = new_pic_inline(tree, drawing, shape_id, &r_id, &name, cx, cy)?;
    append(tree, drawing, inline)?;
    append(tree, r, drawing)?;
    Ok(InlineShape {
        width: cx,
        height: cy,
        r_id,
        image_part,
    })
}

// ------------------------------------------------------------- sections

pub(crate) fn section_properties(tree: &XmlTree) -> Result<Vec<NodeId>> {
    let body = body(tree)?;
    let mut out = Vec::new();
    for node in tree.child_elements(body) {
        if tree.is(node, W, "p") {
            if let Some(ppr) = w_child(tree, node, "pPr") {
                out.extend(w_children(tree, ppr, "sectPr"));
            }
        } else if tree.is(node, W, "sectPr") {
            out.push(node);
        }
    }
    Ok(out)
}

fn twips_attr(tree: &XmlTree, element: Option<NodeId>, local: &str) -> Result<Option<i64>> {
    match element.and_then(|e| w_attr(tree, e, local)) {
        None => Ok(None),
        Some(value) => parse_twips_measure(value).map(Some),
    }
}

/// `Document._block_width`: page width minus left and right margins of the
/// last section (8.5in / 1in defaults for missing or zero values).
pub(crate) fn block_width(tree: &XmlTree) -> Result<i64> {
    let Some(sect_pr) = section_properties(tree)?.last().copied() else {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "no_section_properties",
            "the document has no section properties (w:sectPr) to size a table from",
        ));
    };
    let pg_sz = w_child(tree, sect_pr, "pgSz");
    let pg_mar = w_child(tree, sect_pr, "pgMar");
    let or = |value: Option<i64>, default: i64| value.filter(|v| *v != 0).unwrap_or(default);
    let page_width = or(twips_attr(tree, pg_sz, "w")?, 7_772_400);
    let left = or(twips_attr(tree, pg_mar, "left")?, 914_400);
    let right = or(twips_attr(tree, pg_mar, "right")?, 914_400);
    Ok(page_width - left - right)
}

pub(crate) const SECT_PR_SEQ: &[&str] = &[
    "footnotePr",
    "endnotePr",
    "type",
    "pgSz",
    "pgMar",
    "paperSrc",
    "pgBorders",
    "lnNumType",
    "pgNumType",
    "cols",
    "formProt",
    "vAlign",
    "noEndnote",
    "titlePg",
    "textDirection",
    "bidi",
    "rtlGutter",
    "docGrid",
    "printerSettings",
    "sectPrChange",
];

// ------------------------------------------------------------- Document

impl Document {
    /// Body paragraphs (`Document.paragraphs`: direct `w:body/w:p`).
    pub fn paragraphs(&self) -> Result<Vec<Paragraph>> {
        let part = self.package().main_part_name().to_string();
        let tree = self.package().xml(&part)?;
        let body = body(tree)?;
        Ok(w_children(tree, body, "p")
            .map(|p| Paragraph {
                h: Handle::capture(self, &part, tree, p),
            })
            .collect())
    }

    /// Body tables (`Document.tables`: direct `w:body/w:tbl`).
    pub fn tables(&self) -> Result<Vec<Table>> {
        let part = self.package().main_part_name().to_string();
        let tree = self.package().xml(&part)?;
        let body = body(tree)?;
        Ok(w_children(tree, body, "tbl")
            .map(|t| Table {
                h: Handle::capture(self, &part, tree, t),
            })
            .collect())
    }

    /// Upstream `Document.add_paragraph(text, style)`: a paragraph at the end
    /// of the body (before `w:sectPr`) with `text` in one run and paragraph
    /// style `style` (UI name, e.g. `"Heading 1"`, or style id).
    pub fn add_paragraph(&mut self, text: &str, style: Option<&str>) -> Result<Paragraph> {
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let p = add_body_paragraph(txn.tree_mut(&part)?)?;
        fill_paragraph(&mut txn, &part, p, Some(text), style)?;
        let handle = staged_handle(&txn, &part, p)?;
        commit!(self, txn, Paragraph { h: handle })
    }

    /// Upstream `Document.add_heading(text, level)`: `Title` for level 0,
    /// `Heading <level>` for 1–9; other levels are invalid.
    pub fn add_heading(&mut self, text: &str, level: i64) -> Result<Paragraph> {
        if !(0..=9).contains(&level) {
            return Err(Error::invalid_argument(
                "invalid_heading_level",
                format!("level must be in range 0-9, got {level}"),
            ));
        }
        let style = if level == 0 {
            "Title".to_string()
        } else {
            format!("Heading {level}")
        };
        self.add_paragraph(text, Some(&style))
    }

    /// Upstream `Document.add_page_break()`: a paragraph holding one run with
    /// `<w:br w:type="page"/>`.
    pub fn add_page_break(&mut self) -> Result<Paragraph> {
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let tree = txn.tree_mut(&part)?;
        let p = add_body_paragraph(tree)?;
        let r = new_w(tree, p, "r")?;
        append(tree, p, r)?;
        add_break_xml(tree, r, BreakType::Page)?;
        let handle = staged_handle(&txn, &part, p)?;
        commit!(self, txn, Paragraph { h: handle })
    }

    /// Upstream `Document.add_table(rows, cols, style)`: a table whose
    /// columns share the block width of the last section evenly.
    pub fn add_table(&mut self, rows: usize, cols: usize, style: Option<&str>) -> Result<Table> {
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let tree = txn.tree_mut(&part)?;
        let width = block_width(tree)?;
        let body = body(tree)?;
        let tbl = new_tbl(tree, body, rows, cols, width)?;
        insert_before_successors(tree, body, tbl, &["sectPr"])?;
        let style_id = resolve_style_id(&mut txn, style, StyleType::Table)?;
        set_table_style_id(txn.tree_mut(&part)?, tbl, style_id.as_deref())?;
        let handle = staged_handle(&txn, &part, tbl)?;
        commit!(self, txn, Table { h: handle })
    }

    /// Upstream `Document.add_picture(image, width, height)`: the picture in
    /// a new paragraph's run, sized natively, by both dimensions, or scaled
    /// from one (EMU). Identical image bytes share one part (SHA-1).
    pub fn add_picture(
        &mut self,
        image: &[u8],
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<InlineShape> {
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let tree = txn.tree_mut(&part)?;
        let p = add_body_paragraph(tree)?;
        let r = new_w(tree, p, "r")?;
        append(tree, p, r)?;
        let shape = add_picture_in(&mut txn, &part, r, image, width, height)?;
        commit!(self, txn, shape)
    }

    /// Upstream `Document.add_section(start_type)`: the sentinel `w:sectPr`
    /// is copied (root attributes cleared) into a new last paragraph, loses
    /// its header/footer references and takes `start_type`.
    pub fn add_section(&mut self, start_type: SectionStart) -> Result<Section> {
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let tree = txn.tree_mut(&part)?;
        let body = body(tree)?;
        let sentinel = match w_child(tree, body, "sectPr") {
            Some(existing) => existing,
            None => {
                let sect = new_w(tree, body, "sectPr")?;
                append(tree, body, sect)?;
                sect
            }
        };
        let p = add_body_paragraph(tree)?;
        let clone = tree.deep_clone(sentinel);
        let attrs: Vec<(Option<String>, String)> = tree
            .element(clone)
            .map(|e| {
                e.attrs
                    .iter()
                    .map(|a| {
                        (
                            a.ns.map(|n| tree.uri(n).to_string()),
                            a.name.local.to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (ns, local) in attrs {
            tree.remove_attr(clone, ns.as_deref(), &local);
        }
        let ppr = get_or_add_ppr(tree, p)?;
        remove_all(tree, ppr, "sectPr")?;
        insert_before_successors(tree, ppr, clone, successors(PPR_SEQ, "sectPr"))?;
        let references: Vec<NodeId> = tree
            .child_elements(sentinel)
            .filter(|n| tree.is(*n, W, "headerReference") || tree.is(*n, W, "footerReference"))
            .collect();
        for reference in references {
            remove(tree, reference)?;
        }
        if start_type == SectionStart::NewPage {
            remove_all(tree, sentinel, "type")?;
        } else {
            let kind = get_or_add(tree, sentinel, "type", SECT_PR_SEQ)?;
            set_w(tree, kind, "val", start_type.as_str())?;
        }
        commit!(self, txn, Section { start_type })
    }
}

/// `CT_Tbl.new_tbl(rows, cols, width)` (`_tbl_xml`): `width // cols` per
/// column, in twips, on every `w:gridCol` and `w:tcW`.
pub(crate) fn new_tbl(
    tree: &mut XmlTree,
    scope: NodeId,
    rows: usize,
    cols: usize,
    width: i64,
) -> Result<NodeId> {
    let col_width = if cols > 0 {
        width.div_euclid(cols as i64)
    } else {
        0
    };
    let col_twips = twips(col_width).to_string();
    let tbl = new_w(tree, scope, "tbl")?;
    let pr = new_w(tree, tbl, "tblPr")?;
    append(tree, tbl, pr)?;
    let tbl_w = new_w(tree, pr, "tblW")?;
    set_w(tree, tbl_w, "type", "auto")?;
    set_w(tree, tbl_w, "w", "0")?;
    append(tree, pr, tbl_w)?;
    let look = new_w(tree, pr, "tblLook")?;
    for (name, value) in [
        ("firstColumn", "1"),
        ("firstRow", "1"),
        ("lastColumn", "0"),
        ("lastRow", "0"),
        ("noHBand", "0"),
        ("noVBand", "1"),
        ("val", "04A0"),
    ] {
        set_w(tree, look, name, value)?;
    }
    append(tree, pr, look)?;
    // Upstream parses a text template with `remove_blank_text`, which keeps
    // the indentation of an element that ends up with no children.
    let blank = |tree: &mut XmlTree, parent: NodeId| -> Result<()> {
        if cols == 0 {
            let text = tree.create_text("\n  ");
            append(tree, parent, text)?;
        }
        Ok(())
    };
    let grid = new_w(tree, tbl, "tblGrid")?;
    append(tree, tbl, grid)?;
    blank(tree, grid)?;
    for _ in 0..cols {
        let col = new_w(tree, grid, "gridCol")?;
        set_w(tree, col, "w", &col_twips)?;
        append(tree, grid, col)?;
    }
    for _ in 0..rows {
        let tr = new_w(tree, tbl, "tr")?;
        append(tree, tbl, tr)?;
        blank(tree, tr)?;
        for _ in 0..cols {
            let tc = new_w(tree, tr, "tc")?;
            append(tree, tr, tc)?;
            let tc_pr = new_w(tree, tc, "tcPr")?;
            append(tree, tc, tc_pr)?;
            let tc_w = new_w(tree, tc_pr, "tcW")?;
            set_w(tree, tc_w, "type", "dxa")?;
            set_w(tree, tc_w, "w", &col_twips)?;
            append(tree, tc_pr, tc_w)?;
            let p = new_w(tree, tc, "p")?;
            append(tree, tc, p)?;
        }
    }
    Ok(tbl)
}

fn add_break_xml(tree: &mut XmlTree, r: NodeId, kind: BreakType) -> Result<()> {
    let (type_, clear) = match kind {
        BreakType::Line => (None, None),
        BreakType::Page => (Some("page"), None),
        BreakType::Column => (Some("column"), None),
        BreakType::LineClearLeft => (Some("textWrapping"), Some("left")),
        BreakType::LineClearRight => (Some("textWrapping"), Some("right")),
        BreakType::LineClearAll => (Some("textWrapping"), Some("all")),
    };
    let br = new_w(tree, r, "br")?;
    append(tree, r, br)?;
    // `w:type` defaults to "textWrapping": assigning the default writes nothing.
    if let Some(type_) = type_.filter(|t| *t != "textWrapping") {
        set_w(tree, br, "type", type_)?;
    }
    if let Some(clear) = clear {
        set_w(tree, br, "clear", clear)?;
    }
    Ok(())
}

const TBLPR_SEQ: &[&str] = &[
    "tblStyle",
    "tblpPr",
    "tblOverlap",
    "bidiVisual",
    "tblStyleRowBandSize",
    "tblStyleColBandSize",
    "tblW",
    "jc",
    "tblCellSpacing",
    "tblInd",
    "tblBorders",
    "shd",
    "tblLayout",
    "tblCellMar",
    "tblLook",
    "tblCaption",
    "tblDescription",
    "tblPrChange",
];

const TCPR_SEQ: &[&str] = &[
    "cnfStyle",
    "tcW",
    "gridSpan",
    "hMerge",
    "vMerge",
    "tcBorders",
    "shd",
    "noWrap",
    "tcMar",
    "textDirection",
    "tcFitText",
    "vAlign",
    "hideMark",
    "headers",
    "cellIns",
    "cellDel",
    "cellMerge",
    "tcPrChange",
];

fn tbl_pr(tree: &XmlTree, tbl: NodeId) -> Result<NodeId> {
    w_child(tree, tbl, "tblPr").ok_or_else(|| {
        Error::refusal(
            RefusalKind::UnsupportedStructure,
            "missing_table_properties",
            "required w:tblPr child element not present",
        )
    })
}

pub(crate) fn set_table_style_id(
    tree: &mut XmlTree,
    tbl: NodeId,
    style_id: Option<&str>,
) -> Result<()> {
    let pr = tbl_pr(tree, tbl)?;
    remove_all(tree, pr, "tblStyle")?;
    if let Some(id) = style_id {
        let style = add_w(tree, pr, "tblStyle", successors(TBLPR_SEQ, "tblStyle"))?;
        set_w(tree, style, "val", id)?;
    }
    Ok(())
}

/// `CT_Tc.width = emu` (`w:tcPr/w:tcW` with `w:type="dxa"`).
fn set_cell_width(tree: &mut XmlTree, tc: NodeId, emu: i64) -> Result<()> {
    let tc_pr = get_or_add_first(tree, tc, "tcPr")?;
    let tc_w = get_or_add(tree, tc_pr, "tcW", TCPR_SEQ)?;
    set_w(tree, tc_w, "type", "dxa")?;
    let value = twips(emu);
    check_range(value, i32::MIN as i128, i32::MAX as i128, "w:w")?;
    set_w(tree, tc_w, "w", &value.to_string())
}

/// `CT_Row.add_tc()` (`<w:tc><w:p/></w:tc>` appended).
fn add_tc(tree: &mut XmlTree, tr: NodeId) -> Result<NodeId> {
    let tc = new_w(tree, tr, "tc")?;
    let p = new_w(tree, tc, "p")?;
    append(tree, tc, p)?;
    append(tree, tr, tc)?;
    Ok(tc)
}

fn tbl_grid(tree: &XmlTree, tbl: NodeId) -> Result<NodeId> {
    w_child(tree, tbl, "tblGrid").ok_or_else(|| {
        Error::refusal(
            RefusalKind::UnsupportedStructure,
            "missing_table_grid",
            "required w:tblGrid child element not present",
        )
    })
}

/// Upstream `Table._cells`: the layout grid with spans repeated.
fn table_cells(tree: &XmlTree, tbl: NodeId) -> Result<Vec<NodeId>> {
    let col_count = w_children(tree, tbl_grid(tree, tbl)?, "gridCol").count() as i64;
    let mut cells: Vec<NodeId> = Vec::new();
    let index_error = || {
        Error::invalid_argument(
            "cell_out_of_range",
            "list index out of range while laying out the table grid",
        )
    };
    for tr in w_children(tree, tbl, "tr") {
        for tc in w_children(tree, tr, "tc") {
            let tc_pr = w_child(tree, tc, "tcPr");
            let span = match tc_pr.and_then(|pr| w_child(tree, pr, "gridSpan")) {
                None => 1,
                Some(grid_span) => {
                    let raw = w_attr(tree, grid_span, "val").unwrap_or("");
                    raw.trim().parse::<i64>().map_err(|_| {
                        Error::invalid_argument(
                            "invalid_grid_span",
                            format!("invalid w:gridSpan value {raw:?}"),
                        )
                    })?
                }
            };
            let v_merge = tc_pr
                .and_then(|pr| w_child(tree, pr, "vMerge"))
                .map(|vm| w_attr(tree, vm, "val").unwrap_or("continue").to_string());
            for span_index in 0..span.max(0) {
                if v_merge.as_deref() == Some("continue") {
                    let len = cells.len() as i64;
                    let index = if col_count == 0 { 0 } else { len - col_count };
                    if index < 0 || index >= len {
                        return Err(index_error());
                    }
                    cells.push(cells[index as usize]);
                } else if span_index > 0 {
                    let last = *cells.last().ok_or_else(index_error)?;
                    cells.push(last);
                } else {
                    cells.push(tc);
                }
            }
        }
    }
    Ok(cells)
}

// ------------------------------------------------------------- Paragraph

/// Paragraph alignment tokens (`WD_PARAGRAPH_ALIGNMENT` XML values).
pub const ALIGNMENTS: &[&str] = &[
    "left",
    "center",
    "right",
    "both",
    "distribute",
    "mediumKashida",
    "highKashida",
    "lowKashida",
    "thaiDistribute",
];

/// Line spacing (`ParagraphFormat.line_spacing`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum LineSpacing {
    /// Multiple of single spacing (`float` upstream), `w:lineRule="auto"`.
    Multiple(f64),
    /// Absolute line height in EMU (`Length` upstream), `exact` unless the
    /// paragraph already says `atLeast`.
    Exactly(i64),
}

/// `WD_LINE_SPACING` (`ParagraphFormat.line_spacing_rule`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum LineSpacingRule {
    /// `SINGLE` (240 twips, multiple).
    Single,
    /// `ONE_POINT_FIVE` (360 twips, multiple).
    OnePointFive,
    /// `DOUBLE` (480 twips, multiple).
    Double,
    /// `AT_LEAST` (`atLeast`).
    AtLeast,
    /// `EXACTLY` (`exact`).
    Exactly,
    /// `MULTIPLE` (`auto`).
    Multiple,
}

impl LineSpacingRule {
    /// Upstream member name (`SINGLE`, `ONE_POINT_FIVE`, …).
    pub fn name(self) -> &'static str {
        match self {
            LineSpacingRule::Single => "SINGLE",
            LineSpacingRule::OnePointFive => "ONE_POINT_FIVE",
            LineSpacingRule::Double => "DOUBLE",
            LineSpacingRule::AtLeast => "AT_LEAST",
            LineSpacingRule::Exactly => "EXACTLY",
            LineSpacingRule::Multiple => "MULTIPLE",
        }
    }

    /// From an upstream member name.
    pub fn from_name(name: &str) -> Result<LineSpacingRule> {
        Ok(match name {
            "SINGLE" => LineSpacingRule::Single,
            "ONE_POINT_FIVE" => LineSpacingRule::OnePointFive,
            "DOUBLE" => LineSpacingRule::Double,
            "AT_LEAST" => LineSpacingRule::AtLeast,
            "EXACTLY" => LineSpacingRule::Exactly,
            "MULTIPLE" => LineSpacingRule::Multiple,
            other => {
                return Err(Error::invalid_argument(
                    "invalid_line_spacing_rule",
                    format!("WD_LINE_SPACING has no member {other:?}"),
                ));
            }
        })
    }

    fn from_xml(token: &str) -> Result<LineSpacingRule> {
        Ok(match token {
            "atLeast" => LineSpacingRule::AtLeast,
            "exact" => LineSpacingRule::Exactly,
            "auto" => LineSpacingRule::Multiple,
            other => {
                return Err(Error::invalid_argument(
                    "invalid_line_spacing_rule",
                    format!("WD_LINE_SPACING has no XML mapping for '{other}'"),
                ));
            }
        })
    }
}

/// A batch of `ParagraphFormat` property assignments. `None` leaves a
/// property alone; `Some(None)` assigns `None` (inherit). Applied in field
/// order, each exactly like the upstream setter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParagraphFormat {
    /// `alignment` (token from [`ALIGNMENTS`]).
    pub alignment: Option<Option<String>>,
    /// `left_indent` (EMU).
    pub left_indent: Option<Option<i64>>,
    /// `right_indent` (EMU).
    pub right_indent: Option<Option<i64>>,
    /// `first_line_indent` (EMU; negative is a hanging indent).
    pub first_line_indent: Option<Option<i64>>,
    /// `space_before` (EMU).
    pub space_before: Option<Option<i64>>,
    /// `space_after` (EMU).
    pub space_after: Option<Option<i64>>,
    /// `line_spacing`.
    pub line_spacing: Option<Option<LineSpacing>>,
    /// `line_spacing_rule`.
    pub line_spacing_rule: Option<Option<LineSpacingRule>>,
    /// `keep_together` (`w:keepLines`).
    pub keep_together: Option<Option<bool>>,
    /// `keep_with_next` (`w:keepNext`).
    pub keep_with_next: Option<Option<bool>>,
    /// `page_break_before`.
    pub page_break_before: Option<Option<bool>>,
    /// `widow_control`.
    pub widow_control: Option<Option<bool>>,
}

/// The direct paragraph formatting of a paragraph or style (upstream
/// `ParagraphFormat` getters; `None` means inherited).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ParagraphFormatValues {
    /// `alignment` (XML token).
    pub alignment: Option<String>,
    /// `left_indent` (EMU).
    pub left_indent: Option<i64>,
    /// `right_indent` (EMU).
    pub right_indent: Option<i64>,
    /// `first_line_indent` (EMU; hanging indents are negative).
    pub first_line_indent: Option<i64>,
    /// `space_before` (EMU).
    pub space_before: Option<i64>,
    /// `space_after` (EMU).
    pub space_after: Option<i64>,
    /// `line_spacing`.
    pub line_spacing: Option<LineSpacing>,
    /// `line_spacing_rule`.
    pub line_spacing_rule: Option<LineSpacingRule>,
    /// `keep_together`.
    pub keep_together: Option<bool>,
    /// `keep_with_next`.
    pub keep_with_next: Option<bool>,
    /// `page_break_before`.
    pub page_break_before: Option<bool>,
    /// `widow_control`.
    pub widow_control: Option<bool>,
}

fn validate_alignment(value: &str) -> Result<()> {
    if ALIGNMENTS.contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_argument(
            "invalid_alignment",
            format!("WD_PARAGRAPH_ALIGNMENT has no XML mapping for {value:?}"),
        ))
    }
}

/// `CT_PPr.jc_val = value` (after `get_or_add_pPr`).
fn set_alignment_xml(tree: &mut XmlTree, p: NodeId, host: Host, value: Option<&str>) -> Result<()> {
    let ppr = host_ppr(tree, p, host)?;
    match value {
        None => remove_all(tree, ppr, "jc"),
        Some(token) => {
            let jc = get_or_add(tree, ppr, "jc", PPR_SEQ)?;
            set_w(tree, jc, "val", token)
        }
    }
}

/// CT_OnOff `val`: `True` is the default (no attribute), `False` is `"0"`.
pub(crate) fn set_on_off(tree: &mut XmlTree, element: NodeId, value: bool) -> Result<()> {
    if value {
        tree.remove_attr(element, Some(W), "val");
        Ok(())
    } else {
        set_w(tree, element, "val", "0")
    }
}

/// `CT_OnOff.val` of `element` (`True` when `w:val` is absent).
pub(crate) fn on_off_val(tree: &XmlTree, element: NodeId) -> Result<bool> {
    match w_attr(tree, element, "val") {
        None => Ok(true),
        Some(value) => on_off(value),
    }
}

fn set_ppr_on_off(tree: &mut XmlTree, ppr: NodeId, local: &str, value: Option<bool>) -> Result<()> {
    match value {
        None => remove_all(tree, ppr, local),
        Some(flag) => {
            let element = get_or_add(tree, ppr, local, PPR_SEQ)?;
            set_on_off(tree, element, flag)
        }
    }
}

/// OptionalAttribute assignment: `None` removes, else sets.
pub(crate) fn set_opt(
    tree: &mut XmlTree,
    element: NodeId,
    local: &str,
    value: Option<String>,
) -> Result<()> {
    match value {
        None => {
            tree.remove_attr(element, Some(W), local);
            Ok(())
        }
        Some(value) => set_w(tree, element, local, &value),
    }
}

/// `CT_PPr.spacing_line = value` (EMU; `None` removes).
fn set_spacing_line(tree: &mut XmlTree, ppr: NodeId, line: Option<i64>) -> Result<()> {
    if line.is_none() && w_child(tree, ppr, "spacing").is_none() {
        return Ok(());
    }
    let spacing = get_or_add(tree, ppr, "spacing", PPR_SEQ)?;
    let xml = line
        .map(|emu| signed_twips_measure(emu, "w:line"))
        .transpose()?;
    set_opt(tree, spacing, "line", xml)
}

/// `CT_PPr.spacing_lineRule = value` (XML token; `None` removes).
fn set_spacing_line_rule(tree: &mut XmlTree, ppr: NodeId, rule: Option<&str>) -> Result<()> {
    if rule.is_none() && w_child(tree, ppr, "spacing").is_none() {
        return Ok(());
    }
    let spacing = get_or_add(tree, ppr, "spacing", PPR_SEQ)?;
    set_opt(tree, spacing, "lineRule", rule.map(str::to_string))
}

/// Upstream `ParagraphFormat` setters on `element` (`w:p` or `w:style`),
/// applied in [`ParagraphFormat`] field order.
pub(crate) fn apply_format(
    tree: &mut XmlTree,
    p: NodeId,
    host: Host,
    format: &ParagraphFormat,
) -> Result<()> {
    if let Some(value) = &format.alignment {
        if let Some(token) = value {
            validate_alignment(token)?;
        }
        set_alignment_xml(tree, p, host, value.as_deref())?;
    }
    for (value, local) in [(format.left_indent, "left"), (format.right_indent, "right")] {
        if let Some(value) = value {
            let ppr = host_ppr(tree, p, host)?;
            if value.is_none() && w_child(tree, ppr, "ind").is_none() {
                continue;
            }
            let ind = get_or_add(tree, ppr, "ind", PPR_SEQ)?;
            let xml = value
                .map(|emu| signed_twips_measure(emu, "w:ind"))
                .transpose()?;
            set_opt(tree, ind, local, xml)?;
        }
    }
    if let Some(value) = format.first_line_indent {
        let ppr = host_ppr(tree, p, host)?;
        if !(value.is_none() && w_child(tree, ppr, "ind").is_none()) {
            let ind = get_or_add(tree, ppr, "ind", PPR_SEQ)?;
            tree.remove_attr(ind, Some(W), "firstLine");
            tree.remove_attr(ind, Some(W), "hanging");
            match value {
                None => {}
                Some(emu) if emu < 0 => {
                    set_w(tree, ind, "hanging", &twips_measure(-emu, "w:hanging")?)?
                }
                Some(emu) => set_w(tree, ind, "firstLine", &twips_measure(emu, "w:firstLine")?)?,
            }
        }
    }
    for (value, local) in [
        (format.space_before, "before"),
        (format.space_after, "after"),
    ] {
        if let Some(value) = value {
            let ppr = host_ppr(tree, p, host)?;
            if value.is_none() && w_child(tree, ppr, "spacing").is_none() {
                continue;
            }
            let spacing = get_or_add(tree, ppr, "spacing", PPR_SEQ)?;
            let xml = value
                .map(|emu| twips_measure(emu, "w:spacing"))
                .transpose()?;
            set_opt(tree, spacing, local, xml)?;
        }
    }
    if let Some(value) = format.line_spacing {
        let ppr = host_ppr(tree, p, host)?;
        match value {
            None => {
                set_spacing_line(tree, ppr, None)?;
                set_spacing_line_rule(tree, ppr, None)?;
            }
            Some(LineSpacing::Exactly(emu)) => {
                set_spacing_line(tree, ppr, Some(emu))?;
                // `if pPr.spacing_lineRule != AT_LEAST: … = EXACTLY`
                if spacing_line_rule(tree, ppr)? != Some(LineSpacingRule::AtLeast) {
                    set_spacing_line_rule(tree, ppr, Some("exact"))?;
                }
            }
            Some(LineSpacing::Multiple(lines)) => {
                // `Emu(value * Twips(240))`: int() truncation of the float.
                let emu = lines * 152_400.0;
                if !emu.is_finite() {
                    return Err(Error::invalid_argument(
                        "invalid_line_spacing",
                        format!("cannot convert line spacing {lines} to an integer length"),
                    ));
                }
                set_spacing_line(tree, ppr, Some(emu as i64))?;
                set_spacing_line_rule(tree, ppr, Some("auto"))?;
            }
        }
    }
    if let Some(value) = format.line_spacing_rule {
        let ppr = host_ppr(tree, p, host)?;
        match value {
            Some(LineSpacingRule::Single) => {
                set_spacing_line(tree, ppr, Some(240 * 635))?;
                set_spacing_line_rule(tree, ppr, Some("auto"))?;
            }
            Some(LineSpacingRule::OnePointFive) => {
                set_spacing_line(tree, ppr, Some(360 * 635))?;
                set_spacing_line_rule(tree, ppr, Some("auto"))?;
            }
            Some(LineSpacingRule::Double) => {
                set_spacing_line(tree, ppr, Some(480 * 635))?;
                set_spacing_line_rule(tree, ppr, Some("auto"))?;
            }
            Some(LineSpacingRule::AtLeast) => set_spacing_line_rule(tree, ppr, Some("atLeast"))?,
            Some(LineSpacingRule::Exactly) => set_spacing_line_rule(tree, ppr, Some("exact"))?,
            Some(LineSpacingRule::Multiple) => set_spacing_line_rule(tree, ppr, Some("auto"))?,
            None => set_spacing_line_rule(tree, ppr, None)?,
        }
    }
    for (value, local) in [
        (format.keep_together, "keepLines"),
        (format.keep_with_next, "keepNext"),
        (format.page_break_before, "pageBreakBefore"),
        (format.widow_control, "widowControl"),
    ] {
        if let Some(value) = value {
            let ppr = host_ppr(tree, p, host)?;
            set_ppr_on_off(tree, ppr, local, value)?;
        }
    }
    Ok(())
}

/// `ST_SignedTwipsMeasure.convert_from_xml` → EMU (`int(round(float(s)))`
/// twips, or a universal measure).
pub(crate) fn parse_signed_twips(value: &str) -> Result<i64> {
    if value.contains('i') || value.contains('m') || value.contains('p') {
        return parse_universal_measure(value);
    }
    let number: f64 = py_float(value).ok_or_else(|| {
        Error::invalid_argument(
            "invalid_measure",
            format!("could not convert string to float: {value:?}"),
        )
    })?;
    if !number.is_finite() {
        return Err(Error::invalid_argument(
            "invalid_measure",
            format!("cannot convert float {value:?} to integer"),
        ));
    }
    Ok((number.round_ties_even() as i64) * 635)
}

fn opt_twips_attr(
    tree: &XmlTree,
    element: Option<NodeId>,
    local: &str,
    signed: bool,
) -> Result<Option<i64>> {
    match element.and_then(|e| w_attr(tree, e, local)) {
        None => Ok(None),
        Some(value) if signed => parse_signed_twips(value).map(Some),
        Some(value) => parse_twips_measure(value).map(Some),
    }
}

/// `CT_PPr.spacing_lineRule` (MULTIPLE when only `w:line` is present).
fn spacing_line_rule(tree: &XmlTree, ppr: NodeId) -> Result<Option<LineSpacingRule>> {
    let Some(spacing) = w_child(tree, ppr, "spacing") else {
        return Ok(None);
    };
    match w_attr(tree, spacing, "lineRule") {
        Some(token) => LineSpacingRule::from_xml(token).map(Some),
        None if w_attr(tree, spacing, "line").is_some() => Ok(Some(LineSpacingRule::Multiple)),
        None => Ok(None),
    }
}

fn ppr_on_off(tree: &XmlTree, ppr: NodeId, local: &str) -> Result<Option<bool>> {
    match w_child(tree, ppr, local) {
        None => Ok(None),
        Some(element) => on_off_val(tree, element).map(Some),
    }
}

/// Upstream `ParagraphFormat` getters for `element` (`w:p` or `w:style`).
pub(crate) fn read_format(tree: &XmlTree, element: NodeId) -> Result<ParagraphFormatValues> {
    let Some(ppr) = w_child(tree, element, "pPr") else {
        return Ok(ParagraphFormatValues::default());
    };
    let ind = w_child(tree, ppr, "ind");
    let spacing = w_child(tree, ppr, "spacing");
    let alignment = match w_child(tree, ppr, "jc").and_then(|jc| w_attr(tree, jc, "val")) {
        None => None,
        Some(token) => {
            validate_alignment(token)?;
            Some(token.to_string())
        }
    };
    let first_line_indent = match opt_twips_attr(tree, ind, "hanging", false)? {
        Some(hanging) => Some(-hanging),
        None => opt_twips_attr(tree, ind, "firstLine", false)?,
    };
    let line = opt_twips_attr(tree, spacing, "line", true)?;
    let rule = spacing_line_rule(tree, ppr)?;
    let line_spacing = line.map(|emu| {
        if rule == Some(LineSpacingRule::Multiple) {
            LineSpacing::Multiple(emu as f64 / 152_400.0)
        } else {
            LineSpacing::Exactly(emu)
        }
    });
    let line_spacing_rule = match (rule, line) {
        (Some(LineSpacingRule::Multiple), Some(152_400)) => Some(LineSpacingRule::Single),
        (Some(LineSpacingRule::Multiple), Some(228_600)) => Some(LineSpacingRule::OnePointFive),
        (Some(LineSpacingRule::Multiple), Some(304_800)) => Some(LineSpacingRule::Double),
        (rule, _) => rule,
    };
    Ok(ParagraphFormatValues {
        alignment,
        left_indent: opt_twips_attr(tree, ind, "left", true)?,
        right_indent: opt_twips_attr(tree, ind, "right", true)?,
        first_line_indent,
        space_before: opt_twips_attr(tree, spacing, "before", false)?,
        space_after: opt_twips_attr(tree, spacing, "after", false)?,
        line_spacing,
        line_spacing_rule,
        keep_together: ppr_on_off(tree, ppr, "keepLines")?,
        keep_with_next: ppr_on_off(tree, ppr, "keepNext")?,
        page_break_before: ppr_on_off(tree, ppr, "pageBreakBefore")?,
        widow_control: ppr_on_off(tree, ppr, "widowControl")?,
    })
}
impl Paragraph {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "p", "paragraph")
    }

    /// Package member holding the paragraph.
    pub fn part(&self) -> &str {
        &self.h.part
    }

    /// Upstream `Paragraph.text`.
    pub fn text(&self, doc: &Document) -> Result<String> {
        Ok(paragraph_text(self.live(doc)?, self.h.element))
    }

    /// `w:pPr/w:pStyle/@w:val` (upstream `CT_P.style`).
    pub fn style_id(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        Ok(w_child(tree, self.h.element, "pPr")
            .and_then(|ppr| w_child(tree, ppr, "pStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string))
    }

    /// Upstream `Paragraph.style`: the paragraph style with the paragraph's
    /// style id, else the default paragraph style. Reading never creates
    /// the styles part.
    pub fn style(&self, doc: &Document) -> Result<Option<crate::styles::Style>> {
        let id = self.style_id(doc)?;
        style_of(doc, id.as_deref(), crate::styles::StyleType::Paragraph)
    }

    /// Direct `w:r` children (upstream `Paragraph.runs`).
    pub fn runs(&self, doc: &Document) -> Result<Vec<Run>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "r")
            .map(|r| Run {
                h: Handle::capture(doc, &self.h.part, tree, r),
            })
            .collect())
    }

    /// Upstream `Paragraph.add_run(text, style)`: a run appended at the end
    /// (character style by UI name or id; empty text or style is ignored).
    pub fn add_run(
        &self,
        doc: &mut Document,
        text: Option<&str>,
        style: Option<&str>,
    ) -> Result<Run> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let r = add_run_in(&mut txn, &self.h.part, self.h.element, text, style)?;
        let handle = staged_handle(&txn, &self.h.part, r)?;
        commit!(doc, txn, Run { h: handle })
    }

    /// Upstream `Paragraph.insert_paragraph_before(text, style)`.
    pub fn insert_paragraph_before(
        &self,
        doc: &mut Document,
        text: Option<&str>,
        style: Option<&str>,
    ) -> Result<Paragraph> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let parent = tree
            .parent(self.h.element)
            .ok_or_else(|| Error::internal("detached_paragraph", "live paragraph has no parent"))?;
        let index = tree
            .index_in_parent(self.h.element)
            .ok_or_else(|| Error::internal("detached_paragraph", "live paragraph has no index"))?;
        let p = new_w(tree, parent, "p")?;
        tree.insert(parent, index, p)?;
        fill_paragraph(&mut txn, &self.h.part, p, text, style)?;
        let handle = staged_handle(&txn, &self.h.part, p)?;
        commit!(doc, txn, Paragraph { h: handle })
    }

    /// Upstream `Paragraph.text = text`: content replaced by one run;
    /// paragraph properties kept.
    pub fn set_text(&self, doc: &mut Document, text: Option<&str>) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        clear_except(txn.tree_mut(&self.h.part)?, self.h.element, "pPr")?;
        add_run_in(&mut txn, &self.h.part, self.h.element, text, None)?;
        commit!(doc, txn, ())
    }

    /// Upstream `Paragraph.clear()`: every child element but `w:pPr` goes.
    pub fn clear(&self, doc: &mut Document) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        clear_except(txn.tree_mut(&self.h.part)?, self.h.element, "pPr")?;
        commit!(doc, txn, ())
    }

    /// Upstream `Paragraph.style = style` (UI name or id; `None` removes).
    pub fn set_style(&self, doc: &mut Document, style: Option<&str>) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let style_id = resolve_style_id(&mut txn, style, StyleType::Paragraph)?;
        set_paragraph_style_id(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            style_id.as_deref(),
        )?;
        commit!(doc, txn, ())
    }

    /// Upstream `Paragraph.alignment = value` (token from [`ALIGNMENTS`]).
    pub fn set_alignment(&self, doc: &mut Document, value: Option<&str>) -> Result<()> {
        self.live(doc)?;
        if let Some(token) = value {
            validate_alignment(token)?;
        }
        let mut txn = Txn::new(doc);
        set_alignment_xml(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            Host::Text,
            value,
        )?;
        commit!(doc, txn, ())
    }

    /// Upstream `ParagraphFormat` getters (direct formatting only).
    pub fn format(&self, doc: &Document) -> Result<ParagraphFormatValues> {
        read_format(self.live(doc)?, self.h.element)
    }

    /// Upstream `ParagraphFormat` setters, applied atomically.
    pub fn set_format(&self, doc: &mut Document, format: &ParagraphFormat) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        apply_format(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            Host::Text,
            format,
        )?;
        commit!(doc, txn, ())
    }
}

// ------------------------------------------------------------------- Run

/// Underline value (`Font.underline`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Underline {
    /// `True` (single) / `False` (none).
    Bool(bool),
    /// A `WD_UNDERLINE` XML token such as `"double"` or `"wave"`.
    Style(String),
}

/// `WD_UNDERLINE` XML tokens.
pub const UNDERLINES: &[&str] = &[
    "none",
    "single",
    "words",
    "double",
    "dotted",
    "thick",
    "dash",
    "dotDash",
    "dotDotDash",
    "wave",
    "dottedHeavy",
    "dashedHeavy",
    "dashDotHeavy",
    "dashDotDotHeavy",
    "wavyHeavy",
    "dashLong",
    "wavyDouble",
    "dashLongHeavy",
];

/// `WD_COLOR_INDEX` XML tokens accepted for highlight.
pub const HIGHLIGHTS: &[&str] = &[
    "default",
    "black",
    "blue",
    "green",
    "darkBlue",
    "darkRed",
    "darkYellow",
    "lightGray",
    "darkGray",
    "darkGreen",
    "magenta",
    "red",
    "darkCyan",
    "cyan",
    "darkMagenta",
    "white",
    "yellow",
];

/// Tri-state boolean `Font` properties and their `w:rPr` child.
pub const FONT_FLAGS: &[(&str, &str)] = &[
    ("all_caps", "caps"),
    ("bold", "b"),
    ("complex_script", "cs"),
    ("cs_bold", "bCs"),
    ("cs_italic", "iCs"),
    ("double_strike", "dstrike"),
    ("emboss", "emboss"),
    ("hidden", "vanish"),
    ("italic", "i"),
    ("imprint", "imprint"),
    ("math", "oMath"),
    ("no_proof", "noProof"),
    ("outline", "outline"),
    ("rtl", "rtl"),
    ("shadow", "shadow"),
    ("small_caps", "smallCaps"),
    ("snap_to_grid", "snapToGrid"),
    ("spec_vanish", "specVanish"),
    ("strike", "strike"),
    ("web_hidden", "webHidden"),
];

/// `MSO_THEME_COLOR` XML tokens.
pub const THEME_COLORS: &[&str] = &[
    "accent1",
    "accent2",
    "accent3",
    "accent4",
    "accent5",
    "accent6",
    "background1",
    "background2",
    "dark1",
    "dark2",
    "followedHyperlink",
    "hyperlink",
    "light1",
    "light2",
    "text1",
    "text2",
];

/// One `Font` property assignment (applied in sequence by
/// [`Run::set_font`]).
#[derive(Debug, Clone, PartialEq)]
pub enum FontChange {
    /// A tri-state flag by upstream property name (see [`FONT_FLAGS`]).
    Flag(String, Option<bool>),
    /// `name` (typeface; `w:rFonts` ascii + hAnsi).
    Name(Option<String>),
    /// `size` in EMU (`w:sz` half-points).
    Size(Option<i64>),
    /// `underline`.
    Underline(Option<Underline>),
    /// `color.rgb` as `(r, g, b)`.
    Color(Option<(u8, u8, u8)>),
    /// `color.theme_color` (token from [`THEME_COLORS`]).
    ThemeColor(Option<String>),
    /// `highlight_color` (token from [`HIGHLIGHTS`]).
    Highlight(Option<String>),
    /// `subscript`.
    Subscript(Option<bool>),
    /// `superscript`.
    Superscript(Option<bool>),
}

/// `Font.color` (upstream `ColorFormat` getters).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ColorValues {
    /// `type`: `"RGB"`, `"THEME"` or `"AUTO"` (`None` without `w:color`).
    pub color_type: Option<String>,
    /// `rgb` as `RRGGBB` (`None` for `auto` or no color).
    pub rgb: Option<String>,
    /// `theme_color` token.
    pub theme_color: Option<String>,
}

/// The direct character formatting of a run or style (upstream `Font`
/// getters; `None` means inherited).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct FontValues {
    /// Tri-state flags by upstream property name, in [`FONT_FLAGS`] order.
    pub flags: Vec<(String, Option<bool>)>,
    /// `name` (`w:rFonts/@w:ascii`).
    pub name: Option<String>,
    /// `size` in EMU.
    pub size: Option<i64>,
    /// `underline`.
    pub underline: Option<Underline>,
    /// `color`.
    pub color: ColorValues,
    /// `highlight_color` token.
    pub highlight: Option<String>,
    /// `subscript`.
    pub subscript: Option<bool>,
    /// `superscript`.
    pub superscript: Option<bool>,
}

fn rpr_get_or_add(tree: &mut XmlTree, rpr: NodeId, local: &str) -> Result<NodeId> {
    get_or_add(tree, rpr, local, RPR_SEQ)
}

/// Validate the value of one font change before anything is written.
fn validate_font_change(change: &FontChange) -> Result<()> {
    match change {
        FontChange::Flag(name, _) if !FONT_FLAGS.iter().any(|(n, _)| n == name) => {
            Err(Error::invalid_argument(
                "unknown_font_property",
                format!("Font has no property {name:?}"),
            ))
        }
        FontChange::Underline(Some(Underline::Style(token)))
            if !UNDERLINES.contains(&token.as_str()) =>
        {
            Err(Error::invalid_argument(
                "invalid_underline",
                format!("WD_UNDERLINE has no XML mapping for {token:?}"),
            ))
        }
        FontChange::ThemeColor(Some(token)) if !THEME_COLORS.contains(&token.as_str()) => {
            Err(Error::invalid_argument(
                "invalid_theme_color",
                format!("MSO_THEME_COLOR has no XML mapping for {token:?}"),
            ))
        }
        FontChange::Highlight(Some(token)) if !HIGHLIGHTS.contains(&token.as_str()) => {
            Err(Error::invalid_argument(
                "invalid_highlight",
                format!("WD_COLOR_INDEX has no XML mapping for {token:?}"),
            ))
        }
        _ => Ok(()),
    }
}

/// Upstream `Font` setter for one change on `r` (`w:r` or `w:style`).
pub(crate) fn apply_font(
    tree: &mut XmlTree,
    r: NodeId,
    host: Host,
    change: &FontChange,
) -> Result<()> {
    validate_font_change(change)?;
    match change {
        FontChange::Flag(name, value) => {
            let local = FONT_FLAGS
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, local)| *local)
                .unwrap_or("b");
            let rpr = host_rpr(tree, r, host)?;
            match value {
                None => remove_all(tree, rpr, local),
                Some(flag) => {
                    let element = rpr_get_or_add(tree, rpr, local)?;
                    set_on_off(tree, element, *flag)
                }
            }
        }
        FontChange::Name(value) => {
            let rpr = host_rpr(tree, r, host)?;
            match value {
                None => remove_all(tree, rpr, "rFonts"),
                Some(name) => {
                    let fonts = rpr_get_or_add(tree, rpr, "rFonts")?;
                    set_w(tree, fonts, "ascii", name)?;
                    set_w(tree, fonts, "hAnsi", name)
                }
            }
        }
        FontChange::Size(value) => {
            let rpr = host_rpr(tree, r, host)?;
            match value {
                None => remove_all(tree, rpr, "sz"),
                Some(emu) => {
                    check_range(*emu, 0, u64::MAX as i128, "w:sz")?;
                    let half_points = ((*emu as f64 / 12_700.0) * 2.0) as i64;
                    let sz = rpr_get_or_add(tree, rpr, "sz")?;
                    set_w(tree, sz, "val", &half_points.to_string())
                }
            }
        }
        FontChange::Underline(value) => {
            let token = match value {
                None => None,
                Some(Underline::Bool(true)) => Some("single".to_string()),
                Some(Underline::Bool(false)) => Some("none".to_string()),
                Some(Underline::Style(token)) => Some(token.clone()),
            };
            let rpr = host_rpr(tree, r, host)?;
            remove_all(tree, rpr, "u")?;
            if let Some(token) = token {
                let u = add_w(tree, rpr, "u", successors(RPR_SEQ, "u"))?;
                set_w(tree, u, "val", &token)?;
            }
            Ok(())
        }
        FontChange::Color(value) => {
            let existing = w_child(tree, r, "rPr").and_then(|rpr| w_child(tree, rpr, "color"));
            if value.is_none() && existing.is_none() {
                return Ok(());
            }
            let rpr = host_rpr(tree, r, host)?;
            remove_all(tree, rpr, "color")?;
            if let Some((red, green, blue)) = value {
                let color = rpr_get_or_add(tree, rpr, "color")?;
                set_w(
                    tree,
                    color,
                    "val",
                    &format!("{red:02X}{green:02X}{blue:02X}"),
                )?;
            }
            Ok(())
        }
        FontChange::ThemeColor(value) => match value {
            None => {
                if let Some(rpr) = w_child(tree, r, "rPr")
                    && w_child(tree, rpr, "color").is_some()
                {
                    remove_all(tree, rpr, "color")?;
                }
                Ok(())
            }
            Some(token) => {
                let rpr = host_rpr(tree, r, host)?;
                let color = match w_child(tree, rpr, "color") {
                    Some(existing) => existing,
                    None => {
                        // `CT_RPr._new_color`: `<w:color w:val="000000"/>`.
                        let color = rpr_get_or_add(tree, rpr, "color")?;
                        set_w(tree, color, "val", "000000")?;
                        color
                    }
                };
                set_w(tree, color, "themeColor", token)
            }
        },
        FontChange::Highlight(value) => {
            let rpr = host_rpr(tree, r, host)?;
            match value {
                None => remove_all(tree, rpr, "highlight"),
                Some(token) => {
                    let highlight = rpr_get_or_add(tree, rpr, "highlight")?;
                    set_w(tree, highlight, "val", token)
                }
            }
        }
        FontChange::Subscript(value) | FontChange::Superscript(value) => {
            let token = if matches!(change, FontChange::Subscript(_)) {
                "subscript"
            } else {
                "superscript"
            };
            let rpr = host_rpr(tree, r, host)?;
            match value {
                None => remove_all(tree, rpr, "vertAlign"),
                Some(true) => {
                    let vert = rpr_get_or_add(tree, rpr, "vertAlign")?;
                    set_w(tree, vert, "val", token)
                }
                Some(false) => {
                    let current =
                        w_child(tree, rpr, "vertAlign").and_then(|v| w_attr(tree, v, "val"));
                    if current == Some(token) {
                        remove_all(tree, rpr, "vertAlign")?;
                    }
                    Ok(())
                }
            }
        }
    }
}

/// `ST_HpsMeasure.convert_from_xml` → EMU.
fn parse_hps_measure(value: &str) -> Result<i64> {
    if value.contains('m') || value.contains('n') || value.contains('p') {
        return parse_universal_measure(value);
    }
    let half_points = crate::styles::py_int(value).ok_or_else(|| {
        Error::invalid_argument(
            "invalid_measure",
            format!("invalid literal for int() with base 10: {value:?}"),
        )
    })?;
    // `Pt(int(s) / 2.0)` = `int(points * 12700)`.
    Ok((half_points as f64 / 2.0 * 12_700.0) as i64)
}

/// Upstream `Font` getters for `element` (`w:r` or `w:style`).
pub(crate) fn read_font(tree: &XmlTree, element: NodeId) -> Result<FontValues> {
    let Some(rpr) = w_child(tree, element, "rPr") else {
        return Ok(FontValues {
            flags: FONT_FLAGS
                .iter()
                .map(|(name, _)| (name.to_string(), None))
                .collect(),
            ..FontValues::default()
        });
    };
    let mut flags = Vec::new();
    for (name, local) in FONT_FLAGS {
        let value = match w_child(tree, rpr, local) {
            None => None,
            Some(child) => Some(on_off_val(tree, child)?),
        };
        flags.push((name.to_string(), value));
    }
    let underline = match w_child(tree, rpr, "u") {
        None => None,
        Some(u) => match w_attr(tree, u, "val") {
            None => None,
            Some("single") => Some(Underline::Bool(true)),
            Some("none") => Some(Underline::Bool(false)),
            Some(token) if UNDERLINES.contains(&token) => Some(Underline::Style(token.into())),
            Some(token) => {
                return Err(Error::invalid_argument(
                    "invalid_underline",
                    format!("WD_UNDERLINE has no XML mapping for '{token}'"),
                ));
            }
        },
    };
    let color = match w_child(tree, rpr, "color") {
        None => ColorValues::default(),
        Some(color) => {
            let val = w_attr(tree, color, "val").ok_or_else(|| {
                Error::invalid_argument(
                    "missing_color_value",
                    "required w:val attribute not present on w:color",
                )
            })?;
            let rgb = if val == "auto" {
                None
            } else {
                let valid = val.len() == 6 && val.chars().all(|c| c.is_ascii_hexdigit());
                if !valid {
                    return Err(Error::invalid_argument(
                        "invalid_rgb",
                        format!("invalid RGB color value {val:?}"),
                    ));
                }
                Some(val.to_ascii_uppercase())
            };
            let theme_color = match w_attr(tree, color, "themeColor") {
                None => None,
                Some(token) if THEME_COLORS.contains(&token) => Some(token.to_string()),
                Some(token) => {
                    return Err(Error::invalid_argument(
                        "invalid_theme_color",
                        format!("MSO_THEME_COLOR has no XML mapping for '{token}'"),
                    ));
                }
            };
            let color_type = if theme_color.is_some() {
                "THEME"
            } else if val == "auto" {
                "AUTO"
            } else {
                "RGB"
            };
            ColorValues {
                color_type: Some(color_type.to_string()),
                rgb,
                theme_color,
            }
        }
    };
    let highlight = match w_child(tree, rpr, "highlight").and_then(|h| w_attr(tree, h, "val")) {
        None => None,
        Some(token) if HIGHLIGHTS.contains(&token) => Some(token.to_string()),
        Some(token) => {
            return Err(Error::invalid_argument(
                "invalid_highlight",
                format!("WD_COLOR_INDEX has no XML mapping for '{token}'"),
            ));
        }
    };
    let vert = w_child(tree, rpr, "vertAlign").map(|v| w_attr(tree, v, "val").unwrap_or(""));
    Ok(FontValues {
        flags,
        name: w_child(tree, rpr, "rFonts")
            .and_then(|f| w_attr(tree, f, "ascii"))
            .map(str::to_string),
        size: match w_child(tree, rpr, "sz").and_then(|sz| w_attr(tree, sz, "val")) {
            None => None,
            Some(value) => Some(parse_hps_measure(value)?),
        },
        underline,
        color,
        highlight,
        subscript: vert.map(|v| v == "subscript"),
        superscript: vert.map(|v| v == "superscript"),
    })
}
impl Run {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "r", "run")
    }

    /// Package member holding the run.
    pub fn part(&self) -> &str {
        &self.h.part
    }

    /// `w:rPr/w:rStyle/@w:val` (upstream `CT_R.style`).
    pub fn style_id(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        Ok(w_child(tree, self.h.element, "rPr")
            .and_then(|rpr| w_child(tree, rpr, "rStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string))
    }

    /// Upstream `Run.style`: the character style with the run's style id,
    /// else the default character style (`None` when there is none). Reading
    /// never creates the styles part.
    pub fn style(&self, doc: &Document) -> Result<Option<crate::styles::Style>> {
        let id = self.style_id(doc)?;
        style_of(doc, id.as_deref(), crate::styles::StyleType::Character)
    }

    /// Upstream `Run.text`.
    pub fn text(&self, doc: &Document) -> Result<String> {
        Ok(run_text(self.live(doc)?, self.h.element))
    }

    fn edit(
        &self,
        doc: &mut Document,
        f: impl FnOnce(&mut XmlTree, NodeId) -> Result<()>,
    ) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        f(txn.tree_mut(&self.h.part)?, self.h.element)?;
        commit!(doc, txn, ())
    }

    /// Upstream `Run.text = text` (tabs → `w:tab`, `\n`/`\r` → `w:br`).
    pub fn set_text(&self, doc: &mut Document, text: &str) -> Result<()> {
        self.edit(doc, |tree, r| set_run_text(tree, r, text))
    }

    /// Upstream `Run.add_text(text)`: one appended `w:t`.
    pub fn add_text(&self, doc: &mut Document, text: &str) -> Result<()> {
        self.edit(doc, |tree, r| add_t(tree, r, text).map(|_| ()))
    }

    /// Upstream `Run.add_tab()`.
    pub fn add_tab(&self, doc: &mut Document) -> Result<()> {
        self.edit(doc, |tree, r| {
            let tab = new_w(tree, r, "tab")?;
            append(tree, r, tab)
        })
    }

    /// Upstream `Run.add_break(break_type)`.
    pub fn add_break(&self, doc: &mut Document, kind: BreakType) -> Result<()> {
        self.edit(doc, |tree, r| add_break_xml(tree, r, kind))
    }

    /// Upstream `Run.clear()`: every child element but `w:rPr` goes.
    pub fn clear(&self, doc: &mut Document) -> Result<()> {
        self.edit(doc, |tree, r| clear_except(tree, r, "rPr"))
    }

    /// Upstream `Run.style = style` (character style UI name or id).
    pub fn set_style(&self, doc: &mut Document, style: Option<&str>) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let style_id = resolve_style_id(&mut txn, style, StyleType::Character)?;
        set_run_style(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            style_id.as_deref(),
        )?;
        commit!(doc, txn, ())
    }

    /// Upstream `Font` getters (direct formatting only).
    pub fn font(&self, doc: &Document) -> Result<FontValues> {
        read_font(self.live(doc)?, self.h.element)
    }

    /// Upstream `Font` setters (and `Run.bold` / `italic` / `underline`),
    /// applied in order and atomically.
    pub fn set_font(&self, doc: &mut Document, changes: &[FontChange]) -> Result<()> {
        self.edit(doc, |tree, r| {
            for change in changes {
                apply_font(tree, r, Host::Text, change)?;
            }
            Ok(())
        })
    }

    /// Upstream `Run.bold = value`.
    pub fn set_bold(&self, doc: &mut Document, value: Option<bool>) -> Result<()> {
        self.set_font(doc, &[FontChange::Flag("bold".into(), value)])
    }

    /// Upstream `Run.italic = value`.
    pub fn set_italic(&self, doc: &mut Document, value: Option<bool>) -> Result<()> {
        self.set_font(doc, &[FontChange::Flag("italic".into(), value)])
    }

    /// Upstream `Run.underline = value`.
    pub fn set_underline(&self, doc: &mut Document, value: Option<Underline>) -> Result<()> {
        self.set_font(doc, &[FontChange::Underline(value)])
    }

    /// Upstream `Run.add_picture(image, width, height)`.
    pub fn add_picture(
        &self,
        doc: &mut Document,
        image: &[u8],
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<InlineShape> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let shape = add_picture_in(&mut txn, &self.h.part, self.h.element, image, width, height)?;
        commit!(doc, txn, shape)
    }
}

// ----------------------------------------------------------------- Table

impl Table {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "tbl", "table")
    }

    /// Grid column count (`w:tblGrid/w:gridCol`).
    pub fn column_count(&self, doc: &Document) -> Result<usize> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, tbl_grid(tree, self.h.element)?, "gridCol").count())
    }

    /// Row count (`w:tr`).
    pub fn row_count(&self, doc: &Document) -> Result<usize> {
        Ok(w_children(self.live(doc)?, self.h.element, "tr").count())
    }

    /// `w:tblPr/w:tblStyle/@w:val`.
    pub fn style_id(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        Ok(w_child(tree, self.h.element, "tblPr")
            .and_then(|pr| w_child(tree, pr, "tblStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string))
    }

    /// Upstream `Table.cell(row, col)` over the layout grid.
    pub fn cell(&self, doc: &Document, row: usize, col: usize) -> Result<Cell> {
        let tree = self.live(doc)?;
        let cells = table_cells(tree, self.h.element)?;
        let cols = w_children(tree, tbl_grid(tree, self.h.element)?, "gridCol").count();
        let index = row.checked_mul(cols).and_then(|i| i.checked_add(col));
        let tc = index.and_then(|i| cells.get(i)).copied().ok_or_else(|| {
            Error::invalid_argument(
                "cell_out_of_range",
                format!("no cell at row {row}, column {col}"),
            )
        })?;
        Ok(Cell {
            h: Handle::capture(doc, &self.h.part, tree, tc),
        })
    }

    /// Upstream `Table.add_row()`: a row of one cell per grid column, each
    /// sized from its `w:gridCol/@w:w`.
    pub fn add_row(&self, doc: &mut Document) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let tbl = self.h.element;
        let grid = tbl_grid(tree, tbl)?;
        let widths: Vec<Option<i64>> = w_children(tree, grid, "gridCol")
            .collect::<Vec<_>>()
            .into_iter()
            .map(|col| w_attr(tree, col, "w").map(parse_twips_measure).transpose())
            .collect::<Result<_>>()?;
        let tr = new_w(tree, tbl, "tr")?;
        append(tree, tbl, tr)?;
        for width in widths {
            let tc = add_tc(tree, tr)?;
            if let Some(width) = width {
                set_cell_width(tree, tc, width)?;
            }
        }
        commit!(doc, txn, ())
    }

    /// Upstream `Table.add_column(width)` (EMU).
    pub fn add_column(&self, doc: &mut Document, width: i64) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let tbl = self.h.element;
        let grid = tbl_grid(tree, tbl)?;
        let col = add_w(tree, grid, "gridCol", &["tblGridChange"])?;
        set_w(tree, col, "w", &twips_measure(width, "w:w")?)?;
        let rows: Vec<NodeId> = w_children(tree, tbl, "tr").collect();
        for tr in rows {
            let tc = add_tc(tree, tr)?;
            set_cell_width(tree, tc, width)?;
        }
        commit!(doc, txn, ())
    }

    /// Upstream `Table.style = style` (table style UI name or id).
    pub fn set_style(&self, doc: &mut Document, style: Option<&str>) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let style_id = resolve_style_id(&mut txn, style, StyleType::Table)?;
        set_table_style_id(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            style_id.as_deref(),
        )?;
        commit!(doc, txn, ())
    }
}

impl Cell {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "tc", "cell")
    }

    /// Upstream `_Cell.text` (paragraph texts joined by `\n`).
    pub fn text(&self, doc: &Document) -> Result<String> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "p")
            .map(|p| paragraph_text(tree, p))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Upstream `_Cell.text = text`: all content but `w:tcPr` replaced by
    /// one paragraph with one run.
    pub fn set_text(&self, doc: &mut Document, text: &str) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let tc = self.h.element;
        clear_except(tree, tc, "tcPr")?;
        let p = new_w(tree, tc, "p")?;
        append(tree, tc, p)?;
        let r = new_w(tree, p, "r")?;
        append(tree, p, r)?;
        set_run_text(tree, r, text)?;
        commit!(doc, txn, ())
    }
}

/// `StoryPart.get_style(style_id, style_type)` on a read-only snapshot.
fn style_of(
    doc: &Document,
    style_id: Option<&str>,
    style_type: crate::styles::StyleType,
) -> Result<Option<crate::styles::Style>> {
    Ok(doc.styles()?.get_by_id(style_id, style_type).cloned())
}

// ------------------------------------------------------- block containers

/// What a [`BlockContainer`] wraps (upstream `BlockItemContainer`
/// subclasses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ContainerKind {
    /// The document body (`w:body`, upstream `_Body`).
    Body,
    /// A header definition (`w:hdr` root of a header part, `_Header`).
    Header,
    /// A footer definition (`w:ftr`, `_Footer`).
    Footer,
    /// A table cell (`w:tc`, `_Cell`).
    Cell,
}

impl ContainerKind {
    fn local(self) -> &'static str {
        match self {
            ContainerKind::Body => "body",
            ContainerKind::Header => "hdr",
            ContainerKind::Footer => "ftr",
            ContainerKind::Cell => "tc",
        }
    }

    fn what(self) -> &'static str {
        match self {
            ContainerKind::Body => "document body",
            ContainerKind::Header => "header",
            ContainerKind::Footer => "footer",
            ContainerKind::Cell => "cell",
        }
    }
}

/// A live block-item container: body, header, footer or table cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockContainer {
    pub(crate) h: Handle,
    pub(crate) kind: ContainerKind,
}

/// A paragraph or table (upstream `iter_inner_content` items).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockItem {
    /// A `w:p`.
    Paragraph(Paragraph),
    /// A `w:tbl`.
    Table(Table),
}

impl BlockContainer {
    pub(crate) fn new(h: Handle, kind: ContainerKind) -> BlockContainer {
        BlockContainer { h, kind }
    }

    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, self.kind.local(), self.kind.what())
    }

    /// The container kind.
    pub fn kind(&self) -> ContainerKind {
        self.kind
    }

    /// Package member holding the container.
    pub fn part(&self) -> &str {
        &self.h.part
    }

    /// Direct `w:p` children (upstream `paragraphs`).
    pub fn paragraphs(&self, doc: &Document) -> Result<Vec<Paragraph>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "p")
            .map(|p| Paragraph {
                h: Handle::capture(doc, &self.h.part, tree, p),
            })
            .collect())
    }

    /// Direct `w:tbl` children (upstream `tables`).
    pub fn tables(&self, doc: &Document) -> Result<Vec<Table>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "tbl")
            .map(|t| Table {
                h: Handle::capture(doc, &self.h.part, tree, t),
            })
            .collect())
    }

    /// Direct `w:p` and `w:tbl` children in document order (upstream
    /// `iter_inner_content`).
    pub fn inner_content(&self, doc: &Document) -> Result<Vec<BlockItem>> {
        let tree = self.live(doc)?;
        Ok(block_items(doc, &self.h.part, tree, self.h.element))
    }

    /// Text of each direct paragraph (convenience for `paragraphs`).
    pub fn paragraph_texts(&self, doc: &Document) -> Result<Vec<String>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "p")
            .map(|p| paragraph_text(tree, p))
            .collect())
    }

    /// Upstream `add_paragraph(text, style)`: a paragraph at the end (before
    /// the body's `w:sectPr`), `text` in one run when non-empty, paragraph
    /// style `style` when given.
    pub fn add_paragraph(
        &self,
        doc: &mut Document,
        text: &str,
        style: Option<&str>,
    ) -> Result<Paragraph> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let p = add_container_paragraph(&mut txn, &self.h.part, self.h.element, self.kind)?;
        fill_paragraph(&mut txn, &self.h.part, p, Some(text), style)?;
        let handle = staged_handle(&txn, &self.h.part, p)?;
        commit!(doc, txn, Paragraph { h: handle })
    }

    /// Upstream `add_table(rows, cols, width)`: a table at the end whose
    /// columns share `width` (EMU) evenly. For a cell this is
    /// `_Cell.add_table` only through [`Cell::add_table`].
    pub fn add_table(
        &self,
        doc: &mut Document,
        rows: usize,
        cols: usize,
        width: i64,
    ) -> Result<Table> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tbl = add_container_table(
            &mut txn,
            &self.h.part,
            self.h.element,
            self.kind,
            rows,
            cols,
            width,
        )?;
        let handle = staged_handle(&txn, &self.h.part, tbl)?;
        commit!(doc, txn, Table { h: handle })
    }
}

pub(crate) fn block_items(
    doc: &Document,
    part: &str,
    tree: &XmlTree,
    parent: NodeId,
) -> Vec<BlockItem> {
    tree.child_elements(parent)
        .filter_map(|child| {
            if tree.is(child, W, "p") {
                Some(BlockItem::Paragraph(Paragraph {
                    h: Handle::capture(doc, part, tree, child),
                }))
            } else if tree.is(child, W, "tbl") {
                Some(BlockItem::Table(Table {
                    h: Handle::capture(doc, part, tree, child),
                }))
            } else {
                None
            }
        })
        .collect()
}

/// `_element.add_p()`: before `w:sectPr` in the body, appended elsewhere.
fn add_container_paragraph(
    txn: &mut Txn<'_>,
    part: &str,
    element: NodeId,
    kind: ContainerKind,
) -> Result<NodeId> {
    let tree = txn.tree_mut(part)?;
    let p = new_w(tree, element, "p")?;
    if kind == ContainerKind::Body {
        insert_before_successors(tree, element, p, &["sectPr"])?;
    } else {
        append(tree, element, p)?;
    }
    Ok(p)
}

/// `CT_Tbl.new_tbl(rows, cols, width)` + `_element._insert_tbl(tbl)`.
fn add_container_table(
    txn: &mut Txn<'_>,
    part: &str,
    element: NodeId,
    kind: ContainerKind,
    rows: usize,
    cols: usize,
    width: i64,
) -> Result<NodeId> {
    let tree = txn.tree_mut(part)?;
    let tbl = new_tbl(tree, element, rows, cols, width)?;
    if kind == ContainerKind::Body {
        insert_before_successors(tree, element, tbl, &["sectPr"])?;
    } else {
        append(tree, element, tbl)?;
    }
    Ok(tbl)
}

impl Document {
    /// The document body as a block container (upstream `Document._body`).
    pub fn body(&self) -> Result<BlockContainer> {
        let part = self.package().main_part_name().to_string();
        let tree = self.package().xml(&part)?;
        let body = body(tree)?;
        Ok(BlockContainer::new(
            Handle::capture(self, &part, tree, body),
            ContainerKind::Body,
        ))
    }

    /// Upstream `Document.iter_inner_content()`.
    pub fn inner_content(&self) -> Result<Vec<BlockItem>> {
        self.body()?.inner_content(self)
    }

    /// Upstream `_Body.clear_content()`: every child element of `w:body`
    /// but `w:sectPr` goes (comments and processing instructions stay, as
    /// lxml's `./*` selects elements only).
    pub fn clear_body_content(&mut self) -> Result<()> {
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let tree = txn.tree_mut(&part)?;
        let body = body(tree)?;
        clear_except(tree, body, "sectPr")?;
        commit!(self, txn, ())
    }
}

// ----------------------------------------------------------- value types

/// Upstream `Length` (an EMU count) with its unit constructors (`Inches`,
/// `Cm`, `Mm`, `Pt`, `Twips`, `Emu`: `int(value * emus_per_unit)`) and
/// read-only unit accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct Length(pub i64);

impl Length {
    /// `Emu(emu)`.
    pub fn from_emu(emu: i64) -> Length {
        Length(emu)
    }

    /// `Inches(inches)`.
    pub fn from_inches(inches: f64) -> Length {
        Length((inches * 914_400.0) as i64)
    }

    /// `Cm(cm)`.
    pub fn from_cm(cm: f64) -> Length {
        Length((cm * 360_000.0) as i64)
    }

    /// `Mm(mm)`.
    pub fn from_mm(mm: f64) -> Length {
        Length((mm * 36_000.0) as i64)
    }

    /// `Pt(points)`.
    pub fn from_pt(points: f64) -> Length {
        Length((points * 12_700.0) as i64)
    }

    /// `Twips(twips)`.
    pub fn from_twips(twips: f64) -> Length {
        Length((twips * 635.0) as i64)
    }

    /// `Length.emu`.
    pub fn emu(self) -> i64 {
        self.0
    }

    /// `Length.cm`.
    pub fn cm(self) -> f64 {
        self.0 as f64 / 360_000.0
    }

    /// `Length.inches`.
    pub fn inches(self) -> f64 {
        self.0 as f64 / 914_400.0
    }

    /// `Length.mm`.
    pub fn mm(self) -> f64 {
        self.0 as f64 / 36_000.0
    }

    /// `Length.pt`.
    pub fn pt(self) -> f64 {
        self.0 as f64 / 12_700.0
    }

    /// `Length.twips` (`int(round(emu / 635.0))`, ties to even).
    pub fn twips(self) -> i64 {
        twips(self.0)
    }
}

/// Upstream `RGBColor`: three channel values; `Display` is `RRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct RgbColor(pub u8, pub u8, pub u8);

/// Python `int(text, 16)`: whitespace, a sign, an optional `0x` prefix and
/// `_` between digits.
fn py_int_hex(text: &str) -> Option<i64> {
    let trimmed = crate::unicode::py_strip(text);
    let (negative, rest) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let rest = rest
        .strip_prefix("0x")
        .or_else(|| rest.strip_prefix("0X"))
        .map(|r| r.strip_prefix('_').unwrap_or(r))
        .unwrap_or(rest);
    if rest.is_empty() || rest.starts_with('_') || rest.ends_with('_') || rest.contains("__") {
        return None;
    }
    let digits: String = rest.chars().filter(|c| *c != '_').collect();
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = i64::from_str_radix(&digits, 16).ok()?;
    Some(if negative { -value } else { value })
}

impl RgbColor {
    /// Upstream `RGBColor.from_string`: `int(s[:2], 16)`, `int(s[2:4], 16)`,
    /// `int(s[4:], 16)` (character slices), each 0–255.
    pub fn from_string(text: &str) -> Result<RgbColor> {
        let chars: Vec<char> = text.chars().collect();
        let slice = |from: usize, to: Option<usize>| -> String {
            let end = to.unwrap_or(chars.len()).min(chars.len());
            chars[from.min(end)..end].iter().collect()
        };
        let parts = [slice(0, Some(2)), slice(2, Some(4)), slice(4, None)];
        let mut out = [0u8; 3];
        for (index, part) in parts.iter().enumerate() {
            let value = py_int_hex(part).ok_or_else(|| {
                Error::invalid_argument(
                    "invalid_rgb",
                    format!("invalid literal for int() with base 16: {part:?}"),
                )
            })?;
            out[index] = u8::try_from(value).map_err(|_| {
                Error::invalid_argument(
                    "invalid_rgb",
                    "RGBColor() takes three integer values 0-255",
                )
            })?;
        }
        Ok(RgbColor(out[0], out[1], out[2]))
    }
}

impl std::fmt::Display for RgbColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
}

// ------------------------------------------------- table, row, column, cell

/// `WD_TABLE_ALIGNMENT` XML tokens.
pub const TABLE_ALIGNMENTS: &[&str] = &["left", "center", "right"];
/// `WD_CELL_VERTICAL_ALIGNMENT` XML tokens.
pub const CELL_VERTICAL_ALIGNMENTS: &[&str] = &["top", "center", "bottom", "both"];
/// `WD_ROW_HEIGHT_RULE` XML tokens.
pub const ROW_HEIGHT_RULES: &[&str] = &["auto", "atLeast", "exact"];

const TRPR_SEQ: &[&str] = &[
    "cnfStyle",
    "divId",
    "gridBefore",
    "gridAfter",
    "wBefore",
    "wAfter",
    "cantSplit",
    "trHeight",
    "tblHeader",
    "tblCellSpacing",
    "jc",
    "hidden",
    "ins",
    "del",
    "trPrChange",
];

/// Table direction (`WD_TABLE_DIRECTION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum TableDirection {
    /// `LTR` (`w:bidiVisual w:val="0"`).
    Ltr,
    /// `RTL` (`w:bidiVisual`).
    Rtl,
}

fn check_token(value: &str, tokens: &[&str], enum_name: &str, reason: &'static str) -> Result<()> {
    if tokens.contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_argument(
            reason,
            format!("{enum_name} has no XML mapping for '{value}'"),
        ))
    }
}

fn decimal_attr(tree: &XmlTree, element: Option<NodeId>, local: &str) -> Result<Option<i64>> {
    match element.and_then(|e| w_attr(tree, e, local)) {
        None => Ok(None),
        Some(raw) => crate::styles::py_int(raw).map(Some).ok_or_else(|| {
            Error::invalid_argument(
                "invalid_decimal",
                format!("invalid literal for int() with base 10: {raw:?}"),
            )
        }),
    }
}

/// `CT_Row.get_or_add_trPr()`: after `w:tblPrEx`, else first.
fn get_or_add_trpr(tree: &mut XmlTree, tr: NodeId) -> Result<NodeId> {
    if let Some(existing) = w_child(tree, tr, "trPr") {
        return Ok(existing);
    }
    let tr_pr = new_w(tree, tr, "trPr")?;
    match w_child(tree, tr, "tblPrEx") {
        Some(ex) => {
            let index = tree
                .index_in_parent(ex)
                .ok_or_else(|| Error::internal("detached", "tblPrEx has no index"))?;
            tree.insert(tr, index + 1, tr_pr)?;
        }
        None => insert_first(tree, tr, tr_pr)?,
    }
    Ok(tr_pr)
}

/// lxml `parent.append(child)` for an attached `child`: the element moves
/// together with its tail text.
fn move_append(tree: &mut XmlTree, parent: NodeId, child: NodeId) -> Result<()> {
    let mut tail = Vec::new();
    if let Some(old_parent) = tree.parent(child) {
        let siblings = tree.children(old_parent).to_vec();
        if let Some(index) = siblings.iter().position(|n| *n == child) {
            for node in &siblings[index + 1..] {
                if matches!(tree.data(*node), NodeData::Text(_)) {
                    tail.push(*node);
                } else {
                    break;
                }
            }
        }
        tree.detach(child)?;
        for node in &tail {
            tree.detach(*node)?;
        }
    }
    append(tree, parent, child)?;
    for node in tail {
        append(tree, parent, node)?;
    }
    Ok(())
}

/// The layout grid of `tbl` (as [`table_cells`]) and its column count.
fn grid(tree: &XmlTree, tbl: NodeId) -> Result<(Vec<NodeId>, usize)> {
    let cols = w_children(tree, tbl_grid(tree, tbl)?, "gridCol").count();
    Ok((table_cells(tree, tbl)?, cols))
}

/// Merge geometry over one table (upstream `CT_Tc` positional helpers).
struct MergeGeometry<'t> {
    tree: &'t XmlTree,
    tbl: NodeId,
}

fn invalid_span(message: &str) -> Error {
    Error::invalid_argument("invalid_span", message.to_string())
}

impl MergeGeometry<'_> {
    fn trs(&self) -> Vec<NodeId> {
        w_children(self.tree, self.tbl, "tr").collect()
    }

    fn tr_of(&self, tc: NodeId) -> Result<NodeId> {
        self.tree
            .ancestors(tc)
            .find(|node| self.tree.is(*node, W, "tr"))
            .ok_or_else(|| Error::internal("merge", "cell outside a row"))
    }

    fn tr_idx(&self, tc: NodeId) -> Result<usize> {
        let tr = self.tr_of(tc)?;
        self.trs().iter().position(|t| *t == tr).ok_or_else(|| {
            Error::invalid_argument("not_in_list", "row is not in the table's row list")
        })
    }

    fn grid_span(&self, tc: NodeId) -> Result<i64> {
        let gs = w_child(self.tree, tc, "tcPr").and_then(|pr| w_child(self.tree, pr, "gridSpan"));
        Ok(decimal_attr(self.tree, gs, "val")?.unwrap_or(1))
    }

    fn v_merge(&self, tc: NodeId) -> Option<String> {
        w_child(self.tree, tc, "tcPr")
            .and_then(|pr| w_child(self.tree, pr, "vMerge"))
            .map(|vm| {
                w_attr(self.tree, vm, "val")
                    .unwrap_or("continue")
                    .to_string()
            })
    }

    fn grid_before(&self, tr: NodeId) -> Result<i64> {
        let gb = w_child(self.tree, tr, "trPr").and_then(|pr| w_child(self.tree, pr, "gridBefore"));
        Ok(decimal_attr(self.tree, gb, "val")?.unwrap_or(0))
    }

    fn grid_offset(&self, tc: NodeId) -> Result<i64> {
        let tr = self.tr_of(tc)?;
        let mut offset = self.grid_before(tr)?;
        let parent = self
            .tree
            .parent(tc)
            .ok_or_else(|| Error::internal("merge", "detached cell"))?;
        for sibling in self.tree.child_elements(parent) {
            if sibling == tc {
                break;
            }
            if self.tree.is(sibling, W, "tc") {
                offset += self.grid_span(sibling)?;
            }
        }
        Ok(offset)
    }

    fn tc_at_grid_offset(&self, tr: NodeId, grid_offset: i64) -> Result<NodeId> {
        let mut remaining = grid_offset - self.grid_before(tr)?;
        for tc in w_children(self.tree, tr, "tc") {
            if remaining < 0 {
                break;
            }
            if remaining == 0 {
                return Ok(tc);
            }
            remaining -= self.grid_span(tc)?;
        }
        Err(Error::invalid_argument(
            "no_cell_at_grid_offset",
            format!("no `tc` element at grid_offset={grid_offset}"),
        ))
    }

    fn tc_above(&self, tc: NodeId) -> Result<NodeId> {
        let tr = self.tr_of(tc)?;
        let above = self
            .tree
            .parent(tr)
            .map(|parent| {
                self.tree
                    .child_elements(parent)
                    .take_while(|n| *n != tr)
                    .filter(|n| self.tree.is(*n, W, "tr"))
                    .last()
            })
            .unwrap_or(None)
            .ok_or_else(|| {
                Error::invalid_argument("no_row_above", "no tr above topmost tr in w:tbl")
            })?;
        self.tc_at_grid_offset(above, self.grid_offset(tc)?)
    }

    fn tc_below(&self, tc: NodeId) -> Result<Option<NodeId>> {
        let trs = self.trs();
        let index = self.tr_idx(tc)?;
        match trs.get(index + 1) {
            None => Ok(None),
            Some(tr) => self.tc_at_grid_offset(*tr, self.grid_offset(tc)?).map(Some),
        }
    }

    fn top(&self, tc: NodeId) -> Result<usize> {
        match self.v_merge(tc).as_deref() {
            None | Some("restart") => self.tr_idx(tc),
            _ => self.top(self.tc_above(tc)?),
        }
    }

    fn bottom(&self, tc: NodeId) -> Result<usize> {
        if self.v_merge(tc).is_some()
            && let Some(below) = self.tc_below(tc)?
            && self.v_merge(below).as_deref() == Some("continue")
        {
            return self.bottom(below);
        }
        Ok(self.tr_idx(tc)? + 1)
    }

    fn left(&self, tc: NodeId) -> Result<i64> {
        self.grid_offset(tc)
    }

    fn right(&self, tc: NodeId) -> Result<i64> {
        Ok(self.grid_offset(tc)? + self.grid_span(tc)?)
    }

    /// `CT_Tc._span_dimensions`.
    fn span_dimensions(&self, a: NodeId, b: NodeId) -> Result<(usize, i64, usize, i64)> {
        let (a_top, a_bottom, a_left, a_right) =
            (self.top(a)?, self.bottom(a)?, self.left(a)?, self.right(a)?);
        let (b_top, b_bottom, b_left, b_right) =
            (self.top(b)?, self.bottom(b)?, self.left(b)?, self.right(b)?);
        if a_top == b_top && a_bottom != b_bottom {
            return Err(invalid_span("requested span not rectangular"));
        }
        if a_left == b_left && a_right != b_right {
            return Err(invalid_span("requested span not rectangular"));
        }
        let (tm, other) = if a_top < b_top {
            ((a_top, a_bottom), (b_top, b_bottom))
        } else {
            ((b_top, b_bottom), (a_top, a_bottom))
        };
        if tm.0 < other.0 && tm.1 > other.1 {
            return Err(invalid_span("requested span not rectangular"));
        }
        let (lm, other) = if a_left < b_left {
            ((a_left, a_right), (b_left, b_right))
        } else {
            ((b_left, b_right), (a_left, a_right))
        };
        if lm.0 < other.0 && lm.1 > other.1 {
            return Err(invalid_span("requested span not rectangular"));
        }
        let top = a_top.min(b_top);
        let left = a_left.min(b_left);
        let bottom = a_bottom.max(b_bottom);
        let right = a_right.max(b_right);
        Ok((top, left, bottom - top, right - left))
    }
}

/// `CT_Tc.iter_block_items()`: direct `w:p`, `w:tbl`, `w:sdt` children.
fn tc_block_items(tree: &XmlTree, tc: NodeId) -> Vec<NodeId> {
    tree.child_elements(tc)
        .filter(|n| tree.is(*n, W, "p") || tree.is(*n, W, "tbl") || tree.is(*n, W, "sdt"))
        .collect()
}

fn index_error(message: &str) -> Error {
    Error::invalid_argument("index_out_of_range", message.to_string())
}

/// `CT_Tc._move_content_to(other_tc)`.
fn move_content_to(tree: &mut XmlTree, tc: NodeId, other: NodeId) -> Result<()> {
    if tc == other {
        return Ok(());
    }
    let items = tc_block_items(tree, tc);
    // `_is_empty`
    let first = *items
        .first()
        .ok_or_else(|| index_error("list index out of range"))?;
    if items.len() == 1 && tree.is(first, W, "p") && w_children(tree, first, "r").next().is_none() {
        return Ok(());
    }
    // `other_tc._remove_trailing_empty_p()`
    let other_items = tc_block_items(tree, other);
    let last = *other_items
        .last()
        .ok_or_else(|| index_error("list index out of range"))?;
    if tree.is(last, W, "p") && w_children(tree, last, "r").next().is_none() {
        remove(tree, last)?;
    }
    for item in tc_block_items(tree, tc) {
        move_append(tree, other, item)?;
    }
    let p = new_w(tree, tc, "p")?;
    append(tree, tc, p)
}

/// `CT_Tc.width` (dxa only).
fn tc_width(tree: &XmlTree, tc: NodeId) -> Result<Option<i64>> {
    let Some(tc_w) = w_child(tree, tc, "tcPr").and_then(|pr| w_child(tree, pr, "tcW")) else {
        return Ok(None);
    };
    if w_attr(tree, tc_w, "type") != Some("dxa") {
        return Ok(None);
    }
    Ok(decimal_attr(tree, Some(tc_w), "w")?.map(|twips| twips * 635))
}

/// `CT_TcPr.grid_span = value`.
fn set_grid_span(tree: &mut XmlTree, tc: NodeId, value: i64) -> Result<()> {
    let tc_pr = get_or_add_first(tree, tc, "tcPr")?;
    remove_all(tree, tc_pr, "gridSpan")?;
    if value > 1 {
        let span = get_or_add(tree, tc_pr, "gridSpan", TCPR_SEQ)?;
        set_w(tree, span, "val", &value.to_string())?;
    }
    Ok(())
}

/// `CT_Tc.vMerge = value`.
fn set_v_merge(tree: &mut XmlTree, tc: NodeId, value: Option<&str>) -> Result<()> {
    let tc_pr = get_or_add_first(tree, tc, "tcPr")?;
    remove_all(tree, tc_pr, "vMerge")?;
    if let Some(value) = value {
        let vm = add_w(tree, tc_pr, "vMerge", successors(TCPR_SEQ, "vMerge"))?;
        // `continue` is the attribute default and is not written.
        if value != "continue" {
            set_w(tree, vm, "val", value)?;
        }
    }
    Ok(())
}

/// `CT_Tc._span_to_width` / `_swallow_next_tc` / `_grow_to`.
fn grow_to(
    tree: &mut XmlTree,
    tbl: NodeId,
    tc: NodeId,
    width: i64,
    height: usize,
    top_tc: Option<NodeId>,
) -> Result<()> {
    let top_tc = top_tc.unwrap_or(tc);
    let v_merge = if top_tc != tc {
        Some("continue")
    } else if height == 1 {
        None
    } else {
        Some("restart")
    };
    // `_span_to_width`
    move_content_to(tree, tc, top_tc)?;
    loop {
        let span = MergeGeometry { tree, tbl }.grid_span(tc)?;
        if span >= width {
            break;
        }
        let next = tree
            .parent(tc)
            .map(|parent| {
                tree.child_elements(parent)
                    .skip_while(|n| *n != tc)
                    .skip(1)
                    .find(|n| tree.is(*n, W, "tc"))
            })
            .unwrap_or(None)
            .ok_or_else(|| invalid_span("not enough grid columns"))?;
        let next_span = MergeGeometry { tree, tbl }.grid_span(next)?;
        if span + next_span > width {
            return Err(invalid_span("span is not rectangular"));
        }
        move_content_to(tree, next, top_tc)?;
        // `_add_width_of`
        if let (Some(a), Some(b)) = (tc_width(tree, tc)?, tc_width(tree, next)?)
            && a != 0
            && b != 0
        {
            set_cell_width(tree, tc, a + b)?;
        }
        set_grid_span(tree, tc, span + next_span)?;
        remove(tree, next)?;
    }
    set_v_merge(tree, tc, v_merge)?;
    if height > 1 {
        let below = MergeGeometry { tree, tbl }
            .tc_below(tc)?
            .ok_or_else(|| Error::internal("merge", "no cell below while growing a span"))?;
        grow_to(tree, tbl, below, width, height - 1, Some(top_tc))?;
    }
    Ok(())
}

/// A live `w:tr` handle (upstream `_Row`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub(crate) h: Handle,
    table: NodeId,
}

/// A live `w:gridCol` handle (upstream `_Column`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub(crate) h: Handle,
    table: NodeId,
}

impl Table {
    fn edit(
        &self,
        doc: &mut Document,
        f: impl FnOnce(&mut XmlTree, NodeId) -> Result<()>,
    ) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        f(txn.tree_mut(&self.h.part)?, self.h.element)?;
        commit!(doc, txn, ())
    }

    /// Package member holding the table.
    pub fn part(&self) -> &str {
        &self.h.part
    }

    /// Upstream `Table.style`: the table style with the table's style id,
    /// else the default table style.
    pub fn style(&self, doc: &Document) -> Result<Option<crate::styles::Style>> {
        let id = self.style_id(doc)?;
        style_of(doc, id.as_deref(), crate::styles::StyleType::Table)
    }

    /// Upstream `Table.alignment` (`w:tblPr/w:jc` token).
    pub fn alignment(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        let pr = tbl_pr(tree, self.h.element)?;
        match w_child(tree, pr, "jc").and_then(|jc| w_attr(tree, jc, "val")) {
            None => Ok(None),
            Some(token) => {
                check_token(
                    token,
                    TABLE_ALIGNMENTS,
                    "WD_TABLE_ALIGNMENT",
                    "invalid_table_alignment",
                )?;
                Ok(Some(token.to_string()))
            }
        }
    }

    /// Upstream `Table.alignment = value`.
    pub fn set_alignment(&self, doc: &mut Document, value: Option<&str>) -> Result<()> {
        if let Some(token) = value {
            check_token(
                token,
                TABLE_ALIGNMENTS,
                "WD_TABLE_ALIGNMENT",
                "invalid_table_alignment",
            )?;
        }
        self.edit(doc, |tree, tbl| {
            let pr = tbl_pr(tree, tbl)?;
            remove_all(tree, pr, "jc")?;
            if let Some(token) = value {
                let jc = get_or_add(tree, pr, "jc", TBLPR_SEQ)?;
                set_w(tree, jc, "val", token)?;
            }
            Ok(())
        })
    }

    /// Upstream `Table.autofit` (`w:tblLayout/@w:type != "fixed"`).
    pub fn autofit(&self, doc: &Document) -> Result<bool> {
        let tree = self.live(doc)?;
        let pr = tbl_pr(tree, self.h.element)?;
        Ok(match w_child(tree, pr, "tblLayout") {
            None => true,
            Some(layout) => w_attr(tree, layout, "type") != Some("fixed"),
        })
    }

    /// Upstream `Table.autofit = value`.
    pub fn set_autofit(&self, doc: &mut Document, value: bool) -> Result<()> {
        self.edit(doc, |tree, tbl| {
            let pr = tbl_pr(tree, tbl)?;
            let layout = get_or_add(tree, pr, "tblLayout", TBLPR_SEQ)?;
            set_w(
                tree,
                layout,
                "type",
                if value { "autofit" } else { "fixed" },
            )
        })
    }

    /// Upstream `Table.table_direction` (`w:bidiVisual`).
    pub fn table_direction(&self, doc: &Document) -> Result<Option<TableDirection>> {
        let tree = self.live(doc)?;
        let pr = tbl_pr(tree, self.h.element)?;
        match w_child(tree, pr, "bidiVisual") {
            None => Ok(None),
            Some(bidi) => Ok(Some(if on_off_val(tree, bidi)? {
                TableDirection::Rtl
            } else {
                TableDirection::Ltr
            })),
        }
    }

    /// Upstream `Table.table_direction = value`.
    pub fn set_table_direction(
        &self,
        doc: &mut Document,
        value: Option<TableDirection>,
    ) -> Result<()> {
        self.edit(doc, |tree, tbl| {
            let pr = tbl_pr(tree, tbl)?;
            match value {
                None => remove_all(tree, pr, "bidiVisual"),
                Some(direction) => {
                    let bidi = get_or_add(tree, pr, "bidiVisual", TBLPR_SEQ)?;
                    set_on_off(tree, bidi, direction == TableDirection::Rtl)
                }
            }
        })
    }

    /// Upstream `Table.rows`.
    pub fn rows(&self, doc: &Document) -> Result<Vec<Row>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "tr")
            .map(|tr| Row {
                h: Handle::capture(doc, &self.h.part, tree, tr),
                table: self.h.element,
            })
            .collect())
    }

    /// Upstream `Table.columns`.
    pub fn columns(&self, doc: &Document) -> Result<Vec<Column>> {
        let tree = self.live(doc)?;
        let grid = tbl_grid(tree, self.h.element)?;
        Ok(w_children(tree, grid, "gridCol")
            .map(|col| Column {
                h: Handle::capture(doc, &self.h.part, tree, col),
                table: self.h.element,
            })
            .collect())
    }

    fn cell_handles(&self, doc: &Document, cells: Vec<NodeId>) -> Result<Vec<Cell>> {
        let tree = self.live(doc)?;
        Ok(cells
            .into_iter()
            .map(|tc| Cell {
                h: Handle::capture(doc, &self.h.part, tree, tc),
            })
            .collect())
    }

    /// Upstream `Table.row_cells(row_idx)` (layout grid, spans repeated).
    pub fn row_cells(&self, doc: &Document, row: usize) -> Result<Vec<Cell>> {
        let (cells, cols) = grid(self.live(doc)?, self.h.element)?;
        let start = row.saturating_mul(cols).min(cells.len());
        let end = start.saturating_add(cols).min(cells.len());
        self.cell_handles(doc, cells[start..end].to_vec())
    }

    /// Upstream `Table.column_cells(column_idx)`.
    pub fn column_cells(&self, doc: &Document, col: usize) -> Result<Vec<Cell>> {
        let (cells, cols) = grid(self.live(doc)?, self.h.element)?;
        if cols == 0 {
            return Err(Error::invalid_argument(
                "zero_step",
                "range() arg 3 must not be zero",
            ));
        }
        let picked = (col..cells.len()).step_by(cols).map(|i| cells[i]).collect();
        self.cell_handles(doc, picked)
    }

    /// Every layout-grid cell row by row (upstream `Table._cells`).
    pub fn layout_cells(&self, doc: &Document) -> Result<Vec<Cell>> {
        let cells = table_cells(self.live(doc)?, self.h.element)?;
        self.cell_handles(doc, cells)
    }
}

impl Row {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "tr", "row")
    }

    fn edit(
        &self,
        doc: &mut Document,
        f: impl FnOnce(&mut XmlTree, NodeId) -> Result<()>,
    ) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        f(txn.tree_mut(&self.h.part)?, self.h.element)?;
        commit!(doc, txn, ())
    }

    /// Upstream `_Row.cells` (vertically merged cells repeat the cell
    /// above; spans repeat their cell).
    pub fn cells(&self, doc: &Document) -> Result<Vec<Cell>> {
        let tree = self.live(doc)?;
        let geometry = MergeGeometry {
            tree,
            tbl: self.table,
        };
        fn expand(geometry: &MergeGeometry<'_>, tc: NodeId, out: &mut Vec<NodeId>) -> Result<()> {
            if geometry.v_merge(tc).as_deref() == Some("continue") {
                return expand(geometry, geometry.tc_above(tc)?, out);
            }
            for _ in 0..geometry.grid_span(tc)?.max(0) {
                out.push(tc);
            }
            Ok(())
        }
        let mut out = Vec::new();
        for tc in w_children(tree, self.h.element, "tc") {
            expand(&geometry, tc, &mut out)?;
        }
        Ok(out
            .into_iter()
            .map(|tc| Cell {
                h: Handle::capture(doc, &self.h.part, tree, tc),
            })
            .collect())
    }

    /// Upstream `_Row.grid_cols_before` (`w:gridBefore`).
    pub fn grid_cols_before(&self, doc: &Document) -> Result<i64> {
        let tree = self.live(doc)?;
        let gb =
            w_child(tree, self.h.element, "trPr").and_then(|pr| w_child(tree, pr, "gridBefore"));
        Ok(decimal_attr(tree, gb, "val")?.unwrap_or(0))
    }

    /// Upstream `_Row.grid_cols_after` (`w:gridAfter`).
    pub fn grid_cols_after(&self, doc: &Document) -> Result<i64> {
        let tree = self.live(doc)?;
        let ga =
            w_child(tree, self.h.element, "trPr").and_then(|pr| w_child(tree, pr, "gridAfter"));
        Ok(decimal_attr(tree, ga, "val")?.unwrap_or(0))
    }

    /// Upstream `_Row.height` (EMU).
    pub fn height(&self, doc: &Document) -> Result<Option<i64>> {
        let tree = self.live(doc)?;
        let h = w_child(tree, self.h.element, "trPr").and_then(|pr| w_child(tree, pr, "trHeight"));
        opt_twips_attr(tree, h, "val", false)
    }

    /// Upstream `_Row.height_rule` (XML token).
    pub fn height_rule(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        let h = w_child(tree, self.h.element, "trPr").and_then(|pr| w_child(tree, pr, "trHeight"));
        match h.and_then(|h| w_attr(tree, h, "hRule")) {
            None => Ok(None),
            Some(token) => {
                check_token(
                    token,
                    ROW_HEIGHT_RULES,
                    "WD_ROW_HEIGHT_RULE",
                    "invalid_height_rule",
                )?;
                Ok(Some(token.to_string()))
            }
        }
    }

    /// Upstream `_Row.height = value` (EMU; `None` removes).
    pub fn set_height(&self, doc: &mut Document, value: Option<i64>) -> Result<()> {
        let xml = value.map(|emu| twips_measure(emu, "w:val")).transpose()?;
        self.edit(doc, |tree, tr| {
            let tr_pr = get_or_add_trpr(tree, tr)?;
            if xml.is_none() && w_child(tree, tr_pr, "trHeight").is_none() {
                return Ok(());
            }
            let h = get_or_add(tree, tr_pr, "trHeight", TRPR_SEQ)?;
            set_opt(tree, h, "val", xml)
        })
    }

    /// Upstream `_Row.height_rule = value` (token; `None` removes).
    pub fn set_height_rule(&self, doc: &mut Document, value: Option<&str>) -> Result<()> {
        if let Some(token) = value {
            check_token(
                token,
                ROW_HEIGHT_RULES,
                "WD_ROW_HEIGHT_RULE",
                "invalid_height_rule",
            )?;
        }
        self.edit(doc, |tree, tr| {
            let tr_pr = get_or_add_trpr(tree, tr)?;
            if value.is_none() && w_child(tree, tr_pr, "trHeight").is_none() {
                return Ok(());
            }
            let h = get_or_add(tree, tr_pr, "trHeight", TRPR_SEQ)?;
            set_opt(tree, h, "hRule", value.map(str::to_string))
        })
    }

    /// Position among the table's rows (upstream `_Row._index`).
    pub fn index(&self, doc: &Document) -> Result<usize> {
        let tree = self.live(doc)?;
        w_children(tree, self.table, "tr")
            .position(|tr| tr == self.h.element)
            .ok_or_else(|| Error::internal("row_index", "row not in its table"))
    }
}

impl Column {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "gridCol", "column")
    }

    /// Upstream `_Column.width` (EMU).
    pub fn width(&self, doc: &Document) -> Result<Option<i64>> {
        let tree = self.live(doc)?;
        opt_twips_attr(tree, Some(self.h.element), "w", false)
    }

    /// Upstream `_Column.width = value` (EMU; `None` removes).
    pub fn set_width(&self, doc: &mut Document, value: Option<i64>) -> Result<()> {
        let xml = value.map(|emu| twips_measure(emu, "w:w")).transpose()?;
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        set_opt(txn.tree_mut(&self.h.part)?, self.h.element, "w", xml)?;
        commit!(doc, txn, ())
    }

    /// Position in the grid (upstream `_Column._index`).
    pub fn index(&self, doc: &Document) -> Result<usize> {
        let tree = self.live(doc)?;
        let parent = tree
            .parent(self.h.element)
            .ok_or_else(|| Error::internal("column_index", "detached column"))?;
        w_children(tree, parent, "gridCol")
            .position(|c| c == self.h.element)
            .ok_or_else(|| Error::internal("column_index", "column not in its grid"))
    }

    /// Upstream `_Column.cells`.
    pub fn cells(&self, doc: &Document) -> Result<Vec<Cell>> {
        let index = self.index(doc)?;
        let tree = self.live(doc)?;
        let table = Table {
            h: Handle::capture(doc, &self.h.part, tree, self.table),
        };
        table.column_cells(doc, index)
    }
}

impl Cell {
    fn edit(
        &self,
        doc: &mut Document,
        f: impl FnOnce(&mut XmlTree, NodeId) -> Result<()>,
    ) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        f(txn.tree_mut(&self.h.part)?, self.h.element)?;
        commit!(doc, txn, ())
    }

    /// The cell as a block container (paragraphs, tables, adding content).
    pub fn container(&self, doc: &Document) -> Result<BlockContainer> {
        self.live(doc)?;
        Ok(BlockContainer::new(self.h.clone(), ContainerKind::Cell))
    }

    /// Upstream `_Cell.paragraphs`.
    pub fn paragraphs(&self, doc: &Document) -> Result<Vec<Paragraph>> {
        self.container(doc)?.paragraphs(doc)
    }

    /// Upstream `_Cell.tables`.
    pub fn tables(&self, doc: &Document) -> Result<Vec<Table>> {
        self.container(doc)?.tables(doc)
    }

    /// Upstream `_Cell.add_paragraph(text, style)`.
    pub fn add_paragraph(
        &self,
        doc: &mut Document,
        text: &str,
        style: Option<&str>,
    ) -> Result<Paragraph> {
        self.container(doc)?.add_paragraph(doc, text, style)
    }

    /// Upstream `_Cell.add_table(rows, cols)`: sized from the cell width
    /// (1 inch without one), followed by an empty paragraph.
    pub fn add_table(&self, doc: &mut Document, rows: usize, cols: usize) -> Result<Table> {
        let tree = self.live(doc)?;
        let width = tc_width(tree, self.h.element)?.unwrap_or(914_400);
        let mut txn = Txn::new(doc);
        let tc = self.h.element;
        let tbl = add_container_table(
            &mut txn,
            &self.h.part,
            tc,
            ContainerKind::Cell,
            rows,
            cols,
            width,
        )?;
        add_container_paragraph(&mut txn, &self.h.part, tc, ContainerKind::Cell)?;
        let handle = staged_handle(&txn, &self.h.part, tbl)?;
        commit!(doc, txn, Table { h: handle })
    }

    /// Upstream `_Cell.grid_span`.
    pub fn grid_span(&self, doc: &Document) -> Result<i64> {
        let tree = self.live(doc)?;
        let gs = w_child(tree, self.h.element, "tcPr").and_then(|pr| w_child(tree, pr, "gridSpan"));
        Ok(decimal_attr(tree, gs, "val")?.unwrap_or(1))
    }

    /// Upstream `_Cell.width` (EMU; `dxa` widths only).
    pub fn width(&self, doc: &Document) -> Result<Option<i64>> {
        tc_width(self.live(doc)?, self.h.element)
    }

    /// Upstream `_Cell.width = value` (EMU).
    pub fn set_width(&self, doc: &mut Document, value: i64) -> Result<()> {
        self.edit(doc, |tree, tc| set_cell_width(tree, tc, value))
    }

    /// Upstream `_Cell.vertical_alignment` (XML token).
    pub fn vertical_alignment(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        let va = w_child(tree, self.h.element, "tcPr").and_then(|pr| w_child(tree, pr, "vAlign"));
        match va.and_then(|v| w_attr(tree, v, "val")) {
            None => Ok(None),
            Some(token) => {
                check_token(
                    token,
                    CELL_VERTICAL_ALIGNMENTS,
                    "WD_CELL_VERTICAL_ALIGNMENT",
                    "invalid_vertical_alignment",
                )?;
                Ok(Some(token.to_string()))
            }
        }
    }

    /// Upstream `_Cell.vertical_alignment = value` (token; `None` removes).
    pub fn set_vertical_alignment(&self, doc: &mut Document, value: Option<&str>) -> Result<()> {
        if let Some(token) = value {
            check_token(
                token,
                CELL_VERTICAL_ALIGNMENTS,
                "WD_CELL_VERTICAL_ALIGNMENT",
                "invalid_vertical_alignment",
            )?;
        }
        self.edit(doc, |tree, tc| {
            let tc_pr = get_or_add_first(tree, tc, "tcPr")?;
            match value {
                None => remove_all(tree, tc_pr, "vAlign"),
                Some(token) => {
                    let va = get_or_add(tree, tc_pr, "vAlign", TCPR_SEQ)?;
                    set_w(tree, va, "val", token)
                }
            }
        })
    }

    /// Upstream `_Cell.merge(other_cell)`: the rectangle spanned by the two
    /// cells becomes one cell (content moved into its top-left cell); returns
    /// that cell. Invalid spans are `invalid_argument(invalid_span)`
    /// (upstream `InvalidSpanError`); nothing changes on error.
    pub fn merge(&self, doc: &mut Document, other: &Cell) -> Result<Cell> {
        let tree = self.live(doc)?;
        other.live(doc)?;
        let tc = self.h.element;
        let other_tc = other.h.element;
        let tbl_of = |tc: NodeId| tree.ancestors(tc).find(|n| tree.is(*n, W, "tbl"));
        let tbl = tbl_of(tc).ok_or_else(|| Error::internal("merge", "cell outside a table"))?;
        if other.h.part != self.h.part || tbl_of(other_tc) != Some(tbl) {
            return Err(Error::refusal(
                RefusalKind::BoundaryViolation,
                "cells_in_different_tables",
                "cells can only be merged with cells of the same table",
            ));
        }
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let (top, left, height, width) =
            MergeGeometry { tree, tbl }.span_dimensions(tc, other_tc)?;
        let trs: Vec<NodeId> = w_children(tree, tbl, "tr").collect();
        let tr = *trs
            .get(top)
            .ok_or_else(|| index_error("list index out of range"))?;
        let top_tc = MergeGeometry { tree, tbl }.tc_at_grid_offset(tr, left)?;
        grow_to(tree, tbl, top_tc, width, height, None)?;
        let handle = staged_handle(&txn, &self.h.part, top_tc)?;
        commit!(doc, txn, Cell { h: handle })
    }
}

// -------------------------------------------------------- inline shapes

const CHART_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const DIAGRAM_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
const MAX_POSITIVE_COORDINATE: i128 = 27_273_042_316_900;

/// A live `wp:inline` handle (upstream `InlineShape`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineShapeRef {
    pub(crate) h: Handle,
}

/// `WD_INLINE_SHAPE` member name of an inline shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum InlineShapeType {
    /// `PICTURE`.
    Picture,
    /// `LINKED_PICTURE`.
    LinkedPicture,
    /// `CHART`.
    Chart,
    /// `SMART_ART`.
    SmartArt,
    /// `NOT_IMPLEMENTED`.
    NotImplemented,
}

impl InlineShapeType {
    /// Upstream member name.
    pub fn name(self) -> &'static str {
        match self {
            InlineShapeType::Picture => "PICTURE",
            InlineShapeType::LinkedPicture => "LINKED_PICTURE",
            InlineShapeType::Chart => "CHART",
            InlineShapeType::SmartArt => "SMART_ART",
            InlineShapeType::NotImplemented => "NOT_IMPLEMENTED",
        }
    }
}

fn required_child(tree: &XmlTree, parent: NodeId, ns: &str, local: &str) -> Result<NodeId> {
    tree.first_child_named(parent, ns, local).ok_or_else(|| {
        Error::invalid_argument(
            "missing_required_child",
            format!("required ``{local}`` child element not present"),
        )
    })
}

fn positive_coordinate(tree: &XmlTree, element: NodeId, local: &str) -> Result<i64> {
    let raw = tree.attr(element, None, local).ok_or_else(|| {
        Error::invalid_argument(
            "missing_required_attribute",
            format!("required '{local}' attribute not present"),
        )
    })?;
    crate::styles::py_int(raw).ok_or_else(|| {
        Error::invalid_argument(
            "invalid_decimal",
            format!("invalid literal for int() with base 10: {raw:?}"),
        )
    })
}

impl Document {
    /// Upstream `Document.inline_shapes`: every
    /// `//w:p/w:r/w:drawing/wp:inline` of the main part, in document order.
    pub fn inline_shapes(&self) -> Result<Vec<InlineShapeRef>> {
        let part = self.package().main_part_name().to_string();
        let tree = self.package().xml(&part)?;
        let parent_is = |node: NodeId, ns: &str, local: &str| {
            tree.parent(node).filter(|p| tree.is(*p, ns, local))
        };
        Ok(tree
            .descendants(tree.root())
            .filter(|node| tree.is(*node, WP, "inline"))
            .filter(|node| {
                parent_is(*node, W, "drawing")
                    .and_then(|d| parent_is(d, W, "r"))
                    .and_then(|r| parent_is(r, W, "p"))
                    .is_some()
            })
            .map(|inline| InlineShapeRef {
                h: Handle::capture(self, &part, tree, inline),
            })
            .collect())
    }
}

impl InlineShapeRef {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check_in(doc, WP, "inline", "inline shape")
    }

    /// Upstream `InlineShape.width` (`wp:extent/@cx`, EMU).
    pub fn width(&self, doc: &Document) -> Result<i64> {
        let tree = self.live(doc)?;
        let extent = required_child(tree, self.h.element, WP, "extent")?;
        positive_coordinate(tree, extent, "cx")
    }

    /// Upstream `InlineShape.height` (`wp:extent/@cy`, EMU).
    pub fn height(&self, doc: &Document) -> Result<i64> {
        let tree = self.live(doc)?;
        let extent = required_child(tree, self.h.element, WP, "extent")?;
        positive_coordinate(tree, extent, "cy")
    }

    /// Upstream `InlineShape.type`.
    pub fn shape_type(&self, doc: &Document) -> Result<InlineShapeType> {
        let tree = self.live(doc)?;
        let graphic = required_child(tree, self.h.element, A, "graphic")?;
        let data = required_child(tree, graphic, A, "graphicData")?;
        Ok(match tree.attr(data, None, "uri") {
            Some(PICTURE_URI) => {
                let blip = tree
                    .first_child_named(data, PIC, "pic")
                    .and_then(|pic| tree.first_child_named(pic, PIC, "blipFill"))
                    .and_then(|fill| tree.first_child_named(fill, A, "blip"))
                    .ok_or_else(|| {
                        Error::invalid_argument(
                            "no_picture_element",
                            "the inline picture has no pic:pic/pic:blipFill/a:blip",
                        )
                    })?;
                if tree.attr(blip, Some(R), "link").is_some() {
                    InlineShapeType::LinkedPicture
                } else {
                    InlineShapeType::Picture
                }
            }
            Some(CHART_URI) => InlineShapeType::Chart,
            Some(DIAGRAM_URI) => InlineShapeType::SmartArt,
            _ => InlineShapeType::NotImplemented,
        })
    }

    fn set_extent(&self, doc: &mut Document, value: i64, attr: &str) -> Result<()> {
        check_range(value, 0, MAX_POSITIVE_COORDINATE, attr)?;
        let tree = self.live(doc)?;
        let inline = self.h.element;
        let extent = required_child(tree, inline, WP, "extent")?;
        let graphic = required_child(tree, inline, A, "graphic")?;
        let data = required_child(tree, graphic, A, "graphicData")?;
        let pic = tree.first_child_named(data, PIC, "pic").ok_or_else(|| {
            Error::invalid_argument(
                "no_picture_element",
                "'NoneType' object has no attribute 'spPr' (the inline shape is not a picture)",
            )
        })?;
        let sp_pr = required_child(tree, pic, PIC, "spPr")?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let text = value.to_string();
        tree.set_attr(extent, None, attr, &text)?;
        // `spPr.cx = value`: get_or_add_xfrm, then get_or_add_ext.
        let xfrm = match tree.first_child_named(sp_pr, A, "xfrm") {
            Some(existing) => existing,
            None => {
                let xfrm = tree.create_element(sp_pr, A, "xfrm")?;
                let successors = [
                    "custGeom",
                    "prstGeom",
                    "ln",
                    "effectLst",
                    "effectDag",
                    "scene3d",
                    "sp3d",
                    "extLst",
                ];
                let index = tree.children(sp_pr).iter().position(|n| {
                    tree.ns_uri(*n) == Some(A)
                        && tree.local_name(*n).is_some_and(|l| successors.contains(&l))
                });
                match index {
                    Some(index) => tree.insert(sp_pr, index, xfrm)?,
                    None => append(tree, sp_pr, xfrm)?,
                }
                xfrm
            }
        };
        let ext = match tree.first_child_named(xfrm, A, "ext") {
            Some(existing) => existing,
            None => {
                let ext = tree.create_element(xfrm, A, "ext")?;
                append(tree, xfrm, ext)?;
                ext
            }
        };
        tree.set_attr(ext, None, attr, &text)?;
        commit!(doc, txn, ())
    }

    /// Upstream `InlineShape.width = cx`: `wp:extent/@cx` and the picture's
    /// `a:xfrm/a:ext/@cx`. A non-picture shape refuses before anything is
    /// written (upstream raises `AttributeError` after setting the extent).
    pub fn set_width(&self, doc: &mut Document, cx: i64) -> Result<()> {
        self.set_extent(doc, cx, "cx")
    }

    /// Upstream `InlineShape.height = cy`.
    pub fn set_height(&self, doc: &mut Document, cy: i64) -> Result<()> {
        self.set_extent(doc, cy, "cy")
    }
}

// ------------------------------------------------------------- tab stops

/// `WD_TAB_ALIGNMENT` XML tokens.
pub const TAB_ALIGNMENTS: &[&str] = &[
    "left", "center", "right", "decimal", "bar", "list", "clear", "end", "num", "start",
];
/// `WD_TAB_LEADER` XML tokens (`none` is the default, `SPACES`).
pub const TAB_LEADERS: &[&str] = &["none", "dot", "hyphen", "underscore", "heavy", "middleDot"];

/// A live `w:pPr/w:tabs/w:tab` handle (upstream `TabStop`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabStop {
    pub(crate) h: Handle,
}

/// Upstream `TabStop` getters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TabStopValues {
    /// `position` (EMU; negative is left of the indent).
    pub position: i64,
    /// `alignment` (XML token).
    pub alignment: String,
    /// `leader` (XML token; `none` by default).
    pub leader: String,
}

fn tab_values(tree: &XmlTree, tab: NodeId) -> Result<TabStopValues> {
    let raw_pos = w_attr(tree, tab, "pos").ok_or_else(|| {
        Error::invalid_argument(
            "missing_required_attribute",
            "required 'w:pos' attribute not present",
        )
    })?;
    let alignment = w_attr(tree, tab, "val").ok_or_else(|| {
        Error::invalid_argument(
            "missing_required_attribute",
            "required 'w:val' attribute not present",
        )
    })?;
    check_token(
        alignment,
        TAB_ALIGNMENTS,
        "WD_TAB_ALIGNMENT",
        "invalid_tab_alignment",
    )?;
    let leader = w_attr(tree, tab, "leader").unwrap_or("none");
    check_token(leader, TAB_LEADERS, "WD_TAB_LEADER", "invalid_tab_leader")?;
    Ok(TabStopValues {
        position: parse_signed_twips(raw_pos)?,
        alignment: alignment.to_string(),
        leader: leader.to_string(),
    })
}

/// `CT_TabStops.insert_tab_in_order(pos, align, leader)`.
fn insert_tab_in_order(
    tree: &mut XmlTree,
    tabs: NodeId,
    pos: i64,
    alignment: &str,
    leader: &str,
) -> Result<NodeId> {
    let pos_xml = signed_twips_measure(pos, "w:pos")?;
    let tab = new_w(tree, tabs, "tab")?;
    set_w(tree, tab, "pos", &pos_xml)?;
    set_w(tree, tab, "val", alignment)?;
    if leader != "none" {
        set_w(tree, tab, "leader", leader)?;
    }
    let new_pos = parse_signed_twips(&pos_xml)?;
    let existing: Vec<NodeId> = w_children(tree, tabs, "tab").collect();
    for other in existing {
        let raw = w_attr(tree, other, "pos").ok_or_else(|| {
            Error::invalid_argument(
                "missing_required_attribute",
                "required 'w:pos' attribute not present",
            )
        })?;
        if new_pos < parse_signed_twips(raw)? {
            let index = tree
                .index_in_parent(other)
                .ok_or_else(|| Error::internal("tab_index", "tab without index"))?;
            tree.insert(tabs, index, tab)?;
            return Ok(tab);
        }
    }
    append(tree, tabs, tab)?;
    Ok(tab)
}

fn validate_tab(alignment: &str, leader: &str) -> Result<()> {
    check_token(
        alignment,
        TAB_ALIGNMENTS,
        "WD_TAB_ALIGNMENT",
        "invalid_tab_alignment",
    )?;
    check_token(leader, TAB_LEADERS, "WD_TAB_LEADER", "invalid_tab_leader")
}

impl Paragraph {
    /// Upstream `ParagraphFormat.tab_stops` (read; never creates `w:pPr`).
    pub fn tab_stops(&self, doc: &Document) -> Result<Vec<TabStop>> {
        let tree = self.live(doc)?;
        let tabs = w_child(tree, self.h.element, "pPr").and_then(|ppr| w_child(tree, ppr, "tabs"));
        Ok(tabs
            .map(|tabs| {
                w_children(tree, tabs, "tab")
                    .map(|tab| TabStop {
                        h: Handle::capture(doc, &self.h.part, tree, tab),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Upstream `TabStops.add_tab_stop(position, alignment, leader)` (EMU,
    /// `WD_TAB_ALIGNMENT` / `WD_TAB_LEADER` tokens), kept in position order.
    pub fn add_tab_stop(
        &self,
        doc: &mut Document,
        position: i64,
        alignment: &str,
        leader: &str,
    ) -> Result<TabStop> {
        validate_tab(alignment, leader)?;
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let ppr = get_or_add_ppr(tree, self.h.element)?;
        let tabs = get_or_add(tree, ppr, "tabs", PPR_SEQ)?;
        let tab = insert_tab_in_order(tree, tabs, position, alignment, leader)?;
        let handle = staged_handle(&txn, &self.h.part, tab)?;
        commit!(doc, txn, TabStop { h: handle })
    }

    /// Upstream `TabStops.clear_all()` (accessing `tab_stops` adds `w:pPr`).
    pub fn clear_tab_stops(&self, doc: &mut Document) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let ppr = get_or_add_ppr(tree, self.h.element)?;
        remove_all(tree, ppr, "tabs")?;
        commit!(doc, txn, ())
    }

    /// Upstream `del tab_stops[index]` (Python indexing over the children of
    /// `w:tabs`; `w:tabs` goes when it becomes empty). Out of range is
    /// `invalid_argument(index_out_of_range)` (upstream `IndexError`).
    pub fn delete_tab_stop(&self, doc: &mut Document, index: i64) -> Result<()> {
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let ppr = get_or_add_ppr(tree, self.h.element)?;
        let tabs =
            w_child(tree, ppr, "tabs").ok_or_else(|| index_error("tab index out of range"))?;
        let children: Vec<NodeId> = crate::wml::node_children(tree, tabs).collect();
        let len = children.len() as i64;
        let real = if index < 0 { len + index } else { index };
        if real < 0 || real >= len {
            return Err(index_error("tab index out of range"));
        }
        remove(tree, children[real as usize])?;
        if crate::wml::node_children(tree, tabs).next().is_none() {
            remove(tree, tabs)?;
        }
        commit!(doc, txn, ())
    }
}

impl TabStop {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        let tree = self.h.check(doc, "tab", "tab stop")?;
        let in_tabs = tree
            .parent(self.h.element)
            .is_some_and(|tabs| tree.is(tabs, W, "tabs"));
        if !in_tabs {
            return Err(Error::refusal(
                RefusalKind::TargetNotFound,
                "stale_handle",
                "tab stop is stale: its element was removed, moved or replaced; reacquire it",
            ));
        }
        Ok(tree)
    }

    /// Upstream `TabStop` getters.
    pub fn values(&self, doc: &Document) -> Result<TabStopValues> {
        tab_values(self.live(doc)?, self.h.element)
    }

    /// Upstream `TabStop.position = value`: a new `w:tab` with the same
    /// alignment and leader goes in position order and the old one is
    /// removed; returns the new handle (this one becomes stale).
    pub fn set_position(&self, doc: &mut Document, position: i64) -> Result<TabStop> {
        let tree = self.live(doc)?;
        let current = tab_values(tree, self.h.element)?;
        let tabs = tree
            .parent(self.h.element)
            .ok_or_else(|| Error::internal("tab_parent", "tab without parent"))?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let tab = insert_tab_in_order(tree, tabs, position, &current.alignment, &current.leader)?;
        remove(tree, self.h.element)?;
        let handle = staged_handle(&txn, &self.h.part, tab)?;
        commit!(doc, txn, TabStop { h: handle })
    }

    /// Upstream `TabStop.alignment = value` (token).
    pub fn set_alignment(&self, doc: &mut Document, alignment: &str) -> Result<()> {
        check_token(
            alignment,
            TAB_ALIGNMENTS,
            "WD_TAB_ALIGNMENT",
            "invalid_tab_alignment",
        )?;
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        set_w(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            "val",
            alignment,
        )?;
        commit!(doc, txn, ())
    }

    /// Upstream `TabStop.leader = value` (token; `None` or `none` removes
    /// the attribute, as the default).
    pub fn set_leader(&self, doc: &mut Document, leader: Option<&str>) -> Result<()> {
        if let Some(token) = leader {
            check_token(token, TAB_LEADERS, "WD_TAB_LEADER", "invalid_tab_leader")?;
        }
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        set_opt(
            tree,
            self.h.element,
            "leader",
            leader.filter(|t| *t != "none").map(str::to_string),
        )?;
        commit!(doc, txn, ())
    }
}

// --------------------------------------------------- comment ranges, paths

impl Run {
    /// Upstream `Run.mark_comment_range(last_run, comment_id)`: a
    /// `w:commentRangeStart` before this run and `w:commentRangeEnd` plus a
    /// `w:commentReference` run after `last`. Runs in different parts refuse
    /// `unsupported_structure(comment_range_cross_story)`; an end before the
    /// start refuses `unsupported_structure(comment_range_reversed)`.
    pub fn mark_comment_range(
        &self,
        doc: &mut Document,
        last: &Run,
        comment_id: i64,
    ) -> Result<()> {
        let tree = self.live(doc)?;
        last.live(doc)?;
        if last.h.part != self.h.part {
            return Err(Error::refusal(
                RefusalKind::UnsupportedStructure,
                "comment_range_cross_story",
                "comment range runs belong to different documents; nothing was changed",
            ));
        }
        for node in tree.descendants(tree.root()) {
            if node == self.h.element {
                break;
            }
            if node == last.h.element {
                return Err(Error::refusal(
                    RefusalKind::UnsupportedStructure,
                    "comment_range_reversed",
                    "comment range ends before it starts; nothing was changed",
                ));
            }
        }
        let mut txn = Txn::new(doc);
        crate::commentops::mark_comment_range(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            last.h.element,
            comment_id,
        )?;
        commit!(doc, txn, ())
    }

    /// Upstream `Run.add_picture(path, width, height)` for a file path: the
    /// picture's `pic:cNvPr/@name` is the file's base name.
    pub fn add_picture_file(
        &self,
        doc: &mut Document,
        path: impl AsRef<std::path::Path>,
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<InlineShape> {
        let (bytes, name) = read_image_file(path.as_ref())?;
        self.live(doc)?;
        let mut txn = Txn::new(doc);
        let shape = add_picture_named(
            &mut txn,
            &self.h.part,
            self.h.element,
            &bytes,
            Some(&name),
            width,
            height,
        )?;
        commit!(doc, txn, shape)
    }
}

fn read_image_file(path: &std::path::Path) -> Result<(Vec<u8>, String)> {
    let bytes = std::fs::read(path).map_err(|err| {
        Error::io(
            "read_image",
            format!("cannot read image {}", path.display()),
            err,
        )
    })?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((bytes, name))
}

impl Document {
    /// Upstream `Document.add_picture(path, width, height)` for a file path.
    pub fn add_picture_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<InlineShape> {
        let (bytes, name) = read_image_file(path.as_ref())?;
        let part = self.package().main_part_name().to_string();
        let mut txn = Txn::new(self);
        let tree = txn.tree_mut(&part)?;
        let p = add_body_paragraph(tree)?;
        let r = new_w(tree, p, "r")?;
        append(tree, p, r)?;
        let shape = add_picture_named(&mut txn, &part, r, &bytes, Some(&name), width, height)?;
        commit!(self, txn, shape)
    }
}

// ------------------------------------------------ paragraph / run content

/// A live `w:hyperlink` handle (upstream `Hyperlink`, read side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperlinkRef {
    pub(crate) h: Handle,
}

/// A live `w:lastRenderedPageBreak` handle (upstream `RenderedPageBreak`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPageBreak {
    pub(crate) h: Handle,
}

/// A detached paragraph fragment (upstream returns a deep copy of the
/// paragraph with the content on the other side of the page break removed).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ParagraphFragment {
    /// `Paragraph.text` of the fragment.
    pub text: String,
    /// Text of each direct `w:r` of the fragment.
    pub runs: Vec<String>,
}

/// An item of `Paragraph.iter_inner_content()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParagraphItem {
    /// A `w:r`.
    Run(Run),
    /// A `w:hyperlink`.
    Hyperlink(HyperlinkRef),
}

/// An item of `Run.iter_inner_content()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunItem {
    /// Accumulated run text between drawings and page breaks.
    Text(String),
    /// A `w:drawing`.
    Drawing(crate::drawing::Drawing),
    /// A `w:lastRenderedPageBreak`.
    RenderedPageBreak(RenderedPageBreak),
}

fn lrpbs_of(tree: &XmlTree, p: NodeId) -> Vec<NodeId> {
    // `./w:r/w:lastRenderedPageBreak | ./w:hyperlink/w:r/w:lastRenderedPageBreak`
    let mut out = Vec::new();
    for child in tree.child_elements(p) {
        let runs: Vec<NodeId> = if tree.is(child, W, "r") {
            vec![child]
        } else if tree.is(child, W, "hyperlink") {
            w_children(tree, child, "r").collect()
        } else {
            Vec::new()
        };
        for r in runs {
            out.extend(w_children(tree, r, "lastRenderedPageBreak"));
        }
    }
    out
}

const RUN_INNER: &[&str] = &["br", "cr", "drawing", "noBreakHyphen", "ptab", "t", "tab"];

fn is_run_inner(tree: &XmlTree, node: NodeId) -> bool {
    tree.ns_uri(node) == Some(W)
        && tree
            .local_name(node)
            .is_some_and(|l| RUN_INNER.contains(&l))
}

/// Text and run texts of the children `items` of a (virtual) paragraph.
fn fragment(tree: &XmlTree, items: &[FragmentItem]) -> ParagraphFragment {
    let mut text = String::new();
    let mut runs = Vec::new();
    for item in items {
        match item {
            FragmentItem::Element(node) => {
                if tree.is(*node, W, "r") {
                    let t = run_text(tree, *node);
                    text.push_str(&t);
                    runs.push(t);
                } else if tree.is(*node, W, "hyperlink") {
                    for r in w_children(tree, *node, "r") {
                        text.push_str(&run_text(tree, r));
                    }
                }
            }
            FragmentItem::PartialRun(children) => {
                let t: String = children
                    .iter()
                    .filter_map(|c| run_child_text(tree, *c))
                    .collect();
                text.push_str(&t);
                runs.push(t);
            }
        }
    }
    ParagraphFragment { text, runs }
}

enum FragmentItem {
    Element(NodeId),
    PartialRun(Vec<NodeId>),
}

impl RenderedPageBreak {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h
            .check(doc, "lastRenderedPageBreak", "rendered page break")
    }

    fn context(&self, tree: &XmlTree) -> Result<(NodeId, NodeId, Option<NodeId>)> {
        let lrpb = self.h.element;
        let p = tree
            .ancestors(lrpb)
            .find(|n| tree.is(*n, W, "p"))
            .ok_or_else(|| Error::internal("lrpb", "page break outside a paragraph"))?;
        let first = lrpbs_of(tree, p).first().copied().ok_or_else(|| {
            Error::invalid_argument(
                "no_rendered_page_break",
                "no rendered page-breaks in paragraph element",
            )
        })?;
        if first != lrpb {
            return Err(Error::invalid_argument(
                "not_first_page_break",
                "only defined on first rendered page-break in paragraph",
            ));
        }
        let r = tree
            .parent(lrpb)
            .filter(|r| tree.is(*r, W, "r"))
            .ok_or_else(|| Error::internal("lrpb", "page break outside a run"))?;
        let hyperlink = tree.parent(r).filter(|h| tree.is(*h, W, "hyperlink"));
        Ok((p, r, hyperlink))
    }

    /// Upstream `preceding_paragraph_fragment`: the paragraph content before
    /// this (first) page break, `None` when the break precedes all content.
    pub fn preceding_paragraph_fragment(
        &self,
        doc: &Document,
    ) -> Result<Option<ParagraphFragment>> {
        let tree = self.live(doc)?;
        let (p, r, hyperlink) = self.context(tree)?;
        let lrpb = self.h.element;
        if hyperlink.is_none() {
            let first_run = w_children(tree, p, "r").next();
            let precedes_all = first_run == Some(r)
                && tree
                    .child_elements(r)
                    .take_while(|n| *n != lrpb)
                    .all(|n| !is_run_inner(tree, n));
            if precedes_all {
                return Ok(None);
            }
        }
        let mut items = Vec::new();
        let anchor = hyperlink.unwrap_or(r);
        for child in tree.child_elements(p) {
            if child == anchor {
                break;
            }
            items.push(FragmentItem::Element(child));
        }
        match hyperlink {
            Some(link) => items.push(FragmentItem::Element(link)),
            None => items.push(FragmentItem::PartialRun(
                tree.child_elements(r).take_while(|n| *n != lrpb).collect(),
            )),
        }
        Ok(Some(fragment(tree, &items)))
    }

    /// Upstream `following_paragraph_fragment`: the paragraph content after
    /// this (first) page break, `None` when it follows all content.
    pub fn following_paragraph_fragment(
        &self,
        doc: &Document,
    ) -> Result<Option<ParagraphFragment>> {
        let tree = self.live(doc)?;
        let (p, r, hyperlink) = self.context(tree)?;
        let lrpb = self.h.element;
        if hyperlink.is_none() {
            let last_run = w_children(tree, p, "r").last();
            // `(./w:r)[last()]/w:lastRenderedPageBreak[not(following-sibling::*[inner])]`
            let follows_all = last_run.is_some_and(|last| {
                w_children(tree, last, "lastRenderedPageBreak").any(|candidate| {
                    tree.child_elements(last)
                        .skip_while(|n| *n != candidate)
                        .skip(1)
                        .all(|n| !is_run_inner(tree, n))
                })
            });
            if follows_all {
                return Ok(None);
            }
        }
        let anchor = hyperlink.unwrap_or(r);
        let mut items = Vec::new();
        if hyperlink.is_none() {
            items.push(FragmentItem::PartialRun(
                tree.child_elements(r)
                    .skip_while(|n| *n != lrpb)
                    .skip(1)
                    .collect(),
            ));
        }
        for child in tree.child_elements(p).skip_while(|n| *n != anchor).skip(1) {
            items.push(FragmentItem::Element(child));
        }
        Ok(Some(fragment(tree, &items)))
    }
}

impl HyperlinkRef {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "hyperlink", "hyperlink")
    }

    /// Upstream `Hyperlink.address`: the target of `r:id` (`""` without one).
    pub fn address(&self, doc: &Document) -> Result<String> {
        let tree = self.live(doc)?;
        let Some(r_id) = tree.attr(self.h.element, Some(R), "id") else {
            return Ok(String::new());
        };
        let source = format!("/{}", self.h.part);
        let rels = doc.package().relationships_at(&source)?;
        rels.iter()
            .find(|rel| rel.id == r_id)
            .map(|rel| rel.target.clone())
            .ok_or_else(|| {
                Error::invalid_argument(
                    "missing_key",
                    format!("no relationship {r_id:?} in {source}"),
                )
            })
    }

    /// Upstream `Hyperlink.address = value`: point the hyperlink at a new
    /// external URL and clear its `w:anchor` (an internal jump becomes an
    /// external link). The old relationship is dropped when this hyperlink
    /// was its only `r:id` reference (python-docx `Part.drop_rel`). Refuses a
    /// protected document and an empty address.
    pub fn set_address(&self, doc: &mut Document, value: &str) -> Result<()> {
        self.live(doc)?;
        refuse_if_protected(doc, "retarget a hyperlink", OperationClass::BodyContent)?;
        if value.is_empty() {
            return Err(Error::invalid_argument(
                "empty_address",
                "address must be a non-empty URL",
            ));
        }
        let mut txn = Txn::new(doc);
        let old_r_id = txn
            .tree(&self.h.part)?
            .attr(self.h.element, Some(R), "id")
            .map(str::to_string);
        if let Some(old) = &old_r_id {
            // `del self.rels[rId]` raises for a missing relationship.
            if !txn
                .relationships(&self.h.part)?
                .iter()
                .any(|rel| &rel.id == old)
            {
                return Err(Error::invalid_argument(
                    "missing_key",
                    format!("no relationship {old:?} in /{}", self.h.part),
                ));
            }
        }
        let new_r_id = txn.relate_to(&self.h.part, rt::HYPERLINK, RelTarget::External(value))?;
        if let Some(old) = old_r_id.filter(|old| *old != new_r_id) {
            let references = {
                let tree = txn.tree(&self.h.part)?;
                tree.descendants(tree.root())
                    .filter(|node| tree.attr(*node, Some(R), "id") == Some(old.as_str()))
                    .count()
            };
            if references < 2 {
                crate::revision::drop_rel(&mut txn, &self.h.part, &old)?;
            }
        }
        let tree = txn.tree_mut(&self.h.part)?;
        tree.set_attr(self.h.element, Some(R), "id", &new_r_id)?;
        tree.remove_attr(self.h.element, Some(W), "anchor");
        commit!(doc, txn, ())
    }

    /// Upstream `Hyperlink.fragment` (`w:anchor`, `""` without one).
    pub fn fragment(&self, doc: &Document) -> Result<String> {
        let tree = self.live(doc)?;
        Ok(w_attr(tree, self.h.element, "anchor")
            .unwrap_or("")
            .to_string())
    }

    /// Upstream `Hyperlink.url`: `address#fragment`, `address`, or `""`.
    pub fn url(&self, doc: &Document) -> Result<String> {
        let address = self.address(doc)?;
        if address.is_empty() {
            return Ok(String::new());
        }
        let fragment = self.fragment(doc)?;
        Ok(if fragment.is_empty() {
            address
        } else {
            format!("{address}#{fragment}")
        })
    }

    /// Upstream `Hyperlink.text`.
    pub fn text(&self, doc: &Document) -> Result<String> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "r")
            .map(|r| run_text(tree, r))
            .collect())
    }

    /// Upstream `Hyperlink.runs`.
    pub fn runs(&self, doc: &Document) -> Result<Vec<Run>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "r")
            .map(|r| Run {
                h: Handle::capture(doc, &self.h.part, tree, r),
            })
            .collect())
    }

    /// Upstream `Hyperlink.contains_page_break`.
    pub fn contains_page_break(&self, doc: &Document) -> Result<bool> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "r").any(|r| {
            w_children(tree, r, "lastRenderedPageBreak")
                .next()
                .is_some()
        }))
    }
}

impl Paragraph {
    /// Upstream `Paragraph.iter_inner_content()`: direct runs and hyperlinks.
    pub fn inner_content(&self, doc: &Document) -> Result<Vec<ParagraphItem>> {
        let tree = self.live(doc)?;
        Ok(tree
            .child_elements(self.h.element)
            .filter_map(|child| {
                let h = Handle::capture(doc, &self.h.part, tree, child);
                if tree.is(child, W, "r") {
                    Some(ParagraphItem::Run(Run { h }))
                } else if tree.is(child, W, "hyperlink") {
                    Some(ParagraphItem::Hyperlink(HyperlinkRef { h }))
                } else {
                    None
                }
            })
            .collect())
    }

    /// Upstream `Paragraph.hyperlinks`.
    pub fn hyperlinks(&self, doc: &Document) -> Result<Vec<HyperlinkRef>> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "hyperlink")
            .map(|link| HyperlinkRef {
                h: Handle::capture(doc, &self.h.part, tree, link),
            })
            .collect())
    }

    /// Upstream `Paragraph.rendered_page_breaks`.
    pub fn rendered_page_breaks(&self, doc: &Document) -> Result<Vec<RenderedPageBreak>> {
        let tree = self.live(doc)?;
        Ok(lrpbs_of(tree, self.h.element)
            .into_iter()
            .map(|lrpb| RenderedPageBreak {
                h: Handle::capture(doc, &self.h.part, tree, lrpb),
            })
            .collect())
    }

    /// Upstream `Paragraph.contains_page_break`.
    pub fn contains_page_break(&self, doc: &Document) -> Result<bool> {
        Ok(!lrpbs_of(self.live(doc)?, self.h.element).is_empty())
    }
}

impl Run {
    /// Upstream `Run.iter_inner_content()`: text runs between drawings and
    /// rendered page breaks.
    pub fn inner_content(&self, doc: &Document) -> Result<Vec<RunItem>> {
        let tree = self.live(doc)?;
        let mut out = Vec::new();
        let mut text = String::new();
        for child in tree.child_elements(self.h.element) {
            let h = || Handle::capture(doc, &self.h.part, tree, child);
            if tree.is(child, W, "drawing") || tree.is(child, W, "lastRenderedPageBreak") {
                if !text.is_empty() {
                    out.push(RunItem::Text(std::mem::take(&mut text)));
                }
                out.push(if tree.is(child, W, "drawing") {
                    RunItem::Drawing(crate::drawing::Drawing { h: h() })
                } else {
                    RunItem::RenderedPageBreak(RenderedPageBreak { h: h() })
                });
            } else if let Some(piece) = run_child_text(tree, child) {
                text.push_str(&piece);
            }
        }
        if !text.is_empty() {
            out.push(RunItem::Text(text));
        }
        Ok(out)
    }

    /// Upstream `Run.contains_page_break`.
    pub fn contains_page_break(&self, doc: &Document) -> Result<bool> {
        let tree = self.live(doc)?;
        Ok(w_children(tree, self.h.element, "lastRenderedPageBreak")
            .next()
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partname_idx_follows_packuri() {
        assert_eq!(partname_idx("word/media/image3.png"), Some(3));
        assert_eq!(partname_idx("word/media/image.png"), None);
        assert_eq!(partname_idx("word/media/image01.png"), None);
        assert_eq!(partname_idx("word/media/img2x.png"), Some(2));
        assert_eq!(partname_idx("word/media/2image.png"), None);
    }

    #[test]
    fn universal_measures_convert_like_python() {
        assert_eq!(parse_twips_measure("1440").unwrap(), 914_400);
        assert_eq!(parse_twips_measure("1in").unwrap(), 914_400);
        assert_eq!(parse_twips_measure("2.5cm").unwrap(), 900_000);
    }
}
