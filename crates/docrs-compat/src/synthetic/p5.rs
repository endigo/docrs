//! Synthetic documents for work package p5 (M5 review workflows).
//!
//! Each document reproduces, as a file, the in-memory XML an upstream test
//! builds before resolving revisions or operating on comments (the tests
//! named next to each document). Derived documents edit an upstream fixture
//! textually and keep every other member byte-identical.

use crate::hostile::build::{CT_XML, ROOT_RELS, RawMember, build_zip};
use crate::hostile::fixtures_dir;
use docrs::Limits;
use docrs::package::zip_read::read_archive;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const DECLS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" \
xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" \
xmlns:o=\"urn:schemas-microsoft-com:office:office\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";

const CT_COMMENTS: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const CT_EXTENDED: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml";
const CT_IDS: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsIds+xml";
const CT_EXTENSIBLE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtensible+xml";
const CT_SETTINGS: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
const CT_FOOTNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";

const RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RT_EXTENDED: &str = "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
const RT_IDS: &str = "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds";
const RT_EXTENSIBLE: &str =
    "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible";

const W15_DECL: &str = "xmlns:w15=\"http://schemas.microsoft.com/office/word/2012/wordml\"";
const W16CID_DECL: &str =
    "xmlns:w16cid=\"http://schemas.microsoft.com/office/word/2016/wordml/cid\"";
const W16CEX_DECL: &str =
    "xmlns:w16cex=\"http://schemas.microsoft.com/office/word/2018/wordml/cex\"";

/// One extra package part.
struct Part {
    name: &'static str,
    content_type: Option<&'static str>,
    data: Vec<u8>,
}

fn xml_part(name: &'static str, content_type: &'static str, xml: String) -> Part {
    Part {
        name,
        content_type: Some(content_type),
        data: xml.into_bytes(),
    }
}

/// A package: `body` inside `w:document/w:body`, extra parts, and main-part
/// relationships `(id, type suffix or full URI, target, external)`.
fn package(
    body: &str,
    parts: Vec<Part>,
    rels: &[(&str, &str, &str, bool)],
    defaults: &[(&str, &str)],
) -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {DECLS}><w:body>{body}</w:body></w:document>"
    );
    let mut types = String::new();
    for (ext, ct) in defaults {
        types.push_str(&format!(
            "<Default Extension=\"{ext}\" ContentType=\"{ct}\"/>"
        ));
    }
    for part in &parts {
        if let Some(ct) = part.content_type {
            types.push_str(&format!(
                "<Override PartName=\"/{}\" ContentType=\"{ct}\"/>",
                part.name
            ));
        }
    }
    let ct_xml = CT_XML.replace("</Types>", &format!("{types}</Types>"));
    let mut members = vec![
        RawMember::new("[Content_Types].xml", ct_xml.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
    ];
    if !rels.is_empty() {
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (id, kind, target, external) in rels {
            let kind = if kind.contains("://") {
                kind.to_string()
            } else {
                format!("{RT}/{kind}")
            };
            let mode = if *external {
                " TargetMode=\"External\""
            } else {
                ""
            };
            xml.push_str(&format!(
                "<Relationship Id=\"{id}\" Type=\"{kind}\" Target=\"{target}\"{mode}/>"
            ));
        }
        xml.push_str("</Relationships>");
        members.push(RawMember::new(
            "word/_rels/document.xml.rels",
            xml.as_bytes(),
        ));
    }
    for part in parts {
        members.push(RawMember::new(part.name, &part.data));
    }
    build_zip(&members)
}

fn body_only(body: &str) -> Vec<u8> {
    package(body, Vec::new(), &[], &[])
}

/// An upstream fixture with textual edits `(member, from, to)` applied (each
/// `from` must occur) and extra members appended.
fn derive(fixture: &str, edits: &[(&str, &str, &str)], extra: Vec<(String, Vec<u8>)>) -> Vec<u8> {
    let bytes =
        std::fs::read(fixtures_dir().join(fixture)).unwrap_or_else(|e| panic!("{fixture}: {e}"));
    let archive =
        read_archive(&bytes, &Limits::default()).unwrap_or_else(|e| panic!("{fixture}: {e}"));
    let mut members: Vec<RawMember> = Vec::new();
    for member in &archive.members {
        let mut data = member.data.clone();
        for (name, from, to) in edits {
            if *name == member.name {
                let text = String::from_utf8(data).expect("utf-8 member");
                assert!(text.contains(from), "{fixture}:{name} lacks {from:?}");
                data = text.replacen(from, to, 1).into_bytes();
            }
        }
        members.push(RawMember::new(&member.name, &data));
    }
    for (name, data) in extra {
        members.push(RawMember::new(&name, &data));
    }
    build_zip(&members)
}

fn p(inner: &str) -> String {
    format!("<w:p>{inner}</w:p>")
}

fn r(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn dr(text: &str) -> String {
    format!("<w:r><w:delText xml:space=\"preserve\">{text}</w:delText></w:r>")
}

fn rev(tag: &str, id: u32, author: &str, inner: &str) -> String {
    format!(
        "<w:{tag} w:id=\"{id}\" w:author=\"{author}\" w:date=\"2026-06-01T09:30:00Z\">{inner}</w:{tag}>"
    )
}

fn table(rows: &[String]) -> String {
    format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"4675\"/></w:tblGrid>{}</w:tbl>",
        rows.concat()
    )
}

fn row(tr_pr: &str, cell: &str) -> String {
    format!(
        "<w:tr>{tr_pr}<w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/></w:tcPr>{cell}</w:tc></w:tr>"
    )
}

fn reference(id: &str) -> String {
    format!(
        "<w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"{id}\"/></w:r>"
    )
}

/// A `w:comment` with one paragraph (`para_id` on it when given).
fn comment_xml(id: &str, author: &str, text: &str, para_id: Option<&str>) -> String {
    let para = para_id
        .map(|v| format!(" w14:paraId=\"{v}\""))
        .unwrap_or_default();
    format!(
        "<w:comment w:id=\"{id}\" w:author=\"{author}\" w:initials=\"AU\" w:date=\"2026-06-03T10:15:00Z\"><w:p{para}><w:pPr><w:pStyle w:val=\"CommentText\"/></w:pPr><w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:annotationRef/></w:r><w:r><w:t>{text}</w:t></w:r></w:p></w:comment>"
    )
}

fn comments_part(inner: &str) -> Part {
    xml_part(
        "word/comments.xml",
        CT_COMMENTS,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:comments {DECLS}>{inner}</w:comments>"
        ),
    )
}

fn extended_part(entries: &[(&str, Option<&str>, &str)]) -> Part {
    let rows: String = entries
        .iter()
        .map(|(para, parent, done)| {
            let parent = parent
                .map(|v| format!(" w15:paraIdParent=\"{v}\""))
                .unwrap_or_default();
            format!("<w15:commentEx w15:paraId=\"{para}\"{parent} w15:done=\"{done}\"/>")
        })
        .collect();
    xml_part(
        "word/commentsExtended.xml",
        CT_EXTENDED,
        format!("<w15:commentsEx {W15_DECL}>{rows}</w15:commentsEx>"),
    )
}

fn ids_part(para_ids: &[&str]) -> Part {
    let rows: String = para_ids
        .iter()
        .map(|v| format!("<w16cid:commentId w16cid:paraId=\"{v}\" w16cid:durableId=\"{v}\"/>"))
        .collect();
    xml_part(
        "word/commentsIds.xml",
        CT_IDS,
        format!("<w16cid:commentsIds {W16CID_DECL}>{rows}</w16cid:commentsIds>"),
    )
}

fn extensible_part(para_ids: &[&str]) -> Part {
    let rows: String = para_ids
        .iter()
        .map(|v| format!("<w16cex:commentExtensible w16cex:durableId=\"{v}\"/>"))
        .collect();
    xml_part(
        "word/commentsExtensible.xml",
        CT_EXTENSIBLE,
        format!("<w16cex:commentsExtensible {W16CEX_DECL}>{rows}</w16cex:commentsExtensible>"),
    )
}

const COMMENT_RELS: &[(&str, &str, &str, bool)] = &[
    ("rId1", "comments", "comments.xml", false),
    ("rId2", RT_EXTENDED, "commentsExtended.xml", false),
    ("rId3", RT_IDS, "commentsIds.xml", false),
    ("rId4", RT_EXTENSIBLE, "commentsExtensible.xml", false),
];

/// A one-pixel 24-bit BMP.
fn bmp(red: u8, green: u8, blue: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&58u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[blue, green, red, 0]);
    out
}

fn picture_run(r_id: &str) -> String {
    format!(
        "<w:r><w:drawing><wp:inline><wp:extent cx=\"9525\" cy=\"9525\"/><wp:docPr id=\"1\" name=\"Picture\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"0\" name=\"image\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{r_id}\"/></pic:blipFill><pic:spPr/></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )
}

fn protected_settings(edit: &str) -> Part {
    xml_part(
        "word/settings.xml",
        CT_SETTINGS,
        format!(
            "<w:settings xmlns:w=\"{W}\"><w:documentProtection w:edit=\"{edit}\" w:enforcement=\"1\"/></w:settings>"
        ),
    )
}

const MOVES: &str = "generated/feature-isolated/tracked-moves.docx";
const NOTES: &str = "generated/feature-isolated/footnotes-endnotes.docx";
const ROWS: &str = "generated/feature-isolated/row-revisions.docx";
const DOC: &str = "word/document.xml";

fn revision_documents() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, bytes: Vec<u8>| out.push((name.to_string(), bytes));

    // test_revisions.py::it_keeps_story_level_section_changes_as_inert_evidence
    add(
        "p5-sect-change",
        body_only(&format!(
            "{}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:sectPrChange w:id=\"991\" w:author=\"A\"><w:sectPr/></w:sectPrChange></w:sectPr>",
            p(&r("Body text"))
        )),
    );
    // test_resolution.py::DescribeExoticTypesStayRefused (one doc per type)
    let cell = p(&r("cell"));
    add(
        "p5-exotic-tblpr",
        body_only(&format!(
            "{}<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblPrChange w:id=\"900\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"><w:tblPr/></w:tblPrChange></w:tblPr><w:tblGrid><w:gridCol w:w=\"4675\"/></w:tblGrid>{}</w:tbl>{}",
            p(&r("Before")),
            row("", &cell),
            p(&r("After"))
        )),
    );
    add(
        "p5-exotic-tcpr",
        body_only(&table(&[format!(
            "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/><w:tcPrChange w:id=\"901\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"><w:tcPr/></w:tcPrChange></w:tcPr>{cell}</w:tc></w:tr>"
        )])),
    );
    add(
        "p5-exotic-cellmerge",
        body_only(&table(&[format!(
            "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/><w:cellMerge w:id=\"902\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"/></w:tcPr>{cell}</w:tc></w:tr>"
        )])),
    );
    // Every remaining exotic type in one document (census and refusal).
    add(
        "p5-exotic-mix",
        body_only(&format!(
            "{}{}{}<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/><w:numberingChange w:id=\"905\" w:author=\"B\" w:original=\"1\"/></w:numPr></w:pPr>{}</w:p><w:p><w:customXmlInsRangeStart w:id=\"906\" w:author=\"B\"/>{}<w:customXmlInsRangeEnd w:id=\"906\"/></w:p>{}",
            p(&rev("ins", 903, "A", &r("plain insertion"))),
            table(&[
                format!(
                    "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/><w:cellIns w:id=\"907\" w:author=\"B\"/></w:tcPr>{cell}</w:tc></w:tr>"
                ),
                format!(
                    "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/><w:cellDel w:id=\"908\" w:author=\"B\"/></w:tcPr>{cell}</w:tc></w:tr>"
                )
            ]),
            p(&r("between")),
            r("numbered"),
            r("custom xml"),
            "<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr>"
        )),
    );
    // Upstream classifies any w:trPr child revision as a row marker, so a
    // w:trPrChange resolves like a row deletion (parity quirk, pinned).
    add(
        "p5-trprchange",
        body_only(&format!(
            "{}{}",
            p(&r("Before")),
            table(&[
                row("", &p(&r("kept row"))),
                row(
                    "<w:trPr><w:cantSplit/><w:trPrChange w:id=\"909\" w:author=\"A\"><w:trPr/></w:trPrChange></w:trPr>",
                    &p(&r("changed row")),
                ),
            ])
        )),
    );
    // test_revisions.py::it_reports_a_census... (moves + a tblPrChange)
    add(
        "p5-moves-plus-tblpr",
        derive(
            MOVES,
            &[(
                DOC,
                "<w:sectPr",
                "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblPrChange w:id=\"991\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"><w:tblPr/></w:tblPrChange></w:tblPr><w:tblGrid><w:gridCol w:w=\"4675\"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:sectPr",
            )],
            Vec::new(),
        ),
    );
    // test_audit_revisions.py::it_refuses_stale_revision_and_revisions_snapshots
    add(
        "p5-two-insertions",
        body_only(&p(&format!(
            "{}{}",
            rev("ins", 10, "Audit", &r("first")),
            rev("ins", 11, "Audit", &r("second"))
        ))),
    );
    // test_audit_revisions.py::it_prevalidates_malformed_comment_reference_ids
    add(
        "p5-malformed-ref-accept",
        body_only(&p(&format!(
            "{}{}",
            rev("ins", 1, "Audit", &r("kept")),
            rev(
                "del",
                2,
                "Audit",
                "<w:r><w:commentReference w:id=\"not-a-number\"/></w:r>"
            )
        ))),
    );
    add(
        "p5-malformed-ref-reject",
        body_only(&p(&format!(
            "{}{}",
            rev("del", 1, "Audit", &dr("restored")),
            rev(
                "ins",
                2,
                "Audit",
                "<w:r><w:commentReference w:id=\"not-a-number\"/></w:r>"
            )
        ))),
    );
    // ...::it_prevalidates_malformed_comment_range_ids
    add(
        "p5-malformed-range-id",
        body_only(&p(&format!(
            "<w:commentRangeStart w:id=\"broken\"/>{}",
            rev(
                "ins",
                3,
                "Audit",
                "<w:r><w:commentReference w:id=\"7\"/></w:r>"
            )
        ))),
    );
    // ...::it_refuses_a_mixed_author_move_unit_before_mutation
    add(
        "p5-mixed-author-move",
        derive(
            MOVES,
            &[
                (
                    DOC,
                    "<w:moveFrom w:id=\"62\" w:author=\"Alice Editor\"",
                    "<w:moveFrom w:id=\"62\" w:author=\"Alice\"",
                ),
                (
                    DOC,
                    "<w:moveTo w:id=\"64\" w:author=\"Alice Editor\"",
                    "<w:moveTo w:id=\"64\" w:author=\"Bob\"",
                ),
            ],
            Vec::new(),
        ),
    );
    // test_resolution.py::it_refuses_an_orphaned_move_atomically
    add(
        "p5-orphan-move",
        derive(
            MOVES,
            &[(
                DOC,
                "<w:p><w:moveToRangeStart w:id=\"63\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\" w:name=\"move1\"/><w:moveTo w:id=\"64\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\"><w:r><w:t>The indemnity clause relocated by tracked move.</w:t></w:r></w:moveTo><w:moveToRangeEnd w:id=\"63\"/></w:p>",
                "",
            )],
            Vec::new(),
        ),
    );
    // test_resolution.py::it_refuses_a_cross_story_move
    add(
        "p5-cross-story-move",
        derive(
            "generated/feature-isolated/header-footer-sections.docx",
            &[
                (
                    DOC,
                    "<w:p><w:r><w:t>Section one body paragraph.</w:t></w:r></w:p>",
                    "<w:moveFromRangeStart w:id=\"801\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\" w:name=\"crossMove\"/><w:p><w:r><w:t>Section one body paragraph.</w:t></w:r><w:moveFrom w:id=\"802\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"><w:r><w:t>wandering text</w:t></w:r></w:moveFrom></w:p><w:moveFromRangeEnd w:id=\"801\"/>",
                ),
                (
                    "word/header1.xml",
                    "<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr><w:r><w:t>Header for section one</w:t></w:r></w:p>",
                    "<w:moveToRangeStart w:id=\"803\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\" w:name=\"crossMove\"/><w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr><w:r><w:t>Header for section one</w:t></w:r><w:moveTo w:id=\"804\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"><w:r><w:t>wandering text</w:t></w:r></w:moveTo></w:p><w:moveToRangeEnd w:id=\"803\"/>",
                ),
            ],
            Vec::new(),
        ),
    );
    // test_resolution.py::it_removes_the_whole_table_when_its_last_row_resolves_away
    add(
        "p5-rows-deleted-only",
        derive(
            ROWS,
            &[
                (
                    DOC,
                    "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/></w:tcPr><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/></w:tcPr><w:p><w:r><w:t>Amount</w:t></w:r></w:p></w:tc></w:tr>",
                    "",
                ),
                (
                    DOC,
                    "<w:tr><w:trPr><w:ins w:id=\"81\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\"/></w:trPr><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/></w:tcPr><w:p><w:pPr><w:rPr><w:ins w:id=\"82\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\"/></w:rPr></w:pPr><w:ins w:id=\"83\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\"><w:r><w:t>Filing fee</w:t></w:r></w:ins></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w=\"4675\" w:type=\"dxa\"/></w:tcPr><w:p><w:pPr><w:rPr><w:ins w:id=\"84\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\"/></w:rPr></w:pPr><w:ins w:id=\"85\" w:author=\"Alice Editor\" w:date=\"2026-06-01T09:30:00Z\"><w:r><w:t>$100</w:t></w:r></w:ins></w:p></w:tc></w:tr>",
                    "",
                ),
            ],
            Vec::new(),
        ),
    );
    // A single-row table that is its cell's only block: an empty paragraph
    // takes its place when the row resolves away.
    add(
        "p5-nested-only-table",
        body_only(&table(&[row(
            "",
            &table(&[row(
                "<w:trPr><w:del w:id=\"70\" w:author=\"A\"/></w:trPr>",
                &p(&rev("del", 71, "A", &dr("gone"))),
            )]),
        )])),
    );
    // test_audit_revision_compounds.py::DescribeIndividualDestructiveClosure /
    // DescribeFilteredDestructiveClosure
    add(
        "p5-outer-del-nested-ins",
        body_only(&p(&rev(
            "del",
            20,
            "Alice",
            &format!("{}{}", dr("outer"), rev("ins", 21, "Bob", &r("nested"))),
        ))),
    );
    add(
        "p5-outer-ins-nested-del",
        body_only(&p(&rev(
            "ins",
            22,
            "Alice",
            &format!("{}{}", r("outer"), rev("del", 23, "Bob", &dr("nested"))),
        ))),
    );
    add(
        "p5-row-other-author",
        body_only(&format!(
            "{}{}",
            table(&[
                row("", "<w:p/>"),
                row(
                    "<w:trPr><w:del w:id=\"30\" w:author=\"Alice\"/></w:trPr>",
                    &p(&rev("ins", 31, "Bob", &r("nested"))),
                ),
            ]),
            p("")
        )),
    );
    add(
        "p5-join-other-author",
        body_only(&format!(
            "<w:p><w:pPr><w:rPr><w:del w:id=\"32\" w:author=\"Alice\"/><w:ins w:id=\"33\" w:author=\"Bob\"/></w:rPr></w:pPr>{}</w:p>{}{}",
            r("first"),
            p(&r("following")),
            p("")
        )),
    );
    add(
        "p5-row-marker-only-move",
        body_only(&format!(
            "{}{}",
            table(&[
                row("", "<w:p/>"),
                row(
                    "<w:trPr><w:del w:id=\"34\" w:author=\"Alice\"/></w:trPr>",
                    "<w:p><w:moveFromRangeStart w:id=\"35\" w:name=\"nested\" w:author=\"Bob\"/><w:moveFromRangeEnd w:id=\"35\"/><w:moveToRangeStart w:id=\"36\" w:name=\"nested\" w:author=\"Bob\"/><w:moveToRangeEnd w:id=\"36\"/></w:p>",
                ),
            ]),
            p("")
        )),
    );
    // DescribeMoveAndParagraphCompounds
    add(
        "p5-marker-only-move",
        body_only(&p(
            "<w:moveFromRangeStart w:id=\"40\" w:name=\"marker-only\"/><w:moveFromRangeEnd w:id=\"40\"/><w:moveToRangeStart w:id=\"41\" w:name=\"marker-only\"/><w:moveToRangeEnd w:id=\"41\"/>",
        )),
    );
    let join = |barrier: &str| {
        body_only(&format!(
            "<w:p><w:pPr><w:rPr><w:del w:id=\"50\" w:author=\"Audit\"/></w:rPr></w:pPr>{}</w:p>{barrier}{}",
            r("first"),
            p(&r("following"))
        ))
    };
    add(
        "p5-join-across-customxml",
        join(&format!("<w:customXml>{}</w:customXml>", p(&r("middle")))),
    );
    add(
        "p5-join-across-altchunk",
        join("<w:altChunk r:id=\"rIdMissing\"/>"),
    );
    add(
        "p5-join-across-table",
        join(&table(&[row("", &p(&r("cell")))])),
    );
    // Orphan and fallback refusals of `_refuse_unaccounted_markup`.
    add(
        "p5-fallback-markup",
        body_only(&format!(
            "{}<w:p><w:r><mc:AlternateContent><mc:Choice Requires=\"w14\"><w:t>choice</w:t></mc:Choice><mc:Fallback><w:ins w:id=\"60\" w:author=\"A\"><w:t>fallback</w:t></w:ins></mc:Fallback></mc:AlternateContent></w:r></w:p>",
            p(&rev("ins", 61, "A", &r("visible")))
        )),
    );
    add(
        "p5-unenumerable-markup",
        body_only(&format!(
            "{}<w:p><w:customXmlInsRangeEnd w:id=\"62\"/>{}</w:p>",
            p(&rev("ins", 63, "A", &r("visible"))),
            r("text")
        )),
    );
    add(
        "p5-move-orphans",
        body_only(&format!(
            "{}{}{}",
            p(&format!(
                "<w:moveFromRangeStart w:id=\"64\" w:author=\"A\"/>{}<w:moveFromRangeEnd w:id=\"64\"/>",
                rev("moveFrom", 65, "A", &r("unnamed range"))
            )),
            p(&format!(
                "<w:moveToRangeEnd w:id=\"66\"/>{}",
                r("stray end")
            )),
            p(&rev("moveTo", 67, "A", &r("outside any range")))
        )),
    );
    add(
        "p5-move-overlap",
        body_only(&p(&format!(
            "<w:moveFromRangeStart w:id=\"68\" w:author=\"A\" w:name=\"m1\"/><w:moveFromRangeStart w:id=\"69\" w:author=\"A\" w:name=\"m2\"/>{}<w:moveFromRangeEnd w:id=\"69\"/><w:moveFromRangeEnd w:id=\"68\"/><w:moveToRangeStart w:id=\"70\" w:author=\"A\" w:name=\"m1\"/>{}<w:moveToRangeEnd w:id=\"70\"/>",
            rev("moveFrom", 71, "A", &r("overlapped")),
            rev("moveTo", 72, "A", &r("overlapped"))
        ))),
    );
    // Paragraph marks: empty last block stays; content before a table stays.
    add(
        "p5-mark-edges",
        body_only(&format!(
            "<w:p><w:pPr><w:rPr><w:ins w:id=\"73\" w:author=\"A\"/></w:rPr></w:pPr>{}</w:p>{}<w:p><w:pPr><w:rPr><w:del w:id=\"74\" w:author=\"A\"/></w:rPr></w:pPr>{}</w:p>{}{}",
            r("inserted split"),
            p(&r("tail of split")),
            r("before table"),
            table(&[row(
                "",
                "<w:p><w:pPr><w:rPr><w:del w:id=\"75\" w:author=\"A\"/></w:rPr></w:pPr></w:p>"
            )]),
            "<w:p><w:pPr><w:rPr><w:del w:id=\"76\" w:author=\"A\"/></w:rPr></w:pPr></w:p>"
        )),
    );
    // Format changes with stamps kept on reject and empty-container cleanup.
    add(
        "p5-format-edges",
        body_only(&format!(
            "<w:p><w:pPr><w:jc w:val=\"center\"/><w:rPr><w:ins w:id=\"77\" w:author=\"A\"/><w:b/><w:rPrChange w:id=\"78\" w:author=\"A\"><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr><w:pPrChange w:id=\"79\" w:author=\"A\"><w:pPr/></w:pPrChange></w:pPr>{}</w:p>{}{}",
            r("mark formatted"),
            p(
                "<w:r><w:rPr><w:rPrChange w:id=\"80\" w:author=\"A\"><w:rPr><w:u w:val=\"single\"/></w:rPr></w:rPrChange></w:rPr><w:t>only change</w:t></w:r>"
            ),
            p(&r("after"))
        )),
    );
    // Revisions inside text boxes, content controls, headers and comments.
    add(
        "p5-stories-and-containers",
        derive(
            "generated/feature-isolated/header-footer-sections.docx",
            &[
                (
                    DOC,
                    "<w:p><w:r><w:t>Section one body paragraph.</w:t></w:r></w:p>",
                    "<w:p><w:r><w:t>Section one body paragraph.</w:t></w:r><w:ins w:id=\"501\" w:author=\"A\" w:date=\"2026-06-01T09:30:00Z\"><w:r><w:t> body insertion</w:t></w:r></w:ins></w:p><w:sdt><w:sdtPr><w:tag w:val=\"t\"/></w:sdtPr><w:sdtContent><w:p><w:del w:id=\"502\" w:author=\"A\"><w:r><w:delText>control deletion</w:delText></w:r></w:del><w:r><w:t>control text</w:t></w:r></w:p></w:sdtContent></w:sdt>",
                ),
                (
                    "word/header1.xml",
                    "<w:r><w:t>Header for section one</w:t></w:r>",
                    "<w:r><w:t>Header for section one</w:t></w:r><w:del w:id=\"503\" w:author=\"B\"><w:r><w:delText> header deletion</w:delText></w:r></w:del>",
                ),
            ],
            Vec::new(),
        ),
    );
    // test_audit_revisions.py::it_drops_unreferenced_relationships_and_payloads_but_keeps_shared_ones
    add(
        "p5-discard-rels",
        package(
            &p(&format!(
                "{}{}",
                picture_run("rId1"),
                rev(
                    "ins",
                    20,
                    "Audit",
                    &format!(
                        "{}{}<w:hyperlink r:id=\"rId3\"><w:r><w:t>discarded link</w:t></w:r></w:hyperlink><w:r><w:object><o:OLEObject r:id=\"rId4\"/></w:object></w:r>",
                        picture_run("rId1"),
                        picture_run("rId2")
                    )
                )
            )),
            vec![
                Part {
                    name: "word/media/image1.bmp",
                    content_type: None,
                    data: bmp(255, 0, 0),
                },
                Part {
                    name: "word/media/image2.bmp",
                    content_type: None,
                    data: bmp(0, 0, 255),
                },
                Part {
                    name: "word/embeddings/audit-object.bin",
                    content_type: Some("application/vnd.openxmlformats-officedocument.oleObject"),
                    data: b"discarded object payload".to_vec(),
                },
            ],
            &[
                ("rId1", "image", "media/image1.bmp", false),
                ("rId2", "image", "media/image2.bmp", false),
                ("rId3", "hyperlink", "https://audit.example.invalid/", true),
                ("rId4", "oleObject", "embeddings/audit-object.bin", false),
            ],
            &[("bmp", "image/bmp")],
        ),
    );
    // test_audit_revisions.py note-body cleanup and
    // test_audit_revision_compounds.py note preflights.
    let fn_run = "<w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteReference w:id=\"1\"/></w:r>";
    let en_run = "<w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:endnoteReference w:id=\"1\"/></w:r>";
    let wrap =
        |run: &str, id: u32| format!("<w:ins w:id=\"{id}\" w:author=\"Audit\">{run}</w:ins>");
    add(
        "p5-notes-wrapped",
        derive(
            NOTES,
            &[
                (DOC, fn_run, &wrap(fn_run, 30)),
                (DOC, en_run, &wrap(en_run, 31)),
            ],
            Vec::new(),
        ),
    );
    let surviving =
        |id: &str| format!("<w:p><w:r><w:footnoteReference w:id=\"{id}\"/></w:r></w:p><w:sectPr");
    add(
        "p5-notes-shared-reference",
        derive(
            NOTES,
            &[
                (DOC, fn_run, &wrap(fn_run, 32)),
                (DOC, "<w:sectPr", &surviving("1")),
            ],
            Vec::new(),
        ),
    );
    add(
        "p5-notes-numeric-ids",
        derive(
            NOTES,
            &[
                (DOC, fn_run, &wrap(fn_run, 16)),
                (DOC, "<w:sectPr", &surviving("01")),
            ],
            Vec::new(),
        ),
    );
    add(
        "p5-notes-malformed-body",
        derive(
            NOTES,
            &[
                (DOC, fn_run, &wrap(fn_run, 17)),
                (
                    "word/footnotes.xml",
                    "<w:footnote w:id=\"1\">",
                    "<w:footnote w:id=\"broken\">",
                ),
            ],
            Vec::new(),
        ),
    );
    add(
        "p5-notes-duplicate-rel",
        derive(
            NOTES,
            &[
                (DOC, fn_run, &wrap(fn_run, 10)),
                (
                    "word/_rels/document.xml.rels",
                    "</Relationships>",
                    "<Relationship Id=\"rId99\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" Target=\"footnotes-audit.xml\"/></Relationships>",
                ),
                (
                    "[Content_Types].xml",
                    "</Types>",
                    &format!(
                        "<Override PartName=\"/word/footnotes-audit.xml\" ContentType=\"{CT_FOOTNOTES}\"/></Types>"
                    ),
                ),
            ],
            vec![("word/footnotes-audit.xml".to_string(), b"<audit/>".to_vec())],
        ),
    );
    for (name, bad) in [
        ("p5-malformed-note-ref", "not-a-number"),
        ("p5-empty-note-ref", ""),
    ] {
        add(
            name,
            body_only(&p(&format!(
                "{}{}",
                rev("ins", 14, "Audit", &r("first")),
                rev(
                    "ins",
                    15,
                    "Audit",
                    &format!("<w:r><w:footnoteReference w:id=\"{bad}\"/></w:r>")
                )
            ))),
        );
    }
    out
}

fn comment_documents() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, bytes: Vec<u8>| out.push((name.to_string(), bytes));
    let tail = "<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr>";

    // A plain target document (comment flows build on it step by step).
    add(
        "p5-comment-target",
        body_only(&format!(
            "{}{}{}{}",
            p(&r("comment target")),
            p(&format!(
                "{}{}{}",
                r("Alpha "),
                r("beta gamma"),
                r(" delta")
            )),
            p(&r("second target")),
            tail
        )),
    );
    // A document with an empty comments part ("Comments object with 0 comments").
    add(
        "p5-empty-comments",
        package(
            &p(&r("Body")),
            vec![comments_part("")],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        ),
    );
    // test_audit_revisions.py::it_removes_only_matching_comments_extended_entries
    let commented = |id: &str, text: &str, wrap_ref: Option<u32>| {
        let reference_run = reference(id);
        let reference_run = match wrap_ref {
            Some(rid) => {
                format!("<w:ins w:id=\"{rid}\" w:author=\"Audit\">{reference_run}</w:ins>")
            }
            None => reference_run,
        };
        format!(
            "<w:p><w:commentRangeStart w:id=\"{id}\"/>{}<w:commentRangeEnd w:id=\"{id}\"/>{reference_run}</w:p>",
            r(text)
        )
    };
    let full_comment_parts =
        |comments: &str, extended: &[(&str, Option<&str>, &str)], ids: &[&str]| {
            vec![
                comments_part(comments),
                extended_part(extended),
                ids_part(ids),
                extensible_part(ids),
            ]
        };
    add(
        "p5-comments-ext-cleanup",
        package(
            &format!(
                "{}{}",
                commented("0", "first comment", Some(40)),
                commented("1", "second comment", None)
            ),
            full_comment_parts(
                &format!(
                    "{}{}",
                    comment_xml("0", "Audit", "discard me", Some("10000001")),
                    comment_xml("1", "Audit", "keep me", Some("10000002"))
                ),
                &[("10000001", None, "1"), ("10000002", None, "1")],
                &["10000001", "10000002"],
            ),
            COMMENT_RELS,
            &[],
        ),
    );
    // ...::it_removes_identity_rows_keyed_on_a_nested_last_paragraph
    add(
        "p5-comments-nested-last",
        package(
            &commented("0", "nested comment", Some(41)),
            full_comment_parts(
                "<w:comment w:id=\"0\" w:author=\"Audit\" w:initials=\"\" w:date=\"2026-06-03T10:15:00Z\"><w:p w14:paraId=\"10000001\"><w:pPr><w:pStyle w:val=\"CommentText\"/></w:pPr><w:r><w:t>discard me</w:t></w:r></w:p><w:tbl><w:tblPr/><w:tblGrid><w:gridCol/></w:tblGrid><w:tr><w:tc><w:p w14:paraId=\"10000002\"/></w:tc></w:tr></w:tbl></w:comment>",
                &[("10000002", None, "0")],
                &["10000002"],
            ),
            COMMENT_RELS,
            &[],
        ),
    );
    // ...::it_removes_an_entire_discarded_comment_thread
    add(
        "p5-comments-thread-deleted",
        package(
            &format!(
                "<w:p><w:pPr><w:rPr><w:del w:id=\"9\" w:author=\"Audit\"/></w:rPr></w:pPr><w:del w:id=\"10\" w:author=\"Audit\"><w:commentRangeStart w:id=\"0\"/><w:commentRangeStart w:id=\"1\"/>{}<w:commentRangeEnd w:id=\"1\"/><w:commentRangeEnd w:id=\"0\"/>{}{}</w:del></w:p>{}",
                dr("threaded comment"),
                reference("0"),
                reference("1"),
                p(&r("survivor"))
            ),
            full_comment_parts(
                &format!(
                    "{}{}",
                    comment_xml("0", "Audit", "parent", Some("10000001")),
                    comment_xml("1", "Audit", "reply", Some("10000002"))
                ),
                &[("10000001", None, "0"), ("10000002", Some("10000001"), "0")],
                &["10000001", "10000002"],
            ),
            COMMENT_RELS,
            &[],
        ),
    );
    // test_audit_revision_compounds.py comment preflights
    let one_comment = |extra_body: &str, wrap_ref: Option<u32>| {
        package(
            &format!("{}{extra_body}", commented("0", "commented", wrap_ref)),
            vec![comments_part(&comment_xml("0", "Audit", "comment", None))],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        )
    };
    add(
        "p5-comment-duplicate-reference",
        one_comment(&p("<w:r><w:commentReference w:id=\"0\"/></w:r>"), Some(18)),
    );
    add(
        "p5-comment-range-boundary",
        package(
            &format!(
                "<w:p><w:ins w:id=\"19\" w:author=\"Audit\"><w:commentRangeStart w:id=\"0\"/></w:ins>{}<w:commentRangeEnd w:id=\"0\"/>{}</w:p>",
                r("commented"),
                reference("0")
            ),
            vec![comments_part(&comment_xml("0", "Audit", "comment", None))],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        ),
    );
    add(
        "p5-comment-duplicate-rel",
        package(
            &commented("0", "commented", Some(12)),
            vec![
                comments_part(&comment_xml("0", "Audit", "comment", None)),
                xml_part(
                    "word/comments-audit.xml",
                    CT_COMMENTS,
                    "<audit/>".to_string(),
                ),
            ],
            &[
                ("rId1", "comments", "comments.xml", false),
                ("rId2", "comments", "comments-audit.xml", false),
            ],
            &[],
        ),
    );
    // test_practical_review_hardening.py
    add(
        "p5-field-boundary",
        body_only(&format!(
            "<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/><w:t>field result</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType=\"begin\"/><w:t>target</w:t></w:r></w:p>{tail}"
        )),
    );
    add(
        "p5-para-id-case",
        package(
            &commented("0", "review", None),
            vec![
                comments_part(&comment_xml("0", "A", "review", Some("abcdef12"))),
                extended_part(&[("ABCDEF12", None, "1")]),
            ],
            &[
                ("rId1", "comments", "comments.xml", false),
                ("rId2", RT_EXTENDED, "commentsExtended.xml", false),
            ],
            &[],
        ),
    );
    for (name, edit) in [
        ("p5-comments-readonly", "readOnly"),
        ("p5-comments-commentmode", "comments"),
    ] {
        add(
            name,
            package(
                &commented("0", "commented", None),
                vec![
                    comments_part(&comment_xml("0", "Audit", "comment", None)),
                    protected_settings(edit),
                ],
                &[
                    ("rId1", "comments", "comments.xml", false),
                    ("rId2", "settings", "settings.xml", false),
                ],
                &[],
            ),
        );
    }
    add(
        "p5-reply-field-boundary",
        package(
            &format!(
                "<w:p><w:commentRangeStart w:id=\"0\"/><w:r><w:fldChar w:fldCharType=\"begin\"/><w:t>target</w:t></w:r><w:commentRangeEnd w:id=\"0\"/>{}</w:p>",
                reference("0")
            ),
            vec![comments_part(&comment_xml("0", "A", "review", None))],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        ),
    );
    add(
        "p5-duplicate-anchor",
        package(
            &format!(
                "<w:p><w:commentRangeStart w:id=\"0\"/><w:commentRangeStart w:id=\"0\"/>{}<w:commentRangeEnd w:id=\"0\"/>{}</w:p>",
                r("target"),
                reference("0")
            ),
            vec![comments_part(&comment_xml("0", "A", "review", None))],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        ),
    );
    // test_audit_commentops.py::it_compares_anchor_marker_ids_numerically
    add(
        "p5-numeric-anchor-ids",
        package(
            &format!(
                "<w:p><w:commentRangeStart w:id=\"01\"/>{}<w:commentRangeEnd w:id=\"01\"/>{}</w:p>",
                r("comment target"),
                reference("01")
            ),
            vec![comments_part(&comment_xml("1", "Reviewer", "Parent", None))],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        ),
    );
    // ...::it_refuses_invalid_anchor_marker_ids_atomically
    for marker in ["commentRangeStart", "commentRangeEnd", "commentReference"] {
        for (suffix, replacement) in [
            ("missing", String::new()),
            ("broken", " w:id=\"broken\"".to_string()),
        ] {
            let body = format!(
                "<w:p><w:commentRangeStart w:id=\"0\"/>{}<w:commentRangeEnd w:id=\"0\"/>{}</w:p>",
                r("comment target"),
                reference("0")
            )
            .replacen(
                &format!("<w:{marker} w:id=\"0\"/>"),
                &format!("<w:{marker}{replacement}/>"),
                1,
            );
            add(
                &format!("p5-bad-{marker}-{suffix}"),
                package(
                    &body,
                    vec![comments_part(&comment_xml("0", "Reviewer", "Parent", None))],
                    &[("rId1", "comments", "comments.xml", false)],
                    &[],
                ),
            );
        }
    }
    // test_audit_text_atomicity.py::it_refuses_a_colliding_comments_extended_part_before_resolution
    add(
        "p5-collision-extended",
        package(
            &format!("{}{tail}", p(&r("comment target"))),
            vec![Part {
                name: "word/commentsExtended.xml",
                content_type: Some("application/octet-stream"),
                data: b"collision".to_vec(),
            }],
            &[(
                "rId1",
                "urn:paper-docx:audit-collision",
                "commentsExtended.xml",
                false,
            )],
            &[],
        ),
    );
    // ...::it_refuses_malformed_existing_comment_ids_before_splitting_runs /
    // it_refuses_malformed_unrelated_comment_ids_before_reply_metadata
    add(
        "p5-broken-comment-id",
        package(
            &format!(
                "{}{}{}",
                commented("0", "parent", None),
                commented("broken", "other", None),
                p(&r("new target"))
            ),
            vec![comments_part(&format!(
                "{}{}",
                comment_xml("0", "Reviewer", "Parent", None),
                comment_xml("broken", "Reviewer", "Other", None)
            ))],
            &[("rId1", "comments", "comments.xml", false)],
            &[],
        ),
    );
    // Span.comment on a locked or data-bound control surface.
    add(
        "p5-control-surfaces",
        body_only(&format!(
            "<w:p><w:sdt><w:sdtPr><w:lock w:val=\"contentLocked\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p><w:p><w:sdt><w:sdtPr><w:dataBinding w:xpath=\"/a\" w:storeItemID=\"{{1}}\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p><w:p><w:fldSimple w:instr=\" DATE \">{}</w:fldSimple></w:p>{}",
            r("locked value"),
            r("bound value"),
            r("simple field"),
            p(&r("plain text"))
        )),
    );
    out
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    let mut out = revision_documents();
    out.extend(comment_documents());
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_document_opens_and_names_are_unique() {
        let docs = super::documents();
        let mut names: Vec<&str> = docs.iter().map(|(name, _)| name.as_str()).collect();
        for (name, bytes) in &docs {
            docrs::Document::open_bytes(bytes, &docrs::Limits::default())
                .unwrap_or_else(|err| panic!("{name}: {err}"));
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), docs.len());
        assert_eq!(docs.len(), 64);
    }
}
