//! Rows across pages: each cell continues from its own cut, page after
//! page, and a nested table splits with the cell holding it.

use super::{
    CellPart, EPSILON, Flow, Headers, Item, ParaOffset, Placed, Point, RowLayout, Sink, notes,
};
use super::{RowSlice, Table};
use crate::Result;
use crate::layout::model::Cell;
use crate::layout::model::NoteId;

/// What is left of a cell's content: from `start`, the content offset at
/// the top of this page's part.
struct Rest<'a, 'm> {
    part: &'a CellPart<'m>,
    /// The cell's top margin: where its content starts on a later page.
    margin_top: f64,
    start: f64,
    /// The next of the cell's lines, drawn items and top-down paragraphs.
    line: usize,
    item: usize,
    para: usize,
    /// Into the cell's paragraphs, top down.
    paras: Vec<usize>,
    /// Nested rows begun above `start` that end below it.
    open: Vec<usize>,
    done: bool,
}

impl<'a, 'm> Rest<'a, 'm> {
    fn new(part: &'a CellPart<'m>, cell: &Cell) -> Rest<'a, 'm> {
        let offsets = &part.layout.paras;
        let mut paras: Vec<usize> = (0..offsets.len()).collect();
        paras.sort_by(|&a, &b| offsets[a].offset.total_cmp(&offsets[b].offset));
        Rest {
            part,
            margin_top: cell.margins[0],
            start: 0.0,
            line: 0,
            item: 0,
            para: 0,
            paras,
            open: Vec::new(),
            done: false,
        }
    }

    /// The first line that moves to the next page when this part's top is
    /// `top` below a slice of `available` points, widow control applied;
    /// none when the rest fits. Also the lines looked at.
    fn cut(&self, top: f64, available: f64) -> (Option<usize>, u64) {
        if self.done {
            return (None, 0);
        }
        let lines = &self.part.layout.lines;
        let shift = top - self.start;
        let Some(k) = (self.line..lines.len())
            .find(|&k| shift + lines[k].offset + lines[k].height > available + EPSILON)
        else {
            return (None, (lines.len() - self.line) as u64);
        };
        let at = lines[k]
            .widow_cut
            .filter(|cut| *cut >= self.start - EPSILON)
            .unwrap_or(lines[k].offset);
        let cut = self.line + lines[self.line..k].partition_point(|l| l.offset < at - EPSILON);
        (Some(cut), (k + 1 - self.line) as u64)
    }

    /// The content's top below the part's top.
    fn top(&self, first: bool) -> f64 {
        if first {
            self.part.top
        } else {
            self.margin_top
        }
    }

    fn places_a_line(&self, cut: Option<usize>) -> bool {
        let lines = &self.part.layout.lines;
        !self.done
            && cut.map_or(self.line < lines.len(), |k| {
                k > self.line && lines[k].cut > lines[self.line].cut + EPSILON
            })
    }

    /// The next line fits in `room` when this part's top is `top` below it.
    fn fits_in(&self, top: f64, room: f64) -> bool {
        self.part
            .layout
            .lines
            .get(self.line)
            .is_none_or(|line| top - self.start + line.offset + line.height <= room + EPSILON)
    }

    /// The first cut below the next line.
    fn forced(&self) -> Option<usize> {
        let lines = &self.part.layout.lines;
        let from = lines.get(self.line).filter(|_| !self.done)?.cut;
        (self.line + 1..lines.len()).find(|&k| lines[k].cut > from + EPSILON)
    }

    /// The bottom of the last line kept above `cut` when this part's top is
    /// `top`; none when no line is kept.
    fn kept_bottom(&self, top: f64, cut: Option<usize>) -> Option<f64> {
        let lines = &self.part.layout.lines;
        let end = cut.unwrap_or(lines.len());
        let line = lines.get(self.line..end)?.last().filter(|_| !self.done)?;
        Some(top - self.start + line.offset + line.height)
    }

    /// Whether a drawn item at `offset` is in this page's part up to `cut`.
    fn holds(&self, offset: f64, cut: Option<usize>) -> bool {
        !self.done && offset >= self.start - EPSILON && offset < self.end(cut) - EPSILON
    }

    fn end(&self, cut: Option<usize>) -> f64 {
        cut.map_or(self.part.layout.content, |k| self.part.layout.lines[k].cut)
    }

    /// The content left from `start`, the cell's margins added.
    fn height(&self, cell: &Cell) -> f64 {
        self.part.layout.content - self.start + cell.margins[0] + cell.margins[1]
    }

    /// Paragraphs above `end`, in document order.
    fn take_paras(&mut self, end: f64) -> Vec<ParaOffset> {
        let offsets = &self.part.layout.paras;
        let from = self.para;
        while self
            .paras
            .get(self.para)
            .is_some_and(|&i| offsets[i].offset < end - EPSILON)
        {
            self.para += 1;
        }
        let mut taken = self.paras[from..self.para].to_vec();
        taken.sort_unstable();
        taken.into_iter().map(|i| offsets[i]).collect()
    }
}

impl<'m, S: Sink> Flow<'m, '_, S> {
    /// Split a row between lines, page after page until all of it is
    /// placed; false, with nothing placed, when no line of it fits here
    /// or all of them do.
    pub(super) fn split_row(
        &mut self,
        table: &'m Table,
        row_index: usize,
        layout: &RowLayout<'m>,
        headers: Option<Headers<'_, 'm>>,
    ) -> Result<bool> {
        let row = &table.rows[row_index];
        let mut rests: Vec<Rest<'_, 'm>> = layout
            .cells
            .iter()
            .zip(&row.cells)
            .map(|(part, cell)| Rest::new(part, cell))
            .collect();
        self.charge(rests.iter().map(|rest| rest.paras.len() as u64).sum())?;
        let mut first = true;
        loop {
            let available = self.bottom - self.y;
            let mut cuts = self.cuts_with_notes(layout, &rests, first, available)?;
            if self.at_top {
                // Headers and footers leave each page its own room. A line
                // taller than any page's room is placed now; else, as a
                // paragraph's line, one is placed only when no cell can
                // place one, so each page places a line and the loop ends.
                let page = &self.model.sections[self.section()].page;
                let most = available + page.height - page.top - page.bottom - self.body_height();
                for (rest, cut) in rests.iter().zip(cuts.iter_mut()) {
                    if !rest.places_a_line(*cut) && !rest.fits_in(rest.top(first), most) {
                        *cut = rest.forced();
                    }
                }
                if !rests
                    .iter()
                    .zip(&cuts)
                    .any(|(rest, cut)| rest.places_a_line(*cut))
                {
                    cuts = rests.iter().map(Rest::forced).collect();
                }
            } else if !rests
                .iter()
                .zip(&cuts)
                .any(|(rest, cut)| rest.places_a_line(*cut))
            {
                return Ok(false);
            }
            let last = cuts.iter().all(Option::is_none);
            if first && last {
                // Only its padding is past the page bottom: Word moves the
                // row whole.
                return Ok(false);
            }
            let refs =
                notes::row_refs(layout, |cell, offset| rests[cell].holds(offset, cuts[cell]));
            let kept = slice_bottom(layout, &rests, &cuts, first);
            self.reserve(&refs, self.y + kept)?;
            let height = if last {
                rests
                    .iter()
                    .zip(&row.cells)
                    .filter(|(rest, _)| !rest.done)
                    .map(|(rest, cell)| rest.height(cell))
                    .fold(0.0, f64::max)
            } else {
                self.bottom - self.y
            };
            let slice = RowSlice {
                height,
                first,
                last,
                repeat: false,
                cells: rests
                    .iter()
                    .zip(&cuts)
                    .map(|(rest, cut)| {
                        let end = rest.end(*cut);
                        if rest.done { end..end } else { rest.start..end }
                    })
                    .collect(),
            };
            let left = self.table_left(table);
            self.row(Point { x: left, y: self.y }, table, row_index, &slice)?;
            for ((rest, cut), cell) in rests.iter_mut().zip(&cuts).zip(&row.cells) {
                if rest.done {
                    continue;
                }
                // Only the row's last part aligns its content vertically.
                let shift = if last && cell.vmerge == 0 {
                    cell.valign.share() * (height - rest.height(cell)).max(0.0)
                } else {
                    0.0
                };
                let top = rest.top(first) + shift;
                let end = rest.end(*cut);
                let origin = Point {
                    x: left + rest.part.x,
                    y: self.y + top - rest.start,
                };
                self.draw_rest(rest, origin, end)?;
                for para in rest.take_paras(end) {
                    self.place(ParaOffset {
                        offset: top + para.offset - rest.start,
                        ..para
                    })?;
                }
                match cut {
                    Some(k) => {
                        rest.start = end;
                        rest.line = *k;
                    }
                    None => rest.done = true,
                }
            }
            if first {
                self.emit(Placed {
                    node: row.node,
                    page: self.page,
                    y: self.y,
                    row_end: true,
                })?;
            }
            if last {
                self.y += height;
                break;
            }
            self.new_page()?;
            if let Some(headers) = headers {
                self.repeat_headers(table, headers)?;
            }
            first = false;
        }
        self.placed_something();
        Ok(true)
    }

    /// Each cell's cut for a slice of `available` points, moved up until
    /// the notes of what it keeps fit: each retry drops the deepest kept
    /// line, until none is kept. When a note goes on to the next page, the
    /// slice ends at the line citing it, as a paragraph's page does.
    fn cuts_with_notes(
        &mut self,
        layout: &RowLayout<'m>,
        rests: &[Rest<'_, 'm>],
        first: bool,
        mut available: f64,
    ) -> Result<Vec<Option<usize>>> {
        loop {
            let mut cuts = Vec::with_capacity(rests.len());
            for rest in rests {
                let (cut, looked) = rest.cut(rest.top(first), available);
                self.charge(looked)?;
                cuts.push(cut);
            }
            let refs =
                notes::row_refs(layout, |cell, offset| rests[cell].holds(offset, cuts[cell]));
            let Some(kept) = kept_bottom(rests, &cuts, first) else {
                return Ok(cuts);
            };
            if refs.is_empty() {
                return Ok(cuts);
            }
            let mut area = self.notes.area.clone();
            let bottom = slice_bottom(layout, rests, &cuts, first);
            if !self.admit(&mut area, &refs, self.y + bottom)? {
                available = kept - 2.0 * EPSILON;
                continue;
            }
            let citing = area.going_on().and_then(|going_on| {
                citations(layout, rests, &cuts, first)
                    .into_iter()
                    .filter(|(_, note)| *note == going_on)
                    .map(|(bottom, _)| bottom)
                    .reduce(f64::min)
            });
            match citing {
                Some(bottom) if bottom < kept - EPSILON => available = bottom,
                _ => return Ok(cuts),
            }
        }
    }

    /// Draw a cell's items from `rest.start` to `end`, the content offset 0
    /// at `origin`: nested rows begun above first, as parts.
    fn draw_rest(&mut self, rest: &mut Rest<'_, 'm>, origin: Point, end: f64) -> Result<()> {
        let drawn = &rest.part.layout.drawn;
        let open = std::mem::take(&mut rest.open);
        for index in open {
            self.draw_part(rest, index, origin, end)?;
        }
        while let Some(item) = drawn.get(rest.item).filter(|d| d.offset < end - EPSILON) {
            let at = Point {
                x: origin.x + item.x,
                y: origin.y + item.offset,
            };
            match &item.item {
                Item::Line(para, line) => self.line(at, para, line)?,
                Item::Row(..) => self.draw_part(rest, rest.item, origin, end)?,
            }
            rest.item += 1;
        }
        Ok(())
    }

    /// Draw the part of the nested row `drawn[index]` between `rest.start`
    /// and `end`; it stays open when it continues below `end`.
    fn draw_part(
        &mut self,
        rest: &mut Rest<'_, 'm>,
        index: usize,
        origin: Point,
        end: f64,
    ) -> Result<()> {
        let drawn = &rest.part.layout.drawn[index];
        let Item::Row(table, row, whole) = &drawn.item else {
            return Ok(());
        };
        let from = (rest.start - drawn.offset).max(0.0);
        let to = (end - drawn.offset).min(whole.height);
        let at = Point {
            x: origin.x + drawn.x,
            y: origin.y + drawn.offset + from,
        };
        self.row(at, table, *row, &whole.part(&table.rows[*row], from, to))?;
        if to < whole.height - EPSILON {
            rest.open.push(index);
        }
        Ok(())
    }
}

/// Where the slice ends above its notes: its deepest kept line, or a line
/// citing a footnote with its paragraph's space after.
fn slice_bottom(
    layout: &RowLayout<'_>,
    rests: &[Rest<'_, '_>],
    cuts: &[Option<usize>],
    first: bool,
) -> f64 {
    citations(layout, rests, cuts, first)
        .into_iter()
        .map(|(bottom, _)| bottom)
        .chain(kept_bottom(rests, cuts, first))
        .fold(0.0, f64::max)
}

/// The bottom of the deepest line the cells keep above their cuts, from
/// the slice's top; none when no cell keeps one.
fn kept_bottom(rests: &[Rest<'_, '_>], cuts: &[Option<usize>], first: bool) -> Option<f64> {
    rests
        .iter()
        .zip(cuts)
        .filter_map(|(rest, cut)| rest.kept_bottom(rest.top(first), *cut))
        .reduce(f64::max)
}

/// The footnotes the kept lines cite, each with its line's bottom above the
/// notes, from the slice's top.
fn citations(
    layout: &RowLayout<'_>,
    rests: &[Rest<'_, '_>],
    cuts: &[Option<usize>],
    first: bool,
) -> Vec<(f64, NoteId)> {
    let mut out = Vec::new();
    for ((rest, part), cut) in rests.iter().zip(&layout.cells).zip(cuts) {
        for drawn in &part.layout.drawn {
            if let Item::Line(para, line) = &drawn.item
                && rest.holds(drawn.offset, *cut)
            {
                let top = rest.top(first) - rest.start + drawn.offset;
                out.extend(notes::citations(para, line, top));
            }
        }
    }
    out
}
