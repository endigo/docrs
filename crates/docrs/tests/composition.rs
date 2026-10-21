//! Composition contracts (upstream `composition.py`): range selection,
//! style/numbering/media/hyperlink/bookmark reconciliation, letterheads,
//! refusal atomicity, source immutability and handle freshness. Expected
//! values are frozen from the pinned oracle (the matching ops cases in
//! `tests/ops/m7-composition-*.json` compare them live).

mod support;

use std::collections::BTreeMap;

use docrs::anchor::BlockTarget;
use docrs::composition::{
    AppendOptions, CompositionReport, HeadersFrom, InsertBlocksOptions, SectionStart, StylesMode,
};
use docrs::{Document, ErrorKind, FindOptions, Limits, RefusalKind, View};
use support::*;

const TINY_PNG: &str = "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de\
0000000c4944415408d763f8cfc000000301010018dd8db00000000049454e44ae426082";
const RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// A package edited member by member, starting from the default template.
struct Pkg(Vec<(String, Vec<u8>)>);

impl Pkg {
    fn base() -> Pkg {
        let (bytes, _) = Document::new().patch_bytes().unwrap();
        let contents = docrs::package::zip_read::read_archive(&bytes, &Limits::default()).unwrap();
        Pkg(contents
            .members
            .into_iter()
            .map(|m| (m.name, m.data))
            .collect())
    }

    fn text(&self, name: &str) -> String {
        let (_, data) = self.0.iter().find(|(n, _)| n == name).unwrap();
        String::from_utf8(data.clone()).unwrap()
    }

    fn put(mut self, name: &str, data: impl Into<Vec<u8>>) -> Pkg {
        let data = data.into();
        match self.0.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = data,
            None => self.0.push((name.to_string(), data)),
        }
        self
    }

    fn replace(self, name: &str, from: &str, to: &str) -> Pkg {
        let text = self.text(name);
        assert!(text.contains(from), "{name} lacks {from}");
        self.put(name, text.replacen(from, to, 1))
    }

    fn body(self, content: &str) -> Pkg {
        self.replace(
            "word/document.xml",
            "<w:sectPr ",
            &format!("{content}<w:sectPr "),
        )
    }

    fn style(self, xml: &str) -> Pkg {
        self.replace(
            "word/styles.xml",
            "</w:styles>",
            &format!("{xml}</w:styles>"),
        )
    }

    fn header(self, kind: &str, content: &str) -> Pkg {
        let xml = format!(
            "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<w:hdr xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"{RT}\">{content}</w:hdr>"
        );
        self.put("word/header1.xml", xml)
            .replace(
                "[Content_Types].xml",
                "</Types>",
                "<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/></Types>",
            )
            .replace(
                "word/_rels/document.xml.rels",
                "</Relationships>",
                &format!("<Relationship Id=\"rId20\" Type=\"{RT}/header\" Target=\"header1.xml\"/></Relationships>"),
            )
            .replace(
                "word/document.xml",
                "w:rsidSect=\"00034616\">",
                &format!("w:rsidSect=\"00034616\"><w:headerReference w:type=\"{kind}\" r:id=\"rId20\"/>"),
            )
    }

    fn open(self) -> Document {
        let members: Vec<RawMember> = self.0.iter().map(|(n, d)| RawMember::new(n, d)).collect();
        Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
    }
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

fn minimal() -> Document {
    Document::open_bytes(
        &fixture("generated/minimal-clean/minimal.docx"),
        &Limits::default(),
    )
    .unwrap()
}

fn destination() -> Document {
    Pkg::base().body(&p("Destination anchor")).open()
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

fn changed_parts(doc: &Document) -> (Vec<String>, Vec<String>) {
    let (_, report) = doc.patch_bytes().unwrap();
    let json = report.to_json();
    let list = |key: &str| -> Vec<String> {
        let mut out: Vec<String> = json[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        out.sort();
        out
    };
    (list("changed_parts"), list("added_parts"))
}

fn part_xml(doc: &Document, part: &str) -> String {
    String::from_utf8(docrs::xml::serialize(doc.package().xml(part).unwrap())).unwrap()
}

fn texts(doc: &Document) -> Vec<String> {
    doc.blocks(View::Current)
        .unwrap()
        .into_iter()
        .filter(|b| b.story == "word/document.xml")
        .map(|b| b.text)
        .collect()
}

fn insert(
    doc: &mut Document,
    source: &Document,
    start: &str,
    anchor: &str,
    options: InsertBlocksOptions<'_>,
) -> docrs::Result<CompositionReport> {
    doc.insert_blocks_from(
        source,
        BlockTarget::Text(start),
        BlockTarget::Text(anchor),
        &options,
    )
}

fn count(n: i64) -> InsertBlocksOptions<'static> {
    InsertBlocksOptions {
        count: n,
        ..InsertBlocksOptions::default()
    }
}

fn refusal_kind(err: &docrs::Error) -> Option<RefusalKind> {
    err.refusal_kind()
}

fn styles_source() -> Document {
    let mut source = Document::new();
    source.add_heading("Clause Library", 1).unwrap();
    source
        .add_paragraph("Indemnity: the supplier shall hold harmless.", None)
        .unwrap();
    source.add_heading("Payment Terms", 2).unwrap();
    source
        .add_paragraph("Payment falls due after thirty days.", None)
        .unwrap();
    source
}

fn blocks_source(names: &[&str]) -> Document {
    let mut source = Document::new();
    for name in names {
        source.add_paragraph(name, None).unwrap();
    }
    source
}

#[test]
fn copies_a_styled_range_adopting_the_house_look() {
    let source = styles_source();
    let source_before = bytes(&source);
    let mut doc = minimal();
    let report = insert(
        &mut doc,
        &source,
        "Payment Terms",
        "Second body paragraph",
        count(2),
    )
    .unwrap();
    assert_eq!(report.inserted_blocks, 2);
    assert_eq!(
        report.style_map.get("Heading2").map(String::as_str),
        Some("Heading2")
    );
    assert!(report.imported_styles.is_empty());
    let json = report.to_json();
    assert_eq!(json["schema"], "paper_composition");
    assert_eq!(json["version"], 1);
    assert_eq!(
        json["declared_parts"],
        serde_json::json!([
            "[Content_Types].xml",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/numbering.xml",
            "word/styles.xml"
        ])
    );
    assert_eq!(
        texts(&doc),
        [
            "Minimal Clean Document",
            "First body paragraph with perfectly ordinary text.",
            "Second body paragraph, equally unremarkable.",
            "Payment Terms",
            "Payment falls due after thirty days."
        ]
    );
    assert_eq!(
        changed_parts(&doc),
        (vec!["word/document.xml".to_string()], vec![])
    );
    assert_eq!(
        bytes(&source),
        source_before,
        "the source is never modified"
    );
}

#[test]
fn controls_endpoint_inclusion_and_ignores_count_with_an_end_anchor() {
    let source = blocks_source(&["Block A", "Block B", "Block C", "Block D"]);
    for (include_start, include_end, expected) in [
        (true, true, vec!["Block A", "Block B", "Block C", "Block D"]),
        (false, true, vec!["Block B", "Block C", "Block D"]),
        (true, false, vec!["Block A", "Block B", "Block C"]),
        (false, false, vec!["Block B", "Block C"]),
    ] {
        let mut doc = destination();
        let end = BlockTarget::Text("Block D");
        let options = InsertBlocksOptions {
            end_anchor: Some(end),
            count: 99,
            include_start,
            include_end,
            ..InsertBlocksOptions::default()
        };
        let report = insert(&mut doc, &source, "Block A", "Destination anchor", options).unwrap();
        assert_eq!(report.inserted_blocks, expected.len());
        let copied: Vec<String> = texts(&doc)
            .into_iter()
            .filter(|t| t.starts_with("Block"))
            .collect();
        assert_eq!(copied, expected);
    }
    let mut doc = destination();
    let options = InsertBlocksOptions {
        count: 2,
        include_start: false,
        ..InsertBlocksOptions::default()
    };
    insert(&mut doc, &source, "Block A", "Destination anchor", options).unwrap();
    let copied: Vec<String> = texts(&doc)
        .into_iter()
        .filter(|t| t.starts_with("Block"))
        .collect();
    assert_eq!(copied, ["Block B", "Block C"]);
}

#[test]
fn range_refusals_leave_the_destination_unchanged() {
    let source = blocks_source(&["Block A", "Block B", "Block C"]);
    let mut doc = destination();
    let before = bytes(&doc);
    let cases: Vec<(InsertBlocksOptions<'_>, &str, Option<RefusalKind>)> = vec![
        (
            InsertBlocksOptions {
                include_end: false,
                ..InsertBlocksOptions::default()
            },
            "Block A",
            None,
        ),
        (count(0), "Block A", None),
        (
            InsertBlocksOptions {
                end_anchor: Some(BlockTarget::Text("Block A")),
                include_start: false,
                ..InsertBlocksOptions::default()
            },
            "Block A",
            Some(RefusalKind::TargetNotFound),
        ),
        (
            InsertBlocksOptions {
                end_anchor: Some(BlockTarget::Text("Block A")),
                ..InsertBlocksOptions::default()
            },
            "Block B",
            Some(RefusalKind::TargetNotFound),
        ),
        (
            InsertBlocksOptions {
                include_start: false,
                ..InsertBlocksOptions::default()
            },
            "Block C",
            Some(RefusalKind::TargetNotFound),
        ),
    ];
    for (options, start, kind) in cases {
        let err = insert(&mut doc, &source, start, "Destination anchor", options).unwrap_err();
        match kind {
            None => assert_eq!(err.kind(), ErrorKind::InvalidArgument, "{err}"),
            Some(kind) => assert_eq!(refusal_kind(&err), Some(kind), "{err}"),
        }
        assert_eq!(bytes(&doc), before);
    }
}

#[test]
fn remaps_numbering_to_fresh_restarted_definitions() {
    let mut source = Document::new();
    let source_num = source.ensure_decimal_definition().unwrap();
    for text in ["First imported item", "Second imported item"] {
        source.add_paragraph(text, None).unwrap();
        source
            .apply_numbering(BlockTarget::Text(text), source_num, 0)
            .unwrap();
    }
    let mut doc = minimal();
    let destination_num = doc.ensure_decimal_definition().unwrap();
    let report = insert(
        &mut doc,
        &source,
        "First imported item",
        "Second body paragraph",
        count(2),
    )
    .unwrap();
    assert_eq!(source_num, 10);
    let new_id = report.numbering_map[&source_num];
    assert_eq!(new_id, 11);
    assert_ne!(new_id, destination_num);
    let numbered = doc.list_numbering().unwrap().numbered_paragraphs;
    let copied: Vec<i64> = numbered
        .iter()
        .filter(|p| p.text.contains("imported item"))
        .map(|p| p.num_id)
        .collect();
    assert_eq!(copied, [11, 11]);
}

#[test]
fn imports_and_renames_styles() {
    let style = |size: &str| {
        format!(
            "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"HouseTerm\"><w:name w:val=\"HouseTerm\"/><w:basedOn w:val=\"Normal\"/><w:rPr><w:sz w:val=\"{size}\"/></w:rPr></w:style>"
        )
    };
    let source = Pkg::base()
        .body("<w:p><w:pPr><w:pStyle w:val=\"HouseTerm\"/></w:pPr><w:r><w:t>Large-type clause.</w:t></w:r></w:p>")
        .style(&style("32"))
        .open();
    // a missing style is imported under its own id
    let mut doc = destination();
    let report = insert(
        &mut doc,
        &source,
        "Large-type clause.",
        "Destination anchor",
        count(1),
    )
    .unwrap();
    assert_eq!(report.imported_styles, ["HouseTerm"]);
    // a colliding, different definition is cloned on request
    let mut doc = Pkg::base()
        .body(&p("Destination anchor"))
        .style(&style("18"))
        .open();
    let options = InsertBlocksOptions {
        styles: StylesMode::ImportRenamed,
        ..InsertBlocksOptions::default()
    };
    let report = insert(
        &mut doc,
        &source,
        "Large-type clause.",
        "Destination anchor",
        options,
    )
    .unwrap();
    let expected: BTreeMap<String, String> =
        [("HouseTerm".to_string(), "HouseTerm (imported)".to_string())].into();
    assert_eq!(report.renamed_styles, expected);
    assert_eq!(report.imported_styles, ["HouseTermImported"]);
    let styles = part_xml(&doc, "word/styles.xml");
    assert!(styles.contains("w:styleId=\"HouseTermImported\""));
    assert!(styles.contains("<w:name w:val=\"HouseTerm (imported)\"/>"));
    assert!(
        part_xml(&doc, "word/document.xml").contains("<w:pStyle w:val=\"HouseTermImported\"/>")
    );
    // match_by_name keeps the destination definition
    let mut doc = Pkg::base()
        .body(&p("Destination anchor"))
        .style(&style("18"))
        .open();
    let report = insert(
        &mut doc,
        &source,
        "Large-type clause.",
        "Destination anchor",
        count(1),
    )
    .unwrap();
    assert!(report.imported_styles.is_empty());
    assert_eq!(report.style_map["HouseTerm"], "HouseTerm");
}

#[test]
fn renames_colliding_bookmarks_and_remaps_refs() {
    let source = Pkg::base()
        .body(
            "<w:p><w:bookmarkStart w:id=\"1\" w:name=\"Term\"/><w:r><w:t>the definition body</w:t></w:r><w:bookmarkEnd w:id=\"1\"/></w:p>\
             <w:p><w:fldSimple w:instr=\" REF Term \\h \"><w:r><w:t>(cross-reference)</w:t></w:r></w:fldSimple></w:p>",
        )
        .open();
    let mut doc = minimal();
    let mut span = doc
        .find_one("perfectly ordinary", &FindOptions::default())
        .unwrap();
    doc.create_bookmark(&mut span, "Term").unwrap();
    let report = insert(
        &mut doc,
        &source,
        "the definition body",
        "Second body paragraph",
        count(2),
    )
    .unwrap();
    let expected: BTreeMap<String, String> =
        [("Term".to_string(), "Term_imported".to_string())].into();
    assert_eq!(report.bookmarks_renamed, expected);
    let xml = part_xml(&doc, "word/document.xml");
    assert!(xml.contains("w:name=\"Term_imported\""));
    assert!(xml.contains(" REF Term_imported \\h "));
    let names: Vec<String> = doc
        .bookmarks()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert_eq!(names, ["Term", "Term_imported"]);
}

#[test]
fn refuses_content_composition_cannot_carry_atomically() {
    let bodies = [
        "<w:p><w:r><w:t xml:space=\"preserve\">Embedded thing: </w:t></w:r><w:r><w:object/></w:r></w:p>",
        "<w:p><w:r><w:t>Embedded thing</w:t></w:r><w:ins w:id=\"1\" w:author=\"A\"><w:r><w:t>x</w:t></w:r></w:ins></w:p>",
        "<w:p><w:r><w:t>Embedded thing</w:t></w:r><w:r><w:footnoteReference w:id=\"1\"/></w:r></w:p>",
        "<w:p><w:r><w:t>Embedded thing</w:t></w:r><w:r><w:commentReference w:id=\"0\"/></w:r></w:p>",
        "<w:p><w:r><w:t>Embedded thing</w:t></w:r></w:p><w:customXml><w:p/></w:customXml><w:p><w:r><w:t>after</w:t></w:r></w:p>",
        "<w:p><w:pPr><w:pStyle w:val=\"Undefined\"/></w:pPr><w:r><w:t>Embedded thing</w:t></w:r></w:p>",
        "<w:p><w:bookmarkStart w:id=\"x\" w:name=\"Bad\"/><w:r><w:t>Embedded thing</w:t></w:r><w:bookmarkEnd w:id=\"x\"/></w:p>",
        "<w:p><w:r><w:t>Embedded thing</w:t></w:r><w:fldSimple w:instr=\" REF Elsewhere \"/></w:p>",
    ];
    for body in bodies {
        let source = Pkg::base().body(body).open();
        let mut doc = destination();
        let before = bytes(&doc);
        let options = InsertBlocksOptions {
            count: if body.contains("customXml") { 2 } else { 1 },
            ..InsertBlocksOptions::default()
        };
        let err = insert(
            &mut doc,
            &source,
            "Embedded thing",
            "Destination anchor",
            options,
        )
        .unwrap_err();
        assert_eq!(
            refusal_kind(&err),
            Some(RefusalKind::UnsupportedStructure),
            "{body}: {err}"
        );
        assert_eq!(bytes(&doc), before, "{body}");
    }
}

#[test]
fn copies_images_as_new_deduplicated_parts() {
    let mut source = Document::new();
    source.add_paragraph("Before the picture.", None).unwrap();
    source.add_picture(&hex(TINY_PNG), None, None).unwrap();
    let mut doc = minimal();
    let report = insert(
        &mut doc,
        &source,
        "Before the picture.",
        "Second body paragraph",
        count(2),
    )
    .unwrap();
    assert_eq!(report.media_copied, ["word/media/image1.png"]);
    assert!(doc.package().part("word/media/image1.png").is_some());
    let (changed, added) = changed_parts(&doc);
    assert_eq!(
        changed,
        [
            "[Content_Types].xml",
            "word/_rels/document.xml.rels",
            "word/document.xml"
        ]
    );
    assert_eq!(added, ["word/media/image1.png"]);
    // the same image again reuses the part and the relationship
    let report = insert(
        &mut doc,
        &source,
        "Before the picture.",
        "First body paragraph",
        count(2),
    )
    .unwrap();
    assert_eq!(report.media_copied, ["word/media/image1.png"]);
    assert!(doc.package().part("word/media/image2.png").is_none());
}

#[test]
fn recreates_external_hyperlinks() {
    let mut source = Document::new();
    source.add_paragraph("Visit the terms page", None).unwrap();
    let span = source
        .find_one("the terms page", &FindOptions::default())
        .unwrap();
    source
        .add_hyperlink(&span, "https://paper.example/terms")
        .unwrap();
    let mut doc = minimal();
    insert(
        &mut doc,
        &source,
        "the terms page",
        "Second body paragraph",
        count(1),
    )
    .unwrap();
    let rels = doc
        .package()
        .relationships_at("/word/document.xml")
        .unwrap();
    assert!(
        rels.iter()
            .any(|rel| rel.external && rel.target == "https://paper.example/terms")
    );
}

#[test]
fn appends_with_a_page_break_and_optionally_the_source_letterhead() {
    let source = Pkg::base()
        .body(&p("Appended body"))
        .header(
            "default",
            "<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr><w:r><w:t>Source letterhead</w:t></w:r></w:p>",
        )
        .open();
    let mut doc = minimal();
    let report = doc
        .append_document(&source, &AppendOptions::default())
        .unwrap();
    assert_eq!(report.inserted_blocks, 1);
    let xml = part_xml(&doc, "word/document.xml");
    assert!(xml.contains("<w:br w:type=\"page\"/>"));
    assert!(doc.package().part("word/header1.xml").is_none());

    let mut doc = minimal();
    let options = AppendOptions {
        section: SectionStart::Continuous,
        headers: HeadersFrom::Source,
        ..AppendOptions::default()
    };
    doc.append_document(&source, &options).unwrap();
    let xml = part_xml(&doc, "word/document.xml");
    assert!(!xml.contains("w:type=\"page\""));
    assert!(xml.contains("<w:headerReference w:type=\"default\" r:id=\"rId9\"/>"));
    assert!(part_xml(&doc, "word/header1.xml").contains("Source letterhead"));
    let (changed, added) = changed_parts(&doc);
    assert_eq!(
        changed,
        [
            "[Content_Types].xml",
            "word/_rels/document.xml.rels",
            "word/document.xml"
        ]
    );
    assert_eq!(added, ["word/header1.xml"]);
}

#[test]
fn letterhead_refusal_after_the_body_compose_is_atomic() {
    let source = Pkg::base()
        .body(&p("Appended body"))
        .header("even", "<w:p><w:r><w:t>Even letterhead</w:t></w:r></w:p>")
        .replace(
            "word/settings.xml",
            "<w:characterSpacingControl",
            "<w:evenAndOddHeaders/><w:characterSpacingControl",
        )
        .open();
    let mut doc = Pkg::base()
        .body(&format!(
            "{}<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr></w:pPr></w:p>",
            p("Destination anchor")
        ))
        .open();
    let before = bytes(&doc);
    let options = AppendOptions {
        headers: HeadersFrom::Source,
        ..AppendOptions::default()
    };
    let err = doc.append_document(&source, &options).unwrap_err();
    assert_eq!(refusal_kind(&err), Some(RefusalKind::UnsupportedStructure));
    assert_eq!(err.reason(), "even_odd_document_wide");
    assert_eq!(bytes(&doc), before);
}

#[test]
fn destination_handles_stay_fresh_and_foreign_handles_refuse() {
    let source = blocks_source(&["Source block"]);
    let mut doc = destination();
    let span = doc
        .find_one("Destination anchor", &FindOptions::default())
        .unwrap();
    let block = doc
        .blocks(View::Current)
        .unwrap()
        .into_iter()
        .find(|b| b.text == "Destination anchor")
        .unwrap();
    let options = InsertBlocksOptions::default();
    doc.insert_blocks_from(
        &source,
        BlockTarget::Text("Source block"),
        BlockTarget::Span(&span),
        &options,
    )
    .unwrap();
    doc.insert_blocks_from(
        &source,
        BlockTarget::Text("Source block"),
        BlockTarget::Block(&block),
        &options,
    )
    .unwrap();
    assert_eq!(
        texts(&doc),
        ["Destination anchor", "Source block", "Source block"]
    );
    // a source span is a foreign destination anchor, a destination span a
    // foreign source anchor
    let source_span = source
        .find_one("Source block", &FindOptions::default())
        .unwrap();
    let before = bytes(&doc);
    let err = doc
        .insert_blocks_from(
            &source,
            BlockTarget::Text("Source block"),
            BlockTarget::Span(&source_span),
            &options,
        )
        .unwrap_err();
    assert_eq!(refusal_kind(&err), Some(RefusalKind::BoundaryViolation));
    let err = doc
        .insert_blocks_from(
            &source,
            BlockTarget::Span(&span),
            BlockTarget::Span(&span),
            &options,
        )
        .unwrap_err();
    assert_eq!(refusal_kind(&err), Some(RefusalKind::BoundaryViolation));
    assert_eq!(bytes(&doc), before);
    // historical destination targets refuse; historical source anchors work
    let historical = doc
        .find_one(
            "Destination anchor",
            &FindOptions {
                view: View::Original,
                ..FindOptions::default()
            },
        )
        .unwrap();
    let err = doc
        .insert_blocks_from(
            &source,
            BlockTarget::Text("Source block"),
            BlockTarget::Span(&historical),
            &options,
        )
        .unwrap_err();
    assert_eq!(err.reason(), "historical_view_target");
    let source_historical = source
        .find_one(
            "Source block",
            &FindOptions {
                view: View::All,
                ..FindOptions::default()
            },
        )
        .unwrap();
    doc.insert_blocks_from(
        &source,
        BlockTarget::Span(&source_historical),
        BlockTarget::Text("Destination anchor"),
        &options,
    )
    .unwrap();
}

#[test]
fn arms_fields_and_reallocates_control_ids() {
    let source = Pkg::base()
        .body(
            "<w:sdt><w:sdtPr><w:id w:val=\"2\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>First imported control</w:t></w:r></w:p></w:sdtContent></w:sdt>\
             <w:sdt><w:sdtPr><w:id w:val=\"1\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Second imported control</w:t></w:r><w:fldSimple w:instr=\" PAGE \"/></w:p></w:sdtContent></w:sdt>",
        )
        .open();
    let mut doc = Pkg::base()
        .body("<w:sdt><w:sdtPr><w:id w:val=\"1\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Existing control</w:t></w:r></w:p></w:sdtContent></w:sdt>")
        .open();
    let options = AppendOptions {
        section: SectionStart::Continuous,
        ..AppendOptions::default()
    };
    let report = doc.append_document(&source, &options).unwrap();
    assert_eq!(report.inserted_blocks, 2);
    let xml = part_xml(&doc, "word/document.xml");
    let ids: Vec<&str> = xml
        .match_indices("<w:id w:val=\"")
        .map(|(i, m)| &xml[i + m.len()..i + m.len() + 1])
        .collect();
    assert_eq!(ids, ["1", "2", "3"]);
    assert!(part_xml(&doc, "word/settings.xml").contains("<w:updateFields w:val=\"true\"/>"));
}

#[test]
fn a_source_without_styles_is_read_but_never_modified() {
    // upstream's `source.styles` adds a default styles part to the source
    let source = Document::open_bytes(
        &minimal_package_with(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>Plain source</w:t></w:r></w:p></w:body></w:document>",
        ),
        &Limits::default(),
    )
    .unwrap();
    let source_before = bytes(&source);
    let mut doc = destination();
    let report = doc
        .append_document(
            &source,
            &AppendOptions {
                section: SectionStart::Continuous,
                ..AppendOptions::default()
            },
        )
        .unwrap();
    assert_eq!(report.inserted_blocks, 1);
    assert_eq!(bytes(&source), source_before);
    assert!(source.package().part("word/styles.xml").is_none());
}

#[test]
fn modes_parse_upstream_tokens() {
    assert_eq!(
        StylesMode::parse("import_renamed").unwrap(),
        StylesMode::ImportRenamed
    );
    assert_eq!(
        SectionStart::parse("continuous").unwrap(),
        SectionStart::Continuous
    );
    assert_eq!(HeadersFrom::parse("source").unwrap(), HeadersFrom::Source);
    for err in [
        StylesMode::parse("steal").unwrap_err(),
        SectionStart::parse("fancy").unwrap_err(),
        HeadersFrom::parse("both").unwrap_err(),
    ] {
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    }
}
