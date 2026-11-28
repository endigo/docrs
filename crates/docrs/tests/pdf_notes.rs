//! Footnotes and endnotes in the PDF, read back by the test reader: the
//! separators' rules and the notes' lines where Word for Mac draws them.

mod support;

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
#[allow(dead_code)]
#[path = "support/glyphs.rs"]
mod text;

use docrs::layout::FontSet;
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{PdfOptions, paint_for_tests, render, render_unchecked_for_tests};
use docrs::{Document, Limits, Tolerance};
use reader::{Pdf, tokenize_content};
use support::{RawMember, build_zip, repo_root};
use text::{Glyph, glyphs, qpdf_check};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const LIBERATION: &str = "liberation/LiberationSerif-Regular.ttf";

fn font(path: &str) -> Vec<u8> {
    std::fs::read(repo_root().join("tests/fixtures/fonts").join(path)).unwrap()
}

/// A filled rule: its page, left, top (from the page's top), width and
/// thickness.
#[derive(Debug, Clone, Copy)]
struct Rule {
    page: usize,
    left: f64,
    top: f64,
    width: f64,
    thickness: f64,
}

/// Every filled rectangle under 1 pt high, page by page.
fn rules(pdf: &Pdf) -> Vec<Rule> {
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    let mut out = Vec::new();
    for (page, (dict, height)) in pdf.pages().unwrap().into_iter().zip(heights).enumerate() {
        let content = pdf.stream_data(pdf.get(dict, "Contents")).unwrap();
        let mut pending = Vec::new();
        for op in tokenize_content(&content).unwrap() {
            let n = |at: usize| op.operands[at].number().unwrap();
            match op.operator.as_str() {
                "re" => pending.push(Rule {
                    page,
                    left: n(0),
                    top: height - n(1) - n(3),
                    width: n(2),
                    thickness: n(3),
                }),
                "f" => out.extend(pending.drain(..).filter(|r| r.thickness < 1.0)),
                _ => pending.clear(),
            }
        }
    }
    out
}

/// A page's lines in drawing order: text and baseline, the lowest origin
/// of its glyphs (a note's mark is raised).
fn lines(drawn: &[Glyph]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for glyph in drawn {
        match out.iter_mut().find(|(_, y)| (*y - glyph.y).abs() < 5.0) {
            Some((text, y)) => {
                text.push_str(&glyph.text);
                *y = y.max(glyph.y);
            }
            None => out.push((glyph.text.clone(), glyph.y)),
        }
    }
    out
}

fn near(ours: f64, word: f64, within: f64) -> bool {
    (ours - word).abs() < within
}

/// The CLI test's long note (`layout_lines_continue_a_long_note_where_word_does`)
/// in the examples' reference, measured with the bundled metrics.
fn long_note() -> Document {
    let mut markdown = String::from(
        "# A note over three pages\n\nThis paragraph cites a long note.[^long] The body goes on after it.\n",
    );
    for n in 1..25 {
        markdown += &format!(
            "\nBody paragraph {n} keeps the text flowing so that the pages fill with ordinary lines of prose around the note.\n"
        );
    }
    markdown += "\n[^long]: The note starts on the page of its reference.\n";
    for n in 1..70 {
        markdown += &format!(
            "\n    Note paragraph {n}: a note this long cannot fit on one page, so Word continues it under the continuation separator.\n"
        );
    }
    let reference = repo_root().join("examples/markdown/reference.docx");
    let mut document = Document::open_path_with(reference, &Limits::default(), Tolerance::word())
        .unwrap()
        .0;
    insert_source_with(
        &mut document,
        &markdown,
        InputFormat::Markdown,
        &MarkdownOptions::default(),
        &FontSet::default(),
    )
    .unwrap();
    document
}

/// Word for Mac 16.115's PDF of the same DOCX (`pr-proof/pr68/word`): a
/// 144 pt rule above the note's start, a full-width one above each part
/// going on, and the note's lines on Word's baselines.
#[test]
fn a_long_note_is_drawn_where_word_draws_it() {
    let bytes = paint_for_tests(&long_note(), &FontSet::default(), &font(LIBERATION)).unwrap();
    qpdf_check("long-note.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let rules = rules(&pdf);
    let word = [(0, 163.44, 144.0), (1, 83.76, 470.64), (2, 470.16, 470.64)];
    assert_eq!(rules.len(), word.len(), "{rules:?}");
    for (rule, (page, top, width)) in rules.iter().zip(word) {
        assert_eq!(rule.page, page, "{rule:?}");
        assert!(near(rule.left, 62.4, 0.1), "{rule:?}");
        assert!(near(rule.top, top, 0.3), "{rule:?} against {top}");
        assert!(near(rule.width, width, 0.1), "{rule:?} against {width}");
        assert!(near(rule.thickness, 0.48, 0.01), "{rule:?}");
    }
    let drawn = glyphs(&pdf);
    let page = |at: usize| lines(&drawn[at]);
    let first = page(0);
    let start = first
        .iter()
        .position(|(text, _)| text.starts_with("1 The note starts"))
        .unwrap();
    let notes: Vec<f64> = first[start..]
        .iter()
        .filter(|(text, _)| !text.starts_with("Page"))
        .map(|(_, y)| *y)
        .collect();
    assert_eq!(notes.len(), 53);
    assert!(near(notes[0], 178.08, 0.5) && near(notes[52], 768.72, 0.5));
    assert!(
        first[..start]
            .iter()
            .all(|(text, _)| !text.contains("Note paragraph")),
        "body text first, then the page's notes"
    );
    let second = page(1);
    let continued = second
        .iter()
        .find(|(text, _)| text.starts_with("Note paragraph"))
        .unwrap();
    assert!(near(continued.1, 98.40, 0.5), "{continued:?}");
    assert!(
        !second
            .iter()
            .any(|(text, _)| text.starts_with("Body paragraph")),
        "the body resumes on page 3"
    );
}

/// A Letter document whose body is `body`, with `footnotes` and `endnotes`
/// (after each part's two separators), in 11 pt Liberation Serif.
fn noted(body: &str, footnotes: &str, endnotes: &str) -> Document {
    let separators = |kind: &str| {
        format!(
            r#"<w:{kind} w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:{kind}><w:{kind} w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{kind}>"#
        )
    };
    let part = |kind: &str, notes: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{kind}s xmlns:w="{W}" xmlns:r="{R}">{}{notes}</w:{kind}s>"#,
            separators(kind)
        )
    };
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}{section}</w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
    );
    let wml = "application/vnd.openxmlformats-officedocument.wordprocessingml";
    let types = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="{wml}.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="{wml}.styles+xml"/><Override PartName="/word/footnotes.xml" ContentType="{wml}.footnotes+xml"/><Override PartName="/word/endnotes.xml" ContentType="{wml}.endnotes+xml"/></Types>"#
    );
    let rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rS" Type="{R}/styles" Target="styles.xml"/><Relationship Id="rF" Type="{R}/footnotes" Target="footnotes.xml"/><Relationship Id="rE" Type="{R}/endnotes" Target="endnotes.xml"/></Relationships>"#
    );
    let (footnotes, endnotes) = (part("footnote", footnotes), part("endnote", endnotes));
    let members = [
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", support::ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
        RawMember::new("word/footnotes.xml", footnotes.as_bytes()),
        RawMember::new("word/endnotes.xml", endnotes.as_bytes()),
    ];
    support::open_bytes(&build_zip(&members))
}

fn run(content: &str) -> String {
    format!("<w:r>{content}</w:r>")
}

fn text(text: &str) -> String {
    run(&format!(r#"<w:t xml:space="preserve">{text}</w:t>"#))
}

fn superscript(content: &str) -> String {
    format!(r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr>{content}</w:r>"#)
}

fn para(content: &str) -> String {
    format!("<w:p>{content}</w:p>")
}

fn note(kind: &str, words: &str) -> String {
    format!(
        r#"<w:{kind} w:id="1">{}</w:{kind}>"#,
        para(&(superscript(&format!("<w:{kind}Ref/>")) + &text(words)))
    )
}

fn both_notes() -> Document {
    let body = para(
        &(text("A footnote")
            + &superscript(r#"<w:footnoteReference w:id="1"/>"#)
            + &text(" and an endnote")
            + &superscript(r#"<w:endnoteReference w:id="1"/>"#)),
    ) + &para(&text("The last paragraph."));
    noted(
        &body,
        &note("footnote", " The footnote."),
        &note("endnote", " The endnote."),
    )
}

fn liberation() -> PdfOptions {
    let mut fonts = FontSet::builder();
    fonts.add(font(LIBERATION)).unwrap();
    let mut options = PdfOptions::default();
    options.fonts = fonts.build();
    options
}

/// The footnote at the foot of the page under the 2-inch separator, and
/// the endnote after the last paragraph under its own; strict accepts it.
#[test]
fn strict_draws_a_footnote_and_an_endnote() {
    let mut options = liberation();
    options.strict = true;
    let (bytes, report) = render(&both_notes(), &options).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    qpdf_check("notes.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let drawn = lines(&glyphs(&pdf)[0]);
    let texts: Vec<&str> = drawn.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "A footnote1 and an endnotei",
            "The last paragraph.",
            "i The endnote.",
            "1 The footnote.",
        ],
        "body text, the endnotes after it, then the page's footnotes"
    );
    let baseline = |at: usize| drawn[at].1;
    let rules = rules(&pdf);
    assert_eq!(rules.len(), 2, "{rules:?}");
    for rule in &rules {
        assert!(near(rule.left, 72.0, 0.01) && near(rule.width, 144.0, 0.01));
    }
    let (endnote, footnote) = (rules[0], rules[1]);
    assert!(baseline(1) < endnote.top && endnote.top < baseline(2));
    assert!(baseline(2) < footnote.top && footnote.top < baseline(3));
    assert!(baseline(3) > 700.0, "at the foot of the page");
}

/// Word never splits a note holding a table across pages: such a note is
/// refused, not drawn otherwise (past the preflight, which still refuses
/// tables).
#[test]
fn a_note_whose_table_would_go_on_is_refused() {
    let row = format!("<w:tr><w:tc>{}</w:tc></w:tr>", para(&text("Row")));
    let table = format!(
        "<w:tbl><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid>{}</w:tbl>",
        row.repeat(80)
    );
    let footnote = format!(
        r#"<w:footnote w:id="1">{}{table}{}</w:footnote>"#,
        para(&superscript("<w:footnoteRef/>")),
        para("")
    );
    let body =
        para(&(text("Cites a long table") + &superscript(r#"<w:footnoteReference w:id="1"/>"#)));
    let err = render_unchecked_for_tests(&noted(&body, &footnote, ""), &liberation()).unwrap_err();
    assert_eq!(err.reason(), "pdf_footnote_layout_unsupported", "{err}");
    assert!(err.message().contains("footnote 1 holds a table"), "{err}");
}
