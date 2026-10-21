//! Hostile-archive regressions for the package guard (upstream
//! `test_audit_zip_preflight.py`, `test_practical_opc_hardening.py`,
//! `test_word_part_name_legality.py`, `test_word_prefix_collisions.py`
//! behaviors, re-expressed as byte-level cases).

mod support;

use docrs::package::zip_read::read_archive;
use docrs::{Document, ErrorKind, Limits, RefusalKind};
use support::*;

fn reason_of(bytes: &[u8]) -> String {
    match Document::open_bytes(bytes, &Limits::default()) {
        Ok(_) => "OPENED".to_string(),
        Err(err) => {
            if err.kind() == ErrorKind::Refusal(RefusalKind::MalformedPackage) {
                err.reason().to_string()
            } else {
                format!("{}:{}", err.code(), err.reason())
            }
        }
    }
}

#[test]
fn minimal_package_opens() {
    assert_eq!(reason_of(&build_zip(&minimal_members())), "OPENED");
}

#[test]
fn envelope_defects() {
    let good = build_zip(&minimal_members());

    let mut appended = good.clone();
    appended.extend_from_slice(b"trailing bytes");
    assert_eq!(reason_of(&appended), "zip_missing_end_record");

    let truncated = &good[..good.len() - 5];
    assert_eq!(reason_of(truncated), "zip_missing_end_record");

    let prefixed = build_zip_with(
        &minimal_members(),
        &ArchiveOptions {
            prefix: b"MZ junk".to_vec(),
            ..Default::default()
        },
    );
    // Offsets are relative to the archive start, so the central directory no
    // longer forms one region at the declared offset.
    assert!(
        reason_of(&prefixed).starts_with("zip_"),
        "{}",
        reason_of(&prefixed)
    );

    let two_ends = build_zip_with(
        &minimal_members(),
        &ArchiveOptions {
            duplicate_end_record: true,
            ..Default::default()
        },
    );
    assert_eq!(reason_of(&two_ends), "zip_ambiguous_end_record");

    let wrong_count = build_zip_with(
        &minimal_members(),
        &ArchiveOptions {
            count_override: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(reason_of(&wrong_count), "zip_central_directory_count");

    let wrong_size = build_zip_with(
        &minimal_members(),
        &ArchiveOptions {
            central_size_delta: -1,
            ..Default::default()
        },
    );
    assert_eq!(reason_of(&wrong_size), "zip_central_directory_region");

    assert_eq!(reason_of(b"not a zip at all"), "zip_missing_end_record");
    assert_eq!(reason_of(b""), "zip_missing_end_record");
}

#[test]
fn prefixed_but_self_consistent_archive_refuses_last() {
    // Shift every offset so the archive is internally consistent but starts
    // after junk: only the leading-bytes rule catches it.
    let members = minimal_members();
    let inner = build_zip(&members);
    let mut shifted = b"JUNK".to_vec();
    shifted.extend_from_slice(&inner);
    // Patch central offsets and the end record by +4.
    let end = shifted.len() - 22;
    let central_offset = u32::from_le_bytes(shifted[end + 16..end + 20].try_into().unwrap()) + 4;
    shifted[end + 16..end + 20].copy_from_slice(&central_offset.to_le_bytes());
    let mut cursor = central_offset as usize;
    while &shifted[cursor..cursor + 4] == b"PK\x01\x02" {
        let offset = u32::from_le_bytes(shifted[cursor + 42..cursor + 46].try_into().unwrap()) + 4;
        shifted[cursor + 42..cursor + 46].copy_from_slice(&offset.to_le_bytes());
        let n = u16::from_le_bytes(shifted[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
        let e = u16::from_le_bytes(shifted[cursor + 30..cursor + 32].try_into().unwrap()) as usize;
        let c = u16::from_le_bytes(shifted[cursor + 32..cursor + 34].try_into().unwrap()) as usize;
        cursor += 46 + n + e + c;
    }
    assert_eq!(reason_of(&shifted), "zip_prefixed_bytes");
}

#[test]
fn member_name_defects() {
    let with_extra = |name: &str| {
        let mut members = minimal_members();
        members.push(RawMember::new(name, b"x"));
        reason_of(&build_zip(&members))
    };
    assert_eq!(with_extra("word/document.xml"), "zip_duplicate_member");
    assert_eq!(with_extra("WORD/Document.xml"), "zip_case_ambiguous_member");
    assert_eq!(with_extra("word"), "zip_directory_prefix_collision");
    assert_eq!(with_extra("/abs.xml"), "zip_illegal_part_name");
    assert_eq!(with_extra("a\\b.xml"), "zip_illegal_part_name");
    assert_eq!(with_extra("a/../b.xml"), "zip_illegal_part_name");
    assert_eq!(with_extra("a b.xml"), "zip_illegal_part_name");
    assert_eq!(with_extra("media/a%2fb.png"), "zip_illegal_part_name");
    assert_eq!(with_extra("media/my%20image.png"), "OPENED");
    assert_eq!(with_extra("C:/x.xml"), "zip_illegal_part_name");
    assert_eq!(with_extra("q?.xml"), "zip_illegal_part_name");

    let mut members = minimal_members();
    let mut nul = RawMember::new("extra.xml", b"x");
    nul.name = b"extra.xml\0hidden".to_vec();
    members.push(nul);
    assert_eq!(reason_of(&build_zip(&members)), "zip_noncanonical_name");

    let mut members = minimal_members();
    let mut utf8 = RawMember::new("x", b"x");
    utf8.name = "word/résumé.xml".as_bytes().to_vec();
    utf8.flags = 0x0800;
    members.push(utf8);
    assert_eq!(reason_of(&build_zip(&members)), "zip_illegal_part_name");
}

#[test]
fn directory_records_are_inert() {
    let mut members = minimal_members();
    members.insert(0, RawMember::new("word/", b"").stored());
    assert_eq!(reason_of(&build_zip(&members)), "OPENED");

    let mut members = minimal_members();
    members.insert(0, RawMember::new("word/", b"payload").stored());
    assert_eq!(reason_of(&build_zip(&members)), "zip_directory_data");
}

#[test]
fn word_tolerated_irregularities_open_only_on_request() {
    use docrs::Tolerance;
    // Office's `[trash]` scratch member and a directory entry holding an
    // empty compressed stream: Word opens both, the guarded reader (like
    // upstream) refuses both by default.
    let mut members = minimal_members();
    members.insert(0, RawMember::new("word/", b""));
    members.push(RawMember::new(
        "[trash]/0000.dat",
        b"discarded by a fast save",
    ));
    let bytes = build_zip(&members);
    assert_eq!(reason_of(&bytes), "zip_directory_data");
    let directory_only = Tolerance {
        empty_directory_data: true,
        ..Tolerance::default()
    };
    let err = Document::open_bytes_with(&bytes, &Limits::default(), directory_only).unwrap_err();
    assert_eq!(err.reason(), "zip_illegal_part_name");

    let (document, set_aside) =
        Document::open_bytes_with(&bytes, &Limits::default(), Tolerance::word()).unwrap();
    assert_eq!(set_aside.len(), 2, "{set_aside:?}");
    assert!(set_aside.iter().any(|s| s.contains("[trash]/0000.dat")));
    assert!(
        document
            .package()
            .resolve_member("[trash]/0000.dat")
            .is_none()
    );
    // Even an unchanged save writes a clean archive the strict reader opens.
    let (saved, report) = document.patch_bytes().unwrap();
    assert!(!report.verbatim_copy);
    assert_eq!(reason_of(&saved), "OPENED");

    // Real data in a directory entry stays refused, and a scratch member's
    // ZIP record is still validated before it is dropped.
    let mut members = minimal_members();
    members.insert(0, RawMember::new("word/", b"payload"));
    let err =
        Document::open_bytes_with(&build_zip(&members), &Limits::default(), Tolerance::word())
            .unwrap_err();
    assert_eq!(err.reason(), "zip_directory_data");
    let mut members = minimal_members();
    let mut trash = RawMember::new("[trash]/0001.dat", b"scratch");
    trash.crc_override = Some(1);
    members.push(trash);
    assert!(
        Document::open_bytes_with(&build_zip(&members), &Limits::default(), Tolerance::word())
            .is_err()
    );
}

#[test]
fn member_metadata_defects() {
    let variant = |edit: &dyn Fn(&mut RawMember)| {
        let mut members = minimal_members();
        edit(&mut members[2]);
        reason_of(&build_zip(&members))
    };
    assert_eq!(variant(&|m| m.flags = 0x1), "zip_encrypted_member");
    assert_eq!(variant(&|m| m.flags = 0x20), "zip_patched_data");
    assert_eq!(
        variant(&|m| m.method_override = Some(12)),
        "zip_unsupported_compression"
    );
    assert_eq!(
        variant(&|m| m.external_attr = 0o120777 << 16),
        "zip_unsupported_entry_type"
    );
    assert_eq!(
        variant(&|m| m.external_attr |= 0x10),
        "zip_directory_member"
    );
    assert_eq!(
        variant(&|m| m.crc_override = Some(0xDEADBEEF)),
        "zip_crc_mismatch"
    );
    assert_eq!(
        variant(&|m| m.local_crc_override = Some(1)),
        "zip_local_header_mismatch"
    );
    assert_eq!(
        variant(&|m| m.local_flags = Some(0x0800)),
        "zip_local_header_mismatch"
    );
    assert_eq!(
        variant(&|m| m.local_name = Some(b"word/other.xml".to_vec())),
        "zip_local_header_mismatch"
    );
    assert_eq!(
        variant(&|m| m.trailing_garbage = b"xx".to_vec()),
        "zip_trailing_data"
    );
    assert_eq!(
        variant(&|m| m.extract_version = 64),
        "zip_unsupported_version"
    );
    assert_eq!(
        variant(&|m| m.central_extra = vec![0x01, 0x00, 0x10, 0x00]),
        "zip_corrupt_extra_field"
    );
    assert_eq!(
        variant(&|m| m.declared_size_override = Some(m.data.len() as u32 - 1)),
        "zip_inflates_past_declared_size"
    );
    assert_eq!(
        variant(&|m| m.declared_size_override = Some(m.data.len() as u32 + 1)),
        "zip_size_mismatch"
    );
    assert_eq!(
        variant(&|m| {
            let full = deflate(&m.data);
            m.compressed_override = Some(full[..full.len() / 2].to_vec());
        }),
        "zip_truncated_deflate"
    );
    assert_eq!(
        variant(&|m| {
            let mut full = deflate(&m.data);
            full.extend_from_slice(b"\x03\x00");
            m.compressed_override = Some(full);
        }),
        "zip_trailing_compressed_data"
    );
    assert_eq!(
        variant(&|m| m.compressed_override = Some(vec![0xFF; 16])),
        "zip_invalid_deflate"
    );
    let stored_mismatch = {
        let mut members = minimal_members();
        let mut member = RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()).stored();
        member.declared_size_override = Some(3);
        members[2] = member;
        reason_of(&build_zip(&members))
    };
    assert_eq!(stored_mismatch, "zip_inconsistent_sizes");
    assert_eq!(
        variant(&|m| m.header_offset_override = Some(0)),
        "zip_shared_offset"
    );
}

#[test]
fn data_descriptors_must_agree() {
    let descriptor_case = |descriptor: Vec<u8>| {
        let mut members = minimal_members();
        let member = &mut members[2];
        member.flags = 0x8;
        member.data_descriptor = Some(descriptor);
        reason_of(&build_zip(&members))
    };
    let data = DOCUMENT_XML.as_bytes();
    let compressed = deflate(data);
    let crc = crc32fast::hash(data);
    let mut good = b"PK\x07\x08".to_vec();
    good.extend_from_slice(&crc.to_le_bytes());
    good.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    good.extend_from_slice(&(data.len() as u32).to_le_bytes());
    assert_eq!(descriptor_case(good.clone()), "OPENED");
    assert_eq!(descriptor_case(good[4..].to_vec()), "OPENED");
    let mut bad = good.clone();
    bad[4] ^= 0xFF;
    assert_eq!(descriptor_case(bad), "zip_data_descriptor");
    assert_eq!(descriptor_case(vec![0; 7]), "zip_data_descriptor");
}

#[test]
fn content_types_validated_before_other_members_inflate() {
    let mut members = minimal_members();
    members[0] = RawMember::new("[Content_Types].xml", b"<Types xmlns='wrong'/>");
    // A later member is also corrupt, but the content-types defect wins.
    members[2].crc_override = Some(1);
    assert_eq!(reason_of(&build_zip(&members)), "content_types_root");
}

#[test]
fn relationship_graph_defects() {
    let with_rels = |rels: &str| {
        let mut members = minimal_members();
        members[1] = RawMember::new("_rels/.rels", rels.as_bytes());
        reason_of(&build_zip(&members))
    };
    let ns = "http://schemas.openxmlformats.org/package/2006/relationships";
    let office =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    assert_eq!(
        with_rels(&format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/missing.xml'/></Relationships>"
        )),
        "broken_relationship_target"
    );
    assert_eq!(
        with_rels(&format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/document.xml'/>\
             <Relationship Id='rId2' Type='{office}' Target='word/document.xml'/></Relationships>"
        )),
        "multiple_office_documents"
    );
    assert_eq!(
        with_rels(&format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='http://x' TargetMode='External'/></Relationships>"
        )),
        "external_office_document"
    );
    assert_eq!(
        with_rels(&format!("<Relationships xmlns='{ns}'/>")),
        "missing_main_document"
    );
    assert_eq!(
        with_rels(&format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/document.xml'/>\
             <Relationship Id='rId2' Type='http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles' Target='word/document.xml'/></Relationships>"
        )),
        "relationship_content_type_mismatch"
    );
    // Case-insensitive member resolution, as Word accepts.
    assert_eq!(
        with_rels(&format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='WORD/Document.XML'/></Relationships>"
        )),
        "OPENED"
    );
}

#[test]
fn main_part_kind_and_xml_validity() {
    let mut members = minimal_members();
    members[0] = RawMember::new(
        "[Content_Types].xml",
        CT_XML
            .replace(
                "wordprocessingml.document.main",
                "wordprocessingml.template.main",
            )
            .as_bytes(),
    );
    assert_eq!(
        reason_of(&build_zip(&members)),
        "invalid_argument:not_a_word_document"
    );

    assert_eq!(
        reason_of(&minimal_package_with("<w:document")),
        "unparseable_xml_part"
    );
    assert_eq!(
        reason_of(&minimal_package_with(
            "<!DOCTYPE x [<!ENTITY a 'b'>]><w:document xmlns:w='u'/>"
        )),
        "xml_dtd_prohibited"
    );
    assert_eq!(
        reason_of(&minimal_package_with("<x:document/>")),
        "unparseable_xml_part"
    );
}

#[test]
fn budgets_are_resource_limits_not_malformed() {
    let bytes = build_zip(&minimal_members());
    let tight = Limits {
        max_part_bytes: 100,
        ..Limits::default()
    };
    let err = Document::open_bytes(&bytes, &tight).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ResourceLimit);
    assert_eq!(err.limit_exceeded().unwrap().limit, "max_part_bytes");

    let err = Document::open_bytes(
        &bytes,
        &Limits {
            max_entries: 2,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.limit_exceeded().unwrap().limit, "max_entries");

    let err = Document::open_bytes(
        &bytes,
        &Limits {
            max_archive_bytes: 10,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.limit_exceeded().unwrap().limit, "max_archive_bytes");

    // A member that *lies* about its size is malformed, but one that honestly
    // exceeds the total budget is a policy outcome.
    let err = read_archive(
        &bytes,
        &Limits {
            max_total_part_bytes: 300,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ResourceLimit);

    let deep = format!(
        "<w:document xmlns:w='http://schemas.openxmlformats.org/wordprocessingml/2006/main'>{}{}</w:document>",
        "<w:x>".repeat(256),
        "</w:x>".repeat(256)
    );
    let err = Document::open_bytes(&minimal_package_with(&deep), &Limits::default()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ResourceLimit);
    assert_eq!(err.limit_exceeded().unwrap().limit, "max_xml_depth");
    assert!(Document::open_bytes(&minimal_package_with(&deep), &Limits::trusted()).is_ok());

    let shallow = format!(
        "<w:document xmlns:w='http://schemas.openxmlformats.org/wordprocessingml/2006/main'>{}{}</w:document>",
        "<w:x>".repeat(255),
        "</w:x>".repeat(255)
    );
    assert!(Document::open_bytes(&minimal_package_with(&shallow), &Limits::default()).is_ok());
}
