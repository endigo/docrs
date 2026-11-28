//! Embedded fonts, read back by the test reader: subsets, instances,
//! permissions and text extraction.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use docrs::Limits;
use docrs::pdf::{Info, SampleLine, Style, glyphs_for_tests, text_document_for_tests};
use reader::{Dict, Pdf, Value, tokenize_content};

const PRODUCER: &str = "docrs-test";
const BOLD: Style = Style {
    weight: 700,
    italic: false,
};

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(path)).unwrap()
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn inter() -> Vec<u8> {
    fixture("fonts/inter/Inter[opsz,wght].ttf")
}

fn liberation() -> Vec<u8> {
    fixture("fonts/liberation/LiberationSerif-Regular.ttf")
}

fn source_sans() -> Vec<u8> {
    fixture("fonts/source-sans/SourceSans3-Regular.otf")
}

fn line<'a>(font: &'a [u8], style: Style, text: &str) -> SampleLine<'a> {
    in_face(font, 0, style, text)
}

fn in_face<'a>(font: &'a [u8], index: u32, style: Style, text: &str) -> SampleLine<'a> {
    SampleLine {
        font,
        index,
        style,
        size: 24.0,
        glyphs: glyphs_for_tests(font, index, text).unwrap(),
    }
}

fn document(lines: &[SampleLine]) -> docrs::Result<Vec<u8>> {
    let info = Info {
        title: Some("Embedded fonts".into()),
        ..Info::default()
    };
    text_document_for_tests(lines, &info, PRODUCER, &Limits::default())
}

/// The fonts of the single page, by resource name.
fn fonts(pdf: &Pdf) -> BTreeMap<String, Dict> {
    let page = pdf.pages().unwrap()[0];
    let resources = pdf.page_attr(page, "Resources").unwrap();
    let fonts = pdf.get(pdf.resolve(resources).dict().unwrap(), "Font");
    pdf.resolve(fonts)
        .dict()
        .unwrap()
        .iter()
        .map(|(name, font)| (name.clone(), pdf.resolve(font).dict().unwrap().clone()))
        .collect()
}

fn descendant<'a>(pdf: &'a Pdf, font: &'a Dict) -> &'a Dict {
    let Value::Array(descendants) = pdf.get(font, "DescendantFonts") else {
        panic!("no DescendantFonts");
    };
    pdf.resolve(&descendants[0]).dict().unwrap()
}

fn descriptor<'a>(pdf: &'a Pdf, font: &'a Dict) -> &'a Dict {
    pdf.get(descendant(pdf, font), "FontDescriptor")
        .dict()
        .unwrap()
}

fn name_of<'a>(pdf: &'a Pdf, dict: &'a Dict, key: &str) -> &'a str {
    match pdf.get(dict, key) {
        Value::Name(name) => name,
        other => panic!("/{key} is {other:?}"),
    }
}

/// The embedded program and the key it is under.
fn program(pdf: &Pdf, font: &Dict) -> (String, Vec<u8>) {
    let descriptor = descriptor(pdf, font);
    let key = ["FontFile2", "FontFile3"]
        .into_iter()
        .find(|key| descriptor.contains_key(*key))
        .unwrap();
    (key.into(), pdf.stream_data(&descriptor[key]).unwrap())
}

/// CID to text, from a `ToUnicode` CMap's `bfchar` entries.
fn to_unicode(pdf: &Pdf, font: &Dict) -> BTreeMap<u16, String> {
    let cmap = pdf.stream_data(pdf.get(font, "ToUnicode")).unwrap();
    let cmap = String::from_utf8(cmap).unwrap();
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
        for entry in block.lines().skip(1).filter(|l| !l.is_empty()) {
            let (cid, text) = entry.split_once(' ').unwrap();
            let cid = u16::from_be_bytes(hex(cid).try_into().unwrap());
            let units: Vec<u16> = hex(text)
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            out.insert(cid, String::from_utf16(&units).unwrap());
        }
    }
    out
}

/// A `TJ` line: its font's resource name, its text read through the font's
/// `ToUnicode`, and the CIDs it draws.
struct Line {
    font: String,
    text: String,
    cids: Vec<u16>,
}

fn extract(pdf: &Pdf) -> Vec<Line> {
    let fonts = fonts(pdf);
    let page = pdf.pages().unwrap()[0];
    let content = pdf.stream_data(pdf.get(page, "Contents")).unwrap();
    let mut font = None;
    let mut out = Vec::new();
    for op in tokenize_content(&content).unwrap() {
        match op.operator.as_str() {
            "Tf" => {
                let Value::Name(name) = &op.operands[0] else {
                    panic!("Tf without a name");
                };
                font = Some((name.clone(), to_unicode(pdf, &fonts[name])));
            }
            "TJ" => {
                let Value::Array(items) = &op.operands[0] else {
                    panic!("TJ without an array");
                };
                let (name, map) = font.as_ref().unwrap();
                let cids: Vec<u16> = items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Str(bytes) => Some(bytes),
                        _ => None,
                    })
                    .flat_map(|bytes| bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])))
                    .collect();
                let text = cids.iter().map(|cid| map[cid].as_str()).collect();
                out.push(Line {
                    font: name.clone(),
                    text,
                    cids,
                });
            }
            _ => {}
        }
    }
    out
}

fn texts(pdf: &Pdf) -> Vec<String> {
    extract(pdf).into_iter().map(|line| line.text).collect()
}

fn be16(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn be32(data: &[u8], at: usize) -> usize {
    u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize
}

/// The tables of face `index` of a font file, by tag.
fn tables(font: &[u8], index: usize) -> BTreeMap<String, Vec<u8>> {
    let view = if &font[..4] == b"ttcf" {
        be32(font, 12 + 4 * index)
    } else {
        0
    };
    (0..usize::from(be16(font, view + 4)))
        .map(|table| {
            let record = view + 12 + 16 * table;
            let tag = String::from_utf8(font[record..record + 4].to_vec()).unwrap();
            let (start, length) = (be32(font, record + 8), be32(font, record + 12));
            (tag, font[start..start + length].to_vec())
        })
        .collect()
}

fn glyph_count(tables: &BTreeMap<String, Vec<u8>>) -> u16 {
    be16(&tables["maxp"], 4)
}

fn advance(tables: &BTreeMap<String, Vec<u8>>, glyph: u16) -> u16 {
    let metrics = be16(&tables["hhea"], 34);
    be16(&tables["hmtx"], 4 * usize::from(glyph.min(metrics - 1)))
}

/// The glyphs `glyphs` are built from, themselves included, and `.notdef`.
fn closure(tables: &BTreeMap<String, Vec<u8>>, glyphs: &[u16]) -> BTreeSet<u16> {
    let long = be16(&tables["head"], 50) == 1;
    let loca = |glyph: u16| {
        let at = usize::from(glyph);
        if long {
            be32(&tables["loca"], 4 * at)
        } else {
            2 * usize::from(be16(&tables["loca"], 2 * at))
        }
    };
    let glyf = &tables["glyf"];
    let mut seen = BTreeSet::from([0]);
    let mut todo = glyphs.to_vec();
    while let Some(glyph) = todo.pop() {
        if !seen.insert(glyph) {
            continue;
        }
        let (start, end) = (loca(glyph), loca(glyph + 1));
        if end == start || (be16(glyf, start) as i16) >= 0 {
            continue;
        }
        let mut at = start + 10;
        loop {
            let flags = be16(glyf, at);
            todo.push(be16(glyf, at + 2));
            let args = if flags & 1 != 0 { 4 } else { 2 };
            let scale = match flags {
                f if f & 0x08 != 0 => 2,
                f if f & 0x40 != 0 => 4,
                f if f & 0x80 != 0 => 8,
                _ => 0,
            };
            at += 4 + args + scale;
            if flags & 0x20 == 0 {
                break;
            }
        }
    }
    seen
}

/// The directory record of table `tag` in a font file (not a collection).
fn record(font: &[u8], tag: &[u8; 4]) -> usize {
    (0..usize::from(be16(font, 4)))
        .map(|table| 12 + 16 * table)
        .find(|record| &font[*record..record + 4] == tag)
        .unwrap()
}

/// A copy of `font` with `fsType` (OS/2 offset 8) set.
fn with_fs_type(font: &[u8], fs_type: u16) -> Vec<u8> {
    let mut font = font.to_vec();
    let os2 = be32(&font, record(&font, b"OS/2") + 8);
    font[os2 + 8..os2 + 10].copy_from_slice(&fs_type.to_be_bytes());
    font
}

/// A font file of `tables`, their checksums left 0.
fn sfnt(version: &[u8; 4], tables: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = version.to_vec();
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([0; 6]);
    let mut offset = 12 + 16 * tables.len();
    for (tag, table) in tables {
        out.extend(tag.as_bytes());
        out.extend([0; 4]);
        out.extend((offset as u32).to_be_bytes());
        out.extend((table.len() as u32).to_be_bytes());
        offset += table.len().next_multiple_of(4);
    }
    for table in tables.values() {
        out.extend(table);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

/// The OpenType checksum: big-endian words summed, the last zero-padded.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0_u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// A collection of whole font files, their tables shared by none.
fn collection(faces: &[&[u8]]) -> Vec<u8> {
    let header = 12 + 4 * faces.len();
    let mut out = b"ttcf\x00\x01\x00\x00".to_vec();
    out.extend((faces.len() as u32).to_be_bytes());
    let mut body = Vec::new();
    for face in faces {
        out.extend(((header + body.len()) as u32).to_be_bytes());
        let base = header + body.len();
        let mut face = face.to_vec();
        for table in 0..usize::from(be16(&face, 4)) {
            let record = 12 + 16 * table + 8;
            let moved = (be32(&face, record) + base) as u32;
            face[record..record + 4].copy_from_slice(&moved.to_be_bytes());
        }
        body.extend(face);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    out.extend(body);
    out
}

fn qpdf_check(name: &str, bytes: &[u8]) {
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

const SAMPLE: &str = "Office fi \u{1F130} \u{e9}t\u{e9} AVA";

#[test]
fn inter_instances_and_a_static_font_extract_as_written() {
    let (inter, liberation) = (inter(), liberation());
    let bytes = document(&[
        line(&inter, Style::REGULAR, SAMPLE),
        line(&inter, BOLD, SAMPLE),
        line(&liberation, Style::REGULAR, "Office fi \u{e9}t\u{e9} AVA"),
    ])
    .unwrap();
    qpdf_check("inter.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(texts(&pdf), [SAMPLE, SAMPLE, "Office fi \u{e9}t\u{e9} AVA"]);
    let fonts = fonts(&pdf);
    let base = |name: &str| name_of(&pdf, &fonts[name], "BaseFont").to_string();
    let (regular, bold, serif) = (base("F1"), base("F2"), base("F3"));
    assert_eq!(&regular[6..], "+Inter-Regular");
    assert_eq!(&bold[6..], "+Inter-Bold");
    assert_eq!(&serif[6..], "+LiberationSerif");
    for name in [&regular, &bold, &serif] {
        assert!(name[..6].bytes().all(|b| b.is_ascii_uppercase()), "{name}");
    }
    assert_ne!(regular[..6], bold[..6]);
    let weight = |name: &str| {
        pdf.get(descriptor(&pdf, &fonts[name]), "FontWeight")
            .number()
    };
    assert_eq!((weight("F1"), weight("F2")), (Some(400.0), Some(700.0)));
    let widths = |name: &str| pdf.get(descendant(&pdf, &fonts[name]), "W").clone();
    assert_ne!(
        widths("F1"),
        widths("F2"),
        "the bold instance has its own advances"
    );
}

#[test]
fn a_subset_holds_the_used_glyphs_and_their_components() {
    let source = liberation();
    let glyphs = glyphs_for_tests(&source, 0, "Aé").unwrap();
    let bytes = document(&[line(&source, Style::REGULAR, "Aé")]).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    let fonts = fonts(&pdf);
    let font = &fonts["F1"];
    let (key, subset) = program(&pdf, font);
    assert_eq!(key, "FontFile2");
    let (source_tables, subset_tables) = (tables(&source, 0), tables(&subset, 0));
    let used: Vec<u16> = glyphs.iter().map(|(glyph, _)| *glyph).collect();
    let expected = closure(&source_tables, &used);
    assert_eq!(expected.len(), 5, ".notdef, A, eacute, e, acute");
    assert_eq!(usize::from(glyph_count(&subset_tables)), expected.len());
    assert!(!subset_tables.contains_key("cmap"));
    let map = pdf
        .stream_data(pdf.get(descendant(&pdf, font), "CIDToGIDMap"))
        .unwrap();
    for ((source_glyph, _), cid) in glyphs.iter().zip(&extract(&pdf)[0].cids) {
        let at = 2 * usize::from(*cid);
        let subset_glyph = be16(&map, at);
        assert_ne!(subset_glyph, 0);
        assert_eq!(
            advance(&subset_tables, subset_glyph),
            advance(&source_tables, *source_glyph)
        );
    }
    let length = pdf.get(descriptor(&pdf, font), "FontFile2");
    let (dict, _) = pdf.raw_stream(length).unwrap();
    assert_eq!(pdf.get(dict, "Length1").number(), Some(subset.len() as f64));
}

#[test]
fn a_space_and_a_no_break_space_sharing_a_glyph_extract_apart() {
    let source = liberation();
    let mut sample = line(&source, Style::REGULAR, "a b\u{a0}c");
    let space = sample.glyphs[1].0;
    sample.glyphs[3].0 = space;
    let bytes = document(&[sample]).unwrap();
    qpdf_check("nbsp.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    let Line { text, cids, .. } = &extract(&pdf)[0];
    assert_eq!(text, "a b\u{a0}c");
    assert_ne!(cids[1], cids[3]);
    let fonts = fonts(&pdf);
    let map = pdf
        .stream_data(pdf.get(descendant(&pdf, &fonts["F1"]), "CIDToGIDMap"))
        .unwrap();
    let glyph = |cid: u16| be16(&map, 2 * usize::from(cid));
    assert_eq!(glyph(cids[1]), glyph(cids[3]));
}

#[test]
fn a_cff_font_embeds_as_opentype() {
    let source = source_sans();
    let text = "Office fi \u{e9}t\u{e9} AVA";
    let bytes = document(&[line(&source, Style::REGULAR, text)]).unwrap();
    qpdf_check("cff.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(texts(&pdf), [text]);
    let fonts = fonts(&pdf);
    let font = &fonts["F1"];
    assert_eq!(
        name_of(&pdf, descendant(&pdf, font), "Subtype"),
        "CIDFontType0"
    );
    assert!(!descendant(&pdf, font).contains_key("CIDToGIDMap"));
    let cid_font = name_of(&pdf, descendant(&pdf, font), "BaseFont");
    assert!(cid_font.ends_with("+SourceSans3-Regular"), "{cid_font}");
    assert_eq!(
        name_of(&pdf, font, "BaseFont"),
        format!("{cid_font}-Identity-H")
    );
    let (key, subset) = program(&pdf, font);
    assert_eq!(key, "FontFile3");
    let file = pdf.get(descriptor(&pdf, font), "FontFile3");
    let (dict, _) = pdf.raw_stream(file).unwrap();
    assert_eq!(pdf.get(dict, "Subtype"), &Value::Name("OpenType".into()));
    let subset_tables = tables(&subset, 0);
    assert!(subset_tables.contains_key("CFF "));
    let distinct: BTreeSet<char> = text.chars().collect();
    assert_eq!(usize::from(glyph_count(&subset_tables)), distinct.len() + 1);
}

#[test]
fn restricted_and_bitmap_only_fonts_are_refused() {
    for fs_type in [0x0002, 0x0202, 0x0200] {
        let font = with_fs_type(&liberation(), fs_type);
        let err = document(&[line(&font, Style::REGULAR, "a")]).unwrap_err();
        assert_eq!(
            err.reason(),
            "pdf_font_embedding_restricted",
            "{fs_type:#06x}"
        );
        assert!(
            err.message().contains("Liberation Serif Regular"),
            "{}",
            err.message()
        );
    }
    for fs_type in [0x0004, 0x0008, 0x000A] {
        let font = with_fs_type(&liberation(), fs_type);
        document(&[line(&font, Style::REGULAR, "a")]).unwrap();
    }
}

#[test]
fn a_face_that_may_not_be_subset_is_embedded_whole() {
    let whole = with_fs_type(&liberation(), 0x0100);
    let bytes = document(&[line(&whole, Style::REGULAR, "Whole")]).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(texts(&pdf), ["Whole"]);
    assert_eq!(program(&pdf, &fonts(&pdf)["F1"]).1, whole);
    assert_eq!(
        name_of(&pdf, &fonts(&pdf)["F1"], "BaseFont"),
        "LiberationSerif",
        "a whole font is not a subset, so it has no tag"
    );

    let file = collection(&[&source_sans(), &whole]);
    let bytes = document(&[in_face(&file, 1, Style::REGULAR, "Whole")]).unwrap();
    qpdf_check("collection.pdf", &bytes);
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(texts(&pdf), ["Whole"]);
    let fonts = fonts(&pdf);
    let (_, embedded) = program(&pdf, &fonts["F1"]);
    assert_eq!(&embedded[..4], b"\x00\x01\x00\x00");
    assert_eq!(name_of(&pdf, &fonts["F1"], "BaseFont"), "LiberationSerif");
    let (mut ours, mut theirs) = (tables(&embedded, 0), tables(&whole, 0));
    let head = ours.get_mut("head").unwrap();
    assert_eq!(checksum(&embedded), 0xB1B0_AFBA);
    head[8..12].fill(0);
    let head_record = record(&embedded, b"head");
    assert_eq!(be32(&embedded, head_record + 4) as u32, checksum(head));
    theirs.get_mut("head").unwrap()[8..12].fill(0);
    assert_eq!(ours, theirs);
    let map = pdf
        .stream_data(pdf.get(descendant(&pdf, &fonts["F1"]), "CIDToGIDMap"))
        .unwrap();
    let glyphs = glyphs_for_tests(&whole, 0, "Whole").unwrap();
    for ((glyph, _), cid) in glyphs.iter().zip(&extract(&pdf)[0].cids) {
        assert_eq!(be16(&map, 2 * usize::from(*cid)), *glyph);
    }
}

#[test]
fn a_variable_face_that_may_not_be_subset_keeps_every_glyph() {
    let source = with_fs_type(&inter(), 0x0100);
    let bytes = document(&[line(&source, BOLD, "Bold")]).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(texts(&pdf), ["Bold"]);
    let (_, embedded) = program(&pdf, &fonts(&pdf)["F1"]);
    let embedded = tables(&embedded, 0);
    let source = tables(&source, 0);
    assert_eq!(glyph_count(&embedded), glyph_count(&source));
    assert!(!embedded.contains_key("fvar"));
}

#[test]
fn a_face_is_one_resource_per_instance() {
    let inter = inter();
    let copy = inter.clone();
    let bytes = document(&[
        line(&inter, Style::REGULAR, "a"),
        line(
            &copy,
            Style {
                weight: 400,
                italic: true,
            },
            "b",
        ),
        line(&inter, BOLD, "c"),
        line(
            &inter,
            Style {
                weight: 900,
                italic: false,
            },
            "d",
        ),
        line(
            &copy,
            Style {
                weight: 1000,
                italic: false,
            },
            "e",
        ),
    ])
    .unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(fonts(&pdf).keys().collect::<Vec<_>>(), ["F1", "F2", "F3"]);
    let lines: Vec<String> = extract(&pdf).into_iter().map(|line| line.font).collect();
    assert_eq!(lines, ["F1", "F1", "F2", "F3", "F3"]);
}

#[test]
fn a_glyph_the_face_lacks_is_refused() {
    let source = liberation();
    let mut sample = line(&source, Style::REGULAR, "a");
    sample.glyphs[0].0 = u16::MAX;
    let err = document(&[sample]).unwrap_err();
    assert_eq!(err.reason(), "pdf_glyph_out_of_range");
}

/// After an intended change to the output, regenerate the file with
/// `DOCRS_BLESS=1 cargo test -p docrs --test pdf_fonts output_matches_the_golden_file`,
/// then run `qpdf --check` on it and review the diff.
#[test]
fn output_matches_the_golden_file() {
    let source = liberation();
    let make = || document(&[line(&source, Style::REGULAR, "Hello, w\u{f6}rld")]).unwrap();
    let bytes = make();
    assert_eq!(bytes, make());
    let path = fixtures().join("pdf/font-line.pdf");
    if std::env::var_os("DOCRS_BLESS").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    assert!(
        std::fs::read(&path).unwrap() == bytes,
        "differs from {}",
        path.display()
    );
}

#[test]
fn a_variation_axis_whose_range_misses_its_default_is_ignored() {
    let mut font = inter();
    let fvar = be32(&font, record(&font, b"fvar") + 8);
    let axes = fvar + usize::from(be16(&font, fvar + 4));
    let wght = (0..usize::from(be16(&font, fvar + 8)))
        .map(|axis| axes + 20 * axis)
        .find(|at| &font[*at..at + 4] == b"wght")
        .unwrap();
    font[wght + 4..wght + 8].copy_from_slice(&(950_i32 << 16).to_be_bytes());
    let bytes = document(&[line(&font, BOLD, "Bold")]).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(texts(&pdf), ["Bold"]);
    let base = name_of(&pdf, &fonts(&pdf)["F1"], "BaseFont").to_string();
    assert_eq!(
        &base[6..],
        "+Inter-Regular",
        "drawn at the default instance"
    );
}

#[test]
fn overlapping_tables_in_a_whole_collection_face_are_refused() {
    let file = overlapping_collection(&with_fs_type(&liberation(), 0x0100));
    let err = document(&[in_face(&file, 0, Style::REGULAR, "a")]).unwrap_err();
    assert_eq!(err.reason(), "font_malformed", "{}", err.message());
    let subsettable = overlapping_collection(&liberation());
    document(&[in_face(&subsettable, 0, Style::REGULAR, "a")]).unwrap();
}

/// A collection of `whole` whose directory adds 300 records naming its `glyf`.
fn overlapping_collection(whole: &[u8]) -> Vec<u8> {
    let real: Vec<(Vec<u8>, usize, usize)> = (0..usize::from(be16(whole, 4)))
        .map(|table| 12 + 16 * table)
        .map(|at| {
            (
                whole[at..at + 4].to_vec(),
                be32(whole, at + 8),
                be32(whole, at + 12),
            )
        })
        .collect();
    let glyf = real
        .iter()
        .find(|(tag, _, _)| tag == b"glyf")
        .unwrap()
        .clone();
    let copies = (0..300).map(|n| (format!("x{n:03}").into_bytes(), glyf.1, glyf.2));
    let records: Vec<_> = real.into_iter().chain(copies).collect();
    let base = 16 + 12 + 16 * records.len();
    let mut file = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x01\x00\x00\x00\x10".to_vec();
    file.extend(&whole[..4]);
    file.extend((records.len() as u16).to_be_bytes());
    file.extend([0; 6]);
    for (tag, offset, length) in &records {
        file.extend(tag);
        file.extend([0; 4]);
        file.extend(((base + offset) as u32).to_be_bytes());
        file.extend((*length as u32).to_be_bytes());
    }
    file.extend(whole);
    assert!(glyf.2 * 300 > file.len());
    file
}

#[test]
fn a_whole_cid_keyed_cff_font_is_refused() {
    let source = source_sans();
    let bytes = document(&[line(&source, Style::REGULAR, "Cid")]).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    let (_, subset) = program(&pdf, &fonts(&pdf)["F1"]);
    let mut font = tables(&subset, 0);
    assert!(
        font["CFF "].windows(2).any(|w| w == [12, 30]),
        "subsetter writes a CID-keyed CFF"
    );
    let mut os2 = tables(&source, 0)["OS/2"].clone();
    os2[8..10].copy_from_slice(&0x0100_u16.to_be_bytes());
    font.insert("OS/2".into(), os2);
    let empty_cmap = [
        0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12, 0, 4, 0, 24, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0, 0xFF, 0xFF,
        0, 0, 0xFF, 0xFF, 0, 1, 0, 0,
    ];
    font.insert("cmap".into(), empty_cmap.to_vec());
    let font = sfnt(b"OTTO", &font);
    let err = document(&[SampleLine {
        font: &font,
        index: 0,
        style: Style::REGULAR,
        size: 24.0,
        glyphs: vec![(1, "C".into())],
    }])
    .unwrap_err();
    assert_eq!(
        err.reason(),
        "pdf_font_embedding_unsupported",
        "{}",
        err.message()
    );
}

#[test]
fn a_font_without_outlines_is_refused_as_such() {
    let mut font = liberation();
    let glyf = record(&font, b"glyf");
    font[glyf..glyf + 4].copy_from_slice(b"glyX");
    let err = document(&[line(&font, Style::REGULAR, "a")]).unwrap_err();
    assert_eq!(err.reason(), "pdf_font_no_outlines", "{}", err.message());
}
