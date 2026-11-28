//! Line breaking and pagination.
//!
//! Lines break greedily at spaces, after hyphens and dashes, and inside a
//! word only when the word alone is wider than the line. A line is as tall
//! as its text (the largest ascent plus line gap and the largest descent of
//! its fonts, times the paragraph's line spacing) or its tallest inline
//! object. Pages fill top to bottom with the
//! paragraph spacing, page breaks, keep-with-next, keep-lines and
//! widow/orphan control; table rows are as tall as their tallest cell and
//! split across pages between lines.

use std::ops::Range;

use super::fields::Values;
use super::furniture::{Furniture, PageAt, Pages};
use super::model::{Align, Block, LineSpacing, Metric, Model, Para, Piece, Row, TabKind, Table};
use crate::xml::NodeId;
use crate::{Error, Result};

/// A tab on a line: its piece, and the x it moves from and to. A left tab
/// that would pass the right margin ends its line with no width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TabPlace {
    pub piece: u32,
    pub from: f64,
    pub to: f64,
}

/// One laid-out line. x values are points from the column's left edge.
///
/// Left, centred and right-aligned lines start where their text is drawn.
/// A justified line starts at the left, and the painter spreads `slack`
/// over its `gaps`; a negative `slack` shrinks them, and can be left on a
/// justified paragraph's last line too.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub height: f64,
    /// The height that must fit above the bottom margin: Word lets the
    /// extra height of multiple line spacing overflow it.
    pub fit: f64,
    pub page_break: bool,
    /// Into the paragraph's pieces, trailing spaces and breaks included.
    pub pieces: Range<u32>,
    /// From the line top.
    pub baseline: f64,
    /// The largest descent of the line's fonts.
    pub below: f64,
    pub start: f64,
    /// From `start` to the end of the last piece that is not a space.
    pub width: f64,
    /// The width left over before alignment: the width available less
    /// the content's extent.
    pub slack: f64,
    /// Spaces between words after the last tab: the spaces a justified
    /// line may shrink.
    pub gaps: u32,
    pub tabs: Vec<TabPlace>,
    /// Ends the paragraph, or in a line or page break.
    pub last: bool,
}

pub(super) const EPSILON: f64 = 0.01;

/// How much of its width each space of a justified line may give up for
/// the line to fit (Word for Mac, measured between 0.235 and 0.240).
const JUSTIFIED_SHRINK: f64 = 0.2375;

fn breaks_after(ch: char) -> bool {
    matches!(ch, '-' | '\u{2010}' | '\u{2013}' | '\u{2014}' | '\u{00AD}')
}

fn metric(metrics: &[Metric], metric: u32) -> Metric {
    metrics.get(metric as usize).copied().unwrap_or(Metric {
        height: 12.0,
        ascent: 9.6,
    })
}

/// The text extent of a line. Word for Mac stacks the largest ascent on
/// the largest descent of the fonts on the line, the line gap counting
/// above the text; multiple line spacing adds each font's share of extra
/// height (`extra`: the multiple less one), which [`baseline`] puts below.
#[derive(Clone, Copy, Default)]
struct Extent {
    above: f64,
    below: f64,
    /// The largest ascent without the extra height.
    ascent: f64,
    /// The tallest text font's single line height.
    single: f64,
    /// A list label's ascent.
    label: Option<f64>,
}

impl Extent {
    fn add(&mut self, metrics: &[Metric], metric: u32, extra: f64) {
        let (height, ascent) = metrics
            .get(metric as usize)
            .map_or((12.0, 9.6), |m| (m.height, m.ascent));
        self.above = self.above.max(ascent + extra * height);
        self.ascent = self.ascent.max(ascent);
        self.below = self.below.max(height - ascent);
        self.single = self.single.max(height);
    }

    /// A list label counts with its ascent only, plus the extra height of
    /// the line's text font (the paragraph mark's on a line without text).
    fn add_label(&mut self, metrics: &[Metric], metric: u32) {
        self.label = Some(metrics.get(metric as usize).map_or(9.6, |m| m.ascent));
    }

    fn height(self, mark: f64, extra: f64) -> f64 {
        let label = self.label.map_or(0.0, |ascent| {
            let text = if self.single > 0.0 { self.single } else { mark };
            ascent + extra * text
        });
        self.above.max(label) + self.below
    }

    /// The largest ascent, the label's included.
    fn top_ascent(self) -> f64 {
        self.ascent.max(self.label.unwrap_or(0.0))
    }

    /// The height without multiple line spacing's extra.
    fn single_height(self) -> f64 {
        self.top_ascent() + self.below
    }
}

/// The height of a line whose text is `text` points tall (multiple line
/// spacing included) and tallest object `object` points.
fn line_height(para: &Para, text: f64, object: f64) -> f64 {
    match para.line {
        LineSpacing::Exact(points) => points,
        LineSpacing::Multiple(_) => text.max(object),
        LineSpacing::AtLeast(points) => text.max(points).max(object),
    }
}

/// From the line top (Word for Mac 16.115, the ascent including the line
/// gap): a multiple of 1 or more puts its extra below the descent,
/// at-least spacing above the ascent, and an exact line or a multiple
/// below 1 has its baseline at 0.8 of its height. A multiple line's object
/// taller than that sits on the baseline, which is never below the line's
/// height less its descent (a line of a list label only).
fn baseline(para: &Para, height: f64, heights: &Heights, object: f64) -> f64 {
    let above = height - heights.below;
    let on_object = object - heights.below;
    match para.line {
        LineSpacing::Multiple(multiple) if multiple < 1.0 => (0.8 * height).max(on_object),
        LineSpacing::Multiple(_) => above.min(heights.ascent.max(on_object)),
        LineSpacing::Exact(points) => 0.8 * points,
        LineSpacing::AtLeast(_) => above,
    }
}

/// Next tab stop after `x` (points from the text-area left edge).
fn tab_stop(para: &Para, x: f64, first_line: bool, default_tab: f64) -> (f64, TabKind) {
    if first_line && para.first < 0.0 && x + EPSILON < para.left {
        // The hanging indent is a tab stop on the first line.
        let custom = para
            .tabs
            .iter()
            .find(|(pos, _)| *pos > x + EPSILON && *pos < para.left);
        return custom.copied().unwrap_or((para.left, TabKind::Left));
    }
    if let Some(stop) = para.tabs.iter().find(|(pos, _)| *pos > x + EPSILON) {
        return *stop;
    }
    let step = if default_tab > 1.0 { default_tab } else { 36.0 };
    ((((x + EPSILON) / step).floor() + 1.0) * step, TabKind::Left)
}

/// Width of `pieces` up to the next tab or break, or up to the decimal
/// point when `decimal` and there is one before them.
fn segment_width(pieces: &[Piece], decimal: bool) -> f64 {
    let mut width = 0.0;
    for piece in pieces {
        match piece {
            Piece::Char { ch: '.', .. } if decimal => break,
            Piece::Char { width: w, .. } | Piece::Object { width: w, .. } => width += w,
            _ => break,
        }
    }
    width
}

/// A line's end if it broke here.
#[derive(Clone, Copy)]
struct End {
    content: f64,
    gaps: u32,
    tabs: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Ending {
    Wrap,
    LineBreak,
    PageBreak,
    Paragraph,
}

struct Heights {
    text: f64,
    single: f64,
    ascent: f64,
    below: f64,
}

/// The line being filled.
struct Open {
    first_piece: usize,
    x: f64,
    text: Extent,
    /// Width of the spaces between words since the last tab.
    spaces: f64,
    object: f64,
    has_content: bool,
    /// Last break opportunity (piece index to restart from).
    last_break: Option<(usize, End)>,
    start: f64,
    content: f64,
    gaps: u32,
    /// Spaces after the last word, gaps once another word follows.
    pending: u32,
    words: bool,
    tabs: Vec<TabPlace>,
}

impl Open {
    fn new(first_piece: usize, x: f64) -> Open {
        Open {
            first_piece,
            x,
            text: Extent::default(),
            spaces: 0.0,
            object: 0.0,
            has_content: false,
            last_break: None,
            start: x,
            content: x,
            gaps: 0,
            pending: 0,
            words: false,
            tabs: Vec::new(),
        }
    }

    fn end(&self) -> End {
        End {
            content: self.content,
            gaps: self.gaps,
            tabs: self.tabs.len(),
        }
    }

    fn mark_break(&mut self, restart: usize) {
        self.last_break = Some((restart, self.end()));
    }

    fn space(&mut self, width: f64) {
        if self.words {
            self.spaces += width;
            self.pending += 1;
        }
    }

    fn word(&mut self) {
        self.has_content = true;
        self.content = self.x;
        if self.words {
            self.gaps += self.pending;
        }
        self.pending = 0;
        self.words = true;
    }

    fn tab(&mut self, piece: usize, to: f64) {
        self.tabs.push(TabPlace {
            piece: piece as u32,
            from: self.x,
            to,
        });
        self.x = to;
        self.content = to;
        self.has_content = true;
        self.spaces = 0.0;
        self.gaps = 0;
        self.pending = 0;
        self.words = false;
        self.mark_break(piece + 1);
    }
}

/// The lines of a paragraph so far and the one being filled.
struct Breaker<'a> {
    para: &'a Para,
    metrics: &'a [Metric],
    default_tab: f64,
    /// The multiple less one of multiple line spacing.
    extra: f64,
    /// The paragraph mark's single line height, and with the extra.
    mark_single: f64,
    mark: f64,
    mark_below: f64,
    right_limit: f64,
    first_line: bool,
    out: Vec<Line>,
    line: Open,
}

impl Breaker<'_> {
    fn heights(&self, paragraph_end: bool) -> Heights {
        let text = &self.line.text;
        let height = text.height(self.mark_single, self.extra);
        let below = if text.below > 0.0 {
            text.below
        } else {
            self.mark_below
        };
        if height > 0.0 && (self.line.has_content || !paragraph_end) {
            Heights {
                text: height,
                single: text.single_height(),
                ascent: text.top_ascent(),
                below,
            }
        } else if paragraph_end {
            Heights {
                text: height.max(self.mark),
                single: text.single_height().max(self.mark_single),
                ascent: text.top_ascent().max(self.mark_single - self.mark_below),
                below: text.below.max(self.mark_below),
            }
        } else {
            Heights {
                text: self.mark,
                single: self.mark_single,
                ascent: self.mark_single - self.mark_below,
                below: self.mark_below,
            }
        }
    }

    fn close(&mut self, ending: Ending, next_piece: usize, end: End) {
        let heights = self.heights(ending == Ending::Paragraph);
        let para = self.para;
        let open = std::mem::replace(&mut self.line, Open::new(next_piece, para.left));
        let height = line_height(para, heights.text, open.object);
        let slack = self.right_limit - end.content;
        let offset = match para.align {
            Align::Center => slack.max(0.0) / 2.0,
            Align::Right => slack.max(0.0),
            _ => 0.0,
        };
        let mut tabs = open.tabs;
        tabs.truncate(end.tabs);
        for tab in &mut tabs {
            tab.from += offset;
            tab.to += offset;
        }
        self.out.push(Line {
            height,
            fit: line_height(para, heights.single, open.object),
            page_break: ending == Ending::PageBreak,
            pieces: open.first_piece as u32..next_piece as u32,
            baseline: baseline(para, height, &heights, open.object),
            below: heights.below,
            start: open.start + offset,
            width: end.content - open.start,
            slack,
            gaps: end.gaps,
            tabs,
            last: ending != Ending::Wrap,
        });
        self.first_line = false;
    }

    /// The piece to go on with.
    fn char(&mut self, index: usize, ch: char, width: f64, metric: u32) -> usize {
        let is_space = ch == ' ' || ch == '\u{3000}';
        let line = &self.line;
        let shrink = if self.para.align.justified() {
            JUSTIFIED_SHRINK * line.spaces
        } else {
            0.0
        };
        if !is_space && line.x + width > self.right_limit + EPSILON + shrink && line.has_content {
            let (restart, end) = match line.last_break {
                Some((at, end)) if at > line.first_piece => (at, end),
                _ if index > line.first_piece => (index, line.end()),
                _ => (
                    index + 1,
                    End {
                        content: line.x + width,
                        ..line.end()
                    },
                ),
            };
            self.close(Ending::Wrap, restart, end);
            return restart;
        }
        let line = &mut self.line;
        line.x += width;
        line.text.add(self.metrics, metric, self.extra);
        if is_space {
            line.space(width);
            line.mark_break(index + 1);
        } else {
            line.word();
            if breaks_after(ch) {
                line.mark_break(index + 1);
            }
        }
        index + 1
    }

    fn object(&mut self, index: usize, width: f64, height: f64) {
        if self.line.x + width > self.right_limit + EPSILON && self.line.has_content {
            self.close(Ending::Wrap, index, self.line.end());
        }
        let line = &mut self.line;
        line.x += width;
        line.object = line.object.max(height);
        line.word();
        line.mark_break(index + 1);
    }

    fn tab(&mut self, index: usize, metric: u32) {
        let x = self.line.x;
        let (stop, kind) = tab_stop(self.para, x, self.first_line, self.default_tab);
        let pieces = &self.para.pieces[index + 1..];
        let target = match kind {
            TabKind::Right => (stop - segment_width(pieces, false)).max(x),
            TabKind::Decimal => (stop - segment_width(pieces, true)).max(x),
            TabKind::Center => (stop - segment_width(pieces, false) / 2.0).max(x),
            _ => stop,
        };
        self.line.text.add(self.metrics, metric, self.extra);
        if target > self.right_limit + EPSILON && self.line.has_content && kind == TabKind::Left {
            self.line.tabs.push(TabPlace {
                piece: index as u32,
                from: x,
                to: x,
            });
            self.close(Ending::Wrap, index + 1, self.line.end());
        } else {
            self.line.tab(index, target);
        }
    }

    fn manual_break(&mut self, index: usize, ending: Ending) {
        self.close(ending, index + 1, self.line.end());
    }
}

/// Break `para` into lines `width` points wide.
pub(super) fn lines(para: &Para, metrics: &[Metric], width: f64, default_tab: f64) -> Vec<Line> {
    let extra = match para.line {
        LineSpacing::Multiple(multiple) => multiple - 1.0,
        _ => 0.0,
    };
    let mark = metric(metrics, para.mark);
    let mut b = Breaker {
        para,
        metrics,
        default_tab,
        extra,
        mark_single: mark.height,
        mark: mark.height * (1.0 + extra),
        mark_below: mark.height - mark.ascent,
        right_limit: width - para.right,
        first_line: true,
        out: Vec::new(),
        line: Open::new(0, para.left + para.first),
    };
    if let Some(label) = &para.label {
        let line = &mut b.line;
        line.text.add_label(metrics, label.metric);
        line.x += label.width + label.space;
        if label.tab {
            line.x = tab_stop(para, line.x, true, default_tab).0;
        }
        line.has_content = true;
        line.start = line.x;
        line.content = line.x;
    }
    let pieces = &para.pieces;
    let mut index = 0;
    while index < pieces.len() {
        index = match pieces[index] {
            Piece::Char {
                ch, width, metric, ..
            } => b.char(index, ch, width, metric),
            Piece::Object { width, height } => {
                b.object(index, width, height);
                index + 1
            }
            Piece::Tab { metric, .. } => {
                b.tab(index, metric);
                index + 1
            }
            Piece::LineBreak { metric, .. } => {
                b.line.text.add(metrics, metric, extra);
                b.manual_break(index, Ending::LineBreak);
                index + 1
            }
            Piece::PageBreak => {
                b.manual_break(index, Ending::PageBreak);
                index + 1
            }
        };
    }
    // The last line carries the paragraph mark, unless the paragraph ends
    // with a page break (the mark then stays beside the break).
    if b.line.has_content || !matches!(pieces.last(), Some(Piece::PageBreak)) {
        b.close(Ending::Paragraph, pieces.len(), b.line.end());
    }
    b.out
}

/// Blocks stacked in `width`: a header or footer.
pub(super) fn story_layout<'m>(
    blocks: &'m [Block],
    metrics: &[Metric],
    width: f64,
    default_tab: f64,
) -> CellLayout<'m> {
    cell_layout(blocks, metrics, width, default_tab, false)
}

impl Para {
    fn before_spacing(&self) -> f64 {
        if self.before_auto { 14.0 } else { self.before }
    }

    pub(super) fn after_spacing(&self) -> f64 {
        if self.after_auto { 14.0 } else { self.after }
    }
}

/// The gap between `previous` and `para`: Word takes the larger of the
/// previous paragraph's space after and this one's space before, after
/// contextual spacing removed either between paragraphs of one style.
fn gap(previous: Option<&Para>, para: &Para) -> f64 {
    let Some(previous) = previous else {
        return para.before_spacing();
    };
    let same = previous.style == para.style;
    // Automatic (HTML) spacing is not added between list items.
    let list = previous.label.is_some() && para.label.is_some();
    let after = if (same && previous.contextual) || (list && previous.after_auto) {
        0.0
    } else {
        previous.after_spacing()
    };
    let before = if (same && para.contextual) || (list && para.before_auto) {
        0.0
    } else {
        para.before_spacing()
    };
    after.max(before)
}

/// A line of a cell's content: top relative to the content, height, and
/// its place in its paragraph (for widow control when a row splits).
struct CellLine {
    offset: f64,
    height: f64,
    index: usize,
    count: usize,
    widow_control: bool,
}

/// A paragraph (or a nested row's end) and its offset from the top of the
/// content holding it.
#[derive(Clone, Copy)]
struct ParaOffset {
    node: Option<NodeId>,
    offset: f64,
    row_end: bool,
}

impl ParaOffset {
    fn shifted(self, by: f64) -> ParaOffset {
        ParaOffset {
            offset: self.offset + by,
            ..self
        }
    }
}

/// What a cell draws: a paragraph's line or a nested table's row.
pub(super) enum Item<'m> {
    Line(&'m Para, Line),
    Row(&'m Table, usize, f64),
}

/// An item, its top from the top of the content holding it and its x from
/// the content's left edge.
pub(super) struct Drawn<'m> {
    pub offset: f64,
    pub x: f64,
    pub item: Item<'m>,
}

pub(super) struct CellLayout<'m> {
    lines: Vec<CellLine>,
    /// Paragraphs (and nested row ends) with their offsets.
    paras: Vec<ParaOffset>,
    pub drawn: Vec<Drawn<'m>>,
    pub content: f64,
    pub work: u64,
}

/// Lay out blocks as one column (a cell when `cell`: automatic spacing at
/// its top and bottom is dropped).
fn cell_layout<'m>(
    blocks: &'m [Block],
    metrics: &[Metric],
    width: f64,
    default_tab: f64,
    cell: bool,
) -> CellLayout<'m> {
    let mut out = CellLayout {
        lines: Vec::new(),
        paras: Vec::new(),
        drawn: Vec::new(),
        content: 0.0,
        work: 0,
    };
    let mut y = 0.0;
    let mut previous: Option<&Para> = None;
    let mut first = true;
    for block in blocks {
        match block {
            Block::Para(para) => {
                y += if first && cell && para.before_auto {
                    0.0
                } else {
                    gap(previous, para)
                };
                out.paras.push(ParaOffset {
                    node: para.node,
                    offset: y,
                    row_end: false,
                });
                let para_lines = lines(para, metrics, width, default_tab);
                let count = para_lines.len();
                out.work += (para.pieces.len() + count) as u64;
                for (index, line) in para_lines.into_iter().enumerate() {
                    let height = line.height;
                    out.lines.push(CellLine {
                        offset: y,
                        height,
                        index,
                        count,
                        widow_control: para.widow_control,
                    });
                    out.drawn.push(Drawn {
                        offset: y,
                        x: 0.0,
                        item: Item::Line(para, line),
                    });
                    y += height;
                }
                previous = Some(para);
            }
            Block::Table(table) => {
                if let Some(previous) = previous {
                    y += previous.after_spacing();
                }
                for (row_index, row) in table.rows.iter().enumerate() {
                    let layout = row_layout(row, metrics, default_tab);
                    out.work += layout.work;
                    out.paras
                        .extend(layout.paras.iter().map(|para| para.shifted(y)));
                    out.paras.push(ParaOffset {
                        node: row.node,
                        offset: y,
                        row_end: true,
                    });
                    out.lines.push(CellLine {
                        offset: y,
                        height: layout.height,
                        index: 0,
                        count: 1,
                        widow_control: false,
                    });
                    out.drawn.push(Drawn {
                        offset: y,
                        x: 0.0,
                        item: Item::Row(table, row_index, layout.height),
                    });
                    for part in layout.cells {
                        out.drawn
                            .extend(part.layout.drawn.into_iter().map(|drawn| Drawn {
                                offset: drawn.offset + part.top + y,
                                x: drawn.x + part.x,
                                item: drawn.item,
                            }));
                    }
                    y += layout.height;
                }
                previous = None;
            }
        }
        first = false;
    }
    if let Some(last) = previous {
        // A cell's last paragraph keeps no automatic spacing, nor any
        // space after when it has contextual spacing.
        if !(cell && (last.after_auto || last.contextual)) {
            y += last.after_spacing();
        }
    }
    out.content = y;
    out
}

/// The line of a cell its row splits before, widow control applied; none
/// when the cell fits in `available`.
fn cut(part: &CellPart<'_>, available: f64) -> Option<usize> {
    let lines = &part.layout.lines;
    let k = lines
        .iter()
        .position(|line| part.top + line.offset + line.height > available + EPSILON)?;
    let line = &lines[k];
    // No paragraph leaves one line behind or sends one line ahead.
    let widow = line.widow_control
        && line.count >= 2
        && (line.index == 1 || (line.index >= 2 && line.count - line.index == 1));
    Some(if widow { k - 1 } else { k })
}

/// A cell's layout and its content's offset from the row's top-left.
struct CellPart<'m> {
    layout: CellLayout<'m>,
    top: f64,
    x: f64,
}

/// A row's height, its cells' layouts and their paragraphs' offsets from
/// the row top.
struct RowLayout<'m> {
    height: f64,
    paras: Vec<ParaOffset>,
    cells: Vec<CellPart<'m>>,
    work: u64,
}

fn row_layout<'m>(row: &'m Row, metrics: &[Metric], default_tab: f64) -> RowLayout<'m> {
    let mut height = 0.0f64;
    let mut paras = Vec::new();
    let mut cells = Vec::new();
    let mut work = 1;
    let mut x = 0.0;
    for cell in &row.cells {
        let width = (cell.width - cell.margins[2] - cell.margins[3]).max(1.0);
        let layout = cell_layout(&cell.blocks, metrics, width, default_tab, true);
        if cell.vmerge != 2 {
            height = height.max(layout.content + cell.margins[0] + cell.margins[1]);
        }
        let top = row.border_top + cell.margins[0];
        paras.extend(layout.paras.iter().map(|para| para.shifted(top)));
        work += layout.work;
        cells.push(CellPart {
            layout,
            top,
            x: x + cell.margins[2],
        });
        x += cell.width;
    }
    if let Some((value, exact)) = row.height {
        height = if exact { value } else { height.max(value) };
    }
    height += row.border_top + row.border_bottom;
    RowLayout {
        height,
        paras,
        cells,
        work,
    }
}

/// Where each paragraph landed.
#[derive(Debug, Clone)]
pub struct Placed {
    pub node: Option<NodeId>,
    /// Physical page, from 1.
    pub page: u32,
    /// Top of the first line, points from the page top.
    pub y: f64,
    /// A table row's end mark rather than a paragraph.
    pub row_end: bool,
}

/// Points from the page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PageStart {
    pub number: u32,
}

/// The part of a row on one page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RowSlice {
    pub height: f64,
    pub first: bool,
    pub last: bool,
}

impl RowSlice {
    pub(super) fn whole(height: f64) -> RowSlice {
        RowSlice {
            height,
            first: true,
            last: true,
        }
    }
}

/// Each call comes once its placement is final; an `Err` stops pagination.
pub(crate) trait Sink {
    fn page(&mut self, _page: &PageStart) -> Result<()> {
        Ok(())
    }

    /// `at`: the column's left edge and the line's top.
    fn line(&mut self, _at: Point, _para: &Para, _line: &Line) -> Result<()> {
        Ok(())
    }

    /// `origin`: the column's (a nested row's own) left edge and the slice's top.
    fn row(
        &mut self,
        _origin: Point,
        _table: &Table,
        _row: usize,
        _slice: &RowSlice,
    ) -> Result<()> {
        Ok(())
    }

    /// Body paragraphs and row ends only.
    fn placed(&mut self, _placed: Placed) -> Result<()> {
        Ok(())
    }
}

impl Sink for Vec<Placed> {
    fn placed(&mut self, placed: Placed) -> Result<()> {
        self.push(placed);
        Ok(())
    }
}

/// Pieces, lines, rows and sink calls, headers and footers included,
/// before pagination gives up.
pub(super) const MAX_LAYOUT_WORK: u64 = 200_000_000;

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    pub(crate) static TEST_LAYOUT_WORK: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(super) fn within(work: u64, budget: u64) -> Result<()> {
    if work > budget {
        return Err(Error::work_bound("max_layout_work", budget, work));
    }
    Ok(())
}

pub(super) fn layout_work() -> u64 {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(budget) = TEST_LAYOUT_WORK.get() {
        return budget;
    }
    MAX_LAYOUT_WORK
}

struct Flow<'m, 's, S: Sink> {
    model: &'m Model,
    sink: &'s mut S,
    furniture: Furniture<'m>,
    section: usize,
    /// The physical page the current section started on.
    section_first_page: u32,
    page: u32,
    y: f64,
    top: f64,
    bottom: f64,
    /// Nothing placed on this page yet.
    at_top: bool,
    /// Nothing placed in the document yet.
    at_start: bool,
    /// Nothing placed in this section yet (its first page keeps space
    /// before).
    at_section_start: bool,
    previous: Option<&'m Para>,
    last: Option<(u32, f64)>,
    work: u64,
    budget: u64,
}

impl<'m, S: Sink> Flow<'m, '_, S> {
    fn charge(&mut self, work: u64) -> Result<()> {
        self.work = self.work.saturating_add(work);
        within(self.work, self.budget)
    }

    fn lines(&mut self, para: &Para) -> Result<Vec<Line>> {
        let lines = lines(
            para,
            self.metrics(),
            self.text_width(),
            self.model.default_tab,
        );
        self.charge((para.pieces.len() + lines.len()) as u64)?;
        Ok(lines)
    }

    fn row_layout(&mut self, row: &'m Row) -> Result<RowLayout<'m>> {
        let layout = row_layout(row, self.metrics(), self.model.default_tab);
        self.charge(layout.work)?;
        Ok(layout)
    }

    fn section(&self) -> usize {
        self.section.min(self.model.sections.len() - 1)
    }

    /// Start the page: its header and footer, then the body at its top.
    fn open_page(&mut self) -> Result<()> {
        self.sink.page(&PageStart { number: self.page })?;
        let at = PageAt {
            page: self.page,
            section: self.section(),
            first: self.page == self.section_first_page,
        };
        let Flow {
            furniture,
            sink,
            work,
            budget,
            ..
        } = self;
        let body = furniture.page(at, *sink, &mut |cost| {
            *work = work.saturating_add(cost);
            within(*work, *budget)
        })?;
        self.top = body.top;
        self.bottom = body.bottom;
        self.y = body.top;
        Ok(())
    }

    fn new_page(&mut self) -> Result<()> {
        self.page += 1;
        self.at_top = true;
        self.previous = None;
        self.open_page()
    }

    fn body_height(&self) -> f64 {
        self.bottom - self.top
    }

    fn emit(&mut self, placed: Placed) -> Result<()> {
        self.last = Some((placed.page, placed.y));
        self.sink.placed(placed)
    }

    /// Place `para` at its offset below the current position.
    fn place(&mut self, para: ParaOffset) -> Result<()> {
        self.emit(Placed {
            node: para.node,
            page: self.page,
            y: self.y + para.offset,
            row_end: para.row_end,
        })
    }

    fn metrics(&self) -> &'m [Metric] {
        &self.model.metrics
    }

    fn text_width(&self) -> f64 {
        self.model.sections[self.section()].page.text_width
    }

    fn column_left(&self) -> f64 {
        let page = &self.model.sections[self.section()].page;
        page.left + page.gutter
    }

    fn line(&mut self, at: Point, para: &Para, line: &Line) -> Result<()> {
        self.charge(1)?;
        self.sink.line(at, para, line)
    }

    fn row(&mut self, origin: Point, table: &Table, row: usize, slice: &RowSlice) -> Result<()> {
        self.charge(1)?;
        self.sink.row(origin, table, row, slice)
    }

    /// Draw a slice of `table`'s row `row` with its top at `y`, and the
    /// cell items `top` gives a top within the slice. Word cuts a header or
    /// can't-split row that runs off the paper: lines whose height less
    /// descent is past its edge are not drawn (measured at single spacing,
    /// where that is the baseline).
    fn draw_slice(
        &mut self,
        (table, row): (&'m Table, usize),
        layout: &RowLayout<'m>,
        y: f64,
        slice: RowSlice,
        top: impl Fn(usize, &CellPart<'m>, &Drawn<'m>) -> Option<f64>,
    ) -> Result<()> {
        let left = self.column_left();
        let paper = if table.rows[row].header || table.rows[row].cant_split {
            self.model.sections[self.section()].page.height
        } else {
            f64::INFINITY
        };
        let slice = RowSlice {
            height: slice.height.min(paper - y),
            ..slice
        };
        self.row(Point { x: left, y }, table, row, &slice)?;
        for (cell, part) in layout.cells.iter().enumerate() {
            for drawn in &part.layout.drawn {
                let Some(top) = top(cell, part, drawn) else {
                    continue;
                };
                let at = Point {
                    x: left + part.x + drawn.x,
                    y: y + top,
                };
                match &drawn.item {
                    Item::Line(para, line)
                        if at.y + line.height - line.below <= paper + EPSILON =>
                    {
                        self.line(at, para, line)?
                    }
                    Item::Row(table, row, height) if at.y < paper => {
                        self.row(at, table, *row, &RowSlice::whole(height.min(paper - at.y)))?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn draw_row(
        &mut self,
        table: &'m Table,
        row: usize,
        layout: &RowLayout<'m>,
        y: f64,
    ) -> Result<()> {
        let slice = RowSlice::whole(layout.height);
        self.draw_slice((table, row), layout, y, slice, |_, part, drawn| {
            Some(part.top + drawn.offset)
        })
    }

    fn repeat_headers(
        &mut self,
        table: &'m Table,
        headers: &[RowLayout<'m>],
        height: f64,
    ) -> Result<()> {
        // Word repeats no header taller than the page body.
        if height > self.body_height() + EPSILON {
            return Ok(());
        }
        let mut y = self.y;
        for (row, layout) in headers.iter().enumerate() {
            self.charge(layout.work)?;
            self.draw_row(table, row, layout, y)?;
            y += layout.height;
        }
        self.y += height;
        Ok(())
    }

    /// Start section `next` after the current paragraph.
    fn next_section(&mut self, next: usize) -> Result<()> {
        self.section = next.min(self.model.sections.len() - 1);
        if self.model.sections[self.section].continuous {
            return Ok(());
        }
        self.section_first_page = self.page + 1;
        self.new_page()?;
        self.at_section_start = true;
        Ok(())
    }

    /// The space above `para` here: none at the top of a page (but the
    /// first), else the gap after the previous paragraph.
    fn space_above(&self, para: &Para) -> f64 {
        if self.at_start {
            // Automatic spacing is HTML's: none at the top of the document.
            if para.before_auto { 0.0 } else { para.before }
        } else if self.at_section_start {
            para.before_spacing()
        } else if self.at_top {
            0.0
        } else {
            gap(self.previous, para)
        }
    }

    fn place_para(&mut self, para: &'m Para) -> Result<()> {
        let lines = self.lines(para)?;
        if para.page_break_before && !self.at_top {
            self.new_page()?;
        }
        self.y += self.space_above(para);
        let total: f64 = lines.iter().map(|l| l.height).sum::<f64>()
            - lines.last().map_or(0.0, |l| l.height - l.fit);
        if para.keep_lines
            && !self.at_top
            && self.y + total > self.bottom + EPSILON
            && total <= self.body_height()
            && !lines.iter().any(|l| l.page_break)
        {
            self.new_page()?;
        }
        let mut first = true;
        let mut index = 0;
        while index < lines.len() {
            let mut fit = 0;
            let mut y = self.y;
            for line in &lines[index..] {
                if y + line.fit > self.bottom + EPSILON {
                    break;
                }
                y += line.height;
                fit += 1;
                if line.page_break {
                    break;
                }
            }
            let remaining = lines.len() - index;
            let hard = lines[index..index + fit].iter().any(|l| l.page_break);
            if fit < remaining && !hard && para.widow_control && remaining >= 2 {
                if remaining - fit == 1 && fit >= 2 {
                    fit -= 1;
                }
                if fit == 1 && first {
                    fit = 0;
                }
            }
            if fit == 0 {
                if self.at_top {
                    fit = 1;
                } else {
                    self.new_page()?;
                    continue;
                }
            }
            if first {
                // Word reports an empty section-break paragraph where the
                // paragraph before it starts.
                let y = match (para.section_break, para.pieces.is_empty(), self.last) {
                    (Some(_), true, Some((page, y))) if page == self.page => y,
                    _ => self.y,
                };
                self.emit(Placed {
                    node: para.node,
                    page: self.page,
                    y,
                    row_end: false,
                })?;
                first = false;
            }
            let left = self.column_left();
            for line in &lines[index..index + fit] {
                self.line(Point { x: left, y: self.y }, para, line)?;
                self.y += line.height;
            }
            self.at_top = false;
            self.at_start = false;
            self.at_section_start = false;
            index += fit;
            if index < lines.len() || lines[index - 1].page_break {
                self.new_page()?;
            }
        }
        if lines.is_empty() {
            self.emit(Placed {
                node: para.node,
                page: self.page,
                y: self.y,
                row_end: false,
            })?;
        }
        if !self.at_top {
            self.previous = Some(para);
        }
        self.at_start = false;
        if let Some(next) = para.section_break {
            self.next_section(next)?;
        }
        Ok(())
    }

    fn place_table(&mut self, table: &'m Table) -> Result<()> {
        if !self.at_top
            && let Some(previous) = self.previous
        {
            self.y += previous.after_spacing();
        }
        let headers = table.rows.iter().take_while(|r| r.header).count();
        let header_layouts = table.rows[..headers]
            .iter()
            .map(|row| self.row_layout(row))
            .collect::<Result<Vec<_>>>()?;
        let header_height: f64 = header_layouts.iter().map(|layout| layout.height).sum();
        for (row_index, row) in table.rows.iter().enumerate() {
            let layout = self.row_layout(row)?;
            let fits = self.y + layout.height <= self.bottom + EPSILON;
            if !fits && !self.at_top {
                let splittable = !row.cant_split && !row.header && layout.height > 0.0;
                let repeat = row_index >= headers && headers > 0;
                let headers = if repeat {
                    Some((header_layouts.as_slice(), header_height))
                } else {
                    None
                };
                if splittable && self.split_row(table, row_index, &layout, headers)? {
                    continue;
                }
                self.new_page()?;
                if let Some((layouts, height)) = headers {
                    self.repeat_headers(table, layouts, height)?;
                }
            }
            for para in &layout.paras {
                self.place(*para)?;
            }
            self.emit(Placed {
                node: row.node,
                page: self.page,
                y: self.y + row.border_top + row.cells.first().map_or(0.0, |c| c.margins[0]),
                row_end: true,
            })?;
            self.draw_row(table, row_index, &layout, self.y)?;
            self.y += layout.height;
            self.at_top = false;
            self.at_start = false;
            self.at_section_start = false;
        }
        self.previous = None;
        Ok(())
    }

    /// Split a row across the page break between lines; false when no
    /// line of any cell fits on this page.
    fn split_row(
        &mut self,
        table: &'m Table,
        row_index: usize,
        layout: &RowLayout<'m>,
        headers: Option<(&[RowLayout<'m>], f64)>,
    ) -> Result<bool> {
        let row = &table.rows[row_index];
        let available = self.bottom - self.y;
        if available <= 0.0 {
            return Ok(false);
        }
        let cuts: Vec<Option<usize>> = layout
            .cells
            .iter()
            .map(|part| cut(part, available))
            .collect();
        if cuts.iter().all(|cut| *cut == Some(0)) {
            return Ok(false);
        }
        let cuts: Vec<Option<f64>> = cuts
            .iter()
            .zip(&layout.cells)
            .map(|(cut, part)| cut.map(|k| part.layout.lines[k].offset))
            .collect();
        let mut rest = 0.0f64;
        let mut moved: Vec<ParaOffset> = Vec::new();
        let mut kept: Vec<ParaOffset> = Vec::new();
        for ((cell, part), cut) in row.cells.iter().zip(&layout.cells).zip(&cuts) {
            let Some(cut) = *cut else {
                kept.extend(part.layout.paras.iter().map(|para| para.shifted(part.top)));
                continue;
            };
            rest = rest.max(part.layout.content - cut + cell.margins[0] + cell.margins[1]);
            for para in &part.layout.paras {
                if para.offset >= cut - EPSILON {
                    moved.push(ParaOffset {
                        offset: para.offset - cut + cell.margins[0],
                        ..*para
                    });
                } else {
                    kept.push(para.shifted(part.top));
                }
            }
        }
        let first = RowSlice {
            height: available,
            first: true,
            last: false,
        };
        self.draw_slice(
            (table, row_index),
            layout,
            self.y,
            first,
            |cell, part, drawn| {
                cuts[cell]
                    .is_none_or(|cut| drawn.offset < cut - EPSILON)
                    .then_some(part.top + drawn.offset)
            },
        )?;
        for para in kept {
            self.place(para)?;
        }
        self.emit(Placed {
            node: row.node,
            page: self.page,
            y: self.y,
            row_end: true,
        })?;
        self.new_page()?;
        if let Some((layouts, height)) = headers {
            self.repeat_headers(table, layouts, height)?;
        }
        let second = RowSlice {
            height: rest,
            first: false,
            last: true,
        };
        self.draw_slice(
            (table, row_index),
            layout,
            self.y,
            second,
            |cell, _, drawn| {
                let cut = cuts[cell]?;
                (drawn.offset >= cut - EPSILON)
                    .then(|| drawn.offset - cut + row.cells[cell].margins[0])
            },
        )?;
        for para in moved {
            self.place(para)?;
        }
        self.y += rest;
        self.at_top = false;
        Ok(true)
    }

    /// Before a keep-with-next chain: break the page when the chain and
    /// the first lines of what follows it do not fit.
    fn keep_chain(&mut self, blocks: &'m [Block], start: usize) -> Result<()> {
        let mut need = 0.0;
        let mut previous = self.previous;
        let mut at_top = self.at_top;
        let mut index = start;
        while let Some(Block::Para(para)) = blocks.get(index) {
            let lines = self.lines(para)?;
            let space = if at_top { 0.0 } else { gap(previous, para) };
            if !para.keep_next {
                let count = if para.widow_control {
                    2.min(lines.len())
                } else {
                    1
                };
                let taken = &lines[..count.min(lines.len())];
                // The last of them only needs its fitting height.
                need += space + taken.iter().map(|l| l.height).sum::<f64>()
                    - taken.last().map_or(0.0, |l| l.height - l.fit);
                break;
            }
            need += space + lines.iter().map(|l| l.height).sum::<f64>();
            previous = Some(para);
            at_top = false;
            index += 1;
        }
        if let Some(Block::Table(table)) = blocks.get(index)
            && let Some(row) = table.rows.first()
        {
            let after = previous.map_or(0.0, |p| p.after_spacing());
            need += after + self.row_layout(row)?.height;
        }
        if !self.at_top && self.y + need > self.bottom + EPSILON && need <= self.body_height() {
            self.new_page()?;
        }
        Ok(())
    }
}

/// Paginate the model into `sink`, headers and footers showing `values`.
pub(super) fn paginate<S: Sink>(model: &Model, values: &Values, sink: &mut S) -> Result<Pages> {
    paginate_within(model, values, sink, layout_work())
}

fn paginate_within<S: Sink>(
    model: &Model,
    values: &Values,
    sink: &mut S,
    budget: u64,
) -> Result<Pages> {
    let mut flow = Flow {
        model,
        sink,
        furniture: Furniture::new(model, values),
        section: 0,
        section_first_page: 1,
        page: 1,
        y: 0.0,
        top: 0.0,
        bottom: 0.0,
        at_top: true,
        at_start: true,
        at_section_start: true,
        previous: None,
        last: None,
        work: 0,
        budget,
    };
    flow.open_page()?;
    let mut chain_until = 0;
    for (index, block) in model.blocks.iter().enumerate() {
        match block {
            Block::Para(para) => {
                if para.keep_next && index >= chain_until {
                    flow.keep_chain(&model.blocks, index)?;
                    chain_until = index
                        + model.blocks[index..]
                            .iter()
                            .take_while(|b| matches!(b, Block::Para(p) if p.keep_next))
                            .count();
                }
                flow.place_para(para)?;
            }
            Block::Table(table) => flow.place_table(table)?,
        }
    }
    Ok(flow.furniture.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::fields::NumberFormat;
    use crate::layout::model::{Label, Page, Section, Slots};

    const METRICS: [Metric; 1] = [Metric {
        height: 10.0,
        ascent: 8.0,
    }];

    /// A paragraph of `words` words, each `width` points wide plus a
    /// 2-point space.
    fn para(words: usize, width: f64) -> Para {
        let mut pieces = Vec::new();
        for index in 0..words {
            if index > 0 {
                pieces.push(Piece::Char {
                    ch: ' ',
                    width: 2.0,
                    metric: 0,
                    look: 0,
                });
            }
            for _ in 0..4 {
                pieces.push(Piece::Char {
                    ch: 'x',
                    width: width / 4.0,
                    metric: 0,
                    look: 0,
                });
            }
        }
        Para {
            pieces,
            style: "Normal".into(),
            ..Para::default()
        }
    }

    fn model(blocks: Vec<Block>, height: f64) -> Model {
        Model {
            sections: vec![Section {
                page: Page {
                    width: 120.0,
                    height,
                    top: 10.0,
                    bottom: 10.0,
                    header: 0.0,
                    footer: 0.0,
                    left: 10.0,
                    right: 10.0,
                    gutter: 0.0,
                    text_width: 100.0,
                },
                headers: Slots::default(),
                footers: Slots::default(),
                title_page: false,
                page_start: None,
                number_format: NumberFormat::Arabic,
                continuous: false,
            }],
            blocks,
            stories: Vec::new(),
            even_and_odd: false,
            metrics: METRICS.to_vec(),
            looks: Vec::new(),
            links: Vec::new(),
            default_tab: 36.0,
            warnings: Vec::new(),
        }
    }

    fn run(model: &Model) -> (Vec<Placed>, u32) {
        let mut placed = Vec::new();
        let pages = paginate(model, &Values::default(), &mut placed).unwrap();
        (placed, pages.count)
    }

    fn text(text: &str, width: f64) -> Vec<Piece> {
        text.chars()
            .map(|ch| match ch {
                '\t' => Piece::Tab { metric: 0, look: 0 },
                _ => Piece::Char {
                    ch,
                    width,
                    metric: 0,
                    look: 0,
                },
            })
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_line_stacks_the_largest_ascent_on_the_largest_descent() {
        // A: ascent 8, descent 2; B: ascent 6, descent 4.
        let metrics = [
            Metric {
                height: 10.0,
                ascent: 8.0,
            },
            Metric {
                height: 10.0,
                ascent: 6.0,
            },
        ];
        let char = |metric| Piece::Char {
            ch: 'x',
            width: 1.0,
            metric,
            look: 0,
        };
        let mut p = Para {
            pieces: vec![char(0), char(1)],
            ..Para::default()
        };
        assert_eq!(lines(&p, &metrics, 100.0, 36.0)[0].height, 12.0);
        // Double spacing adds each font's own extra height above it: A
        // 8 + 10, B 6 + 10; the line only needs its single height to fit.
        p.line = LineSpacing::Multiple(2.0);
        let line = lines(&p, &metrics, 100.0, 36.0).remove(0);
        assert_eq!((line.height, line.fit), (22.0, 12.0));
        // A list label counts with its ascent only (plus the text's extra).
        p.pieces = vec![char(0)];
        p.line = LineSpacing::Multiple(1.0);
        p.label = Some(crate::layout::model::Label {
            width: 5.0,
            look: 0,
            space: 0.0,
            tab: false,
            metric: 1,
        });
        assert_eq!(lines(&p, &metrics, 100.0, 36.0)[0].height, 10.0);
    }

    #[test]
    fn justified_lines_let_their_spaces_shrink() {
        // Six 15-point words and five 2.2-point spaces: 101 points, one
        // over the line; the spaces may give up 0.2375 of their 11 points.
        let mut p = para(6, 15.0);
        for piece in &mut p.pieces {
            if let Piece::Char { ch: ' ', width, .. } = piece {
                *width = 2.2;
            }
        }
        assert_eq!(lines(&p, &METRICS, 100.0, 36.0).len(), 2);
        p.align = Align::Both;
        assert_eq!(lines(&p, &METRICS, 100.0, 36.0).len(), 1);
        assert_eq!(lines(&p, &METRICS, 98.0, 36.0).len(), 2);
    }

    #[test]
    fn the_extra_height_of_line_spacing_may_overflow_the_page() {
        // Page body 100 points: nine single 10-point lines leave 10, enough
        // for a double-spaced line's text though not its 20-point height.
        let mut blocks: Vec<Block> = (0..9).map(|_| Block::Para(para(1, 10.0))).collect();
        let mut double = para(1, 10.0);
        double.line = LineSpacing::Multiple(2.0);
        blocks.push(Block::Para(double));
        let (placed, pages) = run(&model(blocks, 120.0));
        assert_eq!((placed[9].page, pages), (1, 1));
    }

    #[test]
    fn lines_wrap_at_spaces_and_inside_overlong_words() {
        // 10 words of 18 points: 18 + 2 + 18 + 2 + ... five words per line.
        assert_eq!(lines(&para(10, 18.0), &METRICS, 100.0, 36.0).len(), 2);
        // One word of four 62.5-point characters breaks between them.
        assert_eq!(lines(&para(1, 250.0), &METRICS, 100.0, 36.0).len(), 4);
        assert_eq!(lines(&para(0, 0.0), &METRICS, 100.0, 36.0).len(), 1);
    }

    #[test]
    fn the_gap_is_the_larger_spacing_and_contextual_spacing_removes_it() {
        let mut a = para(1, 10.0);
        a.after = 12.0;
        let mut b = para(1, 10.0);
        b.before = 6.0;
        assert_eq!(gap(Some(&a), &b), 12.0);
        b.before = 20.0;
        assert_eq!(gap(Some(&a), &b), 20.0);
        a.contextual = true;
        b.contextual = true;
        assert_eq!(gap(Some(&a), &b), 0.0);
        b.style = "Other".into();
        assert_eq!(gap(Some(&a), &b), 20.0);
    }

    #[test]
    fn widow_control_keeps_two_lines_together() {
        // Page body 100 points: ten 10-point lines. A 9-line paragraph,
        // then a 3-line one: without widow control 1 line + 2 lines split.
        let first = para(9 * 5, 18.0);
        let mut second = para(3 * 5, 18.0);
        let blocks = vec![Block::Para(first.clone()), Block::Para(second.clone())];
        let (placed, _) = run(&model(blocks, 120.0));
        assert_eq!(placed[1].page, 1);
        second.widow_control = true;
        let blocks = vec![Block::Para(first), Block::Para(second)];
        let (placed, pages) = run(&model(blocks, 120.0));
        assert_eq!((placed[1].page, pages), (2, 2));
    }

    #[test]
    fn space_before_is_dropped_at_the_top_of_a_page_but_the_first() {
        let mut p = para(1, 10.0);
        p.before = 30.0;
        let blocks = (0..8).map(|_| Block::Para(p.clone())).collect();
        let (placed, _) = run(&model(blocks, 120.0));
        assert_eq!(placed[0].y, 40.0);
        let top_of_two = placed.iter().find(|p| p.page == 2).unwrap();
        assert_eq!(top_of_two.y, 10.0);
    }

    #[test]
    fn a_left_aligned_line_keeps_its_start_width_and_slack() {
        // Ten 18-point words, 2-point spaces, left indent 10, first line 5
        // more: 4 words fit each line (to 93 and to 88), then 2.
        let mut p = para(10, 18.0);
        p.left = 10.0;
        p.first = 5.0;
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        let geometry: Vec<_> = lines
            .iter()
            .map(|l| (l.pieces.clone(), l.start, l.width, l.slack, l.gaps, l.last))
            .collect();
        assert_eq!(
            geometry,
            [
                (0..20, 15.0, 78.0, 7.0, 3, false),
                (20..40, 10.0, 78.0, 12.0, 3, false),
                (40..49, 10.0, 38.0, 52.0, 1, true),
            ]
        );
        assert!(lines.iter().all(|l| l.baseline == 8.0 && l.tabs.is_empty()));
    }

    #[test]
    fn each_line_rule_places_its_baseline() {
        let mut p = para(2, 10.0);
        let baseline = |p: &Para| {
            let line = lines(p, &METRICS, 100.0, 36.0).remove(0);
            (line.height, line.baseline)
        };
        p.line = LineSpacing::Multiple(1.5);
        assert_eq!(baseline(&p), (15.0, 8.0));
        p.line = LineSpacing::Multiple(0.8);
        assert_eq!(baseline(&p), (8.0, 6.4));
        p.line = LineSpacing::AtLeast(20.0);
        assert_eq!(baseline(&p), (20.0, 18.0));
        p.line = LineSpacing::Exact(5.0);
        assert_eq!(baseline(&p), (5.0, 4.0));
    }

    #[test]
    fn a_multiple_line_takes_the_largest_ascent_of_its_fonts_label_and_objects() {
        // A: ascent 8, descent 2; B: ascent 10, descent 6; C: ascent 12.
        let metrics = [(10.0, 8.0), (16.0, 10.0), (14.0, 12.0)]
            .map(|(height, ascent)| Metric { height, ascent });
        let char = |metric| Piece::Char {
            ch: 'x',
            width: 1.0,
            metric,
            look: 0,
        };
        let line = |p: &Para| {
            let line = lines(p, &metrics, 100.0, 36.0).remove(0);
            (line.height, line.baseline, line.below)
        };
        let mut p = Para {
            pieces: vec![char(0), char(1)],
            line: LineSpacing::Multiple(1.5),
            ..Para::default()
        };
        // Mixed fonts: B's 10 + 8 extra above, B's descent 6.
        assert_eq!(line(&p), (24.0, 10.0, 6.0));
        // A label's ascent with A's extra: 12 + 5 above A's descent 2.
        p.pieces = vec![char(0)];
        p.label = Some(crate::layout::model::Label {
            width: 5.0,
            look: 0,
            space: 0.0,
            tab: false,
            metric: 2,
        });
        assert_eq!(line(&p), (19.0, 12.0, 2.0));
        // An object taller than the text sits on the baseline.
        p.label = None;
        p.pieces.push(Piece::Object {
            width: 5.0,
            height: 30.0,
        });
        assert_eq!(line(&p), (30.0, 28.0, 2.0));
    }

    #[test]
    fn a_justified_line_counts_its_gaps_and_may_overrun() {
        // Twelve 15-point words, 2.2-point spaces: six a line, 101 points
        // wide in 100, fitting only because the spaces may shrink.
        let mut p = para(12, 15.0);
        for piece in &mut p.pieces {
            if let Piece::Char { ch: ' ', width, .. } = piece {
                *width = 2.2;
            }
        }
        p.align = Align::Both;
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].pieces, 0..30);
        for (line, last) in lines.iter().zip([false, true]) {
            assert_eq!((line.gaps, line.last), (5, last));
            assert!(close(line.width, 101.0), "{}", line.width);
            assert!(close(line.slack, -1.0), "{}", line.slack);
        }
    }

    #[test]
    fn a_right_tab_moves_the_text_after_it_to_the_stop() {
        // "a b<tab>c d" in 5-point characters, a right tab at 90: "c d" is
        // 15 wide, so the tab moves from 15 to 75; one gap after the tab.
        let p = Para {
            pieces: text("a b\tc d", 5.0),
            tabs: vec![(90.0, TabKind::Right)],
            ..Para::default()
        };
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        let line = &lines[0];
        assert_eq!(lines.len(), 1);
        assert_eq!(
            line.tabs,
            [TabPlace {
                piece: 3,
                from: 15.0,
                to: 75.0
            }]
        );
        assert_eq!(
            (line.start, line.width, line.slack, line.gaps),
            (0.0, 90.0, 10.0, 1)
        );
    }

    #[test]
    fn a_decimal_tab_ends_the_text_before_the_point_at_the_stop() {
        // 5-point characters, a decimal tab at 60: "12" ends at 60, and a
        // number without a point ends there whole.
        for (number, to) in [("12.50", 50.0), ("1250", 40.0)] {
            let p = Para {
                pieces: text(&format!("a\t{number}"), 5.0),
                tabs: vec![(60.0, TabKind::Decimal)],
                ..Para::default()
            };
            let line = lines(&p, &METRICS, 100.0, 36.0).remove(0);
            assert_eq!(line.tabs[0].to, to, "{number}");
        }
    }

    #[test]
    fn a_list_item_starts_its_text_after_the_label() {
        // A 10-point label and a tab, hanging indent 18: the text starts at
        // the indent on the first line and every other.
        let p = Para {
            pieces: text("aaaa aaaa aaaa", 5.0),
            left: 18.0,
            first: -18.0,
            label: Some(Label {
                width: 10.0,
                look: 0,
                space: 0.0,
                tab: true,
                metric: 0,
            }),
            ..Para::default()
        };
        let lines = lines(&p, &METRICS, 70.0, 36.0);
        let geometry: Vec<_> = lines
            .iter()
            .map(|l| (l.pieces.clone(), l.start, l.width, l.slack))
            .collect();
        assert_eq!(
            geometry,
            [(0..10, 18.0, 45.0, 7.0), (10..14, 18.0, 20.0, 32.0)]
        );
    }

    #[test]
    fn a_line_break_ends_a_line_as_the_paragraph_end_does() {
        let mut p = para(2, 10.0);
        p.pieces.insert(4, Piece::LineBreak { metric: 0, look: 0 });
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        let ends: Vec<_> = lines.iter().map(|l| (l.pieces.clone(), l.last)).collect();
        assert_eq!(ends, [(0..5, true), (5..10, true)]);
    }

    struct Failing {
        fail_at: usize,
        lines: usize,
        pages: u32,
        placed: usize,
    }

    impl Sink for Failing {
        fn page(&mut self, _: &PageStart) -> Result<()> {
            self.pages += 1;
            Ok(())
        }

        fn line(&mut self, _: Point, _: &Para, _: &Line) -> Result<()> {
            self.lines += 1;
            if self.lines == self.fail_at {
                return Err(Error::invalid_argument("sink_failed", "the sink failed"));
            }
            Ok(())
        }

        fn placed(&mut self, _: Placed) -> Result<()> {
            self.placed += 1;
            Ok(())
        }
    }

    #[test]
    fn a_failing_sink_stops_pagination() {
        // Thirty one-line paragraphs on three pages; the sink fails at the
        // fifteenth line, on the second page.
        let blocks = (0..30).map(|_| Block::Para(para(1, 10.0))).collect();
        let model = model(blocks, 120.0);
        let mut sink = Failing {
            fail_at: 15,
            lines: 0,
            pages: 0,
            placed: 0,
        };
        let error = paginate(&model, &Values::default(), &mut sink).unwrap_err();
        assert_eq!(error.reason(), "sink_failed");
        assert_eq!((sink.lines, sink.placed, sink.pages), (15, 15, 2));
        assert_eq!(run(&model).1, 3);
    }

    #[test]
    fn layout_work_has_a_budget() {
        // Ten paragraphs of 49 pieces and two lines, each line drawn: 530.
        let blocks: Vec<Block> = (0..10).map(|_| Block::Para(para(10, 18.0))).collect();
        let model = model(blocks, 120.0);
        let error = paginate_within(&model, &Values::default(), &mut Vec::new(), 529).unwrap_err();
        assert_eq!(error.reason(), "max_layout_work");
        assert!(paginate_within(&model, &Values::default(), &mut Vec::new(), 530).is_ok());
    }

    fn header_table(header_lines: usize, rows: usize) -> Table {
        let cell = |lines: usize| super::super::model::Cell {
            width: 100.0,
            blocks: (0..lines).map(|_| Block::Para(para(1, 10.0))).collect(),
            ..Default::default()
        };
        let mut table = Table {
            rows: vec![Row {
                cells: vec![cell(header_lines)],
                header: true,
                ..Row::default()
            }],
            ..Table::default()
        };
        table.rows.extend((0..rows).map(|_| Row {
            cells: vec![cell(1)],
            ..Row::default()
        }));
        table
    }

    #[test]
    fn repeated_header_rows_count_as_work() {
        // A 50-point header row on a 100-point body, then 20 rows: three
        // later pages repeat the header, each its layout (26), 5 lines and
        // the row.
        let need = |table: Table| {
            let model = model(vec![Block::Table(table)], 120.0);
            (1..)
                .find(|budget| {
                    paginate_within(&model, &Values::default(), &mut Vec::new(), *budget).is_ok()
                })
                .unwrap()
        };
        let repeated = header_table(5, 20);
        assert_eq!(
            run(&model(vec![Block::Table(repeated.clone())], 120.0)).1,
            4
        );
        let mut plain = repeated.clone();
        plain.rows[0].header = false;
        assert!(need(repeated) - need(plain) >= 3 * (26 + 5 + 1));
    }

    #[test]
    fn a_row_after_a_header_past_the_page_bottom_is_not_split_there() {
        // A 200-point header ends past the 100-point body; the next row has
        // an empty cell, which has no line to cut before.
        #[derive(Default)]
        struct Slices(Vec<RowSlice>);
        impl Sink for Slices {
            fn row(&mut self, _: Point, _: &Table, _: usize, slice: &RowSlice) -> Result<()> {
                self.0.push(*slice);
                Ok(())
            }
        }
        let mut table = header_table(20, 0);
        table.rows.push(Row {
            cells: vec![
                super::super::model::Cell {
                    width: 50.0,
                    ..Default::default()
                },
                super::super::model::Cell {
                    width: 50.0,
                    blocks: vec![Block::Para(para(1, 10.0))],
                    ..Default::default()
                },
            ],
            ..Row::default()
        });
        let mut sink = Slices::default();
        let pages = paginate(
            &model(vec![Block::Table(table)], 120.0),
            &Values::default(),
            &mut sink,
        )
        .unwrap()
        .count;
        assert_eq!(pages, 2);
        assert!(
            sink.0
                .iter()
                .all(|slice| slice.height >= 0.0 && slice.first && slice.last)
        );
    }

    #[test]
    fn a_header_taller_than_the_page_is_not_repeated() {
        // 3,000 header lines over 3,000 rows: drawn once, not 3,000 times.
        let large = model(vec![Block::Table(header_table(3000, 3000))], 120.0);
        assert!(paginate_within(&large, &Values::default(), &mut Vec::new(), 100_000).is_ok());
    }

    #[test]
    fn centred_and_right_aligned_lines_start_where_their_text_does() {
        // Two 10-point words and a 2-point space in 100: 78 points spare.
        let mut p = para(2, 10.0);
        let starts: Vec<f64> = [Align::Left, Align::Center, Align::Right, Align::Both]
            .into_iter()
            .map(|align| {
                p.align = align;
                let line = lines(&p, &METRICS, 100.0, 36.0).remove(0);
                assert_eq!((line.width, line.slack), (22.0, 78.0));
                line.start
            })
            .collect();
        assert_eq!(starts, [0.0, 39.0, 78.0, 0.0]);
    }

    #[test]
    fn a_label_only_line_leaves_room_for_the_descent() {
        let p = Para {
            label: Some(Label {
                width: 5.0,
                look: 0,
                space: 0.0,
                tab: false,
                metric: 0,
            }),
            ..Para::default()
        };
        let line = lines(&p, &METRICS, 100.0, 36.0).remove(0);
        assert_eq!(line.baseline, line.height - 2.0);
    }

    #[test]
    fn a_left_tab_past_the_margin_ends_its_line_with_no_width() {
        // "aaaa" is 96 wide; the next default stop, 108, is past 100.
        let p = Para {
            pieces: text("aaaa\tb", 24.0),
            ..Para::default()
        };
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].tabs,
            [TabPlace {
                piece: 4,
                from: 96.0,
                to: 96.0
            }]
        );
        assert_eq!((lines[0].pieces.clone(), lines[0].width), (0..5, 96.0));
    }

    #[test]
    fn spaces_before_the_first_word_after_a_tab_do_not_shrink() {
        // A tab to 36, two 10-point spaces, then a 45-point word: 101 in
        // 100. Only spaces between words count as gaps, and only gaps may
        // shrink, so the word goes to the next line.
        let mut pieces = text("\t  ", 10.0);
        pieces.extend(text("aaaaaaaaa", 5.0));
        let p = Para {
            pieces,
            align: Align::Both,
            ..Para::default()
        };
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].pieces.clone(), lines[0].gaps), (0..3, 0));
    }

    #[test]
    fn lines_in_table_cells_are_drawn_inside_their_cells() {
        #[derive(Default)]
        struct Lines(Vec<(u32, Point)>, u32);
        impl Sink for Lines {
            fn page(&mut self, page: &PageStart) -> Result<()> {
                self.1 = page.number;
                Ok(())
            }
            fn line(&mut self, at: Point, _: &Para, line: &Line) -> Result<()> {
                self.0.push((
                    self.1,
                    Point {
                        x: at.x + line.start,
                        y: at.y,
                    },
                ));
                Ok(())
            }
        }
        let cell = |words| super::super::model::Cell {
            width: 50.0,
            margins: [1.0, 1.0, 5.0, 5.0],
            blocks: vec![Block::Para(para(words, 10.0))],
            ..Default::default()
        };
        // A one-line cell beside a 12-line one, on a 100-point body after
        // 9 lines: the row splits after one line of the long cell.
        let row = Row {
            cells: vec![cell(1), cell(36)],
            ..Row::default()
        };
        let mut blocks: Vec<Block> = (0..8).map(|_| Block::Para(para(1, 10.0))).collect();
        blocks.push(Block::Table(Table {
            rows: vec![row],
            ..Table::default()
        }));
        let mut sink = Lines::default();
        paginate(&model(blocks, 120.0), &Values::default(), &mut sink).unwrap();
        let cells = &sink.0[8..];
        assert_eq!(cells.len(), 13);
        assert_eq!((cells[0].0, cells[0].1), (1, Point { x: 15.0, y: 91.0 }));
        assert_eq!((cells[1].0, cells[1].1), (1, Point { x: 65.0, y: 91.0 }));
        assert_eq!((cells[2].0, cells[2].1), (2, Point { x: 65.0, y: 11.0 }));
    }
}
