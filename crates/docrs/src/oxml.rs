//! Schema-aware WordprocessingML construction helpers (the behavior of
//! python-docx's `oxml` layer that authoring operations rely on).
//!
//! Children are inserted in schema order using the same successor lists as
//! upstream `xmlchemy` (`insert_element_before`): a new child goes before the
//! first existing child named in its successor list, else at the end. Every
//! new element takes a prefix already in scope for its namespace (or declares
//! one on itself), so construction never disturbs existing namespace scopes.

use crate::Result;
use crate::unicode::py_strip;
use crate::wml::{W, w_child};
use crate::xml::{NodeId, XML_NS, XmlTree};

/// `CT_PPr` child order.
pub const PPR_SEQ: &[&str] = &[
    "pStyle",
    "keepNext",
    "keepLines",
    "pageBreakBefore",
    "framePr",
    "widowControl",
    "numPr",
    "suppressLineNumbers",
    "pBdr",
    "shd",
    "tabs",
    "suppressAutoHyphens",
    "kinsoku",
    "wordWrap",
    "overflowPunct",
    "topLinePunct",
    "autoSpaceDE",
    "autoSpaceDN",
    "bidi",
    "adjustRightInd",
    "snapToGrid",
    "spacing",
    "ind",
    "contextualSpacing",
    "mirrorIndents",
    "suppressOverlap",
    "jc",
    "textDirection",
    "textAlignment",
    "textboxTightWrap",
    "outlineLvl",
    "divId",
    "cnfStyle",
    "rPr",
    "sectPr",
    "pPrChange",
];

/// `CT_RPr` child order.
pub const RPR_SEQ: &[&str] = &[
    "rStyle",
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "strike",
    "dstrike",
    "outline",
    "shadow",
    "emboss",
    "imprint",
    "noProof",
    "snapToGrid",
    "vanish",
    "webHidden",
    "color",
    "spacing",
    "w",
    "kern",
    "position",
    "sz",
    "szCs",
    "highlight",
    "u",
    "effect",
    "bdr",
    "shd",
    "fitText",
    "vertAlign",
    "rtl",
    "cs",
    "em",
    "lang",
    "eastAsianLayout",
    "specVanish",
    "oMath",
];

/// Successors of `name` within `sequence` (the elements that must follow it).
pub fn successors<'a>(sequence: &'a [&'a str], name: &str) -> &'a [&'a str] {
    match sequence.iter().position(|item| *item == name) {
        Some(index) => &sequence[index + 1..],
        None => &[],
    }
}

/// A new detached `w:local` element scoped for insertion under `scope`.
pub fn w_el(tree: &mut XmlTree, scope: NodeId, local: &str) -> Result<NodeId> {
    tree.create_element(scope, W, local)
}

/// Set `w:local` on `id`.
pub fn set_w(tree: &mut XmlTree, id: NodeId, local: &str, value: &str) -> Result<()> {
    tree.set_attr(id, Some(W), local, value)
}

/// Append detached `child` as the last child of `parent`.
pub fn append(tree: &mut XmlTree, parent: NodeId, child: NodeId) -> Result<()> {
    let end = tree.children(parent).len();
    tree.insert(parent, end, child)
}

/// Upstream `insert_element_before`: before the first child whose `w:` local
/// name is in `successors`, else appended.
pub fn insert_before_successors(
    tree: &mut XmlTree,
    parent: NodeId,
    child: NodeId,
    successors: &[&str],
) -> Result<()> {
    insert_before_successors_in(tree, parent, child, W, successors)
}

/// [`insert_before_successors`] for children in the namespace `ns`.
pub fn insert_before_successors_in(
    tree: &mut XmlTree,
    parent: NodeId,
    child: NodeId,
    ns: &str,
    successors: &[&str],
) -> Result<()> {
    let position = tree.children(parent).iter().position(|existing| {
        tree.ns_uri(*existing) == Some(ns)
            && tree
                .local_name(*existing)
                .is_some_and(|local| successors.contains(&local))
    });
    match position {
        Some(index) => tree.insert(parent, index, child),
        None => append(tree, parent, child),
    }
}

/// Get or add `w:local` under `parent` honoring `sequence` order.
pub fn get_or_add(
    tree: &mut XmlTree,
    parent: NodeId,
    local: &str,
    sequence: &[&str],
) -> Result<NodeId> {
    if let Some(existing) = w_child(tree, parent, local) {
        return Ok(existing);
    }
    let child = w_el(tree, parent, local)?;
    insert_before_successors(tree, parent, child, successors(sequence, local))?;
    Ok(child)
}

/// `CT_P.get_or_add_pPr` (pPr is always the first child).
pub fn get_or_add_ppr(tree: &mut XmlTree, paragraph: NodeId) -> Result<NodeId> {
    if let Some(existing) = w_child(tree, paragraph, "pPr") {
        return Ok(existing);
    }
    let ppr = w_el(tree, paragraph, "pPr")?;
    tree.insert(paragraph, 0, ppr)?;
    Ok(ppr)
}

/// `CT_R.get_or_add_rPr` (rPr is always the first child).
pub fn get_or_add_rpr(tree: &mut XmlTree, run: NodeId) -> Result<NodeId> {
    if let Some(existing) = w_child(tree, run, "rPr") {
        return Ok(existing);
    }
    let rpr = w_el(tree, run, "rPr")?;
    tree.insert(run, 0, rpr)?;
    Ok(rpr)
}

/// Remove every direct `w:local` child of `parent`.
pub fn remove_w_children(tree: &mut XmlTree, parent: NodeId, local: &str) -> Result<()> {
    let doomed: Vec<NodeId> = crate::wml::w_children(tree, parent, local).collect();
    for node in doomed {
        tree.detach(node)?;
    }
    Ok(())
}

/// `CT_P.style = value` (`None` removes `w:pStyle`).
pub fn set_paragraph_style(
    tree: &mut XmlTree,
    paragraph: NodeId,
    style_id: Option<&str>,
) -> Result<()> {
    let ppr = get_or_add_ppr(tree, paragraph)?;
    remove_w_children(tree, ppr, "pStyle")?;
    if let Some(style_id) = style_id {
        let style = w_el(tree, ppr, "pStyle")?;
        set_w(tree, style, "val", style_id)?;
        insert_before_successors(tree, ppr, style, successors(PPR_SEQ, "pStyle"))?;
    }
    Ok(())
}

/// `CT_R.add_t`: append `w:t` (with `xml:space="preserve"` when `text` has
/// leading or trailing Python whitespace).
pub fn add_t(tree: &mut XmlTree, run: NodeId, text: &str) -> Result<NodeId> {
    let t = w_el(tree, run, "t")?;
    if !text.is_empty() {
        let node = tree.create_text(text);
        tree.insert(t, 0, node)?;
    }
    if py_strip(text).chars().count() < text.chars().count() {
        tree.set_attr(t, Some(XML_NS), "space", "preserve")?;
    }
    // `_add_t` inserts before nothing in particular: append after rPr.
    append(tree, run, t)?;
    Ok(t)
}

/// EMU → twips with Python's `round` (banker's rounding).
pub fn emu_to_twips(emu: i64) -> i64 {
    let value = emu as f64 / 635.0;
    let floor = value.floor();
    let diff = value - floor;
    let rounded = if (diff - 0.5).abs() < f64::EPSILON {
        if floor as i64 % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        value.round()
    };
    rounded as i64
}

/// A detached `w:tbl` per `CT_Tbl.new_tbl(rows, cols, width)` with optional
/// cell text (one run per non-empty cell, as `blocks._new_table`).
pub fn new_table(
    tree: &mut XmlTree,
    scope: NodeId,
    cells: &[Vec<String>],
    width_emu: i64,
) -> Result<NodeId> {
    let rows = cells.len();
    let cols = cells.first().map(Vec::len).unwrap_or(0);
    let col_width_emu = if cols > 0 {
        width_emu.div_euclid(cols as i64)
    } else {
        0
    };
    let twips = emu_to_twips(col_width_emu).to_string();
    let tbl = w_el(tree, scope, "tbl")?;
    let tbl_pr = w_el(tree, tbl, "tblPr")?;
    append(tree, tbl, tbl_pr)?;
    let tbl_w = w_el(tree, tbl_pr, "tblW")?;
    set_w(tree, tbl_w, "type", "auto")?;
    set_w(tree, tbl_w, "w", "0")?;
    append(tree, tbl_pr, tbl_w)?;
    let look = w_el(tree, tbl_pr, "tblLook")?;
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
    append(tree, tbl_pr, look)?;
    let grid = w_el(tree, tbl, "tblGrid")?;
    append(tree, tbl, grid)?;
    for _ in 0..cols {
        let col = w_el(tree, grid, "gridCol")?;
        set_w(tree, col, "w", &twips)?;
        append(tree, grid, col)?;
    }
    for row_values in cells.iter().take(rows) {
        let tr = w_el(tree, tbl, "tr")?;
        append(tree, tbl, tr)?;
        for value in row_values {
            let tc = w_el(tree, tr, "tc")?;
            append(tree, tr, tc)?;
            let tc_pr = w_el(tree, tc, "tcPr")?;
            append(tree, tc, tc_pr)?;
            let tc_w = w_el(tree, tc_pr, "tcW")?;
            set_w(tree, tc_w, "type", "dxa")?;
            set_w(tree, tc_w, "w", &twips)?;
            append(tree, tc_pr, tc_w)?;
            let p = w_el(tree, tc, "p")?;
            append(tree, tc, p)?;
            if !value.is_empty() {
                let run = w_el(tree, p, "r")?;
                add_t(tree, run, value)?;
                append(tree, p, run)?;
            }
        }
    }
    Ok(tbl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twips_rounding_matches_python() {
        assert_eq!(emu_to_twips(914400), 1440);
        assert_eq!(emu_to_twips(952), 1); // 1.499
        assert_eq!(emu_to_twips(953), 2); // 1.5008
        assert_eq!(emu_to_twips(1_828_800), 2880);
    }

    #[test]
    fn style_and_run_construction_follow_schema_order() {
        let mut tree = crate::xml::parse_str(&format!(
            r#"<w:p xmlns:w="{W}"><w:pPr><w:jc w:val="center"/><w:numPr/></w:pPr></w:p>"#
        ))
        .unwrap();
        let p = tree.root();
        set_paragraph_style(&mut tree, p, Some("Heading1")).unwrap();
        let run = w_el(&mut tree, p, "r").unwrap();
        add_t(&mut tree, run, " padded").unwrap();
        append(&mut tree, p, run).unwrap();
        let out = String::from_utf8(crate::xml::serialize(&tree)).unwrap();
        assert_eq!(
            out,
            format!(
                r#"<w:p xmlns:w="{W}"><w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/><w:numPr/></w:pPr><w:r><w:t xml:space="preserve"> padded</w:t></w:r></w:p>"#
            )
        );
    }
}
