//! The painter: a layout sink that draws each line where layout placed it,
//! every glyph at the x layout measured, in the face that measured it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::decorate::{self, Span};
use super::font::{Cid, FaceRef, FontResources};
use super::writer::{Ref, Writer, write_hex, write_name, write_number};
use super::{PdfLimits, check_page};
use crate::layout::flow::{Line, PageStart, Point, Sink};
use crate::layout::fonts::{Face, FontFile};
use crate::layout::mark_characters;
use crate::layout::model::{Appearance, Block, Model, NoteId, Para, Piece, Rgb, TabKind};
use crate::txn::Txn;
use crate::{Error, ErrorDetails, MissingFont, Result};

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

pub(super) struct Painter<'m, 't> {
    model: &'m Model,
    txn: &'t Txn<'t>,
    pub writer: Writer,
    pub fonts: FontResources,
    /// The resource of each of `model.faces`, once drawn with.
    faces: Vec<Option<FaceRef>>,
    /// Font programs read from the document's parts, by face file.
    programs: HashMap<usize, Arc<[u8]>>,
    pub pages: Vec<Page>,
    page: Option<(f64, f64)>,
    content: Vec<u8>,
    state: TextState,
    max_pages: u32,
    /// Content-stream bytes of the closed pages before compression, and
    /// their cap: a leader draws many glyphs for one tab.
    drawn: u64,
    max_drawn: u64,
    /// Draws, for tests, what no font program draws (a space for what it
    /// lacks), at the positions layout gives.
    stand_in: Option<Arc<Face>>,
    /// Each note's mark, as pagination numbers it.
    note_marks: HashMap<NoteId, String>,
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

/// One glyph to draw: its CID and the x layout gives its origin.
struct Glyph {
    cid: Cid,
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
}

/// Content streams compress well; uncompressed, they may be this many times
/// the output's byte cap.
const MAX_CONTENT_RATIO: u64 = 16;

/// Word for Mac strokes a bar tab 0.24 pt wide, its left edge at the stop.
const BAR_WIDTH: f64 = 0.24;

/// Glyphs of one face and look, drawn with one `TJ`.
struct Run<'a> {
    face: FaceRef,
    look: &'a Appearance,
    size: f64,
    synthetic: (bool, bool),
    glyphs: Vec<Glyph>,
}

impl<'m, 't> Painter<'m, 't> {
    pub(super) fn new(
        model: &'m Model,
        txn: &'t Txn<'t>,
        limits: &PdfLimits,
        stand_in: Option<Arc<Face>>,
    ) -> Result<Self> {
        Ok(Painter {
            model,
            txn,
            writer: Writer::new(limits.max_output_bytes)?,
            fonts: FontResources::default(),
            faces: vec![None; model.faces.len()],
            programs: HashMap::new(),
            pages: Vec::new(),
            page: None,
            content: Vec::new(),
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
        let Some(size) = self.page.take() else {
            return Ok(());
        };
        let contents = super::page_content(&mut self.writer, &self.content)?;
        self.drawn += self.content.len() as u64;
        self.content.clear();
        self.pages.push(Page { size, contents });
        Ok(())
    }

    /// The face that draws `model.faces[index]`.
    fn drawing(&self, index: u32) -> &Arc<Face> {
        let face = &self.model.faces[index as usize].face;
        match &self.stand_in {
            Some(stand_in) if face.file().is_none() => stand_in,
            _ => face,
        }
    }

    fn face(&mut self, index: u32) -> Result<FaceRef> {
        if let Some(Some(face)) = self.faces.get(index as usize) {
            return Ok(*face);
        }
        let face = self.drawing(index).clone();
        let file = face.file().ok_or_else(|| {
            Error::internal(
                "pdf_font_unchecked",
                "a face without a program reached the painter",
            )
        })?;
        let data = self.program(file, &face.family)?;
        let resource = self.fonts.add_face(data, file.index(), face.instance())?;
        self.faces[index as usize] = Some(resource);
        Ok(resource)
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
            let Some(Mark { ch, look, x }) = *mark else {
                open = false;
                continue;
            };
            let look = &model.looks[look as usize];
            let index = look.face(ch);
            let drawing = self.drawing(index).clone();
            let (mut drawn, size) = look.caps.apply(ch, look.size);
            if self.stand_in.is_some() && drawing.glyph(drawn).is_none() {
                drawn = ' ';
            }
            let glyph = match drawing.glyph(drawn) {
                Some(glyph) => glyph,
                None => {
                    return Err(Error::unsupported(
                        "pdf_glyph_missing",
                        format!(
                            "{} has no glyph for U+{:04X}; per-glyph fallback is not drawn yet",
                            drawing.family, drawn as u32
                        ),
                    ));
                }
            };
            let face = self.face(index)?;
            let cid = self.fonts.use_glyph(face, glyph, &ch.to_string())?;
            let synthetic = (look.bold && !drawing.bold, look.italic && !drawing.italic);
            let glyph = Glyph {
                cid,
                x,
                advance: drawing.advance(drawn) * 1000.0,
            };
            match runs.last_mut() {
                Some(run)
                    if open
                        && run.face == face
                        && std::ptr::eq(run.look, look)
                        && run.size == size
                        && run.synthetic == synthetic =>
                {
                    run.glyphs.push(glyph);
                }
                _ => runs.push(Run {
                    face,
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
        let Some((_, height)) = self.page else {
            return Ok(());
        };
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

    fn page_height(&self) -> Result<f64> {
        self.page
            .map(|(_, height)| height)
            .ok_or_else(|| Error::internal("pdf_no_page", "a line before the first page"))
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
        let pieces = para
            .pieces
            .get(first..line.pieces.end as usize)
            .unwrap_or_default();
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
        write_name(out, &run.face.resource_name())?;
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
        tj(out, &run.glyphs, run.size * state.scale)?;
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
fn tj(out: &mut Vec<u8>, glyphs: &[Glyph], em: f64) -> Result<()> {
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
        codes.extend_from_slice(&glyph.cid.to_be_bytes());
        pen += round(glyph.advance) * em / 1000.0;
    }
    write_hex(out, &codes);
    out.push(b']');
    Ok(())
}

/// A number as [`write_number`] writes it.
fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

impl Sink for Painter<'_, '_> {
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
        let geometry = &self.model.sections[page.section].page;
        let size = (geometry.width, geometry.height);
        check_page(size.0, size.1)?;
        self.page = Some(size);
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
        decorate::backgrounds(&mut self.content, &spans, height, at.y, line.height)?;
        for run in &runs {
            self.draw(run, baseline, para.shade)?;
        }
        decorate::decorations(&mut self.content, &spans, height - baseline, para.shade)?;
        self.bars(at, para, line)?;
        let drawn = self.drawn + self.content.len() as u64;
        if drawn > self.max_drawn {
            return Err(Error::limit("max_pdf_content_bytes", self.max_drawn, drawn));
        }
        Ok(())
    }
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
    let char_mark = |piece: &Piece, x: f64| match piece {
        Piece::Char { ch, look, .. } => Some(Mark {
            ch: *ch,
            look: *look,
            x: left + x,
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
                .map(|(p, x)| char_mark(p, *x)),
        );
        out.push(None);
    }
    let first = line.pieces.start;
    let pieces = para
        .pieces
        .get(first as usize..line.pieces.end as usize)
        .unwrap_or_default();
    for (at, (piece, x)) in pieces.iter().zip(&origins).enumerate() {
        match piece {
            Piece::NoteRef {
                note, width, look, ..
            } => {
                let mark = note_marks.get(note).map_or("", String::as_str);
                out.extend(
                    mark_characters(mark, *width, left + x)
                        .map(|(ch, x)| Some(Mark { ch, look: *look, x })),
                );
            }
            _ => out.push(char_mark(piece, *x)),
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
            })
        }));
        out.push(None);
    }
    out
}

/// Refuses a model with characters whose face has no font program, naming
/// every family and style that lacks one.
pub(super) fn check_programs(model: &Model) -> Result<()> {
    let mut missing = BTreeSet::new();
    let mut visit = |pieces: &[Piece], leaders: &[char]| {
        for piece in pieces {
            let (chars, look) = match piece {
                Piece::Char { ch, look, .. } => (std::slice::from_ref(ch), look),
                // A mark's digits are in the reference run's ASCII face.
                Piece::NoteRef { look, .. } => (&['0'][..], look),
                Piece::Tab { look, .. } => (leaders, look),
                _ => continue,
            };
            for ch in chars {
                let face = &model.faces[model.looks[*look as usize].face(*ch) as usize];
                if face.face.file().is_none() {
                    missing.insert((face.family.clone(), face.bold, face.italic));
                }
            }
        }
    };
    each_para(&model.blocks, &mut |para| {
        if let Some(label) = &para.label {
            visit(&label.pieces, &[]);
        }
        let leaders: Vec<char> = para.tabs.iter().filter_map(|t| t.leader.char()).collect();
        visit(&para.pieces, &leaders);
    });
    if missing.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = missing
        .iter()
        .map(|(family, bold, italic)| format!("{family} {}", style_name(*bold, *italic)))
        .collect();
    let details = ErrorDetails {
        missing_fonts: missing
            .into_iter()
            .map(|(family, bold, italic)| MissingFont {
                family,
                bold,
                italic,
            })
            .collect(),
        ..ErrorDetails::default()
    };
    Err(Error::unsupported(
        "pdf_font_program_missing",
        format!(
            "no font program for {}; supply the font files",
            names.join(", ")
        ),
    )
    .with_details(details))
}

fn each_para(blocks: &[Block], visit: &mut impl FnMut(&Para)) {
    for block in blocks {
        match block {
            Block::Para(para) => visit(para),
            Block::Table(table) => {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    each_para(&cell.blocks, visit);
                }
            }
        }
    }
}

fn style_name(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "Bold Italic",
    }
}
