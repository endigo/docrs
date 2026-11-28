//! PDF export. **Experimental**: exempt from semantic versioning until the
//! release that completes it. Output is deterministic for a given build.
//!
//! [`preflight`] reports what a document holds that a PDF would not show
//! faithfully. While the engine is incomplete, every construct it does not
//! draw yet is a `pdf_not_yet_drawn` warning, and [`PdfReport::check`]
//! refuses it, so a PDF is never silently incomplete.

#![cfg_attr(not(feature = "test-support"), allow(dead_code))]

mod font;
mod meta;
mod preflight;
mod writer;

pub use preflight::preflight;
#[cfg(feature = "test-support")]
pub use preflight::preflight_steps;

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use font::{SampleLine, Style};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use meta::Info;

use serde::Serialize;
use writer::{Dict, Object, Writer, count, name};

use crate::{Error, ErrorDetails, Limits, Result, WarningCount};

/// The code of a construct the engine does not draw yet.
pub const NOT_YET_DRAWN: &str = "pdf_not_yet_drawn";

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
        "pdf_equation_linearized",
        "an equation is drawn as a line of text",
    ),
    (
        "pdf_image_unsupported",
        "a TIFF, EMF or WMF image is left out of the PDF",
    ),
    (
        "pdf_text_unsupported",
        "text that needs more than Latin script (right-to-left, complex scripts, CJK, ruby, combining marks, emoji sequences) may be drawn wrongly",
    ),
    (
        "pdf_layout_feature_ignored",
        "a layout feature (page borders, line numbers, frames, drop caps, floating tables, rotated text, vertical page alignment, automatic hyphenation) is ignored",
    ),
    (
        "pdf_revisions_flattened",
        "tracked changes and comments are not shown: the PDF shows the text as if every change were accepted",
    ),
    (
        "pdf_field_cached",
        "a date or time field shows the value saved in the document, not the current one",
    ),
    (
        "pdf_link_scheme_skipped",
        "a link uses a scheme other than http, https, mailto, tel or a bookmark; its text is kept without the link",
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

/// What a PDF of the document would leave out or change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PdfReport {
    /// In order of first occurrence.
    pub warnings: Vec<PdfWarning>,
}

impl PdfReport {
    /// Replace every message with its code's fixed description and every
    /// location's part name with its story kind (`body`, `header`,
    /// `footer`, `footnotes`, `endnotes`), so the report holds nothing from
    /// the input.
    pub fn redact(&mut self) {
        for warning in &mut self.warnings {
            warning.message = description(warning.code).to_string();
            for location in &mut warning.locations {
                location.story = location.kind.to_string();
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

    /// Refuse a construct not drawn yet (`pdf_not_yet_drawn`) always, and
    /// with `strict` any warning (`pdf_not_faithful`). The refusal's
    /// details count each code.
    pub fn check(&self, strict: bool) -> Result<()> {
        let missing: Vec<&str> = self
            .warnings
            .iter()
            .filter(|w| w.code == NOT_YET_DRAWN)
            .map(|w| w.subject)
            .collect();
        let error = if !missing.is_empty() {
            Error::unsupported(
                NOT_YET_DRAWN,
                format!("the PDF engine does not draw {} yet", missing.join(", ")),
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
        ErrorDetails { warnings }
    }
}

/// Page side limits in points (ISO 32000-1 Annex C).
const MIN_PAGE_SIDE: f64 = 3.0;
const MAX_PAGE_SIDE: f64 = 14_400.0;

/// A file of empty pages, `(width, height)` in points.
pub(crate) fn blank_document(
    pages: &[(f64, f64)],
    info: &meta::Info,
    producer: &str,
    limits: &Limits,
) -> Result<Vec<u8>> {
    check_pages(pages)?;
    let pages: Vec<((f64, f64), Vec<u8>)> = pages.iter().map(|&size| (size, Vec::new())).collect();
    let writer = Writer::new(limits.max_output_bytes)?;
    write_document(writer, &pages, Dict::new(), info, producer)
}

/// Writes the pages, each `(size, content stream)`, sharing `resources`,
/// then the catalog, the `/Info` and the trailer.
fn write_document(
    mut writer: Writer,
    pages: &[((f64, f64), Vec<u8>)],
    resources: Dict,
    info: &meta::Info,
    producer: &str,
) -> Result<Vec<u8>> {
    let catalog = writer.alloc()?;
    let tree = writer.alloc()?;
    let mut kids = Vec::with_capacity(pages.len());
    for ((width, height), content) in pages {
        let page = writer.alloc()?;
        let contents = writer.alloc()?;
        writer.stream(contents, Dict::new(), content)?;
        let media_box: Vec<Object> =
            vec![0.0.into(), 0.0.into(), (*width).into(), (*height).into()];
        writer.dict(
            page,
            Dict::new()
                .with("Type", name("Page"))
                .with("Parent", tree)
                .with("MediaBox", media_box)
                .with("Contents", contents),
        )?;
        kids.push(page.into());
    }
    writer.dict(
        tree,
        Dict::new()
            .with("Type", name("Pages"))
            .with("Count", count(pages.len())?)
            .with("Kids", kids)
            .with("Resources", resources),
    )?;
    writer.dict(
        catalog,
        Dict::new()
            .with("Type", name("Catalog"))
            .with("Pages", tree),
    )?;
    let info_ref = writer.alloc()?;
    writer.dict(info_ref, info.dict(producer)?)?;
    let id = meta::file_id(writer.body());
    writer.finish(
        Dict::new()
            .with("Root", catalog)
            .with("Info", info_ref)
            .with("ID", id),
    )
}

fn check_pages(pages: &[(f64, f64)]) -> Result<()> {
    if pages.is_empty() {
        return Err(Error::invalid_argument(
            "pdf_no_pages",
            "a PDF needs at least one page",
        ));
    }
    for &(width, height) in pages {
        for side in [width, height] {
            if !(MIN_PAGE_SIDE..=MAX_PAGE_SIDE).contains(&side) {
                return Err(Error::invalid_argument(
                    "pdf_page_size",
                    format!(
                        "page {width} × {height} pt: each side must be {MIN_PAGE_SIDE} to {MAX_PAGE_SIDE} pt"
                    ),
                ));
            }
        }
    }
    Ok(())
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
    blank_document(pages, info, producer, limits)
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

/// [`font::glyphs_by_cmap`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn glyphs_for_tests(font: &[u8], index: u32, text: &str) -> Result<Vec<(u16, String)>> {
    font::glyphs_by_cmap(font, index, text)
}
