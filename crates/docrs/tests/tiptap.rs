//! TipTap document JSON (`editor.getJSON()`) as a source: the fixtures a
//! Tiptap 3 editor produced (`tests/fixtures/markdown/tiptap`, generated
//! through Tiptap's own schema) render as the same document from their JSON
//! as from their HTML; nodes the HTML only hints at (math, details,
//! mentions) come through from the JSON; what has no Word equivalent is
//! reported; what is not a document is refused with nothing changed.

use std::path::PathBuf;

use docrs::markdown::{
    MAX_NESTING_DEPTH, MarkdownOptions, MarkdownReport, insert_html, insert_tiptap,
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

    /// `(style name, text)` of every body paragraph.
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

/// Convert into the example reference document, export and reopen (saving
/// a copy under `DOCRS_MARKDOWN_EXPORT` when set).
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

/// A document of `blocks` (JSON node objects).
fn doc(blocks: &str) -> String {
    format!("{{\"type\":\"doc\",\"content\":[{blocks}]}}")
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

// ------------------------------------------------------- the editor's page

#[test]
fn the_json_renders_the_same_document_as_the_editors_html() {
    let options = MarkdownOptions {
        figure_captions: false,
        images: [(
            "https://store.example/logo.png?token=t".to_string(),
            png_bytes(),
        )]
        .into(),
        ..MarkdownOptions::default()
    };
    let json = tiptap_with("30-tiptap-core", &fixture("core.json"), &options);
    let html = html_with("30-tiptap-core-html", &fixture("core.html"), &options);
    assert!(json.report.warnings.is_empty(), "{}", json.messages());
    assert!(html.report.warnings.is_empty(), "{}", html.messages());
    assert_eq!(json.body(), html.body(), "the two sources render alike");
    assert_eq!(
        json.part("word/_rels/document.xml.rels"),
        html.part("word/_rels/document.xml.rels")
    );
    assert_eq!(json.report.counts.tables, html.report.counts.tables);
    assert_eq!(json.report.counts.images, 1);
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
    // for (and the cell's) carry one.
    assert_eq!(count(&body, "<w:jc w:val=\"left\"/>"), 0);
    assert_eq!(count(&body, "<w:jc "), 4);

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

// ------------------------------------------------------------- the mapping

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
    // app's own and not reported; the picture keeps its height.
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
    // Two columns: the header spans both; widths 1:3 from `colwidth`.
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

// ------------------------------------------------------------- refusals

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
