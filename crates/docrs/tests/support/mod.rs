//! Shared test helpers: fixture access and a raw ZIP builder able to emit
//! deliberately inconsistent archives.

#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;

use docrs::{Document, Limits};
use flate2::Compression;
use flate2::write::DeflateEncoder;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn fixtures_dir() -> PathBuf {
    repo_root().join("tests/fixtures/upstream")
}

pub fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(rel)).unwrap_or_else(|err| panic!("fixture {rel}: {err}"))
}

/// Every `.docx` under the upstream corpus, relative paths, sorted.
pub fn all_fixtures() -> Vec<String> {
    let manifest = std::fs::read_to_string(fixtures_dir().join("MANIFEST.sha256")).unwrap();
    let mut out: Vec<String> = manifest
        .lines()
        .filter_map(|line| line.split_once("  ").map(|(_, path)| path.to_string()))
        .filter(|path| path.ends_with(".docx"))
        .collect();
    out.sort();
    out
}

pub fn is_corrupt_fixture(rel: &str) -> bool {
    rel.contains("/corrupt/")
}

/// A fixture opened with the default budgets.
pub fn open_fixture(rel: &str) -> Document {
    open_bytes(&fixture(rel))
}

pub fn open_bytes(bytes: &[u8]) -> Document {
    Document::open_bytes(bytes, &Limits::default()).unwrap()
}

/// The package as a patch-save writes it.
pub fn snapshot(doc: &Document) -> Vec<u8> {
    doc.patch_bytes().unwrap().0
}

pub fn reopen(doc: &Document) -> Document {
    open_bytes(&snapshot(doc))
}

/// A part serialized back to text.
pub fn part_xml(doc: &Document, part: &str) -> String {
    String::from_utf8(docrs::xml::serialize(doc.package().xml(part).unwrap())).unwrap()
}

pub fn document_xml(doc: &Document) -> String {
    part_xml(doc, "word/document.xml")
}

/// A 3×2 PNG at 150 dpi, as hex.
pub const PNG_150DPI: &str = "89504e470d0a1a0a0000000d49484452000000030000000208020000001216f14d0000000970485973000017120000171201679fd2520000000b49444154789c6360800100000a00017f80745e0000000049454e44ae426082";

/// Bytes from a hex string.
pub fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

pub fn png_150dpi() -> Vec<u8> {
    hex(PNG_150DPI)
}

/// One raw member for [`build_zip`].
#[derive(Clone)]
pub struct RawMember {
    pub name: Vec<u8>,
    pub local_name: Option<Vec<u8>>,
    pub data: Vec<u8>,
    pub deflate: bool,
    pub flags: u16,
    pub local_flags: Option<u16>,
    pub method_override: Option<u16>,
    pub crc_override: Option<u32>,
    pub local_crc_override: Option<u32>,
    pub declared_size_override: Option<u32>,
    pub compressed_override: Option<Vec<u8>>,
    pub external_attr: u32,
    pub trailing_garbage: Vec<u8>,
    pub data_descriptor: Option<Vec<u8>>,
    pub central_extra: Vec<u8>,
    pub extract_version: u16,
    pub header_offset_override: Option<u32>,
}

impl RawMember {
    pub fn new(name: &str, data: &[u8]) -> Self {
        RawMember {
            name: name.as_bytes().to_vec(),
            local_name: None,
            data: data.to_vec(),
            deflate: true,
            flags: 0,
            local_flags: None,
            method_override: None,
            crc_override: None,
            local_crc_override: None,
            declared_size_override: None,
            compressed_override: None,
            external_attr: 0o644 << 16,
            trailing_garbage: Vec::new(),
            data_descriptor: None,
            central_extra: Vec::new(),
            extract_version: 20,
            header_offset_override: None,
        }
    }

    pub fn stored(mut self) -> Self {
        self.deflate = false;
        self
    }
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Archive-level knobs for [`build_zip_with`].
#[derive(Default, Clone)]
pub struct ArchiveOptions {
    pub prefix: Vec<u8>,
    pub suffix: Vec<u8>,
    pub comment: Vec<u8>,
    pub count_override: Option<u16>,
    pub central_size_delta: i64,
    pub duplicate_end_record: bool,
}

pub fn build_zip(members: &[RawMember]) -> Vec<u8> {
    build_zip_with(members, &ArchiveOptions::default())
}

pub fn build_zip_with(members: &[RawMember], options: &ArchiveOptions) -> Vec<u8> {
    let mut out = options.prefix.clone();
    let base = out.len();
    let mut central = Vec::new();
    for member in members {
        let offset = (out.len() - base) as u32;
        let compressed = member.compressed_override.clone().unwrap_or_else(|| {
            if member.deflate {
                deflate(&member.data)
            } else {
                member.data.clone()
            }
        });
        let method = member
            .method_override
            .unwrap_or(if member.deflate { 8 } else { 0 });
        let crc = member
            .crc_override
            .unwrap_or_else(|| crc32fast::hash(&member.data));
        let size = member
            .declared_size_override
            .unwrap_or(member.data.len() as u32);
        let local_name = member
            .local_name
            .clone()
            .unwrap_or_else(|| member.name.clone());
        let local_flags = member.local_flags.unwrap_or(member.flags);
        let (local_crc, local_csize, local_size) = if member.flags & 0x8 != 0 {
            (0, 0, 0)
        } else {
            (
                member.local_crc_override.unwrap_or(crc),
                compressed.len() as u32,
                size,
            )
        };
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&member.extract_version.to_le_bytes());
        out.extend_from_slice(&local_flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&local_crc.to_le_bytes());
        out.extend_from_slice(&local_csize.to_le_bytes());
        out.extend_from_slice(&local_size.to_le_bytes());
        out.extend_from_slice(&(local_name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&local_name);
        out.extend_from_slice(&compressed);
        if let Some(descriptor) = &member.data_descriptor {
            out.extend_from_slice(descriptor);
        }
        out.extend_from_slice(&member.trailing_garbage);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes());
        central.extend_from_slice(&member.extract_version.to_le_bytes());
        central.extend_from_slice(&member.flags.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(member.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&(member.central_extra.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&member.external_attr.to_le_bytes());
        central.extend_from_slice(
            &member
                .header_offset_override
                .unwrap_or(offset)
                .to_le_bytes(),
        );
        central.extend_from_slice(&member.name);
        central.extend_from_slice(&member.central_extra);
    }
    let central_offset = (out.len() - base) as u32;
    out.extend_from_slice(&central);
    let count = options.count_override.unwrap_or(members.len() as u16);
    let central_size = (central.len() as i64 + options.central_size_delta) as u32;
    let mut end = Vec::new();
    end.extend_from_slice(b"PK\x05\x06");
    end.extend_from_slice(&0u16.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&central_size.to_le_bytes());
    end.extend_from_slice(&central_offset.to_le_bytes());
    end.extend_from_slice(&(options.comment.len() as u16).to_le_bytes());
    end.extend_from_slice(&options.comment);
    if options.duplicate_end_record {
        // A second complete end record embedded in the comment of the first.
        let mut first = end.clone();
        let mut comment = end.clone();
        comment.truncate(22);
        comment[20] = 0;
        comment[21] = 0;
        let len = comment.len() as u16;
        first.truncate(20);
        first.extend_from_slice(&len.to_le_bytes());
        first.extend_from_slice(&comment);
        end = first;
    }
    out.extend_from_slice(&end);
    out.extend_from_slice(&options.suffix);
    out
}

pub const CT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

pub const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

pub const DOCUMENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"#;

/// A minimal valid package built member-by-member.
pub fn minimal_members() -> Vec<RawMember> {
    vec![
        RawMember::new("[Content_Types].xml", CT_XML.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()),
    ]
}

pub fn minimal_package_with(document_xml: &str) -> Vec<u8> {
    let mut members = minimal_members();
    members[2] = RawMember::new("word/document.xml", document_xml.as_bytes());
    build_zip(&members)
}

/// A package whose main part relates to a settings part.
pub fn package_with_settings(document_xml: &str, settings_xml: &str) -> Vec<u8> {
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/></Types>",
    );
    let doc_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/></Relationships>";
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document_xml.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        RawMember::new("word/settings.xml", settings_xml.as_bytes()),
    ])
}
