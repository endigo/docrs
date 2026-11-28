//! Tables in the PDF, read back by the test reader: cell fills behind the
//! text, every border style where Word draws it against layout's rows,
//! repeated headers, merges and cuts across pages, nested tables and the
//! cells' text in reading order.

mod support;

#[allow(dead_code)]
#[path = "support/glyphs.rs"]
mod glyphs;
#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;
#[allow(dead_code)]
#[path = "support/shapes.rs"]
mod shapes;

use docrs::Document;
use docrs::layout::{FontSet, Record, RowRecord};
use docrs::pdf::{PdfOptions, preflight, render};
use docrs::txn::Txn;
use glyphs::qpdf_check;
use reader::Pdf;
use shapes::{Shape, coloured, shapes};
use support::*;

/// Word for Mac's device unit: positions snap to it and lines are drawn in
/// whole units.
const DEVICE: f64 = 0.24;

fn device(value: f64) -> f64 {
    (value / DEVICE).round() * DEVICE
}

fn fonts() -> FontSet {
    let mut builder = FontSet::builder();
    for path in [
        "inter/Inter[opsz,wght].ttf",
        "liberation/LiberationSerif-Regular.ttf",
    ] {
        builder
            .add(std::fs::read(repo_root().join("tests/fixtures/fonts").join(path)).unwrap())
            .unwrap();
    }
    builder.build()
}

fn para(text: &str) -> String {
    format!(
        r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Inter" w:hAnsi="Inter"/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#
    )
}

/// A cell 2400 twips (120 pt) wide per grid column it spans.
fn cell(tc_pr: &str, content: &str) -> String {
    format!(r#"<w:tc><w:tcPr><w:tcW w:w="2400" w:type="dxa"/>{tc_pr}</w:tcPr>{content}</w:tc>"#)
}

fn row(tr_pr: &str, cells: &[String]) -> String {
    format!("<w:tr><w:trPr>{tr_pr}</w:trPr>{}</w:tr>", cells.concat())
}

fn table(tbl_pr: &str, columns: usize, rows: &[String]) -> String {
    format!(
        r#"<w:tbl><w:tblPr><w:tblLayout w:type="fixed"/>{tbl_pr}</w:tblPr><w:tblGrid>{}</w:tblGrid>{}</w:tbl>"#,
        r#"<w:gridCol w:w="2400"/>"#.repeat(columns),
        rows.concat()
    )
}

/// `w:tblBorders` with `(edge, style, eighths, colour)` lines.
fn borders(edges: &[(&str, &str, u32, &str)]) -> String {
    let lines: String = edges
        .iter()
        .map(|(edge, style, size, colour)| {
            format!(r#"<w:{edge} w:val="{style}" w:sz="{size}" w:space="0" w:color="{colour}"/>"#)
        })
        .collect();
    format!("<w:tblBorders>{lines}</w:tblBorders>")
}

fn all_borders(style: &str, size: u32, colour: &str) -> String {
    let edges: Vec<_> = ["top", "left", "bottom", "right", "insideH", "insideV"]
        .into_iter()
        .map(|edge| (edge, style, size, colour))
        .collect();
    borders(&edges)
}

fn document(body: &str) -> Document {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    open_bytes(&minimal_package_with(&xml))
}

/// The PDF, through the preflight.
fn pdf(document: &Document) -> Vec<u8> {
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    let (bytes, report) = render(document, &options).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    bytes
}

fn row_records(document: &Document) -> Vec<RowRecord> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name().to_string();
    let mut out = Vec::new();
    docrs::layout::records_with(
        &txn,
        &main,
        &fonts(),
        |_, _| Ok(()),
        |record| {
            if let Record::Row(row) = record {
                out.push(row);
            }
            Ok(())
        },
    )
    .unwrap();
    out
}

/// Each page's text in drawing order.
fn page_texts(bytes: &[u8]) -> Vec<String> {
    glyphs::glyphs(&Pdf::parse(bytes).unwrap())
        .iter()
        .map(|page| page.iter().map(|glyph| glyph.text.as_str()).collect())
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.002
}

/// The shapes of `colour` running across, top down.
fn across(shapes: &[Shape], colour: u32) -> Vec<Shape> {
    let mut out: Vec<Shape> = coloured(shapes, colour)
        .into_iter()
        .filter(Shape::horizontal)
        .collect();
    out.sort_by(|a, b| a.top.total_cmp(&b.top));
    out
}

/// The shapes of `colour` running down, left to right.
fn down(shapes: &[Shape], colour: u32) -> Vec<Shape> {
    let mut out: Vec<Shape> = coloured(shapes, colour)
        .into_iter()
        .filter(|shape| !shape.horizontal())
        .collect();
    out.sort_by(|a, b| a.left.total_cmp(&b.left));
    out
}

const STYLES: [(&str, u32); 9] = [
    ("single", 0xC00001),
    ("thick", 0xC00002),
    ("double", 0xC00003),
    ("triple", 0xC00004),
    ("dotted", 0xC00005),
    ("dashed", 0xC00006),
    ("dashSmallGap", 0xC00007),
    ("dotDash", 0xC00008),
    ("dotDotDash", 0xC00009),
];

/// A broken style's first dash and period, a 1 pt line drawn 0.96 thick
/// (Word for Mac's PDFs, as paragraph borders).
fn dash(style: &str) -> Option<(f64, f64)> {
    Some(match style {
        "dotted" => (0.96, 1.92),
        "dashed" => (3.84, 7.68),
        "dashSmallGap" => (3.84, 4.8),
        "dotDash" => (6.72, 15.36),
        "dotDotDash" => (5.76, 15.36),
        _ => return None,
    })
}

#[test]
fn every_border_style_is_drawn_on_its_side_of_the_grid_line() {
    let body: String = STYLES
        .iter()
        .map(|(style, colour)| {
            para(style)
                + &table(
                    &all_borders(style, 8, &format!("{colour:06X}")),
                    2,
                    &[row("", &[cell("", &para("a")), cell("", &para("b"))])],
                )
        })
        .collect();
    let document = document(&body);
    let rows = row_records(&document);
    let page = &shapes(&pdf(&document))[0];
    for ((style, colour), row) in STYLES.iter().zip(&rows) {
        let (top, bottom) = (device(row.y), device(row.y + row.height));
        let (left, right) = (device(row.x), device(row.x + row.width));
        let sides = [left, device(row.cells[1].x), right];
        let across = across(page, *colour);
        let down = down(page, *colour);
        if let Some(pattern) = dash(style) {
            // Stroked along the middle of one thickness below, right of or
            // above the grid line.
            assert_eq!((across.len(), down.len()), (2, 3), "{style}: {page:?}");
            for line in across.iter().chain(&down) {
                assert!(line.stroked && close(line.thickness(), 0.96), "{style}");
                let (first, period) = line.dash.unwrap();
                assert!(
                    close(first, pattern.0) && close(period, pattern.1),
                    "{style}"
                );
            }
            assert!(close(across[0].top, top) && close(across[1].bottom, bottom));
            for (line, x) in down.iter().zip(sides) {
                assert!(close(line.left, x), "{style}: {line:?} {x}");
                assert!(close(line.top, top) && close(line.bottom, bottom));
            }
            continue;
        }
        // Each line 0.96 thick, the gaps between them as wide.
        let lines = match *style {
            "double" => 2,
            "triple" => 3,
            _ => 1,
        };
        assert_eq!(
            (across.len(), down.len()),
            (2 * lines, 3 * lines),
            "{style}: {page:?}"
        );
        for (k, (above, below)) in across[..lines]
            .iter()
            .zip(across[lines..].iter().rev())
            .enumerate()
        {
            let offset = 2.0 * k as f64 * 0.96;
            assert!(!above.stroked && close(above.thickness(), 0.96), "{style}");
            assert!(close(above.top, top + offset), "{style}: {above:?}");
            assert!(close(below.bottom, bottom - offset), "{style}: {below:?}");
            assert!(close(above.left, left) && close(above.right, right));
        }
        for (k, line) in down.iter().enumerate() {
            let x = sides[k / lines] + 2.0 * (k % lines) as f64 * 0.96;
            assert!(close(line.left, x), "{style}: {line:?}");
            assert!(close(line.top, top) && close(line.bottom, bottom));
        }
    }
}

#[test]
fn a_style_drawn_as_a_single_line_is_reported() {
    let body = table(
        &all_borders("wave", 8, "000000"),
        1,
        &[row("", &[cell("", &para("waves"))])],
    );
    let report = preflight(&document(&body)).unwrap();
    let approximated: Vec<_> = report
        .warnings
        .iter()
        .map(|w| (w.code, w.subject))
        .collect();
    assert_eq!(
        approximated,
        [("pdf_decoration_approximated", "table border")]
    );
}

#[test]
fn a_paragraphs_border_or_shading_in_a_cell_is_reported() {
    let boxed = r#"<w:p><w:pPr><w:shd w:val="clear" w:fill="EEEEEE"/></w:pPr></w:p>"#;
    let body = table("", 1, &[row("", &[cell("", boxed)])]) + boxed;
    let report = preflight(&document(&body)).unwrap();
    let found: Vec<_> = report
        .warnings
        .iter()
        .map(|w| (w.code, w.subject, w.count))
        .collect();
    assert_eq!(
        found,
        [(
            "pdf_not_yet_drawn",
            "paragraph border or shading in a table cell",
            1
        )]
    );
}

#[test]
fn cell_fills_are_behind_the_text_and_the_lines_over_it() {
    let shaded = |fill: &str| format!(r#"<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>"#);
    let highlighted = r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Inter" w:hAnsi="Inter"/><w:highlight w:val="yellow"/></w:rPr><w:t>marked</w:t></w:r></w:p>"#;
    let body = table(
        &all_borders("single", 8, "404040"),
        2,
        &[
            row(
                "",
                &[
                    cell(&shaded("FFC000"), &para("orange")),
                    cell(&shaded("D9E2F3"), highlighted),
                ],
            ),
            row("", &[cell("", &para("plain")), cell("", &para("plain"))]),
        ],
    );
    let document = document(&body);
    let rows = row_records(&document);
    let page = &shapes(&pdf(&document))[0];
    for (cell, fill) in rows[0].cells.iter().zip([0xFFC000, 0xD9E2F3]) {
        let filled = coloured(page, fill);
        assert_eq!(filled.len(), 1, "{page:?}");
        let rect = filled[0];
        assert!(!rect.over_text, "{rect:?}");
        assert!(close(rect.left, device(cell.x)) && close(rect.right, device(cell.x + cell.width)));
        assert!(close(rect.top, device(rows[0].y)));
        assert!(close(rect.bottom, device(rows[0].y + rows[0].height)));
    }
    let order = |colour: u32| page.iter().position(|s| s.colour == colour_bytes(colour));
    assert!(
        order(0xD9E2F3) < order(0xFFFF00),
        "the highlight is over the fill"
    );
    assert!(coloured(page, 0x404040).iter().all(|line| line.over_text));
}

fn colour_bytes(hex: u32) -> [u8; 3] {
    [16, 8, 0].map(|shift| (hex >> shift) as u8)
}

#[test]
fn a_header_row_is_painted_again_on_every_page_the_table_reaches() {
    let header = row(
        "<w:tblHeader/>",
        &[
            cell(r#"<w:shd w:val="clear" w:fill="1F4E79"/>"#, &para("Head A")),
            cell(r#"<w:shd w:val="clear" w:fill="1F4E79"/>"#, &para("Head B")),
        ],
    );
    let mut rows = vec![header];
    rows.extend((0..110).map(|n| {
        row(
            "",
            &[
                cell("", &para(&format!("row {n}"))),
                cell("", &para("value")),
            ],
        )
    }));
    let document = document(&table(&all_borders("single", 4, "404040"), 2, &rows));
    let bytes = pdf(&document);
    qpdf_check("headers.pdf", &bytes);
    let records = row_records(&document);
    let pages = shapes(&bytes);
    let texts = page_texts(&bytes);
    assert!(pages.len() >= 3, "{}", pages.len());
    for (index, (page, text)) in pages.iter().zip(&texts).enumerate() {
        let header = records
            .iter()
            .find(|r| r.page as usize == index + 1 && r.row == 0)
            .unwrap();
        assert_eq!(header.repeat, index > 0);
        let fills = coloured(page, 0x1F4E79);
        assert_eq!(fills.len(), 2, "page {}", index + 1);
        assert!(fills.iter().all(|f| close(f.top, device(header.y))));
        assert!(
            text.starts_with("Head AHead Brow"),
            "page {}: {text}",
            index + 1
        );
    }
}

#[test]
fn a_merged_cell_keeps_its_fill_across_a_page_and_no_line_inside() {
    let rows: Vec<String> = (0..60)
        .map(|n| {
            let merge = if n == 0 {
                r#"<w:vMerge w:val="restart"/><w:shd w:val="clear" w:fill="FFC000"/>"#
            } else {
                "<w:vMerge/>"
            };
            let first = if n == 0 { para("Merged") } else { para("") };
            row(
                "",
                &[cell(merge, &first), cell("", &para(&format!("row {n}")))],
            )
        })
        .collect();
    let document = document(&table(&all_borders("single", 8, "0070C0"), 2, &rows));
    let records = row_records(&document);
    let pages = shapes(&pdf(&document));
    assert_eq!(pages.len(), 2);
    let split = records[1].cells[1].x;
    for (index, page) in pages.iter().enumerate() {
        let on_page: Vec<_> = records
            .iter()
            .filter(|r| r.page as usize == index + 1)
            .collect();
        let (first, last) = (on_page[0], on_page[on_page.len() - 1]);
        let mut fills = coloured(page, 0xFFC000);
        fills.sort_by(|a, b| a.top.total_cmp(&b.top));
        assert!(close(fills[0].top, device(first.y)), "page {}", index + 1);
        assert!(close(
            fills[fills.len() - 1].bottom,
            device(last.y + last.height)
        ));
        assert!(
            fills
                .windows(2)
                .all(|pair| close(pair[0].bottom, pair[1].top))
        );
        // Only the table's top and bottom cross the merged column.
        let inside = across(page, 0x0070C0)
            .into_iter()
            .filter(|line| line.left < split - 1.0)
            .count();
        let expected = usize::from(first.row == 0) + usize::from(last.row == 59);
        assert_eq!(inside, expected, "page {}", index + 1);
    }
}

#[test]
fn a_row_cut_by_the_page_shows_the_tables_bottom_and_top_at_the_seam() {
    let lines: String = (0..70).map(|n| para(&format!("line {n}"))).collect();
    let edges = [
        ("top", "single", 24, "C00000"),
        ("bottom", "single", 24, "00B050"),
        ("left", "single", 8, "404040"),
        ("right", "single", 8, "404040"),
        ("insideH", "single", 8, "0070C0"),
        ("insideV", "single", 8, "404040"),
    ];
    let rows = [
        row("", &[cell("", &para("before")), cell("", &para("before"))]),
        row("", &[cell("", &lines), cell("", &para("short"))]),
        row("", &[cell("", &para("after")), cell("", &para("after"))]),
    ];
    let document = document(&table(&borders(&edges), 2, &rows));
    let records = row_records(&document);
    let pages = shapes(&pdf(&document));
    let cut: Vec<_> = records.iter().filter(|r| r.row == 1).collect();
    assert_eq!(cut.len(), 2, "{records:?}");
    let page_one = across(&pages[0], 0x00B050);
    assert_eq!(page_one.len(), 1, "{:?}", pages[0]);
    let bottom = device(cut[0].y + cut[0].height);
    assert!(close(page_one[0].bottom, bottom) && close(page_one[0].thickness(), 2.88));
    let page_two = across(&pages[1], 0xC00000);
    assert_eq!(page_two.len(), 1, "{:?}", pages[1]);
    assert!(close(page_two[0].top, device(cut[1].y)));
}

#[test]
fn a_nested_table_is_drawn_in_its_cell_over_the_cells_fill() {
    let inner = table(
        &all_borders("single", 8, "4472C4"),
        1,
        &[
            row(
                "",
                &[cell(
                    r#"<w:shd w:val="clear" w:fill="D9E2F3"/>"#,
                    &para("Inner 1"),
                )],
            ),
            row("", &[cell("", &para("Inner 2"))]),
        ],
    );
    let outer = table(
        &all_borders("single", 8, "404040"),
        2,
        &[row(
            "",
            &[
                cell(
                    r#"<w:shd w:val="clear" w:fill="FFF2CC"/>"#,
                    &(para("Outer A") + &inner + &para("Outer A after")),
                ),
                cell("", &para("Outer B")),
            ],
        )],
    );
    let document = document(&outer);
    let records = row_records(&document);
    let bytes = pdf(&document);
    let page = &shapes(&bytes)[0];
    let outer_fill = coloured(page, 0xFFF2CC)[0];
    let inner_fill = coloured(page, 0xD9E2F3)[0];
    let position = |shape: Shape| page.iter().position(|s| *s == shape).unwrap();
    assert!(position(outer_fill) < position(inner_fill));
    let lines = coloured(page, 0x4472C4);
    assert!(!lines.is_empty());
    for shape in lines.iter().chain([&inner_fill]) {
        assert!(shape.left >= outer_fill.left && shape.right <= outer_fill.right);
        assert!(shape.top >= outer_fill.top && shape.bottom <= outer_fill.bottom);
    }
    let nested = records.iter().filter(|r| r.width < 200.0).count();
    assert_eq!(nested, 2, "{records:?}");
    assert_eq!(
        page_texts(&bytes)[0],
        "Outer AInner 1Inner 2Outer A afterOuter B"
    );
}

#[test]
fn spans_and_row_exceptions_draw_their_own_lines_and_text_reads_cell_by_cell() {
    let exception = r#"<w:tblPrEx><w:tblBorders><w:insideV w:val="single" w:sz="8" w:space="0" w:color="00B050"/></w:tblBorders></w:tblPrEx>"#;
    let rows = [
        row("", &[cell("", &para("r1c1")), cell("", &para("r1c2"))]),
        row(
            "",
            &[format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="4800" w:type="dxa"/><w:gridSpan w:val="2"/></w:tcPr>{}</w:tc>"#,
                para("r2 spans")
            )],
        ),
        format!(
            "<w:tr>{exception}{}{}</w:tr>",
            cell("", &para("r3c1")),
            cell("", &para("r3c2"))
        ),
    ];
    let document = document(&table(&all_borders("single", 8, "404040"), 2, &rows));
    let records = row_records(&document);
    let bytes = pdf(&document);
    let page = &shapes(&bytes)[0];
    let middle = device(records[0].cells[1].x);
    let at_middle = |colour| -> Vec<Shape> {
        down(page, colour)
            .into_iter()
            .filter(|line| close(line.left, middle))
            .collect()
    };
    let grey = at_middle(0x404040);
    assert_eq!(grey.len(), 1, "{grey:?}");
    assert!(
        close(grey[0].bottom, device(records[1].y)),
        "no line in the span"
    );
    let green = at_middle(0x00B050);
    assert_eq!(green.len(), 1, "{green:?}");
    assert!(close(green[0].top, device(records[2].y)));
    assert_eq!(page_texts(&bytes)[0], "r1c1r1c2r2 spansr3c1r3c2");
}

#[test]
fn the_content_cap_counts_the_table_layer() {
    let filled = cell(r#"<w:shd w:val="clear" w:fill="EEEEEE"/>"#, "<w:p/>");
    let rows: Vec<String> = (0..2000)
        .map(|_| {
            row(
                r#"<w:trHeight w:val="20" w:hRule="exact"/>"#,
                &vec![filled.clone(); 4],
            )
        })
        .collect();
    let document = document(&table("", 4, &rows));
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.limits.max_output_bytes = 10_000;
    let err = render(&document, &options).unwrap_err();
    assert_eq!(err.limit_exceeded().unwrap().limit, "max_pdf_content_bytes");
}

#[test]
fn a_tables_fill_in_a_header_is_under_the_headers_text() {
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let header = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="{W}">{}</w:hdr>"#,
        table(
            &all_borders("single", 8, "404040"),
            1,
            &[row(
                "",
                &[cell(
                    r#"<w:shd w:val="clear" w:fill="FFC000"/>"#,
                    &para("Letterhead")
                )]
            )]
        )
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{}<w:sectPr><w:headerReference w:type="default" r:id="rId1"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        para("Body")
    );
    let types = CT_XML.replace(
        "</Types>",
        r#"<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/></Types>"#,
    );
    let rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/header" Target="header1.xml"/></Relationships>"#
    );
    let package = build_zip(&[
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/header1.xml", header.as_bytes()),
    ]);
    let bytes = pdf(&open_bytes(&package));
    let page = &shapes(&bytes)[0];
    let fill = coloured(page, 0xFFC000);
    assert_eq!(fill.len(), 1, "{page:?}");
    assert!(!fill[0].over_text, "drawn before the header's text");
    assert!(coloured(page, 0x404040).iter().all(|line| line.over_text));
    assert_eq!(page_texts(&bytes)[0], "LetterheadBody");
}

/// `body` in a package whose styles part holds `styles`.
fn styled_document(body: &str, styles: &str) -> Document {
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}">{styles}</w:styles>"#
    );
    let types = CT_XML.replace(
        "</Types>",
        r#"<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#,
    );
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    open_bytes(&build_zip(&[
        RawMember::new("[Content_Types].xml", types.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", xml.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
    ]))
}

#[test]
fn every_leading_header_row_takes_the_first_rows_look() {
    // direct-formatted-table.nested-json: five tblHeader rows under a look
    // with firstRow, each drawn in the first row's fill as Word does.
    let style = |first_row: &str| {
        format!(
            r#"<w:style w:type="table" w:styleId="Heads"><w:tblStylePr w:type="firstRow">{first_row}</w:tblStylePr><w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:val="clear" w:fill="DDEBF7"/></w:tcPr></w:tblStylePr></w:style>"#
        )
    };
    let tbl_pr = r#"<w:tblStyle w:val="Heads"/><w:tblLook w:firstRow="1" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#;
    let rows: Vec<String> = (0..7)
        .map(|n| {
            let tr_pr = if n < 3 { "<w:tblHeader/>" } else { "" };
            row(
                tr_pr,
                &[cell("", &para(&format!("r{n}"))), cell("", &para("v"))],
            )
        })
        .collect();
    let body = table(tbl_pr, 2, &rows);
    let head_fill = r#"<w:tcPr><w:shd w:val="clear" w:fill="1F4E79"/></w:tcPr>"#;
    let document = styled_document(&body, &style(head_fill));
    let records = row_records(&document);
    let page = &shapes(&pdf(&document))[0];
    let tops = |colour: u32| {
        let mut tops: Vec<f64> = coloured(page, colour).iter().map(|s| s.top).collect();
        tops.dedup_by(|a, b| close(*a, *b));
        tops
    };
    let at = |rows: &[usize]| -> Vec<f64> { rows.iter().map(|&r| device(records[r].y)).collect() };
    let heads = tops(0x1F4E79);
    assert_eq!(heads.len(), 3, "{heads:?}");
    assert!(heads.iter().zip(at(&[0, 1, 2])).all(|(a, b)| close(*a, b)));
    let bands = tops(0xDDEBF7);
    assert_eq!(
        bands.len(),
        2,
        "bands start below the header rows: {bands:?}"
    );
    assert!(bands.iter().zip(at(&[3, 5])).all(|(a, b)| close(*a, b)));

    // The preflight's scan sees the first row's paragraph look in each.
    let boxed = r#"<w:pPr><w:shd w:val="clear" w:fill="EEEEEE"/></w:pPr>"#;
    let report = preflight(&styled_document(&body, &style(boxed))).unwrap();
    let found: Vec<_> = report
        .warnings
        .iter()
        .map(|w| (w.subject, w.count))
        .collect();
    assert_eq!(found, [("paragraph border or shading in a table cell", 6)]);
}

#[test]
fn the_content_cap_counts_border_bytes_as_they_are_written() {
    let rows: Vec<String> = (0..40)
        .map(|_| row("", &vec![cell("", "<w:p/>"); 4]))
        .collect();
    let document = document(&table(&all_borders("dotDotDash", 4, "404040"), 4, &rows));
    let mut options = PdfOptions::default();
    options.fonts = fonts();
    options.limits.max_output_bytes = 100;
    let err = render(&document, &options).unwrap_err();
    let limit = err.limit_exceeded().unwrap();
    assert_eq!(limit.limit, "max_pdf_content_bytes");
    assert!(
        limit.observed < limit.allowed + 200,
        "stopped within a run of the cap, not at the page's close: {limit:?}"
    );
}
