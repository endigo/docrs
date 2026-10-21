//! Synthetic documents for M4 work package p3 (tableops, formatting,
//! styles).
//!
//! Most documents start from the upstream `minimal.docx` fixture (the
//! python-docx default template) with a replaced body and extra styles, so
//! they reproduce the shapes upstream tests build with python-docx
//! (`add_table`, `cell.text = …`, `add_style`, …). The two `p3-sty-*`
//! documents carry the styles parts of upstream's Behave test files
//! `sty-behav-props.docx` and `sty-known-styles.docx` (python-docx, MIT;
//! see `NOTICE.md`), trimmed to the latent-style exceptions the scenarios
//! use.

use docrs::Limits;
use docrs::package::zip_read::read_archive;

use crate::hostile::build::{RawMember, build_zip, minimal_package_with};

const MINIMAL: &str = "generated/minimal-clean/minimal.docx";

/// `minimal.docx` with `body` replacing the body content (the `w:sectPr` is
/// kept) and `styles` appended to `word/styles.xml` (or replacing the whole
/// part when `replace_styles` is set).
fn from_minimal(body: &str, styles: &str, replace_styles: bool) -> Vec<u8> {
    let bytes = std::fs::read(crate::hostile::fixtures_dir().join(MINIMAL))
        .expect("minimal.docx fixture is vendored");
    let archive = read_archive(&bytes, &Limits::default()).expect("minimal.docx opens");
    let members: Vec<RawMember> = archive
        .members
        .iter()
        .map(|member| {
            let data = match member.name.as_str() {
                "word/document.xml" => {
                    let xml = String::from_utf8(member.data.clone()).expect("utf-8");
                    let start = xml.find("<w:body>").expect("body") + "<w:body>".len();
                    let end = xml.find("<w:sectPr").expect("sectPr");
                    format!("{}{}{}", &xml[..start], body, &xml[end..]).into_bytes()
                }
                "word/styles.xml" if replace_styles => styles.as_bytes().to_vec(),
                "word/styles.xml" => {
                    let xml = String::from_utf8(member.data.clone()).expect("utf-8");
                    xml.replace("</w:styles>", &format!("{styles}</w:styles>"))
                        .into_bytes()
                }
                _ => member.data.clone(),
            };
            RawMember::new(&member.name, &data)
        })
        .collect();
    build_zip(&members)
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `<w:r><w:t>text</w:t></w:r>` with `xml:space` as python-docx writes it.
fn r(text: &str) -> String {
    let space = if text.trim() != text {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    format!("<w:r><w:t{space}>{}</w:t></w:r>", esc(text))
}

/// A run with direct properties.
fn rp(props: &str, text: &str) -> String {
    let space = if text.trim() != text {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    format!(
        "<w:r><w:rPr>{props}</w:rPr><w:t{space}>{}</w:t></w:r>",
        esc(text)
    )
}

fn p(inner: &str) -> String {
    format!("<w:p>{inner}</w:p>")
}

fn pt(text: &str) -> String {
    p(&r(text))
}

/// A python-docx `add_table` cell: `tcW` then content.
fn tc(inner: &str) -> String {
    format!("<w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/></w:tcPr>{inner}</w:tc>")
}

fn tc_pr(props: &str, inner: &str) -> String {
    format!("<w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/>{props}</w:tcPr>{inner}</w:tc>")
}

fn tr(cells: &[String]) -> String {
    format!("<w:tr>{}</w:tr>", cells.concat())
}

fn tbl(columns: usize, rows: &[String]) -> String {
    let grid: String = (0..columns)
        .map(|_| "<w:gridCol w:w=\"4320\"/>".to_string())
        .collect();
    format!(
        "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/><w:tblLook w:firstColumn=\"1\" w:firstRow=\"1\" \
         w:lastColumn=\"0\" w:lastRow=\"0\" w:noHBand=\"0\" w:noVBand=\"1\" w:val=\"04A0\"/></w:tblPr>\
         <w:tblGrid>{grid}</w:tblGrid>{}</w:tbl>",
        rows.concat()
    )
}

/// A one-row, two-column table: marker cell plus `cell`.
fn marked(marker: &str, cell: &str) -> String {
    tbl(2, &[tr(&[tc(&pt(marker)), cell.to_string()])])
}

fn fld(kind: &str) -> String {
    format!("<w:r><w:fldChar w:fldCharType=\"{kind}\"/></w:r>")
}

fn table_documents() -> Vec<(String, Vec<u8>)> {
    let simple = tbl(
        2,
        &[
            tr(&[tc(&pt("cell 00")), tc(&pt("cell 01"))]),
            tr(&[tc(&pt("cell 10")), tc(&pt("cell 11"))]),
        ],
    );
    let bold = "<w:b/>";
    let tables = [
        simple,
        marked("T-empty", &tc("<w:p/>")),
        marked(
            "T-uniform",
            &tc(&p(&format!("{}{}", rp(bold, "cell "), rp(bold, "value")))),
        ),
        marked(
            "T-mixed",
            &tc(&p(&format!("{}{}", rp(bold, "Label: "), r("value")))),
        ),
        marked(
            "T-marker",
            &tc(&p(&format!(
                "{}<w:proofErr w:type=\"spellStart\"/>{}",
                r("cell "),
                r("value")
            ))),
        ),
        marked(
            "T-shade",
            &tc(&p(&rp("<w:shd w:fill=\"FFFF00\"/>", "shaded text"))),
        ),
        marked(
            "T-term",
            &tc(&p(&format!("{}{}", rp(bold, "Term"), r("Term")))),
        ),
        marked(
            "T-charstyle",
            &tc(&p(&format!(
                "{}{}",
                rp("<w:rStyle w:val=\"CellFirst\"/>", "cell "),
                rp("<w:rStyle w:val=\"CellSecond\"/>", "value")
            ))),
        ),
        marked(
            "T-link",
            &tc(&p(&format!(
                "<w:hyperlink w:anchor=\"target\">{}</w:hyperlink>{}",
                r("cell "),
                r("value")
            ))),
        ),
        marked(
            "T-tab",
            &tc(&p("<w:r><w:t>left</w:t><w:tab/><w:t>right</w:t></w:r>")),
        ),
        marked(
            "T-break",
            &tc(&p("<w:r><w:t>left</w:t><w:br/><w:t>right</w:t></w:r>")),
        ),
        marked(
            "T-field",
            &tc(&format!(
                "{}{}{}",
                p(&fld("begin")),
                pt("result"),
                p(&fld("end"))
            )),
        ),
        marked(
            "T-checkbox",
            &tc(&p(
                "<w:sdt><w:sdtPr><w14:checkbox/></w:sdtPr><w:sdtContent/></w:sdt>",
            )),
        ),
        format!(
            "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2880\"/>\
             <w:gridCol w:w=\"2880\"/><w:gridCol w:w=\"2880\"/></w:tblGrid><w:tr><w:trPr><w:gridBefore w:val=\"1\"/>\
             </w:trPr>{}{}</w:tr></w:tbl>",
            tc(&pt("visible column one")),
            tc(&pt("visible column two"))
        ),
        marked(
            "T-host",
            &tc(&format!(
                "{}{}<w:p/>",
                pt("host text"),
                tbl(1, &[tr(&[tc(&pt("inner cell"))])])
            )),
        ),
        marked(
            "T-multi",
            &tc(&format!("{}{}", pt("first para"), pt("second para"))),
        ),
        marked(
            "T-rev",
            &tc(&p(
                "<w:ins w:id=\"5\" w:author=\"Ann\" w:date=\"2020-01-01T00:00:00Z\"><w:r><w:t>inserted text</w:t></w:r></w:ins>",
            )),
        ),
        marked(
            "T-sdt",
            &tc(&p(&format!(
                "<w:sdt><w:sdtPr><w:tag w:val=\"cellctl\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>",
                r("control text")
            ))),
        ),
        marked(
            "T-placeholder",
            &tc(&p(&format!(
                "<w:sdt><w:sdtPr><w:showingPlcHdr/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>",
                r("Click here")
            ))),
        ),
        marked("T-second", &tc(&format!("<w:p/>{}", pt("second only")))),
        marked("T-space", &tc(&pt("padded"))),
        marked("T-unicode", &tc(&pt("Résumé – Total"))),
        tbl(2, &[tr(&[tc(&pt("Account")), tc(&pt("Balance"))])]),
        tbl(
            1,
            &[tr(&[tc(&format!("{}{}", pt("Acct line"), pt("Bal line")))])],
        ),
        tbl(
            2,
            &[tr(&[tc_pr(
                "<w:gridSpan w:val=\"2\"/>",
                &pt("merged marker"),
            )])],
        ),
        tbl(
            2,
            &[tr(&[
                tc(&pt("multi marker one")),
                tc(&pt("multi marker two")),
            ])],
        ),
        marked("dup marker A", &tc(&pt("dup a"))),
        marked("dup marker B", &tc(&pt("dup b"))),
        marked("T-single", &tc(&pt("only row"))),
        marked(
            "T-bookmark",
            &tc(&p(&format!(
                "{}<w:bookmarkStart w:id=\"9\" w:name=\"mark\"/>{}<w:bookmarkEnd w:id=\"9\"/>",
                r("keep "),
                r("marked")
            ))),
        ),
        marked(
            "T-mid",
            &tc(&p(&format!(
                "{}{}{}",
                rp(bold, "alpha "),
                rp("<w:i/>", "beta "),
                r("gamma")
            ))),
        ),
        marked(
            "T-trailing",
            &tc(&p(
                "<w:r><w:rPr><w:b/></w:rPr><w:t>head</w:t><w:t>tail</w:t></w:r>",
            )),
        ),
        marked(
            "X-lrpb",
            &tc(&p(
                "<w:r><w:rPr><w:i/></w:rPr><w:t>page text</w:t><w:lastRenderedPageBreak/></w:r>",
            )),
        ),
        marked(
            "X-comment",
            &tc(&p("<w:r><w:t>noted</w:t><!-- note --></w:r>")),
        ),
        marked(
            "X-linkonly",
            &tc(&p(&format!(
                "<w:hyperlink w:anchor=\"target\">{}</w:hyperlink>",
                rp("<w:u w:val=\"single\"/>", "link words")
            ))),
        ),
        marked(
            "X-emptyrun",
            &tc(&p(
                "<w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:t></w:t></w:r>",
            )),
        ),
    ];
    let mut body = pt("Paragraph before the tables.");
    for table in &tables {
        body.push_str(table);
        body.push_str(&p(""));
    }
    let styles = "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"CellFirst\"><w:name w:val=\"Cell First\"/>\
<w:rPr><w:b/></w:rPr></w:style><w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"CellSecond\">\
<w:name w:val=\"Cell Second\"/><w:rPr><w:i/></w:rPr></w:style>";
    vec![("p3-tables".to_string(), from_minimal(&body, styles, false))]
}

fn row_documents() -> Vec<(String, Vec<u8>)> {
    let uniform_run = |text: &str| {
        rp(
            "<w:highlight w:val=\"yellow\"/><w:lang w:val=\"en-US\"/>",
            text,
        )
    };
    let mut tables = vec![
        tbl(
            2,
            &[
                tr(&[tc(&pt("cell 00")), tc(&pt("cell 01"))]),
                tr(&[tc(&pt("cell 10")), tc(&pt("cell 11"))]),
            ],
        ),
        format!(
            "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"8640\"/>\
             </w:tblGrid><w:tr><w:trPr><w:trHeight w:val=\"576\"/></w:trPr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" \
             w:w=\"8640\"/><w:shd w:fill=\"D9EAF7\"/></w:tcPr><w:p><w:pPr><w:jc w:val=\"right\"/></w:pPr>{}{}</w:p>\
             </w:tc></w:tr></w:tbl>",
            uniform_run("uniform "),
            uniform_run("template")
        ),
        marked(
            "valid conflicting",
            &tc(&p(&format!(
                "{}{}",
                rp("<w:b/>", "first"),
                rp("<w:highlight w:val=\"yellow\"/>", "second")
            ))),
        ),
        tbl(
            2,
            &[tr(&[
                tc(&pt("plain template")),
                tc(&format!(
                    "<w:sdt><w:sdtPr><w:tag w:val=\"wrapped\"/><w:text/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>",
                    pt("template")
                )),
            ])],
        ),
        tbl(1, &[tr(&[tc(&p(&rp("<w:b/>", "bold cell")))])]),
        tbl(
            1,
            &[
                tr(&[tc(&p(&rp("<w:b/>", "")))]),
                tr(&[tc(&pt("styled empty above"))]),
            ],
        ),
        tbl(2, &[tr(&[tc(&pt("empty neighbour")), tc("<w:p/>")])]),
        tbl(1, &[tr(&[tc(&pt("T-only"))])]),
        tbl(
            2,
            &[
                tr(&[
                    tc(&pt("vm head")),
                    tc_pr("<w:vMerge w:val=\"restart\"/>", &pt("vm top")),
                ]),
                tr(&[tc(&pt("vm body")), tc_pr("<w:vMerge/>", "<w:p/>")]),
                tr(&[tc(&pt("vm tail")), tc(&pt("vm end"))]),
            ],
        ),
    ];
    // Complex later template cells (upstream `_make_complex_template_cell`).
    let control = |tag: &str, content: &str| {
        format!(
            "<w:sdt><w:sdtPr><w:tag w:val=\"{tag}\"/><w:text/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"
        )
    };
    let shapes: Vec<(&str, String)> = vec![
        (
            "multiple-paragraphs",
            tc(&format!("{}{}", pt("template"), pt("second"))),
        ),
        (
            "simple-field",
            tc(&p(&format!("<w:fldSimple>{}</w:fldSimple>", r("result")))),
        ),
        (
            "complex-field",
            tc(&p(
                "<w:r><w:fldChar w:fldCharType=\"begin\"/><w:t>template</w:t></w:r>",
            )),
        ),
        (
            "inline-control",
            tc(&p(&control("inline-template", &r("controlled")))),
        ),
        (
            "block-control",
            tc(&control("block-template", &pt("controlled"))),
        ),
        (
            "revision",
            tc(&p(&format!(
                "<w:ins w:id=\"81\" w:author=\"Reviewer\">{}</w:ins>",
                r("pending")
            ))),
        ),
        (
            "hyperlink",
            tc(&p(&format!("<w:hyperlink>{}</w:hyperlink>", r("linked")))),
        ),
        (
            "marker",
            tc(&p(&format!(
                "<w:bookmarkStart w:id=\"82\" w:name=\"template-marker\"/>{}",
                r("template")
            ))),
        ),
        (
            "drawing",
            tc(&p("<w:r><w:t>template</w:t><w:drawing/></w:r>")),
        ),
        (
            "nested-table",
            tc(&format!(
                "{}{}<w:p/>",
                pt("template"),
                tbl(1, &[tr(&[tc("<w:p/>")])])
            )),
        ),
        (
            "late-cell-properties",
            format!(
                "<w:tc>{}<w:tcPr><w:tcW w:type=\"dxa\" w:w=\"4320\"/></w:tcPr></w:tc>",
                pt("template")
            ),
        ),
        (
            "late-paragraph-properties",
            tc(&format!("<w:p>{}<w:pPr/></w:p>", r("template"))),
        ),
        (
            "unknown-wrapper",
            tc(&p(&format!("<w:customXml>{}</w:customXml>", r("wrapped")))),
        ),
    ];
    for (shape, cell) in shapes {
        tables.push(tbl(
            2,
            &[tr(&[tc(&pt(&format!("valid first {shape}"))), cell])],
        ));
    }
    // Tracked revision metadata (upstream `_append_revision_marker`).
    let marker = |tag: &str, snapshot: Option<&str>| {
        let inner = snapshot.map(|s| format!("<w:{s}/>")).unwrap_or_default();
        format!("<w:{tag} w:id=\"77\" w:author=\"Reviewer\">{inner}</w:{tag}>")
    };
    let markers: Vec<(&str, String)> = vec![
        (
            "trpr-ins",
            format!(
                "<w:tr><w:trPr>{}</w:trPr>{}</w:tr>",
                marker("ins", None),
                tc(&pt("template trpr-ins"))
            ),
        ),
        (
            "trpr-del",
            format!(
                "<w:tr><w:trPr>{}</w:trPr>{}</w:tr>",
                marker("del", None),
                tc(&pt("template trpr-del"))
            ),
        ),
        (
            "move-range",
            format!(
                "<w:tr>{}{}</w:tr>",
                tc(&pt("template move-range")),
                marker("moveFromRangeStart", None)
            ),
        ),
        (
            "row-properties",
            format!(
                "<w:tr><w:trPr>{}</w:trPr>{}</w:tr>",
                marker("trPrChange", Some("trPr")),
                tc(&pt("template row-properties"))
            ),
        ),
        (
            "table-property-exceptions",
            format!(
                "<w:tr><w:tblPrEx>{}</w:tblPrEx>{}</w:tr>",
                marker("tblPrExChange", Some("tblPrEx")),
                tc(&pt("template table-property-exceptions"))
            ),
        ),
        (
            "cell-insertion",
            tr(&[tc_pr(
                &marker("cellIns", None),
                &pt("template cell-insertion"),
            )]),
        ),
        (
            "cell-deletion",
            tr(&[tc_pr(
                &marker("cellDel", None),
                &pt("template cell-deletion"),
            )]),
        ),
        (
            "cell-merge",
            tr(&[tc_pr(
                &marker("cellMerge", None),
                &pt("template cell-merge"),
            )]),
        ),
        (
            "cell-properties",
            tr(&[tc_pr(
                &marker("tcPrChange", Some("tcPr")),
                &pt("template cell-properties"),
            )]),
        ),
        (
            "paragraph-properties",
            tr(&[tc(&format!(
                "<w:p><w:pPr>{}</w:pPr>{}</w:p>",
                marker("pPrChange", Some("pPr")),
                r("template paragraph-properties")
            ))]),
        ),
        (
            "run-properties",
            tr(&[tc(&p(&format!(
                "<w:r><w:rPr>{}</w:rPr><w:t>template run-properties</w:t></w:r>",
                marker("rPrChange", Some("rPr"))
            )))]),
        ),
        (
            "numbering-properties",
            tr(&[tc(&format!(
                "<w:p><w:pPr><w:numPr>{}</w:numPr></w:pPr>{}</w:p>",
                marker("numberingChange", None),
                r("template numbering-properties")
            ))]),
        ),
    ];
    for (_, row) in markers {
        tables.push(tbl(1, &[row]));
    }
    // Nonrectangular templates (gridBefore / gridAfter with omitted cells).
    for (name, property) in [("start", "gridBefore"), ("end", "gridAfter")] {
        tables.push(tbl(
            3,
            &[format!(
                "<w:tr><w:trPr><w:{property} w:val=\"1\"/></w:trPr>{}{}</w:tr>",
                tc(&pt(&format!("omitted {name} a"))),
                tc(&pt(&format!("omitted {name} b")))
            )],
        ));
    }
    // Omitted column without gridBefore/gridAfter.
    tables.push(tbl(
        3,
        &[tr(&[tc(&pt("short row a")), tc(&pt("short row b"))])],
    ));
    let mut body = pt("Row operations.");
    for table in &tables {
        body.push_str(table);
        body.push_str(&p(""));
    }
    vec![("p3-rows".to_string(), from_minimal(&body, "", false))]
}

fn styled(style: &str, text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>{}</w:p>",
        r(text)
    )
}

fn char_styled(style: &str, text: &str) -> String {
    rp(&format!("<w:rStyle w:val=\"{style}\"/>"), text)
}

fn para_style(id: &str, extra: &str) -> String {
    format!(
        "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"{id}\"><w:name w:val=\"{id}\"/>{extra}</w:style>"
    )
}

fn formatting_documents() -> Vec<(String, Vec<u8>)> {
    let styles = [
        para_style("ChainBase", "<w:rPr><w:sz w:val=\"32\"/></w:rPr>"),
        para_style("ChainLeaf", "<w:basedOn w:val=\"ChainBase\"/>"),
        para_style("SizedBase", "<w:rPr><w:sz w:val=\"32\"/></w:rPr>"),
        para_style(
            "SizedLeaf",
            "<w:basedOn w:val=\"SizedBase\"/><w:rPr><w:sz w:val=\"18\"/></w:rPr>",
        ),
        para_style("ColoredPara", "<w:rPr><w:sz w:val=\"30\"/></w:rPr>"),
        "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"Emph\"><w:name w:val=\"Emph\"/><w:rPr><w:sz w:val=\"16\"/></w:rPr></w:style>".to_string(),
        "<w:style w:type=\"paragraph\" w:styleId=\"CycleA\"><w:name w:val=\"Cycle A\"/><w:basedOn w:val=\"CycleB\"/></w:style>".to_string(),
        "<w:style w:type=\"paragraph\" w:styleId=\"CycleB\"><w:name w:val=\"Cycle B\"/><w:basedOn w:val=\"CycleA\"/><w:rPr><w:sz w:val=\"40\"/></w:rPr></w:style>".to_string(),
        para_style("BoldPara", "<w:rPr><w:b/></w:rPr>"),
        "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"BoldChar\"><w:name w:val=\"BoldChar\"/><w:rPr><w:b/></w:rPr></w:style>".to_string(),
        para_style("JustBold", "<w:rPr><w:b/></w:rPr>"),
        para_style("BoldBase", "<w:rPr><w:b/></w:rPr>"),
        para_style("LinkyPara", "<w:rPr><w:sz w:val=\"34\"/></w:rPr>"),
        para_style("LinkOnly", "<w:rPr><w:sz w:val=\"38\"/></w:rPr>"),
        "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"AlsoBold\"><w:name w:val=\"AlsoBold\"/><w:rPr><w:b/></w:rPr></w:style>".to_string(),
        para_style(
            "Aligned",
            "<w:pPr><w:jc w:val=\"both\"/></w:pPr><w:rPr><w:color w:val=\"FF0000\"/><w:u w:val=\"double\"/><w:rFonts w:ascii=\"Courier New\"/></w:rPr>",
        ),
        "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"Toggles\"><w:name w:val=\"Toggles\"/><w:basedOn w:val=\"ToggleRoot\"/><w:rPr><w:i/><w:caps w:val=\"0\"/><w:vanish/></w:rPr></w:style>".to_string(),
        "<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"ToggleRoot\"><w:name w:val=\"Toggle Root\"/><w:rPr><w:i/><w:smallCaps/><w:highlight w:val=\"green\"/><w:vertAlign w:val=\"superscript\"/></w:rPr></w:style>".to_string(),
        para_style("BadSize", "<w:rPr><w:sz w:val=\"big\"/></w:rPr>"),
        para_style("EmptyStyleRef", "<w:rPr><w:sz w:val=\"\"/></w:rPr>"),
    ]
    .concat();
    let body = [
        pt("plain run text"),
        p(&rp("<w:b/><w:sz w:val=\"28\"/>", "bolded direct")),
        styled("ChainLeaf", "chained text"),
        styled("SizedLeaf", "near wins"),
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"ColoredPara\"/></w:pPr>{}</w:p>",
            char_styled("Emph", "char styled")
        ),
        styled("CycleA", "cyclic text"),
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"BoldPara\"/></w:pPr>{}</w:p>",
            char_styled("BoldChar", "double bold cancels")
        ),
        styled("JustBold", "single layer"),
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"BoldBase\"/></w:pPr>{}</w:p>",
            rp("<w:b w:val=\"0\"/>", "explicitly unbold")
        ),
        format!("<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr>{}</w:p>", r("centered")),
        p(&format!("{}{}", rp("<w:b/>", "bold half"), r(" plain half"))),
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"LinkyPara\"/></w:pPr><w:hyperlink w:anchor=\"DefinedTerm\">{}</w:hyperlink></w:p>",
            r("linked text")
        ),
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"LinkOnly\"/></w:pPr>{}<w:hyperlink w:anchor=\"x\">{}</w:hyperlink></w:p>",
            rp("<w:b w:val=\"0\"/><w:sz w:val=\"16\"/>", "ordinary text first "),
            rp("<w:b/>", "only linked text here")
        ),
        p(&format!(
            "{}{}",
            rp("<w:b/>", "direct half"),
            char_styled("AlsoBold", " styled half")
        )),
        format!(
            "<w:p><w:pPr><w:spacing w:after=\"120\"/></w:pPr>{}</w:p>",
            rp("<w:shd w:fill=\"FFFF00\"/>", "formatted shading")
        ),
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"Aligned\"/></w:pPr>{}</w:p>",
            char_styled("Toggles", "toggle stack")
        ),
        styled("Heading1", "heading paragraph"),
        styled("MissingStyle", "unknown style ref"),
        styled("BadSize", "bad size value"),
        styled("EmptyStyleRef", "empty size value"),
        "<w:p><w:pPr><w:pStyle w:val=\"\"/></w:pPr><w:r><w:t>empty style id</w:t></w:r></w:p>".to_string(),
        p(&format!(
            "{}<w:r><w:tab/></w:r>{}",
            rp("<w:i/>", "tab left"),
            rp("<w:i/>", "tab right")
        )),
        p(&format!(
            "<w:ins w:id=\"3\" w:author=\"A\"><w:r><w:rPr><w:strike/></w:rPr><w:t>inserted run</w:t></w:r></w:ins>{}",
            r(" base run")
        )),
        p(&rp(
            "<w:rFonts w:asciiTheme=\"majorHAnsi\"/><w:color w:val=\"auto\" w:themeColor=\"accent1\"/><w:caps w:val=\"off\"/><w:emboss w:val=\"true\"/><w:imprint w:val=\"FALSE\"/>",
            "theme run",
        )),
        tbl(1, &[tr(&[tc(&pt("cell paragraph text"))])]),
    ]
    .concat();
    vec![
        (
            "p3-formatting".to_string(),
            from_minimal(&body, &styles, false),
        ),
        (
            "p3-no-styles".to_string(),
            minimal_package_with(&format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{}{}</w:body></w:document>",
                pt("no styles part here"),
                styled("Heading1", "unstyled heading")
            )),
        ),
    ]
}

const STY_HEAD: &str = "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<w:styles xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" mc:Ignorable=\"w14\">\n  <w:docDefaults>\n    <w:rPrDefault>\n      <w:rPr>\n        <w:rFonts w:asciiTheme=\"minorHAnsi\" w:eastAsiaTheme=\"minorEastAsia\" w:hAnsiTheme=\"minorHAnsi\" w:cstheme=\"minorBidi\"/>\n        <w:sz w:val=\"24\"/>\n        <w:szCs w:val=\"24\"/>\n        <w:lang w:val=\"en-US\" w:eastAsia=\"en-US\" w:bidi=\"ar-SA\"/>\n      </w:rPr>\n    </w:rPrDefault>\n    <w:pPrDefault/>\n  </w:docDefaults>\n";

const STY_BUILTINS: &str = "  <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\">\n    <w:name w:val=\"Normal\"/>\n    <w:qFormat/>\n  </w:style>\n  <w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\">\n    <w:name w:val=\"Default Paragraph Font\"/>\n    <w:uiPriority w:val=\"1\"/>\n    <w:semiHidden/>\n    <w:unhideWhenUsed/>\n  </w:style>\n  <w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\">\n    <w:name w:val=\"Normal Table\"/>\n    <w:uiPriority w:val=\"99\"/>\n    <w:semiHidden/>\n    <w:unhideWhenUsed/>\n    <w:tblPr>\n      <w:tblInd w:w=\"0\" w:type=\"dxa\"/>\n      <w:tblCellMar>\n        <w:top w:w=\"0\" w:type=\"dxa\"/>\n        <w:left w:w=\"108\" w:type=\"dxa\"/>\n        <w:bottom w:w=\"0\" w:type=\"dxa\"/>\n        <w:right w:w=\"108\" w:type=\"dxa\"/>\n      </w:tblCellMar>\n    </w:tblPr>\n  </w:style>\n  <w:style w:type=\"numbering\" w:default=\"1\" w:styleId=\"NoList\">\n    <w:name w:val=\"No List\"/>\n    <w:uiPriority w:val=\"99\"/>\n    <w:semiHidden/>\n    <w:unhideWhenUsed/>\n  </w:style>\n";

/// Drop the indentation between tags: python-docx parses with
/// `remove_blank_text`, so upstream `patch_save` would rewrite any
/// pretty-printed part it loaded, even unchanged.
fn compact(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut pending = String::new();
    for c in xml.chars() {
        if c.is_whitespace() {
            pending.push(c);
            continue;
        }
        if !(c == '<' && out.ends_with('>')) {
            out.push_str(&pending);
        }
        pending.clear();
        out.push(c);
    }
    out
}

fn style_documents() -> Vec<(String, Vec<u8>)> {
    let behav = format!(
        "{STY_HEAD}  <w:latentStyles w:defLockedState=\"0\" w:defUIPriority=\"99\" w:defSemiHidden=\"1\" w:defUnhideWhenUsed=\"1\" w:defQFormat=\"0\" w:count=\"276\"/>\n{STY_BUILTINS}\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Foo\">\n    <w:name w:val=\"Foo\"/>\n    <w:uiPriority w:val=\"42\"/>\n    <w:semiHidden/>\n    <w:unhideWhenUsed/>\n    <w:qFormat/>\n    <w:locked/>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Bar\">\n    <w:name w:val=\"Bar\"/>\n    <w:uiPriority w:val=\"24\"/>\n    <w:semiHidden w:val=\"0\"/>\n    <w:unhideWhenUsed w:val=\"0\"/>\n    <w:qFormat w:val=\"0\"/>\n    <w:locked w:val=\"0\"/>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Baz\">\n    <w:name w:val=\"Baz\"/>\n  </w:style>\n\n</w:styles>\n"
    );
    let known = format!(
        "{STY_HEAD}  <w:latentStyles w:defLockedState=\"0\" w:defUIPriority=\"99\" w:defSemiHidden=\"1\" w:defUnhideWhenUsed=\"1\" w:defQFormat=\"0\" w:count=\"276\">\n    <w:lsdException w:name=\"Normal\" w:semiHidden=\"1\" w:locked=\"1\" w:qFormat=\"1\" w:unhideWhenUsed=\"1\" w:uiPriority=\"42\"/>\n    <w:lsdException w:name=\"Title\" w:semiHidden=\"0\" w:locked=\"0\" w:qFormat=\"0\" w:unhideWhenUsed=\"0\" w:uiPriority=\"10\"/>\n    <w:lsdException w:name=\"Subtitle\"/>\n    <w:lsdException w:name=\"heading 1\" w:semiHidden=\"0\" w:uiPriority=\"9\" w:unhideWhenUsed=\"0\" w:qFormat=\"1\"/>\n    <w:lsdException w:name=\"heading 2\" w:uiPriority=\"9\" w:qFormat=\"1\"/>\n    <w:lsdException w:name=\"caption\" w:uiPriority=\"35\" w:qFormat=\"1\"/>\n  </w:latentStyles>\n{STY_BUILTINS}\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Base\">\n    <w:name w:val=\"Base\"/>\n    <w:qFormat/>\n    <w:rsid w:val=\"00914546\"/>\n    <w:pPr>\n      <w:keepNext/>\n      <w:jc w:val=\"center\"/>\n    </w:pPr>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Citation\">\n    <w:name w:val=\"Citation\"/>\n    <w:basedOn w:val=\"Base\"/>\n    <w:next w:val=\"SubNormal\"/>\n    <w:qFormat/>\n    <w:pPr>\n      <w:jc w:val=\"right\"/>\n    </w:pPr>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"SubNormal\">\n    <w:name w:val=\"Sub Normal\"/>\n    <w:basedOn w:val=\"Normal\"/>\n    <w:next w:val=\"Foobar\"/>\n    <w:qFormat/>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"Foo\">\n    <w:name w:val=\"Foo\"/>\n    <w:next w:val=\"Base\"/>\n  </w:style>\n\n  <w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"Quote\">\n    <w:name w:val=\"Quote Char\"/>\n    <w:basedOn w:val=\"DefaultParagraphFont\"/>\n    <w:next w:val=\"Base\"/>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:styleId=\"Heading1\">\n    <w:name w:val=\"heading 1\"/>\n    <w:basedOn w:val=\"Missing\"/>\n    <w:next w:val=\"Quote\"/>\n    <w:uiPriority w:val=\" 9 \"/>\n  </w:style>\n\n  <w:style w:type=\"table\" w:customStyle=\"0\" w:styleId=\"Grid\">\n    <w:name w:val=\"Table Grid\"/>\n    <w:basedOn w:val=\"TableNormal\"/>\n  </w:style>\n\n  <w:style w:type=\"paragraph\" w:default=\"1\" w:customStyle=\"1\" w:styleId=\"SecondDefault\">\n    <w:name w:val=\"Second Default\"/>\n  </w:style>\n\n  <w:style w:type=\"numbering\" w:customStyle=\"1\" w:styleId=\"Bullets\">\n    <w:name w:val=\"Bullets\"/>\n    <w:basedOn w:val=\"NoList\"/>\n  </w:style>\n\n</w:styles>\n"
    );
    let body = format!("{}{}", pt("Styles document."), styled("Citation", "cited"));
    vec![
        (
            "p3-sty-behav-props".to_string(),
            from_minimal(&body, &compact(&behav), true),
        ),
        (
            "p3-sty-known-styles".to_string(),
            from_minimal(&body, &compact(&known), true),
        ),
    ]
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    let mut out = table_documents();
    out.extend(row_documents());
    out.extend(formatting_documents());
    out.extend(style_documents());
    out
}
