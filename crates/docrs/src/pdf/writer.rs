//! A deterministic PDF 1.7 object serializer with a classic xref table.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::{Error, Result};

const HEADER: &[u8] = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";
const MAX_XREF_OFFSET: usize = 9_999_999_999;

/// An indirect object number allocated by [`Writer::alloc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ref(u32);

/// A direct PDF object.
#[derive(Debug, Clone)]
pub(crate) enum Object {
    Int(i64),
    Real(f64),
    Name(String),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<Object>),
    Dict(Dict),
    Ref(Ref),
}

/// A dictionary that keeps its keys in insertion order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dict(Vec<(String, Object)>);

impl Dict {
    pub(crate) fn new() -> Self {
        Dict::default()
    }

    pub(crate) fn with(mut self, key: &str, value: impl Into<Object>) -> Self {
        self.0.push((key.to_string(), value.into()));
        self
    }
}

impl From<i64> for Object {
    fn from(value: i64) -> Self {
        Object::Int(value)
    }
}

impl From<f64> for Object {
    fn from(value: f64) -> Self {
        Object::Real(value)
    }
}

impl From<Ref> for Object {
    fn from(value: Ref) -> Self {
        Object::Ref(value)
    }
}

impl From<Dict> for Object {
    fn from(value: Dict) -> Self {
        Object::Dict(value)
    }
}

impl From<Vec<Object>> for Object {
    fn from(value: Vec<Object>) -> Self {
        Object::Array(value)
    }
}

pub(crate) fn name(value: &str) -> Object {
    Object::Name(value.to_string())
}

pub(crate) fn text(value: &str) -> Object {
    Object::Text(value.to_string())
}

/// Writes objects in any order, then the xref table and trailer.
pub(crate) struct Writer {
    out: Vec<u8>,
    offsets: Vec<Option<usize>>,
    max_bytes: u64,
}

impl Writer {
    pub(crate) fn new(max_bytes: u64) -> Result<Self> {
        let mut writer = Writer {
            out: Vec::new(),
            offsets: Vec::new(),
            max_bytes,
        };
        writer.put(HEADER)?;
        Ok(writer)
    }

    pub(crate) fn alloc(&mut self) -> Ref {
        self.offsets.push(None);
        Ref(self.offsets.len() as u32)
    }

    pub(crate) fn dict(&mut self, at: Ref, dict: Dict) -> Result<()> {
        self.object(at, &Object::Dict(dict))
    }

    pub(crate) fn array(&mut self, at: Ref, items: Vec<Object>) -> Result<()> {
        self.object(at, &Object::Array(items))
    }

    /// Writes `data` Flate-compressed, adding `/Filter` and `/Length`.
    pub(crate) fn stream(&mut self, at: Ref, dict: Dict, data: &[u8]) -> Result<()> {
        let compressed = self.compress(data)?;
        let dict = dict
            .with("Filter", name("FlateDecode"))
            .with("Length", compressed.len() as i64);
        let mut buf = self.begin(at)?;
        write_object(&mut buf, &Object::Dict(dict))?;
        buf.extend_from_slice(b"\nstream\n");
        self.put(&buf)?;
        self.put(&compressed)?;
        self.put(b"\nendstream\nendobj\n")
    }

    /// Everything written so far: the header and the objects.
    pub(crate) fn body(&self) -> &[u8] {
        &self.out
    }

    /// Writes the xref table and `trailer` (which gets `/Size`), then returns the file.
    pub(crate) fn finish(mut self, trailer: Dict) -> Result<Vec<u8>> {
        let xref_at = self.out.len();
        let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", self.offsets.len() + 1);
        for (index, offset) in self.offsets.iter().enumerate() {
            let offset = offset.ok_or_else(|| {
                Error::internal(
                    "pdf_object_unwritten",
                    format!("object {} never written", index + 1),
                )
            })?;
            table.push_str(&format!("{offset:010} 00000 n \n"));
        }
        let mut buf = table.into_bytes();
        buf.extend_from_slice(b"trailer\n");
        let trailer = trailer.with("Size", self.offsets.len() as i64 + 1);
        write_object(&mut buf, &Object::Dict(trailer))?;
        buf.extend_from_slice(format!("\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
        self.put(&buf)?;
        Ok(self.out)
    }

    fn object(&mut self, at: Ref, object: &Object) -> Result<()> {
        let mut buf = self.begin(at)?;
        write_object(&mut buf, object)?;
        buf.extend_from_slice(b"\nendobj\n");
        self.put(&buf)
    }

    /// Records the object's offset and returns its `N 0 obj` line.
    fn begin(&mut self, at: Ref) -> Result<Vec<u8>> {
        let offset = self.out.len();
        if offset > MAX_XREF_OFFSET {
            return Err(Error::internal(
                "pdf_offset_overflow",
                "object offset does not fit the xref table",
            ));
        }
        let slot = self.offsets.get_mut(at.0 as usize - 1).ok_or_else(|| {
            Error::internal("pdf_ref_unknown", format!("object {} not allocated", at.0))
        })?;
        if slot.is_some() {
            return Err(Error::internal(
                "pdf_object_twice",
                format!("object {} written twice", at.0),
            ));
        }
        *slot = Some(offset);
        Ok(format!("{} 0 obj\n", at.0).into_bytes())
    }

    fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        let budget = self.remaining();
        let mut encoder = ZlibEncoder::new(
            Bounded {
                out: Vec::new(),
                budget,
            },
            Compression::new(6),
        );
        let flushed = encoder.write_all(data).and_then(|()| encoder.finish());
        match flushed {
            Ok(bounded) => Ok(bounded.out),
            Err(_) => Err(self.over_limit(budget.saturating_add(1))),
        }
    }

    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > self.remaining() {
            return Err(self.over_limit(bytes.len() as u64));
        }
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn remaining(&self) -> u64 {
        self.max_bytes.saturating_sub(self.out.len() as u64)
    }

    fn over_limit(&self, adding: u64) -> Error {
        Error::limit(
            "max_output_bytes",
            self.max_bytes,
            self.out.len() as u64 + adding,
        )
    }
}

/// Collects compressed bytes, failing once they pass the output budget.
struct Bounded {
    out: Vec<u8>,
    budget: u64,
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if (self.out.len() + buf.len()) as u64 > self.budget {
            return Err(std::io::Error::other("output budget exceeded"));
        }
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn write_object(out: &mut Vec<u8>, object: &Object) -> Result<()> {
    match object {
        Object::Int(value) => out.extend_from_slice(value.to_string().as_bytes()),
        Object::Real(value) => out.extend_from_slice(number(*value)?.as_bytes()),
        Object::Name(value) => write_name(out, value),
        Object::Text(value) => write_text(out, value),
        Object::Bytes(value) => write_hex(out, value),
        Object::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b' ');
                }
                write_object(out, item)?;
            }
            out.push(b']');
        }
        Object::Dict(Dict(entries)) => {
            out.extend_from_slice(b"<<");
            for (key, value) in entries {
                out.push(b' ');
                write_name(out, key);
                out.push(b' ');
                write_object(out, value)?;
            }
            out.extend_from_slice(b" >>");
        }
        Object::Ref(at) => out.extend_from_slice(format!("{} 0 R", at.0).as_bytes()),
    }
    Ok(())
}

/// Fixed notation with at most three decimals; never `-0` or an exponent.
fn number(value: f64) -> Result<String> {
    if !value.is_finite() {
        return Err(Error::internal(
            "pdf_number_not_finite",
            format!("cannot write {value} as a PDF number"),
        ));
    }
    let fixed = format!("{value:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    Ok(match trimmed {
        "-0" => "0".to_string(),
        other => other.to_string(),
    })
}

/// ISO 32000-1 7.3.5: bytes outside `!`..`~`, delimiters and `#` as `#xx`.
fn write_name(out: &mut Vec<u8>, value: &str) {
    out.push(b'/');
    for &byte in value.as_bytes() {
        let regular = (b'!'..=b'~').contains(&byte) && !b"()<>[]{}/%#".contains(&byte);
        if regular {
            out.push(byte);
        } else {
            out.extend_from_slice(format!("#{byte:02X}").as_bytes());
        }
    }
}

fn write_text(out: &mut Vec<u8>, value: &str) {
    if !value.is_ascii() {
        let mut utf16 = vec![0xFE, 0xFF];
        utf16.extend(value.encode_utf16().flat_map(u16::to_be_bytes));
        return write_hex(out, &utf16);
    }
    out.push(b'(');
    for &byte in value.as_bytes() {
        match byte {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', byte]),
            // A raw CR in a literal string is read back as LF (7.3.4.2).
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(byte),
        }
    }
    out.push(b')');
}

fn write_hex(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'<');
    for byte in bytes {
        out.extend_from_slice(format!("{byte:02X}").as_bytes());
    }
    out.push(b'>');
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn serialized(object: Object) -> String {
        let mut out = Vec::new();
        write_object(&mut out, &object).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn numbers_are_fixed_with_three_decimals_at_most() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (-0.0004, "0"),
            (1.0, "1"),
            (595.2756, "595.276"),
            (841.8898, "841.89"),
            (0.1 + 0.2, "0.3"),
            (-12.5, "-12.5"),
            (1e-7, "0"),
            (1e15, "1000000000000000"),
            (2.0004, "2"),
            (2.0015, "2.002"),
        ];
        for (value, expected) in cases {
            assert_eq!(number(value).unwrap(), expected, "{value}");
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(number(value).unwrap_err().code(), "internal");
        }
    }

    #[test]
    fn names_escape_delimiters_space_and_hash() {
        assert_eq!(serialized(name("A b#(c)/d")), "/A#20b#23#28c#29#2Fd");
        assert_eq!(serialized(name("é")), "/#C3#A9");
    }

    #[test]
    fn strings_are_literal_ascii_or_utf16_hex() {
        assert_eq!(serialized(text(r"a(b)\c")), r"(a\(b\)\\c)");
        assert_eq!(serialized(text("é")), "<FEFF00E9>");
    }

    #[test]
    fn streams_record_the_compressed_length() {
        let data = b"0 0 m 100 100 l S\n".repeat(500);
        let mut writer = Writer::new(u64::MAX).unwrap();
        let at = writer.alloc();
        writer.stream(at, Dict::new(), &data).unwrap();
        let out = writer.finish(Dict::new()).unwrap();
        let text = String::from_utf8_lossy(&out);
        let length: usize = text
            .split("/Length ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let start = out.windows(8).position(|w| w == b"\nstream\n").unwrap() + 8;
        let end = out.windows(11).position(|w| w == b"\nendstream\n").unwrap();
        assert_eq!(end - start, length);
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(&out[start..end])
            .read_to_end(&mut inflated)
            .unwrap();
        assert_eq!(inflated, data);
        assert!(length < data.len() / 10);
    }

    #[test]
    fn arrays_are_written_as_objects() {
        let mut writer = Writer::new(u64::MAX).unwrap();
        let at = writer.alloc();
        writer
            .array(at, vec![Object::Int(1), name("A"), at.into()])
            .unwrap();
        assert!(writer.body().ends_with(b"1 0 obj\n[1 /A 1 0 R]\nendobj\n"));
    }

    #[test]
    fn unwritten_and_twice_written_objects_are_internal_errors() {
        let mut writer = Writer::new(u64::MAX).unwrap();
        let first = writer.alloc();
        writer.dict(first, Dict::new()).unwrap();
        assert_eq!(
            writer.dict(first, Dict::new()).unwrap_err().reason(),
            "pdf_object_twice"
        );
        writer.alloc();
        assert_eq!(
            writer.finish(Dict::new()).unwrap_err().reason(),
            "pdf_object_unwritten"
        );
    }
}
