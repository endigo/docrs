//! The painter: a layout sink that draws each line where layout placed it,
//! every glyph at the x layout measured, in the face that measured it.

use std::collections::HashMap;
use std::sync::Arc;

use super::chain::{Chain, Draw};
use super::decorate::{self, Span};
use super::font::{FaceRef, FontResources, PdfFont};
use super::picture::{Pictures, Spot, inline_rect};
use super::places::{Painted, Places};
use super::writer::{Ref, Writer, write_hex, write_name, write_number};
use super::{PdfLimits, check_page};
use crate::layout::flow::{Line, PageStart, ParaBox, Point, RowSlice, Sink};
use crate::layout::fonts::{Face, FontFile};
use crate::layout::mark_characters;
use crate::layout::model::Table;
use crate::layout::model::{self, Appearance, Model, NoteId, Para, Piece, Rgb, StoryId, TabKind};
use crate::layout::strokes::{Stroke, row_strokes};
use crate::txn::Txn;
use crate::{Error, Result};

/// Word for Mac's synthesized styles (measured): a bold face drawn from a
/// regular one is stroked size / 32.3 wide and scaled to 0.98 of its height
/// (layout adds its wider advance), and an italic one is sheared 87/256.
const SYNTHETIC_STROKE: f64 = 1.0 / 32.3;
const SYNTHETIC_HEIGHT: f64 = 0.98;
const SYNTHETIC_SHEAR: f64 = 87.0 / 256.0;

/// A page: its size and its content stream.
pub(super) struct Page {
    pub size: (f64, f64),
    pub contents: Ref,
}

pub(super) struct Painter<'m, 't, 'c, 'b> {
    model: &'m Model,
    txn: &'t Txn<'t>,
    chain: &'c mut Chain<'b>,
    pub writer: Writer,
    pub fonts: FontResources,
    /// The resource of each face drawn with, by address.
    faces: HashMap<usize, FaceRef>,
    /// Font programs read from the document's parts, by face file.
    programs: HashMap<usize, Arc<[u8]>>,
    pub pictures: Pictures,
    pub pages: Vec<Page>,
    pub places: Places,
    page: Option<PageStart>,
    /// Table cells' fills and border lines, the body's and the headers'
    /// and footers' apart: the fills under the text, the lines over it.
    cells: [Vec<u8>; 2],
    strokes: [Vec<Stroke>; 2],
    /// Paragraph boxes' shading, drawn under the body's text (and over its
    /// behind-text pictures).
    fills: Vec<u8>,
    content: Vec<u8>,
    /// Where the body's text starts in `content`, after the headers' and
    /// footers'.
    body_at: Option<usize>,
    /// The paragraph last drawn on the page and its first line's top.
    para_top: Option<ParaTop>,
    state: TextState,
    max_pages: u32,
    /// Content-stream bytes of the closed pages before compression, and
    /// their cap: a leader draws many glyphs for one tab, and a header
    /// anchors any number of pictures on every page.
    drawn: u64,
    max_drawn: u64,
    /// Draws, for tests, what no font program draws (a space for what it
    /// lacks), at the positions layout gives.
    stand_in: Option<Arc<Face>>,
    /// Each note's mark, as pagination numbers it.
    note_marks: HashMap<NoteId, String>,
}

/// The paragraph whose lines are being drawn on the page.
#[derive(Clone, Copy)]
struct ParaTop {
    story: StoryId,
    /// Its address, told apart from another paragraph's only together with
    /// the rest: a header or footer may be drawn from a per-page copy freed
    /// before the next one is made.
    para: usize,
    /// The end of its last line drawn: the next line must start there.
    end: u32,
    top: f64,
}

/// The text and graphics state a content stream carries between runs.
#[derive(Clone, Copy, PartialEq)]
struct TextState {
    fill: [u8; 3],
    scale: f64,
    stroke: Option<f64>,
}

const PAGE_START: TextState = TextState {
    fill: [0, 0, 0],
    scale: 1.0,
    stroke: None,
};

/// One glyph to draw: its code and the x layout gives its origin.
struct Glyph {
    code: u16,
    x: f64,
    /// Its advance in the embedded font, in thousandths of an em.
    advance: f64,
}

/// A character to draw in a look, `x` from the page's left edge.
#[derive(Clone, Copy)]
struct Mark {
    ch: char,
    look: u32,
    x: f64,
    /// In the paragraph's list label.
    label: bool,
}

/// Content streams compress well; uncompressed, they may be this many times
/// the output's byte cap.
const MAX_CONTENT_RATIO: u64 = 16;

/// Word for Mac strokes a bar tab 0.24 pt wide, its left edge at the stop.
const BAR_WIDTH: f64 = 0.24;

/// Word for Mac's note separator: a 2-inch rule 0.48 pt thick, its top
/// this share of its line's height down (corpus and long-note PDFs, within
/// 0.6 pt); the continuation separator spans the line.
const SEPARATOR_LENGTH: f64 = 144.0;
const SEPARATOR_THICKNESS: f64 = 0.48;
const SEPARATOR_DROP: f64 = 0.61;

/// Glyphs of one font and look, drawn with one `TJ`.
struct Run<'a> {
    font: PdfFont,
    look: &'a Appearance,
    size: f64,
    synthetic: (bool, bool),
    glyphs: Vec<Glyph>,
}

impl<'m, 't, 'c, 'b> Painter<'m, 't, 'c, 'b> {
    pub(super) fn new(
        model: &'m Model,
        txn: &'t Txn<'t>,
        chain: &'c mut Chain<'b>,
        limits: &PdfLimits,
        places: Places,
        stand_in: Option<Arc<Face>>,
    ) -> Result<Self> {
        Ok(Painter {
            model,
            txn,
            chain,
            writer: Writer::new(limits.max_output_bytes)?,
            fonts: FontResources::default(),
            faces: HashMap::new(),
            programs: HashMap::new(),
            pictures: Pictures::new(*limits),
            pages: Vec::new(),
            places,
            page: None,
            cells: Default::default(),
            fills: Vec::new(),
            content: Vec::new(),
            body_at: None,
            para_top: None,
            strokes: Default::default(),
            state: PAGE_START,
            max_pages: limits.max_pages,
            drawn: 0,
            max_drawn: limits.max_output_bytes.saturating_mul(MAX_CONTENT_RATIO),
            stand_in,
            note_marks: HashMap::new(),
        })
    }

    /// Writes the open page's content stream.
    pub(super) fn close_page(&mut self) -> Result<()> {
        let Some(page) = self.page.take() else {
            return Ok(());
        };
        let geometry = self.geometry(&page);
        let size = (geometry.width, geometry.height);
        let content = std::mem::take(&mut self.content);
        let (furniture, body) = content.split_at(self.body_at.take().unwrap_or(content.len()));
        let [body_cells, furniture_cells] = std::mem::take(&mut self.cells);
        let [body_strokes, furniture_strokes] = std::mem::take(&mut self.strokes);
        let mut under = furniture_cells;
        under.extend_from_slice(furniture);
        let mut text = body_cells;
        text.append(&mut self.fills);
        text.extend_from_slice(body);
        let taken = self.drawn + self.pictures.layered() as u64;
        let cap = |other: &[u8]| super::table::Cap {
            max: self.max_drawn,
            taken: taken + other.len() as u64,
        };
        super::table::borders(&mut under, furniture_strokes, size.1, cap(&text))?;
        super::table::borders(&mut text, body_strokes, size.1, cap(&under))?;
        let stream = self.pictures.page_stream(&under, &text);
        self.drawn += stream.len() as u64;
        self.check_drawn()?;
        let contents = super::page_content(&mut self.writer, &stream)?;
        self.pages.push(Page { size, contents });
        Ok(())
    }

    fn geometry(&self, page: &PageStart) -> &'m model::Page {
        &self.model.sections[page.section].page
    }

    /// The line's inline objects at their origins, `left` from the page's
    /// left edge.
    fn objects(
        &mut self,
        pieces: &[Piece],
        origins: &[f64],
        left: f64,
        baseline: f64,
    ) -> Result<()> {
        let height = self.page_height()?;
        for (piece, x) in pieces.iter().zip(origins) {
            if let Piece::Object { object, .. } = piece {
                let object = &self.model.objects[*object as usize];
                let rect = inline_rect(object, left + x, baseline);
                self.pictures.draw(
                    &mut self.writer,
                    self.txn,
                    object,
                    rect,
                    height,
                    &mut self.content,
                )?;
            }
        }
        Ok(())
    }

    /// The paragraph's anchors whose piece is on `line`.
    fn anchors(&mut self, at: Point, para: &Para, line: &Line, origins: &[f64]) -> Result<()> {
        let Some(page) = self.page else {
            return Err(no_page());
        };
        let (start, end) = (line.pieces.start, line.pieces.end);
        // A paragraph's first line starts it again; a later one continues
        // the paragraph only straight after its line before.
        let key = (para.story, para as *const Para as usize);
        let para_top = match self.para_top {
            Some(top) if start > 0 && (top.story, top.para) == key && top.end == start => top.top,
            _ => at.y,
        };
        self.para_top = Some(ParaTop {
            story: para.story,
            para: key.1,
            end,
            top: para_top,
        });
        let last = end as usize >= para.pieces.len();
        for anchor in &para.anchors {
            if anchor.piece < start || (anchor.piece >= end && !last) {
                continue;
            }
            let character = origins
                .get((anchor.piece - start) as usize)
                .map_or(line.start + line.width, |x| *x);
            let spot = Spot {
                page: self.geometry(&page),
                odd: page.odd,
                para_top,
                line_top: at.y,
                line_height: line.height,
                character: at.x + character,
            };
            let object = &self.model.objects[anchor.object as usize];
            let furniture = is_furniture(para.story);
            self.pictures
                .anchored(&mut self.writer, self.txn, object, anchor, &spot, furniture)?;
        }
        Ok(())
    }

    fn page_height(&self) -> Result<f64> {
        let page = self.page.as_ref().ok_or_else(no_page)?;
        Ok(self.geometry(page).height)
    }

    /// The face whose metrics decorate `model.faces[index]`.
    fn drawing(&self, index: u32) -> &Arc<Face> {
        let face = &self.model.faces[index as usize].face;
        match &self.stand_in {
            Some(stand_in) if face.file().is_none() => stand_in,
            _ => face,
        }
    }

    fn face(&mut self, face: &Arc<Face>) -> Result<FaceRef> {
        let key = Arc::as_ptr(face) as usize;
        if let Some(known) = self.faces.get(&key) {
            return Ok(*known);
        }
        let file = face.file().ok_or_else(|| {
            Error::internal(
                "pdf_font_unchecked",
                "a face without a program reached the painter",
            )
        })?;
        let data = self.program(file, &face.family)?;
        let resource = self.fonts.add_face(data, file.index(), face.instance())?;
        self.faces.insert(key, resource);
        Ok(resource)
    }

    /// The font, code and advance (in thousandths of an em) that draw `ch`
    /// in `look`, and whether its bold and italic are synthesized.
    fn glyph(
        &mut self,
        look: &Appearance,
        ch: char,
        label: bool,
    ) -> Result<(PdfFont, u16, f64, (bool, bool))> {
        let drawn = self
            .chain
            .drawn(self.model, look, ch, label)
            .ok_or_else(|| {
                Error::internal(
                    "pdf_font_unchecked",
                    "an undrawable character reached the painter",
                )
            })?;
        let (face, shown) = match drawn.draw {
            Draw::Face(face, shown) => (face, shown),
            Draw::StandIn => {
                let stand_in = self.stand_in.clone().ok_or_else(|| {
                    Error::internal("pdf_font_unchecked", "no stand-in to draw with")
                })?;
                let shown = look.caps.apply(ch, look.size).0;
                let shown = if stand_in.glyph(shown).is_some() {
                    shown
                } else {
                    ' '
                };
                (stand_in, shown)
            }
            Draw::Shape(shape) => {
                let (font, code, advance) = self.fonts.use_shape(shape, &drawn.text);
                return Ok((font, u16::from(code), advance, (false, false)));
            }
            Draw::Named(code) => {
                let run = &self.model.faces[look.face(ch) as usize];
                let program = run.face.file().and_then(|file| {
                    let data = self.program(file, &run.face.family).ok()?;
                    Some((data, file.index()))
                });
                let program = program.as_ref().map(|(data, index)| (&data[..], *index));
                let at = self
                    .fonts
                    .named(&run.family, run.bold, run.italic, &run.face, program);
                let advance = self.fonts.named_width(at, code);
                return Ok((PdfFont::Named(at), u16::from(code), advance, (false, false)));
            }
        };
        let font = self.face(&face)?;
        let glyph = face.glyph(shown).unwrap_or(0);
        let cid = self.fonts.use_glyph(font, glyph, &drawn.text)?;
        let synthetic = (look.bold && !face.bold, look.italic && !face.italic);
        Ok((
            PdfFont::Embedded(font),
            cid,
            face.advance(shown) * 1000.0,
            synthetic,
        ))
    }

    /// The bytes of `file`; a font the document embeds is read once.
    fn program(&mut self, file: &FontFile, family: &str) -> Result<Arc<[u8]>> {
        if let Some(bytes) = file.bytes() {
            return Ok(bytes.clone());
        }
        let key = file as *const FontFile as usize;
        if let Some(data) = self.programs.get(&key) {
            return Ok(data.clone());
        }
        let data: Arc<[u8]> = file
            .read(self.txn)
            .ok_or_else(|| {
                Error::malformed(
                    "pdf_embedded_font_unreadable",
                    format!("the embedded font of {family} cannot be read"),
                )
            })?
            .into();
        self.programs.insert(key, data.clone());
        Ok(data)
    }

    /// The glyphs of `marks` grouped into runs; `None` ends a run.
    fn runs(&mut self, marks: &[Option<Mark>]) -> Result<Vec<Run<'m>>> {
        let model = self.model;
        let mut runs: Vec<Run<'m>> = Vec::new();
        let mut open = false;
        for mark in marks {
            let Some(Mark { ch, look, x, label }) = *mark else {
                open = false;
                continue;
            };
            let look = &model.looks[look as usize];
            let (font, code, advance, synthetic) = self.glyph(look, ch, label)?;
            let size = look.caps.apply(ch, look.size).1;
            let glyph = Glyph { code, x, advance };
            match runs.last_mut() {
                Some(run)
                    if open
                        && run.font == font
                        && std::ptr::eq(run.look, look)
                        && run.size == size
                        && run.synthetic == synthetic =>
                {
                    run.glyphs.push(glyph);
                }
                _ => runs.push(Run {
                    font,
                    look,
                    size,
                    synthetic,
                    glyphs: vec![glyph],
                }),
            }
            open = true;
        }
        Ok(runs)
    }

    /// A vertical rule for each bar tab of `para`, from the line's top to
    /// its bottom, and on the paragraph's last line through its space after
    /// (Word for Mac, measured).
    fn bars(&mut self, at: Point, para: &Para, line: &Line) -> Result<()> {
        let height = self.page_height()?;
        let after = if line.pieces.end as usize >= para.pieces.len() {
            para.after_spacing()
        } else {
            0.0
        };
        for stop in para.tabs.iter().filter(|stop| stop.kind == TabKind::Bar) {
            let x = at.x + stop.pos + BAR_WIDTH / 2.0;
            let out = &mut self.content;
            out.extend_from_slice(b"q 0 0 0 RG ");
            write_number(out, BAR_WIDTH)?;
            out.extend_from_slice(b" w ");
            for (y, operator) in [(at.y, "m"), (at.y + line.height + after, "l")] {
                write_number(out, x)?;
                out.push(b' ');
                write_number(out, height - y)?;
                out.push(b' ');
                out.extend_from_slice(operator.as_bytes());
                out.push(b' ');
            }
            out.extend_from_slice(b"S Q\n");
        }
        Ok(())
    }

    /// The rule of each note separator on `line`, in its run's colour.
    fn separators(&mut self, at: Point, para: &Para, line: &Line, origins: &[f64]) -> Result<()> {
        let height = self.page_height()?;
        let end = line.start + line.width + line.slack.max(0.0);
        for (piece, x) in line_pieces(para, line).iter().zip(origins) {
            let Piece::Separator {
                continuation, look, ..
            } = piece
            else {
                continue;
            };
            let room = (end - x).max(0.0);
            let length = if *continuation {
                room
            } else {
                SEPARATOR_LENGTH.min(room)
            };
            let look = &self.model.looks[*look as usize];
            let top = at.y + line.height * SEPARATOR_DROP;
            let out = &mut self.content;
            out.extend_from_slice(b"q ");
            decorate::colour(out, decorate::ink(look.colour, look, para.shade))?;
            out.extend_from_slice(b"rg ");
            for value in [
                at.x + x,
                height - top - SEPARATOR_THICKNESS,
                length,
                SEPARATOR_THICKNESS,
            ] {
                write_number(out, value)?;
                out.push(b' ');
            }
            out.extend_from_slice(b"re f Q\n");
        }
        Ok(())
    }

    fn check_drawn(&self) -> Result<()> {
        let cells: usize = self.cells.iter().map(Vec::len).sum();
        let open = cells + self.fills.len() + self.content.len() + self.pictures.layered();
        let drawn = self.drawn + open as u64;
        if drawn > self.max_drawn {
            return Err(Error::limit("max_pdf_content_bytes", self.max_drawn, drawn));
        }
        Ok(())
    }

    /// The decorations' view of `pieces` at `origins`, `left` from the
    /// page's left edge, `tab_end` giving the stop each tab moves to; none
    /// when no piece is decorated. Ordinary spaces after the last drawn
    /// piece trail, before a line break too, as Word leaves them; tabs and
    /// no-break spaces do not.
    fn spans(
        &self,
        pieces: &[Piece],
        origins: &[f64],
        left: f64,
        tab_end: impl Fn(usize) -> Option<f64>,
    ) -> Vec<Span<'m>> {
        let model = self.model;
        let look_of = |piece: &Piece| match piece {
            Piece::Char { look, .. } | Piece::Tab { look, .. } => {
                Some(&model.looks[*look as usize])
            }
            _ => None,
        };
        if !pieces.iter().filter_map(look_of).any(decorate::decorated) {
            return Vec::new();
        }
        let visible = pieces.iter().rposition(|piece| match piece {
            Piece::Char { ch, .. } => *ch != ' ',
            Piece::Tab { .. } | Piece::Object { .. } | Piece::NoteRef { .. } => true,
            Piece::LineBreak { .. } | Piece::PageBreak | Piece::Separator { .. } => false,
        });
        let mut spans = Vec::new();
        for (at, (piece, x)) in pieces.iter().zip(origins).enumerate() {
            let (look, face, end, blank) = match piece {
                Piece::Char {
                    ch, look, width, ..
                } => {
                    let look = &model.looks[*look as usize];
                    (look, look.face(*ch), x + width, ch.is_whitespace())
                }
                Piece::Tab { look, .. } => {
                    let look = &model.looks[*look as usize];
                    (look, look.ascii, tab_end(at).unwrap_or(*x), true)
                }
                _ => continue,
            };
            let to = origins.get(at + 1).copied().unwrap_or(end);
            spans.push(Span {
                look,
                face: self.drawing(face).clone(),
                from: left + x,
                to: left + to,
                blank,
                trailing: visible.is_none_or(|last| at > last),
            });
        }
        spans
    }

    /// The decorations' view of `line`: its label, then its pieces, each
    /// tab to its stop.
    fn line_spans(&self, left: f64, para: &Para, line: &Line) -> Vec<Span<'m>> {
        let (origins, label_origins) = line.origins(para);
        let mut spans = Vec::new();
        if let Some(label) = para.label.as_ref().filter(|_| line.label.is_some()) {
            spans = self.spans(&label.pieces, &label_origins, left, |_| None);
        }
        let first = line.pieces.start as usize;
        let pieces = line_pieces(para, line);
        spans.extend(self.spans(pieces, &origins, left, |at| {
            let tab = line
                .tabs
                .iter()
                .find(|tab| tab.piece as usize == first + at);
            tab.map(|tab| tab.to)
        }));
        spans
    }

    fn draw(&mut self, run: &Run, baseline: f64, paragraph: Option<Rgb>) -> Result<()> {
        let height = self.page_height()?;
        let look = run.look;
        let state = TextState {
            fill: decorate::ink(look.colour, look, paragraph),
            scale: look.scale,
            stroke: run.synthetic.0.then_some(run.size * SYNTHETIC_STROKE),
        };
        let out = &mut self.content;
        if state.fill != self.state.fill {
            decorate::colour(out, state.fill)?;
            out.extend_from_slice(b"rg\n");
        }
        if let Some(width) = state.stroke.filter(|_| state != self.state) {
            decorate::colour(out, state.fill)?;
            out.extend_from_slice(b"RG ");
            write_number(out, width)?;
            out.extend_from_slice(b" w\n");
        }
        out.extend_from_slice(b"BT ");
        write_name(out, &run.font.resource_name())?;
        out.push(b' ');
        write_number(out, run.size)?;
        out.extend_from_slice(b" Tf ");
        if state.scale != self.state.scale {
            write_number(out, state.scale * 100.0)?;
            out.extend_from_slice(b" Tz ");
        }
        if state.stroke.is_some() != self.state.stroke.is_some() {
            out.extend_from_slice(if state.stroke.is_some() {
                b"2 Tr "
            } else {
                b"0 Tr "
            });
        }
        let first = run.glyphs.first().map_or(0.0, |glyph| glyph.x);
        let shear = if run.synthetic.1 {
            SYNTHETIC_SHEAR
        } else {
            0.0
        };
        let vertical = if run.synthetic.0 {
            SYNTHETIC_HEIGHT
        } else {
            1.0
        };
        // The rise is in the matrix, not `Ts`, so a synthesized style shears
        // and scales about the raised baseline.
        let y = height - baseline + look.rise;
        for value in [1.0, 0.0, shear, vertical, first, y] {
            write_number(out, value)?;
            out.push(b' ');
        }
        out.extend_from_slice(b"Tm ");
        tj(
            out,
            &run.glyphs,
            run.size * state.scale,
            run.font.code_bytes(),
        )?;
        out.extend_from_slice(b" TJ ET\n");
        self.state = state;
        Ok(())
    }
}

/// Shifts smaller than this, in thousandths of an em, are left out: they
/// only undo the rounding of `/W`, and the pen keeps the error bounded.
const MIN_SHIFT: f64 = 0.01;

/// The `TJ` array that sets each glyph at its x: after a glyph, the
/// pen moves by the advance `/W` gives it, and a number moves it on to
/// the next glyph's x. The pen is followed as a reader computes it, from
/// the numbers as written, so no rounding accumulates.
fn tj(out: &mut Vec<u8>, glyphs: &[Glyph], em: f64, bytes: usize) -> Result<()> {
    out.push(b'[');
    let mut pen = glyphs.first().map_or(0.0, |glyph| glyph.x);
    let mut codes = Vec::new();
    for glyph in glyphs {
        let shift = round((pen - glyph.x) * 1000.0 / em);
        if shift.abs() >= MIN_SHIFT {
            write_hex(out, &codes);
            codes.clear();
            write_number(out, shift)?;
            pen -= shift * em / 1000.0;
        }
        codes.extend_from_slice(&glyph.code.to_be_bytes()[2 - bytes..]);
        pen += round(glyph.advance) * em / 1000.0;
    }
    write_hex(out, &codes);
    out.push(b']');
    Ok(())
}

fn no_page() -> Error {
    Error::internal("pdf_no_page", "a line before the first page")
}

/// A number as [`write_number`] writes it.
fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

impl Sink for Painter<'_, '_, '_, '_> {
    fn page(&mut self, page: &PageStart) -> Result<()> {
        self.close_page()?;
        let observed = self.pages.len() as u64 + 1;
        if observed > u64::from(self.max_pages) {
            return Err(Error::limit(
                "max_pdf_pages",
                u64::from(self.max_pages),
                observed,
            ));
        }
        let geometry = self.geometry(page);
        check_page(geometry.width, geometry.height)?;
        if self.pages.is_empty() {
            self.places.first_page(
                geometry.left + geometry.gutter,
                geometry.height - geometry.top,
            );
        }
        self.page = Some(*page);
        self.para_top = None;
        self.state = PAGE_START;
        Ok(())
    }

    fn note(&mut self, note: NoteId, mark: &str) -> Result<()> {
        self.note_marks.insert(note, mark.to_string());
        Ok(())
    }

    /// Word never splits a note holding a table across pages.
    fn note_unplaced(&mut self, note: NoteId) -> Result<()> {
        Err(Error::unsupported(
            super::FOOTNOTE_LAYOUT_UNSUPPORTED,
            format!(
                "footnote {} holds a table that would go on to the next page",
                note.index + 1
            ),
        ))
    }

    fn line(&mut self, at: Point, para: &Para, line: &Line) -> Result<()> {
        let marks = marks(self.model, &self.note_marks, at.x, para, line);
        let runs = self.runs(&marks)?;
        let spans = self.line_spans(at.x, para, line);
        let height = self.page_height()?;
        let baseline = at.y + line.baseline;
        if !is_furniture(para.story) && self.body_at.is_none() {
            self.body_at = Some(self.content.len());
        }
        decorate::backgrounds(&mut self.content, &spans, height, at.y, line.height)?;
        for run in &runs {
            self.draw(run, baseline, para.shade)?;
        }
        decorate::decorations(&mut self.content, &spans, height - baseline, para.shade)?;
        self.bars(at, para, line)?;
        let painted = Painted {
            page: self.pages.len(),
            height,
            at,
            para,
            line,
        };
        self.places.line(self.model, &painted);
        self.check_drawn()?;
        let (origins, _) = line.origins(para);
        self.separators(at, para, line, &origins)?;
        self.objects(line_pieces(para, line), &origins, at.x, baseline)?;
        self.anchors(at, para, line, &origins)?;
        self.check_drawn()
    }

    fn row(&mut self, origin: Point, table: &Table, row: usize, slice: &RowSlice) -> Result<()> {
        let height = self.page_height()?;
        let layer = usize::from(is_furniture(table.story));
        super::table::fills(&mut self.cells[layer], origin, (table, row), slice, height)?;
        self.strokes[layer].extend(row_strokes(origin, table, row, slice));
        self.check_drawn()
    }

    fn para_box(&mut self, boxed: &ParaBox) -> Result<()> {
        let height = self.page_height()?;
        super::boxes::fill(&mut self.fills, boxed, height)?;
        super::boxes::lines(&mut self.content, boxed, height)?;
        self.check_drawn()
    }
}

/// Headers and footers, drawn in their own layers under the main text's;
/// notes are main text.
fn is_furniture(story: StoryId) -> bool {
    matches!(story, StoryId::Header(_) | StoryId::Footer(_))
}

fn line_pieces<'p>(para: &'p Para, line: &Line) -> &'p [Piece] {
    para.pieces
        .get(line.pieces.start as usize..line.pieces.end as usize)
        .unwrap_or_default()
}

/// What `line` draws, in reading order: its label, then its characters,
/// each note's mark as `layout --lines` places it, and each tab's leader in
/// its place. A tab ends a run, as the label does.
fn marks(
    model: &Model,
    note_marks: &HashMap<NoteId, String>,
    left: f64,
    para: &Para,
    line: &Line,
) -> Vec<Option<Mark>> {
    let (origins, label_origins) = line.origins(para);
    let mut leaders = line.leaders(para, model, left).into_iter().peekable();
    let char_mark = |piece: &Piece, x: f64, label: bool| match piece {
        Piece::Char { ch, look, .. } => Some(Mark {
            ch: *ch,
            look: *look,
            x: left + x,
            label,
        }),
        _ => None,
    };
    let mut out = Vec::new();
    if let Some(label) = para.label.as_ref().filter(|_| line.label.is_some()) {
        out.extend(
            label
                .pieces
                .iter()
                .zip(&label_origins)
                .map(|(p, x)| char_mark(p, *x, true)),
        );
        out.push(None);
    }
    let first = line.pieces.start;
    let pieces = line_pieces(para, line);
    for (at, (piece, x)) in pieces.iter().zip(&origins).enumerate() {
        match piece {
            Piece::NoteRef {
                note, width, look, ..
            } => {
                let mark = note_marks.get(note).map_or("", String::as_str);
                out.extend(mark_characters(mark, *width, left + x).map(|(ch, x)| {
                    Some(Mark {
                        ch,
                        look: *look,
                        x,
                        label: false,
                    })
                }));
            }
            _ => out.push(char_mark(piece, *x, false)),
        }
        let piece = first + at as u32;
        let Some(leader) = leaders.next_if(|leader| leader.piece == piece) else {
            continue;
        };
        out.extend(leader.origins.iter().map(|x| {
            Some(Mark {
                ch: leader.ch,
                look: leader.look,
                x: *x,
                label: false,
            })
        }));
        out.push(None);
    }
    out
}
