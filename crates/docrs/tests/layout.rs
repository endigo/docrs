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

fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 1, 0, 0];
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([0u8; 6]);
    let mut offset = 12 + 16 * tables.len();
    for (tag, table) in tables {
        out.extend(*tag);
        out.extend(0u32.to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((table.len() as u32).to_be_bytes());
        offset += table.len();
    }
    for (_, table) in tables {
        out.extend(table);
    }
    out
}

/// A font mapping every code point, U+0000-10FFFF, in format 12 groups.
fn full_unicode_font() -> Vec<u8> {
    let mut head = vec![0u8; 54];
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
    let starts: Vec<u32> = (0..0x11_0000).step_by(0xFFFF).collect();
    let mut cmap = Vec::new();
    for v in [0u16, 1, 3, 10] {
        cmap.extend(v.to_be_bytes());
    }
    cmap.extend(12u32.to_be_bytes());
    cmap.extend([0, 12, 0, 0]);
    cmap.extend((16 + 12 * starts.len() as u32).to_be_bytes());
    cmap.extend([0u8; 4]);
    cmap.extend((starts.len() as u32).to_be_bytes());
    for start in starts {
        for v in [start, (start + 0xFFFE).min(0x10_FFFF), 0] {
            cmap.extend(v.to_be_bytes());
        }
    }
    sfnt(&[
        (b"cmap", cmap),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", vec![1, 244, 0, 0]),
        (b"maxp", vec![0, 0, 0x50, 0, 0xFF, 0xFF]),
    ])
}

/// `fonts` font table entries, each embedding `font` from one of `parts`
/// parts; with `keyed`, each under a different font key.
fn embedding_package(font: &[u8], fonts: usize, parts: usize, keyed: bool) -> Vec<u8> {
    let r = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\"><w:body><w:p><w:r><w:t>Text</w:t></w:r></w:p></w:body></w:document>"
    );
    let entries: String = (0..fonts)
        .map(|i| {
            let key = if keyed {
                format!(" w:fontKey=\"{{{i:08X}-0000-0000-0000-000000000000}}\"")
            } else {
                String::new()
            };
            format!(
                "<w:font w:name=\"F{i}\"><w:embedRegular r:id=\"rId{}\"{key}/></w:font>",
                i % parts
            )
        })
        .collect();
    let font_table = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:fonts xmlns:w=\"{W}\" xmlns:r=\"{r}\">{entries}</w:fonts>"
    );
    let font_rels: String = (0..parts)
        .map(|j| {
            format!(
                "<Relationship Id=\"rId{j}\" Type=\"{r}/font\" Target=\"fonts/font{j}.odttf\"/>"
            )
        })
        .collect();
    let relationships = |inner: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{inner}</Relationships>"
        )
    };
    let ct = CT_XML.replace(
        "</Types>",
        "<Default Extension=\"odttf\" ContentType=\"application/vnd.openxmlformats-officedocument.obfuscatedFont\"/><Override PartName=\"/word/fontTable.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml\"/></Types>",
    );
    let document_rels = relationships(&format!(
        "<Relationship Id=\"rId1\" Type=\"{r}/fontTable\" Target=\"fontTable.xml\"/>"
    ));
    let font_rels = relationships(&font_rels);
    let names: Vec<String> = (0..parts)
        .map(|j| format!("word/fonts/font{j}.odttf"))
        .collect();
    let mut members = vec![
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", document_rels.as_bytes()),
        RawMember::new("word/fontTable.xml", font_table.as_bytes()),
        RawMember::new("word/_rels/fontTable.xml.rels", font_rels.as_bytes()),
    ];
    members.extend(names.iter().map(|name| RawMember::new(name, font)));
    build_zip(&members)
}

fn layout_of(package: &[u8]) -> docrs::layout::Layout {
    let doc = Document::open_bytes(package, &Limits::default()).unwrap();
    let txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    layout(&txn, &main, Vec::new()).unwrap()
}

#[test]
fn one_font_embedded_a_thousand_times_is_read_once() {
    let package = embedding_package(&full_unicode_font(), 1000, 1, false);
    assert!(package.len() < 8 * 1024, "{}", package.len());
    let started = std::time::Instant::now();
    assert_eq!(layout_of(&package).embedded_fonts.len(), 1000);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_large_part_under_many_font_keys_is_copied_once() {
    let package = embedding_package(&vec![0; 64 << 20], 2000, 1, true);
    let started = std::time::Instant::now();
    assert!(layout_of(&package).embedded_fonts.is_empty());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn embedded_fonts_share_one_budget_and_report_what_it_leaves_unread() {
    let estimate = layout_of(&embedding_package(&full_unicode_font(), 8, 8, false));
    assert_eq!(estimate.embedded_fonts, ["F0", "F1", "F2", "F3"]);
    assert_eq!(estimate.missing_fonts, ["F4", "F5", "F6", "F7"]);
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

const SECT_PR: &str = "<w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"0\" w:footer=\"0\" w:gutter=\"0\"/>";

/// A package with `body` and `styles` (inner XML of `w:styles`).
fn styled_package(body: &str, styles: &str) -> Vec<u8> {
    package_with(
        &format!("{body}<w:sectPr>{SECT_PR}</w:sectPr>"),
        styles,
        None,
    )
}

fn package_with(body: &str, styles: &str, settings: Option<&str>) -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\"><w:body>{body}</w:body></w:document>"
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"{W}\"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>{styles}</w:styles>"
    );
    let override_for = |part: &str, kind: &str| {
        format!(
            "<Override PartName=\"/word/{part}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml\"/>"
        )
    };
    let rel = |id: &str, kind: &str| {
        format!(
            "<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}\" Target=\"{kind}.xml\"/>"
        )
    };
    let mut overrides = override_for("styles", "styles");
    let mut rels = rel("rId1", "styles");
    let settings = settings.map(|inner| {
        overrides += &override_for("settings", "settings");
        rels += &rel("rId2", "settings");
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"{W}\">{inner}</w:settings>"
        )
    });
    let ct = CT_XML.replace("</Types>", &format!("{overrides}</Types>"));
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{rels}</Relationships>"
    );
    let mut members = vec![
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
    ];
    if let Some(settings) = &settings {
        members.push(RawMember::new("word/settings.xml", settings.as_bytes()));
    }
    build_zip(&members)
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

#[test]
fn a_field_nested_in_field_code_measures_nothing_of_its_result() {
    let fld = |kind: &str| format!("<w:r><w:fldChar w:fldCharType=\"{kind}\"/></w:r>");
    let instr =
        |code: &str| format!("<w:r><w:instrText xml:space=\"preserve\">{code}</w:instrText></w:r>");
    let text = |t: &str| format!("<w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r>");
    let nested = format!(
        "<w:p>{}{}{}{}{}{}{}{}{}{}{}</w:p>",
        fld("begin"),
        instr("IF "),
        fld("begin"),
        instr(" MERGEFIELD Name "),
        fld("separate"),
        text(&"word ".repeat(40)),
        fld("end"),
        instr(" = \"\" \"a\" \"b\" "),
        fld("separate"),
        text("a"),
        fld("end"),
    );
    let plain = para("a", "");
    let after = para("after", "");
    let y = |first: &str| estimate(&format!("{first}{after}"), "").placed[1].y;
    assert!((y(&nested) - y(&plain)).abs() < 0.01);
}

fn page_ref(bookmark: &str, switches: &str, result: &str) -> String {
    format!(
        "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGEREF {bookmark} {switches} </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{result}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>"
    )
}

/// As Word writes an entry: a web-hidden PAGEREF result.
fn word_entry(bookmark: &str, label: &str, shown: &str, switches: &str) -> String {
    let field = page_ref(
        bookmark,
        switches,
        &format!("<w:r><w:rPr><w:webHidden/></w:rPr><w:t>{shown}</w:t></w:r>"),
    );
    entry(bookmark, label, &field)
}

fn entry(bookmark: &str, label: &str, field: &str) -> String {
    format!(
        "<w:hyperlink w:anchor=\"{bookmark}\" w:history=\"1\"><w:r><w:t>{label}</w:t></w:r><w:r><w:tab/></w:r>{field}</w:hyperlink>"
    )
}

fn word_toc(entries: &[String]) -> String {
    let paragraphs: String = entries.iter().map(|e| format!("<w:p>{e}</w:p>")).collect();
    format!(
        "<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent>\
         <w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-3\" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r></w:p>\
         {paragraphs}<w:p><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p></w:sdtContent></w:sdt>"
    )
}

fn heading(bookmark: &str, text: &str, ppr: &str) -> String {
    format!(
        "<w:p><w:pPr><w:pageBreakBefore/>{ppr}</w:pPr><w:bookmarkStart w:id=\"0\" w:name=\"{bookmark}\"/><w:r><w:t>{text}</w:t></w:r><w:bookmarkEnd w:id=\"0\"/></w:p>"
    )
}

fn entry_numbers(doc: &Document) -> Vec<String> {
    let xml = document_xml(doc);
    xml.split("<w:hyperlink ")
        .skip(1)
        .map(|chunk| {
            let mut chunk = &chunk[..chunk.find("</w:hyperlink>").unwrap()];
            let mut text = String::new();
            while let Some(open) = chunk.find("<w:t>") {
                chunk = &chunk[open + 5..];
                text.push_str(&chunk[..chunk.find('<').unwrap()]);
            }
            text.trim_start_matches(char::is_alphabetic).to_string()
        })
        .collect()
}

fn refresh(doc: &mut Document) -> usize {
    docrs::layout::refresh_toc_page_numbers(doc, false)
        .unwrap()
        .changed
}

fn open(bytes: &[u8]) -> Document {
    Document::open_bytes(bytes, &Limits::default()).unwrap()
}

#[test]
fn refresh_rewrites_the_page_numbers_of_a_contents_word_wrote() {
    let toc = word_toc(&[
        word_entry("_Toc1", "One", "7", "\\h"),
        word_entry("_Toc2", "Two", "1", "\\h \\* MERGEFORMAT"),
        word_entry("_Toc2", "Two", "1", "\\p"),
        entry(
            "_Toc2",
            "Two",
            "<w:fldSimple w:instr=\" PAGEREF _Toc2 \\h \"><w:r><w:t>8</w:t></w:r></w:fldSimple>",
        ),
    ]);
    let outside = format!(
        "<w:p>{}</w:p>",
        page_ref("_Toc2", "\\h", "<w:r><w:t>9</w:t></w:r>")
    );
    let body = format!(
        "{toc}{outside}{}{}",
        heading("_Toc1", "One", ""),
        heading("_Toc2", "Two", "")
    );
    let mut doc = open(&styled_package(&body, ""));
    assert_eq!(refresh(&mut doc), 3);
    assert_eq!(entry_numbers(&doc), ["2", "3", "1", "3"]);
    assert!(document_xml(&doc).contains("<w:t>9</w:t>"));
    assert_eq!(refresh(&mut doc), 0);
}

#[test]
fn refresh_leaves_page_numbers_it_cannot_read_whole() {
    let split = page_ref(
        "_Toc1",
        "\\h",
        "<w:r><w:t>1</w:t></w:r><w:r><w:t>2</w:t></w:r>",
    );
    let deleted_begin = format!(
        "<w:del w:id=\"1\" w:author=\"A\"><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r></w:del>{}",
        &page_ref("_Toc1", "\\h", "<w:r><w:t>5</w:t></w:r>")
            ["<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>".len()..]
    );
    let unclosed = "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGEREF _Toc1 \\h </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>6</w:t></w:r>";
    let toc = word_toc(&[
        entry("_Toc1", "Split", &split),
        entry("_Toc1", "Deleted", &deleted_begin),
        word_entry("_Toc1", "Whole", "7", "\\h"),
        entry("_Toc1", "Unclosed", unclosed),
    ]);
    let body = format!("{toc}{}", heading("_Toc1", "One", ""));
    let mut doc = open(&styled_package(&body, ""));
    assert_eq!(refresh(&mut doc), 1);
    assert_eq!(entry_numbers(&doc), ["12", "5", "2", "6"]);
}

#[test]
fn refresh_leaves_roman_and_restarted_page_numbers() {
    let toc = word_toc(&[
        word_entry("_Toc1", "One", "7", "\\h"),
        word_entry("_Toc2", "Two", "9", "\\h"),
    ]);
    let roman = format!(
        "{toc}{}{}<w:sectPr>{SECT_PR}<w:pgNumType w:fmt=\"lowerRoman\"/></w:sectPr>",
        heading("_Toc1", "One", ""),
        heading("_Toc2", "Two", "")
    );
    let mut doc = open(&package_with(&roman, "", None));
    assert_eq!(refresh(&mut doc), 0);
    assert_eq!(entry_numbers(&doc), ["7", "9"]);

    let restarted = format!(
        "{toc}{}{}<w:sectPr>{SECT_PR}<w:pgNumType w:start=\"1\"/></w:sectPr>",
        heading("_Toc1", "One", &format!("<w:sectPr>{SECT_PR}</w:sectPr>")),
        heading("_Toc2", "Two", "")
    );
    let mut doc = open(&package_with(&restarted, "", None));
    assert_eq!(refresh(&mut doc), 1);
    assert_eq!(entry_numbers(&doc), ["2", "9"]);

    let break_in_sdt = format!(
        "{toc}{}<w:sdt><w:sdtContent><w:p><w:pPr><w:sectPr>{SECT_PR}</w:sectPr></w:pPr></w:p></w:sdtContent></w:sdt>{}<w:sectPr>{SECT_PR}<w:pgNumType w:start=\"1\"/></w:sectPr>",
        heading("_Toc1", "One", ""),
        heading("_Toc2", "Two", "")
    );
    let mut doc = open(&package_with(&break_in_sdt, "", None));
    assert_eq!(refresh(&mut doc), 1);
    assert_eq!(entry_numbers(&doc), ["2", "9"]);
}

#[test]
fn an_unclosed_entry_field_does_not_keep_the_contents_open() {
    let unclosed = "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGEREF _Toc1 \\h </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>6</w:t></w:r>";
    let toc = word_toc(&[
        entry("_Toc1", "Unclosed", unclosed),
        word_entry("_Toc1", "Whole", "7", "\\h"),
    ]);
    let outside = format!(
        "<w:p>{}</w:p>",
        page_ref("_Toc1", "\\h", "<w:r><w:t>9</w:t></w:r>")
    );
    let body = format!("{toc}{outside}{}", heading("_Toc1", "One", ""));
    let mut doc = open(&styled_package(&body, ""));
    assert_eq!(refresh(&mut doc), 1);
    assert_eq!(entry_numbers(&doc), ["6", "2"]);
    assert!(document_xml(&doc).contains("<w:t>9</w:t>"));
}

fn stale_contents() -> String {
    format!(
        "{}{}<w:sectPr>{SECT_PR}</w:sectPr>",
        word_toc(&[word_entry("_Toc1", "One", "7", "\\h")]),
        heading("_Toc1", "One", "")
    )
}

#[test]
fn a_protected_document_is_refused_and_left_byte_for_byte() {
    let settings = "<w:documentProtection w:edit=\"readOnly\" w:enforcement=\"1\"/>";
    let mut doc = open(&package_with(&stale_contents(), "", Some(settings)));
    let (before, _) = doc.patch_bytes().unwrap();
    let err = docrs::layout::refresh_toc_page_numbers(&mut doc, false).unwrap_err();
    assert_eq!(err.code(), "document_protected", "{err}");
    let (after, _) = doc.patch_bytes().unwrap();
    assert!(before == after);
}

#[test]
fn tracked_changes_refresh_the_cached_result_untracked() {
    let settings =
        "<w:trackRevisions/><w:documentProtection w:edit=\"trackedChanges\" w:enforcement=\"1\"/>";
    let mut doc = open(&package_with(&stale_contents(), "", Some(settings)));
    assert_eq!(refresh(&mut doc), 1);
    assert_eq!(entry_numbers(&doc), ["2"]);
    let xml = document_xml(&doc);
    assert!(
        !xml.contains("<w:ins ") && !xml.contains("<w:del "),
        "{xml}"
    );
}

#[test]
fn a_conversion_refreshes_its_own_contents_only() {
    let mut doc = open(&package_with(&stale_contents(), "", None));
    let options = docrs::markdown::MarkdownOptions {
        placement: docrs::markdown::Placement::Append,
        ..Default::default()
    };
    docrs::markdown::insert_markdown(&mut doc, "[TOC]\n\n# New\n\nDear {{Client}}.\n", &options)
        .unwrap();
    doc.replace_all("{{Client}}", "Jo", &Default::default())
        .unwrap();
    let own = docrs::layout::refresh_toc_page_numbers(&mut doc, true).unwrap();
    assert_eq!(own.changed, 0);
    assert_eq!(entry_numbers(&doc), ["7", "2"]);
    assert_eq!(refresh(&mut doc), 1);
    assert_eq!(entry_numbers(&doc), ["2", "2"]);
}

#[test]
fn placed_paragraphs_are_the_body_s_only() {
    // Node ids are per part: header paragraphs must not show.
    let r = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let header = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:hdr xmlns:w=\"{W}\"><w:p><w:bookmarkStart w:id=\"1\" w:name=\"inHeader\"/><w:r><w:t>Header</w:t></w:r></w:p><w:p/><w:p/></w:hdr>"
    );
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\" xmlns:r=\"{r}\"><w:body><w:p><w:r><w:t>One</w:t></w:r></w:p><w:p><w:bookmarkStart w:id=\"2\" w:name=\"inBody\"/><w:r><w:t>Two</w:t></w:r></w:p><w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId1\"/></w:sectPr></w:body></w:document>"
    );
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/></Types>",
    );
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{r}/header\" Target=\"header1.xml\"/></Relationships>"
    );
    let bytes = build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/header1.xml", header.as_bytes()),
    ]);
    let doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
    let txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    let estimate = layout(&txn, &main, Vec::new()).unwrap();
    let tree = txn.tree(&main).unwrap();
    let texts: Vec<String> = estimate
        .placed
        .iter()
        .map(|p| tree.text_content(p.node.unwrap()))
        .collect();
    assert_eq!(texts, ["One", "Two"]);
    assert_eq!(estimate.bookmark_page("inBody"), Some(1));
    assert_eq!(estimate.bookmark_page("inHeader"), None);
}

#[test]
fn tables_nested_past_32_levels_are_measured_as_empty() {
    // Sixty tables, each in the one cell of the one before; each cell's
    // paragraph is bookmarked with its table's level.
    let mut body = String::new();
    for level in (1..=60).rev() {
        body = format!(
            "<w:tbl><w:tr><w:tc><w:p><w:bookmarkStart w:id=\"{level}\" w:name=\"level{level}\"/><w:r><w:t>{level}</w:t></w:r></w:p>{body}<w:p/></w:tc></w:tr></w:tbl>"
        );
    }
    let estimate = estimate(&format!("{body}{}", para("after", "")), "");
    assert_eq!(estimate.bookmark_page("level32"), Some(1));
    assert_eq!(estimate.bookmark_page("level33"), None);
    assert_eq!(estimate.bookmark_page("level60"), None);
}

#[test]
fn absurd_numbers_count_as_absent() {
    let long = "word ".repeat(40);
    let plain = estimate(&format!("{}{}", para(&long, ""), para("after", "")), "");
    for (ppr, rpr) in [
        ("<w:ind w:left=\"1e300\" w:right=\"NaN\"/>", ""),
        ("<w:spacing w:after=\"inf\" w:line=\"1e12\"/>", ""),
        ("<w:ind w:firstLine=\"-40000\"/>", ""),
        ("", "<w:sz w:val=\"NaN\"/>"),
        ("", "<w:sz w:val=\"1e400\"/>"),
        ("", "<w:sz w:val=\"4000\"/><w:spacing w:val=\"inf\"/>"),
        ("", "<w:sz w:val=\"0\"/><w:w w:val=\"1e9\"/>"),
    ] {
        let first = format!(
            "<w:p><w:pPr>{ppr}</w:pPr><w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{long}</w:t></w:r></w:p>"
        );
        let odd = estimate(&format!("{first}{}", para("after", "")), "");
        assert_eq!(odd.placed[1].y, plain.placed[1].y, "{ppr}{rpr}");
    }
}
