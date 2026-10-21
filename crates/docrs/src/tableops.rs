//! Narrow, guarded table operations (upstream `tableops.py`).
//!
//! Guards are cell-wise: an operation refuses when the cells or rows it
//! actually touches participate in a merge or hold a nested table — a merged
//! header row elsewhere does not block edits to plain data cells. Cell
//! updates route through the replacement machinery (`replace.rs`), so
//! formatting inheritance and refusal atomicity are the same as for body
//! text; tracked cell updates author `w:ins`/`w:del` exactly as upstream
//! `Span.replace(tracked=True)` does for a cell's text.
//!
//! Tables are addressed through a live [`Table`] handle from
//! [`find_table`]: document identity, the `w:tbl` element and freshness
//! evidence (story root, parent, structural generation). Row and cell
//! addresses are python-docx's: 0-based rows, layout-grid columns, merged
//! cells repeated per spanned grid column and vertically continued cells
//! resolved to the cell that starts the merge.

use std::collections::HashSet;

use crate::clock::Timestamp;
use crate::document::DocumentId;
use crate::normalize::{content_hash, normalize_text};
use crate::oxml::{add_t, append, w_el};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::{
    ReplaceOptions, ReplaceReport, canonical, narrow_to_change, ordinary_outcome,
    placeholder_controls, refuse_intervening_positional_nodes, validate_fresh,
    validate_one_inline_ancestry, validate_replaceable, validate_writable_text,
};
use crate::search::{Atom, MatchPolicy, Span, collect_block_atoms, fresh_span_token, signature};
use crate::story::{self, Anchor, BlockKind, View, iter_block_elements};
use crate::styles::py_int;
use crate::txn::Txn;
use crate::unicode::{is_py_space, py_strip};
use crate::wml::{
    W, is_w, leading_text, node_children, set_leading_text, w_attr, w_child, w_children,
};
use crate::xml::{NodeData, NodeId, XML_NS, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

fn not_found(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::TargetNotFound, reason, message)
}

fn malformed_table(message: impl Into<String>) -> Error {
    // python-docx raises a plain ValueError for these shapes.
    Error::invalid_argument("malformed_table_grid", message)
}

// ---------------------------------------------------------------------------
// The live handle
// ---------------------------------------------------------------------------

/// A live table handle (upstream `docx.table.Table` as returned by
/// `find_table`). It stays valid across this module's own row and cell
/// edits; moving, removing or replacing its `w:tbl` (or its story part)
/// makes it stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    document: DocumentId,
    part: String,
    root: NodeId,
    element: NodeId,
    parent: Option<NodeId>,
    struct_gen: u32,
}

impl Table {
    /// Package member holding the table.
    pub fn part(&self) -> &str {
        &self.part
    }

    /// Owning document.
    pub fn document_id(&self) -> DocumentId {
        self.document
    }

    fn capture(document: &Document, part: &str, tree: &XmlTree, element: NodeId) -> Table {
        Table {
            document: document.id(),
            part: part.to_string(),
            root: tree.root(),
            element,
            parent: tree.parent(element),
            struct_gen: tree.struct_gen(element),
        }
    }
}

/// Validate a live handle against `document`; returns the `w:tbl` element.
fn locate(document: &Document, table: &Table) -> Result<NodeId> {
    if table.document != document.id() {
        return Err(Error::refusal(
            RefusalKind::BoundaryViolation,
            "foreign_table",
            "table belongs to a different document; find it in the document passed to this operation",
        ));
    }
    let stale = || {
        not_found(
            "stale_table",
            "table handle is stale: its w:tbl was removed, moved or its story part replaced; find the table again",
        )
    };
    if document.package().part(&table.part).is_none() {
        return Err(stale());
    }
    let tree = document.package().xml(&table.part)?;
    let element = table.element;
    if tree.root() != table.root
        || !tree.contains(element)
        || !tree.is_attached(element)
        || tree.parent(element) != table.parent
        || tree.struct_gen(element) != table.struct_gen
        || !is_w(tree, element, "tbl")
    {
        return Err(stale());
    }
    Ok(element)
}

// ---------------------------------------------------------------------------
// python-docx table model
// ---------------------------------------------------------------------------

fn tc_pr(tree: &XmlTree, tc: NodeId) -> Option<NodeId> {
    w_child(tree, tc, "tcPr")
}

fn decimal_val(tree: &XmlTree, element: NodeId, what: &str) -> Result<i64> {
    let raw = w_attr(tree, element, "val")
        .ok_or_else(|| malformed_table(format!("required w:val attribute missing on w:{what}")))?;
    py_int(raw).ok_or_else(|| {
        malformed_table(format!(
            "invalid literal for int() with base 10: {raw:?} (w:{what})"
        ))
    })
}

/// `CT_Tc.grid_span` (default 1).
fn grid_span(tree: &XmlTree, tc: NodeId) -> Result<i64> {
    match tc_pr(tree, tc).and_then(|pr| w_child(tree, pr, "gridSpan")) {
        Some(span) => decimal_val(tree, span, "gridSpan"),
        None => Ok(1),
    }
}

/// `CT_Tc.vMerge`: `None` without `w:vMerge`; its `w:val` defaults to
/// `continue`.
fn v_merge(tree: &XmlTree, tc: NodeId) -> Option<&str> {
    let element = w_child(tree, tc_pr(tree, tc)?, "vMerge")?;
    Some(w_attr(tree, element, "val").unwrap_or("continue"))
}

fn row_property(tree: &XmlTree, tr: NodeId, local: &str) -> Result<i64> {
    match w_child(tree, tr, "trPr").and_then(|pr| w_child(tree, pr, local)) {
        Some(element) => decimal_val(tree, element, local),
        None => Ok(0),
    }
}

/// `CT_Row.grid_before`.
fn grid_before(tree: &XmlTree, tr: NodeId) -> Result<i64> {
    row_property(tree, tr, "gridBefore")
}

/// `CT_Row.grid_after`.
fn grid_after(tree: &XmlTree, tr: NodeId) -> Result<i64> {
    row_property(tree, tr, "gridAfter")
}

fn rows(tree: &XmlTree, tbl: NodeId) -> Vec<NodeId> {
    w_children(tree, tbl, "tr").collect()
}

fn row_tcs(tree: &XmlTree, tr: NodeId) -> Vec<NodeId> {
    w_children(tree, tr, "tc").collect()
}

/// `CT_Row.tc_at_grid_offset`.
fn tc_at_grid_offset(tree: &XmlTree, tr: NodeId, offset: i64) -> Result<NodeId> {
    let mut remaining = offset - grid_before(tree, tr)?;
    for tc in row_tcs(tree, tr) {
        if remaining < 0 {
            break;
        }
        if remaining == 0 {
            return Ok(tc);
        }
        remaining -= grid_span(tree, tc)?;
    }
    Err(malformed_table(format!(
        "no `tc` element at grid_offset={offset}"
    )))
}

/// `CT_Tc._tc_above`.
fn tc_above(tree: &XmlTree, tc: NodeId) -> Result<NodeId> {
    let tr = tree
        .ancestors(tc)
        .find(|node| is_w(tree, *node, "tr"))
        .ok_or_else(|| malformed_table("w:tc outside a w:tr"))?;
    let parent = tree
        .parent(tr)
        .ok_or_else(|| malformed_table("no tr above topmost tr in w:tbl"))?;
    let siblings: Vec<NodeId> = w_children(tree, parent, "tr").collect();
    let position = siblings.iter().position(|row| *row == tr).unwrap_or(0);
    if position == 0 {
        return Err(malformed_table("no tr above topmost tr in w:tbl"));
    }
    let mut offset = grid_before(tree, tr)?;
    for sibling in row_tcs(tree, tr) {
        if sibling == tc {
            break;
        }
        offset += grid_span(tree, sibling)?;
    }
    tc_at_grid_offset(tree, siblings[position - 1], offset)
}

/// `_Row.cells`: the `w:tc` for each populated layout-grid cell (spans
/// repeated, vertical continuations resolved upward).
fn row_cells(tree: &XmlTree, tr: NodeId) -> Result<Vec<NodeId>> {
    let mut out = Vec::new();
    for tc in row_tcs(tree, tr) {
        let mut current = tc;
        while v_merge(tree, current) == Some("continue") {
            current = tc_above(tree, current)?;
        }
        for _ in 0..grid_span(tree, current)?.max(0) {
            out.push(current);
        }
    }
    Ok(out)
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

/// python-docx `CT_P.text` (direct runs and hyperlink runs).
fn paragraph_text(tree: &XmlTree, paragraph: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(paragraph) {
        if is_w(tree, child, "r") {
            out.push_str(&run_text(tree, child));
        } else if is_w(tree, child, "hyperlink") {
            for run in w_children(tree, child, "r") {
                out.push_str(&run_text(tree, run));
            }
        }
    }
    out
}

fn cell_paragraphs(tree: &XmlTree, tc: NodeId) -> Vec<NodeId> {
    w_children(tree, tc, "p").collect()
}

/// `_Cell.text`.
fn cell_text(tree: &XmlTree, tc: NodeId) -> String {
    cell_paragraphs(tree, tc)
        .into_iter()
        .map(|p| paragraph_text(tree, p))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `[[cell.text for cell in row.cells] for row in table.rows]`.
fn grid_texts(tree: &XmlTree, tbl: NodeId) -> Result<Vec<Vec<String>>> {
    rows(tree, tbl)
        .into_iter()
        .map(|tr| {
            Ok(row_cells(tree, tr)?
                .into_iter()
                .map(|tc| cell_text(tree, tc))
                .collect())
        })
        .collect()
}

fn body_tables(document: &Document) -> Result<(String, Vec<NodeId>)> {
    let part = document.package().main_part_name().to_string();
    let tree = document.package().xml(&part)?;
    let tables = match w_child(tree, tree.root(), "body") {
        Some(body) => w_children(tree, body, "tbl").collect(),
        None => Vec::new(),
    };
    Ok((part, tables))
}

// ---------------------------------------------------------------------------
// find_table
// ---------------------------------------------------------------------------

/// The table with one physical cell containing `near_text` (upstream
/// `find_table`). Matching is literal by default; `Normalized` folds case,
/// typography and whitespace. Paragraphs within one cell join with `"\n"`;
/// a match never crosses cells. Only top-level body tables are searched.
pub fn find_table(document: &Document, near_text: &str, policy: MatchPolicy) -> Result<Table> {
    let needle = match policy {
        MatchPolicy::Exact => near_text.to_string(),
        MatchPolicy::Normalized => normalize_text(near_text),
    };
    if needle.is_empty() || (policy == MatchPolicy::Normalized && py_strip(&needle).is_empty()) {
        return Err(not_found(
            "empty_near_text",
            "near_text must contain searchable text; an empty target cannot identify a table",
        ));
    }
    let (part, tables) = body_tables(document)?;
    let tree = document.package().xml(&part)?;
    let mut matches = Vec::new();
    'tables: for tbl in tables {
        let mut seen: HashSet<NodeId> = HashSet::new();
        for tr in rows(tree, tbl) {
            for tc in row_cells(tree, tr)? {
                if !seen.insert(tc) {
                    continue;
                }
                let text = cell_text(tree, tc);
                let candidate = match policy {
                    MatchPolicy::Exact => text,
                    MatchPolicy::Normalized => normalize_text(&text),
                };
                if candidate.contains(&needle) {
                    matches.push(tbl);
                    continue 'tables;
                }
            }
        }
    }
    match matches.as_slice() {
        [] => Err(not_found(
            "no_table_match",
            format!("no table contains {near_text:?}"),
        )),
        [tbl] => Ok(Table::capture(document, &part, tree, *tbl)),
        _ => Err(Error::refusal(
            RefusalKind::AmbiguousTarget,
            "ambiguous_table",
            format!(
                "{} tables contain {near_text:?}; use more specific text",
                matches.len()
            ),
        )),
    }
}

/// Cell texts per row, python-docx `row.cells` semantics.
pub fn table_grid(document: &Document, table: &Table) -> Result<Vec<Vec<String>>> {
    let tbl = locate(document, table)?;
    grid_texts(document.package().xml(&table.part)?, tbl)
}

// ---------------------------------------------------------------------------
// Shared guards
// ---------------------------------------------------------------------------

/// `_MARKUP_SCAN_TAGS` (revision markup of every kind).
const MARKUP_SCAN_TAGS: &[&str] = &[
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
    tree.element(node).is_some()
        && tree.ns_uri(node) == Some(W)
        && tree
            .local_name(node)
            .is_some_and(|local| MARKUP_SCAN_TAGS.contains(&local))
}

fn first_markup_in(tree: &XmlTree, root: NodeId) -> Option<NodeId> {
    tree.descendants(root).find(|node| is_markup(tree, *node))
}

fn local_of(tree: &XmlTree, node: NodeId) -> String {
    match tree.data(node) {
        NodeData::Element(_) => tree.local_name(node).unwrap_or("").to_string(),
        NodeData::Comment(_) => "#comment".to_string(),
        NodeData::Pi { .. } => "#pi".to_string(),
        NodeData::Text(_) => "#text".to_string(),
    }
}

fn cell_is_merged(tree: &XmlTree, tc: NodeId) -> bool {
    tc_pr(tree, tc).is_some_and(|pr| {
        w_child(tree, pr, "gridSpan").is_some() || w_child(tree, pr, "vMerge").is_some()
    })
}

fn cell_has_nested_table(tree: &XmlTree, tc: NodeId) -> bool {
    w_child(tree, tc, "tbl").is_some()
}

fn row_has_vmerge(tree: &XmlTree, tr: NodeId) -> bool {
    row_tcs(tree, tr)
        .into_iter()
        .any(|tc| tc_pr(tree, tc).is_some_and(|pr| w_child(tree, pr, "vMerge").is_some()))
}

fn row_continues_merge_from_above(tree: &XmlTree, tr: NodeId) -> bool {
    row_tcs(tree, tr)
        .into_iter()
        .any(|tc| v_merge(tree, tc) == Some("continue"))
}

/// `_refuse_row_op`.
fn refuse_row_op(
    tree: &XmlTree,
    all_rows: &[NodeId],
    affected: usize,
    splits_before: Option<usize>,
) -> Result<()> {
    let tr = all_rows[affected];
    if row_has_vmerge(tree, tr) {
        return Err(unsupported(
            "vertical_merge_row",
            format!(
                "row {affected} participates in a vertical merge; structural row operations there would corrupt \
                 the merged region"
            ),
        ));
    }
    if row_tcs(tree, tr)
        .into_iter()
        .any(|tc| cell_has_nested_table(tree, tc))
    {
        return Err(unsupported(
            "nested_table_row",
            format!(
                "row {affected} contains a nested table; refusing rather than duplicating or destroying it wholesale"
            ),
        ));
    }
    if let Some(split) = splits_before
        && split < all_rows.len()
        && row_continues_merge_from_above(tree, all_rows[split])
    {
        return Err(unsupported(
            "splits_vertical_merge",
            format!(
                "inserting between rows {} and {split} would split a vertical merge",
                split - 1
            ),
        ));
    }
    Ok(())
}

fn row_out_of_range(row: usize, count: usize) -> Error {
    not_found(
        "row_out_of_range",
        format!("row {row} does not exist (0..{})", count as i64 - 1),
    )
}

// ---------------------------------------------------------------------------
// update_cell
// ---------------------------------------------------------------------------

/// Options for [`update_cell`].
#[derive(Debug, Clone, Default)]
pub struct UpdateCellOptions {
    /// Author the change as a tracked revision.
    pub tracked: bool,
    /// Revision author (required when tracked).
    pub author: Option<String>,
    /// Revision date (`None` → now).
    pub date: Option<Timestamp>,
}

fn is_xml_representable(c: char) -> bool {
    matches!(c as u32, 0x09 | 0x0A | 0x0D | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// Upstream `_validate_xml_characters`.
fn validate_xml_characters(value: &str, argument: &str) -> Result<()> {
    let mut invalid: Vec<String> = value
        .chars()
        .filter(|c| !is_xml_representable(*c))
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    invalid.sort();
    invalid.dedup();
    if invalid.is_empty() {
        Ok(())
    } else {
        Err(Error::invalid_argument(
            "invalid_xml_characters",
            format!("{argument} contains character(s) XML 1.0 cannot represent: {invalid:?}"),
        ))
    }
}

/// `_cell_at`: (`w:tc`, owning `w:tr`).
fn cell_at(tree: &XmlTree, tbl: NodeId, row: usize, column: usize) -> Result<NodeId> {
    let all_rows = rows(tree, tbl);
    let Some(tr) = all_rows.get(row).copied() else {
        return Err(row_out_of_range(row, all_rows.len()));
    };
    let offset = column as i64 - grid_before(tree, tr)?;
    let cells = row_cells(tree, tr)?;
    if offset < 0 || offset as usize >= cells.len() {
        return Err(not_found(
            "column_has_no_cell",
            format!("layout-grid column {column} has no cell in row {row}"),
        ));
    }
    Ok(cells[offset as usize])
}

fn refuse_complex_cell(tree: &XmlTree, tc: NodeId, row: usize, column: usize) -> Result<()> {
    if cell_is_merged(tree, tc) {
        return Err(unsupported(
            "merged_cell",
            format!(
                "cell ({row}, {column}) participates in a merge (gridSpan/vMerge); editing it would silently \
                 reshuffle the merged region"
            ),
        ));
    }
    if cell_has_nested_table(tree, tc) {
        return Err(unsupported(
            "nested_table_cell",
            format!(
                "cell ({row}, {column}) contains a nested table; address the inner table directly instead"
            ),
        ));
    }
    Ok(())
}

fn refuse_whole_cell_semantics(tree: &XmlTree, tc: NodeId) -> Result<()> {
    if tree
        .descendants(tc)
        .any(|node| is_w(tree, node, "fldChar") || is_w(tree, node, "fldSimple"))
    {
        return Err(unsupported(
            "cell_field",
            "cell contains a Word field; update its field/result explicitly",
        ));
    }
    if first_markup_in(tree, tc).is_some() {
        return Err(unsupported(
            "cell_revision",
            "cell contains pending revision markup; resolve it before replacing the cell",
        ));
    }
    Ok(())
}

fn locate_table_block(document: &Document, tbl: NodeId, part: &str) -> Result<(String, usize)> {
    for story in story::stories(document) {
        if story.part != part {
            continue;
        }
        let tree = document.package().xml(&story.part)?;
        for block in iter_block_elements(tree, &story.name)? {
            if block.kind == BlockKind::Table && block.element == tbl {
                return Ok((story.name, block.index));
            }
        }
    }
    Err(not_found(
        "table_not_in_traversal",
        "table is not part of this document's traversal",
    ))
}

/// Replace one cell's visible text (upstream `update_cell`; 0-based `row`,
/// layout-grid `column`). A non-empty replacement inherits the cell text's
/// starting run properties. Refuses a protected document, an out-of-range
/// address, and merged, nested, field-bearing, revision-bearing,
/// multi-paragraph or marker/wrapper-divided targets — atomically.
pub fn update_cell(
    document: &mut Document,
    table: &Table,
    row: usize,
    column: usize,
    new_text: &str,
    options: &UpdateCellOptions,
) -> Result<ReplaceReport> {
    if options.tracked && options.author.as_deref().is_none_or(str::is_empty) {
        return Err(Error::invalid_argument(
            "author_required",
            "author is required when tracked=True",
        ));
    }
    validate_writable_text(new_text, "new_text")?;
    let tbl = locate(document, table)?;
    refuse_if_protected(document, "update a table cell", OperationClass::BodyContent)?;
    let part = table.part.clone();
    let tree = document.package().xml(&part)?;
    let tc = cell_at(tree, tbl, row, column)?;
    refuse_complex_cell(tree, tc, row, column)?;
    refuse_whole_cell_semantics(tree, tc)?;
    let paragraphs = cell_paragraphs(tree, tc);
    let populated: Vec<NodeId> = paragraphs
        .iter()
        .copied()
        .filter(|p| !paragraph_text(tree, *p).is_empty())
        .collect();
    if populated.len() > 1 {
        return Err(unsupported(
            "multiple_paragraphs",
            "cell holds multiple paragraphs of text; update them individually through docx.search spans",
        ));
    }
    let (story_name, block_index) = locate_table_block(document, tbl, &part)?;
    let Some(target) = populated.first().or(paragraphs.first()).copied() else {
        return Err(unsupported(
            "cell_without_paragraph",
            "cell has no paragraph to hold text",
        ));
    };
    let mut all_atoms = Vec::new();
    let mut depth = 0u64;
    collect_block_atoms(
        tree,
        block_index,
        target,
        false,
        false,
        &mut depth,
        &mut all_atoms,
    );
    if all_atoms.iter().any(Atom::is_synthetic) {
        return Err(unsupported(
            "cell_non_text_content",
            "cell text contains a tab, break, no-break hyphen, or other visible run content that cannot be \
             represented by new_text; update its text segments individually instead",
        ));
    }
    let atoms: Vec<Atom> = all_atoms
        .into_iter()
        .filter(|atom| atom.is_tag("t"))
        .collect();
    if atoms.is_empty() {
        if tree.descendants(tc).any(|node| is_w(tree, node, "sdt")) {
            return Err(unsupported(
                "empty_cell_control",
                "empty cell contains a content control; set it through Control.set_value() instead",
            ));
        }
        return fill_empty_cell(document, &part, &story_name, target, new_text, options);
    }
    let text: String = atoms.iter().map(|atom| atom.text(tree)).collect();
    let end_offset = atoms[atoms.len() - 1].text(tree).chars().count();
    let span = Span {
        document: document.id(),
        token: fresh_span_token(),
        story: story_name.clone(),
        part: part.clone(),
        anchor: Anchor {
            story: story_name.clone(),
            index: block_index,
            content_hash: content_hash(&text),
        },
        in_insert: atoms.iter().any(|a| a.in_insert),
        in_delete: false,
        in_content_control: atoms.iter().any(|a| a.sdt.is_some()),
        in_text_box: atoms.iter().any(|a| a.in_text_box),
        in_field: atoms.iter().any(|a| a.in_field),
        crosses_paragraphs: false,
        signatures: atoms.iter().map(|a| signature(tree, a)).collect(),
        atom_sequence: atoms.iter().map(|a| a.element).collect(),
        sequence_view: Some(View::Current),
        start_offset: 0,
        end_offset,
        raw_start: 0,
        match_start: 0,
        match_policy: None,
        view: View::Current,
        text,
        atoms,
    };
    if options.tracked {
        let author = options.author.clone().unwrap_or_default();
        return tracked_replace(document, &span, new_text, &author, options.date);
    }
    crate::replace::replace(document, &span, new_text, &ReplaceOptions::default())
}

fn next_revision_id(document: &Document) -> Result<i64> {
    const REVISION_TAGS: &[&str] = &[
        "ins",
        "del",
        "moveFrom",
        "moveTo",
        "moveFromRangeStart",
        "moveToRangeStart",
        "rPrChange",
        "pPrChange",
        "tblPrChange",
        "sectPrChange",
        "cellIns",
        "cellDel",
        "cellMerge",
    ];
    let mut highest = 0i64;
    for story in story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        for node in tree.descendants(tree.root()) {
            if tree.element(node).is_none()
                || tree.ns_uri(node) != Some(W)
                || !tree
                    .local_name(node)
                    .is_some_and(|local| REVISION_TAGS.contains(&local))
            {
                continue;
            }
            if let Some(value) = w_attr(tree, node, "id")
                .filter(|v| !v.is_empty())
                .and_then(py_int)
            {
                highest = highest.max(value);
            }
        }
    }
    Ok(highest + 1)
}

/// `CT_RunTrackChange.new(tag, id, author, date)`.
fn new_track_change(
    tree: &mut XmlTree,
    scope: NodeId,
    tag: &str,
    id: i64,
    author: &str,
    date: Timestamp,
) -> Result<NodeId> {
    let element = w_el(tree, scope, tag)?;
    tree.set_attr(element, Some(W), "id", &id.to_string())?;
    tree.set_attr(element, Some(W), "author", author)?;
    tree.set_attr(element, Some(W), "date", &date.to_xml())?;
    Ok(element)
}

/// `CT_RunTrackChange.add_tracked_run`.
fn add_tracked_run(
    tree: &mut XmlTree,
    change: NodeId,
    text: &str,
    rpr: Option<NodeId>,
    deleted: bool,
) -> Result<NodeId> {
    let run = w_el(tree, change, "r")?;
    append(tree, change, run)?;
    if let Some(rpr) = rpr {
        let copy = tree.deep_clone(rpr);
        tree.insert(run, 0, copy)?;
    }
    if deleted {
        let del_text = w_el(tree, run, "delText")?;
        if !text.is_empty() {
            let node = tree.create_text(text);
            tree.insert(del_text, 0, node)?;
        }
        if text.chars().next().is_some_and(is_py_space)
            || text.chars().last().is_some_and(is_py_space)
        {
            tree.set_attr(del_text, Some(XML_NS), "space", "preserve")?;
        }
        append(tree, run, del_text)?;
    } else {
        add_t(tree, run, text)?;
    }
    Ok(run)
}

fn fill_empty_cell(
    document: &mut Document,
    part: &str,
    story_name: &str,
    paragraph: NodeId,
    new_text: &str,
    options: &UpdateCellOptions,
) -> Result<ReplaceReport> {
    let mut report = ReplaceReport {
        story: story_name.to_string(),
        deleted_text: String::new(),
        inserted_text: new_text.to_string(),
        tracked: options.tracked,
        revision_ids: Vec::new(),
        preserved_revision_ids: Vec::new(),
        preserved_formatting_regions: false,
        changed: true,
    };
    let revision_id = if options.tracked {
        validate_xml_characters(options.author.as_deref().unwrap_or(""), "author")?;
        Some(next_revision_id(document)?)
    } else {
        None
    };
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(part)?;
    match revision_id {
        None => {
            // `paragraph.add_run(new_text)`.
            let run = w_el(tree, paragraph, "r")?;
            if !new_text.is_empty() {
                add_t(tree, run, new_text)?;
            }
            append(tree, paragraph, run)?;
        }
        Some(id) => {
            let stamp = options.date.unwrap_or_else(Timestamp::now);
            let author = options.author.as_deref().unwrap_or("");
            let ins = new_track_change(tree, paragraph, "ins", id, author, stamp)?;
            add_tracked_run(tree, ins, new_text, None, false)?;
            append(tree, paragraph, ins)?;
            report.revision_ids.push(id);
        }
    }
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Tracked cell replacement (upstream `Span._replace(tracked=True)` →
// `_tracked_replace`, for the revision-free cell spans update_cell builds)
// ---------------------------------------------------------------------------

/// Upstream `_hollowed_bookmarks(atoms, end_offset)`.
fn hollowed_bookmarks(tree: &XmlTree, span: &Span) -> Vec<String> {
    let Some(paragraph) = span.atoms.first().and_then(|a| a.paragraph) else {
        return Vec::new();
    };
    let mut emptied: HashSet<NodeId> = HashSet::new();
    if span.atoms.len() > 2 {
        emptied.extend(
            span.atoms[1..span.atoms.len() - 1]
                .iter()
                .map(|a| a.element),
        );
    }
    let last = &span.atoms[span.atoms.len() - 1];
    if span.atoms.len() > 1 && span.end_offset >= last.text(tree).chars().count() {
        emptied.insert(last.element);
    }
    if emptied.is_empty() {
        return Vec::new();
    }
    let stream: Vec<NodeId> = crate::wml::iter_nodes(tree, paragraph).collect();
    let mut starts: Vec<(Option<String>, usize, String)> = Vec::new();
    let mut hollowed = Vec::new();
    for (position, node) in stream.iter().enumerate() {
        if is_w(tree, *node, "bookmarkStart") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let name = w_attr(tree, *node, "name").unwrap_or("").to_string();
            starts.retain(|(existing, _, _)| *existing != id);
            starts.push((id, position, name));
        } else if is_w(tree, *node, "bookmarkEnd") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let Some((_, start, name)) = starts.iter().find(|(existing, _, _)| *existing == id)
            else {
                continue;
            };
            if name == "_GoBack" {
                continue;
            }
            let texts: Vec<NodeId> = stream[start + 1..position]
                .iter()
                .copied()
                .filter(|inner| {
                    (is_w(tree, *inner, "t") || is_w(tree, *inner, "delText"))
                        && !leading_text(tree, *inner).is_empty()
                })
                .collect();
            if !texts.is_empty() && texts.iter().all(|inner| emptied.contains(inner)) {
                hollowed.push(name.clone());
            }
        }
    }
    hollowed
}

fn set_preserved_text(tree: &mut XmlTree, element: NodeId, text: &str) -> Result<()> {
    set_leading_text(tree, element, text)?;
    if text.chars().next().is_some_and(is_py_space) || text.chars().last().is_some_and(is_py_space)
    {
        tree.set_attr(element, Some(XML_NS), "space", "preserve")?;
    } else {
        tree.remove_attr(element, Some(XML_NS), "space");
    }
    Ok(())
}

/// Position just after `node` and its lxml tail (following text nodes).
fn after_with_tail(tree: &XmlTree, node: NodeId) -> Result<(NodeId, usize)> {
    let parent = tree
        .parent(node)
        .ok_or_else(|| Error::internal("detached_anchor", "insertion anchor is detached"))?;
    let siblings = tree.children(parent);
    let mut index = siblings
        .iter()
        .position(|child| *child == node)
        .ok_or_else(|| {
            Error::internal("detached_anchor", "anchor not among its parent's children")
        })?
        + 1;
    while index < siblings.len() && matches!(tree.data(siblings[index]), NodeData::Text(_)) {
        index += 1;
    }
    Ok((parent, index))
}

/// lxml `anchor.addnext(new)`.
fn add_next(tree: &mut XmlTree, anchor: NodeId, new: NodeId) -> Result<()> {
    let (parent, index) = after_with_tail(tree, anchor)?;
    tree.insert(parent, index, new)
}

/// `(run, text)` pieces intersected with `[start, end)`, consecutive
/// same-run pieces merged (upstream `_pieces_in_range`).
fn pieces_in_range(
    pieces: &[(Option<NodeId>, String)],
    start: usize,
    end: usize,
) -> Vec<(Option<NodeId>, String)> {
    let mut out: Vec<(Option<NodeId>, String)> = Vec::new();
    let mut position = 0usize;
    for (run, text) in pieces {
        let length = text.chars().count();
        let lo = start.max(position);
        let hi = end.min(position + length);
        if hi > lo {
            let fragment: String = text.chars().skip(lo - position).take(hi - lo).collect();
            match out.last_mut() {
                Some(last) if last.0 == *run => last.1.push_str(&fragment),
                _ => out.push((*run, fragment)),
            }
        }
        position += length;
    }
    out
}

fn slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn tracked_replace(
    document: &mut Document,
    span: &Span,
    new_text: &str,
    author: &str,
    date: Option<Timestamp>,
) -> Result<ReplaceReport> {
    validate_xml_characters(author, "author")?;
    validate_writable_text(new_text, "new_text")?;
    refuse_if_protected(document, "replace text", OperationClass::BodyContent)?;
    let tree = document.package().xml(&span.part)?;
    validate_fresh(document, tree, span, None)?;
    if span.text == new_text {
        return Err(not_found(
            "tracked_noop",
            "replacement equals the existing text; nothing to change",
        ));
    }
    // Upstream validates, then narrows the span to the changed interval.
    validate_replaceable(tree, span, false)?;
    if let Some((sub, sub_new)) = narrow_to_change(tree, span, new_text)? {
        return tracked_replace(document, &sub, &sub_new, author, date);
    }
    validate_replaceable(tree, span, true)?;
    let hollowed = hollowed_bookmarks(tree, span);
    if !hollowed.is_empty() {
        return Err(unsupported(
            "hollows_bookmark",
            format!(
                "replacing this span would hollow out bookmark(s) {hollowed:?} — the targets of REF/PAGEREF \
                 cross-references and TOC entries; narrow the span to inside the bookmark, or remove the \
                 bookmark deliberately first"
            ),
        ));
    }
    if !placeholder_controls(tree, &span.atoms).is_empty() {
        return Err(unsupported(
            "tracked_placeholder",
            "span lies in a form control still showing PLACEHOLDER prompt text; a tracked replacement would \
             fabricate a deletion of text that was never real content — fill the control untracked (or via \
             docx.controls) first",
        ));
    }
    if span.story == "word/comments.xml" {
        return Err(unsupported(
            "tracked_comment_text",
            "tracked changes inside comment text are not representable; edit the comment untracked or reply instead",
        ));
    }
    if span.atoms.iter().any(|atom| atom.run.is_none()) {
        return Err(unsupported(
            "not_in_run",
            "matched text is not inside a run; cannot anchor a revision",
        ));
    }
    let outcome = ordinary_outcome(tree, span, new_text)?;
    let total = span.text.chars().count();
    let prefix_len = outcome.changed_start;
    let suffix_len = total - outcome.changed_end;
    let old_mid = slice(&span.text, outcome.changed_start, outcome.changed_end);
    let new_mid = outcome.replacement.clone();
    let changed_atoms: Vec<&Atom> = outcome
        .segments
        .iter()
        .map(|(index, _, _)| &span.atoms[*index])
        .collect();
    refuse_intervening_positional_nodes(tree, &changed_atoms)?;
    validate_one_inline_ancestry(tree, &changed_atoms)?;
    let stamp = date.unwrap_or_else(Timestamp::now);
    let mut next_id = next_revision_id(document)?;
    let first = span.atoms[0].clone();
    let last = span.atoms[span.atoms.len() - 1].clone();
    let first_run = first
        .run
        .ok_or_else(|| Error::internal("tracked_run", "atom lost its run"))?;
    let rpr = w_child(tree, first_run, "rPr");
    let atom_count = span.atoms.len();
    let span_pieces: Vec<(Option<NodeId>, String)> = span
        .atoms
        .iter()
        .enumerate()
        .map(|(i, atom)| {
            let text = atom.text(tree);
            let length = text.chars().count();
            let start = if i == 0 { span.start_offset } else { 0 };
            let end = if i == atom_count - 1 {
                span.end_offset
            } else {
                length
            };
            (atom.run, slice(&text, start, end))
        })
        .collect();
    let deleted_pieces = pieces_in_range(&span_pieces, prefix_len, total - suffix_len);
    let ins_source_run = changed_atoms
        .first()
        .and_then(|a| a.run)
        .unwrap_or(first_run);
    let ins_rpr = w_child(tree, ins_source_run, "rPr");
    let deleted_rprs: Vec<(Option<NodeId>, String)> = deleted_pieces
        .iter()
        .map(|(run, piece)| (run.and_then(|r| w_child(tree, r, "rPr")), piece.clone()))
        .collect();

    let first_text = first.text(tree);
    let before = slice(&first_text, 0, span.start_offset);
    let last_text = last.text(tree);
    let after = slice(&last_text, span.end_offset, last_text.chars().count());
    let prefix = slice(&span.text, 0, prefix_len);
    let suffix = if suffix_len > 0 {
        slice(&span.text, total - suffix_len, total)
    } else {
        String::new()
    };
    let same_run = first.run == last.run;
    let first_is_last = atom_count == 1;
    let tail_in_new_run = first_is_last || same_run;
    let middle: Vec<NodeId> = if atom_count > 2 {
        span.atoms[1..atom_count - 1]
            .iter()
            .map(|a| a.element)
            .collect()
    } else {
        Vec::new()
    };

    let part = span.part.clone();
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&part)?;
    set_preserved_text(tree, first.element, &format!("{before}{prefix}"))?;
    for element in &middle {
        set_preserved_text(tree, *element, "")?;
    }
    if !tail_in_new_run {
        set_preserved_text(tree, last.element, &format!("{suffix}{after}"))?;
    } else if !first_is_last {
        set_preserved_text(tree, last.element, "")?;
    }
    // Run content after the matched node moves behind the revision, each
    // moved node with its lxml tail.
    let nodes: Vec<NodeId> = node_children(tree, first_run).collect();
    let position = nodes.iter().position(|node| *node == first.element);
    let mut trailing: Vec<NodeId> = Vec::new();
    if let Some(position) = position {
        for node in &nodes[position + 1..] {
            if is_w(tree, *node, "rPr") {
                continue;
            }
            trailing.push(*node);
            let (_, end) = after_with_tail(tree, *node)?;
            let start = tree.index_in_parent(*node).unwrap_or(end) + 1;
            trailing.extend_from_slice(&tree.children(first_run)[start.min(end)..end]);
        }
    }
    let moved_run = if trailing.is_empty() {
        None
    } else {
        let moved = w_el(tree, first_run, "r")?;
        if let Some(rpr) = rpr {
            let copy = tree.deep_clone(rpr);
            append(tree, moved, copy)?;
        }
        for node in &trailing {
            tree.detach(*node)?;
            append(tree, moved, *node)?;
        }
        Some(moved)
    };
    let mut revision_ids = Vec::new();
    let mut insert_point = first_run;
    if !old_mid.is_empty() && !deleted_rprs.is_empty() {
        let del = new_track_change(tree, first_run, "del", next_id, author, stamp)?;
        for (source_rpr, piece) in &deleted_rprs {
            add_tracked_run(tree, del, piece, *source_rpr, true)?;
        }
        add_next(tree, insert_point, del)?;
        insert_point = del;
        revision_ids.push(next_id);
        next_id += 1;
    }
    if !new_mid.is_empty() {
        let ins = new_track_change(tree, first_run, "ins", next_id, author, stamp)?;
        add_tracked_run(tree, ins, &new_mid, ins_rpr, false)?;
        add_next(tree, insert_point, ins)?;
        insert_point = ins;
        revision_ids.push(next_id);
    }
    let tail = format!("{suffix}{after}");
    if tail_in_new_run && !tail.is_empty() {
        let run = w_el(tree, first_run, "r")?;
        if let Some(rpr) = rpr {
            let copy = tree.deep_clone(rpr);
            append(tree, run, copy)?;
        }
        add_t(tree, run, &tail)?;
        add_next(tree, insert_point, run)?;
        insert_point = run;
    }
    if let Some(moved) = moved_run {
        add_next(tree, insert_point, moved)?;
    }
    let staged = txn.finish()?;
    document.commit(staged)?;
    Ok(ReplaceReport {
        story: span.story.clone(),
        deleted_text: old_mid,
        inserted_text: new_mid,
        tracked: true,
        revision_ids,
        preserved_revision_ids: Vec::new(),
        preserved_formatting_regions: false,
        changed: true,
    })
}

// ---------------------------------------------------------------------------
// insert_row_after / delete_row
// ---------------------------------------------------------------------------

fn template_refusal(row: usize, column: usize, detail: &str) -> Error {
    unsupported(
        "complex_template_cell",
        format!(
            "template row {row} physical cell {column} {detail}; populating it would discard or flatten \
             structure or formatting. Use a simpler uniform template cell with exactly one paragraph of plain \
             text runs sharing identical complete direct run properties"
        ),
    )
}

/// `_refuse_tracked_template_row`: revision markup at row level (not
/// inside any cell).
fn refuse_tracked_template_row(tree: &XmlTree, tr: NodeId, row: usize) -> Result<()> {
    for node in tree.descendants(tr) {
        if !is_markup(tree, node) {
            continue;
        }
        let mut current = tree.parent(node);
        while let Some(ancestor) = current {
            if ancestor == tr || is_w(tree, ancestor, "tc") {
                break;
            }
            current = tree.parent(ancestor);
        }
        if current.is_none_or(|ancestor| ancestor == tr) {
            return Err(unsupported(
                "tracked_template_row",
                format!(
                    "template row {row} contains tracked revision metadata ({}); resolve its pending revisions \
                     before copying formatting from it",
                    local_of(tree, node)
                ),
            ));
        }
    }
    Ok(())
}

/// `_copied_cell_template_rpr`: the one proved direct `w:rPr` of a plain
/// template cell (or `None`), refusing complex cells.
fn copied_cell_template_rpr(
    tree: &XmlTree,
    tc: NodeId,
    row: usize,
    column: usize,
) -> Result<Option<NodeId>> {
    if let Some(revision) = first_markup_in(tree, tc) {
        return Err(template_refusal(
            row,
            column,
            &format!(
                "contains tracked revision metadata ({})",
                local_of(tree, revision)
            ),
        ));
    }
    let children: Vec<NodeId> = node_children(tree, tc).collect();
    let is_tc_pr = |node: NodeId| is_w(tree, node, "tcPr");
    let paragraphs: Vec<NodeId> = children
        .iter()
        .copied()
        .filter(|c| is_w(tree, *c, "p"))
        .collect();
    if let Some(unexpected) = children
        .iter()
        .copied()
        .find(|c| !is_tc_pr(*c) && !is_w(tree, *c, "p"))
    {
        let detail = if is_w(tree, unexpected, "sdt") {
            "contains a content control".to_string()
        } else {
            format!(
                "contains unsupported cell content ({})",
                local_of(tree, unexpected)
            )
        };
        return Err(template_refusal(row, column, &detail));
    }
    if paragraphs.len() != 1 {
        return Err(template_refusal(
            row,
            column,
            &format!(
                "has {} direct paragraphs instead of exactly one",
                paragraphs.len()
            ),
        ));
    }
    if children.iter().filter(|c| is_tc_pr(**c)).count() > 1 {
        return Err(template_refusal(
            row,
            column,
            "contains duplicate cell properties",
        ));
    }
    if let Some(pr) = children.iter().copied().find(|c| is_tc_pr(*c))
        && children[0] != pr
    {
        return Err(template_refusal(
            row,
            column,
            "has cell properties after cell content",
        ));
    }
    let paragraph = paragraphs[0];
    let p_children: Vec<NodeId> = node_children(tree, paragraph).collect();
    let is_ppr = |node: NodeId| is_w(tree, node, "pPr");
    if p_children.iter().filter(|c| is_ppr(**c)).count() > 1 {
        return Err(template_refusal(
            row,
            column,
            "contains duplicate paragraph properties",
        ));
    }
    if let Some(ppr) = p_children.iter().copied().find(|c| is_ppr(*c))
        && p_children[0] != ppr
    {
        return Err(template_refusal(
            row,
            column,
            "has paragraph properties after paragraph content",
        ));
    }
    if let Some(other) = p_children
        .iter()
        .copied()
        .find(|c| !is_ppr(*c) && !is_w(tree, *c, "r"))
    {
        return Err(template_refusal(
            row,
            column,
            &format!(
                "contains unsupported paragraph content ({})",
                local_of(tree, other)
            ),
        ));
    }
    let mut evidence: Vec<(String, Option<NodeId>)> = Vec::new();
    let mut has_explicit_empty_rpr = false;
    for run in p_children.iter().copied().filter(|c| is_w(tree, *c, "r")) {
        let run_children: Vec<NodeId> = node_children(tree, run).collect();
        let rprs: Vec<NodeId> = run_children
            .iter()
            .copied()
            .filter(|c| is_w(tree, *c, "rPr"))
            .collect();
        if rprs.len() > 1 || (!rprs.is_empty() && run_children[0] != rprs[0]) {
            return Err(template_refusal(
                row,
                column,
                "contains malformed or duplicate run properties",
            ));
        }
        if let Some(other) = run_children
            .iter()
            .copied()
            .find(|c| !is_w(tree, *c, "rPr") && !is_w(tree, *c, "t"))
        {
            return Err(template_refusal(
                row,
                column,
                &format!("contains non-text run content ({})", local_of(tree, other)),
            ));
        }
        let text_bearing = run_children
            .iter()
            .any(|c| is_w(tree, *c, "t") && !leading_text(tree, *c).is_empty());
        if !text_bearing {
            has_explicit_empty_rpr |= !rprs.is_empty();
            continue;
        }
        let rpr = rprs.first().copied();
        evidence.push((rpr.map(|r| canonical(tree, r)).unwrap_or_default(), rpr));
    }
    let Some((first_canonical, first_rpr)) = evidence.first().cloned() else {
        if has_explicit_empty_rpr {
            return Err(template_refusal(
                row,
                column,
                "has only styled empty runs, which do not prove formatting intent",
            ));
        }
        return Ok(None);
    };
    if evidence[1..]
        .iter()
        .any(|(canon, _)| *canon != first_canonical)
    {
        return Err(template_refusal(
            row,
            column,
            "has text runs with conflicting complete direct run properties",
        ));
    }
    Ok(first_rpr)
}

/// Insert a row after `row` (upstream `insert_row_after`), copying the
/// formatting of `copy_format_from` (default `row`) and filling `values`
/// positionally (missing values are empty). The template must be a
/// rectangular row of plain cells — one paragraph of plain text runs with
/// identical complete direct run properties each; complex or conflicting
/// templates refuse before anything changes.
pub fn insert_row_after<S: AsRef<str>>(
    document: &mut Document,
    table: &Table,
    row: usize,
    values: &[S],
    copy_format_from: Option<usize>,
) -> Result<()> {
    let tbl = locate(document, table)?;
    refuse_if_protected(document, "insert a table row", OperationClass::BodyContent)?;
    let part = table.part.clone();
    let tree = document.package().xml(&part)?;
    let all_rows = rows(tree, tbl);
    if row >= all_rows.len() {
        return Err(row_out_of_range(row, all_rows.len()));
    }
    let template_index = copy_format_from.unwrap_or(row);
    if template_index >= all_rows.len() {
        return Err(not_found(
            "copy_format_row_out_of_range",
            format!("copy_format_from row {template_index} does not exist"),
        ));
    }
    let template_tr = all_rows[template_index];
    let template_cells = row_tcs(tree, template_tr);
    let has_grid_span = template_cells
        .iter()
        .any(|tc| tc_pr(tree, *tc).is_some_and(|pr| w_child(tree, pr, "gridSpan").is_some()));
    let nonrectangular = grid_before(tree, template_tr)? != 0
        || grid_after(tree, template_tr)? != 0
        || (!has_grid_span && {
            let grid = w_child(tree, tbl, "tblGrid").ok_or_else(|| {
                unsupported(
                    "missing_table_grid",
                    "required ``w:tblGrid`` child element not present",
                )
            })?;
            template_cells.len() != w_children(tree, grid, "gridCol").count()
        });
    if nonrectangular {
        return Err(unsupported(
            "nonrectangular_template",
            format!(
                "template row {template_index} is nonrectangular: its physical cells do not map one-to-one to \
                 every table grid column (gridBefore/gridAfter or omitted columns); copy formatting from a \
                 complete rectangular row"
            ),
        ));
    }
    for (index, tc) in template_cells.iter().enumerate() {
        if cell_has_nested_table(tree, *tc) {
            return Err(template_refusal(
                template_index,
                index,
                "contains a nested table",
            ));
        }
    }
    refuse_row_op(tree, &all_rows, template_index, Some(row + 1))?;
    if has_grid_span {
        return Err(unsupported(
            "grid_span_template",
            format!(
                "template row {template_index} contains horizontally merged cells (gridSpan); positional values \
                 cannot be assigned unambiguously — copy formatting from an unmerged row"
            ),
        ));
    }
    for (position, value) in values.iter().enumerate() {
        validate_writable_text(value.as_ref(), &format!("values[{position}]"))?;
    }
    if values.len() > template_cells.len() {
        return Err(Error::invalid_argument(
            "too_many_values",
            format!(
                "{} values for a {}-column table",
                values.len(),
                template_cells.len()
            ),
        ));
    }
    refuse_tracked_template_row(tree, template_tr, template_index)?;
    let template_rprs = template_cells
        .iter()
        .enumerate()
        .map(|(index, tc)| copied_cell_template_rpr(tree, *tc, template_index, index))
        .collect::<Result<Vec<_>>>()?;
    let anchor_tr = all_rows[row];
    let template_tail: Option<String> = {
        let parent = tree.parent(template_tr);
        parent.and_then(|parent| {
            let siblings = tree.children(parent);
            let at = siblings.iter().position(|c| *c == template_tr)?;
            let mut tail = String::new();
            for sibling in &siblings[at + 1..] {
                match tree.data(*sibling) {
                    NodeData::Text(text) => tail.push_str(text),
                    _ => break,
                }
            }
            (!tail.is_empty()).then_some(tail)
        })
    };

    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&part)?;
    // Populate the copied row while detached.
    let new_tr = tree.deep_clone(template_tr);
    let new_cells = row_tcs(tree, new_tr);
    for (index, (new_tc, template_rpr)) in new_cells.iter().zip(template_rprs.iter()).enumerate() {
        let rpr_copy = template_rpr.map(|rpr| tree.deep_clone(rpr));
        let Some(paragraph) = w_child(tree, *new_tc, "p") else {
            return Err(Error::internal(
                "copied_cell",
                "copied cell lost its paragraph",
            ));
        };
        // `paragraph.clear()`: every element child except pPr (with tails).
        let doomed: Vec<NodeId> = {
            let mut out = Vec::new();
            let mut removing_tail = false;
            for child in tree.children(paragraph).to_vec() {
                match tree.data(child) {
                    NodeData::Text(_) => {
                        if removing_tail {
                            out.push(child);
                        }
                    }
                    NodeData::Element(_) => {
                        removing_tail = !is_w(tree, child, "pPr");
                        if removing_tail {
                            out.push(child);
                        }
                    }
                    _ => removing_tail = false,
                }
            }
            out
        };
        for node in doomed {
            tree.detach(node)?;
        }
        let value = values.get(index).map(AsRef::as_ref).unwrap_or("");
        let run = w_el(tree, paragraph, "r")?;
        if !value.is_empty() {
            add_t(tree, run, value)?;
        }
        append(tree, paragraph, run)?;
        if let Some(copy) = rpr_copy {
            tree.insert(run, 0, copy)?;
        }
    }
    add_next(tree, anchor_tr, new_tr)?;
    if let Some(tail) = template_tail {
        let text = tree.create_text(tail);
        add_next_raw(tree, new_tr, text)?;
    }
    let staged = txn.finish()?;
    document.commit(staged)
}

/// Insert `new` immediately after `anchor` (no tail skipping).
fn add_next_raw(tree: &mut XmlTree, anchor: NodeId, new: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "insertion anchor is detached"))?;
    let index = tree.index_in_parent(anchor).ok_or_else(|| {
        Error::internal("detached_anchor", "anchor not among its parent's children")
    })?;
    tree.insert(parent, index + 1, new)
}

/// Delete row `row` (upstream `delete_row`). Refuses the last remaining row,
/// a row in a vertical merge or holding a nested table, an out-of-range
/// index and a protected document.
pub fn delete_row(document: &mut Document, table: &Table, row: usize) -> Result<()> {
    let tbl = locate(document, table)?;
    refuse_if_protected(document, "delete a table row", OperationClass::BodyContent)?;
    let part = table.part.clone();
    let tree = document.package().xml(&part)?;
    let all_rows = rows(tree, tbl);
    if row >= all_rows.len() {
        return Err(row_out_of_range(row, all_rows.len()));
    }
    refuse_row_op(tree, &all_rows, row, None)?;
    if all_rows.len() == 1 {
        return Err(unsupported(
            "last_row",
            "deleting the last remaining row would leave an invalid table; remove the table itself instead",
        ));
    }
    let tr = all_rows[row];
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&part)?;
    // lxml `remove` takes the element's tail with it.
    let (parent, end) = after_with_tail(tree, tr)?;
    let start = tree
        .index_in_parent(tr)
        .ok_or_else(|| Error::internal("detached_row", "row not among its parent's children"))?;
    let doomed: Vec<NodeId> = tree.children(parent)[start..end].to_vec();
    for node in doomed {
        tree.detach(node)?;
    }
    let staged = txn.finish()?;
    document.commit(staged)
}

impl Document {
    /// The table with one cell containing `near_text` (see [`find_table`]).
    pub fn find_table(&self, near_text: &str, policy: MatchPolicy) -> Result<Table> {
        find_table(self, near_text, policy)
    }

    /// Cell texts per row (see [`table_grid`]).
    pub fn table_grid(&self, table: &Table) -> Result<Vec<Vec<String>>> {
        table_grid(self, table)
    }

    /// Replace one cell's text (see [`update_cell`]).
    pub fn update_cell(
        &mut self,
        table: &Table,
        row: usize,
        column: usize,
        new_text: &str,
        options: &UpdateCellOptions,
    ) -> Result<ReplaceReport> {
        update_cell(self, table, row, column, new_text, options)
    }

    /// Insert a row (see [`insert_row_after`]).
    pub fn insert_row_after<S: AsRef<str>>(
        &mut self,
        table: &Table,
        row: usize,
        values: &[S],
        copy_format_from: Option<usize>,
    ) -> Result<()> {
        insert_row_after(self, table, row, values, copy_format_from)
    }

    /// Delete a row (see [`delete_row`]).
    pub fn delete_row(&mut self, table: &Table, row: usize) -> Result<()> {
        delete_row(self, table, row)
    }
}
