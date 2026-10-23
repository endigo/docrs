//! Restrict Editing gates every inherited writer (authoring, sections,
//! styles, core properties): a forbidden edit refuses `document_protected`
//! and leaves the package byte-identical; acknowledging protection lets it
//! through.

mod support;

use docrs::authoring::{
    BreakType, FontChange, Paragraph, ParagraphFormat, SectionStart, Table, TableDirection,
};
use docrs::coreprops::{CoreProperty, CorePropertyChange, CoreValue};
use docrs::sections::SectionChange;
use docrs::styles::StyleType;
use docrs::styles_write::{LatentDefaultsChange, LatentStyleChange, StyleChange};
use docrs::{Document, ErrorKind, Limits, RefusalKind, Result};
use support::{DOCUMENT_XML, package_with_settings};

const INCH: i64 = 914_400;
const PNG_150DPI: &str = "89504e470d0a1a0a0000000d49484452000000030000000208020000001216f14d0000000970485973000017120000171201679fd2520000000b49444154789c6360800100000a00017f80745e0000000049454e44ae426082";

fn png() -> Vec<u8> {
    (0..PNG_150DPI.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&PNG_150DPI[i..i + 2], 16).unwrap())
        .collect()
}

/// A document with something for every writer to touch, under `edit`.
fn protected(edit: &str) -> Vec<u8> {
    let mut doc = Document::new();
    let p = doc.add_paragraph("alpha beta", None).unwrap();
    p.add_run(&mut doc, Some("gamma"), None).unwrap();
    p.add_tab_stop(&mut doc, INCH, "left", "dot").unwrap();
    doc.add_table(2, 2, None).unwrap();
    doc.add_picture(&png(), None, None).unwrap();
    doc.add_style("Gate Style", StyleType::Paragraph, false)
        .unwrap();
    doc.add_latent_style("Gate Latent").unwrap();
    doc.sections().unwrap()[0]
        .header()
        .set_linked_to_previous(&mut doc, false)
        .unwrap();
    doc.set_protection(edit).unwrap();
    doc.patch_bytes().unwrap().0
}

fn open(bytes: &[u8]) -> Document {
    Document::open_bytes(bytes, &Limits::default()).unwrap()
}

fn para(doc: &Document) -> Paragraph {
    doc.paragraphs().unwrap().remove(0)
}

fn table(doc: &Document) -> Table {
    doc.tables().unwrap().remove(0)
}

fn picture_path() -> tempfile::TempPath {
    let file = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
    std::fs::write(file.path(), png()).unwrap();
    file.into_temp_path()
}

/// A writer is body content unless it is listed as a comment edit.
type Writer = fn(&mut Document) -> Result<()>;

fn writers() -> Vec<(&'static str, bool, Writer)> {
    vec![
        ("Document::add_paragraph", false, |d| {
            d.add_paragraph("x", None).map(drop)
        }),
        ("Document::add_heading", false, |d| {
            d.add_heading("x", 1).map(drop)
        }),
        ("Document::add_page_break", false, |d| {
            d.add_page_break().map(drop)
        }),
        ("Document::add_table", false, |d| {
            d.add_table(1, 1, None).map(drop)
        }),
        ("Document::add_picture", false, |d| {
            d.add_picture(&png(), None, None).map(drop)
        }),
        ("Document::add_picture_file", false, |d| {
            d.add_picture_file(picture_path(), None, None).map(drop)
        }),
        ("Document::add_section", false, |d| {
            d.add_section(SectionStart::OddPage).map(drop)
        }),
        ("Document::clear_body_content", false, |d| {
            d.clear_body_content()
        }),
        ("Paragraph::add_run", false, |d| {
            para(d).add_run(d, Some("x"), None).map(drop)
        }),
        ("Paragraph::insert_paragraph_before", false, |d| {
            para(d)
                .insert_paragraph_before(d, Some("x"), None)
                .map(drop)
        }),
        ("Paragraph::set_text", false, |d| {
            para(d).set_text(d, Some("x"))
        }),
        ("Paragraph::clear", false, |d| para(d).clear(d)),
        ("Paragraph::set_style", false, |d| {
            para(d).set_style(d, Some("Gate Style"))
        }),
        ("Paragraph::set_alignment", false, |d| {
            para(d).set_alignment(d, Some("center"))
        }),
        ("Paragraph::set_format", false, |d| {
            let format = ParagraphFormat {
                left_indent: Some(Some(INCH)),
                ..ParagraphFormat::default()
            };
            para(d).set_format(d, &format)
        }),
        ("Paragraph::add_tab_stop", false, |d| {
            para(d).add_tab_stop(d, 2 * INCH, "right", "dot").map(drop)
        }),
        ("Paragraph::clear_tab_stops", false, |d| {
            para(d).clear_tab_stops(d)
        }),
        ("Paragraph::delete_tab_stop", false, |d| {
            para(d).delete_tab_stop(d, 0)
        }),
        ("TabStop::set_position", false, |d| {
            let tab = para(d).tab_stops(d)?.remove(0);
            tab.set_position(d, 3 * INCH).map(drop)
        }),
        ("TabStop::set_alignment", false, |d| {
            let tab = para(d).tab_stops(d)?.remove(0);
            tab.set_alignment(d, "center")
        }),
        ("TabStop::set_leader", false, |d| {
            let tab = para(d).tab_stops(d)?.remove(0);
            tab.set_leader(d, Some("dot"))
        }),
        ("Run::set_text", false, |d| {
            para(d).runs(d)?[0].set_text(d, "x")
        }),
        ("Run::add_text", false, |d| {
            para(d).runs(d)?[0].add_text(d, "x")
        }),
        ("Run::add_tab", false, |d| para(d).runs(d)?[0].add_tab(d)),
        ("Run::add_break", false, |d| {
            para(d).runs(d)?[0].add_break(d, BreakType::Line)
        }),
        ("Run::clear", false, |d| para(d).runs(d)?[0].clear(d)),
        ("Run::set_style", false, |d| {
            para(d).runs(d)?[0].set_style(d, None)
        }),
        ("Run::set_font", false, |d| {
            para(d).runs(d)?[0].set_font(d, &[FontChange::Size(Some(12 * 12_700))])
        }),
        ("Run::set_bold", false, |d| {
            para(d).runs(d)?[0].set_bold(d, Some(true))
        }),
        ("Run::add_picture", false, |d| {
            para(d).runs(d)?[0]
                .add_picture(d, &png(), None, None)
                .map(drop)
        }),
        ("Run::add_picture_file", false, |d| {
            para(d).runs(d)?[0]
                .add_picture_file(d, picture_path(), None, None)
                .map(drop)
        }),
        ("Run::mark_comment_range", true, |d| {
            let runs = para(d).runs(d)?;
            runs[0].mark_comment_range(d, &runs[1], 7)
        }),
        ("Table::add_row", false, |d| table(d).add_row(d)),
        ("Table::add_column", false, |d| table(d).add_column(d, INCH)),
        ("Table::set_style", false, |d| table(d).set_style(d, None)),
        ("Table::set_alignment", false, |d| {
            table(d).set_alignment(d, Some("center"))
        }),
        ("Table::set_autofit", false, |d| {
            table(d).set_autofit(d, false)
        }),
        ("Table::set_table_direction", false, |d| {
            table(d).set_table_direction(d, Some(TableDirection::Rtl))
        }),
        ("Row::set_height", false, |d| {
            table(d).rows(d)?[0].set_height(d, Some(INCH))
        }),
        ("Row::set_height_rule", false, |d| {
            table(d).rows(d)?[0].set_height_rule(d, Some("exact"))
        }),
        ("Column::set_width", false, |d| {
            table(d).columns(d)?[0].set_width(d, Some(INCH))
        }),
        ("Cell::set_text", false, |d| {
            table(d).cell(d, 0, 0)?.set_text(d, "x")
        }),
        ("Cell::set_width", false, |d| {
            table(d).cell(d, 0, 0)?.set_width(d, INCH)
        }),
        ("Cell::set_vertical_alignment", false, |d| {
            table(d)
                .cell(d, 0, 0)?
                .set_vertical_alignment(d, Some("bottom"))
        }),
        ("Cell::add_paragraph", false, |d| {
            table(d)
                .cell(d, 0, 0)?
                .add_paragraph(d, "x", None)
                .map(drop)
        }),
        ("Cell::add_table", false, |d| {
            table(d).cell(d, 0, 0)?.add_table(d, 1, 1).map(drop)
        }),
        ("Cell::merge", false, |d| {
            let t = table(d);
            let other = t.cell(d, 0, 1)?;
            t.cell(d, 0, 0)?.merge(d, &other).map(drop)
        }),
        ("InlineShapeRef::set_width", false, |d| {
            d.inline_shapes()?[0].set_width(d, INCH)
        }),
        ("InlineShapeRef::set_height", false, |d| {
            d.inline_shapes()?[0].set_height(d, INCH)
        }),
        ("Section::set", false, |d| {
            d.sections()?[0].set(d, &[SectionChange::Gutter(Some(INCH))])
        }),
        (
            "Document::set_odd_and_even_pages_header_footer",
            false,
            |d| d.set_odd_and_even_pages_header_footer(true),
        ),
        ("HeaderFooter::set_linked_to_previous", false, |d| {
            d.sections()?[0].header().set_linked_to_previous(d, true)
        }),
        ("HeaderFooter::definition", false, |d| {
            d.sections()?[0].footer().definition(d).map(drop)
        }),
        ("BlockContainer::add_paragraph", false, |d| {
            let header = d.sections()?[0].header().existing_definition(d)?.unwrap();
            header.add_paragraph(d, "x", None).map(drop)
        }),
        ("BlockContainer::add_table", false, |d| {
            let header = d.sections()?[0].header().existing_definition(d)?.unwrap();
            header.add_table(d, 1, 1, INCH).map(drop)
        }),
        ("Document::add_style", false, |d| {
            d.add_style("Another", StyleType::Character, false)
                .map(drop)
        }),
        ("StyleRef::delete", false, |d| {
            d.style("Gate Style")?.delete(d)
        }),
        ("StyleRef::set", false, |d| {
            d.style("Gate Style")?.set(d, &[StyleChange::Hidden(true)])
        }),
        ("StyleRef::set_font", false, |d| {
            d.style("Gate Style")?
                .set_font(d, &[FontChange::Name(Some("Arial".into()))])
        }),
        ("StyleRef::set_paragraph_format", false, |d| {
            let format = ParagraphFormat {
                left_indent: Some(Some(INCH)),
                ..ParagraphFormat::default()
            };
            d.style("Gate Style")?.set_paragraph_format(d, &format)
        }),
        ("Document::add_latent_style", false, |d| {
            d.add_latent_style("Another").map(drop)
        }),
        ("Document::set_latent_defaults", false, |d| {
            d.set_latent_defaults(&[LatentDefaultsChange::DefaultToLocked(true)])
        }),
        ("LatentStyleRef::delete", false, |d| {
            d.latent_style("Gate Latent")?.delete(d)
        }),
        ("LatentStyleRef::set", false, |d| {
            d.latent_style("Gate Latent")?
                .set(d, &[LatentStyleChange::Locked(Some(true))])
        }),
        ("Document::set_core_properties", false, |d| {
            let change =
                CorePropertyChange::new(CoreProperty::Title, CoreValue::Text("Gated".into()));
            d.set_core_properties(&[change], None).map(drop)
        }),
    ]
}

#[test]
fn every_inherited_writer_honours_the_protection_matrix() {
    for (edit, comments_allowed, body_allowed) in [
        ("readOnly", false, false),
        ("comments", true, false),
        ("forms", false, false),
        ("trackedChanges", true, true),
    ] {
        let bytes = protected(edit);
        for (name, comment, writer) in writers() {
            let allowed = if comment {
                comments_allowed
            } else {
                body_allowed
            };
            let mut doc = open(&bytes);
            let result = writer(&mut doc);
            if allowed {
                result.unwrap_or_else(|err| panic!("{name} under {edit}: {err}"));
                continue;
            }
            let err = result.expect_err(&format!("{name} under {edit} must refuse"));
            assert_eq!(
                err.kind(),
                ErrorKind::Refusal(RefusalKind::DocumentProtected),
                "{name} under {edit}: {err}"
            );
            assert_eq!(err.reason(), "restrict_editing", "{name}");
            assert_eq!(doc.patch_bytes().unwrap().0, bytes, "{name} changed bytes");

            let mut doc = open(&bytes);
            doc.acknowledge_protection().unwrap();
            writer(&mut doc).unwrap_or_else(|err| panic!("{name} acknowledged: {err}"));
            assert!(doc.protection_status().unwrap().enforced, "{name}");
        }
    }
}

#[test]
fn ensure_styles_part_refuses_only_when_it_would_create_the_part() {
    let mut doc = Document::new();
    doc.set_protection("readOnly").unwrap();
    let bytes = doc.patch_bytes().unwrap().0;
    let mut doc = open(&bytes);
    assert_eq!(doc.ensure_styles_part().unwrap(), "word/styles.xml");
    assert_eq!(doc.patch_bytes().unwrap().0, bytes);

    let bytes = package_with_settings(
        DOCUMENT_XML,
        r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:documentProtection w:edit="readOnly" w:enforcement="1"/></w:settings>"#,
    );
    let mut bare = open(&bytes);
    let err = bare.ensure_styles_part().unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::DocumentProtected)
    );
    assert_eq!(bare.patch_bytes().unwrap().0, bytes);
    bare.acknowledge_protection().unwrap();
    bare.ensure_styles_part().unwrap();
}
