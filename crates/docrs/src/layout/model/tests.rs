use super::look::{Border, BorderStyle};
use super::*;
use crate::Document;
use crate::layout::fonts::Face;
use crate::wml::M;
use crate::xml::parse_str;

/// The bundled document, with this body and these styles. Its theme is
/// Office 2007's: light 2 is EEECE1, accent 1 4F81BD.
fn model(body: &str, styles: &str) -> Model {
    model_with(body, styles, "", Vec::new())
}

/// With this settings part too, unless empty, and the document's own fonts `own`.
fn model_with(body: &str, styles: &str, settings: &str, own: Vec<Face>) -> Model {
    let doc = Document::new();
    let mut txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    let styles_part = txn.part_related_by(&main, rt::STYLES).unwrap().unwrap();
    *txn.tree_mut(&main).unwrap() = parse_str(&format!(
        "<w:document xmlns:w=\"{W}\" xmlns:m=\"{M}\"><w:body>{body}</w:body></w:document>"
    ))
    .unwrap();
    *txn.tree_mut(&styles_part).unwrap() =
        parse_str(&format!("<w:styles xmlns:w=\"{W}\">{styles}</w:styles>")).unwrap();
    if !settings.is_empty() {
        let part = txn.part_related_by(&main, rt::SETTINGS).unwrap().unwrap();
        *txn.tree_mut(&part).unwrap() = parse_str(settings).unwrap();
    }
    build(&txn, &main, &mut FontBook::new(own)).unwrap()
}

fn paras(blocks: &[Block]) -> Vec<&Para> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Para(p) => Some(p),
            Block::Table(_) => None,
        })
        .collect()
}

fn borders(para: &Para) -> ParaBorders {
    para.borders.as_deref().copied().unwrap_or_default()
}

fn border(style: BorderStyle, width: f64, space: f64, colour: Colour) -> Border {
    Border {
        style,
        width,
        space,
        colour,
    }
}

fn rgb(hex: &str) -> Rgb {
    colour::Rgb::hex(hex).unwrap()
}

const BOXED: &str = r#"<w:style w:type="paragraph" w:styleId="Boxed"><w:pPr><w:pBdr><w:top w:val="single" w:sz="8" w:space="1" w:color="FF0000"/><w:bottom w:val="double" w:sz="4" w:space="1" w:color="auto"/><w:between w:val="dotted" w:sz="4" w:space="1" w:color="4472C4" w:themeColor="accent1"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="E7E6E6" w:themeFill="background2"/></w:pPr></w:style>"#;

#[test]
fn paragraph_shading_and_borders_come_from_the_style_and_direct_formatting() {
    let body = concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="Boxed"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Boxed"/><w:pBdr><w:left w:val="thick" w:sz="96"/><w:bottom w:val="nil"/></w:pBdr><w:shd w:val="clear" w:fill="auto"/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr><w:pBdr><w:start w:val="threeDEmboss" w:sz="200" w:space="99"/><w:right w:val="apples" w:sz="20"/></w:pBdr></w:pPr></w:p>"#,
    );
    let model = model(body, BOXED);
    let [styled, direct, odd] = paras(&model.blocks)[..] else {
        panic!()
    };
    assert_eq!(
        styled.shade,
        Some(rgb("EEECE1")),
        "the theme's, not the cached"
    );
    assert_eq!(
        borders(styled).top,
        Some(border(
            BorderStyle::Single,
            1.0,
            1.0,
            Colour::Rgb(rgb("FF0000"))
        ))
    );
    assert_eq!(
        borders(styled).bottom,
        Some(border(BorderStyle::Double, 0.5, 1.0, Colour::Auto))
    );
    assert_eq!(
        borders(styled).between.map(|b| b.colour),
        Some(Colour::Rgb(rgb("4F81BD")))
    );
    assert_eq!(borders(styled).left, None);

    assert_eq!(direct.shade, None, "an automatic fill is none");
    assert_eq!(
        borders(direct).top,
        borders(styled).top,
        "sides merge one by one"
    );
    assert_eq!(borders(direct).bottom, None);
    assert_eq!(
        borders(direct).left,
        Some(border(BorderStyle::Thick, 12.0, 0.0, Colour::Auto))
    );

    assert_eq!(
        borders(odd).left,
        Some(border(BorderStyle::ThreeDEmboss, 12.0, 31.0, Colour::Auto)),
        "clamped to 12 points wide and 31 away"
    );
    assert_eq!(
        borders(odd).right.map(|b| (b.style, b.width)),
        Some((BorderStyle::Art, 20.0))
    );
}

fn tables(model: &Model) -> Vec<&Table> {
    model
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Table(t) => Some(t),
            Block::Para(_) => None,
        })
        .collect()
}

/// A table of `rows` rows of three cells, each cell's text its row and
/// column, `extra` in its `w:tblPr` and `row_extra(row)` before its cells.
fn table_xml(style: &str, extra: &str, rows: usize, row_extra: impl Fn(usize) -> String) -> String {
    let cells = |row: usize| {
        (0..3)
            .map(|column| format!("<w:tc><w:p><w:r><w:t>r{row}c{column}</w:t></w:r></w:p></w:tc>"))
            .collect::<String>()
    };
    let rows: String = (0..rows)
        .map(|row| format!("<w:tr>{}{}</w:tr>", row_extra(row), cells(row)))
        .collect();
    format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="{style}"/>{extra}</w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>{rows}</w:tbl><w:p/>"#
    )
}

fn fills(table: &Table) -> Vec<Vec<Option<String>>> {
    table
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|c| c.fill.map(|f| f.to_string()))
                .collect()
        })
        .collect()
}

fn cell_look<'m>(model: &'m Model, table: &Table, row: usize, column: usize) -> &'m Appearance {
    let Block::Para(para) = &table.rows[row].cells[column].blocks[0] else {
        panic!()
    };
    let Piece::Char { look, .. } = para.pieces[0] else {
        panic!()
    };
    &model.looks[look as usize]
}

const BANDED: &str = r#"<w:style w:type="table" w:styleId="Banded"><w:rPr><w:sz w:val="18"/></w:rPr><w:tblPr><w:tblStyleRowBandSize w:val="1"/><w:tblBorders><w:top w:val="single" w:sz="4" w:color="BFBFBF"/><w:bottom w:val="single" w:sz="4" w:color="BFBFBF"/><w:insideH w:val="single" w:sz="4" w:color="BFBFBF"/></w:tblBorders></w:tblPr><w:tcPr><w:tcBorders><w:left w:val="single" w:sz="48" w:color="FF0000"/></w:tcBorders><w:vAlign w:val="center"/></w:tcPr><w:tblStylePr w:type="firstRow"><w:rPr><w:b/><w:color w:val="FFFFFF"/></w:rPr><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="2F5496"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="DEEAF6"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="firstCol"><w:rPr><w:i/></w:rPr><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="EEEEEE"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="lastRow"><w:rPr><w:b/></w:rPr><w:tcPr><w:tcBorders><w:top w:val="double" w:sz="4" w:color="2F5496"/></w:tcBorders></w:tcPr></w:tblStylePr></w:style>"#;

#[test]
fn conditional_formats_reach_cells_by_position_in_order() {
    let look = r#"<w:tblLook w:firstRow="1" w:lastRow="1" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#;
    let model = model(&table_xml("Banded", look, 5, |_| String::new()), BANDED);
    let table = tables(&model)[0];
    let header = Some("2F5496".to_string());
    let band = Some("DEEAF6".to_string());
    let column = Some("EEEEEE".to_string());
    assert_eq!(
        fills(table),
        [
            // The first row comes after the first column.
            vec![header.clone(), header.clone(), header],
            vec![column.clone(), band.clone(), band.clone()],
            vec![column.clone(), None, None],
            vec![column.clone(), band.clone(), band],
            // The last row is in no band and sets no fill.
            vec![column, None, None],
        ]
    );
    let first = cell_look(&model, table, 0, 1);
    assert!(first.bold && !first.italic);
    assert_eq!(first.colour.to_string(), "FFFFFF");
    assert!(
        cell_look(&model, table, 0, 0).italic,
        "first column and row"
    );
    assert!(cell_look(&model, table, 4, 1).bold);
    assert!(!cell_look(&model, table, 2, 1).bold);
    assert_eq!(cell_look(&model, table, 2, 1).size, 9.0);
    // The last row's top border beats the inside one: double 4 weighs 12.
    let above_last = table.edges.horizontal(4, 1).unwrap();
    assert_eq!(above_last.style, BorderStyle::Double);
    assert_eq!(
        table.edges.horizontal(3, 1).unwrap().style,
        BorderStyle::Single
    );
    // Word reads no whole-table cell borders from a style (MS-OI29500
    // §2.1.255), but keeps its alignment.
    assert_eq!(table.edges.vertical(1, 1), None);
    assert_eq!(table.rows[1].cells[1].valign, VAlign::Center);
}

#[test]
fn without_a_look_the_table_shows_the_first_row_column_and_row_bands() {
    let model = model(&table_xml("Banded", "", 3, |_| String::new()), BANDED);
    let fills = fills(tables(&model)[0]);
    assert_eq!(fills[0][2].as_deref(), Some("2F5496"));
    assert_eq!(fills[1][0].as_deref(), Some("EEEEEE"));
    assert_eq!(fills[1][2].as_deref(), Some("DEEAF6"));
}

#[test]
fn cnf_style_does_not_move_a_cell_into_a_format() {
    // cnfStyle records what applied; the position decides (ECMA-376
    // §17.4.7).
    let claims_first_row = |row: usize| {
        if row == 2 {
            r#"<w:trPr><w:cnfStyle w:val="100000000000" w:firstRow="1"/></w:trPr>"#.to_string()
        } else {
            String::new()
        }
    };
    let look = r#"<w:tblLook w:val="0420"/>"#;
    let with = model(&table_xml("Banded", look, 4, claims_first_row), BANDED);
    let without = model(&table_xml("Banded", look, 4, |_| String::new()), BANDED);
    assert_eq!(fills(tables(&with)[0]), fills(tables(&without)[0]));
    assert_eq!(fills(tables(&with)[0])[2][1], None);
}

#[test]
fn a_row_exception_gives_its_row_table_borders_but_no_fill() {
    let exception = |row: usize| {
        if row == 1 {
            r#"<w:tblPrEx><w:tblBorders><w:top w:val="single" w:sz="24" w:color="00FF00"/><w:insideH w:val="single" w:sz="24" w:color="00FF00"/><w:insideV w:val="dashed" w:sz="8" w:color="0000FF"/></w:tblBorders><w:shd w:val="clear" w:fill="FFFF00"/></w:tblPrEx>"#.to_string()
        } else {
            String::new()
        }
    };
    let look = r#"<w:tblLook w:val="0600"/>"#;
    let model = model(&table_xml("Banded", look, 3, exception), BANDED);
    let table = tables(&model)[0];
    let above = table.edges.horizontal(1, 0).unwrap();
    assert_eq!(
        (above.style, above.width),
        (BorderStyle::Single, 3.0),
        "its insideH beats the table's"
    );
    let below = table.edges.horizontal(2, 0).unwrap();
    assert_eq!(below.width, 3.0, "the row's own bottom edge is its insideH");
    assert_eq!(
        table.edges.vertical(1, 1).unwrap().style,
        BorderStyle::Dashed
    );
    assert_eq!(table.edges.vertical(0, 1), None);
    // Word for Mac draws no fill from the exception's w:shd.
    assert_eq!(fills(table)[1], vec![None; 3]);
}

#[test]
fn a_style_format_beats_a_row_exception_and_a_cell_nil_stops_both() {
    let exception = |row: usize| {
        match row {
        0 => r#"<w:tblPrEx><w:shd w:val="clear" w:fill="FFFF00"/></w:tblPrEx>"#.to_string(),
        3 => r#"<w:tblPrEx><w:tblBorders><w:insideH w:val="thick" w:sz="4"/></w:tblBorders></w:tblPrEx>"#.to_string(),
        _ => String::new(),
    }
    };
    let look = r#"<w:tblLook w:val="0660"/>"#;
    let xml = table_xml("Banded", look, 4, exception).replacen(
        "<w:tc><w:p><w:r><w:t>r3c2",
        r#"<w:tc><w:tcPr><w:tcBorders><w:top w:val="nil"/></w:tcBorders></w:tcPr><w:p><w:r><w:t>r3c2"#,
        1,
    );
    let model = model(&xml, BANDED);
    let table = tables(&model)[0];
    assert_eq!(
        fills(table)[0][0].as_deref(),
        Some("2F5496"),
        "the first row's fill"
    );
    // Above the last row: its top (double, from the style) beats the
    // exception's insideH. The third cell's own nil stops its lookup before
    // the double, and yields to the row above's insideH.
    assert_eq!(
        table.edges.horizontal(3, 0).unwrap().style,
        BorderStyle::Double
    );
    let above_nil = table.edges.horizontal(3, 2).unwrap();
    assert_eq!(
        (above_nil.style, above_nil.colour.to_string()),
        (BorderStyle::Single, "BFBFBF".to_string())
    );
}

#[test]
fn a_vertical_merge_keeps_its_first_cell_s_fill_and_alignment() {
    let body = r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/><w:shd w:val="clear" w:fill="FFC000"/><w:vAlign w:val="bottom"/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:gridSpan w:val="1"/></w:tcPr><w:p/></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let model = model(body, "");
    let table = tables(&model)[0];
    let first: Vec<_> = table
        .rows
        .iter()
        .map(|r| (r.cells[0].fill, r.cells[0].valign))
        .collect();
    let orange = Some(rgb("FFC000"));
    assert_eq!(
        first,
        [
            (orange, VAlign::Bottom),
            (orange, VAlign::Bottom),
            (orange, VAlign::Bottom),
            (None, VAlign::Top)
        ]
    );
}

#[test]
fn small_capitals_and_their_spaces_are_four_fifths_rounded_to_a_half_point() {
    use super::look::Caps;
    assert_eq!(Caps::Small.apply('a', 11.0), ('A', 9.0));
    assert_eq!(Caps::Small.apply(' ', 11.0), (' ', 9.0));
    assert_eq!(Caps::Small.apply('a', 12.0), ('A', 9.5));
    assert_eq!(Caps::Small.apply('A', 11.0), ('A', 11.0));
    assert_eq!(Caps::All.apply('a', 11.0), ('A', 11.0));
}

fn text(para: &Para) -> String {
    para.pieces
        .iter()
        .map(|piece| match piece {
            Piece::Char { ch, .. } => *ch,
            Piece::LineBreak { .. } => '\n',
            _ => '?',
        })
        .collect()
}

fn widths(para: &Para) -> Vec<f64> {
    para.pieces
        .iter()
        .filter_map(|piece| match piece {
            Piece::Char { width, .. } => Some(*width),
            _ => None,
        })
        .collect()
}

const SQUARE: &str = "<m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath>";

#[test]
fn an_inline_equation_is_its_linear_text_where_it_stands() {
    let body = format!(
        "<w:p><w:r><w:t xml:space=\"preserve\">a </w:t></w:r>{SQUARE}<w:r><w:t xml:space=\"preserve\"> b</w:t></w:r></w:p>"
    );
    let model = model(&body, "");
    let para = paras(&model.blocks)[0];
    assert_eq!(
        text(para),
        "a x2  b",
        "Cambria Math's space after the script"
    );
    assert_eq!(para.align, Align::Left);
    let [equation] = &para.equations[..] else {
        panic!()
    };
    assert_eq!(equation.pieces, 2..5);
    assert_eq!(equation.text, "x^2", "the text a reader extracts");
}

#[test]
fn a_script_is_set_small_and_raised_and_its_line_extent_moves_with_it() {
    let model = model(&format!("<w:p><w:r><w:t>x</w:t></w:r>{SQUARE}</w:p>"), "");
    let para = paras(&model.blocks)[0];
    let piece = |at: usize| match para.pieces[at] {
        Piece::Char {
            width,
            metric,
            look,
            ..
        } => (
            width,
            model.metrics[metric as usize],
            model.looks[look as usize],
        ),
        _ => panic!(),
    };
    let (_, text, _) = piece(0);
    let (_, base, base_look) = piece(1);
    let (two_width, two, two_look) = piece(2);
    assert_eq!((base.height, base.ascent), (text.height, text.ascent));
    assert!(base_look.italic, "a letter without m:sty is slanted");
    let (scale, shift) = (math::CAMBRIA_MATH.script, math::CAMBRIA_MATH.up);
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(close(two_look.size, 10.0 * scale) && close(two_look.rise, 10.0 * shift));
    assert!(two_width < widths(para)[0]);
    assert!(close(two.ascent, text.ascent * scale + 10.0 * shift));
    assert!(
        two.ascent > text.ascent,
        "the raised 2 makes the line taller"
    );
}

#[test]
fn a_display_equation_is_as_tall_as_its_stack() {
    let fraction =
        "<m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f>";
    let body = format!(
        "<w:p><w:r><w:t>x</w:t></w:r></w:p><w:p><m:oMathPara><m:oMath>{fraction}</m:oMath></m:oMathPara></w:p><w:p><w:bookmarkStart w:id=\"0\" w:name=\"f\"/><m:oMath>{fraction}</m:oMath></w:p><w:p><w:r><w:t>y</w:t></w:r><m:oMath>{fraction}</m:oMath></w:p>"
    );
    let model = model(&body, "");
    let [text, display, lone, inline] = paras(&model.blocks)[..] else {
        panic!()
    };
    let metric = |para: &Para| match para.pieces.last() {
        Some(Piece::Char { metric, .. }) => model.metrics[*metric as usize],
        _ => panic!(),
    };
    let line = model.metrics[text.mark as usize];
    let (tall, single) = (metric(display), metric(inline));
    assert_eq!((single.height, single.ascent), (line.height, line.ascent));
    assert_eq!(tall.height, 2.0 * line.height, "numerator over denominator");
    assert_eq!(tall.ascent, line.ascent + line.height / 2.0);
    assert_eq!(
        (display.align, lone.align, inline.align),
        (Align::Center, Align::Center, Align::Left)
    );
    assert_eq!(
        metric(lone).height,
        tall.height,
        "alone in its paragraph, it is a display"
    );
}

#[test]
fn a_display_equation_is_a_line_of_its_own_aligned_by_its_jc() {
    let display = |jc: &str| format!("<m:oMathPara>{jc}{SQUARE}{SQUARE}</m:oMathPara>");
    let body = [
        format!("<w:p>{}</w:p>", display("")),
        format!(
            "<w:p>{}</w:p>",
            display(r#"<m:oMathParaPr><m:jc m:val="left"/></m:oMathParaPr>"#)
        ),
        format!(
            "<w:p><w:pPr><w:jc w:val=\"right\"/></w:pPr><w:r><w:t>Then</w:t></w:r>{}<w:r><w:t>so.</w:t></w:r></w:p>",
            display("")
        ),
    ]
    .concat();
    let model = model(&body, "");
    let [centred, left, mixed] = paras(&model.blocks)[..] else {
        panic!()
    };
    assert_eq!(text(centred), "x2 \nx2 ");
    assert_eq!(centred.align, Align::Center, "Word's centerGroup");
    assert_eq!(left.align, Align::Left);
    assert_eq!(text(mixed), "Then\nx2 \nx2 \nso.");
    assert_eq!(mixed.align, Align::Right, "the paragraph's, for its text");
}

#[test]
fn equations_use_cambria_math_only_when_it_is_supplied() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/fonts/inter/Inter[opsz,wght].ttf"
    );
    let data = std::fs::read(path).unwrap();
    let cambria_math = Face::parse(&data, "Cambria Math", false, false).unwrap();
    let named = r#"<m:r><m:rPr><m:sty m:val="p"/></m:rPr><w:rPr><w:rFonts w:ascii="Cambria Math" w:hAnsi="Cambria Math"/></w:rPr><m:t>x</m:t></m:r>"#;
    let unnamed = "<m:r><m:t>^2</m:t></m:r>";
    let other_hansi = r#"<m:r><m:rPr><m:sty m:val="p"/></m:rPr><w:rPr><w:rFonts w:ascii="Cambria Math" w:hAnsi="Unsupplied Sans"/></w:rPr><m:t>ε</m:t></m:r>"#;
    let body = format!(
        "<w:p><w:r><w:t>x^2ε</w:t></w:r><m:oMath>{named}{unnamed}{other_hansi}</m:oMath></w:p>"
    );
    let widths_of = |model: &Model| {
        let widths = widths(paras(&model.blocks)[0]);
        let (text, math) = widths.split_at(4);
        (text.to_vec(), math.to_vec())
    };
    let unsupplied = model(&body, "");
    let (text, math) = widths_of(&unsupplied);
    assert_eq!(math, text, "the paragraph's font, in both slots");
    let (text, math) = widths_of(&model_with(&body, "", "", vec![cambria_math.clone()]));
    let size = 10.0;
    let expected: Vec<f64> = "x^2ε"
        .chars()
        .map(|c| cambria_math.advance(c) * size)
        .collect();
    assert_eq!(
        math, expected,
        "an unsupplied hAnsi font takes Cambria Math too"
    );
    assert_ne!(math, text);
}

#[test]
fn scripts_are_set_as_cambria_math_sets_them_whatever_font_draws_them() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/fonts/inter/Inter[opsz,wght].ttf"
    );
    let without_math_table =
        Face::parse(&std::fs::read(path).unwrap(), "Cambria Math", false, false).unwrap();
    assert_eq!(without_math_table.math_scripts(), None);
    let body = format!("<w:p>{SQUARE}</w:p>");
    let size_of_two = |model: &Model| {
        let para = paras(&model.blocks)[0];
        para.pieces
            .iter()
            .find_map(|piece| match piece {
                Piece::Char { ch: '2', look, .. } => Some(model.looks[*look as usize].size),
                _ => None,
            })
            .unwrap()
    };
    assert!((size_of_two(&model(&body, "")) - 7.3).abs() < 1e-9);
    let supplied = model_with(&body, "", "", vec![without_math_table]);
    assert!(
        (size_of_two(&supplied) - 7.3).abs() < 1e-9,
        "a supplied Cambria Math without a MATH table takes Word's copy's"
    );
}

#[test]
fn cells_sit_on_the_grid_and_the_table_on_its_indent() {
    let body = r#"<w:tbl><w:tblPr><w:tblInd w:w="720" w:type="dxa"/><w:tblCellMar><w:left w:w="200" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="2000"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:trPr><w:gridBefore w:val="1"/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let centred = body.replace(
        r#"<w:tblInd w:w="720" w:type="dxa"/>"#,
        r#"<w:jc w:val="center"/>"#,
    );
    let compat = |mode: &str| {
        format!(
            r#"<w:settings xmlns:w="{W}"><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="{mode}"/></w:compat></w:settings>"#
        )
    };
    let table = |body: &str, settings: &str| {
        let model = model_with(body, "", settings, Vec::new());
        tables(&model)[0].clone()
    };
    let word_2013 = table(body, &compat("15"));
    assert_eq!(word_2013.grid, [0.0, 50.0, 150.0, 300.0]);
    let cell = &word_2013.rows[0].cells[0];
    assert_eq!((cell.column, cell.width), (1, 250.0));
    assert_eq!(word_2013.left(468.0), 36.0);
    // Below mode 15 the first cell's left margin hangs outside the indent.
    assert_eq!(table(body, &compat("14")).left(468.0), 26.0);
    assert_eq!(
        table(
            body,
            "<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"/>"
        )
        .left(468.0),
        26.0
    );
    assert_eq!(
        table(&centred, &compat("14")).left(468.0),
        84.0,
        "no outdent when centred"
    );
}

#[test]
fn an_equation_after_a_page_field_keeps_its_pieces_when_the_field_is_shown() {
    use crate::layout::fields::{Context, Number, NumberFormat, Values, show};
    let field = r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#;
    let mut model = model(&format!("<w:p>{field}{SQUARE}</w:p>"), "");
    let values = Values {
        pages: None,
        section_pages: Vec::new(),
        bookmarks: Default::default(),
        paragraphs: Default::default(),
    };
    let page = Number {
        shown: 1000,
        format: NumberFormat::Arabic,
        section: 0,
    };
    show(
        &mut model.blocks,
        &Context {
            values: &values,
            page: Some(page),
        },
    );
    let para = paras(&model.blocks)[0];
    let equation = &para.equations[0];
    let drawn: String = text(para).chars().skip(equation.pieces.start).collect();
    assert_eq!(text(para), "1000x2 ");
    assert_eq!(equation.pieces.end, para.pieces.len());
    assert_eq!(drawn, "x2 ");
}

#[test]
fn without_the_override_setting_normal_s_size_and_left_alignment_give_way_to_the_table_style() {
    let styles = format!(
        r#"{BANDED}<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:pPr><w:jc w:val="left"/></w:pPr><w:rPr><w:sz w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Big"><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="28"/></w:rPr></w:style>"#
    )
    .replace(
        "<w:rPr><w:sz w:val=\"18\"/></w:rPr><w:tblPr>",
        "<w:pPr><w:jc w:val=\"center\"/></w:pPr><w:rPr><w:sz w:val=\"18\"/></w:rPr><w:tblPr>",
    );
    let body = table_xml("Banded", r#"<w:tblLook w:val="0000"/>"#, 1, |_| {
        String::new()
    })
    .replacen(
        "<w:p><w:r><w:t>r0c1",
        r#"<w:p><w:pPr><w:pStyle w:val="Big"/></w:pPr><w:r><w:t>r0c1"#,
        1,
    );
    let settings = |value: &str| {
        format!(
            r#"<w:settings xmlns:w="{W}"><w:compat><w:compatSetting w:name="overrideTableStyleFontSizeAndJustification" w:uri="http://schemas.microsoft.com/office/word" w:val="{value}"/></w:compat></w:settings>"#
        )
    };
    let para = |model: &Model, column: usize| {
        let Block::Para(para) = &tables(model)[0].rows[0].cells[column].blocks[0] else {
            panic!()
        };
        para.align
    };
    let word_2007 = model_with(&body, &styles, &settings("0"), Vec::new());
    let table = tables(&word_2007)[0];
    assert_eq!(
        cell_look(&word_2007, table, 0, 0).size,
        9.0,
        "Normal's 12 points give way"
    );
    assert_eq!(para(&word_2007, 0), Align::Center);
    assert_eq!(
        cell_look(&word_2007, table, 0, 1).size,
        14.0,
        "not a size from Normal"
    );
    let standard = model_with(&body, &styles, &settings("1"), Vec::new());
    let table = tables(&standard)[0];
    assert_eq!(cell_look(&standard, table, 0, 0).size, 12.0);
    assert_eq!(para(&standard, 0), Align::Left);
}

#[test]
fn a_band_size_past_32_bits_reads_as_the_largest() {
    let body = table_xml(
        "Banded",
        r#"<w:tblStyleRowBandSize w:val="18446744073709551615"/>"#,
        4,
        |_| String::new(),
    );
    let model = model(&body, BANDED);
    let table = tables(&model)[0];
    assert_eq!(
        fills(table)[1],
        fills(table)[3],
        "one band below the header"
    );
}

#[test]
fn cells_past_the_grid_s_last_column_follow_the_cell_before() {
    let cells: String = (0..70)
        .map(|_| r#"<w:tc><w:tcPr><w:tcW w:w="200" w:type="dxa"/></w:tcPr><w:p/></w:tc>"#)
        .collect();
    let body = format!("<w:tbl><w:tr>{cells}</w:tr></w:tbl><w:p/>");
    let model = model(&body, "");
    let table = tables(&model)[0];
    let lefts: Vec<f64> = table.cell_lefts(0).collect();
    let expected: Vec<f64> = (0..70).map(|i| f64::from(i) * 10.0).collect();
    assert_eq!(lefts, expected);
}

#[test]
fn outline_levels_follow_the_style_chain_and_built_in_heading_names() {
    let styles = concat!(
        r#"<w:style w:type="paragraph" w:styleId="H1"><w:name w:val="heading 1"/></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Chapter"><w:name w:val="Chapter"/><w:basedOn w:val="H1"/></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Toc"><w:name w:val="TOC Heading"/><w:basedOn w:val="H1"/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Deep"><w:name w:val="Heading 9"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr></w:style>"#,
        r#"<w:style w:type="paragraph" w:styleId="Odd"><w:name w:val="heading 10"/></w:style>"#,
    );
    let para = |style: &str, direct: &str| {
        format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/>{direct}</w:pPr></w:p>"#)
    };
    let body = [
        para("H1", ""),
        para("Chapter", ""),
        para("Toc", ""),
        para("Deep", ""),
        para("Odd", ""),
        para("Odd", r#"<w:outlineLvl w:val="4"/>"#),
        para("H1", r#"<w:outlineLvl w:val="9"/>"#),
    ]
    .concat();
    let model = model(&body, styles);
    let levels: Vec<Option<u8>> = paras(&model.blocks).iter().map(|p| p.outline).collect();
    assert_eq!(
        levels,
        [Some(0), Some(0), None, Some(2), None, Some(4), None]
    );
}

#[test]
fn bookmarks_keep_the_piece_they_stand_before() {
    let body = concat!(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="start"/><w:r><w:t>ab</w:t></w:r>"#,
        r#"<w:bookmarkStart w:id="1" w:name="middle"/><w:r><w:t>c</w:t></w:r>"#,
        r#"<w:bookmarkStart w:id="2" w:name="end"/></w:p>"#,
    );
    let model = model(body, "");
    let marks: Vec<(&str, u32)> = paras(&model.blocks)[0]
        .bookmarks
        .iter()
        .map(|b| (b.name.as_str(), b.piece))
        .collect();
    assert_eq!(marks, [("start", 0), ("middle", 2), ("end", 3)]);
}
