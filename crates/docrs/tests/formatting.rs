//! Effective-format resolver (upstream `formatting.py`): the observable
//! assertions of `tests/paper/test_formatting.py` and the formatting
//! regressions, with values frozen from the pinned oracle.

mod support;

use docrs::anchor::BlockTarget;
use docrs::formatting::{FormatTarget, FormatValue, UNRESOLVED};
use docrs::package::zip_read::read_archive;
use docrs::{BlockKind, Document, ErrorKind, FindOptions, Limits, RefusalKind, View};
use support::*;

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";

/// `minimal.docx` with its body replaced and `styles` appended.
fn with(body: &str, styles: &str) -> Document {
    let bytes = fixture(MINIMAL);
    let archive = read_archive(&bytes, &Limits::default()).unwrap();
    let members: Vec<RawMember> = archive
        .members
        .iter()
        .map(|member| {
            let text = || String::from_utf8(member.data.clone()).unwrap();
            let data = match member.name.as_str() {
                "word/document.xml" => {
                    let xml = text();
                    let start = xml.find("<w:body>").unwrap() + "<w:body>".len();
                    let end = xml.find("<w:sectPr").unwrap();
                    format!("{}{}{}", &xml[..start], body, &xml[end..]).into_bytes()
                }
                "word/styles.xml" => text()
                    .replace("</w:styles>", &format!("{styles}</w:styles>"))
                    .into_bytes(),
                _ => member.data.clone(),
            };
            RawMember::new(&member.name, &data)
        })
        .collect();
    Document::open_bytes(&build_zip(&members), &Limits::default()).unwrap()
}

fn r(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn rp(props: &str, text: &str) -> String {
    format!("<w:r><w:rPr>{props}</w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn styled(style: &str, inner: &str) -> String {
    format!("<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>{inner}</w:p>")
}

fn para_style(id: &str, extra: &str) -> String {
    format!(
        "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"{id}\"><w:name w:val=\"{id}\"/>{extra}</w:style>"
    )
}

fn char_style(id: &str, extra: &str) -> String {
    format!(
        "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"{id}\"><w:name w:val=\"{id}\"/>{extra}</w:style>"
    )
}

fn span(doc: &Document, text: &str) -> docrs::Span {
    doc.find_one(text, &FindOptions::default()).unwrap()
}

fn run_format(doc: &Document, text: &str) -> docrs::formatting::EffectiveFormat {
    doc.format_of(FormatTarget::RunAt(&span(doc, text)))
        .unwrap()
}

fn num(value: f64) -> FormatValue {
    FormatValue::Number(value)
}

#[test]
fn direct_run_formatting_and_doc_defaults() {
    let doc = with(
        &format!(
            "<w:p>{}</w:p><w:p>{}</w:p>",
            rp("<w:b/><w:sz w:val=\"28\"/>", "bolded"),
            r("plain")
        ),
        "",
    );
    let resolved = run_format(&doc, "bolded");
    assert_eq!(resolved["bold"].value, FormatValue::Bool(true));
    assert_eq!(resolved["bold"].source, "direct");
    assert_eq!(resolved["size_pt"].value, num(14.0));
    assert_eq!(resolved["size_pt"].source, "direct");

    let plain = run_format(&doc, "plain");
    assert_eq!(plain["italic"].value, FormatValue::Bool(false));
    assert_eq!(plain["italic"].source, "none");
    assert_eq!(plain["size_pt"].value, num(11.0));
    assert_eq!(plain["size_pt"].source, "doc_defaults");
    assert_eq!(plain["font_name"].value.as_str(), Some("theme:minorHAnsi"));
    assert!(plain.unresolved.contains(&"theme_font_resolution"));
    assert!(
        plain
            .unresolved
            .contains(&"table_style_conditional_formatting")
    );
    assert_eq!(plain.unresolved, UNRESOLVED.to_vec());
    let payload = plain.to_json();
    assert_eq!(payload["schema"], "paper_effective_format");
    assert_eq!(payload["version"], 1);
    let keys: Vec<&String> = payload["properties"].as_object().unwrap().keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert_eq!(keys.len(), 20);
    assert_eq!(payload["properties"]["style_name"]["value"], "Normal");
}

#[test]
fn style_chain_resolution() {
    let styles = [
        para_style("ChainBase", "<w:rPr><w:sz w:val=\"32\"/></w:rPr>"),
        para_style("ChainLeaf", "<w:basedOn w:val=\"ChainBase\"/>"),
        para_style(
            "SizedLeaf",
            "<w:basedOn w:val=\"ChainBase\"/><w:rPr><w:sz w:val=\"18\"/></w:rPr>",
        ),
        para_style("ColoredPara", "<w:rPr><w:sz w:val=\"30\"/></w:rPr>"),
        char_style("Emph", "<w:rPr><w:sz w:val=\"16\"/></w:rPr>"),
        "<w:style w:type=\"paragraph\" w:styleId=\"CycleA\"><w:name w:val=\"Cycle A\"/><w:basedOn w:val=\"CycleB\"/></w:style>".to_string(),
        "<w:style w:type=\"paragraph\" w:styleId=\"CycleB\"><w:name w:val=\"Cycle B\"/><w:basedOn w:val=\"CycleA\"/><w:rPr><w:sz w:val=\"40\"/></w:rPr></w:style>".to_string(),
    ]
    .concat();
    let body = [
        styled("ChainLeaf", &r("chained text")),
        styled("SizedLeaf", &r("near wins")),
        styled(
            "ColoredPara",
            &rp("<w:rStyle w:val=\"Emph\"/>", "char styled"),
        ),
        styled("CycleA", &r("cyclic")),
    ]
    .concat();
    let doc = with(&body, &styles);
    let chained = run_format(&doc, "chained text");
    assert_eq!(chained["size_pt"].value, num(16.0));
    assert_eq!(chained["size_pt"].source, "paragraph_style:ChainBase");
    let near = run_format(&doc, "near wins");
    assert_eq!(near["size_pt"].value, num(9.0));
    assert_eq!(near["size_pt"].source, "paragraph_style:SizedLeaf");
    let character = run_format(&doc, "char styled");
    assert_eq!(character["size_pt"].value, num(8.0));
    assert_eq!(character["size_pt"].source, "character_style:Emph");
    assert_eq!(run_format(&doc, "cyclic")["size_pt"].value, num(20.0));
}

#[test]
fn toggles_xor_through_style_layers() {
    let styles = [
        para_style("BoldPara", "<w:rPr><w:b/></w:rPr>"),
        char_style("BoldChar", "<w:rPr><w:b/></w:rPr>"),
    ]
    .concat();
    let body = [
        styled(
            "BoldPara",
            &rp("<w:rStyle w:val=\"BoldChar\"/>", "double bold cancels"),
        ),
        styled("BoldPara", &r("single layer")),
        styled("BoldPara", &rp("<w:b w:val=\"0\"/>", "explicitly unbold")),
    ]
    .concat();
    let doc = with(&body, &styles);
    let double = run_format(&doc, "double bold cancels");
    assert_eq!(double["bold"].value, FormatValue::Bool(false));
    assert_eq!(double["bold"].source, "toggle_xor");
    assert_eq!(
        double["bold"].chain,
        vec!["paragraph_style:BoldPara", "character_style:BoldChar"]
    );
    let single = run_format(&doc, "single layer");
    assert_eq!(single["bold"].value, FormatValue::Bool(true));
    assert_eq!(single["bold"].source, "paragraph_style:BoldPara");
    let direct = run_format(&doc, "explicitly unbold");
    assert_eq!(direct["bold"].value, FormatValue::Bool(false));
    assert_eq!(direct["bold"].source, "direct");
}

#[test]
fn paragraph_and_span_targets() {
    let styles = [
        para_style("LinkyPara", "<w:rPr><w:sz w:val=\"34\"/></w:rPr>"),
        para_style("LinkOnly", "<w:rPr><w:sz w:val=\"38\"/></w:rPr>"),
        char_style("AlsoBold", "<w:rPr><w:b/></w:rPr>"),
    ]
    .concat();
    let body = [
        format!(
            "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr>{}</w:p>",
            r("centered")
        ),
        format!(
            "<w:p>{}{}</w:p>",
            rp("<w:b/>", "bold half"),
            r(" plain half")
        ),
        styled(
            "LinkyPara",
            &format!(
                "<w:hyperlink w:anchor=\"DefinedTerm\">{}</w:hyperlink>",
                r("hyperlinked words")
            ),
        ),
        styled(
            "LinkOnly",
            &format!(
                "{}<w:hyperlink w:anchor=\"x\">{}</w:hyperlink>",
                rp(
                    "<w:b w:val=\"0\"/><w:sz w:val=\"16\"/>",
                    "ordinary text first "
                ),
                rp("<w:b/>", "only linked text here")
            ),
        ),
        format!(
            "<w:p>{}{}</w:p>",
            rp("<w:b/>", "direct half"),
            rp("<w:rStyle w:val=\"AlsoBold\"/>", " styled half")
        ),
    ]
    .concat();
    let doc = with(&body, &styles);
    let centered = span(&doc, "centered");
    let paragraph = doc
        .format_of(FormatTarget::Paragraph(BlockTarget::Span(&centered)))
        .unwrap();
    assert_eq!(paragraph["alignment"].value.as_str(), Some("center"));
    assert_eq!(paragraph["alignment"].source, "direct");
    assert_eq!(paragraph["style_name"].value.as_str(), Some("Normal"));
    let block = doc
        .blocks(View::Current)
        .unwrap()
        .into_iter()
        .find(|b| b.text == "centered")
        .unwrap();
    assert_eq!(
        doc.format_of(FormatTarget::Paragraph(BlockTarget::Block(&block)))
            .unwrap(),
        paragraph
    );
    assert_eq!(
        doc.format_of(FormatTarget::Paragraph(BlockTarget::Text("centered")))
            .unwrap(),
        paragraph
    );

    let mixed = doc
        .format_of(FormatTarget::Span(&span(&doc, "bold half plain half")))
        .unwrap();
    assert!(mixed["bold"].value.is_null());
    assert_eq!(mixed["bold"].source, "mixed");
    assert_eq!(mixed["size_pt"].value, num(11.0));

    let linked = run_format(&doc, "hyperlinked words");
    assert_eq!(linked["size_pt"].value, num(17.0));
    assert_eq!(linked["size_pt"].source, "paragraph_style:LinkyPara");
    let only = doc
        .format_of(FormatTarget::Span(&span(&doc, "only linked text here")))
        .unwrap();
    assert_eq!(only["bold"].value, FormatValue::Bool(true));
    assert_eq!(only["size_pt"].value, num(19.0));

    let agreeing = doc
        .format_of(FormatTarget::Span(&span(&doc, "direct half styled half")))
        .unwrap();
    assert_eq!(agreeing["bold"].value, FormatValue::Bool(true));
    assert_eq!(agreeing["bold"].source, "agreeing_layers");
    assert!(agreeing["bold"].chain.iter().any(|s| s == "direct"));
    assert!(
        agreeing["bold"]
            .chain
            .iter()
            .any(|s| s.starts_with("character_style:"))
    );
}

#[test]
fn declares_uncomputed_categories_unresolved() {
    let doc = with(
        &format!(
            "<w:p><w:pPr><w:spacing w:after=\"120\"/></w:pPr>{}</w:p>",
            rp("<w:shd w:fill=\"FFFF00\"/>", "formatted")
        ),
        "",
    );
    let resolved = run_format(&doc, "formatted");
    for key in ["run_shading", "paragraph_spacing", "theme_color_resolution"] {
        assert!(resolved.unresolved.contains(&key));
    }
}

#[test]
fn rejects_unsupported_and_stale_targets() {
    let mut doc = with(
        &format!(
            "<w:p>{}</w:p><w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"100\"/></w:tblGrid><w:tr><w:tc><w:p>{}</w:p></w:tc></w:tr></w:tbl>",
            r("some words here"),
            r("in a cell")
        ),
        "<w:style w:type=\"paragraph\" w:styleId=\"Bad\"><w:name w:val=\"Bad\"/><w:rPr><w:sz w:val=\"big\"/></w:rPr></w:style>",
    );
    let table = doc
        .blocks(View::Current)
        .unwrap()
        .into_iter()
        .find(|b| b.kind == BlockKind::Table)
        .unwrap();
    let err = doc
        .format_of(FormatTarget::Paragraph(BlockTarget::Block(&table)))
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    let cell = span(&doc, "in a cell");
    assert!(
        doc.format_of(FormatTarget::Paragraph(BlockTarget::Span(&cell)))
            .is_ok()
    );

    let other = with(&format!("<w:p>{}</w:p>", r("some words here")), "");
    let foreign = span(&other, "some words here");
    let err = doc.format_of(FormatTarget::Span(&foreign)).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));

    let words = span(&doc, "some words here");
    doc.replace(&words, "other words here", &Default::default())
        .unwrap();
    let err = doc.format_of(FormatTarget::Span(&words)).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    let err = doc.format_of(FormatTarget::RunAt(&words)).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
}

#[test]
fn invalid_size_is_an_argument_error() {
    let doc = with(
        &styled("Bad", &r("bad size")),
        "<w:style w:type=\"paragraph\" w:styleId=\"Bad\"><w:name w:val=\"Bad\"/><w:rPr><w:sz w:val=\"big\"/></w:rPr></w:style>",
    );
    let err = doc
        .format_of(FormatTarget::RunAt(&span(&doc, "bad size")))
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
}

#[test]
fn no_styles_part_reads_bundled_defaults() {
    let bytes = minimal_package_with(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>no styles part here</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>unstyled heading</w:t></w:r></w:p></w:body></w:document>",
    );
    let doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    // Frozen from the oracle (which adds the default styles part first).
    let run = run_format(&doc, "no styles part here");
    assert_eq!(run["size_pt"].value, num(12.0));
    assert_eq!(run["size_pt"].source, "doc_defaults");
    assert_eq!(run["font_name"].value.as_str(), Some("theme:minorHAnsi"));
    assert_eq!(run["style_name"].value.as_str(), Some("Normal"));
    assert_eq!(run["style_name"].source, "paragraph_style:Normal");
    let heading = doc
        .format_of(FormatTarget::Paragraph(BlockTarget::Text(
            "unstyled heading",
        )))
        .unwrap();
    assert!(heading["style_name"].value.is_null());
    assert_eq!(heading["style_name"].source, "paragraph_style:Heading1");
    assert_eq!(heading["size_pt"].value, num(12.0));
    let (out, patch) = doc.patch_bytes().unwrap();
    assert_eq!(patch.to_json()["changed_parts"], serde_json::json!([]));
    assert_eq!(out, bytes);
}
