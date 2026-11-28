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

mod notes;
mod split;

use std::ops::Range;

use super::fields::Values;
use super::furniture::{Furniture, PageAt, Pages};
use super::model::{
    Align, Block, Leader, LineSpacing, Metric, Model, NoteId, Para, Piece, Row, TabKind, TabStop,
    Table,
};
use crate::xml::NodeId;
use crate::{Error, Result};

/// A tab on a line: its piece, the x it moves from and to, and its stop's
/// leader. A left tab that would pass the right margin ends its line with
/// no width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TabPlace {
    pub piece: u32,
    pub from: f64,
    pub to: f64,
    pub leader: Leader,
}

/// A tab's leader as drawn: its character, in the tab's look, at each x
/// from the page's left edge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LeaderRun {
    pub piece: u32,
    pub ch: char,
    pub look: u32,
    pub origins: Vec<f64>,
}

/// Leader characters narrower than this are not drawn: it bounds the
/// glyphs one tab draws.
const MIN_LEADER_ADVANCE: f64 = 0.25;

/// One laid-out line. x values are points from the column's left edge.
///
/// Left, centred and right-aligned lines start where their text is drawn.
/// A justified line starts at the left, and the painter spreads `slack`
/// over its `gaps` when `justified`; a negative `slack` shrinks them, and
/// can be left on a justified paragraph's last line too.
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
    /// Its slack is spread over its gaps.
    pub justified: bool,
    /// Where the list label starts, on a paragraph's first line.
    pub label: Option<f64>,
}

impl Line {
    /// The x of each piece in `pieces`, and of each label character on
    /// the first line: the widths layout measured from `start`, each tab
    /// moving to its stop, and a justified line's slack shared among its
    /// gaps.
    pub(crate) fn origins(&self, para: &Para) -> (Vec<f64>, Vec<f64>) {
        let label = match (self.label, &para.label) {
            (Some(x), Some(label)) => advance(x, &label.pieces, |_, piece, x| x + width(piece)),
            _ => Vec::new(),
        };
        let first = self.pieces.start as usize;
        let pieces = para
            .pieces
            .get(first..self.pieces.end as usize)
            .unwrap_or_default();
        let after_tab = self
            .tabs
            .last()
            .map_or(0, |tab| tab.piece as usize + 1 - first);
        let gaps = gap_pieces(pieces, after_tab);
        let share = if self.justified && self.gaps > 0 {
            self.slack / f64::from(self.gaps)
        } else {
            0.0
        };
        let origins = advance(self.start, pieces, |at, piece, x| {
            let tab = self
                .tabs
                .iter()
                .find(|tab| tab.piece as usize == first + at);
            match (piece, tab) {
                (Piece::Tab { .. }, Some(tab)) => tab.to,
                _ if gaps[at] => x + width(piece) + share,
                _ => x + width(piece),
            }
        });
        (origins, label)
    }

    /// Each tab's leader, `left` the column's x on the page. Word for Mac
    /// sets leader characters at whole multiples of their advance from the
    /// page's left edge, each one that fits between the tab's ends
    /// (measured in 60 of its PDFs).
    pub(crate) fn leaders(&self, para: &Para, model: &Model, left: f64) -> Vec<LeaderRun> {
        self.tabs
            .iter()
            .filter_map(|tab| {
                let ch = tab.leader.char()?;
                let Some(Piece::Tab { look, .. }) = para.pieces.get(tab.piece as usize) else {
                    return None;
                };
                let appearance = model.looks.get(*look as usize)?;
                let face = &model.faces.get(appearance.face(ch) as usize)?.face;
                let advance = face.advance(ch) * appearance.size * appearance.scale;
                if advance < MIN_LEADER_ADVANCE {
                    return None;
                }
                let first = ((left + tab.from) / advance - 1e-6).ceil();
                let end = ((left + tab.to) / advance + 1e-6).floor();
                let origins = (first as i64..end as i64)
                    .map(|cell| cell as f64 * advance)
                    .collect();
                Some(LeaderRun {
                    piece: tab.piece,
                    ch,
                    look: *look,
                    origins,
                })
            })
            .collect()
    }
}

/// Each piece's x, from `start`; `next` gives the x after a piece.
fn advance(
    start: f64,
    pieces: &[Piece],
    mut next: impl FnMut(usize, &Piece, f64) -> f64,
) -> Vec<f64> {
    let mut x = start;
    pieces
        .iter()
        .enumerate()
        .map(|(at, piece)| {
            let origin = x;
            x = next(at, piece, x);
            origin
        })
        .collect()
}

fn width(piece: &Piece) -> f64 {
    match piece {
        Piece::Char { width, .. } | Piece::Object { width, .. } | Piece::NoteRef { width, .. } => {
            *width
        }
        _ => 0.0,
    }
}

fn is_space(ch: char) -> bool {
    ch == ' ' || ch == '\u{3000}'
}

/// The spaces from `from` on that a justified line widens: between two
/// words, as the breaker counts its gaps.
fn gap_pieces(pieces: &[Piece], from: usize) -> Vec<bool> {
    let word = |piece: &Piece| match piece {
        Piece::Char { ch, .. } => !is_space(*ch),
        Piece::Object { .. } => true,
        _ => false,
    };
    let first = pieces.iter().skip(from).position(word).map(|at| at + from);
    let last = pieces.iter().rposition(word);
    pieces
        .iter()
        .enumerate()
        .map(|(at, piece)| {
            matches!(piece, Piece::Char { ch, .. } if is_space(*ch))
                && first.is_some_and(|first| at > first)
                && last.is_some_and(|last| at < last)
        })
        .collect()
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

/// Next tab stop after `x` (points from the text-area left edge). Bar
/// tabs are not stops.
fn tab_stop(para: &Para, x: f64, first_line: bool, default_tab: f64) -> TabStop {
    let left = |pos| TabStop {
        pos,
        kind: TabKind::Left,
        leader: Leader::None,
    };
    let mut stops = para
        .tabs
        .iter()
        .filter(|stop| stop.kind != TabKind::Bar && stop.pos > x + EPSILON);
    if first_line && para.first < 0.0 && x + EPSILON < para.left {
        // The hanging indent is a tab stop on the first line.
        let custom = stops.find(|stop| stop.pos < para.left);
        return custom.copied().unwrap_or(left(para.left));
    }
    if let Some(stop) = stops.next() {
        return *stop;
    }
    let step = if default_tab > 1.0 { default_tab } else { 36.0 };
    left((((x + EPSILON) / step).floor() + 1.0) * step)
}

/// Width of `pieces` up to the next tab or break, or up to the decimal
/// point when `decimal` and there is one before them.
fn segment_width(pieces: &[Piece], decimal: bool) -> f64 {
    let mut width = 0.0;
    for piece in pieces {
        match piece {
            Piece::Char { ch: '.', .. } if decimal => break,
            Piece::Char { width: w, .. }
            | Piece::Object { width: w, .. }
            | Piece::NoteRef { width: w, .. } => width += w,
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

    fn tab(&mut self, piece: usize, to: f64, leader: Leader) {
        self.tabs.push(TabPlace {
            piece: piece as u32,
            from: self.x,
            to,
            leader,
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
        let first_line = self.first_line;
        let open = std::mem::replace(&mut self.line, Open::new(next_piece, para.left));
        let height = line_height(para, heights.text, open.object);
        let slack = self.right_limit - end.content;
        let spread = match ending {
            Ending::Wrap => true,
            Ending::LineBreak => para.spread_breaks,
            _ => false,
        };
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
            justified: para.align.justified() && (spread || slack < 0.0),
            label: para
                .label
                .as_ref()
                .filter(|_| first_line)
                .map(|label| para.left + para.first - label.lead() + offset),
        });
        self.first_line = false;
    }

    /// The piece to go on with.
    fn char(&mut self, index: usize, ch: char, width: f64, metric: u32) -> usize {
        let is_space = is_space(ch);
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
        let stop = tab_stop(self.para, x, self.first_line, self.default_tab);
        let kind = stop.kind;
        let pieces = &self.para.pieces[index + 1..];
        let target = match kind {
            // Word for Mac ends the text of a right tab past the margin at
            // the margin.
            TabKind::Right => {
                (stop.pos.min(self.right_limit) - segment_width(pieces, false)).max(x)
            }
            TabKind::Decimal => (stop.pos - segment_width(pieces, true)).max(x),
            TabKind::Center => (stop.pos - segment_width(pieces, false) / 2.0).max(x),
            _ => stop.pos,
        };
        self.line.text.add(self.metrics, metric, self.extra);
        if target > self.right_limit + EPSILON && self.line.has_content && kind == TabKind::Left {
            self.line.tabs.push(TabPlace {
                piece: index as u32,
                from: x,
                to: x,
                leader: Leader::None,
            });
            self.close(Ending::Wrap, index + 1, self.line.end());
        } else {
            self.line.tab(index, target, stop.leader);
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
        line.x += label.width - label.lead() + label.space;
        if label.tab {
            line.x = tab_stop(para, line.x, true, default_tab).pos;
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
            Piece::NoteRef { width, metric, .. } => b.char(index, '0', width, metric),
            Piece::Separator { metric, .. } => b.char(index, '0', 0.0, metric),
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

    pub(crate) fn after_spacing(&self) -> f64 {
        if self.after_auto { 14.0 } else { self.after }
    }
}

/// The gap between `previous` and `para`: Word takes the larger of the
/// previous paragraph's space after and this one's space before, after
/// contextual spacing removed either between paragraphs of one style.
fn gap(previous: Option<&Para>, para: &Para) -> f64 {
    previous.map_or(para.before_spacing(), |previous| {
        let (after, before) = spacing(previous, para);
        after.max(before)
    })
}

/// The space above a paragraph a page break moved to a new page (after a
/// next-page section break, or `pageBreakBefore` before Word 2013): Word
/// keeps what the previous paragraph's space after did not cover.
fn space_after_break(previous: Option<&Para>, para: &Para) -> f64 {
    previous.map_or(para.before_spacing(), |previous| {
        let (after, before) = spacing(previous, para);
        (before - after).max(0.0)
    })
}

/// The space after `previous` and before `para` where they meet.
fn spacing(previous: &Para, para: &Para) -> (f64, f64) {
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
    (after, before)
}

/// A place a row can split before: a line of a cell's content (a nested
/// table's included), or a nested row that cannot split. Top relative to
/// the content.
struct CellLine {
    offset: f64,
    height: f64,
    /// Where the content splits when this line is the first to move: its
    /// top, or a nested row's top for the row's first lines, so the row's
    /// top border and end move with them.
    cut: f64,
    /// Where the row splits instead when this is the first line that does
    /// not fit: the line before it, when widow control would otherwise
    /// leave one line of the paragraph behind or send one ahead.
    widow_cut: Option<f64>,
}

impl CellLine {
    fn shifted(&self, by: f64) -> CellLine {
        CellLine {
            offset: self.offset + by,
            height: self.height,
            cut: self.cut + by,
            widow_cut: self.widow_cut.map(|cut| cut + by),
        }
    }
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
    Row(&'m Table, usize, RowSlice),
}

/// An item, its top from the top of the content holding it and its x from
/// the content's left edge.
pub(super) struct Drawn<'m> {
    pub offset: f64,
    pub x: f64,
    pub item: Item<'m>,
}

pub(super) struct CellLayout<'m> {
    /// Top down.
    lines: Vec<CellLine>,
    /// Paragraphs (and nested row ends) with their offsets, in document
    /// order.
    paras: Vec<ParaOffset>,
    /// Top down.
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
                let mut above = y;
                for (index, line) in para_lines.into_iter().enumerate() {
                    let height = line.height;
                    let widow = para.widow_control
                        && count >= 2
                        && (index == 1 || (index >= 2 && count - index == 1));
                    out.lines.push(CellLine {
                        offset: y,
                        height,
                        cut: y,
                        widow_cut: widow.then_some(above),
                    });
                    above = y;
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
                    let lineless = layout.cells.iter().all(|p| p.layout.lines.is_empty());
                    if row.cant_split || row.header || lineless {
                        out.lines.push(CellLine {
                            offset: y,
                            height: layout.height,
                            cut: y,
                            widow_cut: None,
                        });
                    } else {
                        for part in &layout.cells {
                            let top = y + part.top;
                            let first = part.layout.lines.first().map_or(0.0, |l| l.offset);
                            out.lines
                                .extend(part.layout.lines.iter().map(|line| CellLine {
                                    cut: if line.offset <= first + EPSILON {
                                        y
                                    } else {
                                        line.cut + top
                                    },
                                    ..line.shifted(top)
                                }));
                        }
                    }
                    out.drawn.push(Drawn {
                        offset: y,
                        x: table.left,
                        item: Item::Row(table, row_index, RowSlice::whole(&layout)),
                    });
                    for part in layout.cells {
                        out.drawn
                            .extend(part.layout.drawn.into_iter().map(|drawn| Drawn {
                                offset: drawn.offset + part.top + y,
                                x: drawn.x + part.x + table.left,
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
    // A nested row's cells put their lines side by side.
    out.lines.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    out.drawn.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    out
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
    /// Into `Model.sections`.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    pub section: usize,
}

/// The part of a row on one page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowSlice {
    pub height: f64,
    /// The row's top is on this page, and its bottom.
    pub first: bool,
    pub last: bool,
    /// A header row drawn again on a page the table continues on.
    pub repeat: bool,
    /// Each cell's content on this page, points from the content's top:
    /// the lines whose top is in the range.
    pub cells: Vec<Range<f64>>,
}

impl RowSlice {
    /// Its height no more than `max`.
    fn cut_at(self, max: f64) -> RowSlice {
        RowSlice {
            height: self.height.min(max),
            ..self
        }
    }

    fn whole(layout: &RowLayout<'_>) -> RowSlice {
        RowSlice {
            height: layout.height,
            first: true,
            last: true,
            repeat: false,
            cells: layout
                .cells
                .iter()
                .map(|part| 0.0..part.layout.content)
                .collect(),
        }
    }

    /// The part of this whole row from `from` to `to`, points from its top.
    fn part(&self, row: &Row, from: f64, to: f64) -> RowSlice {
        let cells = self
            .cells
            .iter()
            .zip(&row.cells)
            .map(|(whole, cell)| {
                let top = row.border_top + cell.margins[0];
                let clamp = |at: f64| (at - top).clamp(whole.start, whole.end);
                clamp(from)..clamp(to)
            })
            .collect();
        RowSlice {
            height: to - from,
            first: from <= EPSILON,
            last: to >= self.height - EPSILON,
            repeat: self.repeat,
            cells,
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

    /// A note's mark (empty for a custom mark), once its reference is
    /// placed: before the line holding it.
    fn note(&mut self, _note: NoteId, _mark: &str) -> Result<()> {
        Ok(())
    }

    /// A note holding a table that goes on to the next page, which Word
    /// would not do; it is laid out all the same.
    fn note_unplaced(&mut self, _note: NoteId) -> Result<()> {
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

/// A table's header rows and their height.
type Headers<'a, 'm> = (&'a [RowLayout<'m>], f64);

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
    /// The paragraph that ended the previous section.
    section_end: Option<&'m Para>,
    previous: Option<&'m Para>,
    last: Option<(u32, f64)>,
    work: u64,
    budget: u64,
    notes: notes::Notes<'m>,
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
        self.sink.page(&PageStart {
            number: self.page,
            section: self.section(),
        })?;
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

    /// Close the page with its notes and open the next one with room for
    /// the body.
    fn new_page(&mut self) -> Result<()> {
        loop {
            self.finish_notes()?;
            self.turn_page()?;
            if !self.open_notes()? {
                return Ok(());
            }
        }
    }

    fn turn_page(&mut self) -> Result<()> {
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
        if self.notes.in_endnotes {
            return Ok(());
        }
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

    /// Draw `table`'s row `row` whole with its top at `y`. Word cuts a
    /// header or can't-split row that runs off the paper: lines whose top
    /// is past its edge are not drawn.
    fn draw_row(
        &mut self,
        (table, row): (&'m Table, usize),
        layout: &RowLayout<'m>,
        y: f64,
        repeat: bool,
    ) -> Result<()> {
        let left = self.column_left() + table.left;
        let paper = if table.rows[row].header || table.rows[row].cant_split {
            self.model.sections[self.section()].page.height
        } else {
            f64::INFINITY
        };
        let slice = RowSlice {
            repeat,
            ..RowSlice::whole(layout)
        };
        self.row(Point { x: left, y }, table, row, &slice.cut_at(paper - y))?;
        for part in &layout.cells {
            for drawn in &part.layout.drawn {
                let at = Point {
                    x: left + part.x + drawn.x,
                    y: y + part.top + drawn.offset,
                };
                match &drawn.item {
                    Item::Line(para, line) if at.y < paper => self.line(at, para, line)?,
                    Item::Row(table, row, slice) if at.y < paper => {
                        self.row(at, table, *row, &slice.clone().cut_at(paper - at.y))?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn repeat_headers(&mut self, table: &'m Table, headers: Headers<'_, 'm>) -> Result<()> {
        let (layouts, height) = headers;
        // Word repeats no header taller than the page body.
        if height > self.body_height() + EPSILON {
            return Ok(());
        }
        let mut y = self.y;
        for (row, layout) in layouts.iter().enumerate() {
            self.charge(layout.work)?;
            self.draw_row((table, row), layout, y, true)?;
            y += layout.height;
        }
        self.y += height;
        Ok(())
    }

    /// Start section `next` after the current paragraph.
    fn next_section(&mut self, next: usize) -> Result<()> {
        self.section = next.min(self.model.sections.len() - 1);
        self.section_notes();
        if self.model.sections[self.section].continuous {
            return Ok(());
        }
        self.section_first_page = self.page + 1;
        self.section_end = self.previous;
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
            space_after_break(self.section_end, para)
        } else if self.at_top {
            0.0
        } else {
            gap(self.previous, para)
        }
    }

    fn place_para(&mut self, para: &'m Para) -> Result<()> {
        let lines = self.lines(para)?;
        if para.page_break_before && !self.at_top {
            let previous = self.previous;
            self.new_page()?;
            if self.model.page_break_keeps_space {
                self.y += space_after_break(previous, para);
            }
        } else {
            self.y += self.space_above(para);
        }
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
            let mut area = self.notes.area.clone();
            for line in &lines[index..] {
                if !self.admit_line(&mut area, para, line, y + line.fit)? {
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
                let refs = self.refs(para, line);
                self.reserve(&refs, self.y + line.fit)?;
                self.line(Point { x: left, y: self.y }, para, line)?;
                self.y += line.height;
            }
            self.placed_something();
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
            let headers = (row_index >= headers && headers > 0)
                .then_some((header_layouts.as_slice(), header_height));
            let refs = notes::row_refs(&layout, |_, _| true);
            if !self.admit(&mut self.notes.area.clone(), &refs, self.y + layout.height)? {
                let splittable = !row.cant_split && !row.header && layout.height > 0.0;
                if splittable && self.split_row(table, row_index, &layout, headers)? {
                    continue;
                }
                if !self.at_top {
                    self.new_page()?;
                    if let Some(headers) = headers {
                        self.repeat_headers(table, headers)?;
                    }
                    if splittable
                        && !self.admit(
                            &mut self.notes.area.clone(),
                            &refs,
                            self.y + layout.height,
                        )?
                        && self.split_row(table, row_index, &layout, headers)?
                    {
                        continue;
                    }
                }
            }
            self.reserve(&refs, self.y + layout.height)?;
            for para in &layout.paras {
                self.place(*para)?;
            }
            self.emit(Placed {
                node: row.node,
                page: self.page,
                y: self.y + row.border_top + row.cells.first().map_or(0.0, |c| c.margins[0]),
                row_end: true,
            })?;
            self.draw_row((table, row_index), &layout, self.y, false)?;
            self.y += layout.height;
            self.placed_something();
        }
        self.previous = None;
        Ok(())
    }

    fn placed_something(&mut self) {
        self.at_top = false;
        self.at_start = false;
        self.at_section_start = false;
    }

    fn place_blocks(&mut self, blocks: &'m [Block]) -> Result<()> {
        let mut chain_until = 0;
        for (index, block) in blocks.iter().enumerate() {
            match block {
                Block::Para(para) => {
                    if para.keep_next && index >= chain_until {
                        self.keep_chain(blocks, index)?;
                        chain_until = index
                            + blocks[index..]
                                .iter()
                                .take_while(|b| matches!(b, Block::Para(p) if p.keep_next))
                                .count();
                    }
                    self.place_para(para)?;
                }
                Block::Table(table) => self.place_table(table)?,
            }
        }
        Ok(())
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
        section_end: None,
        previous: None,
        last: None,
        work: 0,
        budget,
        notes: notes::Notes::new(model),
    };
    flow.open_page()?;
    flow.open_notes()?;
    flow.place_blocks(&model.blocks)?;
    flow.endnotes()?;
    flow.finish_document()?;
    Ok(flow.furniture.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::fields::NumberFormat;
    use crate::layout::model::{Label, NoteSettings, Notes, Page, Section, Slots};

    fn spaced(style: &str, before: f64, after: f64, contextual: bool) -> Para {
        Para {
            style: style.into(),
            before,
            after,
            contextual,
            ..Para::default()
        }
    }

    #[test]
    fn a_page_break_keeps_the_space_before_less_the_previous_space_after() {
        let next = spaced("Heading", 24.0, 0.0, false);
        let after = |after| spaced("Body", 0.0, after, false);
        assert_eq!(space_after_break(Some(&after(5.0)), &next), 19.0);
        assert_eq!(space_after_break(Some(&after(30.0)), &next), 0.0);
        assert_eq!(space_after_break(None, &next), 24.0);
        let contextual = spaced("Spaced", 6.0, 18.0, true);
        assert_eq!(space_after_break(Some(&contextual), &contextual), 0.0);
    }

    fn stop(pos: f64, kind: TabKind) -> TabStop {
        TabStop {
            pos,
            kind,
            leader: Leader::None,
        }
    }

    const METRICS: [Metric; 1] = [Metric {
        height: 10.0,
        ascent: 8.0,
    }];

    #[test]
    fn a_justified_line_spreads_its_slack_over_its_gaps() {
        let mut p = para(12, 20.0);
        p.align = Align::Both;
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        assert!(lines.len() > 2);
        for line in &lines {
            let (origins, _) = line.origins(&p);
            let pieces = &p.pieces[line.pieces.start as usize..line.pieces.end as usize];
            let gaps = gap_pieces(pieces, 0).iter().filter(|gap| **gap).count();
            assert_eq!(gaps as u32, line.gaps);
            let last = pieces
                .iter()
                .rposition(|p| matches!(p, Piece::Char { ch: 'x', .. }));
            let end = origins[last.unwrap()] + 5.0;
            if line.last {
                assert_eq!(end, line.start + line.width);
            } else {
                assert!((end - 100.0).abs() < 1e-9, "{end}");
            }
        }
    }

    #[test]
    fn a_tab_moves_the_next_piece_to_its_stop() {
        let mut p = para(2, 20.0);
        p.pieces.insert(4, Piece::Tab { metric: 0, look: 0 });
        let line = &lines(&p, &METRICS, 200.0, 36.0)[0];
        let (origins, _) = line.origins(&p);
        assert_eq!((origins[4], origins[5]), (20.0, 36.0));
    }

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
                notes: NoteSettings::default(),
            }],
            blocks,
            stories: Vec::new(),
            even_and_odd: false,
            metrics: METRICS.to_vec(),
            looks: Vec::new(),
            faces: Vec::new(),
            links: Vec::new(),
            default_tab: 36.0,
            warnings: Vec::new(),
            page_break_keeps_space: true,
            notes: Notes::default(),
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
            pieces: Vec::new(),
            width: 5.0,
            space: 0.0,
            tab: false,
            metric: 1,
            align: Align::Left,
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
            pieces: Vec::new(),
            width: 5.0,
            space: 0.0,
            tab: false,
            metric: 2,
            align: Align::Left,
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
            tabs: vec![stop(90.0, TabKind::Right)],
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
                to: 75.0,
                leader: Leader::None
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
                tabs: vec![stop(60.0, TabKind::Decimal)],
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
                pieces: Vec::new(),
                width: 10.0,
                space: 0.0,
                tab: true,
                metric: 0,
                align: Align::Left,
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
                self.0.push(slice.clone());
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
                pieces: Vec::new(),
                width: 5.0,
                space: 0.0,
                tab: false,
                metric: 0,
                align: Align::Left,
            }),
            ..Para::default()
        };
        let line = lines(&p, &METRICS, 100.0, 36.0).remove(0);
        assert_eq!(line.baseline, line.height - 2.0);
    }

    #[test]
    fn a_right_tab_past_the_margin_ends_its_text_at_the_margin() {
        // A right stop at 150 on a 100-wide line: "bb" ends at 100, on the line.
        let p = Para {
            pieces: text("a\tbb", 5.0),
            tabs: vec![stop(150.0, TabKind::Right)],
            ..Para::default()
        };
        let lines = lines(&p, &METRICS, 100.0, 36.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].tabs[0].to, 90.0);
    }

    #[test]
    fn a_centred_label_is_centred_with_its_suffix_space() {
        // A 10-wide label and a 4-wide space centred on the indent at 10.
        let p = Para {
            pieces: text("a", 5.0),
            left: 20.0,
            first: -10.0,
            label: Some(Label {
                pieces: Vec::new(),
                width: 10.0,
                space: 4.0,
                tab: false,
                metric: 0,
                align: Align::Center,
            }),
            ..Para::default()
        };
        let line = lines(&p, &METRICS, 100.0, 36.0).remove(0);
        assert_eq!((line.label, line.start), (Some(3.0), 17.0));
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
                to: 96.0,
                leader: Leader::None
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
