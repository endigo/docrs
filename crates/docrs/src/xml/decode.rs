//! Byte-to-text decoding for XML parts.
//!
//! Supported encodings: UTF-8 (with or without BOM), UTF-16 LE/BE (BOM or
//! byte-pattern autodetection, as libxml2 does), US-ASCII, ISO-8859-1 and
//! windows-1252. Any other declared encoding is refused rather than guessed.
//! A modified part is always re-serialized as UTF-8 with a matching
//! declaration.

use super::XmlError;

/// The encoding a part was read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEncoding {
    /// UTF-8, `bom` recording whether a byte-order mark was present.
    Utf8 {
        /// A UTF-8 BOM preceded the document.
        bom: bool,
    },
    /// UTF-16 little-endian.
    Utf16Le,
    /// UTF-16 big-endian.
    Utf16Be,
    /// 7-bit US-ASCII.
    Ascii,
    /// ISO-8859-1.
    Latin1,
    /// windows-1252.
    Windows1252,
}

/// Decode `bytes` to a `String` according to BOM, byte pattern and declaration.
pub fn decode(bytes: &[u8]) -> Result<(String, SourceEncoding), XmlError> {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return utf8(rest).map(|s| (s, SourceEncoding::Utf8 { bom: true }));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return utf16(rest, false).map(|s| (s, SourceEncoding::Utf16Le));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return utf16(rest, true).map(|s| (s, SourceEncoding::Utf16Be));
    }
    if bytes.starts_with(b"\x00\x00\xFE\xFF") || bytes.starts_with(b"\xFF\xFE\x00\x00") {
        return Err(XmlError::malformed(
            "unsupported_encoding",
            "UTF-32 XML is not supported",
        ));
    }
    if bytes.starts_with(b"<\x00?\x00") {
        return utf16(bytes, false).map(|s| (s, SourceEncoding::Utf16Le));
    }
    if bytes.starts_with(b"\x00<\x00?") {
        return utf16(bytes, true).map(|s| (s, SourceEncoding::Utf16Be));
    }

    let label = declared_encoding(bytes).map(|label| label.to_ascii_lowercase());
    match label.as_deref() {
        None | Some("utf-8") | Some("utf8") => {
            utf8(bytes).map(|s| (s, SourceEncoding::Utf8 { bom: false }))
        }
        Some("us-ascii") | Some("ascii") => {
            if let Some(position) = bytes.iter().position(|b| *b > 0x7F) {
                return Err(XmlError::malformed(
                    "invalid_encoding",
                    format!(
                        "byte 0x{:02X} at offset {position} is not US-ASCII",
                        bytes[position]
                    ),
                ));
            }
            utf8(bytes).map(|s| (s, SourceEncoding::Ascii))
        }
        Some("iso-8859-1") | Some("latin1") | Some("latin-1") | Some("iso_8859-1") | Some("l1") => {
            Ok((
                bytes.iter().map(|b| *b as char).collect(),
                SourceEncoding::Latin1,
            ))
        }
        Some("windows-1252") | Some("cp1252") => {
            let mut out = String::with_capacity(bytes.len());
            for (offset, byte) in bytes.iter().enumerate() {
                match windows_1252(*byte) {
                    Some(c) => out.push(c),
                    None => {
                        return Err(XmlError::malformed(
                            "invalid_encoding",
                            format!(
                                "byte 0x{byte:02X} at offset {offset} is undefined in windows-1252"
                            ),
                        ));
                    }
                }
            }
            Ok((out, SourceEncoding::Windows1252))
        }
        Some("utf-16") | Some("utf-16le") | Some("utf-16be") => Err(XmlError::malformed(
            "invalid_encoding",
            "the declaration names UTF-16 but the bytes are not UTF-16",
        )),
        Some(other) => Err(XmlError::malformed(
            "unsupported_encoding",
            format!("XML encoding {other:?} is not supported"),
        )),
    }
}

fn utf8(bytes: &[u8]) -> Result<String, XmlError> {
    String::from_utf8(bytes.to_vec()).map_err(|err| {
        XmlError::malformed(
            "invalid_encoding",
            format!(
                "invalid UTF-8 at byte offset {}",
                err.utf8_error().valid_up_to()
            ),
        )
    })
}

fn utf16(bytes: &[u8], big_endian: bool) -> Result<String, XmlError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(XmlError::malformed(
            "invalid_encoding",
            "UTF-16 data has an odd byte length",
        ));
    }
    let units = bytes.as_chunks::<2>().0.iter().map(|&pair| {
        if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        }
    });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|_| XmlError::malformed("invalid_encoding", "invalid UTF-16 surrogate sequence"))
}

/// The `encoding` pseudo-attribute of an ASCII-compatible XML declaration.
fn declared_encoding(bytes: &[u8]) -> Option<&str> {
    let rest = bytes.strip_prefix(b"<?xml")?;
    if !matches!(rest.first(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        return None;
    }
    let end = rest.windows(2).position(|w| w == b"?>")?;
    let decl = std::str::from_utf8(&rest[..end]).ok()?;
    let index = decl.find("encoding")?;
    let after = decl[index + "encoding".len()..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &after[1..];
    let close = value.find(quote)?;
    Some(&value[..close])
}

fn windows_1252(byte: u8) -> Option<char> {
    const HIGH: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    match byte {
        0x80..=0x9F => {
            let code = HIGH[(byte - 0x80) as usize];
            if code == 0 {
                None
            } else {
                char::from_u32(code as u32)
            }
        }
        _ => Some(byte as char),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_utf16_with_and_without_bom() {
        let text = "<?xml version='1.0' encoding='UTF-16'?><b>é</b>";
        let le: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut bom = vec![0xFF, 0xFE];
        bom.extend(&le);
        assert_eq!(
            decode(&bom).unwrap(),
            (text.to_string(), SourceEncoding::Utf16Le)
        );
        assert_eq!(
            decode(&le).unwrap(),
            (text.to_string(), SourceEncoding::Utf16Le)
        );
        let be: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(decode(&be).unwrap().1, SourceEncoding::Utf16Be);
    }

    #[test]
    fn legacy_single_byte_encodings() {
        let latin1 = b"<?xml version='1.0' encoding='ISO-8859-1'?><b>\xE9</b>";
        assert!(decode(latin1).unwrap().0.contains('é'));
        let cp1252 = b"<?xml version='1.0' encoding='windows-1252'?><b>\x80</b>";
        assert!(decode(cp1252).unwrap().0.contains('€'));
        assert!(decode(b"<?xml version='1.0' encoding='windows-1252'?><b>\x81</b>").is_err());
        assert!(decode(b"<?xml version='1.0' encoding='EBCDIC'?><b/>").is_err());
        assert!(decode(b"<b>\xFF</b>").is_err());
    }
}
