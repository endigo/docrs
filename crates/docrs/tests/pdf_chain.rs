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
use docrs::layout::{FontKind, FontSet, FontSetBuilder, LayoutTarget};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{PdfOptions, PdfReport, render, render_unchecked_for_tests, used_fonts};
use docrs::{Document, Limits, Tolerance};
use reader::{Pdf, Value};
use text::{
    Glyph, assert_same_positions, built_in, expected, glyphs, page_fonts, qpdf_check, records_for,
};

const INTER: &str = "inter/Inter[opsz,wght].ttf";
const WORD: LayoutTarget = LayoutTarget::Word;
const PDF: LayoutTarget = LayoutTarget::Pdf {
    allow_unembedded: false,
};
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
    let target = LayoutTarget::Pdf {
        allow_unembedded: options.allow_unembedded,
    };
    let laid_out = records_for(document, &options.fonts, target);
    assert_same_positions("pdf", &drawn, &expected(&laid_out));
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

/// Each line's text and character origins, laid out for `target`.
fn breaks(document: &Document, fonts: &FontSet, target: LayoutTarget) -> Vec<(String, Vec<f64>)> {
    records_for(document, fonts, target)
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
        breaks(&document, &aliased, PDF),
        breaks(&document, &FontSet::default(), WORD)
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
fn a_tested_pair_declared_as_an_alias_is_silent_but_where_it_differs_from_words_font() {
    let declared = set(vec![font(SERIF)], |b| {
        b.alias("Times New Roman", "Liberation Serif");
    });
    let document = convert(PLAIN, InputFormat::Markdown, "times-new-roman.docx");
    let (_, report, _) = pdf(&document, &options(&declared));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let mut strict = options(&declared);
    strict.strict = true;
    render(&document, &strict).unwrap();
    // Word's Times New Roman draws U+2010 HYPHEN wider than Liberation
    // Serif does: each one drawn through the alias is reported.
    let hyphens = "Co\u{2010}operation and re\u{2010}entry.\n";
    let document = convert(hyphens, InputFormat::Markdown, "times-new-roman.docx");
    for aliased in [
        declared,
        set(vec![font(SERIF)], |b| {
            b.metric_aliases();
        }),
    ] {
        let (_, report, _) = pdf(&document, &options(&aliased));
        assert_eq!(codes(&report), [("pdf_font_substituted", 2)]);
        assert!(
            report.warnings[0]
                .message
                .contains("Times New Roman Regular (drawn in Liberation Serif, whose U+2010 is another width than Word's)"),
            "{}",
            report.warnings[0].message
        );
    }
}

#[test]
fn a_declared_fallback_draws_a_missing_family_and_says_so() {
    let document = html(&para("Calibri", "Calibri was never uploaded"));
    let refused = render(&document, &options(&fonts(&[INTER]))).unwrap_err();
    assert_eq!(refused.reason(), "pdf_font_program_missing");
    let fallback = set(vec![font(INTER)], |b| {
        b.fallback(FontKind::Sans, "Inter");
    });
    let words = breaks(&document, &FontSet::default(), WORD);
    assert_eq!(
        breaks(&document, &fallback, WORD),
        words,
        "Word's pages keep Calibri's metrics"
    );
    assert_ne!(breaks(&document, &fallback, PDF), words);
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
    let (lines, _) = records_for(&document, &fallback, PDF);
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

/// `font` with its subfamily (name id 2) renamed Bold.
fn as_bold(font: &[u8]) -> Vec<u8> {
    let be = |at: usize, n: usize| font[at..at + n].iter().fold(0, |v, b| v << 8 | *b as usize);
    let name = (0..be(4, 2))
        .map(|index| 12 + 16 * index)
        .find(|record| &font[*record..record + 4] == b"name")
        .map(|record| be(record + 8, 4))
        .unwrap();
    let strings = name + be(name + 4, 2);
    let mut out = font.to_vec();
    for record in (0..be(name + 2, 2)).map(|index| name + 6 + 12 * index) {
        if be(record + 6, 2) != 2 {
            continue;
        }
        let text: Vec<u8> = if be(record, 2) == 1 {
            b"Bold".to_vec()
        } else {
            "Bold".encode_utf16().flat_map(u16::to_be_bytes).collect()
        };
        let at = strings + be(record + 10, 2);
        out[at..at + text.len()].copy_from_slice(&text);
        out[record + 8..record + 10].copy_from_slice(&(text.len() as u16).to_be_bytes());
    }
    out
}

#[test]
fn a_style_whose_licence_forbids_embedding_is_drawn_from_another_and_says_so() {
    let bold = with_fs_type(&as_bold(&font(SERIF)), 0x0002);
    let fonts = set(vec![font(SERIF), bold], |_| {});
    assert!(fonts.faces().iter().any(|f| f.bold && !f.embeddable));
    let document = html(&para("Liberation Serif", "Plain and <strong>bold</strong>"));
    let (_, report, _) = pdf(&document, &options(&fonts));
    assert_eq!(codes(&report), [("pdf_font_substituted", 4)]);
    let message = &report.warnings[0].message;
    let named = "Liberation Serif Bold (its licence forbids embedding; drawn from Regular)";
    assert!(message.contains(named), "{message}");
}

fn serif_paragraph() -> Document {
    html(&para(
        "Liberation Serif",
        "Licensed text, measured as uploaded",
    ))
}

#[test]
fn a_font_whose_licence_forbids_embedding_is_missing_for_a_pdf() {
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
        let serif = fonts(&[SERIF]);
        assert_eq!(
            breaks(&document, &fallback, WORD),
            breaks(&document, &serif, WORD),
            "Word's pages measure with the font itself"
        );
        assert_ne!(
            breaks(&document, &fallback, PDF),
            breaks(&document, &serif, PDF),
            "the face that draws measures"
        );
        let (bytes, report, drawn) = pdf(&document, &options(&fallback));
        assert_eq!(
            codes(&report),
            [("pdf_font_substituted", drawn.len())],
            "{fs_type:#06x}"
        );
        let message = &report.warnings[0].message;
        let named = "Liberation Serif Regular (its licence forbids embedding; drawn in Inter)";
        assert!(message.contains(named), "{message}");
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
    let font = pdf
        .resolve(
            pdf.get(
                pdf.resolve(pdf.page_attr(page, "Resources").unwrap())
                    .dict()
                    .unwrap(),
                "Font",
            ),
        )
        .dict()
        .unwrap();
    let font = pdf.resolve(&font[&drawn[0].font]).dict().unwrap();
    let descriptor = pdf.get(font, "FontDescriptor").dict().unwrap();
    let number = |key: &str| pdf.get(descriptor, key).number().unwrap();
    let Value::Array(bbox) = pdf.get(descriptor, "FontBBox") else {
        panic!("no FontBBox");
    };
    // Liberation Serif's own head bbox and hhea ascent, in thousandths.
    assert!(bbox[0].number().unwrap() < 0.0, "{bbox:?}");
    assert_eq!((number("Ascent"), number("ItalicAngle")), (891.0, 0.0));
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
        .filter(|g| built_in(&g.font))
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
        .map(|g| (g.text.as_str(), built_in(&g.font)))
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

#[test]
fn more_than_255_distinct_symbols_start_another_built_in_font() {
    let symbols: String = (0x21..=0x7E)
        .chain(0xF021..=0xF0FF)
        .filter_map(char::from_u32)
        .filter(|ch| !"&<>".contains(*ch))
        .take(300)
        .collect();
    assert_eq!(symbols.chars().count(), 300);
    let document = runs(&[("Wingdings 3", &format!("<w:t>{symbols}</w:t>"))]);
    let (_, report, drawn) = pdf(&document, &options(&FontSet::default()));
    assert_eq!(codes(&report), [("pdf_symbol_substituted", 300)]);
    let fonts: BTreeSet<&str> = drawn.iter().map(|g| g.font.as_str()).collect();
    assert_eq!(fonts, BTreeSet::from(["S1", "S2"]));
    let texts: String = drawn.iter().map(|g| g.text.as_str()).collect();
    assert_eq!(texts, symbols);
}

const MAIN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const WML: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";
const RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A run in `family` (the document default, Liberation Serif, when empty).
fn run(family: &str, rpr: &str, content: &str) -> String {
    let fonts = if family.is_empty() {
        String::new()
    } else {
        format!(r#"<w:rFonts w:ascii="{family}" w:hAnsi="{family}"/>"#)
    };
    format!("<w:r><w:rPr>{fonts}{rpr}</w:rPr>{content}</w:r>")
}

/// "Plain line" citing footnote 1, whose mark is in `body_mark`; the note's
/// own mark is in `note_mark` and its text, `text`, in `note_text`.
fn noted(body_mark: &str, note_mark: &str, note_text: &str, text: &str) -> Document {
    use support::{ROOT_RELS, RawMember, build_zip};
    let superscript = r#"<w:vertAlign w:val="superscript"/>"#;
    let body = format!(
        r#"<w:document {MAIN}><w:body><w:p>{}{}</w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        run("", "", "<w:t>Plain line</w:t>"),
        run(body_mark, superscript, r#"<w:footnoteReference w:id="1"/>"#),
    );
    let notes = format!(
        r#"<w:footnotes {MAIN}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p>{}{}</w:p></w:footnote></w:footnotes>"#,
        run(note_mark, superscript, "<w:footnoteRef/>"),
        run(
            note_text,
            "",
            &format!(r#"<w:t xml:space="preserve"> {text}</w:t>"#)
        ),
    );
    let styles = format!(
        r#"<w:styles {MAIN}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
    );
    let types = format!(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="{WML}.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="{WML}.styles+xml"/><Override PartName="/word/footnotes.xml" ContentType="{WML}.footnotes+xml"/></Types>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/styles" Target="styles.xml"/><Relationship Id="rId2" Type="{RELS}/footnotes" Target="footnotes.xml"/></Relationships>"#
    );
    let members = [
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", body.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
        RawMember::new("word/footnotes.xml", notes.as_bytes()),
    ];
    Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
}

#[test]
fn the_refusal_names_the_faces_of_notes_and_their_marks() {
    let document = noted("Body Mark", "Note Mark", "Note Text", "A note.");
    let error = render_unchecked_for_tests(&document, &options(&fonts(&[SERIF]))).unwrap_err();
    assert_eq!(error.reason(), "pdf_font_program_missing");
    for name in [
        "Body Mark Regular",
        "Note Mark Regular",
        "Note Text Regular",
    ] {
        assert!(error.message().contains(name), "{}", error.message());
    }
    assert!(!error.message().contains("Liberation Serif"));
}

#[test]
fn a_note_character_its_face_lacks_is_drawn_by_the_next_face_and_counted() {
    let document = noted("", "", "", "Checked \u{2713} note.");
    let without = fonts(&[SERIF, INTER]);
    let error = render_unchecked_for_tests(&document, &options(&without)).unwrap_err();
    assert_eq!(error.reason(), "pdf_glyph_missing");
    let fallback = set(vec![font(SERIF), font(INTER)], |b| {
        b.fallback(FontKind::Sans, "Inter");
    });
    let (bytes, report) = render_unchecked_for_tests(&document, &options(&fallback)).unwrap();
    qpdf_check("notes.pdf", &bytes);
    assert_eq!(codes(&report), [("pdf_glyph_missing", 1)]);
    let drawn = glyphs(&Pdf::parse(&bytes).unwrap()).concat();
    let check = drawn.iter().find(|g| g.text == "\u{2713}").unwrap();
    let letter = drawn.iter().find(|g| g.text == "C").unwrap();
    assert_ne!(check.font, letter.font);
    assert_eq!(drawn.iter().filter(|g| g.text == "1").count(), 2);
}

/// "Plain line", an upright equation of `equation`, and a `TITLE` field showing
/// `title`, all in Liberation Serif.
fn equation_and_title(equation: &str, title: &str) -> Document {
    use support::{RawMember, build_zip};
    let math = r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#;
    let body = format!(
        r#"<w:document {MAIN} {math}><w:body><w:p>{}</w:p><w:p><m:oMath><m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>{equation}</m:t></m:r></m:oMath></w:p><w:p><w:fldSimple w:instr=" TITLE ">{}</w:fldSimple></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        run("", "", "<w:t>Plain line</w:t>"),
        run("", "", "<w:t>Stored</w:t>"),
    );
    let styles = format!(
        r#"<w:styles {MAIN}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
    );
    let core = format!(
        r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></cp:coreProperties>"#
    );
    let types = format!(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="{WML}.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="{WML}.styles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#
    );
    let root = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/styles" Target="styles.xml"/></Relationships>"#
    );
    let members = [
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", root.as_bytes()),
        RawMember::new("docProps/core.xml", core.as_bytes()),
        RawMember::new("word/document.xml", body.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
    ];
    Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
}

#[test]
fn equations_and_field_results_draw_a_character_their_face_lacks_in_the_next_face() {
    let without = fonts(&[SERIF, INTER]);
    let fallback = set(vec![font(SERIF), font(INTER)], |b| {
        b.fallback(FontKind::Sans, "Inter");
    });
    // The preflight still refuses a TITLE field, which layout computes.
    for (equation, title) in [("x\u{2713}", "Title"), ("x", "Checked \u{2713}")] {
        let document = equation_and_title(equation, title);
        let error = render_unchecked_for_tests(&document, &options(&without)).unwrap_err();
        assert_eq!(error.reason(), "pdf_glyph_missing", "{}", error.message());
        assert!(
            error.message().contains("U+2713 (Liberation Serif)"),
            "{}",
            error.message()
        );
        let (bytes, report) = render_unchecked_for_tests(&document, &options(&fallback)).unwrap();
        qpdf_check("equation.pdf", &bytes);
        assert_eq!(codes(&report), [("pdf_glyph_missing", 1)]);
        let drawn = glyphs(&Pdf::parse(&bytes).unwrap());
        assert_same_positions(
            "pdf",
            &drawn,
            &expected(&records_for(&document, &fallback, PDF)),
        );
        let drawn = drawn.concat();
        let check = drawn.iter().find(|g| g.text == "\u{2713}").unwrap();
        let letter = drawn.iter().find(|g| g.text == "P").unwrap();
        assert_ne!(check.font, letter.font, "{equation} {title}");
        assert!(
            !drawn.iter().any(|g| g.text == "S"),
            "the stored result is replaced"
        );
    }
}
