//! Opening a package with tens of thousands of parts stays linear: the
//! relationship walk, content-type lookups and relationship-Id checks use
//! hashed lookups. This checks correctness only; it is no timing assertion.

mod support;

use docrs::{Document, Limits};
use support::*;

const CHAIN_REL: &str = "http://example.com/docrs/test/chain";
const LEAF_REL: &str = "http://example.com/docrs/test/leaf";
const NODE_CT: &str = "application/x-docrs-test-node+xml";
const LEAF_CT: &str = "application/x-docrs-test-leaf+xml";
const RELS_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">";

/// `steps` node parts chained by relationships (`n1 → n2 → …`), each also
/// relating to a leaf part the main document relates to as well. Node parts
/// use case-variant targets and every part has its own `Override`, so the
/// walk exercises case-insensitive lookups, revisits and a large
/// relationships part. About `3 * steps` members.
fn scale_package(steps: usize) -> Vec<u8> {
    let mut ct = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
    );
    let mut doc_rels = String::from(RELS_HEAD);
    doc_rels.push_str(&format!(
        "<Relationship Id=\"rId0\" Type=\"{CHAIN_REL}\" Target=\"parts/N1.xml\"/>"
    ));
    let mut members = vec![RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()).stored()];
    members.push(RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()).stored());
    for i in 1..=steps {
        // Overrides spell node names in upper case; members are lower case.
        ct.push_str(&format!(
            "<Override PartName=\"/word/parts/N{i}.XML\" ContentType=\"{NODE_CT}\"/><Override PartName=\"/word/parts/l{i}.xml\" ContentType=\"{LEAF_CT}\"/>"
        ));
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rIdL{i}\" Type=\"{LEAF_REL}\" Target=\"parts/l{i}.xml\"/>"
        ));
        let mut rels = String::from(RELS_HEAD);
        if i < steps {
            rels.push_str(&format!(
                "<Relationship Id=\"rId1\" Type=\"{CHAIN_REL}\" Target=\"N{}.xml\"/>",
                i + 1
            ));
        }
        rels.push_str(&format!(
            "<Relationship Id=\"rId2\" Type=\"{LEAF_REL}\" Target=\"l{i}.xml\"/><Relationship Id=\"rId3\" Type=\"{CHAIN_REL}\" Target=\"N1.xml\"/></Relationships>"
        ));
        members.push(RawMember::new(&format!("word/parts/n{i}.xml"), b"<n/>").stored());
        members.push(
            RawMember::new(&format!("word/parts/_rels/n{i}.xml.rels"), rels.as_bytes()).stored(),
        );
        members.push(RawMember::new(&format!("word/parts/l{i}.xml"), b"<l/>").stored());
    }
    ct.push_str("</Types>");
    doc_rels.push_str("</Relationships>");
    members.insert(
        0,
        RawMember::new("[Content_Types].xml", ct.as_bytes()).stored(),
    );
    members.push(RawMember::new("word/_rels/document.xml.rels", doc_rels.as_bytes()).stored());
    build_zip(&members)
}

fn steps() -> usize {
    std::env::var("DOCRS_SCALE_STEPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(6_700)
}

#[test]
fn twenty_thousand_chained_parts_open_under_trusted_limits() {
    let steps = steps();
    let bytes = scale_package(steps);
    let started = std::time::Instant::now();
    let doc = Document::open_bytes(&bytes, &Limits::trusted()).unwrap();
    eprintln!(
        "opened {} members ({} bytes) in {:?}",
        3 * steps + 4,
        bytes.len(),
        started.elapsed()
    );
    let package = doc.package();
    assert_eq!(package.parts().count(), 3 * steps + 4);
    // Every part but the content types and relationships parts is reached.
    let unreached: Vec<&str> = package
        .parts()
        .filter(|part| !part.is_reachable())
        .map(|part| part.name())
        .filter(|name| *name != "[Content_Types].xml" && !name.ends_with(".rels"))
        .collect();
    assert!(unreached.is_empty(), "{unreached:?}");
    for i in [1, steps / 2, steps] {
        let node = package.part(&format!("word/parts/n{i}.xml")).unwrap();
        assert_eq!(node.content_type(), Some(NODE_CT));
        let leaf = package.part(&format!("word/parts/l{i}.xml")).unwrap();
        assert_eq!(leaf.content_type(), Some(LEAF_CT));
    }
    // Member order is archive order.
    let names: Vec<&str> = package.parts().map(|part| part.name()).collect();
    assert_eq!(
        names[..3],
        ["[Content_Types].xml", "_rels/.rels", "word/document.xml"]
    );
    assert_eq!(names[3], "word/parts/n1.xml");
    assert_eq!(*names.last().unwrap(), "word/_rels/document.xml.rels");

    let (saved, report) = doc.patch_bytes().unwrap();
    assert!(report.verbatim_copy);
    assert_eq!(saved, bytes);
}

#[test]
fn missing_content_type_and_duplicate_ids_still_refuse_at_scale() {
    let bytes = scale_package(500);
    // Drop the Override of one leaf: no Default covers it any more once the
    // xml Default is gone too.
    let mut members = Vec::new();
    for (name, data) in members_of(&bytes) {
        let data = if name == "[Content_Types].xml" {
            String::from_utf8(data)
                .unwrap()
                .replace("<Override PartName=\"/word/parts/l250.xml\" ContentType=\"application/x-docrs-test-leaf+xml\"/>", "")
                .replace("<Default Extension=\"xml\" ContentType=\"application/xml\"/>", "")
                .into_bytes()
        } else {
            data
        };
        members.push(RawMember::new(&name, &data).stored());
    }
    let err = Document::open_bytes(&build_zip(&members), &Limits::trusted()).unwrap_err();
    assert_eq!(err.reason(), "undeclared_content_type");

    let mut members = Vec::new();
    for (name, data) in members_of(&bytes) {
        let data = if name == "word/_rels/document.xml.rels" {
            String::from_utf8(data)
                .unwrap()
                .replace("Id=\"rIdL400\"", "Id=\"rIdL7\"")
                .into_bytes()
        } else {
            data
        };
        members.push(RawMember::new(&name, &data).stored());
    }
    let err = Document::open_bytes(&build_zip(&members), &Limits::trusted()).unwrap_err();
    assert_eq!(err.reason(), "relationship_duplicate_id");
}

fn members_of(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let doc = Document::open_bytes(bytes, &Limits::trusted()).unwrap();
    doc.package()
        .parts()
        .map(|part| (part.name().to_string(), part.original().unwrap().to_vec()))
        .collect()
}
