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

/// A number written can change the contents' length and so later pages.
const PAGE_NUMBER_PASSES: usize = 3;

/// Returns how many numbers changed and the last layout. With
/// `blank_unplaced`, a bookmark the layout does not place gets no number.
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

/// Internal to docrs-wasm.
#[doc(hidden)]
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TocRefresh {
    pub changed: usize,
    pub layout: Layout,
}

/// Internal to docrs-wasm, not covered by semver. With `converted_only`,
/// only the contents the last Markdown conversion of this document wrote.
/// Entries in a section numbered other than in decimal or after a restart
/// keep their numbers.
#[doc(hidden)]
pub fn refresh_toc_page_numbers(
    document: &mut Document,
    converted_only: bool,
) -> Result<TocRefresh> {
    refuse_if_protected(
        document,
        "refresh contents page numbers",
        OperationClass::BodyContent,
    )?;
    let main = document.package().main_part_name().to_string();
    let numbers = {
        let tree = document.package().xml(&main)?;
        let decimal = decimal_bookmarks(tree);
        let converted = document.converted_toc();
        toc_page_refs(tree)
            .into_iter()
            .filter(|(t, bookmark)| {
                decimal.contains(bookmark) && (!converted_only || converted.contains(t))
            })
            .collect::<Vec<_>>()
    };
    let mut txn = Txn::new(document);
    let (changed, layout) = write_page_numbers(&mut txn, &main, &numbers, false)?;
    if changed > 0 {
        let staged = txn.finish()?;
        document.commit(staged)?;
    }
    Ok(TocRefresh { changed, layout })
}

/// Bookmarks in decimal sections with no restart after the first section.
fn decimal_bookmarks(tree: &XmlTree) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(body) = w_child(tree, tree.root(), "body") else {
        return out;
    };
    let mut section = Vec::new();
    let mut restarted = false;
    let mut first = true;
    let mut close = |sect_pr: Option<NodeId>, section: &mut Vec<String>| {
        let numbering = sect_pr.and_then(|s| w_child(tree, s, "pgNumType"));
        restarted |= !first && numbering.is_some_and(|n| w_attr(tree, n, "start").is_some());
        first = false;
        let decimal = numbering.is_none_or(|n| {
            w_attr(tree, n, "fmt").is_none_or(|fmt| fmt == "decimal")
                && w_attr(tree, n, "chapStyle").is_none()
        });
        if decimal && !restarted {
            out.extend(section.drain(..));
        }
        section.clear();
    };
    for child in tree.child_elements(body) {
        if tree.is(child, W, "sectPr") {
            close(Some(child), &mut section);
            continue;
        }
        section.extend(
            tree.descendants(child)
                .filter(|n| tree.is(*n, W, "bookmarkStart"))
                .filter_map(|n| w_attr(tree, n, "name").map(str::to_string)),
        );
        let sect_pr = w_child(tree, child, "pPr").and_then(|ppr| w_child(tree, ppr, "sectPr"));
        if tree.is(child, W, "p") && sect_pr.is_some() {
            close(sect_pr, &mut section);
        }
    }
    if !section.is_empty() {
        close(None, &mut section);
    }
    out
}

struct OpenField {
    instruction: String,
    in_code: bool,
    deleted: bool,
    paragraph: Option<NodeId>,
    texts: Vec<NodeId>,
}

#[derive(Clone, Copy)]
struct At {
    deleted: bool,
    paragraph: Option<NodeId>,
}

impl OpenField {
    fn new(instruction: String, in_code: bool, at: At) -> Self {
        OpenField {
            instruction,
            in_code,
            deleted: at.deleted,
            paragraph: at.paragraph,
            texts: Vec::new(),
        }
    }
}

/// `(w:t, bookmark)` of every PAGEREF field nested in a TOC field's result.
fn toc_page_refs(tree: &XmlTree) -> Vec<(NodeId, String)> {
    let mut open = Vec::new();
    let mut out = Vec::new();
    let at = At {
        deleted: false,
        paragraph: None,
    };
    walk_fields(tree, tree.root(), at, &mut open, &mut out);
    out
}

fn walk_fields(
    tree: &XmlTree,
    node: NodeId,
    at: At,
    open: &mut Vec<OpenField>,
    out: &mut Vec<(NodeId, String)>,
) {
    let deleted = at.deleted;
    for child in tree.child_elements(node) {
        if tree.ns_uri(child) != Some(W) {
            walk_fields(tree, child, at, open, out);
            continue;
        }
        match tree.local_name(child).unwrap_or("") {
            "del" | "moveFrom" => {
                let at = At {
                    deleted: true,
                    ..at
                };
                walk_fields(tree, child, at, open, out);
            }
            "p" => {
                let at = At {
                    paragraph: Some(child),
                    ..at
                };
                walk_fields(tree, child, at, open, out);
            }
            "fldSimple" => {
                let instruction = w_attr(tree, child, "instr").unwrap_or("").to_string();
                open.push(OpenField::new(instruction, false, at));
                walk_fields(tree, child, at, open, out);
                close_field(tree, open, at, out);
            }
            "fldChar" => match w_attr(tree, child, "fldCharType") {
                Some("begin") => open.push(OpenField::new(String::new(), true, at)),
                Some("separate") => {
                    if let Some(field) = open.last_mut() {
                        field.in_code = false;
                        field.deleted |= deleted;
                    }
                }
                Some("end") => {
                    if let Some(field) = open.last_mut() {
                        field.deleted |= deleted;
                    }
                    close_field(tree, open, at, out);
                }
                _ => {}
            },
            "instrText" | "delInstrText" => {
                if let Some(field) = open.last_mut().filter(|f| f.in_code) {
                    field.deleted |= deleted;
                    field.instruction.push_str(&tree.text_content(child));
                }
            }
            "t" => {
                if let Some(field) = open.last_mut().filter(|f| !f.in_code) {
                    field.texts.push(child);
                }
            }
            _ => walk_fields(tree, child, at, open, out),
        }
    }
}

/// A PAGEREF in a contents begins and ends in one paragraph; one that does
/// not is a field left open, closed by the end of an outer field.
fn close_field(tree: &XmlTree, open: &mut Vec<OpenField>, at: At, out: &mut Vec<(NodeId, String)>) {
    let Some(field) = open.pop() else { return };
    let in_toc = open
        .iter()
        .any(|outer| !outer.in_code && !outer.deleted && is_command(&outer.instruction, "TOC"));
    if let (true, false, true, [t], Some(bookmark)) = (
        in_toc,
        field.deleted,
        field.paragraph == at.paragraph,
        field.texts.as_slice(),
        page_ref_bookmark(&field.instruction),
    ) && tree.text_content(*t).chars().all(|c| c.is_ascii_digit())
    {
        out.push((*t, bookmark));
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
