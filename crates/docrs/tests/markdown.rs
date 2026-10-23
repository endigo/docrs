//! Markdown → DOCX, one feature per test. Every conversion is exported
//! (patch-saved), reopened through the guarded reader and inspected from
//! the reopened package. With `DOCRS_MARKDOWN_EXPORT=<dir>` each
//! exported file is also written there (`<test>.docx`) for inspection in
//! Word, LibreOffice or pandoc (`tools/markdown_check.sh`).

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

    fn warning_codes(&self) -> Vec<&str> {
        self.report.warnings.iter().map(|w| w.code).collect()
    }
}

/// Convert into `reference` (or the bundled blank document), export, reopen.
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

// ------------------------------------------------------------- blocks

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
    // Without template lists, the converter's own definitions are used.
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
    // Three ordered lists, three restarted instances.
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
    // Continuations line up with the list level's text indent.
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
    // Full A4 text width (11906 - 2 * 1247 twips) in twips, not a
    // percentage (Word for the web mis-lays those), columns as computed,
    // cells in the compact cell style.
    assert_eq!(count(&body, "<w:tblW w:type=\"dxa\" w:w=\"9412\"/>"), 2);
    assert_eq!(count(&body, "<w:tblLayout w:type=\"fixed\"/>"), 2);
    assert!(body.contains("<w:pStyle w:val=\"TableContents\"/>"));
    let styles = out.part("word/styles.xml");
    assert!(styles.contains("<w:name w:val=\"Table Contents\"/>"));
    assert!(body.contains("<w:jc w:val=\"center\"/>"));
    // Caption before the table, and a separator paragraph keeps the two
    // tables from merging.
    let paragraphs = out.paragraphs();
    assert_eq!(paragraphs[0], ("Table Caption".into(), "Results.".into()));
    assert_eq!(paragraphs.len(), 2);
    let between = &body[body.find("</w:tbl>").unwrap()..body.rfind("<w:tbl>").unwrap()];
    assert!(between.contains("<w:p"), "{between}");
    // Column widths add up to exactly that width.
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
    // Distinct drawing ids.
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
    // Identical bytes share one media part; the reference document's own
    // sample-body image is gone with the sample body.
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

    // Editor HTML pictures go through the same check.
    let html = export_html(
        "15-images-confined-html",
        "<p><img src=\"../secret.png\" alt=\"up\"><img src=\"img/dot.png\" alt=\"in\"></p>",
        &options,
    );
    assert_eq!(html.report.counts.images, 1);
    assert_eq!(html.warning_codes(), ["image_outside_resource_dir"]);
    // Strict refuses.
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
    // The supplied picture and the data: URI; nothing from disk.
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
    // The text stays.
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

    // Editor HTML links follow the same policy, control characters stripped.
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
    // As deep as the nesting cap allows; deeper sources are refused (tests/markdown_parse.rs).
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

/// A horizontal rule: an empty paragraph with a bottom border.
const RULE: &str = "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr><w:spacing w:before=\"120\" w:after=\"120\"/><w:rPr><w:sz w:val=\"2\"/><w:szCs w:val=\"2\"/></w:rPr></w:pPr></w:p>";

#[test]
fn rules_page_breaks_and_toc() {
    let out = export(
        "17-breaks-toc",
        "[TOC]\n\n# A\n\nx\n\n## A.1\n\n---\n\n\\newpage\n\n<!-- pagebreak -->\n\n# B\n",
    );
    let body = out.body();
    // One flat line, no shape.
    assert_eq!(count(&body, RULE), 1, "{body}");
    for shape in ["<v:rect", "o:hr=", "<w:pict>"] {
        assert!(!body.contains(shape), "{shape} in {body}");
    }
    // The same from the bundled blank document: no template style is needed.
    let blank = export_with(
        "17f-rule-blank",
        "a\n\n---\n\nb\n",
        None,
        &MarkdownOptions::default(),
    );
    assert_eq!(count(&blank.body(), RULE), 1, "{}", blank.body());
    assert_eq!(count(&body, "<w:br w:type=\"page\"/>"), 2);
    // The contents is written out (linked entries with estimated page
    // numbers), so it needs no field update and Word opens it without
    // asking; the field keeps page numbers, so Word's own update redoes them.
    assert!(body.contains(" TOC \\o \"1-3\" \\h \\z \\u </w:instrText>"));
    // A [TOC] marker in the text adds no page break of its own.
    assert!(!body.contains("pageBreakBefore"));
    let paragraphs = out.paragraphs();
    assert_eq!(
        paragraphs[..4],
        [
            ("TOC Heading".to_string(), "Contents".to_string()),
            // python-docx shows Word's internal names for these built-ins.
            ("toc 1".to_string(), "A\t1".to_string()),
            ("toc 2".to_string(), "A.1\t1".to_string()),
            ("toc 1".to_string(), "B\t3".to_string()),
        ]
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
    // A body that opens with its own page break gets no second one.
    let own = export_with(
        "17d-toc-own-break",
        "\\newpage\n\n# A\n",
        Some(reference()),
        &options,
    );
    assert!(!own.body().contains("pageBreakBefore"));
    assert_eq!(count(&own.body(), "<w:br w:type=\"page\"/>"), 1);
    // A body that opens with a table gets a page-break paragraph.
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
    // Each entry ends in a dotted right tab and a PAGEREF to its heading,
    // its result the estimated number: contents on 1, A on 2 (the page
    // break after the contents), B on 3, C on 5.
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

    // Off: the entries carry no numbers and there is no report.
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
    let entries: Vec<String> = out
        .paragraphs()
        .into_iter()
        .filter(|(style, _)| style.starts_with("toc "))
        .map(|(_, text)| text)
        .collect();
    assert_eq!(entries, ["A\t2", "B\t2"]);
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
        ("TOC Heading", "Contents"),
    ]
    .iter()
    .map(|(s, t)| (s.to_string(), t.to_string()))
    .collect();
    assert_eq!(paragraphs[..7], expected[..]);
    assert!(out.body().contains("TOC \\o \"1-2\""));
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

/// Editor HTML (the shapes a rich-text editor emits), converted
/// with HTML as the source.
fn export_html(name: &str, html: &str, options: &MarkdownOptions) -> Export {
    let mut doc = reference();
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
    // Colours, font and highlights as direct run formatting.
    assert!(body.contains("<w:color w:val=\"DC2626\"/>"), "{body}");
    assert!(body.contains(
        "<w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\" w:cs=\"Courier New\"/>"
    ));
    assert!(body.contains("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FEF08A\"/>"));
    assert!(body.contains("<w:highlight w:val=\"yellow\"/>"));
    // Alignment on headings and paragraphs.
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
    // Merged cells and editor widths over the full text width.
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
    // Nothing is dropped silently: unknown CSS is reported by property.
    let out = export_html(
        "21-editor-html-dropped",
        "<p style=\"line-height: 2\"><span style=\"letter-spacing: 1px\">x</span></p>",
        &MarkdownOptions::default(),
    );
    assert_eq!(
        out.warning_codes(),
        ["html_style_dropped", "html_style_dropped"]
    );
    // An aligned list nested in an item keeps its level.
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
    assert_eq!(inspection["roles"]["heading1"]["name"], "Heading 1");
    // Read-only: inspecting changes nothing.
    let before = doc.patch_bytes().unwrap().0;
    assert_eq!(before, reference().patch_bytes().unwrap().0);
    // A plain Word template: the styles a conversion adds are problems.
    let blank = docrs::Document::new();
    let inspection = docrs::markdown::inspect(&blank, &MarkdownOptions::default()).unwrap();
    let problems = inspection["problems"].as_array().unwrap();
    assert!(
        problems.iter().any(|p| p["code"] == "style_added"),
        "{problems:?}"
    );
}

// --------------------------------- an HTML-converted export style
//
// `html-converted-template.docx` (tools/markdown/make_html_converted_template.py)
// has the shape of an integrator's template-comparison case: a finished advice
// document converted from HTML, its look in direct formatting.

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

fn advice_export_from(name: &str, mut doc: Document) -> Export {
    let options = MarkdownOptions {
        toc: Some(true),
        page_numbers: true,
        now: Some(docrs::clock::Timestamp::from_unix(ADVICE_NOW)),
        ..MarkdownOptions::default()
    };
    let report = docrs::markdown::insert_html(&mut doc, ADVICE_HTML, &options).unwrap();
    let (bytes, _) = doc.patch_bytes().unwrap();
    if let Some(dir) = std::env::var_os("DOCRS_MARKDOWN_EXPORT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(PathBuf::from(dir).join(format!("{name}.docx")), &bytes).unwrap();
    }
    Export {
        doc: Document::open_bytes(&bytes, &Limits::default()).unwrap(),
        report,
        bytes,
    }
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
    // The footer and its logo stay.
    assert!(names.contains(&"word/footer1.xml"));
    assert!(names.contains(&"word/media/logo.png"));
    let rels = out.part("word/_rels/document.xml.rels");
    assert!(
        !rels.contains("media/image") && !rels.contains("comments") && !rels.contains("customXml")
    );
    assert!(!out.part("[Content_Types].xml").contains("comments"));
    // Nothing of the sample content is left in any part.
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
    assert!(!out.body().contains("<w:pStyle w:val=\"NormalWeb\"/>"));
}

#[test]
fn table_look_comes_from_typical_body_cells_with_row_lines() {
    let out = advice_export("32-table-look");
    let table = style_of(&out, "Table");
    let whole_rpr = &table[table.find("<w:rPr>").unwrap()..table.find("</w:rPr>").unwrap()];
    assert!(!whole_rpr.contains("<w:b/>"), "{table}");
    assert!(!whole_rpr.contains("FAA436"), "{table}");
    assert!(whole_rpr.contains("w:ascii=\"Arial\""));
    assert!(
        table.contains("<w:insideH w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"D0D0D0\"/>"),
        "{table}"
    );
    assert!(
        table.contains("<w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"0\" w:color=\"D0D0D0\"/>")
    );
    // Row lines are table borders, not a whole-table cell border.
    let whole_tcpr =
        &table[table.find("</w:tblPr>").unwrap()..table.find("<w:tblStylePr").unwrap()];
    assert!(!whole_tcpr.contains("tcBorders"), "{table}");
    assert!(table.contains("<w:tblCellMar><w:top w:w=\"100\""));
    // The header row keeps its cyan fill and white bold text.
    assert!(table.contains("w:fill=\"40C7F2\""));
    assert!(table.contains("<w:color w:val=\"FFFFFF\"/>"));
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
    // Heading paragraphs carry no direct formatting.
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

/// The HTML-converted template with one footer field's instruction
/// replaced.
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
    // The language stays; the document is new, made now.
    assert!(core.contains("<dc:language>en-AU</dc:language>"));
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
    // The template's own TOC 1 is used as it is.
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

// ------------------------------------------------------ reference document

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
    // Every role was found, so the styles part is untouched.
    assert!(out.report.created_styles.is_empty());
    assert_eq!(out.part("word/styles.xml"), reference_styles);
    // Page setup and the header/footer references survive in the sectPr.
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
    // Body, its relationships, the footnotes part losing the sample
    // body's footnote, the content types losing the removed parts, the
    // package relationships losing the thumbnail, and the template's
    // document properties cleared.
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
    // The sample body's picture, the custom XML no content control is
    // bound to and the thumbnail of the template's first page leave the
    // package; the header, footer and their parts stay.
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

// ------------------------------------------------------------ placement

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

// ------------------------------------------------------------ guarantees

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

// ------------------------------------------- real-world template shapes

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
    // The converter's own definitions are not needed.
    assert!(
        !out.part("word/numbering.xml")
            .contains("<w:numId w:val=\"6\"/><w:ilvl w:val=\"3\"/>")
    );
    // The HTML table keeps its bulleted cell.
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
    // Headings drawn with direct outline levels become Heading 1-3.
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
    // A hand-formatted sample table becomes the Table style.
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
    // A Word gallery table style used by the sample table is reused.
    let out = export_with(
        "34-v2-builtin-table-style-only",
        markdown,
        Some(v2("builtin-table-style-only")),
        &MarkdownOptions::default(),
    );
    assert_ne!(out.report.styles["table"].name, "Table");
    assert!(!out.report.styles["table"].created);
    // Body text formatted by hand: its font becomes Body Text's.
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
    // Bullets made with Word's bullet button keep their look.
    let out = export_with(
        "36-v2-list-paragraph-bullets",
        markdown,
        Some(v2("list-paragraph-bullets")),
        &MarkdownOptions::default(),
    );
    assert!(out.report.styles.contains_key("list_bullet"));
    // A custom style with a heading outline level is the heading.
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

// ---------------------------------------------------------------------------
// Synthetic templates in the shapes practices upload (observed in real
// export templates; built here from scratch, no customer content).

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Default)]
struct Synthetic<'a> {
    /// `w:style` elements after Normal.
    styles: &'a str,
    /// Body children before the final section properties.
    body: &'a str,
    /// `w:abstractNum` and `w:num` elements.
    numbering: &'a str,
    /// A distinct first page whose header holds a picture this tall (EMU).
    cover_height: Option<i64>,
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
    if let Some(height) = template.cover_height {
        doc_rels += &rel("rId4", "header", "header1.xml");
        section = "<w:headerReference w:type=\"first\" r:id=\"rId4\"/>".to_string();
        let header = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:hdr xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" \
             xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <w:p><w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
             <wp:extent cx=\"5000000\" cy=\"{height}\"/><wp:docPr id=\"1\" name=\"Cover\"/>\
             <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <pic:pic><pic:nvPicPr><pic:cNvPr id=\"1\" name=\"cover.png\"/><pic:cNvPicPr/></pic:nvPicPr>\
             <pic:blipFill><a:blip r:embed=\"rIdImage\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"5000000\" cy=\"{height}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
             </a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:hdr>"
        );
        members.push(("word/header1.xml".into(), header.into_bytes()));
        members.push((
            "word/_rels/header1.xml.rels".into(),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}</Relationships>",
                rel("rIdImage", "image", "media/cover.png")
            )
            .into_bytes(),
        ));
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
    if template.cover_height.is_none() {
        members.retain(|(name, _)| name != "word/header1.xml");
    }
    let bytes = docrs::package::zip_write::write_archive(
        members.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
        &Limits::default(),
    )
    .unwrap();
    Document::open_bytes(&bytes, &Limits::default()).expect("synthetic template opens")
}

fn style_xml<'a>(styles: &'a str, id: &str) -> &'a str {
    let start = styles
        .find(&format!("w:styleId=\"{id}\""))
        .unwrap_or_else(|| panic!("no style {id}"));
    let rest = &styles[start..];
    &rest[..rest.find("</w:style>").unwrap()]
}

/// A table row of `cells`, each with cell properties `tc_pr` and run
/// properties `r_pr`.
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
    // Header fill and white bold header text applied by hand on a Table Grid
    // sample, next to an empty "Table" style; body text is grey.
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
    assert!(lifted.contains("<w:color w:val=\"FFFFFF\"/>"), "{lifted}");
    // The cell paragraphs inherit grey from Body Text, which in Word beats a
    // table style's colour, so the header runs carry white themselves.
    let body = out.body();
    let header = &body[body.find("<w:tr>").unwrap()..body.find("</w:tr>").unwrap()];
    assert_eq!(count(header, "<w:color w:val=\"FFFFFF\"/>"), 2, "{header}");
    let second = &body[body.find("</w:tr>").unwrap()..];
    assert!(!second[..second.find("</w:tbl>").unwrap()].contains("FFFFFF"));
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
    // The table asks for horizontal banding.
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
    // Bullets in custom styles that bind numbering; numbered headings that
    // must not be mistaken for a numbered list.
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
    // Detected: the content starts on page 2.
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
