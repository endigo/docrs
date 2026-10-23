//! Style collection read API (python-docx `styles` package): the read-side
//! scenarios of the inherited Behave features `sty-style-props`,
//! `sty-access-latent-styles`, `sty-latent-props` and `doc-styles`, with
//! values frozen from the pinned oracle.

mod support;

use docrs::package::zip_read::read_archive;
use docrs::styles::{StyleType, Styles, internal_to_ui, ui_to_internal};
use docrs::{Document, ErrorKind, Limits};
use support::*;

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";

const HEAD: &str = "<?xml version='1.0' encoding='UTF-8' standalone='yes'?><w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val=\"24\"/></w:rPr></w:rPrDefault><w:pPrDefault/></w:docDefaults>";

const BUILTINS: &str = "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style><w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\"><w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/><w:semiHidden/><w:unhideWhenUsed/></w:style><w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/></w:style><w:style w:type=\"numbering\" w:default=\"1\" w:styleId=\"NoList\"><w:name w:val=\"No List\"/><w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/></w:style>";

/// `minimal.docx` with `word/styles.xml` replaced.
fn with_styles(styles_xml: &str) -> Document {
    let bytes = fixture(MINIMAL);
    let archive = read_archive(&bytes, &Limits::default()).unwrap();
    let members: Vec<RawMember> = archive
        .members
        .iter()
        .map(|member| {
            if member.name == "word/styles.xml" {
                RawMember::new(&member.name, styles_xml.as_bytes())
            } else {
                RawMember::new(&member.name, &member.data)
            }
        })
        .collect();
    Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
}

/// The styles of upstream's Behave `sty-behav-props.docx`.
fn behav_props() -> Styles {
    with_styles(&format!(
        "{HEAD}<w:latentStyles w:defLockedState=\"0\" w:defUIPriority=\"99\" w:defSemiHidden=\"1\" w:defUnhideWhenUsed=\"1\" w:defQFormat=\"0\" w:count=\"276\"/>{BUILTINS}\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Foo\"><w:name w:val=\"Foo\"/><w:uiPriority w:val=\"42\"/><w:semiHidden/><w:unhideWhenUsed/><w:qFormat/><w:locked/></w:style>\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Bar\"><w:name w:val=\"Bar\"/><w:uiPriority w:val=\"24\"/><w:semiHidden w:val=\"0\"/><w:unhideWhenUsed w:val=\"0\"/><w:qFormat w:val=\"0\"/><w:locked w:val=\"0\"/></w:style>\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Baz\"><w:name w:val=\"Baz\"/></w:style></w:styles>"
    ))
    .styles()
    .unwrap()
}

/// The styles of upstream's Behave `sty-known-styles.docx` (trimmed).
fn known_styles() -> Styles {
    with_styles(&format!(
        "{HEAD}<w:latentStyles w:defLockedState=\"0\" w:defUIPriority=\"99\" w:defSemiHidden=\"1\" w:defUnhideWhenUsed=\"1\" w:defQFormat=\"0\" w:count=\"276\">\
<w:lsdException w:name=\"Normal\" w:semiHidden=\"1\" w:locked=\"1\" w:qFormat=\"1\" w:unhideWhenUsed=\"1\" w:uiPriority=\"42\"/>\
<w:lsdException w:name=\"Title\" w:semiHidden=\"0\" w:locked=\"0\" w:qFormat=\"0\" w:unhideWhenUsed=\"0\" w:uiPriority=\"10\"/>\
<w:lsdException w:name=\"Subtitle\"/><w:lsdException w:name=\"heading 1\" w:uiPriority=\"9\" w:qFormat=\"1\"/></w:latentStyles>{BUILTINS}\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Base\"><w:name w:val=\"Base\"/><w:qFormat/></w:style>\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Citation\"><w:name w:val=\"Citation\"/><w:basedOn w:val=\"Base\"/><w:next w:val=\"SubNormal\"/><w:qFormat/></w:style>\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"SubNormal\"><w:name w:val=\"Sub Normal\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Foobar\"/><w:qFormat/></w:style>\
<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Foo\"><w:name w:val=\"Foo\"/><w:next w:val=\"Base\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/></w:style></w:styles>"
    ))
    .styles()
    .unwrap()
}

fn id(style: Option<&docrs::styles::Style>) -> Option<&str> {
    style.and_then(|s| s.style_id.as_deref())
}

#[test]
fn style_display_properties() {
    let styles = behav_props();
    let foo = styles.get("Foo").unwrap();
    assert!(foo.hidden && foo.locked && foo.quick_style && foo.unhide_when_used);
    assert_eq!(foo.priority, Some(42));
    assert!(!foo.builtin);
    let bar = styles.get("Bar").unwrap();
    assert!(!bar.hidden && !bar.locked && !bar.quick_style && !bar.unhide_when_used);
    assert_eq!(bar.priority, Some(24));
    let baz = styles.get("Baz").unwrap();
    assert!(!baz.hidden && !baz.locked && !baz.quick_style && !baz.unhide_when_used);
    assert_eq!(baz.priority, None);
    assert!(styles.get("Normal").unwrap().builtin);
}

#[test]
fn base_and_next_paragraph_styles() {
    let styles = known_styles();
    let get = |name: &str| styles.get(name).unwrap();
    assert_eq!(id(styles.base_style(get("Base"))), None);
    assert_eq!(id(styles.base_style(get("Sub Normal"))), Some("Normal"));
    assert_eq!(id(styles.base_style(get("Citation"))), Some("Base"));
    let next = |name: &str| styles.next_paragraph_style(get(name)).style_id.clone();
    assert_eq!(next("Base").as_deref(), Some("Base"));
    assert_eq!(next("Citation").as_deref(), Some("SubNormal"));
    assert_eq!(next("Sub Normal").as_deref(), Some("SubNormal"));
    assert_eq!(next("Foo").as_deref(), Some("Base"));
}

#[test]
fn style_types_lookups_and_defaults() {
    let styles = known_styles();
    for (name, kind) in [
        ("Default Paragraph Font", StyleType::Character),
        ("No List", StyleType::Numbering),
        ("Normal", StyleType::Paragraph),
        ("Normal Table", StyleType::Table),
    ] {
        assert_eq!(styles.get(name).unwrap().style_type, kind, "{name}");
    }
    assert_eq!(id(styles.get("Heading 1")), Some("Heading1"));
    assert_eq!(
        styles.get("Heading 1").unwrap().name.as_deref(),
        Some("Heading 1")
    );
    assert_eq!(id(styles.get("SubNormal")), Some("SubNormal"));
    assert!(styles.get("Nope").is_none());
    assert!(styles.contains("Heading 1"));
    assert!(styles.contains("heading 1"));
    assert!(!styles.contains("SubNormal"));
    for (kind, expected) in [
        (StyleType::Paragraph, "Normal"),
        (StyleType::Character, "DefaultParagraphFont"),
        (StyleType::Table, "TableNormal"),
        (StyleType::Numbering, "NoList"),
    ] {
        assert_eq!(id(styles.default_style(kind)), Some(expected));
    }
    assert_eq!(
        id(styles.get_by_id(Some("Citation"), StyleType::Paragraph)),
        Some("Citation")
    );
    assert_eq!(
        id(styles.get_by_id(Some("Citation"), StyleType::Character)),
        Some("DefaultParagraphFont")
    );
    assert_eq!(
        id(styles.get_by_id(None, StyleType::Table)),
        Some("TableNormal")
    );
    assert_eq!(
        styles.get_style_id("Normal", StyleType::Paragraph).unwrap(),
        None
    );
    assert_eq!(
        styles
            .get_style_id("Citation", StyleType::Paragraph)
            .unwrap()
            .as_deref(),
        Some("Citation")
    );
    let err = styles
        .get_style_id("Citation", StyleType::Character)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert_eq!(err.reason(), "style_type_mismatch");
    assert!(styles.get_style_id("Nope", StyleType::Paragraph).is_err());
}

#[test]
fn latent_styles_defaults_and_exceptions() {
    let styles = known_styles();
    let latent = styles.latent_styles().unwrap();
    assert_eq!(latent.default_priority, Some(99));
    assert_eq!(latent.load_count, Some(276));
    assert!(latent.default_to_hidden);
    assert!(!latent.default_to_locked);
    assert!(!latent.default_to_quick_style);
    assert!(latent.default_to_unhide_when_used);
    assert_eq!(latent.len(), 4);
    assert_eq!(latent.exceptions[0].name, "Normal");
    let normal = latent.get("Normal").unwrap();
    assert_eq!(
        (
            normal.hidden,
            normal.locked,
            normal.quick_style,
            normal.unhide_when_used
        ),
        (Some(true), Some(true), Some(true), Some(true))
    );
    assert_eq!(normal.priority, Some(42));
    let title = latent.get("Title").unwrap();
    assert_eq!(
        (
            title.hidden,
            title.locked,
            title.quick_style,
            title.unhide_when_used
        ),
        (Some(false), Some(false), Some(false), Some(false))
    );
    let subtitle = latent.get("Subtitle").unwrap();
    assert_eq!(
        (subtitle.hidden, subtitle.priority, subtitle.quick_style),
        (None, None, None)
    );
    assert_eq!(latent.get("Heading 1").unwrap().name, "Heading 1");
    assert!(latent.get("heading 1").is_some());
    assert!(latent.get("Nope").is_none());
    assert!(behav_props().latent_styles().unwrap().is_empty());
}

#[test]
fn template_styles_match_the_oracle() {
    // Frozen from the oracle over the upstream minimal fixture.
    let styles = Document::open_bytes(&fixture(MINIMAL), &Limits::default())
        .unwrap()
        .styles()
        .unwrap();
    assert_eq!(styles.len(), 164);
    assert_eq!(styles.part.as_deref(), Some("word/styles.xml"));
    assert_eq!(styles.latent_styles().unwrap().len(), 137);
    let header = styles.get("Header").unwrap();
    assert_eq!(header.priority, Some(99));
    assert!(header.unhide_when_used && !header.quick_style);
    assert_eq!(id(styles.base_style(header)), Some("Normal"));
}

#[test]
fn no_styles_part_reads_the_bundled_default() {
    let bytes = minimal_package_with(DOCUMENT_XML);
    let doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    let styles = doc.styles().unwrap();
    // Behave doc-styles: a document with no styles part has 4 styles.
    assert_eq!(styles.len(), 4);
    assert_eq!(styles.part, None);
    assert_eq!(styles.latent_styles().unwrap().len(), 137);
    assert_eq!(styles.latent_styles().unwrap().load_count, Some(276));
    let names: Vec<_> = styles.iter().map(|s| s.name.clone().unwrap()).collect();
    assert_eq!(
        names,
        [
            "Normal",
            "Default Paragraph Font",
            "Normal Table",
            "No List"
        ]
    );
    let (out, patch) = doc.patch_bytes().unwrap();
    assert_eq!(patch.to_json()["changed_parts"], serde_json::json!([]));
    assert_eq!(out, bytes);
}

#[test]
fn babel_fish_maps_only_the_special_cases() {
    assert_eq!(ui_to_internal("Heading 1"), "heading 1");
    assert_eq!(ui_to_internal("Caption"), "caption");
    assert_eq!(ui_to_internal("Footer"), "footer");
    assert_eq!(ui_to_internal("Heading 10"), "Heading 10");
    assert_eq!(internal_to_ui("header"), "Header");
    assert_eq!(internal_to_ui("Body Text"), "Body Text");
}
