//! Links, named destinations and the outline, read back by the test reader.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
mod support;
#[allow(dead_code)]
#[path = "support/glyphs.rs"]
mod text;

use std::path::PathBuf;
use std::process::Command;

use docrs::clock::Timestamp;
use docrs::layout::{FontSet, Link};
use docrs::markdown::{InputFormat, MarkdownOptions, insert_source_with};
use docrs::pdf::{
    Info, LinkId, Navigation, PdfOptions, PdfWarning, Rect, linked_document_for_tests,
    paint_for_tests, preflight, render,
};
use docrs::{Document, ErrorKind, Limits, Tolerance};
use reader::{Dict, Pdf, Value, decode_text};
use support::{RawMember, build_zip};
use text::{Glyph, glyphs};

const A4: (f64, f64) = (595.2756, 841.8898);

fn write(pages: usize, navigation: Navigation) -> (Vec<u8>, Vec<PdfWarning>) {
    linked_document_for_tests(
        &vec![A4; pages],
        navigation,
        &Info::default(),
        "docrs-test",
        &Limits::default(),
    )
    .unwrap()
}

fn rect(left: f64, bottom: f64, right: f64, top: f64) -> Rect {
    Rect {
        left,
        bottom,
        right,
        top,
    }
}

fn uri(target: &str) -> Link {
    Link::Uri(target.into())
}

fn anchor(name: &str) -> Link {
    Link::Anchor(name.into())
}

#[derive(Debug, Clone, PartialEq)]
enum Goes {
    Uri(String),
    /// `[page /XYZ null top null]`.
    Page(usize, f64),
    /// `[page /XYZ x top 0]`.
    At(usize, f64, f64),
}

/// A bookmark whose line's text starts at 72 pt.
fn at(page: usize, top: f64) -> Goes {
    Goes::At(page, 69.84, top)
}

#[derive(Debug, PartialEq)]
struct Annot {
    page: usize,
    rect: [f64; 4],
    goes: Goes,
}

fn numbers(pdf: &Pdf, value: &Value) -> Vec<f64> {
    let Value::Array(items) = pdf.resolve(value) else {
        panic!("not an array: {value:?}");
    };
    items
        .iter()
        .map(|item| pdf.resolve(item).number().unwrap())
        .collect()
}

fn string(value: &Value) -> Vec<u8> {
    let Value::Str(bytes) = value else {
        panic!("not a string: {value:?}");
    };
    bytes.clone()
}

/// An explicit `[page /XYZ null top null]` destination.
fn place(pdf: &Pdf, dest: &Value) -> Goes {
    let Value::Array(items) = pdf.resolve(dest) else {
        panic!("destination is not an array: {dest:?}");
    };
    assert_eq!(items.len(), 5, "{items:?}");
    assert_eq!(items[1], Value::Name("XYZ".into()));
    let page = pdf.page_index(&items[0]).unwrap();
    let top = items[3].number().unwrap();
    match (&items[2], &items[4]) {
        (Value::Null, Value::Null) => Goes::Page(page, top),
        (x, Value::Int(0)) => Goes::At(page, x.number().unwrap(), top),
        other => panic!("destination {other:?}"),
    }
}

fn dests(pdf: &Pdf) -> Vec<(Vec<u8>, &Value)> {
    let names = pdf.get(pdf.catalog().unwrap(), "Names");
    let Some(names) = names.dict() else {
        return Vec::new();
    };
    pdf.name_tree(names.get("Dests").unwrap()).unwrap()
}

fn goes(pdf: &Pdf, action: &Dict) -> Goes {
    match pdf.get(action, "S") {
        Value::Name(kind) if kind == "URI" => {
            Goes::Uri(String::from_utf8(string(pdf.get(action, "URI"))).unwrap())
        }
        Value::Name(kind) if kind == "GoTo" => {
            let explicit = pdf.get(action, "D");
            if matches!(explicit, Value::Array(_)) {
                return place(pdf, explicit);
            }
            let key = string(explicit);
            let dests = dests(pdf);
            let (_, dest) = dests
                .iter()
                .find(|(name, _)| *name == key)
                .expect("GoTo names a destination in /Dests");
            place(pdf, dest)
        }
        other => panic!("action {other:?}"),
    }
}

fn annots(pdf: &Pdf) -> Vec<Annot> {
    let mut out = Vec::new();
    for (page, dict) in pdf.pages().unwrap().into_iter().enumerate() {
        let Value::Array(refs) = pdf.get(dict, "Annots") else {
            continue;
        };
        for annot in refs {
            assert!(matches!(annot, Value::Ref(..)), "annotations are indirect");
            let annot = pdf.resolve(annot).dict().unwrap();
            assert_eq!(pdf.get(annot, "Subtype"), &Value::Name("Link".into()));
            assert_eq!(numbers(pdf, pdf.get(annot, "Border")), [0.0, 0.0, 0.0]);
            assert_eq!(pdf.page_index(&annot["P"]).unwrap(), page);
            out.push(Annot {
                page,
                rect: numbers(pdf, pdf.get(annot, "Rect")).try_into().unwrap(),
                goes: goes(pdf, pdf.get(annot, "A").dict().unwrap()),
            });
        }
    }
    out
}

#[derive(Debug, PartialEq)]
struct Item {
    depth: usize,
    title: String,
    goes: Goes,
    count: Option<f64>,
}

/// The outline in reading order. Checks every `/Parent`, `/Prev`, `/Last`
/// and that each open item's `/Count` is its number of descendants.
fn outline(pdf: &Pdf) -> Vec<Item> {
    let root = &pdf.catalog().unwrap()["Outlines"];
    let mut out = Vec::new();
    let total = walk(pdf, root, 0, &mut out);
    let root = pdf.resolve(root).dict().unwrap();
    assert_eq!(pdf.get(root, "Count").number(), Some(total as f64));
    out
}

fn walk(pdf: &Pdf, parent: &Value, depth: usize, out: &mut Vec<Item>) -> usize {
    let dict = pdf.resolve(parent).dict().unwrap();
    let mut seen = 0;
    let mut previous: Option<&Value> = None;
    let mut next = dict.get("First");
    while let Some(at) = next {
        let item = pdf.resolve(at).dict().unwrap();
        assert_eq!(&item["Parent"], parent);
        assert_eq!(item.get("Prev"), previous);
        let index = out.len();
        out.push(Item {
            depth,
            title: decode_text(&string(pdf.get(item, "Title"))).unwrap(),
            goes: place(pdf, pdf.get(item, "Dest")),
            count: pdf.get(item, "Count").number(),
        });
        let below = walk(pdf, at, depth + 1, out);
        assert_eq!(out[index].count, (below > 0).then_some(below as f64));
        seen += 1 + below;
        previous = Some(at);
        next = item.get("Next");
    }
    assert_eq!(dict.get("Last"), previous);
    seen
}

fn codes(warnings: &[PdfWarning]) -> Vec<(&str, usize)> {
    warnings.iter().map(|w| (w.code, w.count)).collect()
}

fn sample() -> Navigation {
    let mut nav = Navigation::default();
    let web = uri("https://example.com/a");
    nav.link(LinkId(0), 0, 0, rect(72.0, 700.0, 140.0, 714.0), &web);
    nav.link(LinkId(0), 0, 0, rect(140.0, 700.0, 200.0, 714.0), &web);
    nav.link(LinkId(0), 0, 1, rect(72.0, 686.0, 120.0, 700.0), &web);
    let results = anchor("Results");
    nav.link(LinkId(1), 1, 5, rect(72.0, 500.0, 160.5, 512.25), &results);
    nav.link(
        LinkId(2),
        1,
        6,
        rect(72.0, 480.0, 130.0, 492.0),
        &anchor("_Toc1"),
    );
    let mail = uri("mailto:team@example.com");
    nav.link(LinkId(3), 2, 9, rect(300.0, 100.0, 340.0, 112.0), &mail);
    let tel = uri("tel:+1-555-0100");
    nav.link(LinkId(4), 2, 10, rect(300.0, 80.0, 340.0, 92.0), &tel);
    nav.destination("Results", 2, 72.0, 640.5);
    nav.destination("_Toc1", 0, 72.0, 780.0);
    nav.destination("Appendix", 2, 72.0, 300.0);
    nav.destination("Results", 1, 72.0, 10.0);
    for (level, title, page, top) in [
        (1, "Introduction", 0, 780.0),
        (2, "Scope", 0, 600.0),
        (2, "Terms", 1, 780.0),
        (1, "Results", 2, 640.5),
        (3, "Raw data", 2, 400.0),
    ] {
        nav.heading(level, title, page, top);
    }
    nav
}

#[test]
fn links_go_to_their_uris_and_destination_pages() {
    let (bytes, warnings) = write(3, sample());
    assert_eq!(warnings, []);
    let pdf = Pdf::parse(&bytes).unwrap();
    let to = |uri: &str| Goes::Uri(uri.into());
    assert_eq!(
        annots(&pdf),
        [
            Annot {
                page: 0,
                rect: [72.0, 700.0, 200.0, 714.0],
                goes: to("https://example.com/a"),
            },
            Annot {
                page: 0,
                rect: [72.0, 686.0, 120.0, 700.0],
                goes: to("https://example.com/a"),
            },
            Annot {
                page: 1,
                rect: [72.0, 500.0, 160.5, 512.25],
                goes: at(2, 640.5),
            },
            Annot {
                page: 1,
                rect: [72.0, 480.0, 130.0, 492.0],
                goes: at(0, 780.0),
            },
            Annot {
                page: 2,
                rect: [300.0, 100.0, 340.0, 112.0],
                goes: to("mailto:team@example.com"),
            },
            Annot {
                page: 2,
                rect: [300.0, 80.0, 340.0, 92.0],
                goes: to("tel:+1-555-0100"),
            },
        ]
    );
}

fn page_rects(nav: Navigation, pages: usize) -> Vec<(usize, [f64; 4])> {
    let (bytes, _) = write(pages, nav);
    annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|annot| (annot.page, annot.rect))
        .collect()
}

#[test]
fn parts_of_a_link_merge_only_on_one_line_when_they_touch() {
    let com = uri("http://example.com");
    let org = uri("http://example.org");
    let mut nav = Navigation::default();
    nav.link(LinkId(0), 0, 0, rect(10.0, 10.0, 20.0, 20.0), &com);
    nav.link(LinkId(0), 0, 0, rect(20.3, 9.0, 30.0, 22.0), &com);
    nav.link(LinkId(0), 0, 0, rect(31.0, 10.0, 40.0, 20.0), &com);
    nav.link(LinkId(1), 0, 0, rect(40.0, 10.0, 50.0, 20.0), &com);
    nav.link(LinkId(2), 0, 0, rect(50.0, 10.0, 60.0, 20.0), &org);
    nav.link(LinkId(2), 1, 0, rect(50.0, 10.0, 60.0, 20.0), &org);
    assert_eq!(
        page_rects(nav, 2),
        [
            (0, [10.0, 9.0, 30.0, 22.0]),
            (0, [31.0, 10.0, 40.0, 20.0]),
            (0, [40.0, 10.0, 50.0, 20.0]),
            (0, [50.0, 10.0, 60.0, 20.0]),
            (1, [50.0, 10.0, 60.0, 20.0]),
        ]
    );
}

#[test]
fn a_wrapped_link_with_overlapping_boxes_is_one_rect_per_line() {
    let target = uri("https://example.com/long");
    let mut nav = Navigation::default();
    nav.link(LinkId(7), 0, 3, rect(0.0, 200.0, 50.0, 230.0), &target);
    nav.link(LinkId(7), 0, 4, rect(0.0, 188.0, 50.0, 201.0), &target);
    nav.link(LinkId(7), 0, 4, rect(50.0, 188.0, 80.0, 201.0), &target);
    let (bytes, warnings) = write(1, nav);
    assert_eq!(warnings, []);
    let rects: Vec<[f64; 4]> = annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|annot| annot.rect)
        .collect();
    assert_eq!(
        rects,
        [[0.0, 200.0, 50.0, 230.0], [0.0, 188.0, 80.0, 201.0]]
    );
}

#[test]
fn destinations_are_a_sorted_name_tree_whose_first_entry_wins() {
    let (bytes, _) = write(3, sample());
    let pdf = Pdf::parse(&bytes).unwrap();
    let entries: Vec<(String, Goes)> = dests(&pdf)
        .into_iter()
        .map(|(key, dest)| (String::from_utf8(key).unwrap(), place(&pdf, dest)))
        .collect();
    assert_eq!(
        entries,
        [
            ("Appendix".into(), at(2, 300.0)),
            ("Results".into(), at(2, 640.5)),
            ("_Toc1".into(), at(0, 780.0)),
        ]
    );
}

#[test]
fn many_destinations_split_into_leaves_with_limits() {
    let mut nav = Navigation::default();
    let names: Vec<String> = (0..150).map(|i| format!("_Toc{i:04}")).collect();
    for (index, name) in names.iter().enumerate().rev() {
        nav.destination(name, index % 2, 72.0, 700.0);
    }
    nav.destination("Zoë", 1, 72.0, 10.0);
    nav.destination("Zoe", 0, 72.0, 20.0);
    nav.link(
        LinkId(0),
        0,
        0,
        rect(1.0, 1.0, 2.0, 2.0),
        &anchor("_Toc0149"),
    );
    nav.link(LinkId(1), 0, 1, rect(1.0, 5.0, 2.0, 6.0), &anchor("Zoë"));
    let (bytes, warnings) = write(2, nav);
    assert_eq!(warnings, []);
    let pdf = Pdf::parse(&bytes).unwrap();
    let keys: Vec<Vec<u8>> = dests(&pdf).into_iter().map(|(key, _)| key).collect();
    let mut expected: Vec<Vec<u8>> = names.iter().map(|n| n.as_bytes().to_vec()).collect();
    expected.extend([b"Zoe".to_vec(), b"Zo%C3%AB".to_vec()]);
    expected.sort();
    assert_eq!(keys, expected);
    let root = pdf.get(pdf.catalog().unwrap(), "Names").dict().unwrap();
    let tree = pdf.get(root, "Dests").dict().unwrap();
    assert!(matches!(pdf.get(tree, "Kids"), Value::Array(kids) if kids.len() == 3));
    let goes: Vec<Goes> = annots(&pdf).into_iter().map(|a| a.goes).collect();
    assert_eq!(goes, [at(1, 700.0), at(1, 10.0)]);
}

#[test]
fn the_outline_nests_headings_whatever_levels_they_skip() {
    let (bytes, _) = write(3, sample());
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(
        pdf.get(pdf.catalog().unwrap(), "PageMode"),
        &Value::Name("UseOutlines".into())
    );
    let item = |depth, title: &str, page, top, count| Item {
        depth,
        title: title.into(),
        goes: Goes::Page(page, top),
        count,
    };
    assert_eq!(
        outline(&pdf),
        [
            item(0, "Introduction", 0, 780.0, Some(2.0)),
            item(1, "Scope", 0, 600.0, None),
            item(1, "Terms", 1, 780.0, None),
            item(0, "Results", 2, 640.5, Some(1.0)),
            item(1, "Raw data", 2, 400.0, None),
        ]
    );
}

#[test]
fn a_deeper_heading_after_a_skip_and_a_shallower_one_after_it() {
    let mut nav = Navigation::default();
    for (level, title) in [
        (2, "Preface"),
        (1, "One"),
        (3, "Deep"),
        (2, "Mid"),
        (4, "Deeper"),
    ] {
        nav.heading(level, title, 0, 500.0);
    }
    let (bytes, _) = write(1, nav);
    let shape: Vec<(usize, String, Option<f64>)> = outline(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|item| (item.depth, item.title, item.count))
        .collect();
    let expected = [
        (0, "Preface", None),
        (0, "One", Some(3.0)),
        (1, "Deep", None),
        (1, "Mid", Some(1.0)),
        (2, "Deeper", None),
    ];
    let expected: Vec<_> = expected
        .into_iter()
        .map(|(depth, title, count)| (depth, title.to_string(), count))
        .collect();
    assert_eq!(shape, expected);
}

#[test]
fn titles_are_utf16_when_not_ascii_and_have_no_controls() {
    let mut nav = Navigation::default();
    nav.heading(1, "Zoë – résumé 日本 😀", 0, 700.0);
    nav.heading(1, "  Line\nbreak\tand (parens)  ", 0, 600.0);
    nav.heading(1, " \n\t ", 0, 500.0);
    let (bytes, _) = write(1, nav);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Title <FEFF005A006F00EB"), "{text}");
    assert!(
        text.contains(r"/Title (Line break and \(parens\))"),
        "{text}"
    );
    let titles: Vec<String> = outline(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|item| item.title)
        .collect();
    assert_eq!(titles, ["Zoë – résumé 日本 😀", "Line break and (parens)"]);
}

#[test]
fn uris_are_seven_bit_ascii() {
    let mut nav = Navigation::default();
    let target = uri("  https://example.com/Zoë a(1)\\b  ");
    nav.link(LinkId(0), 0, 0, rect(1.0, 1.0, 2.0, 2.0), &target);
    let (bytes, _) = write(1, nav);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(r"/URI (https://example.com/Zo%C3%AB%20a\(1\)\\b)"));
    let goes: Vec<Goes> = annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|a| a.goes)
        .collect();
    assert_eq!(
        goes,
        [Goes::Uri("https://example.com/Zo%C3%AB%20a(1)\\b".into())]
    );
}

#[test]
fn every_other_scheme_is_skipped_and_never_becomes_an_action() {
    let refused = [
        "file:///etc/passwd",
        "FILE://server/share",
        "javascript:alert(1)",
        "JavaScript:app.alert(1)",
        "data:text/html,<b>x</b>",
        "launch:calc.exe",
        "vbscript:msgbox",
        "ftp://example.com",
        "relative/page.html",
        "#fragment",
        "",
        "C:\\Windows\\calc.exe",
    ];
    let mut nav = Navigation::default();
    for (index, target) in refused.iter().enumerate() {
        let bottom = index as f64 * 10.0;
        for (part, left) in [1.0, 2.0].into_iter().enumerate() {
            let part_rect = rect(left, bottom, left + 1.0, bottom + 5.0);
            nav.link(LinkId(index), 0, index + part, part_rect, &uri(target));
        }
    }
    let allowed = uri("HTTPS://example.com");
    nav.link(LinkId(99), 0, 99, rect(1.0, 500.0, 2.0, 505.0), &allowed);
    let (bytes, warnings) = write(1, nav);
    assert_eq!(
        codes(&warnings),
        [("pdf_link_scheme_skipped", refused.len())]
    );
    assert_eq!(
        warnings[0].message,
        "link (12): a link uses a scheme other than http, https, mailto, tel or a bookmark; \
         its text is kept without the link (\"file:\", \"file:\", \"javascript:\", \"javascript:\", \"data:\")"
    );
    let text = String::from_utf8_lossy(&bytes);
    for action in [
        "/Launch",
        "/JavaScript",
        "/JS",
        "/GoToR",
        "/SubmitForm",
        "file:",
        "javascript",
    ] {
        assert!(!text.contains(action), "{action}");
    }
    let goes: Vec<Goes> = annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|a| a.goes)
        .collect();
    assert_eq!(goes, [Goes::Uri("HTTPS://example.com".into())]);
}

#[test]
fn a_link_finds_its_bookmark_in_any_case_or_is_dropped_with_a_warning() {
    let mut nav = Navigation::default();
    let nowhere = anchor("Nowhere");
    nav.link(LinkId(0), 1, 0, rect(1.0, 1.0, 2.0, 2.0), &nowhere);
    nav.link(LinkId(0), 1, 1, rect(1.0, 3.0, 2.0, 4.0), &nowhere);
    nav.link(
        LinkId(1),
        1,
        2,
        rect(1.0, 10.0, 2.0, 12.0),
        &anchor("Résumé"),
    );
    nav.link(
        LinkId(2),
        1,
        3,
        rect(1.0, 20.0, 2.0, 22.0),
        &anchor("RÉSUMÉ"),
    );
    nav.link(LinkId(3), 1, 4, rect(1.0, 30.0, 2.0, 32.0), &anchor(""));
    nav.destination("résumé", 0, 72.0, 100.0);
    nav.destination("RÉSUMÉ", 0, 72.0, 50.0);
    let (bytes, warnings) = write(2, nav);
    assert_eq!(codes(&warnings), [("pdf_link_target_missing", 2)]);
    assert_eq!(
        warnings[0].message,
        "link (2): a link goes to a bookmark the document does not have; \
         its text is kept without the link (\"Nowhere\", \"\")"
    );
    let pdf = Pdf::parse(&bytes).unwrap();
    let goes: Vec<Goes> = annots(&pdf).into_iter().map(|a| a.goes).collect();
    assert_eq!(goes, [at(0, 100.0), at(0, 100.0)]);
    let keys: Vec<Vec<u8>> = dests(&pdf).into_iter().map(|(key, _)| key).collect();
    assert_eq!(keys, [b"r%C3%A9sum%C3%A9"]);
}

#[test]
fn top_goes_to_page_one_s_text_area_without_a_bookmark() {
    let mut nav = Navigation::default();
    nav.link(LinkId(0), 1, 0, rect(1.0, 1.0, 2.0, 2.0), &anchor("_top"));
    nav.link(LinkId(1), 1, 1, rect(1.0, 5.0, 2.0, 6.0), &anchor("_TOP"));
    let (bytes, warnings) = write(2, nav);
    assert_eq!(warnings, []);
    let pdf = Pdf::parse(&bytes).unwrap();
    let goes: Vec<Goes> = annots(&pdf).into_iter().map(|a| a.goes).collect();
    assert_eq!(goes, [Goes::Page(0, 841.89), Goes::Page(0, 841.89)]);
    assert!(dests(&pdf).is_empty());

    let mut nav = Navigation::default();
    nav.text_area(72.0, 720.0);
    nav.destination("_top", 1, 72.0, 300.0);
    nav.link(LinkId(0), 1, 0, rect(1.0, 1.0, 2.0, 2.0), &anchor("_top"));
    let (bytes, warnings) = write(2, nav);
    assert_eq!(warnings, []);
    let goes: Vec<Goes> = annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|a| a.goes)
        .collect();
    assert_eq!(goes, [Goes::At(0, 69.84, 720.0)]);
}

#[test]
fn destination_names_are_ascii_so_every_reader_sorts_them_alike() {
    let names = [
        "Àpropos",
        "été",
        "Введение",
        "Рис1",
        "рис2",
        "Итоги",
        "\u{FEFF}Intro",
        "100% done",
        "Intro",
    ];
    let mut nav = Navigation::default();
    for (index, name) in names.iter().enumerate() {
        nav.destination(name, index % 2, 72.0, 100.0 + index as f64);
        let part = rect(1.0, index as f64 * 10.0, 2.0, index as f64 * 10.0 + 5.0);
        nav.link(LinkId(index), 0, index, part, &anchor(&name.to_uppercase()));
    }
    let (bytes, warnings) = write(2, nav);
    assert_eq!(warnings, []);
    let pdf = Pdf::parse(&bytes).unwrap();
    let keys: Vec<String> = dests(&pdf)
        .into_iter()
        .map(|(key, _)| String::from_utf8(key).unwrap())
        .collect();
    assert!(
        keys.iter()
            .all(|key| key.bytes().all(|b| (b' '..=b'~').contains(&b)))
    );
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{keys:?}");
    assert!(keys.contains(&"%EF%BB%BFIntro".to_string()), "{keys:?}");
    assert!(keys.contains(&"100%25 done".to_string()), "{keys:?}");
    assert!(keys.contains(&"%D0%92%D0%B2%D0%B5%D0%B4%D0%B5%D0%BD%D0%B8%D0%B5".to_string()));
    let goes: Vec<Goes> = annots(&pdf).into_iter().map(|a| a.goes).collect();
    let expected: Vec<Goes> = (0..names.len())
        .map(|index| at(index % 2, 100.0 + index as f64))
        .collect();
    assert_eq!(goes, expected);
    qpdf_check("names.pdf", &bytes);
}

#[test]
fn the_new_codes_are_registered() {
    for code in ["pdf_link_scheme_skipped", "pdf_link_target_missing"] {
        assert!(
            docrs::pdf::WARNING_CODES
                .iter()
                .any(|(known, _)| *known == code)
        );
    }
}

#[test]
fn no_navigation_adds_nothing_to_the_catalog() {
    let (bytes, _) = write(2, Navigation::default());
    let pdf = Pdf::parse(&bytes).unwrap();
    let catalog = pdf.catalog().unwrap();
    assert_eq!(catalog.keys().collect::<Vec<_>>(), ["Pages", "Type"]);
    assert!(
        pdf.pages()
            .unwrap()
            .iter()
            .all(|p| !p.contains_key("Annots"))
    );
}

#[test]
fn a_page_beyond_the_document_is_an_internal_error() {
    let mut far_bookmark = Navigation::default();
    far_bookmark.destination("Far", 3, 72.0, 10.0);
    let mut far_link = Navigation::default();
    let part = rect(1.0, 1.0, 2.0, 2.0);
    far_link.link(LinkId(0), 3, 0, part, &anchor("Missing"));
    for nav in [far_bookmark, far_link] {
        let err = linked_document_for_tests(
            &[A4],
            nav,
            &Info::default(),
            "docrs-test",
            &Limits::default(),
        )
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert_eq!(err.reason(), "pdf_nav_page");
    }
}

#[test]
fn output_is_deterministic() {
    assert_eq!(write(3, sample()).0, write(3, sample()).0);
}

/// `qpdf --check` exits 0 with no warning; skipped where qpdf is missing.
fn qpdf_check(name: &str, bytes: &[u8]) {
    let found = Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        let required = std::env::var_os("CI").is_some() && !cfg!(windows);
        assert!(!required, "qpdf must be installed in CI");
        eprintln!("SKIPPED: qpdf is not installed; `brew install qpdf` to run this check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    let errors = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{name}: {report}{errors}");
    assert!(
        errors.is_empty() && !report.contains("WARNING"),
        "{name}: {report}{errors}"
    );
    assert!(
        report.contains("No syntax or stream encoding errors found"),
        "{name}: {report}"
    );
}

#[test]
fn qpdf_accepts_links_and_the_outline() {
    qpdf_check("links.pdf", &write(3, sample()).0);
}

// Links, bookmarks and headings as the painter places them.

fn fixtures() -> PathBuf {
    support::repo_root().join("tests/fixtures")
}

fn font(path: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("fonts").join(path)).unwrap()
}

const LIBERATION: &str = "liberation/LiberationSerif-Regular.ttf";

/// How far Word's link rectangles reach past the text.
const LINK_MARGIN: f64 = 2.16;

fn fonts() -> FontSet {
    let mut builder = FontSet::builder();
    for path in ["inter/Inter[opsz,wght].ttf", LIBERATION] {
        builder.add(font(path)).unwrap();
    }
    builder.build()
}

fn options() -> PdfOptions {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options
}

/// `source` converted into `reference`, at a fixed time.
fn markdown(source: &str, reference: &std::path::Path) -> Document {
    let mut document = Document::open_path_with(reference, &Limits::default(), Tolerance::word())
        .unwrap()
        .0;
    let options = MarkdownOptions {
        now: Some(Timestamp::from_utc(2026, 11, 28, 9, 0, 0).unwrap()),
        ..MarkdownOptions::default()
    };
    insert_source_with(
        &mut document,
        source,
        InputFormat::Markdown,
        &options,
        &fonts(),
    )
    .unwrap();
    document
}

fn inter(source: &str) -> Document {
    markdown(source, &fixtures().join("pdf/references/inter.docx"))
}

/// A Letter document of `body`, with hyperlink relationships `rLink` to
/// `https://example.com/raw` and `rTop` to `#_top`.
fn raw(body: &str) -> Document {
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}{section}</w:body></w:document>"#
    );
    let types = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rLink" Type="{R}/hyperlink" Target="https://example.com/raw" TargetMode="External"/><Relationship Id="rTop" Type="{R}/hyperlink" Target="#_top" TargetMode="External"/></Relationships>"##
    );
    support::open_bytes(&build_zip(&[
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", support::ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
    ]))
}

/// A run of `text` in 11 pt Liberation Serif.
fn run(text: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/><w:sz w:val="22"/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#
    )
}

fn para(ppr: &str, content: &str) -> String {
    format!("<w:p><w:pPr>{ppr}</w:pPr>{content}</w:p>")
}

const PAGE_BREAK: &str = r#"<w:r><w:br w:type="page"/></w:r>"#;

fn pdf_of(document: &Document) -> Vec<u8> {
    render(document, &options()).unwrap().0
}

/// The text of the glyphs whose origin is inside `rect` on `page`, in
/// drawing order, with runs of spaces as one.
fn text_in(drawn: &[Vec<Glyph>], heights: &[f64], page: usize, rect: [f64; 4]) -> String {
    let [left, bottom, right, top] = rect;
    let text: String = drawn[page]
        .iter()
        .filter(|g| {
            let y = heights[page] - g.y;
            g.x >= left - 0.001 && g.x < right - 0.001 && y > bottom && y < top
        })
        .map(|g| g.text.as_str())
        .collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of the line just below `top` (points from the page's bottom),
/// and where it starts.
fn line_below(drawn: &[Vec<Glyph>], heights: &[f64], page: usize, top: f64) -> (String, f64) {
    let baseline = drawn[page]
        .iter()
        .map(|g| heights[page] - g.y)
        .filter(|y| *y < top && *y > top - 30.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let line = [0.0, baseline - 0.01, f64::INFINITY, baseline + 0.01];
    let start = drawn[page]
        .iter()
        .filter(|g| (heights[page] - g.y - baseline).abs() < 0.01)
        .map(|g| g.x)
        .fold(f64::INFINITY, f64::min);
    (text_in(drawn, heights, page, line), start)
}

/// Each link's text and where it goes, the parts of a wrapped link joined.
fn linked_texts(pdf: &Pdf) -> Vec<(String, Goes, usize)> {
    let drawn = glyphs(pdf);
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    let mut out: Vec<(String, Goes, usize)> = Vec::new();
    let mut previous_bottom = f64::NAN;
    for annot in annots(pdf) {
        let [left, bottom, right, top] = annot.rect;
        let inside = [left + LINK_MARGIN, bottom, right - LINK_MARGIN, top];
        let text = text_in(&drawn, &heights, annot.page, inside);
        assert!(!text.is_empty(), "{annot:?} covers no text");
        let first = drawn[annot.page]
            .iter()
            .filter(|g| {
                let y = heights[annot.page] - g.y;
                g.x >= inside[0] - 0.001 && g.x < inside[2] && y > bottom && y < top
            })
            .map(|g| g.x)
            .fold(f64::INFINITY, f64::min);
        assert!(
            (first - inside[0]).abs() < 0.001,
            "{annot:?} starts at {first}"
        );
        let continues = out.last().is_some_and(|(_, goes, _)| *goes == annot.goes)
            && (annot.rect[3] - previous_bottom).abs() < 0.01;
        previous_bottom = annot.rect[1];
        match out.last_mut() {
            Some((joined, _, parts)) if continues => {
                joined.push(' ');
                joined.push_str(&text);
                *parts += 1;
            }
            _ => out.push((text, annot.goes, 1)),
        }
    }
    out
}

fn showcase() -> Document {
    let source =
        std::fs::read_to_string(support::repo_root().join("examples/markdown/showcase.md"))
            .unwrap();
    markdown(
        &source,
        &support::repo_root().join("examples/markdown/reference.docx"),
    )
}

/// The headings of a Markdown source outside code blocks: level and text.
fn markdown_headings(source: &str) -> Vec<(usize, String)> {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in source.lines() {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if fenced || hashes == 0 || !line[hashes..].starts_with(' ') {
            continue;
        }
        let text = line[hashes..].split(" {#").next().unwrap().trim();
        out.push((hashes, text.to_string()));
    }
    out
}

#[test]
fn every_showcase_link_lands_on_its_text_and_resolves() {
    let bytes = paint_for_tests(&showcase(), &fonts(), &font(LIBERATION)).unwrap();
    qpdf_check("showcase.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let drawn = glyphs(&pdf);
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    let links = linked_texts(&pdf);
    let uri = |uri: &str| Goes::Uri(uri.into());
    let body: Vec<(&str, &Goes)> = links
        .iter()
        .filter(|(_, goes, _)| matches!(goes, Goes::Uri(_)))
        .map(|(text, goes, _)| (text.as_str(), goes))
        .collect();
    assert_eq!(
        body,
        [
            ("example.com", &uri("https://example.com")),
            (
                "https://www.rust-lang.org",
                &uri("https://www.rust-lang.org")
            ),
            ("team@example.com", &uri("mailto:team@example.com")),
            ("links", &uri("https://example.com")),
        ]
    );
    let headings = markdown_headings(
        &std::fs::read_to_string(support::repo_root().join("examples/markdown/showcase.md"))
            .unwrap(),
    );
    let internal: Vec<(&str, &Goes)> = links
        .iter()
        .filter(|(_, goes, _)| matches!(goes, Goes::At(..)))
        .map(|(text, goes, _)| (text.as_str(), goes))
        .collect();
    // The contents entries, one per heading, then the two internal links.
    assert_eq!(internal.len(), headings.len() + 2, "{internal:?}");
    let (toc, jumps) = internal.split_at(headings.len());
    for ((text, goes), (_, heading)) in toc.iter().zip(&headings) {
        assert!(
            text.starts_with(heading.as_str()),
            "{text:?} for {heading:?}"
        );
        let Goes::At(page, _, top) = goes else {
            unreachable!()
        };
        assert_eq!(&line_below(&drawn, &heights, *page, *top).0, heading);
        let number: String = text
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect();
        let number: String = number.chars().rev().collect();
        assert_eq!(number, (page + 1).to_string(), "{text:?} shows its page");
    }
    assert_eq!(jumps[0].0, "the results section");
    assert_eq!(jumps[1].0, "a custom anchor");
    for ((_, goes), heading) in jumps.iter().zip(["Tables", "Overview"]) {
        let Goes::At(page, left, top) = goes else {
            unreachable!()
        };
        let (text, start) = line_below(&drawn, &heights, *page, *top);
        assert_eq!(text, heading);
        assert!((left - (start - 2.16)).abs() < 0.001, "{left} for {start}");
    }
}

#[test]
fn the_showcase_outline_is_its_heading_tree() {
    let bytes = paint_for_tests(&showcase(), &fonts(), &font(LIBERATION)).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    let drawn = glyphs(&pdf);
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    let items = outline(&pdf);
    let tree: Vec<(usize, String)> = items
        .iter()
        .map(|item| (item.depth + 1, item.title.clone()))
        .collect();
    let source =
        std::fs::read_to_string(support::repo_root().join("examples/markdown/showcase.md"))
            .unwrap();
    assert_eq!(tree, markdown_headings(&source));
    for item in &items {
        let Goes::Page(page, top) = item.goes else {
            panic!("{item:?}");
        };
        assert_eq!(line_below(&drawn, &heights, page, top).0, item.title);
    }
    assert_eq!(
        pdf.get(pdf.catalog().unwrap(), "PageMode"),
        &Value::Name("UseOutlines".into())
    );
}

#[test]
fn a_wrapped_link_has_an_annotation_per_line_over_its_whole_text() {
    let words = "this link has so many words that it cannot possibly fit on the one line \
                 it starts on, so it wraps onto the next";
    let source = format!("Before the link [{words}](https://example.com/wrapped) and after it.\n");
    let bytes = pdf_of(&inter(&source));
    qpdf_check("wrapped.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(annots(&pdf).len(), 2);
    assert_eq!(
        linked_texts(&pdf),
        [(
            words.to_string(),
            Goes::Uri("https://example.com/wrapped".into()),
            2
        )]
    );
}

#[test]
fn a_link_on_a_justified_line_covers_its_widened_spaces() {
    let filler = "Justified text spreads its spaces so the line meets the right margin, ";
    let link = r#"<w:hyperlink r:id="rLink">"#.to_string()
        + &run("a link of several words")
        + "</w:hyperlink>";
    let width = |ppr: &str| {
        let body = para(ppr, &(run(filler) + &link + &run(&filler.repeat(3))));
        let pdf = Pdf::parse(&pdf_of(&raw(&body))).unwrap();
        assert_eq!(
            linked_texts(&pdf),
            [(
                "a link of several words".to_string(),
                Goes::Uri("https://example.com/raw".into()),
                1
            )]
        );
        let [left, _, right, _] = annots(&pdf)[0].rect;
        right - left
    };
    let (justified, left) = (width(r#"<w:jc w:val="both"/>"#), width(""));
    assert!(justified > left + 1.0, "{justified} against {left}");
}

#[test]
fn contents_fields_jump_to_their_toc_bookmarks() {
    let field = |name: &str, text: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK \l "{name}" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
            run(text)
        )
    };
    let heading = |name: &str, level: u8, text: &str| {
        para(
            &format!(r#"<w:outlineLvl w:val="{level}"/>"#),
            &format!(
                r#"<w:bookmarkStart w:id="0" w:name="{name}"/>{}<w:bookmarkEnd w:id="0"/>"#,
                run(text)
            ),
        )
    };
    let body = [
        para("", &field("_Toc101", "Start")),
        para(
            "",
            &format!(
                r#"<w:hyperlink w:anchor="_Toc102">{}</w:hyperlink>"#,
                run("Later")
            ),
        ),
        para("", PAGE_BREAK),
        heading("_Toc101", 0, "Start"),
        para("", PAGE_BREAK),
        heading("_Toc102", 1, "Later"),
    ]
    .concat();
    let bytes = pdf_of(&raw(&body));
    qpdf_check("contents.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let goes: Vec<(String, Goes)> = linked_texts(&pdf)
        .into_iter()
        .map(|(text, goes, _)| (text, goes))
        .collect();
    assert_eq!(
        goes,
        [
            ("Start".to_string(), Goes::At(1, 69.84, 720.0)),
            ("Later".to_string(), Goes::At(2, 69.84, 720.0)),
        ]
    );
    let tree: Vec<(usize, String, Goes)> = outline(&pdf)
        .into_iter()
        .map(|item| (item.depth, item.title, item.goes))
        .collect();
    assert_eq!(
        tree,
        [
            (0, "Start".to_string(), Goes::Page(1, 720.0)),
            (1, "Later".to_string(), Goes::Page(2, 720.0)),
        ]
    );
}

#[test]
fn top_goes_to_page_one_and_a_missing_bookmark_warns_with_no_link() {
    let link = |anchor: &str, text: &str| {
        format!(
            r#"<w:hyperlink w:anchor="{anchor}">{}</w:hyperlink>"#,
            run(text)
        )
    };
    let body = [
        para("", PAGE_BREAK),
        para(
            "",
            &(link("_top", "Back to the top") + &link("nowhere", " and away")),
        ),
        para("", &run("Between them.")),
        para(
            "",
            &format!(
                r#"<w:hyperlink r:id="rTop">{}</w:hyperlink>"#,
                run("Top again")
            ),
        ),
    ]
    .concat();
    let (bytes, report) = render(&raw(&body), &options()).unwrap();
    qpdf_check("top.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let goes: Vec<(String, Goes)> = linked_texts(&pdf)
        .into_iter()
        .map(|(text, goes, _)| (text, goes))
        .collect();
    let top = Goes::At(0, 69.84, 720.0);
    assert_eq!(
        goes,
        [
            ("Back to the top".to_string(), top.clone()),
            ("Top again".to_string(), top),
        ]
    );
    let missing: Vec<(&str, usize)> = report
        .warnings
        .iter()
        .filter(|w| w.code == "pdf_link_target_missing")
        .map(|w| (w.code, w.count))
        .collect();
    assert_eq!(missing, [("pdf_link_target_missing", 1)]);
}

#[test]
fn links_and_the_outline_can_be_left_out() {
    let source = "# Heading\n\nA [link](https://example.com) and [a jump](#heading).\n";
    let document = inter(source);
    let catalog = |options: &PdfOptions| {
        let bytes = render(&document, options).unwrap().0;
        let pdf = Pdf::parse(&bytes).unwrap();
        let mut keys: Vec<String> = pdf.catalog().unwrap().keys().cloned().collect();
        keys.retain(|key| ["Names", "Outlines", "PageMode"].contains(&key.as_str()));
        (keys, annots(&pdf).len())
    };
    let mut options = options();
    assert_eq!(
        catalog(&options),
        (
            vec!["Names".into(), "Outlines".into(), "PageMode".into()],
            2
        )
    );
    options.links = false;
    assert_eq!(
        catalog(&options),
        (vec!["Outlines".into(), "PageMode".into()], 0)
    );
    options.outline = false;
    assert_eq!(catalog(&options), (vec![], 0));
    options.links = true;
    assert_eq!(catalog(&options), (vec!["Names".into()], 2));
}

/// Word for Mac's PDF of `word-links.docx` (Arial 11 pt, 6 pt space after):
/// its link rectangles reach 2.16 pt past the text and down through the
/// space after. Drawn here in Liberation Sans, which has Arial's metrics;
/// Word's right edges are on a 0.24 pt grid.
#[test]
fn link_rects_are_word_s() {
    let document = Document::open_path_with(
        fixtures().join("pdf/word-links.docx"),
        &Limits::default(),
        Tolerance::word(),
    )
    .unwrap()
    .0;
    let mut builder = FontSet::builder();
    builder
        .add(font("liberation/LiberationSans-Regular.ttf"))
        .unwrap();
    builder.alias("Arial", "Liberation Sans");
    let mut options = PdfOptions::default();
    options.fonts = builder.build();
    let (bytes, report) = render(&document, &options).unwrap();
    let missing: Vec<usize> = report
        .warnings
        .iter()
        .filter(|w| w.code == "pdf_link_target_missing")
        .map(|w| w.count)
        .collect();
    assert_eq!(missing, [1]);
    let word = [
        [69.84, 682.80, 237.60, 701.28],
        [69.84, 664.08, 329.76, 682.80],
        [69.84, 626.64, 225.84, 645.36],
        [69.84, 589.44, 290.64, 608.16],
    ];
    let ours: Vec<[f64; 4]> = annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|annot| annot.rect)
        .collect();
    assert_eq!(ours.len(), word.len(), "{ours:?}");
    for (ours, word) in ours.iter().zip(word) {
        for (side, (a, b)) in ours.iter().zip(word).enumerate() {
            assert!(
                (a - b).abs() < 0.25,
                "side {side}: {ours:?} against {word:?}"
            );
        }
    }
}

#[test]
fn a_wrapped_link_s_first_line_stops_at_its_last_word_inside_the_margin() {
    let words = "word ".repeat(40);
    for ppr in ["", r#"<w:jc w:val="both"/>"#] {
        let link = format!(r#"<w:hyperlink r:id="rLink">{}</w:hyperlink>"#, run(&words));
        let pdf = Pdf::parse(&pdf_of(&raw(&para(ppr, &link)))).unwrap();
        let drawn = glyphs(&pdf);
        let rects: Vec<[f64; 4]> = annots(&pdf).into_iter().map(|a| a.rect).collect();
        assert!(rects.len() >= 2, "{rects:?}");
        let first = rects[0];
        let last_glyph = drawn[0]
            .iter()
            .filter(|g| g.text != " " && 792.0 - g.y < first[3] && 792.0 - g.y > first[1])
            .map(|g| g.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let d_width = 11.0 * 0.5;
        assert!(
            first[2] - LINK_MARGIN <= 540.0 + 0.001,
            "{ppr}: {first:?} passes the margin"
        );
        assert!(
            (first[2] - LINK_MARGIN - last_glyph) < d_width + 0.01,
            "{ppr}: {first:?} runs past its last word at {last_glyph}"
        );
    }
}

#[test]
fn a_link_of_only_a_tab_covers_the_tab() {
    let link = r#"<w:hyperlink r:id="rLink"><w:r><w:tab/></w:r></w:hyperlink>"#;
    let pdf = Pdf::parse(&pdf_of(&raw(&para("", link)))).unwrap();
    let rects: Vec<[f64; 4]> = annots(&pdf).into_iter().map(|a| a.rect).collect();
    assert_eq!(rects.len(), 1);
    let [left, _, right, _] = rects[0];
    assert!((left - (72.0 - LINK_MARGIN)).abs() < 0.001, "{rects:?}");
    assert!((right - (108.0 + LINK_MARGIN)).abs() < 0.001, "{rects:?}");
}

#[test]
fn a_hyperlink_field_to_a_hash_name_goes_to_the_bookmark() {
    let field = |instruction: &str, text: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
            run(text)
        )
    };
    let body = [
        para("", &field(r##"HYPERLINK "#Here""##, "By hash")),
        para("", &run("Between.")),
        para("", &field(r#"HYPERLINK \l "Here""#, "By switch")),
        para("", PAGE_BREAK),
        para(
            "",
            &(r#"<w:bookmarkStart w:id="0" w:name="Here"/>"#.to_string() + &run("Here")),
        ),
    ]
    .concat();
    let (bytes, report) = render(&raw(&body), &options()).unwrap();
    assert_eq!(report.warnings, []);
    let goes: Vec<(String, Goes)> = linked_texts(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|(text, goes, _)| (text, goes))
        .collect();
    let here = Goes::At(1, 69.84, 720.0);
    assert_eq!(
        goes,
        [
            ("By hash".to_string(), here.clone()),
            ("By switch".to_string(), here),
        ]
    );
}

/// Word writes bookmarks between blocks: before a table, between its rows
/// and deleted with tracked text. Each goes to the next paragraph.
#[test]
fn bookmarks_outside_paragraphs_go_to_the_next_paragraph() {
    let link = |name: &str| {
        para(
            "",
            &format!(
                r#"<w:hyperlink w:anchor="{name}">{}</w:hyperlink>"#,
                run(name)
            ),
        )
    };
    let cell = |text: &str| {
        format!(
            r#"<w:tc><w:tcPr><w:tcW w:w="4000" w:type="dxa"/></w:tcPr>{}</w:tc>"#,
            para("", &run(text))
        )
    };
    let body = [
        link("Loose"),
        para("", &run("-")),
        link("BeforeTable"),
        para("", &run("-")),
        link("Rows"),
        para("", &run("-")),
        link("Deleted"),
        para("", PAGE_BREAK),
        r#"<w:bookmarkStart w:id="1" w:name="Loose"/><w:bookmarkEnd w:id="1"/>"#.to_string(),
        para("", &run("After the loose bookmark")),
        r#"<w:bookmarkStart w:id="2" w:name="BeforeTable"/>"#.to_string(),
        format!(
            r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr>{}</w:tr><w:bookmarkStart w:id="3" w:name="Rows"/><w:tr>{}</w:tr></w:tbl>"#,
            cell("First row"),
            cell("Second row")
        ),
        para(
            "",
            &(r#"<w:del w:id="9" w:author="A"><w:bookmarkStart w:id="4" w:name="Deleted"/><w:r><w:delText>gone</w:delText></w:r></w:del>"#.to_string()
                + &run("Kept")),
        ),
    ]
    .concat();
    let document = raw(&body);
    let bytes = paint_for_tests(&document, &fonts(), &font(LIBERATION)).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    let drawn = glyphs(&pdf);
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    let landed: Vec<(String, String)> = linked_texts(&pdf)
        .into_iter()
        .map(|(text, goes, _)| {
            let Goes::At(page, _, top) = goes else {
                panic!("{text}: {goes:?}");
            };
            (text, line_below(&drawn, &heights, page, top).0)
        })
        .collect();
    let pairs =
        |pairs: [(&str, &str); 4]| pairs.map(|(a, b)| (a.to_string(), b.to_string())).to_vec();
    assert_eq!(
        landed,
        pairs([
            ("Loose", "After the loose bookmark"),
            ("BeforeTable", "First row"),
            ("Rows", "Second row"),
            ("Deleted", "Kept"),
        ])
    );
    assert!(
        preflight(&document)
            .unwrap()
            .warnings
            .iter()
            .all(|w| w.code != "pdf_link_target_missing")
    );
}
