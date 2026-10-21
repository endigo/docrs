//! Synthetic documents for M4 work package p4 (pictures, media naming,
//! whitespace-sensitive authoring). Every image is generated here from code
//! (tiny PNG/GIF bytes), never committed as an opaque binary.

use crate::hostile::build::{DECLS, RawMember, build_zip};

const CT_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#;
const MAIN_CT: &str = r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#;
const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
const IMAGE_RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const DRAWING_NS: &str = "xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";

/// A 1×1 PNG (upstream `TINY_PNG`, `tests/paper/test_composition.py`).
pub const TINY_PNG_HEX: &str = "89504e470d0a1a0a0000000d494844520000000100000001080200000090775\
3de0000000c4944415408d763f8cfc000000301010018dd8db00000000049454e44ae426082";

/// A 5×7 GIF89a.
pub const GIF_HEX: &str = "47494638396105000700800000000000ffffff2c00000000010001000002024401003b";

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn rels(entries: &[(&str, &str, &str, bool)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for (id, kind, target, external) in entries {
        out.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{kind}\" Target=\"{target}\"{}/>",
            if *external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        ));
    }
    out.push_str("</Relationships>");
    out
}

fn inline_pic(id: u32, blip: &str, frame: &str) -> String {
    let (open, close) = if frame == "anchor" {
        (
            "<wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"0\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"column\"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>0</wp:posOffset></wp:positionV>",
            "</wp:anchor>",
        )
    } else {
        ("<wp:inline>", "</wp:inline>")
    };
    format!(
        "<w:r><w:drawing>{open}<wp:extent cx=\"457200\" cy=\"228600\"/><wp:docPr id=\"{id}\" name=\"Picture {id}\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"0\" name=\"x.png\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip {blip}/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"457200\" cy=\"228600\"/></a:xfrm><a:prstGeom prst=\"rect\"/></pic:spPr></pic:pic></a:graphicData></a:graphic>{close}</w:drawing></w:r>"
    )
}

fn chart_drawing() -> String {
    "<w:r><w:drawing><wp:inline><wp:extent cx=\"100\" cy=\"100\"/><wp:docPr id=\"40\" name=\"Chart 1\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><x:chart/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>".to_string()
}

fn document(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {DECLS} {DRAWING_NS}><w:body>{body}</w:body></w:document>"
    )
}

fn para(text: &str, extra: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r>{extra}</w:p>")
}

fn drawings_package(protected: Option<&str>, png_type: &str) -> Vec<u8> {
    let body = [
        para("inline picture", &inline_pic(1, "r:embed=\"rId10\"", "inline")),
        para("anchored picture", &inline_pic(2, "r:embed=\"rId10\"", "anchor")),
        para("chart", &chart_drawing()),
        para("linked picture", &inline_pic(3, "r:link=\"rId11\"", "inline")),
        para("dangling picture", &inline_pic(4, "r:embed=\"rId99\"", "inline")),
        "<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId20\"/><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr>".to_string(),
    ]
    .concat();
    let header = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:hdr {DECLS} {DRAWING_NS}>{}</w:hdr>",
        para(
            "header picture",
            &inline_pic(5, "r:embed=\"rId1\"", "inline")
        )
    );
    let mut doc_rels = vec![
        ("rId10", IMAGE_RT, "media/image1.png", false),
        ("rId11", IMAGE_RT, "http://example.com/linked.png", true),
        (
            "rId20",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header",
            "header1.xml",
            false,
        ),
    ];
    let mut ct = format!(
        "{CT_HEAD}<Default Extension=\"png\" ContentType=\"{png_type}\"/><Default Extension=\"gif\" ContentType=\"image/gif\"/>{MAIN_CT}<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>"
    );
    let mut members = vec![
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document(&body).as_bytes()),
        RawMember::new("word/header1.xml", header.as_bytes()),
        RawMember::new(
            "word/_rels/header1.xml.rels",
            rels(&[("rId1", IMAGE_RT, "media/image2.gif", false)]).as_bytes(),
        ),
        RawMember::new("word/media/image1.png", &hex(TINY_PNG_HEX)),
        RawMember::new("word/media/image2.gif", &hex(GIF_HEX)),
    ];
    if let Some(edit) = protected {
        doc_rels.push((
            "rId30",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings",
            "settings.xml",
            false,
        ));
        ct.push_str("<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>");
        let settings = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:documentProtection w:edit=\"{edit}\" w:enforcement=\"1\"/></w:settings>"
        );
        members.push(RawMember::new("word/settings.xml", settings.as_bytes()));
    }
    ct.push_str("</Types>");
    members.insert(0, RawMember::new("[Content_Types].xml", ct.as_bytes()));
    members.push(RawMember::new(
        "word/_rels/document.xml.rels",
        rels(&doc_rels).as_bytes(),
    ));
    build_zip(&members)
}

/// Image parts with numbering gaps and a non-numbered name, plus a
/// thumbnail-style package image that no `image` relationship reaches.
fn media_gaps_package() -> Vec<u8> {
    let body = [
        para("third", &inline_pic(7, "r:embed=\"rId3\"", "inline")),
        para("named", &inline_pic(9, "r:embed=\"rId4\"", "inline")),
        "<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:left=\"1134\" w:right=\"1134\"/></w:sectPr>".to_string(),
    ]
    .concat();
    let ct = format!(
        "{CT_HEAD}<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"gif\" ContentType=\"image/gif\"/>{MAIN_CT}</Types>"
    );
    let root_rels = rels(&[
        (
            "rId1",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
            "word/document.xml",
            false,
        ),
        (
            "rId2",
            "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail",
            "docProps/thumbnail.png",
            false,
        ),
    ]);
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", root_rels.as_bytes()),
        RawMember::new("word/document.xml", document(&body).as_bytes()),
        RawMember::new(
            "word/_rels/document.xml.rels",
            rels(&[
                ("rId3", IMAGE_RT, "media/image3.png", false),
                ("rId4", IMAGE_RT, "media/picture.gif", false),
            ])
            .as_bytes(),
        ),
        RawMember::new("word/media/image3.png", &hex(TINY_PNG_HEX)),
        RawMember::new("word/media/picture.gif", &hex(GIF_HEX)),
        RawMember::new("docProps/thumbnail.png", &hex(TINY_PNG_HEX)),
    ])
}

/// Pretty-printed WordprocessingML: whitespace text between elements,
/// a universal-measure page width, rsid attributes on `w:sectPr`, a merged
/// table and a header reference (lxml tail semantics, `_block_width`,
/// `add_section`, `Table.cell`).
fn whitespace_package() -> Vec<u8> {
    let body = "\n    <w:p>\n      <w:pPr>\n        <w:jc w:val=\"center\"/>\n      </w:pPr>\n      <w:r>\n        <w:rPr>\n          <w:b/>\n        </w:rPr>\n        <w:t>Spaced bold</w:t>\n      </w:r>\n      <w:r>\n        <w:t>tail run</w:t>\n      </w:r>\n    </w:p>\n    <w:p>\n      <w:r>\n        <w:t>No props</w:t>\n      </w:r>\n    </w:p>\n    <w:p><!-- keep me --><w:r><w:t>Commented</w:t></w:r><?pi data?></w:p>\n    <w:tbl>\n      <w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>\n      <w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"3000\"/><w:gridCol w:w=\"1in\"/></w:tblGrid>\n      <w:tr><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr><w:p><w:r><w:t>span</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:vMerge w:val=\"restart\"/></w:tcPr><w:p><w:r><w:t>top</w:t></w:r></w:p></w:tc></w:tr>\n      <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr>\n    </w:tbl>\n    <w:sectPr w:rsidR=\"00A1\" w:rsidSect=\"00B2\">\n      <w:headerReference w:type=\"default\" r:id=\"rId8\"/>\n      <w:type w:val=\"oddPage\"/>\n      <w:pgSz w:w=\"8.5in\" w:h=\"15840\"/>\n      <w:pgMar w:left=\"1800\" w:right=\"0\"/>\n    </w:sectPr>\n  ";
    let ct = format!(
        "{CT_HEAD}{MAIN_CT}<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/></Types>"
    );
    let header = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:hdr {DECLS}>{}</w:hdr>",
        para("Header text", "")
    );
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document(body).as_bytes()),
        RawMember::new(
            "word/_rels/document.xml.rels",
            rels(&[(
                "rId8",
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header",
                "header1.xml",
                false,
            )])
            .as_bytes(),
        ),
        RawMember::new("word/header1.xml", header.as_bytes()),
    ])
}

/// The blank document (`Document()` / bundled `default.docx`) with the
/// pretty-printing whitespace of its XML parts removed the way
/// python-docx's `remove_blank_text` parser drops it on load. Upstream never
/// keeps that whitespace once it edits the part, while this port preserves
/// it, so parity cases on "a blank document" use this variant.
fn blank_package() -> Vec<u8> {
    let (bytes, _) = docrs::Document::new()
        .patch_bytes()
        .expect("blank document serializes");
    compact(&bytes)
}

/// Every XML part of `package` re-serialized without blank text
/// (python-docx `remove_blank_text` semantics, see `parts::template`).
fn compact(package: &[u8]) -> Vec<u8> {
    let contents = docrs::package::zip_read::read_archive(package, &docrs::Limits::default())
        .expect("synthetic package reads");
    let members: Vec<RawMember> = contents
        .members
        .iter()
        .map(|member| {
            if member.name.ends_with(".xml") {
                let tree = docrs::parts::template(&member.data).expect("XML part parses");
                RawMember::new(&member.name, &docrs::xml::serialize(&tree))
            } else {
                RawMember::new(&member.name, &member.data)
            }
        })
        .collect();
    build_zip(&members)
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    vec![
        ("p4-blank".into(), blank_package()),
        ("p4-drawings".into(), drawings_package(None, "image/png")),
        (
            "p4-drawings-readonly".into(),
            drawings_package(Some("readOnly"), "image/png"),
        ),
        (
            "p4-drawings-tracked".into(),
            drawings_package(Some("trackedChanges"), "image/png"),
        ),
        (
            "p4-octet-image".into(),
            drawings_package(None, "application/octet-stream"),
        ),
        ("p4-media-gaps".into(), media_gaps_package()),
        ("p4-whitespace".into(), whitespace_package()),
        (
            "p4-whitespace-compact".into(),
            compact(&whitespace_package()),
        ),
    ]
}
