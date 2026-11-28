//! Footnotes at the foot of each page, endnotes after the last block.
//!
//! A body line holding references needs its notes at the foot of its page,
//! below the separator when they are the page's first; when they don't fit,
//! the line moves on with them. The line's last note may instead start
//! here and go on under the continuation separator, when one of its lines
//! fits. The notes sit on the body's bottom edge (`w:pos` `pageBottom`).

use std::collections::{HashMap, HashSet};

use super::{CellLayout, EPSILON, Flow, Item, Line, Point, RowLayout, Sink, cell_layout};
use crate::Result;
use crate::layout::model::{Block, Model, NoteCounter, NoteId, NoteKind, Para, Piece, StoryId};

/// Lines `from..to` of a note.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Part {
    note: NoteId,
    from: usize,
    to: usize,
}

/// The footnote area of one page.
#[derive(Debug, Clone, Default)]
pub(super) struct Area {
    parts: Vec<Part>,
    /// The separator's and the parts'.
    height: f64,
    /// The separator's share of `height`.
    separator: f64,
    /// Opens with the rest of a note from the page before.
    continued: bool,
    /// The bottom of the line whose note goes on: nothing more fits.
    full_at: Option<f64>,
    carry: Vec<Part>,
}

/// A note laid out in the column, and whether it holds a table.
struct Laid<'m> {
    layout: CellLayout<'m>,
    table: bool,
}

impl Laid<'_> {
    fn len(&self) -> usize {
        self.layout.lines.len()
    }

    /// A note's first line starts at its top, space before included.
    fn start(&self, line: usize) -> f64 {
        if line == 0 { 0.0 } else { self.end(line) }
    }

    fn end(&self, line: usize) -> f64 {
        self.layout
            .lines
            .get(line)
            .map_or(self.layout.content, |l| l.offset)
    }

    fn height(&self, from: usize, to: usize) -> f64 {
        self.end(to) - self.start(from)
    }

    fn whole(&self) -> f64 {
        self.height(0, self.len())
    }

    /// How many lines from `from` fit in `room`.
    fn fitting(&self, from: usize, room: f64) -> usize {
        let start = self.start(from);
        self.layout.lines[from.min(self.len())..]
            .iter()
            .take_while(|l| l.offset + l.height - start <= room + EPSILON)
            .count()
    }
}

pub(super) struct Notes<'m> {
    any: bool,
    laid: HashMap<NoteId, Laid<'m>>,
    /// The separator and the continuation separator, laid out as their
    /// paragraphs.
    separators: [Option<CellLayout<'m>>; 2],
    placed: HashSet<NoteId>,
    unplaced: HashSet<NoteId>,
    counter: NoteCounter,
    /// The page body's bottom without notes.
    page_bottom: f64,
    pub(super) area: Area,
    /// Endnotes in reference order.
    endnotes: Vec<NoteId>,
    /// Laying out the endnotes: their paragraphs are not body paragraphs.
    pub(super) in_endnotes: bool,
    /// Below compatibility mode 15, the separator's paragraph spacing
    /// counts only above a note going on to the next page. The corpus's
    /// reference and compat-word2003/2007 showcases need this; its mode-15
    /// showcases (builtin-table-style-only and seven more) need the spacing.
    legacy: bool,
}

impl<'m> Notes<'m> {
    pub(super) fn new(model: &'m Model) -> Notes<'m> {
        let notes = &model.notes;
        Notes {
            any: !notes.footnotes.notes.is_empty() || !notes.endnotes.notes.is_empty(),
            laid: HashMap::new(),
            separators: [None, None],
            placed: HashSet::new(),
            unplaced: HashSet::new(),
            counter: NoteCounter::new(&model.sections[0].notes),
            page_bottom: 0.0,
            area: Area::default(),
            endnotes: Vec::new(),
            in_endnotes: false,
            legacy: model.page_break_keeps_space,
        }
    }

    /// The separator's height, its paragraph spacing included (Word for
    /// Mac 16.115, corpus and Word checks): the continuation separator's
    /// line alone, and below mode 15 the separator's while the page's notes
    /// fit whole.
    fn separator_height(&self, continued: bool, going_on: bool) -> f64 {
        let Some(layout) = self.separators[usize::from(continued)].as_ref() else {
            return 0.0;
        };
        if continued || (self.legacy && !going_on) {
            layout.lines.iter().map(|l| l.height).sum()
        } else {
            layout.content
        }
    }

    fn laid(&self, note: NoteId) -> &Laid<'m> {
        &self.laid[&note]
    }

    /// The body's bottom with this page's notes.
    fn body_bottom(&self) -> f64 {
        self.area
            .full_at
            .unwrap_or(self.page_bottom - self.area.height)
    }
}

impl Area {
    /// The note going on to the next page.
    pub(super) fn going_on(&self) -> Option<NoteId> {
        self.carry.first().map(|part| part.note)
    }

    fn holds(&self, note: NoteId) -> bool {
        self.parts.iter().chain(&self.carry).any(|p| p.note == note)
    }

    /// Take the notes of a line ending at `line_bottom`; false, with the
    /// area unchanged, when the line and its notes don't fit.
    fn admit(&mut self, notes: &Notes<'_>, refs: &[NoteId], line_bottom: f64) -> bool {
        if self.full_at.is_some() {
            return false;
        }
        let heights: Vec<f64> = refs.iter().map(|n| notes.laid(*n).whole()).collect();
        let whole: f64 = heights.iter().sum();
        let room = notes.page_bottom - line_bottom - self.height;
        let opening = self.opening(notes, refs, false);
        if whole <= room - opening + EPSILON {
            self.open(opening);
            self.take_whole(refs, whole);
            return true;
        }
        let (Some((last, earlier)), Some(last_height)) = (refs.split_last(), heights.last()) else {
            return false;
        };
        let opening = self.opening(notes, refs, true);
        let room = room - opening - (whole - last_height);
        let laid = notes.laid(*last);
        let fit = laid.fitting(0, room);
        if room <= 0.0 || fit == 0 {
            return false;
        }
        self.open(opening);
        self.take_whole(earlier, whole - last_height);
        self.split(*last, laid, 0, fit, line_bottom);
        true
    }

    /// Take the notes of a line placed although they don't fit: those that
    /// do, then a part of the next, the rest going on.
    fn force(&mut self, notes: &Notes<'_>, refs: &[NoteId], line_bottom: f64) {
        let opening = self.opening(notes, refs, true);
        let mut room = notes.page_bottom - line_bottom - self.height - opening;
        for &note in refs {
            let laid = notes.laid(note);
            if self.full_at.is_some() {
                self.carry.push(Part {
                    note,
                    from: 0,
                    to: laid.len(),
                });
            } else if laid.whole() <= room + EPSILON {
                self.take_whole(&[note], laid.whole());
                room -= laid.whole();
            } else {
                let fit = if room > 0.0 { laid.fitting(0, room) } else { 0 };
                self.split(note, laid, 0, fit, line_bottom);
            }
        }
        if !self.parts.is_empty() {
            self.open(opening);
        }
    }

    /// What the separator above the page's notes adds for `refs`: all of
    /// it above the first, its spacing once a note goes on.
    fn opening(&self, notes: &Notes<'_>, refs: &[NoteId], going_on: bool) -> f64 {
        if self.continued || refs.is_empty() {
            return 0.0;
        }
        (notes.separator_height(false, going_on) - self.separator).max(0.0)
    }

    fn open(&mut self, opening: f64) {
        self.separator += opening;
        self.height += opening;
    }

    fn take_whole(&mut self, refs: &[NoteId], height: f64) {
        self.parts.extend(refs.iter().map(|&note| Part {
            note,
            from: 0,
            to: usize::MAX,
        }));
        self.height += height;
    }

    /// Lines `from..from + fit` of `note` here, the rest on the next page.
    fn split(&mut self, note: NoteId, laid: &Laid<'_>, from: usize, fit: usize, line_bottom: f64) {
        let to = from + fit;
        let rest = to < laid.len();
        if fit > 0 || !rest {
            self.parts.push(Part { note, from, to });
            self.height += laid.height(from, to);
        }
        if rest {
            self.carry.push(Part {
                note,
                from: to,
                to: laid.len(),
            });
            self.full_at = Some(line_bottom);
        }
    }
}

/// Where a line at `top` ends for the notes below it. Without notes on
/// its page, multiple spacing's extra may run past the bottom margin;
/// above notes it may not, and a line citing footnotes keeps its
/// paragraph's space after too (Word for Mac 16.115).
fn note_bottom(area: &Area, refs: &[NoteId], para: &Para, line: &Line, top: f64) -> f64 {
    let cites = refs.iter().any(|n| n.kind == NoteKind::Footnote);
    if area.height == 0.0 && !cites {
        return top + line.fit;
    }
    citing_bottom(para, line, top, cites)
}

fn citing_bottom(para: &Para, line: &Line, top: f64, cites: bool) -> f64 {
    let ends = line.pieces.end as usize >= para.pieces.len();
    let after = if cites && ends {
        para.after_spacing()
    } else {
        0.0
    };
    top + line.height + after
}

/// The footnotes a line at `top` cites, each with the line's bottom above
/// the notes.
pub(super) fn citations(para: &Para, line: &Line, top: f64) -> Vec<(f64, NoteId)> {
    let bottom = citing_bottom(para, line, top, true);
    line_refs(para, line)
        .into_iter()
        .filter(|note| note.kind == NoteKind::Footnote)
        .map(|note| (bottom, note))
        .collect()
}

/// The notes a line refers to, footnotes and endnotes alike.
fn line_refs(para: &Para, line: &Line) -> Vec<NoteId> {
    if para.story != StoryId::Body {
        return Vec::new();
    }
    let pieces = para
        .pieces
        .get(line.pieces.start as usize..line.pieces.end as usize)
        .unwrap_or_default();
    pieces
        .iter()
        .filter_map(|piece| match piece {
            Piece::NoteRef { note, .. } => Some(*note),
            _ => None,
        })
        .collect()
}

/// The notes a row's lines refer to, those of nested rows included.
pub(super) fn row_refs(layout: &RowLayout<'_>, kept: impl Fn(usize, f64) -> bool) -> Vec<NoteId> {
    let mut refs = Vec::new();
    for (cell, part) in layout.cells.iter().enumerate() {
        for drawn in &part.layout.drawn {
            if let Item::Line(para, line) = &drawn.item
                && kept(cell, drawn.offset)
            {
                refs.extend(line_refs(para, line));
            }
        }
    }
    refs
}

impl<'m, S: Sink> Flow<'m, '_, S> {
    /// The footnotes among `refs` not placed yet, laid out.
    fn new_footnotes(&mut self, refs: &[NoteId], area: &Area) -> Result<Vec<NoteId>> {
        let mut out: Vec<NoteId> = Vec::new();
        for &note in refs {
            if note.kind != NoteKind::Footnote
                || self.notes.placed.contains(&note)
                || area.holds(note)
                || out.contains(&note)
                || self.model.notes.note(note).is_none()
            {
                continue;
            }
            self.lay_out(note)?;
            out.push(note);
        }
        Ok(out)
    }

    fn lay_out(&mut self, note: NoteId) -> Result<()> {
        if self.notes.laid.contains_key(&note) {
            return Ok(());
        }
        let blocks = self.model.notes.note(note).map_or(&[][..], |n| &n.blocks);
        let layout = self.stack(blocks)?;
        let table = blocks.iter().any(|b| matches!(b, Block::Table(_)));
        self.notes.laid.insert(note, Laid { layout, table });
        if self.notes.separators[0].is_none() {
            let story = &self.model.notes.footnotes;
            self.notes.separators = [
                Some(self.stack(&story.separator)?),
                Some(self.stack(&story.continuation)?),
            ];
        }
        Ok(())
    }

    fn stack(&mut self, blocks: &'m [Block]) -> Result<CellLayout<'m>> {
        let model = self.model;
        let layout = cell_layout(
            blocks,
            &model.metrics,
            self.text_width(),
            model.default_tab,
            false,
        );
        self.charge(layout.work)?;
        Ok(layout)
    }

    /// Whether a line ending at `line_bottom` fits with its notes, which
    /// `area` then takes.
    pub(super) fn admit(
        &mut self,
        area: &mut Area,
        refs: &[NoteId],
        line_bottom: f64,
    ) -> Result<bool> {
        let footnotes = self.new_footnotes(refs, area)?;
        Ok(area.admit(&self.notes, &footnotes, line_bottom))
    }

    /// Whether a row ending at `bottom` fits with its notes, and whether
    /// one of them would then go on to the next page.
    pub(super) fn row_admits(&mut self, refs: &[NoteId], bottom: f64) -> Result<(bool, bool)> {
        let mut area = self.notes.area.clone();
        let whole = self.admit(&mut area, refs, bottom)?;
        Ok((whole, area.going_on().is_some()))
    }

    /// `admit` for a paragraph's line.
    pub(super) fn admit_line(
        &mut self,
        area: &mut Area,
        para: &Para,
        line: &Line,
        line_bottom: f64,
    ) -> Result<bool> {
        let refs = self.refs(para, line);
        let bottom = note_bottom(area, &refs, para, line, line_bottom - line.fit);
        self.admit(area, &refs, bottom)
    }

    /// `reserve` for a paragraph's line placed at `top`.
    pub(super) fn reserve_line(&mut self, para: &Para, line: &Line, top: f64) -> Result<()> {
        let refs = self.refs(para, line);
        let bottom = note_bottom(&self.notes.area, &refs, para, line, top);
        self.reserve(&refs, bottom)
    }

    pub(super) fn refs(&self, para: &Para, line: &Line) -> Vec<NoteId> {
        if self.notes.any {
            line_refs(para, line)
        } else {
            Vec::new()
        }
    }

    /// Place the notes of a line placed with its bottom at `line_bottom`,
    /// and number them; before the line is drawn.
    pub(super) fn reserve(&mut self, refs: &[NoteId], line_bottom: f64) -> Result<()> {
        if refs.is_empty() {
            return Ok(());
        }
        let mut area = std::mem::take(&mut self.notes.area);
        let footnotes = self.new_footnotes(refs, &area)?;
        if !footnotes.is_empty() && !area.admit(&self.notes, &footnotes, line_bottom) {
            area.force(&self.notes, &footnotes, line_bottom);
        }
        self.notes.area = area;
        self.bottom = self.notes.body_bottom();
        self.charge(refs.len() as u64)?;
        for &note in refs {
            self.number(note)?;
        }
        self.report_split_tables()
    }

    fn number(&mut self, note: NoteId) -> Result<()> {
        let Some(shown) = self.model.notes.note(note) else {
            return Ok(());
        };
        if !self.notes.placed.insert(note) {
            return Ok(());
        }
        if note.kind == NoteKind::Endnote {
            self.notes.endnotes.push(note);
        }
        let mark = if shown.custom {
            String::new()
        } else {
            let number = self.notes.counter.take(note.kind);
            self.model.sections[self.section()]
                .notes
                .get(note.kind)
                .mark(number)
        };
        self.sink.note(note, &mark)
    }

    /// Word never splits a note holding a table: report one going on.
    fn report_split_tables(&mut self) -> Result<()> {
        let split: Vec<NoteId> = self
            .notes
            .area
            .carry
            .iter()
            .map(|part| part.note)
            .filter(|note| self.notes.laid(*note).table && !self.notes.unplaced.contains(note))
            .collect();
        for note in split {
            self.notes.unplaced.insert(note);
            self.sink.note_unplaced(note)?;
        }
        Ok(())
    }

    pub(super) fn section_notes(&mut self) {
        let section = &self.model.sections[self.section()];
        self.notes.counter.section(&section.notes);
    }

    /// A new page's area: the rest of a note from the page before, under
    /// the continuation separator. True when it fills the page.
    pub(super) fn open_notes(&mut self) -> Result<bool> {
        let section = &self.model.sections[self.section()];
        self.notes.counter.page(&section.notes);
        self.notes.page_bottom = self.bottom;
        let carry = std::mem::take(&mut self.notes.area.carry);
        self.notes.area = Area::default();
        if carry.is_empty() {
            return Ok(false);
        }
        let separator = self.notes.separator_height(true, true);
        let mut area = Area {
            continued: true,
            height: separator,
            separator,
            ..Area::default()
        };
        for part in carry {
            let laid = self.notes.laid(part.note);
            let room = self.notes.page_bottom - self.top - area.height;
            let rest = laid.height(part.from, part.to);
            if area.full_at.is_some() {
                area.carry.push(part);
            } else if rest <= room + EPSILON {
                area.parts.push(part);
                area.height += rest;
            } else {
                // Each page takes at least one line of a note going on.
                let fit = laid
                    .fitting(part.from, room)
                    .max(usize::from(area.parts.is_empty()));
                area.split(part.note, laid, part.from, fit, self.top);
            }
        }
        let full = area.full_at.is_some();
        self.notes.area = area;
        self.bottom = self.notes.body_bottom();
        self.report_split_tables()?;
        Ok(full)
    }

    /// Draw the page's notes on the body's bottom edge.
    pub(super) fn finish_notes(&mut self) -> Result<()> {
        let parts = std::mem::take(&mut self.notes.area.parts);
        if parts.is_empty() {
            return Ok(());
        }
        let laid = std::mem::take(&mut self.notes.laid);
        let slot = usize::from(self.notes.area.continued);
        let separator = self.notes.separators[slot].take();
        let drawn = self.draw_notes(&laid, separator.as_ref(), &parts);
        self.notes.laid = laid;
        self.notes.separators[slot] = separator;
        drawn
    }

    fn draw_notes(
        &mut self,
        laid: &HashMap<NoteId, Laid<'m>>,
        separator: Option<&CellLayout<'m>>,
        parts: &[Part],
    ) -> Result<()> {
        let left = self.column_left();
        // A line taller than the page runs past its bottom, not its top.
        let mut y = (self.notes.page_bottom - self.notes.area.height).max(self.top);
        if let Some(separator) = separator {
            self.draw_separator(separator, left, y)?;
            y += self.notes.area.separator;
        }
        for part in parts {
            let note = &laid[&part.note];
            self.draw_note_part(note, part, left, y)?;
            y += note.height(part.from, part.to);
        }
        Ok(())
    }

    fn draw_separator(&mut self, separator: &CellLayout<'m>, left: f64, y: f64) -> Result<()> {
        for drawn in &separator.drawn {
            if let Item::Line(para, line) = &drawn.item {
                let at = Point {
                    x: left + drawn.x,
                    y: y + drawn.offset,
                };
                self.line(at, para, line)?;
            }
        }
        Ok(())
    }

    fn draw_note_part(&mut self, note: &Laid<'m>, part: &Part, left: f64, y: f64) -> Result<()> {
        let (start, end) = (note.start(part.from), note.end(part.to));
        let last = part.to >= note.len();
        for drawn in &note.layout.drawn {
            let inside = (part.from == 0 || drawn.offset >= start - EPSILON)
                && (last || drawn.offset < end - EPSILON);
            if !inside {
                continue;
            }
            let at = Point {
                x: left + drawn.x,
                y: y + drawn.offset - start,
            };
            match &drawn.item {
                Item::Line(para, line) => self.line(at, para, line)?,
                Item::Row(table, row, slice) => self.row(at, table, *row, slice)?,
            }
        }
        Ok(())
    }

    /// The endnotes, after the last block: their separator, then each in
    /// reference order.
    pub(super) fn endnotes(&mut self) -> Result<()> {
        let order = std::mem::take(&mut self.notes.endnotes);
        if order.is_empty() {
            return Ok(());
        }
        let story = &self.model.notes.endnotes;
        self.notes.in_endnotes = true;
        self.place_blocks(&story.separator)?;
        for note in order {
            if let Some(note) = story.notes.get(note.index as usize) {
                self.place_blocks(&note.blocks)?;
            }
        }
        Ok(())
    }

    /// Pages for the notes still going on after the last block.
    pub(super) fn finish_document(&mut self) -> Result<()> {
        while !self.notes.area.carry.is_empty() {
            self.new_page()?;
        }
        self.close_page()
    }
}
