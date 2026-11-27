//! The PDF writer, read back by an independent reader.

#[path = "support/pdf.rs"]
mod reader;

use std::path::PathBuf;
use std::process::Command;

use docrs::clock::Timestamp;
use docrs::pdf::{Info, blank_document_for_tests as blank_document};
use docrs::{ErrorKind, Limits};
use reader::{Op, Pdf, Value, tokenize_content};
use sha2::{Digest, Sha256};

const A4: (f64, f64) = (595.2756, 841.8898);
const LETTER_LANDSCAPE: (f64, f64) = (792.0, 612.0);
const CUSTOM: (f64, f64) = (419.5276, 300.0004);
const PRODUCER: &str = "docrs-test";

fn sample_info() -> Info {
    Info {
        title: Some(r"Report (draft) \ v2".into()),
        author: Some("Zoë Ångström – 日本".into()),
        subject: Some("Unbalanced ) and ( and \\".into()),
        keywords: Some("pdf, #tag, a/b\ttab\u{18}breve\u{7f}".into()),
        created: Some(Timestamp::parse("2026-11-27T09:30:00Z").unwrap()),
        modified: Some(Timestamp::parse("2026-11-27T18:45:59+08:00").unwrap()),
    }
}

fn write(pages: &[(f64, f64)], info: &Info, limits: &Limits) -> docrs::Result<Vec<u8>> {
    blank_document(pages, info, PRODUCER, limits)
}

fn sample() -> Vec<u8> {
    write(
        &[A4, LETTER_LANDSCAPE, CUSTOM],
        &sample_info(),
        &Limits::default(),
    )
    .unwrap()
}

fn one_page() -> Vec<u8> {
    let info = Info {
        title: Some("Golden (one page)".into()),
        created: Some(Timestamp::parse("2026-01-02T03:04:05Z").unwrap()),
        ..Info::default()
    };
    write(&[A4], &info, &Limits::default()).unwrap()
}

/// After an intended change to the output, regenerate the file with
/// `DOCRS_BLESS=1 cargo test -p docrs --test pdf output_matches_the_golden_file`,
/// then run `qpdf --check` on it and review the diff.
fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pdf/one-page.pdf")
}

#[test]
fn three_pages_read_back() {
    let pdf = Pdf::parse(&sample()).unwrap();
    assert_eq!(pdf.version, "1.7");
    assert_eq!(pdf.pages().unwrap().len(), 3);
    assert_eq!(
        pdf.media_boxes().unwrap(),
        [
            [0.0, 0.0, 595.276, 841.89],
            [0.0, 0.0, 792.0, 612.0],
            [0.0, 0.0, 419.528, 300.0],
        ]
    );
    let info = pdf.info().unwrap();
    let info = |key: &str| info.get(key).map(String::as_str);
    assert_eq!(info("Title"), Some(r"Report (draft) \ v2"));
    assert_eq!(info("Author"), Some("Zoë Ångström – 日本"));
    assert_eq!(info("Subject"), Some("Unbalanced ) and ( and \\"));
    assert_eq!(
        info("Keywords"),
        Some("pdf, #tag, a/b\ttab\u{18}breve\u{7f}")
    );
    assert_eq!(info("CreationDate"), Some("D:20261127093000Z"));
    assert_eq!(info("ModDate"), Some("D:20261127104559Z"));
    assert_eq!(info("Producer"), Some(PRODUCER));
}

#[test]
fn strings_are_literal_only_when_pdfdoc_and_ascii_agree() {
    let bytes = sample();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(r"/Title (Report \(draft\) \\ v2)"));
    assert!(text.contains(r"/Subject (Unbalanced \) and \( and \\)"));
    assert!(text.contains("/Author <FEFF005A006F00EB"));
    assert!(
        text.contains("/Keywords <FEFF"),
        "U+0018 and U+007F force UTF-16"
    );
}

#[test]
fn empty_info_still_names_the_producer() {
    let bytes = write(&[A4], &Info::default(), &Limits::default()).unwrap();
    let info = Pdf::parse(&bytes).unwrap().info().unwrap();
    assert_eq!(info.keys().collect::<Vec<_>>(), ["Producer"]);
}

#[test]
fn streams_have_their_written_length_and_inflate() {
    let pdf = Pdf::parse(&sample()).unwrap();
    for page in pdf.pages().unwrap() {
        let contents = page.get("Contents").unwrap();
        let Value::Stream(dict, raw) = pdf.resolve(contents) else {
            panic!("page contents are not a stream");
        };
        assert_eq!(pdf.get(dict, "Length").number(), Some(raw.len() as f64));
        assert!(!raw.is_empty(), "an empty stream is still a zlib frame");
        let data = pdf.stream_data(contents).unwrap();
        assert_eq!(tokenize_content(&data).unwrap(), []);
    }
}

#[test]
fn xref_offsets_point_at_their_objects() {
    let pdf = Pdf::parse(&sample()).unwrap();
    let size = pdf.get(&pdf.trailer, "Size").number().unwrap() as usize;
    assert_eq!(pdf.xref.len() + 1, size);
    for entry in &pdf.xref {
        let (number, offset) = (entry.number, entry.offset);
        let expected = format!("{number} 0 obj\n");
        assert!(
            pdf.bytes[offset..].starts_with(expected.as_bytes()),
            "object {number} at {offset}"
        );
    }
}

#[test]
fn output_matches_the_golden_file() {
    assert_eq!(sample(), sample());
    let bytes = one_page();
    if std::env::var_os("DOCRS_BLESS").is_some() {
        std::fs::write(golden_path(), &bytes).unwrap();
    }
    let golden = std::fs::read(golden_path()).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        String::from_utf8_lossy(&golden)
    );
    assert_eq!(bytes, golden);
}

#[test]
fn file_id_hashes_the_body() {
    let bytes = sample();
    let pdf = Pdf::parse(&bytes).unwrap();
    let expected = Sha256::digest(&bytes[..pdf.xref_at])[..16].to_vec();
    let Value::Array(id) = pdf.get(&pdf.trailer, "ID") else {
        panic!("no /ID");
    };
    assert_eq!(id, &[Value::Str(expected.clone()), Value::Str(expected)]);
}

#[test]
fn output_limit_stops_the_write() {
    let full = sample().len() as u64;
    let pages = [A4, LETTER_LANDSCAPE, CUSTOM];
    let limits = |max_output_bytes| Limits {
        max_output_bytes,
        ..Limits::default()
    };
    let fits = write(&pages, &sample_info(), &limits(full)).unwrap();
    assert_eq!(fits.len() as u64, full);
    for max in [0, 10, 200, full - 1] {
        let err = write(&pages, &sample_info(), &limits(max)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ResourceLimit);
        let limit = err.limit_exceeded().unwrap();
        assert_eq!((limit.limit, limit.allowed), ("max_output_bytes", max));
        assert!(limit.observed > max);
    }
}

#[test]
fn page_lists_and_sizes_are_checked() {
    let info = Info::default();
    let refused = |pages: &[(f64, f64)]| {
        let err = write(pages, &info, &Limits::default()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument, "{pages:?}");
        err.reason()
    };
    assert_eq!(refused(&[]), "pdf_no_pages");
    for side in [f64::NAN, f64::INFINITY, 0.0, -1.0, 0.001, 2.999, 14_400.001] {
        assert_eq!(refused(&[A4, (side, 100.0)]), "pdf_page_size");
        assert_eq!(refused(&[(100.0, side)]), "pdf_page_size");
    }
    let extremes = write(&[(14_400.0, 3.0)], &info, &Limits::default()).unwrap();
    assert_eq!(
        Pdf::parse(&extremes).unwrap().media_boxes().unwrap(),
        [[0.0, 0.0, 14_400.0, 3.0]]
    );
}

#[test]
fn qpdf_accepts_the_output() {
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
    let files = [
        ("three-pages.pdf", sample()),
        ("one-page.pdf", one_page()),
        (
            "empty-info.pdf",
            write(&[A4], &Info::default(), &Limits::default()).unwrap(),
        ),
    ];
    for (name, bytes) in files {
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
}

/// A classic-xref file from object bodies, numbered from 1.
fn handmade(objects: &[&str], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", index + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    let size = objects.len() + 1;
    out.extend(
        format!("trailer\n<< /Size {size} {trailer} >>\nstartxref\n{xref}\n%%EOF\n").bytes(),
    );
    out
}

const TREE: [&str; 3] = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 200] >>",
    "<< /Type /Page /Parent 2 0 R >>",
];

fn rejected(bytes: &[u8]) -> String {
    Pdf::parse(bytes)
        .err()
        .expect("the reader should refuse this file")
}

#[test]
fn reader_inherits_page_attributes() {
    let pdf = Pdf::parse(&handmade(&TREE, "/Root 1 0 R")).unwrap();
    assert_eq!(pdf.media_boxes().unwrap(), [[0.0, 0.0, 100.0, 200.0]]);
}

#[test]
fn reader_checks_xref_entries_exactly() {
    let good = handmade(&TREE, "/Root 1 0 R");
    let text = String::from_utf8(good).unwrap();
    let bad_eol = text.replacen(" n \n", "n\n\n", 1);
    assert!(rejected(bad_eol.as_bytes()).contains("bad xref entry"));
    let bad_digit = text.replacen("00000 n", "0000x n", 1);
    assert!(rejected(bad_digit.as_bytes()).contains("bad xref entry"));
    let used_zero = text.replace("0000000000 65535 f", "0000000000 65535 n");
    assert!(rejected(used_zero.as_bytes()).contains("entry 0"));
}

#[test]
fn reader_requires_exact_stream_lengths() {
    let stream = |length: &str| format!("<< /Length {length} >>\nstream\nhello\nendstream");
    let exact = stream("5");
    let pdf = Pdf::parse(&handmade(&[&exact], "")).unwrap();
    assert_eq!(pdf.stream_data(&Value::Ref(1, 0)).unwrap(), b"hello");
    assert!(rejected(&handmade(&[&stream("3")], "")).contains("does not end at endstream"));
    assert!(rejected(&handmade(&[&stream("9")], "")).contains("does not end at endstream"));
    let forward = stream("2 0 R");
    let pdf = Pdf::parse(&handmade(&[&forward, "5"], "")).unwrap();
    assert_eq!(pdf.stream_data(&Value::Ref(1, 0)).unwrap(), b"hello");
    let missing = "<< >>\nstream\nhello\nendstream";
    assert!(rejected(&handmade(&[missing], "")).contains("without /Length"));
}

#[test]
fn reader_refuses_duplicate_keys() {
    let duplicate = "<< /Type /Catalog /Type /Pages >>";
    assert!(rejected(&handmade(&[duplicate], "")).contains("duplicate key /Type"));
}

#[test]
fn reader_follows_prev() {
    let mut bytes = handmade(&TREE, "/Root 1 0 R");
    let first_xref = Pdf::parse(&bytes).unwrap().xref_at;
    let page_at = bytes.len();
    bytes.extend(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 50 60] >>\nendobj\n");
    let xref = bytes.len();
    bytes.extend(
        format!(
            "xref\n3 1\n{page_at:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R /Prev {first_xref} >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .bytes(),
    );
    let pdf = Pdf::parse(&bytes).unwrap();
    assert_eq!(pdf.xref.len(), 3);
    assert_eq!(pdf.media_boxes().unwrap(), [[0.0, 0.0, 50.0, 60.0]]);
}

#[test]
fn reader_decodes_only_the_ascii_part_of_pdfdocencoding() {
    assert_eq!(reader::decode_text(b"a\tb").unwrap(), "a\tb");
    assert!(reader::decode_text(b"a\x18b").is_err());
    assert!(reader::decode_text(b"\xFE\xFF\x00").is_err());
}

#[test]
fn content_tokenizer_reads_operators_and_operands() {
    let ops = tokenize_content(
        b"q 1 0 0 1 72 720.5 cm BT /F#201 12 Tf (a\\(b\\)) Tj <48 69> Tj [(A) -20 (B)] TJ ET Q",
    )
    .unwrap();
    let operators: Vec<&str> = ops.iter().map(|op| op.operator.as_str()).collect();
    assert_eq!(
        operators,
        ["q", "cm", "BT", "Tf", "Tj", "Tj", "TJ", "ET", "Q"]
    );
    assert_eq!(ops[1].operands[5], Value::Real(720.5));
    assert_eq!(ops[3].operands, [Value::Name("F 1".into()), Value::Int(12)]);
    assert_eq!(ops[4].operands, [Value::Str(b"a(b)".to_vec())]);
    assert_eq!(ops[5].operands, [Value::Str(b"Hi".to_vec())]);
}

#[test]
fn content_tokenizer_reads_inline_images() {
    let ops = tokenize_content(b"q BI /W 2 /H 1 /BPC 8 /CS /G ID \x00EI\xFF\nEI Q").unwrap();
    let Op { operator, operands } = &ops[1];
    assert_eq!(operator, "BI");
    assert_eq!(operands[0].dict().unwrap()["W"], Value::Int(2));
    assert_eq!(operands[1], Value::Str(b"\x00EI\xFF".to_vec()));
    assert_eq!(ops[2].operator, "Q");
}

#[test]
fn content_tokenizer_is_strict() {
    for (content, error) in [
        (&b"1 0 R"[..], "unknown operator \"R\""),
        (b"1e3 0 m", "bad number"),
        (b"0 0 mm", "unknown operator \"mm\""),
        (b"0 0", "operands without an operator"),
        (b"EI", "unknown operator \"EI\""),
        (b"5 m", "m with operands"),
        (b"(x) /N re", "re with operands"),
        (b"/F1 Tf", "Tf with operands"),
        (b"0 0 0 0 0 sc", "sc with operands"),
        (b"Q", "Q without a matching q"),
        (b"q", "content ends inside q or BT"),
        (b"BT BT ET ET", "BT inside a text object"),
        (b"ET", "ET without BT"),
        (b"BT", "content ends inside q or BT"),
    ] {
        let err = tokenize_content(content).unwrap_err();
        assert!(err.contains(error), "{content:?}: {err}");
    }
}

#[test]
fn content_tokenizer_checks_colour_operands() {
    for content in [
        &b"0.5 sc"[..],
        b"1 0 0 1 SC",
        b"/P0 scn",
        b"0.2 0.3 /P1 SCN",
        b"1 scn",
    ] {
        tokenize_content(content).unwrap();
    }
    assert!(tokenize_content(b"/P0 0.5 scn").is_err());
}

#[test]
fn reader_refuses_reference_cycles() {
    let own_length = "<< /Length 1 0 R >>\nstream\nhello\nendstream";
    assert!(rejected(&handmade(&[own_length], "")).contains("refers to itself"));
    let kids_cycle = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Pages /Kids [2 0 R] /Count 1 >>",
    ];
    let pdf = Pdf::parse(&handmade(&kids_cycle, "/Root 1 0 R")).unwrap();
    assert!(pdf.pages().unwrap_err().contains("twice"));
    let parent_cycle = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 4 0 R >>",
        "<< /Parent 3 0 R >>",
    ];
    let pdf = Pdf::parse(&handmade(&parent_cycle, "/Root 1 0 R")).unwrap();
    assert!(pdf.media_boxes().unwrap_err().contains("cyclic /Parent"));
}

#[test]
fn reader_checks_the_page_tree() {
    let miscounted = [
        TREE[0],
        "<< /Type /Pages /Kids [3 0 R] /Count 2 >>",
        TREE[2],
    ];
    let pdf = Pdf::parse(&handmade(&miscounted, "/Root 1 0 R")).unwrap();
    assert!(pdf.pages().unwrap_err().contains("/Count"));
    let dangling = ["<< /Type /Catalog /Pages 9 0 R >>"];
    let pdf = Pdf::parse(&handmade(&dangling, "/Root 1 0 R")).unwrap();
    assert!(pdf.pages().unwrap_err().contains("not a dictionary"));
}

#[test]
fn reader_checks_the_file_structure() {
    let text = String::from_utf8(handmade(&TREE, "/Root 1 0 R")).unwrap();
    let lone_cr =
        String::from_utf8(handmade(&["<< /Length 5 >>\rstream\rhello\nendstream"], "")).unwrap();
    let lone_cr = lone_cr.replace("<< /Length 5 >>\rstream\r", "<< /Length 5 >>\nstream\r");
    assert!(rejected(lone_cr.as_bytes()).contains("CR LF or LF"));
    let generation = text.replacen("3 0 obj", "3 5 obj", 1);
    assert!(rejected(generation.as_bytes()).contains("found 3 5"));
    let xref_generation = text.replacen("00000 n", "00007 n", 1);
    assert!(rejected(xref_generation.as_bytes()).contains("xref says object 1 7"));
    let junk = format!("{text}junk\n");
    assert!(rejected(junk.as_bytes()).contains("after %%EOF"));
    let one_line = text.replace("startxref\n", "startxref ");
    assert!(rejected(one_line.as_bytes()).contains("startxref"));
    let size = text.replace("/Size 4", "/Size 5");
    assert!(rejected(size.as_bytes()).contains("/Size"));
    let overflow = text.replace("xref\n0 4\n", "xref\n4294967295 4\n");
    assert!(rejected(overflow.as_bytes()).contains("past u32"));
}

#[test]
fn reader_returns_raw_streams() {
    let jpeg = "<< /Filter /DCTDecode /Length 4 >>\nstream\nJPEG\nendstream";
    let pdf = Pdf::parse(&handmade(&[jpeg], "")).unwrap();
    let (dict, raw) = pdf.raw_stream(&Value::Ref(1, 0)).unwrap();
    assert_eq!(
        (dict["Filter"].clone(), raw),
        (Value::Name("DCTDecode".into()), &b"JPEG"[..])
    );
    assert!(
        pdf.stream_data(&Value::Ref(1, 0))
            .unwrap_err()
            .contains("unsupported filter")
    );
}

#[test]
fn reader_refuses_integers_beyond_i64() {
    let huge = "[340282346638528859811704183484516925440]";
    assert!(rejected(&handmade(&[huge], "")).contains("bad number"));
}
