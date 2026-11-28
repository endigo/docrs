//! The glyphs a PDF's pages draw, followed through `Tm`, `Tf`, `Tz` and
//! `TJ` and read back through each font's codes, widths and `ToUnicode`,
//! and the file checked with qpdf.

use std::collections::BTreeMap;
use std::process::Command;

use docrs::Document;
use docrs::layout::{FontSet, LayoutTarget, LineRecord, RunRecord};
use docrs::txn::Txn;

use crate::reader::{Dict, Pdf, Value, tokenize_content};

/// Whether a resource name is one of the built-in symbol fonts.
pub fn built_in(font: &str) -> bool {
    font.starts_with('S')
}

/// One glyph as the reader sees it: its text, its origin in points from
/// the page's top-left corner, and the resource name of its font.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub font: String,
}

/// A font as the reader uses it.
pub struct Font {
    pub subtype: String,
    /// Bytes per code: 2 for `Type0`, else 1.
    pub bytes: usize,
    pub texts: BTreeMap<u16, String>,
    /// In thousandths of the font size.
    pub widths: BTreeMap<u16, f64>,
}

/// Every glyph each page draws.
pub fn glyphs(pdf: &Pdf) -> Vec<Vec<Glyph>> {
    let heights: Vec<f64> = pdf.media_boxes().unwrap().iter().map(|b| b[3]).collect();
    pdf.pages()
        .unwrap()
        .into_iter()
        .zip(heights)
        .map(|(page, height)| {
            let fonts = page_fonts(pdf, page);
            let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
            let mut out = Vec::new();
            let (mut font, mut size, mut scale) = (None, 0.0, 1.0);
            let (mut origin, mut pen) = ((0.0, 0.0), 0.0);
            for op in tokenize_content(&content).unwrap() {
                let number = |at: usize| op.operands[at].number().unwrap();
                match op.operator.as_str() {
                    "Tf" => {
                        let Value::Name(name) = &op.operands[0] else {
                            panic!("Tf without a name");
                        };
                        font = Some((name.clone(), &fonts[name]));
                        size = number(1);
                    }
                    "Tz" => scale = number(0) / 100.0,
                    "Tm" => {
                        assert_eq!((number(0), number(1)), (1.0, 0.0));
                        origin = (number(4), number(5));
                        pen = 0.0;
                    }
                    "TJ" => {
                        let Value::Array(items) = &op.operands[0] else {
                            panic!("TJ without an array");
                        };
                        let (name, font): &(String, &Font) = font.as_ref().unwrap();
                        for item in items {
                            match item {
                                Value::Str(bytes) => {
                                    for code in bytes.chunks(font.bytes) {
                                        let code =
                                            code.iter().fold(0, |c, b| c << 8 | u16::from(*b));
                                        out.push(Glyph {
                                            text: font.texts[&code].clone(),
                                            x: origin.0 + pen,
                                            y: height - origin.1,
                                            font: name.clone(),
                                        });
                                        pen += font.widths[&code] / 1000.0 * size * scale;
                                    }
                                }
                                other => pen -= other.number().unwrap() / 1000.0 * size * scale,
                            }
                        }
                    }
                    _ => {}
                }
            }
            out
        })
        .collect()
}

pub fn page_fonts(pdf: &Pdf, page: &Dict) -> BTreeMap<String, Font> {
    let resources = pdf.page_attr(page, "Resources").unwrap();
    let fonts = pdf.get(pdf.resolve(resources).dict().unwrap(), "Font");
    pdf.resolve(fonts)
        .dict()
        .unwrap()
        .iter()
        .map(|(name, font)| {
            (
                name.clone(),
                font_of(pdf, pdf.resolve(font).dict().unwrap()),
            )
        })
        .collect()
}

fn font_of(pdf: &Pdf, font: &Dict) -> Font {
    let Value::Name(subtype) = pdf.get(font, "Subtype") else {
        panic!("a font without a subtype");
    };
    let bytes = if subtype == "Type0" { 2 } else { 1 };
    let widths = if subtype == "Type0" {
        cid_widths(pdf, font)
    } else {
        simple_widths(pdf, font)
    };
    let texts = match pdf.get(font, "ToUnicode") {
        Value::Null => widths
            .keys()
            .map(|code| (*code, char::from(*code as u8).to_string()))
            .collect(),
        _ => to_unicode(pdf, font, bytes),
    };
    Font {
        subtype: subtype.clone(),
        bytes,
        texts,
        widths,
    }
}

pub fn to_unicode(pdf: &Pdf, font: &Dict, bytes: usize) -> BTreeMap<u16, String> {
    let cmap = String::from_utf8(pdf.stream_data(pdf.get(font, "ToUnicode")).unwrap()).unwrap();
    let hex = |token: &str| -> Vec<u8> {
        let digits = token.trim_matches(['<', '>']);
        (0..digits.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&digits[at..at + 2], 16).unwrap())
            .collect()
    };
    let mut out = BTreeMap::new();
    for block in cmap.split("beginbfchar").skip(1) {
        let block = block.split("endbfchar").next().unwrap();
        for entry in block.lines().filter(|l| !l.trim().is_empty()) {
            let (code, text) = entry.split_once(' ').unwrap();
            let code = hex(code);
            assert_eq!(code.len(), bytes, "{entry}");
            let code = code.iter().fold(0, |c, b| c << 8 | u16::from(*b));
            let units: Vec<u16> = hex(text)
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            out.insert(code, String::from_utf16(&units).unwrap());
        }
    }
    out
}

/// `/W` of a `Type0` font's descendant: `c [w1 w2 …]` runs.
fn cid_widths(pdf: &Pdf, font: &Dict) -> BTreeMap<u16, f64> {
    let Value::Array(descendants) = pdf.get(font, "DescendantFonts") else {
        panic!("no DescendantFonts");
    };
    let cid_font = pdf.resolve(&descendants[0]).dict().unwrap();
    let Value::Array(w) = pdf.resolve(pdf.get(cid_font, "W")) else {
        panic!("no /W");
    };
    let mut out = BTreeMap::new();
    for pair in w.chunks(2) {
        let first = pair[0].number().unwrap() as u16;
        let Value::Array(run) = &pair[1] else {
            panic!("only c [w …] runs are written");
        };
        for (at, width) in run.iter().enumerate() {
            out.insert(first + at as u16, width.number().unwrap());
        }
    }
    out
}

/// `/Widths` from `/FirstChar`, scaled by a `Type3` font's `/FontMatrix`.
fn simple_widths(pdf: &Pdf, font: &Dict) -> BTreeMap<u16, f64> {
    let first = pdf.get(font, "FirstChar").number().unwrap() as u16;
    let Value::Array(widths) = pdf.get(font, "Widths") else {
        panic!("no /Widths");
    };
    let scale = match pdf.get(font, "FontMatrix") {
        Value::Array(matrix) => matrix[0].number().unwrap() * 1000.0,
        _ => 1.0,
    };
    widths
        .iter()
        .enumerate()
        .map(|(at, width)| (first + at as u16, width.number().unwrap() * scale))
        .collect()
}

/// Every line layout records for `document`'s body, and each look's rise.
pub fn records(document: &Document, fonts: &FontSet) -> (Vec<LineRecord>, Vec<f64>) {
    records_for(document, fonts, LayoutTarget::Word)
}

/// [`records`], laid out for `target`.
pub fn records_for(
    document: &Document,
    fonts: &FontSet,
    target: LayoutTarget,
) -> (Vec<LineRecord>, Vec<f64>) {
    let txn = Txn::new(document);
    let main = document.package().main_part_name().to_string();
    let (mut out, mut rises) = (Vec::new(), Vec::new());
    docrs::layout::records_for(
        &txn,
        &main,
        fonts,
        target,
        |looks, _| {
            rises = looks.iter().map(|look| look.rise).collect();
            Ok(())
        },
        |record| {
            if let docrs::layout::Record::Line(line) = record {
                out.push(line);
            }
            Ok(())
        },
    )
    .unwrap();
    (out, rises)
}

/// The glyphs each page should draw, as layout records them: each line's
/// label, then its characters, each tab replaced by its leader, on its
/// baseline raised by the run's rise.
pub fn expected((records, rises): &(Vec<LineRecord>, Vec<f64>)) -> Vec<Vec<Glyph>> {
    let pages = records.iter().map(|line| line.page).max().unwrap_or(0) as usize;
    let mut out = vec![Vec::new(); pages];
    for line in records {
        let page = &mut out[line.page as usize - 1];
        let mut leaders = line.leaders.iter();
        let mut push = |run: &RunRecord, ch: char, x: f64| {
            page.push(Glyph {
                text: ch.to_string(),
                x,
                y: line.baseline - rises[run.look as usize],
                font: String::new(),
            })
        };
        for run in line.label.iter().chain(&line.runs) {
            for (ch, x) in run.text.chars().zip(&run.origins) {
                if ch != '\t' {
                    push(run, ch, *x);
                } else if let Some(Some(leader)) = leaders.next() {
                    for (ch, x) in leader.text.chars().zip(&leader.origins) {
                        push(leader, ch, *x);
                    }
                }
            }
        }
    }
    out
}

/// Each glyph at layout's place with layout's text, except a symbol the
/// built-in font draws, which extracts as what it stands for.
pub fn assert_same_places(name: &str, drawn: &[Vec<Glyph>], expected: &[Vec<Glyph>]) {
    assert_same_positions(name, drawn, expected);
    for (glyph, want) in drawn.iter().flatten().zip(expected.iter().flatten()) {
        assert!(
            glyph.text == want.text || built_in(&glyph.font),
            "{name}: drew {glyph:?}, layout placed {want:?}"
        );
    }
}

/// Each glyph at layout's place, within 0.001 pt.
pub fn assert_same_positions(name: &str, drawn: &[Vec<Glyph>], expected: &[Vec<Glyph>]) {
    assert_eq!(drawn.len(), expected.len(), "{name}: pages");
    for (page, (drawn, expected)) in drawn.iter().zip(expected).enumerate() {
        assert_eq!(drawn.len(), expected.len(), "{name} page {}", page + 1);
        for (glyph, want) in drawn.iter().zip(expected) {
            let close = (glyph.x - want.x).abs() < 0.001 && (glyph.y - want.y).abs() < 0.001;
            assert!(
                close,
                "{name} page {}: drew {glyph:?}, layout placed {want:?}",
                page + 1
            );
        }
    }
}

/// `qpdf --check` passes; skipped where qpdf is missing, except in CI.
pub fn qpdf_check(name: &str, bytes: &[u8]) {
    let found = Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        let required = std::env::var_os("CI").is_some() && !cfg!(windows);
        assert!(!required, "qpdf must be installed in CI");
        eprintln!("SKIPPED: qpdf is not installed; `brew install qpdf` to run this check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{name}: {report}");
    assert!(
        report.contains("No syntax or stream encoding errors found"),
        "{name}: {report}"
    );
}
