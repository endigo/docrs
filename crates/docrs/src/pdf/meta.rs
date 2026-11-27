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
    pub(crate) fn dict(&self, producer: &str) -> Result<Dict> {
        let mut dict = Dict::new();
        let texts = [
            ("Title", &self.title),
            ("Author", &self.author),
            ("Subject", &self.subject),
            ("Keywords", &self.keywords),
        ];
        for (key, value) in texts {
            if let Some(value) = value {
                dict.set(key, text(value));
            }
        }
        for (key, value) in [("CreationDate", self.created), ("ModDate", self.modified)] {
            if let Some(at) = value {
                dict.set(key, text(&pdf_date(at)?));
            }
        }
        dict.set("Producer", text(producer));
        Ok(dict)
    }
}

/// The trailer's `/ID`: both halves are the first 16 bytes of SHA-256 over the body.
pub(crate) fn file_id(body: &[u8]) -> Object {
    let half = Sha256::digest(body)[..16].to_vec();
    Object::Array(vec![Object::Bytes(half.clone()), Object::Bytes(half)])
}

fn pdf_date(at: Timestamp) -> Result<String> {
    let [year, month, day, hour, minute, second] = at.utc_fields();
    if !(0..=9999).contains(&year) {
        return Err(Error::invalid_argument(
            "pdf_date_out_of_range",
            format!("year {year} is outside 0000-9999"),
        ));
    }
    Ok(format!(
        "D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_pdf_dates() {
        let at = Timestamp::parse("2026-11-27T08:09:10+08:00").unwrap();
        assert_eq!(pdf_date(at).unwrap(), "D:20261127000910Z");
        let first = Timestamp::from_utc(0, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(pdf_date(first).unwrap(), "D:00000101000000Z");
        for year in [-1, 10_000] {
            let far = Timestamp::from_utc(year, 1, 1, 0, 0, 0).unwrap();
            assert_eq!(pdf_date(far).unwrap_err().reason(), "pdf_date_out_of_range");
        }
    }
}
