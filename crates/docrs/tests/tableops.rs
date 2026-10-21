//! Table operations (upstream `tableops.py`): oracle-independent
//! regressions with expected values frozen from the pinned oracle, refusal
//! atomicity (the package is byte-identical after every refusal) and live
//! table-handle freshness.

mod support;

use docrs::clock::Timestamp;
use docrs::package::zip_read::read_archive;
use docrs::tableops::UpdateCellOptions;
use docrs::xml;
use docrs::{Document, ErrorKind, Limits, MatchPolicy, RefusalKind};
use support::*;

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";
const MERGED: &str = "generated/feature-isolated/table-merged-nested.docx";

/// `minimal.docx` with its body content replaced (sectPr kept).
fn with_body(body: &str) -> Vec<u8> {
    let bytes = fixture(MINIMAL);
    let archive = read_archive(&bytes, &Limits::default()).unwrap();
    let members: Vec<RawMember> = archive
        .members
        .iter()
        .map(|member| {
            if member.name == "word/document.xml" {
                let xml = String::from_utf8(member.data.clone()).unwrap();
                let start = xml.find("<w:body>").unwrap() + "<w:body>".len();
                let end = xml.find("<w:sectPr").unwrap();
                let data = format!("{}{}{}", &xml[..start], body, &xml[end..]);
                RawMember::new(&member.name, data.as_bytes())
            } else {
                RawMember::new(&member.name, &member.data)
            }
        })
        .collect();
    build_zip(&members)
}

fn r(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn rp(props: &str, text: &str) -> String {
    format!("<w:r><w:rPr>{props}</w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn tc(inner: &str) -> String {
    format!("<w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/></w:tcPr>{inner}</w:tc>")
}

fn pt(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn tbl(columns: usize, rows: &[Vec<String>]) -> String {
    let grid: String = (0..columns).map(|_| "<w:gridCol w:w=\"4320\"/>").collect();
    let rows: String = rows
        .iter()
        .map(|cells| format!("<w:tr>{}</w:tr>", cells.concat()))
        .collect();
    format!(
        "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{rows}</w:tbl>"
    )
}

/// The python-docx `_doc_with_simple_table()` shape.
fn simple() -> Document {
    let table = tbl(
        2,
        &[
            vec![tc(&pt("cell 00")), tc(&pt("cell 01"))],
            vec![tc(&pt("cell 10")), tc(&pt("cell 11"))],
        ],
    );
    open(&with_body(&format!("{}{table}<w:p/>", pt("before"))))
}

fn open(bytes: &[u8]) -> Document {
    Document::open_bytes(bytes, &Limits::default()).unwrap()
}

fn snapshot(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

fn document_xml(doc: &Document) -> String {
    String::from_utf8(xml::serialize(
        doc.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap()
}

fn reopen(doc: &Document) -> Document {
    open(&snapshot(doc))
}

fn tracked(author: &str) -> UpdateCellOptions {
    UpdateCellOptions {
        tracked: true,
        author: Some(author.to_string()),
        date: Some(Timestamp::parse("2024-01-02T03:04:05Z").unwrap()),
    }
}

fn grid(doc: &Document, near: &str) -> Vec<Vec<String>> {
    let table = doc.find_table(near, MatchPolicy::Exact).unwrap();
    doc.table_grid(&table).unwrap()
}

fn strings(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|s| s.to_string()).collect())
        .collect()
}

#[test]
fn find_table_matches_one_physical_cell_and_refuses_ambiguity() {
    let doc = simple();
    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    assert_eq!(
        doc.table_grid(&table).unwrap(),
        strings(&[&["cell 00", "cell 01"], &["cell 10", "cell 11"]])
    );
    for (needle, policy) in [("", MatchPolicy::Exact), ("   \t", MatchPolicy::Normalized)] {
        let err = doc.find_table(needle, policy).unwrap_err();
        assert_eq!(err.reason(), "empty_near_text");
    }
    let err = doc
        .find_table("cell 00\ncell 01", MatchPolicy::Exact)
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    assert_eq!(
        doc.find_table("CELL 10", MatchPolicy::Normalized)
            .map(|t| doc.table_grid(&t).unwrap()[1][0].clone())
            .unwrap(),
        "cell 10"
    );
    let err = doc.find_table("cell", MatchPolicy::Exact);
    assert!(
        err.is_ok(),
        "one table holding several matching cells is one match"
    );

    let two = open(&with_body(&format!(
        "{}{}",
        tbl(1, &[vec![tc(&pt("dup marker A"))]]),
        tbl(1, &[vec![tc(&pt("dup marker B"))]])
    )));
    let err = two
        .find_table("dup marker", MatchPolicy::Exact)
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::AmbiguousTarget));
    assert!(err.message().contains("2 tables"));
}

#[test]
fn merged_grid_matches_python_docx_row_cells() {
    let doc = open(&fixture(MERGED));
    // Frozen from the oracle: `[[c.text for c in row.cells] for row in rows]`.
    assert_eq!(
        grid(&doc, "R1C0"),
        strings(&[
            &["R0C0\nR0C1", "R0C0\nR0C1", "R0C2"],
            &["R1C0", "R1C1", "R1C2\nR2C2"],
            &["R2C0\n", "R2C1", "R1C2\nR2C2"],
        ])
    );
    assert!(doc.find_table("N00", MatchPolicy::Exact).is_err());
}

#[test]
fn update_cell_routes_through_replacement_with_exact_budget() {
    let mut doc = simple();
    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    let report = doc
        .update_cell(&table, 0, 1, "updated value", &UpdateCellOptions::default())
        .unwrap();
    assert_eq!(report.deleted_text, "cell 01");
    assert_eq!(report.inserted_text, "updated value");
    assert!(report.preserved_formatting_regions);
    assert!(!report.tracked);
    let (bytes, patch) = doc.patch_bytes().unwrap();
    let json = patch.to_json();
    assert_eq!(
        json["changed_parts"],
        serde_json::json!(["word/document.xml"])
    );
    assert_eq!(json["added_parts"], serde_json::json!([]));
    let reopened = open(&bytes);
    assert_eq!(grid(&reopened, "updated value")[0][1], "updated value");
    // The handle stays live across this module's own edits.
    assert_eq!(doc.table_grid(&table).unwrap()[0][1], "updated value");
}

#[test]
fn update_cell_fills_an_empty_cell_untracked_and_tracked() {
    let body = tbl(1, &[vec![tc("<w:p/>")], vec![tc(&pt("marker"))]]);
    let mut doc = open(&with_body(&body));
    let table = doc.find_table("marker", MatchPolicy::Exact).unwrap();
    let report = doc
        .update_cell(&table, 0, 0, "was empty", &UpdateCellOptions::default())
        .unwrap();
    assert!(!report.preserved_formatting_regions);
    assert_eq!(report.deleted_text, "");
    assert!(document_xml(&doc).contains("<w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/></w:tcPr><w:p><w:r><w:t>was empty</w:t></w:r></w:p></w:tc>"));

    let mut doc = open(&with_body(&body));
    let table = doc.find_table("marker", MatchPolicy::Exact).unwrap();
    let report = doc
        .update_cell(&table, 0, 0, "was empty", &tracked("Carol QA"))
        .unwrap();
    assert!(report.tracked);
    assert_eq!(report.revision_ids, vec![1]);
    assert!(document_xml(&doc).contains(
        "<w:p><w:ins w:id=\"1\" w:author=\"Carol QA\" w:date=\"2024-01-02T03:04:05Z\"><w:r><w:t>was empty</w:t></w:r></w:ins></w:p>"
    ));
}

#[test]
fn tracked_update_authors_deletion_and_insertion_as_upstream() {
    let mut doc = simple();
    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    let report = doc
        .update_cell(&table, 1, 0, "cell 99", &tracked("Carol QA"))
        .unwrap();
    // Frozen from the oracle (`update_cell(..., tracked=True)`).
    assert_eq!(report.deleted_text, "10");
    assert_eq!(report.inserted_text, "99");
    assert_eq!(report.revision_ids, vec![1, 2]);
    assert!(!report.preserved_formatting_regions);
    assert!(document_xml(&doc).contains(
        "<w:p><w:r><w:t xml:space=\"preserve\">cell </w:t></w:r><w:del w:id=\"1\" w:author=\"Carol QA\" \
         w:date=\"2024-01-02T03:04:05Z\"><w:r><w:delText>10</w:delText></w:r></w:del><w:ins w:id=\"2\" \
         w:author=\"Carol QA\" w:date=\"2024-01-02T03:04:05Z\"><w:r><w:t>99</w:t></w:r></w:ins></w:p>"
    ));
    // Next ids continue above every existing revision id.
    let report = doc
        .update_cell(&table, 0, 0, "cell 0X", &tracked("Carol QA"))
        .unwrap();
    assert_eq!(report.revision_ids, vec![3, 4]);
    assert!(
        reopen(&doc)
            .find_table("cell 11", MatchPolicy::Exact)
            .is_ok()
    );
}

#[test]
fn tracked_update_keeps_each_source_runs_formatting() {
    let cell = format!(
        "<w:p>{}{}{}</w:p>",
        rp("<w:b/>", "alpha "),
        rp("<w:i/>", "beta "),
        r("gamma")
    );
    let mut doc = open(&with_body(&tbl(2, &[vec![tc(&pt("mid")), tc(&cell)]])));
    let table = doc.find_table("mid", MatchPolicy::Exact).unwrap();
    let report = doc
        .update_cell(&table, 0, 1, "alpha BETA gamma", &tracked("Carol QA"))
        .unwrap();
    assert_eq!(
        (report.deleted_text.as_str(), report.inserted_text.as_str()),
        ("beta", "BETA")
    );
    assert_eq!(report.revision_ids, vec![1, 2]);
    // Frozen from the oracle.
    let xml = document_xml(&doc);
    assert!(xml.contains(
        "<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">alpha </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t></w:t></w:r>\
         <w:del w:id=\"1\" w:author=\"Carol QA\" w:date=\"2024-01-02T03:04:05Z\"><w:r><w:rPr><w:i/></w:rPr><w:delText>beta</w:delText></w:r></w:del>\
         <w:ins w:id=\"2\" w:author=\"Carol QA\" w:date=\"2024-01-02T03:04:05Z\"><w:r><w:rPr><w:i/></w:rPr><w:t>BETA</w:t></w:r></w:ins>\
         <w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\"> </w:t></w:r><w:r><w:t xml:space=\"preserve\">gamma</w:t></w:r>"
    ), "{xml}");
}

#[test]
fn update_cell_argument_and_address_refusals() {
    let mut doc = simple();
    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    let before = snapshot(&doc);
    let options = UpdateCellOptions {
        tracked: true,
        ..UpdateCellOptions::default()
    };
    let err = doc.update_cell(&table, 0, 0, "x", &options).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert_eq!(err.reason(), "author_required");
    let err = doc
        .update_cell(&table, 7, 0, "x", &UpdateCellOptions::default())
        .unwrap_err();
    assert!(err.message().contains("row 7"));
    let err = doc
        .update_cell(&table, 0, 9, "x", &UpdateCellOptions::default())
        .unwrap_err();
    assert!(err.message().contains("column 9"));
    let err = doc
        .update_cell(&table, 0, 0, "a\nb", &UpdateCellOptions::default())
        .unwrap_err();
    assert_eq!(err.reason(), "control_characters");
    let err = doc
        .update_cell(&table, 1, 0, "cell 10", &tracked("A"))
        .unwrap_err();
    assert_eq!(err.reason(), "tracked_noop");
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn cell_wise_guards_refuse_atomically_and_leave_neighbours_editable() {
    let mut doc = open(&fixture(MERGED));
    let table = doc.find_table("R1C0", MatchPolicy::Exact).unwrap();
    let before = snapshot(&doc);
    for (row, column, reason) in [
        (0, 0, "merged_cell"),
        (1, 2, "merged_cell"),
        (2, 2, "merged_cell"),
        (2, 0, "nested_table_cell"),
    ] {
        let err = doc
            .update_cell(&table, row, column, "x", &UpdateCellOptions::default())
            .unwrap_err();
        assert_eq!(err.reason(), reason, "cell ({row}, {column})");
        assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
        assert_eq!(snapshot(&doc), before);
    }
    doc.update_cell(
        &table,
        1,
        0,
        "updated plain cell",
        &UpdateCellOptions::default(),
    )
    .unwrap();
    assert_eq!(
        grid(&reopen(&doc), "updated plain cell")[1][0],
        "updated plain cell"
    );
}

#[test]
fn structure_refusals_leave_the_package_byte_identical() {
    let cells = [
        (
            "marker",
            format!(
                "<w:p>{}<w:proofErr w:type=\"spellStart\"/>{}</w:p>",
                r("cell "),
                r("value")
            ),
            RefusalKind::UnsupportedStructure,
        ),
        (
            "term",
            format!("<w:p>{}{}</w:p>", rp("<w:b/>", "Term"), r("Term")),
            RefusalKind::UnsupportedStructure,
        ),
        (
            "link",
            format!(
                "<w:p><w:hyperlink w:anchor=\"target\">{}</w:hyperlink>{}</w:p>",
                r("cell "),
                r("value")
            ),
            RefusalKind::BoundaryViolation,
        ),
        (
            "tab",
            "<w:p><w:r><w:t>left</w:t><w:tab/><w:t>right</w:t></w:r></w:p>".to_string(),
            RefusalKind::UnsupportedStructure,
        ),
        (
            "field",
            format!(
                "<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r></w:p>{}<w:p><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>",
                pt("result")
            ),
            RefusalKind::UnsupportedStructure,
        ),
        (
            "checkbox",
            "<w:p><w:sdt><w:sdtPr><w14:checkbox/></w:sdtPr><w:sdtContent/></w:sdt></w:p>"
                .to_string(),
            RefusalKind::UnsupportedStructure,
        ),
        (
            "multi",
            format!("{}{}", pt("first para"), pt("second para")),
            RefusalKind::UnsupportedStructure,
        ),
        (
            "revision",
            "<w:p><w:ins w:id=\"5\" w:author=\"Ann\"><w:r><w:t>inserted</w:t></w:r></w:ins></w:p>"
                .to_string(),
            RefusalKind::UnsupportedStructure,
        ),
    ];
    for (marker, cell, kind) in cells {
        let mut doc = open(&with_body(&tbl(2, &[vec![tc(&pt(marker)), tc(&cell)]])));
        let table = doc.find_table(marker, MatchPolicy::Exact).unwrap();
        let before = snapshot(&doc);
        for options in [UpdateCellOptions::default(), tracked("A")] {
            let new_text = if marker == "term" { "Term" } else { "updated" };
            let err = doc
                .update_cell(&table, 0, 1, new_text, &options)
                .unwrap_err();
            assert_eq!(err.refusal_kind(), Some(kind), "{marker}: {err}");
            assert_eq!(snapshot(&doc), before, "{marker}");
        }
        // The handle is still live after refusals.
        doc.update_cell(&table, 0, 0, "neighbour", &UpdateCellOptions::default())
            .unwrap();
    }
}

#[test]
fn protection_gates_every_table_mutation() {
    let mut doc = simple();
    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    doc.set_protection("readOnly").unwrap();
    let before = snapshot(&doc);
    let protected = |err: docrs::Error| {
        assert_eq!(err.refusal_kind(), Some(RefusalKind::DocumentProtected));
    };
    protected(
        doc.update_cell(&table, 0, 0, "x", &UpdateCellOptions::default())
            .unwrap_err(),
    );
    protected(doc.insert_row_after(&table, 0, &["x"], None).unwrap_err());
    protected(doc.delete_row(&table, 0).unwrap_err());
    assert_eq!(snapshot(&doc), before);
    doc.acknowledge_protection().unwrap();
    doc.delete_row(&table, 0).unwrap();
}

#[test]
fn insert_row_copies_uniform_template_formatting_exactly() {
    let run = |text: &str| {
        rp(
            "<w:highlight w:val=\"yellow\"/><w:lang w:val=\"en-US\"/>",
            text,
        )
    };
    let body = format!(
        "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"8640\"/></w:tblGrid>\
         <w:tr><w:trPr><w:trHeight w:val=\"576\"/></w:trPr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"8640\"/>\
         <w:shd w:fill=\"D9EAF7\"/></w:tcPr><w:p><w:pPr><w:jc w:val=\"right\"/></w:pPr>{}{}</w:p></w:tc></w:tr></w:tbl>",
        run("uniform "),
        run("template")
    );
    let mut doc = open(&with_body(&body));
    let table = doc
        .find_table("uniform template", MatchPolicy::Exact)
        .unwrap();
    doc.insert_row_after(&table, 0, &["replacement value"], None)
        .unwrap();
    let (bytes, patch) = doc.patch_bytes().unwrap();
    assert_eq!(
        patch.to_json()["changed_parts"],
        serde_json::json!(["word/document.xml"])
    );
    let reopened = open(&bytes);
    assert_eq!(
        grid(&reopened, "uniform template"),
        strings(&[&["uniform template"], &["replacement value"]])
    );
    let xml = document_xml(&reopened);
    assert!(xml.contains(
        "<w:tr><w:trPr><w:trHeight w:val=\"576\"/></w:trPr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"8640\"/>\
         <w:shd w:fill=\"D9EAF7\"/></w:tcPr><w:p><w:pPr><w:jc w:val=\"right\"/></w:pPr><w:r><w:rPr>\
         <w:highlight w:val=\"yellow\"/><w:lang w:val=\"en-US\"/></w:rPr><w:t>replacement value</w:t></w:r></w:p></w:tc></w:tr>"
    ));
}

#[test]
fn row_operations_follow_python_docx_and_refuse_atomically() {
    let mut doc = simple();
    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    doc.insert_row_after(&table, 0, &["new a", "new b"], None)
        .unwrap();
    doc.insert_row_after(&table, 2, &["only one"], None)
        .unwrap();
    assert_eq!(
        doc.table_grid(&table).unwrap(),
        strings(&[
            &["cell 00", "cell 01"],
            &["new a", "new b"],
            &["cell 10", "cell 11"],
            &["only one", ""],
        ])
    );
    let before = snapshot(&doc);
    let err = doc
        .insert_row_after(&table, 0, &["a", "b", "c"], None)
        .unwrap_err();
    assert_eq!(err.reason(), "too_many_values");
    let err = doc.insert_row_after(&table, 9, &["a"], None).unwrap_err();
    assert_eq!(err.reason(), "row_out_of_range");
    let err = doc
        .insert_row_after(&table, 0, &["a"], Some(9))
        .unwrap_err();
    assert_eq!(err.reason(), "copy_format_row_out_of_range");
    let err = doc
        .insert_row_after(&table, 0, &["a\tb"], None)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert_eq!(snapshot(&doc), before);
    for _ in 0..3 {
        doc.delete_row(&table, 0).unwrap();
    }
    assert_eq!(
        doc.table_grid(&table).unwrap(),
        strings(&[&["only one", ""]])
    );
    let before = snapshot(&doc);
    let err = doc.delete_row(&table, 0).unwrap_err();
    assert_eq!(err.reason(), "last_row");
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn merged_rows_refuse_and_grid_span_rows_delete() {
    let mut doc = open(&fixture(MERGED));
    let table = doc.find_table("R1C0", MatchPolicy::Exact).unwrap();
    let before = snapshot(&doc);
    let reason = |err: docrs::Error| err.reason();
    assert_eq!(
        reason(doc.insert_row_after(&table, 1, &["x"], None).unwrap_err()),
        "vertical_merge_row"
    );
    assert_eq!(
        reason(doc.delete_row(&table, 1).unwrap_err()),
        "vertical_merge_row"
    );
    assert_eq!(
        reason(
            doc.insert_row_after(&table, 1, &["x"], Some(0))
                .unwrap_err()
        ),
        "splits_vertical_merge"
    );
    assert_eq!(
        reason(
            doc.insert_row_after(&table, 0, &["a", "b", "c"], None)
                .unwrap_err()
        ),
        "grid_span_template"
    );
    assert_eq!(snapshot(&doc), before);
    doc.delete_row(&table, 0).unwrap();
    assert_eq!(doc.table_grid(&table).unwrap().len(), 2);
}

#[test]
fn complex_and_tracked_templates_refuse_before_population() {
    let complex = [
        format!("<w:p><w:fldSimple>{}</w:fldSimple></w:p>", r("result")),
        "<w:p><w:r><w:t>template</w:t><w:drawing/></w:r></w:p>".to_string(),
        format!(
            "<w:p>{}{}</w:p>",
            rp("<w:b/>", "first"),
            rp("<w:i/>", "second")
        ),
        format!("<w:p><w:customXml>{}</w:customXml></w:p>", r("wrapped")),
        format!("<w:p>{}<w:pPr/></w:p>", r("template")),
        "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t></w:t></w:r></w:p>".to_string(),
    ];
    for cell in complex {
        let mut doc = open(&with_body(&tbl(
            2,
            &[vec![tc(&pt("valid first")), tc(&cell)]],
        )));
        let table = doc.find_table("valid first", MatchPolicy::Exact).unwrap();
        let before = snapshot(&doc);
        let err = doc
            .insert_row_after(&table, 0, &["changed", "changed"], None)
            .unwrap_err();
        assert_eq!(err.reason(), "complex_template_cell", "{cell}");
        assert!(err.message().contains("simpler uniform template"));
        assert_eq!(snapshot(&doc), before);
    }
    let row = format!(
        "<w:tr><w:trPr><w:ins w:id=\"77\" w:author=\"Reviewer\"/></w:trPr>{}</w:tr>",
        tc(&pt("template"))
    );
    let body =
        format!("<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"4320\"/></w:tblGrid>{row}</w:tbl>");
    let mut doc = open(&with_body(&body));
    let table = doc.find_table("template", MatchPolicy::Exact).unwrap();
    let before = snapshot(&doc);
    let err = doc
        .insert_row_after(&table, 0, &["new row"], None)
        .unwrap_err();
    assert_eq!(err.reason(), "tracked_template_row");
    assert!(err.message().contains("tracked revision metadata (ins)"));
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn table_handles_are_document_bound_and_go_stale() {
    let mut doc = simple();
    let other = simple();
    let foreign = other.find_table("cell 10", MatchPolicy::Exact).unwrap();
    let err = doc
        .update_cell(&foreign, 0, 0, "x", &UpdateCellOptions::default())
        .unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::BoundaryViolation));
    assert_eq!(err.reason(), "foreign_table");

    let table = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    // Detach and reinsert the w:tbl behind the handle's back.
    let mut tree = doc.package().xml("word/document.xml").unwrap().clone();
    let body = tree.child_elements(tree.root()).next().unwrap();
    let tbl = tree
        .child_elements(body)
        .find(|node| tree.local_name(*node) == Some("tbl"))
        .unwrap();
    let index = tree.index_in_parent(tbl).unwrap();
    tree.detach(tbl).unwrap();
    tree.insert(body, index, tbl).unwrap();
    doc.package_mut_for_tests()
        .commit_tree("word/document.xml", tree)
        .unwrap();
    let before = snapshot(&doc);
    for err in [
        doc.update_cell(&table, 0, 0, "x", &UpdateCellOptions::default())
            .unwrap_err(),
        doc.insert_row_after(&table, 0, &["x"], None).unwrap_err(),
        doc.delete_row(&table, 0).unwrap_err(),
        doc.table_grid(&table).unwrap_err(),
    ] {
        assert_eq!(err.reason(), "stale_table");
        assert_eq!(err.refusal_kind(), Some(RefusalKind::TargetNotFound));
    }
    assert_eq!(snapshot(&doc), before);
    // A fresh handle works.
    let fresh = doc.find_table("cell 10", MatchPolicy::Exact).unwrap();
    doc.delete_row(&fresh, 0).unwrap();
}
