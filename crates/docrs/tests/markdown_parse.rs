//! Markdown, HTML and math parsing under hostile input: misnested and
//! unclosed inline HTML, non-ASCII front matter indentation, nesting past
//! [`MAX_NESTING_DEPTH`], tables wider than Word's 63 columns and HTML
//! elements browsers do not display. Nothing may panic or overflow the
//! stack (test threads have 2 MB, so a recursive regression fails loudly),
//! refusals are typed, and text is not lost.

use docrs::markdown::{MAX_NESTING_DEPTH, MarkdownOptions, MarkdownReport, convert, insert_html};
use docrs::{Document, Error, ErrorKind, Limits, RefusalKind};
use proptest::prelude::*;

/// A conversion, exported and reopened under the default limits.
struct Converted {
    doc: Document,
    report: MarkdownReport,
}

impl Converted {
    fn body(&self) -> String {
        String::from_utf8(docrs::xml::serialize(
            self.doc.package().xml("word/document.xml").unwrap(),
        ))
        .unwrap()
    }

    /// The text of every `w:t` in the body, in order.
    fn texts(&self) -> Vec<String> {
        let body = self.body();
        let mut out = Vec::new();
        let mut rest = body.as_str();
        while let Some(start) = rest.find("<w:t") {
            rest = &rest[start + 4..];
            // `<w:t>` or `<w:t xml:space=…>`, not `<w:tbl>`, `<w:tc>`, ….
            if !rest.starts_with('>') && !rest.starts_with(' ') {
                continue;
            }
            let open = rest.find('>').unwrap();
            let close = rest.find("</w:t>").unwrap();
            out.push(rest[open + 1..close].to_string());
            rest = &rest[close..];
        }
        out
    }

    fn text(&self) -> String {
        self.texts().concat()
    }

    fn warnings(&self, code: &str) -> usize {
        self.report
            .warnings
            .iter()
            .filter(|w| w.code == code)
            .count()
    }
}

fn reopen(result: docrs::Result<(Document, MarkdownReport)>, what: &str) -> Converted {
    let (doc, report) = result.unwrap_or_else(|err| panic!("{what}: conversion failed: {err}"));
    let (bytes, _) = doc.patch_bytes().unwrap();
    let doc = Document::open_bytes(&bytes, &Limits::default())
        .unwrap_or_else(|err| panic!("{what}: exported file does not reopen: {err}"));
    Converted { doc, report }
}

fn markdown(source: &str) -> Converted {
    reopen(
        convert(source, None, &MarkdownOptions::default()),
        &format!("markdown {:?}", truncated(source)),
    )
}

fn html_result(source: &str) -> docrs::Result<(Document, MarkdownReport)> {
    let mut doc = Document::new();
    let report = insert_html(&mut doc, source, &MarkdownOptions::default())?;
    Ok((doc, report))
}

fn html(source: &str) -> Converted {
    reopen(
        html_result(source),
        &format!("html {:?}", truncated(source)),
    )
}

fn truncated(source: &str) -> String {
    source.chars().take(60).collect()
}

/// The refusal for nesting past the cap; the document is left unchanged.
fn assert_too_deep(result: docrs::Result<(Document, MarkdownReport)>, what: &str) {
    let err: Error = match result {
        Ok(_) => panic!("{what}: nesting past the cap was converted"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), ErrorKind::ResourceLimit, "{what}: {err}");
    assert_eq!(err.reason(), "markdown_nesting_too_deep", "{what}: {err}");
    let limit = err.limit_exceeded().expect("a budget");
    assert_eq!(limit.allowed, MAX_NESTING_DEPTH as u64);
}

fn assert_depth_cap(what: &str, build: impl Fn(usize) -> String, is_html: bool) {
    let run = |depth: usize| {
        let source = build(depth);
        if is_html {
            html_result(&source)
        } else {
            convert(&source, None, &MarkdownOptions::default())
        }
    };
    let at_cap = reopen(run(MAX_NESTING_DEPTH), &format!("{what} at the cap"));
    // In body text, or in an equation.
    assert!(
        at_cap.text().contains('x') || at_cap.body().contains(">x</m:t>"),
        "{what}: the text survives"
    );
    assert_too_deep(run(MAX_NESTING_DEPTH + 1), &format!("{what} one over"));
    assert_too_deep(run(50_000), &format!("{what} 50k deep"));
}

// ------------------------------------------------------------- nesting

#[test]
fn nested_block_quotes_stop_at_the_cap() {
    assert_depth_cap("block quotes", |n| format!("{} x\n", ">".repeat(n)), false);
}

#[test]
fn nested_lists_stop_at_the_cap() {
    assert_depth_cap("bullet lists", |n| format!("{}x\n", "- ".repeat(n)), false);
    assert_depth_cap(
        "ordered lists",
        |n| format!("{}x\n", "1. ".repeat(n)),
        false,
    );
}

#[test]
fn nested_emphasis_stops_at_the_cap() {
    // `*a _a *a … x … a* a_ a*`: one emphasis level per delimiter pair.
    let build = |n: usize| {
        let delimiter = |i: usize| if i.is_multiple_of(2) { '*' } else { '_' };
        let mut source = String::new();
        for i in 0..n {
            source.push(delimiter(i));
            source.push_str("a ");
        }
        source.push('x');
        for i in (0..n).rev() {
            source.push_str(" a");
            source.push(delimiter(i));
        }
        source.push('\n');
        source
    };
    assert_depth_cap("emphasis", build, false);
}

#[test]
fn nested_images_and_links_stop_at_the_cap() {
    // An image's description may hold images; the innermost is a link.
    assert_depth_cap(
        "images",
        |n| {
            format!(
                "{}[x](https://example.com){}\n",
                "![".repeat(n - 1),
                "](i.png)".repeat(n - 1)
            )
        },
        false,
    );
}

#[test]
fn nested_inline_html_stops_at_the_cap() {
    assert_depth_cap(
        "inline HTML spans",
        |n| format!("a {}x{}\n", "<b>".repeat(n), "</b>".repeat(n)),
        false,
    );
}

#[test]
fn nested_html_elements_stop_at_the_cap() {
    assert_depth_cap(
        "HTML divs",
        |n| format!("{}x{}", "<div>".repeat(n), "</div>".repeat(n)),
        true,
    );
    assert_depth_cap(
        "HTML spans",
        |n| {
            format!(
                "<p>{}x{}</p>",
                "<span>".repeat(n - 1),
                "</span>".repeat(n - 1)
            )
        },
        true,
    );
    // Tables in cells nest three elements (and three Word elements) a
    // level: the deepest that fits still reopens under the default limits.
    let tables = |n: usize| {
        format!(
            "{}x{}",
            "<table><tr><td>".repeat(n),
            "</td></tr></table>".repeat(n)
        )
    };
    let fits = MAX_NESTING_DEPTH / 3;
    let converted = html(&tables(fits));
    assert_eq!(converted.body().matches("<w:tbl>").count(), fits);
    assert_too_deep(html_result(&tables(fits + 1)), "tables");
    // An HTML block in Markdown nests inside the Markdown around it.
    assert_depth_cap(
        "HTML block in a quote",
        |n| format!("> {}x{}\n", "<div>".repeat(n - 1), "</div>".repeat(n - 1)),
        false,
    );
}

#[test]
fn nested_math_groups_stop_at_the_cap() {
    assert_depth_cap(
        "inline math groups",
        |n| format!("a ${}x{}$\n", "{".repeat(n), "}".repeat(n)),
        false,
    );
    assert_depth_cap(
        "display math groups",
        |n| format!("$${}x{}$$\n", "{".repeat(n), "}".repeat(n)),
        false,
    );
    // Math nests on top of the Markdown around it.
    assert_depth_cap(
        "math in quotes",
        |n| format!("{} ${{x}}$\n", ">".repeat(n - 1)),
        false,
    );
}

#[test]
fn deep_equations_count_their_word_structure() {
    // A root nests two equation elements and its group one more level:
    // 42 roots fit, 43 do not, and the deepest equation reopens under the
    // default XML depth limit.
    let roots = |n: usize| format!("$${}x{}$$\n", "\\sqrt{".repeat(n), "}".repeat(n));
    let fits = MAX_NESTING_DEPTH / 3;
    let converted = markdown(&roots(fits));
    assert!(converted.body().matches("<m:rad>").count() == fits);
    assert_too_deep(
        convert(&roots(fits + 1), None, &MarkdownOptions::default()),
        "roots",
    );
    // Operators taking operators, without braces, are counted too.
    // (Math braces must balance for the parser to read `$…$` as math.)
    for (open, close) in [
        ("\\sqrt", ""),
        ("\\sum", ""),
        ("\\sin", ""),
        ("\\hat", ""),
        ("\\sqrt[", ""),
        ("x^{", "}"),
        ("\\frac{", "}{y}"),
        ("\\left(", "\\right)"),
    ] {
        let command = open;
        let source = format!("${}x{}$\n", open.repeat(50_000), close.repeat(50_000));
        assert_too_deep(convert(&source, None, &MarkdownOptions::default()), command);
    }
    assert_too_deep(
        convert(
            &format!("${}$\n", "\\begin{pmatrix}".repeat(50_000)),
            None,
            &MarkdownOptions::default(),
        ),
        "matrices",
    );
}

#[test]
fn a_refused_conversion_leaves_the_document_unchanged() {
    let mut doc = Document::new();
    let before = doc.patch_bytes().unwrap().0;
    let deep = format!("# Title\n\n{} x\n", ">".repeat(MAX_NESTING_DEPTH + 1));
    let err =
        docrs::markdown::insert_markdown(&mut doc, &deep, &MarkdownOptions::default()).unwrap_err();
    assert_eq!(err.reason(), "markdown_nesting_too_deep");
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

// --------------------------------------------------------- inline HTML

#[test]
fn inline_html_closing_across_emphasis_keeps_the_text() {
    let converted = markdown("<b>*x</b>*\n");
    assert_eq!(converted.text(), "x");
    let body = converted.body();
    assert!(body.contains("<w:b/>") && body.contains("<w:i/>"), "{body}");
    for source in [
        "<b>**x</b>** y\n",
        "<i>a *b <u>c</i> d* e</u> f\n",
        "<b><i>*x</b>*</i> y\n",
        "*a <b>x* y</b> z\n",
        "<b>[x</b>](https://example.com) y\n",
        "<b>![x</b>](i.png) y\n",
        "</b></i> x <b>\n",
        "- <b>*x</b>*\n- y\n",
        "> <b>*x</b>*\n> y\n",
    ] {
        let converted = markdown(source);
        let text = converted.text();
        for word in ["x", "y", "z", "a", "b", "c", "d", "e", "f"] {
            let plain: String = source
                .replace("<b>", "")
                .replace("</b>", "")
                .replace("<i>", "")
                .replace("</i>", "")
                .replace("<u>", "")
                .replace("</u>", "");
            if plain.contains(&format!("{word} ")) || plain.contains(&format!("{word}*")) {
                assert!(text.contains(word), "{source:?}: {word} lost from {text:?}");
            }
        }
    }
}

#[test]
fn unclosed_inline_html_ends_with_its_block() {
    // A tight list item's unclosed tag used to swallow the nested list.
    let converted = markdown("- <b>a\n  - b\n");
    assert_eq!(converted.texts(), vec!["a", "b"]);
    assert_eq!(converted.report.counts.list_items, 2);
    // The tag does not leak into the next item or block.
    let converted = markdown("- <u>a\n- b\n\n> <i>q\n\nafter\n");
    assert_eq!(converted.text(), "abqafter");
    let body = converted.body();
    assert_eq!(body.matches("<w:u ").count(), 1, "{body}");
    // Display math inside an unclosed span or emphasis is kept.
    let converted = markdown("<b>x $$y$$ z\n\n*a $$b$$ c*\n");
    let body = converted.body();
    for word in ["x", "z", "a", "c"] {
        assert!(converted.text().contains(word), "{word} lost: {body}");
    }
    assert_eq!(converted.report.counts.equations, 2);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any mix of Markdown delimiters and inline HTML converts, and every
    /// `x` written survives.
    #[test]
    fn misnested_inline_html_never_panics_or_loses_text(
        pieces in prop::collection::vec(
            prop::sample::select(vec![
                "<b>", "</b>", "<i>", "</i>", "<u>", "</u>", "<sup>", "</sup>",
                "<span style=\"color: red\">", "</span>", "<code>", "</code>",
                "*", "_", "**", "~~", "[", "](https://e.org)", "![", "](i.png)",
                "x", "x ", " ", "\n", "\n\n", "- ", "> ", "1. ", "  ", "`",
            ]),
            0..40,
        )
    ) {
        let source: String = pieces.concat();
        let converted = markdown(&source);
        let written = source.matches('x').count();
        // Inside a code span, `x` stays; image descriptions become alt text
        // (not body text), so only check sources without images.
        if !source.contains("![") {
            prop_assert_eq!(
                converted.text().matches('x').count(),
                written,
                "{:?} -> {:?}",
                source,
                converted.texts()
            );
        }
    }
}

// --------------------------------------------------------- front matter

#[test]
fn front_matter_indented_with_non_ascii_spaces_converts() {
    for source in [
        "---\nabstract: |\n x\n\u{a0}y\n---\n\nbody\n",
        "---\nabstract: >\n  x\n\u{a0}\u{a0}\n  y\n---\n\nbody\n",
        "---\nabstract: |\n\u{3000}x\n y\n---\n\nbody\n",
        "---\ntitle: T\n\u{a0}author: A\n\tkeywords: k\n---\n\nbody\n",
    ] {
        let converted = markdown(source);
        assert!(converted.text().ends_with("body"), "{source:?}");
    }
    let converted = markdown("---\ntitle: T\nabstract: |\n  one\n  \u{a0}two\n---\n\nbody\n");
    let text = converted.text();
    assert!(
        text.contains("one") && text.contains("\u{a0}two"),
        "{text:?}"
    );
}

// --------------------------------------------------------------- tables

#[test]
fn wide_colspans_are_cut_to_word_columns() {
    let cells: String = (0..300)
        .map(|i| format!("<td colspan=\"1000\">c{i}</td>"))
        .collect();
    let converted = html(&format!("<table><tr>{cells}</tr></table>"));
    let body = converted.body();
    assert_eq!(body.matches("<w:gridCol ").count(), 63);
    assert_eq!(converted.warnings("table_columns_clamped"), 1);
    // Every cell's text is kept, merged into the last column.
    let text = converted.text();
    for i in 0..300 {
        assert!(text.contains(&format!("c{i}")), "c{i} lost");
    }
    // Strict conversion refuses the loss.
    let mut doc = Document::new();
    let strict = MarkdownOptions {
        strict: true,
        ..MarkdownOptions::default()
    };
    let err = insert_html(
        &mut doc,
        &format!("<table><tr>{cells}</tr></table>"),
        &strict,
    )
    .unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::UnsupportedStructure)
    );
}

#[test]
fn row_spans_and_ragged_rows_stay_within_word_columns() {
    // Each row's cell spans every row below: a staircase that used to add
    // a column per row.
    let rows: String = (0..200)
        .map(|i| format!("<tr><td rowspan=\"1000\">r{i}</td></tr>"))
        .collect();
    let converted = html(&format!("<table>{rows}</table>"));
    assert!(converted.body().matches("<w:gridCol ").count() <= 63);
    let text = converted.text();
    for i in 0..200 {
        assert!(text.contains(&format!("r{i}")), "r{i} lost");
    }
    // One very long row over many short ones: the grid was rows x cells.
    let long: String = (0..2_000).map(|i| format!("<td>l{i}</td>")).collect();
    let short: String = (0..2_000).map(|_| "<tr><td>s</td></tr>").collect();
    let converted = html(&format!("<table><tr>{long}</tr>{short}</table>"));
    let body = converted.body();
    assert_eq!(body.matches("<w:gridCol ").count(), 63);
    assert!(body.matches("<w:tc>").count() <= 63 * 2_001);
    assert!(converted.text().contains("l1999"));
    // A spanning header over a narrow table is unchanged.
    let converted = html(
        "<table><tr><th colspan=\"3\">h</th></tr><tr><td>a</td><td>b</td><td>c</td></tr></table>",
    );
    assert_eq!(converted.body().matches("<w:gridCol ").count(), 3);
    assert_eq!(converted.warnings("table_columns_clamped"), 0);
}

#[test]
fn wide_markdown_tables_are_narrowed_too() {
    let header: String = (0..70).map(|i| format!("| h{i} ")).collect();
    let rule = "| - ".repeat(70);
    let row: String = (0..70).map(|i| format!("| c{i} ")).collect();
    let converted = markdown(&format!("{header}|\n{rule}|\n{row}|\n"));
    assert_eq!(converted.body().matches("<w:gridCol ").count(), 63);
    assert_eq!(converted.warnings("table_columns_clamped"), 1);
    let text = converted.text();
    assert!(text.contains("h69") && text.contains("c69"), "{text}");
}

// ------------------------------------------------------- hidden elements

#[test]
fn html_elements_browsers_do_not_display_are_dropped() {
    let converted =
        html("<title>T</title><style>p{color:red}</style><script>alert(1)</script><p>hi</p>");
    assert_eq!(converted.texts(), vec!["hi"]);
    assert_eq!(converted.warnings("html_element_skipped"), 3);
    assert_eq!(converted.warnings("raw_html_dropped"), 0);
    // A whole document: the head goes with everything in it; one warning
    // per element kind; script text is not read as markup.
    let converted = html(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>T</title>\
         <style>p { color: red }</style></head><body><p>one</p>\
         <script>if (a<b && c>d) { document.write('<p>no</p>') }</script>\
         <template><p>tpl</p></template><noscript><p>ns</p></noscript>\
         <p>two</p><SCRIPT>x</SCRIPT></body></html>",
    );
    assert_eq!(converted.texts(), vec!["one", "two"]);
    let skipped: Vec<&str> = converted
        .report
        .warnings
        .iter()
        .filter(|w| w.code == "html_element_skipped")
        .map(|w| w.message.as_str())
        .collect();
    assert_eq!(skipped.len(), 4, "{skipped:?}");
    // In Markdown: an HTML block and inline tags.
    let converted = markdown(
        "<script>\nalert(1)\n</script>\n\na <style>p{}</style>b <meta name=\"x\"> c\n\n<style>\nq{}\n</style>\n",
    );
    assert_eq!(converted.text(), "a b  c");
    assert_eq!(converted.warnings("html_element_skipped"), 3);
}
