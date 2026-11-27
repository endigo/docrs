//! Page layout estimation: where Word for Mac breaks lines and pages, from
//! font metrics and the document's formatting, without rendering. Used to
//! write the page numbers of a table of contents.
//!
//! The estimate covers paragraphs (line breaking with tabs, indents, list
//! labels and line spacing), paragraph spacing with contextual spacing,
//! page breaks, keep-with-next, keep-lines, widow/orphan control, tables
//! (rows as tall as their tallest cell, split between lines, repeated
//! header rows), inline pictures, and the header and footer heights that
//! push the body down. Floating objects, footnotes, columns and multiple
//! sections with different page setups are not modelled.

mod flow;
pub mod fonts;
mod model;

use std::collections::{HashMap, HashSet};

pub use flow::Placed;
pub use fonts::{Face, FontBook};

use fonts::{FontError, Parsed};

use crate::fields::fieldcode;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::Txn;
use crate::wml::{R, W, set_leading_text, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Result};

/// The estimated layout of a document's body.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Every paragraph (and table row end) in Word's order.
    pub placed: Vec<Placed>,
    /// Physical page count.
    pub pages: u32,
    /// The number the first page shows (`w:pgNumType w:start`, else 1).
    pub first_page_number: i64,
    /// Font families with no metrics, substituted, and families whose
    /// embedded fonts went unread because the document's fonts exceeded
    /// the parse budget.
    pub missing_fonts: Vec<String>,
    /// Families measured from fonts embedded in the document.
    pub embedded_fonts: Vec<String>,
    bookmarks: HashMap<String, u32>,
}

impl Layout {
    /// The physical page (from 1) holding the paragraph that starts
    /// bookmark `name`.
    pub fn bookmark_page(&self, name: &str) -> Option<u32> {
        self.bookmarks.get(name).copied()
    }

    /// The page number shown on physical page `page`.
    pub fn page_number(&self, page: u32) -> i64 {
        self.first_page_number + i64::from(page) - 1
    }
}

/// Embedded fonts of the document (`w:embedRegular` and friends in the
/// font table), deobfuscated and measured within one budget, and the
/// families whose fonts the budget left unread.
fn embedded_fonts(txn: &Txn<'_>, main: &str) -> Result<(Vec<Face>, Vec<String>)> {
    let (mut out, mut unread) = (Vec::new(), Vec::new());
    let Some(table) = txn.part_related_by(main, rt::FONT_TABLE)? else {
        return Ok((out, unread));
    };
    let tree = txn.tree(&table)?;
    let rels = txn.resolved_relationships(&table)?;
    let mut budget = fonts::FILE_BUDGET;
    let mut copies: HashMap<&str, Vec<u8>> = HashMap::new();
    let mut parsed: HashMap<(&str, Option<[u8; 16]>), Parsed<Face>> = HashMap::new();
    for font in w_children(tree, tree.root(), "font") {
        let Some(name) = w_attr(tree, font, "name") else {
            continue;
        };
        for (local, bold, italic) in [
            ("embedRegular", false, false),
            ("embedBold", true, false),
            ("embedItalic", false, true),
            ("embedBoldItalic", true, true),
        ] {
            let Some(embed) = w_child(tree, font, local) else {
                continue;
            };
            let Some(id) = tree.attr(embed, Some(R), "id") else {
                continue;
            };
            let Some((_, part)) = rels.iter().find(|(rel, _)| rel.id == id) else {
                continue;
            };
            let key = match w_attr(tree, embed, "fontKey") {
                Some(key) => match fonts::font_key(key) {
                    Some(key) => Some(key),
                    None => continue,
                },
                None => None,
            };
            let face = parsed.entry((part.as_str(), key)).or_insert_with(|| {
                let unreadable = Err(FontError::Malformed("embedded font part"));
                let Some(bytes) = txn.binary(part) else {
                    return unreadable;
                };
                let Some(key) = key else {
                    return Face::parse_within(&bytes, name, bold, italic, &mut budget);
                };
                let copy = copies.entry(part).or_insert_with(|| bytes.to_vec());
                fonts::xor_font_key(copy, &key);
                let face = Face::parse_within(copy, name, bold, italic, &mut budget);
                fonts::xor_font_key(copy, &key);
                face
            });
            match face {
                Ok(face) => out.push(face.restyled(name, bold, italic)),
                Err(FontError::TooLarge) => unread.push(name.to_string()),
                Err(_) => {}
            }
        }
    }
    Ok((out, unread))
}

/// Estimate the layout of `main`'s body. `fonts` are extra font files the
/// caller has (measured locally, never shipped); the document's embedded
/// fonts take precedence over them and over the bundled table.
pub fn layout(txn: &Txn<'_>, main: &str, extra: Vec<Face>) -> Result<Layout> {
    let (embedded, unread) = embedded_fonts(txn, main)?;
    let mut embedded_names: Vec<String> = embedded.iter().map(|f| f.family.clone()).collect();
    embedded_names.dedup();
    let mut own = embedded;
    own.extend(extra);
    let mut book = FontBook::new(own);
    let model = model::build(txn, main, &mut book)?;
    let mut by_node: HashMap<NodeId, Vec<String>> = HashMap::new();
    collect_bookmarks(&model.blocks, &mut by_node);
    let (placed, pages) = flow::paginate(&model);
    let mut bookmarks = HashMap::new();
    for entry in &placed {
        if let Some(names) = entry.node.and_then(|n| by_node.get(&n)) {
            for name in names {
                bookmarks.entry(name.clone()).or_insert(entry.page);
            }
        }
    }
    Ok(Layout {
        placed,
        pages,
        first_page_number: model
            .sections
            .first()
            .and_then(|s| s.page_start)
            .unwrap_or(1),
        missing_fonts: missing_fonts(&book.missing, unread),
        embedded_fonts: embedded_names,
        bookmarks,
    })
}

fn missing_fonts(substituted: &[String], unread: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = substituted.iter().map(|m| m.to_lowercase()).collect();
    let mut out = substituted.to_vec();
    out.extend(
        unread
            .into_iter()
            .filter(|family| seen.insert(family.to_lowercase())),
    );
    out
}

/// Layout passes [`write_page_numbers`] makes at most: a number written
/// can change the contents' own length and so the pages after it.
const PAGE_NUMBER_PASSES: usize = 3;

/// Write each `(w:t, bookmark)` page number from a fresh layout of `main`,
/// re-laying out until the numbers are stable. A bookmark the layout does
/// not place gets an empty number when `blank_unplaced`, else keeps its
/// own. Returns how many numbers differ from before, and the last layout.
pub(crate) fn write_page_numbers(
    txn: &mut Txn<'_>,
    main: &str,
    numbers: &[(NodeId, String)],
    blank_unplaced: bool,
) -> Result<(usize, Layout)> {
    let before: Vec<String> = {
        let tree = txn.tree(main)?;
        numbers.iter().map(|(t, _)| tree.text_content(*t)).collect()
    };
    let mut pass = 0;
    let estimate = loop {
        let estimate = layout(txn, main, Vec::new())?;
        pass += 1;
        let updates: Vec<(NodeId, String)> = {
            let tree = txn.tree(main)?;
            numbers
                .iter()
                .filter_map(|(t, bookmark)| {
                    let shown = match estimate.bookmark_page(bookmark) {
                        Some(page) => estimate.page_number(page).to_string(),
                        None if blank_unplaced => String::new(),
                        None => return None,
                    };
                    (tree.text_content(*t) != shown).then_some((*t, shown))
                })
                .collect()
        };
        if !updates.is_empty() {
            let tree = txn.tree_mut(main)?;
            for (t, shown) in &updates {
                set_leading_text(tree, *t, shown)?;
            }
        }
        if updates.is_empty() || pass == PAGE_NUMBER_PASSES {
            break estimate;
        }
    };
    let tree = txn.tree(main)?;
    let changed = numbers
        .iter()
        .zip(&before)
        .filter(|((t, _), old)| tree.text_content(*t) != **old)
        .count();
    Ok((changed, estimate))
}

/// Rewrite the page numbers every table of contents in the main body shows
/// (the cached results of the PAGEREF fields inside a TOC field) from a
/// fresh layout, as the Markdown converter computes them. For a document
/// edited after conversion, such as merge fields filled with longer text.
/// Returns how many numbers changed; with none, the document is untouched.
///
/// A PAGEREF is rewritten when its result is a single `w:t` and its only
/// switches are `\h` and `\* MERGEFORMAT`; one whose bookmark the layout
/// does not place keeps its number.
pub fn refresh_page_numbers(document: &mut Document) -> Result<usize> {
    refresh_page_numbers_with_layout(document).map(|refreshed| refreshed.changed)
}

/// What [`refresh_page_numbers_with_layout`] did.
#[derive(Debug, Clone)]
pub struct PageNumberRefresh {
    /// Page numbers that changed.
    pub changed: usize,
    /// Contents entries found and numbered.
    pub entries: usize,
    /// The layout the numbers were taken from; `None` without entries.
    pub layout: Option<Layout>,
}

/// [`refresh_page_numbers`], also handing back the entries found and the
/// layout of the refreshed document (its page count and fonts).
pub fn refresh_page_numbers_with_layout(document: &mut Document) -> Result<PageNumberRefresh> {
    let main = document.package().main_part_name().to_string();
    let numbers = toc_page_refs(document.package().xml(&main)?);
    let mut refreshed = PageNumberRefresh {
        changed: 0,
        entries: numbers.len(),
        layout: None,
    };
    if numbers.is_empty() {
        return Ok(refreshed);
    }
    let mut txn = Txn::new(document);
    let (changed, layout) = write_page_numbers(&mut txn, &main, &numbers, false)?;
    refreshed.layout = Some(layout);
    if changed == 0 {
        return Ok(refreshed);
    }
    let staged = txn.finish()?;
    refuse_if_protected(
        document,
        "refresh contents page numbers",
        OperationClass::BodyContent,
    )?;
    document.commit(staged)?;
    refreshed.changed = changed;
    Ok(refreshed)
}

struct OpenField {
    instruction: String,
    in_code: bool,
    texts: Vec<NodeId>,
}

/// `(w:t, bookmark)` of every PAGEREF field nested in a TOC field's result,
/// in document order.
fn toc_page_refs(tree: &XmlTree) -> Vec<(NodeId, String)> {
    let mut open = Vec::new();
    let mut out = Vec::new();
    walk_fields(tree, tree.root(), &mut open, &mut out);
    out
}

fn walk_fields(
    tree: &XmlTree,
    node: NodeId,
    open: &mut Vec<OpenField>,
    out: &mut Vec<(NodeId, String)>,
) {
    for child in tree.child_elements(node) {
        if tree.ns_uri(child) != Some(W) {
            walk_fields(tree, child, open, out);
            continue;
        }
        match tree.local_name(child).unwrap_or("") {
            "del" | "moveFrom" => {}
            "fldChar" => match w_attr(tree, child, "fldCharType") {
                Some("begin") => open.push(OpenField {
                    instruction: String::new(),
                    in_code: true,
                    texts: Vec::new(),
                }),
                Some("separate") => {
                    if let Some(field) = open.last_mut() {
                        field.in_code = false;
                    }
                }
                Some("end") => {
                    let Some(field) = open.pop() else { continue };
                    let in_toc = open
                        .iter()
                        .any(|outer| !outer.in_code && is_command(&outer.instruction, "TOC"));
                    if let (true, [t], Some(bookmark)) = (
                        in_toc,
                        field.texts.as_slice(),
                        page_ref_bookmark(&field.instruction),
                    ) {
                        out.push((*t, bookmark));
                    }
                }
                _ => {}
            },
            "instrText" => {
                if let Some(field) = open.last_mut().filter(|f| f.in_code) {
                    field.instruction.push_str(&tree.text_content(child));
                }
            }
            "t" => {
                if let Some(field) = open.last_mut().filter(|f| !f.in_code) {
                    field.texts.push(child);
                }
            }
            _ => walk_fields(tree, child, open, out),
        }
    }
}

fn is_command(instruction: &str, command: &str) -> bool {
    instruction
        .split_whitespace()
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case(command))
}

fn page_ref_bookmark(instruction: &str) -> Option<String> {
    let operand = fieldcode::command_operand(instruction, "PAGEREF").ok()??;
    let tokens = fieldcode::tokens(instruction).ok()?;
    let mut switches = tokens[2..].iter().map(|t| (t.quoted, t.value.as_str()));
    while let Some(switch) = switches.next() {
        match switch {
            (false, s) if s.eq_ignore_ascii_case("\\h") => {}
            (false, "\\*") => match switches.next() {
                Some((_, format)) if format.eq_ignore_ascii_case("MERGEFORMAT") => {}
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(operand.value)
}

fn collect_bookmarks(blocks: &[model::Block], out: &mut HashMap<NodeId, Vec<String>>) {
    for block in blocks {
        match block {
            model::Block::Para(para) => {
                if let (Some(node), false) = (para.node, para.bookmarks.is_empty()) {
                    out.insert(node, para.bookmarks.clone());
                }
            }
            model::Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_bookmarks(&cell.blocks, out);
                    }
                }
            }
        }
    }
}
