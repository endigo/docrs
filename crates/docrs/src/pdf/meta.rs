//! The `/Info` dictionary and the file identifier.

use sha2::{Digest, Sha256};

use super::writer::{Dict, Object, text};
use crate::clock::Timestamp;
use crate::{Error, Result};

/// Document information. Dates come from the caller, never the clock.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub created: Option<Timestamp>,
    pub modified: Option<Timestamp>,
}

impl Info {
    pub(crate) fn dict(&self) -> Result<Dict> {
        let mut dict = Dict::new();
        let texts = [
            ("Title", &self.title),
            ("Author", &self.author),
            ("Subject", &self.subject),
            ("Keywords", &self.keywords),
        ];
        for (key, value) in texts {
            if let Some(value) = value {
                dict = dict.with(key, text(value));
            }
        }
        for (key, value) in [("CreationDate", self.created), ("ModDate", self.modified)] {
            if let Some(at) = value {
                dict = dict.with(key, text(&pdf_date(at)?));
            }
        }
        Ok(dict.with(
            "Producer",
            text(concat!("docrs ", env!("CARGO_PKG_VERSION"))),
        ))
    }
}

/// The trailer's `/ID`: both halves are the first 16 bytes of SHA-256 over the body.
pub(crate) fn file_id(body: &[u8]) -> Object {
    let half = Sha256::digest(body)[..16].to_vec();
    Object::Array(vec![Object::Bytes(half.clone()), Object::Bytes(half)])
}

fn pdf_date(at: Timestamp) -> Result<String> {
    let iso = at.to_xml();
    let digits: String = iso.chars().filter(char::is_ascii_digit).collect();
    if iso.starts_with('-') || digits.len() != 14 {
        return Err(Error::invalid_argument(
            "pdf_date_out_of_range",
            format!("{iso} has no four-digit year"),
        ));
    }
    Ok(format!("D:{digits}Z"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_pdf_dates() {
        let at = Timestamp::parse("2026-11-27T08:09:10+08:00").unwrap();
        assert_eq!(pdf_date(at).unwrap(), "D:20261127000910Z");
        let far = Timestamp::from_utc(10_000, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(pdf_date(far).unwrap_err().reason(), "pdf_date_out_of_range");
    }
}
