//! Synthetic documents for work package p7 (M7: the remaining inherited
//! python-docx API). Most P7 cases use upstream's Behave test documents from
//! the pinned checkout (`features/steps/test_files`, read at run time, not
//! copied); these documents cover structures those files lack: shared
//! header definitions, merges that fail half-way, unknown markup next to
//! edited properties, and universal-measure / hanging-indent values.

use crate::hostile::build::{DECLS, RawMember, build_zip};

const CT_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#;
const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
const HEADER_RT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const FOOTER_RT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
const HEADER_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
const FOOTER_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";

fn document(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {DECLS}><w:body>{body}</w:body></w:document>"
    )
}

fn story(root: &str, text: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:{root} {DECLS}><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:{root}>"
    )
}

fn rels(entries: &[(&str, &str, &str)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for (id, kind, target) in entries {
        out.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{kind}\" Target=\"{target}\"/>"
        ));
    }
    out.push_str("</Relationships>");
    out
}

fn package(
    document_xml: &str,
    extra: &[(&str, String, Option<&str>)],
    doc_rels: Option<String>,
) -> Vec<u8> {
    let mut ct = String::from(CT_HEAD);
    for (name, _, content_type) in extra {
        if let Some(content_type) = content_type {
            ct.push_str(&format!(
                "<Override PartName=\"/{name}\" ContentType=\"{content_type}\"/>"
            ));
        }
    }
    ct.push_str("</Types>");
    let mut members = vec![
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document_xml.as_bytes()),
    ];
    if let Some(doc_rels) = doc_rels {
        members.push(RawMember::new(
            "word/_rels/document.xml.rels",
            doc_rels.as_bytes(),
        ));
    }
    for (name, data, _) in extra {
        members.push(RawMember::new(name, data.as_bytes()));
    }
    build_zip(&members)
}

/// Two sections whose default headers share one relationship (`rId7`), a
/// third section with its own header and a footer used by the first
/// section only; universal measures and a `w:titlePg w:val="0"`.
fn shared_header() -> Vec<u8> {
    let sect = |refs: &str, extra: &str| {
        format!(
            "<w:sectPr>{refs}<w:pgSz w:w=\"8.5in\" w:h=\"15840\"/><w:pgMar w:top=\"1440.6\" w:right=\"2.54cm\" w:bottom=\"-720\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/>{extra}</w:sectPr>"
        )
    };
    let first = sect(
        "<w:headerReference w:type=\"default\" r:id=\"rId7\"/><w:footerReference w:type=\"default\" r:id=\"rId8\"/>",
        "<w:titlePg w:val=\"0\"/>",
    );
    let second = sect(
        "<w:headerReference w:type=\"default\" r:id=\"rId7\"/>",
        "<w:type w:val=\"continuous\"/>",
    );
    let third = sect(
        "<w:headerReference w:type=\"default\" r:id=\"rId9\"/><w:headerReference w:type=\"even\" r:id=\"rId9\"/>",
        "<x:custom x:keep=\"1\"/>",
    );
    let body = format!(
        "<w:p><w:pPr>{first}</w:pPr><w:r><w:t>section one</w:t></w:r></w:p><w:p><w:pPr>{second}</w:pPr><w:r><w:t>section two</w:t></w:r></w:p><w:p><w:r><w:t>section three</w:t></w:r></w:p>{third}"
    );
    package(
        &document(&body),
        &[
            (
                "word/header1.xml",
                story("hdr", "shared header"),
                Some(HEADER_CT),
            ),
            (
                "word/footer1.xml",
                story("ftr", "first footer"),
                Some(FOOTER_CT),
            ),
            (
                "word/header2.xml",
                story("hdr", "third header"),
                Some(HEADER_CT),
            ),
        ],
        Some(rels(&[
            ("rId7", HEADER_RT, "header1.xml"),
            ("rId8", FOOTER_RT, "footer1.xml"),
            ("rId9", HEADER_RT, "header2.xml"),
        ])),
    )
}

fn cell(text: &str, props: &str) -> String {
    let p = if text.is_empty() {
        "<w:p/>".to_string()
    } else {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    };
    format!("<w:tc><w:tcPr><w:tcW w:w=\"1440\" w:type=\"dxa\"/>{props}</w:tcPr>{p}</w:tc>")
}

/// Tables for merge edge cases and property edits next to unknown markup:
/// table 0 is ragged (row 1 lacks two cells), table 1 carries unknown
/// elements in `w:tblPr`, `w:trPr` and `w:tcPr` plus a `w:tblPrEx` row,
/// table 2 has an empty cell without any block content.
fn tables() -> Vec<u8> {
    let grid = |n: usize| {
        format!(
            "<w:tblGrid>{}</w:tblGrid>",
            "<w:gridCol w:w=\"1440\"/>".repeat(n)
        )
    };
    let ragged = format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>{}<w:tr>{}{}{}</w:tr><w:tr><w:trPr><w:gridAfter w:val=\"2\"/></w:trPr>{}</w:tr></w:tbl>",
        grid(3),
        cell("a", ""),
        cell("b", ""),
        cell("c", ""),
        cell("d", ""),
    );
    let unknown = format!(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><x:tblExt x:a=\"1\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>{}<w:tr><w:tblPrEx><w:tblBorders/></w:tblPrEx>{}{}</w:tr><w:tr><w:trPr><x:rowExt/><w:cantSplit/></w:trPr>{}{}</w:tr></w:tbl>",
        grid(2),
        cell("e", "<x:cellExt/>"),
        cell("f", "<w:vAlign w:val=\"bottom\"/>"),
        cell("g", ""),
        cell("h", ""),
    );
    let empty = format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>{}<w:tr><w:tc><w:tcPr><w:tcW w:w=\"1440\" w:type=\"dxa\"/></w:tcPr></w:tc>{}</w:tr></w:tbl>",
        grid(2),
        cell("j", ""),
    );
    let body = format!(
        "{ragged}<w:p/>{unknown}<w:p/>{empty}<w:p/><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr>"
    );
    package(&document(&body), &[], None)
}

/// Paragraphs with direct formatting the Behave documents lack: a hanging
/// indent, universal-measure spacing, `atLeast` line spacing, tab stops
/// around a comment, and runs with theme / auto colors next to unknown
/// run properties.
fn text() -> Vec<u8> {
    let body = concat!(
        "<w:p><w:pPr><w:spacing w:before=\"12pt\" w:line=\"300\" w:lineRule=\"atLeast\"/><w:ind w:left=\"720.4\" w:hanging=\"360\"/><w:jc w:val=\"both\"/></w:pPr><w:r><w:t>hanging</w:t></w:r></w:p>",
        "<w:p><w:pPr><w:keepNext w:val=\"off\"/><w:tabs><w:tab w:val=\"left\" w:pos=\"720\"/><!--keep--><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"-1440\"/></w:tabs><w:spacing w:line=\"360\"/></w:pPr><w:r><w:t>tabs</w:t></w:r></w:p>",
        "<w:p><w:pPr><x:pExt/></w:pPr><w:r><w:rPr><w:b w:val=\"false\"/><w:color w:val=\"auto\"/><x:rExt/><w:sz w:val=\"25\"/><w:u/></w:rPr><w:t>auto color</w:t></w:r><w:r><w:rPr><w:color w:val=\"1F497D\" w:themeColor=\"text2\"/><w:highlight w:val=\"darkCyan\"/><w:vertAlign w:val=\"superscript\"/></w:rPr><w:t>theme</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>plain</w:t></w:r></w:p>",
        "<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr>"
    );
    package(&document(body), &[], None)
}

/// Hyperlink retargeting: an internal jump, two hyperlinks sharing one
/// relationship, an external link with a fragment next to a plain one, and
/// a hyperlink whose `r:id` names no relationship.
fn hyperlinks() -> Vec<u8> {
    let link = |attrs: &str, text: &str| {
        format!("<w:hyperlink {attrs}><w:r><w:t>{text}</w:t></w:r></w:hyperlink>")
    };
    let body = format!(
        "<w:p><w:bookmarkStart w:id=\"0\" w:name=\"target\"/><w:r><w:t>jump: </w:t></w:r>{}<w:bookmarkEnd w:id=\"0\"/></w:p><w:p>{}{}</w:p><w:p>{}{}</w:p><w:p>{}</w:p><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr>",
        link("w:anchor=\"target\" w:history=\"1\"", "internal"),
        link("r:id=\"rId3\"", "shared one"),
        link("r:id=\"rId3\"", "shared two"),
        link("r:id=\"rId4\" w:anchor=\"frag\"", "with fragment"),
        link("r:id=\"rId5\"", "plain"),
        link("r:id=\"rId99\"", "dangling"),
    );
    let hyperlink = |id: &str, target: &str| {
        format!(
            "<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{target}\" TargetMode=\"External\"/>"
        )
    };
    let doc_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}{}{}</Relationships>",
        hyperlink("rId3", "http://a.example/"),
        hyperlink("rId4", "http://b.example/page"),
        hyperlink("rId5", "http://c.example/"),
    );
    package(&document(&body), &[], Some(doc_rels))
}

/// Every XML part of `package` re-serialized without blank text
/// (python-docx `remove_blank_text` semantics, see `parts::template`), so a
/// pretty-printed upstream document can be compared part by part.
fn compact(package: &[u8]) -> Option<Vec<u8>> {
    let contents =
        docrs::package::zip_read::read_archive(package, &docrs::Limits::default()).ok()?;
    let mut members = Vec::new();
    for member in &contents.members {
        if member.name.ends_with(".xml") || member.name.ends_with(".rels") {
            let tree = docrs::parts::template(&member.data).ok()?;
            members.push(RawMember::new(&member.name, &docrs::xml::serialize(&tree)));
        } else {
            members.push(RawMember::new(&member.name, &member.data));
        }
    }
    Some(build_zip(&members))
}

/// Every Behave test document (`features/steps/test_files/*.docx`, read
/// from the pinned checkout at run time; none when the checkout is not
/// linked) re-serialized without blank text, as `p7-compact-<stem>`.
/// Upstream re-serializes every part it loads without inter-element
/// whitespace, so only blank-text-free inputs compare part by part.
fn compacted() -> &'static [(String, Vec<u8>)] {
    static CACHE: std::sync::OnceLock<Vec<(String, Vec<u8>)>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        let dir =
            crate::oracle::repo_root().join(".reference/paper-docx/features/steps/test_files");
        let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|e| e.path()))
                    .filter(|path| path.extension().is_some_and(|ext| ext == "docx"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
            .into_iter()
            .filter_map(|path| {
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                let bytes = compact(&std::fs::read(&path).ok()?)?;
                Some((format!("p7-compact-{stem}"), bytes))
            })
            .collect()
    })
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    let mut out = vec![
        ("p7-shared-header".to_string(), shared_header()),
        ("p7-tables".to_string(), tables()),
        ("p7-text".to_string(), text()),
        ("p7-hyperlinks".to_string(), hyperlinks()),
    ];
    out.extend(compacted().iter().cloned());
    out
}
