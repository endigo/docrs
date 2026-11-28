//! Sections: page setup, starts and page numbering per section.

use super::story_tests::{
    Doc, LINES_PER_PAGE, PAGE, body, break_para, field, lines, para, para_with, run, sect,
};
use super::*;

/// Letter paper turned, 1-inch margins: 39 body lines a page.
const LANDSCAPE: &str = r#"<w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>"#;
const LANDSCAPE_LINES: usize = 39;

/// `sect`'s properties on `page` instead of portrait Letter.
fn sect_on(page: &str, refs: &[&str], extra: &str) -> String {
    sect(refs, extra).replace(PAGE, page)
}

fn start(kind: &str) -> String {
    format!(r#"<w:type w:val="{kind}"/>"#)
}

fn footer_doc(content: String, last: String) -> Doc<'static> {
    let footer = para(&field("PAGE", "1"));
    Doc {
        body: body(content, last),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    }
}

/// Each body line: page, left edge, width, text.
fn body_lines(doc: &Doc<'_>) -> Vec<(u32, f64, f64, String)> {
    let mut out = Vec::new();
    doc.with(|txn, main| {
        records_with(
            txn,
            main,
            &FontSet::default(),
            |_, _| Ok(()),
            |record| {
                if let Record::Line(line) = record
                    && line.story == "body"
                {
                    out.push((line.page, line.x, line.width, line.text));
                }
                Ok(())
            },
        )
    })
    .unwrap();
    out
}

fn lines_on(lines: &[(u32, f64, f64, String)], page: u32) -> usize {
    lines.iter().filter(|l| l.0 == page).count()
}

fn words(count: usize) -> String {
    para(&run(&vec!["word"; count].join(" ")))
}

#[test]
fn a_landscape_section_between_portrait_ones_has_its_own_paper() {
    let content = [
        words(120),
        break_para(&sect(&["footer:default:f1"], "")),
        words(120),
        lines(LANDSCAPE_LINES),
        break_para(&sect_on(LANDSCAPE, &[], "")),
        words(120),
    ]
    .concat();
    let doc = footer_doc(content, sect(&[], ""));
    let lines = body_lines(&doc);
    let widest = |page| {
        lines
            .iter()
            .filter(|l| l.0 == page)
            .map(|l| l.2)
            .fold(0.0, f64::max)
    };
    assert!(widest(1) <= 468.0 && widest(1) > 400.0, "{}", widest(1));
    assert!(widest(2) > 600.0 && widest(2) <= 648.0, "{}", widest(2));
    assert!(widest(4) <= 468.0, "{}", widest(4));
    assert_eq!(lines_on(&lines, 2), LANDSCAPE_LINES);
    assert!(lines.iter().all(|l| l.1 == 72.0));
    let settled = doc.settle().unwrap();
    assert_eq!(settled.layout.pages, 4);
    assert_eq!(
        doc.story_text("footer"),
        ["1", "2", "3", "4"],
        "the landscape pages inherit the footer"
    );
}

#[test]
fn roman_front_matter_is_followed_by_decimal_pages_from_1() {
    let footer = para(
        &[
            field("PAGE", "1"),
            run(" of "),
            field("SECTIONPAGES", "1"),
            run(" ("),
            field("NUMPAGES", "1"),
            run(")"),
        ]
        .concat(),
    );
    let front = sect(
        &["footer:default:f1"],
        r#"<w:pgNumType w:fmt="lowerRoman"/>"#,
    );
    let content = [lines(2 * LINES_PER_PAGE + 1), break_para(&front)].concat();
    let doc = Doc {
        body: body(
            content + &lines(LINES_PER_PAGE + 1),
            sect(&[], r#"<w:pgNumType w:start="1"/>"#),
        ),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    };
    assert_eq!(
        doc.story_text("footer"),
        [
            "i of 3 (5)",
            "ii of 3 (5)",
            "iii of 3 (5)",
            "1 of 2 (5)",
            "2 of 2 (5)"
        ]
    );
    let layout = doc.settle().unwrap().layout;
    let labels: Vec<_> = (1..=6).map(|page| layout.page_label(page)).collect();
    let shown = ["i", "ii", "iii", "1", "2"].map(|s| Some(s.to_string()));
    assert_eq!(labels[..5], shown);
    assert_eq!(labels[5], None);
    assert_eq!(layout.page_number(4), 4);
}

/// One page in a first section, then `kind`'s section with `numbering`:
/// each page's footer and body text.
fn second_section_starting(kind: &str, numbering: &str) -> Vec<(String, String)> {
    let first = sect(&["footer:default:f1"], "");
    let content = [para(&run("First")), break_para(&first)].concat();
    let doc = footer_doc(
        content + &para(&run("Second")),
        sect(&[], &(start(kind) + numbering)),
    );
    let lines = body_lines(&doc);
    doc.story_text("footer")
        .into_iter()
        .enumerate()
        .map(|(index, footer)| {
            let page = index as u32 + 1;
            let text: Vec<_> = lines.iter().filter(|l| l.0 == page).map(|l| &l.3).collect();
            (footer, text.iter().map(|t| t.as_str()).collect::<String>())
        })
        .collect()
}

fn pages(pages: &[(&str, &str)]) -> Vec<(String, String)> {
    pages
        .iter()
        .map(|(footer, text)| (footer.to_string(), text.to_string()))
        .collect()
}

#[test]
fn an_odd_or_even_start_inserts_a_blank_page_when_the_number_needs_it() {
    assert_eq!(
        second_section_starting("oddPage", ""),
        pages(&[("1", "First"), ("", ""), ("3", "Second")]),
        "Word draws nothing on the blank page"
    );
    assert_eq!(
        second_section_starting("evenPage", ""),
        pages(&[("1", "First"), ("2", "Second")])
    );
    assert_eq!(
        second_section_starting("oddPage", r#"<w:pgNumType w:start="1"/>"#),
        pages(&[("1", "First"), ("1", "Second")]),
        "parity is the shown number's"
    );
    assert_eq!(
        second_section_starting("evenPage", r#"<w:pgNumType w:start="1"/>"#),
        pages(&[("1", "First"), ("", ""), ("1", "Second")])
    );
    assert_eq!(
        second_section_starting("nextColumn", ""),
        pages(&[("1", "First"), ("2", "Second")])
    );
}

#[test]
fn a_continuous_section_on_the_same_paper_stays_on_the_page() {
    let narrow = PAGE.replace(r#"w:left="1440""#, r#"w:left="2880""#);
    let content = [para(&run("First")), break_para(&sect(&[], ""))].concat();
    let doc = footer_doc(
        content + &para(&run("Second")),
        sect_on(&narrow, &[], &start("continuous")),
    );
    let lines = body_lines(&doc);
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|l| l.0 == 1));
    assert_eq!((lines[0].1, lines[2].1), (72.0, 144.0));
    assert!(doc.settle().unwrap().warnings.is_empty());
}

/// Each page's size, from the section `paginate` opens it in.
struct Paper<'m>(&'m [model::Section], Vec<(f64, f64)>);

impl flow::Sink for Paper<'_> {
    fn page(&mut self, page: &flow::PageStart) -> Result<()> {
        let geometry = &self.0[page.section].page;
        self.1.push((geometry.width, geometry.height));
        Ok(())
    }
}

fn papers(doc: &Doc<'_>) -> Vec<(f64, f64)> {
    let settled = doc.settle().unwrap();
    let mut paper = Paper(&settled.model().sections, Vec::new());
    settled.paginate(&mut paper).unwrap();
    paper.1
}

#[test]
fn a_blank_page_has_the_next_section_s_paper_and_no_section_counts_it() {
    let footer = para(
        &[
            field("PAGE", "1"),
            run(" of "),
            field("SECTIONPAGES", "1"),
            run(" ("),
            field("NUMPAGES", "1"),
            run(")"),
        ]
        .concat(),
    );
    let landscape = sect_on(LANDSCAPE, &["footer:default:f1"], "");
    let content = [para(&run("Landscape")), break_para(&landscape)].concat();
    let doc = Doc {
        body: body(
            content + &para(&run("Portrait")),
            sect(&[], &start("oddPage")),
        ),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    };
    assert_eq!(doc.story_text("footer"), ["1 of 1 (3)", "", "3 of 1 (3)"]);
    let (portrait, landscape) = ((612.0, 792.0), (792.0, 612.0));
    assert_eq!(papers(&doc), [landscape, portrait, portrait]);
}

#[test]
fn a_continuous_section_on_other_paper_starts_a_page_as_in_word() {
    let content = [para(&run("First")), break_para(&sect(&[], ""))].concat();
    let doc = footer_doc(
        content + &para(&run("Second")),
        sect_on(LANDSCAPE, &[], &start("continuous")),
    );
    let lines = body_lines(&doc);
    assert_eq!(lines.last().unwrap().0, 2);
    assert_eq!(papers(&doc), [(612.0, 792.0), (792.0, 612.0)]);
    assert!(doc.settle().unwrap().warnings.is_empty());
}

#[test]
fn a_section_s_first_paragraph_keeps_the_space_before_its_previous_after_left() {
    let spaced = |before: u32, after: u32| {
        format!(
            r#"<w:spacing w:before="{before}" w:after="{after}" w:line="240" w:lineRule="exact"/>"#
        )
    };
    let tops = |after: u32| {
        let ended = format!(
            "<w:p><w:pPr>{}{}</w:pPr>{}</w:p>",
            spaced(0, after),
            sect(&[], ""),
            run("Last")
        );
        let first = format!(
            "<w:p><w:pPr>{}</w:pPr>{}</w:p>",
            spaced(480, 0),
            run("First")
        );
        let doc = Doc {
            body: body(ended + &first, sect(&[], "")),
            stories: Vec::new(),
            even_and_odd: false,
        };
        let lines = doc.lines();
        lines.iter().find(|l| l.3 == "First").unwrap().2
    };
    assert_eq!(tops(200), 72.0 + 24.0 - 10.0);
    assert_eq!(tops(600), 72.0);
}

/// A contents field listing bookmarks `front` (page 3 of roman front
/// matter) and `body` (page 1 after a restart), with stored numbers.
fn contents_doc() -> Doc<'static> {
    let char = |kind: &str| format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#);
    let instr = |code: &str| {
        format!(r#"<w:r><w:instrText xml:space="preserve"> {code} </w:instrText></w:r>"#)
    };
    let entry = |name: &str| {
        para(
            &[
                run(name),
                char("begin"),
                instr(&format!(r"PAGEREF {name} \h")),
                char("separate"),
                run("9"),
                char("end"),
            ]
            .concat(),
        )
    };
    let heading = |name: &str| {
        para_with(
            "<w:pageBreakBefore/>",
            &format!(
                r#"<w:bookmarkStart w:id="{0}" w:name="{1}"/>{2}<w:bookmarkEnd w:id="{0}"/>"#,
                name.len(),
                name,
                run(name)
            ),
        )
    };
    let toc = [
        para(&[char("begin"), instr(r#"TOC \o "1-3" \h"#), char("separate")].concat()),
        entry("front"),
        entry("body"),
        para(&char("end")),
    ]
    .concat();
    let content = [
        toc,
        lines(LINES_PER_PAGE),
        heading("front"),
        break_para(&sect(&[], r#"<w:pgNumType w:fmt="lowerRoman"/>"#)),
        heading("body"),
    ]
    .concat();
    Doc {
        body: body(content, sect(&[], r#"<w:pgNumType w:start="1"/>"#)),
        stories: Vec::new(),
        even_and_odd: false,
    }
}

#[test]
fn contents_numbers_take_their_section_s_format_and_restart() {
    let doc = contents_doc();
    let mut document = doc.document();
    let refresh = refresh_toc_page_numbers(&mut document, false).unwrap();
    assert_eq!(refresh.changed, 2);
    let main = document.package().main_part_name().to_string();
    let tree = document.package().xml(&main).unwrap();
    let shown: Vec<String> = toc_page_refs(tree)
        .into_iter()
        .map(|(t, _)| tree.text_content(t))
        .collect();
    assert_eq!(shown, ["iii", "1"]);
    let layout = &refresh.layout;
    let label = |name| layout.page_label(layout.bookmark_page(name).unwrap());
    assert_eq!(
        (label("front"), label("body")),
        (Some("iii".into()), Some("1".into()))
    );
    let again = refresh_toc_page_numbers(&mut document, false).unwrap();
    assert_eq!(again.changed, 0);
}

#[test]
fn contents_entries_in_chapter_numbered_sections_keep_their_numbers() {
    let doc = contents_doc();
    let body = doc.body.replace(
        r#"w:fmt="lowerRoman""#,
        r#"w:fmt="lowerRoman" w:chapStyle="1""#,
    );
    let mut document = Doc { body, ..doc }.document();
    let refresh = refresh_toc_page_numbers(&mut document, false).unwrap();
    assert_eq!(refresh.changed, 1);
}

/// Each body line's top on a one-page section aligned `valign`, its
/// content ending in an empty section-break paragraph when `broken`.
fn aligned_tops(valign: &str, broken: bool) -> Vec<f64> {
    let extra = format!(r#"<w:vAlign w:val="{valign}"/>"#);
    let content = lines(2);
    let doc = if broken {
        Doc {
            body: body(content + &break_para(&sect(&[], &extra)), sect(&[], "")),
            stories: Vec::new(),
            even_and_odd: false,
        }
    } else {
        Doc {
            body: body(content, sect(&[], &extra)),
            stories: Vec::new(),
            even_and_odd: false,
        }
    };
    let lines = doc.lines();
    lines.iter().filter(|l| l.0 == 1).map(|l| l.2).collect()
}

#[test]
fn a_centred_or_bottom_aligned_section_moves_its_pages_body_down() {
    // 648 pt between the margins; two 12 pt lines leave 624.
    assert_eq!(aligned_tops("top", false), [72.0, 84.0]);
    assert_eq!(aligned_tops("center", false), [384.0, 396.0]);
    assert_eq!(aligned_tops("bottom", false), [696.0, 708.0]);
    assert_eq!(aligned_tops("both", false), [72.0, 84.0]);
    // The empty paragraph ending the section takes no room in the block.
    assert_eq!(aligned_tops("center", true), [384.0, 396.0, 408.0]);
}
