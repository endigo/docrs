//! Markdown → DOCX: render Markdown into a document, styled by a reference
//! document (pandoc's `--reference-doc` model).
//!
//! The reference document supplies every style, the list and page setup,
//! headers and footers, the theme and the settings; the Markdown supplies
//! the body. Elements take styles by *role* ([`Role`]) — pandoc's style
//! names first, then Word's built-ins — so a pandoc reference document and
//! an ordinary Word template both work. A conversion is one transaction:
//! it commits completely or, on any refusal or error, not at all.
//!
//! Supported: CommonMark plus GitHub tables, task lists, strikethrough,
//! footnotes and alerts; definition lists, heading ids (`{#id}`),
//! super/subscript, inline HTML spans (`<u>`, `<sup>`, `<br>`, …), front
//! matter (title block and core properties), internal links to headings
//! (bookmarks), local and `data:` images, page breaks (`\newpage`,
//! `<!-- pagebreak -->`), a table of contents (`[TOC]` or `toc: true`) and
//! LaTeX math as native Word equations. Remote images are not fetched and
//! math commands outside the supported subset stay as text; both are
//! reported as warnings.
//!
//! Untrusted input: the source decides which images and links the output
//! carries, so the conversion confines both. Local images are read only
//! from [`MarkdownOptions::resource_dir`] (by default nothing is read from
//! disk), and no picture larger than the document's
//! [`Limits::max_part_bytes`](crate::Limits) is taken. Links keep their
//! target only for `http`, `https`, `mailto`, `tel`, `#fragment` and
//! relative references; any other scheme (`javascript:`, `file:`, …) keeps
//! the text without the link (`link_scheme_skipped`). Warnings quote the
//! source as written, never a resolved path or an operating system error.
//!
//! Editor HTML is a second source format ([`insert_html`]): the HTML subset
//! rich-text editors emit, including text and highlight colours, fonts,
//! paragraph and cell alignment, `colgroup` widths, `colspan`/`rowspan`,
//! picture sizes and alignment, and page-break markers. CSS without a Word
//! equivalent is reported (`html_style_dropped`).
//!
//! TipTap document JSON (`editor.getJSON()`) is a third source format
//! ([`insert_tiptap`]): the nodes and marks of Tiptap's own extensions
//! convert as the same document's HTML does, from the JSON directly, with
//! the editor's default alignment left to the reference document's styles.
//! Node, mark and attribute types with no Word equivalent are reported
//! (`tiptap_node_dropped`, `tiptap_mark_dropped`,
//! `tiptap_attribute_dropped`); `docs/tiptap.md` in the repository lists
//! every node and mark with what Word gets. [`inspect`] reports what a
//! conversion will take from a reference document.

mod ast;
mod frontmatter;
mod html;
mod inspect;
mod math;
mod render;
mod styles;
mod tiptap;

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use ast::MAX_NESTING_DEPTH;
pub use inspect::inspect;
pub use styles::{Role, RoleStyle, StylePolicy};

use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::Txn;
use crate::{Document, Error, RefusalKind, Result};

/// Where the converted body goes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Placement {
    /// Replace the reference document's body (its final section properties
    /// — page setup, headers, footers — stay).
    #[default]
    ReplaceBody,
    /// Append after the existing body content.
    Append,
    /// Replace the one top-level body paragraph whose text is exactly this
    /// marker (e.g. `{{content}}`), keeping everything around it.
    Placeholder(String),
}

/// Conversion options.
#[derive(Debug, Clone)]
pub struct MarkdownOptions {
    /// Where the body goes.
    pub placement: Placement,
    /// Missing reference styles: create or refuse.
    pub style_policy: StylePolicy,
    /// Explicit role → style-name assignments (the style must exist).
    pub style_map: BTreeMap<Role, String>,
    /// The one directory local images are read from; `None` (the default)
    /// reads nothing from the file system, so only `images` and `data:`
    /// URIs supply pictures. With a directory, a reference must be a
    /// relative path that stays inside it once `..` and symbolic links are
    /// resolved; absolute paths, `C:`/UNC prefixes and `file:` URLs are
    /// refused (`image_outside_resource_dir`). Point it only at a directory
    /// whose every picture the output may contain.
    pub resource_dir: Option<PathBuf>,
    /// Typographic quotes, dashes and ellipses.
    pub smart_punctuation: bool,
    /// Table of contents after the title block; `None` follows the front
    /// matter (`toc: true`).
    pub toc: Option<bool>,
    /// Heading levels the table of contents lists (1–9, default 3).
    pub toc_depth: Option<i64>,
    /// Title above the table of contents (default `Contents`, or the
    /// front matter's `toc-title`).
    pub toc_title: Option<String>,
    /// Added to every heading level (`-1` makes `##` a Heading 1).
    pub heading_offset: i8,
    /// Render the front matter title, subtitle, authors, date and abstract.
    pub title_block: bool,
    /// Copy front matter into the core document properties.
    pub set_properties: bool,
    /// Give a stand-alone image a caption paragraph from its alt text.
    pub figure_captions: bool,
    /// Refuse instead of warning (missing image, unsupported HTML, …).
    pub strict: bool,
    /// Lists take the reference document's `List Bullet` / `List Number`
    /// numbering (bullet glyphs, colours, indents) for the levels it
    /// defines; `false` always uses the converter's own definitions.
    pub template_lists: bool,
    /// Table of contents page numbers. They need page layout, so Word fills
    /// them in on open after asking to update fields; without them the
    /// contents is complete as written and opens without a prompt.
    pub toc_page_numbers: bool,
    /// Write page numbers into the contents, computed by estimating Word
    /// for Mac's layout (`crate::layout`), so they show without updating
    /// fields; Word's own update replaces them.
    pub toc_numbers: bool,
    /// Add a centred page number footer when the reference document's
    /// final section has no footer (an existing footer is kept as is).
    pub page_numbers: bool,
    /// The reference document's first page as a cover (replacing the body
    /// only). `None` detects one: a distinct first page whose header draws a
    /// graphic at least half the page tall, above a sample body that leaves
    /// that page blank; the content then starts on page 2. `Some(true)`
    /// starts the content on page 2 whenever the first page is distinct;
    /// `Some(false)` starts it on page 1 and turns a detected cover's
    /// distinct first page off, so page 1 takes the regular header.
    pub cover_page: Option<bool>,
    /// Image bytes by the path the Markdown writes (`chart.png`,
    /// `./images/logo.png`), consulted before the file system; for callers
    /// without one (WebAssembly) or with images held in memory.
    pub images: BTreeMap<String, Vec<u8>>,
    /// Creation time for a core-properties part the conversion has to add
    /// (front matter into a template without one); `None` reads the system
    /// clock, which WebAssembly does not have.
    pub now: Option<crate::clock::Timestamp>,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        MarkdownOptions {
            placement: Placement::ReplaceBody,
            style_policy: StylePolicy::Create,
            style_map: BTreeMap::new(),
            resource_dir: None,
            smart_punctuation: false,
            toc: None,
            toc_depth: None,
            toc_title: None,
            heading_offset: 0,
            title_block: true,
            set_properties: true,
            figure_captions: true,
            strict: false,
            template_lists: true,
            toc_page_numbers: false,
            toc_numbers: true,
            page_numbers: false,
            cover_page: None,
            images: BTreeMap::new(),
            now: None,
        }
    }
}

/// Something the conversion could not represent faithfully.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MarkdownWarning {
    /// Stable code (`image_not_found`, `raw_html_dropped`, …).
    pub code: &'static str,
    /// What happened.
    pub message: String,
    /// 1-based Markdown line, when known.
    pub line: Option<usize>,
}

/// Element counts of a conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MarkdownCounts {
    /// Headings.
    pub headings: usize,
    /// Paragraphs of every kind (list items and cell paragraphs included).
    pub paragraphs: usize,
    /// List items.
    pub list_items: usize,
    /// Tables.
    pub tables: usize,
    /// Pictures.
    pub images: usize,
    /// Footnotes.
    pub footnotes: usize,
    /// Hyperlinks (external and internal).
    pub links: usize,
    /// Code blocks.
    pub code_blocks: usize,
    /// Equations (inline and display math).
    pub equations: usize,
}

/// What a conversion did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MarkdownReport {
    /// Element counts.
    pub counts: MarkdownCounts,
    /// Role → style used (keyed by role key).
    pub styles: BTreeMap<String, RoleStyle>,
    /// Styles the conversion added to the reference document.
    pub created_styles: Vec<String>,
    /// Package parts written.
    pub changed_parts: Vec<String>,
    /// Template parts left out of the output: sample-body pictures,
    /// embedded files, comments, the package thumbnail (a picture of the
    /// template's first page) and anything else nothing kept refers to.
    pub removed_parts: Vec<String>,
    /// Template document properties left out of the output (`core:creator`,
    /// `app:Pages`, `custom:<name>`, ...): the template's people, statistics
    /// and metadata.
    pub cleared_properties: Vec<String>,
    /// Page numbers: `footer_added`, `template_footer_kept` or
    /// `no_section` when requested; absent otherwise.
    pub page_numbers: Option<String>,
    /// Cover page: `content_after_cover` (the content starts on page 2) or
    /// `cover_removed` (the distinct first page was turned off); absent when
    /// neither applied.
    pub cover_page: Option<String>,
    /// The contents' computed page numbers; absent without a contents (or
    /// with `toc_numbers` off).
    pub toc: Option<TocReport>,
    /// Losses and substitutions.
    pub warnings: Vec<MarkdownWarning>,
}

/// How the contents' page numbers were computed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TocReport {
    /// Entries numbered.
    pub entries: usize,
    /// Estimated page count.
    pub pages: u32,
    /// Fonts with no metrics, laid out with a substitute (each also a
    /// `font_metrics_missing` warning).
    pub missing_fonts: Vec<String>,
    /// Fonts measured from the template's embedded copies.
    pub embedded_fonts: Vec<String>,
}

/// Every warning code, with a fixed description that quotes nothing from
/// the input ([`MarkdownReport::redact`]).
pub const WARNING_CODES: &[(&str, &str)] = &[
    (
        "broken_internal_link",
        "an internal link matches no heading; its text is kept without a link",
    ),
    (
        "control_characters_dropped",
        "characters XML cannot carry (control characters other than tab and line breaks, U+FFFE, U+FFFF) were dropped from the text",
    ),
    (
        "font_metrics_missing",
        "a font has no metrics; contents page numbers are estimated with a substitute",
    ),
    (
        "html_element_skipped",
        "an HTML element browsers do not display (script, style, head, …) was dropped with its content",
    ),
    (
        "html_style_dropped",
        "a CSS declaration has no Word equivalent and was ignored",
    ),
    (
        "image_not_found",
        "an image was not supplied or could not be read; its alt text is shown instead",
    ),
    (
        "image_outside_resource_dir",
        "an image path leads outside the resource directory and was not read; its alt text is shown instead",
    ),
    (
        "image_too_large",
        "an image is larger than the part size limit; its alt text is shown instead",
    ),
    (
        "image_unreadable",
        "an image is not valid base64 or has a damaged header; its alt text is shown instead",
    ),
    (
        "image_unsupported",
        "an image is not a BMP, GIF, JPEG, PNG or TIFF file; its alt text is shown instead",
    ),
    (
        "link_scheme_skipped",
        "a link uses a scheme other than http, https, mailto or tel; its text is kept without a link",
    ),
    (
        "math_unknown_command",
        "a math command outside the supported subset is kept as text",
    ),
    (
        "nested_footnote",
        "a footnote inside a footnote is kept as text",
    ),
    (
        "raw_html_dropped",
        "an HTML element has no Word equivalent; its text is kept",
    ),
    (
        "remote_image_skipped",
        "a remote image is not downloaded; its alt text is shown instead",
    ),
    (
        "table_columns_clamped",
        "a table is wider than Word's 63 columns; it was narrowed to fit",
    ),
    (
        "template_property_cleared",
        "a field in the template shows a document property that was cleared; it will show empty",
    ),
    (
        "tiptap_attribute_dropped",
        "a TipTap node or mark attribute has no Word equivalent and was ignored",
    ),
    (
        "tiptap_mark_dropped",
        "a TipTap mark type has no Word equivalent; its text is kept without it",
    ),
    (
        "tiptap_node_dropped",
        "a TipTap node type has no Word equivalent; its text is kept",
    ),
    (
        "toc_outside_body",
        "a table of contents marker inside a footnote was dropped",
    ),
    (
        "undefined_footnote",
        "a footnote reference has no definition and is kept as text",
    ),
];

impl MarkdownReport {
    /// Replace every warning message with its code's fixed description, so
    /// the report holds nothing from the input (no text, URLs, names). The
    /// code and line stay.
    pub fn redact(&mut self) {
        for warning in &mut self.warnings {
            warning.message = WARNING_CODES
                .iter()
                .find(|(code, _)| *code == warning.code)
                .map_or("the conversion lost something", |(_, text)| text)
                .to_string();
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
}

/// The source format of a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputFormat {
    /// CommonMark with the extensions listed in the module docs.
    #[default]
    Markdown,
    /// HTML as rich-text editors emit it (see the module docs).
    Html,
    /// TipTap document JSON, `editor.getJSON()` (see the module docs).
    Tiptap,
}

/// Render `markdown` into `document` (see the module docs). Atomic.
pub fn insert_markdown(
    document: &mut Document,
    markdown: &str,
    options: &MarkdownOptions,
) -> Result<MarkdownReport> {
    insert_with(document, markdown, InputFormat::Markdown, options, false)
}

/// Render editor HTML into `document`: the same conversion as
/// [`insert_markdown`] with HTML as the source. Atomic.
pub fn insert_html(
    document: &mut Document,
    html: &str,
    options: &MarkdownOptions,
) -> Result<MarkdownReport> {
    insert_with(document, html, InputFormat::Html, options, false)
}

/// Render a TipTap document (`editor.getJSON()` as JSON text) into
/// `document`: the same conversion as [`insert_markdown`] with the editor's
/// document as the source (the node-by-node table is `docs/tiptap.md` in
/// the repository). Node, mark and attribute types with no Word equivalent
/// are reported as `tiptap_node_dropped`, `tiptap_mark_dropped` and
/// `tiptap_attribute_dropped` with the text kept; a source that is not a
/// `doc` node is refused as `tiptap_json_invalid`. Atomic.
pub fn insert_tiptap(
    document: &mut Document,
    json: &str,
    options: &MarkdownOptions,
) -> Result<MarkdownReport> {
    insert_with(document, json, InputFormat::Tiptap, options, false)
}

/// Render `source` in `format` into `document`. Atomic.
pub fn insert_source(
    document: &mut Document,
    source: &str,
    format: InputFormat,
    options: &MarkdownOptions,
) -> Result<MarkdownReport> {
    insert_with(document, source, format, options, false)
}

fn insert_with(
    document: &mut Document,
    markdown: &str,
    format: InputFormat,
    options: &MarkdownOptions,
    prefer_own_styles: bool,
) -> Result<MarkdownReport> {
    if let Some(depth) = options.toc_depth
        && !(1..=9).contains(&depth)
    {
        return Err(Error::invalid_argument(
            "invalid_toc_depth",
            format!("the table of contents depth must be 1-9, got {depth}"),
        ));
    }
    if let Placement::Placeholder(marker) = &options.placement
        && marker.trim().is_empty()
    {
        return Err(Error::invalid_argument(
            "empty_placeholder",
            "the placeholder marker must contain text",
        ));
    }
    refuse_if_protected(
        document,
        "convert Markdown into",
        OperationClass::BodyContent,
    )?;
    let mut parsed = match format {
        InputFormat::Markdown => ast::parse(markdown, options.smart_punctuation),
        InputFormat::Html => ast::parse_html(markdown),
        InputFormat::Tiptap => tiptap::parse(markdown),
    }?;
    // Word refuses a file holding a character XML cannot carry; the source
    // loses it, not the conversion.
    let dropped = ast::drop_illegal_chars(&mut parsed);
    if dropped > 0 {
        ast::warn_once(
            &mut parsed.warnings,
            "control_characters_dropped",
            format!(
                "{dropped} character{} XML cannot carry (control characters other than tab and line breaks, U+FFFE, U+FFFF) dropped from the text",
                if dropped == 1 { "" } else { "s" }
            ),
            None,
        );
    }
    let metadata = parsed
        .metadata
        .as_ref()
        .map(|(kind, text)| frontmatter::metadata(*kind, text))
        .unwrap_or_default();
    let mut report = MarkdownReport::default();
    report.warnings.extend(parsed.warnings.iter().cloned());
    let staged = {
        let doc: &Document = document;
        let mut txn = Txn::new(doc);
        render::render(
            &mut txn,
            &parsed,
            &metadata,
            options,
            prefer_own_styles,
            &mut report,
        )?;
        if options.strict && !report.warnings.is_empty() {
            let summary: Vec<String> = report
                .warnings
                .iter()
                .map(|w| match w.line {
                    Some(line) => format!("line {line}: {}", w.message),
                    None => w.message.clone(),
                })
                .collect();
            return Err(Error::refusal(
                RefusalKind::UnsupportedStructure,
                "markdown_not_faithful",
                format!(
                    "strict conversion refused; nothing was changed: {}",
                    summary.join("; ")
                ),
            ));
        }
        txn.finish()?
    };
    report.changed_parts = staged
        .touched_parts()
        .into_iter()
        .map(str::to_string)
        .collect();
    report.removed_parts = staged
        .removed_parts()
        .into_iter()
        .map(str::to_string)
        .collect();
    document.commit(staged)?;
    Ok(report)
}

/// Convert `markdown` with `reference` (or the bundled blank document)
/// supplying the styles.
pub fn convert(
    markdown: &str,
    reference: Option<Document>,
    options: &MarkdownOptions,
) -> Result<(Document, MarkdownReport)> {
    let mut document = reference.unwrap_or_default();
    let report = insert_markdown(&mut document, markdown, options)?;
    Ok((document, report))
}

/// Markdown exercising every style role; converting it into the bundled
/// blank document gives a starter reference document whose styles can be
/// restyled in Word (pandoc's `reference.docx`).
pub const REFERENCE_SAMPLE: &str = include_str!("reference_sample.md");

/// A starter reference document: the bundled blank document with every
/// role's style present and demonstrated.
pub fn reference_document() -> Result<Document> {
    // Stamped with the bundled blank document's own date: the starter is
    // the same every time, and WebAssembly has no clock.
    let options = MarkdownOptions {
        toc: Some(true),
        now: Some(crate::clock::Timestamp::from_utc(2013, 12, 23, 23, 15, 0)?),
        ..MarkdownOptions::default()
    };
    let mut document = Document::new();
    insert_with(
        &mut document,
        REFERENCE_SAMPLE,
        InputFormat::Markdown,
        &options,
        true,
    )?;
    Ok(document)
}

/// The style each role would take from `document` (`None`: missing, so a
/// conversion would add it, or refuse under [`StylePolicy::Strict`]).
/// Read-only: nothing is created.
pub fn role_styles(
    document: &Document,
    style_map: &BTreeMap<Role, String>,
) -> Result<Vec<(Role, Option<RoleStyle>)>> {
    let mut txn = Txn::new(document);
    let mut resolver = styles::Resolver::new(style_map.clone(), StylePolicy::Strict);
    let mut out = Vec::new();
    for role in Role::ALL {
        resolver.id(&mut txn, role)?;
        out.push((role, resolver.resolved.get(&role).cloned()));
    }
    Ok(out)
}

impl Document {
    /// See [`insert_markdown`].
    pub fn insert_markdown(
        &mut self,
        markdown: &str,
        options: &MarkdownOptions,
    ) -> Result<MarkdownReport> {
        insert_markdown(self, markdown, options)
    }
}
