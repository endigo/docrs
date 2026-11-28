//! The painter: a layout sink that draws each line where layout placed it,
//! every glyph at the x layout measured, in the face that measured it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::font::{Cid, FaceRef, FontResources};
use super::writer::{Ref, Writer, write_hex, write_name, write_number};
use super::{PdfLimits, check_page};
use crate::layout::flow::{Line, PageStart, Point, Sink};
use crate::layout::fonts::{Face, FontFile};
use crate::layout::model::{Appearance, Block, Colour, Model, Para, Piece};
use crate::txn::Txn;
use crate::{Error, ErrorDetails, MissingFont, Result};

/// Word's synthesized styles (assumption A2): the outline stroke of a bold
/// face drawn from a regular one, in ems, and the shear of an italic one.
const SYNTHETIC_STROKE: f64 = 1.0 / 36.0;
const SYNTHETIC_SHEAR: f64 = 0.21;

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
    /// Draws, for tests, what no font program draws (a space for what it
    /// lacks), at the positions layout gives.
    stand_in: Option<Arc<Face>>,
}

/// The text and graphics state a content stream carries between runs.
#[derive(Clone, Copy, PartialEq)]
struct TextState {
    fill: [u8; 3],
    scale: f64,
    rise: f64,
    stroke: Option<f64>,
}

const PAGE_START: TextState = TextState {
    fill: [0, 0, 0],
    scale: 1.0,
    rise: 0.0,
    stroke: None,
};

/// One glyph to draw: its CID and the x layout gives its origin.
struct Glyph {
    cid: Cid,
    x: f64,
    /// Its advance in the embedded font, in thousandths of an em.
    advance: f64,
}

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
            stand_in,
        })
    }

    /// Writes the open page's content stream.
    pub(super) fn close_page(&mut self) -> Result<()> {
        let Some(size) = self.page.take() else {
            return Ok(());
        };
        let contents = super::page_content(&mut self.writer, &self.content)?;
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

    /// The glyphs of `pieces` at `origins`, `left` from the page's left
    /// edge, grouped into runs.
    fn runs(&mut self, pieces: &[Piece], origins: &[f64], left: f64) -> Result<Vec<Run<'m>>> {
        let model = self.model;
        let mut runs: Vec<Run<'m>> = Vec::new();
        let mut open = false;
        for (piece, x) in pieces.iter().zip(origins) {
            let Piece::Char { ch, look, .. } = piece else {
                open = false;
                continue;
            };
            let look = &model.looks[*look as usize];
            let index = look.face(*ch);
            let drawing = self.drawing(index).clone();
            let (mut drawn, share) = look.caps.apply(*ch);
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
            let size = look.size * share;
            let synthetic = (look.bold && !drawing.bold, look.italic && !drawing.italic);
            let glyph = Glyph {
                cid,
                x: left + x,
                advance: drawing.advance(drawn) * 1000.0,
            };
            match runs.last_mut() {
                Some(run)
                    if open
                        && run.face == face
                        && std::ptr::eq(run.look, look)
                        && run.size == size =>
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

    fn draw(&mut self, run: &Run, baseline: f64) -> Result<()> {
        let Some((_, height)) = self.page else {
            return Err(Error::internal(
                "pdf_no_page",
                "a line before the first page",
            ));
        };
        let look = run.look;
        let state = TextState {
            fill: match look.colour {
                Colour::Rgb(rgb) => [rgb.r, rgb.g, rgb.b],
                _ => [0, 0, 0],
            },
            scale: look.scale,
            rise: look.rise,
            stroke: run.synthetic.0.then_some(run.size * SYNTHETIC_STROKE),
        };
        let out = &mut self.content;
        if state.fill != self.state.fill {
            colour(out, state.fill)?;
            out.extend_from_slice(b"rg\n");
        }
        if let Some(width) = state.stroke.filter(|_| state != self.state) {
            colour(out, state.fill)?;
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
        if state.rise != self.state.rise {
            write_number(out, state.rise)?;
            out.extend_from_slice(b" Ts ");
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
        for value in [1.0, 0.0, shear, 1.0, first, height - baseline] {
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

fn colour(out: &mut Vec<u8>, rgb: [u8; 3]) -> Result<()> {
    for value in rgb {
        write_number(out, f64::from(value) / 255.0)?;
        out.push(b' ');
    }
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

    fn line(&mut self, at: Point, para: &Para, line: &Line) -> Result<()> {
        let (origins, label_origins) = line.origins(para);
        let baseline = at.y + line.baseline;
        let label = para.label.as_ref().filter(|_| line.label.is_some());
        let pieces = para
            .pieces
            .get(line.pieces.start as usize..line.pieces.end as usize)
            .unwrap_or_default();
        let mut runs = match label {
            Some(label) => self.runs(&label.pieces, &label_origins, at.x)?,
            None => Vec::new(),
        };
        runs.extend(self.runs(pieces, &origins, at.x)?);
        for run in &runs {
            self.draw(run, baseline)?;
        }
        Ok(())
    }
}

/// Refuses a model with characters whose face has no font program, naming
/// every family and style that lacks one.
pub(super) fn check_programs(model: &Model) -> Result<()> {
    let mut missing = BTreeSet::new();
    let mut visit = |pieces: &[Piece]| {
        for piece in pieces {
            if let Piece::Char { ch, look, .. } = piece {
                let face = &model.faces[model.looks[*look as usize].face(*ch) as usize];
                if face.face.file().is_none() {
                    missing.insert((face.family.clone(), face.bold, face.italic));
                }
            }
        }
    };
    each_para(&model.blocks, &mut |para| {
        if let Some(label) = &para.label {
            visit(&label.pieces);
        }
        visit(&para.pieces);
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
