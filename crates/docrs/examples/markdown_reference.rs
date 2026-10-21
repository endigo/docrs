//! Build a customized Markdown reference document: the starter reference
//! (every Markdown style present) restyled with the library's style API,
//! on A4 with a running header and a page-numbered footer.
//!
//! It also writes `letterhead.docx` next to it: the same styles with a memo
//! heading block and a `{{content}}` paragraph, for `--placeholder`.
//!
//! cargo run -p docrs --features markdown --example markdown_reference -- reference.docx

use docrs::anchor::BlockTarget;
use docrs::authoring::{FontChange, LineSpacing, ParagraphFormat};
use docrs::sections::SectionChange;
use docrs::{Document, Result};

const PT: i64 = 12_700;
const CM: i64 = 360_000;
const TEAL: (u8, u8, u8) = (0x0F, 0x5C, 0x66);
const INK: (u8, u8, u8) = (0x26, 0x32, 0x38);

fn font(doc: &mut Document, style: &str, changes: &[FontChange]) -> Result<()> {
    let style = doc.style(style)?;
    style.set_font(doc, changes)
}

fn format(doc: &mut Document, style: &str, format: ParagraphFormat) -> Result<()> {
    let style = doc.style(style)?;
    style.set_paragraph_format(doc, &format)
}

fn flag(name: &str, value: bool) -> FontChange {
    FontChange::Flag(name.to_string(), Some(value))
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "reference.docx".to_string());
    let mut doc = docrs::markdown::reference_document()?;

    let serif = || FontChange::Name(Some("Georgia".to_string()));
    let sans = || FontChange::Name(Some("Arial".to_string()));
    let mono = || FontChange::Name(Some("Consolas".to_string()));

    font(
        &mut doc,
        "Normal",
        &[
            serif(),
            FontChange::Size(Some(21 * PT / 2)),
            FontChange::Color(Some(INK)),
        ],
    )?;
    format(
        &mut doc,
        "Body Text",
        ParagraphFormat {
            space_before: Some(Some(0)),
            space_after: Some(Some(8 * PT)),
            line_spacing: Some(Some(LineSpacing::Multiple(1.15))),
            ..ParagraphFormat::default()
        },
    )?;
    font(
        &mut doc,
        "Title",
        &[
            sans(),
            FontChange::Size(Some(28 * PT)),
            flag("bold", true),
            FontChange::Color(Some(TEAL)),
        ],
    )?;
    format(
        &mut doc,
        "Title",
        ParagraphFormat {
            alignment: Some(Some("left".to_string())),
            space_after: Some(Some(4 * PT)),
            ..ParagraphFormat::default()
        },
    )?;
    font(
        &mut doc,
        "Subtitle",
        &[
            sans(),
            FontChange::Size(Some(14 * PT)),
            flag("italic", false),
            FontChange::Color(Some((0x5F, 0x6B, 0x70))),
        ],
    )?;
    for name in ["Author", "Date"] {
        font(
            &mut doc,
            name,
            &[
                sans(),
                FontChange::Size(Some(10 * PT)),
                FontChange::Color(Some((0x5F, 0x6B, 0x70))),
            ],
        )?;
        format(
            &mut doc,
            name,
            ParagraphFormat {
                alignment: Some(Some("left".to_string())),
                ..ParagraphFormat::default()
            },
        )?;
    }
    for (level, size) in [(1, 18), (2, 14), (3, 12), (4, 11), (5, 11), (6, 11)] {
        let name = format!("Heading {level}");
        font(
            &mut doc,
            &name,
            &[
                sans(),
                FontChange::Size(Some(size * PT)),
                flag("bold", level <= 4),
                flag("italic", false),
                FontChange::Color(Some(TEAL)),
            ],
        )?;
        format(
            &mut doc,
            &name,
            ParagraphFormat {
                space_before: Some(Some(if level == 1 { 18 } else { 12 } * PT)),
                space_after: Some(Some(4 * PT)),
                ..ParagraphFormat::default()
            },
        )?;
    }
    font(
        &mut doc,
        "TOC Heading",
        &[
            sans(),
            FontChange::Size(Some(14 * PT)),
            FontChange::Color(Some(TEAL)),
        ],
    )?;
    font(
        &mut doc,
        "Block Text",
        &[
            flag("italic", true),
            FontChange::Color(Some((0x4A, 0x55, 0x5A))),
        ],
    )?;
    font(
        &mut doc,
        "Source Code",
        &[mono(), FontChange::Size(Some(9 * PT))],
    )?;
    font(
        &mut doc,
        "Verbatim Char",
        &[
            mono(),
            FontChange::Size(Some(19 * PT / 2)),
            FontChange::Color(Some((0xA3, 0x1F, 0x34))),
        ],
    )?;
    font(&mut doc, "Hyperlink", &[FontChange::Color(Some(TEAL))])?;
    for name in ["Image Caption", "Table Caption"] {
        font(
            &mut doc,
            name,
            &[
                sans(),
                FontChange::Size(Some(9 * PT)),
                FontChange::Color(Some((0x5F, 0x6B, 0x70))),
            ],
        )?;
    }
    font(
        &mut doc,
        "Definition Term",
        &[sans(), FontChange::Color(Some(TEAL))],
    )?;

    // A4, 2.2 cm margins, running header and page-numbered footer.
    let section = doc.sections()?.remove(0);
    section.set(
        &mut doc,
        &[
            SectionChange::PageWidth(Some(7_560_310)),
            SectionChange::PageHeight(Some(10_692_130)),
            SectionChange::LeftMargin(Some(22 * CM / 10)),
            SectionChange::RightMargin(Some(22 * CM / 10)),
            SectionChange::TopMargin(Some(25 * CM / 10)),
            SectionChange::BottomMargin(Some(25 * CM / 10)),
        ],
    )?;
    let header = section.header().definition(&mut doc)?;
    let paragraph = header.paragraphs(&doc)?.remove(0);
    let run = paragraph.add_run(
        &mut doc,
        Some("Paper Instruments \u{2014} Markdown export"),
        None,
    )?;
    run.set_font(
        &mut doc,
        &[
            sans(),
            FontChange::Size(Some(8 * PT)),
            FontChange::Color(Some(TEAL)),
        ],
    )?;
    let footer = section.footer().definition(&mut doc)?;
    let paragraph = footer.paragraphs(&doc)?.remove(0);
    paragraph.set_alignment(&mut doc, Some("right"))?;
    paragraph.add_run(&mut doc, Some("Page "), None)?;
    doc.add_page_number_field(BlockTarget::Text("Page "))?;
    // Re-render the sample body: a replaced body also drops the "update
    // fields on open" flag the PAGE field set (Word computes PAGE itself),
    // so documents made from this template open without a prompt.
    let options = docrs::markdown::MarkdownOptions {
        toc: Some(true),
        ..docrs::markdown::MarkdownOptions::default()
    };
    doc.insert_markdown(docrs::markdown::REFERENCE_SAMPLE, &options)?;

    doc.save(&out)?;
    println!("wrote {out}");

    // The letterhead: the reference with a memo block around a placeholder.
    let mut memo = Document::open_path(&out, &docrs::Limits::default())?;
    memo.clear_body_content()?;
    memo.add_paragraph("MEMORANDUM", Some("Title"))?;
    let table = memo.add_table(4, 2, Some("Table"))?;
    for (row, (label, value)) in [
        ("To", "Platform team"),
        ("From", "Paper Instruments"),
        ("Date", "19 October 2026"),
        ("Subject", "Quarterly review"),
    ]
    .into_iter()
    .enumerate()
    {
        table.cell(&memo, row, 0)?.set_text(&mut memo, label)?;
        table.cell(&memo, row, 1)?.set_text(&mut memo, value)?;
    }
    memo.add_paragraph("{{content}}", None)?;
    memo.add_paragraph(
        "Confidential \u{2014} internal distribution only.",
        Some("Subtitle"),
    )?;
    let letterhead = std::path::Path::new(&out).with_file_name("letterhead.docx");
    memo.save(&letterhead)?;
    println!("wrote {}", letterhead.display());
    Ok(())
}
