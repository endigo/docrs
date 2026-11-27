use super::look::{Border, BorderSource, BorderStyle};
use super::*;
use crate::Document;
use crate::xml::parse_str;

/// The bundled document, with this body and these styles. Its theme is
/// Office 2007's: light 2 is EEECE1, accent 1 4F81BD.
fn model(body: &str, styles: &str) -> Model {
    let doc = Document::new();
    let mut txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    let styles_part = txn.part_related_by(&main, rt::STYLES).unwrap().unwrap();
    *txn.tree_mut(&main).unwrap() = parse_str(&format!(
        "<w:document xmlns:w=\"{W}\"><w:body>{body}</w:body></w:document>"
    ))
    .unwrap();
    *txn.tree_mut(&styles_part).unwrap() =
        parse_str(&format!("<w:styles xmlns:w=\"{W}\">{styles}</w:styles>")).unwrap();
    build(&txn, &main, &mut FontBook::new(Vec::new())).unwrap()
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
    assert_eq!(style_only.borders.top, Some((single, BorderSource::Style)));
    assert_eq!(style_only.borders.right, None);

    assert_eq!(
        own.fill,
        Some(rgb("0000FF")),
        "a solid pattern is its colour"
    );
    assert_eq!(own.valign, VAlign::Center);
    assert_eq!(own.borders.top, None, "nil overrides the style");
    assert_eq!(own.borders.left, Some((single, BorderSource::Style)));
    assert_eq!(
        own.borders.right,
        Some((
            border(BorderStyle::Dashed, 1.5, 0.0, Colour::Rgb(rgb("00FF00"))),
            BorderSource::Direct
        ))
    );
    assert_eq!(
        own.borders.inside_h.map(|(b, s)| (b.width, s)),
        Some((0.25, BorderSource::Direct))
    );
}
