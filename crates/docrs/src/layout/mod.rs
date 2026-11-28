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
pub(crate) mod model;

use std::collections::{HashMap, HashSet};

pub use flow::Placed;
#[cfg(test)]
pub(crate) use flow::TEST_LAYOUT_WORK;

/// Lay out with this work bound instead of the default on this thread
/// (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn set_test_layout_work(budget: Option<u64>) {
    flow::TEST_LAYOUT_WORK.set(budget);
}
pub use fonts::{Face, FontBook, FontFile, FontInfo, FontSet, FontSetBuilder};
#[doc(hidden)]
pub use model::{Appearance, Caps, Colour, Link, Rgb, Strike, Underline, UnderlineStyle};

use fonts::{FontError, Parsed};

use crate::fields::fieldcode;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::Txn;
use crate::wml::{R, W, set_leading_text, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Result};
use model::StoryId;

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
                let Some(bytes) = txn.binary(part) else {
                    return Err(FontError::Malformed("embedded font part"));
                };
                let Some(key) = key else {
                    return Face::parse_embedded(&bytes, part, None, name, &mut budget);
                };
                let copy = copies.entry(part).or_insert_with(|| bytes.to_vec());
                fonts::xor_font_key(copy, &key);
                let face = Face::parse_embedded(copy, part, Some(key), name, &mut budget);
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

fn model_of(
    txn: &Txn<'_>,
    main: &str,
    fonts: &FontSet,
) -> Result<(model::Model, FontBook, Vec<String>, Vec<String>)> {
    let (embedded, unread) = embedded_fonts(txn, main)?;
    let mut embedded_names: Vec<String> = embedded.iter().map(|f| f.family.clone()).collect();
    embedded_names.dedup();
    let mut book = FontBook::with_fonts(embedded, fonts);
    let model = model::build(txn, main, &mut book)?;
    Ok((model, book, embedded_names, unread))
}

/// Estimate the layout of `main`'s body, measuring `extra` faces in the
/// styles the document embeds no font for. See [`layout_with`].
pub fn layout(txn: &Txn<'_>, main: &str, extra: Vec<Face>) -> Result<Layout> {
    layout_with(txn, main, &FontSet::from_faces(extra))
}

/// Estimate the layout of `main`'s body. Each family and style is measured
/// from the document's embedded fonts, else `fonts`, else the bundled
/// table, else a substitute of its kind (reported in `missing_fonts`).
pub fn layout_with(txn: &Txn<'_>, main: &str, fonts: &FontSet) -> Result<Layout> {
    let (model, book, embedded_names, unread) = model_of(txn, main, fonts)?;
    let mut by_node: HashMap<(StoryId, NodeId), Vec<String>> = HashMap::new();
    collect_bookmarks(&model.blocks, &mut by_node);
    let mut placed = Vec::new();
    let pages = flow::paginate(&model, &mut placed)?;
    let mut bookmarks = HashMap::new();
    for entry in &placed {
        if let Some(names) = entry.node.and_then(|n| by_node.get(&(StoryId::Body, n))) {
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
    fonts: &FontSet,
) -> Result<(usize, Layout)> {
    let before: Vec<String> = {
        let tree = txn.tree(main)?;
        numbers.iter().map(|(t, _)| tree.text_content(*t)).collect()
    };
    let mut pass = 0;
    let estimate = loop {
        let estimate = layout_with(txn, main, fonts)?;
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
    refresh_toc_page_numbers_with(document, converted_only, &FontSet::default())
}

/// [`refresh_toc_page_numbers`], measuring with `fonts`. Internal to
/// docrs-wasm, not covered by semver.
#[doc(hidden)]
pub fn refresh_toc_page_numbers_with(
    document: &mut Document,
    converted_only: bool,
    fonts: &FontSet,
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
    let (changed, layout) = write_page_numbers(&mut txn, &main, &numbers, false, fonts)?;
    if changed > 0 {
        let staged = txn.finish()?;
        document.commit(staged)?;
    }
    Ok(TocRefresh { changed, layout })
}

/// Bookmarks in decimal sections with no restart after the first section.
fn decimal_bookmarks(tree: &XmlTree) -> HashSet<String> {
    let mut sections = Sections {
        tree,
        decimal: HashSet::new(),
        pending: Vec::new(),
        first: true,
        restarted: false,
    };
    if let Some(body) = w_child(tree, tree.root(), "body") {
        sections.visit(body);
        sections.close(None);
    }
    sections.decimal
}

struct Sections<'t> {
    tree: &'t XmlTree,
    decimal: HashSet<String>,
    pending: Vec<String>,
    first: bool,
    restarted: bool,
}

impl Sections<'_> {
    /// Section breaks are found at any depth, as the layout model finds
    /// them; a break paragraph's own bookmarks belong to the section it ends.
    fn visit(&mut self, node: NodeId) {
        let tree = self.tree;
        for child in tree.child_elements(node) {
            if tree.is(child, W, "bookmarkStart") {
                self.pending
                    .extend(w_attr(tree, child, "name").map(str::to_string));
            } else if tree.is(child, W, "sectPr") {
                if tree.is(node, W, "body") {
                    self.close(Some(child));
                }
            } else {
                self.visit(child);
            }
            let break_sect =
                w_child(tree, child, "pPr").and_then(|ppr| w_child(tree, ppr, "sectPr"));
            if tree.is(child, W, "p") && break_sect.is_some() {
                self.close(break_sect);
            }
        }
    }

    fn close(&mut self, sect_pr: Option<NodeId>) {
        if self.pending.is_empty() && sect_pr.is_none() {
            return;
        }
        let tree = self.tree;
        let numbering = sect_pr.and_then(|s| w_child(tree, s, "pgNumType"));
        self.restarted |=
            !self.first && numbering.is_some_and(|n| w_attr(tree, n, "start").is_some());
        self.first = false;
        let decimal = numbering.is_none_or(|n| {
            w_attr(tree, n, "fmt").is_none_or(|fmt| fmt == "decimal")
                && w_attr(tree, n, "chapStyle").is_none()
        });
        if decimal && !self.restarted {
            self.decimal.extend(self.pending.drain(..));
        }
        self.pending.clear();
    }
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
                close_field(tree, open, out);
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
                    drop_unclosed_page_refs(open, at);
                    if let Some(field) = open.last_mut() {
                        field.deleted |= deleted;
                    }
                    close_field(tree, open, out);
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

fn close_field(tree: &XmlTree, open: &mut Vec<OpenField>, out: &mut Vec<(NodeId, String)>) {
    let Some(field) = open.pop() else { return };
    let in_toc = open
        .iter()
        .any(|outer| !outer.in_code && !outer.deleted && is_command(&outer.instruction, "TOC"));
    let [t] = field.texts.as_slice() else { return };
    let shows_digits = tree.text_content(*t).chars().all(|c| c.is_ascii_digit());
    if !in_toc || field.deleted || !shows_digits {
        return;
    }
    if let Some(bookmark) = page_ref_bookmark(&field.instruction) {
        out.push((*t, bookmark));
    }
}

/// A PAGEREF begins and ends in one paragraph: one still open when an end
/// comes in another paragraph was left unclosed, and that end is an outer
/// field's.
fn drop_unclosed_page_refs(open: &mut Vec<OpenField>, at: At) {
    while open
        .last()
        .is_some_and(|f| f.paragraph != at.paragraph && is_command(&f.instruction, "PAGEREF"))
    {
        open.pop();
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

/// A laid-out line (`docrs layout --lines`). Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LineRecord {
    /// Physical page, from 1.
    pub page: u32,
    pub story: String,
    /// Points from the page's top-left corner, as are `baseline` and `tabs`.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub baseline: f64,
    /// Where each tab moves from and to.
    pub tabs: Vec<(f64, f64)>,
    /// The width left over before alignment; a justified line spreads it
    /// over its `gaps`.
    pub slack: f64,
    pub gaps: u32,
    /// Ends the paragraph, or in a line or page break.
    pub last: bool,
    /// `w:jc`'s value: `left`, `center`, `right`, `both`, `distribute`…
    pub align: &'static str,
    pub text: String,
    /// The line's text by look.
    pub runs: Vec<RunRecord>,
}

/// Text of a line in one look. Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RunRecord {
    pub text: String,
    /// Index into the looks `line_records` gives first.
    pub look: u32,
}

/// The looks and links of `main`, then every line of its body.
/// Experimental.
#[doc(hidden)]
pub fn line_records(
    txn: &Txn<'_>,
    main: &str,
    extra: Vec<Face>,
    looks: impl FnOnce(&[Appearance], &[Link]) -> Result<()>,
    each: impl FnMut(LineRecord) -> Result<()>,
) -> Result<()> {
    line_records_with(txn, main, &FontSet::from_faces(extra), looks, each)
}

/// [`line_records`], measuring with `fonts`. Experimental.
#[doc(hidden)]
pub fn line_records_with(
    txn: &Txn<'_>,
    main: &str,
    fonts: &FontSet,
    looks: impl FnOnce(&[Appearance], &[Link]) -> Result<()>,
    each: impl FnMut(LineRecord) -> Result<()>,
) -> Result<()> {
    let (model, ..) = model_of(txn, main, fonts)?;
    looks(&model.looks, &model.links)?;
    flow::paginate(&model, &mut LineDump { page: 0, each })?;
    Ok(())
}

struct LineDump<F> {
    page: u32,
    each: F,
}

impl<F: FnMut(LineRecord) -> Result<()>> flow::Sink for LineDump<F> {
    fn page(&mut self, page: &flow::PageStart) -> Result<()> {
        self.page = page.number;
        Ok(())
    }

    fn line(&mut self, at: flow::Point, para: &model::Para, line: &flow::Line) -> Result<()> {
        let pieces = para
            .pieces
            .get(line.pieces.start as usize..line.pieces.end as usize)
            .unwrap_or_default();
        let mut runs: Vec<RunRecord> = Vec::new();
        for piece in pieces {
            let (ch, look) = match piece {
                model::Piece::Char { ch, look, .. } => (*ch, *look),
                model::Piece::Tab { look, .. } => ('\t', *look),
                _ => continue,
            };
            match runs.last_mut() {
                Some(run) if run.look == look => run.text.push(ch),
                _ => runs.push(RunRecord {
                    text: ch.to_string(),
                    look,
                }),
            }
        }
        let text = runs.iter().map(|run| run.text.as_str()).collect();
        let story = match para.story {
            StoryId::Body => "body".to_string(),
            StoryId::Header(n) => format!("header{n}"),
            StoryId::Footer(n) => format!("footer{n}"),
        };
        (self.each)(LineRecord {
            page: self.page,
            story,
            x: at.x + line.start,
            y: at.y,
            width: line.width,
            height: line.height,
            baseline: at.y + line.baseline,
            tabs: line
                .tabs
                .iter()
                .map(|tab| (at.x + tab.from, at.x + tab.to))
                .collect(),
            slack: line.slack,
            gaps: line.gaps,
            last: line.last,
            align: para.align.name(),
            text,
            runs,
        })
    }
}

fn collect_bookmarks(blocks: &[model::Block], out: &mut HashMap<(StoryId, NodeId), Vec<String>>) {
    for block in blocks {
        match block {
            model::Block::Para(para) => {
                if let (Some(node), false) = (para.node, para.bookmarks.is_empty()) {
                    out.insert((para.story, node), para.bookmarks.clone());
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

#[cfg(test)]
mod tests {
    use super::model::{Block, Para, StoryId};
    use super::*;

    #[test]
    fn bookmarks_are_keyed_by_story_and_node() {
        // Node ids are per part: a header paragraph can share one with a
        // body paragraph.
        let node = Some(NodeId(3));
        let para = |story, name: &str| {
            Block::Para(Para {
                story,
                node,
                bookmarks: vec![name.to_string()],
                ..Para::default()
            })
        };
        let blocks = [
            para(StoryId::Header(1), "inHeader"),
            para(StoryId::Body, "inBody"),
        ];
        let mut map = HashMap::new();
        collect_bookmarks(&blocks, &mut map);
        assert_eq!(map[&(StoryId::Body, NodeId(3))], ["inBody"]);
        assert_eq!(map[&(StoryId::Header(1), NodeId(3))], ["inHeader"]);
    }
}
