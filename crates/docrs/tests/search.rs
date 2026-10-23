//! M2 read/locate regressions (values frozen from the pinned oracle; the
//! full cross-check is `docrs-compat m2-parity`).

mod support;

use docrs::{Document, ErrorKind, FindOptions, Limits, MatchPolicy, RefusalKind, View};
use support::*;

const DECLS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:x="urn:unknown""#;

fn doc_with(body: &str) -> Document {
    let xml = format!(r#"<w:document {DECLS}><w:body>{body}</w:body></w:document>"#);
    Document::open_bytes(&minimal_package_with(&xml), &Limits::default()).unwrap()
}

fn r(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn exact() -> FindOptions {
    FindOptions::default()
}

fn normalized() -> FindOptions {
    FindOptions {
        policy: MatchPolicy::Normalized,
        ..FindOptions::default()
    }
}

#[test]
fn exact_search_assembles_fragmented_runs() {
    let doc = doc_with(&format!(
        "<w:p>{}{}{}</w:p>",
        r("Pay"),
        r("ment te"),
        r("rms apply")
    ));
    let spans = doc.find_text("Payment terms", &exact()).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].text(), "Payment terms");
    assert_eq!(spans[0].raw_start(), 0);
    assert!(!spans[0].crosses_paragraphs());
}

#[test]
fn normalized_search_maps_expansions_back_to_source() {
    let doc = doc_with(&format!("<w:p>{}{}</w:p>", r("Stra"), r("ße und STRASSE")));
    let spans = doc.find_text("strasse", &normalized()).unwrap();
    let texts: Vec<&str> = spans.iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["Straße", "STRASSE"]);
    // A needle covering half of an expansion is not source-aligned.
    assert!(doc.find_text("stras", &normalized()).unwrap().len() == 1);
    assert!(doc.find_text("Straße", &exact()).unwrap().len() == 1);
    assert!(doc.find_text("strasse", &exact()).unwrap().is_empty());
}

#[test]
fn offsets_are_unicode_scalars() {
    let doc = doc_with(&format!("<w:p>{}</w:p>", r("😀😀 ᠮᠣᠩ é")));
    let span = doc.find_one("ᠮᠣᠩ", &exact()).unwrap();
    assert_eq!(span.raw_start(), 3);
    let span = doc.find_one("e\u{301}", &exact());
    assert!(span.is_err(), "precomposed é is not e + combining acute");
}

#[test]
fn paragraph_boundaries_are_single_newlines() {
    let doc = doc_with(&format!(
        "<w:p>{}</w:p><w:p>{}</w:p>",
        r("end of one"),
        r("start of two")
    ));
    let span = doc.find_one("one\nstart", &exact()).unwrap();
    assert!(span.crosses_paragraphs());
    // Exact needles cannot begin or end on a separator.
    assert!(doc.find_text("\nstart", &exact()).unwrap().is_empty());
    // Normalized matching collapses the separator into whitespace.
    assert_eq!(doc.find_text("ONE START", &normalized()).unwrap().len(), 1);
}

#[test]
fn barriers_split_text_and_views_filter_revisions() {
    let doc = doc_with(&format!(
        r#"<w:p>{}<w:r><w:sym w:char="F0FC"/></w:r>{}</w:p><w:p><w:ins w:id="1" w:author="A">{}</w:ins><w:del w:id="2" w:author="A"><w:r><w:delText>old</w:delText></w:r></w:del></w:p>"#,
        r("sym"),
        r("bol"),
        r("new")
    ));
    assert!(doc.find_text("symbol", &exact()).unwrap().is_empty());
    assert_eq!(doc.find_text("sym\u{FFFC}bol", &exact()).unwrap().len(), 1);
    assert_eq!(doc.find_text("new", &exact()).unwrap().len(), 1);
    assert!(doc.find_text("old", &exact()).unwrap().is_empty());
    let original = FindOptions {
        view: View::Original,
        ..exact()
    };
    assert!(doc.find_text("new", &original).unwrap().is_empty());
    assert_eq!(doc.find_text("old", &original).unwrap().len(), 1);
}

#[test]
fn find_one_refuses_ambiguity_and_absence() {
    let doc = doc_with(&format!("<w:p>{}</w:p>", r("the cat and the dog")));
    let err = doc.find_one("the", &exact()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Refusal(RefusalKind::AmbiguousTarget));
    let err = doc.find_one("bird", &exact()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Refusal(RefusalKind::TargetNotFound));
    let second = doc
        .find_one(
            "the",
            &FindOptions {
                nth: Some(2),
                ..exact()
            },
        )
        .unwrap();
    assert_eq!(second.raw_start(), 12);
    let ranked = doc
        .find_text(
            "the",
            &FindOptions {
                near: Some("dog".into()),
                ..exact()
            },
        )
        .unwrap();
    assert_eq!(ranked[0].raw_start(), 12);
    let err = doc
        .find_text(
            "the",
            &FindOptions {
                near: Some("dog".into()),
                nth: Some(1),
                ..exact()
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
}

#[test]
fn outline_reports_blocks_and_blind_regions() {
    let doc = doc_with(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{}</w:p><w:p/><w:sdt><w:sdtContent><w:p>{}</w:p></w:sdtContent></w:sdt><w:tbl><w:tr><w:tc><w:p>{}</w:p></w:tc><w:tc><w:p>{}</w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:rPr><w:vanish/></w:rPr><w:t>hidden</w:t></w:r></w:p>"#,
        r("Title"),
        r("controlled"),
        r("a"),
        r("b")
    ));
    let outline = doc.outline(View::Current).unwrap();
    assert_eq!(outline.story_parts, vec!["word/document.xml".to_string()]);
    let texts: Vec<&str> = outline.blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, vec!["Title", "", "controlled", "a\nb", "hidden"]);
    assert_eq!(outline.blocks[0].style_id.as_deref(), Some("Heading1"));
    assert!(outline.blocks[2].in_content_control);
    assert_eq!(outline.blocks[3].table.as_ref().unwrap().columns, 2);
    assert_eq!(outline.blind_region_counts["content_controls"], 1);
    assert_eq!(outline.blind_region_counts["hidden_text"], 1);
    assert_eq!(
        outline.to_json(),
        doc.outline(View::Current).unwrap().to_json()
    );
}

#[test]
fn fixture_story_parts_follow_upstream_order() {
    let doc = Document::open_bytes(
        &fixture("generated/feature-isolated/header-footer-sections.docx"),
        &Limits::default(),
    )
    .unwrap();
    let names: Vec<String> = doc.stories().into_iter().map(|s| s.name).collect();
    assert_eq!(names[0], "word/document.xml");
    assert!(names.iter().any(|n| n.starts_with("word/header")));
    let first_footer = names
        .iter()
        .position(|n| n.starts_with("word/footer"))
        .unwrap();
    let last_header = names
        .iter()
        .rposition(|n| n.starts_with("word/header"))
        .unwrap();
    assert!(last_header < first_footer);
}
