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

use super::model::{Block, LineSpacing, Metric, Model, Para, Piece, TabKind, Table};
use crate::xml::NodeId;

/// One laid-out line: its height and whether a page break follows it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Line {
    pub height: f64,
    /// The height that must fit above the bottom margin: Word lets the
    /// extra height of multiple line spacing overflow it.
    pub fit: f64,
    pub page_break: bool,
}

const EPSILON: f64 = 0.01;

/// How much of its width each space of a justified line may give up for
/// the line to fit (Word for Mac, measured between 0.235 and 0.240).
const JUSTIFIED_SHRINK: f64 = 0.2375;

fn breaks_after(ch: char) -> bool {
    matches!(ch, '-' | '\u{2010}' | '\u{2013}' | '\u{2014}' | '\u{00AD}')
}

fn single(metrics: &[Metric], metric: u32) -> f64 {
    metrics.get(metric as usize).map_or(12.0, |m| m.height)
}

/// The text extent of a line. Word for Mac stacks the largest ascent on
/// the largest descent of the fonts on the line, the line gap counting
/// above the text; multiple line spacing adds each font's share of extra
/// height above it (`extra`: the multiple less one).
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

    /// The height without multiple line spacing's extra.
    fn single_height(self) -> f64 {
        self.ascent.max(self.label.unwrap_or(0.0)) + self.below
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

/// Width of the pieces from `start` up to the next tab or break.
fn segment_width(pieces: &[Piece], start: usize) -> f64 {
    let mut width = 0.0;
    for piece in &pieces[start..] {
        match piece {
            Piece::Char { width: w, .. } => width += w,
            Piece::Object { width: w, .. } => width += w,
            _ => break,
        }
    }
    width
}

/// The lines of a paragraph so far and the one being filled.
struct Breaker<'a> {
    para: &'a Para,
    /// The multiple less one of multiple line spacing.
    extra: f64,
    /// The paragraph mark's single line height, and with the extra.
    mark_single: f64,
    mark: f64,
    out: Vec<Line>,
    x: f64,
    text: Extent,
    /// Width of the spaces between words on the line so far.
    spaces: f64,
    object: f64,
    has_content: bool,
    /// Last break opportunity on this line (piece index to restart from).
    last_break: Option<usize>,
    line_start: usize,
    first_line: bool,
}

impl Breaker<'_> {
    /// Close the line (`page_break` after it) and open the next one at
    /// piece `restart`.
    fn break_line(&mut self, page_break: bool, restart: usize) {
        let height = self.text.height(self.mark_single, self.extra);
        let (text, single) = if height > 0.0 {
            (height, self.text.single_height())
        } else {
            (self.mark, self.mark_single)
        };
        self.out.push(Line {
            height: line_height(self.para, text, self.object),
            fit: line_height(self.para, single, self.object),
            page_break,
        });
        self.first_line = false;
        self.x = self.para.left;
        self.text = Extent::default();
        self.spaces = 0.0;
        self.object = 0.0;
        self.has_content = false;
        self.last_break = None;
        self.line_start = restart;
    }
}

/// Break `para` into lines `width` points wide.
pub(super) fn lines(para: &Para, metrics: &[Metric], width: f64, default_tab: f64) -> Vec<Line> {
    let extra = match para.line {
        LineSpacing::Multiple(multiple) => multiple - 1.0,
        _ => 0.0,
    };
    let mark_single = single(metrics, para.mark);
    let mut b = Breaker {
        para,
        extra,
        mark_single,
        mark: mark_single * (1.0 + extra),
        out: Vec::new(),
        x: para.left + para.first,
        text: Extent::default(),
        spaces: 0.0,
        object: 0.0,
        has_content: false,
        last_break: None,
        line_start: 0,
        first_line: true,
    };
    let pieces = &para.pieces;
    let right_limit = width - para.right;
    if let Some(label) = &para.label {
        b.text.add_label(metrics, label.metric);
        b.x += label.width + label.space;
        if label.tab {
            b.x = tab_stop(para, b.x, true, default_tab).0;
        }
        b.has_content = true;
    }
    let mut index = 0;
    while index < pieces.len() {
        match &pieces[index] {
            Piece::Char {
                ch,
                width: w,
                metric,
            } => {
                let is_space = *ch == ' ' || *ch == '\u{3000}';
                let shrink = if para.justified {
                    JUSTIFIED_SHRINK * b.spaces
                } else {
                    0.0
                };
                if !is_space && b.x + w > right_limit + EPSILON + shrink && b.has_content {
                    let restart = match b.last_break {
                        Some(at) if at > b.line_start => at,
                        _ if index > b.line_start => index,
                        _ => index + 1,
                    };
                    b.break_line(false, restart);
                    index = restart;
                    continue;
                }
                b.x += w;
                b.text.add(metrics, *metric, extra);
                if is_space {
                    if b.has_content {
                        b.spaces += w;
                    }
                    b.last_break = Some(index + 1);
                } else {
                    b.has_content = true;
                    if breaks_after(*ch) {
                        b.last_break = Some(index + 1);
                    }
                }
            }
            Piece::Object { width: w, height } => {
                if b.x + w > right_limit + EPSILON && b.has_content {
                    b.break_line(false, index);
                }
                b.x += w;
                b.object = b.object.max(*height);
                b.has_content = true;
                b.last_break = Some(index + 1);
            }
            Piece::Tab { metric } => {
                let (stop, kind) = tab_stop(para, b.x, b.first_line, default_tab);
                let segment = segment_width(pieces, index + 1);
                let target = match kind {
                    TabKind::Right | TabKind::Decimal => (stop - segment).max(b.x),
                    TabKind::Center => (stop - segment / 2.0).max(b.x),
                    _ => stop,
                };
                b.text.add(metrics, *metric, extra);
                if target > right_limit + EPSILON && b.has_content && kind == TabKind::Left {
                    b.break_line(false, index + 1);
                } else {
                    b.x = target;
                    b.spaces = 0.0;
                    b.has_content = true;
                    b.last_break = Some(index + 1);
                }
            }
            Piece::LineBreak { metric } => {
                b.text.add(metrics, *metric, extra);
                b.break_line(false, index + 1);
            }
            Piece::PageBreak => b.break_line(true, index + 1),
        }
        index += 1;
    }
    // The last line carries the paragraph mark, unless the paragraph ends
    // with a page break (the mark then stays beside the break).
    if b.has_content || !matches!(pieces.last(), Some(Piece::PageBreak)) {
        let height = b.text.height(mark_single, extra);
        let (text, single) = if b.has_content && height > 0.0 {
            (height, b.text.single_height())
        } else {
            (height.max(b.mark), b.text.single_height().max(mark_single))
        };
        b.out.push(Line {
            height: line_height(para, text, b.object),
            fit: line_height(para, single, b.object),
            page_break: false,
        });
    }
    b.out
}

/// Total height of blocks stacked in `width` (headers, footers).
pub(super) fn stack_height(
    blocks: &[Block],
    metrics: &[Metric],
    width: f64,
    default_tab: f64,
) -> f64 {
    cell_layout(blocks, metrics, width, default_tab, false).content
}

impl Para {
    fn before_spacing(&self) -> f64 {
        if self.before_auto { 14.0 } else { self.before }
    }

    fn after_spacing(&self) -> f64 {
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

struct CellLayout {
    lines: Vec<CellLine>,
    /// Paragraphs (and nested row ends) with their offsets.
    paras: Vec<ParaOffset>,
    content: f64,
}

/// Lay out blocks as one column (a cell when `cell`: automatic spacing at
/// its top and bottom is dropped).
fn cell_layout(
    blocks: &[Block],
    metrics: &[Metric],
    width: f64,
    default_tab: f64,
    cell: bool,
) -> CellLayout {
    let mut out = CellLayout {
        lines: Vec::new(),
        paras: Vec::new(),
        content: 0.0,
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
                for (index, line) in para_lines.into_iter().enumerate() {
                    out.lines.push(CellLine {
                        offset: y,
                        height: line.height,
                        index,
                        count,
                        widow_control: para.widow_control,
                    });
                    y += line.height;
                }
                previous = Some(para);
            }
            Block::Table(table) => {
                if let Some(previous) = previous {
                    y += previous.after_spacing();
                }
                for row in &table.rows {
                    let layout = row_layout(row, metrics, default_tab);
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

/// A row's height, its cells' layouts and their paragraphs' offsets from
/// the row top.
struct RowLayout {
    height: f64,
    paras: Vec<ParaOffset>,
    cells: Vec<(CellLayout, f64)>,
}

fn row_layout(row: &super::model::Row, metrics: &[Metric], default_tab: f64) -> RowLayout {
    let mut height = 0.0f64;
    let mut paras = Vec::new();
    let mut cells = Vec::new();
    for cell in &row.cells {
        let width = (cell.width - cell.margins[2] - cell.margins[3]).max(1.0);
        let layout = cell_layout(&cell.blocks, metrics, width, default_tab, true);
        if cell.vmerge != 2 {
            height = height.max(layout.content + cell.margins[0] + cell.margins[1]);
        }
        let top = row.border_top + cell.margins[0];
        paras.extend(layout.paras.iter().map(|para| para.shifted(top)));
        cells.push((layout, top));
    }
    if let Some((value, exact)) = row.height {
        height = if exact { value } else { height.max(value) };
    }
    height += row.border_top + row.border_bottom;
    RowLayout {
        height,
        paras,
        cells,
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

struct Flow<'m> {
    model: &'m Model,
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
    out: Vec<Placed>,
}

impl<'m> Flow<'m> {
    fn bounds(model: &Model, section: usize, first: bool) -> (f64, f64) {
        let s = &model.sections[section.min(model.sections.len() - 1)];
        let p = &s.page;
        let (header, footer) = if first {
            (
                s.first_header_height.unwrap_or(s.header_height),
                s.first_footer_height.unwrap_or(s.footer_height),
            )
        } else {
            (s.header_height, s.footer_height)
        };
        let top = p.top.max(p.header + header);
        let bottom = p.height - p.bottom.max(p.footer + footer);
        (top, bottom.max(top + 36.0))
    }

    fn new_page(&mut self) {
        self.page += 1;
        let first = self.page == self.section_first_page;
        let (top, bottom) = Flow::bounds(self.model, self.section, first);
        self.top = top;
        self.bottom = bottom;
        self.y = top;
        self.at_top = true;
        self.previous = None;
    }

    fn body_height(&self) -> f64 {
        self.bottom - self.top
    }

    /// Place `para` at its offset below the current position.
    fn place(&mut self, para: ParaOffset) {
        self.out.push(Placed {
            node: para.node,
            page: self.page,
            y: self.y + para.offset,
            row_end: para.row_end,
        });
    }

    fn metrics(&self) -> &'m [Metric] {
        &self.model.metrics
    }

    fn text_width(&self) -> f64 {
        self.model.sections[self.section.min(self.model.sections.len() - 1)]
            .page
            .text_width
    }

    /// Start section `next` after the current paragraph.
    fn next_section(&mut self, next: usize) {
        self.section = next.min(self.model.sections.len() - 1);
        if self.model.sections[self.section].continuous {
            return;
        }
        self.section_first_page = self.page + 1;
        self.new_page();
        self.at_section_start = true;
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

    fn place_para(&mut self, para: &'m Para) {
        let lines = lines(
            para,
            self.metrics(),
            self.text_width(),
            self.model.default_tab,
        );
        if para.page_break_before && !self.at_top {
            self.new_page();
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
            self.new_page();
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
                    self.new_page();
                    continue;
                }
            }
            if first {
                // Word reports an empty section-break paragraph where the
                // paragraph before it starts.
                let y = match (para.section_break, para.pieces.is_empty(), self.out.last()) {
                    (Some(_), true, Some(last)) if last.page == self.page => last.y,
                    _ => self.y,
                };
                self.out.push(Placed {
                    node: para.node,
                    page: self.page,
                    y,
                    row_end: false,
                });
                first = false;
            }
            for line in &lines[index..index + fit] {
                self.y += line.height;
            }
            self.at_top = false;
            self.at_start = false;
            self.at_section_start = false;
            index += fit;
            if index < lines.len() || lines[index - 1].page_break {
                self.new_page();
            }
        }
        if lines.is_empty() {
            self.out.push(Placed {
                node: para.node,
                page: self.page,
                y: self.y,
                row_end: false,
            });
        }
        if !self.at_top {
            self.previous = Some(para);
        }
        self.at_start = false;
        if let Some(next) = para.section_break {
            self.next_section(next);
        }
    }

    fn place_table(&mut self, table: &'m Table) {
        if !self.at_top
            && let Some(previous) = self.previous
        {
            self.y += previous.after_spacing();
        }
        let metrics = self.metrics();
        let default_tab = self.model.default_tab;
        let headers = table.rows.iter().take_while(|r| r.header).count();
        let header_height: f64 = table.rows[..headers]
            .iter()
            .map(|row| row_layout(row, metrics, default_tab).height)
            .sum();
        for (row_index, row) in table.rows.iter().enumerate() {
            let layout = row_layout(row, metrics, default_tab);
            let fits = self.y + layout.height <= self.bottom + EPSILON;
            if !fits && !self.at_top {
                let splittable = !row.cant_split && !row.header && layout.height > 0.0;
                if splittable && self.split_row(row, &layout, header_height, headers, row_index) {
                    continue;
                }
                self.new_page();
                if row_index >= headers && headers > 0 {
                    self.y += header_height;
                }
            }
            for para in &layout.paras {
                self.place(*para);
            }
            self.out.push(Placed {
                node: row.node,
                page: self.page,
                y: self.y + row.border_top + row.cells.first().map_or(0.0, |c| c.margins[0]),
                row_end: true,
            });
            self.y += layout.height;
            self.at_top = false;
            self.at_start = false;
            self.at_section_start = false;
        }
        self.previous = None;
    }

    /// Split a row across the page break between lines; false when no
    /// line of any cell fits on this page.
    fn split_row(
        &mut self,
        row: &super::model::Row,
        layout: &RowLayout,
        header_height: f64,
        headers: usize,
        row_index: usize,
    ) -> bool {
        let available = self.bottom - self.y;
        let mut any = false;
        let mut rest = 0.0f64;
        let mut moved: Vec<ParaOffset> = Vec::new();
        let mut kept: Vec<ParaOffset> = Vec::new();
        for (cell, (cell_layout, top)) in row.cells.iter().zip(&layout.cells) {
            let lines = &cell_layout.lines;
            let mut cut = lines
                .iter()
                .position(|line| top + line.offset + line.height > available + EPSILON);
            // Widow control: no paragraph leaves one line behind or sends
            // one line ahead.
            if let Some(k) = cut {
                let line = &lines[k];
                if line.widow_control
                    && line.count >= 2
                    && (line.index == 1 || (line.index >= 2 && line.count - line.index == 1))
                {
                    cut = Some(k - 1);
                }
            }
            any |= cut != Some(0);
            let cut = cut.map(|k| lines[k].offset);
            match cut {
                Some(cut) => {
                    rest = rest.max(cell_layout.content - cut + cell.margins[0] + cell.margins[1]);
                    for para in &cell_layout.paras {
                        if para.offset >= cut - EPSILON {
                            moved.push(ParaOffset {
                                offset: para.offset - cut + cell.margins[0],
                                ..*para
                            });
                        } else {
                            kept.push(para.shifted(*top));
                        }
                    }
                }
                None => kept.extend(cell_layout.paras.iter().map(|para| para.shifted(*top))),
            }
        }
        if !any {
            return false;
        }
        for para in kept {
            self.place(para);
        }
        self.out.push(Placed {
            node: row.node,
            page: self.page,
            y: self.y,
            row_end: true,
        });
        self.new_page();
        if row_index >= headers && headers > 0 {
            self.y += header_height;
        }
        for para in moved {
            self.place(para);
        }
        self.y += rest;
        self.at_top = false;
        true
    }

    /// Before a keep-with-next chain: break the page when the chain and
    /// the first lines of what follows it do not fit.
    fn keep_chain(&mut self, blocks: &'m [Block], start: usize) {
        let mut need = 0.0;
        let mut previous = self.previous;
        let mut at_top = self.at_top;
        let mut index = start;
        while let Some(Block::Para(para)) = blocks.get(index) {
            let lines = lines(
                para,
                self.metrics(),
                self.text_width(),
                self.model.default_tab,
            );
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
            need += after + row_layout(row, self.metrics(), self.model.default_tab).height;
        }
        if !self.at_top && self.y + need > self.bottom + EPSILON && need <= self.body_height() {
            self.new_page();
        }
    }
}

/// Paginate the model; the placed paragraphs in document order (with
/// table cell paragraphs and row end marks, as Word counts paragraphs).
pub(super) fn paginate(model: &Model) -> (Vec<Placed>, u32) {
    let (top, bottom) = Flow::bounds(model, 0, true);
    let mut flow = Flow {
        model,
        section: 0,
        section_first_page: 1,
        page: 1,
        y: top,
        top,
        bottom,
        at_top: true,
        at_start: true,
        at_section_start: true,
        previous: None,
        out: Vec::new(),
    };
    let mut chain_until = 0;
    for (index, block) in model.blocks.iter().enumerate() {
        match block {
            Block::Para(para) => {
                if para.keep_next && index >= chain_until {
                    flow.keep_chain(&model.blocks, index);
                    chain_until = index
                        + model.blocks[index..]
                            .iter()
                            .take_while(|b| matches!(b, Block::Para(p) if p.keep_next))
                            .count();
                }
                flow.place_para(para);
            }
            Block::Table(table) => flow.place_table(table),
        }
    }
    let pages = flow.page;
    (flow.out, pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::model::{Page, Section};

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
                });
            }
            for _ in 0..4 {
                pieces.push(Piece::Char {
                    ch: 'x',
                    width: width / 4.0,
                    metric: 0,
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
                    height,
                    top: 10.0,
                    bottom: 10.0,
                    header: 0.0,
                    footer: 0.0,
                    text_width: 100.0,
                },
                header_height: 0.0,
                footer_height: 0.0,
                first_header_height: None,
                first_footer_height: None,
                page_start: None,
                continuous: false,
            }],
            blocks,
            metrics: METRICS.to_vec(),
            default_tab: 36.0,
        }
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
        };
        let mut p = Para {
            pieces: vec![char(0), char(1)],
            ..Para::default()
        };
        assert_eq!(lines(&p, &metrics, 100.0, 36.0)[0].height, 12.0);
        // Double spacing adds each font's own extra height above it: A
        // 8 + 10, B 6 + 10; the line only needs its single height to fit.
        p.line = LineSpacing::Multiple(2.0);
        let line = lines(&p, &metrics, 100.0, 36.0)[0];
        assert_eq!((line.height, line.fit), (22.0, 12.0));
        // A list label counts with its ascent only (plus the text's extra).
        p.pieces = vec![char(0)];
        p.line = LineSpacing::Multiple(1.0);
        p.label = Some(crate::layout::model::Label {
            width: 5.0,
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
        p.justified = true;
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
        let (placed, pages) = paginate(&model(blocks, 120.0));
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
        let (placed, _) = paginate(&model(blocks, 120.0));
        assert_eq!(placed[1].page, 1);
        second.widow_control = true;
        let blocks = vec![Block::Para(first), Block::Para(second)];
        let (placed, pages) = paginate(&model(blocks, 120.0));
        assert_eq!((placed[1].page, pages), (2, 2));
    }

    #[test]
    fn space_before_is_dropped_at_the_top_of_a_page_but_the_first() {
        let mut p = para(1, 10.0);
        p.before = 30.0;
        let blocks = (0..8).map(|_| Block::Para(p.clone())).collect();
        let (placed, _) = paginate(&model(blocks, 120.0));
        assert_eq!(placed[0].y, 40.0);
        let top_of_two = placed.iter().find(|p| p.page == 2).unwrap();
        assert_eq!(top_of_two.y, 10.0);
    }
}
