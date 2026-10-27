//! Markdown → DOCX, one feature per test. Every conversion is exported
//! (patch-saved), reopened through the guarded reader and inspected from
//! the reopened package. With `DOCRS_MARKDOWN_EXPORT=<dir>` each
//! exported file is also written there (`<test>.docx`) for inspection in
//! Word, LibreOffice or pandoc (`tools/markdown_check.sh`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use docrs::markdown::{
    MarkdownOptions, MarkdownReport, Placement, Role, StylePolicy, convert, reference_document,
    role_styles,
};
use docrs::{Document, ErrorKind, Limits, RefusalKind};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
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

struct Export {
    doc: Document,
    report: MarkdownReport,
    bytes: Vec<u8>,
}

impl Export {
    fn part(&self, name: &str) -> String {
        part_of(&self.doc, name)
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

    fn warning_codes(&self) -> Vec<&str> {
        self.report.warnings.iter().map(|w| w.code).collect()
    }
}

fn export_with(
    name: &str,
    markdown: &str,
    reference: Option<Document>,
    options: &MarkdownOptions,
) -> Export {
    let (doc, report) = convert(markdown, reference, options)
        .unwrap_or_else(|err| panic!("{name}: conversion failed: {err}"));
    let (bytes, _) = doc.patch_bytes().unwrap();
    if let Some(dir) = std::env::var_os("DOCRS_MARKDOWN_EXPORT") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.docx")), &bytes).unwrap();
    }
    let reopened = Document::open_bytes(&bytes, &Limits::default())
        .unwrap_or_else(|err| panic!("{name}: exported file does not reopen: {err}"));
    Export {
        doc: reopened,
        report,
        bytes,
    }
}

fn export(name: &str, markdown: &str) -> Export {
    export_with(
        name,
        markdown,
        Some(reference()),
        &MarkdownOptions::default(),
    )
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

const PNG_150DPI: &str = "89504e470d0a1a0a0000000d49484452000000030000000208020000001216f14d0000000970485973000017120000171201679fd2520000000b49444154789c6360800100000a00017f80745e0000000049454e44ae426082";

fn png_bytes() -> Vec<u8> {
    (0..PNG_150DPI.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&PNG_150DPI[i..i + 2], 16).unwrap())
        .collect()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn headings_take_heading_styles_and_bookmarks() {
    let out = export(
        "01-headings",
        "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n\n# Custom {#my-id}\n\n# One\n",
    );
    let styles: Vec<String> = out.paragraphs().into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        styles,
        [
            "Heading 1",
            "Heading 2",
            "Heading 3",
            "Heading 4",
            "Heading 5",
            "Heading 6",
            "Heading 1",
            "Heading 1"
        ]
    );
    let body = out.body();
    for name in ["one", "two", "three", "my_id", "one_1"] {
        assert!(
            body.contains(&format!("w:name=\"{name}\"")),
            "bookmark {name}: {body}"
        );
    }
    assert_eq!(
        count(&body, "<w:bookmarkStart"),
        count(&body, "<w:bookmarkEnd")
    );
    assert_eq!(out.report.counts.headings, 8);
}

#[test]
fn heading_offset_shifts_levels() {
    let options = MarkdownOptions {
        heading_offset: 1,
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "01b-heading-offset",
        "# A\n\n###### F\n",
        Some(reference()),
        &options,
    );
    let styles: Vec<String> = out.paragraphs().into_iter().map(|(s, _)| s).collect();
    assert_eq!(styles, ["Heading 2", "Heading 7"]);
}

#[test]
fn paragraphs_use_first_paragraph_then_body_text() {
    let out = export(
        "02-paragraphs",
        "# H\n\nFirst.\n\nSecond.\n\n- item\n\nAfter list.\n",
    );
    assert_eq!(
        out.paragraphs(),
        [
            ("Heading 1".into(), "H".into()),
            ("First Paragraph".into(), "First.".into()),
            ("Body Text".into(), "Second.".into()),
            ("List Bullet".into(), "item".into()),
            ("First Paragraph".into(), "After list.".into()),
        ]
    );
}

#[test]
fn inline_formatting_maps_to_run_properties() {
    let out = export(
        "03-inline",
        "**b** *i* ~~s~~ `code` <kbd>Ctrl</kbd> H~2~O x^2^ <u>u</u> <mark>m</mark> <sup>up</sup> <sub>dn</sub>\n",
    );
    let body = out.body();
    for fragment in [
        "<w:b/><w:bCs/></w:rPr><w:t>b</w:t>",
        "<w:i/><w:iCs/></w:rPr><w:t>i</w:t>",
        "<w:strike/></w:rPr><w:t>s</w:t>",
        "<w:rStyle w:val=\"VerbatimChar\"/></w:rPr><w:t>code</w:t>",
        "<w:rStyle w:val=\"VerbatimChar\"/></w:rPr><w:t>Ctrl</w:t>",
        "<w:vertAlign w:val=\"subscript\"/></w:rPr><w:t>2</w:t>",
        "<w:vertAlign w:val=\"superscript\"/></w:rPr><w:t>2</w:t>",
        "<w:u w:val=\"single\"/></w:rPr><w:t>u</w:t>",
        "<w:highlight w:val=\"yellow\"/></w:rPr><w:t>m</w:t>",
        "<w:vertAlign w:val=\"superscript\"/></w:rPr><w:t>up</w:t>",
        "<w:vertAlign w:val=\"subscript\"/></w:rPr><w:t>dn</w:t>",
    ] {
        assert!(body.contains(fragment), "missing {fragment}\n{body}");
    }
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
}

#[test]
fn soft_breaks_join_and_hard_breaks_break() {
    let out = export("04-breaks", "one\ntwo  \nthree\\\nfour<br>five\n");
    let (_, text) = &out.paragraphs()[0];
    assert_eq!(text, "one two\nthree\nfour\nfive");
    assert_eq!(count(&out.body(), "<w:br/>"), 3);
}

#[test]
fn external_links_are_hyperlink_relationships() {
    let out = export(
        "05-links",
        "[site](<https://example.com/a b> \"Tip\"), <https://rust-lang.org>, <me@example.com>, [rel](docs/other.md).\n",
    );
    let body = out.body();
    assert_eq!(count(&body, "<w:hyperlink "), 4);
    assert!(body.contains("w:tooltip=\"Tip\""));
    assert!(body.contains("<w:rStyle w:val=\"Hyperlink\"/>"));
    let rels = out.part("word/_rels/document.xml.rels");
    for target in [
        "https://example.com/a%20b",
        "https://rust-lang.org",
        "mailto:me@example.com",
        "docs/other.md",
    ] {
        assert!(
            rels.contains(&format!("Target=\"{target}\" TargetMode=\"External\"")),
            "{target}: {rels}"
        );
    }
    let links = out.doc.paragraphs().unwrap()[0]
        .hyperlinks(&out.doc)
        .unwrap();
    assert_eq!(links.len(), 4);
    assert_eq!(out.report.counts.links, 4);
}

#[test]
fn internal_links_point_to_heading_bookmarks() {
    let out = export(
        "06-internal-links",
        "See [later](#later-section) and [custom](#cid) and [gone](#nowhere).\n\n# Later Section\n\n# Other {#cid}\n",
    );
    let body = out.body();
    assert!(body.contains("w:anchor=\"later_section\""));
    assert!(body.contains("w:anchor=\"cid\""));
    assert!(body.contains("w:name=\"later_section\""));
    assert_eq!(out.warning_codes(), ["broken_internal_link"]);
    assert_eq!(out.paragraphs()[0].1, "See later and custom and gone.");
}

#[test]
fn footnotes_become_word_footnotes() {
    let out = export(
        "07-footnotes",
        "Claim.[^a] Again.[^b] Missing.[^zzz]\n\n[^a]: Note **A** with a [link](https://example.com).\n\n[^b]: First paragraph.\n\n    Second paragraph.\n",
    );
    let body = out.body();
    assert_eq!(count(&body, "<w:footnoteReference "), 2);
    assert!(body.contains("<w:rStyle w:val=\"FootnoteReference\"/>"));
    let notes = out.part("word/footnotes.xml");
    assert_eq!(count(&notes, "<w:footnoteRef/>"), 2);
    assert!(notes.contains("<w:pStyle w:val=\"FootnoteText\"/>"));
    assert!(notes.contains("<w:hyperlink "));
    assert!(notes.contains("Second paragraph."));
    let rels = out.part("word/_rels/footnotes.xml.rels");
    assert!(rels.contains("https://example.com"));
    // An undefined label is not a footnote reference in GFM: it stays text.
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
    assert!(out.paragraphs()[0].1.contains("[^zzz]"));
    // The reference document's sample footnote went with its body.
    assert_eq!(count(&notes, "<w:footnote w:id="), 2);
}

#[test]
fn bullet_lists_nest_through_levels() {
    let out = export("08-bullets", "- a\n- b\n  - c\n    - d\n- e\n");
    // The reference already defines the list styles; they are reused.
    let numbering = out.part("word/numbering.xml");
    assert_eq!(count(&numbering, "w:val=\"PaperMarkdownBullet\""), 1);
    assert_eq!(count(&numbering, "w:val=\"PaperMarkdownDecimal\""), 1);
    assert_eq!(out.report.counts.list_items, 5);
    // The reference defines List Bullet, List Bullet 2 and List Bullet 3
    // (separate lists, as Word's built-ins are), so each level takes its
    // own style and numbering.
    let styles: Vec<String> = out.paragraphs().into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        styles,
        [
            "List Bullet",
            "List Bullet",
            "List Bullet 2",
            "List Bullet 3",
            "List Bullet"
        ]
    );
    assert_eq!(out.report.styles["list_bullet"].name, "List Bullet");
    let options = MarkdownOptions {
        template_lists: false,
        ..MarkdownOptions::default()
    };
    let own = export_with(
        "08b-converter-bullets",
        "- a\n- b\n  - c\n",
        Some(reference()),
        &options,
    );
    assert!(own.paragraphs().iter().all(|(style, _)| style == "Compact"));
    for level in 0..=1 {
        assert!(own.body().contains(&format!("<w:ilvl w:val=\"{level}\"/>")));
    }
    assert!(!own.report.styles.contains_key("list_bullet"));
}

#[test]
fn ordered_lists_start_where_written_and_restart() {
    let out = export_with(
        "09-ordered",
        "3. c\n4. d\n\ntext\n\n1. a\n2. b\n   1. nested\n",
        None,
        &MarkdownOptions::default(),
    );
    let numbering = out.part("word/numbering.xml");
    assert!(numbering.contains("<w:startOverride w:val=\"3\"/>"));
    assert_eq!(count(&numbering, "<w:startOverride w:val=\"1\"/>"), 2);
    assert_eq!(count(&numbering, "<w:startOverride "), 3);
    assert!(numbering.contains("PaperMarkdownDecimal"));
}

#[test]
fn task_lists_get_ballot_boxes() {
    let out = export("10-tasks", "- [x] done\n- [ ] open\n");
    let texts: Vec<String> = out.paragraphs().into_iter().map(|(_, t)| t).collect();
    assert_eq!(texts, ["\u{2612} done", "\u{2610} open"]);
}

#[test]
fn loose_list_items_keep_continuation_paragraphs_indented() {
    let out = export(
        "11-loose-list",
        "1. first\n\n   more\n\n2. second\n\n   ```\n   code\n   ```\n",
    );
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0].0, "List Number");
    assert_eq!(paragraphs[1], ("Body Text".into(), "more".into()));
    assert_eq!(paragraphs[3], ("Source Code".into(), "code".into()));
    let body = out.body();
    let indents: Vec<&str> = body
        .split("<w:ind w:left=\"")
        .skip(1)
        .map(|s| &s[..s.find('"').unwrap()])
        .collect();
    assert_eq!(indents.len(), 2);
    assert_eq!(indents[0], indents[1]);
}

#[test]
fn quotes_nest_and_alerts_get_labels() {
    let out = export(
        "12-quotes",
        "> quoted\n>\n> > nested\n\n> [!TIP]\n> Use the reference document.\n",
    );
    let paragraphs = out.paragraphs();
    assert!(paragraphs.iter().all(|(style, _)| style == "Block Text"));
    assert_eq!(paragraphs[2].1, "Tip");
    assert!(out.body().contains("<w:ind w:left=\"1440\"/>"));
}

#[test]
fn code_blocks_keep_whitespace_tabs_and_lines() {
    let out = export(
        "13-code",
        "```rust\nfn main() {\n    let x = 1;\n\tlet y = 2;\n}\n```\n\n    indented\n",
    );
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0].0, "Source Code");
    assert_eq!(
        paragraphs[0].1,
        "fn main() {\n    let x = 1;\n\tlet y = 2;\n}"
    );
    assert_eq!(paragraphs[1], ("Source Code".into(), "indented".into()));
    let body = out.body();
    assert!(body.contains("<w:t xml:space=\"preserve\">    let x = 1;</w:t>"));
    assert!(body.contains("<w:tab/>"));
    assert_eq!(out.report.counts.code_blocks, 2);
}

#[test]
fn tables_have_header_rows_alignment_widths_and_captions() {
    let out = export(
        "14-tables",
        "Table: Results.\n\n| Name | Value | Note |\n|:-----|------:|:----:|\n| a | 1 | *x* |\n| b | 22 | `y` |\n\n| Only |\n|------|\n| one |\n",
    );
    let tables = out.doc.tables().unwrap();
    assert_eq!(tables.len(), 2);
    let first = &tables[0];
    assert_eq!(
        first.cell(&out.doc, 0, 1).unwrap().text(&out.doc).unwrap(),
        "Value"
    );
    assert_eq!(
        first.cell(&out.doc, 2, 1).unwrap().text(&out.doc).unwrap(),
        "22"
    );
    assert_eq!(
        first
            .style(&out.doc)
            .unwrap()
            .and_then(|s| s.name)
            .as_deref(),
        Some("Table")
    );
    let body = out.body();
    assert_eq!(count(&body, "<w:tblHeader/>"), 2);
    assert!(body.contains("<w:jc w:val=\"right\"/>"));
    // The full A4 text width, 11906 - 2 * 1247 twips, in twips and not a
    // percentage: Word for the web mis-lays percentages.
    assert_eq!(count(&body, "<w:tblW w:type=\"dxa\" w:w=\"9412\"/>"), 2);
    assert_eq!(count(&body, "<w:tblLayout w:type=\"fixed\"/>"), 2);
    assert!(body.contains("<w:pStyle w:val=\"TableContents\"/>"));
    let styles = out.part("word/styles.xml");
    assert!(styles.contains("<w:name w:val=\"Table Contents\"/>"));
    assert!(body.contains("<w:jc w:val=\"center\"/>"));
    // Word joins tables that touch; a paragraph keeps the two apart.
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0], ("Table Caption".into(), "Results.".into()));
    assert_eq!(paragraphs.len(), 2);
    let between = &body[body.find("</w:tbl>").unwrap()..body.rfind("<w:tbl>").unwrap()];
    assert!(between.contains("<w:p"), "{between}");
    let grid: i64 = body[body.find("<w:tblGrid>").unwrap()..body.find("</w:tblGrid>").unwrap()]
        .split("w:w=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].parse::<i64>().unwrap())
        .sum();
    assert_eq!(grid, 9412);
}

#[test]
fn images_embed_scale_caption_and_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dot.png"), png_bytes()).unwrap();
    std::fs::copy(
        repo().join("examples/markdown/images/banner.jpg"),
        dir.path().join("wide photo.jpg"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("vector.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    let markdown = format!(
        "![A dot](dot.png \"Dot\")\n\n![Wide](wide%20photo.jpg)\n\nInline ![i](data:image/png;base64,{}) here.\n\n![missing](nope.png)\n\n![svg](vector.svg)\n\n![remote](https://example.com/x.png)\n",
        base64(&png_bytes())
    );
    let options = MarkdownOptions {
        resource_dir: Some(dir.path().to_path_buf()),
        ..MarkdownOptions::default()
    };
    let out = export_with("15-images", &markdown, Some(reference()), &options);
    assert_eq!(out.report.counts.images, 3);
    assert_eq!(
        out.warning_codes(),
        [
            "image_not_found",
            "image_unsupported",
            "remote_image_skipped"
        ]
    );
    let body = out.body();
    assert!(body.contains("descr=\"A dot\""));
    assert!(body.contains("title=\"Dot\""));
    let mut ids: Vec<&str> = body
        .split("<wp:docPr id=\"")
        .skip(1)
        .map(|s| &s[..s.find('"').unwrap()])
        .collect();
    let all = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), all);
    // The 2400 px banner shrinks to the A4 text width: 11906 - 2 * 1247
    // twips = 5,976,620 EMU.
    let widths: Vec<i64> = body
        .split("<wp:extent cx=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].parse().unwrap())
        .collect();
    assert!(widths.contains(&5_976_620), "{widths:?}");
    // Identical bytes share one media part; the reference document's sample
    // image went with its body.
    assert_eq!(
        out.doc
            .package()
            .parts()
            .filter(|p| p.name().starts_with("word/media/"))
            .count(),
        2
    );
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0].0, "Captioned Figure");
    assert_eq!(paragraphs[1], ("Image Caption".into(), "A dot".into()));
    assert!(paragraphs.iter().any(|(_, t)| t == "[missing]"));
}

#[test]
fn local_images_stay_inside_the_resource_directory() {
    let outer = tempfile::tempdir().unwrap();
    let dir = outer.path().join("docs");
    std::fs::create_dir_all(dir.join("img")).unwrap();
    std::fs::write(dir.join("img/dot.png"), png_bytes()).unwrap();
    let secret = outer.path().join("secret.png");
    std::fs::write(&secret, png_bytes()).unwrap();
    let markdown = format!(
        "![in](img/dot.png)\n\n![dotted](./img/../img/dot.png)\n\n\
         ![up](../secret.png)\n\n![up missing](../nope.png)\n\n\
         ![encoded](%2e%2e%2fsecret.png)\n\n![absolute](<{abs}>)\n\n\
         ![file](<file://{abs}>)\n\n![host](file://server/share/x.png)\n\n\
         ![unc](//server/share/x.png)\n\n![unc2](\\\\\\\\server\\\\share\\\\x.png)\n",
        abs = secret.display()
    );
    // Symbolic links are a Unix-only case; the source is immutable elsewhere.
    #[cfg(unix)]
    let markdown = {
        let mut markdown = markdown;
        std::os::unix::fs::symlink(&secret, dir.join("img/link.png")).unwrap();
        markdown.push_str("\n![link](img/link.png)\n");
        // A linked directory leading out: an existing and a missing file
        // behind it read alike, so the link is no probe for files outside.
        std::os::unix::fs::symlink(outer.path(), dir.join("img/out")).unwrap();
        markdown.push_str(
            "\n![via link](img/out/secret.png)\n\n![via link missing](img/out/nope.png)\n",
        );
        markdown
    };
    let options = MarkdownOptions {
        resource_dir: Some(dir.clone()),
        ..MarkdownOptions::default()
    };
    let out = export_with("15-images-confined", &markdown, None, &options);
    assert_eq!(out.report.counts.images, 2);
    let outside = out
        .warning_codes()
        .iter()
        .filter(|code| **code == "image_outside_resource_dir")
        .count();
    // `../nope.png` is refused before the file system is asked, so a
    // missing file outside reads the same as an existing one.
    assert_eq!(
        outside,
        if cfg!(unix) { 11 } else { 8 },
        "{:?}",
        out.report.warnings
    );
    assert_eq!(out.warning_codes().len(), outside);
    // Warnings quote the reference as written: no resolved path, no
    // operating system error.
    let outer_path = std::fs::canonicalize(outer.path()).unwrap();
    for warning in &out.report.warnings {
        assert!(
            !warning.message.contains(&*outer_path.to_string_lossy())
                || warning.message.contains(&*secret.to_string_lossy()),
            "{}",
            warning.message
        );
        assert!(!warning.message.contains("os error"), "{}", warning.message);
    }
    assert!(out.report.warnings[0].message.contains("../secret.png"));
    let mut report = out.report.clone();
    report.redact();
    assert!(
        report
            .warnings
            .iter()
            .all(|w| !w.message.contains("secret"))
    );

    let html = export_html(
        "15-images-confined-html",
        "<p><img src=\"../secret.png\" alt=\"up\"><img src=\"img/dot.png\" alt=\"in\"></p>",
        &options,
    );
    assert_eq!(html.report.counts.images, 1);
    assert_eq!(html.warning_codes(), ["image_outside_resource_dir"]);
    let strict = MarkdownOptions {
        strict: true,
        ..options
    };
    let err = convert("![up](../secret.png)\n", None, &strict).unwrap_err();
    assert_eq!(err.reason(), "markdown_not_faithful");
}

#[test]
fn without_a_resource_directory_no_file_is_read() {
    let chart = repo().join("examples/markdown/images/chart.png");
    assert!(chart.exists());
    let markdown = format!(
        "![abs](<{}>)\n\n![rel](examples/markdown/images/chart.png)\n\n![given](./given.png)\n\n![inline](data:image/png;base64,{})\n",
        chart.display(),
        base64(&png_bytes())
    );
    let options = MarkdownOptions {
        images: [("given.png".to_string(), png_bytes())].into(),
        ..MarkdownOptions::default()
    };
    let out = export_with("15-images-no-dir", &markdown, None, &options);
    assert_eq!(out.report.counts.images, 2);
    assert_eq!(out.warning_codes(), ["image_not_found", "image_not_found"]);
    assert!(
        out.report.warnings[0]
            .message
            .contains("no resource directory")
    );
}

#[test]
fn supplied_image_keys_match_with_or_without_dot_slash_and_escapes() {
    let options = MarkdownOptions {
        images: [
            ("a.png".to_string(), png_bytes()),
            ("./b.png".to_string(), png_bytes()),
            ("c d.png".to_string(), png_bytes()),
            ("e%20f.png".to_string(), png_bytes()),
        ]
        .into(),
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "15-images-keys",
        "![a](./a.png) ![b](b.png) ![c](c%20d.png) ![e](<./e f.png>)\n",
        None,
        &options,
    );
    assert_eq!(out.report.counts.images, 4, "{:?}", out.report.warnings);
    assert!(out.report.warnings.is_empty());
}

/// A PNG of `size` bytes or more (a padding chunk after the header).
fn padded_png(size: usize) -> Vec<u8> {
    let png = png_bytes();
    let mut out = png[..33].to_vec();
    out.extend_from_slice(&(size as u32).to_be_bytes());
    out.extend_from_slice(b"zPAD");
    out.extend(std::iter::repeat_n(0u8, size));
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&png[33..]);
    out
}

#[test]
fn images_larger_than_the_part_limit_are_left_out() {
    let limits = Limits {
        max_part_bytes: 600 << 10,
        ..Limits::default()
    };
    let template = || {
        let (bytes, _) = Document::new().patch_bytes().unwrap();
        Document::open_bytes(&bytes, &limits).unwrap()
    };
    let big = padded_png(700 << 10);
    // Still a picture within the default limits.
    docrs::image::Image::from_blob(&big).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.png"), &big).unwrap();
    let options = MarkdownOptions {
        resource_dir: Some(dir.path().to_path_buf()),
        images: [("given.png".to_string(), big.clone())].into(),
        ..MarkdownOptions::default()
    };
    let markdown = format!(
        "![data](data:image/png;base64,{})\n\n![given](given.png)\n\n![file](big.png)\n\n![small](data:image/png;base64,{})\n",
        base64(&big),
        base64(&png_bytes())
    );
    let (_, report) = convert(&markdown, Some(template()), &options).unwrap();
    assert_eq!(report.counts.images, 1);
    let codes: Vec<&str> = report.warnings.iter().map(|w| w.code).collect();
    assert_eq!(codes, ["image_too_large"; 3]);
    // The data: URI is not quoted.
    assert!(report.warnings[0].message.len() < 200);
    let strict = MarkdownOptions {
        strict: true,
        ..options
    };
    let mut doc = template();
    let before = doc.patch_bytes().unwrap().0;
    let err = docrs::markdown::insert_markdown(&mut doc, &markdown, &strict).unwrap_err();
    assert_eq!(err.reason(), "markdown_not_faithful");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

/// A JFIF JPEG header, 3 × 2 pixels, whose density is zero dots per inch.
fn zero_dpi_jpeg() -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    // APP0: JFIF 1.1, units 1 (dpi), density 0 × 0, no thumbnail.
    out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    out.extend_from_slice(b"JFIF\0");
    out.extend_from_slice(&[1, 1, 1, 0, 0, 0, 0, 0, 0]);
    // SOF0: 8 bits, height 2, width 3, one component.
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 8, 0, 2, 0, 3, 1, 1, 0x11, 0]);
    out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 1, 1, 0, 0, 0x3F, 0, 0xFF, 0xD9]);
    out
}

#[test]
fn malformed_images_become_warnings_and_zero_dpi_counts_as_96() {
    let jpeg = zero_dpi_jpeg();
    let image = docrs::image::Image::from_blob(&jpeg).unwrap();
    // Upstream's own sizing divides by the zero resolution.
    assert_eq!(image.width().unwrap_err().reason(), "zero_division");
    let mut zero_width = png_bytes();
    zero_width[16..20].copy_from_slice(&[0; 4]);
    let options = MarkdownOptions {
        images: [
            ("zero-dpi.jpg".to_string(), jpeg),
            ("truncated.png".to_string(), png_bytes()[..20].to_vec()),
            ("zero-width.png".to_string(), zero_width),
            ("dot.png".to_string(), png_bytes()),
        ]
        .into(),
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "15-images-malformed",
        "![z](zero-dpi.jpg)\n\n![t](truncated.png)\n\n![ok](dot.png)\n",
        None,
        &options,
    );
    assert_eq!(out.report.counts.images, 2);
    assert_eq!(out.warning_codes(), ["image_unreadable"]);
    // 3 × 2 pixels at 96 dpi.
    assert!(
        out.body()
            .contains("<wp:extent cx=\"28575\" cy=\"19050\"/>"),
        "{}",
        out.body()
    );
    assert!(out.paragraphs().iter().any(|(_, t)| t == "[t]"));
    // A width asked of a picture with no pixel width cannot be scaled.
    let html = export_html(
        "15-images-malformed-html",
        "<p><img src=\"zero-width.png\" width=\"50\" alt=\"w\"></p>",
        &options,
    );
    assert_eq!(html.report.counts.images, 0);
    assert_eq!(html.warning_codes(), ["image_unreadable"]);
}

#[test]
fn links_keep_only_web_mail_phone_and_relative_targets() {
    let out = export_with(
        "05-links-schemes",
        "[a](javascript:alert(1)) [b](JavaScript:alert(2)) [c](vbscript:x) \
         [d](file:///etc/passwd) [e](data:text/html,x) [f](//host/share) \
         [g](https://ok.example) [h](HTTP://ok.example/up) [i](mailto:me@example.com) \
         [j](tel:+15551234) [k](docs/a.html) [l](a/b:c)\n",
        None,
        &MarkdownOptions::default(),
    );
    assert_eq!(out.report.counts.links, 6);
    assert_eq!(out.warning_codes(), ["link_scheme_skipped"; 6]);
    let rels = out.part("word/_rels/document.xml.rels");
    for gone in [
        "javascript",
        "JavaScript",
        "vbscript",
        "file:",
        "data:",
        "//host",
    ] {
        assert!(!rels.contains(gone), "{gone}: {rels}");
    }
    let text = &out.paragraphs()[0].1;
    for label in ["a", "b", "c", "d", "e", "f"] {
        assert!(text.contains(label));
    }
    let strict = MarkdownOptions {
        strict: true,
        ..MarkdownOptions::default()
    };
    let err = convert("[a](javascript:alert(1))\n", None, &strict).unwrap_err();
    assert_eq!(err.reason(), "markdown_not_faithful");

    // Control characters in the address are stripped.
    let html = export_html(
        "05-links-schemes-html",
        "<p><a href=\"javascript:alert(1)\">a</a> <a href=\"java&#9;script:alert(2)\">b</a> \
         <a href=\"https://ok.example\">c</a></p>",
        &MarkdownOptions::default(),
    );
    assert_eq!(html.report.counts.links, 1);
    assert_eq!(html.warning_codes(), ["link_scheme_skipped"; 2]);
    let rels = html.part("word/_rels/document.xml.rels");
    assert!(!rels.contains("avascript"), "{rels}");
    assert!(rels.contains("https://ok.example"));
}

#[test]
fn deeply_nested_quotes_convert() {
    let markdown = format!("{} deep\n", ">".repeat(docrs::markdown::MAX_NESTING_DEPTH));
    let out = export_with(
        "06-deep-quotes",
        &markdown,
        None,
        &MarkdownOptions::default(),
    );
    assert!(out.paragraphs().iter().any(|(_, t)| t == "deep"));
}

#[test]
fn definition_lists_use_term_and_definition_styles() {
    let out = export(
        "16-definitions",
        "Term\n: First definition.\n: Second definition.\n\nOther\n: Its definition.\n",
    );
    let styles: Vec<String> = out.paragraphs().into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        styles,
        [
            "Definition Term",
            "Definition",
            "Definition",
            "Definition Term",
            "Definition"
        ]
    );
}

/// The opening of Word's table-of-contents block, up to the heading's text.
const TOC_BLOCK: &str = "<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val=\"TOCHeading\"/></w:pPr><w:r><w:t>";

/// The block's opening with the title the example reference document's own
/// contents carries, then the first entry.
fn reference_toc_block() -> String {
    format!("{TOC_BLOCK}Contents</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val=\"TOC1\"/>")
}

const RULE: &str = "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr><w:spacing w:before=\"120\" w:after=\"120\"/><w:rPr><w:sz w:val=\"2\"/><w:szCs w:val=\"2\"/></w:rPr></w:pPr></w:p>";

#[test]
fn rules_page_breaks_and_toc() {
    let out = export(
        "17-breaks-toc",
        "[TOC]\n\n# A\n\nx\n\n## A.1\n\n---\n\n\\newpage\n\n<!-- pagebreak -->\n\n# B\n",
    );
    let body = out.body();
    assert_eq!(count(&body, RULE), 1, "{body}");
    for shape in ["<v:rect", "o:hr=", "<w:pict>"] {
        assert!(!body.contains(shape), "{shape} in {body}");
    }
    let blank = export_with(
        "17f-rule-blank",
        "a\n\n---\n\nb\n",
        None,
        &MarkdownOptions::default(),
    );
    assert_eq!(count(&blank.body(), RULE), 1, "{}", blank.body());
    assert_eq!(count(&body, "<w:br w:type=\"page\"/>"), 2);
    // Written out with estimated page numbers, the contents needs no field
    // update, so Word opens it without asking; the field stays for Word's own.
    assert!(body.contains(" TOC \\o \"1-3\" \\h \\z \\u </w:instrText>"));
    // A [TOC] marker in the text adds no page break of its own.
    assert!(!body.contains("pageBreakBefore"));
    // Word's table-of-contents block: heading, entries and the field inside
    // one content control; the body paragraphs python-docx lists follow it.
    assert_eq!(count(&body, "<w:sdt>"), 1, "{body}");
    assert!(body.contains(&reference_toc_block()), "{body}");
    assert_eq!(out.report.toc_title_source.as_deref(), Some("reference"));
    let close = body
        .find("</w:sdtContent></w:sdt>")
        .expect("the block closes");
    let block = &body[..close];
    assert_eq!(count(block, "w:fldCharType=\"begin\""), 4);
    assert_eq!(count(block, "w:fldCharType=\"end\""), 4);
    for text in ["A", "A.1", "B", "1", "3"] {
        let entry = format!("<w:t>{text}</w:t>");
        assert!(block.contains(&entry), "{entry} in {block}");
    }
    assert!(block.contains("<w:pStyle w:val=\"TOC2\"/>"), "{block}");
    assert_eq!(
        out.paragraphs()[0],
        ("Heading 1".to_string(), "A".to_string())
    );
    assert!(
        body.contains("<w:hyperlink w:anchor=\"a1\" w:history=\"1\">"),
        "{body}"
    );
    // The TOC field, and a PAGEREF in each of the three entries.
    assert_eq!(count(&body, "w:fldCharType=\"begin\""), 4);
    assert_eq!(count(&body, "w:fldCharType=\"end\""), 4);
    assert!(!out.part("word/settings.xml").contains("updateFields"));
}

#[test]
fn content_after_the_toc_option_starts_a_new_page() {
    let options = MarkdownOptions {
        toc: Some(true),
        ..MarkdownOptions::default()
    };
    let paragraph = export_with("17c-toc-break", "# A\n\nx\n", Some(reference()), &options);
    let body = paragraph.body();
    let first = body
        .find("<w:pageBreakBefore/>")
        .expect("a page break before the body");
    assert!(first > body.find("w:fldCharType=\"end\"").unwrap());
    assert_eq!(count(&body, "pageBreakBefore"), 1);
    let own = export_with(
        "17d-toc-own-break",
        "\\newpage\n\n# A\n",
        Some(reference()),
        &options,
    );
    assert!(!own.body().contains("pageBreakBefore"));
    assert_eq!(count(&own.body(), "<w:br w:type=\"page\"/>"), 1);
    let table = export_with(
        "17e-toc-table",
        "| a |\n|---|\n| b |\n",
        Some(reference()),
        &options,
    );
    let body = table.body();
    let page_break = body.find("<w:br w:type=\"page\"/>").expect("a page break");
    assert!(page_break < body.find("<w:tbl>").unwrap());
}

#[test]
fn toc_page_numbers_ask_word_to_update() {
    let options = MarkdownOptions {
        toc: Some(true),
        toc_page_numbers: true,
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "17b-toc-page-numbers",
        "# A\n\n# B\n",
        Some(reference()),
        &options,
    );
    let body = out.body();
    assert!(body.contains(" TOC \\o \"1-3\" \\h \\z \\u </w:instrText>"));
    assert!(body.contains("w:fldCharType=\"begin\" w:dirty=\"true\""));
    assert!(
        out.part("word/settings.xml")
            .contains("<w:updateFields w:val=\"true\"/>")
    );
}

#[test]
fn toc_entries_show_computed_page_numbers() {
    let options = MarkdownOptions {
        toc: Some(true),
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "17g-toc-numbers",
        "# A\n\nx\n\n\\newpage\n\n# B\n\n\\newpage\n\n\\newpage\n\n# C\n",
        Some(reference()),
        &options,
    );
    let body = out.body();
    // Estimated pages: the contents on 1, A on 2 (the page break after the
    // contents), B on 3, C on 5.
    assert!(
        body.contains("<w:tab w:val=\"right\" w:leader=\"dot\""),
        "{body}"
    );
    for (bookmark, page) in [("a", 2), ("b", 3), ("c", 5)] {
        let field = format!(" PAGEREF {bookmark} \\h </w:instrText>");
        let at = body
            .find(&field)
            .unwrap_or_else(|| panic!("{field} in {body}"));
        let result = &body[at..];
        let number = &result[result.find("<w:t>").unwrap() + 5..];
        assert_eq!(&number[..number.find('<').unwrap()], page.to_string());
    }
    let toc = out.report.toc.as_ref().expect("a contents report");
    assert_eq!((toc.entries, toc.pages), (3, 5));
    assert!(toc.missing_fonts.is_empty(), "{toc:?}");
    assert!(!out.part("word/settings.xml").contains("updateFields"));

    let plain = export_with(
        "17h-toc-no-numbers",
        "# A\n",
        Some(reference()),
        &MarkdownOptions {
            toc_numbers: false,
            ..options.clone()
        },
    );
    assert!(!plain.body().contains("PAGEREF"));
    assert!(plain.report.toc.is_none());
}

#[test]
fn headings_without_text_get_no_contents_entry() {
    // The editor allows empty headings; Word's update leaves them out.
    let options = MarkdownOptions {
        toc: Some(true),
        ..MarkdownOptions::default()
    };
    let out = export_html(
        "17j-toc-empty-heading",
        "<h2>A</h2><p>x</p><h1></h1><h1> </h1><h1><br></h1><h2>B</h2><p>y</p>",
        &options,
    );
    let body = out.body();
    assert_eq!(count(&body, " PAGEREF "), 2, "{body}");
    assert_eq!(out.report.toc.as_ref().map(|t| t.entries), Some(2));
    let block = &body[..body.find("</w:sdtContent></w:sdt>").unwrap()];
    assert_eq!(count(block, "<w:pStyle w:val=\"TOC2\"/>"), 2, "{block}");
    for text in ["A", "B", "2"] {
        let entry = format!("<w:t>{text}</w:t>");
        assert!(block.contains(&entry), "{entry} in {block}");
    }
    assert_eq!(count(block, "<w:t>2</w:t>"), 2, "{block}");
}

#[test]
fn a_font_without_metrics_is_named_in_a_warning() {
    let options = MarkdownOptions {
        toc: Some(true),
        ..MarkdownOptions::default()
    };
    let out = export_html(
        "17i-toc-missing-font",
        "<h1>A</h1><p><span style=\"font-family: 'Nosuch Grotesk'\">text</span></p>",
        &options,
    );
    let toc = out.report.toc.as_ref().expect("a contents report");
    assert_eq!(toc.missing_fonts, ["Nosuch Grotesk"]);
    let warning = out
        .report
        .warnings
        .iter()
        .find(|w| w.code == "font_metrics_missing")
        .expect("a font_metrics_missing warning");
    assert!(warning.message.contains("Nosuch Grotesk"), "{warning:?}");
}

#[test]
fn page_number_footer_only_when_the_template_has_none() {
    let options = MarkdownOptions {
        page_numbers: true,
        ..MarkdownOptions::default()
    };
    let out = export_with("17c-page-numbers", "Text.\n", None, &options);
    assert_eq!(out.report.page_numbers.as_deref(), Some("footer_added"));
    let body = out.body();
    assert!(body.contains("<w:footerReference w:type=\"default\""));
    let footer = out.part("word/footer1.xml");
    assert!(footer.contains(" PAGE "));
    assert!(footer.contains("<w:jc w:val=\"center\"/>"));
    let kept = export_with(
        "17d-page-numbers-kept",
        "Text.\n",
        Some(reference()),
        &options,
    );
    assert_eq!(
        kept.report.page_numbers.as_deref(),
        Some("template_footer_kept")
    );
    assert!(
        !kept
            .report
            .changed_parts
            .iter()
            .any(|p| p.contains("footer"))
    );
}

#[test]
fn front_matter_renders_a_title_block_and_sets_properties() {
    let out = export(
        "18-front-matter",
        "---\ntitle: The Title\nsubtitle: Sub\nauthor: [Ann, Bo]\ndate: 2026-10-19\nabstract: Short summary.\nkeywords: [a, b]\nlang: en-GB\ntoc: true\ntoc-depth: 2\n---\n\n# Body\n",
    );
    let paragraphs = out.paragraphs();
    let expected: Vec<(String, String)> = [
        ("Title", "The Title"),
        ("Subtitle", "Sub"),
        ("Author", "Ann"),
        ("Author", "Bo"),
        ("Date", "2026-10-19"),
        ("Abstract", "Short summary."),
    ]
    .iter()
    .map(|(s, t)| (s.to_string(), t.to_string()))
    .collect();
    assert_eq!(paragraphs[..6], expected[..]);
    assert!(out.body().contains("TOC \\o \"1-2\""));
    assert!(
        out.body().contains(&reference_toc_block()),
        "{}",
        out.body()
    );
    let props = out.doc.core_properties().unwrap();
    assert_eq!(props.title, "The Title");
    assert_eq!(props.author, "Ann; Bo");
    assert_eq!(props.subject, "Sub");
    assert_eq!(props.keywords, "a, b");
    assert_eq!(props.comments, "Short summary.");
    assert_eq!(props.language, "en-GB");
}

#[test]
fn front_matter_can_be_left_out() {
    let options = MarkdownOptions {
        title_block: false,
        set_properties: false,
        toc: Some(false),
        ..MarkdownOptions::default()
    };
    let before = reference().core_properties().unwrap();
    let out = export_with(
        "18b-no-front-matter",
        "---\ntitle: T\ntoc: true\n---\n\nText.\n",
        Some(reference()),
        &options,
    );
    assert_eq!(
        out.paragraphs(),
        [("First Paragraph".into(), "Text.".into())]
    );
    // The template's own title is cleared all the same, not replaced.
    assert!(!before.title.is_empty());
    assert_eq!(out.doc.core_properties().unwrap().title, "");
}

#[test]
fn math_becomes_word_equations() {
    let out = export(
        "19-math",
        "Inline $a^2 + b^2 = c^2$ and $\\frac{1}{2}$.\n\n$$\n\\sum_{i=1}^n i = \\frac{n(n+1)}{2}\n$$\n\n$\\unknowncmd x$\n",
    );
    let body = out.body();
    assert_eq!(count(&body, "<m:oMath>"), 4);
    assert_eq!(count(&body, "<m:oMathPara>"), 1);
    for element in ["<m:sSup>", "<m:f>", "<m:nary>", "<m:chr m:val=\"∑\"/>"] {
        assert!(body.contains(element), "{element}");
    }
    assert_eq!(out.warning_codes(), ["math_unknown_command"]);
    assert_eq!(out.report.counts.equations, 4);
}

#[test]
fn html_blocks_convert_and_unknown_tags_are_reported() {
    let out = export(
        "20-html",
        "<div class=\"x\">\n<p>Block <b>bold</b> &amp; <a href=\"https://example.com\">link</a></p>\n</div>\n\n<p></p>\n\n<table><tr><th>Step</th><th>Checks</th></tr><tr><td><p>Rollover</p></td><td><ul><li>One</li><li>Two</li></ul></td></tr></table>\n\nText <span>kept</span> <video>v</video>.\n",
    );
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0].1, "Block bold & link");
    assert_eq!(paragraphs[1].1, "", "an empty <p></p> stays a blank line");
    assert_eq!(paragraphs[2].1, "Text kept v.");
    let table = &out.doc.tables().unwrap()[0];
    assert_eq!(
        table.cell(&out.doc, 1, 0).unwrap().text(&out.doc).unwrap(),
        "Rollover"
    );
    assert_eq!(
        table.cell(&out.doc, 1, 1).unwrap().text(&out.doc).unwrap(),
        "One\nTwo"
    );
    assert!(out.body().contains("<w:tblHeader/>"));
    assert_eq!(out.warning_codes(), ["raw_html_dropped"]);
    assert!(out.report.warnings[0].message.contains("<video>"));
}

fn export_html(name: &str, html: &str, options: &MarkdownOptions) -> Export {
    export_html_with(name, reference(), html, options)
}

fn export_html_with(
    name: &str,
    mut doc: Document,
    html: &str,
    options: &MarkdownOptions,
) -> Export {
    let report = docrs::markdown::insert_html(&mut doc, html, options)
        .unwrap_or_else(|err| panic!("{name}: conversion failed: {err}"));
    let (bytes, _) = doc.patch_bytes().unwrap();
    if let Some(dir) = std::env::var_os("DOCRS_MARKDOWN_EXPORT") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.docx")), &bytes).unwrap();
    }
    Export {
        doc: Document::open_bytes(&bytes, &Limits::default()).unwrap(),
        report,
        bytes,
    }
}

#[test]
fn editor_html_maps_every_construct() {
    let html = concat!(
        "<h2 style=\"text-align: center;\">Centred heading</h2>",
        "<p><span style=\"color: #dc2626;\">red</span> <span style=\"font-family: &quot;Courier New&quot;, monospace;\">mono</span> ",
        "<mark data-color=\"#fef08a\" style=\"background-color: #fef08a; color: inherit;\">pale</mark> ",
        "<mark data-color=\"#ffff00\" style=\"background-color: #ffff00; color: inherit;\">yellow</mark></p>",
        "<p style=\"text-align: right;\">Right</p><p style=\"text-align: justify;\">Justified</p><p></p>",
        "<ol start=\"3\"><li><p>Third</p></li></ol><hr>",
        "<table style=\"width: 600px;\"><colgroup><col style=\"width: 300px;\"><col style=\"width: 150px;\"><col style=\"width: 150px;\"></colgroup><tbody>",
        "<tr><th colspan=\"1\" rowspan=\"1\" colwidth=\"300\"><p>Product</p></th><th colspan=\"2\" rowspan=\"1\" colwidth=\"150,150\"><p>Both</p></th></tr>",
        "<tr><td colspan=\"1\" rowspan=\"2\" colwidth=\"300\"><p>Two rows</p></td><td colwidth=\"150\"><p style=\"text-align: right;\">$1</p></td><td colwidth=\"150\"><p>x</p></td></tr>",
        "<tr><td colwidth=\"150\"><p style=\"text-align: center;\">Nil</p></td><td colwidth=\"150\"><p>y</p></td></tr></tbody></table>",
        "<div data-image-align=\"center\" style=\"text-align: center;\"><img src=\"https://store.example/logo.png?token=t\" alt=\"Logo\" width=\"200\" height=\"80\"></div>",
        "<div data-image-align=\"right\" style=\"text-align: right;\"><img src=\"img-7\" alt=\"Wide\" width=\"4000\" height=\"100\"></div>",
        "<div data-page-break=\"\" class=\"rich-text-page-break not-prose\"></div><h2>After</h2>",
    );
    let options = MarkdownOptions {
        figure_captions: false,
        images: [
            (
                "https://store.example/logo.png?token=t".to_string(),
                png_bytes(),
            ),
            ("img-7".to_string(), png_bytes()),
        ]
        .into(),
        ..MarkdownOptions::default()
    };
    let out = export_html("21-editor-html", html, &options);
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
    let body = out.body();
    assert!(body.contains("<w:color w:val=\"DC2626\"/>"), "{body}");
    assert!(body.contains(
        "<w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\" w:cs=\"Courier New\"/>"
    ));
    assert!(body.contains("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FEF08A\"/>"));
    assert!(body.contains("<w:highlight w:val=\"yellow\"/>"));
    for jc in ["center", "right", "both"] {
        assert!(body.contains(&format!("<w:jc w:val=\"{jc}\"/>")), "{jc}");
    }
    let paragraphs = out.paragraphs();
    assert!(
        paragraphs
            .iter()
            .any(|(style, text)| style == "Heading 2" && text == "Centred heading")
    );
    assert!(
        paragraphs.iter().any(|(_, text)| text.is_empty()),
        "the empty paragraph stays"
    );
    let width: i64 = {
        let start = body.find("<w:tblW w:type=\"dxa\" w:w=\"").unwrap() + 26;
        body[start..start + body[start..].find('"').unwrap()]
            .parse()
            .unwrap()
    };
    let grid: Vec<i64> = body
        .match_indices("<w:gridCol w:w=\"")
        .map(|(at, m)| {
            let rest = &body[at + m.len()..];
            rest[..rest.find('"').unwrap()].parse().unwrap()
        })
        .collect();
    assert_eq!(grid.iter().sum::<i64>(), width);
    assert_eq!(grid[0], width / 2);
    assert!((grid[1] - grid[2]).abs() <= 1, "{grid:?}");
    assert_eq!(count(&body, "<w:gridSpan w:val=\"2\"/>"), 1);
    assert_eq!(count(&body, "<w:vMerge w:val=\"restart\"/>"), 1);
    assert_eq!(count(&body, "<w:vMerge/>"), 1);
    assert_eq!(count(&body, "<w:tblHeader/>"), 1);
    // Pictures at the editor's size (96 dpi), capped at the text width.
    assert!(body.contains("<wp:extent cx=\"1905000\" cy=\"762000\"/>"));
    let capped = width * 635;
    assert!(
        body.contains(&format!("<wp:extent cx=\"{capped}\"")),
        "capped at {capped}"
    );
    assert_eq!(count(&body, "descr=\"Logo\""), 1);
    assert!(body.contains("<w:br w:type=\"page\"/>"));
    assert_eq!(count(&body, RULE), 1, "{body}");
    assert!(!body.contains("CaptionedFigure\"/><w:jc w:val=\"center\"/></w:pPr><w:r><w:t"));
    // Unknown CSS is reported by property.
    let out = export_html(
        "21-editor-html-dropped",
        "<p style=\"line-height: 2\"><span style=\"letter-spacing: 1px\">x</span></p>",
        &MarkdownOptions::default(),
    );
    assert_eq!(
        out.warning_codes(),
        ["html_style_dropped", "html_style_dropped"]
    );
    let out = export_html(
        "21-editor-html-nested",
        "<ul><li><p>a</p><ul style=\"text-align: center\"><li><p>b</p></li></ul></li></ul>",
        &MarkdownOptions::default(),
    );
    let paragraphs = out.paragraphs();
    assert_eq!(
        paragraphs[1],
        ("List Bullet 2".to_string(), "b".to_string())
    );
    assert!(out.body().contains("<w:jc w:val=\"center\"/>"));
}

#[test]
fn inspection_reports_what_a_conversion_takes() {
    let doc = reference();
    let options = MarkdownOptions {
        page_numbers: true,
        ..MarkdownOptions::default()
    };
    let inspection = docrs::markdown::inspect(&doc, &options).unwrap();
    let section = &inspection["section"];
    assert_eq!(section["used"], "last");
    assert_eq!(section["page"]["orientation"], "portrait");
    let width = section["page"]["width"].as_i64().unwrap();
    assert!(section["text_width"].as_i64().unwrap() < width);
    assert!(
        section["headers_footers"]["header"]["default"].is_object(),
        "{section}"
    );
    assert_eq!(inspection["page_numbers"], "template_footer_kept");
    // The reference's own contents, a TOC field under a TOC Heading paragraph.
    assert_eq!(inspection["toc_title"], "Contents");
    assert_eq!(inspection["roles"]["heading1"]["name"], "Heading 1");
    let before = doc.patch_bytes().unwrap().0;
    assert_eq!(before, reference().patch_bytes().unwrap().0);
    let blank = docrs::Document::new();
    let inspection = docrs::markdown::inspect(&blank, &MarkdownOptions::default()).unwrap();
    let problems = inspection["problems"].as_array().unwrap();
    assert!(
        problems.iter().any(|p| p["code"] == "style_added"),
        "{problems:?}"
    );
}

// `html-converted-template.docx` (tools/markdown/make_html_converted_template.py):
// a finished advice document converted from HTML, its look in direct formatting.

fn html_converted() -> Document {
    Document::open_path(
        repo().join("tests/fixtures/markdown/html-converted-template.docx"),
        &Limits::default(),
    )
    .expect("the HTML-converted template opens")
}

const ADVICE_HTML: &str = "<h1>Advice</h1><p>Intro.</p>\
    <ul><li><p>One</p><ul><li><p>One a</p></li></ul></li><li><p>Two</p></li></ul>\
    <p><strong>Lead-in after the list</strong></p>\
    <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr><tr><td>3</td><td>4</td></tr></table>\
    <p>After the table.</p><h2>Part</h2><p>Text.</p>\
    <ol><li><p>First step</p></li><li><p>Second step</p></li></ol><p>After the steps.</p>";

const ADVICE_NOW: i64 = 1_760_000_000;

fn advice_export(name: &str) -> Export {
    advice_export_from(name, html_converted())
}

fn advice_export_from(name: &str, doc: Document) -> Export {
    let options = MarkdownOptions {
        toc: Some(true),
        page_numbers: true,
        now: Some(docrs::clock::Timestamp::from_unix(ADVICE_NOW)),
        ..MarkdownOptions::default()
    };
    export_html_with(name, doc, ADVICE_HTML, &options)
}

fn style_of(out: &Export, id: &str) -> String {
    style_xml(&out.part("word/styles.xml"), id).to_string()
}

#[test]
fn template_sample_media_comments_and_custom_xml_do_not_ship() {
    let out = advice_export("30-sample-parts");
    let names: Vec<&str> = out.doc.package().parts().map(|p| p.name()).collect();
    for gone in [
        "word/media/image1.png",
        "word/media/image2.png",
        "word/media/image3.png",
        "word/comments.xml",
        "customXml/item1.xml",
        "customXml/itemProps1.xml",
    ] {
        assert!(!names.contains(&gone), "{gone} shipped");
        assert!(out.report.removed_parts.iter().any(|p| p == gone), "{gone}");
    }
    assert!(names.contains(&"word/footer1.xml"));
    assert!(names.contains(&"word/media/logo.png"));
    let rels = out.part("word/_rels/document.xml.rels");
    assert!(
        !rels.contains("media/image") && !rels.contains("comments") && !rels.contains("customXml")
    );
    assert!(!out.part("[Content_Types].xml").contains("comments"));
    for name in &names {
        if name.ends_with(".xml") {
            let xml = out.part(name);
            for sample in [
                "Example Client",
                "Sample review note",
                "Unstyled sample text",
            ] {
                assert!(!xml.contains(sample), "{sample} in {name}");
            }
        }
    }
}

#[test]
fn list_items_never_take_the_sample_body_style() {
    let out = advice_export("31-list-style");
    let paragraphs = out.paragraphs();
    let items: Vec<&(String, String)> = paragraphs
        .iter()
        .filter(|(_, t)| t == "One" || t == "Two")
        .collect();
    assert_eq!(items.len(), 2);
    for (style, _) in items {
        assert_eq!(style, "List Paragraph");
    }
    // The sample's Normal (Web) is for its table cells (below), not the body.
    assert!(
        out.paragraphs()
            .iter()
            .all(|(style, _)| style != "Normal (Web)")
    );
}

#[test]
fn table_look_comes_from_typical_body_cells_with_row_lines() {
    let out = advice_export("32-table-look");
    let table = style_of(&out, "Table");
    // The table style carries the cells' borders, fill and margins; their
    // text look goes to the cell paragraph styles.
    assert!(!table.contains("<w:rPr>"), "{table}");
    assert!(
        table.contains("<w:insideH w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"D0D0D0\"/>"),
        "{table}"
    );
    assert!(
        table.contains("<w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"D0D0D0\"/>")
    );
    let whole_tcpr =
        &table[table.find("</w:tblPr>").unwrap()..table.find("<w:tblStylePr").unwrap()];
    assert!(!whole_tcpr.contains("tcBorders"), "{table}");
    assert!(table.contains("<w:tblCellMar><w:top w:w=\"100\""));
    assert!(table.contains("w:fill=\"40C7F2\""));
    // The cells' own style, Normal (Web), also carries sample body text, so
    // it is left as it is: a Table Contents on it takes the look most of
    // the body cells' text sets by hand (Arial, black, 10 pt, not the first
    // column's bold orange), and a Table Heading on it their white bold.
    assert_eq!(out.report.styles["table_contents"].name, "Table Contents");
    let contents = style_of(&out, "TableContents");
    assert!(
        contents.contains("<w:basedOn w:val=\"NormalWeb\"/>")
            && contents.contains("w:ascii=\"Arial\"")
            && contents.contains("<w:color w:val=\"000000\"/>")
            && contents.contains("<w:sz w:val=\"20\"/>"),
        "{contents}"
    );
    assert!(
        !contents.contains("<w:b/>") && !contents.contains("FAA436"),
        "{contents}"
    );
    assert!(style_rpr(&out, "NormalWeb").contains("Times New Roman"));
    let heading = style_of(&out, "TableHeading");
    assert!(
        heading.contains("<w:basedOn w:val=\"NormalWeb\"/>")
            && heading.contains("<w:b/>")
            && heading.contains("<w:color w:val=\"FFFFFF\"/>"),
        "{heading}"
    );
    let (header, body) = header_row(&out);
    assert_eq!(
        count(&header, "<w:pStyle w:val=\"TableHeading\"/>"),
        2,
        "{header}"
    );
    assert_eq!(count(&body, "<w:pStyle w:val=\"TableContents\"/>"), 4);
    assert!(!body.contains("<w:pStyle w:val=\"NormalWeb\"/>"));
    let inspection = inspect_default(&html_converted());
    let cells = &inspection["lifted"]["table"];
    assert_eq!(cells["cell_styles"]["body"], "NormalWeb");
    assert_eq!(cells["cell_text"]["header"]["color"], "FFFFFF");
    assert_eq!(cells["cell_text"]["body"]["bold"], serde_json::Value::Null);
}

#[test]
fn headings_take_the_look_the_template_shows() {
    let out = advice_export("33-heading-look");
    let h1 = style_of(&out, "Heading1");
    assert!(
        h1.contains("w:ascii=\"Arial\"")
            && h1.contains("<w:b w:val=\"0\"/>")
            && h1.contains("<w:color w:val=\"000000\"/>"),
        "{h1}"
    );
    assert!(!h1.contains("w:ascii=\"Times New Roman\""));
    let h2 = style_of(&out, "Heading2");
    assert!(
        h2.contains("<w:color w:val=\"FAA436\"/>") && h2.contains("<w:sz w:val=\"24\"/>"),
        "{h2}"
    );
    assert!(!out.body().contains("<w:pStyle w:val=\"Heading2\"/></w:pPr><w:bookmarkStart w:id=\"2\" w:name=\"part\"/><w:r><w:rPr>"));
    let inspection =
        docrs::markdown::inspect(&html_converted(), &MarkdownOptions::default()).unwrap();
    assert_eq!(
        inspection["lifted"]["headings"]["heading_2"]["color"],
        "FAA436"
    );
    assert_eq!(inspection["lifted"]["headings"]["heading_1"]["bold"], false);
    assert_eq!(
        inspection["lifted"]["headings"]["heading_2"]["bold"],
        serde_json::Value::Null
    );
    assert_eq!(inspection["lifted"]["body"]["font"], "Arial");
}

#[test]
fn a_paragraph_after_a_table_gets_space_above() {
    let out = advice_export("34-space-after-table");
    let body = out.body();
    let after = &body[body.find("</w:tbl>").unwrap()..];
    let next = &after[..after.find("</w:p>").unwrap()];
    assert!(next.contains("<w:spacing w:before="), "{next}");
    assert!(next.contains("After the table."));
}

#[test]
fn a_missing_toc_heading_follows_heading_1() {
    let out = advice_export("35-toc-heading");
    let toc = style_of(&out, "TOCHeading");
    assert!(toc.contains("<w:basedOn w:val=\"Heading1\"/>"), "{toc}");
    assert!(!toc.contains("2F5496"));
    assert!(toc.contains("<w:outlineLvl w:val=\"9\"/>"));
}

fn html_converted_with_footer_field(field: &str) -> Document {
    let bytes =
        std::fs::read(repo().join("tests/fixtures/markdown/html-converted-template.docx")).unwrap();
    let contents = docrs::package::zip_read::read_archive(&bytes, &Limits::default()).unwrap();
    let members: Vec<(String, Vec<u8>)> = contents
        .members
        .into_iter()
        .map(|member| {
            let data = if member.name == "word/footer1.xml" {
                String::from_utf8(member.data)
                    .unwrap()
                    .replace(
                        "<w:instrText xml:space=\"preserve\"> DOCPROPERTY </w:instrText></w:r><w:r>\
                         <w:instrText xml:space=\"preserve\">\"PracticeName\" \\* MERGEFORMAT </w:instrText>",
                        &format!("<w:instrText xml:space=\"preserve\">{field}</w:instrText>"),
                    )
                    .into_bytes()
            } else {
                member.data
            };
            (member.name, data)
        })
        .collect();
    let bytes = docrs::package::zip_write::write_archive(
        members.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
        &Limits::default(),
    )
    .unwrap();
    Document::open_bytes(&bytes, &Limits::default()).unwrap()
}

#[test]
fn template_document_properties_do_not_ship() {
    let out = advice_export("36-doc-props");
    let core = out.part("docProps/core.xml");
    for gone in [
        "Example Adviser",
        "Example Editor",
        "Sample advice",
        "Example subject",
        "example keywords",
        "Example description",
        "Example category",
        "lastPrinted",
        "contentStatus",
    ] {
        assert!(!core.contains(gone), "{gone} in {core}");
    }
    assert!(core.contains("<dc:language>en-AU</dc:language>"));
    // The document is new, made now.
    let now = docrs::clock::Timestamp::from_unix(ADVICE_NOW).to_xml();
    assert!(
        core.contains(&format!("W3CDTF\">{now}</dcterms:created>")),
        "{core}"
    );
    assert!(
        core.contains(&format!("W3CDTF\">{now}</dcterms:modified>")),
        "{core}"
    );
    assert!(core.contains("<cp:revision>1</cp:revision>"));
    let app = out.part("docProps/app.xml");
    for gone in [
        "<Pages>",
        "<Words>",
        "<Characters>",
        "<CharactersWithSpaces>",
        "<Lines>",
        "<Paragraphs>",
        "<TotalTime>",
        "Example Manager",
        "Sample advice",
    ] {
        assert!(!app.contains(gone), "{gone} in {app}");
    }
    for kept in [
        "<Company>Example Practice</Company>",
        "<Template>Normal.dotm</Template>",
        "<Application>Microsoft Office Word</Application>",
    ] {
        assert!(app.contains(kept), "{kept}");
    }
    // Only the property the footer's DOCPROPERTY field reads stays.
    let custom = out.part("docProps/custom.xml");
    assert!(custom.contains("name=\"PracticeName\""));
    assert!(!custom.contains("ContentTypeId") && !custom.contains("ClientRef"));
    for cleared in [
        "core:creator",
        "core:lastModifiedBy",
        "core:title",
        "app:Pages",
        "app:Manager",
        "app:TitlesOfParts",
        "custom:ContentTypeId",
        "custom:ClientRef",
    ] {
        assert!(
            out.report.cleared_properties.iter().any(|p| p == cleared),
            "{cleared}: {:?}",
            out.report.cleared_properties
        );
    }
    assert!(
        !out.report
            .cleared_properties
            .iter()
            .any(|p| p == "custom:PracticeName")
    );
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
}

#[test]
fn custom_properties_nothing_reads_leave_with_their_part() {
    let out = advice_export_from(
        "37-custom-props",
        html_converted_with_footer_field(" AUTHOR "),
    );
    let names: Vec<&str> = out.doc.package().parts().map(|p| p.name()).collect();
    assert!(!names.contains(&"docProps/custom.xml"));
    assert!(
        out.report
            .removed_parts
            .iter()
            .any(|p| p == "docProps/custom.xml")
    );
    assert!(!out.part("_rels/.rels").contains("custom-properties"));
    assert!(!out.part("[Content_Types].xml").contains("custom.xml"));
    // The footer's AUTHOR field now shows a cleared property.
    let codes: Vec<&str> = out.report.warnings.iter().map(|w| w.code).collect();
    assert_eq!(codes, ["template_property_cleared"]);
}

#[test]
fn a_paragraph_after_a_list_gets_space_above() {
    let out = advice_export("38-space-after-list");
    let body = out.body();
    let paragraph = |text: &str| {
        let end = body.find(text).unwrap();
        let start = body[..end].rfind("<w:p>").unwrap();
        body[start..end].to_string()
    };
    let lead_in = paragraph("Lead-in after the list");
    assert!(lead_in.contains("<w:spacing w:before="), "{lead_in}");
    // Also after a numbered list, whose items leave a little space below.
    assert!(paragraph("After the steps.").contains("<w:spacing w:before="));
    // Only after the outermost list: an item after a nested list gets none.
    assert!(!paragraph(">Two<").contains("w:before="));
}

#[test]
fn added_toc_levels_follow_toc_1() {
    let out = advice_export("39-toc-levels");
    assert!(!out.report.created_styles.iter().any(|s| s == "TOC 1"));
    let toc2 = style_of(&out, "TOC2");
    assert!(toc2.contains("<w:basedOn w:val=\"TOC1\"/>"), "{toc2}");
    assert!(toc2.contains("<w:ind w:left=\"220\"/>"), "{toc2}");
    assert!(!toc2.contains("<w:rPr>"), "{toc2}");
}

#[test]
fn unicode_text_survives() {
    let text = "Ünïcödé — 漢字かな — עברית — 🎉👩‍👩‍👧 — e\u{301}";
    let out = export("21-unicode", &format!("# {text}\n\n{text}\n"));
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0].1, text);
    assert_eq!(paragraphs[1].1, text);
}

#[test]
fn smart_punctuation_is_optional() {
    let options = MarkdownOptions {
        smart_punctuation: true,
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "22-smart",
        "\"Quotes\" -- and --- dots...\n",
        Some(reference()),
        &options,
    );
    assert_eq!(
        out.paragraphs()[0].1,
        "\u{201c}Quotes\u{201d} \u{2013} and \u{2014} dots\u{2026}"
    );
}

#[test]
fn reference_styles_and_layout_are_used_unchanged() {
    let reference_doc = reference();
    let reference_styles = String::from_utf8(docrs::xml::serialize(
        reference_doc.package().xml("word/styles.xml").unwrap(),
    ))
    .unwrap();
    let out = export(
        "23-reference",
        "# Heading\n\nText with `code` and [a link](https://example.com).\n",
    );
    assert!(out.report.created_styles.is_empty());
    assert_eq!(out.part("word/styles.xml"), reference_styles);
    let body = out.body();
    assert!(body.contains("<w:pgSz w:w=\"11906\" w:h=\"16838\"/>"));
    assert!(body.contains("<w:headerReference "));
    assert!(body.contains("<w:footerReference "));
    // The sample body of the reference document is gone.
    assert!(!body.contains("Document Title"));
    let changed: Vec<&str> = out
        .report
        .changed_parts
        .iter()
        .map(String::as_str)
        .collect();
    // The footnotes part loses the sample footnote, the content types the
    // removed parts, the package relationships the thumbnail, and the
    // document properties are cleared.
    assert_eq!(
        changed,
        [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/app.xml",
            "docProps/core.xml",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/footnotes.xml"
        ]
    );
    // The sample body's picture, the custom XML no content control is bound
    // to and the first page's thumbnail leave; the header and footer stay.
    assert_eq!(
        out.report.removed_parts,
        [
            "customXml/_rels/item1.xml.rels",
            "customXml/item1.xml",
            "customXml/itemProps1.xml",
            "docProps/thumbnail.jpeg",
            "word/media/image1.png"
        ]
    );
    for kept in ["word/header1.xml", "word/footer1.xml"] {
        assert!(out.doc.package().part(kept).is_some(), "{kept}");
    }
    assert!(out.doc.package().part("docProps/thumbnail.jpeg").is_none());
    assert!(!out.part("_rels/.rels").contains("thumbnail"));
    assert!(!out.part("[Content_Types].xml").contains("itemProps1"));
    assert!(
        !out.part("word/_rels/document.xml.rels")
            .contains("customXml")
    );
}

#[test]
fn a_plain_word_template_gets_missing_styles_added() {
    let out = export_with(
        "24-blank-template",
        "# H\n\n> quote\n\n```\ncode\n```\n\nText[^n] `x`.\n\n[^n]: note\n",
        None,
        &MarkdownOptions::default(),
    );
    let created = &out.report.created_styles;
    for name in [
        "Source Code",
        "Verbatim Char",
        "Footnote Text",
        "Footnote Reference",
    ] {
        assert!(created.iter().any(|c| c == name), "{name} in {created:?}");
    }
    // Existing Word built-ins are used rather than duplicated.
    assert_eq!(out.report.styles["block_quote"].name, "Quote");
    assert!(!out.report.styles["block_quote"].created);
    let styles = out.part("word/styles.xml");
    assert!(styles.contains("w:name w:val=\"footnote text\""));
    assert_eq!(count(&styles, "w:styleId=\"SourceCode\""), 1);
}

#[test]
fn strict_style_policy_refuses_and_changes_nothing() {
    let mut doc = Document::new();
    let before = doc.patch_bytes().unwrap().0;
    let options = MarkdownOptions {
        style_policy: StylePolicy::Strict,
        ..MarkdownOptions::default()
    };
    let err = doc
        .insert_markdown("```\ncode\n```\n\n`x`\n", &options)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Refusal(RefusalKind::TargetNotFound));
    assert!(err.to_string().contains("code_block"), "{err}");
    assert!(err.to_string().contains("inline_code"), "{err}");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn style_map_sends_roles_to_named_styles() {
    let mut options = MarkdownOptions::default();
    options.style_map.insert(Role::Body, "Normal".into());
    options
        .style_map
        .insert(Role::FirstParagraph, "Normal".into());
    options
        .style_map
        .insert(Role::BlockQuote, "Intense Quote".into());
    let out = export_with("25-style-map", "Text.\n\n> q\n", None, &options);
    assert_eq!(
        out.paragraphs(),
        [
            ("Normal".into(), "Text.".into()),
            ("Intense Quote".into(), "q".into())
        ]
    );
    let mut options = MarkdownOptions::default();
    options
        .style_map
        .insert(Role::CodeBlock, "No Such Style".into());
    let err = convert("```\nx\n```\n", None, &options).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
}

/// Levels the starter's three-level sample does not reach.
fn deep_level(role: Role) -> bool {
    matches!(
        role,
        Role::Heading7
            | Role::Heading8
            | Role::Heading9
            | Role::Toc4
            | Role::Toc5
            | Role::Toc6
            | Role::Toc7
            | Role::Toc8
            | Role::Toc9
    )
}

#[test]
fn starter_reference_has_every_role_style() {
    let doc = reference_document().unwrap();
    let roles = role_styles(&doc, &Default::default()).unwrap();
    let missing: Vec<&str> = roles
        .iter()
        .filter(|(role, style)| style.is_none() && !deep_level(*role))
        .map(|(role, _)| role.key())
        .collect();
    assert!(missing.is_empty(), "missing {missing:?}");
    for (role, style) in &roles {
        if let Some(style) = style
            && !deep_level(*role)
        {
            assert_eq!(style.name, role.candidates()[0], "{}", role.key());
        }
    }
}

#[test]
fn append_keeps_the_existing_body() {
    let mut base = Document::new();
    base.add_paragraph("Existing", None).unwrap();
    let options = MarkdownOptions {
        placement: Placement::Append,
        ..MarkdownOptions::default()
    };
    let out = export_with("26-append", "Added.\n", Some(base), &options);
    let texts: Vec<String> = out.paragraphs().into_iter().map(|(_, t)| t).collect();
    assert_eq!(texts, ["Existing", "Added."]);
}

#[test]
fn placeholder_replaces_only_the_marker_paragraph() {
    let mut base = Document::new();
    base.add_paragraph("Cover", Some("Title")).unwrap();
    base.add_paragraph("{{content}}", None).unwrap();
    base.add_paragraph("Signature", None).unwrap();
    let options = MarkdownOptions {
        placement: Placement::Placeholder("{{content}}".into()),
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "27-placeholder",
        "# Middle\n\nBody.\n",
        Some(base),
        &options,
    );
    let texts: Vec<String> = out.paragraphs().into_iter().map(|(_, t)| t).collect();
    assert_eq!(texts, ["Cover", "Middle", "Body.", "Signature"]);
}

#[test]
fn placeholder_problems_refuse_without_changes() {
    for (body, expected) in [
        (vec!["no marker here"], RefusalKind::TargetNotFound),
        (
            vec!["{{content}}", "{{content}}"],
            RefusalKind::AmbiguousTarget,
        ),
    ] {
        let mut doc = Document::new();
        for text in body {
            doc.add_paragraph(text, None).unwrap();
        }
        let before = doc.patch_bytes().unwrap().0;
        let options = MarkdownOptions {
            placement: Placement::Placeholder("{{content}}".into()),
            ..MarkdownOptions::default()
        };
        let err = doc.insert_markdown("x", &options).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Refusal(expected));
        assert_eq!(doc.patch_bytes().unwrap().0, before);
    }
}

#[test]
fn strict_mode_refuses_on_any_loss_and_changes_nothing() {
    let mut doc = reference();
    let before = doc.patch_bytes().unwrap().0;
    let options = MarkdownOptions {
        strict: true,
        ..MarkdownOptions::default()
    };
    let err = doc
        .insert_markdown("Text ![x](missing.png)\n", &options)
        .unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::UnsupportedStructure)
    );
    assert!(err.to_string().contains("missing.png"));
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn protected_references_need_acknowledgment() {
    let mut doc = Document::new();
    doc.set_protection("readOnly").unwrap();
    let before = doc.patch_bytes().unwrap().0;
    let err = doc
        .insert_markdown("x", &MarkdownOptions::default())
        .unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::DocumentProtected)
    );
    assert_eq!(doc.patch_bytes().unwrap().0, before);
    doc.acknowledge_protection().unwrap();
    doc.insert_markdown("x", &MarkdownOptions::default())
        .unwrap();
    assert!(doc.protection_status().unwrap().enforced);
}

#[test]
fn conversions_are_deterministic() {
    let markdown = std::fs::read_to_string(repo().join("examples/markdown/showcase.md")).unwrap();
    // Conversions stamp the document as created now.
    let options = MarkdownOptions {
        resource_dir: Some(repo().join("examples/markdown")),
        now: Some(docrs::clock::Timestamp::from_unix(1_700_000_000)),
        ..MarkdownOptions::default()
    };
    let first = export_with("28-showcase", &markdown, Some(reference()), &options);
    let second = export_with("28-showcase-again", &markdown, Some(reference()), &options);
    assert_eq!(first.bytes, second.bytes);
    assert!(
        first.report.warnings.is_empty(),
        "{:?}",
        first.report.warnings
    );
    let c = &first.report.counts;
    assert_eq!((c.tables, c.images, c.footnotes), (2, 3, 2));
}

#[test]
fn empty_markdown_leaves_a_valid_empty_body() {
    let out = export("29-empty", "");
    assert_eq!(out.paragraphs(), [("Normal".into(), String::new())]);
}

fn fixture(name: &str) -> PathBuf {
    repo().join("tests/fixtures/markdown/templates").join(name)
}

fn fixture_template(name: &str) -> Document {
    Document::open_path(fixture(name), &Limits::default()).unwrap()
}

#[test]
fn branded_list_templates_keep_their_numbering() {
    let markdown = std::fs::read_to_string(fixture("list-shapes.md")).unwrap();
    let out = export_with(
        "30-list-style-template",
        &markdown,
        Some(fixture_template("list-style-template.docx")),
        &MarkdownOptions::default(),
    );
    assert!(out.report.warnings.is_empty(), "{:?}", out.report.warnings);
    assert_eq!(out.report.styles["list_bullet"].name, "List Bullet");
    assert_eq!(out.report.styles["list_number"].name, "List Number");
    let body = out.body();
    // Bullets use the template's instance (numId 6, with its lvlOverride);
    // the level the template lacks continues its indent step.
    assert!(body.contains("<w:numId w:val=\"6\"/>"));
    assert!(
        body.contains("<w:ind w:left=\"1136\" w:hanging=\"284\"/>"),
        "{body}"
    );
    assert!(
        !out.part("word/numbering.xml")
            .contains("<w:numId w:val=\"6\"/><w:ilvl w:val=\"3\"/>")
    );
    let table = &out.doc.tables().unwrap()[0];
    assert_eq!(
        table.cell(&out.doc, 1, 1).unwrap().text(&out.doc).unwrap(),
        "Confirm the closing balance\nConfirm the insurance cover"
    );
}

#[test]
fn messy_editor_markdown_converts_without_loss() {
    let markdown = std::fs::read_to_string(fixture("record-of-advice.md")).unwrap();
    for template in [
        "docx-template-missing-pandoc-styles.normalised.docx",
        "list-style-template.docx",
        "existing-template.docx",
    ] {
        let out = export_with(
            &format!("31-record-of-advice-{}", template.trim_end_matches(".docx")),
            &markdown,
            Some(fixture_template(template)),
            &MarkdownOptions::default(),
        );
        assert!(
            out.report.warnings.is_empty(),
            "{template}: {:?}",
            out.report.warnings
        );
        assert!(out.report.counts.tables >= 4, "{template}");
        // Intentional blank lines (`<p></p>`) survive.
        assert!(
            out.paragraphs().iter().any(|(_, text)| text.is_empty()),
            "{template}"
        );
    }
}

#[test]
fn templates_with_stray_directory_data_are_refused_like_upstream() {
    let err = Document::open_path(
        fixture("docx-template-missing-pandoc-styles.docx"),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::MalformedPackage)
    );
}

fn v2(name: &str) -> Document {
    fixture_template(&format!("v2/{name}.docx"))
}

#[test]
fn branding_shown_only_in_sample_content_is_lifted() {
    let markdown = "# Heading\n\nBody text.\n\n- one\n- two\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
    let out = export_with(
        "32-v2-outline-level-headings",
        markdown,
        Some(v2("outline-level-headings")),
        &MarkdownOptions::default(),
    );
    let styles = out.part("word/styles.xml");
    let heading = &styles[styles.find("w:styleId=\"Heading1\"").unwrap()..];
    let heading = &heading[..heading.find("</w:style>").unwrap()];
    assert!(heading.contains("<w:outlineLvl w:val=\"0\"/>"), "{heading}");
    assert!(
        !heading.contains("2F5496"),
        "lifted, not the converter's default: {heading}"
    );
    let out = export_with(
        "33-v2-direct-formatted-table",
        markdown,
        Some(v2("direct-formatted-table")),
        &MarkdownOptions::default(),
    );
    let styles = out.part("word/styles.xml");
    let table = &styles[styles.find("w:styleId=\"Table\"").unwrap()..];
    let table = &table[..table.find("</w:style>").unwrap()];
    assert!(
        table.contains("<w:tblBorders>") && table.contains("firstRow"),
        "{table}"
    );
    let out = export_with(
        "34-v2-builtin-table-style-only",
        markdown,
        Some(v2("builtin-table-style-only")),
        &MarkdownOptions::default(),
    );
    assert_ne!(out.report.styles["table"].name, "Table");
    assert!(!out.report.styles["table"].created);
    let out = export_with(
        "35-v2-direct-run-formatting",
        markdown,
        Some(v2("direct-run-formatting")),
        &MarkdownOptions::default(),
    );
    assert!(out.report.created_styles.iter().any(|s| s == "Body Text"));
    let styles = out.part("word/styles.xml");
    let body_text = &styles[styles.find("w:styleId=\"BodyText\"").unwrap()..];
    assert!(body_text[..body_text.find("</w:style>").unwrap()].contains("Arial"));
    let out = export_with(
        "36-v2-list-paragraph-bullets",
        markdown,
        Some(v2("list-paragraph-bullets")),
        &MarkdownOptions::default(),
    );
    assert!(out.report.styles.contains_key("list_bullet"));
    let out = export_with(
        "37-v2-paragraph-style",
        markdown,
        Some(v2("paragraph-style")),
        &MarkdownOptions::default(),
    );
    assert_eq!(out.report.styles["heading1"].name, "Section Heading");
    assert_eq!(out.report.styles["body"].name, "Paragraph");
}

#[test]
fn every_v2_template_converts_cleanly() {
    let markdown = std::fs::read_to_string(fixture("record-of-advice.md")).unwrap();
    let dir = fixture("v2");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".docx"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 20);
    for name in names {
        let stem = name.trim_end_matches(".docx");
        let options = MarkdownOptions {
            toc: Some(true),
            page_numbers: true,
            ..MarkdownOptions::default()
        };
        let out = export_with(
            &format!("38-v2-{stem}"),
            &markdown,
            Some(v2(stem)),
            &options,
        );
        assert!(
            out.report.warnings.is_empty(),
            "{stem}: {:?}",
            out.report.warnings
        );
        let body = out.body();
        assert!(!body.contains("<w:tblW w:type=\"auto\""), "{stem}");
        assert!(
            !out.part("word/settings.xml").contains("updateFields"),
            "{stem}"
        );
        assert!(out.part("word/settings.xml").contains("compat"), "{stem}");
    }
}

// Synthetic templates in the shapes practices upload: observed in real export
// templates, built here from scratch, no customer content.

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Default)]
struct Synthetic<'a> {
    styles: &'a str,
    body: &'a str,
    numbering: &'a str,
    /// A distinct first page whose header holds a picture this tall (EMU).
    cover_height: Option<i64>,
    /// A default footer holding these paragraphs.
    footer: Option<&'a str>,
}

fn synthetic(template: Synthetic<'_>) -> Document {
    let ct = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Default Extension=\"png\" ContentType=\"image/png\"/>\
        <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
        <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
        <Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>\
        <Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
        <Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>\
        <Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>\
        </Types>";
    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
        </Relationships>";
    let rel = |id: &str, kind: &str, target: &str| {
        format!(
            "<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}\" Target=\"{target}\"/>"
        )
    };
    let mut doc_rels = rel("rId1", "styles", "styles.xml")
        + &rel("rId2", "numbering", "numbering.xml")
        + &rel("rId3", "settings", "settings.xml");
    let mut section = String::new();
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    let story = |root: &str, inner: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:{root} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" \
             xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">{inner}</w:{root}>"
        )
        .into_bytes()
    };
    let picture_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}</Relationships>",
        rel("rIdImage", "image", "media/cover.png")
    )
    .into_bytes();
    if let Some(height) = template.cover_height {
        doc_rels += &rel("rId4", "header", "header1.xml");
        section += "<w:headerReference w:type=\"first\" r:id=\"rId4\"/>";
        let cover = picture(5_000_000, height, false);
        members.push(("word/header1.xml".into(), story("hdr", &cover)));
        members.push(("word/_rels/header1.xml.rels".into(), picture_rels.clone()));
    }
    if let Some(footer) = template.footer {
        doc_rels += &rel("rId5", "footer", "footer1.xml");
        section += "<w:footerReference w:type=\"default\" r:id=\"rId5\"/>";
        members.push(("word/footer1.xml".into(), story("ftr", footer)));
        members.push(("word/_rels/footer1.xml.rels".into(), picture_rels.clone()));
    }
    if template.cover_height.is_some() || template.footer.is_some() {
        members.push(("word/media/cover.png".into(), png_bytes()));
    }
    let title_pg = if template.cover_height.is_some() {
        "<w:titlePg/>"
    } else {
        ""
    };
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:body>{}\
         <w:sectPr>{section}<w:pgSz w:w=\"12240\" w:h=\"15840\"/>\
         <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/>\
         {title_pg}</w:sectPr></w:body></w:document>",
        template.body
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:styles xmlns:w=\"{W_NS}\">\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
         <w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/>\
         <w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>\
         {}</w:styles>",
        template.styles
    );
    let numbering = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:numbering xmlns:w=\"{W_NS}\">{}</w:numbering>",
        template.numbering
    );
    let settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"{W_NS}\"/>"
    );
    members.extend([
        ("[Content_Types].xml".to_string(), ct.as_bytes().to_vec()),
        ("_rels/.rels".to_string(), root_rels.as_bytes().to_vec()),
        ("word/document.xml".to_string(), document.into_bytes()),
        (
            "word/_rels/document.xml.rels".to_string(),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{doc_rels}</Relationships>"
            )
            .into_bytes(),
        ),
        ("word/styles.xml".to_string(), styles.into_bytes()),
        ("word/numbering.xml".to_string(), numbering.into_bytes()),
        ("word/settings.xml".to_string(), settings.into_bytes()),
    ]);
    let bytes = docrs::package::zip_write::write_archive(
        members.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
        &Limits::default(),
    )
    .unwrap();
    Document::open_bytes(&bytes, &Limits::default()).expect("synthetic template opens")
}

/// A paragraph holding a picture `cx` by `cy` EMU, inline or anchored to
/// the page.
fn picture(cx: i64, cy: i64, anchored: bool) -> String {
    let extent = format!("<wp:extent cx=\"{cx}\" cy=\"{cy}\"/>");
    let graphic = format!(
        "<wp:docPr id=\"1\" name=\"Picture\"/>\
         <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:pic><pic:nvPicPr><pic:cNvPr id=\"1\" name=\"picture.png\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"rIdImage\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
         </a:graphicData></a:graphic>"
    );
    let drawing = if anchored {
        format!(
            "<wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"1\" \
             behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/>\
             <wp:positionH relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionH>\
             <wp:positionV relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionV>\
             {extent}<wp:wrapNone/>{graphic}</wp:anchor>"
        )
    } else {
        format!(
            "<wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">{extent}{graphic}</wp:inline>"
        )
    };
    format!("<w:p><w:r><w:drawing>{drawing}</w:drawing></w:r></w:p>")
}

fn style_xml<'a>(styles: &'a str, id: &str) -> &'a str {
    let start = styles
        .find(&format!("w:styleId=\"{id}\""))
        .unwrap_or_else(|| panic!("no style {id}"));
    let rest = &styles[start..];
    &rest[..rest.find("</w:style>").unwrap()]
}

fn row(cells: &[&str], tc_pr: &str, r_pr: &str) -> String {
    let mut xml = String::from("<w:tr>");
    for cell in cells {
        xml.push_str(&format!(
            "<w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/>{tc_pr}</w:tcPr>\
             <w:p><w:r><w:rPr>{r_pr}</w:rPr><w:t>{cell}</w:t></w:r></w:p></w:tc>"
        ));
    }
    xml.push_str("</w:tr>");
    xml
}

fn sample_table(style: Option<&str>, rows: &[String]) -> String {
    let style = style
        .map(|s| format!("<w:tblStyle w:val=\"{s}\"/>"))
        .unwrap_or_default();
    format!(
        "<w:tbl><w:tblPr>{style}<w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>\
         <w:tblGrid><w:gridCol w:w=\"3000\"/><w:gridCol w:w=\"3000\"/></w:tblGrid>{}</w:tbl>",
        rows.concat()
    )
}

/// Pandoc's own `Table` style as an upload normaliser leaves it: no borders,
/// no fill (advisers cannot create a style named "Table" in Word).
const EMPTY_TABLE_STYLE: &str = "<w:style w:type=\"table\" w:customStyle=\"1\" w:styleId=\"Table\"><w:name w:val=\"Table\"/>\
    <w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/></w:tblPr>\
    <w:tblStylePr w:type=\"firstRow\"><w:tblPr/><w:tcPr><w:tcBorders><w:bottom w:val=\"single\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/></w:tcBorders></w:tcPr></w:tblStylePr></w:style>";

const TABLE_GRID: &str = "<w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:basedOn w:val=\"TableNormal\"/>\
    <w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
    <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
    <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/></w:tblBorders></w:tblPr></w:style>";

const GREY_BODY_TEXT: &str = "<w:style w:type=\"paragraph\" w:styleId=\"BodyText\"><w:name w:val=\"Body Text\"/><w:basedOn w:val=\"Normal\"/>\
    <w:rPr><w:color w:val=\"595959\"/></w:rPr></w:style>";

const TABLE_MARKDOWN: &str = "| Client | Age |\n|---|---|\n| Jane | 57 |\n| John | 60 |\n";

#[test]
fn a_hand_formatted_sample_table_beats_an_empty_table_style() {
    let rows = [
        row(
            &["Head", "Head"],
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"22524F\"/>",
            "<w:b/><w:color w:val=\"FFFFFF\"/>",
        ),
        row(&["Cell", "Cell"], "", ""),
    ];
    let template = synthetic(Synthetic {
        styles: &format!("{EMPTY_TABLE_STYLE}{TABLE_GRID}{GREY_BODY_TEXT}"),
        body: &sample_table(Some("TableGrid"), &rows),
        ..Synthetic::default()
    });
    let out = export_with(
        "41-synthetic-hand-formatted-table",
        TABLE_MARKDOWN,
        Some(template),
        &MarkdownOptions::default(),
    );
    let table = &out.report.styles["table"];
    assert_eq!(table.name, "Template Table");
    assert!(table.created);
    let styles = out.part("word/styles.xml");
    let lifted = style_xml(&styles, table.style_id.as_deref().unwrap());
    assert!(
        lifted.contains("<w:basedOn w:val=\"TableGrid\"/>"),
        "{lifted}"
    );
    assert!(lifted.contains("w:fill=\"22524F\""), "{lifted}");
    assert!(!lifted.contains("<w:rPr>"), "{lifted}");
    // The header text's white bold goes to a Table Heading on the cells'
    // style (Body Text, grey), where every reader applies it; a table
    // style's colour would lose to the paragraph style's in Word.
    let heading = style_of(&out, "TableHeading");
    assert!(
        heading.contains("<w:basedOn w:val=\"BodyText\"/>")
            && heading.contains("<w:b/>")
            && heading.contains("<w:color w:val=\"FFFFFF\"/>"),
        "{heading}"
    );
    let (header, body) = header_row(&out);
    assert_eq!(
        count(&header, "<w:pStyle w:val=\"TableHeading\"/>"),
        2,
        "{header}"
    );
    assert!(!header.contains("FFFFFF"), "{header}");
    let second = &body[body.find("</w:tr>").unwrap()..];
    assert!(!second[..second.find("</w:tbl>").unwrap()].contains("TableHeading"));
}

#[test]
fn the_style_most_sample_tables_use_wins_over_the_pandoc_name() {
    // "Table1": what a practice makes when Word will not let it name a
    // style "Table".
    let table1 = "<w:style w:type=\"table\" w:customStyle=\"1\" w:styleId=\"Table1\"><w:name w:val=\"Table1\"/><w:basedOn w:val=\"TableGrid\"/>\
        <w:tblStylePr w:type=\"firstRow\"><w:rPr><w:b/></w:rPr><w:tblPr/><w:tcPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"A6A6A6\"/></w:tcPr></w:tblStylePr></w:style>";
    let plain = [row(&["A", "B"], "", ""), row(&["1", "2"], "", "")];
    // These samples switch first-column emphasis off (Word 2007's hex form).
    let table1_sample = sample_table(Some("Table1"), &plain).replace(
        "<w:tblW w:w=\"0\" w:type=\"auto\"/>",
        "<w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblLook w:val=\"0420\"/>",
    );
    let template = synthetic(Synthetic {
        styles: &format!("{EMPTY_TABLE_STYLE}{TABLE_GRID}{table1}"),
        body: &(sample_table(Some("TableGrid"), &plain)
            + "<w:p/>"
            + &table1_sample
            + "<w:p/>"
            + &table1_sample),
        ..Synthetic::default()
    });
    let out = export_with(
        "42-synthetic-table1",
        TABLE_MARKDOWN,
        Some(template),
        &MarkdownOptions::default(),
    );
    assert_eq!(out.report.styles["table"].name, "Table1");
    assert!(!out.report.styles["table"].created);
    let body = out.body();
    assert!(body.contains("<w:tblStyle w:val=\"Table1\"/>"));
    let look = &body[body.find("<w:tblLook").unwrap()..];
    let look = &look[..look.find("/>").unwrap()];
    assert!(
        look.contains("w:firstRow=\"1\"") && look.contains("w:firstColumn=\"0\""),
        "{look}"
    );
    assert!(
        look.contains("w:noVBand=\"1\"") && look.contains("w:val=\"0420\""),
        "{look}"
    );
}

#[test]
fn alternating_row_fills_become_banding() {
    let rows = [
        row(
            &["Head", "Head"],
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"1F3864\"/>",
            "<w:color w:val=\"FFFFFF\"/>",
        ),
        row(
            &["1", "2"],
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"DEEAF6\"/>",
            "",
        ),
        row(&["3", "4"], "", ""),
        row(
            &["5", "6"],
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"DEEAF6\"/>",
            "",
        ),
    ];
    let template = synthetic(Synthetic {
        styles: TABLE_GRID,
        body: &sample_table(None, &rows),
        ..Synthetic::default()
    });
    let out = export_with(
        "43-synthetic-banding",
        TABLE_MARKDOWN,
        Some(template),
        &MarkdownOptions::default(),
    );
    // No "Table" style to keep apart from: the lift is named Table.
    assert_eq!(out.report.styles["table"].name, "Table");
    let styles = out.part("word/styles.xml");
    let table = style_xml(&styles, "Table");
    assert!(
        table.contains("<w:tblStyleRowBandSize w:val=\"1\"/>"),
        "{table}"
    );
    let band = &table[table.find("w:type=\"band1Horz\"").expect(table)..];
    assert!(
        band[..band.find("</w:tblStylePr>").unwrap()].contains("DEEAF6"),
        "{table}"
    );
    let first = &table[table.find("w:type=\"firstRow\"").expect(table)..];
    assert!(first.contains("1F3864"), "{table}");
    assert!(out.body().contains("w:noHBand=\"0\""));
}

#[test]
fn custom_bullet_styles_bound_to_numbering_are_the_lists() {
    let level = |ilvl: u8, format: &str, text: &str, left: u32| {
        format!(
            "<w:lvl w:ilvl=\"{ilvl}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{format}\"/><w:lvlText w:val=\"{text}\"/>\
             <w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{left}\" w:hanging=\"360\"/></w:pPr></w:lvl>"
        )
    };
    let numbering = format!(
        "<w:abstractNum w:abstractNumId=\"1\">{}</w:abstractNum>\
         <w:abstractNum w:abstractNumId=\"2\">{}</w:abstractNum>\
         <w:abstractNum w:abstractNumId=\"3\">{}{}{}</w:abstractNum>\
         <w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num>\
         <w:num w:numId=\"3\"><w:abstractNumId w:val=\"2\"/></w:num>\
         <w:num w:numId=\"8\"><w:abstractNumId w:val=\"3\"/></w:num>",
        level(0, "bullet", "\u{2022}", 720),
        level(0, "bullet", "\u{2013}", 1440),
        level(0, "decimal", "%1", 0),
        level(1, "decimal", "%1.%2", 0),
        level(2, "decimal", "%1.%2.%3", 0),
    );
    let styles = "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"BulletPoints\"><w:name w:val=\"Bullet Points\"/><w:basedOn w:val=\"Normal\"/>\
        <w:pPr><w:numPr><w:numId w:val=\"2\"/></w:numPr><w:spacing w:before=\"120\" w:after=\"120\"/></w:pPr></w:style>\
        <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"SecondBulletPoint\"><w:name w:val=\"Second Bullet Point\"/><w:basedOn w:val=\"Normal\"/>\
        <w:pPr><w:numPr><w:numId w:val=\"3\"/></w:numPr></w:pPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/>\
        <w:pPr><w:numPr><w:ilvl w:val=\"2\"/><w:numId w:val=\"8\"/></w:numPr><w:outlineLvl w:val=\"2\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>";
    let body = "<w:p><w:pPr><w:pStyle w:val=\"Heading3\"/></w:pPr><w:r><w:t>Numbered heading</w:t></w:r></w:p>\
        <w:p><w:pPr><w:pStyle w:val=\"BulletPoints\"/></w:pPr><w:r><w:t>Bullet</w:t></w:r></w:p>\
        <w:p><w:pPr><w:pStyle w:val=\"SecondBulletPoint\"/></w:pPr><w:r><w:t>Second bullet</w:t></w:r></w:p>";
    let template = synthetic(Synthetic {
        styles,
        body,
        numbering: &numbering,
        ..Synthetic::default()
    });
    let out = export_with(
        "44-synthetic-custom-bullets",
        "- one\n  - two\n\n1. first\n2. second\n",
        Some(template),
        &MarkdownOptions::default(),
    );
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0], ("Bullet Points".into(), "one".into()));
    assert_eq!(paragraphs[1], ("Second Bullet Point".into(), "two".into()));
    assert!(
        paragraphs[2..]
            .iter()
            .all(|(style, _)| style != "heading 3"),
        "{paragraphs:?}"
    );
    let body = out.body();
    let first = &body[body.find(">first<").unwrap() - 400..body.find(">first<").unwrap()];
    assert!(!first.contains("<w:numId w:val=\"8\"/>"), "{first}");
}

#[test]
fn tables_are_sized_in_twips_and_justified_breaks_do_not_stretch() {
    let template = synthetic(Synthetic::default());
    let out = export_with(
        "45-synthetic-dxa-width",
        &format!("Line one  \nline two\n\n{TABLE_MARKDOWN}"),
        Some(template),
        &MarkdownOptions::default(),
    );
    // Letter, 1 inch margins: 12240 - 2 * 1440.
    assert!(out.body().contains("<w:tblW w:type=\"dxa\" w:w=\"9360\"/>"));
    assert!(!out.body().contains("w:type=\"pct\""));
    assert!(
        out.part("word/settings.xml")
            .contains("<w:doNotExpandShiftReturn/>")
    );
}

#[test]
fn a_cover_drawn_in_the_first_page_header_keeps_page_one() {
    let padding = "<w:p/>".repeat(10) + "<w:p><w:r><w:t>Sample</w:t></w:r></w:p>";
    let cover = || {
        synthetic(Synthetic {
            body: &padding,
            cover_height: Some(9_000_000),
            ..Synthetic::default()
        })
    };
    let starts_with_page_break = |out: &Export| {
        let body = out.body();
        let first = &body[body.find("<w:body>").unwrap()..];
        let first = &first[..first.find("</w:p>").unwrap()];
        first.contains("<w:br w:type=\"page\"/>")
    };
    let out = export_with(
        "46-synthetic-cover",
        "# Advice\n\nBody.\n",
        Some(cover()),
        &MarkdownOptions::default(),
    );
    assert_eq!(
        out.report.cover_page.as_deref(),
        Some("content_after_cover")
    );
    assert!(starts_with_page_break(&out));
    assert!(out.body().contains("<w:titlePg/>"));
    // Turned off: content on page 1, which takes the regular header.
    let options = MarkdownOptions {
        cover_page: Some(false),
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "47-synthetic-no-cover",
        "# Advice\n\nBody.\n",
        Some(cover()),
        &options,
    );
    assert_eq!(out.report.cover_page.as_deref(), Some("cover_removed"));
    assert!(!starts_with_page_break(&out));
    assert!(!out.body().contains("titlePg"));
    // A letterhead logo, or a sample that does not leave page 1 blank, is
    // not a cover.
    for (height, body) in [
        (600_000, padding.as_str()),
        (9_000_000, "<w:p><w:r><w:t>Sample</w:t></w:r></w:p>"),
    ] {
        let template = synthetic(Synthetic {
            body,
            cover_height: Some(height),
            ..Synthetic::default()
        });
        let out = export_with(
            "48-synthetic-letterhead",
            "Body.\n",
            Some(template),
            &MarkdownOptions::default(),
        );
        assert_eq!(out.report.cover_page, None);
        assert!(!starts_with_page_break(&out));
    }
}

#[test]
fn supplied_images_without_an_extension_get_one() {
    let options = MarkdownOptions {
        images: std::collections::BTreeMap::from([("img-123".to_string(), png_bytes())]),
        ..MarkdownOptions::default()
    };
    let out = export_with(
        "50-image-key-without-extension",
        "![Chart](img-123)\n",
        None,
        &options,
    );
    assert_eq!(out.report.counts.images, 1);
    let media: Vec<String> = out
        .doc
        .package()
        .parts()
        .map(|part| part.name().to_string())
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert_eq!(media, ["word/media/image1.png"]);
}

#[test]
fn tolerated_reference_packages_convert() {
    let (template, set_aside) = Document::open_path_with(
        fixture("docx-template-missing-pandoc-styles.docx"),
        &Limits::default(),
        docrs::Tolerance::word(),
    )
    .unwrap();
    // Its `word/` and `word/_rels/` entries each hold an empty stream.
    assert_eq!(set_aside.len(), 2, "{set_aside:?}");
    let out = export_with(
        "49-tolerated-directory-data",
        TABLE_MARKDOWN,
        Some(template),
        &MarkdownOptions::default(),
    );
    assert_eq!(out.report.counts.tables, 1);
}

/// A TOC field as Word writes one, with a stale result.
const TOC_FIELD: &str = "<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-3\" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>Old entry</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>";

const TOC_HEADING_STYLE: &str = "<w:style w:type=\"paragraph\" w:styleId=\"TOCHeading\"><w:name w:val=\"TOC Heading\"/></w:style>";

/// Word's table-of-contents block with `title` above the field.
fn word_toc(title: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:id w:val=\"7\"/><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val=\"TOCHeading\"/></w:pPr><w:r><w:t>{title}</w:t></w:r></w:p>{TOC_FIELD}</w:sdtContent></w:sdt>"
    )
}

/// The title the conversion wrote above its contents (the last heading in
/// `style`, after any the template kept).
fn written_title(out: &Export, style: &str) -> String {
    let body = out.body();
    let marker = format!("<w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t>");
    let start = body.rfind(&marker).expect("a contents heading") + marker.len();
    body[start..start + body[start..].find("</w:t>").unwrap()].to_string()
}

#[test]
fn the_contents_title_comes_from_the_option_the_template_or_the_default() {
    let toc = MarkdownOptions {
        toc: Some(true),
        ..MarkdownOptions::default()
    };
    let source = |out: &Export| out.report.toc_title_source.clone();
    let inspect =
        |doc: &Document| docrs::markdown::inspect(doc, &MarkdownOptions::default()).unwrap();
    let block = word_toc("Index of Advice");
    let word = || {
        synthetic(Synthetic {
            styles: TOC_HEADING_STYLE,
            body: &block,
            ..Synthetic::default()
        })
    };
    assert_eq!(inspect(&word())["toc_title"], "Index of Advice");
    let out = export_with("51-toc-title-word", "# A\n", Some(word()), &toc);
    assert_eq!(written_title(&out, "TOCHeading"), "Index of Advice");
    assert_eq!(source(&out).as_deref(), Some("reference"));
    let appended = export_with(
        "51-toc-title-append",
        "# A\n",
        Some(word()),
        &MarkdownOptions {
            placement: Placement::Append,
            ..toc.clone()
        },
    );
    assert_eq!(written_title(&appended, "TOCHeading"), "Index of Advice");
    assert_eq!(count(&appended.body(), "<w:sdt>"), 2);
    let with_marker = format!("{block}<w:p><w:r><w:t>{{{{BODY}}}}</w:t></w:r></w:p>");
    let placed = export_with(
        "51-toc-title-placeholder",
        "# A\n",
        Some(synthetic(Synthetic {
            styles: TOC_HEADING_STYLE,
            body: &with_marker,
            ..Synthetic::default()
        })),
        &MarkdownOptions {
            placement: Placement::Placeholder("{{BODY}}".into()),
            ..toc.clone()
        },
    );
    assert_eq!(written_title(&placed, "TOCHeading"), "Index of Advice");
    assert_eq!(source(&placed).as_deref(), Some("reference"));
    let chosen = export_with(
        "51-toc-title-option",
        "# A\n",
        Some(word()),
        &MarkdownOptions {
            toc_title: Some("Mine".into()),
            ..toc.clone()
        },
    );
    assert_eq!(written_title(&chosen, "TOCHeading"), "Mine");
    assert_eq!(source(&chosen).as_deref(), Some("option"));
    let fronted = export_with(
        "51-toc-title-front-matter",
        "---\ntoc-title: From Front Matter\n---\n\n# A\n",
        Some(word()),
        &toc,
    );
    assert_eq!(written_title(&fronted, "TOCHeading"), "From Front Matter");
    assert_eq!(source(&fronted).as_deref(), Some("front_matter"));
    let named = "<w:style w:type=\"paragraph\" w:styleId=\"Heading0\"><w:name w:val=\"TOC Heading\"/></w:style>";
    for (styles, style) in [(TOC_HEADING_STYLE, "TOCHeading"), (named, "Heading0")] {
        let body = format!(
            "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t xml:space=\"preserve\"> Advice Contents </w:t></w:r></w:p><w:p/>{TOC_FIELD}"
        );
        let bare = synthetic(Synthetic {
            styles,
            body: &body,
            ..Synthetic::default()
        });
        assert_eq!(inspect(&bare)["toc_title"], "Advice Contents");
        let out = export_with(
            &format!("51-toc-title-bare-{style}"),
            "# A\n",
            Some(bare),
            &toc,
        );
        assert_eq!(written_title(&out, style), "Advice Contents");
        assert_eq!(source(&out).as_deref(), Some("reference"));
    }
    let long = word_toc(&"x".repeat(81));
    let plain = format!("<w:p><w:r><w:t>Not a heading</w:t></w:r></w:p>{TOC_FIELD}");
    let blank = word_toc(" ");
    for (name, styles, body) in [
        ("51-toc-title-plain", TOC_HEADING_STYLE, plain.as_str()),
        ("51-toc-title-blank", TOC_HEADING_STYLE, blank.as_str()),
        ("51-toc-title-long", TOC_HEADING_STYLE, long.as_str()),
        ("51-toc-title-none", "", ""),
    ] {
        let template = synthetic(Synthetic {
            styles,
            body,
            ..Synthetic::default()
        });
        assert!(inspect(&template)["toc_title"].is_null(), "{name}");
        let out = export_with(name, "# A\n", Some(template), &toc);
        assert_eq!(
            written_title(&out, "TOCHeading"),
            "Table of Contents",
            "{name}"
        );
        assert_eq!(source(&out).as_deref(), Some("default"), "{name}");
    }
    let none = export("51-toc-title-no-toc", "# A\n");
    assert_eq!(none.report.toc_title_source, None);
}

// ------------------------------------------------- looks shown by hand

fn styled_paragraph(style: &str, text: &str, rpr: &str) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r>{rpr}<w:t>{text}</w:t></w:r></w:p>"
    )
}

fn style_rpr(out: &Export, id: &str) -> String {
    let style = style_of(out, id);
    match style.find("<w:rPr>") {
        Some(start) => style[start..].to_string(),
        None => String::new(),
    }
}

const BOOKMAN_BODY_TEXT: &str = "<w:style w:type=\"paragraph\" w:styleId=\"BodyText\"><w:name w:val=\"Body Text\"/><w:basedOn w:val=\"Normal\"/>\
    <w:rPr><w:rFonts w:ascii=\"Bookman Old Style\" w:hAnsi=\"Bookman Old Style\"/><w:sz w:val=\"20\"/></w:rPr></w:style>";

const CALIBRI: &str = "<w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:cs=\"Calibri\"/><w:color w:val=\"404040\"/><w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/></w:rPr>";
const GEORGIA: &str =
    "<w:rPr><w:rFonts w:ascii=\"Georgia\" w:hAnsi=\"Georgia\"/><w:sz w:val=\"24\"/></w:rPr>";
const VERDANA: &str = "<w:rPr><w:rFonts w:ascii=\"Verdana\" w:hAnsi=\"Verdana\"/></w:rPr>";

const HEADINGS_AND_BODY: &str =
    "# Heading one\n\nBody paragraph.\n\n## Heading two\n\nMore body.\n";

#[test]
fn a_heading_look_merges_into_the_style_font() {
    let out = export_with(
        "60-heading-cs-font",
        HEADINGS_AND_BODY,
        Some(fixture_template("heading-cs-font.docx")),
        &MarkdownOptions::default(),
    );
    for id in ["Heading1", "Heading2"] {
        let style = style_of(&out, id);
        let fonts = &style[style.find("<w:rFonts").unwrap()..];
        let fonts = &fonts[..fonts.find("/>").unwrap()];
        for attribute in [
            "w:ascii=\"Arial\"",
            "w:hAnsi=\"Arial\"",
            "w:eastAsiaTheme=\"majorEastAsia\"",
            "w:cs=\"Arial\"",
        ] {
            assert!(fonts.contains(attribute), "{id}: {fonts}");
        }
        assert!(!fonts.contains("cstheme"), "{id}: {fonts}");
        assert_eq!(count(&style, "<w:rFonts"), 1, "{style}");
        assert!(
            style.contains("<w:b/><w:color w:val=\"003A64\"/>"),
            "{style}"
        );
    }
    assert!(!out.body().contains("<w:rFonts"));
}

#[test]
fn the_body_style_takes_the_look_the_template_shows() {
    let out = export_with(
        "61-body-direct-formatting",
        HEADINGS_AND_BODY,
        Some(fixture_template("body-direct-formatting.docx")),
        &MarkdownOptions::default(),
    );
    let rpr = style_rpr(&out, "BodyText");
    for property in [
        "w:ascii=\"Calibri\"",
        "w:hAnsi=\"Calibri\"",
        "w:cs=\"Calibri\"",
        "<w:color w:val=\"404040\"/>",
        "<w:sz w:val=\"22\"/>",
        "<w:szCs w:val=\"22\"/>",
    ] {
        assert!(rpr.contains(property), "{rpr}");
    }
    assert!(!rpr.contains("Bookman"), "{rpr}");
    assert_eq!(out.report.styles["body"].name, "Body Text");
    assert!(!out.report.styles["body"].created);
    assert!(style_of(&out, "Heading1").contains("w:ascii=\"Arial\""));
    assert!(!style_of(&out, "Normal").contains("Calibri"));
    let body = out.body();
    assert!(
        !body.contains("Calibri") && !body.contains("404040"),
        "{body}"
    );
    let inspection = docrs::markdown::inspect(
        &fixture_template("body-direct-formatting.docx"),
        &MarkdownOptions::default(),
    )
    .unwrap();
    assert_eq!(inspection["lifted"]["body"]["font"], "Calibri");
    assert_eq!(inspection["lifted"]["body"]["size_pt"], 11.0);
    assert_eq!(inspection["lifted"]["body"]["color"], "404040");
}

#[test]
fn the_body_look_is_weighed_by_characters_outside_tables() {
    let cell = format!(
        "<w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>",
        styled_paragraph(
            "BodyText",
            "A label in Verdana long enough to outweigh every body paragraph",
            VERDANA
        )
    );
    let table = format!(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>\
         <w:tblGrid><w:gridCol w:w=\"3000\"/><w:gridCol w:w=\"3000\"/></w:tblGrid>\
         <w:tr>{cell}{cell}</w:tr><w:tr>{cell}{cell}</w:tr></w:tbl>"
    );
    let long = "A single paragraph in Georgia that carries most of the characters.";
    let export = |name: &str, body: &str| {
        let template = synthetic(Synthetic {
            styles: &format!("{BOOKMAN_BODY_TEXT}{TABLE_GRID}"),
            body,
            ..Synthetic::default()
        });
        export_with(name, "Body.\n", Some(template), &MarkdownOptions::default())
    };
    let out = export(
        "62-body-look-by-characters",
        &format!(
            "{}{}{}{table}",
            styled_paragraph("BodyText", "Short.", CALIBRI),
            styled_paragraph("BodyText", "Short.", CALIBRI),
            styled_paragraph("BodyText", long, GEORGIA),
        ),
    );
    let rpr = style_rpr(&out, "BodyText");
    assert!(
        rpr.contains("w:ascii=\"Georgia\"") && rpr.contains("<w:sz w:val=\"24\"/>"),
        "{rpr}"
    );
    assert!(
        !rpr.contains("Verdana") && !rpr.contains("Calibri"),
        "{rpr}"
    );
    // Half and half: no look is dominant, the style stays.
    let out = export(
        "63-body-look-split",
        &format!(
            "{}{}",
            styled_paragraph("BodyText", long, CALIBRI),
            styled_paragraph("BodyText", long, GEORGIA),
        ),
    );
    assert!(style_rpr(&out, "BodyText").contains("Bookman"));
    // Mostly plain: the style already shows what the template shows.
    let out = export(
        "64-body-look-mostly-plain",
        &format!(
            "{}{}",
            styled_paragraph("BodyText", "Short.", CALIBRI),
            styled_paragraph("BodyText", long, ""),
        ),
    );
    let rpr = style_rpr(&out, "BodyText");
    assert!(rpr.contains("Bookman") && !rpr.contains("Calibri"), "{rpr}");
}

/// A first paragraph, a body paragraph, then another first paragraph.
const SECTIONS: &str = "# Heading one\n\nFirst paragraph after a heading.\n\nSecond paragraph.\n\n\
## Heading two\n\nAnother first paragraph.\n";

/// Exports `SECTIONS` with a synthetic template of these styles and body.
fn export_synthetic(name: &str, styles: &str, body: &str) -> Export {
    let template = synthetic(Synthetic {
        styles,
        body,
        ..Synthetic::default()
    });
    export_with(name, SECTIONS, Some(template), &MarkdownOptions::default())
}

fn first_paragraph_style(rpr: &str) -> String {
    format!(
        "<w:style w:type=\"paragraph\" w:styleId=\"FirstParagraph\"><w:name w:val=\"First Paragraph\"/>\
         <w:basedOn w:val=\"Normal\"/><w:next w:val=\"BodyText\"/>{rpr}</w:style>"
    )
}

#[test]
fn the_body_look_is_the_plurality_by_characters() {
    // Prose in Arial 10 pt grey carries 52% of the text, an address block in
    // 8 pt and a bold sub-heading the rest, all in Normal.
    let out = export_with(
        "65-normal-direct-formatting",
        SECTIONS,
        Some(fixture_template("normal-direct-formatting.docx")),
        &MarkdownOptions::default(),
    );
    let rpr = style_rpr(&out, "BodyText");
    for property in [
        "w:ascii=\"Arial\"",
        "<w:color w:val=\"333333\"/>",
        "<w:sz w:val=\"20\"/>",
    ] {
        assert!(rpr.contains(property), "{rpr}");
    }
    assert!(!rpr.contains("808080") && !rpr.contains("0070C0"), "{rpr}");
    assert!(out.report.styles["body"].created);
    assert_eq!(
        out.report.styles["first_paragraph"].style_id.as_deref(),
        Some("BodyText")
    );
    assert!(!style_of(&out, "Normal").contains("Arial"));
    assert_eq!(count(&out.body(), "<w:pStyle w:val=\"BodyText\"/>"), 3);
    let inspection = docrs::markdown::inspect(
        &fixture_template("normal-direct-formatting.docx"),
        &MarkdownOptions::default(),
    )
    .unwrap();
    assert_eq!(inspection["lifted"]["body"]["font"], "Arial");
    assert_eq!(inspection["lifted"]["body"]["size_pt"], 10.0);
    assert_eq!(inspection["lifted"]["body"]["color"], "333333");
}

#[test]
fn a_first_paragraph_style_takes_the_body_look() {
    let out = export_with(
        "66-first-paragraph-style",
        SECTIONS,
        Some(fixture_template("first-paragraph-style.docx")),
        &MarkdownOptions::default(),
    );
    for id in ["BodyText", "FirstParagraph"] {
        let rpr = style_rpr(&out, id);
        for property in [
            "w:ascii=\"Calibri\"",
            "<w:color w:val=\"404040\"/>",
            "<w:sz w:val=\"22\"/>",
        ] {
            assert!(rpr.contains(property), "{id}: {rpr}");
        }
        assert!(!rpr.contains("Bookman"), "{id}: {rpr}");
    }
    assert_eq!(out.report.styles["first_paragraph"].name, "First Paragraph");
    assert_eq!(out.report.styles["body"].name, "Body Text");
    let body = out.body();
    assert_eq!(count(&body, "<w:pStyle w:val=\"FirstParagraph\"/>"), 2);
    assert_eq!(count(&body, "<w:pStyle w:val=\"BodyText\"/>"), 1);
    assert!(!body.contains("Calibri"), "{body}");
    assert!(!style_of(&out, "Normal").contains("Calibri"));

    // A First Paragraph the template uses, wearing only Normal's look,
    // follows the body; one with a font of its own keeps it.
    let long = "A body paragraph in Calibri, long enough to carry the look.";
    let prose = format!(
        "{}{}{}",
        styled_paragraph("FirstParagraph", long, ""),
        styled_paragraph("BodyText", long, CALIBRI),
        styled_paragraph("BodyText", long, CALIBRI)
    );
    let export = |name: &str, first_paragraph: &str| {
        let template = synthetic(Synthetic {
            styles: &format!(
                "{BOOKMAN_BODY_TEXT}{}",
                first_paragraph_style(first_paragraph)
            ),
            body: &prose,
            ..Synthetic::default()
        });
        export_with(name, SECTIONS, Some(template), &MarkdownOptions::default())
    };
    let out = export("67-first-paragraph-follows-the-body", "");
    let rpr = style_rpr(&out, "FirstParagraph");
    assert!(
        rpr.contains("w:ascii=\"Calibri\"") && rpr.contains("<w:sz w:val=\"22\"/>"),
        "{rpr}"
    );
    assert!(style_rpr(&out, "BodyText").contains("Calibri"));
    let out = export("68-first-paragraph-with-a-font-of-its-own", GEORGIA);
    let rpr = style_rpr(&out, "FirstParagraph");
    assert!(rpr.contains("Georgia") && !rpr.contains("Calibri"), "{rpr}");
    assert!(style_rpr(&out, "BodyText").contains("Calibri"));
}

#[test]
fn a_body_text_style_the_template_writes_no_text_in_is_not_the_body() {
    // A 6 pt Body Text used on empty spacer paragraphs only; the prose is
    // Normal, Arial 11 pt by style.
    let out = export_with(
        "69-unused-body-text-style",
        SECTIONS,
        Some(fixture_template("unused-body-text-style.docx")),
        &MarkdownOptions::default(),
    );
    for role in ["body", "first_paragraph"] {
        assert_eq!(out.report.styles[role].name, "Normal", "{role}");
        assert_eq!(out.report.styles[role].style_id, None, "{role}");
    }
    let body = out.body();
    assert!(
        !body.contains("BodyText") && !body.contains("<w:rFonts"),
        "{body}"
    );
    assert!(style_rpr(&out, "BodyText").contains("<w:sz w:val=\"12\"/>"));
    assert!(
        !out.report
            .created_styles
            .iter()
            .any(|s| s.contains("Body Text"))
    );
    let inspection = docrs::markdown::inspect(
        &fixture_template("unused-body-text-style.docx"),
        &MarkdownOptions::default(),
    )
    .unwrap();
    assert_eq!(inspection["roles"]["body"]["name"], "Normal");
    assert!(inspection["lifted"]["body"].is_null());

    // The same spacer style beside Normal prose formatted by hand: the look
    // goes into a style added beside the template's, named apart from it.
    let spacer = "<w:style w:type=\"paragraph\" w:styleId=\"BodyText\"><w:name w:val=\"Body Text\"/>\
        <w:basedOn w:val=\"Normal\"/><w:rPr><w:sz w:val=\"12\"/></w:rPr></w:style>";
    let long = "A Normal paragraph in Calibri, long enough to carry the look.";
    let template = synthetic(Synthetic {
        styles: spacer,
        body: &format!(
            "<w:p><w:pPr><w:pStyle w:val=\"BodyText\"/></w:pPr></w:p>\
             <w:p><w:r>{CALIBRI}<w:t>{long}</w:t></w:r></w:p>\
             <w:p><w:pPr><w:pStyle w:val=\"BodyText\"/></w:pPr></w:p>\
             <w:p><w:r>{CALIBRI}<w:t>{long}</w:t></w:r></w:p>"
        ),
        ..Synthetic::default()
    });
    let out = export_with(
        "70-unused-body-text-beside-a-hand-look",
        SECTIONS,
        Some(template),
        &MarkdownOptions::default(),
    );
    assert_eq!(out.report.styles["body"].name, "Template Body Text");
    assert!(out.report.styles["body"].created);
    assert_eq!(
        out.report.styles["first_paragraph"].style_id.as_deref(),
        Some("TemplateBodyText")
    );
    assert!(style_rpr(&out, "TemplateBodyText").contains("w:ascii=\"Calibri\""));
    let spacer = style_rpr(&out, "BodyText");
    assert!(
        spacer.contains("<w:sz w:val=\"12\"/>") && !spacer.contains("Calibri"),
        "{spacer}"
    );
    assert_eq!(
        count(&out.body(), "<w:pStyle w:val=\"TemplateBodyText\"/>"),
        3
    );
}

// ------------------------------------------------- spacing shown by hand

fn style_ppr(out: &Export, id: &str) -> String {
    let style = style_of(out, id);
    match (style.find("<w:pPr>"), style.find("</w:pPr>")) {
        (Some(start), Some(end)) => style[start..end + "</w:pPr>".len()].to_string(),
        _ => String::new(),
    }
}

fn spaced_paragraph(style: Option<&str>, text: &str, spacing: &str) -> String {
    let style = style
        .map(|id| format!("<w:pStyle w:val=\"{id}\"/>"))
        .unwrap_or_default();
    format!("<w:p><w:pPr>{style}{spacing}</w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>")
}

const TIGHT: &str = "<w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>";
const LOOSE: &str =
    "<w:spacing w:before=\"0\" w:after=\"240\" w:line=\"360\" w:lineRule=\"auto\"/>";

#[test]
fn the_body_style_takes_the_spacing_the_prose_shows() {
    // Body Text says 6 pt after at 1.5 lines; every body paragraph is set
    // by hand to none at single lines.
    let template = || fixture_template("body-spacing-by-hand.docx");
    let out = export_with(
        "71-body-spacing-by-hand",
        SECTIONS,
        Some(template()),
        &MarkdownOptions::default(),
    );
    assert_eq!(
        style_ppr(&out, "BodyText"),
        format!("<w:pPr>{TIGHT}</w:pPr>")
    );
    assert_eq!(out.report.styles["body"].name, "Body Text");
    assert!(!out.report.styles["body"].created);
    assert!(!style_of(&out, "Normal").contains("<w:spacing"));
    assert!(!out.body().contains("<w:spacing"), "{}", out.body());
    let inspection = docrs::markdown::inspect(&template(), &MarkdownOptions::default()).unwrap();
    assert_eq!(inspection["lifted"]["spacing"]["before_pt"], 0.0);
    assert_eq!(inspection["lifted"]["spacing"]["after_pt"], 0.0);
    assert_eq!(inspection["lifted"]["spacing"]["line"], 1.0);
    assert_eq!(inspection["lifted"]["spacing"]["line_rule"], "auto");
    // The gap above a paragraph after a table stays: the style now leaves
    // none, so the conversion's 6 pt fallback goes on the paragraph.
    let out = export_with(
        "72-body-spacing-by-hand-after-a-table",
        "| A | B |\n| --- | --- |\n| 1 | 2 |\n\nAfter the table.\n",
        Some(template()),
        &MarkdownOptions::default(),
    );
    let body = out.body();
    let after = &body[body.find("</w:tbl>").unwrap()..];
    assert!(after.contains("<w:spacing w:before=\"120\"/>"), "{after}");
}

#[test]
fn the_body_spacing_follows_the_prose_in_normal() {
    // The prose is Normal (10 pt after, 1.15 lines by style); Body Text
    // (6 pt after, single) holds one short sample line.
    let template = || fixture_template("body-spacing-in-normal.docx");
    let out = export_with(
        "73-body-spacing-in-normal",
        SECTIONS,
        Some(template()),
        &MarkdownOptions::default(),
    );
    assert_eq!(
        style_ppr(&out, "BodyText"),
        "<w:pPr><w:spacing w:before=\"0\" w:after=\"200\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr>"
    );
    assert_eq!(
        style_ppr(&out, "Normal"),
        "<w:pPr><w:spacing w:after=\"200\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr>"
    );
    assert_eq!(out.report.styles["body"].name, "Body Text");
    assert_eq!(count(&out.body(), "<w:pStyle w:val=\"BodyText\"/>"), 3);
    let inspection = docrs::markdown::inspect(&template(), &MarkdownOptions::default()).unwrap();
    assert_eq!(inspection["lifted"]["spacing"]["after_pt"], 10.0);
    assert_eq!(inspection["lifted"]["spacing"]["line"], 1.15);
    assert!(inspection["lifted"]["body"].is_null());
}

#[test]
fn body_spacing_is_added_followed_and_left_alone() {
    let long = "A body paragraph spaced by hand, long enough to carry the spacing.";
    // Unstyled prose spaced by hand, no run look: the Body Text added for
    // it carries the spacing and nothing else.
    let out = export_synthetic(
        "74-unstyled-prose-spaced-by-hand",
        "",
        &format!(
            "{}{}",
            spaced_paragraph(None, long, LOOSE),
            spaced_paragraph(None, long, LOOSE)
        ),
    );
    assert!(out.report.styles["body"].created);
    assert_eq!(out.report.styles["body"].name, "Body Text");
    let style = style_of(&out, "BodyText");
    assert!(
        style.contains(LOOSE) && !style.contains("<w:rPr>"),
        "{style}"
    );
    // A First Paragraph the template uses, with no spacing of its own,
    // follows the body; one with its own keeps it.
    let prose = format!(
        "{}{}{}",
        spaced_paragraph(Some("FirstParagraph"), long, ""),
        spaced_paragraph(Some("BodyText"), long, LOOSE),
        spaced_paragraph(Some("BodyText"), long, LOOSE)
    );
    let out = export_synthetic(
        "75-first-paragraph-follows-the-body-spacing",
        &format!("{BOOKMAN_BODY_TEXT}{}", first_paragraph_style("")),
        &prose,
    );
    assert_eq!(
        style_ppr(&out, "BodyText"),
        format!("<w:pPr>{LOOSE}</w:pPr>")
    );
    assert_eq!(
        style_ppr(&out, "FirstParagraph"),
        format!("<w:pPr>{LOOSE}</w:pPr>")
    );
    let own = "<w:pPr><w:spacing w:after=\"480\"/></w:pPr>";
    let out = export_synthetic(
        "76-first-paragraph-with-spacing-of-its-own",
        &format!("{BOOKMAN_BODY_TEXT}{}", first_paragraph_style(own)),
        &prose,
    );
    assert_eq!(style_ppr(&out, "FirstParagraph"), own);
    // Half and half: no spacing leads, the style stays as it is.
    let out = export_synthetic(
        "77-body-spacing-split",
        BOOKMAN_BODY_TEXT,
        &format!(
            "{}{}",
            spaced_paragraph(Some("BodyText"), long, LOOSE),
            spaced_paragraph(Some("BodyText"), long, TIGHT)
        ),
    );
    assert_eq!(style_ppr(&out, "BodyText"), "");
}

// ------------------------------ pictures a header or footer cannot show

fn pictures_reported(doc: &Document) -> Vec<String> {
    let inspection = docrs::markdown::inspect(doc, &MarkdownOptions::default()).unwrap();
    inspection["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["code"] == "picture_larger_than_page")
        .map(|p| p["message"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn an_oversized_footer_picture_is_reported_and_kept() {
    let template = fixture_template("oversized-footer-picture.docx");
    let message = "The footer has a picture 37.3 cm wide and 16.7 cm tall, wider than the page and taller than half of it (21.0 by 29.7 cm). Word hides what does not fit, but it can break PDFs and previews. Resize or remove it in the template.";
    assert_eq!(pictures_reported(&template), [message]);
    let inspection = docrs::markdown::inspect(&template, &MarkdownOptions::default()).unwrap();
    assert_eq!(
        inspection["section"]["headers_footers"]["footer"]["default"]["images"],
        2
    );
    let out = export_with(
        "78-oversized-footer-picture",
        "# Advice\n\nBody.\n",
        Some(template),
        &MarkdownOptions::default(),
    );
    let warnings: Vec<_> = out
        .report
        .warnings
        .iter()
        .filter(|w| w.code == "picture_larger_than_page")
        .collect();
    assert_eq!(warnings.len(), 1, "{:?}", out.report.warnings);
    assert_eq!(warnings[0].message, message);
    assert_eq!(warnings[0].line, None);
    // The footer is kept as it is, picture included.
    let footer = out.part("word/footer1.xml");
    assert_eq!(count(&footer, "<w:drawing>"), 2);
    assert!(
        footer.contains("cx=\"13442950\" cy=\"6005195\""),
        "{footer}"
    );
}

#[test]
fn pictures_wider_than_the_page_or_taller_than_half_of_it_are_reported() {
    // Letter: 21.6 by 27.9 cm.
    let page_width = 12240 * 635;
    let half_height = 15840 * 635 / 2;
    let template = |footer: &str, cover_height: Option<i64>| {
        synthetic(Synthetic {
            body: "<w:p><w:r><w:t>Sample</w:t></w:r></w:p>",
            footer: Some(footer),
            cover_height,
            ..Synthetic::default()
        })
    };
    let fits = picture(page_width, half_height, false);
    assert_eq!(
        pictures_reported(&template(&fits, None)),
        Vec::<String>::new()
    );
    let wide = picture(page_width + 360_000, 1_000_000, true);
    assert_eq!(
        pictures_reported(&template(&wide, None)),
        [
            "The footer has a picture 22.6 cm wide and 2.8 cm tall, wider than the page (21.6 by 27.9 cm). Word hides what does not fit, but it can break PDFs and previews. Resize or remove it in the template."
        ]
    );
    let tall = picture(3_600_000, half_height + 360_000, false);
    let reported = pictures_reported(&template(&tall, None));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(
        reported[0].starts_with(
            "The footer has a picture 10.0 cm wide and 15.0 cm tall, taller than half the page (21.6 by 27.9 cm)."
        ),
        "{reported:?}"
    );
    // An anchored picture is placed by position and may cover the page, as
    // a letterhead background does: reported only when it does not fit.
    let background = picture(page_width, 2 * half_height, true);
    assert_eq!(
        pictures_reported(&template(&background, None)),
        Vec::<String>::new()
    );
    let overflowing = picture(page_width, 2 * half_height + 360_000, true);
    let reported = pictures_reported(&template(&overflowing, None));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(
        reported[0].starts_with(
            "The footer has a picture 21.6 cm wide and 28.9 cm tall, taller than the page (21.6 by 27.9 cm)."
        ),
        "{reported:?}"
    );
    // Word's watermarks are VML shapes; they are not measured.
    let watermark = "<w:p><w:r><w:pict><v:shape xmlns:v=\"urn:schemas-microsoft-com:vml\" \
        id=\"PowerPlusWaterMarkObject1\" style=\"position:absolute;width:527.85pt;height:131.95pt;rotation:315\"/>\
        </w:pict></w:r></w:p>";
    assert_eq!(
        pictures_reported(&template(watermark, None)),
        Vec::<String>::new()
    );
    // A first-page header holds the cover of templates that have one: its
    // picture is reported only when it does not fit on the page.
    let logo = picture(900_000, 432_000, false);
    assert_eq!(
        pictures_reported(&template(&logo, Some(9_000_000))),
        Vec::<String>::new()
    );
    let reported = pictures_reported(&template(&logo, Some(11_000_000)));
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(
        reported[0].starts_with(
            "The first-page header has a picture 13.9 cm wide and 30.6 cm tall, taller than the page (21.6 by 27.9 cm)."
        ),
        "{reported:?}"
    );
}

// ------------------------------ validator errors and the unused First Paragraph

/// The repro content of the 2026-10-27 re-scoring, its `input.html`.
const SECTIONS_HTML: &str = "<h1>Heading one</h1>\
    <p>First paragraph after a heading, long enough to read as body text.</p>\
    <p>Second paragraph, an ordinary body paragraph long enough to read as body text.</p>\
    <table><tbody><tr><th><p>Asset</p></th><th><p>Value</p></th></tr>\
    <tr><td><p>Home</p></td><td><p>$1,200,000</p></td></tr></tbody></table>\
    <h2>Heading two</h2>\
    <p>Another first paragraph after a heading, long enough to read as body text.</p>";

fn export_html_from(name: &str, doc: Document, html: &str) -> Export {
    export_html_with(name, doc, html, &MarkdownOptions::default())
}

/// Exports the repro content with `fixture`, whose First Paragraph the
/// template defines but writes no prose in: the paragraph after each
/// heading takes Body Text, the template's First Paragraph is left as it
/// is, and `inspect` agrees. Returns the inspection.
fn assert_first_paragraph_passed_over(name: &str, fixture: &str) -> serde_json::Value {
    let template = || fixture_template(fixture);
    let out = export_html_from(name, template(), SECTIONS_HTML);
    assert_eq!(
        out.report.styles["first_paragraph"].style_id.as_deref(),
        Some("BodyText")
    );
    let body = out.body();
    assert_eq!(count(&body, "<w:pStyle w:val=\"BodyText\"/>"), 3);
    assert!(!body.contains("FirstParagraph"), "{body}");
    assert_eq!(
        style_of(&out, "FirstParagraph"),
        style_xml(&part_of(&template(), "word/styles.xml"), "FirstParagraph")
    );
    let inspection = docrs::markdown::inspect(&template(), &MarkdownOptions::default()).unwrap();
    assert_eq!(inspection["roles"]["first_paragraph"]["name"], "Body Text");
    inspection
}

fn part_of(doc: &Document, name: &str) -> String {
    String::from_utf8(docrs::xml::serialize(doc.package().xml(name).unwrap())).unwrap()
}

/// Each tag's first occurrence, in the order given.
fn assert_in_order(xml: &str, tags: &[&str]) {
    let positions: Vec<usize> = tags
        .iter()
        .map(|tag| {
            xml.find(tag)
                .unwrap_or_else(|| panic!("{tag} missing from {xml}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{tags:?} out of order in {xml}"
    );
}

#[test]
fn a_lifted_colour_keeps_its_value_beside_the_theme_colour() {
    // Every heading is coloured by hand with a value and a theme colour;
    // the style takes both (w:val is required).
    let out = export_html_from(
        "79-heading-theme-colour",
        fixture_template("heading-theme-colour.docx"),
        SECTIONS_HTML,
    );
    for id in ["Heading1", "Heading2"] {
        let rpr = style_rpr(&out, id);
        let color = rpr[rpr.find("<w:color").unwrap()..]
            .split("/>")
            .next()
            .unwrap();
        assert!(
            color.contains("w:val=\"000000\"") && color.contains("w:themeColor=\"text1\""),
            "{id}: {rpr}"
        );
        assert!(!rpr.contains("2F5496"), "{id}: {rpr}");
    }
    // A colour set by value alone still drops the style's theme colour.
    let heading = "<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/>\
        <w:basedOn w:val=\"Normal\"/><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr>\
        <w:rPr><w:b/><w:color w:val=\"000000\" w:themeColor=\"text1\"/></w:rPr></w:style>";
    let out = export_synthetic(
        "80-heading-colour-by-value",
        heading,
        &styled_paragraph(
            "Heading1",
            "Sample heading",
            "<w:rPr><w:color w:val=\"C00000\"/></w:rPr>",
        ),
    );
    let rpr = style_rpr(&out, "Heading1");
    assert!(rpr.contains("<w:color w:val=\"C00000\"/>"), "{rpr}");
}

#[test]
fn lifted_properties_are_written_in_schema_order() {
    // The sample table's header cells put w:shd before w:tcBorders, as some
    // editors save it; the table style is written in schema order.
    let out = export_html_from(
        "81-table-cell-shading-order",
        fixture_template("table-cell-shading-order.docx"),
        SECTIONS_HTML,
    );
    let table = style_of(&out, "Table");
    let first_row = &table[table.find("w:type=\"firstRow\"").unwrap()..];
    assert_in_order(first_row, &["<w:tcPr>", "<w:tcBorders>", "<w:shd "]);
    assert!(first_row.contains("w:fill=\"1F3A5F\""), "{first_row}");

    // Everything copied from a template part is ordered the same: a heading
    // made by hand with its paragraph and run properties shuffled, a table
    // whose cells list margins, shading and borders backwards.
    let heading = "<w:p><w:pPr><w:outlineLvl w:val=\"0\"/><w:jc w:val=\"center\"/><w:keepNext/>\
        <w:spacing w:before=\"240\"/></w:pPr><w:r><w:rPr><w:sz w:val=\"32\"/><w:b/>\
        <w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/></w:rPr><w:t>Hand-made heading</w:t></w:r></w:p>";
    let shading = "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"1F3A5F\"/>";
    let header = format!(
        "<w:tcMar><w:left w:w=\"80\" w:type=\"dxa\"/><w:top w:w=\"40\" w:type=\"dxa\"/></w:tcMar>{shading}\
         <w:tcBorders><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"1F3A5F\"/>\
         <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"1F3A5F\"/></w:tcBorders>"
    );
    let plain = header.replace(shading, "");
    let table = sample_table(
        None,
        &[
            row(&["Asset", "Value"], &header, ""),
            row(&["Home", "1"], &plain, ""),
            row(&["Car", "2"], &plain, ""),
        ],
    );
    let template = synthetic(Synthetic {
        body: &format!("{heading}{table}"),
        ..Synthetic::default()
    });
    let out = export_with(
        "82-shuffled-template-properties",
        "# Heading\n\nText.\n\n| A | B |\n| - | - |\n| 1 | 2 |\n",
        Some(template),
        &MarkdownOptions::default(),
    );
    let heading = style_of(&out, "Heading1");
    assert_in_order(
        &heading,
        &[
            "<w:pPr>",
            "<w:keepNext/>",
            "<w:spacing ",
            "<w:jc ",
            "<w:outlineLvl ",
            "<w:rPr>",
            "<w:rFonts ",
            "<w:b/>",
            "<w:sz ",
        ],
    );
    // The table-level order comes from how the lifted table is assembled;
    // the margin sides inside it from the sort.
    let table = style_of(&out, "Table");
    assert_in_order(
        &table,
        &[
            "<w:tblPr>",
            "<w:tblBorders>",
            "<w:tblCellMar>",
            "<w:top w:w=\"40\"",
            "<w:left w:w=\"80\"",
        ],
    );
    let first_row = &table[table.find("w:type=\"firstRow\"").unwrap()..];
    assert_in_order(
        first_row,
        &[
            "<w:tcPr>",
            "<w:tcBorders>",
            "<w:top ",
            "<w:bottom ",
            "<w:shd ",
            "<w:tcMar>",
            "<w:top w:w=\"40\"",
            "<w:left ",
        ],
    );
}

#[test]
fn an_unused_first_paragraph_style_is_passed_over_for_the_body() {
    // First Paragraph is defined (based on Normal, no look of its own) but
    // no paragraph uses it; the prose is Body Text.
    let inspection = assert_first_paragraph_passed_over(
        "83-first-paragraph-unused",
        "first-paragraph-unused.docx",
    );
    assert_eq!(inspection["roles"]["body"]["name"], "Body Text");

    // The control: a First Paragraph the template uses, with a look of its
    // own (italic, 12 pt before), is kept as the template defines it.
    let template = || fixture_template("first-paragraph-used.docx");
    let out = export_html_from("84-first-paragraph-used", template(), SECTIONS_HTML);
    assert_eq!(out.report.styles["first_paragraph"].name, "First Paragraph");
    let body = out.body();
    assert_eq!(count(&body, "<w:pStyle w:val=\"FirstParagraph\"/>"), 2);
    assert_eq!(count(&body, "<w:pStyle w:val=\"BodyText\"/>"), 1);
    let style = style_of(&out, "FirstParagraph");
    assert!(
        style.contains("<w:i/>") && style.contains("w:before=\"240\""),
        "{style}"
    );
    let inspection = docrs::markdown::inspect(&template(), &MarkdownOptions::default()).unwrap();
    assert_eq!(
        inspection["roles"]["first_paragraph"]["name"],
        "First Paragraph"
    );

    // When the prose is the default style, so is the first paragraph.
    let long = "A Normal paragraph, long enough to read as prose.";
    let export = |name: &str, body: &str| export_synthetic(name, &first_paragraph_style(""), body);
    let out = export(
        "85-first-paragraph-unused-normal-prose",
        &format!(
            "{}{}",
            spaced_paragraph(None, long, ""),
            spaced_paragraph(None, long, "")
        ),
    );
    assert_eq!(out.report.styles["first_paragraph"].name, "Normal");
    assert_eq!(out.report.styles["first_paragraph"].style_id, None);
    assert!(!out.body().contains("<w:pStyle w:val=\"FirstParagraph\"/>"));
    // A template with no prose at all says nothing: First Paragraph is used.
    let out = export("86-first-paragraph-no-prose", "");
    assert_eq!(out.report.styles["first_paragraph"].name, "First Paragraph");
    assert_eq!(
        count(&out.body(), "<w:pStyle w:val=\"FirstParagraph\"/>"),
        2
    );
}

#[test]
fn a_body_look_needs_prose_to_go_by() {
    // Body Text is defined (15 pt) and the template's only text is two
    // short Normal lines in Times: nothing is lifted, Body Text is the body.
    let template = || {
        Document::open_path_with(
            fixture("body-text-no-prose.docx"),
            &Limits::default(),
            docrs::Tolerance::word(),
        )
        .unwrap()
        .0
    };
    let out = export_html_from("87-body-text-no-prose", template(), SECTIONS_HTML);
    for role in ["body", "first_paragraph", "compact"] {
        assert_eq!(out.report.styles[role].name, "Body Text", "{role}");
        assert!(!out.report.styles[role].created, "{role}");
    }
    let styles = out.part("word/styles.xml");
    assert!(!styles.contains("Template Body Text"), "{styles}");
    assert_eq!(
        style_of(&out, "BodyText"),
        style_xml(&part_of(&template(), "word/styles.xml"), "BodyText")
    );
    assert_eq!(count(&out.body(), "<w:pStyle w:val=\"BodyText\"/>"), 3);
    let inspection = docrs::markdown::inspect(&template(), &MarkdownOptions::default()).unwrap();
    assert_eq!(inspection["roles"]["body"]["name"], "Body Text");
    assert!(
        inspection["lifted"]["body"].is_null(),
        "{}",
        inspection["lifted"]
    );
    assert!(
        inspection["lifted"]["spacing"].is_null(),
        "{}",
        inspection["lifted"]
    );

    // Letterhead lines (under 40 characters) in a look of their own do not
    // lift it over the template's Body Text; the same look on prose does.
    let brown = "<w:rPr><w:rFonts w:ascii=\"Trebuchet MS\" w:hAnsi=\"Trebuchet MS\"/>\
        <w:color w:val=\"6B4F2A\"/><w:sz w:val=\"18\"/></w:rPr>";
    let arial_body_text = "<w:style w:type=\"paragraph\" w:styleId=\"BodyText\"><w:name w:val=\"Body Text\"/>\
        <w:basedOn w:val=\"Normal\"/><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"20\"/></w:rPr></w:style>";
    let export = |name: &str, styles: &str, text: &str, rpr: &str| {
        let lines: String = (0..3)
            .map(|_| format!("<w:p><w:r>{rpr}<w:t>{text}</w:t></w:r></w:p>"))
            .collect();
        export_synthetic(name, styles, &lines)
    };
    let out = export(
        "88-letterhead-lines",
        arial_body_text,
        "Acme Wealth, 1 Main Street",
        brown,
    );
    assert_eq!(out.report.styles["body"].name, "Body Text");
    assert!(!out.report.styles["body"].created);
    assert!(!out.part("word/styles.xml").contains("Trebuchet"));
    let out = export(
        "89-prose-in-a-look-of-its-own",
        arial_body_text,
        "A paragraph of advice long enough to read as prose.",
        brown,
    );
    assert_eq!(out.report.styles["body"].name, "Template Body Text");
    assert!(style_rpr(&out, "TemplateBodyText").contains("Trebuchet"));
    // A spacer Body Text is passed over for Normal only when Normal
    // carries prose.
    let spacer = "<w:style w:type=\"paragraph\" w:styleId=\"BodyText\"><w:name w:val=\"Body Text\"/>\
        <w:basedOn w:val=\"Normal\"/><w:rPr><w:sz w:val=\"12\"/></w:rPr></w:style>";
    let out = export(
        "90-spacer-beside-short-lines",
        spacer,
        "Acme Wealth, 1 Main Street",
        "",
    );
    assert_eq!(out.report.styles["body"].name, "Body Text");
    let out = export(
        "91-spacer-beside-prose",
        spacer,
        "A Normal paragraph, long enough to read as prose.",
        "",
    );
    assert_eq!(out.report.styles["body"].name, "Normal");
}

#[test]
fn a_first_paragraph_holding_only_a_sample_label_is_not_used() {
    // The template's only First Paragraph text is the line "First
    // Paragraph.", one line per style as Pandoc's sample reference has;
    // the prose is Body Text.
    assert_first_paragraph_passed_over(
        "92-first-paragraph-sample-label",
        "first-paragraph-sample-label.docx",
    );

    // The same label beside no prose at all says nothing: First Paragraph
    // stays in use.
    let out = export_synthetic(
        "93-sample-labels-only",
        &format!("{BOOKMAN_BODY_TEXT}{}", first_paragraph_style("")),
        &format!(
            "{}{}",
            styled_paragraph("FirstParagraph", "First Paragraph.", ""),
            styled_paragraph("BodyText", "Body Text.", "")
        ),
    );
    assert_eq!(out.report.styles["first_paragraph"].name, "First Paragraph");
    assert_eq!(
        count(&out.body(), "<w:pStyle w:val=\"FirstParagraph\"/>"),
        2
    );
}

/// The repro content for lists and deeper headings: a heading, a paragraph,
/// two bullets, then headings two to four with a paragraph each.
const LISTS_HTML: &str = "<h1>Heading one</h1>\
    <p>First paragraph after a heading, long enough to read as body text.</p>\
    <ul><li><p>A bullet point in the export.</p></li><li><p>Another bullet point in the export.</p></li></ul>\
    <h2>Heading two</h2><p>Body text under heading two, long enough to read as body text.</p>\
    <h3>Heading three</h3><p>Body text under heading three, long enough to read as body text.</p>\
    <h4>Heading four</h4><p>Body text under heading four, long enough to read as body text.</p>";

#[test]
fn a_body_text_holding_only_steps_is_not_the_body() {
    // Body Text holds two short lines and four numbered steps of a closing
    // section; the prose, sixteen paragraphs, is Normal (Arial 11 pt).
    let template = || fixture_template("body-text-steps-only.docx");
    let out = export_html_from("94-body-text-steps-only", template(), LISTS_HTML);
    for role in ["body", "first_paragraph", "compact"] {
        assert_eq!(out.report.styles[role].name, "Normal", "{role}");
        assert_eq!(out.report.styles[role].style_id, None, "{role}");
    }
    assert!(!out.body().contains("BodyText"), "{}", out.body());
    assert_eq!(
        style_of(&out, "BodyText"),
        style_xml(&part_of(&template(), "word/styles.xml"), "BodyText")
    );
    let inspection = docrs::markdown::inspect(&template(), &MarkdownOptions::default()).unwrap();
    assert_eq!(inspection["roles"]["body"]["name"], "Normal");

    // A Body Text holding prose beside its steps is the body.
    let prose = "A Body Text paragraph, long enough to read as the prose of the advice.";
    let out = export_synthetic(
        "95-body-text-prose-and-steps",
        BOOKMAN_BODY_TEXT,
        &format!(
            "{}{}{}{}",
            spaced_paragraph(None, prose, ""),
            styled_paragraph("BodyText", prose, ""),
            styled_paragraph("BodyText", "Review the following:", ""),
            styled_paragraph("BodyText", "Do you wish to proceed?", "")
        ),
    );
    assert_eq!(out.report.styles["body"].name, "Body Text");
}

const BULLET_AND_DECIMAL_NUMBERING: &str = "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>\
    <w:lvlText w:val=\"\u{2022}\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>\
    <w:abstractNum w:abstractNumId=\"1\"><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/>\
    <w:lvlText w:val=\"%1.\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>\
    <w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num>";

/// A `List Bullet` (`num_id` 1) or `List Number` (2) style bound to its
/// numbering, in the theme's minor font.
fn list_style(id: &str, name: &str, num_id: u8) -> String {
    format!(
        "<w:style w:type=\"paragraph\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"Normal\"/>\
         <w:pPr><w:numPr><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>\
         <w:rPr><w:rFonts w:asciiTheme=\"minorHAnsi\" w:hAnsiTheme=\"minorHAnsi\"/></w:rPr></w:style>"
    )
}

fn list_item(style: &str, num_id: u8, text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>\
         <w:r><w:t>{text}</w:t></w:r></w:p>"
    )
}

#[test]
fn an_unused_list_style_takes_the_sample_lists_font_and_size() {
    // List Bullet binds the bullet numbering but no paragraph uses it; the
    // template's bullets are List Paragraph (Arial from Normal, 11 pt from
    // the defaults) in the theme's minor font by the style.
    let template = || fixture_template("theme-default-list-bullet-and-heading-4.docx");
    let out = export_html_from("96-theme-default-list-bullet", template(), LISTS_HTML);
    assert_eq!(out.report.styles["list_bullet"].name, "List Bullet");
    assert_eq!(
        style_rpr(&out, "ListBullet"),
        "<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"22\"/></w:rPr>"
    );
    assert_eq!(count(&out.body(), "<w:pStyle w:val=\"ListBullet\"/>"), 2);

    // List Number the same way, from numbered paragraphs (Georgia 12 pt by
    // List Paragraph); a List Bullet the template writes in keeps its font.
    let list_paragraph = "<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\"><w:name w:val=\"List Paragraph\"/>\
        <w:basedOn w:val=\"Normal\"/><w:pPr><w:ind w:left=\"720\"/></w:pPr>\
        <w:rPr><w:rFonts w:ascii=\"Georgia\" w:hAnsi=\"Georgia\"/><w:sz w:val=\"24\"/></w:rPr></w:style>";
    let template = synthetic(Synthetic {
        styles: &format!(
            "{list_paragraph}{}{}",
            list_style("ListBullet", "List Bullet", 1),
            list_style("ListNumber", "List Number", 2)
        ),
        body: &format!(
            "{}{}{}",
            list_item(
                "ListParagraph",
                2,
                "The first numbered step in the template."
            ),
            list_item(
                "ListParagraph",
                2,
                "The second numbered step in the template."
            ),
            list_item(
                "ListBullet",
                1,
                "A bullet in the template's own List Bullet."
            )
        ),
        numbering: BULLET_AND_DECIMAL_NUMBERING,
        ..Synthetic::default()
    });
    let out = export_with(
        "97-list-number-from-samples",
        "- A bullet\n- Another bullet\n\n1. A step\n2. Another step\n",
        Some(template),
        &MarkdownOptions::default(),
    );
    assert_eq!(
        style_rpr(&out, "ListNumber"),
        "<w:rPr><w:rFonts w:ascii=\"Georgia\" w:hAnsi=\"Georgia\"/><w:sz w:val=\"24\"/></w:rPr>"
    );
    assert_eq!(
        style_rpr(&out, "ListBullet"),
        "<w:rPr><w:rFonts w:asciiTheme=\"minorHAnsi\" w:hAnsiTheme=\"minorHAnsi\"/></w:rPr>"
    );
}

#[test]
fn an_unused_heading_on_the_theme_font_follows_the_levels_in_use() {
    // Headings 1 to 3 are used and set Arial by name; Heading 4 is unused
    // and still on the theme's major font. It follows Heading 3's font and
    // keeps the rest of its look.
    let template = || fixture_template("theme-default-list-bullet-and-heading-4.docx");
    let out = export_html_from("98-theme-default-heading-4", template(), LISTS_HTML);
    let rpr = style_rpr(&out, "Heading4");
    assert!(
        rpr.contains("w:ascii=\"Arial\"")
            && rpr.contains("w:hAnsi=\"Arial\"")
            && !rpr.contains("Theme=")
            && rpr.contains("<w:i/>")
            && rpr.contains("<w:color w:val=\"2F5496\"/>"),
        "{rpr}"
    );
    let template_styles = part_of(&template(), "word/styles.xml");
    for id in ["Heading1", "Heading2", "Heading3"] {
        assert_eq!(style_of(&out, id), style_xml(&template_styles, id), "{id}");
    }

    // The nearest level in use decides: Heading 2 (theme font, unused)
    // follows Heading 1's Arial; Heading 3 names Georgia and is left alone;
    // Heading 4 (theme font, used) is left alone, and so is Heading 5 behind
    // it, since the nearest level in use has no named font.
    let heading = |level: u8, rfonts: &str| {
        format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\"><w:name w:val=\"heading {level}\"/>\
             <w:basedOn w:val=\"Normal\"/><w:pPr><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr>{rfonts}<w:b/></w:rPr></w:style>",
            level - 1
        )
    };
    let theme = "<w:rFonts w:asciiTheme=\"majorHAnsi\" w:hAnsiTheme=\"majorHAnsi\"/>";
    let template = synthetic(Synthetic {
        styles: &format!(
            "{}{}{}{}{}",
            heading(1, "<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/>"),
            heading(2, theme),
            heading(3, "<w:rFonts w:ascii=\"Georgia\" w:hAnsi=\"Georgia\"/>"),
            heading(4, theme),
            heading(5, theme)
        ),
        body: &format!(
            "{}{}",
            styled_paragraph("Heading1", "Sample heading one", ""),
            styled_paragraph("Heading4", "Sample heading four", "")
        ),
        ..Synthetic::default()
    });
    let out = export_with(
        "99-heading-levels-follow-the-nearest-in-use",
        "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\nText.\n",
        Some(template),
        &MarkdownOptions::default(),
    );
    assert_eq!(
        style_rpr(&out, "Heading2"),
        "<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:b/></w:rPr>"
    );
    for id in ["Heading3", "Heading4", "Heading5"] {
        let expected = if id == "Heading3" {
            "<w:rPr><w:rFonts w:ascii=\"Georgia\" w:hAnsi=\"Georgia\"/><w:b/></w:rPr>".to_string()
        } else {
            format!("<w:rPr>{theme}<w:b/></w:rPr>")
        };
        assert_eq!(style_rpr(&out, id), expected, "{id}");
    }
}

/// The headers and footers the export's section shows: whether it has a
/// distinct first page, and each story's text by (`header`/`footer`, type).
fn section_stories(out: &Export) -> (bool, BTreeMap<(String, String), String>) {
    let body = out.part("word/document.xml");
    let sect_pr = &body[body.rfind("<w:sectPr").unwrap()..];
    let rels = out.part("word/_rels/document.xml.rels");
    let attr = |element: &str, name: &str| {
        let key = format!("{name}=\"");
        let at = element.find(&key).unwrap() + key.len();
        element[at..at + element[at..].find('"').unwrap()].to_string()
    };
    let mut stories = BTreeMap::new();
    for element in sect_pr.split('<') {
        let Some(story) = ["header", "footer"]
            .into_iter()
            .find(|story| element.starts_with(&format!("w:{story}Reference")))
        else {
            continue;
        };
        let r_id = attr(element, "r:id");
        let rel = rels
            .split("<Relationship ")
            .find(|rel| rel.contains(&format!("Id=\"{r_id}\"")))
            .unwrap();
        let part = out.part(&format!("word/{}", attr(rel, "Target")));
        let text: String = part
            .match_indices("<w:t")
            .filter(|(at, _)| matches!(part[at + 4..].chars().next(), Some('>' | ' ')))
            .map(|(at, _)| {
                let rest = &part[at..];
                let open = rest.find('>').unwrap() + 1;
                rest[open..open + rest[open..].find('<').unwrap()].to_string()
            })
            .collect();
        stories.insert((story.to_string(), attr(element, "w:type")), text);
    }
    (sect_pr.contains("<w:titlePg/>"), stories)
}

fn stories_of(entries: &[(&str, &str, &str)]) -> BTreeMap<(String, String), String> {
    entries
        .iter()
        .map(|(story, kind, text)| ((story.to_string(), kind.to_string()), text.to_string()))
        .collect()
}

#[test]
fn page_one_shows_what_the_templates_page_one_shows() {
    // Two-section templates: a cover or letter section whose page 1 carries
    // the logo, then a section for the body pages. The export's single
    // section shows page 1's header and footer on page 1 and the body
    // pages' after, with a distinct first page when they differ.
    let export = |name: &str, fixture: &str| {
        section_stories(&export_html_from(
            name,
            fixture_template(fixture),
            SECTIONS_HTML,
        ))
    };
    // Section 1 has a distinct first page with the logo and a page 1
    // footer; the last section turns the first page off and inherits.
    let (title_page, stories) = export(
        "100-first-page-header-in-first-section",
        "first-page-header-in-first-section.docx",
    );
    assert!(title_page);
    assert_eq!(
        stories,
        stories_of(&[
            ("header", "first", "PAGE 1 LOGO"),
            ("footer", "first", "Page 1 footer"),
            ("footer", "default", "Body footer"),
        ])
    );
    // No distinct first page anywhere; each section has its own default
    // header and footer.
    let (title_page, stories) = export(
        "101-default-header-differs-by-section",
        "default-header-differs-by-section.docx",
    );
    assert!(title_page);
    assert_eq!(
        stories,
        stories_of(&[
            ("header", "first", "PAGE 1 LOGO"),
            ("footer", "first", "Page 1 footer"),
            ("header", "default", "Body header"),
            ("footer", "default", "Body footer"),
        ])
    );
    // Section 1 shows a banner in its default header; the last section
    // turns a distinct first page on with only a first-page footer, and
    // inherits the rest: page 1 and the body pages are the same.
    let (title_page, stories) = export(
        "102-title-page-only-in-last-section",
        "title-page-only-in-last-section.docx",
    );
    assert!(!title_page);
    assert_eq!(
        stories,
        stories_of(&[
            ("header", "default", "PAGE 1 BANNER"),
            ("footer", "default", "Body footer"),
        ])
    );
    let inspection = docrs::markdown::inspect(
        &fixture_template("default-header-differs-by-section.docx"),
        &MarkdownOptions::default(),
    )
    .unwrap();
    assert_eq!(inspection["section"]["title_page"], true);
    let shown = &inspection["section"]["headers_footers"];
    assert_eq!(shown["header"]["first"]["inherited"], true);
    assert_eq!(shown["header"]["default"]["inherited"], false);
    assert!(
        inspection["problems"]
            .to_string()
            .contains("multiple_sections"),
        "{inspection}"
    );
}

/// The table-text repro of 2026-10-27: a heading, a paragraph, a two-column
/// table with a header row, a paragraph.
const TABLE_TEXT_HTML: &str = "<h1>Heading one</h1>\
    <p>Body text, long enough to read as the prose of a real advice document.</p>\
    <table><tbody><tr><th><p>Asset</p></th><th><p>Balance</p></th></tr>\
    <tr><td><p>Superannuation</p></td><td><p>$250,000</p></td></tr>\
    <tr><td><p>Cash</p></td><td><p>$40,000</p></td></tr></tbody></table>\
    <p>More body text after the table, long enough to read as prose.</p>";

/// The first table's header row and the whole body.
fn header_row(out: &Export) -> (String, String) {
    let body = out.body();
    let header = body[body.find("<w:tr>").unwrap()..body.find("</w:tr>").unwrap()].to_string();
    (header, body)
}

/// What a conversion with default options takes from `document`.
fn inspect_default(document: &Document) -> serde_json::Value {
    docrs::markdown::inspect(document, &MarkdownOptions::default()).unwrap()
}

#[test]
fn table_cells_take_the_templates_own_table_text_styles() {
    // The sample tables put header cells in "Table Header" (bold 10 pt) and
    // body cells in "Table Normal Body" (8 pt); 0.3.10 put every cell in a
    // Table Contents built on the body.
    let out = export_html_from(
        "103-table-cell-styles",
        fixture_template("table-cell-styles.docx"),
        TABLE_TEXT_HTML,
    );
    assert_eq!(out.report.styles["table_header"].name, "Table Header");
    assert_eq!(
        out.report.styles["table_contents"].name,
        "Table Normal Body"
    );
    assert_eq!(out.report.styles["table"].name, "Table Grid");
    assert!(
        out.report.created_styles.is_empty(),
        "{:?}",
        out.report.created_styles
    );
    let (header, body) = header_row(&out);
    assert_eq!(
        count(&header, "<w:pStyle w:val=\"TableHeader\"/>"),
        2,
        "{header}"
    );
    assert_eq!(count(&body, "<w:pStyle w:val=\"TableNormalBody\"/>"), 4);
    assert!(!body.contains("TableContents"));
    // The style map sends the role elsewhere like any other.
    let mut options = MarkdownOptions::default();
    options
        .style_map
        .insert(Role::TableHeader, "Table Normal Body".into());
    let out = export_html_with(
        "103-table-cell-styles-mapped",
        fixture_template("table-cell-styles.docx"),
        TABLE_TEXT_HTML,
        &options,
    );
    assert_eq!(out.report.styles["table_header"].name, "Table Normal Body");
    assert_eq!(
        count(&header_row(&out).0, "<w:pStyle w:val=\"TableNormalBody\"/>"),
        2
    );
    let roles = role_styles(
        &fixture_template("table-cell-styles.docx"),
        &Default::default(),
    )
    .unwrap();
    let header = roles
        .iter()
        .find(|(role, _)| *role == Role::TableHeader)
        .and_then(|(_, style)| style.clone())
        .unwrap();
    assert_eq!(header.name, "Table Header");
    let inspection = inspect_default(&fixture_template("table-cell-styles.docx"));
    assert_eq!(inspection["roles"]["table_header"]["name"], "Table Header");
    assert_eq!(
        inspection["lifted"]["table"]["cell_styles"]["header"],
        "TableHeader"
    );
    assert_eq!(
        inspection["lifted"]["table"]["cell_styles"]["body"],
        "TableNormalBody"
    );
}

#[test]
fn hand_formatted_table_text_goes_to_the_cell_styles() {
    // The same look applied by hand in Normal: header runs bold, body runs
    // 8 pt. 0.3.10 lifted the looks into a table style, which only Word
    // applies, and which gave the header the body's size as well.
    let out = export_html_from(
        "104-table-cell-hand-formatting",
        fixture_template("table-cell-hand-formatting.docx"),
        TABLE_TEXT_HTML,
    );
    assert_eq!(
        out.report.created_styles,
        ["Table Contents", "Table Heading"]
    );
    assert_eq!(out.report.styles["table"].name, "Table Grid");
    let contents = style_of(&out, "TableContents");
    assert!(
        contents.contains("<w:basedOn w:val=\"Normal\"/>")
            && contents.contains("<w:sz w:val=\"16\"/>"),
        "{contents}"
    );
    let heading = style_of(&out, "TableHeading");
    assert!(
        heading.contains("<w:basedOn w:val=\"Normal\"/>")
            && heading.contains("<w:b/>")
            && !heading.contains("<w:sz "),
        "{heading}"
    );
    let (header, body) = header_row(&out);
    assert_eq!(
        count(&header, "<w:pStyle w:val=\"TableHeading\"/>"),
        2,
        "{header}"
    );
    assert_eq!(count(&body, "<w:pStyle w:val=\"TableContents\"/>"), 4);
    // The look sits in the styles, not on the runs.
    assert!(!body.contains("<w:sz w:val=\"16\"/>") && !header.contains("<w:b/>"));
    let inspection = inspect_default(&fixture_template("table-cell-hand-formatting.docx"));
    let table = &inspection["lifted"]["table"];
    assert_eq!(table["cell_text"]["body"]["size_pt"], 8.0);
    assert_eq!(table["cell_text"]["header"]["bold"], true);
    assert_eq!(table["direct_formatting_lifted"], false);
    assert_eq!(inspection["roles"]["table_header"]["created"], true);

    // Tables whose text carries no look of its own keep today's Table
    // Contents built on the body, and the header cells share it.
    let plain = [row(&["A", "B"], "", ""), row(&["1", "2"], "", "")];
    let template = synthetic(Synthetic {
        styles: TABLE_GRID,
        body: &sample_table(Some("TableGrid"), &plain),
        ..Synthetic::default()
    });
    let out = export_with(
        "105-plain-table-text",
        TABLE_MARKDOWN,
        Some(template),
        &MarkdownOptions::default(),
    );
    assert_eq!(out.report.created_styles, ["Table Contents"]);
    let header = &out.report.styles["table_header"];
    assert_eq!(
        (header.name.as_str(), header.created),
        ("Table Contents", false)
    );
    assert_eq!(count(&out.body(), "<w:pStyle w:val=\"TableContents\"/>"), 6);
}
