//! How each character is drawn, decided once for the check before painting
//! and the painter alike: in the face that measured it when that face has
//! the glyph in a program the PDF may embed; else in the next face of the
//! chain, the built-in symbol font, a font referenced by name
//! (`allow_unembedded`), or not at all (a refusal naming what is missing).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::font::{Shape, win_ansi};
use super::symbols::meaning;
use super::{PdfFontUse, PdfWarning};
use crate::layout::NumberFormat;
use crate::layout::fonts::{Face, FaceSource, FontBook, metric_alias_differs, symbol_font};
use crate::layout::model::{Appearance, Block, Model, NoteId, NoteKind, Para, Piece, RunFace};
use crate::layout::variation::Style;
use crate::{Error, ErrorDetails, MissingFont, Result};

#[derive(Clone)]
pub(super) enum Draw {
    /// The glyph of the character from the face's program.
    Face(Arc<Face>, char),
    Shape(Shape),
    /// Referenced by name, at this `WinAnsiEncoding` code.
    Named(u8),
    /// The tests' stand-in face.
    StandIn,
}

#[derive(Clone)]
pub(super) struct Drawn {
    pub draw: Draw,
    /// The text it extracts as.
    pub text: String,
    /// Into the warnings, for each character drawn so.
    warning: Option<usize>,
}

struct Warning {
    code: &'static str,
    subject: &'static str,
    items: Vec<String>,
    count: usize,
}

/// Items quoted in a warning's message.
const MAX_ITEMS: usize = 5;

pub(super) struct Chain<'b> {
    book: &'b mut FontBook,
    allow_unembedded: bool,
    stand_in: bool,
    /// By face index, character, the character drawn, and label.
    decided: HashMap<(u32, char, char, bool), Option<Drawn>>,
    missing: BTreeSet<(String, bool, bool)>,
    restricted: BTreeSet<String>,
    no_glyph: BTreeSet<String>,
    warnings: Vec<Warning>,
    fonts: Vec<PdfFontUse>,
    /// By kind of note, every character a mark of that kind can show.
    mark_alphabets: HashMap<NoteKind, BTreeSet<char>>,
}

impl<'b> Chain<'b> {
    pub(super) fn new(book: &'b mut FontBook, allow_unembedded: bool, stand_in: bool) -> Self {
        Chain {
            book,
            allow_unembedded,
            stand_in,
            decided: HashMap::new(),
            missing: BTreeSet::new(),
            restricted: BTreeSet::new(),
            no_glyph: BTreeSet::new(),
            warnings: Vec::new(),
            fonts: Vec::new(),
            mark_alphabets: HashMap::new(),
        }
    }

    /// Decides every character of the body, the headers and footers some
    /// page shows and the notes, marks and page field values included,
    /// counting what the report states; refuses a family and style without
    /// a program, naming every one (`pdf_font_program_missing`), and a
    /// character nothing draws (`pdf_glyph_missing`).
    pub(super) fn check(&mut self, model: &Model) -> Result<()> {
        let formats: Vec<NumberFormat> = model.sections.iter().map(|s| s.number_format).collect();
        let mut visit = |para: &Para| {
            self.count_para(model, para);
            self.decide_fields(model, para, &formats);
        };
        each_para(&model.blocks, &mut visit);
        for story in model.stories.iter().filter(|s| model.shows(s.id)) {
            each_para(&story.blocks, &mut visit);
        }
        for story in [&model.notes.footnotes, &model.notes.endnotes] {
            for note in &story.notes {
                each_para(&note.blocks, &mut visit);
            }
            each_para(&story.separator, &mut visit);
            each_para(&story.continuation, &mut visit);
        }
        self.refusal().map_or(Ok(()), Err)
    }

    /// Decides the characters of `para`: its label, its text and its tab
    /// leaders. Paragraphs outside the body are checked through this too.
    pub(super) fn count_para(&mut self, model: &Model, para: &Para) {
        for piece in para.label.iter().flat_map(|label| &label.pieces) {
            if let Piece::Char { ch, look, .. } = piece {
                self.count(model, *look, *ch, true);
            }
        }
        for piece in &para.pieces {
            match piece {
                Piece::Char { ch, look, .. } => self.count(model, *look, *ch, false),
                Piece::Tab { look, .. } => {
                    for ch in para.tabs.iter().filter_map(|tab| tab.leader.char()) {
                        self.count(model, *look, ch, false);
                    }
                }
                Piece::NoteRef { note, look, .. } => self.count_mark(model, *note, *look),
                _ => {}
            }
        }
    }

    /// Decides every character `para`'s live fields can show, in the face
    /// the layout writes it in, since pagination writes their values.
    fn decide_fields(&mut self, model: &Model, para: &Para, formats: &[NumberFormat]) {
        for field in &para.fields {
            for ch in field.value_chars(formats) {
                if let Piece::Char { look, .. } = field.pen.piece(ch) {
                    self.drawn(model, &model.looks[look as usize], ch, false);
                }
            }
        }
    }

    /// Decides every character a mark of `note`'s kind can show, since
    /// pagination numbers the marks; counts those of its number in
    /// document order.
    fn count_mark(&mut self, model: &Model, note: NoteId, look: u32) {
        if model.notes.note(note).is_none_or(|n| n.custom) {
            return;
        }
        let alphabet = self
            .mark_alphabets
            .entry(note.kind)
            .or_insert_with(|| mark_alphabet(model, note.kind))
            .clone();
        let appearance = &model.looks[look as usize];
        for ch in alphabet {
            self.drawn(model, appearance, ch, false);
        }
        let Some(section) = model.sections.first() else {
            return;
        };
        let numbering = section.notes.get(note.kind);
        let mark = numbering.mark(numbering.start.saturating_add(note.index));
        for ch in mark.chars() {
            self.count(model, look, ch, false);
        }
    }

    /// Decides `ch` in look `look`, counting its warning.
    pub(super) fn count(&mut self, model: &Model, look: u32, ch: char, label: bool) {
        let look = &model.looks[look as usize];
        if let Some(at) = self.drawn(model, look, ch, label).and_then(|d| d.warning) {
            self.warnings[at].count += 1;
        }
    }

    fn refusal(&self) -> Option<Error> {
        if !self.missing.is_empty() {
            let names: Vec<String> = self
                .missing
                .iter()
                .map(|(family, bold, italic)| {
                    let restricted = if self.restricted.contains(family) {
                        " (its licence does not allow embedding)"
                    } else {
                        ""
                    };
                    format!("{family} {}{restricted}", style_name(*bold, *italic))
                })
                .collect();
            let details = ErrorDetails {
                missing_fonts: self
                    .missing
                    .iter()
                    .map(|(family, bold, italic)| MissingFont {
                        family: family.clone(),
                        bold: *bold,
                        italic: *italic,
                    })
                    .collect(),
                ..ErrorDetails::default()
            };
            let message = format!(
                "no font program for {}; supply the font files",
                names.join(", ")
            );
            return Some(
                Error::unsupported("pdf_font_program_missing", message).with_details(details),
            );
        }
        let missing: Vec<&str> = self.no_glyph.iter().map(String::as_str).collect();
        (!missing.is_empty()).then(|| {
            Error::unsupported(
                "pdf_glyph_missing",
                format!("no font draws {}", missing.join(", ")),
            )
        })
    }

    /// How `ch` in `look` is drawn; `None` when nothing draws it.
    pub(super) fn drawn(
        &mut self,
        model: &Model,
        look: &Appearance,
        ch: char,
        label: bool,
    ) -> Option<Drawn> {
        let index = look.face(ch);
        let drawn = look.caps.apply(ch, look.size).0;
        let key = (index, ch, drawn, label);
        if let Some(known) = self.decided.get(&key) {
            return known.clone();
        }
        let decision = self.decide(&model.faces[index as usize], drawn, ch, label);
        self.decided.insert(key, decision.clone());
        decision
    }

    fn decide(&mut self, run: &RunFace, drawn: char, ch: char, label: bool) -> Option<Drawn> {
        let face = &run.face;
        let substitute = run.source == FaceSource::Substitute;
        if !substitute && face.draws(drawn) {
            let warning = self.own_face(run, drawn);
            return Some(self.face(face.clone(), drawn, ch.to_string(), warning));
        }
        if let Some(symbol) = self.symbol(run, drawn, ch, label) {
            return Some(symbol);
        }
        if let Some(code) = win_ansi(drawn).filter(|_| self.allow_unembedded) {
            let item = format!("{} {}", run.family, style_name(run.bold, run.italic));
            let warning = self.warn("pdf_font_not_embedded", "font", item);
            self.used(run, &run.family.clone(), "not_embedded", face);
            return Some(Drawn {
                draw: Draw::Named(code),
                text: ch.to_string(),
                warning: Some(warning),
            });
        }
        if self.stand_in {
            return Some(Drawn {
                draw: Draw::StandIn,
                text: ch.to_string(),
                warning: None,
            });
        }
        if !substitute && face.embeddable() {
            if let Some(shape) = Shape::of(drawn).filter(|shape| *shape != Shape::Ring) {
                let item = format!("U+{:04X} in {}", drawn as u32, run.family);
                let warning = self.warn("pdf_symbol_substituted", "symbol", item);
                return Some(self.shape(run, shape, ch.to_string(), Some(warning)));
            }
            self.no_glyph
                .insert(format!("U+{:04X} ({})", drawn as u32, run.family));
            return None;
        }
        if run.restricted {
            self.restricted.insert(run.family.clone());
        }
        self.missing
            .insert((run.family.clone(), run.bold, run.italic));
        None
    }

    fn face(&self, face: Arc<Face>, drawn: char, text: String, warning: Option<usize>) -> Drawn {
        Drawn {
            draw: Draw::Face(face, drawn),
            text,
            warning,
        }
    }

    /// What drawing in the run's own face states.
    fn own_face(&mut self, run: &RunFace, drawn: char) -> Option<usize> {
        let face = &run.face;
        let style = style_name(run.bold, run.italic);
        let licence = if run.restricted {
            "its licence forbids embedding; "
        } else {
            ""
        };
        let substituted = || {
            let item = format!("{} {style} ({licence}drawn in {})", run.family, face.family);
            Some(("pdf_font_substituted", "font", item))
        };
        let (source, warning) = match run.source {
            FaceSource::Document => ("document", None),
            FaceSource::Supplied => ("supplied", None),
            FaceSource::Alias { tested } => {
                let silent = tested && !run.restricted && !metric_alias_differs(&run.family, drawn);
                ("alias", substituted().filter(|_| !silent))
            }
            FaceSource::Fallback => ("fallback", substituted()),
            FaceSource::Glyph => {
                let item = format!(
                    "U+{:04X} ({}, drawn in {})",
                    drawn as u32, run.family, face.family
                );
                ("glyph_fallback", Some(("pdf_glyph_missing", "glyph", item)))
            }
            FaceSource::Measured | FaceSource::Substitute => ("measured", None),
        };
        let warning = warning.or_else(|| {
            let from = style_name(face.bold, face.italic);
            let item = format!("{} {style} ({licence}drawn from {from})", run.family);
            run.style_missing
                .then_some(("pdf_font_substituted", "style", item))
        });
        self.used(run, &face.family, source, face);
        warning.map(|(code, subject, item)| self.warn(code, subject, item))
    }

    /// A character of a symbol font drawn without its program, or a bullet
    /// in Courier New the converter writes: the built-in font draws the
    /// converter's own shapes; another symbol is drawn as what it stands
    /// for by a declared fallback, else as a box (`pdf_symbol_substituted`).
    fn symbol(&mut self, run: &RunFace, drawn: char, ch: char, label: bool) -> Option<Drawn> {
        let ring = courier_ring(run, drawn, label);
        if !ring && !symbol_font(&run.family) {
            return None;
        }
        let meant = meaning(&run.family, drawn);
        let text = meant.map_or_else(|| ch.to_string(), String::from);
        if let Some(shape) = own_shape(run, drawn, label) {
            return Some(self.shape(run, shape, text, None));
        }
        let item = format!("U+{:04X} in {}", drawn as u32, run.family);
        let warning = Some(self.warn("pdf_symbol_substituted", "symbol", item));
        let style = Style::of(run.bold, run.italic);
        let face = meant.and_then(|m| Some((self.book.declared_drawing(style, m)?, m)));
        if let Some((face, m)) = face {
            self.used(run, &face.family, "symbol_fallback", &face);
            return Some(self.face(face, m, text, warning));
        }
        Some(self.shape(run, Shape::Unknown, text, warning))
    }

    fn shape(
        &mut self,
        run: &RunFace,
        shape: Shape,
        text: String,
        warning: Option<usize>,
    ) -> Drawn {
        let entry = PdfFontUse {
            requested_family: run.family.clone(),
            bold: run.bold,
            italic: run.italic,
            selected_family: None,
            source: "symbols",
            synthesized: false,
        };
        if !self.fonts.contains(&entry) {
            self.fonts.push(entry);
        }
        Drawn {
            draw: Draw::Shape(shape),
            text,
            warning,
        }
    }

    fn used(&mut self, run: &RunFace, selected: &str, source: &'static str, face: &Face) {
        let entry = PdfFontUse {
            requested_family: run.family.clone(),
            bold: run.bold,
            italic: run.italic,
            selected_family: Some(selected.to_string()),
            source,
            synthesized: face.file().is_some()
                && ((run.bold && !face.bold) || (run.italic && !face.italic)),
        };
        if !self.fonts.contains(&entry) {
            self.fonts.push(entry);
        }
    }

    fn warn(&mut self, code: &'static str, subject: &'static str, item: String) -> usize {
        let at = match self
            .warnings
            .iter()
            .position(|w| w.code == code && w.subject == subject)
        {
            Some(at) => at,
            None => {
                self.warnings.push(Warning {
                    code,
                    subject,
                    items: Vec::new(),
                    count: 0,
                });
                self.warnings.len() - 1
            }
        };
        let items = &mut self.warnings[at].items;
        if items.len() < MAX_ITEMS && !items.contains(&item) {
            items.push(item);
        }
        at
    }

    /// The warnings and the fonts drawn with.
    pub(super) fn report(&self) -> (Vec<PdfWarning>, Vec<PdfFontUse>) {
        let warnings = self
            .warnings
            .iter()
            .filter(|w| w.count > 0)
            .map(|w| PdfWarning {
                code: w.code,
                subject: w.subject,
                message: format!("{}: {}", super::description(w.code), w.items.join("; ")),
                locations: Vec::new(),
                count: w.count,
            })
            .collect();
        (warnings, self.fonts.clone())
    }
}

/// The converter's own shape for `drawn` set in `run`'s family, which the
/// built-in font draws: a list bullet in Symbol or Wingdings, or a label's
/// `o` in Courier New, and a ballot box in MS Gothic.
pub(super) fn own_shape(run: &RunFace, drawn: char, label: bool) -> Option<Shape> {
    let ring = courier_ring(run, drawn, label);
    if !ring && !symbol_font(&run.family) {
        return None;
    }
    meaning(&run.family, drawn)
        .and_then(Shape::of)
        .filter(|shape| ring || *shape != Shape::Ring)
}

/// Word's second-level bullet: a label's `o` in Courier New.
fn courier_ring(run: &RunFace, drawn: char, label: bool) -> bool {
    label && drawn == 'o' && run.family.eq_ignore_ascii_case("courier new")
}

/// Consecutive numbers that show every character a longer run of a note
/// format shows (roman numerals reach `m` at 1000).
const MARK_RUN: u32 = 1000;

/// The characters of every mark of `kind` a section's numbering gives,
/// from its first number to one past as many as there are notes.
fn mark_alphabet(model: &Model, kind: NoteKind) -> BTreeSet<char> {
    let count = (model.notes.story(kind).notes.len() as u32).min(MARK_RUN - 1);
    let mut alphabet = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for numbering in model.sections.iter().map(|s| s.notes.get(kind)) {
        if !seen.insert((numbering.format.as_str(), numbering.start)) {
            continue;
        }
        let last = numbering.start.saturating_add(count);
        for number in numbering.start..=last {
            alphabet.extend(numbering.mark(number).chars());
        }
    }
    alphabet
}

pub(super) fn each_para(blocks: &[Block], visit: &mut impl FnMut(&Para)) {
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

pub(super) fn style_name(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "Bold Italic",
    }
}
