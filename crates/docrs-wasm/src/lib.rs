//! WebAssembly bindings of `docrs` for Node.js: Markdown, editor HTML or
//! TipTap JSON to DOCX with a reference document, the same conversion as
//! the CLI's `from-markdown`, and the reference document inspection of
//! `markdown-inspect`.
//!
//! The conversion itself ([`convert`]) is plain Rust and tested natively;
//! the `#[wasm_bindgen]` functions only translate JavaScript values. Bytes
//! cross as `Uint8Array`; reports cross as plain objects shaped like the
//! CLI's JSON. Nothing touches a file system: the reference document and
//! images are passed in, and the time comes from JavaScript's clock.

use std::collections::BTreeMap;

use docrs::clock::Timestamp;
use docrs::markdown::{self, InputFormat, MarkdownOptions, Placement, Role, StylePolicy};
use docrs::{Document, Error, Limits, Tolerance};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use wasm_bindgen::prelude::*;

/// Options of [`from_markdown`], camelCase in JavaScript. Every field is
/// optional; the defaults match the CLI's.
#[derive(Debug, Clone, Default, Deserialize)]
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
}

/// Budgets on what the caller passes in, beyond the package budgets.
struct InputLimits {
    /// Markdown, HTML or JSON source, in bytes.
    source: u64,
    /// The reference document and all supplied images together, in bytes.
    input: u64,
}

fn limits_of(options: &Options) -> Result<(Limits, InputLimits), Error> {
    profile(options.limits.as_deref())
}

/// The budgets of the `limits` option `name`.
fn profile(name: Option<&str>) -> Result<(Limits, InputLimits), Error> {
    match name {
        None | Some("default") => Ok((
            Limits::default(),
            InputLimits {
                source: u64::MAX,
                input: u64::MAX,
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
    /// The `.docx` package.
    pub docx: Vec<u8>,
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
    let (limits, input) = limits_of(&options)?;
    check_sizes(
        &limits,
        &input,
        source.len() as u64,
        options.reference.as_ref().map_or(0, |b| b.len() as u64),
        options.images.values().map(|b| b.len() as u64).sum(),
    )?;
    let mut markdown_options = markdown_options(&options, format, now)?;
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
    let mut report = markdown::insert_source(&mut document, source, format, &markdown_options)?;
    drop(markdown_options);
    if options.redact {
        report.redact();
    }
    let (docx, _) = document.patch_bytes()?;
    drop(document);
    Ok(Converted {
        docx,
        report: report.to_json(),
        reference_set_aside,
    })
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

// ------------------------------------------------------------ JavaScript

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
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
}

/** What a conversion will take from a reference document (`inspectReference`). */
export interface ReferenceInspection {
  schema_version: 1;
  /** Role key to the style a conversion uses. */
  roles: Record<string, { name: string; style_id: string | null; created: boolean }>;
  /** Styles a conversion adds because the template lacks them. */
  created_styles: string[];
  /** Branding lifted from the template's sample content. */
  lifted: {
    body: RunLook | null;
    headings: Record<string, RunLook>;
    table: { style_id: string | null; direct_formatting_lifted: boolean; look: Record<string, boolean> };
  };
  /** The section whose page setup, headers and footers the output keeps (the last). */
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

export interface HeaderFooterSummary {
  images: number;
  tables: number;
  /** Field names such as PAGE, NUMPAGES. */
  fields: string[];
  has_text: boolean;
  /** Defined by an earlier section and inherited. */
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
}
"#;

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
    for key in ["code", "reason", "refusal", "limit"] {
        let _ = js_sys::Reflect::set(&js, &JsValue::from_str(key), &to_js(&fields[key]));
    }
    js.into()
}

/// Option names, as JavaScript writes them (`Options` in camelCase).
const OPTION_NAMES: &[&str] = &[
    "reference",
    "placeholder",
    "append",
    "styles",
    "strictStyles",
    "strict",
    "toc",
    "tocDepth",
    "tocTitle",
    "tocPageNumbers",
    "tocNumbers",
    "shiftHeadingLevel",
    "smart",
    "titleBlock",
    "properties",
    "captions",
    "templateLists",
    "pageNumbers",
    "coverPage",
    "images",
    "strictPackage",
    "acknowledgeProtection",
    "now",
    "limits",
    "redact",
];

fn to_js(value: &serde_json::Value) -> JsValue {
    use serde::Serialize;
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .unwrap_or(JsValue::NULL)
}

/// The size of a JavaScript value: a string's UTF-16 code units, else its
/// `length` (a `Uint8Array`'s or an array's elements) or `byteLength`; 0
/// when it has neither.
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
/// before any of them is copied into WebAssembly memory. A string's UTF-8
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
    .map_err(refuse)
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

fn options_from(value: Option<js_sys::Object>) -> Result<Options, JsValue> {
    let Some(value) = value else {
        return Ok(Options::default());
    };
    // Option names and types are the caller's code, not input data, but a
    // value echoed in a type error could be; honour `redact` early.
    let redact = redact_of(&value);
    // serde-wasm-bindgen reads only the fields it knows, so a misspelt
    // option would be ignored silently; refuse it instead. An option set to
    // `undefined` or `null` counts as not set, as JavaScript callers
    // forwarding optional values expect.
    let given = js_sys::Object::new();
    for key in js_sys::Object::keys(&value).iter() {
        let name = key.as_string().unwrap_or_default();
        if !OPTION_NAMES.contains(&name.as_str()) {
            return Err(js_error(&Error::invalid_argument(
                "invalid_options",
                format!("unknown option {name:?}"),
            )));
        }
        let field = js_sys::Reflect::get(&value, &key).unwrap_or(JsValue::UNDEFINED);
        if !field.is_undefined() && !field.is_null() {
            let _ = js_sys::Reflect::set(&given, &key, &field);
        }
    }
    serde_wasm_bindgen::from_value(given.into()).map_err(|err| {
        js_error_with(
            &Error::invalid_argument("invalid_options", format!("options: {err}")),
            redact,
        )
    })
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
    set(
        "docx",
        &js_sys::Uint8Array::from(converted.docx.as_slice()).into(),
    );
    set("report", &to_js(&converted.report));
    set(
        "referenceSetAside",
        &to_js(&serde_json::json!(converted.reference_set_aside)),
    );
    result.into()
}

fn run(
    source: JsValue,
    format: InputFormat,
    options: Option<js_sys::Object>,
) -> Result<JsValue, JsValue> {
    check_js_sizes(Some(&source), None, options.as_ref())?;
    let options = options_from(options)?;
    let redact = options.redact;
    let now = timestamp_from_millis(options.now.unwrap_or_else(js_sys::Date::now))
        .map_err(|err| js_error_with(&err, redact))?;
    let source = source_from(&source, redact)?;
    convert(&source, format, options, now)
        .map(converted_to_js)
        .map_err(|err| js_error_with(&err, redact))
}

/// Convert Markdown into a `.docx` styled by a reference document.
#[wasm_bindgen(js_name = fromMarkdown, unchecked_return_type = "FromMarkdownResult")]
pub fn from_markdown(
    #[wasm_bindgen(unchecked_param_type = "string")] markdown: JsValue,
    #[wasm_bindgen(unchecked_optional_param_type = "FromMarkdownOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
    run(markdown, InputFormat::Markdown, options)
}

/// Convert editor HTML (TipTap's `editor.getHTML()` and similar) into a
/// `.docx` styled by a reference document. Same options and result as
/// `fromMarkdown`; pictures get no captions unless `captions: true`.
#[wasm_bindgen(js_name = fromHtml, unchecked_return_type = "FromMarkdownResult")]
pub fn from_html(
    #[wasm_bindgen(unchecked_param_type = "string")] html: JsValue,
    #[wasm_bindgen(unchecked_optional_param_type = "FromMarkdownOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
    run(html, InputFormat::Html, options)
}

/// Convert a TipTap document (`editor.getJSON()`, as the object or as JSON
/// text) into a `.docx` styled by a reference document. Same options and
/// result as `fromMarkdown`; pictures get no captions unless `captions: true`.
#[wasm_bindgen(js_name = fromJson, unchecked_return_type = "FromMarkdownResult")]
pub fn from_json(
    #[wasm_bindgen(unchecked_param_type = "object | string")] document: JsValue,
    #[wasm_bindgen(unchecked_optional_param_type = "FromMarkdownOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
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
#[wasm_bindgen(js_name = fromTiptap, unchecked_return_type = "FromMarkdownResult")]
pub fn from_tiptap(
    #[wasm_bindgen(unchecked_param_type = "object | string")] document: JsValue,
    #[wasm_bindgen(unchecked_optional_param_type = "FromMarkdownOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
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
    #[wasm_bindgen(unchecked_optional_param_type = "FromMarkdownOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
    check_js_sizes(None, Some(&reference), options.as_ref())?;
    let options = options_from(options)?;
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
    #[wasm_bindgen(unchecked_optional_param_type = "FromMarkdownOptions")] options: Option<
        js_sys::Object,
    >,
) -> Result<JsValue, JsValue> {
    let reference = Some(reference).filter(|r| !r.is_undefined() && !r.is_null());
    check_js_sizes(None, reference.as_ref(), options.as_ref())?;
    let redact = options.as_ref().is_some_and(redact_of);
    let mut options = options_from(options)?;
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

/// Runs when the module is instantiated. The build aborts on a panic (a bug):
/// the instance traps with `RuntimeError: unreachable` and is unusable
/// afterwards, and the package's entry point (`js/index.js`) replaces it
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
        let document = reopen(&converted.docx);
        let core = String::from_utf8(docrs::xml::serialize(
            document.package().xml("docProps/core.xml").unwrap(),
        ))
        .unwrap();
        assert!(core.contains("Plan"), "{core}");
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
    fn option_names_match_the_fields() {
        let fields: Vec<String> = serde_json::to_value(OPTION_NAMES)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect();
        // Every listed name deserializes; a field missing from the list
        // would be refused before reaching serde.
        for name in &fields {
            let value = match name.as_str() {
                "styles" | "images" => serde_json::json!({}),
                "placeholder" | "tocTitle" => serde_json::json!("x"),
                "limits" => serde_json::json!("serverless"),
                "tocDepth" | "shiftHeadingLevel" | "now" => serde_json::json!(1),
                "reference" => serde_json::Value::Null,
                _ => serde_json::json!(true),
            };
            let object = serde_json::json!({ name.as_str(): value });
            assert!(serde_json::from_value::<Options>(object).is_ok(), "{name}");
        }
        let source = include_str!("lib.rs");
        let start = source.find("pub struct Options {").unwrap();
        let body = &source[start..start + source[start..].find("\n}").unwrap()];
        let declared = body.matches("    pub ").count();
        assert_eq!(declared, OPTION_NAMES.len());
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
        // Budgets on what the caller passes in.
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
            reopen(&converted.docx)
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
        // Sizes alone refuse; the bytes are never parsed.
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
        // The default budgets read the same bytes (and refuse them as a
        // malformed package, not by size).
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
