//! Paragraph borders and shading in the PDF, read back by the test reader:
//! the converter's rule, quote and code block where Word draws them, fills
//! behind the text, and `tests/fixtures/pdf/borders.docx`, which `fixture()`
//! builds (written with `DOCRS_UPDATE_SNAPSHOTS=1`).

mod support;

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;

use docrs::clock::Timestamp;
use docrs::layout::{FontSet, LineRecord};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{PdfOptions, paint_for_tests, render};
use docrs::txn::Txn;
use docrs::{Document, Limits, Tolerance};
use reader::{Pdf, tokenize_content};
use support::*;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
/// Word for Mac draws a paragraph's box this far outside its indents.
const OUTSET: f64 = 1.44;

/// Under `tests/fixtures`.
fn fixtures() -> std::path::PathBuf {
    repo_root().join("tests/fixtures")
}

fn font(path: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("fonts").join(path)).unwrap()
}

/// Inter, and Liberation Serif as Times New Roman and as the converter's
/// code font.
fn fonts() -> FontSet {
    let mut builder = FontSet::builder();
    for path in [
        "inter/Inter[opsz,wght].ttf",
        "liberation/LiberationSerif-Regular.ttf",
    ] {
        builder.add(font(path)).unwrap();
    }
    builder.alias("Times New Roman", "Liberation Serif");
    builder.alias("Consolas", "Liberation Serif");
    builder.build()
}

fn open(path: &str) -> Document {
    Document::open_path_with(
        repo_root().join(path),
        &Limits::default(),
        Tolerance::word(),
    )
    .unwrap()
    .0
}

/// `markdown` converted into the reference at `path` (from the repository
/// root), at a fixed time.
fn convert(markdown: &str, path: &str) -> Document {
    let mut document = open(path);
    let options = MarkdownOptions {
        now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
        ..MarkdownOptions::default()
    };
    insert_source_with(
        &mut document,
        markdown,
        InputFormat::Markdown,
        &options,
        &fonts(),
    )
    .unwrap();
    document
}

fn pdf_of(document: &Document) -> Vec<u8> {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    render(document, &options).unwrap().0
}

/// The body's lines as layout records them.
fn records(document: &Document) -> Vec<LineRecord> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name().to_string();
    let mut out = Vec::new();
    docrs::layout::line_records_with(
        &txn,
        &main,
        &fonts(),
        |_, _| Ok(()),
        |line| {
            out.push(line);
            Ok(())
        },
    )
    .unwrap();
    out
}

fn line<'a>(records: &'a [LineRecord], text: &str) -> &'a LineRecord {
    records.iter().find(|r| r.text.starts_with(text)).unwrap()
}

/// A filled rectangle or a stroked line as drawn, in points from the
/// page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    colour: [u8; 3],
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    stroked: bool,
    /// Drawn after the page's first text.
    over_text: bool,
}

impl Shape {
    fn horizontal(&self) -> bool {
        self.right - self.left > self.bottom - self.top
    }

    fn thickness(&self) -> f64 {
        (self.right - self.left).min(self.bottom - self.top)
    }
}

/// Each page's shapes, in drawing order.
fn shapes(bytes: &[u8]) -> Vec<Vec<Shape>> {
    let pdf = Pdf::parse(bytes).unwrap();
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    pdf.pages()
        .unwrap()
        .into_iter()
        .zip(heights)
        .map(|(page, height)| {
            let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
            let mut out = Vec::new();
            let (mut fill, mut stroke, mut width) = ([0; 3], [0; 3], 1.0);
            let mut saved = Vec::new();
            let (mut rects, mut path) = (Vec::new(), Vec::new());
            let mut over_text = false;
            for op in tokenize_content(&content).unwrap() {
                let n = |at: usize| op.operands[at].number().unwrap();
                let rgb = || [0, 1, 2].map(|at| (n(at) * 255.0).round() as u8);
                let shape = |colour, (left, top, right, bottom), stroked| Shape {
                    colour,
                    left,
                    top,
                    right,
                    bottom,
                    stroked,
                    over_text,
                };
                match op.operator.as_str() {
                    "q" => saved.push((fill, stroke, width)),
                    "Q" => (fill, stroke, width) = saved.pop().unwrap(),
                    "rg" => fill = rgb(),
                    "RG" => stroke = rgb(),
                    "w" => width = n(0),
                    "BT" => over_text = true,
                    "re" => rects.push((n(0), height - n(1) - n(3), n(0) + n(2), height - n(1))),
                    "f" => out.extend(rects.drain(..).map(|r| shape(fill, r, false))),
                    "m" | "l" => path.push((n(0), height - n(1))),
                    "S" => {
                        let [(x0, y0), (x1, y1)] = [path[0], path[path.len() - 1]];
                        let half = width / 2.0;
                        let bounds = if y0 == y1 {
                            (x0.min(x1), y0 - half, x0.max(x1), y0 + half)
                        } else {
                            (x0 - half, y0.min(y1), x0 + half, y0.max(y1))
                        };
                        out.push(shape(stroke, bounds, true));
                        path.clear();
                    }
                    _ => {}
                }
            }
            out
        })
        .collect()
}

fn coloured(shapes: &[Shape], hex: u32) -> Vec<Shape> {
    let colour = [16, 8, 0].map(|shift| (hex >> shift) as u8);
    shapes
        .iter()
        .filter(|s| s.colour == colour)
        .copied()
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.002
}

fn converted() -> Document {
    let source = std::fs::read_to_string(fixtures().join("pdf/sources/borders.md")).unwrap();
    convert(&source, "tests/fixtures/pdf/references/inter.docx")
}

#[test]
fn the_converters_rule_is_a_line_below_its_paragraph() {
    let document = converted();
    let records = records(&document);
    let above = records
        .iter()
        .position(|r| r.text.starts_with("Text above"))
        .unwrap();
    let rule = &records[above + 1];
    let below = line(&records, "Text below");
    let shapes = &shapes(&pdf_of(&document))[0];
    let lines: Vec<_> = coloured(shapes, 0x000000)
        .into_iter()
        .filter(|s| s.horizontal())
        .collect();
    assert_eq!(lines.len(), 1, "{shapes:?}");
    let drawn = lines[0];
    // ¾ pt drawn 0.72 thick, its space of 1 pt below the line.
    assert!(close(drawn.thickness(), 0.72), "{drawn:?}");
    assert!(close(drawn.top, rule.y + rule.height + 1.0), "{drawn:?}");
    assert!(close(drawn.left, rule.x - OUTSET), "{drawn:?}");
    assert!(close(drawn.right, 612.0 - 90.0 + OUTSET), "{drawn:?}");
    // The paragraph is the line's width and space taller.
    let after_rule = below.y - (rule.y + rule.height);
    assert!(close(after_rule, 0.75 + 1.0 + 6.0), "{after_rule}");
}

#[test]
fn a_quote_has_its_line_left_of_its_space_and_through_its_spacing() {
    let document = converted();
    let records = records(&document);
    let first = line(&records, "A block quote");
    let second = line(&records, "Its second paragraph");
    let shapes = &shapes(&pdf_of(&document))[0];
    let bars = coloured(shapes, 0xBFBFBF);
    assert_eq!(bars.len(), 2, "{shapes:?}");
    for (bar, text) in bars.iter().zip([first, second]) {
        // 2¼ pt drawn 2.16 thick, its inner edge 8 pt and the outset left
        // of the text.
        assert!(close(bar.thickness(), 2.16), "{bar:?}");
        assert!(close(bar.right, text.x - 8.0 - OUTSET), "{bar:?}");
        let (top, bottom) = (text.y + 0.002, text.y + text.height - 0.002);
        assert!(bar.top <= top && bar.bottom >= bottom, "{bar:?} {text:?}");
    }
    assert!(close(bars[0].bottom, bars[1].top), "{bars:?}");
}

#[test]
fn a_code_block_is_filled_behind_its_text_to_the_outset() {
    let document = converted();
    let records = records(&document);
    let first = line(&records, "fn main");
    let last = records.iter().rfind(|r| r.text == "}").unwrap();
    let shapes = &shapes(&pdf_of(&document))[0];
    let fills = coloured(shapes, 0xF5F5F5);
    assert_eq!(fills.len(), 1, "{shapes:?}");
    let fill = fills[0];
    assert!(!fill.over_text, "{fill:?}");
    assert!(close(fill.top, first.y) && close(fill.bottom, last.y + last.height));
    assert!(close(fill.left, first.x - OUTSET) && close(fill.right, 612.0 - 90.0 + OUTSET));
}

/// `text` with `ppr`, in Times New Roman.
fn para(ppr: &str, text: &str) -> String {
    format!("<w:p><w:pPr>{ppr}</w:pPr><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

fn sides(sides: &[&str], style: &str, size: u32, space: u32, colour: &str) -> String {
    sides
        .iter()
        .map(|side| {
            format!(
                "<w:{side} w:val=\"{style}\" w:sz=\"{size}\" w:space=\"{space}\" w:color=\"{colour}\"/>"
            )
        })
        .collect()
}

fn borders(inner: &str) -> String {
    format!("<w:pBdr>{inner}</w:pBdr>")
}

const BOX: [&str; 4] = ["top", "left", "bottom", "right"];
const BROKEN: [&str; 5] = ["dotted", "dashed", "dashSmallGap", "dotDash", "dotDotDash"];
const SENTENCE: &str = "Each paragraph of this group runs over several lines, so that the group \
                        starts on the first page and ends on the second one, and the box \
                        around it opens at the page break.";

/// Each case's heading, then its paragraphs; every case has a colour of
/// its own.
fn body() -> String {
    let heading = |text: &str| para("<w:keepNext/><w:spacing w:before=\"240\"/>", text);
    let joined = borders(
        &(sides(&BOX, "single", 4, 4, "1F4E79") + &sides(&["between"], "single", 4, 4, "1F4E79")),
    );
    let thin = borders(&sides(&BOX, "single", 4, 4, "7F6000"));
    let thick = borders(&sides(&BOX, "single", 12, 4, "7F6000"));
    let callout = borders(&sides(&["left"], "thick", 24, 6, "2E75B6"))
        + "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"DEEAF6\"/><w:ind w:left=\"360\"/>";
    let shaded_box = borders(&sides(&BOX, "double", 4, 4, "C00000"))
        + "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FFF2CC\"/>";
    let styles: String = ["single", "thick", "double", "triple"]
        .iter()
        .chain(&BROKEN)
        .chain(&["thinThickSmallGap", "wave"])
        .map(|style| para(&borders(&sides(&["bottom"], style, 8, 2, "auto")), style))
        .collect();
    let split = borders(
        &(sides(&BOX, "single", 8, 4, "548235") + &sides(&["between"], "single", 4, 4, "548235")),
    );
    let long = [SENTENCE; 3].join(" ");
    [
        heading("Joined: two paragraphs with equal borders and a between line"),
        para(&joined, "The first paragraph of the group."),
        para(&joined, "The second paragraph of the group."),
        heading("Not joined: the second paragraph's lines are thicker"),
        para(&thin, "A paragraph in a box of ½ pt lines."),
        para(&thick, "A paragraph in a box of 1½ pt lines."),
        heading("A shaded callout with a thick line on its left"),
        para(&callout, "The callout's first paragraph."),
        para(
            &callout,
            "Its second paragraph: the fill runs through the gap.",
        ),
        heading("A shaded box of double lines"),
        para(&shaded_box, "The fill stops at the lines' inner edges."),
        heading("Bottom borders in each style"),
        styles,
        heading("A group split across the page"),
        para(&split, &long),
        para(&split, &long),
        para(&split, &long),
    ]
    .concat()
}

fn fixture() -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\"><w:body>{}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        body()
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"{W}\"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\" w:cs=\"Times New Roman\"/><w:sz w:val=\"24\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style></w:styles>"
    );
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>",
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>";
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
    ])
}

#[test]
fn the_committed_fixture_is_the_one_these_tests_build() {
    let path = repo_root().join("tests/fixtures/pdf/borders.docx");
    if std::env::var_os("DOCRS_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, fixture()).unwrap();
    }
    assert!(
        std::fs::read(&path).unwrap() == fixture(),
        "rerun with DOCRS_UPDATE_SNAPSHOTS=1"
    );
}

fn fixture_shapes() -> Vec<Vec<Shape>> {
    shapes(&pdf_of(&open_bytes(&fixture())))
}

#[test]
fn equal_borders_share_one_box_and_different_ones_do_not() {
    let pages = fixture_shapes();
    let horizontal = |hex| {
        coloured(&pages[0], hex)
            .iter()
            .filter(|s| s.horizontal())
            .count()
    };
    // Top, between and bottom; then two boxes of their own.
    assert_eq!(horizontal(0x1F4E79), 3);
    assert_eq!(horizontal(0x7F6000), 4);
    let sides: Vec<_> = coloured(&pages[0], 0x1F4E79)
        .into_iter()
        .filter(|s| !s.horizontal())
        .collect();
    assert_eq!(sides.len(), 4, "{sides:?}");
    assert!(close(sides[0].bottom, sides[2].top), "{sides:?}");
}

#[test]
fn fills_are_behind_the_text_and_inside_the_lines() {
    let pages = fixture_shapes();
    let page = &pages[0];
    let callout = coloured(page, 0xDEEAF6);
    let fill = coloured(page, 0xFFF2CC);
    assert_eq!((callout.len(), fill.len()), (2, 1));
    assert!(callout.iter().chain(&fill).all(|s| !s.over_text));
    // The callout's fill runs through the gap to its second paragraph.
    assert!(close(callout[0].bottom, callout[1].top), "{callout:?}");
    // Double ½ pt lines: two lines and a gap, 1½ pt in all.
    let lines = coloured(page, 0xC00000);
    let outer = |pick: fn(&Shape) -> f64, min: bool| {
        lines
            .iter()
            .map(pick)
            .fold(if min { f64::MAX } else { f64::MIN }, |a, b| {
                if min { a.min(b) } else { a.max(b) }
            })
    };
    assert_eq!(lines.len(), 8, "{lines:?}");
    assert!(
        close(fill[0].left, outer(|s| s.left, true) + 1.5),
        "{fill:?}"
    );
    assert!(close(fill[0].top, outer(|s| s.top, true) + 1.5), "{fill:?}");
    assert!(lines.iter().all(|s| s.over_text));
}

#[test]
fn broken_styles_are_stroked_and_the_rest_filled() {
    let pages = fixture_shapes();
    let black = coloured(&pages[0], 0x000000);
    let stroked = black.iter().filter(|s| s.stroked).count();
    // Single, thick, thinThickSmallGap and wave: one line each; double
    // two; triple three.
    let filled = black.iter().filter(|s| !s.stroked).count();
    assert_eq!((stroked, filled), (BROKEN.len(), 4 + 2 + 3));
}

#[test]
fn a_group_split_across_the_page_opens_at_the_break() {
    let pages = fixture_shapes();
    let tops = |page: &[Shape]| {
        let lines = coloured(page, 0x548235);
        let horizontal: Vec<f64> = lines
            .iter()
            .filter(|s| s.horizontal())
            .map(|s| s.thickness())
            .collect();
        (lines.len(), horizontal)
    };
    let (first, second) = (tops(&pages[0]), tops(&pages[1]));
    assert!(first.0 > 0 && second.0 > 0, "{first:?} {second:?}");
    // 1 pt top and bottom lines, ½ pt between lines.
    assert!(
        first.1.first().is_some_and(|t| close(*t, 0.96)),
        "{first:?}"
    );
    assert!(
        second.1.last().is_some_and(|t| close(*t, 0.96)),
        "{second:?}"
    );
}

/// `markdown` in the template at `path`, drawn with Liberation Serif for
/// the fonts the tests do not have.
fn drawn_in(path: &str, markdown: &str) -> (Vec<LineRecord>, Vec<Shape>) {
    let document = convert(markdown, path);
    let stand_in = font("liberation/LiberationSerif-Regular.ttf");
    let bytes = paint_for_tests(&document, &fonts(), &stand_in).unwrap();
    (records(&document), shapes(&bytes).swap_remove(0))
}

#[test]
fn a_templates_bordered_title_and_heading_are_drawn_below_their_text() {
    let title = "---\ntitle: Quarterly report\n---\n\nText.\n";
    let (records, shapes) = drawn_in("examples/markdown/reference.docx", title);
    let text = line(&records, "Quarterly report");
    let rule = coloured(&shapes, 0x4F81BD);
    // 1 pt, 4 pt below the title.
    assert_eq!(rule.len(), 1, "{shapes:?}");
    assert!(close(rule[0].thickness(), 0.96) && close(rule[0].top, text.y + text.height + 4.0));
    let (records, shapes) = drawn_in(
        "tests/fixtures/markdown/templates/v2/sans-teal.docx",
        "# Overview\n\nText.\n",
    );
    let text = line(&records, "Overview");
    let rule = coloured(&shapes, 0x00807F);
    assert_eq!(rule.len(), 1, "{shapes:?}");
    assert!(
        close(rule[0].top, text.y + text.height),
        "{rule:?} {text:?}"
    );
}
