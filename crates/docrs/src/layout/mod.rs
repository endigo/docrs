//! Page layout estimation: where Word for Mac breaks lines and pages, from
//! font metrics and the document's formatting, without rendering. Used to
//! write the page numbers of a table of contents.
//!
//! The estimate covers paragraphs (line breaking with tabs, indents, list
//! labels and line spacing), paragraph spacing with contextual spacing,
//! page breaks, keep-with-next, keep-lines, widow/orphan control, tables
//! (rows as tall as their tallest cell, split between lines, repeated
//! header rows), inline pictures, each page's header and footer (first,
//! even or default), which push the body down when taller than the margin,
//! footnotes at the foot of their page, endnotes after the last block,
//! and sections with their own page setup, start (next, even or odd page,
//! continuous) and page numbering. Page numbers, page counts, page
//! references and document properties are computed; other fields show
//! their stored result. Floating objects and columns are not
//! modelled.

mod fields;
pub(crate) mod flow;
pub mod fonts;
mod furniture;
pub(crate) mod model;
#[cfg(test)]
mod note_tests;
mod strokes;
pub(crate) mod variation;

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

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
pub use fonts::{
    Face, FontBook, FontFile, FontInfo, FontKind, FontSet, FontSetBuilder, LayoutTarget,
};
#[doc(hidden)]
pub use model::{
    Appearance, Border, BorderStyle, Caps, Colour, Link, Rgb, Strike, Underline, UnderlineStyle,
};

use fonts::{FontError, Parsed};

use crate::fields::fieldcode;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::Txn;
use crate::wml::{R, W, set_leading_text, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, Result};
pub(crate) use fields::LayoutWarning;
use fields::{Context, Values};
#[cfg(feature = "pdf")]
pub(crate) use fields::{
    FIELD_CACHED, FIELD_FORMAT_APPROXIMATED, FIELD_SWITCH_IGNORED, NumberFormat,
    Shown as FieldShown, shown as field_shown,
};
#[cfg(feature = "pdf")]
pub(crate) use furniture::STORY_OVERFLOW;
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
    numbers: Vec<fields::Number>,
}

impl Layout {
    /// The physical page (from 1) holding the paragraph that starts
    /// bookmark `name`.
    pub fn bookmark_page(&self, name: &str) -> Option<u32> {
        self.bookmarks.get(name).copied()
    }

    /// The first page's number counted on to physical page `page`, past
    /// any later section's restart; [`Layout::page_label`] gives the shown one.
    pub fn page_number(&self, page: u32) -> i64 {
        self.first_page_number + i64::from(page) - 1
    }

    /// The page number physical page `page` (from 1) shows, with its
    /// section's restarts and written in its section's format (`iii`, `1`).
    pub fn page_label(&self, page: u32) -> Option<String> {
        let index = usize::try_from(page.checked_sub(1)?).ok()?;
        let number = self.numbers.get(index)?;
        Some(number.write(fields::NumberFormat::Section))
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

pub(crate) fn model_of(
    txn: &Txn<'_>,
    main: &str,
    fonts: &FontSet,
    target: LayoutTarget,
) -> Result<(model::Model, FontBook, Vec<String>, Vec<String>)> {
    let (embedded, unread) = embedded_fonts(txn, main)?;
    let mut embedded_names: Vec<String> = embedded.iter().map(|f| f.family.clone()).collect();
    embedded_names.dedup();
    let mut book = FontBook::with_fonts(embedded, fonts).with_target(target);
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
/// Headers and footers show their page's number; other page fields show
/// their stored result.
pub fn layout_with(txn: &Txn<'_>, main: &str, fonts: &FontSet) -> Result<Layout> {
    let (mut model, book, embedded_names, unread) = model_of(txn, main, fonts, LayoutTarget::Word)?;
    let values = Values::default();
    show_body(&mut model, &values);
    let mut placed = Vec::new();
    let pages = flow::paginate(&model, &values, &mut placed)?;
    Ok(Layout {
        bookmarks: bookmark_pages(&model, &placed),
        placed,
        pages: pages.count,
        first_page_number: first_page_number(&model),
        missing_fonts: missing_fonts(&book.missing, unread),
        embedded_fonts: embedded_names,
        numbers: pages.numbers,
    })
}

fn first_page_number(model: &model::Model) -> i64 {
    model
        .sections
        .first()
        .and_then(|s| s.page_start)
        .unwrap_or(1)
}

fn show_body(model: &mut model::Model, values: &Values) {
    let context = Context { values, page: None };
    fields::show(&mut model.blocks, &context);
}

/// The physical page of each body bookmark: its paragraph's first.
fn bookmark_pages(model: &model::Model, placed: &[Placed]) -> HashMap<String, u32> {
    let mut by_node: HashMap<(StoryId, NodeId), Vec<String>> = HashMap::new();
    collect_bookmarks(&model.blocks, &mut by_node);
    let mut bookmarks = HashMap::new();
    for entry in placed {
        if let Some(names) = entry.node.and_then(|n| by_node.get(&(StoryId::Body, n))) {
            for name in names {
                bookmarks.entry(name.clone()).or_insert(entry.page);
            }
        }
    }
    bookmarks
}

/// Layout passes before [`settle`] refuses.
const MAX_PASSES: usize = 8;

/// Field values that change the layout that gives them.
pub(crate) const NOT_CONVERGED: &str = "pdf_layout_not_converged";

/// A layout whose computed fields show the values it gives them.
pub(crate) struct Settled {
    model: model::Model,
    values: Values,
    pub layout: Layout,
    #[allow(dead_code, reason = "the PDF report will give them")]
    pub passes: usize,
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF report gives them")
    )]
    pub warnings: Vec<LayoutWarning>,
}

impl Settled {
    /// The settled layout again, into `sink`.
    pub(crate) fn paginate<S: flow::Sink>(&self, sink: &mut S) -> Result<()> {
        flow::paginate(&self.model, &self.values, sink).map(|_| ())
    }

    /// The model laid out: body fields show their settled values.
    #[allow(dead_code, reason = "the PDF painter reads it")]
    pub(crate) fn model(&self) -> &model::Model {
        &self.model
    }

    /// The part of a header or footer; `None` for the body.
    #[cfg(feature = "pdf")]
    pub(crate) fn story_part(&self, story: StoryId) -> Option<&str> {
        self.model
            .stories
            .iter()
            .find(|s| s.id == story)
            .map(|s| s.part.as_str())
    }
}

/// Lay `main` out until every page number, page count and page reference,
/// in the body and in each page's header and footer, shows what the layout
/// it is shown in gives it, and every page breaks where the pass before
/// broke it. Refuses with `pdf_layout_not_converged` when the values
/// repeat an earlier pass's or after 8 passes. Also gives the fonts it
/// measured with.
pub(crate) fn settle(
    txn: &Txn<'_>,
    main: &str,
    fonts: &FontSet,
    target: LayoutTarget,
) -> Result<(Settled, FontBook)> {
    let (mut model, book, embedded_names, unread) = model_of(txn, main, fonts, target)?;
    let feeds_back = fields::feeds_back(&model);
    let page_fields = fields::page_field_nodes(&model.blocks);
    let mut values = Values::default();
    let mut seen: Vec<u64> = Vec::new();
    for passes in 1..=MAX_PASSES {
        show_body(&mut model, &values);
        let mut record = PassRecord::default();
        let pages = flow::paginate(&model, &values, &mut record)?;
        let bookmarks = bookmark_pages(&model, &record.placed);
        let next = Values::of(&pages, &bookmarks, &record.placed, &page_fields);
        let signature = signature(&next, record.breaks.finish());
        if !feeds_back || seen.last() == Some(&signature) {
            let mut warnings = pages.warnings;
            let stories = model.stories.iter().filter(|s| model.shows(s.id));
            let stories = stories.map(|s| (s.id, s.blocks.as_slice()));
            let body = std::iter::once((StoryId::Body, model.blocks.as_slice()));
            warnings.extend(fields::unplaced_references(body.chain(stories), &values));
            let layout = Layout {
                bookmarks,
                placed: record.placed,
                pages: pages.count,
                first_page_number: first_page_number(&model),
                missing_fonts: missing_fonts(&book.missing, unread),
                embedded_fonts: embedded_names,
                numbers: pages.numbers,
            };
            let settled = Settled {
                model,
                values,
                layout,
                passes,
                warnings,
            };
            return Ok((settled, book));
        }
        if seen.contains(&signature) {
            return Err(Error::unsupported(
                NOT_CONVERGED,
                format!("page numbers repeat an earlier layout's after {passes} passes"),
            ));
        }
        seen.push(signature);
        values = next;
    }
    Err(Error::unsupported(
        NOT_CONVERGED,
        format!("page numbers still change after {MAX_PASSES} passes"),
    ))
}

fn signature(values: &Values, breaks: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    values.pages.hash(&mut hasher);
    values.section_pages.hash(&mut hasher);
    let mut paragraphs: Vec<_> = values.paragraphs.iter().collect();
    paragraphs.sort_by_key(|(node, _)| node.0);
    paragraphs.hash(&mut hasher);
    let mut bookmarks: Vec<_> = values.bookmarks.iter().collect();
    bookmarks.sort_by_key(|(name, _)| *name);
    bookmarks.hash(&mut hasher);
    breaks.hash(&mut hasher);
    hasher.finish()
}

/// Where each body paragraph landed and where every line broke.
#[derive(Default)]
struct PassRecord {
    placed: Vec<Placed>,
    breaks: DefaultHasher,
    page: u32,
}

impl flow::Sink for PassRecord {
    fn page(&mut self, page: &flow::PageStart) -> Result<()> {
        self.page = page.number;
        Ok(())
    }

    fn line(&mut self, _: flow::Point, para: &model::Para, line: &flow::Line) -> Result<()> {
        (self.page, para.story, para.node, line.pieces.start).hash(&mut self.breaks);
        Ok(())
    }

    fn placed(&mut self, placed: Placed) -> Result<()> {
        self.placed.push(placed);
        Ok(())
    }
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

/// Settle the layout (contents page numbers included) and write each of
/// `numbers`' bookmark page numbers as the settled layout shows them, in
/// their section's format. Returns how many changed and the layout. With
/// `blank_unplaced`, a bookmark the layout does not place gets no number.
pub(crate) fn write_page_numbers(
    txn: &mut Txn<'_>,
    main: &str,
    numbers: &[(NodeId, String)],
    blank_unplaced: bool,
    fonts: &FontSet,
) -> Result<(usize, Layout)> {
    let (settled, _) = settle(txn, main, fonts, LayoutTarget::Word)?;
    let tree = txn.tree_mut(main)?;
    let mut changed = 0;
    for (t, bookmark) in numbers {
        let page = settled.layout.bookmark_page(bookmark);
        let shown = match page.and_then(|page| settled.layout.page_label(page)) {
            Some(shown) => shown,
            None if blank_unplaced => String::new(),
            None => continue,
        };
        if tree.text_content(*t) != shown {
            set_leading_text(tree, *t, &shown)?;
            changed += 1;
        }
    }
    Ok((changed, settled.layout))
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
/// Entries in a section whose page number format layout does not write, or
/// with chapter numbers, keep their numbers.
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
        let written = written_bookmarks(tree);
        let converted = document.converted_toc();
        toc_page_refs(tree)
            .into_iter()
            .filter(|(t, bookmark)| {
                written.contains(bookmark) && (!converted_only || converted.contains(t))
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

/// [`refresh_toc_page_numbers`], except that page numbers which do not
/// settle (`pdf_layout_not_converged`) keep their stored values, Word is
/// asked to update fields when it opens the document, and `None` is
/// returned. Internal to docrs-wasm, not covered by semver.
#[doc(hidden)]
pub fn refresh_toc_page_numbers_if_settled(
    document: &mut Document,
    converted_only: bool,
) -> Result<Option<TocRefresh>> {
    refresh_toc_page_numbers_if_settled_with(document, converted_only, &FontSet::default())
}

/// [`refresh_toc_page_numbers_if_settled`], measuring with `fonts`. Internal
/// to docrs-wasm, not covered by semver.
#[doc(hidden)]
pub fn refresh_toc_page_numbers_if_settled_with(
    document: &mut Document,
    converted_only: bool,
    fonts: &FontSet,
) -> Result<Option<TocRefresh>> {
    match refresh_toc_page_numbers_with(document, converted_only, fonts) {
        Err(error) if error.reason() == NOT_CONVERGED => {
            let mut txn = Txn::new(document);
            crate::fields::set_update_fields_on_open(&mut txn)?;
            let staged = txn.finish()?;
            document.commit(staged)?;
            Ok(None)
        }
        result => result.map(Some),
    }
}

/// Bookmarks in sections whose page numbers layout writes.
fn written_bookmarks(tree: &XmlTree) -> HashSet<String> {
    let mut sections = Sections {
        tree,
        written: HashSet::new(),
        pending: Vec::new(),
    };
    if let Some(body) = w_child(tree, tree.root(), "body") {
        sections.visit(body);
        sections.close(None);
    }
    sections.written
}

struct Sections<'t> {
    tree: &'t XmlTree,
    written: HashSet<String>,
    pending: Vec<String>,
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
        let tree = self.tree;
        let numbering = sect_pr.and_then(|s| w_child(tree, s, "pgNumType"));
        let written = numbering.is_none_or(|n| {
            fields::NumberFormat::of_section(w_attr(tree, n, "fmt")).is_some()
                && w_attr(tree, n, "chapStyle").is_none()
        });
        if written {
            self.written.extend(self.pending.drain(..));
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
    if !in_toc || field.deleted || !shows_page_number(&tree.text_content(*t)) {
        return;
    }
    if let Some(bookmark) = page_ref_bookmark(&field.instruction) {
        out.push((*t, bookmark));
    }
}

/// Whether `text` reads as a page number: digits, roman numerals or one
/// letter repeated, in one case.
fn shows_page_number(text: &str) -> bool {
    let one_case = text.chars().all(|c| c.is_ascii_lowercase())
        || text.chars().all(|c| c.is_ascii_uppercase());
    let roman = || {
        text.chars()
            .all(|c| "ivxlcdm".contains(c.to_ascii_lowercase()))
    };
    let repeated = || text.chars().all(|c| text.starts_with(c));
    text.chars().all(|c| c.is_ascii_digit()) || one_case && (roman() || repeated())
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
    /// The page's size in points.
    pub page_width: f64,
    pub page_height: f64,
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
    /// A list label, on its paragraph's first line.
    pub label: Option<RunRecord>,
    /// Each tab's leader, in the order of `tabs`; `None` without one.
    pub leaders: Vec<Option<RunRecord>>,
}

/// Text of a line in one look. Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RunRecord {
    pub text: String,
    /// Index into the looks `line_records` gives first.
    pub look: u32,
    /// Where each character of `text` starts, points from the page's left.
    pub origins: Vec<f64>,
}

/// The part of a table row on one page (`docrs layout --lines`).
/// Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RowRecord {
    /// Physical page, from 1.
    pub page: u32,
    /// The page's size in points.
    pub page_width: f64,
    pub page_height: f64,
    pub story: String,
    /// The row's index in its table.
    pub row: usize,
    /// Points from the page's top-left corner.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// The row starts on this page, and ends on it.
    pub first: bool,
    pub last: bool,
    /// A header row repeated on a page the table continues on.
    pub repeat: bool,
    pub cells: Vec<CellRecord>,
    /// Its borders after conflicts: above, below and beside its cells.
    pub edges: Vec<EdgeRecord>,
}

/// A border line of a [`RowRecord`], centred on its grid line, points from
/// the page's top-left corner. Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct EdgeRecord {
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub border: Border,
}

/// A cell of a [`RowRecord`]. Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CellRecord {
    pub x: f64,
    pub width: f64,
    /// The cell's content on this page, points from the content's top:
    /// the lines whose top is in `from..to`.
    pub from: f64,
    pub to: f64,
    /// Drawn behind the cell's part.
    pub fill: Option<Rgb>,
}

/// A line or a row part, in the order they are laid out. Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Record {
    Line(LineRecord),
    Row(RowRecord),
}

/// The looks and links of `main`, then every line of each page: its
/// header's, its footer's, then its body's, with fields settled.
/// Experimental.
#[doc(hidden)]
pub fn line_records(
    txn: &Txn<'_>,
    main: &str,
    extra: Vec<Face>,
    looks: impl FnOnce(&[Appearance], &[Link]) -> Result<()>,
    mut each: impl FnMut(LineRecord) -> Result<()>,
) -> Result<()> {
    records_with(
        txn,
        main,
        &FontSet::from_faces(extra),
        looks,
        |record| match record {
            Record::Line(line) => each(line),
            Record::Row(_) => Ok(()),
        },
    )
}

/// The looks and links of `main`, then every line and row part of its
/// body, measuring with `fonts`. Experimental.
#[doc(hidden)]
pub fn records_with(
    txn: &Txn<'_>,
    main: &str,
    fonts: &FontSet,
    looks: impl FnOnce(&[Appearance], &[Link]) -> Result<()>,
    each: impl FnMut(Record) -> Result<()>,
) -> Result<()> {
    records_for(txn, main, fonts, LayoutTarget::Word, looks, each)
}

/// [`records_with`], laid out for `target`. Experimental.
#[doc(hidden)]
pub fn records_for(
    txn: &Txn<'_>,
    main: &str,
    fonts: &FontSet,
    target: LayoutTarget,
    looks: impl FnOnce(&[Appearance], &[Link]) -> Result<()>,
    each: impl FnMut(Record) -> Result<()>,
) -> Result<()> {
    let (settled, _) = settle(txn, main, fonts, target)?;
    looks(&settled.model.looks, &settled.model.links)?;
    settled.paginate(&mut LineDump {
        model: &settled.model,
        page: 0,
        marks: HashMap::new(),
        paper: (0.0, 0.0),
        each,
    })
}

struct LineDump<'m, F> {
    model: &'m model::Model,
    page: u32,
    marks: HashMap<model::NoteId, String>,
    paper: (f64, f64),
    each: F,
}

fn story_name(story: StoryId) -> String {
    match story {
        StoryId::Body => "body".to_string(),
        StoryId::Header(n) => format!("header{n}"),
        StoryId::Footer(n) => format!("footer{n}"),
        StoryId::Footnotes => "footnotes".to_string(),
        StoryId::Endnotes => "endnotes".to_string(),
    }
}

impl<F: FnMut(Record) -> Result<()>> flow::Sink for LineDump<'_, F> {
    fn page(&mut self, page: &flow::PageStart) -> Result<()> {
        let geometry = &self.model.sections[page.section].page;
        self.page = page.number;
        self.paper = (geometry.width, geometry.height);
        Ok(())
    }

    fn note(&mut self, note: model::NoteId, mark: &str) -> Result<()> {
        self.marks.insert(note, mark.to_string());
        Ok(())
    }

    fn line(&mut self, at: flow::Point, para: &model::Para, line: &flow::Line) -> Result<()> {
        let pieces = para
            .pieces
            .get(line.pieces.start as usize..line.pieces.end as usize)
            .unwrap_or_default();
        let (origins, label_origins) = line.origins(para);
        let runs = by_look(pieces, &origins, at.x, &self.marks);
        let label = para
            .label
            .as_ref()
            .filter(|_| line.label.is_some())
            .and_then(|label| by_look(&label.pieces, &label_origins, at.x, &self.marks).pop());
        let mut leaders = line.leaders(para, self.model, at.x).into_iter().peekable();
        let leaders = line
            .tabs
            .iter()
            .map(|tab| {
                leaders
                    .next_if(|leader| leader.piece == tab.piece)
                    .map(|leader| RunRecord {
                        text: std::iter::repeat_n(leader.ch, leader.origins.len()).collect(),
                        look: leader.look,
                        origins: leader.origins,
                    })
            })
            .collect();
        let text = runs.iter().map(|run| run.text.as_str()).collect();
        (self.each)(Record::Line(LineRecord {
            page: self.page,
            page_width: self.paper.0,
            page_height: self.paper.1,
            story: story_name(para.story),
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
            label,
            leaders,
        }))
    }

    fn row(
        &mut self,
        origin: flow::Point,
        table: &model::Table,
        row: usize,
        slice: &flow::RowSlice,
    ) -> Result<()> {
        let cells = table.rows[row]
            .cells
            .iter()
            .zip(table.cell_lefts(row))
            .zip(&slice.cells)
            .map(|((cell, left), content)| CellRecord {
                x: origin.x + left,
                width: cell.width,
                from: content.start,
                to: content.end,
                fill: cell.fill,
            })
            .collect();
        let edges = strokes::row_strokes(origin, table, row, slice)
            .into_iter()
            .map(|stroke| EdgeRecord {
                from: (stroke.from.x, stroke.from.y),
                to: (stroke.to.x, stroke.to.y),
                border: stroke.border,
            })
            .collect();
        (self.each)(Record::Row(RowRecord {
            page: self.page,
            page_width: self.paper.0,
            page_height: self.paper.1,
            story: story_name(table.story),
            row,
            x: origin.x,
            y: origin.y,
            width: table.width(),
            height: slice.height,
            first: slice.first,
            last: slice.last,
            repeat: slice.repeat,
            cells,
            edges,
        }))
    }
}

/// `pieces` by look, their characters at `origins` from `left`; a tab is
/// `\t`.
/// A note's mark, each character at its x: spread evenly over the mark's
/// width, which layout measured with the mark's number in document order.
pub(crate) fn mark_characters(mark: &str, width: f64, x: f64) -> impl Iterator<Item = (char, f64)> {
    let step = width / mark.chars().count().max(1) as f64;
    mark.chars()
        .enumerate()
        .map(move |(i, ch)| (ch, x + step * i as f64))
}

/// A line's text by look, each character at its origin.
fn by_look(
    pieces: &[model::Piece],
    origins: &[f64],
    left: f64,
    marks: &HashMap<model::NoteId, String>,
) -> Vec<RunRecord> {
    let mut runs: Vec<RunRecord> = Vec::new();
    for (piece, origin) in pieces.iter().zip(origins) {
        let mut push = |ch: char, look: u32, x: f64| match runs.last_mut() {
            Some(run) if run.look == look => {
                run.text.push(ch);
                run.origins.push(x);
            }
            _ => runs.push(RunRecord {
                text: ch.to_string(),
                look,
                origins: vec![x],
            }),
        };
        match piece {
            model::Piece::Char { ch, look, .. } => push(*ch, *look, left + origin),
            model::Piece::Tab { look, .. } => push('\t', *look, left + origin),
            model::Piece::NoteRef {
                note, width, look, ..
            } => {
                let mark = marks.get(note).map_or("", String::as_str);
                for (ch, x) in mark_characters(mark, *width, left + origin) {
                    push(ch, *look, x);
                }
            }
            _ => {}
        }
    }
    runs
}

fn collect_bookmarks(blocks: &[model::Block], out: &mut HashMap<(StoryId, NodeId), Vec<String>>) {
    for block in blocks {
        match block {
            model::Block::Para(para) => {
                if let (Some(node), false) = (para.node, para.bookmarks.is_empty()) {
                    let names = para.bookmarks.iter().map(|b| b.name.clone()).collect();
                    out.insert((para.story, node), names);
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
mod section_tests;
#[cfg(test)]
mod story_tests;

#[cfg(test)]
mod tests {
    use super::model::{Block, Bookmark, Para, StoryId};
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
                bookmarks: vec![Bookmark {
                    name: name.to_string(),
                    piece: 0,
                }],
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
