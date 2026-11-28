//! Links, named destinations and the outline, read back by the test reader.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;

use std::process::Command;

use docrs::layout::Link;
use docrs::pdf::{Info, Navigation, PdfWarning, Rect, linked_document_for_tests};
use docrs::{ErrorKind, Limits};
use reader::{Dict, Pdf, Value, decode_text};

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

#[derive(Debug, PartialEq)]
enum Goes {
    Uri(String),
    Page(usize, f64),
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
    assert_eq!((&items[2], &items[4]), (&Value::Null, &Value::Null));
    Goes::Page(
        pdf.page_index(&items[0]).unwrap(),
        items[3].number().unwrap(),
    )
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
            let key = string(pdf.get(action, "D"));
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
    nav.link(
        0,
        rect(72.0, 700.0, 140.0, 714.0),
        &uri("https://example.com/a"),
    );
    nav.link(
        0,
        rect(140.0, 700.0, 200.0, 714.0),
        &uri("https://example.com/a"),
    );
    nav.link(
        0,
        rect(72.0, 686.0, 120.0, 700.0),
        &uri("https://example.com/a"),
    );
    nav.link(1, rect(72.0, 500.0, 160.5, 512.25), &anchor("Results"));
    nav.link(1, rect(72.0, 480.0, 130.0, 492.0), &anchor("_Toc1"));
    nav.link(
        2,
        rect(300.0, 100.0, 340.0, 112.0),
        &uri("mailto:team@example.com"),
    );
    nav.link(2, rect(300.0, 80.0, 340.0, 92.0), &uri("tel:+1-555-0100"));
    nav.destination("Results", 2, 640.5);
    nav.destination("_Toc1", 0, 780.0);
    nav.destination("Appendix", 2, 300.0);
    nav.destination("Results", 1, 10.0);
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
                goes: Goes::Page(2, 640.5),
            },
            Annot {
                page: 1,
                rect: [72.0, 480.0, 130.0, 492.0],
                goes: Goes::Page(0, 780.0),
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

#[test]
fn parts_of_a_link_merge_only_on_one_line_when_they_touch() {
    let target = uri("http://example.com");
    let mut nav = Navigation::default();
    nav.link(0, rect(10.0, 10.0, 20.0, 20.0), &target);
    nav.link(0, rect(20.3, 9.0, 30.0, 22.0), &target);
    nav.link(0, rect(31.0, 10.0, 40.0, 20.0), &target);
    nav.link(0, rect(40.0, 10.0, 50.0, 20.0), &uri("http://example.org"));
    nav.link(1, rect(50.0, 10.0, 60.0, 20.0), &uri("http://example.org"));
    let (bytes, _) = write(2, nav);
    let rects: Vec<(usize, [f64; 4])> = annots(&Pdf::parse(&bytes).unwrap())
        .into_iter()
        .map(|annot| (annot.page, annot.rect))
        .collect();
    assert_eq!(
        rects,
        [
            (0, [10.0, 9.0, 30.0, 22.0]),
            (0, [31.0, 10.0, 40.0, 20.0]),
            (0, [40.0, 10.0, 50.0, 20.0]),
            (1, [50.0, 10.0, 60.0, 20.0]),
        ]
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
            ("Appendix".into(), Goes::Page(2, 300.0)),
            ("Results".into(), Goes::Page(2, 640.5)),
            ("_Toc1".into(), Goes::Page(0, 780.0)),
        ]
    );
}

#[test]
fn many_destinations_split_into_leaves_with_limits() {
    let mut nav = Navigation::default();
    let names: Vec<String> = (0..150).map(|i| format!("_Toc{i:04}")).collect();
    for (index, name) in names.iter().enumerate().rev() {
        nav.destination(name, index % 2, 700.0);
    }
    nav.destination("Zoë", 1, 10.0);
    nav.destination("Zoe", 0, 20.0);
    nav.link(0, rect(1.0, 1.0, 2.0, 2.0), &anchor("_Toc0149"));
    nav.link(0, rect(1.0, 5.0, 2.0, 6.0), &anchor("Zoë"));
    let (bytes, warnings) = write(2, nav);
    assert_eq!(warnings, []);
    let pdf = Pdf::parse(&bytes).unwrap();
    let keys: Vec<Vec<u8>> = dests(&pdf).into_iter().map(|(key, _)| key).collect();
    let mut expected: Vec<Vec<u8>> = names.iter().map(|n| n.as_bytes().to_vec()).collect();
    expected.extend(["Zoe".into(), "Zoë".as_bytes().to_vec()]);
    expected.sort();
    assert_eq!(keys, expected);
    let root = pdf.get(pdf.catalog().unwrap(), "Names").dict().unwrap();
    let tree = pdf.get(root, "Dests").dict().unwrap();
    assert!(matches!(pdf.get(tree, "Kids"), Value::Array(kids) if kids.len() == 3));
    let goes: Vec<Goes> = annots(&pdf).into_iter().map(|a| a.goes).collect();
    assert_eq!(goes, [Goes::Page(1, 700.0), Goes::Page(1, 10.0)]);
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
    nav.link(
        0,
        rect(1.0, 1.0, 2.0, 2.0),
        &uri("  https://example.com/Zoë a(1)\\b  "),
    );
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
        nav.link(
            0,
            rect(1.0, index as f64 * 10.0, 2.0, index as f64 * 10.0 + 5.0),
            &uri(target),
        );
    }
    nav.link(0, rect(1.0, 500.0, 2.0, 505.0), &uri("HTTPS://example.com"));
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
    nav.link(0, rect(1.0, 1.0, 2.0, 2.0), &anchor("Nowhere"));
    nav.link(0, rect(1.0, 10.0, 2.0, 12.0), &anchor("Résumé"));
    nav.link(0, rect(1.0, 20.0, 2.0, 22.0), &anchor("RÉSUMÉ"));
    nav.link(0, rect(1.0, 30.0, 2.0, 32.0), &anchor(""));
    nav.destination("résumé", 0, 100.0);
    nav.destination("RÉSUMÉ", 0, 50.0);
    let (bytes, warnings) = write(1, nav);
    assert_eq!(codes(&warnings), [("pdf_link_target_missing", 2)]);
    assert!(warnings[0].message.ends_with("(\"Nowhere\", \"\")"));
    let pdf = Pdf::parse(&bytes).unwrap();
    let goes: Vec<Goes> = annots(&pdf).into_iter().map(|a| a.goes).collect();
    assert_eq!(goes, [Goes::Page(0, 100.0), Goes::Page(0, 100.0)]);
    let keys: Vec<Vec<u8>> = dests(&pdf).into_iter().map(|(key, _)| key).collect();
    assert_eq!(keys, ["résumé".as_bytes()]);
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
    let mut nav = Navigation::default();
    nav.destination("Far", 3, 10.0);
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

#[test]
fn output_is_deterministic() {
    assert_eq!(write(3, sample()).0, write(3, sample()).0);
}

#[test]
fn qpdf_accepts_links_and_the_outline() {
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
    let path = dir.path().join("links.pdf");
    std::fs::write(&path, write(3, sample()).0).unwrap();
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{report}");
    assert!(
        report.contains("No syntax or stream encoding errors found"),
        "{report}"
    );
}
