//! The PDF preflight scan: one generated document per warning code.

mod support;

use docrs::markdown::{MarkdownOptions, insert_markdown};
use docrs::pdf::{
    FOOTNOTE_LAYOUT_UNSUPPORTED, NOT_YET_DRAWN, PdfReport, WARNING_CODES, preflight,
    preflight_work_for_tests,
};
use docrs::{Document, Limits, RefusalKind};
use support::{ROOT_RELS, RawMember, build_zip, png_150dpi, snapshot};

const NAMESPACES: &str = concat!(
    r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
    r#"xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" "#,
    r#"xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" "#,
    r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
    r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" "#,
    r#"xmlns:v="urn:schemas-microsoft-com:vml" "#,
    r#"xmlns:o="urn:schemas-microsoft-com:office:office""#,
);
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PICTURE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const SHAPE_URI: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";

/// An extra part: name, content type, bytes and the relationship from the
/// main part (id, type, target).
struct Part {
    name: &'static str,
    content_type: &'static str,
    data: Vec<u8>,
    rel: (&'static str, &'static str),
}

fn xml_part(name: &'static str, kind: &'static str, root: &str, content: &str) -> Part {
    Part {
        name,
        content_type: match kind {
            "header" => "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
            "footnotes" => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"
            }
            "settings" => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"
            }
            "styles" => "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
            "numbering" => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"
            }
            _ => "application/xml",
        },
        data: format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} {NAMESPACES}>{content}</w:{root}>"#)
            .into_bytes(),
        rel: (kind, name.trim_start_matches("word/")),
    }
}

fn header(name: &'static str, content: &str) -> Part {
    xml_part(name, "header", "hdr", content)
}

fn image_part(name: &'static str, content_type: &'static str) -> Part {
    Part {
        name,
        content_type,
        data: png_150dpi(),
        rel: ("image", name.trim_start_matches("word/")),
    }
}

/// A package whose body is `body`; each extra part relates as `rId<n>`
/// from 1, and `links` are external hyperlink relationships `(id, target)`.
fn docx(body: &str, parts: &[Part], links: &[(&str, &str)]) -> Document {
    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (index, part) in parts.iter().enumerate() {
        types.push_str(&format!(
            r#"<Override PartName="/{}" ContentType="{}"/>"#,
            part.name, part.content_type
        ));
        rels.push_str(&format!(
            r#"<Relationship Id="rId{}" Type="{REL}/{}" Target="{}"/>"#,
            index + 1,
            part.rel.0,
            part.rel.1
        ));
    }
    for (id, target) in links {
        rels.push_str(&format!(
            r#"<Relationship Id="{id}" Type="{REL}/hyperlink" Target="{target}" TargetMode="External"/>"#
        ));
    }
    types.push_str("</Types>");
    rels.push_str("</Relationships>");
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NAMESPACES}><w:body>{body}</w:body></w:document>"#
    );
    let mut members = vec![
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
    ];
    for part in parts {
        members.push(RawMember::new(part.name, &part.data));
    }
    Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
}

fn para(content: &str) -> String {
    format!("<w:p>{content}</w:p>")
}

fn run(content: &str) -> String {
    format!("<w:r>{content}</w:r>")
}

fn graphic(uri: &str, inner: &str) -> String {
    run(&drawing(uri, inner))
}

fn drawing(uri: &str, inner: &str) -> String {
    format!(
        r#"<w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Graphic"/><a:graphic><a:graphicData uri="{uri}">{inner}</a:graphicData></a:graphic></wp:inline></w:drawing>"#
    )
}

fn picture(rel: &str) -> String {
    format!(r#"<pic:pic><pic:blipFill><a:blip r:embed="{rel}"/></pic:blipFill></pic:pic>"#)
}

/// A DrawingML shape in Word's `mc:AlternateContent`, with a VML fallback
/// that must not count again.
fn alternate_shape(inner: &str) -> String {
    run(&format!(
        r#"<mc:AlternateContent><mc:Choice Requires="wps">{}</mc:Choice><mc:Fallback><w:pict><v:rect style="width:10pt;height:10pt"/></w:pict></mc:Fallback></mc:AlternateContent>"#,
        graphic(SHAPE_URI, &format!("<wps:wsp>{inner}</wps:wsp>"))
    ))
}

fn bordered(text: &str) -> String {
    para(&run(&format!(
        r#"<w:rPr><w:bdr w:val="single" w:sz="4"/></w:rPr><w:t>{text}</w:t>"#
    )))
}

fn section(content: &str) -> String {
    format!("<w:sectPr>{content}</w:sectPr>")
}

/// A document showing `code` exactly `count` times under `subject`, and no
/// other warning besides constructs not drawn yet.
struct Case {
    code: &'static str,
    subject: &'static str,
    count: usize,
    document: Document,
    /// Document-wide settings have no location.
    located: bool,
}

fn case(code: &'static str, subject: &'static str, count: usize, document: Document) -> Case {
    Case {
        code,
        subject,
        count,
        document,
        located: true,
    }
}

impl Case {
    fn unlocated(self) -> Case {
        Case {
            located: false,
            ..self
        }
    }
}

fn cases() -> Vec<Case> {
    let text = |t: &str| para(&run(&format!("<w:t>{t}</w:t>")));
    vec![
        case(
            NOT_YET_DRAWN,
            "border",
            2,
            docx(
                &(bordered("One") + &bordered("Two") + &text("Three") + &para("")),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_justification_unsupported",
            "distributed justification",
            1,
            docx(
                &para(
                    &(r#"<w:pPr><w:jc w:val="distribute"/></w:pPr>"#.to_string()
                        + &run("<w:t>spread</w:t>")),
                ),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_decoration_approximated",
            "paragraph border",
            2,
            docx(
                &["wave", "threeDEmboss", "single"]
                    .map(|style| {
                        para(&format!(
                            r#"<w:pPr><w:pBdr><w:left w:val="{style}" w:sz="4"/></w:pBdr></w:pPr>{}"#,
                            run("<w:t>boxed</w:t>")
                        ))
                    })
                    .concat(),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_chart_skipped",
            "chart",
            1,
            docx(
                &para(&graphic(
                    "http://schemas.openxmlformats.org/drawingml/2006/chart",
                    "<c/>",
                )),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_smartart_skipped",
            "SmartArt",
            1,
            docx(
                &para(&graphic(
                    "http://schemas.openxmlformats.org/drawingml/2006/diagram",
                    "<d/>",
                )),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_object_skipped",
            "OLE object",
            2,
            docx(
                &para(&(run("<w:object/>") + &run("<w:object><v:shape/></w:object>"))),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_shape_skipped",
            "shape",
            2,
            docx(
                &para(
                    &(alternate_shape("<wps:spPr/>")
                        + &run(r#"<w:pict><v:oval style="width:5pt;height:5pt"/></w:pict>"#)
                        + &run(
                            r#"<w:pict><v:rect o:hr="t" style="width:0;height:1.5pt"/></w:pict>"#,
                        )),
                ),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_text_box_skipped",
            "text box",
            1,
            docx(
                &para(&alternate_shape(&format!(
                    "<wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx>",
                    text("Boxed words")
                ))),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_watermark_skipped",
            "watermark",
            1,
            docx(
                &(text("Body")
                    + &section(
                        r#"<w:headerReference w:type="default" r:id="rId1"/><w:headerReference w:type="first" r:id="rId2"/>"#,
                    )),
                &[
                    header(
                        "word/header1.xml",
                        &para(&run(
                            r#"<w:pict><v:shape id="PowerPlusWaterMarkObject1" style="width:400pt;height:100pt"><v:textpath string="DRAFT"/></v:shape></w:pict>"#,
                        )),
                    ),
                    // A first-page header without w:titlePg is never shown.
                    header(
                        "word/header2.xml",
                        &para(&run(
                            r#"<w:pict><v:shape id="PowerPlusWaterMarkObject2" style="width:400pt;height:100pt"/></w:pict>"#,
                        )),
                    ),
                ],
                &[],
            ),
        ),
        case(
            "pdf_columns_ignored",
            "multi-column section",
            1,
            docx(
                &(text("Body") + &section(r#"<w:cols w:num="2"/>"#)),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_wrap_ignored",
            "floating object",
            1,
            docx(
                &para(&run(&format!(
                    r#"<w:drawing><wp:anchor><wp:extent cx="914400" cy="914400"/><wp:wrapSquare wrapText="bothSides"/><wp:docPr id="1" name="Picture 1"/><a:graphic><a:graphicData uri="{PICTURE_URI}">{}</a:graphicData></a:graphic></wp:anchor></w:drawing>"#,
                    picture("rId1")
                ))),
                &[image_part("word/media/image1.png", "image/png")],
                &[],
            ),
        ),
        case(
            "pdf_drawing_fallback_used",
            "chart",
            1,
            docx(
                &para(&run(&format!(
                    r#"<mc:AlternateContent><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
                    drawing(
                        "http://schemas.microsoft.com/office/drawing/2014/chartex",
                        "<cx/>"
                    ),
                    drawing(PICTURE_URI, &picture("rId1")),
                ))),
                &[image_part("word/media/image1.png", "image/png")],
                &[],
            ),
        ),
        case(
            "pdf_equation_linearized",
            "equation",
            2,
            docx(
                &(para("<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath>")
                    + &para(
                        "<m:oMathPara><m:oMath><m:r><m:t>y</m:t></m:r></m:oMath></m:oMathPara>",
                    )),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_image_unsupported",
            "EMF image",
            1,
            docx(
                &para(
                    &(graphic(PICTURE_URI, &picture("rId1"))
                        + &graphic(PICTURE_URI, &picture("rId2"))),
                ),
                &[
                    image_part("word/media/image1.emf", "image/x-emf"),
                    image_part("word/media/image2.png", "image/png"),
                ],
                &[],
            ),
        ),
        case(
            "pdf_text_unsupported",
            "CJK text",
            2,
            docx(
                &(text("日本語と漢字") + &text("Latin, then 中文") + &text("Plain")),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_layout_feature_ignored",
            "page borders",
            1,
            docx(
                &(text("Body")
                    + &section(r#"<w:pgBorders><w:top w:val="single" w:sz="4"/></w:pgBorders>"#)),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_revisions_flattened",
            "tracked change",
            2,
            docx(
                &para(
                    &(r#"<w:ins w:id="1" w:author="A">"#.to_string()
                        + &run("<w:t>new</w:t>")
                        + r#"</w:ins><w:del w:id="2" w:author="A">"#
                        + &run("<w:delText>old</w:delText>")
                        + "</w:del>"),
                ),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_field_cached",
            "DATE",
            2,
            docx(
                &para(
                    &(r#"<w:fldSimple w:instr=" DATE \@ &quot;d MMMM yyyy&quot; ">"#.to_string()
                        + &run("<w:t>1 May 2026</w:t>")
                        + "</w:fldSimple>"
                        + &run(r#"<w:fldChar w:fldCharType="begin"/>"#)
                        + &run("<w:instrText xml:space=\"preserve\"> DA</w:instrText>")
                        + &run("<w:instrText>TE </w:instrText>")
                        + &run(r#"<w:fldChar w:fldCharType="separate"/>"#)
                        + &run("<w:t>2 May 2026</w:t>")
                        + &run(r#"<w:fldChar w:fldCharType="end"/>"#)),
                ),
                &[],
                &[],
            ),
        ),
        case(
            "pdf_link_scheme_skipped",
            "link",
            2,
            docx(
                &para(
                    &(r#"<w:bookmarkStart w:id="0" w:name="intro"/><w:hyperlink r:id="rIdL1">"#
                        .to_string()
                        + &run("<w:t>run</w:t>")
                        + r#"</w:hyperlink><w:hyperlink r:id="rIdL2">"#
                        + &run("<w:t>file</w:t>")
                        + r#"</w:hyperlink><w:hyperlink r:id="rIdL3">"#
                        + &run("<w:t>web</w:t>")
                        + r#"</w:hyperlink><w:hyperlink w:anchor="intro">"#
                        + &run("<w:t>inside</w:t>")
                        + "</w:hyperlink>"),
                ),
                &[],
                &[
                    ("rIdL1", "javascript:alert(1)"),
                    ("rIdL2", "file:///etc/passwd"),
                    ("rIdL3", "https://example.com/"),
                ],
            ),
        ),
        case(
            FOOTNOTE_LAYOUT_UNSUPPORTED,
            "footnotes beneath the text",
            1,
            docx(
                &para(FOOTNOTE_REF),
                &[xml_part(
                    "word/settings.xml",
                    "settings",
                    "settings",
                    r#"<w:footnotePr><w:pos w:val="beneathText"/></w:footnotePr>"#,
                )],
                &[],
            ),
        )
        .unlocated(),
        case(
            "pdf_link_target_missing",
            "link",
            2,
            docx(
                &(para(
                    &(r#"<w:hyperlink w:anchor="Gone">"#.to_string()
                        + &run("<w:t>gone</w:t>")
                        + r#"</w:hyperlink><w:hyperlink w:anchor="INTRO">"#
                        + &run("<w:t>intro</w:t>")
                        + r#"</w:hyperlink><w:hyperlink w:anchor="_top" w:history="1">"#
                        + &run("<w:t>top</w:t>")
                        + "</w:hyperlink>"),
                ) + &para(
                    &(run(r#"<w:fldChar w:fldCharType="begin"/>"#)
                        + &run(r#"<w:instrText> HYPERLINK \l "Nowhere" </w:instrText>"#)
                        + &run(r#"<w:fldChar w:fldCharType="end"/>"#)),
                ) + &para(
                    r#"<w:bookmarkStart w:id="0" w:name="Intro"/><w:bookmarkEnd w:id="0"/>"#,
                )),
                &[],
                &[],
            ),
        ),
    ]
}

const FOOTNOTE_REF: &str = r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#;

fn report(document: &Document) -> PdfReport {
    preflight(document).unwrap()
}

#[test]
fn each_fixture_reports_its_code_and_count() {
    for case in cases() {
        let report = report(&case.document);
        let found: Vec<_> = report
            .warnings
            .iter()
            .filter(|w| w.code == case.code && w.subject == case.subject)
            .collect();
        assert_eq!(found.len(), 1, "{}: {report:#?}", case.code);
        assert_eq!(found[0].count, case.count, "{}: {report:#?}", case.code);
        let located = if case.located { case.count } else { 0 };
        assert_eq!(
            found[0].locations.len(),
            located.min(docrs::pdf::MAX_LOCATIONS)
        );
        let others: Vec<_> = report
            .warnings
            .iter()
            .filter(|w| w.code != NOT_YET_DRAWN && w.code != case.code)
            .collect();
        assert!(others.is_empty(), "{}: {others:#?}", case.code);
    }
}

/// Raised when an image is read or the document laid out, not by
/// preflight; `pdf_images.rs` and `pdf_text.rs` have their fixtures.
/// Codes the painter's tests cover: images, drawing and what layout
/// changes in the stories it draws.
const LATER_CODES: [&str; 5] = [
    "pdf_image_profile_ignored",
    "pdf_image_animated",
    "pdf_field_switch_ignored",
    "pdf_field_format_approximated",
    "pdf_story_overflow",
];

#[test]
fn every_code_has_a_fixture() {
    let covered: Vec<&str> = cases().iter().map(|c| c.code).collect();
    for (code, description) in WARNING_CODES {
        assert!(
            covered.contains(code) || LATER_CODES.contains(code) || DRAWING_CODES.contains(code),
            "{code} has no fixture"
        );
        assert!(!description.is_empty());
    }
    assert_eq!(
        covered.len() + LATER_CODES.len() + DRAWING_CODES.len(),
        WARNING_CODES.len()
    );
}

/// Codes the font chain reports while drawing, tested in `pdf_fonts.rs`.
const DRAWING_CODES: &[&str] = &[
    "pdf_font_substituted",
    "pdf_glyph_missing",
    "pdf_symbol_substituted",
    "pdf_font_not_embedded",
];

#[test]
fn text_box_and_link_scheme_messages_quote_the_input() {
    let cases = cases();
    let message = |code: &str| {
        let case = cases.iter().find(|c| c.code == code).unwrap();
        report(&case.document)
            .warnings
            .into_iter()
            .find(|w| w.code == code)
            .unwrap()
            .message
    };
    assert!(message("pdf_text_box_skipped").contains("\"Boxed words\""));
    let links = message("pdf_link_scheme_skipped");
    assert!(
        links.contains("\"javascript:\"") && links.contains("\"file:\""),
        "{links}"
    );
}

#[test]
fn the_vml_horizontal_rule_is_not_a_shape() {
    let document = docx(
        &para(&run(
            r#"<w:pict><v:rect o:hr="t" style="width:0;height:1.5pt"/></w:pict>"#,
        )),
        &[],
        &[],
    );
    let report = report(&document);
    assert_eq!(report.warnings.len(), 1, "{report:#?}");
    assert_eq!(
        (report.warnings[0].code, report.warnings[0].subject),
        (NOT_YET_DRAWN, "horizontal rule")
    );
}

#[test]
fn a_converted_document_is_only_not_yet_drawn() {
    let mut document = Document::new();
    let markdown = "# Title\n\nSome *text* with a [link](https://example.com) and a note.[^1]\n\n\
                    - one\n- two\n\n1. first\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n\
                    ```\ncode\n```\n\n> quoted\n\n[^1]: The note.\n";
    insert_markdown(&mut document, markdown, &MarkdownOptions::default()).unwrap();
    let report = report(&document);
    let codes: Vec<_> = report
        .warnings
        .iter()
        .map(|w| (w.code, w.subject))
        .collect();
    assert!(
        report.warnings.iter().all(|w| w.code == NOT_YET_DRAWN),
        "{codes:?}"
    );
    let subjects: Vec<_> = report.warnings.iter().map(|w| w.subject).collect();
    for subject in ["table", "footnote"] {
        assert!(subjects.contains(&subject), "{subject}: {subjects:?}");
    }
    for drawn in ["text", "list", "link"] {
        assert!(!subjects.contains(&drawn), "{drawn}: {subjects:?}");
    }
    let table = report
        .warnings
        .iter()
        .find(|w| w.subject == "table")
        .unwrap();
    assert!(
        table
            .locations
            .iter()
            .all(|l| l.page.is_none() && l.ordinal >= 1)
    );
}

#[test]
fn redact_keeps_codes_and_counts_only() {
    let cases = cases();
    for code in [
        "pdf_text_box_skipped",
        "pdf_link_scheme_skipped",
        "pdf_watermark_skipped",
    ] {
        let case = cases.iter().find(|c| c.code == code).unwrap();
        let mut report = report(&case.document);
        let before = report.clone();
        report.redact();
        let json = report.to_json().to_string();
        for secret in ["Boxed words", "javascript", "passwd", "word/"] {
            assert!(!json.contains(secret), "{secret} in {json}");
        }
        let stories: Vec<&str> = report
            .warnings
            .iter()
            .flat_map(|w| &w.locations)
            .map(|l| l.story.as_str())
            .collect();
        assert!(!stories.is_empty());
        assert!(
            stories.iter().all(|s| *s == "body" || *s == "header"),
            "{stories:?}"
        );
        if code == "pdf_watermark_skipped" {
            assert!(stories.contains(&"header"), "{stories:?}");
        }
        assert_eq!(report.to_json()["schema_version"], 1);
        for (redacted, original) in report.warnings.iter().zip(&before.warnings) {
            assert_eq!(
                (redacted.code, redacted.subject, redacted.count),
                (original.code, original.subject, original.count)
            );
            let fixed = WARNING_CODES
                .iter()
                .find(|(c, _)| *c == redacted.code)
                .unwrap()
                .1;
            assert_eq!(redacted.message, fixed);
        }
    }
}

#[test]
fn not_yet_drawn_refuses_even_without_strict() {
    let document = docx(&bordered("Words"), &[], &[]);
    let err = report(&document).check(false).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
    assert_eq!(err.reason(), NOT_YET_DRAWN);
    assert!(err.message().contains("border"), "{}", err.message());
    let json = err.to_json();
    assert_eq!(
        json["error"]["details"]["warnings"][0]["code"],
        NOT_YET_DRAWN
    );
    assert_eq!(json["error"]["details"]["warnings"][0]["count"], 1);
}

#[test]
fn notes_the_layout_cannot_place_refuse_even_without_strict() {
    let endnote = r#"<w:r><w:endnoteReference w:id="1"/></w:r>"#;
    let section_end = section(r#"<w:endnotePr><w:pos w:val="sectEnd"/></w:endnotePr>"#);
    let body = para(&(FOOTNOTE_REF.to_string() + endnote))
        + &para(&format!("<w:pPr>{section_end}</w:pPr>"))
        + &section(
            r#"<w15:footnoteColumns xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" w:val="2"/>"#,
        );
    let mut report = report(&docx(&body, &[], &[]));
    report
        .warnings
        .retain(|w| w.code == FOOTNOTE_LAYOUT_UNSUPPORTED);
    let subjects: Vec<&str> = report.warnings.iter().map(|w| w.subject).collect();
    assert_eq!(subjects, ["footnote columns", "endnotes at section ends"]);
    let err = report.check(false).unwrap_err();
    assert_eq!(err.reason(), FOOTNOTE_LAYOUT_UNSUPPORTED);
    assert!(
        err.message()
            .contains("footnote columns, endnotes at section ends"),
        "{}",
        err.message()
    );
    let one_section = para(&(FOOTNOTE_REF.to_string() + endnote)) + &section_end;
    assert!(
        preflight(&docx(&one_section, &[], &[]))
            .unwrap()
            .warnings
            .iter()
            .all(|w| w.code != FOOTNOTE_LAYOUT_UNSUPPORTED),
        "one section's end is the document's"
    );
}

#[test]
fn a_note_referenced_only_from_a_note_refuses() {
    let notes = xml_part(
        "word/footnotes.xml",
        "footnotes",
        "footnotes",
        &(r#"<w:footnote w:id="1">"#.to_string()
            + &para(r#"<w:r><w:footnoteReference w:id="2"/></w:r>"#)
            + r#"</w:footnote><w:footnote w:id="2">"#
            + &para(&run("<w:t>Nested.</w:t>"))
            + "</w:footnote>"),
    );
    let mut report = report(&docx(&para(FOOTNOTE_REF), &[notes], &[]));
    report
        .warnings
        .retain(|w| w.code == FOOTNOTE_LAYOUT_UNSUPPORTED);
    assert_eq!(report.warnings.len(), 1, "{report:#?}");
    assert_eq!(
        report.warnings[0].subject,
        "notes referenced only from a note"
    );
    assert_eq!(report.warnings[0].count, 1);
}

#[test]
fn strict_refuses_any_warning_and_changes_nothing() {
    let document = docx(
        &section(r#"<w:cols w:num="3"/><w:lnNumType w:countBy="1"/>"#),
        &[],
        &[],
    );
    let before = snapshot(&document);
    let report = report(&document);
    assert_eq!(report.warnings.len(), 2, "{report:#?}");
    report.check(false).unwrap();
    let err = report.check(true).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
    assert_eq!(err.reason(), "pdf_not_faithful");
    let details = err.details().unwrap();
    let counts: Vec<_> = details.warnings.iter().map(|w| (w.code, w.count)).collect();
    assert_eq!(
        counts,
        [
            ("pdf_columns_ignored", 1),
            ("pdf_layout_feature_ignored", 1)
        ]
    );
    let redacted = err.redacted_message();
    assert!(redacted.contains("pdf_columns_ignored x1"), "{redacted}");
    assert!(redacted.contains("laid out in one column"), "{redacted}");
    assert_eq!(snapshot(&document), before);
    assert_eq!(preflight(&document).unwrap(), report);
}

#[test]
fn errors_without_details_keep_their_json() {
    let err = docrs::Error::unsupported("x", "y");
    assert!(err.details().is_none());
    assert!(err.to_json()["error"].get("details").is_none());
}

/// The subjects of `code` in `report`, with their counts.
fn subjects(report: &PdfReport, code: &str) -> Vec<(&'static str, usize)> {
    report
        .warnings
        .iter()
        .filter(|w| w.code == code)
        .map(|w| (w.subject, w.count))
        .collect()
}

fn styled(body: &str) -> Document {
    let styles = concat!(
        r#"<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>"#,
        r#"<w:style w:type="paragraph" w:styleId="Base"><w:pPr><w:pBdr><w:top w:val="wave" w:sz="4"/></w:pBdr></w:pPr></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Fancy"><w:basedOn w:val="Base"/><w:pPr><w:pageBreakBefore/><w:numPr><w:numId w:val="1"/></w:numPr><w:shd w:val="clear" w:fill="EEEEEE"/><w:framePr w:dropCap="drop" w:lines="3"/><w:bidi/></w:pPr></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Plain"><w:basedOn w:val="Base"/><w:pPr><w:pBdr><w:top w:val="none"/></w:pBdr></w:pPr></w:style>"#,
        r#"<w:style w:type="character" w:styleId="Mark"><w:rPr><w:rtl/><w:highlight w:val="yellow"/></w:rPr></w:style>"#,
        r#"<w:style w:type="table" w:styleId="Grid"><w:tblPr><w:tblpPr w:tblpY="100"/><w:tblBorders><w:top w:val="single" w:sz="4"/></w:tblBorders></w:tblPr></w:style>"#,
        r#"<w:style w:type="table" w:styleId="Shaded"><w:basedOn w:val="Grid"/><w:tblStylePr w:type="firstRow"><w:tcPr><w:shd w:val="clear" w:fill="DDDDDD"/></w:tcPr></w:tblStylePr></w:style>"#,
    );
    let numbering = concat!(
        r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>"#,
        r#"<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#,
    );
    docx(
        body,
        &[
            xml_part("word/styles.xml", "styles", "styles", styles),
            xml_part("word/numbering.xml", "numbering", "numbering", numbering),
        ],
        &[],
    )
}

#[test]
fn constructs_set_through_styles_are_reported() {
    let styled_para = |style: &str, text: &str| {
        para(&format!(
            r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>{}"#,
            run(&format!("<w:t>{text}</w:t>"))
        ))
    };
    let body = styled_para("Fancy", "Fancy")
        + &styled_para("Plain", "No border: its style turns the base's off")
        + &para(&run(
            r#"<w:rPr><w:rStyle w:val="Mark"/></w:rPr><w:t>marked</w:t>"#,
        ))
        + r#"<w:tbl><w:tblPr><w:tblStyle w:val="Shaded"/></w:tblPr><w:tr><w:tc>"#
        + &para(&run("<w:t>cell</w:t>"))
        + "</w:tc></w:tr></w:tbl>";
    let report = report(&styled(&body));
    let not_drawn = subjects(&report, NOT_YET_DRAWN);
    for expected in [("border", 1), ("shading", 1), ("table", 1)] {
        assert!(not_drawn.contains(&expected), "{expected:?}: {not_drawn:?}");
    }
    assert_eq!(
        subjects(&report, "pdf_decoration_approximated"),
        [("paragraph border", 1)]
    );
    assert_eq!(
        subjects(&report, "pdf_layout_feature_ignored"),
        [("drop caps", 1), ("floating tables", 1)]
    );
    assert_eq!(
        subjects(&report, "pdf_text_unsupported"),
        [("right-to-left text", 2)]
    );
}

#[test]
fn the_converters_own_styles_are_reported() {
    let convert = |markdown: &str| {
        let mut document = Document::new();
        insert_markdown(&mut document, markdown, &MarkdownOptions::default()).unwrap();
        report(&document)
    };
    let has = |report: &PdfReport, subject: &str| {
        report
            .warnings
            .iter()
            .any(|w| w.code == NOT_YET_DRAWN && w.subject == subject)
    };
    let table = convert("| A | B |\n| - | - |\n| 1 | 2 |\n");
    assert!(
        has(&table, "border"),
        "{:?}",
        subjects(&table, NOT_YET_DRAWN)
    );
    for drawn in [
        "```\nlet x = 1;\n```\n",
        "Above\n\n---\n\nBelow\n",
        "> Quoted\n",
    ] {
        let report = convert(drawn);
        assert!(
            report.warnings.is_empty(),
            "{drawn:?}: {:?}",
            report.warnings
        );
    }
}

#[test]
fn many_pictures_and_links_scan_in_linear_time() {
    // 8x the objects: linear work grows 8x plus a constant, quadratic 64x.
    for (small, large) in [
        (pictures(1_000), pictures(8_000)),
        (links(1_000), links(8_000)),
    ] {
        let (small_work, large_work) = (work(&small), work(&large));
        assert!(
            large_work <= 8 * small_work,
            "{}: {small_work} then {large_work} units of work",
            small.1
        );
    }
}

/// A document of `count` objects, with the code and subject it reports
/// them under.
type Objects = (&'static str, &'static str, usize, Document);

fn pictures(count: usize) -> Objects {
    let body: String = (0..count)
        .map(|_| para(&graphic(PICTURE_URI, &picture("rId1"))))
        .collect();
    let parts = [image_part("word/media/image1.emf", "image/x-emf")];
    let document = docx(&body, &parts, &[]);
    ("pdf_image_unsupported", "EMF image", count, document)
}

fn links(count: usize) -> Objects {
    let ids: Vec<String> = (0..count).map(|i| format!("rIdL{i}")).collect();
    let links: Vec<(&str, &str)> = ids
        .iter()
        .map(|id| (id.as_str(), "ftp://example.com/"))
        .collect();
    let body = para(
        &ids.iter()
            .map(|id| format!(r#"<w:hyperlink r:id="{id}"></w:hyperlink>"#))
            .collect::<String>(),
    );
    (
        "pdf_link_scheme_skipped",
        "link",
        count,
        docx(&body, &[], &links),
    )
}

/// The children listed and relationships read to scan `objects`, checking
/// the scan reports them all.
fn work((code, subject, count, document): &Objects) -> u64 {
    let (found, work) = preflight_work_for_tests(document).unwrap();
    assert_eq!(subjects(&found, code), [(*subject, *count)]);
    work
}

#[test]
fn constructs_word_draws_are_not_skipped_silently() {
    let body = para(
        &(run(r#"<w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/>"#)
            + &run("<w:br/>")
            + &run("<w:cr/>")
            + &run("<w:noBreakHyphen/>")),
    ) + &para(&graphic(
        "http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas",
        "<lc/>",
    )) + &para(&format!(
        r#"<w:dir w:val="rtl">{}</w:dir>"#,
        run("<w:t>a</w:t>")
    )) + &para(&format!(
        r#"<w:bdo w:val="rtl">{}</w:bdo>"#,
        run("<w:t>b</w:t>")
    )) + r#"<w:tbl><w:tblPr><w:bidiVisual/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#
        + &para(r#"<w:pPr><w:textDirection w:val="tbRl"/></w:pPr>"#)
        + r#"<w:altChunk r:id="rIdChunk"/>"#
        + &section(r#"<w:textDirection w:val="tbRl"/>"#);
    let report = report(&docx(&body, &[], &[]));
    let not_drawn = subjects(&report, NOT_YET_DRAWN);
    for expected in [("positional tab", 1), ("no-break hyphen", 1)] {
        assert!(not_drawn.contains(&expected), "{expected:?}: {not_drawn:?}");
    }
    assert_eq!(subjects(&report, "pdf_shape_skipped"), [("drawing", 1)]);
    assert_eq!(
        subjects(&report, "pdf_text_unsupported"),
        [("right-to-left text", 3)]
    );
    assert_eq!(
        subjects(&report, "pdf_layout_feature_ignored"),
        [("rotated text", 2)]
    );
    assert_eq!(
        subjects(&report, "pdf_object_skipped"),
        [("imported document", 1)]
    );
}

#[test]
fn what_the_painter_does_not_draw_yet_stays_refused_and_header_text_does_not() {
    let body = para(&run(r#"<w:sym w:font="Wingdings" w:char="F04A"/>"#))
        + &para(&run("<w:t>soft</w:t><w:softHyphen/><w:t>after</w:t>"))
        + &text_para("hy\u{AD}phen")
        + &para(&(r#"<w:pPr><w:sectPr/></w:pPr>"#.to_string() + &run("<w:t>end</w:t>")))
        + &text_para("next section")
        + &section(r#"<w:headerReference w:type="default" r:id="rId1"/>"#);
    let report = report(&docx(
        &body,
        &[header("word/header1.xml", &text_para("Letterhead"))],
        &[],
    ));
    assert_eq!(
        subjects(&report, NOT_YET_DRAWN),
        [("soft hyphen", 2), ("section break", 1),]
    );
}

#[test]
fn run_decorations_and_fills_are_drawn() {
    let body = [
        r#"<w:u w:val="wave" w:color="FF0000"/>"#,
        "<w:strike/>",
        "<w:dstrike/>",
        r#"<w:highlight w:val="yellow"/>"#,
        r#"<w:shd w:val="pct50" w:color="auto" w:fill="auto"/>"#,
        r#"<w:color w:val="auto"/><w:shd w:val="clear" w:fill="1F3864"/>"#,
    ]
    .map(|rpr| para(&run(&format!("<w:rPr>{rpr}</w:rPr><w:t>look</w:t>"))))
    .concat();
    let report = report(&docx(&body, &[], &[]));
    assert_eq!(report.warnings, [], "{report:#?}");
}

fn text_para(text: &str) -> String {
    para(&run(&format!("<w:t>{text}</w:t>")))
}

#[test]
fn a_vml_rule_holding_a_text_box_or_picture_counts_as_them() {
    let body = para(&run(
        r#"<w:pict><v:rect o:hr="t" style="width:0;height:20pt"><v:textbox><w:txbxContent><w:p><w:r><w:t>Ruled words</w:t></w:r></w:p></w:txbxContent></v:textbox></v:rect></w:pict>"#,
    )) + &para(&run(
        r#"<w:pict><v:rect o:hr="t" style="width:0;height:20pt"><v:imagedata r:id="rId1"/></v:rect></w:pict>"#,
    ));
    let report = report(&docx(
        &body,
        &[image_part("word/media/image1.emf", "image/x-emf")],
        &[],
    ));
    assert_eq!(subjects(&report, "pdf_text_box_skipped"), [("text box", 1)]);
    assert_eq!(
        subjects(&report, "pdf_image_unsupported"),
        [("EMF image", 1)]
    );
    assert!(
        !subjects(&report, NOT_YET_DRAWN)
            .iter()
            .any(|(s, _)| *s == "horizontal rule")
    );
}

#[test]
fn fields_and_drawing_links_are_classified() {
    let complex = |instruction: &str| {
        run(r#"<w:fldChar w:fldCharType="begin"/>"#)
            + &run(&format!("<w:instrText>{instruction}</w:instrText>"))
            + &run(r#"<w:fldChar w:fldCharType="separate"/>"#)
            + &run("<w:t>result</w:t>")
            + &run(r#"<w:fldChar w:fldCharType="end"/>"#)
    };
    let body = para(&complex(r#"DATE\@ "d MMMM""#))
        + &para(&complex(r#" HYPERLINK "javascript:alert(1)" "#))
        + &para(&complex(r#" HYPERLINK \l "intro" "#))
        + &para(
            &(run(r#"<w:fldChar w:fldCharType="begin"/>"#)
                + &run("<w:instrText> TIME </w:instrText>")),
        )
        + &para(&run(
            r#"<w:drawing><wp:inline><wp:extent cx="1" cy="1"/><wp:docPr id="1" name="P"><a:hlinkClick r:id="rIdL1"/></wp:docPr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic/></a:graphicData></a:graphic></wp:inline></w:drawing>"#,
        ));
    let report = report(&docx(&body, &[], &[("rIdL1", "file:///etc/passwd")]));
    assert_eq!(
        subjects(&report, "pdf_field_cached"),
        [("DATE", 1), ("TIME", 1), ("field without an end", 1)]
    );
    assert_eq!(subjects(&report, "pdf_link_scheme_skipped"), [("link", 2)]);
    assert_eq!(subjects(&report, "pdf_link_target_missing"), [("link", 1)]);
    // The picture names no image: drawn as a placeholder.
    assert_eq!(
        subjects(&report, "pdf_image_unsupported"),
        [("missing image", 1)]
    );
}

#[test]
fn text_links_are_drawn_and_picture_links_are_not_yet() {
    let picture = |link: &str| {
        para(&run(&format!(
            r#"<w:drawing><wp:inline><wp:extent cx="1" cy="1"/><wp:docPr id="1" name="P"><a:hlinkClick r:id="{link}"/></wp:docPr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic/></a:graphicData></a:graphic></wp:inline></w:drawing>"#
        )))
    };
    let body = para(
        &(r#"<w:bookmarkStart w:id="0" w:name="here"/><w:hyperlink r:id="rIdL1">"#.to_string()
            + &run("<w:t>web</w:t>")
            + r#"</w:hyperlink><w:hyperlink r:id="rIdL2">"#
            + &run("<w:t>here</w:t>")
            + r#"</w:hyperlink><w:hyperlink r:id="rIdL3">"#
            + &run("<w:t>gone</w:t>")
            + "</w:hyperlink>"),
    ) + &picture("rIdL1")
        + &picture("rIdL2");
    let report = report(&docx(
        &body,
        &[],
        &[
            ("rIdL1", "https://example.com/"),
            ("rIdL2", "#here"),
            ("rIdL3", "#gone"),
        ],
    ));
    let not_drawn = subjects(&report, NOT_YET_DRAWN);
    assert!(not_drawn.contains(&("picture link", 2)), "{not_drawn:?}");
    assert!(
        !not_drawn.iter().any(|(s, _)| *s == "link"),
        "{not_drawn:?}"
    );
    assert_eq!(subjects(&report, "pdf_link_target_missing"), [("link", 1)]);
}

#[test]
fn scripts_that_need_shaping_are_flagged() {
    let cases = [
        ("\u{0418}\u{0483}", "combining marks"),
        ("a\u{1CD0}", "combining marks"),
        ("\u{1D165}", "combining marks"),
        ("\u{ABC0}", "complex script"),
        ("\u{1700}", "complex script"),
        ("\u{A960}", "CJK text"),
        ("\u{D7B0}", "CJK text"),
        ("\u{1F468}\u{200D}\u{1F469}", "emoji sequence"),
        ("\u{2764}\u{FE0F}", "emoji sequence"),
        ("\u{1F44D}\u{1F3FD}", "emoji sequence"),
    ];
    for (text, subject) in cases {
        let document = docx(&para(&run(&format!("<w:t>{text}</w:t>"))), &[], &[]);
        assert_eq!(
            subjects(&report(&document), "pdf_text_unsupported"),
            [(subject, 1)],
            "{text:?}"
        );
    }
    let latin = docx(
        &para(&run(
            "<w:t>Caf\u{e9} \u{201c}na\u{ef}ve\u{201d} \u{2013} \u{20ac}5 \u{2610} \u{1F600}</w:t>",
        )),
        &[],
        &[],
    );
    assert!(subjects(&report(&latin), "pdf_text_unsupported").is_empty());
}

#[test]
fn a_style_border_survives_another_edge_cleared() {
    let styles = concat!(
        r#"<w:style w:type="paragraph" w:styleId="Ruled"><w:pPr><w:pBdr><w:bottom w:val="wave" w:sz="4"/></w:pBdr></w:pPr></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="RuledChild"><w:basedOn w:val="Ruled"/><w:pPr><w:pBdr><w:top w:val="nil"/></w:pBdr></w:pPr></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Unruled"><w:basedOn w:val="Ruled"/><w:pPr><w:pBdr><w:bottom w:val="nil"/></w:pBdr></w:pPr></w:style>"#,
    );
    let paragraph = |ppr: &str| para(&format!("<w:pPr>{ppr}</w:pPr>{}", run("<w:t>x</w:t>")));
    let body = paragraph(r#"<w:pStyle w:val="Ruled"/>"#)
        + &paragraph(r#"<w:pStyle w:val="Ruled"/><w:pBdr><w:top w:val="nil"/></w:pBdr>"#)
        + &paragraph(r#"<w:pStyle w:val="RuledChild"/>"#)
        + &paragraph(r#"<w:pStyle w:val="Unruled"/>"#);
    let document = docx(
        &body,
        &[xml_part("word/styles.xml", "styles", "styles", styles)],
        &[],
    );
    let found = report(&document);
    assert_eq!(
        subjects(&found, "pdf_decoration_approximated"),
        [("paragraph border", 3)]
    );
}

#[test]
fn row_shading_vml_links_and_computed_hyperlinks_are_checked() {
    let complex = |instruction: &str, nested: &str| {
        run(r#"<w:fldChar w:fldCharType="begin"/>"#)
            + &run(&format!("<w:instrText>{instruction}</w:instrText>"))
            + nested
            + &run(r#"<w:fldChar w:fldCharType="separate"/>"#)
            + &run("<w:t>result</w:t>")
            + &run(r#"<w:fldChar w:fldCharType="end"/>"#)
    };
    let nested = complex(" REF target ", "");
    let body = concat!(
        r#"<w:tbl><w:tr><w:tblPrEx><w:shd w:val="clear" w:fill="FFFF00"/></w:tblPrEx>"#,
        r#"<w:tc><w:p/></w:tc></w:tr></w:tbl>"#
    )
    .to_string()
        + &para(&run(
            r#"<w:pict><v:rect href="javascript:alert(1)" style="width:5pt;height:5pt"/></w:pict>"#,
        ))
        + &para(&complex(" HYPERLINK ", &nested));
    let found = report(&docx(&body, &[], &[]));
    assert!(
        subjects(&found, NOT_YET_DRAWN).contains(&("shading", 1)),
        "{:?}",
        subjects(&found, NOT_YET_DRAWN)
    );
    assert!(
        !subjects(&found, NOT_YET_DRAWN)
            .iter()
            .any(|(s, _)| *s == "link"),
        "{:?}",
        subjects(&found, NOT_YET_DRAWN)
    );
    assert_eq!(subjects(&found, "pdf_link_scheme_skipped"), [("link", 2)]);
    let message = &found
        .warnings
        .iter()
        .find(|w| w.code == "pdf_link_scheme_skipped")
        .unwrap()
        .message;
    assert!(
        message.contains("\"javascript:\"") && message.contains("\"computed\""),
        "{message}"
    );
}

#[test]
fn a_bar_border_is_reported_and_between_is_drawn() {
    let bordered = |edge: &str| {
        para(&format!(
            r#"<w:pPr><w:pBdr><w:{edge} w:val="single" w:sz="4"/></w:pBdr></w:pPr>{}"#,
            run("<w:t>x</w:t>")
        ))
    };
    let found = report(&docx(&(bordered("between") + &bordered("bar")), &[], &[]));
    assert!(
        subjects(&found, NOT_YET_DRAWN).contains(&("border", 1)),
        "{:?}",
        subjects(&found, NOT_YET_DRAWN)
    );
}

#[test]
fn a_link_on_a_shape_in_a_vml_group_is_checked() {
    let body = para(&run(concat!(
        r#"<w:pict><v:group style="width:20pt;height:20pt">"#,
        r#"<v:rect href="javascript:alert(1)" style="width:5pt;height:5pt"/>"#,
        r#"</v:group></w:pict>"#
    )));
    let found = report(&docx(&body, &[], &[]));
    assert_eq!(subjects(&found, "pdf_link_scheme_skipped"), [("link", 1)]);
    assert_eq!(subjects(&found, "pdf_shape_skipped"), [("shape", 1)]);
}

#[test]
fn contents_fields_are_drawn_and_mirrored_indents_are_ignored() {
    let field = |instruction: &str| {
        run(r#"<w:fldChar w:fldCharType="begin"/>"#)
            + &run(&format!("<w:instrText>{instruction}</w:instrText>"))
            + &run(r#"<w:fldChar w:fldCharType="separate"/>"#)
            + &run("<w:t>2</w:t>")
            + &run(r#"<w:fldChar w:fldCharType="end"/>"#)
    };
    let mirrored = r#"<w:pPr><w:mirrorIndents/><w:ind w:left="720"/></w:pPr>"#;
    // A contents whose entry's number is a page reference.
    let contents = run(r#"<w:fldChar w:fldCharType="begin"/>"#)
        + &run(r#"<w:instrText> TOC \o "1-3" \h </w:instrText>"#)
        + &run(r#"<w:fldChar w:fldCharType="separate"/>"#)
        + &run("<w:t>Intro</w:t>")
        + &field(r#" PAGEREF _Toc1 \h "#);
    let body = para(&contents)
        + &para(&run(r#"<w:fldChar w:fldCharType="end"/>"#))
        + &para(&field(r#" PAGEREF _Toc1 \h "#))
        + &para(&(mirrored.to_string() + &run("<w:t>mirrored</w:t>")))
        + &para(
            &(r#"<w:pPr><w:pStyle w:val="Facing"/></w:pPr>"#.to_string()
                + &run("<w:t>by style</w:t>")),
        );
    let styles = r#"<w:style w:type="paragraph" w:styleId="Facing"><w:pPr><w:mirrorIndents/></w:pPr></w:style>"#;
    let found = report(&docx(
        &body,
        &[xml_part("word/styles.xml", "styles", "styles", styles)],
        &[],
    ));
    assert_eq!(subjects(&found, NOT_YET_DRAWN), []);
    assert_eq!(subjects(&found, "pdf_field_cached"), []);
    assert_eq!(
        subjects(&found, "pdf_layout_feature_ignored"),
        [("mirrored indents", 2)]
    );
    // One whose entries hold their numbers as text keeps them; one without
    // numbers (`\n`) is drawn as it is.
    let plain = |instruction: &str, entry: &str| {
        report(&docx(
            &para(
                &(run(r#"<w:fldChar w:fldCharType="begin"/>"#)
                    + &run(&format!("<w:instrText>{instruction}</w:instrText>"))
                    + &run(r#"<w:fldChar w:fldCharType="separate"/>"#)
                    + &run(&format!("<w:t>{entry}</w:t>"))
                    + &run(r#"<w:fldChar w:fldCharType="end"/>"#)),
            ),
            &[],
            &[],
        ))
    };
    let saved = plain(r#" TOC \o "1-3" "#, "Intro 2");
    assert_eq!(subjects(&saved, "pdf_field_cached"), [("TOC", 1)]);
    let unnumbered = plain(r#" TOC \o "1-3" \n "#, "Intro");
    assert_eq!(subjects(&unnumbered, "pdf_field_cached"), []);
    let numbered = plain(r#" TOC \o "1-3" \n "#, "Intro 2");
    assert_eq!(subjects(&numbered, "pdf_field_cached"), [("TOC", 1)]);
}

#[test]
fn equations_are_counted_located_and_quoted_as_their_linear_text() {
    let r = |text: &str| format!("<m:r><m:t>{text}</m:t></m:r>");
    let fraction = format!(
        "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
        r("a"),
        r("b")
    );
    let square = format!(
        "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
        r("x"),
        r("2")
    );
    let hat = format!("<m:acc><m:e>{}</m:e></m:acc>", r("y"));
    let body = para(&format!("<m:oMath>{fraction}</m:oMath>"))
        + &para(&run("<w:t>Between</w:t>"))
        + &para(&format!(
            "<m:oMathPara><m:oMath>{square}</m:oMath><m:oMath>{hat}</m:oMath></m:oMathPara>"
        ));
    let report = report(&docx(&body, &[], &[]));
    let equations = report
        .warnings
        .iter()
        .find(|w| w.code == "pdf_equation_linearized")
        .unwrap();
    assert_eq!(equations.count, 3);
    let ordinals: Vec<usize> = equations.locations.iter().map(|l| l.ordinal).collect();
    assert_eq!(ordinals, [1, 3, 3]);
    assert!(
        equations
            .message
            .ends_with("(\"a/b\", \"x^2\", \"y\u{302}\")"),
        "{}",
        equations.message
    );
    assert_eq!(
        subjects(&report, "pdf_text_unsupported"),
        [("combining marks", 1)],
        "the accent is a combining mark in the linear text"
    );
}

#[test]
fn a_deleted_equation_is_not_reported() {
    let math = "<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath>";
    let document = docx(
        &para(&format!(
            r#"<w:del w:id="1" w:author="A">{math}</w:del><w:moveFrom w:id="2" w:author="A">{math}</w:moveFrom><w:ins w:id="3" w:author="A">{math}</w:ins>"#
        )),
        &[],
        &[],
    );
    let report = report(&document);
    assert_eq!(
        subjects(&report, "pdf_equation_linearized"),
        [("equation", 1)]
    );
}

#[test]
fn strict_refuses_a_linearized_equation() {
    let document = docx(
        &para("<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath>"),
        &[],
        &[],
    );
    let report = report(&document);
    report.check(false).unwrap();
    let err = report.check(true).unwrap_err();
    assert_eq!(err.reason(), "pdf_not_faithful");
    let counts: Vec<_> = err
        .details()
        .unwrap()
        .warnings
        .iter()
        .map(|w| (w.code, w.count))
        .collect();
    assert_eq!(counts, [("pdf_equation_linearized", 1)]);
}

#[test]
fn conversions_that_keep_a_template_s_sections_report_only_its_content() {
    use docrs::markdown::Placement;
    let root = support::repo_root().join("tests/fixtures/markdown/templates/v2");
    let open = |name: &str| Document::open_path(root.join(name), &Limits::default()).unwrap();
    let mut placeholder = open("multi-section-portrait-then-landscape.docx");
    let xml = support::document_xml(&placeholder).replace(
        "The first section is portrait A4 with 2.54 cm margins.",
        "{{content}}",
    );
    placeholder
        .package_mut_for_tests()
        .commit_tree("word/document.xml", docrs::xml::parse_str(&xml).unwrap())
        .unwrap();
    let cases = [
        (
            open("multi-section-portrait-then-landscape.docx"),
            Placement::Append,
        ),
        (open("cover-page-sections.docx"), Placement::Append),
        (placeholder, Placement::Placeholder("{{content}}".into())),
    ];
    let found = |document: &Document| -> Vec<(&str, &str)> {
        let report = report(document);
        report
            .warnings
            .iter()
            .map(|w| (w.code, w.subject))
            .collect()
    };
    for (mut document, placement) in cases {
        let template = found(&document);
        let options = MarkdownOptions {
            placement,
            ..MarkdownOptions::default()
        };
        insert_markdown(&mut document, "# Heading\n\nText.\n", &options).unwrap();
        let converted = found(&document);
        assert!(
            converted
                .iter()
                .all(|w| template.contains(w) || *w == (NOT_YET_DRAWN, "text")),
            "{converted:?} against {template:?}"
        );
    }
}

#[test]
fn only_justified_vertical_page_alignment_is_reported() {
    let aligned = |value: &str| {
        let body = para(&run("Cover")) + &section(&format!(r#"<w:vAlign w:val="{value}"/>"#));
        report(&docx(&body, &[], &[]))
    };
    for value in ["top", "center", "bottom"] {
        assert!(aligned(value).warnings.is_empty(), "{value}");
    }
    assert_eq!(
        subjects(&aligned("both"), "pdf_layout_feature_ignored"),
        [("vertical page justification", 1)]
    );
}

#[test]
fn table_layout_features_the_pdf_ignores_are_reported() {
    let table = |tbl_pr: &str, tc_pr: &str| {
        format!(
            "<w:tbl><w:tblPr>{tbl_pr}</w:tblPr><w:tr><w:tc><w:tcPr>{tc_pr}</w:tcPr>{}</w:tc></w:tr></w:tbl>",
            para(&run("<w:t>cell</w:t>"))
        )
    };
    let body = table(r#"<w:tblCellSpacing w:w="58" w:type="dxa"/>"#, "")
        + &table(r#"<w:tblCellSpacing w:w="0" w:type="dxa"/>"#, "")
        + &table(r#"<w:tblpPr w:tblpX="2000" w:tblpY="2000"/>"#, "")
        + &table("", r#"<w:textDirection w:val="btLr"/>"#)
        + &para("");
    let report = report(&docx(&body, &[], &[]));
    assert_eq!(
        subjects(&report, "pdf_layout_feature_ignored"),
        [
            ("cell spacing", 1),
            ("floating tables", 1),
            ("rotated cell text", 1)
        ]
    );
}

#[test]
fn a_vml_picture_naming_no_part_is_not_yet_drawn() {
    // VML pictures are not drawn at all, so a missing part is no
    // placeholder: the picture is reported as not drawn.
    let body = para(&run(
        r#"<w:pict><v:shape style="width:20pt;height:20pt"><v:imagedata r:id="rIdNone"/></v:shape></w:pict>"#,
    ));
    let report = report(&docx(&body, &[], &[]));
    assert_eq!(subjects(&report, NOT_YET_DRAWN), [("image", 1)]);
    assert!(subjects(&report, "pdf_image_unsupported").is_empty());
}
