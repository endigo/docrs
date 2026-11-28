//! Pictures painted from the layout, read back by the test reader: inline
//! ones on their line's baseline, anchored ones where their position puts
//! them, in their layer, each part written once.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use docrs::clock::Timestamp;
use docrs::layout::{FontSet, LineRecord, Record, records_with};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{PdfLimits, PdfOptions, paint_for_tests, preflight, render};
use docrs::txn::Txn;
use docrs::{Document, Limits, Tolerance};
use reader::{Pdf, Value, tokenize_content};
use support::{ROOT_RELS, RawMember, build_zip};

const LIBERATION: &str = "liberation/LiberationSerif-Regular.ttf";
const EMU: f64 = 12_700.0;
const NAMESPACES: &str = concat!(
    r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
    r#"xmlns:wp14="http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" "#,
    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
    r#"xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" "#,
    r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#,
);
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PICTURE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
/// Letter, with margins that differ on every side.
const SECTION: &str = r#"<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1080" w:left="1800" w:header="720" w:footer="720" w:gutter="0"/>"#;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(path)).unwrap()
}

fn liberation() -> Vec<u8> {
    read(&format!("fonts/{LIBERATION}"))
}

fn fonts() -> FontSet {
    let mut builder = FontSet::builder();
    builder.add(liberation()).unwrap();
    builder.build()
}

fn logo() -> Vec<u8> {
    read("pdf/images/logo.png")
}

/// A package: the body (its `w:sectPr` included), headers, footers and
/// footnotes `(name, root, content)`, and media `(name, bytes)`. Every
/// story relates to every media part as `rIdM<n>` from 1 (footnotes as
/// `rIdF<n>`, so a picture there reads its own part's relationships) and
/// the main part to each story as `rIdS<n>`.
struct Package<'a> {
    body: String,
    stories: Vec<(&'a str, &'a str, String)>,
    media: Vec<(&'a str, Vec<u8>)>,
}

impl Package<'_> {
    fn document(&self) -> Document {
        let mut types = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
        );
        let media_rels: String = self
            .media
            .iter()
            .enumerate()
            .map(|(at, (name, _))| {
                format!(
                    r#"<Relationship Id="rIdM{}" Type="{REL}/image" Target="media/{name}"/>"#,
                    at + 1
                )
            })
            .collect();
        let rels = |media: &str, extra: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{media}{extra}</Relationships>"#
            )
        };
        let mut story_rels = String::new();
        let mut members = Vec::new();
        for (at, (name, root, content)) in self.stories.iter().enumerate() {
            let kind = match *root {
                "hdr" => "header",
                "ftr" => "footer",
                _ => "footnotes",
            };
            types.push_str(&format!(
                r#"<Override PartName="/word/{name}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml"/>"#
            ));
            story_rels.push_str(&format!(
                r#"<Relationship Id="rIdS{}" Type="{REL}/{kind}" Target="{name}"/>"#,
                at + 1
            ));
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:{root} {NAMESPACES}>{content}</w:{root}>"#
            );
            members.push(RawMember::new(&format!("word/{name}"), xml.as_bytes()));
            members.push(RawMember::new(
                &format!("word/_rels/{name}.rels"),
                if kind == "footnotes" {
                    rels(&media_rels.replace("rIdM", "rIdF"), "")
                } else {
                    rels(&media_rels, "")
                }
                .as_bytes(),
            ));
        }
        for (name, bytes) in &self.media {
            let extension = name.rsplit('.').next().unwrap();
            let content_type = match extension {
                "jpg" => "image/jpeg".to_string(),
                "tif" => "image/tiff".to_string(),
                other => format!("image/{other}"),
            };
            types.push_str(&format!(
                r#"<Override PartName="/word/media/{name}" ContentType="{content_type}"/>"#
            ));
            members.push(RawMember::new(&format!("word/media/{name}"), bytes));
        }
        types.push_str("</Types>");
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document {NAMESPACES}><w:body>{}</w:body></w:document>"#,
            self.body
        );
        members.splice(
            0..0,
            [
                RawMember::new("[Content_Types].xml", types.as_bytes()),
                RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
                RawMember::new("word/document.xml", document.as_bytes()),
                RawMember::new(
                    "word/_rels/document.xml.rels",
                    rels(&media_rels, &story_rels).as_bytes(),
                ),
            ],
        );
        Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
    }
}

fn body(content: &str) -> Package<'static> {
    Package {
        body: format!("{content}<w:sectPr>{SECTION}</w:sectPr>"),
        stories: Vec::new(),
        media: vec![("logo.png", logo())],
    }
}

fn para(content: &str) -> String {
    format!("<w:p>{content}</w:p>")
}

/// A run of Liberation Serif.
fn text(text: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#
    )
}

/// `pic:pic` for `rel`, `width` × `height` points, with `fill` in its
/// `pic:blipFill` after the blip and `xfrm` attributes.
fn pic(rel: &str, width: f64, height: f64, fill: &str, xfrm: &str) -> String {
    let (cx, cy) = (width * EMU, height * EMU);
    format!(
        r#"<a:graphic><a:graphicData uri="{PICTURE_URI}"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="p"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rel}"/>{fill}<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm {xfrm}><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>"#
    )
}

fn inline(width: f64, height: f64, fill: &str, xfrm: &str) -> String {
    inline_of("rIdM1", width, height, fill, xfrm)
}

fn inline_of(rel: &str, width: f64, height: f64, fill: &str, xfrm: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{}" cy="{}"/><wp:docPr id="1" name="Picture 1"/>{}</wp:inline></w:drawing></w:r>"#,
        width * EMU,
        height * EMU,
        pic(rel, width, height, fill, xfrm)
    )
}

/// An anchored picture: `attributes` on `wp:anchor` (behindDoc,
/// relativeHeight, …) and its `wp:positionH`/`wp:positionV`.
fn anchor(width: f64, height: f64, attributes: &str, h: &str, v: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" locked="0" layoutInCell="1" allowOverlap="1" {attributes}><wp:simplePos x="0" y="0"/>{h}{v}<wp:extent cx="{}" cy="{}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="2" name="Picture 2"/>{}</wp:anchor></w:drawing></w:r>"#,
        width * EMU,
        height * EMU,
        pic("rIdM1", width, height, "", "")
    )
}

fn offset(axis: &str, from: &str, points: f64) -> String {
    format!(
        r#"<wp:position{axis} relativeFrom="{from}"><wp:posOffset>{}</wp:posOffset></wp:position{axis}>"#,
        points * EMU
    )
}

fn aligned(axis: &str, from: &str, align: &str) -> String {
    format!(
        r#"<wp:position{axis} relativeFrom="{from}"><wp:align>{align}</wp:align></wp:position{axis}>"#
    )
}

/// What a page draws, in content order: each image with the matrix that
/// maps its unit square and whether a clip was set, each placeholder box
/// and each text object.
#[derive(Debug, Clone, PartialEq)]
enum Mark {
    Image {
        name: String,
        ctm: [f64; 6],
        clipped: bool,
    },
    Placeholder([f64; 6]),
    /// A filled path: a paragraph box's shading or border.
    Fill,
    Text,
}

fn multiply(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn marks(pdf: &Pdf) -> Vec<Vec<Mark>> {
    pdf.pages()
        .unwrap()
        .into_iter()
        .map(|page| {
            let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
            let mut stack = Vec::new();
            let (mut ctm, mut clipped) = ([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], false);
            let mut out = Vec::new();
            for op in tokenize_content(&content).unwrap() {
                let numbers: Vec<f64> = op.operands.iter().filter_map(Value::number).collect();
                match op.operator.as_str() {
                    "q" => stack.push((ctm, clipped)),
                    "Q" => (ctm, clipped) = stack.pop().unwrap(),
                    "cm" => ctm = multiply(numbers.clone().try_into().unwrap(), ctm),
                    "W" => clipped = true,
                    "B" => out.push(Mark::Placeholder(ctm)),
                    "f" => out.push(Mark::Fill),
                    "BT" => out.push(Mark::Text),
                    "Do" => {
                        let Value::Name(name) = &op.operands[0] else {
                            panic!("Do without a name");
                        };
                        out.push(Mark::Image {
                            name: name.clone(),
                            ctm,
                            clipped,
                        });
                    }
                    _ => {}
                }
            }
            out
        })
        .collect()
}

fn images(marks: &[Mark]) -> Vec<(String, [f64; 6], bool)> {
    marks
        .iter()
        .filter_map(|mark| match mark {
            Mark::Image { name, ctm, clipped } => Some((name.clone(), *ctm, *clipped)),
            _ => None,
        })
        .collect()
}

/// Where `ctm` takes unit-square point `(u, v)` (`v` up), in points from
/// the top-left corner of a page `height` tall.
fn point(ctm: [f64; 6], (u, v): (f64, f64), height: f64) -> (f64, f64) {
    (
        u * ctm[0] + v * ctm[2] + ctm[4],
        height - (u * ctm[1] + v * ctm[3] + ctm[5]),
    )
}

/// The box `ctm` takes the unit square to: left, top, right, bottom.
fn bounds(ctm: [f64; 6], height: f64) -> [f64; 4] {
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|c| point(ctm, c, height));
    let xs = corners.map(|c| c.0);
    let ys = corners.map(|c| c.1);
    [
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn assert_near(found: [f64; 4], want: [f64; 4], what: &str) {
    let close = found.iter().zip(want).all(|(f, w)| (f - w).abs() < 0.01);
    assert!(close, "{what}: drawn at {found:?}, expected {want:?}");
}

fn painted(document: &Document) -> Pdf {
    Pdf::parse(&paint_for_tests(document, &FontSet::default(), &liberation()).unwrap()).unwrap()
}

fn rendered(document: &Document) -> (Pdf, docrs::pdf::PdfReport) {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let (bytes, report) = render(document, &options).unwrap();
    (Pdf::parse(&bytes).unwrap(), report)
}

/// The body's lines, as layout records them.
fn lines(document: &Document) -> Vec<LineRecord> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name().to_string();
    let mut out = Vec::new();
    records_with(
        &txn,
        &main,
        &fonts(),
        |_, _| Ok(()),
        |record| {
            if let Record::Line(line) = record {
                out.push(line);
            }
            Ok(())
        },
    )
    .unwrap();
    out
}

const PAGE_HEIGHT: f64 = 792.0;

#[test]
fn an_inline_picture_sits_on_its_line_baseline_at_its_extent() {
    let package = body(
        &(para(&inline(120.0, 40.0, "", ""))
            + &para(&(text("Before ") + &inline(30.0, 20.0, "", "") + &text(" after")))),
    );
    let document = package.document();
    let (pdf, report) = rendered(&document);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let lines = lines(&document);
    let drawn = images(&marks(&pdf)[0]);
    assert_eq!(drawn.len(), 2);
    let first = &lines[0];
    assert_near(
        bounds(drawn[0].1, PAGE_HEIGHT),
        [
            first.x,
            first.baseline - 40.0,
            first.x + 120.0,
            first.baseline,
        ],
        "alone on its line",
    );
    let second = &lines[1];
    let origins = &second.runs[0].origins;
    // "Before " then the picture: the space after it starts 30 pt on.
    let x = origins[6] + (origins[8] - origins[7]);
    assert!((origins[7] - x - 30.0).abs() < 0.01, "{second:?}");
    assert_near(
        bounds(drawn[1].1, PAGE_HEIGHT),
        [x, second.baseline - 20.0, x + 30.0, second.baseline],
        "in the text",
    );
    assert!(
        drawn
            .iter()
            .all(|(name, _, clipped)| name == "Im1" && !clipped)
    );
}

#[test]
fn an_inline_box_holds_the_effect_extent_around_the_picture() {
    let drawing = r#"<w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><wp:effectExtent l="127000" t="254000" r="381000" b="508000"/><wp:docPr id="1" name="p"/>PIC</wp:inline></w:drawing></w:r>"#
        .replace("PIC", &pic("rIdM1", 100.0, 50.0, "", ""));
    let document = body(&para(&drawing)).document();
    let line = &lines(&document)[0];
    assert!(
        (line.height - (50.0 + 20.0 + 40.0)).abs() < 0.01,
        "{line:?}"
    );
    let drawn = images(&marks(&painted(&document))[0]);
    assert_near(
        bounds(drawn[0].1, PAGE_HEIGHT),
        [
            line.x + 10.0,
            line.baseline - 40.0 - 50.0,
            line.x + 110.0,
            line.baseline - 40.0,
        ],
        "inside its effect extent",
    );
}

#[test]
fn crop_rotation_and_flips_follow_the_picture_properties() {
    let crop = r#"<a:srcRect l="25000" t="10000" r="25000" b="30000"/>"#;
    let document = body(
        &(para(&inline(100.0, 60.0, crop, ""))
            + &para(&inline(100.0, 50.0, "", r#"rot="5400000""#))
            + &para(&inline(100.0, 50.0, "", r#"flipH="1""#))
            + &para(&inline(100.0, 50.0, "", r#"flipV="1" rot="10800000""#))),
    )
    .document();
    let lines = lines(&document);
    let drawn = images(&marks(&painted(&document))[0]);
    let boxes: Vec<[f64; 4]> = lines
        .iter()
        .map(|l| [l.x, l.baseline - 50.0, l.x + 100.0, l.baseline])
        .collect();

    let (_, cropped, clipped) = &drawn[0];
    assert!(clipped, "a crop clips to the extent");
    let line = &lines[0];
    // The visible half of the width and 60% of the height fill 100 × 60.
    assert_near(
        bounds(*cropped, PAGE_HEIGHT),
        [
            line.x - 50.0,
            line.baseline - 60.0 - 10.0,
            line.x + 150.0,
            line.baseline + 30.0,
        ],
        "cropped",
    );

    let [left, top, right, bottom] = boxes[1];
    let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
    let (_, rotated, clipped) = &drawn[1];
    assert!(!clipped);
    assert_near(
        bounds(*rotated, PAGE_HEIGHT),
        [cx - 25.0, cy - 50.0, cx + 25.0, cy + 50.0],
        "rotated a quarter turn",
    );
    let top_left = point(*rotated, (0.0, 1.0), PAGE_HEIGHT);
    assert_near(
        [top_left.0, top_left.1, 0.0, 0.0],
        [cx + 25.0, cy - 50.0, 0.0, 0.0],
        "clockwise: the image's top-left corner goes to the top right",
    );

    let flipped = point(drawn[2].1, (0.0, 1.0), PAGE_HEIGHT);
    assert_near(
        [flipped.0, flipped.1, 0.0, 0.0],
        [boxes[2][2], boxes[2][1], 0.0, 0.0],
        "flipped horizontally",
    );
    // A vertical flip turned half round is a horizontal flip.
    let turned = point(drawn[3].1, (0.0, 1.0), PAGE_HEIGHT);
    assert_near(
        [turned.0, turned.1, 0.0, 0.0],
        [boxes[3][2], boxes[3][1], 0.0, 0.0],
        "flipped vertically and turned half round",
    );
}

#[test]
fn anchored_pictures_are_placed_against_each_frame() {
    // Letter 612 × 792; margins left 90, right 72, top 72, bottom 54.
    let (w, h) = (100.0, 50.0);
    let cases: Vec<(&str, String, String, [f64; 2])> = vec![
        (
            "page offsets",
            offset("H", "page", 36.0),
            offset("V", "page", 100.0),
            [36.0, 100.0],
        ),
        (
            "margin right, bottom",
            aligned("H", "margin", "right"),
            aligned("V", "margin", "bottom"),
            [440.0, 688.0],
        ),
        (
            "column centre, top margin centre",
            aligned("H", "column", "center"),
            aligned("V", "topMargin", "center"),
            [265.0, 11.0],
        ),
        (
            "left margin centre, bottom margin top",
            aligned("H", "leftMargin", "center"),
            aligned("V", "bottomMargin", "top"),
            [-5.0, 738.0],
        ),
        (
            "right margin left, page bottom",
            aligned("H", "rightMargin", "left"),
            aligned("V", "page", "bottom"),
            [540.0, 742.0],
        ),
        (
            "inside and outside margins on an odd page",
            aligned("H", "insideMargin", "left"),
            offset("V", "outsideMargin", 2.0),
            [0.0, 740.0],
        ),
        (
            "outside margin, inside alignment on the page",
            offset("H", "outsideMargin", 10.0),
            aligned("V", "page", "inside"),
            [550.0, 0.0],
        ),
        (
            "half the page across",
            r#"<wp:positionH relativeFrom="page"><mc:AlternateContent><mc:Choice Requires="wp14"><wp14:pctPosHOffset>50000</wp14:pctPosHOffset></mc:Choice><mc:Fallback><wp:posOffset>0</wp:posOffset></mc:Fallback></mc:AlternateContent></wp:positionH>"#.to_string(),
            aligned("V", "page", "outside"),
            [306.0, 742.0],
        ),
    ];
    for (what, x, y, [left, top]) in cases {
        let document = body(&para(&anchor(w, h, r#"relativeHeight="1""#, &x, &y))).document();
        let drawn = images(&marks(&painted(&document))[0]);
        assert_eq!(drawn.len(), 1, "{what}");
        assert_near(
            bounds(drawn[0].1, PAGE_HEIGHT),
            [left, top, left + w, top + h],
            what,
        );
    }
}

#[test]
fn anchored_pictures_follow_their_paragraph_line_and_character() {
    let document = body(
        &(para(&text("First paragraph."))
            + &para(
                &(text("Second, ")
                    + &anchor(
                        40.0,
                        20.0,
                        r#"relativeHeight="1""#,
                        &offset("H", "character", 0.0),
                        &offset("V", "paragraph", 30.0),
                    )
                    + &text("anchored.")
                    + &anchor(
                        40.0,
                        20.0,
                        r#"relativeHeight="2""#,
                        &offset("H", "margin", 0.0),
                        &aligned("V", "line", "bottom"),
                    )),
            )),
    )
    .document();
    let lines = lines(&document);
    let second = &lines[1];
    let drawn = images(&marks(&painted(&document))[0]);
    let comma = &second.runs[0];
    let character = comma.origins[comma.text.chars().count() - 1] + 3.0;
    let at = drawn[0].1[4];
    assert!(
        at > comma.origins[0] && at < character + 5.0,
        "{at} {second:?}"
    );
    assert_near(
        bounds(drawn[0].1, PAGE_HEIGHT),
        [at, second.y + 30.0, at + 40.0, second.y + 50.0],
        "the anchor's character, below its paragraph's top",
    );
    let bottom = second.y + second.height;
    assert_near(
        bounds(drawn[1].1, PAGE_HEIGHT),
        [90.0, bottom - 20.0, 130.0, bottom],
        "the margin, on its line's bottom",
    );
}

#[test]
fn an_anchor_is_drawn_with_the_line_that_holds_it() {
    let words = "Many words fill the lines of this paragraph. ".repeat(160);
    let document = body(
        &(para(
            &(text(&words)
                + &anchor(
                    40.0,
                    20.0,
                    r#"relativeHeight="1""#,
                    &offset("H", "margin", 0.0),
                    &offset("V", "line", 0.0),
                )),
        )),
    )
    .document();
    let lines = lines(&document);
    let last = lines.last().unwrap();
    assert!(last.page > 1, "the paragraph crosses a page");
    let marks = marks(&painted(&document));
    let pages: Vec<usize> = marks.iter().map(|page| images(page).len()).collect();
    assert_eq!(pages.iter().sum::<usize>(), 1, "{pages:?}");
    let page = last.page as usize - 1;
    assert_near(
        bounds(images(&marks[page])[0].1, PAGE_HEIGHT),
        [90.0, last.y, 130.0, last.y + 20.0],
        "on the last line",
    );
}

#[test]
fn layers_put_behind_text_under_it_and_order_by_relative_height() {
    let place = |points: f64| (offset("H", "page", points), offset("V", "page", points));
    let picture = |points: f64, attributes: &str| {
        let (x, y) = place(points);
        anchor(20.0, 20.0, attributes, &x, &y)
    };
    let header = para(
        &(picture(1.0, r#"behindDoc="1" relativeHeight="9""#)
            + &text("Header")
            + &picture(2.0, r#"relativeHeight="8""#)),
    );
    let body_content = para(
        &(picture(3.0, r#"relativeHeight="5""#)
            + &picture(4.0, r#"behindDoc="1" relativeHeight="7""#)
            + &text("Body")
            + &picture(5.0, r#"relativeHeight="3""#)
            + &picture(6.0, r#"behindDoc="1" relativeHeight="6""#)),
    );
    let package = Package {
        body: format!(
            r#"{body_content}<w:sectPr><w:headerReference w:type="default" r:id="rIdS1"/>{SECTION}</w:sectPr>"#
        ),
        stories: vec![("header1.xml", "hdr", header)],
        media: vec![("logo.png", logo())],
    };
    let marks = &marks(&painted(&package.document()))[0];
    let order: Vec<String> = marks
        .iter()
        .map(|mark| match mark {
            Mark::Image { ctm, .. } => format!("{}", ctm[4]),
            Mark::Text => "text".to_string(),
            Mark::Placeholder(_) => "placeholder".to_string(),
            Mark::Fill => "fill".to_string(),
        })
        .collect();
    assert_eq!(
        order,
        ["1", "text", "2", "6", "4", "text", "5", "3"],
        "the header's layer, then the body's, each behind, text, in front"
    );
}

#[test]
fn a_footnote_picture_reads_its_own_part_s_relationships() {
    let reference = r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteReference w:id="1"/></w:r>"#;
    let note = format!(
        r#"<w:footnote w:id="1"><w:p>{}{}</w:p></w:footnote>"#,
        text("Note "),
        inline_of("rIdF1", 30.0, 15.0, "", "")
    );
    let package = Package {
        body: format!(
            r#"{}<w:sectPr><w:footnoteReference w:type="separator" w:id="-1"/>{SECTION}</w:sectPr>"#,
            para(&(text("Body") + reference))
        ),
        stories: vec![("footnotes.xml", "footnotes", note)],
        media: vec![("logo.png", logo())],
    };
    let document = package.document();
    let txn = Txn::new(&document);
    let main = document.package().main_part_name().to_string();
    let mut notes = Vec::new();
    records_with(
        &txn,
        &main,
        &fonts(),
        |_, _| Ok(()),
        |record| {
            if let Record::Line(line) = record
                && line.story == "footnotes"
            {
                notes.push(line);
            }
            Ok(())
        },
    )
    .unwrap();
    let line = notes.last().expect("the note's line");
    let marks = &marks(&painted(&document))[0];
    let drawn = images(marks);
    assert_eq!(drawn.len(), 1, "{marks:?}");
    assert!(!marks.iter().any(|m| matches!(m, Mark::Placeholder(_))));
    let found = bounds(drawn[0].1, PAGE_HEIGHT);
    let baseline = line.baseline;
    assert_near(
        [found[1], found[3], 0.0, 0.0],
        [baseline - 15.0, baseline, 0.0, 0.0],
        "on the note's baseline",
    );
}

#[test]
fn a_picture_in_a_centred_cell_sits_on_its_line_there() {
    let cell = |props: &str, content: &str| {
        format!(r#"<w:tc><w:tcPr><w:tcW w:w="4000" w:type="dxa"/>{props}</w:tcPr>{content}</w:tc>"#)
    };
    let tall: String = (0..6).map(|i| para(&text(&format!("Line {i}")))).collect();
    let table = format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="8000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="4000"/><w:gridCol w:w="4000"/></w:tblGrid><w:tr>{}{}</w:tr></w:tbl>"#,
        cell("", &tall),
        cell(
            r#"<w:vAlign w:val="center"/>"#,
            &para(&inline(30.0, 15.0, "", ""))
        ),
    );
    let document = body(&(table + &para(&text("After")))).document();
    let lines = lines(&document);
    let picture = lines
        .iter()
        .find(|l| l.text.is_empty() && l.height >= 15.0)
        .expect("the picture's line");
    let first = &lines[0];
    assert!(
        picture.y > first.y + 20.0,
        "centred in its row: {} vs {}",
        picture.y,
        first.y
    );
    let drawn = images(&marks(&painted(&document))[0]);
    assert_eq!(drawn.len(), 1);
    let found = bounds(drawn[0].1, PAGE_HEIGHT);
    let baseline = picture.baseline;
    assert_near(
        found,
        [picture.x, baseline - 15.0, picture.x + 30.0, baseline],
        "on its cell line's baseline",
    );
}

#[test]
fn a_shaded_box_covers_a_behind_text_picture_and_text_covers_the_box() {
    let shaded = r#"<w:pPr><w:pBdr><w:top w:val="single" w:sz="8" w:space="1" w:color="000000"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="DDDDDD"/></w:pPr>"#;
    let behind = anchor(
        20.0,
        20.0,
        r#"behindDoc="1" relativeHeight="1""#,
        &offset("H", "margin", 0.0),
        &offset("V", "paragraph", 0.0),
    );
    let front = anchor(
        20.0,
        20.0,
        r#"relativeHeight="2""#,
        &offset("H", "margin", 40.0),
        &offset("V", "paragraph", 0.0),
    );
    let document = body(&para(&format!("{shaded}{}{behind}{front}", text("Boxed")))).document();
    let order: Vec<&str> = marks(&painted(&document))[0]
        .iter()
        .map(|mark| match mark {
            Mark::Image { .. } => "image",
            Mark::Text => "text",
            Mark::Fill => "fill",
            Mark::Placeholder(_) => "placeholder",
        })
        .collect();
    let first_fill = order.iter().position(|m| *m == "fill").unwrap();
    let text_at = order.iter().position(|m| *m == "text").unwrap();
    assert_eq!(order[0], "image", "behind the box: {order:?}");
    assert!(
        first_fill < text_at,
        "the shading under the text: {order:?}"
    );
    assert_eq!(order.last(), Some(&"image"), "in front of all: {order:?}");
}

#[test]
fn a_paragraph_anchor_follows_its_paragraph_down_a_centred_page() {
    let picture = anchor(
        20.0,
        20.0,
        r#"relativeHeight="1""#,
        &offset("H", "margin", 0.0),
        &offset("V", "paragraph", 10.0),
    );
    let page_picture = anchor(
        20.0,
        20.0,
        r#"relativeHeight="2""#,
        &offset("H", "page", 36.0),
        &offset("V", "page", 36.0),
    );
    let document = Package {
        body: format!(
            r#"{}<w:sectPr>{SECTION}<w:vAlign w:val="center"/></w:sectPr>"#,
            para(&(text("Centred") + &picture + &page_picture))
        ),
        stories: Vec::new(),
        media: vec![("logo.png", logo())],
    }
    .document();
    let line = &lines(&document)[0];
    assert!(line.y > 300.0, "the section centres its text: {}", line.y);
    let drawn = images(&marks(&painted(&document))[0]);
    assert_near(
        bounds(drawn[0].1, PAGE_HEIGHT),
        [90.0, line.y + 10.0, 110.0, line.y + 30.0],
        "below the centred paragraph's top",
    );
    assert_near(
        bounds(drawn[1].1, PAGE_HEIGHT),
        [36.0, 36.0, 56.0, 56.0],
        "the page's own frame",
    );
}

#[test]
fn a_repeated_logo_is_one_xobject() {
    let header = para(
        &(inline(72.0, 24.0, "", "")
            + &anchor(
                144.0,
                48.0,
                r#"relativeHeight="2""#,
                &offset("H", "page", 36.0),
                &offset("V", "page", 18.0),
            )),
    );
    let words = para(&text(&"Words that fill pages. ".repeat(600)));
    let package = Package {
        body: format!(
            r#"{words}{}<w:sectPr><w:headerReference w:type="default" r:id="rIdS1"/>{SECTION}</w:sectPr>"#,
            para(&inline(50.0, 50.0, "", ""))
        ),
        stories: vec![("header1.xml", "hdr", header)],
        media: vec![("logo.png", logo())],
    };
    let bytes = paint_for_tests(&package.document(), &FontSet::default(), &liberation()).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    let marks = marks(&pdf);
    assert!(marks.len() >= 3, "{} pages", marks.len());
    for page in &marks {
        let names: Vec<String> = images(page).into_iter().map(|(name, ..)| name).collect();
        assert!(
            names.len() >= 2 && names.iter().all(|n| n == "Im1"),
            "{names:?}"
        );
    }
    let images = pdf
        .objects
        .values()
        .filter(|value| {
            matches!(value, Value::Stream(dict, _) if dict.get("Subtype") == Some(&Value::Name("Image".into())))
        })
        .count();
    assert_eq!(images, 2, "the logo and its soft mask, once");
}

#[test]
fn pictures_are_held_to_the_pixel_budgets() {
    let document = body(&(para(&inline(40.0, 20.0, "", "")))).document();
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.limits = PdfLimits::default();
    options.limits.max_image_pixels = 100;
    let err = render(&document, &options).unwrap_err();
    assert_eq!(err.reason(), "max_image_pixels", "{err}");
    options.limits = PdfLimits::default();
    options.limits.max_document_pixels = 100;
    let err = render(&document, &options).unwrap_err();
    assert_eq!(err.reason(), "max_document_pixels", "{err}");
}

#[test]
fn what_a_picture_cannot_draw_is_a_placeholder_with_a_warning() {
    let mut package = body(
        &(para(&inline_of("rIdM2", 40.0, 20.0, "", ""))
            + &para(&inline_of("rIdM3", 40.0, 20.0, "", ""))
            + &para(&inline_of("rIdM4", 40.0, 20.0, "", ""))
            + &para(&inline_of("rIdM5", 40.0, 20.0, "", ""))
            + &para(&inline_of("rIdMissing", 40.0, 20.0, "", ""))),
    );
    package.media.extend([
        ("scan.tif", b"II*\0rest of a TIFF".to_vec()),
        ("broken.png", b"\x89PNG\r\n\x1A\ntruncated".to_vec()),
        ("animated.gif", read("pdf/images/animated.gif")),
        (
            "drawing.svg",
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
        ),
    ]);
    let document = package.document();
    let (pdf, report) = rendered(&document);
    let marks = &marks(&pdf)[0];
    let placeholders = marks
        .iter()
        .filter(|m| matches!(m, Mark::Placeholder(_)))
        .count();
    assert_eq!(placeholders, 4, "{marks:?}");
    assert_eq!(images(marks).len(), 1, "the GIF's first frame");
    let mut warnings: Vec<(&str, &str, usize)> = report
        .warnings
        .iter()
        .map(|w| (w.code, w.subject, w.count))
        .collect();
    warnings.sort();
    assert_eq!(
        warnings,
        [
            ("pdf_image_animated", "image", 1),
            ("pdf_image_unsupported", "TIFF image", 1),
            ("pdf_image_unsupported", "image", 1),
            ("pdf_image_unsupported", "missing image", 1),
            ("pdf_image_unsupported", "unreadable image", 1),
        ]
    );
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.strict = true;
    let only_drawing_finds = body(&para(&inline_of("rIdM2", 40.0, 20.0, "", "")));
    let mut only_drawing_finds = only_drawing_finds;
    only_drawing_finds
        .media
        .push(("animated.gif", read("pdf/images/animated.gif")));
    let document = only_drawing_finds.document();
    assert!(preflight(&document).unwrap().warnings.is_empty());
    let err = render(&document, &options).unwrap_err();
    assert_eq!(err.reason(), "pdf_not_faithful", "{err}");
}

#[test]
fn a_chart_is_drawn_from_its_picture_fallback() {
    let chart = format!(
        r#"<w:r><mc:AlternateContent><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1"><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><wp:docPr id="1" name="Chart"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"/></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><wp:docPr id="2" name="Chart picture"/>{}</wp:inline></w:drawing></mc:Fallback></mc:AlternateContent></w:r>"#,
        pic("rIdM1", 100.0, 50.0, "", "")
    );
    let document = body(&para(&chart)).document();
    let (pdf, report) = rendered(&document);
    let codes: Vec<(&str, &str)> = report
        .warnings
        .iter()
        .map(|w| (w.code, w.subject))
        .collect();
    assert_eq!(codes, [("pdf_drawing_fallback_used", "chart")]);
    let line = &lines(&document)[0];
    assert_near(
        bounds(images(&marks(&pdf)[0])[0].1, PAGE_HEIGHT),
        [line.x, line.baseline - 50.0, line.x + 100.0, line.baseline],
        "the fallback picture",
    );
}

/// `source` converted into the bundled blank document, its images from
/// `examples/markdown`.
fn convert(source: &str, format: InputFormat) -> Document {
    let mut document = Document::new();
    let images: BTreeMap<String, Vec<u8>> = ["images/chart.png", "images/banner.jpg"]
        .into_iter()
        .map(|name| {
            let path = fixtures().join("../../examples/markdown").join(name);
            (name.to_string(), std::fs::read(path).unwrap())
        })
        .collect();
    let options = MarkdownOptions {
        now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
        images,
        ..MarkdownOptions::default()
    };
    insert_source_with(&mut document, source, format, &options, &fonts()).unwrap();
    document
}

#[test]
fn converted_markdown_and_html_pictures_are_drawn_on_their_lines() {
    let markdown = convert(
        "![Revenue](images/chart.png)\n\nA line with ![tiny](images/chart.png) inline.\n\n![Banner](images/banner.jpg)\n",
        InputFormat::Markdown,
    );
    let html = convert(
        r#"<p><img src="images/banner.jpg" alt="Banner"></p><p>Text then <img src="images/chart.png" alt="Chart"></p>"#,
        InputFormat::Html,
    );
    for (name, document, count) in [("markdown", markdown, 3), ("html", html, 2)] {
        let pdf = painted(&document);
        let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
        let drawn: Vec<(usize, [f64; 4])> = marks(&pdf)
            .iter()
            .enumerate()
            .flat_map(|(page, marks)| {
                images(marks)
                    .into_iter()
                    .map(move |(_, ctm, _)| (page, ctm))
                    .collect::<Vec<_>>()
            })
            .map(|(page, ctm)| (page, bounds(ctm, heights[page])))
            .collect();
        assert_eq!(drawn.len(), count, "{name}");
        let lines = lines(&document);
        for (page, [left, top, right, bottom]) in drawn {
            let line = lines
                .iter()
                .find(|l| {
                    l.page as usize == page + 1
                        && (l.baseline - bottom).abs() < 0.01
                        && left >= l.x - 0.01
                        && right <= l.x + l.width + 0.01
                })
                .unwrap_or_else(|| panic!("{name}: no line holds {left} {top} {right} {bottom}"));
            assert!(top >= line.y - 0.01, "{name}: {top} above {line:?}");
        }
    }
}

fn open(path: &str) -> Document {
    Document::open_path_with(fixtures().join(path), &Limits::default(), Tolerance::word())
        .unwrap()
        .0
}

/// `source` converted into the template at `path`.
fn into_template(path: &str, source: &str) -> Document {
    let mut document = open(path);
    let options = MarkdownOptions {
        now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
        ..MarkdownOptions::default()
    };
    insert_source_with(
        &mut document,
        source,
        InputFormat::Markdown,
        &options,
        &fonts(),
    )
    .unwrap();
    document
}

/// Each page's images, as boxes from the top-left corner.
fn page_images(pdf: &Pdf) -> Vec<Vec<(String, [f64; 4])>> {
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    marks(pdf)
        .iter()
        .zip(heights)
        .map(|(marks, height)| {
            images(marks)
                .into_iter()
                .map(|(name, ctm, _)| (name, bounds(ctm, height)))
                .collect()
        })
        .collect()
}

#[test]
fn the_letterhead_draws_its_first_page_logo_where_anchored_and_later_ones_in_the_header() {
    let memo = std::fs::read_to_string(fixtures().join("pdf/sources/memo.md")).unwrap();
    let document = into_template("markdown/templates/v2/letterhead-header-footer.docx", &memo);
    let pdf = painted(&document);
    let pages = page_images(&pdf);
    assert!(pages.len() >= 2, "{} pages", pages.len());
    // The first-page header anchors a 6 × 2 cm logo 1.5 cm from the
    // page's left edge and 1 cm from its top.
    let [(name, first)] = &pages[0][..] else {
        panic!("{:?}", pages[0]);
    };
    assert_near(
        *first,
        [42.52, 28.35, 42.52 + 170.08, 28.35 + 56.69],
        "first page",
    );
    let header = lines_of(&document, "header1");
    for page in &pages[1..] {
        let [(other, at)] = &page[..] else {
            panic!("{page:?}");
        };
        assert_eq!(other, name, "one XObject for both headers' logo");
        let line = header
            .iter()
            .find(|l| (l.baseline - at[3]).abs() < 0.01)
            .unwrap_or_else(|| panic!("no header line under {at:?}"));
        assert_near(
            *at,
            [line.x, line.baseline - 37.8, line.x + 113.39, line.baseline],
            "the header cell's inline logo",
        );
    }
}

/// The lines of story `story` on page 2, as layout records them.
fn lines_of(document: &Document, story: &str) -> Vec<LineRecord> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name().to_string();
    let mut out = Vec::new();
    records_with(
        &txn,
        &main,
        &fonts(),
        |_, _| Ok(()),
        |record| {
            if let Record::Line(line) = record
                && line.story == story
                && line.page == 2
            {
                out.push(line);
            }
            Ok(())
        },
    )
    .unwrap();
    out
}

#[test]
fn the_cover_page_draws_its_logo_centred_on_its_line() {
    let document = open("markdown/templates/v2/cover-page-sections.docx");
    let pages = page_images(&painted(&document));
    let [(_, logo)] = &pages[0][..] else {
        panic!("{pages:?}");
    };
    let line = lines(&document)
        .into_iter()
        .find(|l| l.page == 1 && (l.baseline - logo[3]).abs() < 0.01)
        .unwrap();
    // 8 × 2.67 cm, centred between A4's 2.54 cm margins.
    let left = (595.3 - 226.77) / 2.0;
    assert_near(
        *logo,
        [left, line.baseline - 75.59, left + 226.77, line.baseline],
        "centred on the cover",
    );
    assert!(pages[1..].iter().all(Vec::is_empty), "{pages:?}");
}

#[test]
fn a_title_page_background_covers_the_first_page_behind_its_text() {
    let background = para(&anchor(
        632.0,
        812.0,
        r#"behindDoc="1" relativeHeight="1""#,
        &offset("H", "page", -10.0),
        &offset("V", "page", -10.0),
    ));
    let words = para(&text(&"Words that fill pages. ".repeat(300)));
    let package = Package {
        body: format!(
            r#"{words}<w:sectPr><w:headerReference w:type="first" r:id="rIdS1"/><w:titlePg/>{SECTION}</w:sectPr>"#
        ),
        stories: vec![("header1.xml", "hdr", background)],
        media: vec![("logo.png", logo())],
    };
    let pdf = painted(&package.document());
    let marks = marks(&pdf);
    assert!(marks.len() >= 2);
    assert!(matches!(marks[0][0], Mark::Image { .. }), "{:?}", marks[0]);
    assert_near(
        bounds(images(&marks[0])[0].1, PAGE_HEIGHT),
        [-10.0, -10.0, 622.0, 802.0],
        "past the MediaBox, which clips it",
    );
    assert!(marks[1..].iter().all(|page| images(page).is_empty()));
}

#[test]
fn an_oversized_footer_picture_is_drawn_past_the_page_edge() {
    let document = open("markdown/templates/oversized-footer-picture.docx");
    let pages = page_images(&painted(&document));
    let [(_, logo), (_, large)] = &pages[0][..] else {
        panic!("{:?}", pages[0]);
    };
    assert!(logo[2] < 595.3);
    let width = 13_442_950.0 / EMU;
    assert!(
        (large[2] - large[0] - width).abs() < 0.01 && large[2] > 595.3,
        "{large:?}"
    );
}
