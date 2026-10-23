//! Removed part names are free again: inside the removing transaction,
//! after its commit, and through save and reopen.

mod support;

use docrs::package::rels::rt;
use docrs::txn::{RelTarget, Txn};
use docrs::{Document, Limits};
use support::*;

const IMAGE: &str = "word/media/image1.png";
const PNG_A: &[u8] = b"\x89PNG\r\n\x1a\nfirst image";
const PNG_B: &[u8] = b"\x89PNG\r\n\x1a\nsecond image";

/// A package whose main part relates to `word/media/image1.png`.
fn package_with_image() -> Vec<u8> {
    let ct = CT_XML.replace(
        "</Types>",
        "<Default Extension=\"png\" ContentType=\"image/png\"/></Types>",
    );
    let doc_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{}\" Target=\"media/image1.png\"/></Relationships>",
        rt::IMAGE
    );
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        RawMember::new(IMAGE, PNG_A),
    ])
}

fn open(bytes: &[u8]) -> Document {
    Document::open_bytes(bytes, &Limits::default()).unwrap()
}

fn commit(doc: &mut Document, edit: impl FnOnce(&mut Txn<'_>)) {
    let staged = {
        let mut txn = Txn::new(doc);
        edit(&mut txn);
        txn.finish().unwrap()
    };
    doc.commit_for_tests(staged).unwrap();
}

fn image_bytes(doc: &Document, name: &str) -> Vec<u8> {
    doc.package()
        .part(name)
        .unwrap()
        .current_bytes()
        .into_owned()
}

fn unrelate_image(txn: &mut Txn<'_>) {
    let rels = txn.resolve("word/_rels/document.xml.rels").unwrap();
    let tree = txn.tree_mut(&rels).unwrap();
    let root = tree.root();
    let targets: Vec<_> = tree
        .child_elements(root)
        .filter(|node| tree.attr(*node, None, "Target") == Some("media/image1.png"))
        .collect();
    for node in targets {
        tree.detach(node).unwrap();
    }
}

#[test]
fn remove_then_readd_in_one_transaction() {
    let original = package_with_image();
    let mut doc = open(&original);
    commit(&mut doc, |txn| {
        txn.remove_part(IMAGE).unwrap();
        assert!(txn.resolve(IMAGE).is_none());
        txn.add_binary_part(IMAGE, "image/png", PNG_B.to_vec())
            .unwrap();
        assert_eq!(txn.resolve(IMAGE).as_deref(), Some(IMAGE));
    });
    assert_eq!(image_bytes(&doc, IMAGE), PNG_B);

    let (saved, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec![IMAGE.to_string()]);
    assert!(report.added_parts.is_empty(), "{report:?}");
    assert!(report.removed_parts.is_empty(), "{report:?}");
    let reopened = open(&saved);
    assert_eq!(image_bytes(&reopened, IMAGE), PNG_B);
    assert_eq!(
        reopened.package().part(IMAGE).unwrap().content_type(),
        Some("image/png")
    );
    assert!(reopened.package().part(IMAGE).unwrap().is_reachable());
}

#[test]
fn readding_identical_bytes_restores_the_original_archive() {
    let original = package_with_image();
    let mut doc = open(&original);
    commit(&mut doc, |txn| {
        txn.remove_part(IMAGE).unwrap();
        txn.add_binary_part(IMAGE, "image/png", PNG_A.to_vec())
            .unwrap();
    });
    let (saved, report) = doc.patch_bytes().unwrap();
    assert!(report.verbatim_copy, "{report:?}");
    assert_eq!(saved, original);
}

#[test]
fn removed_staged_part_is_not_reported_removed_when_readded() {
    let doc = open(&package_with_image());
    let mut txn = Txn::new(&doc);
    txn.remove_part(IMAGE).unwrap();
    txn.add_binary_part(IMAGE, "image/png", PNG_B.to_vec())
        .unwrap();
    let staged = txn.finish().unwrap();
    assert!(staged.removed_parts().is_empty());
    assert!(staged.touched_parts().contains(&IMAGE));
}

#[test]
fn remove_commit_then_readd_in_a_later_transaction() {
    let original = package_with_image();
    let mut doc = open(&original);
    commit(&mut doc, |txn| {
        unrelate_image(txn);
        txn.remove_part(IMAGE).unwrap();
    });
    assert!(doc.package().part(IMAGE).is_none());
    assert!(doc.package().resolve_member(IMAGE).is_none());

    let (removed_only, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.removed_parts, vec![IMAGE.to_string()]);
    assert!(open(&removed_only).package().part(IMAGE).is_none());

    commit(&mut doc, |txn| {
        assert!(txn.resolve(IMAGE).is_none());
        txn.add_binary_part(IMAGE, "image/png", PNG_B.to_vec())
            .unwrap();
        txn.relate_to("word/document.xml", rt::IMAGE, RelTarget::Part(IMAGE))
            .unwrap();
    });
    let part = doc.package().part(IMAGE).unwrap();
    assert_eq!(part.current_bytes().as_ref(), PNG_B);
    assert!(part.is_reachable());
    assert_eq!(
        doc.package().parts().filter(|p| p.name() == IMAGE).count(),
        1
    );

    let (saved, report) = doc.patch_bytes().unwrap();
    assert!(report.changed_parts.contains(&IMAGE.to_string()));
    assert!(report.removed_parts.is_empty(), "{report:?}");
    let reopened = open(&saved);
    assert_eq!(image_bytes(&reopened, IMAGE), PNG_B);
    assert!(reopened.package().part(IMAGE).unwrap().is_reachable());

    commit(&mut doc, |txn| {
        txn.remove_part(IMAGE).unwrap();
        txn.add_binary_part(IMAGE, "image/png", PNG_A.to_vec())
            .unwrap();
    });
    let (saved, _) = doc.patch_bytes().unwrap();
    assert_eq!(saved, original);
}

#[test]
fn readded_xml_part_keeps_its_content_type_override() {
    let mut doc = open(&package_with_settings(DOCUMENT_XML, SETTINGS_XML));
    let settings = "word/settings.xml";
    let settings_ct = "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
    commit(&mut doc, |txn| {
        txn.remove_part(settings).unwrap();
        let tree = docrs::xml::parse_str(SETTINGS_ZOOM_XML).unwrap();
        txn.add_xml_part(settings, settings_ct, tree).unwrap();
    });
    let (saved, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.changed_parts, vec![settings.to_string()]);
    let reopened = open(&saved);
    let part = reopened.package().part(settings).unwrap();
    assert_eq!(part.content_type(), Some(settings_ct));
    assert!(
        String::from_utf8_lossy(&part.current_bytes()).contains("w:zoom"),
        "re-added tree was not saved"
    );
}

#[test]
fn case_variant_of_a_removed_name_is_a_new_member() {
    let mut doc = open(&package_with_image());
    let variant = "word/media/IMAGE1.png";
    commit(&mut doc, |txn| {
        txn.remove_part(IMAGE).unwrap();
        txn.add_binary_part(variant, "image/png", PNG_B.to_vec())
            .unwrap();
    });
    assert!(doc.package().part(IMAGE).is_none());
    assert_eq!(doc.package().resolve_member(IMAGE), Some(variant));
    let (saved, report) = doc.patch_bytes().unwrap();
    assert_eq!(report.removed_parts, vec![IMAGE.to_string()]);
    assert_eq!(report.added_parts, vec![variant.to_string()]);
    // The relationship resolves case-insensitively to the new member.
    let reopened = open(&saved);
    assert_eq!(image_bytes(&reopened, variant), PNG_B);
    assert!(reopened.package().part(variant).unwrap().is_reachable());
}

#[test]
fn case_folded_collision_with_a_live_part_is_still_refused() {
    let doc = open(&package_with_image());
    let mut txn = Txn::new(&doc);
    let err = txn
        .add_binary_part("Word/Media/Image1.PNG", "image/png", PNG_B.to_vec())
        .unwrap_err();
    assert_eq!(err.reason(), "duplicate_part");

    let mut txn = Txn::new(&doc);
    txn.add_binary_part("word/media/image2.png", "image/png", PNG_B.to_vec())
        .unwrap();
    let err = txn
        .add_binary_part("Word/Media/Image2.PNG", "image/png", PNG_B.to_vec())
        .unwrap_err();
    assert_eq!(err.reason(), "duplicate_part");
}

#[test]
fn removed_name_is_refused_again_once_readded() {
    let doc = open(&package_with_image());
    let mut txn = Txn::new(&doc);
    txn.remove_part(IMAGE).unwrap();
    txn.add_binary_part(IMAGE, "image/png", PNG_B.to_vec())
        .unwrap();
    let err = txn
        .add_binary_part("Word/Media/Image1.PNG", "image/png", PNG_A.to_vec())
        .unwrap_err();
    assert_eq!(err.reason(), "duplicate_part");
}

const SETTINGS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#;

const SETTINGS_ZOOM_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:zoom w:percent="120"/></w:settings>"#;
