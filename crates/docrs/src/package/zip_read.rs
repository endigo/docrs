//! Unambiguous ZIP reads for OPC packages (upstream `_zipguard.py`).
//!
//! This is a self-contained central-directory parser rather than a wrapper
//! over a general ZIP crate: every structural rule is enforced here, so the
//! guarantee does not depend on what a third-party reader tolerates.
//!
//! Rules (all refusals are `malformed_package`):
//!
//! * exactly one end-of-central-directory record accounting for the file's
//!   end; consistent ZIP64 records; single-disk only; a central directory that
//!   is exactly one region whose record count matches;
//! * the archive begins at byte 0 (no prefixed bytes);
//! * member names are canonical OPC part names: no duplicates, no
//!   case-ambiguous pairs (Python `casefold`), no file/directory prefix
//!   collisions, URI-legal ASCII with canonical percent escapes;
//! * stored or deflated only, no encryption or patched data, regular files;
//! * local headers agree with the central directory (flags, method, name,
//!   CRC and sizes, data descriptors); no overlapping or trailing data;
//! * every member is inflated from raw bytes so the actual inflated length,
//!   deflate end-of-stream and CRC are checked even when metadata lies.
//!
//! Directory records (names ending in `/`) are validated as inert and then
//! ignored, as OPC readers do. `[Content_Types].xml` is inflated and validated
//! first so a broken content-types item is diagnosed before other members are
//! inflated.
//!
//! Budgets from [`Limits`] are applied to declared sizes before allocation and
//! to actual sizes while inflating.

use std::collections::{HashMap, HashSet};

use flate2::{Decompress, FlushDecompress, Status};

use super::content_types::parse_content_types;
use crate::unicode::casefold;
use crate::{Error, Limits, Result};

const LOCAL_HEADER_SIZE: usize = 30;
const CENTRAL_HEADER_SIZE: usize = 46;
const END_RECORD_SIZE: usize = 22;
const ZIP64_END_RECORD_SIZE: usize = 56;
const ZIP64_LOCATOR_SIZE: usize = 20;
const MAX_END_COMMENT: usize = 65_535;
const READ_CHUNK: usize = 64 * 1024;

const SIG_LOCAL: &[u8; 4] = b"PK\x03\x04";
const SIG_CENTRAL: &[u8; 4] = b"PK\x01\x02";
const SIG_END: &[u8; 4] = b"PK\x05\x06";
const SIG_ZIP64_END: &[u8; 4] = b"PK\x06\x06";
const SIG_ZIP64_LOCATOR: &[u8; 4] = b"PK\x06\x07";

const FLAG_ENCRYPTED: u16 = 0x0001;
const FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
const FLAG_PATCHED_DATA: u16 = 0x0020;
const FLAG_STRONG_ENCRYPTION: u16 = 0x0040;
const FLAG_UTF8_NAME: u16 = 0x0800;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;
/// CPython `zipfile.MAX_EXTRACT_VERSION`.
const MAX_EXTRACT_VERSION: u8 = 63;

pub(crate) const CONTENT_TYPES_NAME: &str = "[Content_Types].xml";

/// One validated package member.
#[derive(Debug, Clone)]
pub struct ZipMember {
    /// Member name (OPC part name without the leading slash).
    pub name: String,
    /// Inflated bytes.
    pub data: Vec<u8>,
}

/// A fully validated archive.
#[derive(Debug, Clone)]
pub struct ZipContents {
    /// File members in central-directory order.
    pub members: Vec<ZipMember>,
    /// Inert directory records, in central-directory order.
    pub directories: Vec<String>,
    /// What [`Tolerance`] set aside, one line each (empty when strict).
    pub set_aside: Vec<String>,
}

/// Package irregularities Word opens without complaint but the guarded
/// reader (like upstream) refuses. Every field defaults to `false`, which is
/// upstream's behaviour; enabling one is a port extension.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tolerance {
    /// Drop `[trash]/…` members: Office's scratch area for parts discarded
    /// by an incremental save. They are not OPC parts and nothing refers to
    /// them; their ZIP records are still fully validated before being dropped.
    pub office_trash: bool,
    /// Accept a directory entry whose record holds an empty compressed stream
    /// (declared size 0, at most 5 compressed bytes), as some ZIP libraries
    /// write.
    pub empty_directory_data: bool,
}

impl Tolerance {
    /// Both tolerances on.
    pub fn word() -> Tolerance {
        Tolerance {
            office_trash: true,
            empty_directory_data: true,
        }
    }
}

const OFFICE_TRASH_PREFIX: &str = "[trash]/";

impl ZipContents {
    /// Member bytes by exact name.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.members
            .iter()
            .find(|member| member.name == name)
            .map(|member| member.data.as_slice())
    }
}

#[derive(Debug, Clone)]
struct Entry {
    orig_name: String,
    /// Name after CPython's sanitization (NUL truncation, Unicode-path field).
    name: String,
    raw_name_crc: u32,
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u64,
    expanded: u64,
    header_offset: u64,
    external_attr: u32,
}

fn malformed(reason: &'static str, message: impl Into<String>) -> Error {
    Error::malformed(reason, message)
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(bytes)
}

/// Validate and inflate every member of `archive`.
pub fn read_archive(archive: &[u8], limits: &Limits) -> Result<ZipContents> {
    read_archive_with(archive, limits, Tolerance::default())
}

/// Every check of [`read_archive`], keeping no member data: each member is
/// inflated, checked and released in turn, so memory stays at one member
/// rather than a copy of the whole package.
pub fn validate_archive(archive: &[u8], limits: &Limits) -> Result<()> {
    read_archive_keeping(archive, limits, Tolerance::default(), false).map(|_| ())
}

/// [`read_archive`] with the given [`Tolerance`].
pub fn read_archive_with(
    archive: &[u8],
    limits: &Limits,
    tolerance: Tolerance,
) -> Result<ZipContents> {
    read_archive_keeping(archive, limits, tolerance, true)
}

fn read_archive_keeping(
    archive: &[u8],
    limits: &Limits,
    tolerance: Tolerance,
    keep: bool,
) -> Result<ZipContents> {
    let archive_size = archive.len() as u64;
    Limits::check("max_archive_bytes", limits.max_archive_bytes, archive_size)?;

    let (central_offset, central_size, total_entries) = preflight(archive, limits)?;

    // Word refuses a package that does not begin at the start of the file.
    // Runs after structural checks so a malformed archive keeps its own,
    // more specific diagnosis.
    let leading = &archive[..archive.len().min(4)];
    if leading != SIG_LOCAL && leading != SIG_END {
        return Err(malformed(
            "zip_prefixed_bytes",
            "the package does not begin with a ZIP local file header, so bytes precede the archive; \
             Word cannot open a .docx with anything in front of the package. Extract the archive \
             to a file of its own and open that",
        ));
    }
    let entries = parse_central_directory(archive, central_offset, central_size, total_entries)?;

    let mut members = Vec::new();
    let mut directories = Vec::new();
    let mut set_aside = Vec::new();
    for entry in entries {
        if entry.orig_name.ends_with('/') {
            let empty_stream = entry.method == METHOD_DEFLATED
                && entry.expanded == 0
                && entry.crc == 0
                && (1..=5).contains(&entry.compressed);
            if tolerance.empty_directory_data && empty_stream {
                set_aside.push(format!(
                    "ignored the empty compressed stream in ZIP directory entry {:?}",
                    entry.orig_name
                ));
                validate_directory_entry(&Entry {
                    compressed: 0,
                    ..entry.clone()
                })?;
            } else {
                validate_directory_entry(&entry)?;
            }
            directories.push(entry);
        } else {
            members.push(entry);
        }
    }
    let trash =
        |entry: &Entry| tolerance.office_trash && entry.orig_name.starts_with(OFFICE_TRASH_PREFIX);
    validate_metadata(&members, &directories, limits, &trash)?;
    let parts = read_members(
        archive,
        central_offset,
        &members,
        &directories,
        limits,
        keep,
    )?;
    let mut kept = Vec::with_capacity(parts.len());
    for (entry, part) in members.iter().zip(parts) {
        if trash(entry) {
            set_aside.push(format!(
                "dropped Office scratch member {:?}",
                entry.orig_name
            ));
        } else {
            kept.push(part);
        }
    }
    Ok(ZipContents {
        members: kept,
        directories: directories
            .into_iter()
            .map(|entry| entry.orig_name)
            .collect(),
        set_aside,
    })
}

/// End-record discovery and central-directory envelope checks.
fn preflight(archive: &[u8], limits: &Limits) -> Result<(u64, u64, u64)> {
    let archive_size = archive.len();
    let tail_size = archive_size.min(END_RECORD_SIZE + MAX_END_COMMENT);
    let tail_start = archive_size - tail_size;
    let tail = &archive[tail_start..];

    let mut candidates = Vec::new();
    let mut cursor = 0;
    while let Some(found) = find(&tail[cursor..], SIG_END) {
        let index = cursor + found;
        cursor = index + 1;
        if index + END_RECORD_SIZE > tail.len() {
            continue;
        }
        let comment_length = u16_at(tail, index + 20) as usize;
        if index + END_RECORD_SIZE + comment_length == tail.len() {
            candidates.push(tail_start + index);
        }
    }
    if candidates.is_empty() {
        return Err(malformed(
            "zip_missing_end_record",
            "the file does not end with an archive footer, so nothing marks where the package stops: \
             usually bytes were appended after the archive, or the file was truncated in transfer. \
             Word cannot open a .docx unless the archive is exactly the file. Re-saving the document \
             fixes an appended tail; a truncated file needs a fresh copy",
        ));
    }
    if candidates.len() != 1 {
        return Err(malformed(
            "zip_ambiguous_end_record",
            "ZIP package has ambiguous end-of-central-directory records",
        ));
    }
    let end_offset = candidates[0];
    let end = &archive[end_offset..end_offset + END_RECORD_SIZE];
    let disk_number = u16_at(end, 4);
    let central_disk = u16_at(end, 6);
    let mut disk_entries = u16_at(end, 8) as u64;
    let mut total_entries = u16_at(end, 10) as u64;
    let mut central_size = u32_at(end, 12) as u64;
    let mut central_offset = u32_at(end, 16) as u64;
    if disk_number != 0 || central_disk != 0 {
        return Err(malformed(
            "zip_multi_disk",
            "multi-disk ZIP packages are not supported",
        ));
    }

    let zip64_required = disk_entries == 0xFFFF
        || total_entries == 0xFFFF
        || central_size == 0xFFFF_FFFF
        || central_offset == 0xFFFF_FFFF;
    let mut central_end = end_offset as u64;
    if zip64_required {
        let (de, te, cs, co, ce) = read_zip64_end(archive, end_offset, end)?;
        disk_entries = de;
        total_entries = te;
        central_size = cs;
        central_offset = co;
        central_end = ce;
    }

    if disk_entries != total_entries {
        return Err(malformed(
            "zip_multi_disk",
            "ZIP central-directory counts disagree across disks",
        ));
    }
    if central_offset.checked_add(central_size) != Some(central_end) {
        return Err(malformed(
            "zip_central_directory_region",
            "ZIP central-directory offset and size do not identify one unambiguous region",
        ));
    }
    if total_entries == 0 && central_size != 0 {
        return Err(malformed(
            "zip_central_directory_region",
            "empty ZIP package has a non-empty central directory",
        ));
    }
    if total_entries > 0 && central_size < total_entries.saturating_mul(CENTRAL_HEADER_SIZE as u64)
    {
        return Err(malformed(
            "zip_central_directory_region",
            "ZIP central directory is too small for its member count",
        ));
    }
    Limits::check("max_entries", limits.max_entries, total_entries)?;

    // Record-structure scan (fixed fields only).
    let mut cursor = central_offset;
    let mut count = 0u64;
    while cursor < central_end {
        if cursor + CENTRAL_HEADER_SIZE as u64 > central_end {
            return Err(malformed(
                "zip_central_directory_record",
                "ZIP central directory ends inside a member record",
            ));
        }
        let at = cursor as usize;
        let header = &archive[at..at + CENTRAL_HEADER_SIZE];
        if &header[..4] != SIG_CENTRAL {
            return Err(malformed(
                "zip_central_directory_record",
                "ZIP central directory contains an unsupported record",
            ));
        }
        let name_length = u16_at(header, 28) as u64;
        let extra_length = u16_at(header, 30) as u64;
        let comment_length = u16_at(header, 32) as u64;
        if u16_at(header, 34) != 0 {
            return Err(malformed(
                "zip_multi_disk",
                "multi-disk ZIP member records are not supported",
            ));
        }
        let record_size = CENTRAL_HEADER_SIZE as u64 + name_length + extra_length + comment_length;
        if cursor + record_size > central_end {
            return Err(malformed(
                "zip_central_directory_record",
                "ZIP central-directory member record exceeds its region",
            ));
        }
        cursor += record_size;
        count += 1;
        if count > total_entries {
            return Err(malformed(
                "zip_central_directory_count",
                "ZIP central-directory member count exceeds its end record",
            ));
        }
    }
    if cursor != central_end || count != total_entries {
        return Err(malformed(
            "zip_central_directory_count",
            "ZIP central-directory member count disagrees with its end record",
        ));
    }
    Ok((central_offset, central_size, total_entries))
}

fn read_zip64_end(
    archive: &[u8],
    end_offset: usize,
    end: &[u8],
) -> Result<(u64, u64, u64, u64, u64)> {
    let missing = || malformed("zip64_locator", "ZIP64 package is missing its locator");
    let locator_offset = end_offset
        .checked_sub(ZIP64_LOCATOR_SIZE)
        .ok_or_else(missing)?;
    let locator = &archive[locator_offset..end_offset];
    if &locator[..4] != SIG_ZIP64_LOCATOR {
        return Err(missing());
    }
    let record_disk = u32_at(locator, 4);
    let record_offset = u64_at(locator, 8);
    let disk_count = u32_at(locator, 16);
    if record_disk != 0 || disk_count != 1 {
        return Err(malformed(
            "zip_multi_disk",
            "multi-disk ZIP64 packages are not supported",
        ));
    }
    if record_offset.checked_add(ZIP64_END_RECORD_SIZE as u64) != Some(locator_offset as u64) {
        return Err(malformed(
            "zip64_end_record",
            "ZIP64 end record has an ambiguous offset or size",
        ));
    }
    let at = record_offset as usize;
    let record = &archive[at..at + ZIP64_END_RECORD_SIZE];
    if &record[..4] != SIG_ZIP64_END || u64_at(record, 4) != 44 {
        return Err(malformed(
            "zip64_end_record",
            "ZIP64 end record has an unsupported or ambiguous shape",
        ));
    }
    let disk_number = u32_at(record, 16);
    let central_disk = u32_at(record, 20);
    let disk_entries = u64_at(record, 24);
    let total_entries = u64_at(record, 32);
    let central_size = u64_at(record, 40);
    let central_offset = u64_at(record, 48);
    if disk_number != 0 || central_disk != 0 || disk_entries != total_entries {
        return Err(malformed(
            "zip_multi_disk",
            "multi-disk ZIP64 packages are not supported",
        ));
    }
    let legacy = |legacy: u64, actual: u64, sentinel: u64, label: &str| -> Result<()> {
        if legacy != sentinel && legacy != actual {
            return Err(malformed(
                "zip64_legacy_mismatch",
                format!("ZIP64 {label} disagrees with the legacy end record"),
            ));
        }
        Ok(())
    };
    legacy(u16_at(end, 8) as u64, disk_entries, 0xFFFF, "member count")?;
    legacy(
        u16_at(end, 10) as u64,
        total_entries,
        0xFFFF,
        "member count",
    )?;
    legacy(
        u32_at(end, 12) as u64,
        central_size,
        0xFFFF_FFFF,
        "central-directory size",
    )?;
    legacy(
        u32_at(end, 16) as u64,
        central_offset,
        0xFFFF_FFFF,
        "central-directory offset",
    )?;
    Ok((
        disk_entries,
        total_entries,
        central_size,
        central_offset,
        record_offset,
    ))
}

fn parse_central_directory(
    archive: &[u8],
    offset: u64,
    size: u64,
    count: u64,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::with_capacity(count.min(65_536) as usize);
    let mut cursor = offset as usize;
    let end = (offset + size) as usize;
    while cursor < end {
        let header = &archive[cursor..cursor + CENTRAL_HEADER_SIZE];
        let extract_version = header[6];
        let flags = u16_at(header, 8);
        let method = u16_at(header, 10);
        let crc = u32_at(header, 16);
        let compressed = u32_at(header, 20) as u64;
        let expanded = u32_at(header, 24) as u64;
        let name_length = u16_at(header, 28) as usize;
        let extra_length = u16_at(header, 30) as usize;
        let comment_length = u16_at(header, 32) as usize;
        let external_attr = u32_at(header, 38);
        let header_offset = u32_at(header, 42) as u64;
        let name_start = cursor + CENTRAL_HEADER_SIZE;
        let raw_name = &archive[name_start..name_start + name_length];
        let extra = &archive[name_start + name_length..name_start + name_length + extra_length];
        cursor = name_start + name_length + extra_length + comment_length;

        if extract_version > MAX_EXTRACT_VERSION {
            return Err(malformed(
                "zip_unsupported_version",
                format!(
                    "ZIP member requires extract version {:.1}",
                    extract_version as f64 / 10.0
                ),
            ));
        }
        let orig_name = decode_name(raw_name, flags).ok_or_else(|| {
            malformed(
                "zip_invalid_name_encoding",
                "ZIP member has an invalid encoded name",
            )
        })?;
        let mut entry = Entry {
            name: sanitize(&orig_name),
            orig_name,
            raw_name_crc: crc32fast::hash(raw_name),
            flags,
            method,
            crc,
            compressed,
            expanded,
            header_offset,
            external_attr,
        };
        decode_extra(&mut entry, extra)?;
        entries.push(entry);
    }
    Ok(entries)
}

/// CPython `ZipInfo._decodeExtra`: ZIP64 sizes and the Unicode path field.
fn decode_extra(entry: &mut Entry, mut extra: &[u8]) -> Result<()> {
    while extra.len() >= 4 {
        let kind = u16_at(extra, 0);
        let length = u16_at(extra, 2) as usize;
        if length + 4 > extra.len() {
            return Err(malformed(
                "zip_corrupt_extra_field",
                format!("Corrupt extra field {kind:04x} (size={length})"),
            ));
        }
        let mut data = &extra[4..4 + length];
        if kind == 0x0001 {
            let corrupt = |field: &str| {
                malformed(
                    "zip_corrupt_extra_field",
                    format!("Corrupt zip64 extra field. {field} not found."),
                )
            };
            if entry.expanded == 0xFFFF_FFFF {
                if data.len() < 8 {
                    return Err(corrupt("File size"));
                }
                entry.expanded = u64_at(data, 0);
                data = &data[8..];
            }
            if entry.compressed == 0xFFFF_FFFF {
                if data.len() < 8 {
                    return Err(corrupt("Compress size"));
                }
                entry.compressed = u64_at(data, 0);
                data = &data[8..];
            }
            if entry.header_offset == 0xFFFF_FFFF {
                if data.len() < 8 {
                    return Err(corrupt("Header offset"));
                }
                entry.header_offset = u64_at(data, 0);
            }
        } else if kind == 0x7075 {
            if data.len() < 5 {
                return Err(malformed(
                    "zip_corrupt_extra_field",
                    "Corrupt unicode path extra field (0x7075)",
                ));
            }
            let version = data[0];
            let name_crc = u32_at(data, 1);
            if version == 1 && name_crc == entry.raw_name_crc {
                let unicode = std::str::from_utf8(&data[5..]).map_err(|_| {
                    malformed(
                        "zip_corrupt_extra_field",
                        "Corrupt unicode path extra field (0x7075): invalid utf-8 bytes",
                    )
                })?;
                if !unicode.is_empty() {
                    entry.name = sanitize(unicode);
                }
            }
        }
        extra = &extra[4 + length..];
    }
    Ok(())
}

/// CPython `_sanitize_filename` on POSIX: truncate at the first NUL.
fn sanitize(name: &str) -> String {
    match name.find('\0') {
        Some(index) => name[..index].to_string(),
        None => name.to_string(),
    }
}

fn decode_name(raw: &[u8], flags: u16) -> Option<String> {
    if flags & FLAG_UTF8_NAME != 0 {
        return String::from_utf8(raw.to_vec()).ok();
    }
    Some(raw.iter().map(|byte| cp437(*byte)).collect())
}

fn cp437(byte: u8) -> char {
    const HIGH: [char; 128] = [
        'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ',
        'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú',
        'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡',
        '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟',
        '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘',
        '┌', '█', '▄', '▌', '▐', '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ',
        '∞', 'φ', 'ε', '∩', '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²',
        '■', '\u{a0}',
    ];
    if byte < 0x80 {
        byte as char
    } else {
        HIGH[(byte - 0x80) as usize]
    }
}

fn validate_directory_entry(entry: &Entry) -> Result<()> {
    if entry.name != entry.orig_name {
        return Err(malformed(
            "zip_noncanonical_name",
            format!(
                "ZIP directory entry {:?} contains a noncanonical NUL suffix",
                entry.orig_name
            ),
        ));
    }
    if entry.flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION | FLAG_PATCHED_DATA) != 0 {
        return Err(malformed(
            "zip_directory_flags",
            format!(
                "ZIP directory entry {:?} uses unsupported encoding flags",
                entry.orig_name
            ),
        ));
    }
    if entry.expanded != 0 || entry.compressed != 0 {
        return Err(malformed(
            "zip_directory_data",
            format!("ZIP directory entry {:?} carries data", entry.orig_name),
        ));
    }
    Ok(())
}

fn validate_metadata(
    members: &[Entry],
    directories: &[Entry],
    limits: &Limits,
    trash: &dyn Fn(&Entry) -> bool,
) -> Result<()> {
    // Directory-prefix collisions: a name cannot denote both a file and a
    // directory. Checked over the whole name set first so the diagnosis does
    // not depend on archive order.
    let names: HashSet<&str> = members
        .iter()
        .map(|entry| entry.orig_name.as_str())
        .collect();
    for entry in members {
        let name = entry.orig_name.as_str();
        for (index, _) in name.match_indices('/') {
            let prefix = &name[..index];
            if names.contains(prefix) {
                return Err(malformed(
                    "zip_directory_prefix_collision",
                    format!(
                        "ZIP member {prefix:?} is also a directory prefix of member {name:?}: one name \
                         cannot denote both a file and a directory. Remove the member named {prefix:?}."
                    ),
                ));
            }
        }
    }

    let mut seen_offsets: HashSet<u64> = HashSet::new();
    for entry in directories {
        if !seen_offsets.insert(entry.header_offset) {
            return Err(malformed(
                "zip_shared_offset",
                format!(
                    "ZIP directory entry {:?} has an invalid or shared local-header offset",
                    entry.orig_name
                ),
            ));
        }
    }
    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut seen_folded: HashSet<String> = HashSet::new();
    let mut declared_total: u64 = 0;
    for entry in members {
        let name = entry.orig_name.as_str();
        if entry.name != entry.orig_name {
            return Err(malformed(
                "zip_noncanonical_name",
                format!("ZIP member name {name:?} contains a noncanonical NUL suffix"),
            ));
        }
        if !trash(entry) {
            super::names::validate_member_name(name)?;
        }
        if !seen_names.insert(name) {
            return Err(malformed(
                "zip_duplicate_member",
                format!("ZIP contains duplicate member name {name:?}"),
            ));
        }
        if !seen_folded.insert(casefold(name)) {
            return Err(malformed(
                "zip_case_ambiguous_member",
                format!("ZIP contains case-ambiguous member name {name:?}"),
            ));
        }
        if !seen_offsets.insert(entry.header_offset) {
            return Err(malformed(
                "zip_shared_offset",
                format!("ZIP member {name:?} has an invalid or shared local-header offset"),
            ));
        }
        if entry.flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION) != 0 {
            return Err(malformed(
                "zip_encrypted_member",
                format!("ZIP member {name:?} is encrypted"),
            ));
        }
        if entry.flags & FLAG_PATCHED_DATA != 0 {
            return Err(malformed(
                "zip_patched_data",
                format!("ZIP member {name:?} uses unsupported patched-data encoding"),
            ));
        }
        if entry.method != METHOD_STORED && entry.method != METHOD_DEFLATED {
            return Err(malformed(
                "zip_unsupported_compression",
                format!(
                    "ZIP member {name:?} uses unsupported compression method {}",
                    entry.method
                ),
            ));
        }
        if entry.name.ends_with('/') || entry.external_attr & 0x10 != 0 {
            return Err(malformed(
                "zip_directory_member",
                format!("ZIP member {name:?} is a directory entry"),
            ));
        }
        let unix_mode = (entry.external_attr >> 16) & 0xFFFF;
        let file_type = unix_mode & 0o170000;
        if file_type != 0 && file_type != 0o100000 {
            return Err(malformed(
                "zip_unsupported_entry_type",
                format!("ZIP member {name:?} uses unsupported filesystem entry type"),
            ));
        }
        if entry.method == METHOD_STORED && entry.expanded != entry.compressed {
            return Err(malformed(
                "zip_inconsistent_sizes",
                format!("stored ZIP member {name:?} has inconsistent size metadata"),
            ));
        }
        Limits::check("max_part_bytes", limits.max_part_bytes, entry.expanded)?;
        declared_total = declared_total.saturating_add(entry.expanded);
        Limits::check(
            "max_total_part_bytes",
            limits.max_total_part_bytes,
            declared_total,
        )?;
    }
    Ok(())
}

fn read_members(
    archive: &[u8],
    start_dir: u64,
    members: &[Entry],
    directories: &[Entry],
    limits: &Limits,
    keep: bool,
) -> Result<Vec<ZipMember>> {
    if members.is_empty() && directories.is_empty() {
        return Ok(Vec::new());
    }
    if start_dir > archive.len() as u64 {
        return Err(malformed(
            "zip_central_directory_region",
            "ZIP central directory lies beyond the package",
        ));
    }
    let mut offsets: Vec<u64> = members
        .iter()
        .chain(directories.iter())
        .map(|entry| entry.header_offset)
        .collect();
    offsets.sort_unstable();
    let boundaries: HashMap<u64, u64> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            (
                *offset,
                offsets.get(index + 1).copied().unwrap_or(start_dir),
            )
        })
        .collect();

    for entry in directories {
        validate_local_header(archive, entry, boundaries[&entry.header_offset])?;
    }

    let mut actual_total: u64 = 0;
    let mut inflated: Vec<Option<Vec<u8>>> = vec![None; members.len()];
    let content_types = members
        .iter()
        .position(|entry| entry.name == CONTENT_TYPES_NAME);
    if let Some(index) = content_types {
        let entry = &members[index];
        let start = validate_local_header(archive, entry, boundaries[&entry.header_offset])?;
        let data = inflate_member(archive, entry, start, limits, &mut actual_total)?;
        parse_content_types(&data)?;
        inflated[index] = Some(if keep { data } else { Vec::new() });
    }
    for (index, entry) in members.iter().enumerate() {
        if Some(index) == content_types {
            continue;
        }
        let start = validate_local_header(archive, entry, boundaries[&entry.header_offset])?;
        let data = inflate_member(archive, entry, start, limits, &mut actual_total)?;
        inflated[index] = Some(if keep { data } else { Vec::new() });
    }
    Ok(members
        .iter()
        .zip(inflated)
        .map(|(entry, data)| ZipMember {
            name: entry.name.clone(),
            data: data.unwrap_or_default(),
        })
        .collect())
}

fn validate_local_header(archive: &[u8], entry: &Entry, boundary: u64) -> Result<u64> {
    let name = &entry.name;
    if entry.header_offset.saturating_add(LOCAL_HEADER_SIZE as u64) > boundary {
        return Err(malformed(
            "zip_overlapping_header",
            format!("ZIP member {name:?} has an overlapping header"),
        ));
    }
    let at = entry.header_offset as usize;
    if at + LOCAL_HEADER_SIZE > archive.len() {
        return Err(malformed(
            "zip_truncated_header",
            format!("ZIP member {name:?} has a truncated header"),
        ));
    }
    let header = &archive[at..at + LOCAL_HEADER_SIZE];
    if &header[..4] != SIG_LOCAL {
        return Err(malformed(
            "zip_invalid_local_header",
            format!("ZIP member {name:?} has an invalid header"),
        ));
    }
    let flags = u16_at(header, 6);
    let method = u16_at(header, 8);
    let crc = u32_at(header, 14);
    let compressed = u32_at(header, 18) as u64;
    let expanded = u32_at(header, 22) as u64;
    let name_length = u16_at(header, 26) as usize;
    let extra_length = u16_at(header, 28) as usize;
    if flags != entry.flags || method != entry.method {
        return Err(malformed(
            "zip_local_header_mismatch",
            format!("ZIP member {name:?} has inconsistent local-header metadata"),
        ));
    }
    let name_start = at + LOCAL_HEADER_SIZE;
    if name_start + name_length > archive.len() {
        return Err(malformed(
            "zip_truncated_header",
            format!("ZIP member {name:?} has a truncated name"),
        ));
    }
    let local_name = decode_name(&archive[name_start..name_start + name_length], flags)
        .ok_or_else(|| {
            malformed(
                "zip_invalid_name_encoding",
                format!("ZIP member {name:?} has an invalid encoded name"),
            )
        })?;
    if local_name != entry.orig_name {
        return Err(malformed(
            "zip_local_header_mismatch",
            format!("ZIP member {name:?} has conflicting local and central names"),
        ));
    }
    let (valid_crc, valid_compressed, valid_expanded) = if flags & FLAG_DATA_DESCRIPTOR != 0 {
        (
            crc == 0 || crc == entry.crc,
            compressed == 0 || compressed == entry.compressed || compressed == 0xFFFF_FFFF,
            expanded == 0 || expanded == entry.expanded || expanded == 0xFFFF_FFFF,
        )
    } else {
        (
            crc == entry.crc,
            compressed == entry.compressed || compressed == 0xFFFF_FFFF,
            expanded == entry.expanded || expanded == 0xFFFF_FFFF,
        )
    };
    if !(valid_crc && valid_compressed && valid_expanded) {
        return Err(malformed(
            "zip_local_header_mismatch",
            format!("ZIP member {name:?} has inconsistent local size or CRC metadata"),
        ));
    }
    let data_start =
        entry.header_offset + LOCAL_HEADER_SIZE as u64 + name_length as u64 + extra_length as u64;
    let data_end = data_start.saturating_add(entry.compressed);
    if data_start > boundary || data_end > boundary {
        return Err(malformed(
            "zip_overlapping_data",
            format!("ZIP member {name:?} overlaps another ZIP record"),
        ));
    }
    validate_data_descriptor(archive, entry, data_end, boundary)?;
    Ok(data_start)
}

fn validate_data_descriptor(
    archive: &[u8],
    entry: &Entry,
    data_end: u64,
    boundary: u64,
) -> Result<()> {
    let name = &entry.name;
    let descriptor_size = boundary - data_end;
    if entry.flags & FLAG_DATA_DESCRIPTOR == 0 {
        if descriptor_size != 0 {
            return Err(malformed(
                "zip_trailing_data",
                format!("ZIP member {name:?} has undeclared trailing data"),
            ));
        }
        return Ok(());
    }
    if !matches!(descriptor_size, 12 | 16 | 20 | 24) {
        return Err(malformed(
            "zip_data_descriptor",
            format!("ZIP member {name:?} has an invalid data descriptor"),
        ));
    }
    let at = data_end as usize;
    let mut descriptor = &archive[at..at + descriptor_size as usize];
    if matches!(descriptor_size, 16 | 24) {
        if &descriptor[..4] != b"PK\x07\x08" {
            return Err(malformed(
                "zip_data_descriptor",
                format!("ZIP member {name:?} has an invalid data descriptor"),
            ));
        }
        descriptor = &descriptor[4..];
    }
    let (crc, compressed, expanded) = if descriptor.len() == 12 {
        (
            u32_at(descriptor, 0),
            u32_at(descriptor, 4) as u64,
            u32_at(descriptor, 8) as u64,
        )
    } else {
        (
            u32_at(descriptor, 0),
            u64_at(descriptor, 4),
            u64_at(descriptor, 12),
        )
    };
    if (crc, compressed, expanded) != (entry.crc, entry.compressed, entry.expanded) {
        return Err(malformed(
            "zip_data_descriptor",
            format!("ZIP member {name:?} has inconsistent data-descriptor metadata"),
        ));
    }
    Ok(())
}

fn inflate_member(
    archive: &[u8],
    entry: &Entry,
    data_start: u64,
    limits: &Limits,
    total: &mut u64,
) -> Result<Vec<u8>> {
    let name = &entry.name;
    let start = data_start as usize;
    let raw = &archive[start..start + entry.compressed as usize];
    let mut out: Vec<u8> = Vec::with_capacity(entry.expanded.min(64 << 20) as usize);
    let mut hasher = crc32fast::Hasher::new();

    let mut consume = |chunk: &[u8], out: &mut Vec<u8>| -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let size = out.len() as u64 + chunk.len() as u64;
        if size > entry.expanded {
            return Err(malformed(
                "zip_inflates_past_declared_size",
                format!("ZIP member {name:?} inflates past its declared size"),
            ));
        }
        *total += chunk.len() as u64;
        Limits::check("max_total_part_bytes", limits.max_total_part_bytes, *total)?;
        hasher.update(chunk);
        out.extend_from_slice(chunk);
        Ok(())
    };

    if entry.method == METHOD_STORED {
        consume(raw, &mut out)?;
    } else {
        let mut decompressor = Decompress::new(false);
        let mut buffer = vec![0u8; READ_CHUNK];
        let mut position = 0usize;
        loop {
            let before_in = decompressor.total_in();
            let before_out = decompressor.total_out();
            let status = decompressor
                .decompress(&raw[position..], &mut buffer, FlushDecompress::None)
                .map_err(|_| {
                    malformed(
                        "zip_invalid_deflate",
                        format!("ZIP member {name:?} has invalid deflate data"),
                    )
                })?;
            let consumed = (decompressor.total_in() - before_in) as usize;
            let produced = (decompressor.total_out() - before_out) as usize;
            position += consumed;
            consume(&buffer[..produced], &mut out)?;
            if status == Status::StreamEnd {
                if position < raw.len() {
                    return Err(malformed(
                        "zip_trailing_compressed_data",
                        format!("ZIP member {name:?} has trailing compressed data"),
                    ));
                }
                break;
            }
            if consumed == 0 && produced == 0 {
                return Err(malformed(
                    "zip_truncated_deflate",
                    format!("ZIP member {name:?} compressed data ends before deflate EOF"),
                ));
            }
        }
    }
    if out.len() as u64 != entry.expanded {
        return Err(malformed(
            "zip_size_mismatch",
            format!(
                "ZIP member {name:?} actual expanded size {} bytes does not match declared size {} bytes",
                out.len(),
                entry.expanded
            ),
        ));
    }
    if hasher.finalize() != entry.crc {
        return Err(malformed(
            "zip_crc_mismatch",
            format!("ZIP member {name:?} fails its CRC check"),
        ));
    }
    Ok(out)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// True when the file ends with a structurally plausible end record (used by
/// diagnosis to tell a prefixed archive from a non-ZIP).
pub fn has_end_record(archive: &[u8]) -> bool {
    let window = archive.len().min(END_RECORD_SIZE + MAX_END_COMMENT);
    let tail = &archive[archive.len() - window..];
    let mut end = tail.len();
    while let Some(index) = tail[..end].windows(4).rposition(|w| w == SIG_END) {
        if index + END_RECORD_SIZE <= tail.len() {
            let comment = u16_at(tail, index + 20) as usize;
            if index + END_RECORD_SIZE + comment == tail.len() {
                return true;
            }
        }
        end = index + 3;
    }
    false
}
