//! The PDF preflight scan: one generated document per warning code.

mod support;

use docrs::markdown::{MarkdownOptions, insert_markdown};
use docrs::pdf::{NOT_YET_DRAWN, PdfReport, WARNING_CODES, preflight};
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
    run(&format!(
        r#"<w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Graphic"/><a:graphic><a:graphicData uri="{uri}">{inner}</a:graphicData></a:graphic></wp:inline></w:drawing>"#
    ))
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
}

fn case(code: &'static str, subject: &'static str, count: usize, document: Document) -> Case {
    Case {
        code,
        subject,
        count,
        document,
    }
}

fn cases() -> Vec<Case> {
    let text = |t: &str| para(&run(&format!("<w:t>{t}</w:t>")));
    vec![
        case(
            NOT_YET_DRAWN,
            "text",
            2,
            docx(&(text("One") + &text("Two") + &para("")), &[], &[]),
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
                    &(r#"<w:hyperlink r:id="rIdL1">"#.to_string()
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
    ]
}

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
        assert_eq!(
            found[0].locations.len(),
            case.count.min(docrs::pdf::MAX_LOCATIONS)
        );
        let others: Vec<_> = report
            .warnings
            .iter()
            .filter(|w| w.code != NOT_YET_DRAWN && w.code != case.code)
            .collect();
        assert!(others.is_empty(), "{}: {others:#?}", case.code);
    }
}

#[test]
fn every_code_has_a_fixture() {
    let covered: Vec<&str> = cases().iter().map(|c| c.code).collect();
    for (code, description) in WARNING_CODES {
        assert!(covered.contains(code), "{code} has no fixture");
        assert!(!description.is_empty());
    }
    assert_eq!(covered.len(), WARNING_CODES.len());
}

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
    for subject in ["text", "list", "table", "link", "footnote"] {
        assert!(subjects.contains(&subject), "{subject}: {subjects:?}");
    }
    let text = report
        .warnings
        .iter()
        .find(|w| w.subject == "text")
        .unwrap();
    assert!(
        text.locations
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
    let document = docx(&para(&run("<w:t>Words</w:t>")), &[], &[]);
    let err = report(&document).check(false).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
    assert_eq!(err.reason(), NOT_YET_DRAWN);
    assert!(err.message().contains("text"), "{}", err.message());
    let json = err.to_json();
    assert_eq!(
        json["error"]["details"]["warnings"][0]["code"],
        NOT_YET_DRAWN
    );
    assert_eq!(json["error"]["details"]["warnings"][0]["count"], 1);
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
        r#"<w:style w:type="paragraph" w:styleId="Base"><w:pPr><w:pBdr><w:top w:val="single" w:sz="4"/></w:pBdr></w:pPr></w:style>"#,
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
    for expected in [
        ("page break", 1),
        ("list", 1),
        ("border", 2),
        ("shading", 3),
        ("table", 1),
    ] {
        assert!(not_drawn.contains(&expected), "{expected:?}: {not_drawn:?}");
    }
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
    let code = convert("```\nlet x = 1;\n```\n");
    assert!(
        has(&code, "shading"),
        "{:?}",
        subjects(&code, NOT_YET_DRAWN)
    );
    let table = convert("| A | B |\n| - | - |\n| 1 | 2 |\n");
    assert!(
        has(&table, "border"),
        "{:?}",
        subjects(&table, NOT_YET_DRAWN)
    );
    let rule = convert("Above\n\n---\n\nBelow\n");
    assert!(has(&rule, "border"), "{:?}", subjects(&rule, NOT_YET_DRAWN));
    let quote = convert("> Quoted\n");
    assert!(
        quote.warnings.iter().all(|w| w.code == NOT_YET_DRAWN),
        "{:?}",
        quote.warnings
    );
}

#[test]
fn many_pictures_and_links_scan_in_linear_time() {
    let pictures: String = (0..8_000)
        .map(|_| para(&graphic(PICTURE_URI, &picture("rId1"))))
        .collect();
    let document = docx(
        &pictures,
        &[image_part("word/media/image1.png", "image/png")],
        &[],
    );
    let started = std::time::Instant::now();
    let found = report(&document);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(subjects(&found, NOT_YET_DRAWN), [("image", 8_000)]);

    // Linear work grows 4x with 4x the links, quadratic 16x; the ratio
    // holds whatever the machine's speed.
    let small = links_scan_time(25_000);
    let large = links_scan_time(100_000);
    assert!(large < small * 8, "{small:?} then {large:?}");
}

/// The fastest of three scans of `count` links.
fn links_scan_time(count: usize) -> std::time::Duration {
    let ids: Vec<String> = (0..count).map(|i| format!("rIdL{i}")).collect();
    let links: Vec<(&str, &str)> = ids
        .iter()
        .map(|id| (id.as_str(), "https://example.com/"))
        .collect();
    let body = para(
        &ids.iter()
            .map(|id| format!(r#"<w:hyperlink r:id="{id}"></w:hyperlink>"#))
            .collect::<String>(),
    );
    let document = docx(&body, &[], &links);
    (0..3)
        .map(|_| {
            let started = std::time::Instant::now();
            let found = report(&document);
            let elapsed = started.elapsed();
            assert_eq!(subjects(&found, NOT_YET_DRAWN), [("link", count)]);
            elapsed
        })
        .min()
        .unwrap()
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
    for expected in [("tab", 1), ("line break", 1), ("text", 3)] {
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
        [("DATE", 1), ("TIME", 1)]
    );
    assert_eq!(subjects(&report, "pdf_link_scheme_skipped"), [("link", 2)]);
    let not_drawn = subjects(&report, NOT_YET_DRAWN);
    assert!(not_drawn.contains(&("link", 1)), "{not_drawn:?}");
    assert!(not_drawn.contains(&("field", 2)), "{not_drawn:?}");
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
        r#"<w:style w:type="paragraph" w:styleId="Ruled"><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="4"/></w:pBdr></w:pPr></w:style>"#,
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
    assert!(
        subjects(&found, NOT_YET_DRAWN).contains(&("border", 3)),
        "{:?}",
        subjects(&found, NOT_YET_DRAWN)
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
