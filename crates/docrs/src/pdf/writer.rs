//! A PDF 1.7 object serializer with a classic xref table, deterministic for a
//! given build.
//!
//! Each object is appended whole or not at all: after an error in an object
//! write (a budget exceeded, a bad value) the writer is unchanged and can go
//! on. [`Writer::finish`] consumes it.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::{Error, Result};

const HEADER: &[u8] = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";
const MAX_XREF_OFFSET: u64 = 9_999_999_999;
const HEX: &[u8; 16] = b"0123456789ABCDEF";
/// A whole real is written as an integer, and readers take integers up to
/// 2^31 − 1 (ISO 32000-1 Annex C).
const MAX_REAL: f64 = 2_147_483_647.0;

/// An indirect object number from [`Writer::alloc`]. There is one writer per
/// export, so a `Ref` is only ever used with the writer that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ref(u32);

/// A direct PDF object.
#[derive(Debug, Clone)]
pub(crate) enum Object {
    Bool(bool),
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
        self.set(key, value);
        self
    }

    /// Sets `key`, replacing an earlier value in place.
    pub(crate) fn set(&mut self, key: &str, value: impl Into<Object>) {
        let value = value.into();
        match self.0.iter_mut().find(|(existing, _)| existing == key) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// Adds a key the writer owns, refusing one the caller already set.
    fn reserve(mut self, key: &str, value: impl Into<Object>) -> Result<Self> {
        if self.0.iter().any(|(existing, _)| existing == key) {
            return Err(Error::internal(
                "pdf_reserved_key",
                format!("/{key} is written by the PDF writer"),
            ));
        }
        self.0.push((key.to_string(), value.into()));
        Ok(self)
    }
}

impl From<bool> for Object {
    fn from(value: bool) -> Self {
        Object::Bool(value)
    }
}

impl From<i64> for Object {
    fn from(value: i64) -> Self {
        Object::Int(value)
    }
}

impl From<i32> for Object {
    fn from(value: i32) -> Self {
        Object::Int(value.into())
    }
}

impl From<u32> for Object {
    fn from(value: u32) -> Self {
        Object::Int(value.into())
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

pub(crate) fn count(value: usize) -> Result<Object> {
    i64::try_from(value).map(Object::Int).map_err(|_| {
        Error::internal(
            "pdf_count_overflow",
            format!("{value} does not fit a PDF integer"),
        )
    })
}

/// Writes objects in any order, then the xref table and trailer.
pub(crate) struct Writer {
    out: Vec<u8>,
    offsets: Vec<Option<u64>>,
    max_bytes: u64,
}

impl Writer {
    pub(crate) fn new(max_bytes: u64) -> Result<Self> {
        let mut writer = Writer {
            out: Vec::new(),
            offsets: Vec::new(),
            max_bytes,
        };
        writer.put(&[HEADER])?;
        Ok(writer)
    }

    pub(crate) fn alloc(&mut self) -> Result<Ref> {
        let number = u32::try_from(self.offsets.len() + 1)
            .map_err(|_| Error::internal("pdf_too_many_objects", "object numbers exhausted"))?;
        self.offsets.push(None);
        Ok(Ref(number))
    }

    pub(crate) fn dict(&mut self, at: Ref, dict: Dict) -> Result<()> {
        self.object(at, &Object::Dict(dict))
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn array(&mut self, at: Ref, items: Vec<Object>) -> Result<()> {
        self.object(at, &Object::Array(items))
    }

    /// Writes `data` Flate-compressed, adding `/Filter` and `/Length`.
    pub(crate) fn stream(&mut self, at: Ref, dict: Dict, data: &[u8]) -> Result<()> {
        let mut stream = self.begin_stream(at, dict)?;
        stream.write(data)?;
        stream.finish(self)
    }

    /// A Flate stream written in pieces, compressed as they come. The writer
    /// stays free for other objects until [`StreamWriter::finish`]; the data
    /// is held to the budget left at this call, and the final write to the
    /// limit itself.
    pub(crate) fn begin_stream(&mut self, at: Ref, dict: Dict) -> Result<StreamWriter> {
        self.slot(at)?;
        let dict = dict.reserve("Filter", name("FlateDecode"))?;
        let encoder = ZlibEncoder::new(
            Bounded {
                out: Vec::new(),
                budget: self.remaining(),
            },
            Compression::new(6),
        );
        Ok(StreamWriter {
            at,
            dict,
            encoder,
            max_bytes: self.max_bytes,
        })
    }

    /// Writes `data` as it is. `Some(filter)` names its encoding; with `None`,
    /// a `/Filter` already in `dict` is kept, which is how to write a filter
    /// array.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn stream_raw(
        &mut self,
        at: Ref,
        dict: Dict,
        filter: Option<&str>,
        data: &[u8],
    ) -> Result<()> {
        let dict = match filter {
            Some("") => return Err(Error::internal("pdf_empty_filter", "a filter needs a name")),
            Some(filter) => dict.reserve("Filter", name(filter))?,
            None => dict,
        };
        self.stream_object(at, dict, data)
    }

    /// Everything written so far: the header and the objects.
    pub(crate) fn body(&self) -> &[u8] {
        &self.out
    }

    /// Writes the xref table and `trailer` (which gets `/Size`), then returns the file.
    pub(crate) fn finish(mut self, trailer: Dict) -> Result<Vec<u8>> {
        let trailer = trailer.reserve("Size", count(self.offsets.len() + 1)?)?;
        let xref_at = self.out.len();
        let mut buf = Vec::new();
        emit(
            &mut buf,
            format_args!("xref\n0 {}\n0000000000 65535 f \n", self.offsets.len() + 1),
        );
        for (index, offset) in self.offsets.iter().enumerate() {
            let offset = offset.ok_or_else(|| {
                Error::internal(
                    "pdf_object_unwritten",
                    format!("object {} never written", index + 1),
                )
            })?;
            emit(&mut buf, format_args!("{offset:010} 00000 n \n"));
        }
        buf.extend_from_slice(b"trailer\n");
        write_object(&mut buf, &Object::Dict(trailer))?;
        emit(&mut buf, format_args!("\nstartxref\n{xref_at}\n%%EOF\n"));
        self.put(&[&buf])?;
        Ok(self.out)
    }

    fn object(&mut self, at: Ref, object: &Object) -> Result<()> {
        let mut buf = Vec::new();
        emit(&mut buf, format_args!("{} 0 obj\n", at.0));
        write_object(&mut buf, object)?;
        buf.extend_from_slice(b"\nendobj\n");
        self.put_object(at, &[&buf])
    }

    fn stream_object(&mut self, at: Ref, dict: Dict, data: &[u8]) -> Result<()> {
        let dict = dict.reserve("Length", count(data.len())?)?;
        let mut head = Vec::new();
        emit(&mut head, format_args!("{} 0 obj\n", at.0));
        write_object(&mut head, &Object::Dict(dict))?;
        head.extend_from_slice(b"\nstream\n");
        self.put_object(at, &[&head, data, b"\nendstream\nendobj\n"])
    }

    fn put_object(&mut self, at: Ref, parts: &[&[u8]]) -> Result<()> {
        let offset = self.out.len() as u64;
        if offset > MAX_XREF_OFFSET {
            return Err(Error::internal(
                "pdf_offset_overflow",
                "object offset does not fit the xref table",
            ));
        }
        self.slot(at)?;
        self.put(parts)?;
        self.offsets[at.0 as usize - 1] = Some(offset);
        Ok(())
    }

    fn slot(&self, at: Ref) -> Result<()> {
        match self.offsets.get(at.0 as usize - 1) {
            None => Err(Error::internal(
                "pdf_ref_unknown",
                format!("object {} not allocated", at.0),
            )),
            Some(Some(_)) => Err(Error::internal(
                "pdf_object_twice",
                format!("object {} written twice", at.0),
            )),
            Some(None) => Ok(()),
        }
    }

    fn put(&mut self, parts: &[&[u8]]) -> Result<()> {
        let adding: u64 = parts.iter().map(|part| part.len() as u64).sum();
        if adding > self.remaining() {
            return Err(self.over_limit(adding));
        }
        for part in parts {
            self.out.extend_from_slice(part);
        }
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

/// An open Flate stream from [`Writer::begin_stream`].
pub(crate) struct StreamWriter {
    at: Ref,
    dict: Dict,
    encoder: ZlibEncoder<Bounded>,
    max_bytes: u64,
}

impl StreamWriter {
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.encoder.write_all(bytes).map_err(|_| self.over_limit())
    }

    /// Writes the stream object into `writer`, the one that began it.
    pub(crate) fn finish(self, writer: &mut Writer) -> Result<()> {
        let over_limit = self.over_limit();
        let data = self.encoder.finish().map_err(|_| over_limit)?.out;
        writer.stream_object(self.at, self.dict, &data)
    }

    fn over_limit(&self) -> Error {
        Error::limit(
            "max_output_bytes",
            self.max_bytes,
            self.max_bytes.saturating_add(1),
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
        Object::Bool(value) => emit(out, format_args!("{value}")),
        Object::Int(value) => emit(out, format_args!("{value}")),
        Object::Real(value) => write_number(out, *value)?,
        Object::Name(value) => write_name(out, value)?,
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
                write_name(out, key)?;
                out.push(b' ');
                write_object(out, value)?;
            }
            out.extend_from_slice(b" >>");
        }
        Object::Ref(at) => emit(out, format_args!("{} 0 R", at.0)),
    }
    Ok(())
}

fn emit(out: &mut Vec<u8>, args: std::fmt::Arguments) {
    // Writing to a Vec cannot fail.
    let _ = out.write_fmt(args);
}

/// Fixed notation with at most three decimals; never `-0` or an exponent.
pub(crate) fn write_number(out: &mut Vec<u8>, value: f64) -> Result<()> {
    if !value.is_finite() {
        return Err(Error::internal(
            "pdf_number_not_finite",
            format!("cannot write {value} as a PDF number"),
        ));
    }
    if value.abs() > MAX_REAL {
        return Err(Error::internal(
            "pdf_number_out_of_range",
            format!("{value} is beyond a PDF real"),
        ));
    }
    let start = out.len();
    emit(out, format_args!("{value:.3}"));
    while out.last() == Some(&b'0') {
        out.pop();
    }
    if out.last() == Some(&b'.') {
        out.pop();
    }
    if &out[start..] == b"-0" {
        out.truncate(start);
        out.push(b'0');
    }
    Ok(())
}

/// ISO 32000-1 7.3.5: bytes outside `!`..`~`, delimiters and `#` as `#xx`;
/// NUL is not allowed at all.
pub(crate) fn write_name(out: &mut Vec<u8>, value: &str) -> Result<()> {
    if value.as_bytes().contains(&0) {
        return Err(Error::internal(
            "pdf_name_nul",
            "a PDF name cannot hold NUL",
        ));
    }
    out.push(b'/');
    for &byte in value.as_bytes() {
        if (b'!'..=b'~').contains(&byte) && !b"()<>[]{}/%#".contains(&byte) {
            out.push(byte);
        } else {
            out.extend_from_slice(&[
                b'#',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 15)],
            ]);
        }
    }
    Ok(())
}

/// Literal only where PDFDocEncoding and ASCII agree; UTF-16BE otherwise.
fn write_text(out: &mut Vec<u8>, value: &str) {
    let literal = value
        .bytes()
        .all(|byte| matches!(byte, b' '..=b'~' | b'\t' | b'\n' | b'\r'));
    if !literal {
        let mut utf16 = vec![0xFE, 0xFF];
        utf16.extend(value.encode_utf16().flat_map(u16::to_be_bytes));
        return write_hex(out, &utf16);
    }
    out.push(b'(');
    for byte in value.bytes() {
        match byte {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', byte]),
            // A raw CR in a literal string is read back as LF (7.3.4.2).
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(byte),
        }
    }
    out.push(b')');
}

pub(crate) fn write_hex(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'<');
    for &byte in bytes {
        out.extend_from_slice(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 15)]]);
    }
    out.push(b'>');
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn serialized(object: Object) -> Result<String> {
        let mut out = Vec::new();
        write_object(&mut out, &object)?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn number(value: f64) -> Result<String> {
        serialized(Object::Real(value))
    }

    fn stream_bytes(file: &[u8]) -> (usize, &[u8]) {
        let text = String::from_utf8_lossy(file);
        let length = text
            .split("/Length ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap();
        let length: usize = length.parse().unwrap();
        let start = file.windows(8).position(|w| w == b"\nstream\n").unwrap() + 8;
        assert!(file[start + length..].starts_with(b"\nendstream\n"));
        (length, &file[start..start + length])
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect()
    }

    fn inflate(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(data)
            .read_to_end(&mut out)
            .unwrap();
        out
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
            (2_147_483_647.0, "2147483647"),
            (-2_147_483_646.999_6, "-2147483647"),
            (2.0004, "2"),
            (2.0015, "2.002"),
        ];
        for (value, expected) in cases {
            assert_eq!(number(value).unwrap(), expected, "{value}");
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(number(value).unwrap_err().reason(), "pdf_number_not_finite");
        }
        for value in [
            2_147_483_648.0,
            -2_147_483_648.0,
            9.3e18,
            f64::from(f32::MAX),
        ] {
            assert_eq!(
                number(value).unwrap_err().reason(),
                "pdf_number_out_of_range"
            );
        }
    }

    #[test]
    fn scalars_and_conversions() {
        let array = Object::from(vec![
            true.into(),
            false.into(),
            (-3_i32).into(),
            u32::MAX.into(),
            count(7).unwrap(),
        ]);
        assert_eq!(serialized(array).unwrap(), "[true false -3 4294967295 7]");
    }

    #[test]
    fn names_escape_delimiters_space_and_hash() {
        assert_eq!(
            serialized(name("A b#(c)/d")).unwrap(),
            "/A#20b#23#28c#29#2Fd"
        );
        assert_eq!(serialized(name("é\u{7f}")).unwrap(), "/#C3#A9#7F");
        assert_eq!(
            serialized(name("a\0b")).unwrap_err().reason(),
            "pdf_name_nul"
        );
    }

    #[test]
    fn strings_are_literal_only_where_pdfdoc_is_ascii() {
        assert_eq!(
            serialized(text("a(b)\\c\t\n\r")).unwrap(),
            "(a\\(b\\)\\\\c\t\n\\r)"
        );
        assert_eq!(serialized(text("é")).unwrap(), "<FEFF00E9>");
        assert_eq!(serialized(text("a\u{18}b")).unwrap(), "<FEFF006100180062>");
        assert_eq!(serialized(text("\u{7f}")).unwrap(), "<FEFF007F>");
    }

    #[test]
    fn dict_keys_are_unique() {
        let dict = Dict::new()
            .with("A", 1_i64)
            .with("B", 2_i64)
            .with("A", 3_i64);
        assert_eq!(serialized(dict.into()).unwrap(), "<< /A 3 /B 2 >>");
        let mut writer = Writer::new(u64::MAX).unwrap();
        let at = writer.alloc().unwrap();
        let filtered = Dict::new().with("Filter", name("DCTDecode"));
        let err = writer.begin_stream(at, filtered.clone()).err().unwrap();
        assert_eq!(err.reason(), "pdf_reserved_key");
        let err = writer
            .stream_raw(at, Dict::new(), Some(""), b"")
            .unwrap_err();
        assert_eq!(err.reason(), "pdf_empty_filter");
        let err = writer
            .stream_raw(at, filtered, Some("DCTDecode"), b"")
            .unwrap_err();
        assert_eq!(err.reason(), "pdf_reserved_key");
        let err = writer
            .stream_raw(at, Dict::new().with("Length", 0_i64), None, b"")
            .unwrap_err();
        assert_eq!(err.reason(), "pdf_reserved_key");
        writer.dict(at, Dict::new()).unwrap();
        let err = writer.finish(Dict::new().with("Size", 1_i64)).unwrap_err();
        assert_eq!(err.reason(), "pdf_reserved_key");
    }

    #[test]
    fn streams_record_the_compressed_length() {
        let data = b"0 0 m 100 100 l S\n".repeat(500);
        let mut writer = Writer::new(u64::MAX).unwrap();
        let at = writer.alloc().unwrap();
        writer.stream(at, Dict::new(), &data).unwrap();
        let file = writer.finish(Dict::new()).unwrap();
        let (length, compressed) = stream_bytes(&file);
        assert_eq!(compressed.len(), length);
        assert_eq!(inflate(compressed), data);
        assert!(length < data.len() / 10);
    }

    #[test]
    fn a_stream_written_in_pieces_equals_one_written_whole() {
        let data = b"BT /F1 12 Tf (Hello) Tj ET\n".repeat(300);
        let mut whole = Writer::new(u64::MAX).unwrap();
        let at = whole.alloc().unwrap();
        whole.stream(at, Dict::new(), &data).unwrap();
        let mut pieces = Writer::new(u64::MAX).unwrap();
        let at = pieces.alloc().unwrap();
        let mut stream = pieces.begin_stream(at, Dict::new()).unwrap();
        for chunk in data.chunks(7) {
            stream.write(chunk).unwrap();
        }
        stream.finish(&mut pieces).unwrap();
        assert_eq!(whole.body(), pieces.body());
    }

    #[test]
    fn streams_stay_open_while_other_objects_are_written() {
        let mut writer = Writer::new(u64::MAX).unwrap();
        let page = writer.alloc().unwrap();
        let mut content = writer.begin_stream(page, Dict::new()).unwrap();
        content.write(b"q ").unwrap();
        let image = writer.alloc().unwrap();
        writer
            .stream_raw(image, Dict::new(), Some("DCTDecode"), b"\xFF\xD8")
            .unwrap();
        let form = writer.alloc().unwrap();
        let mut header = writer.begin_stream(form, Dict::new()).unwrap();
        header.write(b"BT ET").unwrap();
        content.write(b"/Im1 Do Q").unwrap();
        header.finish(&mut writer).unwrap();
        content.finish(&mut writer).unwrap();
        let file = writer.finish(Dict::new()).unwrap();
        let text = String::from_utf8_lossy(&file);
        let order: Vec<usize> = ["2 0 obj", "3 0 obj", "1 0 obj"]
            .iter()
            .map(|header| text.find(header).unwrap())
            .collect();
        assert!(order.is_sorted());
    }

    #[test]
    fn an_open_stream_is_held_to_the_limit_when_it_finishes() {
        let mut writer = Writer::new(200).unwrap();
        let content = writer.alloc().unwrap();
        let filler = writer.alloc().unwrap();
        let mut stream = writer.begin_stream(content, Dict::new()).unwrap();
        stream.write(b"0 0 m 10 10 l S").unwrap();
        writer.array(filler, vec![text(&"x".repeat(100))]).unwrap();
        let before = writer.body().to_vec();
        let err = stream.finish(&mut writer).unwrap_err();
        assert_eq!(err.reason(), "max_output_bytes");
        assert_eq!(writer.body(), before);
    }

    #[test]
    fn content_stream_tokens() {
        let mut out = Vec::new();
        write_name(&mut out, "F1").unwrap();
        out.push(b' ');
        write_number(&mut out, 12.0).unwrap();
        out.extend_from_slice(b" Tf ");
        write_hex(&mut out, &[0x00, 0x41]);
        out.extend_from_slice(b" Tj");
        assert_eq!(out, b"/F1 12 Tf <0041> Tj");
    }

    #[test]
    fn raw_streams_keep_their_bytes() {
        let mut writer = Writer::new(u64::MAX).unwrap();
        let jpeg = writer.alloc().unwrap();
        let xml = writer.alloc().unwrap();
        writer
            .stream_raw(jpeg, Dict::new(), Some("DCTDecode"), b"\xFF\xD8jpeg")
            .unwrap();
        writer.stream_raw(xml, Dict::new(), None, b"<x/>").unwrap();
        let body = writer.body();
        let text = String::from_utf8_lossy(body);
        assert!(text.contains("1 0 obj\n<< /Filter /DCTDecode /Length 6 >>\nstream\n"));
        assert!(body.windows(6).any(|w| w == b"\xFF\xD8jpeg"));
        assert!(text.contains("2 0 obj\n<< /Length 4 >>\nstream\n<x/>\nendstream\nendobj\n"));
        let array = writer.alloc().unwrap();
        let filters = Dict::new().with("Filter", vec![name("ASCIIHexDecode"), name("DCTDecode")]);
        writer.stream_raw(array, filters, None, b"FFD8>").unwrap();
        let text = String::from_utf8_lossy(writer.body());
        assert!(text.contains("<< /Filter [/ASCIIHexDecode /DCTDecode] /Length 5 >>"));
    }

    #[test]
    fn arrays_are_written_as_objects() {
        let mut writer = Writer::new(u64::MAX).unwrap();
        let at = writer.alloc().unwrap();
        writer
            .array(at, vec![Object::Int(1), name("A"), at.into()])
            .unwrap();
        assert!(writer.body().ends_with(b"1 0 obj\n[1 /A 1 0 R]\nendobj\n"));
    }

    #[test]
    fn unwritten_and_twice_written_objects_are_internal_errors() {
        let mut writer = Writer::new(u64::MAX).unwrap();
        let first = writer.alloc().unwrap();
        writer.dict(first, Dict::new()).unwrap();
        let twice = writer.dict(first, Dict::new()).unwrap_err();
        assert_eq!(twice.reason(), "pdf_object_twice");
        let reopened = writer.begin_stream(first, Dict::new()).err().unwrap();
        assert_eq!(reopened.reason(), "pdf_object_twice");
        writer.alloc().unwrap();
        let unwritten = writer.finish(Dict::new()).unwrap_err();
        assert_eq!(unwritten.reason(), "pdf_object_unwritten");
    }

    #[test]
    fn a_failed_write_leaves_the_writer_usable() {
        let mut writer = Writer::new(400).unwrap();
        let at = writer.alloc().unwrap();
        let small = noise(4000);
        let before = writer.body().to_vec();
        let whole = writer.stream(at, Dict::new(), &small).unwrap_err();
        assert_eq!(whole.reason(), "max_output_bytes");
        let mut stream = writer.begin_stream(at, Dict::new()).unwrap();
        stream.write(&small).unwrap();
        let finished = stream.finish(&mut writer).unwrap_err();
        assert_eq!(finished.reason(), "max_output_bytes");
        let mut stream = writer.begin_stream(at, Dict::new()).unwrap();
        let failed = (0..100).find_map(|_| stream.write(&noise(100_000)).err());
        assert_eq!(failed.unwrap().reason(), "max_output_bytes");
        drop(stream);
        let long = text(&"x".repeat(500));
        assert_eq!(
            writer.array(at, vec![long]).unwrap_err().reason(),
            "max_output_bytes"
        );
        let nul = Dict::new().with("a\0", 1_i64);
        assert_eq!(writer.dict(at, nul).unwrap_err().reason(), "pdf_name_nul");
        assert_eq!(writer.body(), before);
        writer.dict(at, Dict::new()).unwrap();
        writer.finish(Dict::new()).unwrap();
    }
}
