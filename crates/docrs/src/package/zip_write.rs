//! Deterministic ZIP output (upstream `_deterministic_zip_bytes`).
//!
//! Members are written in the given order, deflated at the default level,
//! with a fixed 1980-01-01 00:00:00 timestamp, `0o644` permissions, no extra
//! fields and no data descriptors. For a fixed input and a pinned build the
//! bytes are identical across runs; they are not expected to match Python's
//! compressor output byte-for-byte. ZIP64 is not emitted: outputs that would
//! need it are refused as a resource limit.

use std::io::Write;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use crate::{Error, Limits, Result};

const DOS_DATE_1980_01_01: u16 = (1 << 5) | 1;
const VERSION_MADE_BY: u16 = (3 << 8) | 20;
const VERSION_NEEDED: u16 = 20;
const EXTERNAL_ATTR: u32 = 0o644 << 16;

/// Serialize `(name, bytes)` members into a ZIP archive.
pub fn write_archive<'a, I>(members: I, limits: &Limits) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    // Reserve the stored size up front (deflate rarely exceeds it), so the
    // output does not grow by doubling: a large package would otherwise hold
    // its old and new buffers at once.
    let members: Vec<(&str, &[u8])> = members.into_iter().collect();
    let estimate: usize = members
        .iter()
        .map(|(name, data)| data.len() + data.len() / 1000 + 2 * name.len() + 128)
        .sum();
    let mut out: Vec<u8> = Vec::with_capacity(estimate);
    let mut central: Vec<u8> = Vec::new();
    let mut count: u64 = 0;
    for (name, data) in members {
        let offset = out.len() as u64;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .map_err(|err| Error::internal("deflate", format!("deflate failed: {err}")))?;
        let compressed = encoder
            .finish()
            .map_err(|err| Error::internal("deflate", format!("deflate failed: {err}")))?;
        let crc = crc32fast::hash(data);
        let fits = |value: u64| value < 0xFFFF_FFFF;
        if !fits(data.len() as u64) || !fits(compressed.len() as u64) || !fits(offset) {
            return Err(Error::limit(
                "max_output_bytes",
                limits.max_output_bytes,
                offset + compressed.len() as u64,
            ));
        }
        let flags: u16 = if name.is_ascii() { 0 } else { 0x0800 };
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| {
            Error::invalid_argument(
                "member_name_too_long",
                format!("member name {name:?} is too long"),
            )
        })?;

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&compressed);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&VERSION_MADE_BY.to_le_bytes());
        central.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&8u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&EXTERNAL_ATTR.to_le_bytes());
        central.extend_from_slice(&(offset as u32).to_le_bytes());
        central.extend_from_slice(name_bytes);

        count += 1;
        let size = out.len() as u64 + central.len() as u64;
        Limits::check("max_output_bytes", limits.max_output_bytes, size)?;
    }
    if count >= 0xFFFF {
        return Err(Error::limit("max_output_bytes", 0xFFFE, count));
    }
    let central_offset = out.len() as u64;
    if central_offset >= 0xFFFF_FFFF || central.len() as u64 >= 0xFFFF_FFFF {
        return Err(Error::limit(
            "max_output_bytes",
            limits.max_output_bytes,
            central_offset,
        ));
    }
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(count as u16).to_le_bytes());
    out.extend_from_slice(&(count as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&(central_offset as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Limits::check(
        "max_output_bytes",
        limits.max_output_bytes,
        out.len() as u64,
    )?;
    Ok(out)
}
