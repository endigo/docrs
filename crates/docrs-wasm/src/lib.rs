//! WebAssembly bindings of `docrs` for Node.js: Markdown, editor HTML or
//! TipTap JSON to DOCX with a reference document, the same conversion as
//! the CLI's `from-markdown`, `from-html` and `from-json`, and the reference
//! document inspection of `markdown-inspect`.
//!
//! The conversion itself ([`convert`]) is plain Rust and tested natively;
//! the `#[wasm_bindgen]` functions only translate JavaScript values. Bytes
//! cross as `Uint8Array`; reports cross as plain objects shaped like the
//! CLI's JSON. Nothing touches a file system: the reference document and
//! images are passed in, and the time comes from JavaScript's clock.

mod fonts;
mod pdf;

use std::collections::BTreeMap;

use docrs::clock::Timestamp;
use docrs::markdown::{self, InputFormat, MarkdownOptions, Placement, Role, StylePolicy};
use docrs::pdf::PdfLimits;
use docrs::replace::ReplaceAllOptions;
use docrs::{Document, Error, Limits, Tolerance};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use wasm_bindgen::prelude::*;

#[cfg(test)]
use fonts::FontOptions;
use fonts::{FontLimits, LoadedFonts};
use pdf::{Pdf, PdfChoice};

/// Options of [`from_markdown`], camelCase in JavaScript. Every field is
/// optional; the defaults match the CLI's.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Options {
    /// Reference document (`.docx` bytes) supplying styles, lists, page
    /// setup, headers and footers; the bundled blank document when absent.
    pub reference: Option<ByteBuf>,
    /// Replace only the body paragraph reading exactly this text.
    pub placeholder: Option<String>,
    /// Append after the reference document's body instead of replacing it.
    pub append: bool,
    /// Role key → existing style name (`{ code_block: "My Code" }`).
    pub styles: BTreeMap<String, String>,
    /// Refuse when the reference document lacks a style.
    pub strict_styles: bool,
    /// Refuse instead of warning when something cannot be converted.
    pub strict: bool,
    /// Table of contents (`None` follows the front matter).
    pub toc: Option<bool>,
    pub toc_depth: Option<i64>,
    pub toc_title: Option<String>,
    /// Ask Word to update the table of contents on open (it prompts).
    pub toc_page_numbers: bool,
    /// Computed page numbers in the table of contents (default true).
    pub toc_numbers: Option<bool>,
    /// Added to every heading level (`-1` makes `##` a Heading 1).
    pub shift_heading_level: i8,
    /// Typographic quotes, dashes and ellipses.
    pub smart: bool,
    /// Render the front-matter title block (default true).
    pub title_block: Option<bool>,
    /// Copy front matter into the document properties (default true).
    pub properties: Option<bool>,
    /// Caption stand-alone images with their alt text (default true).
    pub captions: Option<bool>,
    /// Use the reference's own list numbering (default true).
    pub template_lists: Option<bool>,
    /// Add a page-number footer when the reference has none.
    pub page_numbers: bool,
    /// Cover page handling (`None` detects one).
    pub cover_page: Option<bool>,
    /// Image bytes by the path the Markdown writes.
    pub images: BTreeMap<String, ByteBuf>,
    /// Refuse a reference with irregularities Word tolerates instead of
    /// setting them aside.
    pub strict_package: bool,
    /// Proceed despite Restrict Editing on the reference.
    pub acknowledge_protection: bool,
    /// Time for any document property the conversion creates, in
    /// milliseconds since the Unix epoch (default: now); finite, from 0 to
    /// the end of the year 9999.
    pub now: Option<f64>,
    /// Resource budgets: `default`, or `serverless` for a 256 MB function.
    pub limits: Option<String>,
    /// Warnings and errors carry no text taken from the input.
    pub redact: bool,
    /// `{{Name}}` merge fields to fill once converted: name → value. A
    /// field is `{{`, the name (spaces around it allowed) and `}}`, or the
    /// same in triple braces, in any story (body, headers, footers, text
    /// boxes); fields not named here are left as they are (give `""` to
    /// blank one). A value is text within one paragraph: no line breaks,
    /// tabs or other control characters, and no `{{` or `}}`.
    pub fields: BTreeMap<String, String>,
    /// Also draw the converted document as a PDF (experimental).
    pub pdf: Option<PdfChoice>,
    /// Return the `.docx` too (default true); `false` needs `pdf`.
    pub docx: Option<bool>,
    /// Fonts to measure and draw with, from the registry id JavaScript passes as
    /// `fonts` (see [`options_from`]).
    #[serde(skip)]
    pub fonts: LoadedFonts,
}

/// Budgets on what the caller passes in, beyond the package budgets.
struct InputLimits {
    /// Markdown, HTML or JSON source, in bytes.
    source: u64,
    /// The reference document and all supplied images together, in bytes.
    input: u64,
    /// Text the merge fields add, every occurrence counted, in bytes.
    fields: u64,
    fonts: FontLimits,
    pdf: PdfLimits,
}

fn limits_of(options: &Options) -> Result<(Limits, InputLimits), Error> {
    profile(options.limits.as_deref())
}

fn profile(name: Option<&str>) -> Result<(Limits, InputLimits), Error> {
    match name {
        None | Some("default") => Ok((
            Limits::default(),
            InputLimits {
                source: u64::MAX,
                input: u64::MAX,
                fields: u64::MAX,
                fonts: FontLimits::DEFAULT,
                pdf: PdfLimits::default(),
            },
        )),
        Some("serverless") => Ok((
            Limits::serverless(),
            // About 190 pages of editor HTML. Measured peaks in Node 24:
            // 384 KiB of table-heavy HTML with 12 MiB of pictures, or with
            // an 8 MiB template, about 190 MB of process memory.
            InputLimits {
                source: 384 << 10,
                input: 12 << 20,
                fields: 384 << 10,
                fonts: FontLimits::SERVERLESS,
                pdf: PdfLimits::serverless(),
            },
        )),
        Some(other) => Err(Error::invalid_argument(
            "invalid_options",
            format!("limits must be \"default\" or \"serverless\", got {other:?}"),
        )),
    }
}

/// Refuse inputs over budget by their sizes alone, before anything reads
/// them: the source text, the reference document (also bounded by the
/// package's `max_archive_bytes`, as opening it would be) and the reference
/// and images together. The JavaScript entry points call this with the
/// lengths of the JavaScript values, before copying them into WebAssembly
/// memory; the Rust functions call it again with the exact sizes.
fn check_sizes(
    limits: &Limits,
    input: &InputLimits,
    source: u64,
    reference: u64,
    images: u64,
) -> Result<(), Error> {
    if source > input.source {
        return Err(Error::limit("max_source_bytes", input.source, source));
    }
    if reference > limits.max_archive_bytes {
        return Err(Error::limit(
            "max_archive_bytes",
            limits.max_archive_bytes,
            reference,
        ));
    }
    let total = reference.saturating_add(images);
    if total > input.input {
        return Err(Error::limit("max_input_bytes", input.input, total));
    }
    Ok(())
}

/// The latest time `now` may give: the end of the year 9999, in
/// milliseconds since the Unix epoch.
const MAX_NOW_MILLIS: f64 = 253_402_300_799_999.0;

/// The time of a conversion from JavaScript milliseconds since the Unix
/// epoch: a finite number from 0 (1970) to the end of the year 9999.
pub fn timestamp_from_millis(millis: f64) -> Result<Timestamp, Error> {
    if !millis.is_finite() || !(0.0..=MAX_NOW_MILLIS).contains(&millis) {
        return Err(Error::invalid_argument(
            "invalid_options",
            format!(
                "now must be milliseconds since the Unix epoch, from 0 to {MAX_NOW_MILLIS} \
                 (the end of 9999), got {millis}"
            ),
        ));
    }
    Ok(Timestamp::from_unix((millis / 1000.0).floor() as i64))
}

/// A finished conversion.
#[derive(Debug)]
pub struct Converted {
    /// The `.docx` package, unless `docx: false`.
    pub docx: Option<Vec<u8>>,
    pub pdf: Option<Pdf>,
    /// The conversion report (the CLI's `markdown` object).
    pub report: serde_json::Value,
    /// What was set aside to read the reference document.
    pub reference_set_aside: Vec<String>,
}

fn style_map(styles: &BTreeMap<String, String>) -> Result<BTreeMap<Role, String>, Error> {
    styles
        .iter()
        .map(|(role, name)| Ok((Role::from_key(role.trim())?, name.clone())))
        .collect()
}

fn open_reference(
    reference: Option<&[u8]>,
    strict_package: bool,
    limits: &Limits,
) -> Result<(Document, Vec<String>), Error> {
    let tolerance = if strict_package {
        Tolerance::default()
    } else {
        Tolerance::word()
    };
    match reference {
        Some(bytes) => Document::open_bytes_with(bytes, limits, tolerance),
        None => Ok((Document::new(), Vec::new())),
    }
}

fn markdown_options(
    options: &Options,
    format: InputFormat,
    now: Timestamp,
) -> Result<MarkdownOptions, Error> {
    Ok(MarkdownOptions {
        placement: match (&options.placeholder, options.append) {
            (Some(marker), _) => Placement::Placeholder(marker.clone()),
            (None, true) => Placement::Append,
            (None, false) => Placement::ReplaceBody,
        },
        style_policy: if options.strict_styles {
            StylePolicy::Strict
        } else {
            StylePolicy::Create
        },
        style_map: style_map(&options.styles)?,
        resource_dir: None,
        smart_punctuation: options.smart,
        toc: options.toc,
        toc_depth: options.toc_depth,
        toc_title: options.toc_title.clone(),
        heading_offset: options.shift_heading_level,
        title_block: options.title_block.unwrap_or(true),
        set_properties: options.properties.unwrap_or(true),
        // Editor HTML has no figure captions.
        figure_captions: options.captions.unwrap_or(format == InputFormat::Markdown),
        strict: options.strict,
        template_lists: options.template_lists.unwrap_or(true),
        toc_page_numbers: options.toc_page_numbers,
        toc_numbers: options.toc_numbers.unwrap_or(true),
        page_numbers: options.page_numbers,
        cover_page: options.cover_page,
        // Moved in by `convert`, not copied.
        images: BTreeMap::new(),
        now: Some(now),
    })
}

/// A converted document, before it is written.
pub struct Built {
    pub document: Document,
    pub report: markdown::MarkdownReport,
    /// What `fields` filled in (the report's `fields`).
    pub fields: Option<serde_json::Value>,
    pub reference_set_aside: Vec<String>,
}

/// Convert `source` into the reference document of `options`, filling its
/// fields; takes the template, images and fonts out of `options`.
pub fn build(
    source: &str,
    format: InputFormat,
    options: &mut Options,
    now: Timestamp,
) -> Result<Built, Error> {
    let (limits, input) = limits_of(options)?;
    check_sizes(
        &limits,
        &input,
        source.len() as u64,
        options.reference.as_ref().map_or(0, |b| b.len() as u64),
        options.images.values().map(|b| b.len() as u64).sum(),
    )?;
    options.fonts.check(&input.fonts)?;
    check_fields(&options.fields)?;
    let fonts = &options.fonts.set;
    let mut markdown_options = markdown_options(options, format, now)?;
    markdown_options.images = std::mem::take(&mut options.images)
        .into_iter()
        .map(|(path, bytes)| (path, bytes.into_vec()))
        .collect();
    let reference = options.reference.take();
    let (mut document, reference_set_aside) = open_reference(
        reference.as_ref().map(|b| b.as_slice()),
        options.strict_package,
        &limits,
    )?;
    drop(reference);
    if options.acknowledge_protection {
        document.acknowledge_protection()?;
    }
    let mut report =
        markdown::insert_source_with(&mut document, source, format, &markdown_options, fonts)?;
    drop(markdown_options);
    let fields = fill_fields(&mut document, &options.fields, input.fields)?;
    let filled = fields
        .as_ref()
        .and_then(|fields| fields["filled"].as_u64())
        .unwrap_or(0);
    if let (true, Some(toc)) = (filled > 0, report.toc.as_mut()) {
        let refresh =
            docrs::layout::refresh_toc_page_numbers_if_settled_with(&mut document, true, fonts)?;
        match refresh {
            Some(refresh) => {
                toc.pages = refresh.layout.pages;
                toc.missing_fonts = refresh.layout.missing_fonts;
                toc.embedded_fonts = refresh.layout.embedded_fonts;
            }
            None => report.warnings.push(toc_page_numbers_skipped()),
        }
    }
    if options.redact {
        report.redact();
    }
    Ok(Built {
        document,
        report,
        fields,
        reference_set_aside,
    })
}

/// Convert `source` (Markdown, editor HTML or TipTap JSON, by `format`) with `options`
/// (see [`Options`]); `now` stamps any document property the conversion
/// has to create. Atomic: a refusal returns the error and produces
/// nothing.
///
/// Takes the options by value: the template and image bytes are moved, not
/// copied, and each input is released once it is no longer needed, so a
/// conversion holds as few copies of large inputs as it can (WebAssembly
/// memory never shrinks, so the peak is what an instance keeps).
pub fn convert(
    source: &str,
    format: InputFormat,
    mut options: Options,
    now: Timestamp,
) -> Result<Converted, Error> {
    let pdf = options.pdf.as_ref().and_then(PdfChoice::settings).cloned();
    let docx = options.docx.unwrap_or(true);
    if !docx && pdf.is_none() {
        return Err(Error::invalid_argument(
            "invalid_options",
            "docx: false needs pdf",
        ));
    }
    let pdf_limits = limits_of(&options)?.1.pdf;
    let built = build(source, format, &mut options, now)?;
    let pdf = pdf
        .map(|settings| {
            pdf::render(
                &built.document,
                &settings,
                options.strict,
                &options.fonts.set,
                pdf_limits,
                options.redact,
            )
        })
        .transpose()?;
    let docx = if docx {
        Some(built.document.patch_bytes()?.0)
    } else {
        None
    };
    drop(built.document);
    let mut report = built.report.to_json();
    if let Some(object) = report.as_object_mut() {
        object.insert("fields".into(), built.fields.unwrap_or_default());
    }
    Ok(Converted {
        docx,
        pdf,
        report,
        reference_set_aside: built.reference_set_aside,
    })
}

/// The contents' page numbers did not settle after `fields` were filled:
/// they keep the conversion's, and Word updates them.
fn toc_page_numbers_skipped() -> markdown::MarkdownWarning {
    let code = "toc_page_numbers_skipped";
    let (_, message) = markdown::WARNING_CODES
        .iter()
        .find(|(known, _)| *known == code)
        .expect("a markdown warning code");
    markdown::MarkdownWarning {
        code,
        message: message.to_string(),
        line: None,
    }
}

/// Refuse a field value that would not fill as text within a paragraph,
/// whether or not the document holds that field.
fn check_fields(fields: &BTreeMap<String, String>) -> Result<(), Error> {
    for (name, value) in fields {
        if value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}'))
        {
            return Err(Error::invalid_argument(
                "invalid_options",
                format!(
                    "fields[{name:?}] has a line break, tab or other control character: \
                     a field fills text within one paragraph"
                ),
            ));
        }
        if value.contains("{{") || value.contains("}}") {
            return Err(Error::invalid_argument(
                "invalid_options",
                format!("fields[{name:?}] has {{{{ or }}}}, which would read as part of a field"),
            ));
        }
    }
    Ok(())
}

/// A distinct `{{Name}}` field text in the document.
struct FieldToken {
    token: String,
    name: String,
    count: u64,
}

/// The `{{Name}}` and `{{{Name}}}` fields in the document's text, in
/// document order.
fn field_tokens(document: &Document) -> Result<Vec<FieldToken>, Error> {
    let mut tokens: Vec<FieldToken> = Vec::new();
    for block in docrs::story::iter_blocks(document, docrs::story::View::Current)? {
        let mut rest = block.text.as_str();
        while let Some(found) = rest.find("{{") {
            let mut start = found + rest[found + 2..].bytes().take_while(|&b| b == b'{').count();
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            let inner = &after[..end];
            if inner.contains("{{") {
                rest = after;
                continue;
            }
            let mut stop = start + 2 + end + 2;
            if start > found && rest[stop..].starts_with('}') {
                start -= 1;
                stop += 1;
            }
            let token = &rest[start..stop];
            let name = inner.trim();
            if !name.is_empty() {
                match tokens.iter_mut().find(|known| known.token == token) {
                    Some(known) => known.count += 1,
                    None => tokens.push(FieldToken {
                        token: token.to_string(),
                        name: name.to_string(),
                        count: 1,
                    }),
                }
            }
            rest = &rest[stop..];
        }
    }
    Ok(tokens)
}

/// Fill `fields` with the core's untracked replace-all; `None` when no
/// fields were given. Refuses before filling when the text added would
/// exceed `budget`. Reports how many replacements were made, the field
/// names left in the document, and any a replacement refused (with the
/// core's code).
fn fill_fields(
    document: &mut Document,
    fields: &BTreeMap<String, String>,
    budget: u64,
) -> Result<Option<serde_json::Value>, Error> {
    if fields.is_empty() {
        return Ok(None);
    }
    let tokens = field_tokens(document)?;
    let added = tokens
        .iter()
        .filter_map(|field| {
            let value = fields.get(&field.name)?;
            Some(field.count.saturating_mul(value.len() as u64))
        })
        .fold(0u64, u64::saturating_add);
    if added > budget {
        return Err(Error::limit("max_field_bytes", budget, added));
    }
    let mut filled = 0;
    let mut left = Vec::new();
    let mut refused = Vec::new();
    for FieldToken { token, name, .. } in tokens {
        let Some(value) = fields.get(&name) else {
            if !left.contains(&name) {
                left.push(name);
            }
            continue;
        };
        let replaced = document.replace_all(&token, value, &ReplaceAllOptions::default())?;
        filled += replaced.replaced_count;
        refused.extend(replaced.refused.into_iter().map(|refusal| {
            serde_json::json!({ "name": name, "code": refusal.code, "reason": refusal.reason })
        }));
    }
    Ok(Some(serde_json::json!({
        "filled": filled,
        "left": left,
        "refused": refused,
    })))
}

/// What a conversion with `options` will take from `reference` (see
/// `markdown::inspect`).
pub fn inspect(reference: &[u8], options: &Options) -> Result<serde_json::Value, Error> {
    let (limits, input) = limits_of(options)?;
    check_sizes(&limits, &input, 0, reference.len() as u64, 0)?;
    let (document, set_aside) = open_reference(Some(reference), options.strict_package, &limits)?;
    let markdown_options =
        markdown_options(options, InputFormat::Markdown, Timestamp::from_unix(0))?;
    let mut inspection = markdown::inspect(&document, &markdown_options)?;
    inspection["reference_set_aside"] = serde_json::json!(set_aside);
    Ok(inspection)
}

/// `(role, candidates, style)` for every Markdown role in `reference`, with
/// `options.styles` (`limits` and `strict_package` apply too).
pub fn styles_of(reference: Option<&[u8]>, options: &Options) -> Result<serde_json::Value, Error> {
    let (limits, input) = limits_of(options)?;
    check_sizes(
        &limits,
        &input,
        0,
        reference.map_or(0, |b| b.len() as u64),
        0,
    )?;
    let (document, _) = open_reference(reference, options.strict_package, &limits)?;
    let roles = markdown::role_styles(&document, &style_map(&options.styles)?)?;
    Ok(serde_json::Value::Array(
        roles
            .into_iter()
            .map(|(role, style)| {
                serde_json::json!({
                    "role": role.key(),
                    "candidates": role.candidates(),
                    "style": style,
                })
            })
            .collect(),
    ))
}

/// The package's TypeScript declarations (`docrs.d.ts`); `pub` because the
/// custom section below only reads it on wasm32.
pub const TYPINGS: &str = r#"
/** Options of `fromMarkdown`; every field is optional. */
export interface FromMarkdownOptions {
  /** Reference document (.docx bytes): styles, lists, page setup, headers, footers. */
  reference?: Uint8Array;
  /** Replace only the body paragraph reading exactly this text. */
  placeholder?: string;
  /** Append after the reference's body instead of replacing it. */
  append?: boolean;
  /** Role key to existing style name, e.g. `{ code_block: "My Code" }`. */
  styles?: Record<string, string>;
  strictStyles?: boolean;
  strict?: boolean;
  toc?: boolean;
  tocDepth?: number;
  /** Title above the table of contents (default: the title of the template's own contents, else "Table of Contents"). */
  tocTitle?: string;
  tocPageNumbers?: boolean;
  /** Page numbers computed by estimating Word for Mac's layout (default true). */
  tocNumbers?: boolean;
  shiftHeadingLevel?: number;
  smart?: boolean;
  titleBlock?: boolean;
  properties?: boolean;
  captions?: boolean;
  templateLists?: boolean;
  pageNumbers?: boolean;
  /** `undefined` detects a cover page; `true` forces one; `false` turns it off. */
  coverPage?: boolean;
  /** Image bytes by the path the Markdown writes (`chart.png`, `./img/logo.png`). */
  images?: Record<string, Uint8Array>;
  strictPackage?: boolean;
  acknowledgeProtection?: boolean;
  /** Milliseconds since the Unix epoch for created document properties (default: now); 0 to the end of 9999. */
  now?: number;
  /** Resource budgets: `"default"`, or `"serverless"` for a 256 MB function. */
  limits?: "default" | "serverless";
  /** Warnings and error messages carry no text taken from the input (codes, lines and budgets only). */
  redact?: boolean;
  /** `{{Name}}` (or `{{{Name}}}`) merge fields to fill once converted, by name (spaces inside the braces allowed), in the body, headers, footers and text boxes. Fields not named here are left; `""` blanks one. A value is text within one paragraph: a line break, tab or other control character, `{{` or `}}` is refused. */
  fields?: Record<string, string>;
  /** Fonts to measure the contents page numbers with, and to draw a PDF in, for the families and styles the template embeds no font for (experimental). */
  fonts?: FontSet;
}

/** Options of a conversion that also draws a PDF (experimental). */
export interface PdfConversionOptions extends FromMarkdownOptions {
  /** Also draw the converted document as a PDF: `true`, or the PDF's options. The result adds `pdf` and `pdfReport`. */
  pdf: boolean | PdfOptions;
  /** Return the .docx too (default true); `false` needs `pdf`. */
  docx?: boolean;
}

/** Caps of one PDF; each lowers the `limits` profile's and never raises it. */
export interface PdfLimits {
  maxPages?: number;
  maxOutputBytes?: number;
  /** Decoded pixels of one picture. */
  maxImagePixels?: number;
  /** Decoded pixels of all the pictures together. */
  maxDocumentPixels?: number;
}

/** How to draw a PDF (experimental). */
export interface PdfOptions extends PdfLimits {
  /** Refuse with `pdf_not_faithful` when the PDF would leave anything out or change it (default: the conversion's `strict`). */
  strict?: boolean;
}

/** Options of `toPdf`. */
export interface ToPdfOptions extends PdfLimits {
  /** Refuse with `pdf_not_faithful` when the PDF would leave anything out or change it. */
  strict?: boolean;
  /** Fonts for the families and styles the document embeds no font for. */
  fonts?: FontSet;
  limits?: "default" | "serverless";
  strictPackage?: boolean;
  redact?: boolean;
}

/** Where a construct a PDF leaves out or changes is. */
export interface PdfLocation {
  /** The part (`word/document.xml`, `word/header1.xml`, ...), or with `redact` its kind (`body`, `header`, ...). */
  story: string;
  /** The story's paragraph it is in or after, from 1. */
  ordinal: number;
  page: number | null;
}

/** One kind of construct a PDF leaves out or changes. */
export interface PdfWarning {
  code: string;
  subject: string;
  message: string;
  /** The first ten. */
  locations: PdfLocation[];
  count: number;
}

/** What a PDF leaves out or changes. */
export interface PdfReport {
  schema_version: 1;
  warnings: PdfWarning[];
  pages: number;
  /** Each family and style the document asks for, and what drew it. */
  fonts: PdfFontUse[];
}

/** A family and style a PDF's text asks for, and the face that drew it. */
export interface PdfFontUse {
  requested_family: string;
  bold: boolean;
  italic: boolean;
  /** `null` for the built-in symbol font. */
  selected_family: string | null;
  source:
    | "document"
    | "supplied"
    | "alias"
    | "fallback"
    | "glyph_fallback"
    | "symbols"
    | "symbol_fallback"
    | "not_embedded";
  /** Bold or italic drawn from another style. */
  synthesized: boolean;
}

/** A PDF and its report. */
export interface PdfOutput {
  pdf: Uint8Array;
  pdfReport: PdfReport;
}

/** A conversion with `pdf` and `docx: false`: the PDF only. */
export interface FromMarkdownPdfResult extends PdfOutput {
  report: MarkdownReport;
  referenceSetAside: string[];
}

export interface ToPdfResult extends PdfOutput {
  referenceSetAside: string[];
}

/** A family and style a PDF draws that the document embeds no font for: what `fonts` must hold. */
export interface UsedFont {
  family: string;
  bold: boolean;
  italic: boolean;
}

/** A source to convert, with the options of its conversion (no `fonts`, `pdf` or `docx`). */
export type UsedFontsSource = Omit<FromMarkdownOptions, "fonts"> &
  ({ html: string } | { markdown: string } | { json: object | string });

/** Options of `usedFonts` for a .docx. */
export interface UsedFontsOptions {
  limits?: "default" | "serverless";
  strictPackage?: boolean;
  redact?: boolean;
}

type WithPdf = { pdf: true | PdfOptions };
/** The plain overload refuses an options type that can carry `pdf` or `docx`, which TypeScript would not split across the overloads. */
type NoPdf = { pdf?: never; docx?: never };

/** Convert Markdown into a .docx styled by a reference document, and with `pdf` draw it as a PDF too. */
export function fromMarkdown(markdown: string, options: FromMarkdownOptions & WithPdf & { docx: false }): FromMarkdownPdfResult;
export function fromMarkdown(markdown: string, options: FromMarkdownOptions & WithPdf & { docx?: true }): FromMarkdownResult & PdfOutput;
export function fromMarkdown(markdown: string, options: FromMarkdownOptions & { pdf: false; docx?: true }): FromMarkdownResult;
export function fromMarkdown(markdown: string, options: PdfConversionOptions): FromMarkdownResult | (FromMarkdownResult & PdfOutput) | FromMarkdownPdfResult;
export function fromMarkdown(markdown: string, options?: FromMarkdownOptions & NoPdf): FromMarkdownResult;

/** Convert editor HTML (TipTap's `editor.getHTML()` and similar) like `fromMarkdown`; pictures get no captions unless `captions: true`. */
export function fromHtml(html: string, options: FromMarkdownOptions & WithPdf & { docx: false }): FromMarkdownPdfResult;
export function fromHtml(html: string, options: FromMarkdownOptions & WithPdf & { docx?: true }): FromMarkdownResult & PdfOutput;
export function fromHtml(html: string, options: FromMarkdownOptions & { pdf: false; docx?: true }): FromMarkdownResult;
export function fromHtml(html: string, options: PdfConversionOptions): FromMarkdownResult | (FromMarkdownResult & PdfOutput) | FromMarkdownPdfResult;
export function fromHtml(html: string, options?: FromMarkdownOptions & NoPdf): FromMarkdownResult;

/** Convert a TipTap document (`editor.getJSON()`, as the object or as JSON text) like `fromMarkdown`; pictures get no captions unless `captions: true`. */
export function fromJson(document: object | string, options: FromMarkdownOptions & WithPdf & { docx: false }): FromMarkdownPdfResult;
export function fromJson(document: object | string, options: FromMarkdownOptions & WithPdf & { docx?: true }): FromMarkdownResult & PdfOutput;
export function fromJson(document: object | string, options: FromMarkdownOptions & { pdf: false; docx?: true }): FromMarkdownResult;
export function fromJson(document: object | string, options: PdfConversionOptions): FromMarkdownResult | (FromMarkdownResult & PdfOutput) | FromMarkdownPdfResult;
export function fromJson(document: object | string, options?: FromMarkdownOptions & NoPdf): FromMarkdownResult;

/** @deprecated Use `fromJson`. */
export function fromTiptap(document: object | string, options: FromMarkdownOptions & WithPdf & { docx: false }): FromMarkdownPdfResult;
/** @deprecated Use `fromJson`. */
export function fromTiptap(document: object | string, options: FromMarkdownOptions & WithPdf & { docx?: true }): FromMarkdownResult & PdfOutput;
/** @deprecated Use `fromJson`. */
export function fromTiptap(document: object | string, options: FromMarkdownOptions & { pdf: false; docx?: true }): FromMarkdownResult;
/** @deprecated Use `fromJson`. */
export function fromTiptap(document: object | string, options: PdfConversionOptions): FromMarkdownResult | (FromMarkdownResult & PdfOutput) | FromMarkdownPdfResult;
/** @deprecated Use `fromJson`. */
export function fromTiptap(document: object | string, options?: FromMarkdownOptions & NoPdf): FromMarkdownResult;

/**
 * Draw a .docx as a PDF, as `docrs pdf` does (experimental). A family and style with no font
 * program, in the document or `fonts`, refuses with `pdf_font_program_missing`; `details.missing_fonts`
 * lists each one.
 */
export function toPdf(docx: Uint8Array, options?: ToPdfOptions): ToPdfResult;

/**
 * The families and styles a PDF of a .docx, or of a source converted with its options, draws with no
 * font the document embeds (experimental): the faces to fetch and pass as `fonts`. For now it
 * draws the PDF without fonts and reads its refusal: it costs a full conversion and layout, and
 * throws `pdf_not_yet_drawn` wherever `toPdf` would, until it reads the document without drawing (#64).
 */
export function usedFonts(docx: Uint8Array, options?: UsedFontsOptions): UsedFont[];
export function usedFonts(source: UsedFontsSource): UsedFont[];

declare const fontSetBrand: unique symbol;

/**
 * Font files loaded once by `loadFonts` or `loadFontManifest` and passed to
 * conversions as `fonts` (experimental). It survives the package replacing
 * its WebAssembly instance; `free()` releases it for good, and using it
 * afterwards throws `pdf_font_handle_disposed`.
 */
export interface FontSet {
  readonly [fontSetBrand]: true;
  /** Every face, named as a document names it, in the order loaded. */
  readonly faces: readonly FontFace[];
  free(): void;
}

export interface FontFace {
  family: string;
  bold: boolean;
  italic: boolean;
  /** Has variation axes; layout measures its default instance. */
  variable: boolean;
  /** CFF outlines (an OpenType `.otf`). */
  cff: boolean;
  embeddable: boolean;
  subsettable: boolean;
}

/** Options of `loadFonts`. */
export interface FontOptions {
  /** Budgets the set is loaded under; every call checks it against its own `limits` too. */
  limits?: "default" | "serverless";
  /** Family a document names to a family of this set to measure it with. */
  aliases?: Record<string, string>;
}

/** One font file of a manifest: its bytes, or an https URL to fetch them from. The file must hold `family` in the declared style (a variable face covers both weights). */
export type FontManifestEntry = {
  /** Names the file in a `FontCache`: unique across tenants, and new when the file changes. */
  id: string;
  family: string;
  bold?: boolean;
  italic?: boolean;
} & ({ url: string; data?: undefined } | { data: Uint8Array; url?: undefined });

/** What `loadFontManifest` uses of an `AbortSignal` (no DOM types needed). */
export interface FontAbortSignal {
  readonly aborted: boolean;
  readonly reason: unknown;
  addEventListener(type: "abort", listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: "abort", listener: () => void): void;
}

/** What `loadFontManifest` reads of a fetch `Response`. */
export interface FontResponse {
  readonly ok: boolean;
  readonly status: number;
  readonly url?: string;
  readonly redirected?: boolean;
  readonly headers?: { get(name: string): string | null };
  readonly body?: { getReader(): { read(): Promise<{ done: boolean; value?: Uint8Array }>; cancel(): Promise<void> } } | null;
  arrayBuffer(): Promise<ArrayBuffer>;
}

declare const fontCacheBrand: unique symbol;

/** Fetched manifest files by entry id, from `createFontCache` (one per tenant). */
export interface FontCache {
  readonly [fontCacheBrand]: true;
}

export interface FontManifestOptions extends FontOptions {
  /**
   * Default: `globalThis.fetch`. Called with `redirect: "error"`; a redirect is refused. `signal` is
   * the manifest's own `AbortSignal` (typed `any` so that the DOM's `fetch` fits without DOM types here).
   */
  fetch?(url: string, init: { signal: any; redirect: "error" }): Promise<FontResponse>;
  signal?: FontAbortSignal;
  /** For all the fetches together: 1 to 2147483647 (default 30000). */
  timeoutMs?: number;
  /** For the files together and each one, below the `limits` budgets. */
  maxBytes?: number;
  /** Reuse files fetched before under the same entry id; without one, nothing is cached. */
  cache?: FontCache;
}

/** Load font files as one set; one file that cannot be read refuses them all (experimental). */
export function loadFonts(files: Uint8Array[], options?: FontOptions): FontSet;

/**
 * Fetch (https only, no redirects) or take every entry's file and load them
 * as one set (experimental). Errors name the entry's `id`, `family` and style.
 */
export function loadFontManifest(entries: FontManifestEntry[], options?: FontManifestOptions): Promise<FontSet>;

/** A cache of fetched manifest files, at most `maxBytes` (default 64 MiB), least recently used dropped first. */
export function createFontCache(options?: { maxBytes?: number }): FontCache;

/** Options of `inspectReference` and `markdownStyles`: those of a conversion, without `fonts`. */
export type ReferenceOptions = Omit<FromMarkdownOptions, "fonts">;

/** What a conversion will take from a reference document (`inspectReference`). */
export interface ReferenceInspection {
  schema_version: 1;
  /** Role key to the style a conversion uses. */
  roles: Record<string, { name: string; style_id: string | null; created: boolean }>;
  /** Styles a conversion adds because the template lacks them. */
  created_styles: string[];
  /** Branding lifted from the template's sample content. */
  lifted: {
    /** The look the body style takes from the template's own body paragraphs (or the body style the conversion adds); null when they show none. */
    body: RunLook | null;
    /** The paragraph spacing the body style takes from the template's prose (the unstyled paragraphs and its own); null when the style already gives it. */
    spacing: Spacing | null;
    headings: Record<string, RunLook>;
    table: {
      style_id: string | null;
      direct_formatting_lifted: boolean;
      look: Record<string, boolean>;
      /** The paragraph style the sample tables' header and body cells write most of their text in, when it is not the default. */
      cell_styles: { header: string | null; body: string | null };
      /** The hand formatting those cells share, which their paragraph styles take; null when they show none. */
      cell_text: { header: RunLook | null; body: RunLook | null };
    };
  };
  /** The section whose page setup the output keeps (the last). Its page 1 shows the
   *  template's page 1 header and footer, the other pages the last section's. */
  section: {
    count: number;
    used: "last";
    page: { width: number; height: number; orientation: "portrait" | "landscape"; margins: Record<string, number | null> };
    /** Text width in twips (dxa): the width tables are written at. */
    text_width: number;
    title_page: boolean;
    headers_footers: {
      header: Record<string, HeaderFooterSummary>;
      footer: Record<string, HeaderFooterSummary>;
    };
  } | null;
  even_and_odd_headers: boolean;
  compatibility: { mode: number; word: string } | null;
  cover_page: string | null;
  page_numbers: string | null;
  /** The title of the template's own table of contents, which a conversion uses unless `tocTitle` is given; null when it has none. */
  toc_title: string | null;
  protected: boolean;
  /** Plain-language problems a practice can fix in Word. */
  problems: { code: string; message: string }[];
  reference_set_aside: string[];
}

/** A run look lifted from sample content; `bold`/`italic` null when the style decides. */
export interface RunLook {
  font: string | null;
  size_pt: number | null;
  color: string | null;
  bold: boolean | null;
  italic: boolean | null;
}

/** Paragraph spacing as Word resolves it: points before and after; `line` is a multiple of single spacing under the `auto` rule, else points. */
export interface Spacing {
  before_pt: number | null;
  after_pt: number | null;
  line: number | null;
  line_rule: string;
}

export interface HeaderFooterSummary {
  images: number;
  tables: number;
  /** Field names such as PAGE, NUMPAGES. */
  fields: string[];
  has_text: boolean;
  /** Not defined by the template's last section itself: page 1's, or inherited from an earlier section. */
  inherited: boolean;
}

/** A conversion warning (something converted with a loss). */
export interface MarkdownWarning { code: string; message: string; line: number | null }

/** The conversion report, as the CLI's `--json` `markdown` object. */
export interface MarkdownReport {
  schema_version: 1;
  counts: Record<string, number>;
  styles: Record<string, { name: string; style_id: string | null; created: boolean }>;
  created_styles: string[];
  changed_parts: string[];
  /** Template parts left out of the output (sample-body media, comments, ...). */
  removed_parts: string[];
  /** Template document properties left out (`core:creator`, `app:Pages`, `custom:<name>`, ...). */
  cleared_properties: string[];
  page_numbers: string | null;
  cover_page: string | null;
  /** Where the contents' title came from; null without a contents. */
  toc_title_source: "option" | "front_matter" | "reference" | "default" | null;
  /** Computed contents page numbers; null without a contents or with `tocNumbers: false`. */
  toc: {
    entries: number;
    /** Estimated page count. */
    pages: number;
    /** Fonts with no metrics, laid out with a substitute (also `font_metrics_missing` warnings). */
    missing_fonts: string[];
    /** Fonts measured from the template's embedded copies. */
    embedded_fonts: string[];
  } | null;
  warnings: MarkdownWarning[];
  /** With `fields`: how many replacements were made (a field used twice counts twice), the names of fields left in the document (given even with `redact`), and any a replacement refused; null without. */
  fields: {
    filled: number;
    left: string[];
    refused: { name: string; code: string; reason: string }[];
  } | null;
}

export interface FromMarkdownResult {
  /** The .docx package. */
  docx: Uint8Array;
  report: MarkdownReport;
  /** Irregularities set aside to read the reference document. */
  referenceSetAside: string[];
}

export interface RoleStyle {
  role: string;
  candidates: string[];
  style: { name: string; style_id: string | null; created: boolean } | null;
}

/**
 * Thrown errors are `Error`s with `code` (a refusal kind such as
 * `malformed_package` or `unsupported_structure`, or `invalid_argument`,
 * `resource_limit`, …), `reason` (a stable identifier such as
 * `malformed_package` reasons or `style_missing`), `refusal` and `limit`.
 */
export interface DocumentCliError extends Error {
  code: string;
  reason: string;
  refusal: boolean;
  /** Set for `code: "resource_limit"`: the budget, its value and what was seen. */
  limit: { limit: string; allowed: number; observed: number } | null;
  /** What caused a refusal: the warnings behind `pdf_not_faithful` or `pdf_not_yet_drawn`, the faces `pdf_font_program_missing` lacks. */
  details: {
    warnings: { code: string; count: number; description: string }[];
    missing_fonts?: { family: string; bold: boolean; italic: boolean }[];
  } | null;
}
"#;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = TYPINGS;

fn js_error(error: &Error) -> JsValue {
    js_error_with(error, false)
}

fn js_error_with(error: &Error, redact: bool) -> JsValue {
    let js = if redact {
        js_sys::Error::new(&error.redacted_message())
    } else {
        js_sys::Error::new(error.message())
    };
    let json = error.to_json();
    let fields = &json["error"];
    for key in ["code", "reason", "refusal", "limit", "details"] {
        let _ = js_sys::Reflect::set(&js, &JsValue::from_str(key), &to_js(&fields[key]));
    }
    js.into()
}

/// Option names, as JavaScript writes them (the fields of `T` in
/// camelCase, read off its serialization).
fn names_of<T: Serialize + Default>() -> Vec<String> {
    let fields = serde_json::to_value(T::default()).expect("options serialize");
    fields
        .as_object()
        .expect("options serialize as an object")
        .keys()
        .cloned()
        .collect()
}

#[cfg(test)]
fn option_names() -> Vec<String> {
    names_of::<Options>()
}

fn to_js(value: &serde_json::Value) -> JsValue {
    use serde::Serialize;
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .unwrap_or(JsValue::NULL)
}

fn js_length(value: &JsValue) -> u64 {
    if let Some(text) = value.dyn_ref::<js_sys::JsString>() {
        return u64::from(text.length());
    }
    ["length", "byteLength"]
        .into_iter()
        .filter_map(|key| js_sys::Reflect::get(value, &JsValue::from_str(key)).ok())
        .find_map(|length| length.as_f64())
        .filter(|length| length.is_finite() && *length > 0.0)
        .map_or(0, |length| length as u64)
}

/// Check the budgets of the `limits` option against the sizes of the source
/// and of the `reference` and `images` options as JavaScript holds them,
/// before any of them is copied into WebAssembly memory, and the `fonts`
/// set by the sizes it was loaded with. A string's UTF-8
/// size is at least its UTF-16 length, so the source check never refuses
/// what fits; the exact size is checked again after the copy.
fn check_js_sizes(
    source: Option<&JsValue>,
    reference: Option<&JsValue>,
    options: Option<&js_sys::Object>,
) -> Result<(), JsValue> {
    let get = |key: &str| {
        options
            .and_then(|options| js_sys::Reflect::get(options, &JsValue::from_str(key)).ok())
            .filter(|value| !value.is_undefined() && !value.is_null())
    };
    let redact = get("redact").and_then(|v| v.as_bool()) == Some(true);
    let refuse = |err: Error| js_error_with(&err, redact);
    let name = match get("limits") {
        None => None,
        Some(value) => Some(value.as_string().ok_or_else(|| {
            refuse(Error::invalid_argument(
                "invalid_options",
                "limits must be \"default\" or \"serverless\"",
            ))
        })?),
    };
    let (limits, input) = profile(name.as_deref()).map_err(refuse)?;
    // Both a positional reference and `options.reference` are copied in,
    // so both count.
    let reference = reference
        .map_or(0, js_length)
        .saturating_add(get("reference").as_ref().map_or(0, js_length));
    let images = get("images")
        .and_then(|images| images.dyn_into::<js_sys::Object>().ok())
        .map_or(0, |images| {
            js_sys::Object::values(&images)
                .iter()
                .map(|image| js_length(&image))
                .fold(0u64, u64::saturating_add)
        });
    check_sizes(
        &limits,
        &input,
        source.map_or(0, js_length),
        reference,
        images,
    )
    .map_err(refuse)?;
    match get("fonts") {
        Some(id) => fonts::id_of(&id)
            .and_then(fonts::registered)
            .and_then(|fonts| fonts.check(&input.fonts))
            .map_err(refuse),
        None => Ok(()),
    }
}

/// The source text, after its size was checked.
fn source_from(source: &JsValue, redact: bool) -> Result<String, JsValue> {
    source.as_string().ok_or_else(|| {
        js_error_with(
            &Error::invalid_argument("invalid_source", "the source must be a string"),
            redact,
        )
    })
}

/// A `Uint8Array` argument, after its size was checked.
fn bytes_from(value: &JsValue, what: &str) -> Result<Vec<u8>, JsValue> {
    value
        .dyn_ref::<js_sys::Uint8Array>()
        .map(js_sys::Uint8Array::to_vec)
        .ok_or_else(|| {
            js_error(&Error::invalid_argument(
                "invalid_options",
                format!("{what} must be a Uint8Array"),
            ))
        })
}

/// The options `value` gives, and the font set its `fonts` names.
fn options_from<T: DeserializeOwned + Serialize + Default>(
    value: Option<js_sys::Object>,
) -> Result<(T, Option<LoadedFonts>), JsValue> {
    let Some(value) = value else {
        return Ok((T::default(), None));
    };
    let names = names_of::<T>();
    // Option names and types are the caller's code, not input data, but a
    // value echoed in a type error could be; honour `redact` early.
    let redact = redact_of(&value);
    // serde-wasm-bindgen reads only the fields it knows, so a misspelt
    // option would be ignored silently; refuse it instead. An option set to
    // `undefined` or `null` counts as not set, as JavaScript callers
    // forwarding optional values expect.
    for key in js_sys::Object::keys(&value).iter() {
        let name = key.as_string().unwrap_or_default();
        // `fonts` is a registry id, which serde does not read.
        if name != "fonts" && !names.contains(&name) {
            return Err(js_error(&Error::invalid_argument(
                "invalid_options",
                format!("unknown option {name:?}"),
            )));
        }
    }
    let given = set_fields(&value);
    let fonts_key = JsValue::from_str("fonts");
    let fonts = match js_sys::Reflect::get(&given, &fonts_key) {
        Ok(id) if !id.is_undefined() => Some(
            fonts::id_of(&id)
                .and_then(fonts::registered)
                .map_err(|err| js_error_with(&err, redact))?,
        ),
        _ => None,
    };
    let _ = js_sys::Reflect::delete_property(&given, &fonts_key);
    let options = serde_wasm_bindgen::from_value(given.into()).map_err(|err| {
        js_error_with(
            &Error::invalid_argument("invalid_options", format!("options: {err}")),
            redact,
        )
    })?;
    Ok((options, fonts))
}

/// The conversion options `value` gives, with their font set.
fn conversion_options(value: Option<js_sys::Object>) -> Result<Options, JsValue> {
    let (options, fonts): (Options, _) = options_from(value)?;
    Ok(Options {
        fonts: fonts.unwrap_or_default(),
        ..options
    })
}

/// The options of `inspectReference` and `markdownStyles`: a conversion's,
/// refusing `pdf` and `docx`, which neither draws nor writes.
fn reference_options(value: Option<js_sys::Object>, name: &str) -> Result<Options, JsValue> {
    let options = conversion_options(value)?;
    if options.pdf.is_some() || options.docx.is_some() {
        return Err(js_error_with(
            &Error::invalid_argument("invalid_options", format!("{name} takes no pdf or docx")),
            options.redact,
        ));
    }
    Ok(options)
}

/// The fields of `options` not set to `undefined` or `null`, which count as
/// not set.
fn set_fields(options: &js_sys::Object) -> js_sys::Object {
    let given = js_sys::Object::new();
    for key in js_sys::Object::keys(options).iter() {
        let field = js_sys::Reflect::get(options, &key).unwrap_or(JsValue::UNDEFINED);
        if !field.is_undefined() && !field.is_null() {
            let _ = js_sys::Reflect::set(&given, &key, &field);
        }
    }
    given
}

fn redact_of(options: &js_sys::Object) -> bool {
    js_sys::Reflect::get(options, &JsValue::from_str("redact"))
        .map(|v| v.as_bool() == Some(true))
        .unwrap_or(false)
}

fn converted_to_js(converted: Converted) -> JsValue {
    let result = js_sys::Object::new();
    let set = |key: &str, value: &JsValue| {
        let _ = js_sys::Reflect::set(&result, &JsValue::from_str(key), value);
    };
    if let Some(docx) = converted.docx {
        set("docx", &js_sys::Uint8Array::from(docx.as_slice()).into());
    }
    set("report", &to_js(&converted.report));
    set(
        "referenceSetAside",
        &to_js(&serde_json::json!(converted.reference_set_aside)),
    );
    if let Some(pdf) = converted.pdf {
        pdf::pdf_to_js(pdf, &result);
    }
    result.into()
}

fn run(
    source: JsValue,
    format: InputFormat,
    options: Option<js_sys::Object>,
) -> Result<JsValue, JsValue> {
    check_js_sizes(Some(&source), None, options.as_ref())?;
    let options = conversion_options(options)?;
    let redact = options.redact;
    let now = timestamp_from_millis(options.now.unwrap_or_else(js_sys::Date::now))
        .map_err(|err| js_error_with(&err, redact))?;
    let source = source_from(&source, redact)?;
    convert(&source, format, options, now)
        .map(converted_to_js)
        .map_err(|err| js_error_with(&err, redact))
}

/// Convert Markdown into a `.docx` styled by a reference document.
#[wasm_bindgen(js_name = fromMarkdown, skip_typescript)]
pub fn from_markdown(
    markdown: JsValue,
    options: Option<js_sys::Object>,
) -> Result<JsValue, JsValue> {
    run(markdown, InputFormat::Markdown, options)
}

/// Convert editor HTML (TipTap's `editor.getHTML()` and similar) into a
/// `.docx` styled by a reference document. Same options and result as
/// `fromMarkdown`; pictures get no captions unless `captions: true`.
#[wasm_bindgen(js_name = fromHtml, skip_typescript)]
pub fn from_html(html: JsValue, options: Option<js_sys::Object>) -> Result<JsValue, JsValue> {
    run(html, InputFormat::Html, options)
}

/// Convert a TipTap document (`editor.getJSON()`, as the object or as JSON
/// text) into a `.docx` styled by a reference document. Same options and
/// result as `fromMarkdown`; pictures get no captions unless `captions: true`.
#[wasm_bindgen(js_name = fromJson, skip_typescript)]
pub fn from_json(document: JsValue, options: Option<js_sys::Object>) -> Result<JsValue, JsValue> {
    let source = if document.is_string() {
        document
    } else if document.is_object() {
        // One JavaScript string, sized like any other source before it is
        // copied into WebAssembly memory.
        js_sys::JSON::stringify(&document)
            .map(JsValue::from)
            .map_err(|_| {
                js_error(&Error::invalid_argument(
                    "invalid_source",
                    "the document could not be serialized as JSON",
                ))
            })?
    } else {
        return Err(js_error(&Error::invalid_argument(
            "invalid_source",
            "the document must be an object (editor.getJSON()) or a JSON string",
        )));
    };
    run(source, InputFormat::Tiptap, options)
}

/// `fromJson` under the name it had in 0.3.0.
///
/// @deprecated Use `fromJson`.
#[wasm_bindgen(js_name = fromTiptap, skip_typescript)]
pub fn from_tiptap(document: JsValue, options: Option<js_sys::Object>) -> Result<JsValue, JsValue> {
    from_json(document, options)
}

/// What a conversion will take from a reference document: role styles,
/// lifted branding, the section kept, headers and footers, cover page,
/// compatibility mode and plain-language problems. Options as for
/// `fromMarkdown` (`styles`, `pageNumbers`, `coverPage`, `strictPackage`,
/// `limits` matter).
#[wasm_bindgen(js_name = inspectReference, unchecked_return_type = "ReferenceInspection")]
pub fn inspect_reference(
    #[wasm_bindgen(unchecked_param_type = "Uint8Array")] reference: JsValue,
    #[wasm_bindgen(unchecked_optional_param_type = "ReferenceOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
    check_js_sizes(None, Some(&reference), options.as_ref())?;
    let options = reference_options(options, "inspectReference")?;
    let reference = bytes_from(&reference, "reference")?;
    inspect(&reference, &options)
        .map(|value| to_js(&value))
        .map_err(|err| js_error_with(&err, options.redact))
}

/// Which style each Markdown role takes from `reference` (the bundled blank
/// document when absent), with an optional role → style name map. Options as
/// for `fromMarkdown` (`limits`, `strictPackage` and `redact` matter; `styles`
/// there adds to the map, which wins).
#[wasm_bindgen(js_name = markdownStyles, unchecked_return_type = "RoleStyle[]")]
pub fn markdown_styles(
    #[wasm_bindgen(unchecked_optional_param_type = "Uint8Array")] reference: JsValue,
    #[wasm_bindgen(unchecked_optional_param_type = "Record<string, string>")] styles: Option<
        js_sys::Object,
    >,
    #[wasm_bindgen(unchecked_optional_param_type = "ReferenceOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
    let reference = Some(reference).filter(|r| !r.is_undefined() && !r.is_null());
    check_js_sizes(None, reference.as_ref(), options.as_ref())?;
    let redact = options.as_ref().is_some_and(redact_of);
    let mut options = reference_options(options, "markdownStyles")?;
    if let Some(styles) = styles {
        let styles: BTreeMap<String, String> = serde_wasm_bindgen::from_value(styles.into())
            .map_err(|err| {
                js_error_with(
                    &Error::invalid_argument("invalid_options", format!("styles: {err}")),
                    redact,
                )
            })?;
        options.styles.extend(styles);
    }
    let reference = reference
        .map(|reference| bytes_from(&reference, "reference"))
        .transpose()?;
    styles_of(reference.as_deref(), &options)
        .map(|value| to_js(&value))
        .map_err(|err| js_error_with(&err, redact))
}

/// A starter reference document showing every Markdown style, to restyle in
/// Word and pass back as `reference`.
#[wasm_bindgen(js_name = referenceDocument)]
pub fn reference_document() -> Result<Vec<u8>, JsValue> {
    markdown::reference_document()
        .and_then(|document| document.patch_bytes())
        .map(|(bytes, _)| bytes)
        .map_err(|err| js_error(&err))
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

/// The build aborts on a panic: the instance traps with `RuntimeError:
/// unreachable` and the package's entry point (`js/index.js`) replaces it
/// with a fresh one. Say what panicked on `console.error` first.
#[wasm_bindgen(start)]
fn start() {
    std::panic::set_hook(Box::new(|info| {
        console_error(&format!(
            "docrs-wasm {} {info}\nThis is a bug in docrs-wasm. The call throws \
             WebAssembly.RuntimeError; the next call runs on a fresh instance.",
            env!("CARGO_PKG_VERSION")
        ));
    }));
}

/// The library version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const PNG: [u8; 67] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn reopen(bytes: &[u8]) -> Document {
        Document::open_bytes(bytes, &Limits::default()).expect("output reopens")
    }

    #[test]
    fn converts_with_images_from_memory_and_the_given_time() {
        let options = Options {
            images: BTreeMap::from([("./img/dot.png".to_string(), ByteBuf::from(PNG.to_vec()))]),
            ..Options::default()
        };
        let markdown = "---\ntitle: Plan\n---\n\n# Heading\n\n![A dot](./img/dot.png)\n";
        let converted = convert(
            markdown,
            InputFormat::Markdown,
            options.clone(),
            Timestamp::from_unix(1_700_000_000),
        )
        .unwrap();
        assert_eq!(converted.report["counts"]["images"], 1);
        assert_eq!(converted.report["warnings"], serde_json::json!([]));
        let document = reopen(converted.docx.as_deref().unwrap());
        let core = String::from_utf8(docrs::xml::serialize(
            document.package().xml("docProps/core.xml").unwrap(),
        ))
        .unwrap();
        assert!(core.contains("Plan"), "{core}");
    }

    #[test]
    fn fills_fields_in_the_body_and_headers() {
        let mut reference = Document::new();
        let section = reference.sections().unwrap().remove(0);
        let header = section.header().definition(&mut reference).unwrap();
        header
            .add_paragraph(&mut reference, "Prepared for {{Client}}", None)
            .unwrap();
        let (reference, _) = reference.patch_bytes().unwrap();
        let options = Options {
            reference: Some(ByteBuf::from(reference)),
            fields: BTreeMap::from([
                ("Client".to_string(), "Jo Smith".to_string()),
                ("Ref".to_string(), String::new()),
            ]),
            ..Options::default()
        };
        let converted = convert(
            "Dear **{{Cli**ent}}, re: {{ Ref }}{{Other}}.\n",
            InputFormat::Markdown,
            options,
            Timestamp::from_unix(0),
        )
        .unwrap();
        assert_eq!(
            converted.report["fields"],
            serde_json::json!({ "filled": 3, "left": ["Other"], "refused": [] })
        );
        let document = reopen(converted.docx.as_deref().unwrap());
        let text: Vec<String> = docrs::story::iter_blocks(&document, docrs::story::View::Current)
            .unwrap()
            .into_iter()
            .map(|block| block.text)
            .collect();
        assert!(
            text.contains(&"Dear Jo Smith, re: {{Other}}.".to_string()),
            "{text:?}"
        );
        assert!(
            text.contains(&"Prepared for Jo Smith".to_string()),
            "{text:?}"
        );
        let plain = convert(
            "{{Client}}",
            InputFormat::Markdown,
            Options::default(),
            Timestamp::from_unix(0),
        )
        .unwrap();
        assert_eq!(plain.report["fields"], serde_json::Value::Null);
    }

    #[test]
    fn contents_page_numbers_and_report_follow_the_fields_filled_in() {
        let filler = "Filler text that takes a full line of the page body here.\n\n".repeat(56);
        let source = format!("# Intro\n\nDear {{{{Client}}}},\n\n{filler}# Late\n\nEnd.\n");
        let late_page = |fields: &[(&str, &str)]| -> (String, u64) {
            let options = Options {
                toc: Some(true),
                fields: fields
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                ..Options::default()
            };
            let converted = convert(
                &source,
                InputFormat::Markdown,
                options,
                Timestamp::from_unix(0),
            )
            .unwrap();
            let pages = converted.report["toc"]["pages"].as_u64().unwrap();
            let document = reopen(converted.docx.as_deref().unwrap());
            let xml = String::from_utf8(docrs::xml::serialize(
                document.package().xml("word/document.xml").unwrap(),
            ))
            .unwrap();
            let rest = &xml[xml.find(" PAGEREF late ").unwrap()..];
            let number = &rest[rest.find("<w:t>").unwrap() + 5..];
            (number[..number.find('<').unwrap()].to_string(), pages)
        };
        let value = "Jo Smith of a long merge value ".repeat(12);
        assert_eq!(late_page(&[]), ("3".to_string(), 3));
        assert_eq!(late_page(&[("Client", &value)]), ("4".to_string(), 4));
    }

    #[test]
    fn a_reference_s_own_contents_is_neither_refreshed_nor_counted() {
        let markdown_options = docrs::markdown::MarkdownOptions {
            toc: Some(true),
            ..Default::default()
        };
        let (mut reference, _) =
            docrs::markdown::convert("{{Pad}}\n\n# Old\n", None, &markdown_options).unwrap();
        // Moves "Old" to a later page while its contents still says 2.
        reference
            .replace_all("{{Pad}}", &"word ".repeat(1200), &Default::default())
            .unwrap();
        let (reference, _) = reference.patch_bytes().unwrap();
        let options = Options {
            reference: Some(ByteBuf::from(reference)),
            append: true,
            fields: BTreeMap::from([("Client".to_string(), "Jo".to_string())]),
            ..Options::default()
        };
        let converted = convert(
            "[TOC]\n\n# New\n\nDear {{Client}}.\n",
            InputFormat::Markdown,
            options,
            Timestamp::from_unix(0),
        )
        .unwrap();
        assert_eq!(converted.report["toc"]["entries"], 1);
        let document = reopen(converted.docx.as_deref().unwrap());
        let xml = String::from_utf8(docrs::xml::serialize(
            document.package().xml("word/document.xml").unwrap(),
        ))
        .unwrap();
        let rest = &xml[xml.find(" PAGEREF old ").unwrap()..];
        let number = &rest[rest.find("<w:t>").unwrap() + 5..];
        assert_eq!(&number[..number.find('<').unwrap()], "2");
    }

    fn fill_fields_in(
        source: &str,
        fields: &[(&str, &str)],
        limits: Option<&str>,
    ) -> Result<(serde_json::Value, Vec<String>), Error> {
        let options = Options {
            fields: fields
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            limits: limits.map(str::to_string),
            ..Options::default()
        };
        let converted = convert(
            source,
            InputFormat::Markdown,
            options,
            Timestamp::from_unix(0),
        )?;
        let text = docrs::story::iter_blocks(
            &reopen(converted.docx.as_deref().unwrap()),
            docrs::story::View::Current,
        )
        .unwrap()
        .into_iter()
        .map(|block| block.text)
        .collect();
        Ok((converted.report["fields"].clone(), text))
    }

    #[test]
    fn a_field_value_with_a_line_break_is_refused_by_name() {
        for source in ["No fields here.\n", "{{Address}}\n"] {
            let err =
                fill_fields_in(source, &[("Address", "1 Main St\nSpringfield")], None).unwrap_err();
            assert!(err.message().contains("Address"), "{source:?}: {err}");
        }
    }

    #[test]
    fn a_field_value_fills_literally_whatever_the_order() {
        let fields = [("A", "{{B}}"), ("B", "bee")];
        for (source, literal) in [
            ("{{A}} and {{B}}\n", "{{B}} and bee"),
            ("{{B}} and {{A}}\n", "bee and {{B}}"),
        ] {
            // Refusing such a value by name passes too.
            match fill_fields_in(source, &fields, None) {
                Ok((_, text)) => assert_eq!(text, [literal], "{source:?}"),
                Err(err) => assert!(err.message().contains("\"A\""), "{err}"),
            }
        }
    }

    #[test]
    fn serverless_budgets_the_text_fields_add() {
        let value = "x".repeat(384 << 10);
        let err = fill_fields_in(
            "{{A}} {{A}} {{A}} {{A}}\n",
            &[("A", &value)],
            Some("serverless"),
        )
        .map(|(report, _)| report)
        .unwrap_err();
        assert!(err.limit_exceeded().is_some(), "{err}");
    }

    #[test]
    fn triple_braces_fill_like_double() {
        let (report, text) = fill_fields_in(
            "Dear {{{Client}}}, {{{Other}}}.\n",
            &[("Client", "Jo")],
            None,
        )
        .unwrap();
        assert_eq!(report["left"], serde_json::json!(["Other"]));
        assert_eq!(text, ["Dear Jo, {{{Other}}}."]);
    }

    #[test]
    fn options_select_placement_and_styles() {
        let starter = reference_document().unwrap();
        let options = Options {
            reference: Some(ByteBuf::from(starter)),
            styles: BTreeMap::from([("code_block".to_string(), "Normal".to_string())]),
            append: true,
            ..Options::default()
        };
        let converted = convert(
            "```\ncode\n```\n",
            InputFormat::Markdown,
            options.clone(),
            Timestamp::from_unix(0),
        )
        .unwrap();
        assert_eq!(converted.report["styles"]["code_block"]["name"], "Normal");
        let missing = Options {
            styles: BTreeMap::from([("nonsense".to_string(), "Normal".to_string())]),
            ..Options::default()
        };
        let err = convert(
            "text",
            InputFormat::Markdown,
            missing.clone(),
            Timestamp::from_unix(0),
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_argument");
    }

    #[test]
    fn strict_refusals_produce_nothing() {
        let options = Options {
            strict: true,
            ..Options::default()
        };
        let err = convert(
            "![x](missing.png)\n",
            InputFormat::Markdown,
            options.clone(),
            Timestamp::from_unix(0),
        )
        .unwrap_err();
        assert!(err.is_refusal(), "{err}");
    }

    #[test]
    fn option_names_are_the_fields_as_javascript_writes_them() {
        let names = option_names();
        assert!(names.contains(&"strictStyles".to_string()), "{names:?}");
        for name in names {
            assert!(!name.contains('_'), "{name}");
            let value = match name.as_str() {
                "styles" | "images" | "fields" => serde_json::json!({}),
                "placeholder" | "tocTitle" => serde_json::json!("x"),
                "limits" => serde_json::json!("serverless"),
                "tocDepth" | "shiftHeadingLevel" | "now" => serde_json::json!(1),
                "reference" => serde_json::Value::Null,
                _ => serde_json::json!(true),
            };
            let object = serde_json::json!({ name.as_str(): value });
            assert!(serde_json::from_value::<Options>(object).is_ok(), "{name}");
        }
    }

    /// The text between the braces of `export interface NAME { ... }` in
    /// `TYPINGS`.
    fn interface_body(name: &str) -> &'static str {
        let start = TYPINGS
            .find(&format!("export interface {name} "))
            .unwrap_or_else(|| panic!("TYPINGS declares {name}"));
        brace_block(&TYPINGS[start..])
    }

    /// The text inside the first `{` of `text` and its matching `}`.
    fn brace_block(text: &str) -> &str {
        let open = text.find('{').expect("a brace block");
        let mut depth = 0usize;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &text[open + 1..open + i];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces in {text}");
    }

    /// The object type of member `key` of a type body (`key: { ... }`).
    fn member_body<'a>(body: &'a str, key: &str) -> &'a str {
        let at = body
            .find(&format!("{key}: {{"))
            .unwrap_or_else(|| panic!("{key} is an object member"));
        brace_block(&body[at..])
    }

    /// The names of the members at the top level of a type body (nested
    /// object types and comments skipped).
    fn member_names(body: &str) -> BTreeSet<String> {
        let mut text = String::new();
        let mut remaining = body;
        while let Some(start) = remaining.find("/*") {
            text.push_str(&remaining[..start]);
            let end = remaining[start..]
                .find("*/")
                .map_or(remaining.len(), |e| start + e + 2);
            remaining = &remaining[end..];
        }
        text.push_str(remaining);
        let mut names = BTreeSet::new();
        let mut depth = 0usize;
        let mut word = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                c if c.is_alphanumeric() || c == '_' => {
                    word.push(c);
                    continue;
                }
                mut mark => {
                    if mark == '?' {
                        mark = chars.next().unwrap_or(' ');
                    }
                    if depth == 0 && mark == ':' && !word.is_empty() {
                        names.insert(word.clone());
                    }
                }
            }
            word.clear();
        }
        names
    }

    fn json_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .unwrap_or_else(|| panic!("an object, not {value}"))
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn typings_name_the_keys_the_reports_carry() {
        let converted = convert(
            "---\ntitle: Plan\n---\n\n# One\n\n![x](missing.png)\n",
            InputFormat::Markdown,
            Options {
                toc: Some(true),
                ..Options::default()
            },
            Timestamp::from_unix(0),
        )
        .unwrap();
        let report = &converted.report;
        let declared = interface_body("MarkdownReport");
        assert_eq!(json_keys(report), member_names(declared));
        assert_eq!(
            json_keys(&report["toc"]),
            member_names(member_body(declared, "toc"))
        );
        assert_eq!(
            json_keys(&report["warnings"][0]),
            member_names(interface_body("MarkdownWarning"))
        );

        let inspection = inspect(&reference_document().unwrap(), &Options::default()).unwrap();
        let declared = interface_body("ReferenceInspection");
        assert_eq!(json_keys(&inspection), member_names(declared));
        assert_eq!(
            json_keys(&inspection["section"]),
            member_names(member_body(declared, "section"))
        );
        let lifted = member_body(declared, "lifted");
        assert_eq!(json_keys(&inspection["lifted"]), member_names(lifted));
        assert_eq!(
            json_keys(&inspection["lifted"]["table"]),
            member_names(member_body(lifted, "table"))
        );

        let roles = styles_of(None, &Options::default()).unwrap();
        assert_eq!(
            json_keys(&roles[0]),
            member_names(interface_body("RoleStyle"))
        );

        let converted = convert(
            "# One\n\nCafe\u{301}\n",
            InputFormat::Markdown,
            pdf::tests::options(Some(PdfChoice::Wanted(true))),
            Timestamp::from_unix(0),
        )
        .unwrap();
        let report = &converted.pdf.unwrap().report;
        assert_eq!(json_keys(report), member_names(interface_body("PdfReport")));
        let warning = &report["warnings"][0];
        assert_eq!(
            json_keys(warning),
            member_names(interface_body("PdfWarning"))
        );
        assert_eq!(
            json_keys(&warning["locations"][0]),
            member_names(interface_body("PdfLocation"))
        );
        assert_eq!(
            json_keys(&report["fonts"][0]),
            member_names(interface_body("PdfFontUse"))
        );

        let error = timestamp_from_millis(-1.0).unwrap_err().to_json();
        let mut keys = json_keys(&error["error"]);
        keys.insert("details".into());
        assert!(member_names(interface_body("DocumentCliError")).is_subset(&keys));
    }

    #[test]
    fn editor_html_redaction_and_serverless_limits() {
        let options = Options {
            redact: true,
            limits: Some("serverless".into()),
            images: BTreeMap::from([(
                "https://x.example/a.png?t=1".to_string(),
                ByteBuf::from(PNG.to_vec()),
            )]),
            ..Options::default()
        };
        let html = "<p style=\"text-align: center\"><span style=\"color: #dc2626\">Client secret</span></p>\
                    <div data-image-align=\"right\"><img src=\"https://x.example/a.png?t=1\" alt=\"Logo\" width=\"10\"></div>\
                    <img src=\"https://x.example/missing-client-name.png\">";
        let converted = convert(
            html,
            InputFormat::Html,
            options.clone(),
            Timestamp::from_unix(0),
        )
        .unwrap();
        assert_eq!(converted.report["counts"]["images"], 1);
        let warnings = converted.report["warnings"].to_string();
        assert!(warnings.contains("remote_image_skipped"), "{warnings}");
        assert!(!warnings.contains("missing-client-name"), "{warnings}");
        let big = "x".repeat((384 << 10) + 1);
        let err = convert(
            &big,
            InputFormat::Markdown,
            options.clone(),
            Timestamp::from_unix(0),
        )
        .unwrap_err();
        assert_eq!(err.reason(), "max_source_bytes");
        assert!(!err.redacted_message().contains("xxxx"));
        let bad = Options {
            limits: Some("huge".into()),
            ..Options::default()
        };
        assert_eq!(
            convert("a", InputFormat::Markdown, bad, Timestamp::from_unix(0))
                .unwrap_err()
                .code(),
            "invalid_argument"
        );
    }

    #[test]
    fn tiptap_json_converts_like_the_editors_html() {
        let json = r##"{"type":"doc","content":[
            {"type":"heading","attrs":{"level":2,"textAlign":"center"},"content":[{"type":"text","text":"Advice"}]},
            {"type":"paragraph","attrs":{"textAlign":null},"content":[
                {"type":"text","text":"red","marks":[{"type":"textStyle","attrs":{"color":"#dc2626","fontFamily":"","fontSize":""}}]},
                {"type":"text","text":" plain","marks":[{"type":"comment","attrs":{"id":"c1"}}]}]}]}"##;
        let converted = convert(
            json,
            InputFormat::Tiptap,
            Options::default(),
            Timestamp::from_unix(0),
        )
        .unwrap();
        let body = String::from_utf8(docrs::xml::serialize(
            reopen(converted.docx.as_deref().unwrap())
                .package()
                .xml("word/document.xml")
                .unwrap(),
        ))
        .unwrap();
        assert!(body.contains("<w:color w:val=\"DC2626\"/>"), "{body}");
        assert!(body.contains("<w:jc w:val=\"center\"/>"), "{body}");
        assert_eq!(
            converted.report["warnings"][0]["code"], "tiptap_mark_dropped",
            "{}",
            converted.report
        );
        let err = convert(
            "[1]",
            InputFormat::Tiptap,
            Options::default(),
            Timestamp::from_unix(0),
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_argument");
        assert_eq!(err.reason(), "tiptap_json_invalid");
        let big = format!(
            "{{\"type\":\"doc\",\"content\":[{}]}}",
            "1,".repeat(200 << 10)
        );
        let err = convert(
            &big,
            InputFormat::Tiptap,
            Options {
                limits: Some("serverless".into()),
                ..Options::default()
            },
            Timestamp::from_unix(0),
        )
        .unwrap_err();
        assert_eq!(err.reason(), "max_source_bytes");
    }

    #[test]
    fn inspection_reports_the_section_and_problems() {
        let starter = reference_document().unwrap();
        let inspection = inspect(&starter, &Options::default()).unwrap();
        assert_eq!(inspection["section"]["used"], "last");
        assert!(inspection["section"]["text_width"].as_i64().unwrap() > 0);
        assert!(inspection["problems"].as_array().is_some());
        assert_eq!(inspection["created_styles"], serde_json::json!([]));
    }

    #[test]
    fn role_styles_list_every_role() {
        let roles = styles_of(None, &Options::default()).unwrap();
        assert_eq!(roles.as_array().unwrap().len(), Role::ALL.len());
    }

    #[test]
    fn every_entry_point_applies_the_limits() {
        let serverless = Options {
            limits: Some("serverless".into()),
            ..Options::default()
        };
        let big = vec![0u8; (8 << 20) + 1];
        for err in [
            styles_of(Some(&big), &serverless).unwrap_err(),
            inspect(&big, &serverless).unwrap_err(),
            convert(
                "x",
                InputFormat::Markdown,
                Options {
                    reference: Some(ByteBuf::from(big.clone())),
                    ..serverless.clone()
                },
                Timestamp::from_unix(0),
            )
            .unwrap_err(),
        ] {
            assert_eq!(err.reason(), "max_archive_bytes", "{err}");
        }
        // The default budgets read the same bytes and refuse them as a malformed
        // package, not by size.
        assert!(
            styles_of(Some(&big), &Options::default())
                .unwrap_err()
                .is_refusal()
        );
        let starter = reference_document().unwrap();
        assert!(styles_of(Some(&starter), &serverless).is_ok());
        let bad = Options {
            limits: Some("huge".into()),
            ..Options::default()
        };
        assert_eq!(
            styles_of(None, &bad).unwrap_err().code(),
            "invalid_argument"
        );
    }

    /// The page numbers the contents shows, in order.
    fn contents_numbers(docx: &[u8]) -> Vec<String> {
        let document = reopen(docx);
        let xml = String::from_utf8(docrs::xml::serialize(
            document.package().xml("word/document.xml").unwrap(),
        ))
        .unwrap();
        xml.split(" PAGEREF ")
            .skip(1)
            .map(|rest| {
                let number = &rest[rest.find("<w:t>").unwrap() + 5..];
                number[..number.find('<').unwrap()].to_string()
            })
            .collect()
    }

    #[test]
    fn supplied_fonts_measure_the_contents_numbers() {
        let paragraph = "Inter sets wider than the substitute Word uses when the font is \
                         missing, so each of these paragraphs wraps onto more lines and the \
                         later headings move to later pages.\n\n";
        let markdown: String = (1..=8)
            .map(|i| format!("# Section {i}\n\n{}", paragraph.repeat(6)))
            .collect();
        let markdown = format!("Dear {{{{Client}}}},\n\n{markdown}");
        let reference = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/pdf/references/inter.docx"),
        )
        .unwrap();
        // A filled field makes the contents numbers measured a second time.
        let convert_with = |fonts: LoadedFonts, client: Option<&str>| {
            let options = Options {
                reference: Some(ByteBuf::from(reference.clone())),
                toc: Some(true),
                fields: client
                    .map(|value| ("Client".to_string(), value.to_string()))
                    .into_iter()
                    .collect(),
                fonts,
                ..Options::default()
            };
            convert(
                &markdown,
                InputFormat::Markdown,
                options,
                Timestamp::from_unix(0),
            )
            .unwrap()
        };
        let inter = fonts::tests::font("inter/Inter[opsz,wght].ttf");
        let sizes = [inter.len() as u64];
        let (loaded, _) = fonts::load(
            [inter].into_iter(),
            &sizes,
            &FontOptions::default(),
            &FontLimits::DEFAULT,
        )
        .unwrap();
        for client in [None, Some("Jo")] {
            let without = convert_with(LoadedFonts::default(), client);
            assert_eq!(
                without.report["toc"]["missing_fonts"],
                serde_json::json!(["Inter"])
            );
            let with = convert_with(loaded.clone(), client);
            assert_eq!(with.report["toc"]["missing_fonts"], serde_json::json!([]));
            assert_ne!(
                contents_numbers(without.docx.as_deref().unwrap()),
                contents_numbers(with.docx.as_deref().unwrap())
            );
        }
    }

    #[test]
    fn the_clock_is_validated() {
        assert_eq!(
            timestamp_from_millis(1_700_000_000_999.0).unwrap().unix(),
            1_700_000_000
        );
        assert_eq!(timestamp_from_millis(0.0).unwrap().unix(), 0);
        assert!(timestamp_from_millis(MAX_NOW_MILLIS).is_ok());
        for bad in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            MAX_NOW_MILLIS + 1.0,
            1e300,
        ] {
            let err = timestamp_from_millis(bad).unwrap_err();
            assert_eq!(err.code(), "invalid_argument", "{bad}");
            assert_eq!(err.reason(), "invalid_options", "{bad}");
        }
    }
}
