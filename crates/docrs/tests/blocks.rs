//! Block-operation contracts (upstream `blocks.py`): section and rich block
//! insertion, tracked paragraph delete/replace, anchor authority, refusal
//! atomicity and the changed-part budget. Expected values are frozen from
//! the pinned oracle.

mod support;

use docrs::anchor::BlockTarget;
use docrs::blocks::{
    ListBlock, ParagraphRange, RichBlock, RichParagraph, SectionOptions, TableBlock, TextRun,
};
use docrs::clock::Timestamp;
use docrs::wml::W;
use docrs::xml::XmlTree;
use docrs::{Document, FindOptions, Limits, RefusalKind, View};
use support::*;

const W_DECL: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const MINIMAL: &str = "generated/minimal-clean/minimal.docx";

fn open(rel: &str) -> Document {
    Document::open_bytes(&fixture(rel), &Limits::default()).unwrap()
}

fn memory(texts: &[&str]) -> Document {
    let body: String = texts
        .iter()
        .map(|t| format!(r#"<w:p><w:r><w:t xml:space="preserve">{t}</w:t></w:r></w:p>"#))
        .collect();
    Document::open_bytes(
        &minimal_package_with(&format!(
            r#"<w:document {W_DECL}><w:body>{body}</w:body></w:document>"#
        )),
        &Limits::default(),
    )
    .unwrap()
}

fn frozen() -> Option<Timestamp> {
    Some(Timestamp::parse("2026-07-07T12:00:00Z").unwrap())
}

fn texts(doc: &Document) -> Vec<String> {
    doc.blocks(View::Current)
        .unwrap()
        .into_iter()
        .map(|b| b.text)
        .collect()
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

fn body_xml(doc: &Document) -> String {
    String::from_utf8(docrs::xml::serialize(
        doc.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn refusal_of<T: std::fmt::Debug>(result: docrs::Result<T>) -> RefusalKind {
    result.unwrap_err().refusal_kind().expect("a refusal")
}

#[test]
fn inserts_a_section_after_a_string_anchor_within_the_part_budget() {
    let mut doc = open(MINIMAL);
    let result = doc
        .insert_section_after(
            BlockTarget::Text("First body paragraph"),
            "New Section",
            &strings(&["Alpha body.", "Beta body."]),
            &SectionOptions::default(),
        )
        .unwrap();
    assert_eq!(
        result.to_json(),
        serde_json::json!({"schema": "paper_block_edit", "version": 1, "story": "word/document.xml",
            "inserted_blocks": 3, "deleted_blocks": 0, "deleted_text": [], "revision_ids": []})
    );
    let (out, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    assert!(report.added_parts.is_empty());
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    let blocks = reopened.blocks(View::Current).unwrap();
    let got: Vec<&str> = blocks[1..5].iter().map(|b| b.text.as_str()).collect();
    assert_eq!(
        got,
        vec![
            "First body paragraph with perfectly ordinary text.",
            "New Section",
            "Alpha body.",
            "Beta body."
        ]
    );
    assert_eq!(blocks[2].style_id.as_deref(), Some("Heading2"));
}

#[test]
fn tracked_sections_stamp_runs_and_marks_with_deterministic_identity() {
    let mut doc = open(MINIMAL);
    let result = doc
        .insert_section_after(
            BlockTarget::Text("First body paragraph"),
            "New Section",
            &strings(&["Alpha body.", "Beta body."]),
            &SectionOptions {
                tracked: true,
                author: Some("Carol QA"),
                date: frozen(),
                ..SectionOptions::default()
            },
        )
        .unwrap();
    assert_eq!(result.revision_ids, vec![1, 2, 3]);
    let xml = body_xml(&doc);
    assert!(xml.contains(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/><w:rPr><w:ins w:id="1" w:author="Carol QA" w:date="2026-07-07T12:00:00Z"/></w:rPr></w:pPr><w:ins w:id="1" w:author="Carol QA" w:date="2026-07-07T12:00:00Z"><w:r><w:t>New Section</w:t></w:r></w:ins></w:p>"#
    ));
    assert_eq!(
        doc.blocks(View::Original)
            .unwrap()
            .iter()
            .filter(|b| b.text == "New Section")
            .count(),
        0,
        "tracked insertions are hidden from the original view"
    );
}

#[test]
fn requires_an_author_when_tracked_and_validates_styles_first() {
    let mut doc = open(MINIMAL);
    let before = bytes(&doc);
    let err = doc
        .insert_section_after(
            BlockTarget::Text("First body paragraph"),
            "X",
            &[],
            &SectionOptions {
                tracked: true,
                ..SectionOptions::default()
            },
        )
        .unwrap_err();
    assert!(err.message().contains("author"));
    assert_eq!(
        refusal_of(doc.insert_section_after(
            BlockTarget::Text("First body paragraph"),
            "X",
            &strings(&["Y"]),
            &SectionOptions {
                heading_style: Some("NoSuchStyle99"),
                ..SectionOptions::default()
            },
        )),
        RefusalKind::TargetNotFound
    );
    assert_eq!(bytes(&doc), before);
}

#[test]
fn refuses_cross_paragraph_targets_before_protection_atomically() {
    let mut doc = memory(&["alpha end", "beta start", "after"]);
    doc.set_protection("readOnly").unwrap();
    let before = bytes(&doc);
    let span = doc
        .find_one("alpha end\nbeta start", &FindOptions::default())
        .unwrap();
    for target in [
        BlockTarget::Span(&span),
        BlockTarget::Text("alpha end\nbeta start"),
    ] {
        let none = SectionOptions {
            heading_style: None,
            ..SectionOptions::default()
        };
        let err = doc
            .insert_section_after(target, "wrong", &[], &none)
            .unwrap_err();
        assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
        assert!(err.message().contains("wholly within one paragraph"));
        assert!(err.message().contains("explicit live Block"));
    }
    assert_eq!(bytes(&doc), before);
}

#[test]
fn live_blocks_follow_their_element_across_index_shifts() {
    let mut doc = memory(&["A", "target", "C"]);
    let target = doc.blocks(View::Current).unwrap().remove(1);
    doc.insert_section_after(
        BlockTarget::Text("A"),
        "new second",
        &[],
        &SectionOptions {
            heading_style: None,
            ..SectionOptions::default()
        },
    )
    .unwrap();
    doc.insert_section_after(
        BlockTarget::Block(&target),
        "after target",
        &[],
        &SectionOptions {
            heading_style: None,
            ..SectionOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        texts(&doc),
        vec!["A", "new second", "target", "after target", "C"]
    );
}

#[test]
fn refuses_foreign_historical_and_table_targets() {
    let source = memory(&["foreign"]);
    let mut doc = memory(&["local", "target"]);
    let before = bytes(&doc);
    let foreign = source.blocks(View::Current).unwrap().remove(0);
    let none = SectionOptions {
        heading_style: None,
        ..SectionOptions::default()
    };
    assert_eq!(
        refusal_of(doc.insert_section_after(BlockTarget::Block(&foreign), "x", &[], &none)),
        RefusalKind::BoundaryViolation
    );
    for view in [View::Original, View::All] {
        let block = doc
            .blocks(view)
            .unwrap()
            .into_iter()
            .find(|b| b.text == "target")
            .unwrap();
        let err = doc
            .insert_section_after(BlockTarget::Block(&block), "x", &[], &none)
            .unwrap_err();
        assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
        assert!(err.message().contains(&format!("view={:?}", view.as_str())));
        assert!(err.message().contains(r#"view="current""#));
    }
    let mut tables = Document::open_bytes(
        &minimal_package_with(&format!(
            r#"<w:document {W_DECL}><w:body><w:p><w:r><w:t>paragraph</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#
        )),
        &Limits::default(),
    )
    .unwrap();
    let table = tables
        .blocks(View::Current)
        .unwrap()
        .into_iter()
        .find(|b| b.kind == docrs::BlockKind::Table)
        .unwrap();
    let err = tables
        .insert_section_after(BlockTarget::Block(&table), "x", &[], &none)
        .unwrap_err();
    assert!(err.message().contains("table block"));
    assert_eq!(
        refusal_of(tables.insert_section_after(BlockTarget::Text("cell"), "x", &[], &none)),
        RefusalKind::UnsupportedStructure
    );
    assert_eq!(bytes(&doc), before);
}

#[test]
fn stale_spans_refuse_before_protection() {
    let mut doc = memory(&["target text"]);
    let span = doc.find_one("target", &FindOptions::default()).unwrap();
    let edit = doc
        .find_one("target text", &FindOptions::default())
        .unwrap();
    doc.replace(&edit, "changed text", &Default::default())
        .unwrap();
    doc.set_protection("readOnly").unwrap();
    let before = bytes(&doc);
    assert_eq!(
        refusal_of(doc.insert_section_after(
            BlockTarget::Span(&span),
            "wrong",
            &[],
            &SectionOptions {
                heading_style: None,
                ..SectionOptions::default()
            }
        )),
        RefusalKind::TargetNotFound
    );
    assert_eq!(bytes(&doc), before);
}

fn count_named(tree: &XmlTree, root: docrs::xml::NodeId, local: &str) -> usize {
    tree.descendants(root)
        .filter(|n| tree.is(*n, W, local))
        .count()
}

#[test]
fn tracked_delete_keeps_each_runs_formatting_inside_the_deletion() {
    let mut doc = open("generated/feature-isolated/fragmented-runs.docx");
    let result = doc
        .tracked_delete_paragraphs(
            ParagraphRange::one(BlockTarget::Text("Consulting rate")),
            "Carol QA",
            frozen(),
        )
        .unwrap();
    assert_eq!(result.deleted_blocks, 1);
    assert_eq!(
        result.deleted_text,
        vec![
            "Consulting rate: $75–100/hr on a “full-service” basis — travel time billed at $37.50/hr."
        ]
    );
    assert_eq!(result.revision_ids, vec![1]);
    let tree = doc.package().xml("word/document.xml").unwrap();
    let deletions: Vec<_> = tree
        .descendants(tree.root())
        .filter(|n| tree.is(*n, W, "del") && tree.parent(*n).is_some_and(|p| tree.is(p, W, "p")))
        .collect();
    assert_eq!(deletions.len(), 1);
    let deletion = deletions[0];
    let runs: Vec<_> = tree
        .child_elements(deletion)
        .filter(|n| tree.is(*n, W, "r"))
        .collect();
    assert_eq!(runs.len(), 8, "each original run survives inside w:del");
    assert!(count_named(tree, deletion, "delText") > 0);
    assert_eq!(count_named(tree, deletion, "t"), 0);
    let bold = runs
        .iter()
        .filter(|r| {
            tree.first_child_named(**r, W, "rPr")
                .is_some_and(|rpr| tree.first_child_named(rpr, W, "b").is_some())
        })
        .count();
    assert_eq!(bold, 3);
    assert!(!texts(&doc).iter().any(|t| t.starts_with("Consulting rate")));
    assert!(
        doc.blocks(View::Original)
            .unwrap()
            .iter()
            .any(|b| b.text.starts_with("Consulting rate"))
    );
}

#[test]
fn tracked_delete_selects_ranges_among_siblings() {
    let mut doc = open(MINIMAL);
    let result = doc
        .tracked_delete_paragraphs(
            ParagraphRange {
                start: BlockTarget::Text("First body paragraph"),
                end: Some(BlockTarget::Text("Second body paragraph")),
                count: 1,
            },
            "Carol QA",
            frozen(),
        )
        .unwrap();
    assert_eq!(result.deleted_blocks, 2);
    assert_eq!(result.revision_ids, vec![1, 2]);

    let mut tables = open("generated/feature-isolated/table-merged-nested.docx");
    let before = bytes(&tables);
    assert_eq!(
        refusal_of(tables.tracked_delete_paragraphs(
            ParagraphRange {
                start: BlockTarget::Text("Paragraph before the merged"),
                end: Some(BlockTarget::Text("N11")),
                count: 1,
            },
            "Carol QA",
            frozen(),
        )),
        RefusalKind::BoundaryViolation
    );
    assert_eq!(bytes(&tables), before);
    let result = tables
        .tracked_delete_paragraphs(
            ParagraphRange {
                start: BlockTarget::Text("Paragraph before the merged"),
                end: None,
                count: 2,
            },
            "Carol QA",
            frozen(),
        )
        .unwrap();
    assert_eq!(result.deleted_text[1], "Paragraph after the tables.");
}

#[test]
fn tracked_delete_refusals_leave_documents_unchanged() {
    let mut doc = open(MINIMAL);
    let before = bytes(&doc);
    let err = doc
        .tracked_delete_paragraphs(
            ParagraphRange {
                start: BlockTarget::Text("Second body paragraph"),
                end: None,
                count: 10,
            },
            "Carol QA",
            frozen(),
        )
        .unwrap_err();
    assert!(err.message().contains("past the last"));
    let err = doc
        .tracked_delete_paragraphs(
            ParagraphRange::one(BlockTarget::Text("Second body paragraph")),
            "",
            frozen(),
        )
        .unwrap_err();
    assert!(err.message().contains("author"));
    assert_eq!(bytes(&doc), before);

    let mut controls = open("generated/feature-isolated/content-control.docx");
    let before = bytes(&controls);
    assert_eq!(
        refusal_of(controls.tracked_delete_paragraphs(
            ParagraphRange::one(BlockTarget::Text("Inline control follows")),
            "Carol QA",
            frozen(),
        )),
        RefusalKind::UnsupportedStructure
    );
    assert_eq!(bytes(&controls), before);

    let mut gauntlet = open("generated/gauntlet/gauntlet.docx");
    let err = gauntlet
        .tracked_delete_paragraphs(
            ParagraphRange {
                start: BlockTarget::Text("Gauntlet section two body"),
                end: Some(BlockTarget::Text("Gauntlet header, section one")),
                count: 1,
            },
            "Carol QA",
            frozen(),
        )
        .unwrap_err();
    assert!(err.message().contains("different story"));
}

#[test]
fn tracked_replace_deletes_and_inserts_within_budget() {
    let mut doc = open(MINIMAL);
    let result = doc
        .tracked_replace_paragraphs(
            ParagraphRange::one(BlockTarget::Text("Second body paragraph")),
            &strings(&["Replacement one.", "Replacement two."]),
            None,
            "Carol QA",
            frozen(),
        )
        .unwrap();
    assert_eq!(
        result.to_json(),
        serde_json::json!({"schema": "paper_block_edit", "version": 1, "story": "word/document.xml",
            "inserted_blocks": 2, "deleted_blocks": 1,
            "deleted_text": ["Second body paragraph, equally unremarkable."], "revision_ids": [1, 2, 3]})
    );
    let (out, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    let got = texts(&reopened);
    assert!(got.contains(&"Replacement one.".to_string()));
    assert!(got.contains(&"Replacement two.".to_string()));
}

#[test]
fn inserts_paragraphs_lists_and_tables() {
    let mut doc = open(MINIMAL);
    let blocks = vec![
        RichBlock::Paragraph(RichParagraph {
            runs: vec![
                TextRun {
                    text: "Key findings".into(),
                    bold: true,
                    italic: false,
                },
                TextRun {
                    text: " (draft)".into(),
                    bold: false,
                    italic: true,
                },
            ],
            style: None,
        }),
        RichBlock::List(ListBlock {
            items: strings(&["Alpha", "Beta"]),
            kind: "decimal".into(),
            level: 0,
        }),
        RichBlock::Table(TableBlock {
            rows: vec![strings(&["Metric", "Value"]), strings(&["Uptime", "99.9%"])],
        }),
    ];
    let result = doc
        .insert_blocks_after(
            BlockTarget::Text("First body paragraph"),
            &blocks,
            false,
            None,
            None,
        )
        .unwrap();
    assert_eq!(result.inserted_blocks, 4);
    let (out, report) = doc.patch_bytes().unwrap();
    let mut changed = report.changed_parts.clone();
    changed.sort();
    assert_eq!(
        changed,
        vec![
            "word/document.xml".to_string(),
            "word/numbering.xml".to_string()
        ]
    );
    assert!(report.added_parts.is_empty());
    let reopened = Document::open_bytes(&out, &Limits::default()).unwrap();
    let blocks = reopened.blocks(View::Current).unwrap();
    assert!(blocks.iter().any(|b| b.text == "Key findings (draft)"));
    let table = blocks
        .iter()
        .find(|b| b.kind == docrs::BlockKind::Table)
        .unwrap();
    assert!(table.text.contains("Uptime"));
    // Section width rule: 12240 - 1800 - 1800 twips over two columns.
    assert!(body_xml(&reopened).contains(r#"<w:gridCol w:w="4320"/><w:gridCol w:w="4320"/>"#));
    let numbered = reopened.list_numbering().unwrap().numbered_paragraphs;
    assert_eq!(numbered.len(), 2);
    assert_eq!(numbered[0].num_id, 10);
}

#[test]
fn tracked_rich_blocks_refuse_tables_and_validate_shapes() {
    let mut doc = open(MINIMAL);
    let before = bytes(&doc);
    let table = |rows: Vec<Vec<String>>| RichBlock::Table(TableBlock { rows });
    assert_eq!(
        refusal_of(doc.insert_blocks_after(
            BlockTarget::Text("First body paragraph"),
            &[table(vec![strings(&["x"])])],
            true,
            Some("Carol QA"),
            frozen(),
        )),
        RefusalKind::UnsupportedStructure
    );
    let err = doc
        .insert_blocks_after(
            BlockTarget::Text("First body paragraph"),
            &[table(vec![strings(&["a", "b"]), strings(&["c"])])],
            false,
            None,
            None,
        )
        .unwrap_err();
    assert!(err.message().contains("rectangular"));
    let err = doc
        .insert_blocks_after(
            BlockTarget::Text("First body paragraph"),
            &[RichBlock::List(ListBlock {
                items: strings(&["x"]),
                kind: "fancy".into(),
                level: 0,
            })],
            false,
            None,
            None,
        )
        .unwrap_err();
    assert!(err.message().contains("kind"));
    assert_eq!(bytes(&doc), before);

    let result = doc
        .insert_blocks_after(
            BlockTarget::Text("First body paragraph"),
            &[
                RichBlock::Paragraph(RichParagraph {
                    runs: vec![TextRun {
                        text: "Tracked lead".into(),
                        bold: true,
                        italic: false,
                    }],
                    style: None,
                }),
                RichBlock::List(ListBlock {
                    items: strings(&["one"]),
                    ..ListBlock::default()
                }),
            ],
            true,
            Some("Carol QA"),
            Some(Timestamp::parse("2026-07-08T09:00:00Z").unwrap()),
        )
        .unwrap();
    assert_eq!(result.revision_ids, vec![1, 2]);
    let original: Vec<String> = doc
        .blocks(View::Original)
        .unwrap()
        .into_iter()
        .map(|b| b.text)
        .collect();
    assert!(!original.contains(&"Tracked lead".to_string()));
}

#[test]
fn tables_are_padded_where_word_would_fuse_them() {
    let mut doc = memory(&["only"]);
    let table = RichBlock::Table(TableBlock {
        rows: vec![strings(&["a"])],
    });
    let result = doc
        .insert_blocks_after(
            BlockTarget::Text("only"),
            &[table.clone(), table.clone()],
            false,
            None,
            None,
        )
        .unwrap();
    // table, pad, table, pad (story end)
    assert_eq!(result.inserted_blocks, 4);
    let xml = body_xml(&doc);
    assert!(xml.contains("</w:tbl><w:p/><w:tbl>"));
    assert!(xml.ends_with("</w:tbl><w:p/></w:body></w:document>"));
    // no sections: 6 inches over one column
    assert!(xml.contains(r#"<w:gridCol w:w="8640"/>"#));
}
