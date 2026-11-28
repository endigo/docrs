//! Footnotes and endnotes through model and pagination.

use super::flow::Sink;
use super::model::{NoteId, NoteKind};
use super::*;
use crate::package::ct;
use crate::txn::RelTarget;
use crate::wml::{R, W};
use crate::xml::parse_str;

/// Arial 11 body text (12.65 points a line), Arial 10 notes (11.5).
const STYLES: &str = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:rPr><w:sz w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#;

/// A 216-point page with half-inch margins: a body from 36 to 180.
const SECT: &str = r#"<w:sectPr>{footer}<w:pgSz w:w="12240" w:h="4320"/><w:pgMar w:top="720" w:right="1440" w:bottom="720" w:left="1440" w:header="0" w:footer="0" w:gutter="0"/></w:sectPr>"#;
const BODY_BOTTOM: f64 = 180.0;

const SEPARATORS: &str = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#;

fn para(text: &str) -> String {
    format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
}

fn reference(local: &str, id: u32) -> String {
    format!(
        r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:{local} w:id="{id}"/></w:r>"#
    )
}

/// A paragraph ending in references to these footnotes.
fn cited(text: &str, ids: &[u32]) -> String {
    let refs: String = ids
        .iter()
        .map(|id| reference("footnoteReference", *id))
        .collect();
    format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r>{refs}</w:p>"#)
}

fn note_para(text: &str, mark: Option<&str>) -> String {
    let mark = mark.map_or(String::new(), |local| {
        format!(r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:{local}/></w:r>"#)
    });
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>{mark}<w:r><w:t xml:space="preserve"> {text}</w:t></w:r></w:p>"#
    )
}

/// A footnote of these paragraphs, the first with the note's mark.
fn footnote(id: u32, paras: &[&str]) -> String {
    let body: String = paras
        .iter()
        .enumerate()
        .map(|(i, text)| note_para(text, (i == 0).then_some("footnoteRef")))
        .collect();
    format!(r#"<w:footnote w:id="{id}">{body}</w:footnote>"#)
}

struct Parts<'a> {
    body: &'a str,
    footnotes: &'a str,
    endnotes: &'a str,
    settings: &'a str,
    /// The default footer's paragraphs.
    footer: &'a str,
}

const NONE: Parts<'static> = Parts {
    body: "",
    footnotes: "",
    endnotes: "",
    settings: "",
    footer: "",
};

fn replace(txn: &mut Txn<'_>, rel: &str, name: &str, content_type: &str, xml: String) {
    let part = crate::parts::ensure_part(txn, rel, name, content_type, xml.as_bytes()).unwrap();
    *txn.tree_mut(&part).unwrap() = parse_str(&xml).unwrap();
}

fn document(parts: Parts<'_>) -> Document {
    let mut doc = Document::new();
    let mut txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    let mut footer = String::new();
    if !parts.footer.is_empty() {
        let tree = parse_str(&format!("<w:ftr xmlns:w=\"{W}\">{}</w:ftr>", parts.footer)).unwrap();
        txn.add_xml_part("word/footer1.xml", ct::WML_FOOTER, tree)
            .unwrap();
        let id = txn
            .relate_to(&main, rt::FOOTER, RelTarget::Part("word/footer1.xml"))
            .unwrap();
        footer = format!(r#"<w:footerReference w:type="default" r:id="{id}"/>"#);
    }
    *txn.tree_mut(&main).unwrap() = parse_str(&format!(
        "<w:document xmlns:w=\"{W}\" xmlns:r=\"{R}\"><w:body>{}{}</w:body></w:document>",
        parts.body,
        SECT.replace("{footer}", &footer)
    ))
    .unwrap();
    let styles = txn.part_related_by(&main, rt::STYLES).unwrap().unwrap();
    *txn.tree_mut(&styles).unwrap() =
        parse_str(&format!("<w:styles xmlns:w=\"{W}\">{STYLES}</w:styles>")).unwrap();
    replace(
        &mut txn,
        rt::SETTINGS,
        "word/settings.xml",
        ct::WML_SETTINGS,
        format!(
            "<w:settings xmlns:w=\"{W}\">{}</w:settings>",
            parts.settings
        ),
    );
    replace(
        &mut txn,
        rt::FOOTNOTES,
        "word/footnotes.xml",
        ct::WML_FOOTNOTES,
        format!(
            "<w:footnotes xmlns:w=\"{W}\">{SEPARATORS}{}</w:footnotes>",
            parts.footnotes
        ),
    );
    if !parts.endnotes.is_empty() {
        let separators = SEPARATORS.replace("w:footnote", "w:endnote");
        replace(
            &mut txn,
            rt::ENDNOTES,
            "word/endnotes.xml",
            ct::WML_ENDNOTES,
            format!(
                "<w:endnotes xmlns:w=\"{W}\">{separators}{}</w:endnotes>",
                parts.endnotes
            ),
        );
    }
    let staged = txn.finish().unwrap();
    doc.commit(staged).unwrap();
    doc
}

fn records(doc: &Document) -> Vec<LineRecord> {
    let txn = Txn::new(doc);
    let main = doc.package().main_part_name().to_string();
    let mut out = Vec::new();
    line_records(
        &txn,
        &main,
        Vec::new(),
        |_, _| Ok(()),
        |line| {
            out.push(line);
            Ok(())
        },
    )
    .unwrap();
    out
}

fn of_story<'r>(records: &'r [LineRecord], story: &str) -> Vec<&'r LineRecord> {
    records.iter().filter(|r| r.story == story).collect()
}

fn find<'r>(records: &'r [LineRecord], text: &str) -> &'r LineRecord {
    records
        .iter()
        .find(|r| r.text == text)
        .unwrap_or_else(|| panic!("no line {text:?} in {records:#?}"))
}

/// One-line paragraphs filling `count` lines.
fn filler(count: usize) -> String {
    (1..=count).map(|n| para(&format!("Line {n}"))).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn a_note_that_fits_sits_on_the_body_bottom_below_its_separator() {
    let doc = document(Parts {
        body: &cited("Text", &[1]),
        footnotes: &footnote(1, &["Note one."]),
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "Text1").page, 1);
    let notes = of_story(&records, "footnotes");
    let [separator, note] = notes[..] else {
        panic!("{notes:#?}")
    };
    assert_eq!(separator.text, "");
    assert_eq!(note.text, "1 Note one.");
    assert!(close(note.y + note.height, BODY_BOTTOM), "{note:?}");
    assert!(
        close(separator.y + separator.height, note.y),
        "{separator:?}"
    );
    let layout = estimate(&doc);
    assert_eq!(layout.pages, 1);
    assert_eq!(
        layout.placed.len(),
        1,
        "note paragraphs are not body paragraphs"
    );
}

fn estimate(doc: &Document) -> Layout {
    let txn = Txn::new(doc);
    layout(&txn, doc.package().main_part_name(), Vec::new()).unwrap()
}

#[test]
fn a_reference_line_that_fits_without_its_note_moves_with_it() {
    // Ten lines leave 17.5 points: room for the line, not for the
    // separator and the note too.
    let body = filler(10) + &cited("Cited", &[1]);
    let doc = document(Parts {
        body: &body,
        footnotes: &footnote(1, &["The note."]),
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "Line 10").page, 1);
    assert_eq!(find(&records, "Cited1").page, 2);
    let notes = of_story(&records, "footnotes");
    assert!(notes.iter().all(|n| n.page == 2), "{notes:#?}");
    assert_eq!(notes.len(), 2);
}

#[test]
fn a_long_note_goes_on_under_the_continuation_separator_each_line_once() {
    let lines: Vec<String> = (1..=30).map(|n| format!("Note line {n}.")).collect();
    let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
    let body = cited("Cited", &[1]) + &para("After");
    let doc = document(Parts {
        body: &body,
        footnotes: &footnote(1, &texts),
        ..NONE
    });
    let records = records(&doc);
    let notes = of_story(&records, "footnotes");
    let shown: Vec<&str> = notes
        .iter()
        .map(|n| n.text.as_str())
        .filter(|t| !t.is_empty())
        .collect();
    let mut expected = vec!["1 Note line 1.".to_string()];
    expected.extend((2..=30).map(|n| format!(" Note line {n}.")));
    assert_eq!(shown, expected, "every note line once, in order");
    let pages: Vec<u32> = notes.iter().map(|n| n.page).collect();
    assert_eq!((pages[0], *pages.last().unwrap()), (1, 3), "{pages:?}");
    for page in [2, 3] {
        let first = notes.iter().find(|n| n.page == page).unwrap();
        assert_eq!(
            first.text, "",
            "page {page} opens with the continuation separator"
        );
    }
    // Page 2 holds only the note; the body goes on above its end on page 3.
    assert!(of_story(&records, "body").iter().all(|r| r.page != 2));
    let after = find(&records, "After");
    let last = notes.last().unwrap();
    assert_eq!(after.page, 3);
    assert!(after.y + after.height <= notes.iter().find(|n| n.page == 3).unwrap().y + 0.01);
    assert!(close(last.y + last.height, BODY_BOTTOM));
    assert_eq!(estimate(&doc).pages, 3);
}

#[test]
fn two_references_on_one_line_take_both_notes_in_order() {
    let body = format!(
        r#"<w:p><w:r><w:t>A</w:t></w:r>{}<w:r><w:t xml:space="preserve"> B</w:t></w:r>{}</w:p>"#,
        reference("footnoteReference", 7),
        reference("footnoteReference", 3),
    );
    let doc = document(Parts {
        body: &body,
        footnotes: &(footnote(3, &["Three."]) + &footnote(7, &["Seven."])),
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "A1 B2").page, 1);
    let notes: Vec<&str> = of_story(&records, "footnotes")
        .iter()
        .map(|n| n.text.as_str())
        .collect();
    assert_eq!(notes, ["", "1 Seven.", "2 Three."]);
}

fn table(rows: &[String]) -> String {
    let rows: String = rows
        .iter()
        .map(|cell| format!("<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>{cell}</w:tc></w:tr>"))
        .collect();
    format!("<w:tbl><w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid>{rows}</w:tbl>")
}

#[test]
fn a_reference_in_a_table_cell_reserves_on_the_page_its_row_lands() {
    let body = filler(9) + &table(&[para("Plain"), cited("Cell", &[1])]) + &para("");
    let doc = document(Parts {
        body: &body,
        footnotes: &footnote(1, &["From a cell."]),
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "Plain").page, 1);
    assert_eq!(find(&records, "Cell1").page, 2);
    let note = find(&records, "1 From a cell.");
    assert_eq!(note.page, 2);
    assert!(close(note.y + note.height, BODY_BOTTOM));
}

#[test]
fn endnotes_follow_the_last_block_numbered_in_roman() {
    let body = format!(
        r#"<w:p><w:r><w:t>End</w:t></w:r>{}</w:p>"#,
        reference("endnoteReference", 1)
    );
    let endnote = format!(
        r#"<w:endnote w:id="1">{}</w:endnote>"#,
        note_para("An endnote.", Some("endnoteRef"))
    );
    let doc = document(Parts {
        body: &body,
        endnotes: &endnote,
        ..NONE
    });
    let records = records(&doc);
    let cited = find(&records, "Endi");
    let endnotes = of_story(&records, "endnotes");
    let [separator, note] = endnotes[..] else {
        panic!("{endnotes:#?}")
    };
    assert_eq!(note.text, "i An endnote.");
    assert!(close(separator.y, cited.y + cited.height));
    assert!(close(note.y, separator.y + separator.height));
    assert!(of_story(&records, "footnotes").is_empty());
    assert_eq!(estimate(&doc).placed.len(), 1);
}

#[test]
fn footnote_numbers_restart_on_each_page_when_settings_say_so() {
    let body =
        cited("One", &[1]) + r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"# + &cited("Two", &[2]);
    let doc = document(Parts {
        body: &body,
        footnotes: &(footnote(1, &["First."]) + &footnote(2, &["Second."])),
        settings: r#"<w:footnotePr><w:numFmt w:val="lowerLetter"/><w:numRestart w:val="eachPage"/></w:footnotePr>"#,
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "Onea").page, 1);
    assert_eq!(find(&records, "Twoa").page, 2);
    assert_eq!(find(&records, "a First.").page, 1);
    assert_eq!(find(&records, "a Second.").page, 2);
}

#[test]
fn a_custom_mark_is_its_own_text_and_takes_no_number() {
    let body = format!(
        r#"<w:p><w:r><w:t>A</w:t></w:r><w:r><w:footnoteReference w:customMarkFollows="1" w:id="1"/><w:t>*</w:t></w:r>{}</w:p>"#,
        reference("footnoteReference", 2)
    );
    let custom = r#"<w:footnote w:id="1"><w:p><w:r><w:t>* Custom.</w:t></w:r></w:p></w:footnote>"#;
    let doc = document(Parts {
        body: &body,
        footnotes: &(custom.to_string() + &footnote(2, &["Numbered."])),
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "A*1").page, 1);
    find(&records, "* Custom.");
    find(&records, "1 Numbered.");
}

#[derive(Default)]
struct Unplaced(Vec<NoteId>);

impl Sink for Unplaced {
    fn note_unplaced(&mut self, note: NoteId) -> Result<()> {
        self.0.push(note);
        Ok(())
    }
}

fn paginate_into<S: Sink>(doc: &Document, sink: &mut S) -> Result<()> {
    let txn = Txn::new(doc);
    settle(&txn, doc.package().main_part_name(), &FontSet::default())?.paginate(sink)
}

#[test]
fn a_note_whose_table_goes_on_to_the_next_page_is_reported_and_laid_out() {
    let rows: Vec<String> = (1..=20).map(|n| para(&format!("Row {n}"))).collect();
    let note = format!(
        r#"<w:footnote w:id="1">{}{}</w:footnote>"#,
        note_para("Table:", Some("footnoteRef")),
        table(&rows)
    );
    let doc = document(Parts {
        body: &cited("Cited", &[1]),
        footnotes: &note,
        ..NONE
    });
    let mut sink = Unplaced::default();
    paginate_into(&doc, &mut sink).unwrap();
    let note = NoteId {
        kind: NoteKind::Footnote,
        index: 0,
    };
    assert_eq!(sink.0, [note], "reported once");
    let records = records(&doc);
    for n in 1..=20 {
        find(&records, &format!("Row {n}"));
    }
}

#[test]
fn a_note_taller_than_the_page_still_ends() {
    let huge = r#"<w:footnote w:id="1"><w:p><w:r><w:rPr><w:sz w:val="300"/></w:rPr><w:t>Huge</w:t></w:r></w:p><w:p><w:r><w:t>After the huge line.</w:t></w:r></w:p></w:footnote>"#;
    let doc = document(Parts {
        body: &cited("Cited", &[1]),
        footnotes: huge,
        ..NONE
    });
    let records = records(&doc);
    let huge = find(&records, "Huge");
    assert_eq!(huge.page, 2);
    assert!(
        huge.height > BODY_BOTTOM - 36.0 && huge.y > 36.0,
        "{huge:?}"
    );
    assert_eq!(find(&records, "After the huge line.").page, 3);
}

#[test]
fn notes_count_toward_the_work_budget() {
    let lines: Vec<String> = (1..=40).map(|n| format!("Note line {n}.")).collect();
    let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
    let notes: String = (1..=300).map(|id| footnote(id, &texts)).collect();
    let body: String = (1..=300).map(|id| cited("Cited", &[id])).collect();
    let doc = document(Parts {
        body: &body,
        footnotes: &notes,
        ..NONE
    });
    let pages = estimate(&doc).pages;
    assert!(pages > 300, "{pages}");
    TEST_LAYOUT_WORK.set(Some(20_000));
    let error = paginate_into(&doc, &mut Vec::new()).unwrap_err();
    TEST_LAYOUT_WORK.set(None);
    assert_eq!(error.reason(), "max_layout_work");
}

#[test]
fn notes_sit_on_the_body_bottom_a_tall_footer_raises() {
    // Four footer lines (50.6 pt) pass the 36 pt margin: the body ends at
    // 165.4, and the note with it.
    let doc = document(Parts {
        body: &cited("Text", &[1]),
        footnotes: &footnote(1, &["Above the footer."]),
        footer: &filler(4),
        ..NONE
    });
    let records = records(&doc);
    let note = find(&records, "1 Above the footer.");
    let footer = of_story(&records, "footer1");
    assert_eq!(footer.len(), 4);
    assert!(
        close(note.y + note.height, footer[0].y),
        "{note:?} {footer:?}"
    );
}

fn heading(bookmark: &str, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:keepNext/></w:pPr><w:bookmarkStart w:id="0" w:name="{bookmark}"/><w:r><w:t>{text}</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#
    )
}

#[test]
fn notes_move_the_pages_bookmarks_report() {
    // Without its note the cited line and the bookmark after it fit on
    // page 1.
    let body = filler(9) + &cited("Cited", &[1]) + &heading("After", "After");
    let with_note = document(Parts {
        body: &body,
        footnotes: &footnote(1, &["A note.", "Two lines."]),
        ..NONE
    });
    assert_eq!(estimate(&with_note).bookmark_page("After"), Some(2));
    let without = document(Parts {
        body: &(filler(9) + &para("Cited") + &heading("After", "After")),
        ..NONE
    });
    assert_eq!(estimate(&without).bookmark_page("After"), Some(1));
}

#[test]
fn keep_with_next_does_not_count_notes_yet() {
    // The heading and the cited line fit together without the note, so
    // the heading stays when the line moves on with its note. Word's
    // choice here is not measured yet.
    let body = filler(8) + &heading("Kept", "Heading") + &cited("Cited", &[1]);
    let doc = document(Parts {
        body: &body,
        footnotes: &footnote(1, &["The note."]),
        ..NONE
    });
    let records = records(&doc);
    assert_eq!(find(&records, "Heading").page, 1);
    assert_eq!(find(&records, "Cited1").page, 2);
}

/// One cell of eight one-line paragraphs with 18 points after each, the
/// one at `cited` (from 1) citing `note`, after one body line: the cell's
/// lines end 12.65, 43.3, 73.95, 104.6 and 135.25 points below its top,
/// so four fit on the page, leaving 26.75 points.
fn spaced_cell(cited: usize, note: &str) -> Vec<LineRecord> {
    let spaced = |text: &str, refs: &str| {
        format!(
            r#"<w:p><w:pPr><w:spacing w:before="0" w:after="360"/></w:pPr><w:r><w:t>{text}</w:t></w:r>{refs}</w:p>"#
        )
    };
    let cell: String = (1..=8)
        .map(|n| match n == cited {
            true => spaced("Cited", &reference("footnoteReference", 1)),
            false => spaced(&format!("Row line {n}"), ""),
        })
        .collect();
    let doc = document(Parts {
        body: &(filler(1) + &table(&[cell]) + &para("")),
        footnotes: note,
        ..NONE
    });
    records(&doc)
}

/// Footnote 1: three paragraphs of `size` half-points.
fn sized_note(size: u32) -> String {
    let lines: String = (1..=3)
        .map(|n| {
            format!(
                r#"<w:p><w:r><w:rPr><w:sz w:val="{size}"/></w:rPr><w:t>Note line {n}.</w:t></w:r></w:p>"#
            )
        })
        .collect();
    format!(r#"<w:footnote w:id="1">{lines}</w:footnote>"#)
}

/// The page of each of the cell's lines, the cited one as `0`.
fn cell_pages(records: &[LineRecord]) -> Vec<(u32, u32)> {
    records
        .iter()
        .filter_map(|r| {
            let line = match r.text.as_str() {
                "Cited1" => 0,
                text => text.strip_prefix("Row line ")?.parse().ok()?,
            };
            Some((line, r.page))
        })
        .collect()
}

#[test]
fn a_splitting_cell_keeps_its_lines_when_their_note_fits() {
    // The separator and the one-line note (24.15) fit under four lines.
    let records = spaced_cell(2, &footnote(1, &["Note line 1."]));
    let pages = cell_pages(&records);
    assert_eq!(pages[..5], [(1, 1), (0, 1), (3, 1), (4, 1), (5, 2)]);
    assert_eq!(find(&records, "1 Note line 1.").page, 1);
}

#[test]
fn a_splitting_cell_gives_up_its_cited_last_line_with_its_note() {
    // A 16.1-point note line does not fit under the separator below four
    // lines: one retry drops the fourth, which cites it.
    let records = spaced_cell(4, &sized_note(28));
    let pages = cell_pages(&records);
    assert_eq!(pages[..4], [(1, 1), (2, 1), (3, 1), (0, 2)]);
    assert_eq!(find(&records, "Note line 1.").page, 2);
    assert!(of_story(&records, "footnotes").iter().all(|n| n.page != 1));
}

#[test]
fn a_splitting_cell_gives_up_lines_until_its_note_starts() {
    // A 46-point note line fits under two lines, not three or four: two
    // retries, and the cited second line stays with the note's start.
    let records = spaced_cell(2, &sized_note(80));
    let pages = cell_pages(&records);
    assert_eq!(pages[..3], [(1, 1), (0, 1), (3, 2)]);
    let first = find(&records, "Note line 1.");
    assert_eq!(first.page, 1);
    assert!(close(first.y + first.height, BODY_BOTTOM), "{first:?}");
}

#[test]
fn a_splitting_row_ends_at_the_line_whose_note_goes_on() {
    // As Word lays out pr68-flag.docx (pr-proof/pr68/word): the cell's
    // lines below the citing line wait for the page after the note's
    // continuation, and the cell beside keeps its line.
    let cell = |content: String| {
        format!(r#"<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr>{content}</w:tc>"#)
    };
    let lines: String = (0..8)
        .map(|n| match n {
            2 => cited("cell line 2", &[1]),
            n => para(&format!("cell line {n}")),
        })
        .collect();
    let row = format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr>{}{}</w:tr></w:tbl>"#,
        cell(lines),
        cell(para("side"))
    );
    let note: Vec<String> = (0..20).map(|n| format!("Note line {n}.")).collect();
    let note: Vec<&str> = note.iter().map(String::as_str).collect();
    let doc = document(Parts {
        body: &(filler(2) + &row + &para("after")),
        footnotes: &footnote(1, &note),
        ..NONE
    });
    let records = records(&doc);
    let page = |text: &str| find(&records, text).page;
    assert_eq!(
        (0..8)
            .map(|n| page(&format!("cell line {n}{}", if n == 2 { "1" } else { "" })))
            .collect::<Vec<_>>(),
        [1, 1, 1, 3, 3, 3, 3, 3]
    );
    assert_eq!((page("side"), page("after")), (1, 3));
    assert_eq!(page("1 Note line 0."), 1);
    assert!(
        records
            .iter()
            .all(|r| r.page != 2 || r.story == "footnotes")
    );
}

#[test]
fn below_mode_15_a_separator_s_spacing_counts_only_above_a_note_going_on() {
    let spaced = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="240"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote>"#;
    let tops = |settings: &str, note_lines: usize| {
        let lines: Vec<String> = (1..=note_lines)
            .map(|n| format!("Note line {n}."))
            .collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let doc = document(Parts {
            body: &(filler(5) + &cited("Cited", &[1])),
            footnotes: &(spaced.to_string() + &footnote(1, &lines)),
            settings,
            ..NONE
        });
        let records = records(&doc);
        let notes = of_story(&records, "footnotes");
        (notes[0].page, notes[0].y, notes[1].y)
    };
    let mode_15 = r#"<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat>"#;
    // A whole one-line note ends at 180: its 12.65-point separator line
    // sits above it, with its 12 points after only in mode 15.
    let (_, separator, note) = tops("", 1);
    assert!(
        close(note, 168.5) && close(separator, 155.85),
        "{separator} {note}"
    );
    let (_, separator, note) = tops(mode_15, 1);
    assert!(
        close(note, 168.5) && close(separator, 143.85),
        "{separator} {note}"
    );
    // Above a note going on, the separator's spacing counts in either
    // mode.
    for settings in ["", mode_15] {
        let (page, separator, note) = tops(settings, 20);
        assert_eq!(page, 1);
        assert!(close(note - separator, 24.65), "{separator} {note}");
    }
}
