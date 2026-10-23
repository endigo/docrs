//! Inherited python-docx authoring API: XML effects frozen from the pinned
//! oracle, refusal atomicity and live-handle freshness.

use docrs::authoring::{
    BreakType, FontChange, LineSpacing, ParagraphFormat, SectionStart, Underline,
};
use docrs::{Document, ErrorKind, RefusalKind};

const PNG_150DPI: &str = "89504e470d0a1a0a0000000d49484452000000030000000208020000001216f14d0000000970485973000017120000171201679fd2520000000b49444154789c6360800100000a00017f80745e0000000049454e44ae426082";
const GIF_5X7: &str = "47494638396105000700800000000000ffffff2c00000000010001000002024401003b";

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn document_xml(doc: &Document) -> String {
    let tree = doc.package().xml("word/document.xml").unwrap();
    String::from_utf8(docrs::xml::serialize(tree)).unwrap()
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

#[test]
fn add_paragraph_with_text_and_style() {
    let mut doc = Document::new();
    let p = doc
        .add_paragraph("Hello\tworld", Some("Heading 1"))
        .unwrap();
    assert_eq!(p.text(&doc).unwrap(), "Hello\tworld");
    assert_eq!(p.style_id(&doc).unwrap().as_deref(), Some("Heading1"));
    assert!(document_xml(&doc).contains(
        "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Hello</w:t><w:tab/><w:t>world</w:t></w:r></w:p>"
    ));
    // The default paragraph style resolves to None but still adds w:pPr.
    doc.add_paragraph("plain", Some("Normal")).unwrap();
    assert!(document_xml(&doc).contains("<w:p><w:pPr/><w:r><w:t>plain</w:t></w:r></w:p>"));
    let (_, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    assert!(report.added_parts.is_empty());
}

#[test]
fn headings_page_breaks_and_level_validation() {
    let mut doc = Document::new();
    assert_eq!(
        doc.add_heading("T", 0)
            .unwrap()
            .style_id(&doc)
            .unwrap()
            .as_deref(),
        Some("Title")
    );
    assert_eq!(
        doc.add_heading("H", 9)
            .unwrap()
            .style_id(&doc)
            .unwrap()
            .as_deref(),
        Some("Heading9")
    );
    doc.add_page_break().unwrap();
    assert!(document_xml(&doc).contains("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"));
    let before = bytes(&doc);
    let err = doc.add_heading("x", 10).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert_eq!(err.reason(), "invalid_heading_level");
    assert_eq!(bytes(&doc), before);
}

#[test]
fn style_errors_change_nothing() {
    let mut doc = Document::new();
    doc.add_paragraph("seed", None).unwrap();
    let before = bytes(&doc);
    let unknown = doc.add_paragraph("x", Some("No Such Style")).unwrap_err();
    assert_eq!(unknown.reason(), "unknown_style");
    let wrong = doc.add_paragraph("x", Some("Emphasis")).unwrap_err();
    assert_eq!(wrong.reason(), "style_type_mismatch");
    let table = doc.add_table(1, 1, Some("Heading 1")).unwrap_err();
    assert_eq!(table.reason(), "style_type_mismatch");
    assert_eq!(bytes(&doc), before);
    assert_eq!(doc.paragraphs().unwrap().len(), 1);
}

#[test]
fn tables_cells_rows_and_columns() {
    let mut doc = Document::new();
    let table = doc.add_table(1, 2, None).unwrap();
    assert!(document_xml(&doc).contains(
        "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/><w:tblLook w:firstColumn=\"1\" w:firstRow=\"1\" w:lastColumn=\"0\" w:lastRow=\"0\" w:noHBand=\"0\" w:noVBand=\"1\" w:val=\"04A0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"4320\"/><w:gridCol w:w=\"4320\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"
    ));
    table
        .cell(&doc, 0, 1)
        .unwrap()
        .set_text(&mut doc, "B1")
        .unwrap();
    assert_eq!(table.cell(&doc, 0, 1).unwrap().text(&doc).unwrap(), "B1");
    table.add_row(&mut doc).unwrap();
    table.add_column(&mut doc, 914_400).unwrap();
    assert_eq!(
        (
            table.row_count(&doc).unwrap(),
            table.column_count(&doc).unwrap()
        ),
        (2, 3)
    );
    table.set_style(&mut doc, Some("Table Grid")).unwrap();
    assert_eq!(table.style_id(&doc).unwrap().as_deref(), Some("TableGrid"));
    let before = bytes(&doc);
    assert_eq!(
        table.cell(&doc, 9, 0).unwrap_err().reason(),
        "cell_out_of_range"
    );
    assert_eq!(
        table.add_column(&mut doc, -1).unwrap_err().reason(),
        "value_out_of_range"
    );
    assert_eq!(bytes(&doc), before);
}

#[test]
fn pictures_dedupe_by_sha1_and_allocate_ids() {
    let mut doc = Document::new();
    let first = doc.add_picture(&hex(PNG_150DPI), None, None).unwrap();
    assert_eq!((first.width, first.height), (18288, 12192));
    assert_eq!(
        (first.r_id.as_str(), first.image_part.as_str()),
        ("rId9", "word/media/image1.png")
    );
    let second = doc.add_picture(&hex(PNG_150DPI), None, Some(1000)).unwrap();
    assert_eq!((second.width, second.height), (1500, 1000));
    assert_eq!(second.image_part, first.image_part);
    assert_eq!(second.r_id, first.r_id);
    let third = doc.add_picture(&hex(GIF_5X7), Some(5), Some(6)).unwrap();
    assert_eq!(third.image_part, "word/media/image2.gif");
    let xml = document_xml(&doc);
    assert!(xml.contains("<wp:docPr id=\"1\" name=\"Picture 1\"/>"));
    assert!(xml.contains("<wp:docPr id=\"2\" name=\"Picture 2\"/>"));
    assert!(xml.contains("<pic:cNvPr id=\"0\" name=\"image.gif\"/>"));
    let (_, report) = doc.patch_bytes().unwrap();
    assert_eq!(
        report.added_parts,
        vec![
            "word/media/image1.png".to_string(),
            "word/media/image2.gif".to_string()
        ]
    );
    // Unlike upstream, a bad image leaves no empty paragraph behind.
    let before = bytes(&doc);
    let count = doc.paragraphs().unwrap().len();
    assert_eq!(
        doc.add_picture(b"nope", None, None).unwrap_err().reason(),
        "unrecognized_image"
    );
    assert_eq!(
        doc.add_picture(&hex(PNG_150DPI), Some(-1), None)
            .unwrap_err()
            .reason(),
        "value_out_of_range"
    );
    assert_eq!(doc.paragraphs().unwrap().len(), count);
    assert_eq!(bytes(&doc), before);
}

#[test]
fn runs_text_breaks_and_fonts() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("", None).unwrap();
    let run = p.add_run(&mut doc, None, None).unwrap();
    run.set_text(&mut doc, "a\tb\nc").unwrap();
    run.set_bold(&mut doc, Some(false)).unwrap();
    run.set_italic(&mut doc, Some(true)).unwrap();
    assert!(document_xml(&doc).contains(
        "<w:r><w:rPr><w:b w:val=\"0\"/><w:i/></w:rPr><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r>"
    ));
    run.add_break(&mut doc, BreakType::LineClearLeft).unwrap();
    run.add_text(&mut doc, " tail ").unwrap();
    run.set_font(
        &mut doc,
        &[
            FontChange::Name(Some("Arial".into())),
            FontChange::Size(Some(152_400)),
            FontChange::Color(Some((0x3C, 0x2F, 0x80))),
            FontChange::Underline(Some(Underline::Style("double".into()))),
            FontChange::Flag("bold".into(), Some(true)),
        ],
    )
    .unwrap();
    let xml = document_xml(&doc);
    assert!(xml.contains("<w:br w:clear=\"left\"/><w:t xml:space=\"preserve\"> tail </w:t>"));
    assert!(xml.contains(
        "<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:b/><w:i/><w:color w:val=\"3C2F80\"/><w:sz w:val=\"24\"/><w:u w:val=\"double\"/></w:rPr>"
    ));
    assert_eq!(run.text(&doc).unwrap(), "a\tb\nc\n tail ");
    run.set_style(&mut doc, Some("Strong")).unwrap();
    assert!(document_xml(&doc).contains("<w:rPr><w:rStyle w:val=\"Strong\"/><w:rFonts"));
    let before = bytes(&doc);
    let err = run
        .set_font(
            &mut doc,
            &[FontChange::Name(None), FontChange::Size(Some(-1))],
        )
        .unwrap_err();
    assert_eq!(err.reason(), "value_out_of_range");
    assert_eq!(
        bytes(&doc),
        before,
        "the earlier change in the batch was not applied"
    );
}

#[test]
fn paragraph_editing_and_format() {
    let mut doc = Document::new();
    let first = doc.add_paragraph("First", None).unwrap();
    let second = doc.add_paragraph("Second", None).unwrap();
    let inserted = second
        .insert_paragraph_before(&mut doc, Some("Between"), Some("Quote"))
        .unwrap();
    let texts: Vec<String> = doc
        .paragraphs()
        .unwrap()
        .iter()
        .map(|p| p.text(&doc).unwrap())
        .collect();
    assert_eq!(texts, ["First", "Between", "Second"]);
    assert_eq!(inserted.style_id(&doc).unwrap().as_deref(), Some("Quote"));
    second.set_alignment(&mut doc, Some("center")).unwrap();
    second.set_text(&mut doc, Some("Replaced")).unwrap();
    assert!(document_xml(&doc).contains(
        "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:t>Replaced</w:t></w:r></w:p>"
    ));
    first
        .set_format(
            &mut doc,
            &ParagraphFormat {
                left_indent: Some(Some(457_200)),
                first_line_indent: Some(Some(-114_300)),
                line_spacing: Some(Some(LineSpacing::Multiple(1.5))),
                keep_with_next: Some(Some(true)),
                widow_control: Some(Some(false)),
                ..ParagraphFormat::default()
            },
        )
        .unwrap();
    assert!(document_xml(&doc).contains(
        "<w:pPr><w:keepNext/><w:widowControl w:val=\"0\"/><w:spacing w:line=\"360\" w:lineRule=\"auto\"/><w:ind w:left=\"720\" w:hanging=\"180\"/></w:pPr>"
    ));
    first.clear(&mut doc).unwrap();
    assert_eq!(first.text(&doc).unwrap(), "");
    assert_eq!(
        second
            .set_alignment(&mut doc, Some("middle"))
            .unwrap_err()
            .reason(),
        "invalid_alignment"
    );
}

#[test]
fn add_section_copies_sentinel_without_attributes() {
    let mut doc = Document::new();
    let section = doc.add_section(SectionStart::EvenPage).unwrap();
    assert_eq!(section.start_type, SectionStart::EvenPage);
    let xml = document_xml(&doc);
    assert!(
        xml.contains("<w:p><w:pPr><w:sectPr>"),
        "the copy lost its rsid attributes"
    );
    assert!(xml.contains("<w:type w:val=\"evenPage\"/>"));
    doc.add_section(SectionStart::NewPage).unwrap();
    assert!(!document_xml(&doc).contains("<w:type w:val=\"evenPage\"/>\n      <w:pgSz"));
}

#[test]
fn handles_are_local_and_fresh() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("", None).unwrap();
    let run = p.add_run(&mut doc, Some("x"), None).unwrap();
    doc.add_paragraph("other", None).unwrap();
    p.insert_paragraph_before(&mut doc, Some("before"), None)
        .unwrap();
    run.add_tab(&mut doc).unwrap();
    p.clear(&mut doc).unwrap();
    let before = bytes(&doc);
    let stale = run.set_text(&mut doc, "y").unwrap_err();
    assert_eq!(stale.refusal_kind(), Some(RefusalKind::TargetNotFound));
    assert_eq!(stale.reason(), "stale_handle");
    assert_eq!(bytes(&doc), before);
    let mut other = Document::new();
    let err = p.add_run(&mut other, Some("z"), None).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
    assert_eq!(err.reason(), "foreign_handle");
    let fresh = doc.paragraphs().unwrap();
    fresh[1].add_run(&mut doc, Some("ok"), None).unwrap();
    assert_eq!(fresh[1].text(&doc).unwrap(), "ok");
}

#[test]
fn styles_part_is_created_on_demand() {
    // Like upstream, a style lookup creates the default styles part; a failed
    // lookup leaves the package alone.
    let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"#;
    let mut doc = Document::open_bytes(&build(xml), &docrs::Limits::default()).unwrap();
    let before = bytes(&doc);
    assert_eq!(
        doc.add_paragraph("x", Some("Heading 1"))
            .unwrap_err()
            .reason(),
        "unknown_style"
    );
    assert_eq!(bytes(&doc), before);
    let p = doc.paragraphs().unwrap().remove(0);
    p.set_style(&mut doc, None).unwrap();
    let (_, report) = doc.patch_bytes().unwrap();
    assert!(report.added_parts.contains(&"word/styles.xml".to_string()));
    assert_eq!(
        doc.add_table(1, 1, None).unwrap_err().reason(),
        "no_section_properties"
    );
}

fn build(document_xml: &str) -> Vec<u8> {
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    docrs::package::zip_write::write_archive(
        [
            ("[Content_Types].xml", ct.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("word/document.xml", document_xml.as_bytes()),
        ],
        &docrs::Limits::default(),
    )
    .unwrap()
}
