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
mod look;
mod numbering;
mod props;
mod styles;
mod table;
mod theme;

use std::collections::HashMap;

use super::fonts::{FontBook, FontKind};
use crate::Result;
use crate::package::rels::{Relationship, rt};
use crate::txn::Txn;
use crate::wml::{R, W, w_attr, w_child};
use crate::xml::{NodeId, XmlTree};
pub use colour::{Colour, Rgb};
pub use look::{Appearance, Caps, Link, Strike, Underline, UnderlineStyle};
use look::{CellBorders, ParaBorders, VAlign};
use numbering::Numbering;
pub(crate) use props::on_off;
use props::twips;
use styles::Styles;
use theme::{Theme, read_font_kinds, read_theme};

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
    Clear,
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
    Object {
        width: f64,
        height: f64,
    },
}

#[derive(Debug, Clone)]
pub struct Label {
    pub width: f64,
    pub metric: u32,
    #[allow(dead_code, reason = "the PDF painter reads it")]
    pub look: u32,
    /// What follows the label: a tab (true) or a space/nothing.
    pub tab: bool,
    pub space: f64,
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
    pub style: String,
    pub tabs: Vec<(f64, TabKind)>,
    pub label: Option<Label>,
    /// The paragraph mark's metric (empty paragraphs, the last line).
    pub mark: u32,
    pub bookmarks: Vec<String>,
    /// Ends a section (`w:sectPr` in its properties): the next section's
    /// index.
    pub section_break: Option<usize>,
    #[allow(dead_code, reason = "the PDF painter reads it")]
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
    pub rows: Vec<Row>,
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

#[derive(Debug, Clone)]
pub struct Section {
    pub page: Page,
    /// Default header and footer content heights.
    pub header_height: f64,
    pub footer_height: f64,
    /// First-page ones, when the section has a distinct first page.
    pub first_header_height: Option<f64>,
    pub first_footer_height: Option<f64>,
    /// `w:pgNumType w:start`.
    pub page_start: Option<i64>,
    /// Starts on the same page (`continuous`).
    pub continuous: bool,
}

pub struct Model {
    pub sections: Vec<Section>,
    pub blocks: Vec<Block>,
    pub metrics: Vec<Metric>,
    pub looks: Vec<Appearance>,
    pub links: Vec<Link>,
    pub default_tab: f64,
    /// Layout work already done measuring headers and footers.
    pub work: u64,
}

pub(super) struct Builder<'a> {
    styles: Styles,
    numbering: Numbering,
    theme: Theme,
    kinds: HashMap<String, FontKind>,
    pub fonts: &'a mut FontBook,
    pub metrics: Vec<Metric>,
    metric_index: HashMap<(u64, u64), u32>,
    /// Each look and its index.
    looks: HashMap<Appearance, u32>,
    links: Vec<Link>,
    link_index: HashMap<Link, u32>,
    /// The story part's hyperlink relationships: id to target.
    hyperlinks: HashMap<String, String>,
    /// The link of the `w:hyperlink` or `w:fldSimple` being read.
    link: Option<u32>,
    sections_seen: usize,
    story: StoryId,
    depth: usize,
}

fn part<'t>(txn: &'t Txn<'_>, main: &str, rel: &str) -> Option<&'t XmlTree> {
    let name = txn.part_related_by(main, rel).ok().flatten()?;
    txn.tree(&name).ok()
}

pub(super) fn build(txn: &Txn<'_>, main: &str, fonts: &mut FontBook) -> Result<Model> {
    let tree = txn.tree(main)?;
    let theme = read_theme(part(txn, main, rt::THEME));
    let mut builder = Builder {
        styles: Styles::read(part(txn, main, rt::STYLES), &theme.palette),
        numbering: Numbering::read(part(txn, main, rt::NUMBERING), &theme.palette),
        theme,
        kinds: read_font_kinds(part(txn, main, rt::FONT_TABLE)),
        fonts,
        metrics: Vec::new(),
        metric_index: HashMap::new(),
        looks: HashMap::new(),
        links: Vec::new(),
        link_index: HashMap::new(),
        hyperlinks: HashMap::new(),
        link: None,
        sections_seen: 0,
        story: StoryId::Body,
        depth: 0,
    };
    let default_tab = part(txn, main, rt::SETTINGS)
        .and_then(|s| w_child(s, s.root(), "defaultTabStop").and_then(|d| twips(s, d, "val")))
        .unwrap_or(36.0);
    let body = w_child(tree, tree.root(), "body");
    let sects = body.map_or_else(Vec::new, |body| section_properties(tree, body));
    let mut sections = Vec::new();
    let mut inherited: HashMap<(&'static str, String), f64> = HashMap::new();
    let rels = txn.resolved_relationships(main)?;
    let mut stories: HashMap<String, Vec<Block>> = HashMap::new();
    let mut heights: HashMap<(String, u64), f64> = HashMap::new();
    let (mut headers, mut footers) = (0, 0);
    let mut work = 0;
    for sect in &sects {
        let page = page_of(tree, Some(*sect));
        for StoryRef {
            kind,
            which,
            part: name,
        } in story_references(tree, *sect, &rels)
        {
            let Ok(story) = txn.tree(name) else {
                continue;
            };
            let blocks = stories.entry(name.to_string()).or_insert_with(|| {
                builder.hyperlinks = hyperlinks(txn, name);
                builder.story = if kind == "header" {
                    headers += 1;
                    StoryId::Header(headers)
                } else {
                    footers += 1;
                    StoryId::Footer(footers)
                };
                let mut blocks = Vec::new();
                builder.blocks(story, story.root(), None, &mut blocks);
                blocks
            });
            let key = (name.to_string(), page.text_width.to_bits());
            let height = *heights.entry(key).or_insert_with(|| {
                let (height, cost) = super::flow::stack_height(
                    blocks,
                    &builder.metrics,
                    page.text_width,
                    default_tab,
                );
                work += cost;
                height
            });
            super::flow::within(work, super::flow::layout_work())?;
            inherited.insert((kind, which), height);
        }
        let get =
            |kind: &'static str, which: &str| inherited.get(&(kind, which.to_string())).copied();
        let first_page = w_child(tree, *sect, "titlePg").is_some_and(|t| on_off(tree, t));
        sections.push(Section {
            page,
            header_height: get("header", "default").unwrap_or(0.0),
            footer_height: get("footer", "default").unwrap_or(0.0),
            first_header_height: first_page.then(|| get("header", "first").unwrap_or(0.0)),
            first_footer_height: first_page.then(|| get("footer", "first").unwrap_or(0.0)),
            page_start: w_child(tree, *sect, "pgNumType")
                .and_then(|n| w_attr(tree, n, "start"))
                .and_then(|v| v.parse().ok()),
            continuous: w_child(tree, *sect, "type").and_then(|t| w_attr(tree, t, "val"))
                == Some("continuous"),
        });
    }
    if sections.is_empty() {
        sections.push(Section {
            page: page_of(tree, None),
            header_height: 0.0,
            footer_height: 0.0,
            first_header_height: None,
            first_footer_height: None,
            page_start: None,
            continuous: false,
        });
    }
    builder.story = StoryId::Body;
    builder.hyperlinks = hyperlinks(txn, main);
    let mut blocks = Vec::new();
    if let Some(body) = body {
        builder.blocks(tree, body, None, &mut blocks);
    }
    Ok(Model {
        sections,
        blocks,
        metrics: builder.metrics,
        looks: in_order(builder.looks),
        links: builder.links,
        default_tab,
        work,
    })
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
