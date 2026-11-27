//! The PDF writer, read back by an independent reader.

#[path = "support/pdf.rs"]
mod reader;

use std::process::Command;

use docrs::clock::Timestamp;
use docrs::pdf::{Info, blank_document_for_tests as blank_document};
use docrs::{ErrorKind, Limits};
use reader::{Pdf, Value, tokenize_content};
use sha2::{Digest, Sha256};

const A4: (f64, f64) = (595.2756, 841.8898);
const LETTER_LANDSCAPE: (f64, f64) = (792.0, 612.0);
const CUSTOM: (f64, f64) = (419.5276, 300.0004);

fn sample_info() -> Info {
    Info {
        title: Some(r"Report (draft) \ v2".into()),
        author: Some("Zoë Ångström – 日本".into()),
        subject: Some("Unbalanced ) and ( and \\".into()),
        keywords: Some("pdf, #tag, a/b".into()),
        created: Some(Timestamp::parse("2026-11-27T09:30:00Z").unwrap()),
        modified: Some(Timestamp::parse("2026-11-27T18:45:59+08:00").unwrap()),
    }
}

fn sample() -> Vec<u8> {
    blank_document(
        &[A4, LETTER_LANDSCAPE, CUSTOM],
        &sample_info(),
        &Limits::default(),
    )
    .unwrap()
}

#[test]
fn three_pages_read_back() {
    let bytes = sample();
    if let Some(dir) = std::env::var_os("DOCRS_PDF_OUT") {
        std::fs::write(std::path::Path::new(&dir).join("three-pages.pdf"), &bytes).unwrap();
    }
    let pdf = Pdf::parse(&bytes).unwrap();
    println!("{}", pdf.dump());
    assert_eq!(pdf.version, "1.7");
    assert_eq!(pdf.page_count(), 3);
    assert_eq!(
        pdf.media_boxes(),
        [
            [0.0, 0.0, 595.276, 841.89],
            [0.0, 0.0, 792.0, 612.0],
            [0.0, 0.0, 419.528, 300.0],
        ]
    );
    let info = pdf.info();
    let info = |key: &str| info.get(key).map(String::as_str);
    assert_eq!(info("Title"), Some(r"Report (draft) \ v2"));
    assert_eq!(info("Author"), Some("Zoë Ångström – 日本"));
    assert_eq!(info("Subject"), Some("Unbalanced ) and ( and \\"));
    assert_eq!(info("Keywords"), Some("pdf, #tag, a/b"));
    assert_eq!(info("CreationDate"), Some("D:20261127093000Z"));
    assert_eq!(info("ModDate"), Some("D:20261127104559Z"));
    assert_eq!(
        info("Producer"),
        Some(concat!("docrs ", env!("CARGO_PKG_VERSION")))
    );
}

#[test]
fn strings_are_written_literal_or_utf16() {
    let bytes = sample();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(r"/Title (Report \(draft\) \\ v2)"));
    assert!(text.contains(r"/Subject (Unbalanced \) and \( and \\)"));
    assert!(text.contains("/Author <FEFF005A006F00EB"));
}

#[test]
fn empty_info_still_names_the_producer() {
    let bytes = blank_document(&[A4], &Info::default(), &Limits::default()).unwrap();
    let info = Pdf::parse(&bytes).unwrap().info();
    assert_eq!(info.keys().collect::<Vec<_>>(), ["Producer"]);
}

#[test]
fn streams_have_their_written_length_and_inflate() {
    let pdf = Pdf::parse(&sample()).unwrap();
    let pages = pdf.pages();
    for page in pages {
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
    let count = pdf.get(&pdf.trailer, "Size").number().unwrap() as usize;
    assert_eq!(pdf.xref.len() + 1, count);
    for &(number, offset) in &pdf.xref {
        let expected = format!("{number} 0 obj\n");
        assert!(
            pdf.bytes[offset..].starts_with(expected.as_bytes()),
            "object {number} at {offset}"
        );
    }
}

#[test]
fn output_is_deterministic() {
    assert_eq!(sample(), sample());
    let bytes = blank_document(&[A4], &Info::default(), &Limits::default()).unwrap();
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // The producer string carries the crate version: re-pin when it changes.
    assert_eq!(
        digest,
        "c856424f62313a41dc4a294f78f9cd1c329eb5c2403067b1be12036d34e30730"
    );
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
    let fits = Limits {
        max_output_bytes: full,
        ..Limits::default()
    };
    assert_eq!(
        blank_document(&pages, &sample_info(), &fits).unwrap().len() as u64,
        full
    );
    for max_output_bytes in [0, 10, 200, full - 1] {
        let limits = Limits {
            max_output_bytes,
            ..Limits::default()
        };
        let err = blank_document(&pages, &sample_info(), &limits).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ResourceLimit);
        let limit = err.limit_exceeded().unwrap();
        assert_eq!(
            (limit.limit, limit.allowed),
            ("max_output_bytes", max_output_bytes)
        );
        assert!(limit.observed > max_output_bytes);
    }
}

#[test]
fn content_tokenizer_reads_operators_and_operands() {
    let ops =
        tokenize_content(b"q 1 0 0 1 72 720.5 cm BT /F#201 12 Tf (a\\(b\\)) Tj <48 69> Tj ET Q")
            .unwrap();
    let operators: Vec<&str> = ops.iter().map(|op| op.operator.as_str()).collect();
    assert_eq!(operators, ["q", "cm", "BT", "Tf", "Tj", "Tj", "ET", "Q"]);
    assert_eq!(ops[1].operands[5], Value::Real(720.5));
    assert_eq!(ops[3].operands, [Value::Name("F 1".into()), Value::Int(12)]);
    assert_eq!(ops[4].operands, [Value::Str(b"a(b)".to_vec())]);
    assert_eq!(ops[5].operands, [Value::Str(b"Hi".to_vec())]);
}

#[test]
fn qpdf_accepts_the_output() {
    if Command::new("qpdf").arg("--version").output().is_err() {
        eprintln!("SKIPPED: qpdf is not installed; `brew install qpdf` to run this check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let files = [
        ("three-pages.pdf", sample()),
        (
            "empty-info.pdf",
            blank_document(&[A4], &Info::default(), &Limits::default()).unwrap(),
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
        println!(
            "qpdf --check {name}\n{report}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{name}: {report}");
        assert!(
            report.contains("No syntax or stream encoding errors found"),
            "{name}: {report}"
        );
    }
}
