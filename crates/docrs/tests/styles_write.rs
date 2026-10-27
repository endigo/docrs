//! Style writing (`styles/styles.py` add_style, `styles/style.py` setters
//! and delete, `styles/latent.py`): XML frozen from the pinned oracle,
//! refusal atomicity and handle freshness.

mod support;

use docrs::authoring::{FontChange, ParagraphFormat};
use docrs::styles::StyleType;
use docrs::styles_write::{
    LatentDefaultsChange, LatentStyleChange, StyleChange, latent_style_refs,
};
use docrs::{Document, ErrorKind, Limits, RefusalKind};
use support::*;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn with_styles(styles_xml: &str) -> Document {
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>",
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>";
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"{W_NS}\">{styles_xml}</w:styles>"
    );
    let bytes = build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
    ]);
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

/// The `w:styles` children, serialized.
fn body(doc: &Document) -> String {
    let xml = String::from_utf8(docrs::xml::serialize(
        doc.package().xml("word/styles.xml").unwrap(),
    ))
    .unwrap();
    let start = xml.find(W_NS).unwrap() + W_NS.len() + 2;
    xml[start..xml.len() - "</w:styles>".len()].to_string()
}

const NORMAL: &str = "<w:docDefaults/><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:rPr><w:b/></w:rPr></w:style>";

#[test]
fn add_style_and_setters_write_upstream_xml() {
    let mut doc = with_styles(NORMAL);
    let code = doc
        .add_style("Inline Code", StyleType::Character, false)
        .unwrap();
    assert_eq!(code.style_id(&doc).unwrap().as_deref(), Some("InlineCode"));
    let normal = doc.style("Normal").unwrap();
    normal
        .set(
            &mut doc,
            &[
                StyleChange::Priority(Some(5)),
                StyleChange::Hidden(true),
                StyleChange::Locked(true),
                StyleChange::QuickStyle(true),
                StyleChange::UnhideWhenUsed(true),
                StyleChange::BaseStyle(Some("InlineCode".into())),
                StyleChange::NextParagraphStyle(None),
            ],
        )
        .unwrap();
    normal
        .set_paragraph_format(
            &mut doc,
            &ParagraphFormat {
                space_before: Some(Some(152_400)),
                ..ParagraphFormat::default()
            },
        )
        .unwrap();
    normal
        .set_font(&mut doc, &[FontChange::Flag("italic".into(), Some(true))])
        .unwrap();
    doc.add_latent_style("Heading 1").unwrap();
    doc.set_latent_defaults(&[LatentDefaultsChange::DefaultToHidden(false)])
        .unwrap();
    // Frozen from the oracle (same calls on the same XML).
    assert_eq!(
        body(&doc),
        "<w:docDefaults/><w:latentStyles w:defSemiHidden=\"0\"><w:lsdException w:name=\"heading 1\"/></w:latentStyles><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:basedOn w:val=\"InlineCode\"/><w:uiPriority w:val=\"5\"/><w:semiHidden/><w:unhideWhenUsed/><w:qFormat/><w:locked/><w:pPr><w:spacing w:before=\"240\"/></w:pPr><w:rPr><w:b/><w:i/></w:rPr></w:style><w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"InlineCode\"><w:name w:val=\"Inline Code\"/></w:style>"
    );
    let info = normal.info(&doc).unwrap();
    assert!(info.hidden && info.locked && info.quick_style && info.unhide_when_used);
    assert_eq!(info.priority, Some(5));
    let base = normal.base_style(&doc).unwrap().unwrap();
    assert_eq!(base, code);
    // next_paragraph_style falls back to the style itself.
    assert_eq!(normal.next_paragraph_style(&doc).unwrap(), normal);
    normal
        .set(
            &mut doc,
            &[
                StyleChange::Hidden(false),
                StyleChange::Locked(false),
                StyleChange::QuickStyle(false),
                StyleChange::UnhideWhenUsed(false),
                StyleChange::Priority(None),
                StyleChange::BaseStyle(None),
            ],
        )
        .unwrap();
    assert!(body(&doc).contains("<w:name w:val=\"Normal\"/><w:pPr>"));
}

#[test]
fn builtin_styles_use_the_python_docx_id_and_babel_fish() {
    let mut doc = with_styles(NORMAL);
    let heading = doc
        .add_style("Heading 1", StyleType::Paragraph, true)
        .unwrap();
    let info = heading.info(&doc).unwrap();
    assert_eq!(info.style_id.as_deref(), Some("Heading1"));
    assert_eq!(info.internal_name.as_deref(), Some("heading 1"));
    assert_eq!(info.name.as_deref(), Some("Heading 1"));
    assert!(info.builtin);
    assert!(body(&doc).ends_with(
        "<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/></w:style>"
    ));
    let list = doc
        .add_style("List Bullet", StyleType::Numbering, true)
        .unwrap();
    assert_eq!(list.style_type(&doc).unwrap(), StyleType::Numbering);
    assert_eq!(doc.style("Heading 1").unwrap(), heading);
    assert_eq!(doc.style("Heading1").unwrap(), heading);
}

#[test]
fn refusals_change_nothing() {
    let mut doc = with_styles(NORMAL);
    let code = doc.add_style("Code", StyleType::Character, false).unwrap();
    let before = snapshot(&doc);
    let err = doc
        .add_style("Normal", StyleType::Paragraph, false)
        .unwrap_err();
    assert_eq!(err.reason(), "style_name_in_use");
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    let err = doc.style("Nope").unwrap_err();
    assert_eq!(err.reason(), "missing_key");
    let normal = doc.style("Normal").unwrap();
    let err = normal
        .set(
            &mut doc,
            &[
                StyleChange::Hidden(true),
                StyleChange::Priority(Some(1 << 31)),
            ],
        )
        .unwrap_err();
    assert_eq!(err.reason(), "value_out_of_range");
    let err = code
        .set(
            &mut doc,
            &[StyleChange::NextParagraphStyle(Some("Normal".into()))],
        )
        .unwrap_err();
    assert_eq!(err.reason(), "style_attribute_unsupported");
    let err = code
        .set_paragraph_format(&mut doc, &ParagraphFormat::default())
        .unwrap_err();
    assert_eq!(err.reason(), "style_attribute_unsupported");
    let err = normal
        .set_font(&mut doc, &[FontChange::Highlight(Some("plaid".into()))])
        .unwrap_err();
    assert_eq!(err.reason(), "invalid_highlight");
    let err = doc
        .set_latent_defaults(&[
            LatentDefaultsChange::DefaultToLocked(true),
            LatentDefaultsChange::LoadCount(Some(-(1 << 40))),
        ])
        .unwrap_err();
    assert_eq!(err.reason(), "value_out_of_range");
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn delete_makes_the_handle_stale_and_leaves_others_fresh() {
    let mut doc = with_styles(NORMAL);
    let a = doc.add_style("A", StyleType::Paragraph, false).unwrap();
    let b = doc.add_style("B", StyleType::Paragraph, false).unwrap();
    a.delete(&mut doc).unwrap();
    let err = a.info(&doc).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    assert_eq!(err.reason(), "stale_handle");
    let before = snapshot(&doc);
    assert!(a.delete(&mut doc).is_err());
    assert_eq!(snapshot(&doc), before);
    assert_eq!(b.info(&doc).unwrap().name.as_deref(), Some("B"));
    assert_eq!(doc.styles().unwrap().len(), 2);
    let other = with_styles(NORMAL);
    let foreign = other.style("Normal").unwrap();
    let err = foreign.info(&doc).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
}

#[test]
fn latent_styles_add_set_delete() {
    let mut doc = with_styles(&format!(
        "<w:docDefaults/><w:latentStyles w:defLockedState=\"0\" w:defUIPriority=\"99\" w:count=\"276\"><w:lsdException w:name=\"Normal\" w:uiPriority=\"42\" w:qFormat=\"1\"/><w:lsdException w:name=\"Title\"/></w:latentStyles>{}",
        &NORMAL["<w:docDefaults/>".len()..]
    ));
    let normal = doc.latent_style("Normal").unwrap();
    assert_eq!(normal.info(&doc).unwrap().priority, Some(42));
    normal
        .set(
            &mut doc,
            &[
                LatentStyleChange::Priority(Some(24)),
                LatentStyleChange::Hidden(Some(true)),
                LatentStyleChange::QuickStyle(None),
                LatentStyleChange::Locked(Some(false)),
            ],
        )
        .unwrap();
    let info = normal.info(&doc).unwrap();
    assert_eq!(info.priority, Some(24));
    assert_eq!(info.hidden, Some(true));
    assert_eq!(info.quick_style, None);
    assert_eq!(info.locked, Some(false));
    assert!(body(&doc).contains(
        "<w:lsdException w:name=\"Normal\" w:uiPriority=\"24\" w:semiHidden=\"1\" w:locked=\"0\"/>"
    ));
    doc.set_latent_defaults(&[
        LatentDefaultsChange::DefaultPriority(Some(42)),
        LatentDefaultsChange::LoadCount(None),
        LatentDefaultsChange::DefaultToQuickStyle(true),
    ])
    .unwrap();
    let latent = doc.styles().unwrap();
    let latent = latent.latent_styles().unwrap();
    assert_eq!(latent.default_priority, Some(42));
    assert_eq!(latent.load_count, None);
    assert!(latent.default_to_quick_style);
    let foobar = doc.add_latent_style("Foobar").unwrap();
    assert_eq!(latent_style_refs(&doc).unwrap().len(), 3);
    normal.delete(&mut doc).unwrap();
    assert!(doc.latent_style("Normal").is_err());
    assert_eq!(foobar.info(&doc).unwrap().name, "Foobar");
    assert_eq!(latent_style_refs(&doc).unwrap().len(), 2);
}

#[test]
fn missing_parts_and_elements_are_created_like_upstream() {
    // No w:latentStyles: added before the first w:style.
    let mut doc = with_styles(NORMAL);
    doc.add_latent_style("Quote").unwrap();
    assert!(body(&doc).starts_with(
        "<w:docDefaults/><w:latentStyles><w:lsdException w:name=\"Quote\"/></w:latentStyles><w:style"
    ));
    let mut bare =
        Document::open_bytes(&build_zip(&minimal_members()), &Limits::default()).unwrap();
    assert_eq!(bare.style("Normal").unwrap_err().reason(), "no_styles_part");
    let style = bare
        .add_style("Custom", StyleType::Paragraph, false)
        .unwrap();
    assert_eq!(style.part(), "word/styles.xml");
    let (_, report) = bare.patch_bytes().unwrap();
    assert_eq!(
        report.added_parts,
        vec![
            "word/_rels/document.xml.rels".to_string(),
            "word/styles.xml".to_string()
        ]
    );
    assert!(bare.style("Normal").is_ok());
}

#[test]
fn style_font_and_paragraph_format_read_back() {
    let mut doc = with_styles(NORMAL);
    let normal = doc.style("Normal").unwrap();
    normal
        .set_font(
            &mut doc,
            &[
                FontChange::Size(Some(177_800)),
                FontChange::ThemeColor(Some("accent1".into())),
            ],
        )
        .unwrap();
    let font = normal.font(&doc).unwrap();
    assert_eq!(font.size, Some(177_800));
    assert_eq!(font.color.theme_color.as_deref(), Some("accent1"));
    assert_eq!(
        font.flags.iter().find(|(n, _)| n == "bold").unwrap().1,
        Some(true)
    );
    let format = normal.paragraph_format(&doc).unwrap();
    assert_eq!(format.alignment, None);
    // The new w:pPr goes before the existing w:rPr (CT_Style order).
    normal
        .set_paragraph_format(
            &mut doc,
            &ParagraphFormat {
                alignment: Some(Some("right".into())),
                ..ParagraphFormat::default()
            },
        )
        .unwrap();
    assert!(body(&doc).contains("<w:pPr><w:jc w:val=\"right\"/></w:pPr><w:rPr>"));
}
