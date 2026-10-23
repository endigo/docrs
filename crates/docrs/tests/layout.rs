//! Layout estimation contracts: fonts embedded in the document take
//! precedence over the bundled metrics table; Word for Mac's pagination
//! rules measured against Word (tools/layout/README.md).

mod support;

use docrs::layout::{fonts::deobfuscate, layout};
use docrs::txn::Txn;
use docrs::{Document, Limits};
use support::*;

/// A TrueType font with only the tables layout reads: 1000 units per em,
/// the given hhea ascender and descender, and every printable ASCII
/// character 500 units wide.
fn tiny_font(ascender: i16, descender: i16) -> Vec<u8> {
    let mut head = vec![0u8; 54];
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[4..6].copy_from_slice(&ascender.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-descender).to_be_bytes());
    hhea[34..36].copy_from_slice(&2u16.to_be_bytes());
    let hmtx: Vec<u8> = [0u16, 0, 500, 0]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    // cmap format 4: U+0020-007E to glyph 1, then the closing segment.
    let mut sub = Vec::new();
    for v in [4u16, 32, 0, 4, 4, 1, 0, 0x7E, 0xFFFF, 0, 0x20, 0xFFFF] {
        sub.extend(v.to_be_bytes());
    }
    for v in [1u16.wrapping_sub(0x20), 1, 0, 0] {
        sub.extend(v.to_be_bytes());
    }
    let mut cmap = Vec::new();
    for v in [0u16, 1, 3, 1] {
        cmap.extend(v.to_be_bytes());
    }
    cmap.extend(12u32.to_be_bytes());
    cmap.extend(sub);
    let tables: [(&[u8; 4], Vec<u8>); 4] = [
        (b"cmap", cmap),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
    ];
    let mut out = vec![0, 1, 0, 0];
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([0u8; 6]);
    let mut offset = 12 + 16 * tables.len();
    let mut data: Vec<u8> = Vec::new();
    for (tag, table) in &tables {
        out.extend(*tag);
        out.extend(0u32.to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((table.len() as u32).to_be_bytes());
        offset += table.len();
        data.extend(table);
    }
    out.extend(data);
    out
}

const KEY: &str = "{0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0}";

/// Two Arial 10-point paragraphs, with `font` embedded as Arial.
fn package(font: &[u8]) -> Vec<u8> {
    let w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let r = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let run = "<w:r><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"20\"/></w:rPr><w:t>Text</w:t></w:r>";
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{w}\"><w:body><w:p>{run}</w:p><w:p>{run}</w:p></w:body></w:document>"
    );
    let font_table = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:fonts xmlns:w=\"{w}\" xmlns:r=\"{r}\"><w:font w:name=\"Arial\"><w:family w:val=\"swiss\"/><w:embedRegular r:id=\"rId1\" w:fontKey=\"{KEY}\"/></w:font></w:fonts>"
    );
    let ct = CT_XML.replace(
        "</Types>",
        "<Default Extension=\"odttf\" ContentType=\"application/vnd.openxmlformats-officedocument.obfuscatedFont\"/><Override PartName=\"/word/fontTable.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml\"/></Types>",
    );
    let rels = |ty: &str, target: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{r}/{ty}\" Target=\"{target}\"/></Relationships>"
        )
    };
    let obfuscated = deobfuscate(font, KEY).unwrap();
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new(
            "word/_rels/document.xml.rels",
            rels("fontTable", "fontTable.xml").as_bytes(),
        ),
        RawMember::new("word/fontTable.xml", font_table.as_bytes()),
        RawMember::new(
            "word/_rels/fontTable.xml.rels",
            rels("font", "fonts/font1.odttf").as_bytes(),
        ),
        RawMember::new("word/fonts/font1.odttf", &obfuscated),
    ])
}

#[test]
fn an_embedded_font_is_measured_before_the_bundled_table() {
    // Ascender 2 em + descender 1 em: 30-point lines at 10 points, where
    // the bundled Arial gives 11.5.
    let doc = Document::open_bytes(&package(&tiny_font(2000, 1000)), &Limits::default()).unwrap();
    let txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    let estimate = layout(&txn, &main, Vec::new()).unwrap();
    assert_eq!(estimate.embedded_fonts, ["Arial"]);
    assert!(estimate.missing_fonts.is_empty());
    let pitch = estimate.placed[1].y - estimate.placed[0].y;
    assert!((pitch - 30.0).abs() < 1e-6, "{pitch}");

    let doc = Document::open_bytes(&package(b"not a font"), &Limits::default()).unwrap();
    let txn = Txn::new(&doc);
    let estimate = layout(&txn, &main, Vec::new()).unwrap();
    assert!(estimate.embedded_fonts.is_empty());
    let pitch = estimate.placed[1].y - estimate.placed[0].y;
    assert!((pitch - 11.499).abs() < 0.01, "{pitch}");
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// A package with `body` and `styles` (inner XML of `w:styles`).
fn styled_package(body: &str, styles: &str) -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"0\" w:footer=\"0\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"{W}\"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>{styles}</w:styles>"
    );
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>",
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>";
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
    ])
}

fn estimate(body: &str, styles: &str) -> docrs::layout::Layout {
    let doc = Document::open_bytes(&styled_package(body, styles), &Limits::default()).unwrap();
    let txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    layout(&txn, &main, Vec::new()).unwrap()
}

fn para(text: &str, ppr: &str) -> String {
    format!("<w:p><w:pPr>{ppr}</w:pPr><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

#[test]
fn widow_control_applies_unless_turned_off() {
    // 50 one-line paragraphs (Arial 11: 12.65 points) fill 632 of the
    // 648-point body, room for one line; a three-line paragraph then moves
    // whole with widow control (absent counts as on) and leaves one line
    // behind without.
    let fill: String = (0..50).map(|i| para(&format!("line {i}"), "")).collect();
    let long = "word ".repeat(40);
    for (ppr, next_page) in [("", 2), ("<w:widowControl w:val=\"0\"/>", 1)] {
        let body = format!("{fill}{}", para(&long, ppr));
        let estimate = estimate(&body, "");
        assert_eq!(estimate.placed[50].page, next_page, "{ppr:?}");
    }
}

/// Twelve one-line rows in table style `style`, the cells in paragraph
/// style `pstyle`; the row pitch.
fn row_pitch(styles: &str, style: &str, pstyle: &str) -> f64 {
    let cell = |t: &str| {
        format!(
            "<w:tc><w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>",
            para(t, &format!("<w:pStyle w:val=\"{pstyle}\"/>"))
        )
    };
    let rows: String = (0..12)
        .map(|i| format!("<w:tr>{}{}</w:tr>", cell(&format!("a{i}")), cell("b")))
        .collect();
    let body = format!(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"{style}\"/><w:tblW w:w=\"8000\" w:type=\"dxa\"/><w:tblLayout w:type=\"fixed\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"4000\"/><w:gridCol w:w=\"4000\"/></w:tblGrid>{rows}</w:tbl>{}",
        para("after", "")
    );
    let placed = estimate(&body, styles).placed;
    // Each row: two cell paragraphs and the row end.
    (placed[3].y - placed[0].y).abs()
}

#[test]
fn table_rows_add_their_borders_and_cells_drop_contextual_space_after() {
    let base = "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:tblPr><w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>";
    let lined = "<w:style w:type=\"table\" w:styleId=\"Lined\"><w:name w:val=\"Lined\"/><w:basedOn w:val=\"TableNormal\"/><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"000000\"/><w:insideH w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"000000\"/></w:tblBorders></w:tblPr></w:style>";
    let spaced = "<w:style w:type=\"paragraph\" w:styleId=\"Spaced\"><w:name w:val=\"Spaced\"/><w:pPr><w:spacing w:before=\"40\" w:after=\"40\"/></w:pPr></w:style><w:style w:type=\"paragraph\" w:styleId=\"Context\"><w:name w:val=\"Context\"/><w:basedOn w:val=\"Spaced\"/><w:pPr><w:contextualSpacing/></w:pPr></w:style>";
    let styles = format!("{base}{lined}{spaced}");
    let arial = 2355.0 / 2048.0 * 11.0;
    // A 1.5-point inside border adds 1.5 points to every row.
    assert!((row_pitch(&styles, "Lined", "Normal") - (arial + 1.5)).abs() < 0.01);
    // 2 points before and after; contextual spacing drops the after.
    assert!((row_pitch(&styles, "TableNormal", "Spaced") - (arial + 4.0)).abs() < 0.01);
    assert!((row_pitch(&styles, "TableNormal", "Context") - (arial + 2.0)).abs() < 0.01);
}

#[test]
fn a_table_style_based_on_a_later_style_takes_no_cell_margins_from_it() {
    // Word reads "Early" (which has table properties) before its base:
    // its cells get no left and right margins. 4000-twip cells hold 200
    // points; "W" in Arial 11 is 10.38 points wide, so 19 of them fit only
    // without the base's 5.4-point margins.
    let base = "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:tblPr><w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>";
    let style = |id: &str| {
        format!(
            "<w:style w:type=\"table\" w:styleId=\"{id}\"><w:name w:val=\"{id}\"/><w:basedOn w:val=\"TableNormal\"/><w:tblPr><w:tblStyleRowBandSize w:val=\"1\"/></w:tblPr></w:style>"
        )
    };
    let styles = format!("{}{base}{}", style("Early"), style("Late"));
    let text = "W".repeat(19);
    let cell = format!(
        "<w:tc><w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>",
        para(&text, "")
    );
    for (id, lines) in [("Early", 1.0), ("Late", 2.0)] {
        let body = format!(
            "<w:tbl><w:tblPr><w:tblStyle w:val=\"{id}\"/><w:tblW w:w=\"4000\" w:type=\"dxa\"/><w:tblLayout w:type=\"fixed\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid><w:tr>{cell}</w:tr></w:tbl>{}",
            para("after", "")
        );
        let placed = estimate(&body, &styles).placed;
        let height = placed[2].y - placed[0].y;
        assert!(
            (height - lines * 2355.0 / 2048.0 * 11.0).abs() < 0.01,
            "{id}: {height}"
        );
    }
}
