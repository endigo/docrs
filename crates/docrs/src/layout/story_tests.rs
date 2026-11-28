//! Headers, footers and their fields per page, and the settled layout.

use super::*;
use crate::Document;
use crate::layout::fonts::FontKind;
use crate::package::ct;
use crate::txn::RelTarget;
use crate::xml::parse_str;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Arial 10 pt, single exact 12 pt lines and no spacing.
const PPR: &str = r#"<w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="exact"/>"#;
const RPR: &str = r#"<w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="20"/></w:rPr>"#;

fn run(text: &str) -> String {
    format!(r#"<w:r>{RPR}<w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn field(code: &str, cached: &str) -> String {
    let char = |kind: &str| format!(r#"<w:r>{RPR}<w:fldChar w:fldCharType="{kind}"/></w:r>"#);
    format!(
        r#"{}<w:r>{RPR}<w:instrText xml:space="preserve"> {code} </w:instrText></w:r>{}{}{}"#,
        char("begin"),
        char("separate"),
        run(cached),
        char("end"),
    )
}

fn para(content: &str) -> String {
    para_with("", content)
}

fn para_with(ppr: &str, content: &str) -> String {
    format!("<w:p><w:pPr>{PPR}{ppr}</w:pPr>{content}</w:p>")
}

fn lines(count: usize) -> String {
    (1..=count)
        .map(|n| para(&run(&format!("Line {n}"))))
        .collect()
}

/// Letter paper, 1-inch margins, headers and footers half an inch from
/// the edge: 54 body lines a page.
const PAGE: &str = r#"<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>"#;
const LINES_PER_PAGE: usize = 54;

/// A section's properties: `PAGE`, `refs` (`header:default:h1`…) and
/// `extra` (`w:titlePg`, `w:pgNumType`).
fn sect(refs: &[&str], extra: &str) -> String {
    let refs: String = refs
        .iter()
        .map(|r| {
            let [kind, which, part] = r.split(':').collect::<Vec<_>>()[..] else {
                panic!("{r}")
            };
            format!(r#"<w:{kind}Reference w:type="{which}" r:id="{{{part}}}"/>"#)
        })
        .collect();
    format!("<w:sectPr>{refs}{PAGE}{extra}</w:sectPr>")
}

/// A section break paragraph ending a section with `sect_pr`.
fn break_para(sect_pr: &str) -> String {
    format!("<w:p><w:pPr>{PPR}{sect_pr}</w:pPr></w:p>")
}

struct Doc<'a> {
    body: String,
    /// Part stem (`h1`, `f1`…), its paragraphs. Stems starting with `h`
    /// are headers.
    stories: Vec<(&'a str, String)>,
    even_and_odd: bool,
}

impl Doc<'_> {
    fn with<T>(&self, f: impl FnOnce(&Txn<'_>, &str) -> T) -> T {
        let doc = Document::new();
        let mut txn = Txn::new(&doc);
        let main = doc.package().main_part_name().to_string();
        let mut body = self.body.clone();
        for (stem, paras) in &self.stories {
            let header = stem.starts_with('h');
            let (root, content_type, rel) = if header {
                ("hdr", ct::WML_HEADER, rt::HEADER)
            } else {
                ("ftr", ct::WML_FOOTER, rt::FOOTER)
            };
            let name = format!("word/{stem}.xml");
            let tree =
                parse_str(&format!("<w:{root} xmlns:w=\"{W_NS}\">{paras}</w:{root}>")).unwrap();
            txn.add_xml_part(&name, content_type, tree).unwrap();
            let id = txn.relate_to(&main, rel, RelTarget::Part(&name)).unwrap();
            body = body.replace(&format!("{{{stem}}}"), &id);
        }
        *txn.tree_mut(&main).unwrap() = parse_str(&format!(
            "<w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:body>{body}</w:body></w:document>"
        ))
        .unwrap();
        let settings = txn.part_related_by(&main, rt::SETTINGS).unwrap().unwrap();
        let even = if self.even_and_odd {
            "<w:evenAndOddHeaders/>"
        } else {
            ""
        };
        *txn.tree_mut(&settings).unwrap() = parse_str(&format!(
            "<w:settings xmlns:w=\"{W_NS}\">{even}</w:settings>"
        ))
        .unwrap();
        f(&txn, &main)
    }

    fn settle(&self) -> Result<Settled> {
        self.with(|txn, main| settle(txn, main, &FontSet::default()))
    }

    /// Each line: page, story, top, text.
    fn lines(&self) -> Vec<(u32, String, f64, String)> {
        let mut out = Vec::new();
        self.with(|txn, main| {
            line_records_with(
                txn,
                main,
                &FontSet::default(),
                |_, _| Ok(()),
                |line| {
                    out.push((line.page, line.story, line.y, line.text));
                    Ok(())
                },
            )
        })
        .unwrap();
        out
    }

    /// Each page's lines of `story` (`header`, `footer`), joined.
    fn story_text(&self, story: &str) -> Vec<String> {
        let lines = self.lines();
        let pages = lines.iter().map(|l| l.0).max().unwrap_or(0);
        (1..=pages)
            .map(|page| {
                lines
                    .iter()
                    .filter(|l| l.0 == page && l.1.starts_with(story))
                    .map(|l| l.3.as_str())
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect()
    }
}

fn body(content: String, sect_pr: String) -> String {
    content + &sect_pr
}

#[test]
fn a_footer_shows_page_3_of_12() {
    let footer = para(&format!(
        "{}{}{}{}",
        run("Page "),
        field("PAGE", "1"),
        run(" of "),
        field("NUMPAGES", "1")
    ));
    let doc = Doc {
        body: body(
            lines(11 * LINES_PER_PAGE + 1),
            sect(&["footer:default:f1"], ""),
        ),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    };
    let footers = doc.story_text("footer");
    assert_eq!(footers.len(), 12);
    assert_eq!(footers[2], "Page 3 of 12");
    assert_eq!(footers[11], "Page 12 of 12");
    let settled = doc.settle().unwrap();
    assert_eq!(settled.layout.pages, 12);
    assert_eq!(settled.passes, 2);
    assert!(settled.layout.placed.iter().all(|p| p.page <= 12));
}

fn chosen(title_page: bool, even_and_odd: bool) -> Vec<String> {
    let extra = if title_page { "<w:titlePg/>" } else { "" };
    let doc = Doc {
        body: body(
            lines(3 * LINES_PER_PAGE + 1),
            sect(
                &["header:default:h1", "header:first:h2", "header:even:h3"],
                extra,
            ),
        ),
        stories: vec![
            ("h1", para(&run("Default"))),
            ("h2", para(&run("First"))),
            ("h3", para(&run("Even"))),
        ],
        even_and_odd,
    };
    doc.story_text("header")
}

#[test]
fn each_page_shows_its_first_even_or_default_header() {
    assert_eq!(chosen(true, true), ["First", "Even", "Default", "Even"]);
    assert_eq!(
        chosen(true, false),
        ["First", "Default", "Default", "Default"]
    );
    assert_eq!(chosen(false, true), ["Default", "Even", "Default", "Even"]);
    assert_eq!(
        chosen(false, false),
        ["Default", "Default", "Default", "Default"]
    );
}

#[test]
fn a_section_inherits_the_headers_it_does_not_name() {
    let first = sect(&["header:default:h1", "header:first:h2"], "<w:titlePg/>");
    let second = sect(&[], "<w:titlePg/>");
    let third = sect(&["header:default:h3"], "");
    let content = [
        lines(LINES_PER_PAGE + 1),
        break_para(&first),
        lines(LINES_PER_PAGE + 1),
        break_para(&second),
        lines(1),
    ]
    .concat();
    let doc = Doc {
        body: body(content, third),
        stories: vec![
            ("h1", para(&run("One"))),
            ("h2", para(&run("One first"))),
            ("h3", para(&run("Three"))),
        ],
        even_and_odd: false,
    };
    // The second section shows the first's first-page header on its first
    // page; the third, without a title page, its own default.
    assert_eq!(
        doc.story_text("header"),
        ["One first", "One", "One first", "One", "Three"]
    );
}

#[test]
fn a_header_taller_than_the_top_margin_pushes_the_body_down() {
    let top = |header_lines: usize| {
        let doc = Doc {
            body: body(lines(2), sect(&["header:default:h1"], "")),
            stories: vec![("h1", lines(header_lines))],
            even_and_odd: false,
        };
        let lines = doc.lines();
        let body = lines.iter().find(|l| l.1 == "body").unwrap();
        let header: Vec<f64> = lines
            .iter()
            .filter(|l| l.1 == "header1")
            .map(|l| l.2)
            .collect();
        (header, body.2)
    };
    // Header from 36 pt: three lines end at 72, the top margin.
    assert_eq!(top(3), (vec![36.0, 48.0, 60.0], 72.0));
    assert_eq!(top(5).1, 96.0);
}

#[test]
fn a_footer_taller_than_the_bottom_margin_pushes_the_body_up() {
    let doc = Doc {
        body: body(lines(LINES_PER_PAGE), sect(&["footer:default:f1"], "")),
        stories: vec![("f1", lines(5))],
        even_and_odd: false,
    };
    let lines = doc.lines();
    let footer: Vec<f64> = lines
        .iter()
        .filter(|l| l.1 == "footer1")
        .map(|l| l.2)
        .collect();
    // Five lines end 36 pt above the paper's edge, so the body ends at 696
    // and loses two of its 54 lines.
    assert_eq!(footer[..2], [696.0, 708.0]);
    assert_eq!(
        lines.iter().filter(|l| l.0 == 1 && l.1 == "body").count(),
        52
    );
}

#[test]
fn a_story_past_its_half_of_the_page_is_clipped_and_warned() {
    let doc = Doc {
        body: body(lines(1), sect(&["header:default:h1"], "")),
        stories: vec![("h1", lines(60))],
        even_and_odd: false,
    };
    // Its half: from 36 pt to 378, a page's half less 18 pt: 28 lines.
    let lines = doc.lines();
    assert_eq!(lines.iter().filter(|l| l.1 == "header1").count(), 28);
    assert_eq!(lines.iter().find(|l| l.1 == "body").unwrap().2, 372.0);
    let settled = doc.settle().unwrap();
    assert_eq!(
        settled.warnings,
        [LayoutWarning {
            code: furniture::STORY_OVERFLOW,
            story: StoryId::Header(1),
            page: Some(1),
        }]
    );
}

#[test]
fn page_numbers_take_their_format_switch_and_the_section_s_format() {
    let footer = para(
        &[
            field("PAGE", "1"),
            run(" "),
            field(r"PAGE \* roman", "1"),
            run(" "),
            field(r"PAGE \* ALPHABETIC \* MERGEFORMAT", "1"),
            run(" "),
            field(r"PAGE \* Arabic", "1"),
        ]
        .concat(),
    );
    let doc = Doc {
        body: body(
            lines(2 * LINES_PER_PAGE + 1),
            sect(
                &["footer:default:f1"],
                r#"<w:pgNumType w:fmt="upperRoman" w:start="4"/>"#,
            ),
        ),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    };
    assert_eq!(
        doc.story_text("footer"),
        ["IV iv D 4", "V v E 5", "VI vi F 6"]
    );
}

#[test]
fn a_date_keeps_its_stored_value_and_warns() {
    let footer = para(&field(r#"DATE \@ "d MMMM yyyy""#, "1 January 2020"));
    let doc = Doc {
        body: body(lines(1), sect(&["footer:default:f1"], "")),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    };
    assert_eq!(doc.story_text("footer"), ["1 January 2020"]);
    assert_eq!(
        doc.settle().unwrap().warnings,
        [LayoutWarning {
            code: fields::FIELD_CACHED,
            story: StoryId::Footer(1),
            page: None,
        }]
    );
}

/// Right indent (twips) leaving the footer room for `fits` on one line
/// but not `wraps`, in Arial 10 pt.
fn room_between(fits: &str, wraps: &str) -> String {
    let face = FontBook::new(Vec::new()).face("Arial", false, false, FontKind::Sans);
    let width = |text: &str| text.chars().map(|c| face.advance(c) * 10.0).sum::<f64>();
    let (fits, wraps) = (width(fits), width(wraps));
    assert!(fits < wraps);
    let room = (fits + wraps) / 2.0;
    format!(r#"<w:ind w:right="{}"/>"#, ((468.0 - room) * 20.0).round())
}

/// A body that fills 9 pages under a one-line footer and runs onto a 10th
/// under a two-line one, the footer's distance from the edge taking the
/// bottom margin's place.
fn wrapping_footer(footer: String) -> Doc<'static> {
    let page = PAGE.replace(r#"w:bottom="1440""#, r#"w:bottom="720""#);
    let sect_pr = format!(
        r#"<w:sectPr><w:footerReference w:type="default" r:id="{{f1}}"/>{page}</w:sectPr>"#
    );
    // One footer line leaves 792 - 72 - 36 - 12 = 672 pt: 56 lines.
    Doc {
        body: body(lines(9 * 56), sect_pr),
        stories: vec![("f1", footer)],
        even_and_odd: false,
    }
}

#[test]
fn a_footer_that_wraps_at_ten_pages_settles_where_its_numbers_hold() {
    let footer = |cached: &str| {
        para_with(
            &room_between("Page 9 of 9", "Page 1 of 10"),
            &[
                run("Page "),
                field("PAGE", "1"),
                run(" of "),
                field("NUMPAGES", cached),
            ]
            .concat(),
        )
    };
    // From a stored 1: nine pages, each footer on one line.
    let nine = wrapping_footer(footer("1"));
    let settled = nine.settle().unwrap();
    assert_eq!(settled.layout.pages, 9);
    assert_eq!(nine.story_text("footer")[8], "Page 9 of 9");
    // From a stored 10: "of 10" wraps, and the taller footers hold ten.
    let ten = wrapping_footer(footer("10"));
    let settled = ten.settle().unwrap();
    assert_eq!(settled.layout.pages, 10);
    let last = &ten.story_text("footer")[9];
    assert!(last.contains('|'), "{last}");
    assert_eq!(last.replace('|', ""), "Page 10 of 10");
}

#[test]
fn page_numbers_that_change_their_own_layout_refuse() {
    // IX wraps and gives ten pages; X does not and gives nine.
    let footer = para_with(
        &room_between("Total X", "Total IX"),
        &[run("Total "), field(r"NUMPAGES \* ROMAN", "I")].concat(),
    );
    let error = wrapping_footer(footer).settle().err().unwrap();
    assert_eq!(error.reason(), NOT_CONVERGED);
    assert!(error.message().contains("repeat"), "{}", error.message());
}

#[test]
fn page_references_and_body_page_fields_settle() {
    let content = [
        para(&[run("See page "), field(r"PAGEREF far \h", "1")].concat()),
        para(&[run("This is page "), field("PAGE", "1")].concat()),
        lines(LINES_PER_PAGE),
        para(
            &[
                r#"<w:bookmarkStart w:id="1" w:name="far"/>"#,
                &run("Far"),
                r#"<w:bookmarkEnd w:id="1"/>"#,
                &field("PAGE", "1"),
            ]
            .concat(),
        ),
    ]
    .concat();
    let doc = Doc {
        body: body(content, sect(&[], "")),
        stories: Vec::new(),
        even_and_odd: false,
    };
    let lines = doc.lines();
    let texts: Vec<&str> = lines.iter().map(|l| l.3.as_str()).collect();
    assert_eq!(texts[0], "See page 2");
    assert_eq!(texts[1], "This is page 1");
    assert_eq!(*texts.last().unwrap(), "Far2");
}

#[test]
fn placed_paragraphs_stay_the_body_s_with_stories_laid_out() {
    let doc = Doc {
        body: body(
            lines(2),
            sect(&["header:default:h1", "footer:default:f1"], ""),
        ),
        stories: vec![("h1", lines(3)), ("f1", lines(2))],
        even_and_odd: false,
    };
    let settled = doc.settle().unwrap();
    assert_eq!(settled.layout.placed.len(), 2);
    let texts = doc.with(|txn, main| {
        let tree = txn.tree(main).unwrap();
        settled
            .layout
            .placed
            .iter()
            .map(|p| tree.text_content(p.node.unwrap()))
            .collect::<Vec<_>>()
    });
    assert_eq!(texts, ["Line 1", "Line 2"]);
}
