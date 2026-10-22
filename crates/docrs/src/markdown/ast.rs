//! Markdown events (pulldown-cmark) folded into a small block/inline tree.
//!
//! Rendering needs things the event stream only reveals later — whether a
//! list is tight, which footnotes are defined, whether a paragraph holds
//! nothing but an image — so the whole document is collected first.

use std::collections::BTreeMap;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, MetadataBlockKind, Options,
    Parser, Tag, TagEnd,
};

use super::MarkdownWarning;
use crate::{Error, Result};

/// The deepest structure a Markdown or HTML source may nest: block quotes,
/// lists, definition lists, footnotes, tables, inline spans (emphasis,
/// links, images, HTML spans), HTML elements and math groups all count, one
/// level each (math counts the levels its Word equation nests). Deeper
/// input is refused with [`crate::ErrorKind::ResourceLimit`] and reason
/// `markdown_nesting_too_deep` before anything is changed. The cap keeps
/// conversion off the call stack's limits and the written XML well inside
/// [`crate::Limits::max_xml_depth`]'s default of 256.
pub const MAX_NESTING_DEPTH: usize = 128;

/// Word's limit on the columns of a table.
pub(crate) const MAX_TABLE_COLUMNS: usize = 63;

/// The refusal for structure nested `observed` levels deep.
pub(crate) fn too_deep(observed: usize) -> Error {
    Error::limit(
        "markdown_nesting_too_deep",
        MAX_NESTING_DEPTH as u64,
        observed as u64,
    )
    .context(format!(
        "the source nests deeper than {MAX_NESTING_DEPTH} levels; nothing was changed"
    ))
}

/// Add a warning unless an identical one (code and message) is already
/// there: one warning per kind of loss, at its first line.
pub(crate) fn warn_once(
    warnings: &mut Vec<MarkdownWarning>,
    code: &'static str,
    message: String,
    line: Option<usize>,
) {
    if !warnings
        .iter()
        .any(|w| w.code == code && w.message == message)
    {
        warnings.push(MarkdownWarning {
            code,
            message,
            line,
        });
    }
}

/// Column alignment of a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    None,
    Left,
    Center,
    Right,
    Justify,
}

/// Vertical alignment of a table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VAlign {
    Top,
    Center,
    Bottom,
}

/// Direct run formatting an editor set on a span (HTML `style`): colours as
/// `RRGGBB`, the first font family, a size in half-points.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunStyle {
    pub color: Option<String>,
    pub highlight: Option<String>,
    pub font: Option<String>,
    pub size: Option<u32>,
}

/// One HTML table cell's spans and vertical alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellSpan {
    pub colspan: usize,
    pub rowspan: usize,
    pub valign: Option<VAlign>,
}

impl Default for CellSpan {
    fn default() -> Self {
        CellSpan {
            colspan: 1,
            rowspan: 1,
            valign: None,
        }
    }
}

/// What an HTML table says beyond its cells: spans, header rows and the
/// editor's column widths (CSS pixels; only their proportions matter).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TableLayout {
    /// Per row (head first, then body rows), per cell.
    pub spans: Vec<Vec<CellSpan>>,
    /// Leading rows that repeat as the header (head first, then body).
    pub header_rows: usize,
    /// Width of each grid column, when the editor gave one.
    pub widths: Vec<Option<f64>>,
    /// The table's own width (`style="width: 520px"`).
    pub table_width: Option<f64>,
}

/// A grid position of a table row once spans are placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The row's cell `index`, starting at column `col`; `merges_down` when
    /// it spans further rows.
    Cell {
        index: usize,
        col: usize,
        span: CellSpan,
        merges_down: bool,
    },
    /// Covered by a cell from a row above (Word's `vMerge` continue).
    Continue { col: usize, span: CellSpan },
    /// No cell: the row is shorter than the grid.
    Empty { col: usize },
}

impl Slot {
    pub(crate) fn col(&self) -> usize {
        match *self {
            Slot::Cell { col, .. } | Slot::Continue { col, .. } | Slot::Empty { col } => col,
        }
    }

    pub(crate) fn colspan(&self) -> usize {
        match self {
            Slot::Cell { span, .. } | Slot::Continue { span, .. } => span.colspan,
            Slot::Empty { .. } => 1,
        }
    }
}

/// Place each row's cells on the grid (HTML's table model: a cell takes the
/// next column not covered by a row span from above). Every row gets a slot
/// for every grid column, sorted by column; returns the rows and the
/// column count. Spans never widen the grid past Word's 63 columns: a span
/// reaching past the last column is cut back to it, and a cell starting
/// beyond it spans one column and one row.
pub(crate) fn place(spans: &[Vec<CellSpan>]) -> (Vec<Vec<Slot>>, usize) {
    let rows = spans.len();
    let mut covered: Vec<Vec<bool>> = vec![Vec::new(); rows];
    let mut out: Vec<Vec<Slot>> = vec![Vec::new(); rows];
    let is_covered = |covered: &Vec<Vec<bool>>, row: usize, col: usize| {
        covered[row].get(col).copied().unwrap_or(false)
    };
    for (row, cells) in spans.iter().enumerate() {
        let mut col = 0;
        for (index, span) in cells.iter().enumerate() {
            while is_covered(&covered, row, col) {
                col += 1;
            }
            let (colspan, rowspan) = clamped_span(span, col, rows - row);
            for (offset, line) in covered[row..row + rowspan].iter_mut().enumerate() {
                if line.len() < col + colspan {
                    line.resize(col + colspan, false);
                }
                line[col..col + colspan].fill(true);
                let span = CellSpan {
                    colspan,
                    rowspan,
                    ..*span
                };
                out[row + offset].push(if offset == 0 {
                    Slot::Cell {
                        index,
                        col,
                        span,
                        merges_down: rowspan > 1,
                    }
                } else {
                    Slot::Continue { col, span }
                });
            }
            col += colspan;
        }
    }
    let columns = covered.iter().map(Vec::len).max().unwrap_or(0);
    for (row, slots) in out.iter_mut().enumerate() {
        for col in 0..columns {
            if !is_covered(&covered, row, col) {
                slots.push(Slot::Empty { col });
            }
        }
        slots.sort_by_key(Slot::col);
    }
    (out, columns)
}

/// A cell's `(colspan, rowspan)` once placed at `col` with `rows_left` rows
/// from its own to the table's end (see [`place`]).
fn clamped_span(span: &CellSpan, col: usize, rows_left: usize) -> (usize, usize) {
    if col >= MAX_TABLE_COLUMNS {
        return (1, 1);
    }
    (
        span.colspan.clamp(1, MAX_TABLE_COLUMNS - col),
        span.rowspan.clamp(1, rows_left.max(1)),
    )
}

/// GitHub alert kind (`> [!NOTE]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Inline {
    Text(String),
    Code(String),
    Math(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Superscript(Vec<Inline>),
    Subscript(Vec<Inline>),
    Underline(Vec<Inline>),
    Highlight(Vec<Inline>),
    /// Editor colours, highlight and font.
    Styled(RunStyle, Vec<Inline>),
    Link {
        url: String,
        title: String,
        content: Vec<Inline>,
    },
    Image {
        url: String,
        title: String,
        alt: Vec<Inline>,
        line: usize,
        /// Display size in CSS pixels (HTML `width` / `height`).
        width: Option<f64>,
        height: Option<f64>,
    },
    FootnoteRef {
        label: String,
        line: usize,
    },
    SoftBreak,
    HardBreak,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    Heading {
        level: u8,
        id: Option<String>,
        content: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    /// Inline content placed directly in a tight list item.
    Plain(Vec<Inline>),
    Code {
        language: Option<String>,
        text: String,
    },
    Quote {
        alert: Option<AlertKind>,
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        tight: bool,
        items: Vec<ListItem>,
    },
    Table {
        aligns: Vec<Align>,
        /// Header cells; each cell holds blocks (GFM cells hold one
        /// `Plain`, HTML cells may hold paragraphs and lists).
        head: Vec<Vec<Block>>,
        rows: Vec<Vec<Vec<Block>>>,
        /// HTML tables: spans, header rows, editor column widths.
        layout: Option<Box<TableLayout>>,
    },
    /// A block with an explicit horizontal alignment (HTML `text-align`).
    Aligned(Align, Box<Block>),
    DefinitionList(Vec<(Vec<Inline>, Vec<Vec<Block>>)>),
    DisplayMath(String),
    Rule,
    PageBreak,
    Toc,
    /// Raw HTML the converter cannot represent (reported, not rendered).
    Html {
        html: String,
        line: usize,
    },
}

/// A parsed document: blocks, footnote definitions and front matter text.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub blocks: Vec<Block>,
    pub footnotes: BTreeMap<String, Vec<Block>>,
    pub metadata: Option<(MetadataKind, String)>,
    pub unknown_html: Vec<(usize, String)>,
    /// CSS declarations with no Word equivalent (`property on <tag>`).
    pub dropped_styles: Vec<(usize, String)>,
    /// Other losses found while parsing (skipped HTML elements, tables
    /// narrowed to Word's column limit).
    pub warnings: Vec<MarkdownWarning>,
    /// Parsed from HTML: a picture alone in a paragraph is an ordinary
    /// paragraph, not a figure.
    pub html: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataKind {
    Yaml,
    Toml,
}

/// Parser options for the supported dialect (CommonMark + GFM tables, task
/// lists, strikethrough, footnotes, alerts; definition lists, heading
/// attributes, super/subscript, math, front matter).
pub(crate) fn options(smart_punctuation: bool) -> Options {
    let mut options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT;
    if smart_punctuation {
        options |= Options::ENABLE_SMART_PUNCTUATION;
    }
    options
}

/// Open container while folding events.
enum Frame {
    Root,
    Paragraph,
    Heading {
        level: u8,
        id: Option<String>,
    },
    Quote(Option<AlertKind>),
    Code {
        language: Option<String>,
        text: String,
    },
    Html {
        html: String,
        line: usize,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
        tight: bool,
    },
    Item {
        task: Option<bool>,
    },
    Footnote(String),
    DefinitionList(Vec<(Vec<Inline>, Vec<Vec<Block>>)>),
    DefinitionTitle,
    Definition,
    Table {
        aligns: Vec<Align>,
        head: Vec<Vec<Block>>,
        rows: Vec<Vec<Vec<Block>>>,
        in_head: bool,
    },
    TableRow(Vec<Vec<Block>>),
    TableCell,
    Metadata(MetadataKind, String),
    /// Inline span (emphasis, link, …): children collect into `inlines`.
    Span(SpanKind),
    Image {
        url: String,
        title: String,
        line: usize,
    },
}

impl Frame {
    /// Whether the frame is a level of [`MAX_NESTING_DEPTH`]. The others
    /// add at most one frame to a level: an item to its list, a row and
    /// cell to their table, a paragraph or heading to its container.
    fn nests(&self) -> bool {
        matches!(
            self,
            Frame::Quote(_)
                | Frame::List { .. }
                | Frame::Footnote(_)
                | Frame::DefinitionList(_)
                | Frame::Table { .. }
                | Frame::Span(_)
                | Frame::Image { .. }
        )
    }
}

#[derive(Clone)]
enum SpanKind {
    Emphasis,
    Strong,
    Strike,
    Superscript,
    Subscript,
    Underline,
    Highlight,
    /// `<kbd>`, `<code>`, `<samp>`, `<tt>`: inline code.
    Code,
    /// `<b>`/`<i>`-style raw HTML spans close on their own tag.
    Html(&'static str, Box<SpanKind>),
    /// Editor formatting of a `<span>`, `<font>` or `<mark>`.
    Format(Box<super::html::SpanFormat>),
    /// An element browsers do not display (`<script>`, `<style>`, …): its
    /// content is dropped.
    Hidden,
    Link {
        url: String,
        title: String,
    },
}

struct Level {
    frame: Frame,
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
}

impl Level {
    fn new(frame: Frame) -> Self {
        Level {
            frame,
            blocks: Vec::new(),
            inlines: Vec::new(),
        }
    }
}

struct Folder<'s> {
    stack: Vec<Level>,
    /// Open frames that count toward [`MAX_NESTING_DEPTH`].
    depth: usize,
    /// The refusal that stops folding (nesting too deep).
    error: Option<Error>,
    footnotes: BTreeMap<String, Vec<Block>>,
    metadata: Option<(MetadataKind, String)>,
    unknown_html: Vec<(usize, String)>,
    dropped_styles: Vec<(usize, String)>,
    warnings: Vec<MarkdownWarning>,
    line_starts: Vec<usize>,
    _source: &'s str,
}

fn line_of(line_starts: &[usize], offset: usize) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(index) => index + 1,
        Err(index) => index,
    }
}

/// Parse `source` into blocks. Refuses structure nested deeper than
/// [`MAX_NESTING_DEPTH`].
pub(crate) fn parse(source: &str, smart_punctuation: bool) -> Result<Parsed> {
    let mut line_starts = vec![0];
    line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
    let mut folder = Folder {
        stack: vec![Level::new(Frame::Root)],
        depth: 0,
        error: None,
        footnotes: BTreeMap::new(),
        metadata: None,
        unknown_html: Vec::new(),
        dropped_styles: Vec::new(),
        warnings: Vec::new(),
        line_starts,
        _source: source,
    };
    for (event, range) in Parser::new_ext(source, options(smart_punctuation)).into_offset_iter() {
        let line = line_of(&folder.line_starts, range.start);
        folder.event(event, line);
        if let Some(error) = folder.error.take() {
            return Err(error);
        }
    }
    // Close anything an unbalanced raw-HTML span left open.
    while folder.stack.len() > 1 {
        folder.close();
    }
    let root = folder.stack.pop().expect("root level");
    let mut blocks = root.blocks;
    if !root.inlines.is_empty() {
        blocks.push(Block::Paragraph(root.inlines));
    }
    let finish = |blocks: Vec<Block>| -> Vec<Block> {
        blocks
            .into_iter()
            .map(special_paragraph)
            .map(intraword_scripts)
            .collect()
    };
    Ok(Parsed {
        blocks: finish(blocks),
        footnotes: folder
            .footnotes
            .into_iter()
            .map(|(label, blocks)| (label, finish(blocks)))
            .collect(),
        metadata: folder.metadata,
        unknown_html: folder.unknown_html,
        dropped_styles: folder.dropped_styles,
        warnings: folder.warnings,
        html: false,
    })
}

/// Parse an HTML document or fragment (an editor's HTML) into blocks.
/// Refuses elements nested deeper than [`MAX_NESTING_DEPTH`].
pub(crate) fn parse_html(source: &str) -> Result<Parsed> {
    let mut parsed = Parsed {
        html: true,
        ..Parsed::default()
    };
    let (blocks, unmapped) = super::html::convert(source, 1, 0)?;
    record_unmapped(
        unmapped,
        1,
        &mut parsed.unknown_html,
        &mut parsed.dropped_styles,
        &mut parsed.warnings,
    );
    parsed.blocks = blocks;
    Ok(parsed)
}

/// File what an HTML conversion starting on `line` could not map.
fn record_unmapped(
    unmapped: super::html::Unmapped,
    line: usize,
    unknown: &mut Vec<(usize, String)>,
    dropped: &mut Vec<(usize, String)>,
    warnings: &mut Vec<MarkdownWarning>,
) {
    for tag in unmapped.tags {
        unknown.push((line, format!("<{tag}>")));
    }
    for style in unmapped.styles {
        dropped.push((line, style));
    }
    for tag in unmapped.skipped {
        warn_once(
            warnings,
            "html_element_skipped",
            skipped_message(&tag),
            Some(line),
        );
    }
    if unmapped.narrowed_table {
        warn_once(
            warnings,
            "table_columns_clamped",
            TABLE_CLAMPED_MESSAGE.to_string(),
            Some(line),
        );
    }
}

/// The warning for an element browsers do not display.
pub(crate) fn skipped_message(tag: &str) -> String {
    format!("HTML <{tag}> is not displayed; it was dropped with its content")
}

const TABLE_CLAMPED_MESSAGE: &str = "a table is wider than Word's 63 columns; spans past the last column were cut back and cells past it were merged into the last column";

/// `\newpage`, `\pagebreak` and `[TOC]` paragraphs become their blocks.
fn special_paragraph(block: Block) -> Block {
    match &block {
        Block::Paragraph(content) => match content.as_slice() {
            [Inline::Text(text)] => match text.trim() {
                "\\newpage" | "\\pagebreak" => Block::PageBreak,
                "[TOC]" | "[[_TOC_]]" | "[[TOC]]" => Block::Toc,
                _ => block,
            },
            _ => block,
        },
        _ => block,
    }
}

/// Pandoc reads `H~2~O` and `x^2^` inside words; the parser only accepts
/// `~sub~` and `^sup^` at word boundaries, so text runs are rescanned.
fn intraword_scripts(block: Block) -> Block {
    match block {
        Block::Heading { level, id, content } => Block::Heading {
            level,
            id,
            content: scripts(content),
        },
        Block::Paragraph(content) => Block::Paragraph(scripts(content)),
        Block::Plain(content) => Block::Plain(scripts(content)),
        Block::Quote { alert, blocks } => Block::Quote {
            alert,
            blocks: blocks.into_iter().map(intraword_scripts).collect(),
        },
        Block::List {
            start,
            tight,
            items,
        } => Block::List {
            start,
            tight,
            items: items
                .into_iter()
                .map(|item| ListItem {
                    task: item.task,
                    blocks: item.blocks.into_iter().map(intraword_scripts).collect(),
                })
                .collect(),
        },
        Block::Table {
            aligns,
            head,
            rows,
            layout,
        } => Block::Table {
            aligns,
            head: head.into_iter().map(cell_scripts).collect(),
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(cell_scripts).collect())
                .collect(),
            layout,
        },
        Block::Aligned(align, block) => Block::Aligned(align, Box::new(intraword_scripts(*block))),
        Block::DefinitionList(entries) => Block::DefinitionList(
            entries
                .into_iter()
                .map(|(term, definitions)| {
                    (
                        scripts(term),
                        definitions
                            .into_iter()
                            .map(|blocks| blocks.into_iter().map(intraword_scripts).collect())
                            .collect(),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

fn cell_scripts(blocks: Vec<Block>) -> Vec<Block> {
    blocks.into_iter().map(intraword_scripts).collect()
}

fn scripts(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.extend(split_scripts(&text)),
            Inline::Emphasis(c) => out.push(Inline::Emphasis(scripts(c))),
            Inline::Strong(c) => out.push(Inline::Strong(scripts(c))),
            Inline::Strike(c) => out.push(Inline::Strike(scripts(c))),
            Inline::Underline(c) => out.push(Inline::Underline(scripts(c))),
            Inline::Highlight(c) => out.push(Inline::Highlight(scripts(c))),
            Inline::Styled(style, c) => out.push(Inline::Styled(style, scripts(c))),
            Inline::Link {
                url,
                title,
                content,
            } => out.push(Inline::Link {
                url,
                title,
                content: scripts(content),
            }),
            other => out.push(other),
        }
    }
    out
}

/// `~x~` → subscript, `^x^` → superscript: non-empty, no whitespace, and
/// not part of a `~~` run.
fn split_scripts(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let delimited = (c == '~' || c == '^')
            && chars
                .get(index + 1)
                .is_some_and(|n| *n != c && !n.is_whitespace())
            && (index == 0 || chars[index - 1] != c);
        if delimited {
            let close = (index + 1..chars.len())
                .take_while(|i| !chars[*i].is_whitespace())
                .find(|i| chars[*i] == c);
            if let Some(close) = close.filter(|close| chars.get(close + 1) != Some(&c)) {
                if !plain.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut plain)));
                }
                let inner = vec![Inline::Text(chars[index + 1..close].iter().collect())];
                out.push(if c == '~' {
                    Inline::Subscript(inner)
                } else {
                    Inline::Superscript(inner)
                });
                index = close + 1;
                continue;
            }
        }
        plain.push(c);
        index += 1;
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) | Inline::Math(text) => out.push_str(text),
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strike(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::Underline(children)
            | Inline::Highlight(children)
            | Inline::Styled(_, children)
            | Inline::Link {
                content: children, ..
            }
            | Inline::Image { alt: children, .. } => out.push_str(&plain_text(children)),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
            Inline::FootnoteRef { .. } => {}
        }
    }
    out
}

/// Text of inline content as a reader sees it (alt text, captions, slugs).
pub(crate) fn inline_text(inlines: &[Inline]) -> String {
    plain_text(inlines)
}

/// A recognized inline HTML tag: `(name, closing)`.
fn html_tag(html: &str) -> Option<(String, bool)> {
    let inner = html.trim().strip_prefix('<')?.strip_suffix('>')?;
    let inner = inner.trim().trim_end_matches('/').trim();
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest.trim()),
        None => (false, inner),
    };
    let name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    (!name.is_empty()).then_some((name, closing))
}

fn html_span(name: &str) -> Option<SpanKind> {
    let (tag, kind): (&'static str, SpanKind) = match name {
        "b" => ("b", SpanKind::Strong),
        "strong" => ("strong", SpanKind::Strong),
        "i" => ("i", SpanKind::Emphasis),
        "em" => ("em", SpanKind::Emphasis),
        "cite" => ("cite", SpanKind::Emphasis),
        "u" => ("u", SpanKind::Underline),
        "ins" => ("ins", SpanKind::Underline),
        "s" => ("s", SpanKind::Strike),
        "del" => ("del", SpanKind::Strike),
        "strike" => ("strike", SpanKind::Strike),
        "sup" => ("sup", SpanKind::Superscript),
        "sub" => ("sub", SpanKind::Subscript),
        "mark" => ("mark", SpanKind::Highlight),
        "kbd" => ("kbd", SpanKind::Code),
        "code" => ("code", SpanKind::Code),
        "samp" => ("samp", SpanKind::Code),
        "tt" => ("tt", SpanKind::Code),
        _ => return None,
    };
    Some(SpanKind::Html(tag, Box::new(kind)))
}

/// HTML that means "page break": `<!-- pagebreak -->`, or one empty element
/// such as `<div data-page-break="">` or a `div` with
/// `page-break-before/after: always` or `break-before/after: page`.
pub(crate) fn is_page_break_html(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "<!--pagebreak-->"
        || compact == "<!--newpage-->"
        || super::html::is_page_break_element(html)
}

/// Blocks for raw HTML at nesting `depth`: page-break markers, intentional
/// blank lines (`<p></p>`), and the HTML subset `html::convert` reads; tags
/// and CSS declarations it cannot map are recorded in `unknown` and
/// `dropped`, other losses in `warnings`.
pub(crate) fn html_blocks(
    html: &str,
    line: usize,
    depth: usize,
    unknown: &mut Vec<(usize, String)>,
    dropped: &mut Vec<(usize, String)>,
    warnings: &mut Vec<MarkdownWarning>,
) -> Result<Vec<Block>> {
    let trimmed = html.trim();
    if is_page_break_html(trimmed) {
        return Ok(vec![Block::PageBreak]);
    }
    if trimmed.is_empty()
        || is_comment(trimmed)
        || matches!(html_tag(trimmed), Some((name, _)) if name == "br")
    {
        return Ok(Vec::new());
    }
    if super::html::is_empty_paragraph(trimmed) {
        return Ok(vec![Block::Paragraph(Vec::new())]);
    }
    let (blocks, unmapped) = super::html::convert(trimmed, line, depth)?;
    let skipped_only = blocks.is_empty() && !unmapped.skipped.is_empty();
    record_unmapped(unmapped, line, unknown, dropped, warnings);
    if blocks.is_empty() && !skipped_only {
        return Ok(vec![Block::Html {
            html: html.to_string(),
            line,
        }]);
    }
    Ok(blocks)
}

fn is_comment(html: &str) -> bool {
    let trimmed = html.trim();
    trimmed.starts_with("<!--") && trimmed.ends_with("-->")
}

impl Folder<'_> {
    fn top(&mut self) -> &mut Level {
        self.stack.last_mut().expect("level")
    }

    fn push(&mut self, frame: Frame) {
        if frame.nests() {
            self.depth += 1;
            if self.depth > MAX_NESTING_DEPTH && self.error.is_none() {
                self.error = Some(too_deep(self.depth));
            }
        }
        self.stack.push(Level::new(frame));
    }

    /// Close the raw-HTML spans open at the top: inline HTML cannot hold
    /// blocks, so an unclosed one ends with its block, as browsers end it.
    fn close_html_spans(&mut self) {
        while matches!(self.top().frame, Frame::Span(SpanKind::Html(..))) {
            self.close();
        }
    }

    /// Inline content of a tight list item (or stray inline content in a
    /// block container) becomes a block before the next block starts.
    fn flush_inlines(&mut self) {
        self.close_html_spans();
        let level = self.top();
        if level.inlines.is_empty() {
            return;
        }
        let inlines = std::mem::take(&mut level.inlines);
        let block = match level.frame {
            Frame::Item { .. } => Block::Plain(inlines),
            _ => Block::Paragraph(inlines),
        };
        level.blocks.push(block);
    }

    /// Adjacent text events (the parser splits text at `[`, `!`, …) merge.
    fn push_inline(&mut self, inline: Inline) {
        let inlines = &mut self.top().inlines;
        if let (Some(Inline::Text(last)), Inline::Text(text)) = (inlines.last_mut(), &inline) {
            last.push_str(text);
            return;
        }
        inlines.push(inline);
    }

    fn event(&mut self, event: Event<'_>, line: usize) {
        // Text inside a code block, HTML block or metadata block is raw.
        match (&mut self.top().frame, &event) {
            (Frame::Code { text, .. }, Event::Text(chunk)) => {
                text.push_str(chunk);
                return;
            }
            (Frame::Html { html, .. }, Event::Html(chunk) | Event::Text(chunk)) => {
                html.push_str(chunk);
                return;
            }
            (Frame::Metadata(_, text), Event::Text(chunk)) => {
                text.push_str(chunk);
                return;
            }
            _ => {}
        }
        match event {
            Event::Start(tag) => self.start(tag, line),
            Event::End(end) => self.end(end),
            Event::Text(text) => self.push_inline(Inline::Text(text.into_string())),
            Event::Code(code) => self.push_inline(Inline::Code(code.into_string())),
            Event::InlineMath(math) => {
                self.check_math(&math);
                self.push_inline(Inline::Math(math.into_string()));
            }
            Event::DisplayMath(math) => {
                self.check_math(&math);
                match self.top().frame {
                    // Display math inside a paragraph splits it.
                    Frame::Paragraph => {
                        let inlines = std::mem::take(&mut self.top().inlines);
                        let level = self.stack.len() - 2;
                        if !inlines.is_empty() {
                            self.stack[level].blocks.push(Block::Paragraph(inlines));
                        }
                        self.stack[level]
                            .blocks
                            .push(Block::DisplayMath(math.into_string()));
                    }
                    Frame::Root
                    | Frame::Quote(_)
                    | Frame::Item { .. }
                    | Frame::Footnote(_)
                    | Frame::Definition => {
                        self.flush_inlines();
                        self.top()
                            .blocks
                            .push(Block::DisplayMath(math.into_string()));
                    }
                    // Inside a span, heading or cell, which hold no blocks:
                    // kept in line.
                    _ => self.push_inline(Inline::Math(math.into_string())),
                }
            }
            Event::Html(html) => {
                // Html outside an HtmlBlock frame: treat as a block of its own.
                self.flush_inlines();
                self.html_block(html.into_string(), line);
            }
            Event::InlineHtml(html) => self.inline_html(&html, line),
            Event::FootnoteReference(label) => self.push_inline(Inline::FootnoteRef {
                label: label.into_string(),
                line,
            }),
            Event::SoftBreak => self.push_inline(Inline::SoftBreak),
            Event::HardBreak => self.push_inline(Inline::HardBreak),
            Event::Rule => {
                self.flush_inlines();
                self.top().blocks.push(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                for level in self.stack.iter_mut().rev() {
                    if let Frame::Item { task } = &mut level.frame {
                        *task = Some(checked);
                        break;
                    }
                }
            }
        }
    }

    fn inline_html(&mut self, html: &str, line: usize) {
        if is_comment(html) {
            return;
        }
        let Some((name, closing)) = html_tag(html) else {
            self.unknown_html.push((line, html.to_string()));
            return;
        };
        if name == "br" {
            self.push_inline(Inline::HardBreak);
            return;
        }
        if closing {
            // Close the innermost matching raw-HTML span, if any. An
            // unmatched end tag needs no report: its start tag (if any) was
            // already reported or was a transparent wrapper.
            let Some(index) = self.stack.iter().rposition(
                |level| matches!(&level.frame, Frame::Span(SpanKind::Html(tag, _)) if *tag == name),
            ) else {
                return;
            };
            // Only spans can be open inside it. With an image open there
            // the end tag waits for the image's container to close it.
            if self.stack[index + 1..]
                .iter()
                .any(|level| !matches!(level.frame, Frame::Span(_)))
            {
                return;
            }
            // Markdown spans opened inside it (`<b>*x</b>*`) end with it and
            // reopen after it, as browsers treat misnested formatting: the
            // parser still has to close them.
            let mut reopen = Vec::new();
            while self.stack.len() > index + 1 {
                if let Frame::Span(kind) = &self.top().frame
                    && !matches!(kind, SpanKind::Html(..))
                {
                    reopen.push(kind.clone());
                }
                self.close();
            }
            self.close();
            for kind in reopen.into_iter().rev() {
                self.push(Frame::Span(kind));
            }
            return;
        }
        if let Some(tag) = super::html::hidden_tag(&name) {
            warn_once(
                &mut self.warnings,
                "html_element_skipped",
                skipped_message(tag),
                Some(line),
            );
            // A void element (`<meta>`) holds nothing to drop.
            if !super::html::is_void(tag) && !html.trim_end().ends_with("/>") {
                self.push(Frame::Span(SpanKind::Html(tag, Box::new(SpanKind::Hidden))));
            }
            return;
        }
        // Spans carrying editor formatting (`style`, `<mark data-color>`).
        if let ("span" | "font" | "mark", Some(format)) =
            (name.as_str(), super::html::tag_format(html))
        {
            for dropped in &format.dropped {
                self.dropped_styles.push((line, dropped.clone()));
            }
            let tag = match name.as_str() {
                "span" => "span",
                "font" => "font",
                _ => "mark",
            };
            self.push(Frame::Span(SpanKind::Html(
                tag,
                Box::new(SpanKind::Format(Box::new(format))),
            )));
            return;
        }
        match html_span(&name) {
            Some(kind) => self.push(Frame::Span(kind)),
            // Presentation-only wrappers: their text needs no markup.
            None if matches!(name.as_str(), "small" | "big" | "abbr") => {}
            None => self.unknown_html.push((line, html.to_string())),
        }
    }

    fn html_block(&mut self, html: String, line: usize) {
        match html_blocks(
            &html,
            line,
            self.depth,
            &mut self.unknown_html,
            &mut self.dropped_styles,
            &mut self.warnings,
        ) {
            Ok(blocks) => self.top().blocks.extend(blocks),
            Err(error) => self.fail(error),
        }
    }

    /// Record the refusal that stops folding (the first one wins).
    fn fail(&mut self, error: Error) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    /// Math nests on top of the Markdown around it.
    fn check_math(&mut self, source: &str) {
        if let Err(error) = super::math::check_depth(source, self.depth) {
            self.fail(error);
        }
    }

    fn start(&mut self, tag: Tag<'_>, line: usize) {
        match tag {
            Tag::Paragraph => {
                self.flush_inlines();
                self.push(Frame::Paragraph);
            }
            Tag::Heading { level, id, .. } => {
                self.flush_inlines();
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.push(Frame::Heading {
                    level,
                    id: id.map(|id| id.into_string()),
                });
            }
            Tag::BlockQuote(kind) => {
                self.flush_inlines();
                self.push(Frame::Quote(kind.map(|kind| match kind {
                    BlockQuoteKind::Note => AlertKind::Note,
                    BlockQuoteKind::Tip => AlertKind::Tip,
                    BlockQuoteKind::Important => AlertKind::Important,
                    BlockQuoteKind::Warning => AlertKind::Warning,
                    BlockQuoteKind::Caution => AlertKind::Caution,
                })));
            }
            Tag::CodeBlock(kind) => {
                self.flush_inlines();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                        .filter(|l| !l.is_empty()),
                    CodeBlockKind::Indented => None,
                };
                self.push(Frame::Code {
                    language,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => {
                self.flush_inlines();
                self.push(Frame::Html {
                    html: String::new(),
                    line,
                });
            }
            Tag::List(start) => {
                self.flush_inlines();
                self.push(Frame::List {
                    start,
                    items: Vec::new(),
                    tight: true,
                });
            }
            Tag::Item => self.push(Frame::Item { task: None }),
            Tag::FootnoteDefinition(label) => {
                self.flush_inlines();
                self.push(Frame::Footnote(label.into_string()));
            }
            Tag::DefinitionList => {
                self.flush_inlines();
                self.push(Frame::DefinitionList(Vec::new()));
            }
            Tag::DefinitionListTitle => self.push(Frame::DefinitionTitle),
            Tag::DefinitionListDefinition => self.push(Frame::Definition),
            Tag::Table(aligns) => {
                self.flush_inlines();
                self.push(Frame::Table {
                    aligns: aligns
                        .into_iter()
                        .map(|a| match a {
                            Alignment::None => Align::None,
                            Alignment::Left => Align::Left,
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::Right,
                        })
                        .collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Frame::Table { in_head, .. } = &mut self.top().frame {
                    *in_head = true;
                }
                self.push(Frame::TableRow(Vec::new()));
            }
            Tag::TableRow => self.push(Frame::TableRow(Vec::new())),
            Tag::TableCell => self.push(Frame::TableCell),
            Tag::Emphasis => self.push(Frame::Span(SpanKind::Emphasis)),
            Tag::Strong => self.push(Frame::Span(SpanKind::Strong)),
            Tag::Strikethrough => self.push(Frame::Span(SpanKind::Strike)),
            Tag::Superscript => self.push(Frame::Span(SpanKind::Superscript)),
            Tag::Subscript => self.push(Frame::Span(SpanKind::Subscript)),
            Tag::Link {
                dest_url, title, ..
            } => self.push(Frame::Span(SpanKind::Link {
                url: dest_url.into_string(),
                title: title.into_string(),
            })),
            Tag::Image {
                dest_url, title, ..
            } => self.push(Frame::Image {
                url: dest_url.into_string(),
                title: title.into_string(),
                line,
            }),
            Tag::MetadataBlock(kind) => self.push(Frame::Metadata(
                match kind {
                    MetadataBlockKind::YamlStyle => MetadataKind::Yaml,
                    MetadataBlockKind::PlusesStyle => MetadataKind::Toml,
                },
                String::new(),
            )),
        }
    }

    fn end(&mut self, end: TagEnd) {
        // Raw-HTML spans left open inside this container close with it.
        self.close_html_spans();
        if matches!(end, TagEnd::List(_) | TagEnd::Item | TagEnd::BlockQuote(_)) {
            self.flush_inlines();
        }
        self.close();
    }

    /// Pop the top level and attach what it built to its parent. The root
    /// stays.
    fn close(&mut self) {
        if self.stack.len() < 2 {
            return;
        }
        let Level {
            frame,
            mut blocks,
            inlines,
        } = self.stack.pop().expect("level to close");
        if frame.nests() {
            self.depth -= 1;
        }
        let parent = self.stack.last_mut().expect("parent level");
        match frame {
            Frame::Root => unreachable!("root is never closed"),
            Frame::Paragraph => {
                if !inlines.is_empty() {
                    parent.blocks.push(Block::Paragraph(inlines));
                }
            }
            Frame::Heading { level, id } => parent.blocks.push(Block::Heading {
                level,
                id,
                content: inlines,
            }),
            Frame::Quote(alert) => {
                if !inlines.is_empty() {
                    blocks.push(Block::Paragraph(inlines));
                }
                parent.blocks.push(Block::Quote { alert, blocks });
            }
            Frame::Code { language, text } => parent.blocks.push(Block::Code { language, text }),
            Frame::Html { html, line } => self.html_block(html, line),
            Frame::List {
                start,
                items,
                tight,
            } => parent.blocks.push(Block::List {
                start,
                tight,
                items,
            }),
            Frame::Item { task } => {
                if !inlines.is_empty() {
                    blocks.push(Block::Plain(inlines));
                }
                let loose = blocks.iter().any(|b| matches!(b, Block::Paragraph(_)));
                if let Frame::List { items, tight, .. } = &mut parent.frame {
                    if loose {
                        *tight = false;
                    }
                    items.push(ListItem { task, blocks });
                }
            }
            Frame::Footnote(label) => {
                if !inlines.is_empty() {
                    blocks.push(Block::Paragraph(inlines));
                }
                self.footnotes.entry(label).or_insert(blocks);
            }
            Frame::DefinitionList(entries) => parent.blocks.push(Block::DefinitionList(entries)),
            Frame::DefinitionTitle => {
                if let Frame::DefinitionList(entries) = &mut parent.frame {
                    entries.push((inlines, Vec::new()));
                }
            }
            Frame::Definition => {
                if !inlines.is_empty() {
                    blocks.push(Block::Paragraph(inlines));
                }
                if let Frame::DefinitionList(entries) = &mut parent.frame {
                    match entries.last_mut() {
                        Some((_, definitions)) => definitions.push(blocks),
                        None => entries.push((Vec::new(), vec![blocks])),
                    }
                }
            }
            Frame::Table {
                mut aligns,
                mut head,
                mut rows,
                ..
            } => {
                let mut narrowed = fold_wide_row(&mut head);
                for row in &mut rows {
                    narrowed |= fold_wide_row(row);
                }
                aligns.truncate(MAX_TABLE_COLUMNS);
                parent.blocks.push(Block::Table {
                    aligns,
                    head,
                    rows,
                    layout: None,
                });
                if narrowed {
                    warn_once(
                        &mut self.warnings,
                        "table_columns_clamped",
                        TABLE_CLAMPED_MESSAGE.to_string(),
                        None,
                    );
                }
            }
            Frame::TableRow(cells) => {
                if let Frame::Table {
                    head,
                    rows,
                    in_head,
                    ..
                } = &mut parent.frame
                {
                    if *in_head {
                        *head = cells;
                        *in_head = false;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            Frame::TableCell => {
                if let Frame::TableRow(cells) = &mut parent.frame {
                    cells.push(vec![Block::Plain(inlines)]);
                }
            }
            Frame::Metadata(kind, text) => {
                if self.metadata.is_none() {
                    self.metadata = Some((kind, text));
                }
            }
            Frame::Span(SpanKind::Html(_, kind)) if matches!(*kind, SpanKind::Hidden) => {}
            Frame::Span(kind) => {
                let inline = span_inline(kind, inlines);
                parent.inlines.push(inline);
            }
            Frame::Image { url, title, line } => parent.inlines.push(Inline::Image {
                url,
                title,
                alt: inlines,
                line,
                width: None,
                height: None,
            }),
        }
    }
}

/// A table row past Word's 63 columns keeps 63 cells: the cells past the
/// last column move into it, one after another. Returns whether any moved.
pub(crate) fn fold_wide_row(cells: &mut Vec<Vec<Block>>) -> bool {
    if cells.len() <= MAX_TABLE_COLUMNS {
        return false;
    }
    let overflow: Vec<Block> = cells.drain(MAX_TABLE_COLUMNS..).flatten().collect();
    cells[MAX_TABLE_COLUMNS - 1].extend(overflow);
    true
}

fn span_inline(kind: SpanKind, children: Vec<Inline>) -> Inline {
    match kind {
        SpanKind::Emphasis => Inline::Emphasis(children),
        SpanKind::Strong => Inline::Strong(children),
        SpanKind::Strike => Inline::Strike(children),
        SpanKind::Superscript => Inline::Superscript(children),
        SpanKind::Subscript => Inline::Subscript(children),
        SpanKind::Underline => Inline::Underline(children),
        SpanKind::Highlight => Inline::Highlight(children),
        SpanKind::Code => Inline::Code(plain_text(&children)),
        SpanKind::Hidden => Inline::Text(String::new()),
        SpanKind::Html(_, inner) => span_inline(*inner, children),
        SpanKind::Format(format) => {
            let mut wrapped = format.wrap(children);
            if wrapped.len() == 1 {
                wrapped.pop().expect("one inline")
            } else {
                Inline::Styled(RunStyle::default(), wrapped)
            }
        }
        SpanKind::Link { url, title } => Inline::Link {
            url,
            title,
            content: children,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(source: &str) -> Vec<Block> {
        parse(source, false).unwrap().blocks
    }

    #[test]
    fn tight_and_loose_lists() {
        let tight = blocks("- a\n- b\n");
        assert!(matches!(&tight[0], Block::List { tight: true, items, .. }
            if matches!(items[0].blocks[0], Block::Plain(_))));
        let loose = blocks("- a\n\n- b\n");
        assert!(matches!(&loose[0], Block::List { tight: false, .. }));
    }

    #[test]
    fn task_markers_attach_to_their_item() {
        let parsed = blocks("- [x] done\n- [ ] todo\n");
        let Block::List { items, .. } = &parsed[0] else {
            panic!("list")
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
    }

    #[test]
    fn inline_html_spans_nest() {
        let parsed = blocks("a <u>b <sup>c</sup></u> d<br>e\n");
        let Block::Paragraph(inlines) = &parsed[0] else {
            panic!("paragraph")
        };
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Underline(children)
            if children.iter().any(|c| matches!(c, Inline::Superscript(_)))))
        );
        assert!(inlines.contains(&Inline::HardBreak));
    }

    #[test]
    fn page_breaks_and_toc_markers() {
        let parsed = blocks("a\n\n\\newpage\n\n<!-- pagebreak -->\n\n[TOC]\n");
        assert_eq!(parsed[1], Block::PageBreak);
        assert_eq!(parsed[2], Block::PageBreak);
        assert_eq!(parsed[3], Block::Toc);
    }

    #[test]
    fn footnotes_and_front_matter_are_collected() {
        let parsed = parse("---\ntitle: T\n---\n\nText[^n].\n\n[^n]: Note.\n", false).unwrap();
        assert_eq!(parsed.metadata.unwrap().1.trim(), "title: T");
        assert!(parsed.footnotes.contains_key("n"));
    }

    #[test]
    fn intraword_sub_and_superscripts() {
        let parsed = blocks("H~2~O, x^2^ and ~~gone~~ but not ~ 5 ~ or 2^ x\n");
        let Block::Paragraph(inlines) = &parsed[0] else {
            panic!("paragraph")
        };
        assert_eq!(
            inlines[1],
            Inline::Subscript(vec![Inline::Text("2".into())])
        );
        assert_eq!(
            inlines[3],
            Inline::Superscript(vec![Inline::Text("2".into())])
        );
        assert!(matches!(inlines[5], Inline::Strike(_)));
        assert_eq!(inlines[6], Inline::Text(" but not ~ 5 ~ or 2^ x".into()));
    }

    #[test]
    fn table_head_and_rows() {
        let parsed = blocks("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        let Block::Table {
            aligns, head, rows, ..
        } = &parsed[0]
        else {
            panic!("table")
        };
        assert_eq!(aligns, &[Align::Left, Align::Right]);
        assert_eq!(head.len(), 2);
        assert_eq!(rows.len(), 1);
    }
}
