//! OPC part-name legality for ZIP member names (upstream
//! `_zipguard._validate_member_name`).

use crate::unicode::casefold;
use crate::{Error, Result};

const URI_UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
const SUB_DELIMS_ETC: &[u8] = b"!$&'()*+,;=:@/";

fn is_path_literal(byte: u8) -> bool {
    URI_UNRESERVED.contains(&byte) || SUB_DELIMS_ETC.contains(&byte)
}

fn malformed(message: String) -> Error {
    Error::malformed("zip_illegal_part_name", message)
}

/// Refuse a member name that cannot denote exactly one OPC part.
pub fn validate_member_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(malformed("ZIP contains an empty member name".into()));
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains('\\') {
        return Err(malformed(format!(
            "ZIP member name {name:?} is noncanonical"
        )));
    }
    if name.contains('?') || name.contains('#') {
        return Err(malformed(format!(
            "ZIP member name {name:?} contains a URI query or fragment"
        )));
    }
    if name.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7F) {
        return Err(malformed(format!(
            "ZIP member name {name:?} contains a control character"
        )));
    }
    let segments: Vec<&str> = name.split('/').collect();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "" | "." | ".."))
    {
        return Err(malformed(format!(
            "ZIP member name {name:?} has a noncanonical path segment"
        )));
    }
    let mut first = segments[0].chars();
    if let (Some(drive), Some(':')) = (first.next(), first.next()) {
        // Python `str.isalpha()`; for the single leading char this is the
        // Unicode letter test, approximated by Rust's alphabetic property.
        if drive.is_alphabetic() {
            return Err(malformed(format!(
                "ZIP member name {name:?} has a drive-qualified path"
            )));
        }
    }
    if casefold(name) == "[content_types].xml" {
        // Reserved package item (case variants reach their own diagnosis).
        return Ok(());
    }
    let bytes = name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != b'%' {
            if byte > 0x7F {
                let c = name[index..].chars().next().unwrap_or('\u{FFFD}');
                return Err(malformed(format!(
                    "ZIP member name {name:?} contains the non-ASCII character {c:?}; an OPC part name \
                     may contain only ASCII characters legal in a URI path"
                )));
            }
            if !is_path_literal(byte) {
                return Err(malformed(format!(
                    "ZIP member name {name:?} contains the character {:?}, which is not legal in an OPC \
                     part name; percent-escape it or rename the part",
                    byte as char
                )));
            }
            index += 1;
            continue;
        }
        let hex = bytes.get(index + 1..index + 3);
        let valid = index + 2 < bytes.len()
            && hex.is_some_and(|h| {
                h.iter()
                    .all(|d| d.is_ascii_digit() || (b'A'..=b'F').contains(d))
            });
        if !valid {
            return Err(malformed(format!(
                "ZIP member name {name:?} has a noncanonical percent escape"
            )));
        }
        let value = u8::from_str_radix(&name[index + 1..index + 3], 16).unwrap_or(0);
        if URI_UNRESERVED.contains(&value)
            || matches!(value, 0x00 | 0x2F | 0x5C | 0x7F)
            || value < 0x20
        {
            return Err(malformed(format!(
                "ZIP member name {name:?} has an unsafe percent escape"
            )));
        }
        if value >= 0x80 {
            return Err(malformed(format!(
                "ZIP member name {name:?} percent-escapes the non-ASCII byte 0x{value:02X}; an OPC part \
                 name may escape only ASCII characters"
            )));
        }
        index += 3;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_member_name as check;

    #[test]
    fn accepts_ordinary_and_escaped_names() {
        for name in [
            "word/document.xml",
            "[Content_Types].xml",
            "word/media/my%20image.png",
            "customXml/item1.xml",
            "a/b:c@d!$&'()*+,;=.xml",
        ] {
            assert!(check(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn refuses_noncanonical_names() {
        for name in [
            "",
            "/word/document.xml",
            "word/",
            "word\\document.xml",
            "word/doc?.xml",
            "word/doc#1.xml",
            "word/\u{1}x.xml",
            "word//document.xml",
            "word/./document.xml",
            "word/../document.xml",
            "C:/document.xml",
            "word/résumé.xml",
            "word/a b.xml",
            "word/%2",
            "word/%2f.xml",
            "word/%41.xml",
            "word/%2F.xml",
            "word/%C3%A9.xml",
            "word/%00.xml",
        ] {
            assert!(check(name).is_err(), "{name:?}");
        }
    }
}
