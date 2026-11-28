//! Text painted from the layout, read back by the test reader: what it
//! says, where each glyph sits, and what is refused.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use docrs::clock::Timestamp;
use docrs::layout::{FontSet, LineRecord, RunRecord};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{PdfOptions, paint_for_tests, render};
use docrs::txn::Txn;
use docrs::{Document, Limits, Tolerance};
use reader::{Dict, Pdf, Value, tokenize_content};
use support::{RawMember, build_zip};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn font(path: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("fonts").join(path)).unwrap()
}

const INTER: &str = "inter/Inter[opsz,wght].ttf";
const LIBERATION: &str = "liberation/LiberationSerif-Regular.ttf";
const SOURCE_SANS: &str = "source-sans/SourceSans3-Regular.otf";

fn fonts() -> FontSet {
    let mut builder = FontSet::builder();
    for path in [INTER, LIBERATION, SOURCE_SANS] {
        builder.add(font(path)).unwrap();
    }
    builder.build()
}

fn open(path: &Path) -> Document {
    Document::open_path_with(path, &Limits::default(), Tolerance::word())
        .unwrap()
        .0
}

/// `source` converted into `reference` (the bundled blank document when
/// `None`), at a fixed time.
fn convert(source: &str, format: InputFormat, reference: Option<&str>) -> Document {
    let mut document = reference.map_or_else(Document::new, |name| {
        open(&fixtures().join("pdf/references").join(name))
    });
    let options = MarkdownOptions {
        now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
        ..MarkdownOptions::default()
    };
    insert_source_with(&mut document, source, format, &options, &fonts()).unwrap();
    document
}

fn markdown(source: &str) -> Document {
    convert(source, InputFormat::Markdown, Some("inter.docx"))
}

fn pdf_of(document: &Document) -> Vec<u8> {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    render(document, &options).unwrap().0
}

/// One glyph as the reader sees it: its text through `ToUnicode`, and its
/// origin in points from the page's top-left corner.
#[derive(Debug, Clone, PartialEq)]
struct Glyph {
    text: String,
    x: f64,
    y: f64,
}

/// A font as the reader uses it: `ToUnicode` and `/W`.
struct Font {
    texts: BTreeMap<u16, String>,
    widths: BTreeMap<u16, f64>,
}

/// Every glyph each page draws, following `Tm`, `Tf`, `Tz` and `TJ`.
fn glyphs(pdf: &Pdf) -> Vec<Vec<Glyph>> {
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    pdf.pages()
        .unwrap()
        .into_iter()
        .zip(heights)
        .map(|(page, height)| {
            let fonts = page_fonts(pdf, page);
            let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
            let mut out = Vec::new();
            let (mut font, mut size, mut scale) = (None, 0.0, 1.0);
            let (mut origin, mut pen) = ((0.0, 0.0), 0.0);
            for op in tokenize_content(&content).unwrap() {
                let number = |at: usize| op.operands[at].number().unwrap();
                match op.operator.as_str() {
                    "Tf" => {
                        let Value::Name(name) = &op.operands[0] else {
                            panic!("Tf without a name");
                        };
                        font = Some(&fonts[name]);
                        size = number(1);
                    }
                    "Tz" => scale = number(0) / 100.0,
                    "Tm" => {
                        assert_eq!((number(0), number(1)), (1.0, 0.0));
                        origin = (number(4), number(5));
                        pen = 0.0;
                    }
                    "TJ" => {
                        let Value::Array(items) = &op.operands[0] else {
                            panic!("TJ without an array");
                        };
                        let font: &Font = font.unwrap();
                        for item in items {
                            match item {
                                Value::Str(bytes) => {
                                    for code in bytes.chunks(2) {
                                        let cid = u16::from_be_bytes([code[0], code[1]]);
                                        out.push(Glyph {
                                            text: font.texts[&cid].clone(),
                                            x: origin.0 + pen,
                                            y: height - origin.1,
                                        });
                                        pen += font.widths[&cid] / 1000.0 * size * scale;
                                    }
                                }
                                other => pen -= other.number().unwrap() / 1000.0 * size * scale,
                            }
                        }
                    }
                    _ => {}
                }
            }
            out
        })
        .collect()
}

fn page_fonts(pdf: &Pdf, page: &Dict) -> BTreeMap<String, Font> {
    let resources = pdf.page_attr(page, "Resources").unwrap();
    let fonts = pdf.get(pdf.resolve(resources).dict().unwrap(), "Font");
    pdf.resolve(fonts)
        .dict()
        .unwrap()
        .iter()
        .map(|(name, font)| {
            let font = pdf.resolve(font).dict().unwrap();
            (
                name.clone(),
                Font {
                    texts: to_unicode(pdf, font),
                    widths: widths(pdf, font),
                },
            )
        })
        .collect()
}

fn to_unicode(pdf: &Pdf, font: &Dict) -> BTreeMap<u16, String> {
    let cmap = String::from_utf8(pdf.stream_data(pdf.get(font, "ToUnicode")).unwrap()).unwrap();
    let hex = |token: &str| -> Vec<u8> {
        let digits = token.trim_matches(['<', '>']);
        (0..digits.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&digits[at..at + 2], 16).unwrap())
            .collect()
    };
    let mut out = BTreeMap::new();
    for block in cmap.split("beginbfchar").skip(1) {
        let block = block.split("endbfchar").next().unwrap();
        for entry in block.lines().filter(|l| !l.trim().is_empty()) {
            let (cid, text) = entry.split_once(' ').unwrap();
            let cid = u16::from_be_bytes(hex(cid).try_into().unwrap());
            let units: Vec<u16> = hex(text)
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            out.insert(cid, String::from_utf16(&units).unwrap());
        }
    }
    out
}

/// `/W` of a `Type0` font's descendant: `c [w1 w2 …]` runs.
fn widths(pdf: &Pdf, font: &Dict) -> BTreeMap<u16, f64> {
    let Value::Array(descendants) = pdf.get(font, "DescendantFonts") else {
        panic!("no DescendantFonts");
    };
    let cid_font = pdf.resolve(&descendants[0]).dict().unwrap();
    let Value::Array(w) = pdf.resolve(pdf.get(cid_font, "W")) else {
        panic!("no /W");
    };
    let mut out = BTreeMap::new();
    for pair in w.chunks(2) {
        let first = pair[0].number().unwrap() as u16;
        let Value::Array(run) = &pair[1] else {
            panic!("only c [w …] runs are written");
        };
        for (at, width) in run.iter().enumerate() {
            out.insert(first + at as u16, width.number().unwrap());
        }
    }
    out
}

/// Every line layout records for `document`'s body, and each look's rise.
fn records(document: &Document, fonts: &FontSet) -> (Vec<LineRecord>, Vec<f64>) {
    let txn = Txn::new(document);
    let main = document.package().main_part_name().to_string();
    let (mut out, mut rises) = (Vec::new(), Vec::new());
    docrs::layout::records_with(
        &txn,
        &main,
        fonts,
        |looks, _| {
            rises = looks.iter().map(|look| look.rise).collect();
            Ok(())
        },
        |record| {
            if let docrs::layout::Record::Line(line) = record {
                out.push(line);
            }
            Ok(())
        },
    )
    .unwrap();
    (out, rises)
}

/// The glyphs each page should draw, as layout records them: each line's
/// label, then its characters, each tab replaced by its leader, on its
/// baseline raised by the run's rise.
fn expected((records, rises): &(Vec<LineRecord>, Vec<f64>)) -> Vec<Vec<Glyph>> {
    let pages = records.iter().map(|line| line.page).max().unwrap_or(0) as usize;
    let mut out = vec![Vec::new(); pages];
    for line in records {
        let page = &mut out[line.page as usize - 1];
        let mut leaders = line.leaders.iter();
        let mut push = |run: &RunRecord, ch: char, x: f64| {
            page.push(Glyph {
                text: ch.to_string(),
                x,
                y: line.baseline - rises[run.look as usize],
            })
        };
        for run in line.label.iter().chain(&line.runs) {
            for (ch, x) in run.text.chars().zip(&run.origins) {
                if ch != '\t' {
                    push(run, ch, *x);
                } else if let Some(Some(leader)) = leaders.next() {
                    for (ch, x) in leader.text.chars().zip(&leader.origins) {
                        push(leader, ch, *x);
                    }
                }
            }
        }
    }
    out
}

fn page_texts(pages: &[Vec<Glyph>]) -> Vec<String> {
    pages
        .iter()
        .map(|glyphs| glyphs.iter().map(|g| g.text.as_str()).collect())
        .collect()
}

fn assert_same_places(name: &str, drawn: &[Vec<Glyph>], expected: &[Vec<Glyph>]) {
    assert_eq!(drawn.len(), expected.len(), "{name}: pages");
    for (page, (drawn, expected)) in drawn.iter().zip(expected).enumerate() {
        assert_eq!(drawn.len(), expected.len(), "{name} page {}", page + 1);
        for (glyph, want) in drawn.iter().zip(expected) {
            let close = (glyph.x - want.x).abs() < 0.001 && (glyph.y - want.y).abs() < 0.001;
            assert!(
                glyph.text == want.text && close,
                "{name} page {}: drew {glyph:?}, layout placed {want:?}",
                page + 1
            );
        }
    }
}

fn qpdf_check(name: &str, bytes: &[u8]) {
    let found = Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        let required = std::env::var_os("CI").is_some() && !cfg!(windows);
        assert!(!required, "qpdf must be installed in CI");
        eprintln!("SKIPPED: qpdf is not installed; `brew install qpdf` to run this check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{name}: {report}");
    assert!(
        report.contains("No syntax or stream encoding errors found"),
        "{name}: {report}"
    );
}

fn memo() -> Document {
    let source = std::fs::read_to_string(fixtures().join("pdf/sources/memo.md")).unwrap();
    markdown(&source)
}

const LISTS: &str = "# Steps\n\nFirst the plan.\n\n1. Collect the keys.\n2. Check the\u{a0}rooms.\n\n\
                     Then the second list, which starts again.\n\n1. Move in.\n2. Unpack.\n";

#[test]
fn extracted_text_is_the_documents_per_page() {
    for (name, document) in [("memo", memo()), ("lists", markdown(LISTS))] {
        let bytes = pdf_of(&document);
        let drawn = page_texts(&glyphs(&Pdf::parse(&bytes).unwrap()));
        let laid_out = page_texts(&expected(&records(&document, &fonts())));
        assert_eq!(drawn, laid_out, "{name}");
    }
    let lists = page_texts(&glyphs(&Pdf::parse(&pdf_of(&markdown(LISTS))).unwrap()));
    assert!(lists[0].contains("1.Collect the keys."), "{lists:?}");
    assert!(lists[0].contains("Check the\u{a0}rooms."), "{lists:?}");
    assert!(lists[0].contains("1.Move in.2.Unpack."), "{lists:?}");
}

#[test]
fn every_glyph_is_where_layout_placed_it() {
    let mut documents: Vec<(String, Document)> = Vec::new();
    let mut docx = |dir: PathBuf| {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "docx"))
            .collect();
        paths.sort();
        for path in paths {
            documents.push((path.display().to_string(), open(&path)));
        }
    };
    docx(fixtures().join("layout/geometry"));
    docx(fixtures().join("markdown/templates"));
    docx(fixtures().join("markdown/templates/v2"));
    docx(fixtures().join("../../examples/markdown"));
    let cases = std::fs::read_to_string(fixtures().join("pdf/cases.tsv")).unwrap();
    for row in cases.lines().skip(1) {
        let [case, source, reference, ..] = row.split('\t').collect::<Vec<_>>()[..] else {
            panic!("{row}");
        };
        let source = std::fs::read_to_string(fixtures().join("../..").join(source)).unwrap();
        let mut document = open(&fixtures().join("../..").join(reference));
        let options = MarkdownOptions {
            now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
            ..MarkdownOptions::default()
        };
        insert_source_with(
            &mut document,
            &source,
            InputFormat::Markdown,
            &options,
            &fonts(),
        )
        .unwrap();
        documents.push((case.to_string(), document));
    }
    let stand_in = font(LIBERATION);
    let fonts = fonts();
    for (name, document) in &documents {
        let bytes = paint_for_tests(document, &fonts, &stand_in).unwrap();
        let drawn = glyphs(&Pdf::parse(&bytes).unwrap());
        assert_same_places(name, &drawn, &expected(&records(document, &fonts)));
    }
    assert!(documents.len() > 50, "{}", documents.len());
}

/// Runs in each fixture font: Inter regular and bold from 4 to 24 pt,
/// Liberation Serif, and Source Sans (CFF).
fn font_mix() -> Document {
    let mut html = String::new();
    for size in ["4", "6.5", "8", "11", "24"] {
        html.push_str(&format!(
            r#"<p><span style="font-size: {size}pt">Inter {size} pt, quick brown fox </span><strong><span style="font-size: {size}pt">and bold jumps</span></strong></p>"#
        ));
    }
    html.push_str(r#"<p><span style="font-family: 'Liberation Serif'">Liberation Serif, upright </span><em><span style="font-family: 'Liberation Serif'">and italic, synthesized</span></em></p>"#);
    html.push_str(r#"<p><span style="font-family: 'Source Sans 3'">Source Sans 3 with CFF outlines</span></p>"#);
    html.push_str("<p>A raised italic<sup><em>note</em></sup> shears about its own baseline.</p>");
    convert(&html, InputFormat::Html, Some("inter.docx"))
}

#[test]
fn w_widths_equal_layouts_advances() {
    let document = font_mix();
    let bytes = pdf_of(&document);
    qpdf_check("fonts.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let page = pdf.pages().unwrap()[0];
    let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
    for op in tokenize_content(&content).unwrap() {
        if op.operator == "TJ" {
            assert!(
                matches!(&op.operands[0], Value::Array(items) if items.iter().all(|i| matches!(i, Value::Str(_)))),
                "a glyph's /W differs from layout's advance: {op:?}"
            );
        }
    }
    let names: Vec<String> = page_fonts(&pdf, page)
        .keys()
        .map(|name| {
            let fonts = pdf.get(
                pdf.resolve(pdf.page_attr(page, "Resources").unwrap())
                    .dict()
                    .unwrap(),
                "Font",
            );
            let font = pdf
                .resolve(&pdf.resolve(fonts).dict().unwrap()[name])
                .dict()
                .unwrap();
            match pdf.get(font, "BaseFont") {
                Value::Name(base) => base[7..].to_string(),
                other => panic!("{other:?}"),
            }
        })
        .collect();
    for instance in [
        "Inter-Regular",
        "Inter-Bold",
        "LiberationSerif",
        "SourceSans3-Regular-Identity-H",
    ] {
        assert!(names.iter().any(|n| n == instance), "{instance}: {names:?}");
    }
    assert!(
        !names.iter().any(|n| n.contains("opsz")),
        "every size lays out and draws at the default optical size: {names:?}"
    );
    assert_same_places(
        "font mix",
        &glyphs(&pdf),
        &expected(&records(&document, &fonts())),
    );
}

#[test]
fn media_boxes_are_the_layouts_page_sizes() {
    let pdf = Pdf::parse(&pdf_of(&memo())).unwrap();
    let boxes = pdf.media_boxes().unwrap();
    assert_eq!(boxes.len(), 2);
    let memo = memo();
    let txn = Txn::new(&memo);
    let xml = String::from_utf8(docrs::xml::write::serialize(
        txn.tree("word/document.xml").unwrap(),
    ))
    .unwrap();
    let twips = |name: &str| {
        let size = &xml[xml.find("<w:pgSz").unwrap()..];
        let at = size.find(&format!("w:{name}=\"")).unwrap() + 5;
        size[at..at + size[at..].find('"').unwrap()]
            .parse::<f64>()
            .unwrap()
            / 20.0
    };
    for media_box in boxes {
        assert_eq!(media_box, [0.0, 0.0, twips("w"), twips("h")]);
    }
}

/// After an intended change to the output, regenerate the file with
/// `DOCRS_BLESS=1 cargo test -p docrs --test pdf_text the_output_is_deterministic`,
/// then run `qpdf --check` on it and review the diff.
#[test]
fn the_output_is_deterministic() {
    let bytes = pdf_of(&markdown(LISTS));
    assert_eq!(bytes, pdf_of(&markdown(LISTS)));
    qpdf_check("lists.pdf", &bytes);
    let golden = fixtures().join("pdf/text.pdf");
    if std::env::var_os("DOCRS_BLESS").is_some() {
        std::fs::write(&golden, &bytes).unwrap();
    }
    assert!(
        bytes == std::fs::read(golden).unwrap(),
        "the output changed"
    );
}

#[test]
fn a_missing_font_program_refuses_naming_every_family_and_style() {
    let document = convert(
        "# Title\n\nPlain, **bold** and *italic* text.\n",
        InputFormat::Markdown,
        None,
    );
    let err = render(&document, &PdfOptions::default()).unwrap_err();
    assert_eq!(err.reason(), "pdf_font_program_missing");
    let missing: Vec<(String, bool, bool)> = err
        .details()
        .unwrap()
        .missing_fonts
        .iter()
        .map(|font| (font.family.clone(), font.bold, font.italic))
        .collect();
    assert!(missing.len() >= 3, "{missing:?}");
    for style in [(false, false), (true, false), (false, true)] {
        assert!(
            missing
                .iter()
                .any(|(_, bold, italic)| (*bold, *italic) == style),
            "{style:?}: {missing:?}"
        );
    }
    for (family, ..) in &missing {
        assert!(err.message().contains(family.as_str()), "{}", err.message());
    }
    let json = err.to_json();
    assert_eq!(
        json["error"]["details"]["missing_fonts"]
            .as_array()
            .unwrap()
            .len(),
        missing.len()
    );
}

#[test]
fn strict_refuses_any_warning_and_not_yet_drawn_always_refuses() {
    let arabic = markdown("Plain words and \u{627}\u{644}\u{639}\u{631}\u{628}\u{64a}\u{629}.\n");
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.strict = true;
    assert_eq!(
        render(&arabic, &options).unwrap_err().reason(),
        "pdf_not_faithful"
    );
    options.strict = false;
    assert_eq!(
        render(&arabic, &options).unwrap_err().reason(),
        "pdf_glyph_missing"
    );
    let table = markdown("| A | B |\n| - | - |\n| 1 | 2 |\n");
    assert_eq!(
        render(&table, &options).unwrap_err().reason(),
        "pdf_not_yet_drawn"
    );
    let (_, report) = render(&memo(), &options).unwrap();
    assert_eq!((report.warnings.len(), report.pages), (0, Some(2)));
}

#[test]
fn the_page_limit_and_the_output_limit_hold() {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.limits.max_pages = 1;
    let err = render(&memo(), &options).unwrap_err();
    assert_eq!(err.limit_exceeded().unwrap().limit, "max_pdf_pages");
    options.limits = docrs::pdf::PdfLimits::default();
    options.limits.max_output_bytes = 10_000;
    let err = render(&memo(), &options).unwrap_err();
    assert_eq!(err.kind(), docrs::ErrorKind::ResourceLimit);
}

/// Word for Mac widens each glyph of a synthesized bold by 0.018 em, layout
/// measures it so, and the painter strokes it and scales it to 0.98.
#[test]
fn a_synthesized_bold_is_measured_wider() {
    let text = "Synthesized bold";
    let width = |html: &str| {
        let document = convert(html, InputFormat::Html, Some("inter.docx"));
        records(&document, &fonts()).0[0].width
    };
    let serif = |tag: &str| {
        format!(
            r#"<p><{tag}><span style="font-family: 'Liberation Serif'">{text}</span></{tag}></p>"#
        )
    };
    let extra = width(&serif("strong")) - width(&serif("span"));
    let glyphs = text.chars().count() as f64;
    assert!((extra - glyphs * 11.0 * 0.018).abs() < 1e-9, "{extra}");
    let bytes = pdf_of(&convert(
        &serif("strong"),
        InputFormat::Html,
        Some("inter.docx"),
    ));
    let content = {
        let pdf = Pdf::parse(&bytes).unwrap();
        let page = pdf.pages().unwrap()[0];
        String::from_utf8_lossy(&pdf.stream_data(pdf.get(page, "Contents")).unwrap()).into_owned()
    };
    assert!(
        content.contains("2 Tr") && content.contains(" 1 0 0 0.98 "),
        "{content}"
    );
}

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// A package whose body is `body`, with `numbering` and `settings` parts
/// when they are not empty, on a Letter page with 1-inch margins.
fn raw(body: &str, numbering: &str, settings: &str) -> Document {
    let part = |root: &str, content: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} xmlns:w="{W_NS}">{content}</w:{root}>"#
        )
    };
    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let mut members = vec![
        RawMember::new("_rels/.rels", support::ROOT_RELS.as_bytes()),
        RawMember::new(
            "word/document.xml",
            part("document", &format!("<w:body>{body}{section}</w:body>")).as_bytes(),
        ),
    ];
    for (name, kind, root, content) in [
        ("numbering", "numbering", "numbering", numbering),
        ("settings", "settings", "settings", settings),
    ] {
        if content.is_empty() {
            continue;
        }
        types.push_str(&format!(
            r#"<Override PartName="/word/{name}.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml"/>"#
        ));
        rels.push_str(&format!(
            r#"<Relationship Id="r{name}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{name}.xml"/>"#
        ));
        members.push(RawMember::new(
            &format!("word/{name}.xml"),
            part(root, content).as_bytes(),
        ));
    }
    types.push_str("</Types>");
    rels.push_str("</Relationships>");
    members.push(RawMember::new("[Content_Types].xml", types.as_bytes()));
    members.push(RawMember::new(
        "word/_rels/document.xml.rels",
        rels.as_bytes(),
    ));
    support::open_bytes(&build_zip(&members))
}

/// A run of `content` in 11 pt Liberation Serif.
fn serif(content: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/><w:sz w:val="22"/></w:rPr>{content}</w:r>"#
    )
}

fn text(text: &str) -> String {
    serif(&format!(r#"<w:t xml:space="preserve">{text}</w:t>"#))
}

const TAB: &str = "<w:tab/>";

/// A paragraph with `ppr` of text and tabs (`\t`).
fn tabbed(ppr: &str, content: &str) -> String {
    let runs: Vec<String> = content.split('\t').map(text).collect();
    format!("<w:p><w:pPr>{ppr}</w:pPr>{}</w:p>", runs.join(&serif(TAB)))
}

fn content_of(bytes: &[u8]) -> String {
    let pdf = Pdf::parse(bytes).unwrap();
    let page = pdf.pages().unwrap()[0];
    String::from_utf8_lossy(&pdf.stream_data(pdf.get(page, "Contents")).unwrap()).into_owned()
}

/// Word for Mac sets leader characters at whole multiples of their advance
/// from the page's left edge, every one that fits between the tab's ends.
#[test]
fn leaders_repeat_their_character_on_a_grid_from_the_page_edge() {
    let source = std::fs::read_to_string(fixtures().join("pdf/sources/toc.md")).unwrap();
    let document = markdown(&source);
    let (lines, _) = records(&document, &fonts());
    let mut leaders = 0;
    for line in &lines {
        for (tab, leader) in line.tabs.iter().zip(&line.leaders) {
            let Some(leader) = leader else { continue };
            leaders += 1;
            assert!(leader.text.chars().all(|ch| ch == '.'), "{leader:?}");
            let step = leader.origins[1] - leader.origins[0];
            let (first, last) = (leader.origins[0], *leader.origins.last().unwrap());
            for x in &leader.origins {
                let cells = x / step;
                assert!((cells - cells.round()).abs() < 1e-6, "{x} is off the grid");
            }
            assert!(
                first >= tab.0 && first - step < tab.0,
                "{first} after {tab:?}"
            );
            assert!(
                last + step <= tab.1 && last + 2.0 * step > tab.1,
                "{last} before {tab:?}"
            );
        }
    }
    assert!(leaders >= 15, "{leaders}");
    let bytes = paint_for_tests(&document, &fonts(), &font(LIBERATION)).unwrap();
    let drawn = page_texts(&glyphs(&Pdf::parse(&bytes).unwrap()));
    assert!(drawn[0].contains("Introduction....."), "{drawn:?}");
    assert_eq!(
        drawn,
        page_texts(&expected(&records(&document, &fonts()))),
        "the leaders' dots stand where the tabs were"
    );
}

/// Word for Mac draws a heavy leader as the plain underscore, at the run's
/// size, as an underscore leader.
#[test]
fn a_heavy_leader_is_a_plain_underscore_and_its_face_must_be_supplied() {
    let stops = |leader: &str| {
        format!(r#"<w:tabs><w:tab w:val="right" w:leader="{leader}" w:pos="5760"/></w:tabs>"#)
    };
    let drawn =
        |leader: &str| content_of(&pdf_of(&raw(&tabbed(&stops(leader), "Total\t12"), "", "")));
    let heavy = drawn("heavy");
    assert!(
        !heavy.contains("2 Tr") && !heavy.contains(" 0.98 "),
        "{heavy}"
    );
    assert_eq!(heavy, drawn("underscore"));
    // The leader is drawn in the tab's font, which may be one the text
    // does not use.
    let absent = r#"<w:r><w:rPr><w:rFonts w:ascii="Absent Sans" w:hAnsi="Absent Sans"/></w:rPr><w:tab/></w:r>"#;
    let body = |leader: &str| {
        format!(
            "<w:p><w:pPr>{}</w:pPr>{}{absent}{}</w:p>",
            stops(leader),
            text("Total"),
            text("12")
        )
    };
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let err = render(&raw(&body("dot"), "", ""), &options).unwrap_err();
    assert!(err.to_string().contains("Absent Sans"), "{err}");
    render(&raw(&body("none"), "", ""), &options).unwrap();
}

/// A bar tab is no stop: a tab after it goes on to the next. Word for Mac
/// strokes it 0.24 pt wide, its left edge at the stop, from each line's top
/// to its bottom, and on the paragraph's last line through its space after.
#[test]
fn a_bar_tab_is_a_rule_and_not_a_stop() {
    let stops = r#"<w:spacing w:after="120"/><w:tabs><w:tab w:val="bar" w:pos="1440"/><w:tab w:val="left" w:pos="2880"/></w:tabs>"#;
    let long = "a note long enough to wrap onto a second line of the paragraph, and then some";
    let document = raw(&tabbed(stops, &format!("Item\t{long}")), "", "");
    let (lines, _) = records(&document, &fonts());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].tabs[0].1, 72.0 + 144.0);
    let content = content_of(&pdf_of(&document));
    let round = |value: f64| (value * 1000.0).round() / 1000.0;
    for (line, after) in lines.iter().zip([0.0, 6.0]) {
        let rule = format!(
            "0.24 w 144.12 {} m 144.12 {} l S",
            round(792.0 - line.y),
            round(792.0 - line.y - line.height - after)
        );
        assert!(content.contains(&rule), "{rule} in {content}");
    }
}

/// A decimal tab ends the text before the point at its stop.
#[test]
fn a_decimal_tab_aligns_the_point() {
    let stops = r#"<w:tabs><w:tab w:val="decimal" w:pos="4320"/></w:tabs>"#;
    let body = tabbed(stops, "Rent\t1,250.00")
        + &tabbed(stops, "Water\t42.5")
        + &tabbed(stops, "Count\t7");
    let (lines, _) = records(&raw(&body, "", ""), &fonts());
    let stop = 72.0 + 216.0;
    for line in &lines[..2] {
        let run = &line.runs[0];
        let point = run
            .text
            .char_indices()
            .filter(|(_, ch)| *ch == '.')
            .map(|(at, _)| at)
            .next()
            .unwrap();
        assert!(
            (run.origins[run.text[..point].chars().count()] - stop).abs() < 1e-9,
            "{line:?}"
        );
    }
    assert!(
        lines[2].tabs[0].1 < stop,
        "a number without a point ends at the stop"
    );
}

/// `w:lvlJc`: the label starts at, is centred on, or ends at the first-line
/// indent; with no suffix the text follows it directly.
#[test]
fn a_label_aligns_on_the_first_line_indent() {
    let level = |jc: &str| {
        format!(
            r#"<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%1."/><w:suff w:val="nothing"/><w:lvlJc w:val="{jc}"/><w:pPr><w:ind w:left="1440" w:hanging="720"/></w:pPr><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/><w:b/><w:sz w:val="22"/></w:rPr></w:lvl>"#
        )
    };
    let mut numbering = String::new();
    let mut body = String::new();
    for (id, jc) in ["left", "center", "right"].iter().enumerate() {
        numbering.push_str(&format!(
            r#"<w:abstractNum w:abstractNumId="{id}">{}</w:abstractNum>"#,
            level(jc)
        ));
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="{}"/></w:numPr></w:pPr>{}</w:p>"#,
            id + 1,
            text("Item")
        ));
    }
    for id in 0..3 {
        numbering.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="{id}"/></w:num>"#,
            id + 1
        ));
    }
    let document = raw(&body, &numbering, "");
    let (lines, _) = records(&document, &fonts());
    let indent = 72.0 + 36.0;
    let starts: Vec<(f64, f64)> = lines
        .iter()
        .map(|line| {
            (
                line.label.as_ref().unwrap().origins[0],
                line.runs[0].origins[0],
            )
        })
        .collect();
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(close(starts[0].0, indent), "{starts:?}");
    assert!(
        close((starts[1].0 + starts[1].1) / 2.0, indent),
        "{starts:?}"
    );
    assert!(close(starts[2].1, indent), "{starts:?}");
    assert_eq!(lines[0].label.as_ref().unwrap().text, "I.");
    let drawn = page_texts(&glyphs(&Pdf::parse(&pdf_of(&document)).unwrap()));
    assert_eq!(drawn[0], "I.ItemI.ItemI.Item");
}

/// Word spreads a justified line that ends in a line break, unless the
/// document sets `w:doNotExpandShiftReturn`.
#[test]
fn a_justified_line_ending_in_a_break_is_spread() {
    let body = format!(
        r#"<w:p><w:pPr><w:jc w:val="both"/></w:pPr>{}{}{}</w:p>"#,
        text("A short line"),
        serif("<w:br/>"),
        text("and the last line of the paragraph")
    );
    let ends = |settings: &str| {
        let (lines, _) = records(&raw(&body, "", settings), &fonts());
        let run = &lines[0].runs[0];
        (*run.origins.last().unwrap(), lines[0].slack)
    };
    let (spread, slack) = ends("");
    let (kept, _) = ends("<w:compat><w:doNotExpandShiftReturn/></w:compat>");
    assert!(slack > 100.0, "{slack}");
    assert!(
        (spread - kept - slack).abs() < 1e-9,
        "{spread} {kept} {slack}"
    );
}
