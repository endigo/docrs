//! Text painted from the layout, read back by the test reader: what it
//! says, where each glyph sits, and what is refused.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
mod support;
#[allow(dead_code)]
#[path = "support/glyphs.rs"]
mod text;

use std::path::{Path, PathBuf};

use docrs::clock::Timestamp;
use docrs::layout::fonts::FontKind;
use docrs::layout::{FontBook, FontSet};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{
    PdfOptions, layout_for_tests, paint_for_tests, preflight, render, render_unchecked_for_tests,
};
use docrs::txn::Txn;
use docrs::{Document, Limits, Tolerance};
use reader::{Pdf, Value, tokenize_content};
use support::{RawMember, build_zip};
use text::{Glyph, assert_same_places, expected, glyphs, page_fonts, qpdf_check, records};

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

fn page_texts(pages: &[Vec<Glyph>]) -> Vec<String> {
    pages
        .iter()
        .map(|glyphs| glyphs.iter().map(|g| g.text.as_str()).collect())
        .collect()
}

fn memo() -> Document {
    let source = std::fs::read_to_string(fixtures().join("pdf/sources/memo.md")).unwrap();
    markdown(&source)
}

const LISTS: &str = "# Steps\n\nFirst the plan.\n\n1. Collect the keys.\n2. Check the\u{a0}rooms.\n\n\
                     Then the second list, which starts again.\n\n1. Move in.\n2. Unpack.\n";

#[test]
fn extracted_text_is_the_documents_per_page() {
    for (name, document) in [
        ("memo", memo()),
        ("lists", markdown(LISTS)),
        ("decorations", decorations()),
    ] {
        let bytes = pdf_of(&document);
        qpdf_check(&format!("{name}.pdf"), &bytes);
        let drawn = page_texts(&glyphs(&Pdf::parse(&bytes).unwrap()));
        let laid_out = page_texts(&expected(&records(&document, &fonts())));
        assert_eq!(drawn, laid_out, "{name}");
    }
    let lists = page_texts(&glyphs(&Pdf::parse(&pdf_of(&markdown(LISTS))).unwrap()));
    assert!(lists[0].contains("1.Collect the keys."), "{lists:?}");
    assert!(lists[0].contains("Check the\u{a0}rooms."), "{lists:?}");
    assert!(lists[0].contains("1.Move in.2.Unpack."), "{lists:?}");
}

/// Each case of `pdf/cases.tsv`, converted at a fixed time.
fn cases() -> Vec<(String, Document)> {
    let cases = std::fs::read_to_string(fixtures().join("pdf/cases.tsv")).unwrap();
    cases
        .lines()
        .skip(1)
        .map(|row| {
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
            (case.to_string(), document)
        })
        .collect()
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
    docx(fixtures().join("layout/tables"));
    docx(fixtures().join("markdown/templates"));
    docx(fixtures().join("markdown/templates/v2"));
    docx(fixtures().join("../../examples/markdown"));
    for name in ["appearance.docx", "decorations.docx"] {
        documents.push((name.to_string(), open(&fixtures().join("pdf").join(name))));
    }
    documents.extend(cases());
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
    let hyphen = markdown("A soft hy\u{AD}phen.\n");
    assert_eq!(
        render(&hyphen, &options).unwrap_err().reason(),
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
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Letter paper, 1-inch margins, headers and footers half an inch from
/// the edge.
const LETTER: &str = r#"<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>"#;

/// A package whose body is `body`, with `numbering` and `settings` parts
/// when they are not empty, on a [`LETTER`] page.
fn raw(body: &str, numbering: &str, settings: &str) -> Document {
    let section = format!("<w:sectPr>{LETTER}</w:sectPr>");
    let parts = [
        ("numbering", "numbering", "numbering", numbering),
        ("settings", "settings", "settings", settings),
    ];
    package(&(body.to_string() + &section), &parts)
}

/// A package whose body is `body`, with each of `parts` (name, kind, root
/// element, content) that is not empty, related as `r<name>`.
fn package(body: &str, parts: &[(&str, &str, &str, &str)]) -> Document {
    let part = |root: &str, content: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} xmlns:w="{W_NS}" xmlns:r="{R_NS}">{content}</w:{root}>"#
        )
    };
    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    let mut members = vec![
        RawMember::new("_rels/.rels", support::ROOT_RELS.as_bytes()),
        RawMember::new(
            "word/document.xml",
            part("document", &format!("<w:body>{body}</w:body>")).as_bytes(),
        ),
    ];
    for (name, kind, root, content) in parts.iter().filter(|part| !part.3.is_empty()) {
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

/// The decoration sheet (`appearance.rs` builds it).
fn decorations() -> Document {
    open(&fixtures().join("pdf/decorations.docx"))
}

/// A line or rectangle the content stream paints outside text.
#[derive(Debug, Clone, PartialEq)]
struct Mark {
    rgb: [f64; 3],
    width: f64,
    dash: Vec<f64>,
    /// Page x from and to, and page y (from the bottom) where it starts.
    from: f64,
    to: f64,
    y: f64,
    wave: bool,
}

/// The stroked paths (`S`) and filled rectangles (`re f`, as marks with
/// their height for width and their bottom for y) of the first page. A
/// wave ends where its clip (`re W n`) does.
fn marks(bytes: &[u8]) -> (Vec<Mark>, Vec<Mark>) {
    let pdf = Pdf::parse(bytes).unwrap();
    let page = pdf.pages().unwrap()[0];
    let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
    let (mut strokes, mut fills) = (Vec::new(), Vec::new());
    let (mut stroke, mut fill) = ([0.0; 3], [0.0; 3]);
    let (mut rect, mut clip): ([f64; 4], Option<[f64; 4]>) = ([0.0; 4], None);
    let mut mark = Mark {
        rgb: [0.0; 3],
        width: 1.0,
        dash: Vec::new(),
        from: 0.0,
        to: 0.0,
        y: 0.0,
        wave: false,
    };
    for op in tokenize_content(&content).unwrap() {
        let number = |at: usize| op.operands[at].number().unwrap();
        match op.operator.as_str() {
            "RG" => stroke = [number(0), number(1), number(2)],
            "rg" => fill = [number(0), number(1), number(2)],
            "w" => mark.width = number(0),
            "d" => {
                let Value::Array(items) = &op.operands[0] else {
                    panic!("d without an array");
                };
                mark.dash = items.iter().map(|i| i.number().unwrap()).collect();
            }
            "m" => (mark.from, mark.y, mark.wave) = (number(0), number(1), false),
            "l" => mark.to = number(0),
            "c" => (mark.to, mark.wave) = (number(4), true),
            "S" => strokes.push(Mark {
                rgb: stroke,
                to: match clip {
                    Some([x, _, w, _]) if mark.wave => x + w,
                    _ => mark.to,
                },
                ..mark.clone()
            }),
            "re" => rect = [number(0), number(1), number(2), number(3)],
            "W" => clip = Some(rect),
            "Q" => clip = None,
            "f" => fills.push(Mark {
                rgb: fill,
                width: rect[3],
                dash: Vec::new(),
                from: rect[0],
                to: rect[0] + rect[2],
                y: rect[1],
                wave: false,
            }),
            _ => {}
        }
    }
    (strokes, fills)
}

/// A table's bytes in a TrueType file.
fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> &'a [u8] {
    let be16 = |at: usize| u16::from_be_bytes([font[at], font[at + 1]]);
    let be32 = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
    (0..usize::from(be16(4)))
        .map(|i| 12 + 16 * i)
        .find(|at| &font[*at..*at + 4] == tag)
        .map(|at| &font[be32(at + 8)..be32(at + 8) + be32(at + 12)])
        .unwrap()
}

/// Inter's underline and strikeout at `size`: each line's centre above the
/// baseline and its thickness, from `post` and `OS/2`.
fn inter_lines(size: f64) -> [(f64, f64); 2] {
    let font = font(INTER);
    let field = |table: &[u8], at: usize| f64::from(i16::from_be_bytes([table[at], table[at + 1]]));
    let em = field(table(&font, b"head"), 18) / size;
    let line = |table: &[u8], top: usize, thick: usize| {
        let (top, thick) = (field(table, top), field(table, thick));
        ((top - thick / 2.0) / em, thick / em)
    };
    [
        line(table(&font, b"post"), 8, 10),
        line(table(&font, b"OS/2"), 28, 26),
    ]
}

/// Where the sheet's line starting `label: ` sets its sample: the first and the
/// following glyph's x, and the baseline from the page's bottom.
fn sample(glyphs: &[Glyph], height: f64, label: &str, length: usize) -> (f64, f64, f64) {
    let text: Vec<&str> = glyphs.iter().map(|g| g.text.as_str()).collect();
    let prefix: Vec<String> = format!("{label}: ").chars().map(String::from).collect();
    let starts_line = |at: usize| at == 0 || glyphs[at - 1].y != glyphs[at].y;
    let at = (0..text.len())
        .find(|at| {
            starts_line(*at)
                && text[*at..]
                    .iter()
                    .zip(&prefix)
                    .filter(|(a, b)| a == b)
                    .count()
                    == prefix.len()
        })
        .unwrap()
        + prefix.len();
    (glyphs[at].x, glyphs[at + length].x, height - glyphs[at].y)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.002
}

#[test]
fn underlines_and_strikethrough_follow_the_face_metrics() {
    let bytes = pdf_of(&decorations());
    let pdf = Pdf::parse(&bytes).unwrap();
    let glyphs = &glyphs(&pdf)[0];
    let height = pdf.media_boxes().unwrap()[0][3];
    let (strokes, _) = marks(&bytes);
    let [(under, thick), (strike, strike_thick)] = inter_lines(11.0);
    let at = |label: &str| {
        let (from, to, baseline) = sample(glyphs, height, label, 15);
        let found: Vec<&Mark> = strokes
            .iter()
            .filter(|m| close(m.from, from) && (m.y - baseline).abs() < 6.0)
            .collect();
        assert!(!found.is_empty(), "{label}: {strokes:?}");
        (found, to, baseline)
    };

    let (single, to, baseline) = at("single");
    assert_eq!(single.len(), 1);
    let line = single[0];
    assert!(
        close(line.to, to) && close(line.y, baseline + under),
        "{line:?}"
    );
    assert!(close(line.width, thick) && line.dash.is_empty() && !line.wave);
    assert_eq!(line.rgb, [0.0; 3]);

    // Word for Mac's shapes on Inter 11 pt (centre and width below the
    // baseline, within Word's 0.24 pt grid); dash lengths are exact.
    let near = |a: f64, b: f64| (a - b).abs() <= 0.25;
    let (double, _, baseline) = at("double");
    let lines: Vec<(f64, f64)> = double.iter().map(|m| (baseline - m.y, m.width)).collect();
    assert!(
        lines.len() == 2
            && near(lines[0].0, 0.96)
            && near(lines[1].0, 2.40)
            && lines.iter().all(|(_, w)| near(*w, 0.48)),
        "{lines:?}"
    );

    for (label, heavy, dash) in [
        ("thick", true, &[][..]),
        ("dotted", false, &[1.44, 1.44][..]),
        ("dashedHeavy", true, &[3.84, 1.92][..]),
        ("dashLong", false, &[7.92, 3.84][..]),
        ("dashDotHeavy", true, &[2.88, 0.96, 0.96, 0.96][..]),
        (
            "dotDotDash",
            false,
            &[2.16, 0.72, 0.72, 0.72, 0.72, 0.72][..],
        ),
    ] {
        let (found, to, baseline) = at(label);
        let line = found[0];
        let placed = if heavy {
            near(baseline - line.y, 1.44) && near(line.width, 1.44)
        } else {
            close(line.y, baseline + under) && close(line.width, thick)
        };
        assert!(placed && close(line.to, to), "{label}: {line:?}");
        assert!(
            line.dash.len() == dash.len() && line.dash.iter().zip(dash).all(|(a, b)| close(*a, *b)),
            "{label}: {line:?}"
        );
    }
    for (label, centres, width) in [
        ("wave", &[1.2][..], 0.24),
        ("wavyHeavy", &[1.2][..], 0.96),
        ("wavyDouble", &[0.96, 1.44][..], 0.24),
    ] {
        let (found, to, baseline) = at(label);
        assert_eq!(found.len(), centres.len(), "{label}: {found:?}");
        for (line, centre) in found.iter().zip(centres) {
            assert!(
                line.wave
                    && near(baseline - line.y, *centre)
                    && close(line.width, width)
                    && close(line.to, to),
                "{label}: {line:?}"
            );
        }
    }

    let (words, _, _) = at("words");
    assert_eq!(words.len(), 1, "a words underline leaves out the spaces");
    let parts = strokes
        .iter()
        .filter(|m| close(m.y, words[0].y) && m.from >= words[0].from)
        .count();
    assert_eq!(parts, 3);

    let (red, _, _) = at("red-underline");
    assert_eq!(red[0].rgb, [1.0, 0.0, 0.0]);

    let (struck, to, baseline) = at("strike");
    let line = struck[0];
    assert!(
        close(line.y, baseline + strike) && close(line.width, strike_thick) && close(line.to, to),
        "{line:?}"
    );
    let (double, _, baseline) = at("dstrike");
    let ys: Vec<f64> = double.iter().map(|m| m.y).collect();
    assert_eq!(ys.len(), 2, "{double:?}");
    assert!(
        ys.iter()
            .all(|y| close((y - baseline - strike).abs(), strike_thick)),
        "{ys:?}"
    );
    let (raised, _, baseline) = at("raised-strike");
    assert!(
        close(raised[0].y, baseline + strike),
        "struck at its own raised baseline"
    );
}

/// Word for Mac draws one underline under a stretch of mixed sizes, at
/// neither size's metrics (`pr-proof/pr65/mix-word.pdf`): its top and
/// thickness, in points below the baseline, for each case, within 0.3 pt.
/// Three-run stretches are further off (11 + 11 + 24: Word 3.12, ours 3.59).
#[test]
fn a_mixed_size_underline_is_one_line_near_words() {
    const CASES: [(&[f64], f64, f64); 9] = [
        (&[8.0], 1.44, 0.48),
        (&[11.0], 1.92, 0.72),
        (&[16.0], 2.64, 1.20),
        (&[24.0], 4.08, 1.68),
        (&[8.0, 11.0], 1.68, 0.72),
        (&[11.0, 16.0], 2.40, 0.96),
        (&[11.0, 22.0], 3.36, 1.20),
        (&[11.0, 24.0], 3.60, 1.44),
        (&[24.0, 11.0], 3.60, 1.44),
    ];
    let html: String = CASES
        .iter()
        .enumerate()
        .map(|(case, (sizes, ..))| {
            let runs: String = sizes
                .iter()
                .map(|size| format!(r#"<span style="font-size: {size}pt">x{size}x </span>"#))
                .collect();
            format!("<p>case{case}: <u>{runs}</u>end</p>")
        })
        .collect();
    let bytes = pdf_of(&convert(&html, InputFormat::Html, Some("inter.docx")));
    let pdf = Pdf::parse(&bytes).unwrap();
    let glyphs = &glyphs(&pdf)[0];
    let height = pdf.media_boxes().unwrap()[0][3];
    let (strokes, _) = marks(&bytes);
    for (case, (sizes, top, thickness)) in CASES.iter().enumerate() {
        let (from, _, baseline) = sample(glyphs, height, &format!("case{case}"), 0);
        let found: Vec<&Mark> = strokes.iter().filter(|m| close(m.from, from)).collect();
        assert_eq!(found.len(), 1, "{sizes:?}: {strokes:?}");
        let line = found[0];
        let ours = (baseline - line.y - line.width / 2.0, line.width);
        assert!(
            (ours.0 - top).abs() <= 0.3 && (ours.1 - thickness).abs() <= 0.3,
            "{sizes:?}: ours {ours:?}, Word {top} and {thickness}"
        );
    }
}

/// Fill-in lines: a trailing underlined or struck tab is drawn to its
/// stop, and trailing no-break spaces are underlined; only trailing
/// ordinary spaces are left out, before a line break too.
#[test]
fn trailing_tabs_and_no_break_spaces_are_decorated() {
    let bytes = pdf_of(&open(&fixtures().join("pdf/fill-in.docx")));
    qpdf_check("fill-in.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let glyphs = &glyphs(&pdf)[0];
    let height = pdf.media_boxes().unwrap()[0][3];
    let (strokes, _) = marks(&bytes);
    let stop = 72.0 + 300.0;
    let to_stop: Vec<&Mark> = strokes.iter().filter(|m| close(m.to, stop)).collect();
    assert_eq!(to_stop.len(), 2, "{strokes:?}");
    let line = |label: &str, length: usize| {
        let (from, end, _) = sample(glyphs, height, label, length);
        let found: Vec<&Mark> = strokes.iter().filter(|m| close(m.from, from)).collect();
        assert_eq!(found.len(), 1, "{label}: {strokes:?}");
        (found[0].to, end)
    };
    let (to, last_nbsp) = line("Date", 7);
    assert!(to > last_nbsp, "{to} {last_nbsp}");
    for label in ["Spaces", "Break"] {
        let (to, first_space) = line(label, 4);
        assert!(close(to, first_space), "{label}: {to} {first_space}");
    }
}

#[test]
fn highlights_and_shading_fill_the_line_behind_the_text() {
    let bytes = pdf_of(&decorations());
    let (_, fills) = marks(&bytes);
    let rgb = |hex: u32| [16, 8, 0].map(|shift| f64::from((hex >> shift) & 0xFF) / 255.0);
    let close3 = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.001);
    for colour in [
        0x000000, 0xFFFF00, 0x000080, 0xC0C0C0, 0x7F7F7F, 0x4472C4, 0xB4C6E7,
    ] {
        assert!(
            fills.iter().any(|f| close3(f.rgb, rgb(colour))),
            "{colour:06X}: {fills:?}"
        );
    }
    assert!(
        !fills.iter().any(|f| close3(f.rgb, rgb(0x00B0F0))),
        "a highlight covers the shading under it"
    );
    let records = records(&decorations(), &fonts()).0;
    let heights: Vec<f64> = records.iter().map(|line| line.height).collect();
    assert!(
        fills
            .iter()
            .all(|f| heights.iter().any(|h| (f.width - h).abs() < 0.002)),
        "each fill is as tall as its line"
    );
    let content = {
        let pdf = Pdf::parse(&bytes).unwrap();
        let page = pdf.pages().unwrap()[0];
        String::from_utf8_lossy(&pdf.stream_data(pdf.get(page, "Contents")).unwrap()).into_owned()
    };
    for (at, _) in content.match_indices("re f\nQ\n") {
        let next = &content[at + 7..];
        let before = |op: &str| next.find(op).unwrap_or(next.len());
        assert!(
            before("BT") < before("re f").min(before(" S\n")),
            "text follows its fills: {}",
            &next[..next.len().min(200)]
        );
    }
}

#[test]
fn auto_text_is_white_on_a_dark_fill_and_hidden_text_is_not_drawn() {
    let bytes = pdf_of(&decorations());
    let pdf = Pdf::parse(&bytes).unwrap();
    let content = String::from_utf8_lossy(
        &pdf.stream_data(pdf.get(pdf.pages().unwrap()[0], "Contents"))
            .unwrap(),
    )
    .into_owned();
    assert!(content.contains("1 1 1 rg"), "white text");
    assert!(content.contains("1 1 1 RG"), "a white auto underline");
    let text: String = glyphs(&pdf)[0].iter().map(|g| g.text.as_str()).collect();
    assert!(
        text.contains("shown shown") && !text.contains("hidden"),
        "{text}"
    );
}

#[test]
fn a_note_whose_table_goes_on_is_refused() {
    let rows: String = (0..80)
        .map(|n| format!("    | row {n} | value |\n"))
        .collect();
    let source = format!("Cited.[^t]\n\n[^t]: A table:\n\n    | a | b |\n    |---|---|\n{rows}");
    let document = convert(&source, InputFormat::Markdown, None);
    let err = paint_for_tests(&document, &fonts(), &font(LIBERATION)).unwrap_err();
    assert_eq!(err.reason(), docrs::pdf::FOOTNOTE_LAYOUT_UNSUPPORTED);
    let fits = convert("Cited.[^t]\n\n[^t]: A note.\n", InputFormat::Markdown, None);
    paint_for_tests(&fits, &fonts(), &font(LIBERATION)).unwrap();
}

/// Each page's lines from top to bottom, a line being the glyphs on one
/// baseline.
fn page_lines(bytes: &[u8]) -> Vec<Vec<String>> {
    glyphs(&Pdf::parse(bytes).unwrap())
        .iter()
        .map(|page| {
            let mut lines: Vec<(f64, String)> = Vec::new();
            for glyph in page {
                match lines.iter_mut().find(|(y, _)| (y - glyph.y).abs() < 0.01) {
                    Some((_, text)) => text.push_str(&glyph.text),
                    None => lines.push((glyph.y, glyph.text.clone())),
                }
            }
            lines.sort_by(|a, b| a.0.total_cmp(&b.0));
            lines.into_iter().map(|(_, text)| text).collect()
        })
        .collect()
}

#[test]
fn each_page_draws_its_header_and_page_x_of_y_footer_as_word_does() {
    let (_, document) = cases()
        .into_iter()
        .find(|(case, _)| case == "stories-arial")
        .unwrap();
    let bytes = paint_for_tests(&document, &fonts(), &font(LIBERATION)).unwrap();
    let word = std::fs::read_to_string(fixtures().join("pdf/stories-arial/word.lines.json"));
    let word: serde_json::Value = serde_json::from_str(&word.unwrap()).unwrap();
    let word: Vec<Vec<String>> = word["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|page| {
            let lines = page["lines"].as_array().unwrap();
            lines
                .iter()
                .map(|l| l["text"].as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    let ends = |lines: &[String]| {
        let last = lines.last().unwrap();
        (lines[0].trim().to_string(), last.trim().to_string())
    };
    let drawn: Vec<_> = page_lines(&bytes).iter().map(|page| ends(page)).collect();
    let word: Vec<_> = word.iter().map(|page| ends(page)).collect();
    assert_eq!(drawn, word);
    assert_eq!(
        drawn[1],
        ("Even page header".to_string(), "Page 2 of 3".to_string())
    );
}

/// A run in Arial 10 pt.
fn arial(content: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="20"/></w:rPr>{content}</w:r>"#
    )
}

fn arial_text(text: &str) -> String {
    arial(&format!(r#"<w:t xml:space="preserve">{text}</w:t>"#))
}

/// A complex field showing `cached`, in Arial 10 pt.
fn arial_field(code: &str, cached: &str) -> String {
    let char = |kind: &str| arial(&format!(r#"<w:fldChar w:fldCharType="{kind}"/>"#));
    [
        char("begin"),
        arial(&format!(
            r#"<w:instrText xml:space="preserve"> {code} </w:instrText>"#
        )),
        char("separate"),
        arial_text(cached),
        char("end"),
    ]
    .concat()
}

/// A paragraph on single exact 12 pt lines with no spacing.
fn exact(ppr: &str, content: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="exact"/>{ppr}</w:pPr>{content}</w:p>"#
    )
}

/// `body` closed by a [`LETTER`] section with `refs` and `extra`.
fn sectioned(body: String, refs: &str, extra: &str) -> String {
    body + &format!("<w:sectPr>{refs}{LETTER}{extra}</w:sectPr>")
}

/// A header and a footer with a page field are laid out again on each
/// page; each one's link to a missing bookmark is counted once.
#[test]
fn a_broken_link_in_a_header_and_a_footer_with_a_field_is_counted_once_each() {
    let story = |target: &str| {
        let link = format!(
            r#"<w:hyperlink w:anchor="{target}">{}</w:hyperlink>"#,
            text("Link")
        );
        exact("", &[link, serif_field("PAGE", "1")].concat())
    };
    let body: String = (1..=4)
        .map(|n| exact(r#"<w:pageBreakBefore/>"#, &text(&format!("Page {n}"))))
        .collect();
    let body = sectioned(
        body,
        r#"<w:headerReference w:type="default" r:id="rheader1"/><w:footerReference w:type="default" r:id="rfooter1"/>"#,
        "",
    );
    let document = package(
        &body,
        &[
            ("header1", "header", "hdr", &story("nowhere")),
            ("footer1", "footer", "ftr", &story("elsewhere")),
        ],
    );
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let (bytes, report) = render_unchecked_for_tests(&document, &options).unwrap();
    assert_eq!(page_lines(&bytes).len(), 4);
    let missing: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.code == "pdf_link_target_missing")
        .map(|w| w.count)
        .collect();
    assert_eq!(missing, [2]);
}

#[test]
fn a_footer_that_wraps_at_ten_pages_is_drawn_on_all_ten() {
    let face = FontBook::new(Vec::new()).face("Arial", false, false, FontKind::Sans);
    let width = |text: &str| text.chars().map(|c| face.advance(c) * 10.0).sum::<f64>();
    let room = (width("Page 9 of 9") + width("Page 1 of 10")) / 2.0;
    let indent = format!(r#"<w:ind w:right="{}"/>"#, ((468.0 - room) * 20.0).round());
    let footer = exact(
        &indent,
        &[
            arial_text("Page "),
            arial_field("PAGE", "1"),
            arial_text(" of "),
            arial_field("NUMPAGES", "1"),
        ]
        .concat(),
    );
    // With the footer half an inch from the edge in place of the bottom
    // margin, one footer line leaves 56 body lines a page: one line more
    // than nine pages of them makes ten pages, whose two-line footers
    // leave 55.
    let body: String = (1..=9 * 56 + 1)
        .map(|n| exact("", &arial_text(&format!("Line {n}"))))
        .collect();
    let body = sectioned(
        body,
        r#"<w:footerReference w:type="default" r:id="rfooter1"/>"#,
        "",
    )
    .replace(r#"w:bottom="1440""#, r#"w:bottom="720""#);
    let document = package(&body, &[("footer1", "footer", "ftr", &footer)]);
    let bytes = paint_for_tests(&document, &fonts(), &font(LIBERATION)).unwrap();
    let pages = page_lines(&bytes);
    assert_eq!(pages.len(), 10);
    for (at, lines) in pages.iter().enumerate() {
        let footer = &lines[lines.len() - 2..];
        assert_eq!(footer[1].trim(), "10", "page {}: {footer:?}", at + 1);
        let shown = footer.concat().replace(' ', "");
        assert_eq!(shown, format!("Page{}of10", at + 1));
    }
}

/// The page number each contents line draws: the glyphs of the line
/// layout records for it from its last tab's end.
fn drawn_contents_numbers(bytes: &[u8], document: &Document, fonts: &FontSet) -> Vec<String> {
    let pages = glyphs(&Pdf::parse(bytes).unwrap());
    let (lines, _) = records(document, fonts);
    lines
        .iter()
        .filter(|line| line.story == "body")
        .filter_map(|line| {
            let &(_, to) = line.tabs.last()?;
            let number = pages[line.page as usize - 1]
                .iter()
                .filter(|g| (g.y - line.baseline).abs() < 0.01 && g.x > to - 0.01)
                .map(|g| g.text.as_str())
                .collect::<String>();
            Some(number.trim().to_string())
        })
        .collect()
}

#[test]
fn the_contents_numbers_drawn_are_the_pdf_s_pages_on_every_contents_fixture() {
    let tsv = |path: &str| {
        let text = std::fs::read_to_string(fixtures().join("../..").join(path)).unwrap();
        text.lines()
            .skip(1)
            .map(|l| l.split('\t').map(str::to_string).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    // Reference, source, font.
    let mut cases: Vec<(String, String, Option<String>)> = tsv("tools/pdf/corpus.tsv")
        .into_iter()
        .map(|row| (row[1].clone(), row[3].clone(), None))
        .collect();
    cases.extend(
        tsv("tests/fixtures/pdf/cases.tsv")
            .into_iter()
            .filter(|row| row[1].ends_with("toc.md"))
            .map(|row| (row[2].clone(), row[1].clone(), Some(row[3].clone())))
            .map(|(reference, source, font)| (reference, source, font.filter(|f| f != "-"))),
    );
    let stand_in = font(LIBERATION);
    let mut checked = 0;
    for (reference, input, font) in &cases {
        let mut builder = FontSet::builder();
        if let Some(font) = font {
            builder
                .add(std::fs::read(fixtures().join("../..").join(font)).unwrap())
                .unwrap();
        }
        let fonts = builder.build();
        let format = match input.rsplit('.').next() {
            Some("html") => InputFormat::Html,
            Some("json") => InputFormat::Tiptap,
            _ => InputFormat::Markdown,
        };
        let source = std::fs::read_to_string(fixtures().join("../..").join(input)).unwrap();
        let mut document = open(&fixtures().join("../..").join(reference));
        let options = MarkdownOptions {
            toc: Some(true),
            ..MarkdownOptions::default()
        };
        insert_source_with(&mut document, &source, format, &options, &fonts).unwrap();
        let entries = support::toc_entries(&document);
        if entries.is_empty() {
            let xml = support::document_xml(&document);
            assert!(!xml.contains("PAGEREF"), "{reference} {input}");
            continue;
        }
        stale_contents(&mut document);
        let stale = support::toc_entries(&document);
        assert!(stale.iter().all(|(_, shown)| shown == "99"), "{stale:?}");
        let bytes = paint_for_tests(&document, &fonts, &stand_in).unwrap();
        let drawn = drawn_contents_numbers(&bytes, &document, &fonts);
        let layout = layout_for_tests(&document, &fonts).unwrap();
        let settled: Vec<String> = entries
            .iter()
            .map(|(bookmark, _)| {
                let page = layout.bookmark_page(bookmark).unwrap();
                layout.page_number(page).to_string()
            })
            .collect();
        // The conversion saved the settled numbers; the PDF draws them
        // again over the stale 99s.
        let saved: Vec<String> = entries.into_iter().map(|(_, shown)| shown).collect();
        assert_eq!(saved, settled, "{reference} {input}");
        assert_eq!(drawn[..saved.len()], settled, "{reference} {input}");
        checked += saved.len();
    }
    assert!(checked >= 300, "{checked}");
}

/// `document` with each contents entry's saved page number replaced by
/// 99, so only a number computed from the PDF's pages can be right.
fn stale_contents(document: &mut Document) {
    const LINK: &str = "<w:hyperlink w:anchor=\"";
    let xml = support::document_xml(document);
    let mut chunks: Vec<String> = xml.split(LINK).map(str::to_string).collect();
    for chunk in chunks.iter_mut().skip(1) {
        let Some(end) = chunk.find("</w:hyperlink>") else {
            continue;
        };
        if !chunk[..end].contains("PAGEREF") {
            continue;
        }
        let at = chunk[..end].rfind("<w:t>").unwrap() + "<w:t>".len();
        let close = at + chunk[at..].find('<').unwrap();
        chunk.replace_range(at..close, "99");
    }
    let tree = docrs::xml::parse_str(&chunks.join(LINK)).unwrap();
    document
        .package_mut_for_tests()
        .commit_tree("word/document.xml", tree)
        .unwrap();
}

/// A footer of `content` over a one-page body, in Liberation Serif.
fn footed(content: &str) -> Document {
    let body = sectioned(
        tabbed("", "Body"),
        r#"<w:footerReference w:type="default" r:id="rfooter1"/>"#,
        "",
    );
    let footer = format!("<w:p>{content}</w:p>");
    package(&body, &[("footer1", "footer", "ftr", &footer)])
}

fn serif_field(code: &str, cached: &str) -> String {
    field_of(code, Some(cached))
}

/// A complex field in Liberation Serif; `None` writes no `separate` and
/// no result.
fn field_of(code: &str, cached: Option<&str>) -> String {
    let char = |kind: &str| serif(&format!(r#"<w:fldChar w:fldCharType="{kind}"/>"#));
    let mut out = char("begin")
        + &serif(&format!(
            r#"<w:instrText xml:space="preserve"> {code} </w:instrText>"#
        ));
    if let Some(cached) = cached {
        out += &(char("separate") + &text(cached));
    }
    out + &char("end")
}

#[test]
fn strict_accepts_page_fields_and_refuses_a_date() {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.strict = true;
    let fields = [
        text("Page "),
        serif_field("PAGE", "7"),
        text(" of "),
        serif_field("NUMPAGES", "7"),
        text(", section "),
        serif_field("SECTIONPAGES", "7"),
    ]
    .concat();
    let body = sectioned(
        tabbed("", "Body")
            + r#"<w:p><w:bookmarkStart w:id="0" w:name="here"/>"#
            + &serif_field("PAGEREF here", "9")
            + r#"<w:bookmarkEnd w:id="0"/></w:p>"#,
        r#"<w:footerReference w:type="default" r:id="rfooter1"/>"#,
        "",
    );
    let document = package(
        &body,
        &[("footer1", "footer", "ftr", &format!("<w:p>{fields}</w:p>"))],
    );
    let (bytes, report) = render(&document, &options).unwrap();
    assert_eq!(report.warnings, []);
    let lines = &page_lines(&bytes)[0];
    assert_eq!(lines[1..], ["1", "Page 1 of 1, section 1"]);

    let date = footed(&[text("Printed "), serif_field("DATE", "1 May 2026")].concat());
    let refused = render(&date, &options).map(|_| ()).unwrap_err();
    assert_eq!(refused.reason(), "pdf_not_faithful");
    options.strict = false;
    let (bytes, report) = render(&date, &options).unwrap();
    let cached = report
        .warnings
        .iter()
        .find(|w| w.code == "pdf_field_cached")
        .unwrap();
    assert_eq!(
        (cached.subject, cached.locations[0].story.as_str()),
        ("DATE", "word/footer1.xml")
    );
    assert_eq!(page_lines(&bytes)[0][1], "Printed 1 May 2026");
}

#[test]
fn what_layout_changes_in_a_story_is_reported_with_its_page() {
    let header: String = (1..=40)
        .map(|n| tabbed("", &format!("Letterhead {n}")))
        .collect();
    let body = sectioned(
        tabbed("", "Body"),
        r#"<w:headerReference w:type="default" r:id="rheader1"/><w:footerReference w:type="default" r:id="rfooter1"/>"#,
        "",
    );
    let footer = format!("<w:p>{}</w:p>", serif_field("PAGE \\* Ordinal", "1st"));
    let document = package(
        &body,
        &[
            ("header1", "header", "hdr", &header),
            ("footer1", "footer", "ftr", &footer),
        ],
    );
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let (_, report) = render(&document, &options).unwrap();
    let found: Vec<_> = report
        .warnings
        .iter()
        .map(|w| {
            let at = &w.locations[0];
            (w.code, w.subject, at.story.as_str(), at.ordinal, at.page)
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "pdf_field_format_approximated",
                "page field",
                "word/footer1.xml",
                0,
                None
            ),
            (
                "pdf_story_overflow",
                "header",
                "word/header1.xml",
                0,
                Some(1)
            ),
        ]
    );
    options.strict = true;
    let refused = render(&document, &options).map(|_| ()).unwrap_err();
    assert_eq!(refused.reason(), "pdf_not_faithful");
}

/// The preflight finds a PAGEREF without a bookmark in the body and layout
/// one to a missing bookmark in the footer: one warning names both.
#[test]
fn a_layout_warning_the_preflight_gave_in_another_story_adds_its_story() {
    let body = sectioned(
        format!("<w:p>{}</w:p>", serif_field("PAGEREF", "9")),
        r#"<w:footerReference w:type="default" r:id="rfooter1"/>"#,
        "",
    );
    let footer = format!("<w:p>{}</w:p>", serif_field("PAGEREF nowhere", "9"));
    let document = package(&body, &[("footer1", "footer", "ftr", &footer)]);
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let (_, report) = render(&document, &options).unwrap();
    let [cached] = report.warnings.as_slice() else {
        panic!("{:?}", report.warnings);
    };
    let stories: Vec<_> = cached.locations.iter().map(|l| l.story.as_str()).collect();
    assert_eq!(
        (cached.code, cached.subject, cached.count),
        ("pdf_field_cached", "PAGEREF", 2)
    );
    assert_eq!(stories, ["word/document.xml", "word/footer1.xml"]);
    assert!(
        cached.message.starts_with("PAGEREF (2): "),
        "{}",
        cached.message
    );
}

/// What a PDF must do with a probe: draw a line, or, under `strict`,
/// refuse it and otherwise draw a line with a warning of code and subject.
enum Expect {
    Drawn(&'static str),
    Warned(&'static str, &'static str, &'static str),
}

/// A contents of `entries` (text and tabs, `\t`), each followed by a
/// field of `code` saved as 99 when `code` is not empty.
fn contents(entries: &[&str], code: &str) -> String {
    let char = |kind: &str| serif(&format!(r#"<w:fldChar w:fldCharType="{kind}"/>"#));
    let mut out = String::new();
    for (at, entry) in entries.iter().enumerate() {
        let mut runs: String = entry
            .split('\t')
            .map(text)
            .collect::<Vec<_>>()
            .join(&serif(TAB));
        if !code.is_empty() {
            runs += &serif_field(code, "99");
        }
        let begin = if at == 0 {
            char("begin")
                + &serif(r#"<w:instrText xml:space="preserve"> TOC \o "1-3" </w:instrText>"#)
                + &char("separate")
        } else {
            String::new()
        };
        let end = if at + 1 == entries.len() {
            char("end")
        } else {
            String::new()
        };
        out += &format!("<w:p>{begin}{runs}{end}</w:p>");
    }
    out
}

/// A one-page body of `body` under a footer of `footer` (none when empty),
/// in a section with `extra`.
fn probe(body: &str, footer: &str, extra: &str) -> Document {
    let refs = if footer.is_empty() {
        ""
    } else {
        r#"<w:footerReference w:type="default" r:id="rfooter1"/>"#
    };
    let body = sectioned(tabbed("", "Body") + body, refs, extra);
    let footer = format!("<w:p>{footer}</w:p>");
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/></w:rPr></w:rPrDefault></w:docDefaults>"#;
    let mut parts = vec![("styles", "styles", "styles", styles)];
    if !refs.is_empty() {
        parts.push(("footer1", "footer", "ftr", &footer));
    }
    package(&body, &parts)
}

/// The fields the PR 73 review probed: drawn as computed, or refused by
/// `strict`, never drawn otherwise in silence.
#[test]
fn strict_never_accepts_a_field_drawn_other_than_as_computed() {
    use Expect::*;
    let page = |code: &str, cached: &str| text("Page ") + &serif_field(code, cached);
    let footed = |content: String, extra: &str| probe("", &content, extra);
    let see = |code: &str| {
        let body = format!("<w:p>{}{}</w:p>", text("See page "), serif_field(code, "9"));
        probe(&body, "", "")
    };
    let format = |fmt: &str| {
        footed(
            serif_field("PAGE", "x"),
            &format!(r#"<w:pgNumType w:fmt="{fmt}"/>"#),
        )
    };
    let cases: Vec<(&str, Document, Expect)> = vec![
        ("saved 7", footed(page("PAGE", "7"), ""), Drawn("Page 1")),
        (
            "no separate",
            footed(text("Page ") + &field_of("PAGE", None), ""),
            Drawn("Page 1"),
        ),
        (
            "empty fldSimple",
            footed(text("Page ") + r#"<w:fldSimple w:instr=" PAGE "/>"#, ""),
            Drawn("Page 1"),
        ),
        (
            "no space before a switch",
            footed(page("PAGE\\* roman", "vii"), ""),
            Drawn("Page i"),
        ),
        (
            "case over roman",
            footed(serif_field("PAGE \\* roman \\* Upper", "i"), ""),
            Drawn("I"),
        ),
        ("numberInDash", format("numberInDash"), Drawn("- 1 -")),
        ("decimalZero", format("decimalZero"), Drawn("01")),
        ("upperRoman", format("upperRoman"), Drawn("I")),
        (
            "body page fields",
            probe(
                &format!(
                    "<w:p>{}{}{}{}</w:p>",
                    text("Page "),
                    serif_field("PAGE", "7"),
                    text(" of "),
                    serif_field("NUMPAGES", "7")
                ),
                "",
                "",
            ),
            Drawn("Page 1 of 1"),
        ),
        (
            "missing bookmark",
            see("PAGEREF nowhere \\h"),
            Warned("See page 9", "pdf_field_cached", "PAGEREF"),
        ),
        (
            "bare PAGEREF",
            see("PAGEREF"),
            Warned("See page 9", "pdf_field_cached", "PAGEREF"),
        ),
        (
            "PAGE with an operand",
            footed(page("PAGE extra", "7"), ""),
            Warned("Page 7", "pdf_field_cached", "PAGE"),
        ),
        (
            "cardinalText",
            format("cardinalText"),
            Warned("1", "pdf_field_format_approximated", "page number"),
        ),
        (
            "ordinal",
            format("ordinal"),
            Warned("1", "pdf_field_format_approximated", "page number"),
        ),
        (
            "chapter numbers",
            footed(
                serif_field("PAGE", "1-1"),
                r#"<w:pgNumType w:chapStyle="1" w:chapSep="hyphen"/>"#,
            ),
            Warned("1", "pdf_field_format_approximated", "page number"),
        ),
        (
            "chapter numbers with an arabic switch",
            footed(
                serif_field("PAGE \\* Arabic", "1-1"),
                r#"<w:pgNumType w:chapStyle="1" w:chapSep="hyphen"/>"#,
            ),
            Warned("1", "pdf_field_format_approximated", "page number"),
        ),
        (
            "locked",
            footed(
                page("PAGE", "7").replace(
                    r#"w:fldCharType="begin""#,
                    r#"w:fldCharType="begin" w:fldLock="1""#,
                ),
                "",
            ),
            Drawn("Page 7"),
        ),
        (
            "no end",
            footed(
                text("Page ") + &field_of("PAGE", None).replace(r#"w:fldCharType="end""#, ""),
                "",
            ),
            Warned("Page ", "pdf_field_cached", "field without an end"),
        ),
        (
            "contents of plain numbers",
            probe(&contents(&["Alpha\t99", "Beta\t98"], ""), "", ""),
            Warned("Alpha99", "pdf_field_cached", "TOC"),
        ),
        (
            "contents of page references",
            probe(
                &(contents(&["Alpha\t", "Beta\t"], "PAGEREF here")
                    + r#"<w:p><w:bookmarkStart w:id="0" w:name="here"/><w:bookmarkEnd w:id="0"/></w:p>"#),
                "",
                "",
            ),
            Drawn("Alpha1"),
        ),
        (
            "contents of a plain number before a page reference",
            probe(
                &(contents(&["Alpha\t99"], "PAGEREF here")
                    + r#"<w:p><w:bookmarkStart w:id="0" w:name="here"/><w:bookmarkEnd w:id="0"/></w:p>"#),
                "",
                "",
            ),
            Warned("Alpha991", "pdf_field_cached", "TOC"),
        ),
        (
            "contents of numbered headings",
            probe(
                &(contents(&["1 Alpha\t"], "PAGEREF here")
                    + r#"<w:p><w:bookmarkStart w:id="0" w:name="here"/><w:bookmarkEnd w:id="0"/></w:p>"#),
                "",
                "",
            ),
            Drawn("1 Alpha1"),
        ),
    ];
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    for (name, document, expect) in cases {
        options.strict = true;
        let strict = render(&document, &options);
        options.strict = false;
        let (bytes, report) = render(&document, &options).unwrap();
        let lines = &page_lines(&bytes)[0];
        let line = match expect {
            Drawn(line) => {
                assert!(strict.is_ok(), "{name}: {:?}", strict.err());
                assert_eq!(report.warnings, [], "{name}");
                line
            }
            Warned(line, code, subject) => {
                let refused = strict.map(|_| ()).unwrap_err();
                assert_eq!(refused.reason(), "pdf_not_faithful", "{name}");
                let found: Vec<_> = report
                    .warnings
                    .iter()
                    .map(|w| (w.code, w.subject))
                    .collect();
                assert_eq!(found, [(code, subject)], "{name}");
                line
            }
        };
        assert!(lines.iter().any(|l| l == line), "{name}: {lines:?}");
    }
}

/// The preflight reports the chapter numbers that layout leaves out of a
/// page number, as the render does.
#[test]
fn the_preflight_reports_chapter_numbers_the_render_leaves_out() {
    let chapters = r#"<w:pgNumType w:chapStyle="1" w:chapSep="hyphen"/>"#;
    let approximated = |document: &Document| {
        let report = preflight(document).unwrap();
        report
            .warnings
            .iter()
            .filter(|w| w.code == "pdf_field_format_approximated")
            .map(|w| (w.subject, w.locations[0].story.clone()))
            .collect::<Vec<_>>()
    };
    let footed = probe("", &serif_field("PAGE \\* Arabic", "1-1"), chapters);
    assert_eq!(
        approximated(&footed),
        [("page number", "word/footer1.xml".to_string())]
    );
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let (_, report) = render(&footed, &options).unwrap();
    let rendered: Vec<_> = report
        .warnings
        .iter()
        .map(|w| (w.code, w.subject))
        .collect();
    assert_eq!(rendered, [("pdf_field_format_approximated", "page number")]);
    let counted = probe("", &serif_field("NUMPAGES", "1"), chapters);
    assert_eq!(approximated(&counted), []);
    let locked = serif_field("PAGE", "1-1").replace(
        r#"w:fldCharType="begin""#,
        r#"w:fldCharType="begin" w:fldLock="1""#,
    );
    assert_eq!(approximated(&probe("", &locked, chapters)), []);
    let plain = probe("", &serif_field("PAGE", "1"), "");
    assert_eq!(approximated(&plain), []);
}

/// A page number, and a page reference to a bookmark, in a paragraph that
/// runs on over pages show the page of the line holding them, not the
/// paragraph's first.
#[test]
fn a_page_field_and_a_bookmark_take_the_page_of_their_line() {
    let words = "word ".repeat(1500);
    let body = format!(
        r#"<w:p>{}{}{}<w:bookmarkStart w:id="0" w:name="end"/>{}<w:bookmarkEnd w:id="0"/></w:p><w:p>{}{}</w:p>"#,
        text(&words),
        text("PAGE="),
        serif_field("PAGE", "9"),
        text("."),
        text("See "),
        serif_field("PAGEREF end", "9"),
    );
    let document = probe(&body, "", "");
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.strict = true;
    let (bytes, report) = render(&document, &options).unwrap();
    assert_eq!(report.warnings, []);
    let pages = page_lines(&bytes);
    let last = pages.len();
    assert!(last > 1, "{last}");
    let lines = pages.last().unwrap();
    let ending = lines.iter().find(|l| l.contains("PAGE=")).unwrap();
    assert!(ending.ends_with(&format!("PAGE={last}.")), "{lines:?}");
    assert!(lines.contains(&format!("See {last}")), "{lines:?}");
}

#[test]
fn a_header_no_page_shows_is_not_checked() {
    let unshown = format!(
        "<w:p>{}{}</w:p>",
        r#"<w:r><w:rPr><w:rFonts w:ascii="Nowhere Sans" w:hAnsi="Nowhere Sans"/></w:rPr><w:t>Even header</w:t></w:r>"#,
        serif_field("PAGE \\* Ordinal", "1st")
    );
    let body = sectioned(
        tabbed("", "Body"),
        r#"<w:headerReference w:type="default" r:id="rheader1"/><w:headerReference w:type="even" r:id="rheader2"/>"#,
        "",
    );
    let document = package(
        &body,
        &[
            ("header1", "header", "hdr", &tabbed("", "Default header")),
            ("header2", "header", "hdr", &unshown),
        ],
    );
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.strict = true;
    let (bytes, report) = render(&document, &options).unwrap();
    assert_eq!(report.warnings, []);
    assert_eq!(page_lines(&bytes)[0], ["Default header", "Body"]);
}

#[test]
fn a_shown_footer_and_a_page_field_s_value_refuse_a_family_without_a_program() {
    let nowhere = |inner: &str| {
        format!(
            r#"<w:r><w:rPr><w:rFonts w:ascii="Nowhere Sans" w:hAnsi="Nowhere Sans"/></w:rPr>{inner}</w:r>"#
        )
    };
    // The text, then a PAGE field with no result: only its value is drawn.
    let text_footer = format!("<w:p>{}</w:p>", nowhere("<w:t>Footer</w:t>"));
    let field_footer = format!(
        "<w:p>{}{}{}</w:p>",
        nowhere(r#"<w:fldChar w:fldCharType="begin"/>"#),
        nowhere(r#"<w:instrText xml:space="preserve"> PAGE \* roman </w:instrText>"#),
        nowhere(r#"<w:fldChar w:fldCharType="end"/>"#),
    );
    for footer in [text_footer, field_footer] {
        let body = sectioned(
            tabbed("", "Body"),
            r#"<w:footerReference w:type="default" r:id="rfooter1"/>"#,
            "",
        );
        let document = package(&body, &[("footer1", "footer", "ftr", &footer)]);
        let mut options = PdfOptions::default();
        options.fonts = fonts();
        let err = render(&document, &options).unwrap_err();
        assert_eq!(err.reason(), "pdf_font_program_missing", "{footer}");
        let families: Vec<&str> = err
            .details()
            .unwrap()
            .missing_fonts
            .iter()
            .map(|font| font.family.as_str())
            .collect();
        assert_eq!(families, ["Nowhere Sans"], "{footer}");
    }
}
