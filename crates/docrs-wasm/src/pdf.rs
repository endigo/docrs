//! PDF output (experimental): `toPdf`, the `pdf` option of the conversions
//! and `usedFonts`.

use docrs::layout::FontSet;
use docrs::pdf::{PdfLimits, PdfOptions};
use docrs::{Document, Error, ErrorDetails};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::fonts::LoadedFonts;
use crate::{
    InputFormat, Options, build, check_js_sizes, check_sizes, js_error, js_error_with,
    open_reference, options_from, profile, redact_of, timestamp_from_millis, to_js,
};

/// The `pdf` option: `true`, or the PDF's own options.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PdfChoice {
    Wanted(bool),
    With(PdfSettings),
}

impl PdfChoice {
    pub fn settings(&self) -> Option<&PdfSettings> {
        const DEFAULT: PdfSettings = PdfSettings {
            strict: None,
            max_pages: None,
            max_output_bytes: None,
            max_image_pixels: None,
            max_document_pixels: None,
        };
        match self {
            PdfChoice::Wanted(true) => Some(&DEFAULT),
            PdfChoice::Wanted(false) => None,
            PdfChoice::With(settings) => Some(settings),
        }
    }
}

/// How to draw the PDF. A cap lowers the `limits` profile's and never
/// raises it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct PdfSettings {
    /// Refuse on any PDF warning (default: the conversion's `strict`).
    pub strict: Option<bool>,
    pub max_pages: Option<u32>,
    pub max_output_bytes: Option<u64>,
    pub max_image_pixels: Option<u64>,
    pub max_document_pixels: Option<u64>,
}

impl PdfSettings {
    fn capped(&self, mut limits: PdfLimits) -> PdfLimits {
        let lower = |cap: &mut u64, to: Option<u64>| *cap = to.map_or(*cap, |to| to.min(*cap));
        limits.max_pages = self
            .max_pages
            .map_or(limits.max_pages, |to| to.min(limits.max_pages));
        lower(&mut limits.max_output_bytes, self.max_output_bytes);
        lower(&mut limits.max_image_pixels, self.max_image_pixels);
        lower(&mut limits.max_document_pixels, self.max_document_pixels);
        limits
    }
}

/// A drawn PDF and its report (`PdfReport::to_json`).
#[derive(Debug)]
pub struct Pdf {
    pub bytes: Vec<u8>,
    pub report: serde_json::Value,
}

/// `document` as a PDF in `fonts`, under `limits` lowered by `settings`.
pub fn render(
    document: &Document,
    settings: &PdfSettings,
    strict: bool,
    fonts: &FontSet,
    limits: PdfLimits,
    redact: bool,
) -> Result<Pdf, Error> {
    let mut options = PdfOptions::default();
    options.fonts = fonts.clone();
    options.strict = settings.strict.unwrap_or(strict);
    options.limits = settings.capped(limits);
    let (bytes, mut report) = docrs::pdf::render(document, &options)?;
    if redact {
        report.redact();
    }
    Ok(Pdf {
        bytes,
        report: report.to_json(),
    })
}

/// Options of `toPdf`, camelCase in JavaScript: the PDF's own and how to
/// read the `.docx`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ToPdfOptions {
    pub strict: bool,
    pub max_pages: Option<u32>,
    pub max_output_bytes: Option<u64>,
    pub max_image_pixels: Option<u64>,
    pub max_document_pixels: Option<u64>,
    pub limits: Option<String>,
    pub strict_package: bool,
    pub redact: bool,
    #[serde(skip)]
    pub fonts: LoadedFonts,
}

impl ToPdfOptions {
    fn settings(&self) -> PdfSettings {
        PdfSettings {
            strict: None,
            max_pages: self.max_pages,
            max_output_bytes: self.max_output_bytes,
            max_image_pixels: self.max_image_pixels,
            max_document_pixels: self.max_document_pixels,
        }
    }
}

/// A `.docx` drawn as a PDF.
#[derive(Debug)]
pub struct ToPdf {
    pub pdf: Pdf,
    pub reference_set_aside: Vec<String>,
}

/// `docx` as a PDF, as `docrs pdf` draws it.
pub fn to_pdf(docx: Vec<u8>, options: ToPdfOptions) -> Result<ToPdf, Error> {
    let (limits, input) = profile(options.limits.as_deref())?;
    check_sizes(&limits, &input, 0, docx.len() as u64, 0)?;
    options.fonts.check(&input.fonts)?;
    let (document, reference_set_aside) =
        open_reference(Some(&docx), options.strict_package, &limits)?;
    drop(docx);
    let pdf = render(
        &document,
        &options.settings(),
        options.strict,
        &options.fonts.set,
        input.pdf,
        options.redact,
    )?;
    Ok(ToPdf {
        pdf,
        reference_set_aside,
    })
}

/// A family and style a PDF of the document draws with no font program
/// the document embeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsedFont {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// The families and styles `document` needs font programs for.
// Until `pdf::used_fonts` lands, read off the refusal of a PDF drawn
// without fonts.
pub fn used_fonts(document: &Document, limits: PdfLimits) -> Result<Vec<UsedFont>, Error> {
    let mut options = PdfOptions::default();
    options.limits = limits;
    match docrs::pdf::render(document, &options) {
        Ok(_) => Ok(Vec::new()),
        Err(err) if err.reason() == "pdf_font_program_missing" => Ok(err
            .details()
            .map_or(&[][..], |details: &ErrorDetails| &details.missing_fonts)
            .iter()
            .map(|font| UsedFont {
                family: font.family.clone(),
                bold: font.bold,
                italic: font.italic,
            })
            .collect()),
        Err(err) => Err(err),
    }
}

/// Options of `usedFonts` for a `.docx`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct DocxOptions {
    pub limits: Option<String>,
    pub strict_package: bool,
    pub redact: bool,
}

fn docx_fonts(docx: Vec<u8>, options: &DocxOptions) -> Result<Vec<UsedFont>, Error> {
    let (limits, input) = profile(options.limits.as_deref())?;
    check_sizes(&limits, &input, 0, docx.len() as u64, 0)?;
    let (document, _) = open_reference(Some(&docx), options.strict_package, &limits)?;
    drop(docx);
    used_fonts(&document, input.pdf)
}

/// The families and styles converting `source` would draw with no font
/// program its reference embeds.
pub fn source_fonts(
    source: &str,
    format: InputFormat,
    mut options: Options,
    now: docrs::clock::Timestamp,
) -> Result<Vec<UsedFont>, Error> {
    let (_, input) = profile(options.limits.as_deref())?;
    let built = build(source, format, &mut options, now)?;
    used_fonts(&built.document, input.pdf)
}

pub fn pdf_to_js(pdf: Pdf, result: &js_sys::Object) {
    let set = |key: &str, value: &JsValue| {
        let _ = js_sys::Reflect::set(result, &JsValue::from_str(key), value);
    };
    set(
        "pdf",
        &js_sys::Uint8Array::from(pdf.bytes.as_slice()).into(),
    );
    set("pdfReport", &to_js(&pdf.report));
}

/// Draw a `.docx` as a PDF (experimental).
#[wasm_bindgen(js_name = toPdf, skip_typescript)]
pub fn to_pdf_js(docx: JsValue, options: Option<js_sys::Object>) -> Result<JsValue, JsValue> {
    check_js_sizes(None, Some(&docx), options.as_ref())?;
    let (options, fonts) = options_from::<ToPdfOptions>(options)?;
    let redact = options.redact;
    let docx = crate::bytes_from(&docx, "docx")?;
    let converted = to_pdf(
        docx,
        ToPdfOptions {
            fonts: fonts.unwrap_or_default(),
            ..options
        },
    )
    .map_err(|err| js_error_with(&err, redact))?;
    let result = js_sys::Object::new();
    pdf_to_js(converted.pdf, &result);
    let _ = js_sys::Reflect::set(
        &result,
        &"referenceSetAside".into(),
        &to_js(&serde_json::json!(converted.reference_set_aside)),
    );
    Ok(result.into())
}

const SOURCES: [(&str, InputFormat); 3] = [
    ("html", InputFormat::Html),
    ("json", InputFormat::Tiptap),
    ("markdown", InputFormat::Markdown),
];

/// The families and styles a PDF of `input` needs font programs for
/// (experimental): a `.docx`, or `{ html | json | markdown, ...options }`.
#[wasm_bindgen(js_name = usedFonts, skip_typescript)]
pub fn used_fonts_js(input: JsValue, options: Option<js_sys::Object>) -> Result<JsValue, JsValue> {
    if input.is_instance_of::<js_sys::Uint8Array>() {
        check_js_sizes(None, Some(&input), options.as_ref())?;
        let (options, _) = options_from::<DocxOptions>(options)?;
        let docx = crate::bytes_from(&input, "docx")?;
        return docx_fonts(docx, &options)
            .map(|fonts| to_js(&serde_json::json!(fonts)))
            .map_err(|err| js_error_with(&err, options.redact));
    }
    let invalid = |message: &str| js_error(&Error::invalid_argument("invalid_source", message));
    let object = input.dyn_into::<js_sys::Object>().map_err(|_| {
        invalid("usedFonts takes a .docx or { html | json | markdown, ...options }")
    })?;
    if options.is_some() {
        return Err(invalid(
            "usedFonts takes the options of a source inside its object",
        ));
    }
    let given: Vec<_> = SOURCES
        .iter()
        .filter(|(key, _)| js_sys::Reflect::has(&object, &JsValue::from_str(key)).unwrap_or(false))
        .collect();
    let [(key, format)] = given[..] else {
        return Err(invalid(
            "usedFonts needs exactly one of html, json and markdown",
        ));
    };
    let rest = js_sys::Object::assign(&js_sys::Object::new(), &object);
    let source = js_sys::Reflect::get(&rest, &JsValue::from_str(key)).unwrap_or_default();
    let _ = js_sys::Reflect::delete_property(&rest, &JsValue::from_str(key));
    let source = match (*format, source.as_string()) {
        (_, Some(text)) => JsValue::from(text),
        (InputFormat::Tiptap, None) if source.is_object() => js_sys::JSON::stringify(&source)
            .map(JsValue::from)
            .map_err(|_| invalid("the document could not be serialized as JSON"))?,
        _ => return Err(invalid("the source must be a string")),
    };
    check_js_sizes(Some(&source), None, Some(&rest))?;
    let redact = redact_of(&rest);
    let (options, fonts) = options_from::<Options>(Some(rest))?;
    if fonts.is_some() || options.pdf.is_some() || options.docx.is_some() {
        return Err(js_error(&Error::invalid_argument(
            "invalid_options",
            "usedFonts takes no fonts, pdf or docx",
        )));
    }
    let now = timestamp_from_millis(options.now.unwrap_or_else(js_sys::Date::now))
        .map_err(|err| js_error_with(&err, redact))?;
    let source = source.as_string().unwrap_or_default();
    source_fonts(&source, *format, options, now)
        .map(|fonts| to_js(&serde_json::json!(fonts)))
        .map_err(|err| js_error_with(&err, redact))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use docrs::clock::Timestamp;
    use serde_bytes::ByteBuf;

    use super::*;
    use crate::convert;
    use crate::fonts::tests::font;
    use crate::fonts::{FontLimits, FontOptions, load};

    const INTER: &str = "inter/Inter[opsz,wght].ttf";

    fn fixture(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(path)
    }

    pub(crate) fn inter() -> LoadedFonts {
        let file = font(INTER);
        let sizes = [file.len() as u64];
        let options = FontOptions::default();
        load([file].into_iter(), &sizes, &options, &FontLimits::DEFAULT)
            .unwrap()
            .0
    }

    pub(crate) fn options(pdf: Option<PdfChoice>) -> Options {
        Options {
            reference: Some(ByteBuf::from(
                std::fs::read(fixture("pdf/references/inter.docx")).unwrap(),
            )),
            pdf,
            fonts: inter(),
            ..Options::default()
        }
    }

    const SOURCE: &str = "# Advice\n\nPrepared for **Jo** on 28 November.\n\n1. One\n2. Two\n";

    fn docx() -> Vec<u8> {
        let converted = convert(
            SOURCE,
            InputFormat::Markdown,
            options(None),
            Timestamp::from_unix(0),
        );
        converted.unwrap().docx.unwrap()
    }

    fn with_inter() -> ToPdfOptions {
        ToPdfOptions {
            fonts: inter(),
            ..ToPdfOptions::default()
        }
    }

    #[test]
    fn a_conversion_draws_its_document_as_a_pdf_and_may_leave_the_docx_out() {
        let both = convert(
            SOURCE,
            InputFormat::Markdown,
            options(Some(PdfChoice::Wanted(true))),
            Timestamp::from_unix(0),
        )
        .unwrap();
        let pdf = both.pdf.unwrap();
        assert!(pdf.bytes.starts_with(b"%PDF-"));
        assert_eq!(pdf.report["pages"], 1);
        assert_eq!(both.docx.unwrap(), docx());
        let mut only = options(None);
        only.pdf = Some(PdfChoice::With(PdfSettings::default()));
        only.docx = Some(false);
        let only = convert(SOURCE, InputFormat::Markdown, only, Timestamp::from_unix(0)).unwrap();
        assert!(only.docx.is_none());
        assert_eq!(only.pdf.unwrap().bytes, pdf.bytes);
        assert_eq!(only.report, both.report);
        let mut neither = options(Some(PdfChoice::Wanted(false)));
        neither.docx = Some(false);
        let err = convert(
            SOURCE,
            InputFormat::Markdown,
            neither,
            Timestamp::from_unix(0),
        );
        assert_eq!(err.unwrap_err().reason(), "invalid_options");
    }

    #[test]
    fn a_missing_font_refuses_naming_each_family_and_style() {
        let err = to_pdf(docx(), ToPdfOptions::default()).unwrap_err();
        assert_eq!(err.reason(), "pdf_font_program_missing");
        let missing = &err.to_json()["error"]["details"]["missing_fonts"];
        assert_eq!(
            missing,
            &serde_json::json!([
                { "family": "Inter", "bold": false, "italic": false },
                { "family": "Inter", "bold": true, "italic": false },
            ])
        );
        let document = Document::open_bytes(&docx(), &docrs::Limits::default()).unwrap();
        let used = used_fonts(&document, PdfLimits::default()).unwrap();
        assert_eq!(serde_json::json!(used), *missing);
        let used = source_fonts(
            SOURCE,
            InputFormat::Markdown,
            options(None),
            Timestamp::from_unix(0),
        );
        assert_eq!(serde_json::json!(used.unwrap()), *missing);
        assert!(to_pdf(docx(), with_inter()).is_ok());
    }

    #[test]
    fn the_smaller_page_cap_wins() {
        let long = format!("{}# End\n", "A paragraph of filler text.\n\n".repeat(80));
        let mut capped = options(Some(PdfChoice::With(PdfSettings {
            max_pages: Some(1),
            ..PdfSettings::default()
        })));
        capped.limits = Some("serverless".into());
        let err = convert(
            &long,
            InputFormat::Markdown,
            capped,
            Timestamp::from_unix(0),
        );
        assert_eq!(err.unwrap_err().reason(), "max_pdf_pages");
        let docx = convert(
            &long,
            InputFormat::Markdown,
            options(None),
            Timestamp::from_unix(0),
        )
        .unwrap()
        .docx
        .unwrap();
        let roomy = ToPdfOptions {
            max_pages: Some(u32::MAX),
            limits: Some("serverless".into()),
            ..with_inter()
        };
        assert!(to_pdf(docx, roomy).unwrap().pdf.report["pages"].as_u64() > Some(1));
    }

    /// The `docrs` command `cargo test --workspace` builds in this test's
    /// profile directory: the nearest directory above the test binary that
    /// holds it (`target/debug/deps/..` before Cargo's build-dir layout,
    /// `target/debug/build/docrs-wasm/<hash>/../../..` since).
    fn cli() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let name = format!("docrs{}", std::env::consts::EXE_SUFFIX);
        exe.ancestors()
            .skip(1)
            .map(|dir| dir.join(&name))
            .find(|cli| cli.is_file())
            .unwrap_or_else(|| panic!("no {name} above {}: build docrs-cli first", exe.display()))
    }

    #[test]
    fn to_pdf_draws_the_bytes_docrs_pdf_writes() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.docx");
        let out = dir.path().join("out.pdf");
        std::fs::write(&input, docx()).unwrap();
        let status = std::process::Command::new(cli())
            .arg("pdf")
            .arg(&input)
            .arg("--out")
            .arg(&out)
            .arg("--font")
            .arg(fixture("fonts").join(INTER))
            .status()
            .unwrap();
        assert!(status.success());
        let wasm = to_pdf(docx(), with_inter()).unwrap();
        assert_eq!(wasm.pdf.bytes, std::fs::read(out).unwrap());
    }
}
