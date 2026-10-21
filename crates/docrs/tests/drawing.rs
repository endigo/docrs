//! `Drawing.has_picture` / `image` / `replace_picture` regressions
//! (upstream `tests/paper/test_missing_apis.py::DescribePictureReplace` and
//! the protection matrix row "picture"), oracle-independent.

use docrs::{Document, ErrorKind, Limits, RefusalKind};

/// `_bmp(r, g, b)` from upstream `test_missing_apis.py`.
fn bmp(red: u8, green: u8, blue: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&58u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    for value in [40u32, 1, 1] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    for value in [0u32, 4, 2835, 2835, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[blue, green, red, 0]);
    out
}

fn embed_of(doc: &Document, nth: usize) -> String {
    let tree = doc.package().xml("word/document.xml").unwrap();
    let r = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    tree.descendants(tree.root())
        .filter(|n| {
            tree.is(
                *n,
                "http://schemas.openxmlformats.org/drawingml/2006/main",
                "blip",
            )
        })
        .nth(nth)
        .and_then(|n| tree.attr(n, Some(r), "embed").map(str::to_string))
        .unwrap()
}

fn two_shared_pictures() -> Document {
    let mut doc = Document::new();
    for _ in 0..2 {
        let p = doc.add_paragraph("", None).unwrap();
        let run = p.add_run(&mut doc, None, None).unwrap();
        run.add_picture(&mut doc, &bmp(255, 0, 0), None, None)
            .unwrap();
    }
    doc
}

#[test]
fn swaps_bytes_without_sharing_the_part() {
    let mut doc = two_shared_pictures();
    let drawings = doc.drawings().unwrap();
    assert_eq!(drawings.len(), 2);
    assert!(drawings.iter().all(|d| d.has_picture(&doc).unwrap()));
    let shared = embed_of(&doc, 0);
    assert_eq!(embed_of(&doc, 1), shared);
    let red_sha = drawings[0].image(&doc).unwrap().sha1().to_string();

    let replaced = drawings[0]
        .replace_picture(&mut doc, &bmp(0, 0, 255))
        .unwrap();
    assert_eq!(replaced.image_part, "word/media/image2.bmp");
    assert_ne!(embed_of(&doc, 0), shared);
    assert_eq!(embed_of(&doc, 0), replaced.r_id);
    assert_eq!(embed_of(&doc, 1), shared);
    assert_ne!(drawings[0].image(&doc).unwrap().sha1(), red_sha);
    assert_eq!(drawings[1].image(&doc).unwrap().sha1(), red_sha);
    // Display size is untouched.
    let xml = String::from_utf8(docrs::xml::serialize(
        doc.package().xml("word/document.xml").unwrap(),
    ))
    .unwrap();
    assert_eq!(
        xml.matches("<wp:extent cx=\"12700\" cy=\"12700\"/>")
            .count(),
        2
    );
}

#[test]
fn refuses_without_changes() {
    let mut doc = two_shared_pictures();
    let drawing = doc.drawings().unwrap().remove(0);
    let before = doc.patch_bytes().unwrap().0;
    let bad = drawing
        .replace_picture(&mut doc, b"not an image")
        .unwrap_err();
    assert_eq!(
        (bad.kind(), bad.reason()),
        (ErrorKind::InvalidArgument, "unrecognized_image")
    );
    doc.set_protection("readOnly").unwrap();
    let protected_before = doc.patch_bytes().unwrap().0;
    let refused = drawing
        .replace_picture(&mut doc, &bmp(0, 255, 0))
        .unwrap_err();
    assert_eq!(refused.refusal_kind(), Some(RefusalKind::DocumentProtected));
    assert_eq!(doc.patch_bytes().unwrap().0, protected_before);
    assert_ne!(before, protected_before);
    doc.acknowledge_protection().unwrap();
    drawing.replace_picture(&mut doc, &bmp(0, 255, 0)).unwrap();
}

const LINKED: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p><w:r><w:drawing><wp:inline><wp:extent cx="1" cy="1"/><wp:docPr id="1" name="x"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:link="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing><w:drawing><wp:inline><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"/></a:graphic></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#;

fn package(document_xml: &str) -> Vec<u8> {
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    docrs::package::zip_write::write_archive(
        [
            ("[Content_Types].xml", ct.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("word/document.xml", document_xml.as_bytes()),
        ],
        &Limits::default(),
    )
    .unwrap()
}

#[test]
fn linked_and_non_picture_drawings() {
    let mut doc = Document::open_bytes(&package(LINKED), &Limits::default()).unwrap();
    let drawings = doc.drawings().unwrap();
    let (linked, chart) = (&drawings[0], &drawings[1]);
    assert!(linked.has_picture(&doc).unwrap());
    assert!(!chart.has_picture(&doc).unwrap());
    assert_eq!(linked.image(&doc).unwrap_err().reason(), "no_picture");
    let before = doc.patch_bytes().unwrap().0;
    let err = linked.replace_picture(&mut doc, &bmp(1, 2, 3)).unwrap_err();
    assert_eq!(err.refusal_kind(), Some(RefusalKind::UnsupportedStructure));
    assert_eq!(err.reason(), "linked_picture");
    let err = chart.replace_picture(&mut doc, &bmp(1, 2, 3)).unwrap_err();
    assert_eq!(
        (err.kind(), err.reason()),
        (ErrorKind::InvalidArgument, "no_picture")
    );
    assert_eq!(doc.patch_bytes().unwrap().0, before);
}

#[test]
fn drawing_handles_go_stale() {
    let mut doc = two_shared_pictures();
    let drawing = doc.drawings().unwrap().remove(0);
    let run = doc.paragraphs().unwrap()[0].runs(&doc).unwrap().remove(0);
    assert_eq!(run.drawings(&doc).unwrap(), vec![drawing.clone()]);
    run.clear(&mut doc).unwrap();
    let err = drawing.has_picture(&doc).unwrap_err();
    assert_eq!(err.reason(), "stale_handle");
    let mut other = Document::new();
    let foreign = doc.drawings().unwrap().remove(0);
    assert_eq!(
        foreign
            .replace_picture(&mut other, &bmp(0, 0, 0))
            .unwrap_err()
            .reason(),
        "foreign_handle"
    );
}
