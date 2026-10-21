//! Synthetic documents for M4 work package p2 (numbering, blocks, fields).
//!
//! Every part is written compactly (no indentation whitespace) so the
//! oracle's `remove_blank_text` parse and the Rust arena agree on content.

use crate::hostile::build::{DECLS, RawMember, build_zip};

const W_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT_BASE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";

/// An extra part related from the main document.
struct Extra<'a> {
    name: &'a str,
    content_type: &'a str,
    rel_type: &'a str,
    xml: String,
}

fn package(body: &str, extras: &[Extra<'_>]) -> Vec<u8> {
    let mut overrides = format!(
        "<Override PartName=\"/word/document.xml\" ContentType=\"{CT_BASE}.document.main+xml\"/>"
    );
    let mut rels = String::new();
    for (index, extra) in extras.iter().enumerate() {
        overrides.push_str(&format!(
            "<Override PartName=\"/word/{}\" ContentType=\"{CT_BASE}.{}\"/>",
            extra.name, extra.content_type
        ));
        rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"{REL_BASE}/{}\" Target=\"{}\"/>",
            index + 1,
            extra.rel_type,
            extra.name
        ));
    }
    let content_types = format!(
        "{XML_HEAD}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>{overrides}</Types>"
    );
    let root_rels = format!(
        "{XML_HEAD}<Relationships xmlns=\"{RELS_NS}\"><Relationship Id=\"rId1\" Type=\"{REL_BASE}/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
    );
    let document = format!("{XML_HEAD}<w:document {DECLS}><w:body>{body}</w:body></w:document>");
    let mut members = vec![
        RawMember::new("[Content_Types].xml", content_types.as_bytes()),
        RawMember::new("_rels/.rels", root_rels.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
    ];
    if !extras.is_empty() {
        let doc_rels =
            format!("{XML_HEAD}<Relationships xmlns=\"{RELS_NS}\">{rels}</Relationships>");
        members.push(RawMember::new(
            "word/_rels/document.xml.rels",
            doc_rels.as_bytes(),
        ));
    }
    for extra in extras {
        members.push(RawMember::new(
            &format!("word/{}", extra.name),
            extra.xml.as_bytes(),
        ));
    }
    build_zip(&members)
}

fn p(inner: &str) -> String {
    format!("<w:p>{inner}</w:p>")
}

fn r(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn para(text: &str) -> String {
    p(&r(text))
}

fn styled(style: &str, text: &str) -> String {
    p(&format!(
        "<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>{}",
        r(text)
    ))
}

fn numbered(num_id: &str, ilvl: &str, text: &str) -> String {
    p(&format!(
        "<w:pPr><w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>{}",
        r(text)
    ))
}

const SECT: &str = "<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId9\"/><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr>";

fn styles_xml() -> String {
    let style = |kind: &str, id: &str, name: &str, extra: &str| {
        format!(
            "<w:style w:type=\"{kind}\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/>{extra}</w:style>"
        )
    };
    format!(
        "{XML_HEAD}<w:styles {W_DECL}><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>{}</w:styles>",
        [
            style("paragraph", "Heading1", "heading 1", "<w:basedOn w:val=\"Normal\"/>"),
            style("paragraph", "Heading2", "heading 2", "<w:basedOn w:val=\"Normal\"/>"),
            style("paragraph", "BodyText", "Body Text", ""),
            style("paragraph", "ListBullet", "List Bullet", "<w:pPr><w:numPr><w:numId w:val=\"1\"/></w:numPr></w:pPr>"),
            style("paragraph", "ListChild", "List Child", "<w:basedOn w:val=\"ListBullet\"/><w:pPr><w:numPr><w:ilvl w:val=\"1\"/></w:numPr></w:pPr>"),
            style("paragraph", "NoNumber", "No Number", "<w:basedOn w:val=\"ListBullet\"/><w:pPr><w:numPr><w:numId w:val=\"0\"/></w:numPr></w:pPr>"),
            style("paragraph", "GhostList", "Ghost List", "<w:pPr><w:numPr><w:numId w:val=\"77\"/></w:numPr></w:pPr>"),
            style("paragraph", "LoopA", "Loop A", "<w:basedOn w:val=\"LoopB\"/>"),
            style("paragraph", "LoopB", "Loop B", "<w:basedOn w:val=\"LoopA\"/>"),
            style("character", "Strong", "Strong", ""),
            style("paragraph", "Caption", "caption", ""),
        ]
        .concat()
    )
}

fn styles() -> Extra<'static> {
    Extra {
        name: "styles.xml",
        content_type: "styles+xml",
        rel_type: "styles",
        xml: styles_xml(),
    }
}

fn numbering(inner: &str) -> Extra<'static> {
    Extra {
        name: "numbering.xml",
        content_type: "numbering+xml",
        rel_type: "numbering",
        xml: format!("{XML_HEAD}<w:numbering {W_DECL}>{inner}</w:numbering>"),
    }
}

fn settings(inner: &str) -> Extra<'static> {
    Extra {
        name: "settings.xml",
        content_type: "settings+xml",
        rel_type: "settings",
        xml: format!(
            "{XML_HEAD}<w:settings {W_DECL} xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">{inner}</w:settings>"
        ),
    }
}

fn header(body: &str) -> Extra<'static> {
    Extra {
        name: "header1.xml",
        content_type: "header+xml",
        rel_type: "header",
        xml: format!("{XML_HEAD}<w:hdr {DECLS}>{body}</w:hdr>"),
    }
}

/// The section properties referencing the header at relationship `rId{n}`.
fn sect_for(header_rid: usize) -> String {
    SECT.replace("rId9", &format!("rId{header_rid}"))
}

const NUMBERING_MAIN: &str = concat!(
    "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{2022}\"/></w:lvl>",
    "<w:lvl w:ilvl=\"1\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"o\"/></w:lvl></w:abstractNum>",
    "<w:abstractNum w:abstractNumId=\"5\"><w:name w:val=\"PaperBulletList\"/><w:multiLevelType w:val=\"hybridMultilevel\"/>",
    "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"-\"/></w:lvl></w:abstractNum>",
    "<w:abstractNum w:abstractNumId=\"3\"><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/></w:lvl>",
    "<w:lvl w:ilvl=\"1\"><w:numFmt w:val=\"lowerLetter\"/></w:lvl></w:abstractNum>",
    "<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
    "<w:num w:numId=\"4\"><w:abstractNumId w:val=\"3\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"3\"/></w:lvlOverride>",
    "<w:lvlOverride w:ilvl=\"2\"><w:lvl w:ilvl=\"2\"><w:numFmt w:val=\"upperRoman\"/><w:lvlText w:val=\"%3)\"/></w:lvl></w:lvlOverride></w:num>",
    "<w:num w:numId=\"2\"><w:abstractNumId w:val=\"5\"/></w:num>",
    "<w:num w:numId=\"6\"/>",
    "<w:num w:numId=\"8\"><w:abstractNumId w:val=\"44\"/></w:num>",
    "<w:num w:numId=\"10\"><!-- note --><w:abstractNumId w:val=\"0\"/></w:num>",
    "<w:numIdMacAtCleanup w:val=\"3\"/>"
);

fn numbering_doc() -> Vec<u8> {
    let textbox = p(&format!(
        "<w:r><mc:AlternateContent><mc:Choice Requires=\"wps\"><w:drawing><wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx></w:drawing></mc:Choice><mc:Fallback><w:pict/></mc:Fallback></mc:AlternateContent></w:r>{}",
        numbered("1", "0", "Boxed numbered"),
        r("Box anchor")
    ));
    let table = format!(
        "<w:tbl><w:tr><w:trPr><w:gridBefore w:val=\"1\"/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr>{}</w:tc><w:tc>{}{}</w:tc></w:tr></w:tbl>",
        para("cell A"),
        para("first in cell"),
        numbered("1", "0", "cell numbered")
    );
    let body = [
        numbered("1", "0", "Bullet one"),
        numbered("1", "1", "Bullet two nested"),
        styled("ListBullet", "Styled bullet"),
        styled("ListChild", "Styled child"),
        styled("NoNumber", "Unnumbered by style"),
        p(&format!(
            "<w:pPr><w:pStyle w:val=\"ListBullet\"/><w:numPr><w:numId w:val=\"0\"/></w:numPr></w:pPr>{}",
            r("Explicit zero")
        )),
        numbered("4", "2", "Override roman"),
        table,
        format!(
            "<w:sdt><w:sdtPr><w:tag w:val=\"c\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>",
            numbered("1", "0", "In control")
        ),
        textbox,
        p(&format!(
            "<w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:del w:id=\"3\" w:author=\"A\"><w:r><w:delText>gone </w:delText></w:r></w:del>{}",
            r("kept text")
        )),
        styled("LoopA", "Loop styled"),
        para("Plain target"),
        para("Second plain"),
        sect_for(4),
    ]
    .concat();
    package(
        &body,
        &[
            styles(),
            numbering(NUMBERING_MAIN),
            settings(""),
            header(&numbered("1", "1", "Header numbered")),
        ],
    )
}

fn blocks_body() -> String {
    let toc = [
        p(&format!(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-3\" </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}",
            r("Toc entry one")
        )),
        para("Toc entry two"),
        p(&format!("{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>", r("Toc entry three"))),
    ]
    .concat();
    [
        styled("Heading1", "Intro heading"),
        para("before"),
        para("alpha end"),
        para("beta start"),
        p(&format!(
            "<w:pPr><w:jc w:val=\"left\"/></w:pPr><w:commentRangeStart w:id=\"0\"/><w:r><w:rPr><w:b/></w:rPr><w:t>Bold start</w:t></w:r><w:proofErr w:type=\"spellStart\"/>{}<w:proofErr w:type=\"spellEnd\"/><w:bookmarkStart w:id=\"1\" w:name=\"_GoBack\"/><w:bookmarkEnd w:id=\"1\"/><w:bookmarkStart w:id=\"2\" w:name=\"PointMark\"/><w:bookmarkEnd w:id=\"2\"/><w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\"> italic</w:t><w:tab/><w:t>end</w:t></w:r><w:commentRangeEnd w:id=\"0\"/>",
            r(" middle")
        )),
        p(&format!(
            "<w:proofErr w:type=\"gramStart\"/><w:bookmarkStart w:id=\"3\" w:name=\"Lead\"/><w:bookmarkEnd w:id=\"3\"/>{}<w:br/>{}",
            r("Proof lead"),
            r("second line")
        )),
        p(&format!(
            "<w:bookmarkStart w:id=\"4\" w:name=\"Clause1\"/>{}<w:bookmarkEnd w:id=\"4\"/>",
            r("Named mark paragraph")
        )),
        p(&format!("<w:hyperlink w:anchor=\"Clause1\">{}</w:hyperlink>", r("Linked para"))),
        p(&format!("{}<w:r><w:sym w:font=\"Wingdings\" w:char=\"F0FC\"/></w:r>", r("Symbol para"))),
        format!(
            "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"4000\"/><w:gridCol w:w=\"4000\"/></w:tblGrid><w:tr><w:tc>{}</w:tc><w:tc>{}{}</w:tc></w:tr></w:tbl>",
            para("Cell para one"),
            para("Cell para two"),
            para("Cell para three")
        ),
        para("after table"),
        toc,
        p(&format!(
            "<w:pPr><w:pPrChange w:id=\"50\" w:author=\"Old\"><w:pPr/></w:pPrChange></w:pPr><w:ins w:id=\"41\" w:author=\"Old\" w:date=\"2020-01-01T00:00:00Z\">{}</w:ins>",
            r("Existing insert")
        )),
        format!(
            "<w:sdt><w:sdtPr><w:tag w:val=\"s\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>",
            para("In sdt para")
        ),
        para("Odd id para"),
        para("last para"),
        sect_for(3),
    ]
    .concat()
}

fn blocks_doc() -> Vec<u8> {
    package(
        &blocks_body(),
        &[
            styles(),
            numbering(NUMBERING_MAIN),
            header(&para("Header para")),
            settings("<w:zoom w:percent=\"100\"/><w:compat/><w:rsids/>"),
        ],
    )
}

fn fields_doc(settings_inner: &str, bookmark_id: &str) -> Vec<u8> {
    let body = [
        styled("Heading1", "Fields heading"),
        p(&format!(
            "<w:bookmarkStart w:id=\"{bookmark_id}\" w:name=\"DefinedTerm\"/>{}<w:bookmarkEnd w:id=\"{bookmark_id}\"/>",
            r("Defined term text")
        )),
        para("Reference: "),
        para("Figure placeholder"),
        p(&format!("<w:pPr><w:pStyle w:val=\"BodyText\"/><w:jc w:val=\"center\"/></w:pPr>{}", r("Styled caption target"))),
        p(&format!(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> TOC </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}",
            r("Open toc one")
        )),
        p(&format!("{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>", r("Open toc two"))),
        para("Closing para"),
        sect_for(3),
    ]
    .concat();
    package(
        &body,
        &[
            styles(),
            settings(settings_inner),
            header(&para("Header field para")),
        ],
    )
}

fn bare_doc(sect: &str) -> Vec<u8> {
    package(
        &[
            para("Alpha paragraph"),
            para("Beta paragraph"),
            sect.to_string(),
        ]
        .concat(),
        &[],
    )
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    vec![
        ("p2-numbering".to_string(), numbering_doc()),
        (
            "p2-numbering-malformed".to_string(),
            package(
                &[numbered("1", "0", "Malformed numbered"), para("Plain")].concat(),
                &[
                    styles(),
                    numbering(
                        "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"/></w:abstractNum><w:num w:numId=\"x1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
                    ),
                ],
            ),
        ),
        (
            "p2-numbering-missing".to_string(),
            package(
                &[numbered("9", "0", "Dangling numbered"), para("Plain")].concat(),
                &[
                    styles(),
                    numbering(
                        "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"/></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
                    ),
                ],
            ),
        ),
        (
            "p2-numbering-level".to_string(),
            package(
                &[numbered("1", "5", "Deep numbered"), para("Plain")].concat(),
                &[
                    styles(),
                    numbering(
                        "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"/></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
                    ),
                ],
            ),
        ),
        (
            "p2-numbering-canonical".to_string(),
            package(
                &[para("Canon target"), para("Plain")].concat(),
                &[
                    styles(),
                    numbering(&format!(
                        "{}<w:num w:numId=\"3\"><w:abstractNumId w:val=\"7\"/></w:num>",
                        canonical_bullet("7")
                    )),
                ],
            ),
        ),
        ("p2-blocks".to_string(), blocks_doc()),
        ("p2-bare".to_string(), bare_doc("")),
        (
            "p2-measure".to_string(),
            bare_doc(
                "<w:sectPr><w:pgSz w:w=\"8.5in\"/><w:pgMar w:left=\"1in\" w:right=\"25.4mm\"/></w:sectPr>",
            ),
        ),
        (
            "p2-halfsect".to_string(),
            bare_doc(
                "<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"11906\"/><w:pgMar w:left=\"1000\" w:right=\"1000\"/></w:sectPr></w:pPr></w:p><w:sectPr><w:pgSz w:w=\"15000\"/></w:sectPr>",
            ),
        ),
        (
            "p2-fields".to_string(),
            fields_doc(
                "<w:zoom w:percent=\"100\"/><w:compat/><w:rsids/><m:mathPr/>",
                "1",
            ),
        ),
        (
            "p2-fields-armed".to_string(),
            fields_doc("<w:updateFields w:val=\"false\"/><w:compat/>", "1"),
        ),
        (
            "p2-fields-badmark".to_string(),
            fields_doc("<w:compat/>", "x"),
        ),
    ]
}

/// The canonical `PaperBulletList` abstract (as upstream writes it).
fn canonical_bullet(abstract_id: &str) -> String {
    format!(
        "<w:abstractNum w:abstractNumId=\"{abstract_id}\"><w:name w:val=\"PaperBulletList\"/><w:multiLevelType w:val=\"hybridMultilevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{2022}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\" w:hint=\"default\"/></w:rPr></w:lvl><w:lvl w:ilvl=\"1\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"o\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"1440\" w:hanging=\"360\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\" w:hint=\"default\"/></w:rPr></w:lvl><w:lvl w:ilvl=\"2\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{25aa}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"2160\" w:hanging=\"360\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Wingdings\" w:hAnsi=\"Wingdings\" w:hint=\"default\"/></w:rPr></w:lvl></w:abstractNum>"
    )
}
