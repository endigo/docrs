//! The font chain: which face draws each character, the fidelity each
//! source gives, symbols, licences, and what the report and refusals say.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
mod support;
#[allow(dead_code)]
#[path = "support/glyphs.rs"]
mod text;

use std::collections::BTreeSet;
use std::path::PathBuf;

use docrs::anchor::BlockTarget;
use docrs::clock::Timestamp;
use docrs::layout::{FontKind, FontSet, FontSetBuilder};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{PdfOptions, PdfReport, render, used_fonts};
use docrs::{Document, Limits, Tolerance};
use reader::{Pdf, Value};
use text::{
    Glyph, SYMBOLS, assert_same_positions, expected, glyphs, page_fonts, qpdf_check, records,
};

const INTER: &str = "inter/Inter[opsz,wght].ttf";
const SERIF: &str = "liberation/LiberationSerif-Regular.ttf";
const SANS: &str = "liberation/LiberationSans-Regular.ttf";
const SOURCE_SANS: &str = "source-sans/SourceSans3-Regular.otf";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn font(path: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("fonts").join(path)).unwrap()
}

/// The fonts at `files`, configured by `with`.
fn set(files: Vec<Vec<u8>>, with: impl FnOnce(&mut FontSetBuilder)) -> FontSet {
    let mut builder = FontSet::builder();
    for file in files {
        builder.add(file).unwrap();
    }
    with(&mut builder);
    builder.build()
}

fn fonts(paths: &[&str]) -> FontSet {
    set(paths.iter().map(|path| font(path)).collect(), |_| {})
}

/// `source` converted into the reference `reference`, at a fixed time.
fn convert(source: &str, format: InputFormat, reference: &str) -> Document {
    let path = fixtures().join("pdf/references").join(reference);
    let mut document = Document::open_path_with(&path, &Limits::default(), Tolerance::word())
        .unwrap()
        .0;
    let options = MarkdownOptions {
        now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
        ..MarkdownOptions::default()
    };
    insert_source_with(&mut document, source, format, &options, &FontSet::default()).unwrap();
    document
}

fn html(body: &str) -> Document {
    convert(body, InputFormat::Html, "inter.docx")
}

/// A paragraph of `text` in `family`.
fn para(family: &str, text: &str) -> String {
    format!(r#"<p><span style="font-family: '{family}'">{text}</span></p>"#)
}

fn options(fonts: &FontSet) -> PdfOptions {
    let mut options = PdfOptions::default();
    options.fonts = fonts.clone();
    options
}

/// The PDF, its report, and its glyphs, which are where layout placed them.
fn pdf(document: &Document, options: &PdfOptions) -> (Vec<u8>, PdfReport, Vec<Glyph>) {
    let (bytes, report) = render(document, options).unwrap();
    qpdf_check("chain.pdf", &bytes);
    let drawn = glyphs(&Pdf::parse(&bytes).unwrap());
    assert_same_positions("pdf", &drawn, &expected(&records(document, &options.fonts)));
    (bytes, report, drawn.concat())
}

fn codes(report: &PdfReport) -> Vec<(&'static str, usize)> {
    report.warnings.iter().map(|w| (w.code, w.count)).collect()
}

/// Each font entry as (requested, selected, source).
fn sources(report: &PdfReport) -> Vec<(String, Option<String>, &'static str)> {
    report
        .fonts
        .iter()
        .map(|f| {
            (
                f.requested_family.clone(),
                f.selected_family.clone(),
                f.source,
            )
        })
        .collect()
}

/// Each line's text and character origins.
fn breaks(document: &Document, fonts: &FontSet) -> Vec<(String, Vec<f64>)> {
    records(document, fonts)
        .0
        .into_iter()
        .map(|line| {
            let origins = line
                .runs
                .iter()
                .flat_map(|run| run.origins.clone())
                .collect();
            (line.text, origins)
        })
        .collect()
}

const PLAIN: &str = "The quarterly report covers revenue, costs and the outlook for the \
next two quarters, with each region's figures set against the plan agreed in January.\n\n\
Office moves start on the fifth of the month; every team receives a floor plan, a packing \
list and the date its desks will be ready, and questions go to the operations desk.\n";

#[test]
fn a_supplied_font_draws_its_family_with_no_warning() {
    let fonts = fonts(&[INTER]);
    let (_, report, _) = pdf(
        &html(&para("Inter", "Inter, as uploaded")),
        &options(&fonts),
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(sources(&report).contains(&("Inter".into(), Some("Inter".into()), "supplied")));
}

#[test]
fn a_metric_alias_breaks_lines_as_words_font_does() {
    let document = convert(PLAIN, InputFormat::Markdown, "times-new-roman.docx");
    let aliased = set(vec![font(SERIF)], |b| {
        b.metric_aliases();
    });
    assert_eq!(
        breaks(&document, &aliased),
        breaks(&document, &FontSet::default())
    );
    let (_, report, _) = pdf(&document, &options(&aliased));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let alias = (
        "Times New Roman".to_string(),
        Some("Liberation Serif".to_string()),
        "alias",
    );
    assert!(sources(&report).contains(&alias), "{:?}", sources(&report));
    let mut redacted = report.clone();
    redacted.redact();
    assert_eq!(
        sources(&redacted),
        sources(&report),
        "bundled names are public"
    );
}

#[test]
fn a_declared_fallback_draws_a_missing_family_and_says_so() {
    let document = html(&para("Calibri", "Calibri was never uploaded"));
    let refused = render(&document, &options(&fonts(&[INTER]))).unwrap_err();
    assert_eq!(refused.reason(), "pdf_font_program_missing");
    let fallback = set(vec![font(INTER)], |b| {
        b.fallback(FontKind::Sans, "Inter");
    });
    assert_ne!(
        breaks(&document, &fallback),
        breaks(&document, &FontSet::default())
    );
    let (_, report, _) = pdf(&document, &options(&fallback));
    assert_eq!(codes(&report), [("pdf_font_substituted", 26)]);
    let message = &report.warnings[0].message;
    assert!(
        message.contains("Calibri Regular (drawn in Inter)"),
        "{message}"
    );
    assert!(sources(&report).contains(&("Calibri".into(), Some("Inter".into()), "fallback")));
    let mut strict = options(&fallback);
    strict.strict = true;
    assert_eq!(
        render(&document, &strict).unwrap_err().reason(),
        "pdf_not_faithful"
    );
    let mut redacted = report.clone();
    redacted.redact();
    assert_eq!(
        redacted
            .fonts
            .iter()
            .find(|f| f.source == "fallback")
            .unwrap()
            .selected_family
            .as_deref(),
        Some("redacted")
    );
}

#[test]
fn the_refusal_names_every_missing_face() {
    let document = html(&format!(
        "{}<p><span style=\"font-family: 'Calibri'\"><strong>bold</strong></span></p>{}",
        para("Calibri", "regular"),
        para("Brand Serif", "<em>italic</em>")
    ));
    let error = render(&document, &options(&fonts(&[INTER]))).unwrap_err();
    assert_eq!(error.reason(), "pdf_font_program_missing");
    let missing: BTreeSet<(String, bool, bool)> = error
        .details()
        .unwrap()
        .missing_fonts
        .iter()
        .map(|f| (f.family.clone(), f.bold, f.italic))
        .collect();
    let expected = BTreeSet::from([
        ("Brand Serif".to_string(), false, true),
        ("Calibri".to_string(), false, false),
        ("Calibri".to_string(), true, false),
    ]);
    assert_eq!(missing, expected);
    for name in ["Brand Serif Italic", "Calibri Regular", "Calibri Bold"] {
        assert!(error.message().contains(name), "{}", error.message());
    }
}

#[test]
fn a_character_its_face_lacks_is_measured_and_drawn_by_the_next_face() {
    let document = html(&format!(
        "{}{}",
        para("Liberation Serif", "Plain line"),
        para("Liberation Serif", "Checked \u{2713} line")
    ));
    let without = fonts(&[SERIF, INTER]);
    let error = render(&document, &options(&without)).unwrap_err();
    assert_eq!(error.reason(), "pdf_glyph_missing");
    assert!(
        error.message().contains("U+2713 (Liberation Serif)"),
        "{}",
        error.message()
    );
    let fallback = set(vec![font(SERIF), font(INTER)], |b| {
        b.fallback(FontKind::Sans, "Inter");
    });
    let (lines, _) = records(&document, &fallback);
    let (plain, checked) = (&lines[0], &lines[1]);
    let face = |path: &str| docrs::layout::Face::parse_file(&font(path)).remove(0);
    let (serif, sans) = (face(SERIF), face(INTER));
    let descent = |f: &docrs::layout::Face| f.line_height() - f.ascent();
    let mixed = serif.ascent().max(sans.ascent()) + descent(&serif).max(descent(&sans));
    let ratio = checked.height / plain.height;
    assert!(
        (ratio - mixed / serif.line_height()).abs() < 1e-6,
        "{} → {}: {ratio}",
        plain.height,
        checked.height
    );
    let (_, report, drawn) = pdf(&document, &options(&fallback));
    assert_eq!(codes(&report), [("pdf_glyph_missing", 1)]);
    assert!(
        report.warnings[0]
            .message
            .contains("U+2713 (Liberation Serif, drawn in Inter)")
    );
    let check = drawn.iter().find(|g| g.text == "\u{2713}").unwrap();
    let letter = drawn.iter().find(|g| g.text == "C").unwrap();
    assert_ne!(check.font, letter.font);
}

/// `font` with its OS/2 `fsType` set to `fs_type`.
fn with_fs_type(font: &[u8], fs_type: u16) -> Vec<u8> {
    let be = |at: usize, n: usize| font[at..at + n].iter().fold(0, |v, b| v << 8 | *b as usize);
    let os2 = (0..be(4, 2))
        .map(|index| 12 + 16 * index)
        .find(|record| &font[*record..record + 4] == b"OS/2")
        .map(|record| be(record + 8, 4))
        .unwrap();
    assert!(be(os2, 2) >= 2, "fsType bits 8 and 9 need OS/2 version 2");
    let mut out = font.to_vec();
    out[os2 + 8..os2 + 10].copy_from_slice(&fs_type.to_be_bytes());
    out
}

fn serif_paragraph() -> Document {
    html(&para(
        "Liberation Serif",
        "Licensed text, measured as uploaded",
    ))
}

#[test]
fn a_font_whose_licence_forbids_embedding_measures_and_the_next_face_draws() {
    let document = serif_paragraph();
    for fs_type in [0x0002, 0x0200] {
        let restricted = with_fs_type(&font(SERIF), fs_type);
        let alone = set(vec![restricted.clone()], |_| {});
        let error = render(&document, &options(&alone)).unwrap_err();
        assert_eq!(error.reason(), "pdf_font_program_missing");
        assert!(error.message().contains("licence"), "{}", error.message());
        let fallback = set(vec![restricted, font(INTER)], |b| {
            b.fallback(FontKind::Serif, "Inter");
        });
        assert_eq!(
            breaks(&document, &fallback),
            breaks(&document, &fonts(&[SERIF]))
        );
        let (bytes, report, drawn) = pdf(&document, &options(&fallback));
        assert_eq!(
            codes(&report),
            [("pdf_font_embedding_restricted", drawn.len())],
            "{fs_type:#06x}"
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("LiberationSerif"));
    }
}

#[test]
fn allow_unembedded_references_a_font_by_name_with_layouts_widths() {
    let document = serif_paragraph();
    let restricted = set(vec![with_fs_type(&font(SERIF), 0x0002)], |_| {});
    let mut options = options(&restricted);
    options.allow_unembedded = true;
    let (bytes, report, drawn) = pdf(&document, &options);
    assert_eq!(codes(&report), [("pdf_font_not_embedded", drawn.len())]);
    let pdf = Pdf::parse(&bytes).unwrap();
    let page = pdf.pages().unwrap()[0];
    let fonts = page_fonts(&pdf, page);
    let named = &fonts[&drawn[0].font];
    assert_eq!(named.subtype, "TrueType");
    assert!(String::from_utf8_lossy(&bytes).contains("/BaseFont /LiberationSerif"));
    assert!(!String::from_utf8_lossy(&bytes).contains("/FontFile"));
    let missing = html(&para("Calibri", "Calibri by name"));
    let (_, report, _) = self::pdf(&missing, &options);
    assert!(sources(&report).contains(&("Calibri".into(), Some("Calibri".into()), "not_embedded")));
}

#[test]
fn a_font_that_forbids_subsetting_is_embedded_whole() {
    let whole = with_fs_type(&font(SERIF), 0x0100);
    let fonts = set(vec![whole.clone()], |_| {});
    let (bytes, report, _) = pdf(&serif_paragraph(), &options(&fonts));
    assert!(report.warnings.is_empty());
    let pdf = Pdf::parse(&bytes).unwrap();
    let page = pdf.pages().unwrap()[0];
    let resources = pdf
        .resolve(pdf.page_attr(page, "Resources").unwrap())
        .dict()
        .unwrap();
    let fonts = pdf.get(resources, "Font").dict().unwrap();
    let type0 = pdf.resolve(fonts.values().next().unwrap()).dict().unwrap();
    let Value::Array(descendants) = pdf.get(type0, "DescendantFonts") else {
        panic!("no descendants");
    };
    let cid_font = pdf.resolve(&descendants[0]).dict().unwrap();
    let descriptor = pdf.get(cid_font, "FontDescriptor").dict().unwrap();
    let file = pdf.get(descriptor, "FontFile2").dict().unwrap();
    assert_eq!(pdf.get(file, "Length1").number(), Some(whole.len() as f64));
    let Value::Name(base_font) = pdf.get(cid_font, "BaseFont") else {
        panic!("no BaseFont");
    };
    assert!(!base_font.contains('+'), "{base_font}");
}

#[test]
fn a_style_the_family_has_but_nobody_supplied_is_substituted() {
    let document = html(&para("Arial", "Arial <strong>bold</strong>"));
    let aliased = set(vec![font(SANS)], |b| {
        b.metric_aliases();
    });
    let (_, report, _) = pdf(&document, &options(&aliased));
    assert_eq!(codes(&report), [("pdf_font_substituted", 4)]);
    assert!(
        report.warnings[0]
            .message
            .contains("Arial Bold (drawn from Regular)")
    );
    let bold = report.fonts.iter().find(|f| f.bold).unwrap();
    assert!(bold.synthesized && bold.source == "alias");
    let unknown = html(&para(
        "Source Sans 3",
        "<strong>Bold</strong> Word cannot know",
    ));
    let (_, report, _) = pdf(&unknown, &options(&fonts(&[SOURCE_SANS])));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.fonts.iter().any(|f| f.bold && f.synthesized));
}

const LISTS: &str = "- first\n- second\n- third\n\n- [ ] open task\n- [x] done task\n";

/// The converter's lists, the second and third items moved to levels 1
/// and 2 of the library's bullet list (Courier New `o`, Wingdings ▪).
fn lists() -> Document {
    let mut document = convert(LISTS, InputFormat::Markdown, "inter.docx");
    let bullets = document.ensure_bullet_definition().unwrap();
    for (text, level) in [("second", 1), ("third", 2)] {
        document
            .apply_numbering(BlockTarget::Text(text), bullets, level)
            .unwrap();
    }
    document
}

#[test]
fn bulleted_and_task_lists_draw_their_symbols_with_no_warning_under_strict() {
    let document = lists();
    let mut strict = options(&fonts(&[INTER]));
    strict.strict = true;
    let (bytes, report, drawn) = pdf(&document, &strict);
    assert!(report.warnings.is_empty());
    let symbols: Vec<&str> = drawn
        .iter()
        .filter(|g| g.font == SYMBOLS)
        .map(|g| g.text.as_str())
        .collect();
    assert_eq!(
        symbols,
        [
            "\u{2022}", "o", "\u{25AA}", "\u{2022}", "\u{2610}", "\u{2022}", "\u{2612}"
        ]
    );
    for family in ["Symbol", "Courier New", "Wingdings", "MS Gothic"] {
        assert!(
            sources(&report).contains(&(family.into(), None, "symbols")),
            "{family}: {:?}",
            sources(&report)
        );
    }
    assert!(bytes.windows(6).any(|w| w == b"/Type3"));
    let used = used_fonts(&document).unwrap();
    let optional: Vec<(&str, bool)> = used
        .iter()
        .map(|u| (u.family.as_str(), u.optional))
        .collect();
    assert_eq!(
        optional,
        [
            ("Symbol", true),
            ("Inter", false),
            ("Courier New", true),
            ("Wingdings", true),
            ("MS Gothic", true)
        ]
    );
    assert!(used.iter().all(|u| !u.embedded));
}

/// A document of `body`'s runs, each `(rFonts, content)`.
fn runs(runs: &[(&str, &str)]) -> Document {
    let body: String = runs
        .iter()
        .map(|(font, content)| {
            format!(
                r#"<w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/></w:rPr>{content}</w:r>"#
            )
        })
        .collect();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p>{body}</w:p></w:body></w:document>"#
    );
    Document::open_bytes(&support::minimal_package_with(&xml), &Limits::default()).unwrap()
}

#[test]
fn other_symbols_warn_and_extract_as_what_they_stand_for() {
    let document = runs(&[
        ("Inter", r#"<w:t xml:space="preserve">Fly </w:t>"#),
        ("Inter", r#"<w:sym w:font="Wingdings" w:char="F051"/>"#),
        ("Symbol", "<w:t>p</w:t>"),
        ("Inter", r#"<w:sym w:font="Symbol" w:char="F0B7"/>"#),
    ]);
    assert!(
        docrs::pdf::preflight(&document)
            .unwrap()
            .warnings
            .is_empty()
    );
    let fallback = set(vec![font(INTER)], |b| {
        b.fallback(FontKind::Serif, "Inter");
    });
    let (_, report, drawn) = pdf(&document, &options(&fallback));
    assert_eq!(codes(&report), [("pdf_symbol_substituted", 2)]);
    let message = &report.warnings[0].message;
    assert!(message.contains("U+F051 in Wingdings") && message.contains("U+0070 in Symbol"));
    let texts: Vec<(&str, bool)> = drawn
        .iter()
        .map(|g| (g.text.as_str(), g.font == SYMBOLS))
        .collect();
    assert_eq!(
        texts,
        [
            ("F", false),
            ("l", false),
            ("y", false),
            (" ", false),
            ("\u{2708}", true),
            ("\u{3C0}", false),
            ("\u{2022}", true)
        ]
    );
    let mut strict = options(&fallback);
    strict.strict = true;
    assert_eq!(
        render(&document, &strict).unwrap_err().reason(),
        "pdf_not_faithful"
    );
}
