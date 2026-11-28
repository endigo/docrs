//! PDF export. **Experimental**: exempt from semantic versioning until the
//! release that completes it. Output is deterministic for a given build.
//!
//! [`render`] lays the document out as the TOC page numbers are laid out
//! and draws its text where layout placed it, in the fonts that measured
//! it. [`preflight`] reports what a document holds that a PDF would not
//! show faithfully. While the engine is incomplete, every construct it does
//! not draw yet is a `pdf_not_yet_drawn` warning, and [`PdfReport::check`]
//! refuses it, so a PDF is never silently incomplete.

#![cfg_attr(not(feature = "test-support"), allow(dead_code))]

mod boxes;
mod chain;
mod decorate;
mod font;
mod image;
mod limits;
mod meta;
mod nav;
mod paint;
mod picture;
mod places;
mod preflight;
mod symbols;
mod table;
mod writer;

pub use limits::PdfLimits;
pub use preflight::preflight;
#[cfg(feature = "test-support")]
pub use preflight::preflight_work_for_tests;

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use font::{SampleLine, Style};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use meta::Info;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use nav::{LinkId, Navigation, Rect};

use std::sync::Arc;

use serde::Serialize;
use writer::{Dict, Object, Ref, Writer, count, name};

use crate::coreprops::core_properties;
use crate::layout::fonts::{FaceSource, FontBook, bundled};
use crate::layout::model::{Piece, StoryId};
use crate::layout::{
    FIELD_CACHED, FIELD_FORMAT_APPROXIMATED, FIELD_SWITCH_IGNORED, Face, FontSet, LayoutTarget,
    STORY_OVERFLOW, Settled, model_of, settle,
};
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::{Document, Error, ErrorDetails, Limits, Result, WarningCount};

/// The code of a construct the engine does not draw yet.
pub const NOT_YET_DRAWN: &str = "pdf_not_yet_drawn";

/// The code of notes the layout cannot place as Word does.
pub const FOOTNOTE_LAYOUT_UNSUPPORTED: &str = "pdf_footnote_layout_unsupported";

/// Every warning code, with a fixed description that quotes nothing from
/// the input ([`PdfReport::redact`]).
pub const WARNING_CODES: &[(&str, &str)] = &[
    (
        NOT_YET_DRAWN,
        "the PDF engine does not draw this yet; a PDF would leave it out",
    ),
    ("pdf_chart_skipped", "a chart is left out of the PDF"),
    (
        "pdf_smartart_skipped",
        "a SmartArt graphic is left out of the PDF",
    ),
    (
        "pdf_object_skipped",
        "an embedded object (OLE object, ActiveX control, imported document) is left out of the PDF",
    ),
    ("pdf_shape_skipped", "a drawn shape is left out of the PDF"),
    (
        "pdf_text_box_skipped",
        "a text box is left out of the PDF with the text inside it",
    ),
    (
        "pdf_watermark_skipped",
        "a watermark is left out of the PDF",
    ),
    (
        "pdf_columns_ignored",
        "a section with several columns is laid out in one column",
    ),
    (
        "pdf_wrap_ignored",
        "text does not wrap around a floating object",
    ),
    (
        "pdf_drawing_fallback_used",
        "a chart or SmartArt graphic is drawn as the picture its document keeps in its place",
    ),
    (
        "pdf_equation_linearized",
        "an equation is drawn as a line of text",
    ),
    (
        image::UNSUPPORTED,
        "an image in a format the PDF engine does not draw (TIFF, EMF, WMF, a compressed bitmap, …) is left out of the PDF",
    ),
    (
        image::PROFILE_IGNORED,
        "an image's embedded colour profile is ignored: its colours are drawn as plain RGB, grey or CMYK",
    ),
    (
        image::ANIMATED,
        "an animated image is drawn as its first frame",
    ),
    (
        "pdf_text_unsupported",
        "text that needs more than Latin script (right-to-left, complex scripts, CJK, ruby, combining marks, emoji sequences) may be drawn wrongly",
    ),
    (
        "pdf_layout_feature_ignored",
        "a layout feature (page borders, line numbers, frames, drop caps, floating tables, cell spacing, rotated text, vertical page justification, automatic hyphenation) is ignored",
    ),
    (
        "pdf_revisions_flattened",
        "tracked changes and comments are not shown: the PDF shows the text as if every change were accepted",
    ),
    (
        FIELD_CACHED,
        "a field shows the result saved in the document, not the one Word would compute",
    ),
    (
        "pdf_decoration_approximated",
        "a border style the PDF does not draw is drawn as a single line",
    ),
    (
        FIELD_SWITCH_IGNORED,
        "a page reference's \\p switch is ignored: it shows the page number, not \"above\" or \"below\"",
    ),
    (
        FIELD_FORMAT_APPROXIMATED,
        "a page number asked in words, as an ordinal or through a numeric picture is shown in arabic digits",
    ),
    (
        STORY_OVERFLOW,
        "a header or footer reaching past the middle of the page is cut off",
    ),
    (
        "pdf_justification_unsupported",
        "distributed, kashida or Thai justification is drawn as ordinary justification",
    ),
    (
        FOOTNOTE_LAYOUT_UNSUPPORTED,
        "footnotes or endnotes placed beneath the text, in columns or at section ends, or a note's table split across pages: the PDF engine cannot place them as Word does",
    ),
    (
        "pdf_link_scheme_skipped",
        "a link uses a scheme other than http, https, mailto, tel or a bookmark; its text is kept without the link",
    ),
    (
        "pdf_link_target_missing",
        "a link goes to a bookmark the document does not have; its text is kept without the link",
    ),
    (
        "pdf_font_substituted",
        "a font or style was drawn and measured with another one, so lines may break differently than in Word",
    ),
    (
        "pdf_glyph_missing",
        "a character its font lacks was drawn and measured with a fallback font",
    ),
    (
        "pdf_symbol_substituted",
        "a symbol was drawn as the character it stands for in another font, or as a box",
    ),
    (
        "pdf_font_not_embedded",
        "a font is referenced by name and not embedded; viewers without it substitute another",
    ),
];

/// The fixed description of `code`.
fn description(code: &str) -> &'static str {
    WARNING_CODES
        .iter()
        .find(|(known, _)| *known == code)
        .map_or(
            "the PDF would not show something faithfully",
            |(_, text)| text,
        )
}

/// What a warning of `code` on `subject` says.
fn message(code: &str, subject: &str) -> &'static str {
    match (code, subject) {
        (FIELD_CACHED, "DATE" | "TIME" | "PRINTDATE") => {
            "a date or time field shows the value saved in the document, not the current one"
        }
        (FIELD_CACHED, "TOC") => {
            "a contents entry whose number is not only a page reference field shows the number saved in the document"
        }
        (FIELD_CACHED, "field without an end") => {
            "a field without an end is not computed: the PDF shows the text saved in its place"
        }
        (FIELD_CACHED, _) => {
            "a page field whose instruction the PDF engine does not read shows the number saved in the document"
        }
        _ => description(code),
    }
}

/// Where a construct is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PdfLocation {
    /// The part holding it (`word/document.xml`, `word/header1.xml`, …).
    pub story: String,
    /// The story's paragraph it is in or after, from 1 (0: before the
    /// first; document-wide settings have no location).
    pub ordinal: usize,
    /// The page, once the document is laid out.
    pub page: Option<u32>,
    /// `body`, `header`, `footer`, `footnotes` or `endnotes`: the story
    /// once redacted.
    #[serde(skip)]
    kind: &'static str,
}

/// One kind of construct a PDF would not show faithfully.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PdfWarning {
    /// Stable code ([`WARNING_CODES`]).
    pub code: &'static str,
    /// What it is about, from a fixed vocabulary (`table`, `CJK text`,
    /// `page borders`, …); one warning per code and subject.
    pub subject: &'static str,
    /// What happened; may quote the input (text box text, link schemes).
    pub message: String,
    /// The first locations, at most [`MAX_LOCATIONS`].
    pub locations: Vec<PdfLocation>,
    /// Occurrences.
    pub count: usize,
}

/// Locations kept per warning.
pub const MAX_LOCATIONS: usize = 10;

/// Quoted input kept per warning, and its length.
const MAX_SAMPLES: usize = 5;
const SAMPLE_CHARS: usize = 60;

/// Occurrences gathered into one warning per code and subject, in order of
/// first occurrence.
#[derive(Debug, Default)]
struct Tally(Vec<Entry>);

#[derive(Debug)]
struct Entry {
    code: &'static str,
    subject: &'static str,
    count: usize,
    locations: Vec<PdfLocation>,
    samples: Vec<String>,
}

impl Tally {
    fn note(
        &mut self,
        code: &'static str,
        subject: &'static str,
        location: Option<PdfLocation>,
        sample: Option<String>,
    ) {
        let index = match self
            .0
            .iter()
            .position(|e| e.code == code && e.subject == subject)
        {
            Some(index) => index,
            None => {
                self.0.push(Entry {
                    code,
                    subject,
                    count: 0,
                    locations: Vec::new(),
                    samples: Vec::new(),
                });
                self.0.len() - 1
            }
        };
        let entry = &mut self.0[index];
        entry.count += 1;
        if let Some(location) = location.filter(|_| entry.locations.len() < MAX_LOCATIONS) {
            entry.locations.push(location);
        }
        if let Some(sample) = sample.filter(|_| entry.samples.len() < MAX_SAMPLES) {
            entry
                .samples
                .push(sample.chars().take(SAMPLE_CHARS).collect());
        }
    }

    fn finish(self) -> Vec<PdfWarning> {
        self.0
            .into_iter()
            .map(|entry| {
                let mut message = format!(
                    "{} ({}): {}",
                    entry.subject,
                    entry.count,
                    message(entry.code, entry.subject)
                );
                if !entry.samples.is_empty() {
                    let quoted: Vec<String> =
                        entry.samples.iter().map(|s| format!("\"{s}\"")).collect();
                    message.push_str(&format!(" ({})", quoted.join(", ")));
                }
                PdfWarning {
                    code: entry.code,
                    subject: entry.subject,
                    message,
                    locations: entry.locations,
                    count: entry.count,
                }
            })
            .collect()
    }
}

/// What a PDF of the document would leave out or change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PdfReport {
    /// In order of first occurrence.
    pub warnings: Vec<PdfWarning>,
    /// Pages written; `None` from [`preflight`].
    pub pages: Option<u32>,
    /// What drew each family and style, in order of first use; empty from
    /// [`preflight`].
    pub fonts: Vec<PdfFontUse>,
}

/// A family and style the document asks for, and what drew it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PdfFontUse {
    pub requested_family: String,
    pub bold: bool,
    pub italic: bool,
    /// The family of the face that drew it; `None` for the built-in symbol
    /// font.
    pub selected_family: Option<String>,
    /// `document` (a font it embeds), `supplied`, `alias`, `fallback`,
    /// `glyph_fallback` (characters the face lacks), `symbols` (the
    /// built-in symbol font), `symbol_fallback` or `not_embedded`.
    pub source: &'static str,
    /// Bold or italic drawn from another style.
    pub synthesized: bool,
}

/// A family and style the document's text is set in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct UsedFont {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// The document embeds a font program for it.
    pub embedded: bool,
    /// The built-in symbol font draws every character set in it, so a PDF
    /// needs no program for it.
    pub optional: bool,
}

/// How to export.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PdfOptions {
    /// Font programs for the families the document does not embed.
    pub fonts: FontSet,
    /// Refuse on any warning (`pdf_not_faithful`).
    pub strict: bool,
    /// Reference a face with no program it may embed by name, measured as
    /// layout measured it (`pdf_font_not_embedded`), instead of refusing.
    pub allow_unembedded: bool,
    pub limits: PdfLimits,
    /// Bookmarks of the headings (on by default).
    pub outline: bool,
    /// Hyperlinks, and the destinations of the document's bookmarks (on by
    /// default).
    pub links: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions {
            fonts: FontSet::default(),
            strict: false,
            allow_unembedded: false,
            limits: PdfLimits::default(),
            outline: true,
            links: true,
        }
    }
}

const PRODUCER: &str = concat!("docrs ", env!("CARGO_PKG_VERSION"));

/// The document as a PDF, with what it leaves out or changes. Runs
/// [`preflight`] and [`PdfReport::check`] first, then decides how each
/// character is drawn: in the face that measured it (the document's
/// embedded font, else one of `options.fonts`: the family's own, an alias,
/// the declared fallback for its kind), a character the face lacks in the
/// next face of that chain, a symbol in the built-in symbol font. A family
/// and style with no program refuses (`pdf_font_program_missing`, every
/// one listed in the error's details) unless `allow_unembedded`, and so
/// does a character nothing draws (`pdf_glyph_missing`). Page numbers,
/// page counts and page references, the contents' included, show the PDF's
/// own pages, and what layout changes in the stories some page shows is
/// reported. With `strict`, any warning refuses. Reads only; the document
/// is unchanged.
pub fn render(document: &Document, options: &PdfOptions) -> Result<(Vec<u8>, PdfReport)> {
    let mut report = preflight(document)?;
    report.check(options.strict)?;
    let (bytes, pages) = paint(document, options, None, &mut report)?;
    // A link navigation left out, or what only drawing finds (an image's
    // profile, animation or unreadable data), that the preflight did not
    // count.
    report.check(options.strict)?;
    report.pages = Some(pages);
    Ok((bytes, report))
}

/// The layout a PDF draws, its fields showing the PDF's own pages, and
/// the font book that measured it.
fn settle_for_pdf(txn: &Txn<'_>, options: &PdfOptions) -> Result<(Settled, FontBook)> {
    let target = LayoutTarget::Pdf {
        allow_unembedded: options.allow_unembedded,
    };
    let main = txn.document().package().main_part_name();
    settle(txn, main, &options.fonts, target)
}

/// What layout changes in the stories some page shows: stories cut off,
/// page numbers written otherwise, page references it cannot place.
/// What `reported` already holds is not repeated.
fn layout_warnings(
    txn: &Txn<'_>,
    settled: &Settled,
    main: &str,
    reported: &[PdfWarning],
) -> Result<Vec<PdfWarning>> {
    let notes = |rel| txn.part_related_by(main, rel);
    let parts = [notes(rt::FOOTNOTES)?, notes(rt::ENDNOTES)?];
    let mut tally = Tally::default();
    let model = settled.model();
    for warning in settled.warnings.iter().filter(|w| model.shows(w.story)) {
        if reported
            .iter()
            .any(|r| r.code == warning.code && r.subject == warning.subject)
        {
            continue;
        }
        let location = PdfLocation {
            story: match warning.story {
                StoryId::Footnotes => parts[0].as_deref(),
                StoryId::Endnotes => parts[1].as_deref(),
                story => settled.story_part(story),
            }
            .unwrap_or(main)
            .to_string(),
            ordinal: 0,
            page: warning.page,
            kind: warning.story.kind(),
        };
        tally.note(warning.code, warning.subject, Some(location), None);
    }
    Ok(tally.finish())
}

/// Lays out and draws `document`, adding what its fonts change, what
/// navigation leaves out and what only drawing finds to `report`, and
/// refusing as [`PdfReport::check`] does before drawing; `stand_in` draws
/// what nothing else does.
fn paint(
    document: &Document,
    options: &PdfOptions,
    stand_in: Option<Arc<Face>>,
    report: &mut PdfReport,
) -> Result<(Vec<u8>, u32)> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name();
    let (settled, mut book) = settle_for_pdf(&txn, options)?;
    let model = settled.model();
    let mut chain = chain::Chain::new(&mut book, options.allow_unembedded, stand_in.is_some());
    chain.check(model)?;
    let (warnings, fonts) = chain.report();
    report.warnings.extend(warnings);
    report.fonts = fonts;
    let changed = layout_warnings(&txn, &settled, main, &report.warnings)?;
    report.warnings.extend(changed);
    report.check(options.strict)?;
    let limits = &options.limits;
    let places = places::Places::new(options.links, options.outline);
    let mut painter = paint::Painter::new(model, &txn, &mut chain, limits, places, stand_in)?;
    settled.paginate(&mut painter)?;
    painter.close_page()?;
    let pages = painter.pages.len() as u32;
    let mut writer = painter.writer;
    let fonts = painter
        .fonts
        .finish(&mut writer)?
        .into_iter()
        .fold(Dict::new(), |fonts, (name, font)| fonts.with(&name, font));
    let sizes: Vec<((f64, f64), Ref)> = painter
        .pages
        .iter()
        .map(|page| (page.size, page.contents))
        .collect();
    let mut resources = Dict::new().with("Font", fonts);
    if let Some(images) = painter.pictures.resources() {
        resources.set("XObject", images);
    }
    let warnings = painter.pictures.tally.finish();
    let info = info(document)?;
    let (bytes, navigation) = write_document(
        writer,
        &sizes,
        resources,
        painter.places.navigation,
        &info,
        PRODUCER,
    )?;
    report.add_navigation(navigation);
    report.warnings.extend(warnings);
    Ok((bytes, pages))
}

/// `/Info` from the core properties.
fn info(document: &Document) -> Result<meta::Info> {
    let core = core_properties(document)?;
    let text = |value: &str| (!value.trim().is_empty()).then(|| value.to_string());
    Ok(meta::Info {
        title: text(&core.title),
        author: text(&core.author),
        subject: text(&core.subject),
        keywords: text(&core.keywords),
        created: core.created,
        modified: core.modified,
    })
}

/// The families and styles `document`'s body text is set in, in order of
/// first use, for loading only the fonts a PDF needs. Reads only.
pub fn used_fonts(document: &Document) -> Result<Vec<UsedFont>> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name();
    let (model, ..) = model_of(&txn, main, &FontSet::default(), LayoutTarget::Word)?;
    let mut used: Vec<UsedFont> = Vec::new();
    chain::each_para(&model.blocks, &mut |para| {
        let label = para.label.iter().flat_map(|label| &label.pieces);
        let pieces = label
            .map(|p| (p, true))
            .chain(para.pieces.iter().map(|p| (p, false)));
        for (piece, in_label) in pieces {
            let Piece::Char { ch, look, .. } = piece else {
                continue;
            };
            let look = &model.looks[*look as usize];
            let run = &model.faces[look.face(*ch) as usize];
            let optional =
                chain::own_shape(run, look.caps.apply(*ch, look.size).0, in_label).is_some();
            let at = used.iter().position(|u| {
                u.family == run.family && (u.bold, u.italic) == (run.bold, run.italic)
            });
            match at {
                Some(at) => used[at].optional &= optional,
                None => used.push(UsedFont {
                    family: run.family.clone(),
                    bold: run.bold,
                    italic: run.italic,
                    embedded: run.source == FaceSource::Document,
                    optional,
                }),
            }
        }
    });
    Ok(used)
}

impl PdfReport {
    /// Navigation's warnings, where the preflight counted fewer of a code
    /// and subject: it reads the same links, so it normally counts as many,
    /// with their locations.
    fn add_navigation(&mut self, warnings: Vec<PdfWarning>) {
        for warning in warnings {
            let known = self
                .warnings
                .iter_mut()
                .find(|known| (known.code, known.subject) == (warning.code, warning.subject));
            match known {
                Some(known) if known.count >= warning.count => {}
                Some(known) => *known = warning,
                None => self.warnings.push(warning),
            }
        }
    }

    /// Replace every message with its code and subject's fixed
    /// description, every location's part name with its story kind
    /// (`body`, `header`, `footer`, `footnotes`, `endnotes`), and every
    /// family name the bundled metrics table does not hold with
    /// `redacted`, so the report holds nothing from the input.
    pub fn redact(&mut self) {
        for warning in &mut self.warnings {
            warning.message = message(warning.code, warning.subject).to_string();
            for location in &mut warning.locations {
                location.story = location.kind.to_string();
            }
        }
        let redact = |family: &mut String| {
            if !bundled(family) {
                *family = "redacted".to_string();
            }
        };
        for font in &mut self.fonts {
            redact(&mut font.requested_family);
            if let Some(selected) = &mut font.selected_family {
                redact(selected);
            }
        }
    }

    /// JSON form (`schema_version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("schema_version".into(), 1.into());
        }
        value
    }

    /// Refuse a construct not drawn yet (`pdf_not_yet_drawn`) and notes
    /// the layout cannot place (`pdf_footnote_layout_unsupported`) always,
    /// and with `strict` any warning (`pdf_not_faithful`). The refusal's
    /// details count each code.
    pub fn check(&self, strict: bool) -> Result<()> {
        let subjects = |code: &str| -> Vec<&str> {
            self.warnings
                .iter()
                .filter(|w| w.code == code)
                .map(|w| w.subject)
                .collect()
        };
        let missing = subjects(NOT_YET_DRAWN);
        let misplaced = subjects(FOOTNOTE_LAYOUT_UNSUPPORTED);
        let error = if !missing.is_empty() {
            Error::unsupported(
                NOT_YET_DRAWN,
                format!("the PDF engine does not draw {} yet", missing.join(", ")),
            )
        } else if !misplaced.is_empty() {
            Error::unsupported(
                FOOTNOTE_LAYOUT_UNSUPPORTED,
                format!(
                    "the PDF engine cannot place {} as Word does",
                    misplaced.join(", ")
                ),
            )
        } else if strict && !self.warnings.is_empty() {
            Error::unsupported(
                "pdf_not_faithful",
                format!(
                    "the PDF would not show the document faithfully: {}",
                    self.codes().join(", ")
                ),
            )
        } else {
            return Ok(());
        };
        Err(error.with_details(self.details()))
    }

    fn codes(&self) -> Vec<&'static str> {
        let mut codes: Vec<&str> = self.warnings.iter().map(|w| w.code).collect();
        codes.sort_unstable();
        codes.dedup();
        codes
    }

    fn details(&self) -> ErrorDetails {
        let warnings = self
            .codes()
            .into_iter()
            .map(|code| WarningCount {
                code,
                count: self
                    .warnings
                    .iter()
                    .filter(|w| w.code == code)
                    .map(|w| w.count)
                    .sum(),
                description: description(code),
            })
            .collect();
        ErrorDetails {
            warnings,
            ..ErrorDetails::default()
        }
    }
}

/// Page side limits in points (ISO 32000-1 Annex C).
const MIN_PAGE_SIDE: f64 = 3.0;
const MAX_PAGE_SIDE: f64 = 14_400.0;

/// A file of empty pages, `(width, height)` in points.
pub(crate) fn blank_document(
    pages: &[(f64, f64)],
    navigation: nav::Navigation,
    info: &meta::Info,
    producer: &str,
    limits: &Limits,
) -> Result<(Vec<u8>, Vec<PdfWarning>)> {
    check_pages(pages)?;
    let mut writer = Writer::new(limits.max_output_bytes)?;
    let pages = pages
        .iter()
        .map(|&size| Ok((size, page_content(&mut writer, &[])?)))
        .collect::<Result<Vec<_>>>()?;
    write_document(writer, &pages, Dict::new(), navigation, info, producer)
}

/// A page's content stream.
fn page_content(writer: &mut Writer, content: &[u8]) -> Result<Ref> {
    let contents = writer.alloc()?;
    writer.stream(contents, Dict::new(), content)?;
    Ok(contents)
}

/// Writes the pages, each `(size, content stream)`, sharing `resources`,
/// with their navigation, then the catalog, the `/Info` and the trailer.
fn write_document(
    mut writer: Writer,
    pages: &[((f64, f64), Ref)],
    resources: Dict,
    navigation: nav::Navigation,
    info: &meta::Info,
    producer: &str,
) -> Result<(Vec<u8>, Vec<PdfWarning>)> {
    let catalog = writer.alloc()?;
    let tree = writer.alloc()?;
    let mut page_refs = Vec::with_capacity(pages.len());
    for _ in pages {
        page_refs.push(writer.alloc()?);
    }
    let heights = pages.iter().map(|((_, height), _)| *height);
    let placed: Vec<_> = page_refs.iter().copied().zip(heights).collect();
    let nav = navigation.write(&mut writer, &placed)?;
    for ((&((width, height), contents), &page), annots) in
        pages.iter().zip(&page_refs).zip(nav.annots)
    {
        let media_box: Vec<Object> = vec![0.0.into(), 0.0.into(), width.into(), height.into()];
        let mut dict = Dict::new()
            .with("Type", name("Page"))
            .with("Parent", tree)
            .with("MediaBox", media_box)
            .with("Contents", contents);
        if !annots.is_empty() {
            dict.set("Annots", annots);
        }
        writer.dict(page, dict)?;
    }
    writer.dict(
        tree,
        Dict::new()
            .with("Type", name("Pages"))
            .with("Count", count(pages.len())?)
            .with(
                "Kids",
                page_refs.into_iter().map(Object::from).collect::<Vec<_>>(),
            )
            .with("Resources", resources),
    )?;
    let mut root = Dict::new()
        .with("Type", name("Catalog"))
        .with("Pages", tree);
    for (key, value) in nav.catalog {
        root.set(key, value);
    }
    writer.dict(catalog, root)?;
    let info_ref = writer.alloc()?;
    writer.dict(info_ref, info.dict(producer)?)?;
    let id = meta::file_id(writer.body());
    let bytes = writer.finish(
        Dict::new()
            .with("Root", catalog)
            .with("Info", info_ref)
            .with("ID", id),
    )?;
    Ok((bytes, nav.warnings))
}

fn check_pages(pages: &[(f64, f64)]) -> Result<()> {
    if pages.is_empty() {
        return Err(Error::invalid_argument(
            "pdf_no_pages",
            "a PDF needs at least one page",
        ));
    }
    pages
        .iter()
        .try_for_each(|&(width, height)| check_page(width, height))
}

fn check_page(width: f64, height: f64) -> Result<()> {
    if [width, height]
        .iter()
        .all(|side| (MIN_PAGE_SIDE..=MAX_PAGE_SIDE).contains(side))
    {
        return Ok(());
    }
    Err(Error::unsupported(
        "pdf_page_size",
        format!(
            "page {width} × {height} pt: each side must be {MIN_PAGE_SIDE} to {MAX_PAGE_SIDE} pt"
        ),
    ))
}

/// [`blank_document`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn blank_document_for_tests(
    pages: &[(f64, f64)],
    info: &Info,
    producer: &str,
    limits: &Limits,
) -> Result<Vec<u8>> {
    blank_document(pages, Navigation::default(), info, producer, limits).map(|(bytes, _)| bytes)
}

/// [`blank_document`] with navigation, for tests only (`test-support`
/// feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn linked_document_for_tests(
    pages: &[(f64, f64)],
    navigation: Navigation,
    info: &Info,
    producer: &str,
    limits: &Limits,
) -> Result<(Vec<u8>, Vec<PdfWarning>)> {
    blank_document(pages, navigation, info, producer, limits)
}

/// [`font::text_document`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn text_document_for_tests(
    lines: &[SampleLine],
    info: &Info,
    producer: &str,
    limits: &Limits,
) -> Result<Vec<u8>> {
    font::text_document(lines, info, producer, limits)
}

/// [`image::image_document`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn image_document_for_tests(
    images: &[(&str, &[u8])],
    info: &Info,
    producer: &str,
    limits: &PdfLimits,
) -> Result<(Vec<u8>, Vec<Vec<&'static str>>)> {
    image::image_document(images, info, producer, limits)
}

/// The document's body drawn without the preflight, every face that has
/// no font program drawn with `stand_in`'s first face, for tests only
/// (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn paint_for_tests(document: &Document, fonts: &FontSet, stand_in: &[u8]) -> Result<Vec<u8>> {
    let stand_in = Face::read_at(&Arc::from(stand_in), 0)?;
    let options = PdfOptions {
        fonts: fonts.clone(),
        ..PdfOptions::default()
    };
    let mut report = PdfReport::default();
    paint(document, &options, Some(Arc::new(stand_in)), &mut report).map(|(bytes, _)| bytes)
}

/// [`render`] without the preflight, for drawing what it still refuses,
/// for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn render_unchecked_for_tests(
    document: &Document,
    options: &PdfOptions,
) -> Result<(Vec<u8>, PdfReport)> {
    let mut report = PdfReport::default();
    paint(document, options, None, &mut report).map(|(bytes, _)| (bytes, report))
}

/// The settled layout [`render`] draws, for tests only (`test-support`
/// feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn layout_for_tests(document: &Document, fonts: &FontSet) -> Result<crate::layout::Layout> {
    let options = PdfOptions {
        fonts: fonts.clone(),
        ..PdfOptions::default()
    };
    settle_for_pdf(&Txn::new(document), &options).map(|(settled, _)| settled.layout)
}

/// [`font::glyphs_by_cmap`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn glyphs_for_tests(font: &[u8], index: u32, text: &str) -> Result<Vec<(u16, String)>> {
    font::glyphs_by_cmap(font, index, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warning(code: &'static str, count: usize) -> PdfWarning {
        PdfWarning {
            code,
            subject: "link",
            message: format!("{count}"),
            locations: Vec::new(),
            count,
        }
    }

    #[test]
    fn navigation_adds_only_what_the_preflight_counted_fewer_of() {
        let mut report = PdfReport {
            warnings: vec![warning(TARGET_MISSING, 2), warning(SCHEME_SKIPPED, 1)],
            ..PdfReport::default()
        };
        report.add_navigation(vec![
            warning(TARGET_MISSING, 1),
            warning(SCHEME_SKIPPED, 3),
            warning(NOT_YET_DRAWN, 1),
        ]);
        let counts: Vec<(&str, usize)> =
            report.warnings.iter().map(|w| (w.code, w.count)).collect();
        assert_eq!(
            counts,
            [(TARGET_MISSING, 2), (SCHEME_SKIPPED, 3), (NOT_YET_DRAWN, 1)]
        );
    }

    const TARGET_MISSING: &str = "pdf_link_target_missing";
    const SCHEME_SKIPPED: &str = "pdf_link_scheme_skipped";
}
