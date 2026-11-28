//! The document as layout sees it: paragraphs with resolved formatting and
//! measured text, tables with their grids, and the page geometry.
//!
//! Formatting is resolved as Word does: document defaults, then the table
//! style (in tables), the paragraph style chain, the numbering level's
//! paragraph properties and direct formatting; run properties likewise
//! through the character style. Within one style chain the nearest value
//! wins; the toggle properties (bold, italic, caps, small caps, strike,
//! hidden) XOR across the document defaults and the table, paragraph and
//! character styles, and direct formatting is absolute.

mod build;
mod colour;
mod equation;
mod formatting;
mod look;
#[cfg(feature = "pdf")]
mod marks;
mod math;
mod notes;
mod numbering;
mod props;
mod styles;
mod table;
mod theme;

use std::collections::HashMap;
use std::sync::Arc;

use super::fields::{Context, FieldSpan, LayoutWarning, NumberFormat, Properties, Values};
use super::fonts::{Face, FontBook, FontKind};
use super::variation::Style;
use crate::Result;
use crate::package::rels::{Relationship, rt};
use crate::txn::Txn;
use crate::wml::{R, W, w_attr, w_child};
use crate::xml::{NodeId, XmlTree};
pub(crate) use build::Measurer;
#[cfg(feature = "pdf")]
pub(crate) use build::field_link;
pub use colour::{Colour, Rgb};
pub(crate) use formatting::Formatting;
#[cfg(feature = "pdf")]
pub(crate) use formatting::ParaFormat;
pub use look::{Appearance, Caps, Link, Strike, Underline, UnderlineStyle};
use look::{CellBorders, ParaBorders, VAlign};
#[cfg(feature = "pdf")]
pub(crate) use marks::any_border;
pub(crate) use math::MathScripts;
#[cfg(feature = "pdf")]
pub(crate) use math::{CAMBRIA_MATH, linear_text, linearize};
#[cfg(feature = "pdf")]
pub(crate) use notes::NotePosition;
pub use notes::{NoteCounter, NoteId, NoteKind, NoteSettings, Notes};
pub(crate) use props::on_off;
use props::twips;
#[cfg(feature = "pdf")]
pub(crate) use props::{Frame, RPr, horizontal, shaded};
#[cfg(feature = "pdf")]
pub(crate) use table::TableCtx;
use theme::read_font_kinds;

/// Page geometry in points.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code, reason = "the PDF painter reads them")]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub top: f64,
    pub bottom: f64,
    pub header: f64,
    pub footer: f64,
    pub left: f64,
    pub right: f64,
    pub gutter: f64,
    /// Text width between the margins.
    pub text_width: f64,
}

/// Node ids are local to a part's tree. Headers and footers count from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StoryId {
    #[default]
    Body,
    Header(u32),
    Footer(u32),
    Footnotes,
    Endnotes,
}

/// Paragraph alignment (`w:jc`). Right-to-left paragraphs are not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Both,
    Distribute,
    LowKashida,
    MediumKashida,
    HighKashida,
    ThaiDistribute,
}

impl Align {
    fn read(value: Option<&str>) -> Align {
        match value {
            Some("center") => Align::Center,
            Some("right" | "end") => Align::Right,
            Some("both") => Align::Both,
            Some("distribute") => Align::Distribute,
            Some("lowKashida") => Align::LowKashida,
            Some("mediumKashida") => Align::MediumKashida,
            Some("highKashida") => Align::HighKashida,
            Some("thaiDistribute") => Align::ThaiDistribute,
            _ => Align::Left,
        }
    }

    /// Word lets the spaces of a justified line shrink to fit it.
    pub fn justified(self) -> bool {
        !matches!(self, Align::Left | Align::Center | Align::Right)
    }

    pub fn name(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Both => "both",
            Align::Distribute => "distribute",
            Align::LowKashida => "lowKashida",
            Align::MediumKashida => "mediumKashida",
            Align::HighKashida => "highKashida",
            Align::ThaiDistribute => "thaiDistribute",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineRule {
    /// `line` is 240ths of the single line height.
    #[default]
    Auto,
    /// Exactly `line` twips.
    Exact,
    /// At least `line` twips.
    AtLeast,
}

/// A paragraph's line spacing, resolved to points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpacing {
    /// A multiple of the single line height.
    Multiple(f64),
    Exact(f64),
    AtLeast(f64),
}

impl Default for LineSpacing {
    fn default() -> Self {
        LineSpacing::Multiple(1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TabKind {
    Left,
    Right,
    Center,
    Decimal,
    /// Not a stop: a vertical rule on every line.
    Bar,
    Clear,
}

/// What fills the space a tab moves over (`w:leader`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Leader {
    #[default]
    None,
    Dot,
    Hyphen,
    Underscore,
    Heavy,
    MiddleDot,
}

impl Leader {
    fn read(value: Option<&str>) -> Leader {
        match value {
            Some("dot") => Leader::Dot,
            Some("hyphen") => Leader::Hyphen,
            Some("underscore") => Leader::Underscore,
            Some("heavy") => Leader::Heavy,
            Some("middleDot") => Leader::MiddleDot,
            _ => Leader::None,
        }
    }

    /// The character repeated: a heavy leader is the plain underscore, as in
    /// Word for Mac.
    pub fn char(self) -> Option<char> {
        match self {
            Leader::None => None,
            Leader::Dot => Some('.'),
            Leader::Hyphen => Some('-'),
            Leader::Underscore | Leader::Heavy => Some('_'),
            Leader::MiddleDot => Some('\u{B7}'),
        }
    }
}

/// A tab stop (`w:tab`), points from the text area's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabStop {
    pub pos: f64,
    pub kind: TabKind,
    pub leader: Leader,
}

/// Font metrics of a text piece, shared by index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metric {
    /// Single line height in points.
    pub height: f64,
    /// Ascent plus line gap in points.
    pub ascent: f64,
}

/// `metric` indexes `Model.metrics`, `look` `Model.looks`.
#[derive(Debug, Clone)]
pub enum Piece {
    Char {
        ch: char,
        width: f64,
        metric: u32,
        look: u32,
    },
    Tab {
        metric: u32,
        look: u32,
    },
    LineBreak {
        metric: u32,
        #[allow(dead_code, reason = "the PDF painter reads it")]
        look: u32,
    },
    PageBreak,
    /// A note's reference, or its mark in its own text.
    NoteRef {
        note: NoteId,
        width: f64,
        metric: u32,
        look: u32,
    },
    /// The rule above the notes (`w:separator`), or above a note continued
    /// from the page before (`w:continuationSeparator`).
    Separator {
        #[allow(dead_code, reason = "the PDF painter reads it")]
        continuation: bool,
        metric: u32,
    },
    Object {
        width: f64,
        height: f64,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Label {
    /// The label's characters (`Piece::Char`), as the paragraph's are.
    pub pieces: Vec<Piece>,
    pub width: f64,
    pub metric: u32,
    /// What follows the label: a tab (true) or a space/nothing.
    pub tab: bool,
    pub space: f64,
    /// `w:lvlJc`: the label and its suffix space start at, are centred on,
    /// or end at the first-line indent.
    pub align: Align,
}

impl Label {
    /// How far the label starts before the first-line indent. Word for Mac
    /// centres the label with its suffix space, measured at the label's size.
    pub(crate) fn lead(&self) -> f64 {
        let extent = self.width + self.space;
        match self.align {
            Align::Center => extent / 2.0,
            Align::Right => extent,
            _ => 0.0,
        }
    }
}

/// An equation's pieces and its linear text, which a PDF gives them as
/// `ActualText` so that the equation copies as `a^2+b^2=c^2`.
#[derive(Debug, Clone)]
#[allow(dead_code, reason = "the PDF painter reads them")]
pub struct Equation {
    pub pieces: std::ops::Range<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Para {
    pub story: StoryId,
    pub node: Option<NodeId>,
    pub pieces: Vec<Piece>,
    pub before: f64,
    pub after: f64,
    pub before_auto: bool,
    pub after_auto: bool,
    pub line: LineSpacing,
    pub left: f64,
    pub right: f64,
    /// First-line indent (negative: hanging).
    pub first: f64,
    pub keep_next: bool,
    pub keep_lines: bool,
    pub page_break_before: bool,
    pub widow_control: bool,
    pub contextual: bool,
    pub align: Align,
    /// A justified line ending in a line break is spread too, unless the
    /// document sets `w:doNotExpandShiftReturn`.
    pub spread_breaks: bool,
    pub style: String,
    pub tabs: Vec<TabStop>,
    pub label: Option<Label>,
    /// The paragraph mark's metric (empty paragraphs, the last line).
    pub mark: u32,
    pub bookmarks: Vec<String>,
    /// Fields layout computes, in order, none inside another.
    pub fields: Vec<FieldSpan>,
    pub equations: Vec<Equation>,
    /// Ends a section (`w:sectPr` in its properties): the next section's
    /// index.
    pub section_break: Option<usize>,
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    pub shade: Option<Rgb>,
    /// Read only: layout gives them no height yet. `None` when it has none.
    #[allow(dead_code, reason = "the PDF painter reads them")]
    pub borders: Option<Box<ParaBorders>>,
}

#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub width: f64,
    pub margins: [f64; 4],
    pub blocks: Vec<Block>,
    /// 0: none, 1: restart, 2: continue.
    pub vmerge: u8,
    #[allow(dead_code, reason = "the PDF painter reads it")]
    pub fill: Option<Rgb>,
    #[allow(dead_code, reason = "the PDF painter reads it")]
    pub valign: VAlign,
    #[allow(dead_code, reason = "the PDF painter reads them")]
    pub borders: CellBorders,
}

#[derive(Debug, Clone, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub height: Option<(f64, bool)>,
    /// Width of the border above the row (the table's top border, else
    /// the inside one), and below it for the last row: Word adds both to
    /// the row's height.
    pub border_top: f64,
    pub border_bottom: f64,
    pub cant_split: bool,
    pub header: bool,
    pub node: Option<NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub story: StoryId,
    pub rows: Vec<Row>,
    /// The table's left edge from the column's: `w:tblInd`, less the first
    /// cell's left margin before Word 2013.
    pub left: f64,
}

/// What writes a field's value: its result's font and look.
#[derive(Debug, Clone)]
pub struct FieldPen {
    pub measurer: Measurer,
    pub look: u32,
}

#[derive(Debug, Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "most blocks are paragraphs: boxing them only adds an allocation each"
)]
pub enum Block {
    Para(Para),
    Table(Table),
}

/// `w:pgNumType w:start` is kept within 0 and the largest 32-bit integer,
/// so page numbers stay small to write.
const MAX_PAGE_START: i64 = i32::MAX as i64;

/// A section's headers or footers: indices into `Model.stories`, each
/// inherited from the previous section when the section names none.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Slots {
    pub default: Option<usize>,
    pub first: Option<usize>,
    pub even: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Story {
    pub id: StoryId,
    pub blocks: Vec<Block>,
    /// Holds a field whose value depends on the page, so it is laid out
    /// for each page; its property fields are shown already.
    pub fields: bool,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub page: Page,
    pub headers: Slots,
    pub footers: Slots,
    /// A distinct first page (`w:titlePg`).
    pub title_page: bool,
    /// `w:pgNumType w:start`.
    pub page_start: Option<i64>,
    /// `w:pgNumType w:fmt`.
    pub number_format: NumberFormat,
    /// Starts on the same page (`continuous`).
    pub continuous: bool,
    pub notes: NoteSettings,
}

/// A face a run asks for, and the face that measures it.
#[derive(Debug, Clone)]
#[cfg_attr(
    not(feature = "pdf"),
    allow(dead_code, reason = "the PDF painter reads it")
)]
pub struct RunFace {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub face: Arc<Face>,
}

pub struct Model {
    pub sections: Vec<Section>,
    pub blocks: Vec<Block>,
    /// Headers and footers, each once however many sections show it.
    pub stories: Vec<Story>,
    /// Even pages show the even headers and footers
    /// (`w:evenAndOddHeaders`).
    pub even_and_odd: bool,
    pub metrics: Vec<Metric>,
    pub looks: Vec<Appearance>,
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    pub faces: Vec<RunFace>,
    pub links: Vec<Link>,
    pub default_tab: f64,
    pub warnings: Vec<LayoutWarning>,
    /// Before Word 2013 (compatibility mode under 15, or none), a paragraph
    /// `pageBreakBefore` moves to a new page keeps some space before.
    pub page_break_keeps_space: bool,
    pub notes: Notes,
}

pub(super) struct Builder<'a> {
    fmt: Formatting,
    kinds: HashMap<String, FontKind>,
    pub fonts: &'a mut FontBook,
    pub metrics: Vec<Metric>,
    metric_index: HashMap<(u64, u64), u32>,
    /// Each look and its index.
    looks: HashMap<Appearance, u32>,
    faces: Vec<RunFace>,
    face_index: HashMap<(String, Style), u32>,
    links: Vec<Link>,
    link_index: HashMap<Link, u32>,
    /// The story part's hyperlink relationships: id to target.
    hyperlinks: HashMap<String, String>,
    /// The link of the `w:hyperlink` or `w:fldSimple` being read.
    link: Option<u32>,
    sections_seen: usize,
    story: StoryId,
    notes: notes::NoteState<'a>,
    depth: usize,
    properties: Properties,
    warnings: Vec<LayoutWarning>,
    /// Before Word 2013 (compatibility mode under 15, or none), a table
    /// not nested in another moves left by its first cell's left margin.
    outdent_tables: bool,
    spread_breaks: bool,
    /// The last display equation of the paragraph being read.
    display: Option<equation::Display>,
}

fn part<'t>(txn: &'t Txn<'_>, main: &str, rel: &str) -> Option<&'t XmlTree> {
    let name = txn.part_related_by(main, rel).ok().flatten()?;
    txn.tree(&name).ok()
}

pub(super) fn build(txn: &Txn<'_>, main: &str, fonts: &mut FontBook) -> Result<Model> {
    let tree = txn.tree(main)?;
    let body = w_child(tree, tree.root(), "body");
    let sects = body.map_or_else(Vec::new, |body| section_properties(tree, body));
    let note_settings = NoteSettings::document(part(txn, main, rt::SETTINGS));
    let mut section_notes: Vec<NoteSettings> = sects
        .iter()
        .map(|sect| note_settings.section(tree, *sect))
        .collect();
    if section_notes.is_empty() {
        section_notes.push(note_settings);
    }
    let settings = part(txn, main, rt::SETTINGS);
    let compat = |name: &str| {
        settings.is_some_and(|s| {
            w_child(s, s.root(), "compat")
                .and_then(|c| w_child(s, c, name))
                .is_some_and(|n| on_off(s, n))
        })
    };
    let mut builder = Builder {
        fmt: Formatting::read(
            part(txn, main, rt::STYLES),
            part(txn, main, rt::NUMBERING),
            part(txn, main, rt::THEME),
        ),
        kinds: read_font_kinds(part(txn, main, rt::FONT_TABLE)),
        fonts,
        metrics: Vec::new(),
        metric_index: HashMap::new(),
        looks: HashMap::new(),
        faces: Vec::new(),
        face_index: HashMap::new(),
        links: Vec::new(),
        link_index: HashMap::new(),
        hyperlinks: HashMap::new(),
        link: None,
        sections_seen: 0,
        story: StoryId::Body,
        notes: notes::NoteState::read(txn, main, section_notes.clone()),
        depth: 0,
        properties: super::fields::properties(txn),
        warnings: Vec::new(),
        outdent_tables: settings
            .and_then(crate::wml::compatibility_mode)
            .is_none_or(|mode| mode < 15),
        spread_breaks: !compat("doNotExpandShiftReturn"),
        display: None,
    };
    let default_tab = settings
        .and_then(|s| w_child(s, s.root(), "defaultTabStop").and_then(|d| twips(s, d, "val")))
        .unwrap_or(36.0);
    let even_and_odd = settings
        .is_some_and(|s| w_child(s, s.root(), "evenAndOddHeaders").is_some_and(|e| on_off(s, e)));
    let rels = txn.resolved_relationships(main)?;
    let mut sections = Vec::new();
    let mut stories: Vec<Story> = Vec::new();
    let mut by_part: HashMap<&str, usize> = HashMap::new();
    let (mut headers, mut footers) = (Slots::default(), Slots::default());
    for (sect, notes) in sects.iter().zip(&section_notes) {
        for StoryRef {
            kind,
            which,
            part: name,
        } in story_references(tree, *sect, &rels)
        {
            let Ok(story) = txn.tree(name) else {
                continue;
            };
            let index = *by_part.entry(name).or_insert_with(|| {
                let count = stories
                    .iter()
                    .filter(|s| s.is_header() == (kind == "header"));
                let n = count.count() as u32 + 1;
                builder.story = if kind == "header" {
                    StoryId::Header(n)
                } else {
                    StoryId::Footer(n)
                };
                builder.hyperlinks = hyperlinks(txn, name);
                let mut blocks = Vec::new();
                builder.blocks(story, story.root(), None, &mut blocks);
                let values = Values::default();
                let properties = Context {
                    values: &values,
                    page: None,
                };
                super::fields::show(&mut blocks, &properties);
                stories.push(Story {
                    id: builder.story,
                    fields: super::fields::per_page(&blocks),
                    blocks,
                });
                stories.len() - 1
            });
            let slots = if kind == "header" {
                &mut headers
            } else {
                &mut footers
            };
            match which.as_str() {
                "first" => slots.first = Some(index),
                "even" => slots.even = Some(index),
                _ => slots.default = Some(index),
            }
        }
        let numbering = w_child(tree, *sect, "pgNumType");
        sections.push(Section {
            page: page_of(tree, Some(*sect)),
            headers,
            footers,
            title_page: w_child(tree, *sect, "titlePg").is_some_and(|t| on_off(tree, t)),
            page_start: numbering
                .and_then(|n| w_attr(tree, n, "start"))
                .and_then(|v| v.parse::<i64>().ok())
                .map(|start| start.clamp(0, MAX_PAGE_START)),
            number_format: NumberFormat::of_section(numbering.and_then(|n| w_attr(tree, n, "fmt"))),
            continuous: w_child(tree, *sect, "type").and_then(|t| w_attr(tree, t, "val"))
                == Some("continuous"),
            notes: notes.clone(),
        });
    }
    if sections.is_empty() {
        sections.push(Section {
            page: page_of(tree, None),
            headers,
            footers,
            title_page: false,
            page_start: None,
            number_format: NumberFormat::Arabic,
            continuous: false,
            notes: section_notes[0].clone(),
        });
    }
    builder.story = StoryId::Body;
    builder.hyperlinks = hyperlinks(txn, main);
    let mut blocks = Vec::new();
    if let Some(body) = body {
        builder.blocks(tree, body, None, &mut blocks);
    }
    let notes = builder.note_stories();
    Ok(Model {
        sections,
        blocks,
        stories,
        even_and_odd,
        metrics: builder.metrics,
        looks: in_order(builder.looks),
        faces: builder.faces,
        links: builder.links,
        default_tab,
        warnings: builder.warnings,
        page_break_keeps_space: settings
            .and_then(crate::wml::compatibility_mode)
            .is_none_or(|mode| mode < 15),
        notes,
    })
}

impl Story {
    fn is_header(&self) -> bool {
        matches!(self.id, StoryId::Header(_))
    }
}

fn in_order<T>(indexed: HashMap<T, u32>) -> Vec<T> {
    let mut items: Vec<(T, u32)> = indexed.into_iter().collect();
    items.sort_by_key(|(_, index)| *index);
    items.into_iter().map(|(item, _)| item).collect()
}

fn hyperlinks(txn: &Txn<'_>, part: &str) -> HashMap<String, String> {
    txn.relationships(part)
        .unwrap_or_default()
        .into_iter()
        .filter(|rel| rel.rel_type == rt::HYPERLINK)
        .map(|rel| (rel.id, rel.target))
        .collect()
}

/// Section properties in body order: those ending a section inside a
/// paragraph, then the body's own (the last section).
pub(crate) fn section_properties(tree: &XmlTree, body: NodeId) -> Vec<NodeId> {
    tree.descendants(body)
        .filter(|&node| {
            tree.is(node, W, "sectPr")
                && tree
                    .parent(node)
                    .is_some_and(|p| p == body || tree.is(p, W, "pPr"))
        })
        .collect()
}

/// A section's header or footer reference that resolves to a part.
pub(crate) struct StoryRef<'r> {
    /// `header` or `footer`.
    pub kind: &'static str,
    /// `default`, `first` or `even`.
    pub which: String,
    pub part: &'r str,
}

pub(crate) fn story_references<'r>(
    tree: &XmlTree,
    sect: NodeId,
    rels: &'r [(Relationship, String)],
) -> Vec<StoryRef<'r>> {
    tree.child_elements(sect)
        .filter_map(|reference| {
            let kind = match tree.local_name(reference) {
                Some("headerReference") => "header",
                Some("footerReference") => "footer",
                _ => return None,
            };
            let id = tree.attr(reference, Some(R), "id")?;
            let (_, part) = rels.iter().find(|(rel, _)| rel.id == id)?;
            Some(StoryRef {
                kind,
                which: w_attr(tree, reference, "type")
                    .unwrap_or("default")
                    .to_string(),
                part,
            })
        })
        .collect()
}

fn page_of(tree: &XmlTree, sect: Option<NodeId>) -> Page {
    let size = sect.and_then(|s| w_child(tree, s, "pgSz"));
    let mar = sect.and_then(|s| w_child(tree, s, "pgMar"));
    let get = |node: Option<NodeId>, name: &str, default: f64| {
        node.and_then(|n| twips(tree, n, name)).unwrap_or(default)
    };
    let width = get(size, "w", 612.0);
    let height = get(size, "h", 792.0);
    let left = get(mar, "left", 72.0);
    let right = get(mar, "right", 72.0);
    let gutter = get(mar, "gutter", 0.0);
    Page {
        width,
        height,
        top: get(mar, "top", 72.0),
        bottom: get(mar, "bottom", 72.0),
        header: get(mar, "header", 36.0),
        footer: get(mar, "footer", 36.0),
        left,
        right,
        gutter,
        text_width: (width - left - right - gutter).max(36.0),
    }
}

#[cfg(test)]
mod tests;
