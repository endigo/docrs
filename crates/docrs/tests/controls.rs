//! Content controls (upstream `controls.py`): enumeration, typed setters,
//! data binding, refusal atomicity and handle ownership. Expected values are
//! frozen from the pinned oracle (see `tests/ops/m4-controls-*.json`).

mod support;

use docrs::clock::Timestamp;
use docrs::controls::ControlValue;
use docrs::{Document, Limits, RefusalKind};
use serde_json::json;
use support::*;

const DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn doc_with(body: &str) -> Document {
    let bytes = minimal_package_with(&format!(
        r#"<w:document {DECLS}><w:body>{body}<w:sectPr/></w:body></w:document>"#
    ));
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

fn inline(pr: &str, text: &str) -> String {
    format!(
        "<w:p><w:sdt><w:sdtPr>{pr}</w:sdtPr><w:sdtContent><w:r><w:t>{text}</w:t></w:r></w:sdtContent></w:sdt></w:p>"
    )
}

#[test]
fn lists_the_placeholder_fixture_like_upstream() {
    let doc = Document::open_bytes(
        &fixture("generated/feature-isolated/placeholder-control.docx"),
        &Limits::default(),
    )
    .unwrap();
    let infos: Vec<_> = doc
        .list_controls()
        .unwrap()
        .iter()
        .map(|i| i.to_json())
        .collect();
    assert_eq!(
        infos,
        vec![json!({
            "tag": "client-name", "alias": "ClientName", "control_type": "rich_text",
            "value": "Click or tap here to enter text.", "story": "word/document.xml",
            "showing_placeholder": true, "is_data_bound": false, "choices": []
        })]
    );
}

#[test]
fn text_values_clear_placeholder_state_and_survive_reopen() {
    let mut doc = Document::open_bytes(
        &fixture("generated/feature-isolated/placeholder-control.docx"),
        &Limits::default(),
    )
    .unwrap();
    doc.set_control_value("Paper Instruments LLC", Some("client-name"), None)
        .unwrap();
    let (_, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    let reopened = reopen(&doc);
    let control = reopened.control(Some("client-name"), None).unwrap();
    assert_eq!(
        control.value(&reopened).unwrap(),
        ControlValue::Text("Paper Instruments LLC".into())
    );
    assert!(!control.showing_placeholder(&reopened).unwrap());
    assert!(!document_xml(&reopened).contains("PlaceholderText"));
}

#[test]
fn checkboxes_flip_state_and_declared_glyph() {
    let mut doc = doc_with(&inline(
        r#"<w:tag w:val="choice"/><w14:checkbox><w14:checked w14:val="0"/><w14:checkedState w14:val="2611" w14:font="Arial Unicode MS"/><w14:uncheckedState w14:val="2610" w14:font="Arial Unicode MS"/></w14:checkbox>"#,
        "☐",
    ));
    doc.set_control_value(true, Some("choice"), None).unwrap();
    let control = doc.control(Some("choice"), None).unwrap();
    assert_eq!(control.value(&doc).unwrap(), ControlValue::Checked(true));
    let xml = document_xml(&doc);
    assert!(xml.contains(r#"<w14:checked w14:val="1"/>"#), "{xml}");
    assert!(xml.contains(
        r#"<w:r><w:rPr><w:rFonts w:ascii="Arial Unicode MS" w:hAnsi="Arial Unicode MS" w:eastAsia="Arial Unicode MS" w:cs="Arial Unicode MS"/></w:rPr><w:t>☑</w:t></w:r>"#
    ), "{xml}");
    let err = doc
        .set_control_value("yes", Some("choice"), None)
        .unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    assert_eq!(control.info(&doc).unwrap().value, "True");
}

#[test]
fn checkbox_without_checked_state_gains_one_first() {
    let mut doc = doc_with(&inline(
        r#"<w:tag w:val="box"/><w14:checkbox><w14:checkedState w14:val="2612"/></w14:checkbox>"#,
        "☐",
    ));
    doc.set_control_value(true, Some("box"), None).unwrap();
    assert!(document_xml(&doc).contains(r#"<w14:checkbox><w14:checked w14:val="1"/><w14:checkedState w14:val="2612"/></w14:checkbox>"#));
}

#[test]
fn dates_format_in_utc_and_stamp_full_date() {
    let mut doc = doc_with(&[
        inline(r#"<w:tag w:val="signed"/><w:date w:fullDate="2020-01-01T00:00:00Z"><w:dateFormat w:val="MMMM d, yyyy"/><w:lid w:val="en-US"/></w:date>"#, "old"),
        inline(r#"<w:tag w:val="iso"/><w:date><w:dateFormat w:val="yyyy-MM-dd"/></w:date>"#, "pick"),
        inline(r#"<w:tag w:val="all"/><w:date><w:dateFormat w:val="dddd, ddd dd MMM MM yy HH:mm:ss h:m:s hh H tt t"/><w:lid w:val="en-GB"/></w:date>"#, "x"),
    ].concat());
    let instant = Timestamp::parse("2026-07-10T01:30:00+02:00").unwrap();
    doc.set_control_value(instant, Some("signed"), None)
        .unwrap();
    doc.set_control_value(
        ControlValue::Date {
            year: 2026,
            month: 7,
            day: 8,
        },
        Some("iso"),
        None,
    )
    .unwrap();
    doc.set_control_value(
        Timestamp::parse("2024-01-02T03:04:05Z").unwrap(),
        Some("all"),
        None,
    )
    .unwrap();
    let text = |tag: &str| {
        doc.control(Some(tag), None)
            .unwrap()
            .info(&doc)
            .unwrap()
            .value
    };
    assert_eq!(text("signed"), "July 9, 2026");
    assert_eq!(text("iso"), "2026-07-08");
    assert_eq!(
        text("all"),
        "Tuesday, Tue 02 Jan 01 24 03:04:05 3:4:5 03 3 AM A"
    );
    let xml = document_xml(&doc);
    assert!(xml.contains(r#"w:fullDate="2026-07-09T23:30:00Z""#));
    assert!(xml.contains(r#"w:fullDate="2026-07-08T00:00:00Z""#));
}

#[test]
fn a_display_string_drops_a_stale_full_date() {
    let mut doc = doc_with(&inline(
        r#"<w:tag w:val="d"/><w:date w:fullDate="2020-01-01T00:00:00Z"/>"#,
        "old",
    ));
    doc.set_control_value("next Tuesday", Some("d"), None)
        .unwrap();
    assert!(document_xml(&doc).contains("<w:date/>"));
}

#[test]
fn every_refusal_leaves_the_package_unchanged() {
    let mut doc = doc_with(&[
        inline(r#"<w:tag w:val="locked"/><w:text/><w:lock w:val="contentLocked"/>"#, "locked"),
        format!(r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="outer"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>"#,
            r#"<w:sdt><w:sdtPr><w:tag w:val="inner"/></w:sdtPr><w:sdtContent><w:r><w:t>inner</w:t></w:r></w:sdtContent></w:sdt>"#),
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="complex"/><w14:checkbox><w14:checked w14:val="0"/></w14:checkbox></w:sdtPr><w:sdtContent><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl></w:sdtContent></w:sdt></w:p>"#.to_string(),
        inline(r#"<w:tag w:val="state"/><w:dropDownList><w:listItem w:displayText="California" w:value="CA"/></w:dropDownList>"#, "Choose."),
        inline(r#"<w:tag w:val="unformatted"/><w:date w:fullDate="2020-01-01T00:00:00Z"/>"#, "old"),
        inline(r#"<w:tag w:val="names"/><w:date><w:dateFormat w:val="MMMM d"/></w:date>"#, "old"),
        inline(r#"<w:tag w:val="pic"/><w:picture/>"#, "p"),
        inline(r#"<w:tag w:val="bound"/><w:dataBinding w:xpath="/x"/>"#, "synced"),
        r#"<w:customXml w:element="x"><w:sdt><w:sdtPr><w:tag w:val="ambiguous"/><w:text/></w:sdtPr><w:sdtContent/></w:sdt></w:customXml>"#.to_string(),
    ].concat());
    let before = snapshot(&doc);
    let day = ControlValue::Date {
        year: 2026,
        month: 7,
        day: 10,
    };
    let cases: Vec<(&str, ControlValue, &str)> = vec![
        ("locked", "x".into(), "locked_control"),
        ("outer", "x".into(), "nested_controls"),
        ("complex", true.into(), "control_holds_table"),
        ("state", "Texas".into(), "not_a_choice"),
        ("unformatted", day.clone(), "missing_date_format"),
        ("names", day, "date_names_without_locale"),
        ("pic", "x".into(), "unsettable_control"),
        ("bound", "x".into(), "incomplete_binding"),
        ("ambiguous", "x".into(), "ambiguous_control_parent"),
    ];
    for (tag, value, reason) in cases {
        let err = doc.set_control_value(value, Some(tag), None).unwrap_err();
        assert!(err.is_refusal(), "{tag}: {err}");
        assert_eq!(err.reason(), reason, "{tag}");
        assert_eq!(snapshot(&doc), before, "{tag} changed the package");
    }
    let err = doc
        .set_control_value("a\tb", Some("inner"), None)
        .unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn empty_controls_get_content_matching_their_level() {
    let mut doc = doc_with(&[
        r#"<w:sdt><w:sdtPr><w:tag w:val="block"/><w:text/></w:sdtPr></w:sdt>"#,
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="inline"/><w:text/></w:sdtPr><w:sdtContent/></w:sdt></w:p>"#,
    ].concat());
    doc.set_control_value("replacement", Some("block"), None)
        .unwrap();
    doc.set_control_value("replacement", Some("inline"), None)
        .unwrap();
    let xml = document_xml(&doc);
    assert!(xml.contains(r#"<w:sdtPr><w:tag w:val="block"/><w:text/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>replacement</w:t></w:r></w:p></w:sdtContent>"#), "{xml}");
    assert!(xml.contains(r#"<w:sdtPr><w:tag w:val="inline"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>replacement</w:t></w:r></w:sdtContent>"#), "{xml}");
}

#[test]
fn selection_refusals() {
    let doc = doc_with(
        &[
            inline(r#"<w:tag w:val="dup"/>"#, "a"),
            inline(r#"<w:tag w:val="dup"/>"#, "b"),
        ]
        .concat(),
    );
    assert_eq!(
        doc.control(Some("dup"), None).unwrap_err().refusal_kind(),
        Some(RefusalKind::AmbiguousTarget)
    );
    assert_eq!(
        doc.control(Some("nope"), None).unwrap_err().refusal_kind(),
        Some(RefusalKind::TargetNotFound)
    );
    assert_eq!(
        doc.control(None, None).unwrap_err().code(),
        "invalid_argument"
    );
}

#[test]
fn handles_belong_to_their_document_and_survive_commits() {
    let body = inline(r#"<w:tag w:val="t"/>"#, "original");
    let mut source = doc_with(&body);
    let mut other = doc_with(&body);
    let control = source.control(Some("t"), None).unwrap();
    control.set_value(&mut source, "one").unwrap();
    control.set_value(&mut source, "two").unwrap();
    assert_eq!(
        control.value(&source).unwrap(),
        ControlValue::Text("two".into())
    );
    let before = snapshot(&other);
    let err = control.set_value(&mut other, "foreign").unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
    assert_eq!(snapshot(&other), before);
}

#[test]
fn protection_gates_form_fields_per_mode() {
    let settings = |edit: &str| {
        format!(
            r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:documentProtection w:edit="{edit}" w:enforcement="1"/></w:settings>"#
        )
    };
    let body = format!(
        r#"<w:document {DECLS}><w:body>{}</w:body></w:document>"#,
        inline(r#"<w:tag w:val="f"/>"#, "x")
    );
    for (edit, allowed) in [
        ("readOnly", false),
        ("comments", false),
        ("trackedChanges", false),
        ("forms", true),
    ] {
        let mut doc = Document::open_bytes(
            &package_with_settings(&body, &settings(edit)),
            &Limits::default(),
        )
        .unwrap();
        let before = snapshot(&doc);
        let result = doc.set_control_value("typed in", Some("f"), None);
        assert_eq!(result.is_ok(), allowed, "{edit}");
        if !allowed {
            assert_eq!(result.unwrap_err().code(), "document_protected");
            assert_eq!(snapshot(&doc), before);
            doc.acknowledge_protection().unwrap();
            doc.set_control_value("typed in", Some("f"), None).unwrap();
        }
    }
}

const ITEM_PROPS_CT: &str = "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";

fn bound_doc(xpath: &str, mappings: Option<&str>, item: &str) -> Document {
    let mapping = mappings
        .map(|m| format!(r#" w:prefixMappings="{m}""#))
        .unwrap_or_default();
    let document = format!(
        r#"<w:document {DECLS}><w:body>{}</w:body></w:document>"#,
        inline(
            &format!(
                r#"<w:tag w:val="bound"/><w:dataBinding{mapping} w:xpath="{xpath}" w:storeItemID="{{11111111-1111-1111-1111-111111111111}}"/>"#
            ),
            "old"
        )
    );
    let ct = CT_XML.replace("</Types>", &format!(r#"<Override PartName="/customXml/itemProps2.xml" ContentType="{ITEM_PROPS_CT}"/></Types>"#));
    let rels = |target: &str, kind: &str| {
        format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/></Relationships>"#
        )
    };
    let bytes = build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels("../customXml/item2.xml", "customXml").as_bytes()),
        RawMember::new("customXml/item2.xml", item.as_bytes()),
        RawMember::new("customXml/_rels/item2.xml.rels", rels("itemProps2.xml", "customXmlProps").as_bytes()),
        RawMember::new("customXml/itemProps2.xml", br#"<ds:datastoreItem xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml" ds:itemID="{11111111-1111-1111-1111-111111111111}"/>"#),
    ]);
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

fn item_xml(doc: &Document) -> String {
    String::from_utf8(docrs::xml::serialize(
        doc.package().xml("customXml/item2.xml").unwrap(),
    ))
    .unwrap()
}

#[test]
fn data_bound_controls_write_the_custom_xml_store() {
    let item = r#"<ns0:root xmlns:ns0="http://example.com/form" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><ns0:name xsi:nil="true" status="old">old</ns0:name></ns0:root>"#;
    let mut doc = bound_doc(
        "/ns0:root/ns0:name",
        Some("xmlns:ns0='http://example.com/form'"),
        item,
    );
    doc.set_control_value("new", Some("bound"), None).unwrap();
    assert_eq!(
        item_xml(&doc),
        r#"<ns0:root xmlns:ns0="http://example.com/form" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><ns0:name status="old">new</ns0:name></ns0:root>"#
    );
    let (_, report) = doc.patch_bytes().unwrap();
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert_eq!(
        changed,
        vec![
            "customXml/item2.xml".to_string(),
            "word/document.xml".to_string()
        ]
    );
    let reopened = reopen(&doc);
    assert!(item_xml(&reopened).contains(">new<"));

    let mut attr = bound_doc("/ns0:root/ns0:name/@status", None, item);
    attr.set_control_value("fresh", Some("bound"), None)
        .unwrap();
    assert!(item_xml(&attr).contains(r#"status="fresh">old<"#));

    let default_item = r#"<root xmlns="http://example.com/form"><name>old</name></root>"#;
    let mut default_ns = bound_doc("/root/name", None, default_item);
    default_ns
        .set_control_value("new", Some("bound"), None)
        .unwrap();
    assert_eq!(
        item_xml(&default_ns),
        r#"<root xmlns="http://example.com/form"><name>new</name></root>"#
    );
}

#[test]
fn unusable_bindings_refuse_without_touching_anything() {
    let item =
        r#"<ns0:root xmlns:ns0="http://example.com/form"><ns0:name>old</ns0:name></ns0:root>"#;
    for (xpath, kind, reason) in [
        (
            "/ns0:root/[[[",
            RefusalKind::UnsupportedStructure,
            "xpath_invalid",
        ),
        (
            "bogus-fn(/ns0:root)",
            RefusalKind::UnsupportedStructure,
            "xpath_invalid",
        ),
        (
            "count(/ns0:root/ns0:name)",
            RefusalKind::UnsupportedStructure,
            "xpath_not_node_set",
        ),
        (
            "string(/ns0:root/ns0:name)",
            RefusalKind::UnsupportedStructure,
            "xpath_not_node_set",
        ),
        (
            "boolean(/ns0:root)",
            RefusalKind::UnsupportedStructure,
            "xpath_not_node_set",
        ),
        (
            "/ns0:root/ns0:missing",
            RefusalKind::TargetNotFound,
            "xpath_matched_nothing",
        ),
        (
            "/ns0:root/ns0:name/text()",
            RefusalKind::UnsupportedStructure,
            "non_element_store_target",
        ),
    ] {
        let mut doc = bound_doc(xpath, Some("xmlns:ns0='http://example.com/form'"), item);
        let before = snapshot(&doc);
        let err = doc
            .set_control_value("new", Some("bound"), None)
            .unwrap_err();
        assert_eq!(
            (err.refusal_kind(), err.reason()),
            (Some(kind), reason),
            "{xpath}"
        );
        assert_eq!(snapshot(&doc), before, "{xpath}");
        if reason == "xpath_invalid" {
            assert!(err.message().contains("not a valid XPath expression"));
            assert!(err.message().contains("w:dataBinding expression"));
            assert!(!err.message().to_lowercase().contains("corrupt"));
        }
    }
}
