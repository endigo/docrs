//! TipTap document JSON (`editor.getJSON()`) as a source: the fixtures a
//! Tiptap 3 editor produced (`tests/fixtures/markdown/tiptap`, generated
//! through Tiptap's own schema) render as the same document from their JSON
//! as from their HTML; nodes the HTML only hints at (math, details,
//! mentions) come through from the JSON; what has no Word equivalent is
//! reported; what is not a document is refused with nothing changed.

use std::path::PathBuf;

use docrs::markdown::{
    MAX_NESTING_DEPTH, MarkdownOptions, MarkdownReport, insert_html, insert_markdown, insert_tiptap,
};
use docrs::{Document, ErrorKind, Limits};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(repo().join("tests/fixtures/markdown/tiptap").join(name))
        .unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// The attached example reference document (restyled starter, A4, header
/// and footer).
fn reference() -> Document {
    Document::open_path(
        repo().join("examples/markdown/reference.docx"),
        &Limits::default(),
    )
    .expect("examples/markdown/reference.docx opens")
}

/// A 3×2 PNG at 150 dpi.
const PNG: &str = "89504e470d0a1a0a0000000d49484452000000030000000208020000001216f14d0000000970485973000017120000171201679fd2520000000b49444154789c6360800100000a00017f80745e0000000049454e44ae426082";

fn png_bytes() -> Vec<u8> {
    (0..PNG.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&PNG[i..i + 2], 16).unwrap())
        .collect()
}

struct Export {
    doc: Document,
    report: MarkdownReport,
}

impl Export {
    fn part(&self, name: &str) -> String {
        String::from_utf8(docrs::xml::serialize(self.doc.package().xml(name).unwrap())).unwrap()
    }

    fn body(&self) -> String {
        self.part("word/document.xml")
    }

    fn paragraphs(&self) -> Vec<(String, String)> {
        self.doc
            .paragraphs()
            .unwrap()
            .iter()
            .map(|p| {
                let style = p
                    .style(&self.doc)
                    .unwrap()
                    .and_then(|s| s.name)
                    .unwrap_or_else(|| "Normal".into());
                (style, p.text(&self.doc).unwrap())
            })
            .collect()
    }

    fn codes(&self) -> Vec<&str> {
        self.report.warnings.iter().map(|w| w.code).collect()
    }

    fn messages(&self) -> String {
        self.report
            .warnings
            .iter()
            .map(|w| w.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn export_with(
    name: &str,
    insert: impl FnOnce(&mut Document) -> docrs::Result<MarkdownReport>,
) -> Export {
    let mut doc = reference();
    let report = insert(&mut doc).unwrap_or_else(|err| panic!("{name}: conversion failed: {err}"));
    let (bytes, _) = doc.patch_bytes().unwrap();
    if let Some(dir) = std::env::var_os("DOCRS_MARKDOWN_EXPORT") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.docx")), &bytes).unwrap();
    }
    Export {
        doc: Document::open_bytes(&bytes, &Limits::default())
            .unwrap_or_else(|err| panic!("{name}: exported file does not reopen: {err}")),
        report,
    }
}

fn tiptap_with(name: &str, json: &str, options: &MarkdownOptions) -> Export {
    export_with(name, |doc| insert_tiptap(doc, json, options))
}

fn tiptap(name: &str, json: &str) -> Export {
    tiptap_with(name, json, &MarkdownOptions::default())
}

fn html_with(name: &str, html: &str, options: &MarkdownOptions) -> Export {
    export_with(name, |doc| insert_html(doc, html, options))
}

fn doc(blocks: &str) -> String {
    format!("{{\"type\":\"doc\",\"content\":[{blocks}]}}")
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn paragraph_holding<'a>(body: &'a str, needle: &str) -> &'a str {
    let at = body
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {body}"));
    let start = body[..at].rfind("<w:p>").unwrap();
    let end = at + body[at..].find("</w:p>").unwrap();
    &body[start..end]
}

fn page_options() -> MarkdownOptions {
    MarkdownOptions {
        figure_captions: false,
        images: [
            (
                "https://store.example/logo.png?token=t".to_string(),
                png_bytes(),
            ),
            (
                "https://store.example/seal.png?token=s".to_string(),
                png_bytes(),
            ),
        ]
        .into(),
        ..MarkdownOptions::default()
    }
}

#[test]
fn the_json_renders_the_same_document_as_the_editors_html() {
    let options = page_options();
    let json = tiptap_with("30-tiptap-core", &fixture("core.json"), &options);
    let html = html_with("30-tiptap-core-html", &fixture("core.html"), &options);
    assert!(json.report.warnings.is_empty(), "{}", json.messages());
    assert!(html.report.warnings.is_empty(), "{}", html.messages());
    assert_eq!(json.body(), html.body(), "the two sources render alike");
    assert_eq!(
        json.part("word/_rels/document.xml.rels"),
        html.part("word/_rels/document.xml.rels")
    );
    assert_eq!(json.report.counts.tables, 2);
    assert_eq!(json.report.counts.images, 2);
    assert_eq!(json.report.counts.links, 1);

    let body = json.body();
    for needle in [
        "<w:color w:val=\"DC2626\"/>",
        "<w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\" w:cs=\"Courier New\"/>",
        "<w:sz w:val=\"27\"/>",
        "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"BBF7D0\"/>",
        "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FEF08A\"/>",
        "<w:highlight w:val=\"yellow\"/>",
        "<w:vertAlign w:val=\"subscript\"/>",
        "<w:vertAlign w:val=\"superscript\"/>",
        "<w:strike/>",
        "<w:u w:val=\"single\"/>",
        "<w:rStyle w:val=\"Hyperlink\"/>",
        "w:tooltip=\"Advice\"",
        "<w:jc w:val=\"center\"/>",
        "<w:jc w:val=\"right\"/>",
        "<w:jc w:val=\"both\"/>",
        "<w:gridSpan w:val=\"2\"/>",
        "<w:vMerge w:val=\"restart\"/>",
        "<w:vMerge/>",
        "<w:tblHeader/>",
        "<wp:extent cx=\"1905000\" cy=\"762000\"/>",
        "descr=\"Logo\"",
        "<w:br/>",
    ] {
        assert!(body.contains(needle), "{needle} in {body}");
    }
    assert!(
        json.part("word/_rels/document.xml.rels")
            .contains("Target=\"https://example.org/advice\" TargetMode=\"External\"")
    );
    // The editor's default alignment is not written: only the three asked
    // for, the cell's and the centred picture's.
    assert_eq!(count(&body, "<w:jc w:val=\"left\"/>"), 0);
    assert_eq!(count(&body, "<w:jc "), 5);
    let logo = paragraph_holding(&body, "<wp:extent cx=\"1905000\" cy=\"762000\"/>");
    assert!(!logo.contains("<w:jc "), "{logo}");
    let seal = paragraph_holding(&body, "<wp:extent cx=\"1143000\" cy=\"1143000\"/>");
    assert!(seal.contains("<w:jc w:val=\"center\"/>"), "{seal}");

    let paragraphs = json.paragraphs();
    let texts: Vec<&str> = paragraphs.iter().map(|(_, text)| text.as_str()).collect();
    for (style, text) in [
        ("Heading 2", "Advice"),
        ("List Bullet", "one"),
        ("List Bullet 2", "nested"),
        ("List Bullet", "two"),
        ("List Number", "third"),
        ("List Number", "fourth"),
        ("List Bullet", "\u{2612} done"),
        ("List Bullet", "\u{2610} open"),
        ("Block Text", "quoted"),
        ("Heading 2", "After"),
    ] {
        assert!(
            paragraphs.iter().any(|(s, t)| s == style && t == text),
            "{style} {text:?} in {paragraphs:?}"
        );
    }
    assert!(
        paragraphs
            .iter()
            .any(|(style, text)| style == "Source Code" && text.starts_with("fn main() {}")),
        "{paragraphs:?}"
    );
    assert!(texts.contains(&""), "the empty paragraph stays");
    assert!(texts.iter().any(|t| t.starts_with("Plain bold italic underlined struck code sub sup a bold link red mono large shaded yellow pale.")), "{texts:?}");
    let numbering = json.part("word/numbering.xml");
    assert!(numbering.contains("<w:startOverride w:val=\"3\"/>"));
    // The second table: the column the editor sized at 120 px keeps it;
    // the two it left alone share the rest by their text.
    let (_, grid) = grids(&body);
    assert_eq!(grid.len(), 6, "{grid:?}");
    assert!((grid[3] - 1800).abs() <= 1, "{grid:?}");
    assert!(grid[4] > 2 * grid[5], "{grid:?}");
}

#[test]
fn nodes_the_html_only_hints_at_come_through_from_the_json() {
    let out = tiptap("31-tiptap-extras", &fixture("extras.json"));
    let body = out.body();
    assert!(body.contains("<m:oMath>"), "{body}");
    assert!(body.contains("<m:oMathPara>"), "{body}");
    assert!(body.contains("mc"), "{body}");
    let text: String = out
        .paragraphs()
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("See @Jane and :rocket: for"), "{text}");
    assert!(text.contains("More\nhidden"), "{text}");
    assert!(body.contains("<w:b/>"), "{body}");
    assert!(
        out.part("word/_rels/document.xml.rels")
            .contains("Target=\"https://www.youtube.com/watch?v=dQw4w9WgXcQ\""),
    );
    assert_eq!(out.codes(), ["tiptap_node_dropped"], "{}", out.messages());
    assert!(out.messages().contains("\"emoji\""), "{}", out.messages());
}

#[test]
fn the_editors_default_alignment_is_left_to_the_template() {
    let out = tiptap(
        "32-tiptap-alignment",
        &doc(
            r#"{"type":"paragraph","attrs":{"textAlign":"left"},"content":[{"type":"text","text":"a"}]},
             {"type":"paragraph","attrs":{"textAlign":"start"},"content":[{"type":"text","text":"b"}]},
             {"type":"paragraph","attrs":{"textAlign":null},"content":[{"type":"text","text":"c"}]},
             {"type":"heading","attrs":{"level":3,"textAlign":"end"},"content":[{"type":"text","text":"d"}]},
             {"type":"paragraph","attrs":{"textAlign":"middle"},"content":[{"type":"text","text":"e"}]},
             {"type":"paragraph","attrs":{"textAlign":"sideways"},"content":[{"type":"text","text":"f"}]}"#,
        ),
    );
    let body = out.body();
    assert_eq!(count(&body, "<w:jc w:val=\"left\"/>"), 0, "{body}");
    assert_eq!(count(&body, "<w:jc w:val=\"right\"/>"), 1, "{body}");
    assert_eq!(count(&body, "<w:jc w:val=\"center\"/>"), 1, "{body}");
    assert_eq!(out.codes(), ["tiptap_attribute_dropped"]);
    assert!(
        out.messages().contains("textAlign on paragraph"),
        "{}",
        out.messages()
    );
    assert_eq!(out.paragraphs().len(), 6);
}

#[test]
fn a_picture_standing_alone_aligns_its_paragraph_by_its_own_text_align() {
    const LOGO: &str = r#""src":"https://store.example/logo.png?token=t","alt":"Logo","title":null,"width":200,"height":80"#;
    let out = tiptap_with(
        "32-tiptap-picture-alignment",
        &doc(&format!(
            r#"{{"type":"image","attrs":{{{LOGO},"fileId":"fil_1","textAlign":"center"}}}},
             {{"type":"paragraph","attrs":{{"textAlign":null}},"content":[{{"type":"image","attrs":{{{LOGO},"textAlign":"right"}}}}]}},
             {{"type":"paragraph","attrs":{{"textAlign":"justify"}},"content":[{{"type":"image","attrs":{{{LOGO},"textAlign":"justify"}}}}]}},
             {{"type":"image","attrs":{{{LOGO},"textAlign":"end"}}}},
             {{"type":"image","attrs":{{{LOGO},"textAlign":"left"}}}},
             {{"type":"paragraph","content":[{{"type":"image","attrs":{{{LOGO},"textAlign":null}}}}]}},
             {{"type":"heading","attrs":{{"level":2}},"content":[{{"type":"image","attrs":{{{LOGO},"textAlign":"center"}}}}]}}"#
        )),
        &page_options(),
    );
    assert!(out.report.warnings.is_empty(), "{}", out.messages());
    assert_eq!(out.report.counts.images, 7);
    let body = out.body();
    assert_eq!(count(&body, "<w:jc w:val=\"center\"/>"), 2, "{body}");
    assert_eq!(count(&body, "<w:jc w:val=\"right\"/>"), 2, "{body}");
    assert_eq!(count(&body, "<w:jc w:val=\"both\"/>"), 1, "{body}");
    assert_eq!(count(&body, "<w:jc w:val=\"left\"/>"), 0, "{body}");
    assert_eq!(out.paragraphs().len(), 7);

    // The same picture from the editor's HTML: an app's `div` around it, or
    // Tiptap's own `text-align` on the `img`.
    let json = tiptap_with(
        "32-tiptap-picture-json",
        &doc(&format!(
            r#"{{"type":"image","attrs":{{{LOGO},"fileId":"fil_1","textAlign":"center"}}}}"#
        )),
        &page_options(),
    );
    let wrapped = html_with(
        "32-tiptap-picture-div",
        r#"<div data-image-align="center" style="text-align: center;"><img src="https://store.example/logo.png?token=t" alt="Logo" width="200" height="80" data-file-id="fil_1"></div>"#,
        &page_options(),
    );
    let styled = html_with(
        "32-tiptap-picture-img",
        r#"<img style="text-align: center;" src="https://store.example/logo.png?token=t" alt="Logo" width="200" height="80">"#,
        &page_options(),
    );
    assert!(wrapped.report.warnings.is_empty(), "{}", wrapped.messages());
    assert!(styled.report.warnings.is_empty(), "{}", styled.messages());
    assert_eq!(json.body(), wrapped.body());
    assert_eq!(json.body(), styled.body());
    let body = json.body();
    let picture = paragraph_holding(&body, "<w:drawing>");
    assert!(picture.contains("<w:jc w:val=\"center\"/>"), "{picture}");

    let out = tiptap_with(
        "32-tiptap-picture-dropped",
        &doc(&format!(
            r#"{{"type":"paragraph","content":[{{"type":"text","text":"before "}},{{"type":"image","attrs":{{{LOGO},"textAlign":"center"}}}},{{"type":"text","text":" after"}}]}},
             {{"type":"paragraph","attrs":{{"textAlign":"right"}},"content":[{{"type":"image","attrs":{{{LOGO},"textAlign":"center"}}}}]}},
             {{"type":"image","attrs":{{{LOGO},"textAlign":"sideways"}}}}"#
        )),
        &page_options(),
    );
    assert_eq!(
        out.codes(),
        ["tiptap_attribute_dropped"],
        "{}",
        out.messages()
    );
    assert!(
        out.messages().contains("textAlign on image"),
        "{}",
        out.messages()
    );
    assert_eq!(out.report.counts.images, 3);
    let body = out.body();
    assert_eq!(count(&body, "<w:jc w:val=\"center\"/>"), 0, "{body}");
    assert_eq!(count(&body, "<w:jc w:val=\"right\"/>"), 1, "{body}");
    assert_eq!(out.paragraphs()[0].1, "before  after");
}

#[test]
fn unknown_nodes_marks_and_attributes_are_reported_and_the_text_kept() {
    let out = tiptap(
        "33-tiptap-unknown",
        &doc(
            r#"{"type":"callout","attrs":{"kind":"info"},"content":[{"type":"paragraph","content":[{"type":"text","text":"inside a callout"}]}]},
             {"type":"paragraph","content":[
                {"type":"text","text":"commented","marks":[{"type":"comment","attrs":{"id":"c1"}},{"type":"bold"}]},
                {"type":"text","text":" spaced","marks":[{"type":"textStyle","attrs":{"lineHeight":"2","letterSpacing":"1px","color":""}}]},
                {"type":"sticker","attrs":{"id":"s1"}}]},
             {"type":"orderedList","attrs":{"start":1,"type":"a"},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"lettered"}]}]}]},
             {"type":"paragraph","content":[{"type":"image","attrs":{"src":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASYtFNAAAACXBIWXMAABcSAAAXEgFnn9JSAAAAC0lEQVR4nGNggAEAAAoAAX+AdF4AAAAASUVORK5CYII=","width":"wide","height":"20px"}}]}"#,
        ),
    );
    let text: Vec<String> = out.paragraphs().into_iter().map(|(_, t)| t).collect();
    assert_eq!(text[0], "inside a callout");
    assert_eq!(text[1], "commented spaced");
    assert_eq!(text[2], "lettered");
    assert!(
        out.body().contains("<w:b/>"),
        "the known mark still applies"
    );
    assert_eq!(
        out.codes(),
        [
            "tiptap_node_dropped",
            "tiptap_node_dropped",
            "tiptap_mark_dropped",
            "tiptap_attribute_dropped",
            "tiptap_attribute_dropped",
            "tiptap_attribute_dropped",
        ],
        "{}",
        out.messages()
    );
    let messages = out.messages();
    for needle in [
        "\"callout\"",
        "\"sticker\"",
        "\"comment\"",
        "lineHeight on textStyle",
        "type on orderedList",
        "width on image",
    ] {
        assert!(messages.contains(needle), "{needle}: {messages}");
    }
    // An unknown attribute that is not formatting (`letterSpacing`) is an
    // app's own and not reported.
    assert!(!messages.contains("letterSpacing"), "{messages}");
    assert_eq!(out.report.counts.images, 1);
    assert!(out.body().contains("cy=\"190500\""), "20px high");

    let mut document = reference();
    let before = document.patch_bytes().unwrap().0;
    let err = insert_tiptap(
        &mut document,
        &doc(r#"{"type":"paragraph","content":[{"type":"text","text":"x","marks":[{"type":"comment"}]}]}"#),
        &MarkdownOptions {
            strict: true,
            ..MarkdownOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.reason(), "markdown_not_faithful");
    assert!(err.message().contains("\"comment\""), "{err}");
    assert_eq!(document.patch_bytes().unwrap().0, before);
}

#[test]
fn lists_tables_images_and_breaks_map_from_hand_written_nodes() {
    let out = tiptap_with(
        "34-tiptap-structure",
        &doc(r#"{"type":"taskList","content":[
                {"type":"taskItem","attrs":{"checked":true},"content":[{"type":"paragraph","content":[{"type":"text","text":"done"}]},
                    {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"sub"}]}]}]}]},
                {"type":"taskItem","attrs":{"checked":"false"},"content":[{"type":"paragraph","content":[{"type":"text","text":"open"}]}]}]},
             {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"stray"}]}]},
             {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"strays"}]}]},
             {"type":"table","content":[
                {"type":"tableRow","content":[
                    {"type":"tableHeader","attrs":{"colspan":2,"colwidth":[100,300]},"content":[{"type":"paragraph","content":[{"type":"text","text":"Both"}]}]}]},
                {"type":"tableRow","content":[
                    {"type":"tableCell","attrs":{"rowspan":2,"align":"right"},"content":[{"type":"paragraph","content":[{"type":"text","text":"tall"}]}]},
                    {"type":"tableCell","attrs":{"colwidth":null},"content":[{"type":"paragraph","content":[{"type":"text","text":"b"}]}]}]},
                {"type":"tableRow","content":[
                    {"type":"tableCell","content":[{"type":"paragraph","content":[{"type":"text","text":"c"}]}]}]}]},
             {"type":"image","attrs":{"src":"img-1","alt":"Chart","title":"","width":"150px","height":null}},
             {"type":"pageBreak"},
             {"type":"paragraph","content":[{"type":"text","text":"page two"}]},
             {"type":"page-break","attrs":{"pageBreakBefore":true}},
             {"type":"paragraph","content":[{"type":"text","text":"page three"}]},
             {"type":"horizontalRule"},
             {"type":"codeBlock","attrs":{"language":"js"},"content":[{"type":"text","text":"a"},{"type":"hardBreak"},{"type":"text","text":"  b"}]},
             {"type":"blockquote","content":[{"type":"paragraph","attrs":{"textAlign":"center"},"content":[{"type":"text","text":"centred quote"}]}]},
             {"type":"paragraph","content":[{"type":"text","text":"  "}]},
             {"type":"text","text":"loose text "},{"type":"text","text":"joins","marks":[{"type":"italic"}]}"#),
        &MarkdownOptions {
            figure_captions: false,
            images: [("img-1".to_string(), png_bytes())].into(),
            ..MarkdownOptions::default()
        },
    );
    assert!(out.report.warnings.is_empty(), "{}", out.messages());
    let paragraphs = out.paragraphs();
    let expect = [
        ("List Bullet", "\u{2612} done"),
        ("List Bullet 2", "sub"),
        ("List Bullet", "\u{2610} open"),
        ("List Bullet", "stray"),
        ("List Bullet", "strays"),
    ];
    for (i, (style, text)) in expect.iter().enumerate() {
        assert_eq!(
            (paragraphs[i].0.as_str(), paragraphs[i].1.as_str()),
            (*style, *text),
            "{paragraphs:?}"
        );
    }
    let body = out.body();
    assert_eq!(count(&body, "<w:gridCol "), 2, "{body}");
    assert_eq!(count(&body, "<w:gridSpan w:val=\"2\"/>"), 1);
    assert_eq!(count(&body, "<w:vMerge w:val=\"restart\"/>"), 1);
    assert_eq!(count(&body, "<w:vMerge/>"), 1);
    assert_eq!(count(&body, "<w:tblHeader/>"), 1);
    let grid: Vec<i64> = body
        .match_indices("<w:gridCol w:w=\"")
        .map(|(at, m)| {
            let rest = &body[at + m.len()..];
            rest[..rest.find('"').unwrap()].parse().unwrap()
        })
        .collect();
    assert!((grid[1] - 3 * grid[0]).abs() <= 3, "{grid:?}");
    assert!(
        body.contains("<w:jc w:val=\"right\"/>"),
        "the cell's alignment"
    );
    // The picture at 150 px wide, its height from the file's proportions.
    assert!(
        body.contains("<wp:extent cx=\"1428750\" cy=\"952500\"/>"),
        "{body}"
    );
    assert_eq!(count(&body, "<w:br w:type=\"page\"/>"), 2);
    assert!(
        paragraphs
            .iter()
            .any(|(s, t)| s == "Source Code" && t.contains("a") && t.contains("  b"))
    );
    assert!(
        paragraphs
            .iter()
            .any(|(s, t)| s == "Block Text" && t == "centred quote")
    );
    assert!(body.contains("<w:jc w:val=\"center\"/>"));
    // A paragraph of spaces stays: the editor wrote it.
    let texts: Vec<&str> = paragraphs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(texts.contains(&"  "), "{texts:?}");
    assert_eq!(*texts.last().unwrap(), "loose text joins");
    assert!(body.contains("<w:i/>"));
}

#[test]
fn links_follow_the_scheme_policy_and_pictures_the_resource_policy() {
    let out = tiptap(
        "35-tiptap-links",
        &doc(r#"{"type":"paragraph","content":[
                {"type":"text","text":"ok","marks":[{"type":"link","attrs":{"href":"https://example.org","title":"T"}}]},
                {"type":"text","text":" mail","marks":[{"type":"link","attrs":{"href":"mailto:a@b.c"}}]},
                {"type":"text","text":" script","marks":[{"type":"link","attrs":{"href":"javascript:alert(1)"}}]},
                {"type":"text","text":" none","marks":[{"type":"link","attrs":{"href":null}}]}]},
             {"type":"paragraph","content":[{"type":"image","attrs":{"src":"https://cdn.example/missing.png","alt":"gone"}}]},
             {"type":"paragraph","content":[{"type":"image","attrs":{"src":"../../etc/passwd","alt":"local"}}]}"#),
    );
    let body = out.body();
    assert_eq!(count(&body, "<w:hyperlink "), 2, "{body}");
    assert!(body.contains("w:tooltip=\"T\""));
    let text: Vec<String> = out.paragraphs().into_iter().map(|(_, t)| t).collect();
    assert_eq!(text[0], "ok mail script none");
    assert_eq!(out.report.counts.images, 0);
    assert_eq!(
        out.codes(),
        [
            "link_scheme_skipped",
            "remote_image_skipped",
            "image_not_found"
        ],
        "{}",
        out.messages()
    );
}

#[test]
fn a_rule_is_one_plain_line_from_every_source() {
    let border = "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr><w:spacing w:before=\"120\" w:after=\"120\"/>";
    let mark = "<w:rPr><w:sz w:val=\"2\"/><w:szCs w:val=\"2\"/></w:rPr></w:pPr></w:p>";
    let options = MarkdownOptions::default();
    let json = tiptap(
        "40-rule",
        &doc(concat!(
            "{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"a\"}]},",
            "{\"type\":\"horizontalRule\"},",
            "{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"b\"}]}"
        )),
    );
    let html = html_with("40-rule-html", "<p>a</p><hr><p>b</p>", &options);
    let markdown = export_with("40-rule-md", |doc| {
        insert_markdown(doc, "a\n\n---\n\nb\n", &options)
    });
    assert_eq!(json.body(), html.body());
    assert_eq!(json.body(), markdown.body());
    let body = json.body();
    assert_eq!(count(&body, &format!("{border}{mark}")), 1, "{body}");
    for shape in ["<v:rect", "o:hr=", "<w:pict>", "<w:drawing>"] {
        assert!(!body.contains(shape), "{shape} in {body}");
    }
    // Inside a list item the line starts at the item's indent.
    let nested = tiptap(
        "40-rule-in-item",
        &doc(concat!(
            "{\"type\":\"bulletList\",\"content\":[{\"type\":\"listItem\",\"content\":[",
            "{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"item\"}]},",
            "{\"type\":\"horizontalRule\"}]}]}"
        )),
    );
    let body = nested.body();
    let indented = format!("{border}<w:ind w:left=\"360\"/>{mark}");
    assert_eq!(count(&body, &indented), 1, "{body}");
}

#[test]
fn a_table_of_contents_is_words_content_control() {
    // Titled as the example reference document's own contents is.
    let open = "<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val=\"TOCHeading\"/></w:pPr><w:r><w:t>Contents</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val=\"TOC1\"/>";
    let headings = concat!(
        "{\"type\":\"heading\",\"attrs\":{\"level\":1},\"content\":[{\"type\":\"text\",\"text\":\"One\"}]},",
        "{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"x\"}]},",
        "{\"type\":\"heading\",\"attrs\":{\"level\":2},\"content\":[{\"type\":\"text\",\"text\":\"Two\"}]}"
    );
    let node = tiptap(
        "41-toc-node",
        &doc(&format!("{{\"type\":\"tableOfContents\"}},{headings}")),
    );
    let body = node.body();
    assert_eq!(count(&body, "<w:sdt>"), 1, "{body}");
    assert!(body.contains(open), "{body}");
    let close = body
        .find("</w:sdtContent></w:sdt>")
        .expect("the block closes");
    let block = &body[..close];
    assert!(
        block.contains(" TOC \\o \"1-3\" \\h \\z \\u </w:instrText>"),
        "{block}"
    );
    assert_eq!(count(block, "w:fldCharType=\"begin\""), 3);
    assert_eq!(count(block, "w:fldCharType=\"end\""), 3);
    assert_eq!(count(block, " PAGEREF "), 2);
    assert!(block.contains("<w:pStyle w:val=\"TOC2\"/>"), "{block}");
    assert!(!body[close..].contains("fldChar"), "{body}");
    assert!(!body.contains("pageBreakBefore"), "{body}");
    let options = MarkdownOptions {
        toc: Some(true),
        ..MarkdownOptions::default()
    };
    let json = tiptap_with("41-toc-option", &doc(headings), &options);
    let html = html_with(
        "41-toc-option-html",
        "<h1>One</h1><p>x</p><h2>Two</h2>",
        &options,
    );
    assert_eq!(json.body(), html.body());
    let body = json.body();
    assert_eq!(count(&body, "<w:sdt>"), 1, "{body}");
    assert!(body.contains(open), "{body}");
    let close = body.find("</w:sdtContent></w:sdt>").unwrap();
    assert!(body[close..].contains("<w:pageBreakBefore/>"), "{body}");
}

#[test]
fn what_is_not_a_document_is_refused_with_nothing_changed() {
    let mut document = reference();
    let before = document.patch_bytes().unwrap().0;
    for (source, needle) in [
        ("", "not JSON"),
        ("{\"type\":\"doc\",\"content\":[}", "not JSON"),
        ("[]", "the root"),
        ("\"doc\"", "the root"),
        ("{\"type\":\"paragraph\"}", "\"doc\""),
        ("{\"content\":[]}", "no \"type\""),
        (
            "{\"type\":\"doc\",\"content\":[null]}",
            "inside a \"doc\" node",
        ),
        (
            "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"text\":\"x\"}]}]}",
            "inside a \"paragraph\" node",
        ),
        (
            "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"x\",\"marks\":[\"bold\"]}]}]}",
            "a mark of a \"text\" node",
        ),
        (
            "{\"type\":\"doc\",\"content\":[{\"type\":\"table\",\"content\":[{\"type\":\"tableRow\",\"content\":[7]}]}]}",
            "inside a \"tableRow\" node",
        ),
    ] {
        let err = insert_tiptap(&mut document, source, &MarkdownOptions::default())
            .err()
            .unwrap_or_else(|| panic!("{source:?} was converted"));
        assert_eq!(err.kind(), ErrorKind::InvalidArgument, "{source:?}: {err}");
        assert_eq!(err.reason(), "tiptap_json_invalid", "{source:?}: {err}");
        assert!(err.message().contains(needle), "{source:?}: {err}");
    }
    assert_eq!(document.patch_bytes().unwrap().0, before);
}

#[test]
fn nesting_past_the_cap_is_refused_before_parsing() {
    // A quote is two levels (the node and its content); the document, the
    // paragraph and its text make the rest.
    let quotes = |n: usize| {
        let mut source = String::from("{\"type\":\"doc\",\"content\":[");
        for _ in 0..n {
            source.push_str("{\"type\":\"blockquote\",\"content\":[");
        }
        source
            .push_str("{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"x\"}]}");
        for _ in 0..n {
            source.push_str("]}");
        }
        source.push_str("]}");
        source
    };
    let deepest = (MAX_NESTING_DEPTH - 5) / 2;
    let out = tiptap("36-tiptap-deep", &quotes(deepest));
    assert_eq!(out.paragraphs()[0].1, "x");
    let mut document = reference();
    let before = document.patch_bytes().unwrap().0;
    for n in [deepest + 1, 50_000] {
        let err = insert_tiptap(&mut document, &quotes(n), &MarkdownOptions::default())
            .err()
            .unwrap_or_else(|| panic!("{n} quotes were converted"));
        assert_eq!(err.kind(), ErrorKind::ResourceLimit, "{n}: {err}");
        assert_eq!(err.reason(), "markdown_nesting_too_deep", "{n}: {err}");
        assert_eq!(
            err.limit_exceeded().expect("a budget").allowed,
            MAX_NESTING_DEPTH as u64
        );
    }
    // Brackets inside strings do not nest.
    let out = tiptap(
        "36-tiptap-brackets",
        &doc(&format!(
            "{{\"type\":\"paragraph\",\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}",
            "[{".repeat(200)
        )),
    );
    assert_eq!(out.paragraphs()[0].1.len(), 400);
    assert_eq!(document.patch_bytes().unwrap().0, before);
}

#[test]
fn wide_tables_are_narrowed_with_a_warning() {
    let cells: String = (0..70)
        .map(|i| format!("{{\"type\":\"tableCell\",\"content\":[{{\"type\":\"paragraph\",\"content\":[{{\"type\":\"text\",\"text\":\"c{i}\"}}]}}]}}"))
        .collect::<Vec<_>>()
        .join(",");
    let out = tiptap(
        "37-tiptap-wide",
        &doc(&format!(
            "{{\"type\":\"table\",\"content\":[{{\"type\":\"tableRow\",\"content\":[{cells}]}}]}}"
        )),
    );
    assert_eq!(out.codes(), ["table_columns_clamped"]);
    let body = out.body();
    assert_eq!(count(&body, "<w:gridCol "), 63);
    for i in 0..70 {
        assert!(body.contains(&format!(">c{i}<")), "c{i} lost");
    }
}

fn resized_table(rows: &[&[&str]], widths: &[Option<u32>]) -> (String, String) {
    let cols: String = widths
        .iter()
        .map(|width| match width {
            Some(px) => format!("<col style=\"width: {px}px;\">"),
            None => "<col style=\"min-width: 25px;\">".to_string(),
        })
        .collect();
    let min: u32 = widths.iter().map(|width| width.unwrap_or(25)).sum();
    let mut html =
        format!("<table style=\"min-width: {min}px;\"><colgroup>{cols}</colgroup><tbody>");
    let mut json = String::from("{\"type\":\"table\",\"content\":[");
    for (r, row) in rows.iter().enumerate() {
        let (tag, kind) = if r == 0 {
            ("th", "tableHeader")
        } else {
            ("td", "tableCell")
        };
        html.push_str("<tr>");
        if r > 0 {
            json.push(',');
        }
        json.push_str("{\"type\":\"tableRow\",\"content\":[");
        for (c, (text, width)) in row.iter().zip(widths).enumerate() {
            let attr = width
                .map(|px| format!(" colwidth=\"{px}\""))
                .unwrap_or_default();
            html.push_str(&format!(
                "<{tag} colspan=\"1\" rowspan=\"1\"{attr}><p>{text}</p></{tag}>"
            ));
            let colwidth = width.map_or("null".to_string(), |px| format!("[{px}]"));
            if c > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "{{\"type\":\"{kind}\",\"attrs\":{{\"colspan\":1,\"rowspan\":1,\"colwidth\":{colwidth}}},\"content\":[{{\"type\":\"paragraph\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}]}}"
            ));
        }
        html.push_str("</tr>");
        json.push_str("]}");
    }
    html.push_str("</tbody></table>");
    json.push_str("]}");
    (html, doc(&json))
}

fn grids(body: &str) -> (Vec<i64>, Vec<i64>) {
    let numbers = |after: &str| -> Vec<i64> {
        body.match_indices(after)
            .map(|(at, m)| {
                let rest = &body[at + m.len()..];
                rest[..rest.find('"').unwrap()].parse().unwrap()
            })
            .collect()
    };
    (
        numbers("<w:tblW w:type=\"dxa\" w:w=\""),
        numbers("<w:gridCol w:w=\""),
    )
}

#[test]
fn a_partly_resized_table_keeps_the_widths_the_editor_set() {
    const ASSETS: [&[&str]; 2] = [
        &["Asset", "Description", "Value"],
        &["Home", "Family home, owned jointly", "$1,200,000"],
    ];
    let render = |name: &str, html: &str, json: &str| -> (i64, Vec<i64>) {
        let from_html = html_with(&format!("{name}-html"), html, &MarkdownOptions::default());
        let from_json = tiptap_with(name, json, &MarkdownOptions::default());
        assert!(
            from_html.report.warnings.is_empty(),
            "{}",
            from_html.messages()
        );
        assert!(
            from_json.report.warnings.is_empty(),
            "{}",
            from_json.messages()
        );
        assert_eq!(
            from_json.body(),
            from_html.body(),
            "{name}: the two sources alike"
        );
        let (widths, grid) = grids(&from_json.body());
        assert_eq!(grid.iter().sum::<i64>(), widths[0], "{name}: {grid:?}");
        (widths[0], grid)
    };
    let resized = |name: &str, widths: &[Option<u32>]| {
        let (html, json) = resized_table(&ASSETS, widths);
        render(name, &html, &json)
    };
    // The first column dragged to 120 px stays 120 px (1800 twips); the
    // others share the rest by their text.
    let (width, first) = resized("38-tiptap-first-column", &[Some(120), None, None]);
    assert!((first[0] - 1800).abs() <= 1, "{first:?}");
    assert!(first[1] > 2 * first[2], "{first:?}");
    let (_, middle) = resized("38-tiptap-middle-column", &[None, Some(200), None]);
    assert!((middle[1] - 3000).abs() <= 1, "{middle:?}");
    assert!(middle[0] < middle[2], "{middle:?}");
    // Sized columns wider than the text: everything scaled down in
    // proportion, the unsized column from the editor's 25 px minimum.
    let (_, over) = resized("38-tiptap-over-width", &[Some(600), Some(400), None]);
    assert!(over[0] < 9000, "{over:?}");
    assert!((over[0] * 2 - over[1] * 3).abs() <= 3, "{over:?}");
    assert!(over[2] > 0 && over[2] < 375, "{over:?}");
    // Every column sized, or none, renders as before.
    let (_, all) = resized("38-tiptap-all-columns", &[Some(120), Some(420), Some(160)]);
    assert_eq!(all, [1613, 5647, 2152]);
    let (_, none) = resized("38-tiptap-no-columns", &[None, None, None]);
    assert_eq!(none, [1464, 5438, 2510]);
    // A spanning cell with no `colwidth` sizes nothing: the column keeps
    // the width a cell below it carries.
    let (_, spanned) = render(
        "38-tiptap-null-span",
        concat!(
            "<table style=\"min-width: 50px;\"><colgroup><col style=\"min-width: 25px;\"><col style=\"min-width: 25px;\"></colgroup><tbody>",
            "<tr><th colspan=\"2\" rowspan=\"1\"><p>Both</p></th></tr>",
            "<tr><td colspan=\"1\" rowspan=\"1\" colwidth=\"120\"><p>Home</p></td><td colspan=\"1\" rowspan=\"1\"><p>Family home, owned jointly</p></td></tr></tbody></table>",
        ),
        &doc(concat!(
            "{\"type\":\"table\",\"content\":[",
            "{\"type\":\"tableRow\",\"content\":[{\"type\":\"tableHeader\",\"attrs\":{\"colspan\":2,\"rowspan\":1,\"colwidth\":null},\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Both\"}]}]}]},",
            "{\"type\":\"tableRow\",\"content\":[{\"type\":\"tableCell\",\"attrs\":{\"colspan\":1,\"rowspan\":1,\"colwidth\":[120]},\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Home\"}]}]},",
            "{\"type\":\"tableCell\",\"attrs\":{\"colspan\":1,\"rowspan\":1,\"colwidth\":null},\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Family home, owned jointly\"}]}]}]}]}",
        )),
    );
    assert_eq!(spanned, [1800, width - 1800]);
}

#[test]
fn a_header_several_rows_deep_keeps_its_spans_and_the_resized_columns() {
    // `nested.{json,html}`: a five-row header over eight leaf columns, the
    // first dragged to 180 px and the fourth to 140 px, which the editor
    // writes onto every cell of those columns, as zero-filled lists on the
    // spanning ones.
    let json = tiptap("39-tiptap-nested", &fixture("nested.json"));
    let html = html_with(
        "39-tiptap-nested-html",
        &fixture("nested.html"),
        &MarkdownOptions::default(),
    );
    assert!(json.report.warnings.is_empty(), "{}", json.messages());
    assert!(html.report.warnings.is_empty(), "{}", html.messages());
    let body = json.body();
    assert_eq!(body, html.body(), "the two sources alike");
    assert_eq!(count(&body, "<w:gridSpan w:val=\"8\"/>"), 3);
    assert_eq!(count(&body, "<w:gridSpan w:val=\"4\"/>"), 2);
    assert_eq!(count(&body, "<w:vMerge w:val=\"restart\"/>"), 1);
    assert_eq!(count(&body, "<w:vMerge/>"), 4);
    assert_eq!(count(&body, "<w:tblHeader/>"), 5);
    let (widths, grid) = grids(&body);
    assert_eq!(grid.len(), 9, "{grid:?}");
    assert_eq!(grid.iter().sum::<i64>(), widths[0], "{grid:?}");
    assert!((grid[0] - 2700).abs() <= 1, "{grid:?}");
    assert!((grid[3] - 2100).abs() <= 1, "{grid:?}");
    // The other seven share the rest by their text, a header by its
    // longest word: "Residents" over "Total", none down at the floor.
    assert!(grid[1] > grid[4], "{grid:?}");
    assert_eq!(grid[1], grid[5], "{grid:?}");
    assert!(grid[1] < grid[0], "{grid:?}");
    assert!(grid.iter().all(|w| *w > 375), "{grid:?}");
    // The same table before any column was dragged: by text alone, the
    // control points' column is the widest.
    fn unsize(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(width) = map.get_mut("colwidth") {
                    *width = serde_json::Value::Null;
                }
                map.values_mut().for_each(unsize);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(unsize),
            _ => {}
        }
    }
    let mut undragged: serde_json::Value = serde_json::from_str(&fixture("nested.json")).unwrap();
    unsize(&mut undragged);
    let out = tiptap("39-tiptap-nested-unsized", &undragged.to_string());
    assert!(out.report.warnings.is_empty(), "{}", out.messages());
    let (widths, grid) = grids(&out.body());
    assert_eq!(grid.len(), 9, "{grid:?}");
    assert_eq!(grid.iter().sum::<i64>(), widths[0], "{grid:?}");
    assert!(grid.iter().all(|w| *w <= grid[0]), "{grid:?}");
    assert!(grid[1] > grid[4], "{grid:?}");
}
