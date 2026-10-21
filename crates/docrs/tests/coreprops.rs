//! Core properties (`opc/coreprops.py`, `oxml/coreprops.py`): the Behave
//! `doc-coreprops` scenarios and `tests/opc/test_coreprops.py` assertions,
//! with XML frozen from the pinned oracle; refusal atomicity.

mod support;

use docrs::clock::Timestamp;
use docrs::coreprops::{CoreProperty, CorePropertyChange, CoreValue, parse_w3cdtf};
use docrs::xml::{parse_str, serialize, trees_equivalent};
use docrs::{Document, ErrorKind, Limits};
use support::*;

const CORE_NS: &str = "xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

/// The Behave `doc-coreprops.docx` core properties (indentation dropped).
const KNOWN: &str = "<dc:title>Title</dc:title><dc:subject>Subject</dc:subject><dc:creator>Steve Canny</dc:creator><cp:keywords>key; word; keyword</cp:keywords><cp:contentStatus>Content Status</cp:contentStatus><dc:description>Description</dc:description><dc:identifier>Identifier</dc:identifier><dc:language>Language</dc:language><cp:lastModifiedBy>Steve Canny</cp:lastModifiedBy><cp:lastPrinted>2014-12-13T22:02:42Z</cp:lastPrinted><cp:revision>2</cp:revision><cp:version>0.7.1a3</cp:version><dcterms:created xsi:type=\"dcterms:W3CDTF\">2014-12-13T22:02:00Z</dcterms:created><dcterms:modified xsi:type=\"dcterms:W3CDTF\">2014-12-13T22:06:00Z</dcterms:modified><cp:category>Category</cp:category>";

fn core_xml(children: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><cp:coreProperties {CORE_NS}>{children}</cp:coreProperties>"
    )
}

fn with_core(children: &str) -> Document {
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/></Types>",
    );
    let rels = ROOT_RELS.replace(
        "</Relationships>",
        "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>",
    );
    let core = core_xml(children);
    let bytes = build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", rels.as_bytes()),
        RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()),
        RawMember::new("docProps/core.xml", core.as_bytes()),
    ]);
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

fn ts(text: &str) -> Timestamp {
    Timestamp::parse(text).unwrap()
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

/// Serialized core part without its XML declaration.
fn core_tree_xml(doc: &Document) -> String {
    let xml =
        String::from_utf8(serialize(doc.package().xml("docProps/core.xml").unwrap())).unwrap();
    xml[xml.find("?>").unwrap() + 2..].trim_start().to_string()
}

fn expected(children: &str) -> String {
    let xml = core_xml(children);
    xml[xml.find("?>").unwrap() + 2..].to_string()
}

fn text(property: CoreProperty, value: &str) -> CorePropertyChange {
    CorePropertyChange::new(property, CoreValue::Text(value.into()))
}

#[test]
fn reads_the_known_core_properties() {
    let doc = with_core(KNOWN);
    let props = doc.core_properties().unwrap();
    assert_eq!(props.part.as_deref(), Some("docProps/core.xml"));
    assert_eq!(props.author, "Steve Canny");
    assert_eq!(props.category, "Category");
    assert_eq!(props.comments, "Description");
    assert_eq!(props.content_status, "Content Status");
    assert_eq!(props.created, Some(ts("2014-12-13T22:02:00Z")));
    assert_eq!(props.identifier, "Identifier");
    assert_eq!(props.keywords, "key; word; keyword");
    assert_eq!(props.language, "Language");
    assert_eq!(props.last_modified_by, "Steve Canny");
    assert_eq!(props.last_printed, Some(ts("2014-12-13T22:02:42Z")));
    assert_eq!(props.modified, Some(ts("2014-12-13T22:06:00Z")));
    assert_eq!(props.revision, 2);
    assert_eq!(props.subject, "Subject");
    assert_eq!(props.title, "Title");
    assert_eq!(props.version, "0.7.1a3");
}

#[test]
fn assigns_new_values_to_every_property() {
    let mut doc = with_core(KNOWN);
    let when = ts("2013-06-15T12:34:56Z");
    let changes = vec![
        text(CoreProperty::Author, "Creator"),
        text(CoreProperty::Category, "Category"),
        text(CoreProperty::Comments, "Description"),
        text(CoreProperty::ContentStatus, "Content Status"),
        CorePropertyChange::new(CoreProperty::Created, CoreValue::Date(when)),
        text(CoreProperty::Identifier, "Identifier"),
        text(CoreProperty::Keywords, "key; word; keyword"),
        text(CoreProperty::Language, "Language"),
        text(CoreProperty::LastModifiedBy, "Last Modified By"),
        CorePropertyChange::new(CoreProperty::LastPrinted, CoreValue::Date(when)),
        CorePropertyChange::new(CoreProperty::Modified, CoreValue::Date(when)),
        CorePropertyChange::new(CoreProperty::Revision, CoreValue::Int(9)),
        text(CoreProperty::Subject, "Subject"),
        text(CoreProperty::Title, "Title"),
        text(CoreProperty::Version, "Version"),
    ];
    let props = doc.set_core_properties(&changes, None).unwrap();
    assert_eq!(props.author, "Creator");
    assert_eq!(props.last_modified_by, "Last Modified By");
    assert_eq!(props.created, Some(when));
    assert_eq!(props.last_printed, Some(when));
    assert_eq!(props.modified, Some(when));
    assert_eq!(props.revision, 9);
    assert_eq!(props.version, "Version");
    let (_, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["docProps/core.xml".to_string()]);
    // Reopened, the values persist.
    let reopened = Document::open_bytes(&bytes(&doc), &Limits::default()).unwrap();
    assert_eq!(reopened.core_properties().unwrap(), props);
}

#[test]
fn string_and_date_setters_write_upstream_xml() {
    // tests/opc/test_coreprops.py: it_can_change_the_{string,date}_property_values
    for (property, tag, value) in [
        (CoreProperty::Author, "dc:creator", "scanny"),
        (CoreProperty::Category, "cp:category", "silly stories"),
        (CoreProperty::Comments, "dc:description", "Bar foo to you"),
        (CoreProperty::ContentStatus, "cp:contentStatus", "FINAL"),
        (CoreProperty::Identifier, "dc:identifier", "GT 5.2.xab"),
        (CoreProperty::Keywords, "cp:keywords", "dog cat moo"),
        (CoreProperty::Language, "dc:language", "GB-EN"),
        (
            CoreProperty::LastModifiedBy,
            "cp:lastModifiedBy",
            "Billy Bob",
        ),
        (CoreProperty::Subject, "dc:subject", "Eggs"),
        (CoreProperty::Title, "dc:title", "Dissertation"),
        (CoreProperty::Version, "cp:version", "81.2.8"),
    ] {
        let mut doc = with_core("");
        doc.set_core_properties(&[text(property, value)], None)
            .unwrap();
        assert_eq!(
            core_tree_xml(&doc),
            expected(&format!("<{tag}>{value}</{tag}>")),
            "{property:?}"
        );
    }
    for (property, tag, value, attrs) in [
        (
            CoreProperty::Created,
            "dcterms:created",
            "2001-02-03T04:05:00Z",
            " xsi:type=\"dcterms:W3CDTF\"",
        ),
        (
            CoreProperty::LastPrinted,
            "cp:lastPrinted",
            "2014-06-04T04:00:00Z",
            "",
        ),
        (
            CoreProperty::Modified,
            "dcterms:modified",
            "2005-04-03T02:01:00Z",
            " xsi:type=\"dcterms:W3CDTF\"",
        ),
    ] {
        let mut doc = with_core("");
        doc.set_core_properties(
            &[CorePropertyChange::new(
                property,
                CoreValue::Date(ts(value)),
            )],
            None,
        )
        .unwrap();
        assert_eq!(
            core_tree_xml(&doc),
            expected(&format!("<{tag}{attrs}>{value}</{tag}>"))
        );
    }
    let mut doc = with_core("");
    doc.set_core_properties(
        &[CorePropertyChange::new(
            CoreProperty::Revision,
            CoreValue::Int(42),
        )],
        None,
    )
    .unwrap();
    assert_eq!(
        core_tree_xml(&doc),
        expected("<cp:revision>42</cp:revision>")
    );
}

#[test]
fn reads_like_the_upstream_unit_fixture() {
    let doc = with_core(
        "<cp:contentStatus>DRAFT</cp:contentStatus><dc:creator>python-docx</dc:creator><dcterms:created xsi:type=\"dcterms:W3CDTF\">2012-11-17T11:07:40-05:30</dcterms:created><dc:description/><dc:identifier>GXS 10.2.1ab</dc:identifier><dc:language>US-EN</dc:language><cp:lastPrinted>2014-06-04T04:28:00Z</cp:lastPrinted><cp:keywords>foo bar baz</cp:keywords><cp:lastModifiedBy>Steve Canny</cp:lastModifiedBy><cp:revision>4</cp:revision><dc:subject>Spam</dc:subject><dc:title>Word Document</dc:title><cp:version>1.2.88</cp:version>",
    );
    let props = doc.core_properties().unwrap();
    assert_eq!(props.author, "python-docx");
    assert_eq!(props.category, "");
    assert_eq!(props.comments, "");
    assert_eq!(props.content_status, "DRAFT");
    assert_eq!(props.created, Some(ts("2012-11-17T16:37:40Z")));
    assert_eq!(props.last_printed, Some(ts("2014-06-04T04:28:00Z")));
    assert_eq!(props.modified, None);
    assert_eq!(props.revision, 4);
    assert_eq!(props.title, "Word Document");
    // it_knows_the_revision_number
    for (raw, expected) in [
        (Some("42"), 42),
        (None, 0),
        (Some("foobar"), 0),
        (Some("-17"), 0),
        (Some("32.7"), 0),
    ] {
        let children = raw
            .map(|v| format!("<cp:revision>{v}</cp:revision>"))
            .unwrap_or_default();
        assert_eq!(
            with_core(&children).core_properties().unwrap().revision,
            expected
        );
    }
    // Only the leading text counts (lxml `.text`); invalid dates read None.
    let odd = with_core(
        "<dc:title>lead<x:extra xmlns:x=\"urn:x\"/>tail</dc:title><cp:lastPrinted>yesterday</cp:lastPrinted>",
    );
    let props = odd.core_properties().unwrap();
    assert_eq!(props.title, "lead");
    assert_eq!(props.last_printed, None);
}

#[test]
fn w3cdtf_forms() {
    let parse = |text: &str| parse_w3cdtf(text).map(|t| t.to_xml());
    assert_eq!(parse("2003").as_deref(), Some("2003-01-01T00:00:00Z"));
    assert_eq!(parse("2003-12").as_deref(), Some("2003-12-01T00:00:00Z"));
    assert_eq!(
        parse("2003-12-31T10:14:55-08:00").as_deref(),
        Some("2003-12-31T18:14:55Z")
    );
    assert_eq!(parse("2003-13-01"), None);
}

#[test]
fn default_part_is_created_on_first_write() {
    let mut doc = Document::open_bytes(&build_zip(&minimal_members()), &Limits::default()).unwrap();
    let read = doc.core_properties().unwrap();
    assert_eq!(read.part, None);
    assert_eq!(read.title, "Word Document");
    assert_eq!(read.last_modified_by, "python-docx");
    assert_eq!(read.revision, 1);
    assert_eq!(read.modified, None);
    // Reading changed nothing.
    assert!(doc.patch_bytes().unwrap().1.changed_parts.is_empty());
    let now = ts("2024-01-02T03:04:05Z");
    let props = doc.set_core_properties(&[], Some(now)).unwrap();
    assert_eq!(props.part.as_deref(), Some("docProps/core.xml"));
    assert_eq!(props.modified, Some(now));
    let frozen = parse_str("<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>Word Document</dc:title><cp:lastModifiedBy>python-docx</cp:lastModifiedBy><cp:revision>1</cp:revision><dcterms:modified xsi:type=\"dcterms:W3CDTF\">2024-01-02T03:04:05Z</dcterms:modified></cp:coreProperties>").unwrap();
    assert!(trees_equivalent(
        doc.package().xml("docProps/core.xml").unwrap(),
        &frozen
    ));
    let (_, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.added_parts, vec!["docProps/core.xml".to_string()]);
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert_eq!(changed, vec!["[Content_Types].xml", "_rels/.rels"]);
    let reopened = Document::open_bytes(&bytes(&doc), &Limits::default()).unwrap();
    assert_eq!(reopened.core_properties().unwrap(), props);
}

#[test]
fn stale_override_for_the_missing_part_is_replaced() {
    // Word leaves an Override for /docProps/core.xml behind
    // (upstream's Behave doc-no-coreprops.docx).
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/></Types>",
    );
    let mut members = minimal_members();
    members[0] = RawMember::new("[Content_Types].xml", ct.as_bytes());
    let mut doc = Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap();
    doc.set_core_properties(&[text(CoreProperty::Title, "T")], None)
        .unwrap();
    let reopened = Document::open_bytes(&bytes(&doc), &Limits::default()).unwrap();
    assert_eq!(reopened.core_properties().unwrap().title, "T");
}

#[test]
fn refusals_change_nothing() {
    let mut doc = with_core(KNOWN);
    let before = bytes(&doc);
    let cases: Vec<(Vec<CorePropertyChange>, &str)> = vec![
        (
            vec![CorePropertyChange::new(
                CoreProperty::Revision,
                CoreValue::Int(0),
            )],
            "invalid_revision",
        ),
        (
            vec![CorePropertyChange::new(
                CoreProperty::Revision,
                CoreValue::Text("7".into()),
            )],
            "invalid_revision",
        ),
        (
            vec![text(CoreProperty::Created, "2001-02-03")],
            "core_property_requires_datetime",
        ),
        (
            vec![text(CoreProperty::Subject, &"x".repeat(256))],
            "core_property_too_long",
        ),
        (
            vec![text(CoreProperty::Keywords, &"\u{1F600}".repeat(256))],
            "core_property_too_long",
        ),
        (
            vec![text(CoreProperty::Title, "a\u{1}b")],
            "xml_incompatible_text",
        ),
        // Upstream would have assigned the title first (no rollback).
        (
            vec![
                text(CoreProperty::Title, "First"),
                CorePropertyChange::new(CoreProperty::Revision, CoreValue::Int(-3)),
            ],
            "invalid_revision",
        ),
    ];
    for (changes, reason) in cases {
        let err = doc.set_core_properties(&changes, None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert_eq!(err.reason(), reason);
        assert_eq!(bytes(&doc), before);
    }
    // 255 scalars (astral characters count once, as in Python) is fine.
    doc.set_core_properties(
        &[text(CoreProperty::Keywords, &"\u{1F600}".repeat(255))],
        None,
    )
    .unwrap();
    // `str(value)` for a non-string text value.
    let props = doc
        .set_core_properties(
            &[CorePropertyChange::new(
                CoreProperty::Version,
                CoreValue::Int(42),
            )],
            None,
        )
        .unwrap();
    assert_eq!(props.version, "42");
    // A refused write on a package without core properties adds no part.
    let mut bare =
        Document::open_bytes(&build_zip(&minimal_members()), &Limits::default()).unwrap();
    let before = bytes(&bare);
    assert!(
        bare.set_core_properties(
            &[CorePropertyChange::new(
                CoreProperty::Revision,
                CoreValue::Int(0)
            )],
            None
        )
        .is_err()
    );
    assert_eq!(bytes(&bare), before);
    assert!(bare.patch_bytes().unwrap().1.added_parts.is_empty());
}
