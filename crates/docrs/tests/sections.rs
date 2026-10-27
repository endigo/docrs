//! Sections, headers/footers and the even/odd setting (upstream
//! `section.py`, `parts/hdrftr.py`, `settings.py`): page setup, definition
//! creation from the bundled templates, reference removal, refusal
//! atomicity and handle freshness. XML expectations were checked against the
//! pinned oracle through the `m7-inherited-*` ops cases.

mod support;

use docrs::authoring::SectionStart;
use docrs::sections::{Orientation, SectionChange};
use docrs::{Document, ErrorKind, Limits, RefusalKind};
use support::*;

const INCH: i64 = 914_400;

const HEADER_RT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const W_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

/// Two sections sharing header `rId7`; the third has its own header twice.
fn shared_header_doc() -> Document {
    let sect =
        |refs: &str| format!("<w:sectPr>{refs}<w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr>");
    let body = format!(
        "<w:p><w:pPr>{}</w:pPr></w:p><w:p><w:pPr>{}</w:pPr></w:p><w:p/>{}",
        sect("<w:headerReference w:type=\"default\" r:id=\"rId7\"/>"),
        sect("<w:headerReference w:type=\"default\" r:id=\"rId7\"/>"),
        sect(
            "<w:headerReference w:type=\"default\" r:id=\"rId9\"/><w:headerReference w:type=\"even\" r:id=\"rId9\"/>"
        ),
    );
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {W_DECL}><w:body>{body}</w:body></w:document>"
    );
    let header = |text: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:hdr {W_DECL}><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:hdr>"
        )
    };
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/><Override PartName=\"/word/header2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/></Types>",
    );
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId7\" Type=\"{HEADER_RT}\" Target=\"header1.xml\"/><Relationship Id=\"rId9\" Type=\"{HEADER_RT}\" Target=\"header2.xml\"/></Relationships>"
    );
    let members = vec![
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/header1.xml", header("shared").as_bytes()),
        RawMember::new("word/header2.xml", header("third").as_bytes()),
    ];
    Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
}

#[test]
fn page_setup_reads_and_writes_like_upstream() {
    let mut doc = Document::new();
    let sections = doc.sections().unwrap();
    assert_eq!(sections.len(), 1);
    let section = &sections[0];
    let setup = section.setup(&doc).unwrap();
    assert_eq!(setup.page_width, Some(12_240 * 635));
    assert_eq!(setup.orientation, Orientation::Portrait);
    assert_eq!(setup.start_type, SectionStart::NewPage);
    section
        .set(
            &mut doc,
            &[
                SectionChange::PageWidth(Some(11 * INCH)),
                SectionChange::PageHeight(Some(17 * INCH / 2)),
                SectionChange::Orientation(Some(Orientation::Landscape)),
                SectionChange::TopMargin(Some(-INCH)),
                SectionChange::Gutter(None),
                SectionChange::StartType(Some(SectionStart::OddPage)),
                SectionChange::DifferentFirstPageHeaderFooter(true),
            ],
        )
        .unwrap();
    let setup = section.setup(&doc).unwrap();
    assert_eq!(setup.page_width, Some(11 * INCH));
    assert_eq!(setup.page_height, Some(17 * INCH / 2));
    assert_eq!(setup.orientation, Orientation::Landscape);
    assert_eq!(setup.top_margin, Some(-INCH));
    assert_eq!(setup.gutter, None);
    assert_eq!(setup.start_type, SectionStart::OddPage);
    assert!(setup.different_first_page_header_footer);
    let document = part_xml(&doc, "word/document.xml");
    assert!(
        document.contains("<w:type w:val=\"oddPage\"/>"),
        "{document}"
    );
    assert!(document.contains("w:w=\"15840\" w:h=\"12240\" w:orient=\"landscape\""));
    assert!(document.contains("w:top=\"-1440\""));
    assert!(document.contains("<w:titlePg/>"));
    // Portrait is the default orientation and is written as no attribute;
    // NEW_PAGE removes w:type.
    section
        .set(
            &mut doc,
            &[
                SectionChange::Orientation(None),
                SectionChange::StartType(Some(SectionStart::NewPage)),
                SectionChange::DifferentFirstPageHeaderFooter(false),
            ],
        )
        .unwrap();
    let document = part_xml(&doc, "word/document.xml");
    assert!(!document.contains("w:orient"));
    assert!(!document.contains("<w:type"));
    assert!(!document.contains("titlePg"));
}

#[test]
fn invalid_lengths_change_nothing() {
    let mut doc = Document::new();
    let section = doc.sections().unwrap().remove(0);
    let before = snapshot(&doc);
    for change in [
        SectionChange::LeftMargin(Some(-1)),
        SectionChange::TopMargin(Some(3_000_000_000)),
        SectionChange::PageWidth(Some(-5)),
    ] {
        let err = section
            .set(&mut doc, &[SectionChange::Gutter(Some(INCH)), change])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert_eq!(snapshot(&doc), before);
    }
}

#[test]
fn unlinking_creates_a_definition_from_the_template() {
    let mut doc = Document::new();
    let section = doc.sections().unwrap().remove(0);
    let header = section.header();
    assert!(header.is_linked_to_previous(&doc).unwrap());
    assert!(header.existing_definition(&doc).unwrap().is_none());
    header.set_linked_to_previous(&mut doc, false).unwrap();
    assert!(!header.is_linked_to_previous(&doc).unwrap());
    let (_, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.added_parts, vec!["word/header1.xml".to_string()]);
    let header_xml = part_xml(&doc, "word/header1.xml");
    assert!(header_xml.contains("<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr></w:p>"));
    let document = part_xml(&doc, "word/document.xml");
    assert!(
        document.contains("<w:headerReference w:type=\"default\" r:id=\""),
        "{document}"
    );
    section
        .even_page_footer()
        .set_linked_to_previous(&mut doc, false)
        .unwrap();
    let (_, report) = doc.patch_bytes().unwrap();
    assert!(report.added_parts.contains(&"word/footer1.xml".to_string()));
    let container = header.definition(&mut doc).unwrap();
    let paragraphs = container.paragraphs(&doc).unwrap();
    assert_eq!(paragraphs.len(), 1);
    paragraphs[0]
        .set_text(&mut doc, Some("Header text"))
        .unwrap();
    assert!(part_xml(&doc, "word/header1.xml").contains("<w:t>Header text</w:t>"));
    let before = snapshot(&doc);
    header.set_linked_to_previous(&mut doc, false).unwrap();
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn definitions_are_inherited_and_created_on_the_first_section() {
    let mut doc = Document::new();
    doc.add_section(SectionStart::EvenPage).unwrap();
    let sections = doc.sections().unwrap();
    assert_eq!(sections.len(), 2);
    let second = sections[1].header();
    let container = second.definition(&mut doc).unwrap();
    container
        .add_paragraph(&mut doc, "from second", None)
        .unwrap();
    let sections = doc.sections().unwrap();
    assert!(!sections[0].header().is_linked_to_previous(&doc).unwrap());
    assert!(sections[1].header().is_linked_to_previous(&doc).unwrap());
    let inherited = sections[1]
        .header()
        .existing_definition(&doc)
        .unwrap()
        .unwrap();
    assert_eq!(
        inherited.paragraph_texts(&doc).unwrap(),
        vec!["".to_string(), "from second".to_string()]
    );
}

#[test]
fn removing_a_shared_reference_keeps_the_relationship() {
    let mut doc = shared_header_doc();
    let sections = doc.sections().unwrap();
    sections[1]
        .header()
        .set_linked_to_previous(&mut doc, true)
        .unwrap();
    // rId7 is still referenced by section one (upstream would drop it).
    assert!(part_xml(&doc, "word/_rels/document.xml.rels").contains("Id=\"rId7\""));
    let first = sections[0]
        .header()
        .existing_definition(&doc)
        .unwrap()
        .unwrap();
    assert_eq!(
        first.paragraph_texts(&doc).unwrap(),
        vec!["shared".to_string()]
    );
    sections[2]
        .even_page_header()
        .set_linked_to_previous(&mut doc, true)
        .unwrap();
    assert!(part_xml(&doc, "word/_rels/document.xml.rels").contains("Id=\"rId9\""));
    sections[2]
        .header()
        .set_linked_to_previous(&mut doc, true)
        .unwrap();
    assert!(!part_xml(&doc, "word/_rels/document.xml.rels").contains("Id=\"rId9\""));
    let inherited = sections[2]
        .header()
        .existing_definition(&doc)
        .unwrap()
        .unwrap();
    assert_eq!(
        inherited.paragraph_texts(&doc).unwrap(),
        vec!["shared".to_string()]
    );
}

#[test]
fn section_inner_content_follows_the_section_boundaries() {
    let doc = shared_header_doc();
    let sections = doc.sections().unwrap();
    let counts: Vec<usize> = sections
        .iter()
        .map(|s| s.inner_content(&doc).unwrap().len())
        .collect();
    assert_eq!(counts, vec![1, 1, 1]);
}

#[test]
fn handles_from_another_document_refuse() {
    let mut doc = Document::new();
    let other = Document::new();
    let foreign = other.sections().unwrap().remove(0);
    let err = foreign
        .set(&mut doc, &[SectionChange::Gutter(Some(1))])
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
    let err = foreign.header().is_linked_to_previous(&doc).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
}

#[test]
fn even_and_odd_setting() {
    let mut doc = Document::new();
    assert!(!doc.odd_and_even_pages_header_footer().unwrap());
    doc.set_odd_and_even_pages_header_footer(true).unwrap();
    assert!(doc.odd_and_even_pages_header_footer().unwrap());
    assert!(part_xml(&doc, "word/settings.xml").contains("<w:evenAndOddHeaders/>"));
    doc.set_odd_and_even_pages_header_footer(false).unwrap();
    assert!(!part_xml(&doc, "word/settings.xml").contains("evenAndOddHeaders"));
    let mut bare =
        Document::open_bytes(&minimal_package_with(DOCUMENT_XML), &Limits::default()).unwrap();
    assert!(!bare.odd_and_even_pages_header_footer().unwrap());
    assert!(!bare.package().is_modified());
    bare.set_odd_and_even_pages_header_footer(true).unwrap();
    let (_, report) = bare.patch_bytes().unwrap();
    // The minimal package has no document relationships part yet.
    assert_eq!(
        report.added_parts,
        vec![
            "word/_rels/document.xml.rels".to_string(),
            "word/settings.xml".to_string()
        ]
    );
}
