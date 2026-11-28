use super::look::{Border, BorderSource, BorderStyle};
use super::*;
use crate::Document;
use crate::layout::fonts::Face;
use crate::wml::M;
use crate::xml::parse_str;

/// The bundled document, with this body and these styles. Its theme is
/// Office 2007's: light 2 is EEECE1, accent 1 4F81BD.
fn model(body: &str, styles: &str) -> Model {
    model_with(body, styles, Vec::new())
}

/// [`model`], with the document's own fonts `own`.
fn model_with(body: &str, styles: &str, own: Vec<Face>) -> Model {
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

#[test]
fn cells_keep_their_fill_alignment_and_borders_with_where_they_came_from() {
    let styles = r#"<w:style w:type="table" w:styleId="Grid"><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="4" w:color="auto"/><w:left w:val="single" w:sz="4" w:color="auto"/></w:tcBorders><w:shd w:val="clear" w:fill="DDDDDD"/><w:vAlign w:val="bottom"/></w:tcPr></w:style>"#;
    let cell = |tc_pr: &str| format!("<w:tc><w:tcPr>{tc_pr}</w:tcPr><w:p/></w:tc>");
    let body = format!(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"Grid\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr>{}{}</w:tr></w:tbl><w:p/>",
        cell(""),
        cell(
            r#"<w:tcBorders><w:top w:val="nil"/><w:end w:val="dashed" w:sz="12" w:color="00FF00"/><w:insideH w:val="single" w:sz="2"/></w:tcBorders><w:shd w:val="solid" w:color="0000FF" w:fill="FFFFFF"/><w:vAlign w:val="center"/>"#
        ),
    );
    let model = model(&body, styles);
    let Block::Table(table) = &model.blocks[0] else {
        panic!()
    };
    let [style_only, own] = &table.rows[0].cells[..] else {
        panic!()
    };
    let single = border(BorderStyle::Single, 0.5, 0.0, Colour::Auto);
    assert_eq!(style_only.fill, Some(rgb("DDDDDD")));
    assert_eq!(style_only.valign, VAlign::Bottom);
    assert_eq!(
        style_only.borders.top,
        Some((Some(single), BorderSource::Style))
    );
    assert_eq!(style_only.borders.right, None);

    assert_eq!(
        own.fill,
        Some(rgb("0000FF")),
        "a solid pattern is its colour"
    );
    assert_eq!(own.valign, VAlign::Center);
    assert_eq!(
        own.borders.top,
        Some((None, BorderSource::Direct)),
        "nil clears the style's side, and is not unset"
    );
    assert_eq!(own.borders.left, Some((Some(single), BorderSource::Style)));
    assert_eq!(
        own.borders.right,
        Some((
            Some(border(
                BorderStyle::Dashed,
                1.5,
                0.0,
                Colour::Rgb(rgb("00FF00"))
            )),
            BorderSource::Direct
        ))
    );
    assert_eq!(
        own.borders.inside_h.map(|(b, s)| (b.map(|b| b.width), s)),
        Some((Some(0.25), BorderSource::Direct))
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
    let (text, math) = widths_of(&model_with(&body, "", vec![cambria_math.clone()]));
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
    let supplied = model_with(&body, "", vec![without_math_table]);
    assert!(
        (size_of_two(&supplied) - 7.3).abs() < 1e-9,
        "a supplied Cambria Math without a MATH table takes Word's copy's"
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
