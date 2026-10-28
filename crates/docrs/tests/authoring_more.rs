//! The rest of the inherited authoring API (P7): paragraph format with line
//! spacing rules, fonts with theme colors, tab stops, inline-shape sizes,
//! tables (merge, rows, columns, cells, nested tables), comment ranges,
//! pictures from a path, inner content and style objects. XML expectations
//! were checked against the pinned oracle through the `m7-inherited-*` ops
//! cases; every refusal is checked for atomicity with `patch_bytes`.

mod support;

use docrs::authoring::{
    BlockItem, FontChange, LineSpacing, LineSpacingRule, ParagraphFormat, ParagraphItem, RunItem,
    TableDirection,
};
use docrs::{Document, ErrorKind, Limits, RefusalKind};
use support::*;

const INCH: i64 = 914_400;
const PT: i64 = 12_700;

fn open_body(body: &str) -> Document {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><w:body>{body}</w:body></w:document>"
    );
    Document::open_bytes(&minimal_package_with(&xml), &Limits::default()).unwrap()
}

#[test]
fn line_spacing_rules_follow_upstream() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("x", None).unwrap();
    let set = |doc: &mut Document, rule| {
        p.set_format(
            doc,
            &ParagraphFormat {
                line_spacing_rule: Some(rule),
                ..ParagraphFormat::default()
            },
        )
        .unwrap();
    };
    set(&mut doc, Some(LineSpacingRule::OnePointFive));
    assert!(document_xml(&doc).contains("<w:spacing w:line=\"360\" w:lineRule=\"auto\"/>"));
    let format = p.format(&doc).unwrap();
    assert_eq!(format.line_spacing, Some(LineSpacing::Multiple(1.5)));
    assert_eq!(
        format.line_spacing_rule,
        Some(LineSpacingRule::OnePointFive)
    );
    set(&mut doc, Some(LineSpacingRule::AtLeast));
    assert_eq!(
        p.format(&doc).unwrap().line_spacing,
        Some(LineSpacing::Exactly(360 * 635))
    );
    // An exact length keeps an existing atLeast rule.
    p.set_format(
        &mut doc,
        &ParagraphFormat {
            line_spacing: Some(Some(LineSpacing::Exactly(14 * PT))),
            ..ParagraphFormat::default()
        },
    )
    .unwrap();
    assert!(document_xml(&doc).contains("<w:spacing w:line=\"280\" w:lineRule=\"atLeast\"/>"));
    set(&mut doc, None);
    assert!(document_xml(&doc).contains("<w:spacing w:line=\"280\"/>"));
    // Only w:line present reads as MULTIPLE.
    assert_eq!(
        p.format(&doc).unwrap().line_spacing_rule,
        Some(LineSpacingRule::Multiple)
    );
}

#[test]
fn hanging_indents_and_flags_read_back() {
    let mut doc = open_body(
        "<w:p><w:pPr><w:keepNext w:val=\"off\"/><w:spacing w:before=\"12pt\"/><w:ind w:left=\"720.4\" w:hanging=\"360\"/></w:pPr></w:p>",
    );
    let p = doc.paragraphs().unwrap().remove(0);
    let format = p.format(&doc).unwrap();
    assert_eq!(format.keep_with_next, Some(false));
    assert_eq!(format.space_before, Some(12 * PT));
    assert_eq!(format.left_indent, Some(720 * 635));
    assert_eq!(format.first_line_indent, Some(-360 * 635));
    let before = snapshot(&doc);
    let err = p
        .set_format(
            &mut doc,
            &ParagraphFormat {
                alignment: Some(Some("center".into())),
                space_after: Some(Some(-1)),
                ..ParagraphFormat::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn fonts_read_theme_colors_and_validate_first() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("styled", None).unwrap();
    let run = p.runs(&doc).unwrap().remove(0);
    run.set_font(
        &mut doc,
        &[
            FontChange::ThemeColor(Some("accent2".into())),
            FontChange::Highlight(Some("darkCyan".into())),
            FontChange::Flag("small_caps".into(), Some(false)),
        ],
    )
    .unwrap();
    assert!(document_xml(&doc).contains(
        "<w:rPr><w:smallCaps w:val=\"0\"/><w:color w:val=\"000000\" w:themeColor=\"accent2\"/><w:highlight w:val=\"darkCyan\"/></w:rPr>"
    ));
    let font = run.font(&doc).unwrap();
    assert_eq!(font.color.color_type.as_deref(), Some("THEME"));
    assert_eq!(font.color.rgb.as_deref(), Some("000000"));
    assert_eq!(font.color.theme_color.as_deref(), Some("accent2"));
    assert_eq!(font.highlight.as_deref(), Some("darkCyan"));
    run.set_font(&mut doc, &[FontChange::Color(Some((0x12, 0x34, 0x56)))])
        .unwrap();
    assert!(document_xml(&doc).contains("<w:color w:val=\"123456\"/>"));
    let before = snapshot(&doc);
    let err = run
        .set_font(
            &mut doc,
            &[
                FontChange::Flag("bold".into(), Some(true)),
                FontChange::ThemeColor(Some("accent9".into())),
            ],
        )
        .unwrap_err();
    assert_eq!(err.reason(), "invalid_theme_color");
    assert_eq!(snapshot(&doc), before);
    run.set_font(&mut doc, &[FontChange::ThemeColor(None)])
        .unwrap();
    assert_eq!(run.font(&doc).unwrap().color.color_type, None);
}

#[test]
fn tab_stops_stay_in_position_order() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("tabs", None).unwrap();
    p.add_tab_stop(&mut doc, 2 * INCH, "left", "none").unwrap();
    p.add_tab_stop(&mut doc, INCH, "center", "dot").unwrap();
    let last = p
        .add_tab_stop(&mut doc, 3 * INCH, "right", "hyphen")
        .unwrap();
    assert!(document_xml(&doc).contains(
        "<w:tabs><w:tab w:pos=\"1440\" w:val=\"center\" w:leader=\"dot\"/><w:tab w:pos=\"2880\" w:val=\"left\"/><w:tab w:pos=\"4320\" w:val=\"right\" w:leader=\"hyphen\"/></w:tabs>"
    ));
    // Moving a tab stop replaces its element: the old handle goes stale.
    let moved = last.set_position(&mut doc, -INCH).unwrap();
    assert_eq!(moved.values(&doc).unwrap().position, -INCH);
    let err = last.values(&doc).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    let positions: Vec<i64> = p
        .tab_stops(&doc)
        .unwrap()
        .iter()
        .map(|t| t.values(&doc).unwrap().position)
        .collect();
    assert_eq!(positions, vec![-INCH, INCH, 2 * INCH]);
    moved.set_leader(&mut doc, None).unwrap();
    assert_eq!(moved.values(&doc).unwrap().leader, "none");
    let before = snapshot(&doc);
    let err = p.delete_tab_stop(&mut doc, 7).unwrap_err();
    assert_eq!(err.reason(), "index_out_of_range");
    assert_eq!(snapshot(&doc), before);
    for _ in 0..3 {
        p.delete_tab_stop(&mut doc, -1).unwrap();
    }
    assert!(!document_xml(&doc).contains("<w:tabs"));
    p.clear_tab_stops(&mut doc).unwrap();
}

#[test]
fn inline_shapes_resize_extent_and_transform() {
    let mut doc = Document::new();
    doc.add_picture(&png_150dpi(), None, None).unwrap();
    let shapes = doc.inline_shapes().unwrap();
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].shape_type(&doc).unwrap().name(), "PICTURE");
    shapes[0].set_width(&mut doc, INCH).unwrap();
    shapes[0].set_height(&mut doc, INCH / 2).unwrap();
    assert_eq!(shapes[0].width(&doc).unwrap(), INCH);
    let xml = document_xml(&doc);
    assert!(xml.contains("<wp:extent cx=\"914400\" cy=\"457200\"/>"));
    assert!(xml.contains("<a:ext cx=\"914400\" cy=\"457200\"/>"));
    let before = snapshot(&doc);
    assert!(shapes[0].set_width(&mut doc, -1).is_err());
    assert_eq!(snapshot(&doc), before);
    let mut chart = open_body(
        "<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"100\" cy=\"100\"/><wp:docPr id=\"1\" name=\"c\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"/></a:graphic></wp:inline></w:drawing></w:r></w:p>",
    );
    let shape = chart.inline_shapes().unwrap().remove(0);
    assert_eq!(shape.shape_type(&chart).unwrap().name(), "CHART");
    let before = snapshot(&chart);
    let err = shape.set_width(&mut chart, 5).unwrap_err();
    assert_eq!(err.reason(), "no_picture_element");
    assert_eq!(snapshot(&chart), before);
}

#[test]
fn merging_cells_spans_and_moves_content() {
    let mut doc = Document::new();
    let table = doc.add_table(3, 3, None).unwrap();
    for (row, col, text) in [(0, 0, "a"), (0, 1, "b"), (1, 0, "c"), (1, 1, "d")] {
        table
            .cell(&doc, row, col)
            .unwrap()
            .set_text(&mut doc, text)
            .unwrap();
    }
    let a = table.cell(&doc, 0, 0).unwrap();
    let d = table.cell(&doc, 1, 1).unwrap();
    let merged = a.merge(&mut doc, &d).unwrap();
    assert_eq!(merged.grid_span(&doc).unwrap(), 2);
    assert_eq!(merged.text(&doc).unwrap(), "a\nb\nc\nd");
    let xml = document_xml(&doc);
    assert!(
        xml.contains("<w:gridSpan w:val=\"2\"/><w:vMerge w:val=\"restart\"/>"),
        "{xml}"
    );
    assert!(xml.contains("<w:gridSpan w:val=\"2\"/><w:vMerge/>"));
    // Widths of swallowed cells are added: 6 in / 3 columns = 2880 twips per cell.
    assert_eq!(merged.width(&doc).unwrap(), Some(2 * 1_828_800));
    let texts: Vec<String> = table
        .row_cells(&doc, 1)
        .unwrap()
        .iter()
        .map(|c| c.text(&doc).unwrap())
        .collect();
    assert_eq!(texts, vec!["a\nb\nc\nd", "a\nb\nc\nd", ""]);
    assert_eq!(
        d.text(&doc).unwrap_err().refusal_kind(),
        Some(RefusalKind::TargetNotFound)
    );
    // Same top, different bottoms: upstream's inverted-L check refuses the span.
    let before = snapshot(&doc);
    let top_right = table.cell(&doc, 0, 2).unwrap();
    let err = merged.merge(&mut doc, &top_right).unwrap_err();
    assert_eq!(err.reason(), "invalid_span");
    assert_eq!(snapshot(&doc), before);
    let other = doc.add_table(1, 1, None).unwrap();
    let foreign = other.cell(&doc, 0, 0).unwrap();
    let before = snapshot(&doc);
    let err = table
        .cell(&doc, 2, 2)
        .unwrap()
        .merge(&mut doc, &foreign)
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn table_row_column_and_cell_properties() {
    let mut doc = Document::new();
    let table = doc.add_table(2, 2, None).unwrap();
    table.set_alignment(&mut doc, Some("center")).unwrap();
    table.set_autofit(&mut doc, false).unwrap();
    table
        .set_table_direction(&mut doc, Some(TableDirection::Rtl))
        .unwrap();
    assert_eq!(table.alignment(&doc).unwrap().as_deref(), Some("center"));
    assert!(!table.autofit(&doc).unwrap());
    assert_eq!(
        table.table_direction(&doc).unwrap(),
        Some(TableDirection::Rtl)
    );
    assert!(document_xml(&doc).contains(
        "<w:tblPr><w:bidiVisual/><w:tblW w:type=\"auto\" w:w=\"0\"/><w:jc w:val=\"center\"/><w:tblLayout w:type=\"fixed\"/>"
    ));
    let row = table.rows(&doc).unwrap().remove(1);
    row.set_height(&mut doc, Some(INCH)).unwrap();
    row.set_height_rule(&mut doc, Some("atLeast")).unwrap();
    assert_eq!(row.height(&doc).unwrap(), Some(INCH));
    assert!(
        document_xml(&doc)
            .contains("<w:trPr><w:trHeight w:val=\"1440\" w:hRule=\"atLeast\"/></w:trPr>")
    );
    let column = table.columns(&doc).unwrap().remove(0);
    column.set_width(&mut doc, Some(424_497)).unwrap();
    assert_eq!(column.width(&doc).unwrap(), Some(424_180));
    let cell = table.cell(&doc, 0, 1).unwrap();
    cell.set_vertical_alignment(&mut doc, Some("bottom"))
        .unwrap();
    assert_eq!(
        cell.vertical_alignment(&doc).unwrap().as_deref(),
        Some("bottom")
    );
    for err in [
        table.set_alignment(&mut doc, Some("justify")).unwrap_err(),
        row.set_height(&mut doc, Some(-1)).unwrap_err(),
        cell.set_vertical_alignment(&mut doc, Some("middle"))
            .unwrap_err(),
    ] {
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    }
}

#[test]
fn nested_tables_and_cell_paragraphs() {
    let mut doc = Document::new();
    let table = doc.add_table(1, 2, None).unwrap();
    let cell = table.cell(&doc, 0, 0).unwrap();
    let inner = cell.add_table(&mut doc, 2, 2).unwrap();
    assert_eq!(inner.column_count(&doc).unwrap(), 2);
    // `_Cell.add_table` sizes from the cell width and appends a paragraph.
    let items = cell.container(&doc).unwrap().inner_content(&doc).unwrap();
    assert!(matches!(
        items.as_slice(),
        [
            BlockItem::Paragraph(_),
            BlockItem::Table(_),
            BlockItem::Paragraph(_)
        ]
    ));
    let deep = inner.cell(&doc, 1, 1).unwrap();
    deep.add_paragraph(&mut doc, "deep", Some("Normal"))
        .unwrap();
    assert_eq!(deep.text(&doc).unwrap(), "\ndeep");
    assert_eq!(cell.tables(&doc).unwrap().len(), 1);
}

#[test]
fn comment_ranges_wrap_runs_in_order() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("one", None).unwrap();
    let second = p.add_run(&mut doc, Some(" two"), None).unwrap();
    let first = p.runs(&doc).unwrap().remove(0);
    let before = snapshot(&doc);
    let err = second.mark_comment_range(&mut doc, &first, 3).unwrap_err();
    assert_eq!(err.reason(), "comment_range_reversed");
    assert_eq!(snapshot(&doc), before);
    first.mark_comment_range(&mut doc, &second, 3).unwrap();
    assert!(document_xml(&doc).contains(
        "<w:commentRangeStart w:id=\"3\"/><w:r><w:t>one</w:t></w:r><w:r><w:t xml:space=\"preserve\"> two</w:t></w:r><w:commentRangeEnd w:id=\"3\"/><w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"3\"/></w:r>"
    ));
}

#[test]
fn pictures_from_a_path_use_the_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logo.picture.PNG");
    std::fs::write(&path, png_150dpi()).unwrap();
    let mut doc = Document::new();
    let shape = doc.add_picture_file(&path, Some(INCH), None).unwrap();
    assert_eq!(shape.image_part, "word/media/image1.PNG");
    assert!(document_xml(&doc).contains("name=\"logo.picture.PNG\""));
    let p = doc.add_paragraph("", None).unwrap();
    let run = p.add_run(&mut doc, None, None).unwrap();
    let again = run.add_picture_file(&mut doc, &path, None, None).unwrap();
    assert_eq!(again.image_part, "word/media/image1.PNG");
    assert!(document_xml(&doc).contains("name=\"image.png\""));
    let missing = doc.add_picture_file(dir.path().join("missing.png"), None, None);
    assert_eq!(missing.unwrap_err().kind(), ErrorKind::Io);
}

#[test]
fn inner_content_hyperlinks_and_page_break_fragments() {
    let mut doc = open_body(
        "<w:p><w:r><w:t>before </w:t><w:lastRenderedPageBreak/><w:t>after</w:t></w:r><w:hyperlink w:anchor=\"top\"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:r><w:tab/></w:r></w:p>",
    );
    let p = doc.paragraphs().unwrap().remove(0);
    assert!(p.contains_page_break(&doc).unwrap());
    let items = p.inner_content(&doc).unwrap();
    assert_eq!(items.len(), 3);
    let ParagraphItem::Run(run) = &items[0] else {
        panic!("expected a run")
    };
    let content = run.inner_content(&doc).unwrap();
    assert!(matches!(
        content.as_slice(),
        [RunItem::Text(a), RunItem::RenderedPageBreak(_), RunItem::Text(b)] if a == "before " && b == "after"
    ));
    let ParagraphItem::Hyperlink(link) = &items[1] else {
        panic!("expected a hyperlink")
    };
    assert_eq!(link.fragment(&doc).unwrap(), "top");
    assert_eq!(link.address(&doc).unwrap(), "");
    assert_eq!(link.url(&doc).unwrap(), "");
    assert_eq!(link.text(&doc).unwrap(), "link");
    let page_break = p.rendered_page_breaks(&doc).unwrap().remove(0);
    let preceding = page_break
        .preceding_paragraph_fragment(&doc)
        .unwrap()
        .unwrap();
    assert_eq!(preceding.text, "before ");
    let following = page_break
        .following_paragraph_fragment(&doc)
        .unwrap()
        .unwrap();
    assert_eq!(following.text, "afterlink\t");
    assert_eq!(following.runs, vec!["after".to_string(), "\t".to_string()]);
    assert!(!doc.package().is_modified());
    let _ = &mut doc;
}

#[test]
fn style_objects_resolve_with_defaults() {
    let mut doc = Document::new();
    let p = doc.add_paragraph("h", Some("Heading 1")).unwrap();
    assert_eq!(
        p.style(&doc).unwrap().and_then(|s| s.name).as_deref(),
        Some("Heading 1")
    );
    let plain = doc.add_paragraph("plain", None).unwrap();
    assert_eq!(
        plain.style(&doc).unwrap().and_then(|s| s.name).as_deref(),
        Some("Normal")
    );
    let run = plain.runs(&doc).unwrap().remove(0);
    assert_eq!(
        run.style(&doc).unwrap().and_then(|s| s.name).as_deref(),
        Some("Default Paragraph Font")
    );
    let table = doc.add_table(1, 1, Some("Light Shading")).unwrap();
    assert_eq!(
        table.style(&doc).unwrap().and_then(|s| s.name).as_deref(),
        Some("Light Shading")
    );
}

#[test]
fn value_types_follow_python_docx_shared() {
    use docrs::authoring::{Length, RgbColor};
    assert_eq!(Length::from_inches(2.54).emu(), 2_322_576);
    assert_eq!(Length::from_pt(10.5).emu(), 133_350);
    assert_eq!(Length::from_twips(0.5).emu(), 317);
    assert_eq!(Length::from_cm(-0.3).emu(), -108_000);
    assert_eq!(Length(133_350).twips(), 210);
    assert_eq!(Length(952).twips(), 1);
    assert_eq!(
        Length(133_350).cm().to_bits(),
        0.370_416_666_666_666_67_f64.to_bits()
    );
    assert_eq!(
        RgbColor::from_string("f00ba5").unwrap().to_string(),
        "F00BA5"
    );
    assert_eq!(
        RgbColor::from_string(" a b c").unwrap(),
        RgbColor(10, 11, 12)
    );
    for bad in ["XYZ123", "", "0x1234", "1234567"] {
        assert_eq!(
            RgbColor::from_string(bad).unwrap_err().kind(),
            ErrorKind::InvalidArgument,
            "{bad:?}"
        );
    }
}

#[test]
fn clearing_the_body_keeps_section_properties() {
    let mut doc = open_body(
        "<w:p/><!--note--><w:tbl><w:tblPr/><w:tblGrid/></w:tbl><w:sectPr><w:pgSz w:w=\"12240\"/></w:sectPr>",
    );
    doc.clear_body_content().unwrap();
    let xml = document_xml(&doc);
    assert!(
        xml.contains("<w:body><!--note--><w:sectPr><w:pgSz w:w=\"12240\"/></w:sectPr></w:body>"),
        "{xml}"
    );
    assert!(doc.inner_content().unwrap().is_empty());
}

#[test]
fn hyperlink_address_retargets_and_clears_the_fragment() {
    let mut doc = open_body(
        "<w:p><w:hyperlink w:anchor=\"top\"><w:r><w:t>jump</w:t></w:r></w:hyperlink></w:p>",
    );
    let p = doc.paragraphs().unwrap().remove(0);
    let link = p.hyperlinks(&doc).unwrap().remove(0);
    let before = snapshot(&doc);
    let empty = link.set_address(&mut doc, "");
    assert_eq!(empty.unwrap_err().kind(), ErrorKind::InvalidArgument);
    assert_eq!(snapshot(&doc), before);
    link.set_address(&mut doc, "http://new.example/").unwrap();
    let link = p.hyperlinks(&doc).unwrap().remove(0);
    assert_eq!(link.address(&doc).unwrap(), "http://new.example/");
    assert_eq!(link.fragment(&doc).unwrap(), "");
    assert_eq!(link.url(&doc).unwrap(), "http://new.example/");
    link.set_address(&mut doc, "http://other.example/").unwrap();
    let rels = doc
        .package()
        .relationships_at("/word/document.xml")
        .unwrap();
    let targets: Vec<&str> = rels.iter().map(|rel| rel.target.as_str()).collect();
    assert_eq!(targets, vec!["http://other.example/"]);
}

#[test]
fn images_from_a_path_keep_the_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logo.picture.PNG");
    std::fs::write(&path, png_150dpi()).unwrap();
    let image = docrs::image::Image::from_file(&path).unwrap();
    assert_eq!(image.filename(), "logo.picture.PNG");
    assert_eq!(image.ext(), "PNG");
    assert_eq!(image.content_type(), "image/png");
    let anonymous = docrs::image::Image::from_blob(&png_150dpi()).unwrap();
    assert_eq!(
        (anonymous.filename(), anonymous.ext()),
        ("image.png".to_string(), "png")
    );
    let missing = docrs::image::Image::from_file(dir.path().join("missing.png"));
    assert_eq!(missing.unwrap_err().kind(), ErrorKind::Io);
}
