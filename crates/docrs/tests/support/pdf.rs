//! A small PDF reader for tests: classic xref files only. Written apart from
//! the library's writer so that a shared mistake cannot pass unnoticed.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Read;

use flate2::read::ZlibDecoder;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Value>),
    Dict(BTreeMap<String, Value>),
    Ref(u32, u16),
    Stream(BTreeMap<String, Value>, Vec<u8>),
    Operator(String),
}

impl Value {
    pub fn number(&self) -> Option<f64> {
        match *self {
            Value::Int(value) => Some(value as f64),
            Value::Real(value) => Some(value),
            _ => None,
        }
    }

    pub fn dict(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Dict(dict) | Value::Stream(dict, _) => Some(dict),
            _ => None,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(value) => write!(f, "{value}"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Real(value) => write!(f, "{value}"),
            Value::Name(name) => write!(f, "/{name}"),
            Value::Str(bytes) if bytes.iter().all(|b| (b' '..=b'~').contains(b)) => {
                write!(f, "({})", String::from_utf8_lossy(bytes))
            }
            Value::Str(bytes) => {
                write!(f, "<")?;
                bytes.iter().try_for_each(|b| write!(f, "{b:02X}"))?;
                write!(f, ">")
            }
            Value::Array(items) => {
                let items: Vec<String> = items.iter().map(Value::to_string).collect();
                write!(f, "[{}]", items.join(" "))
            }
            Value::Dict(dict) => {
                write!(f, "<<")?;
                dict.iter()
                    .try_for_each(|(key, value)| write!(f, " /{key} {value}"))?;
                write!(f, " >>")
            }
            Value::Ref(number, generation) => write!(f, "{number} {generation} R"),
            Value::Stream(dict, data) => write!(
                f,
                "{} stream ({} bytes)",
                Value::Dict(dict.clone()),
                data.len()
            ),
            Value::Operator(operator) => write!(f, "{operator}"),
        }
    }
}

/// One content stream operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub operator: String,
    pub operands: Vec<Value>,
}

pub struct Pdf {
    pub bytes: Vec<u8>,
    pub version: String,
    pub xref_at: usize,
    /// `(object number, byte offset)` for every in-use xref entry.
    pub xref: Vec<(u32, usize)>,
    pub trailer: BTreeMap<String, Value>,
    pub objects: BTreeMap<u32, Value>,
}

impl Pdf {
    pub fn parse(bytes: &[u8]) -> Result<Pdf, String> {
        let version = header_version(bytes)?;
        let xref_at = startxref(bytes)?;
        let mut lexer = Lexer::at(bytes, xref_at);
        lexer.expect_keyword("xref")?;
        let mut xref = Vec::new();
        while let Some(Value::Int(first)) = lexer.peek_value()? {
            lexer.value()?;
            let count = lexer.int()?;
            lexer.skip_space();
            for index in 0..count {
                let entry = lexer.take(20)?;
                let entry = std::str::from_utf8(entry).map_err(|_| "xref entry not ASCII")?;
                let (offset, rest) = entry.split_at(10);
                if rest.as_bytes()[7] == b'n' {
                    let offset = offset
                        .parse()
                        .map_err(|_| format!("bad xref offset {offset:?}"))?;
                    xref.push(((first + index) as u32, offset));
                }
            }
        }
        lexer.expect_keyword("trailer")?;
        let Value::Dict(trailer) = lexer.value()? else {
            return Err("trailer is not a dictionary".into());
        };
        let mut pdf = Pdf {
            bytes: bytes.to_vec(),
            version,
            xref_at,
            xref,
            trailer,
            objects: BTreeMap::new(),
        };
        for &(number, offset) in &pdf.xref.clone() {
            let object = pdf.indirect_at(number, offset)?;
            pdf.objects.insert(number, object);
        }
        Ok(pdf)
    }

    fn indirect_at(&self, number: u32, offset: usize) -> Result<Value, String> {
        let mut lexer = Lexer::at(&self.bytes, offset);
        let found = lexer.int()?;
        lexer.int()?;
        lexer.expect_keyword("obj")?;
        if found != number as i64 {
            return Err(format!("xref says object {number} but found {found}"));
        }
        let value = lexer.value()?;
        if lexer.peek_keyword("stream") {
            let Value::Dict(dict) = value else {
                return Err(format!("stream {number} has no dictionary"));
            };
            lexer.expect_keyword("stream")?;
            lexer.eol()?;
            let length = self
                .resolve(&dict["Length"])
                .number()
                .ok_or("bad /Length")? as usize;
            let data = lexer.take(length)?.to_vec();
            lexer.expect_keyword("endstream")?;
            lexer.expect_keyword("endobj")?;
            return Ok(Value::Stream(dict, data));
        }
        lexer.expect_keyword("endobj")?;
        Ok(value)
    }

    /// A readable listing: header, objects, trailer, pages and `/Info`.
    pub fn dump(&self) -> String {
        let mut out = format!(
            "%PDF-{}  ({} bytes, xref at {})\n",
            self.version,
            self.bytes.len(),
            self.xref_at
        );
        for &(number, offset) in &self.xref {
            let object = &self.objects[&number];
            match object {
                Value::Stream(dict, raw) => {
                    let ops = self
                        .stream_data(object)
                        .and_then(|data| tokenize_content(&data));
                    out += &format!(
                        "{number} 0 obj @{offset}: stream {}, {} bytes -> {ops:?}\n",
                        Value::Dict(dict.clone()),
                        raw.len()
                    );
                }
                other => out += &format!("{number} 0 obj @{offset}: {other}\n"),
            }
        }
        out += &format!("trailer: {}\n", Value::Dict(self.trailer.clone()));
        for (index, media_box) in self.media_boxes().iter().enumerate() {
            out += &format!("page {}: MediaBox {media_box:?}\n", index + 1);
        }
        for (key, value) in self.info() {
            out += &format!("Info /{key}: {value}\n");
        }
        out
    }

    pub fn resolve<'a>(&'a self, value: &'a Value) -> &'a Value {
        match value {
            Value::Ref(number, _) => self.objects.get(number).unwrap_or(&Value::Null),
            other => other,
        }
    }

    pub fn get<'a>(&'a self, dict: &'a BTreeMap<String, Value>, key: &str) -> &'a Value {
        dict.get(key)
            .map_or(&Value::Null, |value| self.resolve(value))
    }

    /// A stream's data, inflated when its filter is `/FlateDecode`.
    pub fn stream_data(&self, value: &Value) -> Result<Vec<u8>, String> {
        let Value::Stream(dict, raw) = self.resolve(value) else {
            return Err("not a stream".into());
        };
        match self.get(dict, "Filter") {
            Value::Null => Ok(raw.clone()),
            Value::Name(filter) if filter == "FlateDecode" => {
                let mut out = Vec::new();
                ZlibDecoder::new(&raw[..])
                    .read_to_end(&mut out)
                    .map_err(|err| format!("inflate: {err}"))?;
                Ok(out)
            }
            other => Err(format!("unsupported filter {other:?}")),
        }
    }

    pub fn catalog(&self) -> &BTreeMap<String, Value> {
        self.get(&self.trailer, "Root")
            .dict()
            .expect("/Root dictionary")
    }

    /// Leaf page dictionaries in document order.
    pub fn pages(&self) -> Vec<&BTreeMap<String, Value>> {
        let mut out = Vec::new();
        self.collect_pages(self.get(self.catalog(), "Pages"), &mut out);
        out
    }

    fn collect_pages<'a>(&'a self, node: &'a Value, out: &mut Vec<&'a BTreeMap<String, Value>>) {
        let dict = node.dict().expect("page tree node");
        match self.get(dict, "Type") {
            Value::Name(kind) if kind == "Pages" => {
                let Value::Array(kids) = self.get(dict, "Kids") else {
                    panic!("/Kids is not an array");
                };
                for kid in kids {
                    self.collect_pages(self.resolve(kid), out);
                }
            }
            _ => out.push(dict),
        }
    }

    pub fn page_count(&self) -> i64 {
        let pages = self.get(self.catalog(), "Pages").dict().expect("/Pages");
        self.get(pages, "Count").number().expect("/Count") as i64
    }

    pub fn media_boxes(&self) -> Vec<[f64; 4]> {
        self.pages()
            .into_iter()
            .map(|page| {
                let Value::Array(items) = self.get(page, "MediaBox") else {
                    panic!("page without /MediaBox");
                };
                let numbers: Vec<f64> = items
                    .iter()
                    .map(|v| self.resolve(v).number().unwrap())
                    .collect();
                numbers.try_into().expect("four MediaBox numbers")
            })
            .collect()
    }

    /// `/Info` with every string value decoded as a text string.
    pub fn info(&self) -> BTreeMap<String, String> {
        let Some(info) = self.get(&self.trailer, "Info").dict() else {
            return BTreeMap::new();
        };
        info.iter()
            .filter_map(|(key, value)| match self.resolve(value) {
                Value::Str(bytes) => Some((key.clone(), decode_text(bytes))),
                _ => None,
            })
            .collect()
    }
}

/// UTF-16BE with a BOM, otherwise bytes as Latin-1 (PDFDocEncoding's ASCII range).
pub fn decode_text(bytes: &[u8]) -> String {
    match bytes {
        [0xFE, 0xFF, rest @ ..] => {
            let units: Vec<u16> = rest
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&units).expect("valid UTF-16")
        }
        _ => bytes.iter().map(|&b| b as char).collect(),
    }
}

pub fn tokenize_content(data: &[u8]) -> Result<Vec<Op>, String> {
    let mut lexer = Lexer::at(data, 0);
    let mut ops = Vec::new();
    let mut operands = Vec::new();
    while let Some(value) = lexer.peek_value()? {
        lexer.value()?;
        match value {
            Value::Operator(operator) => ops.push(Op {
                operator,
                operands: std::mem::take(&mut operands),
            }),
            operand => operands.push(operand),
        }
    }
    if !operands.is_empty() {
        return Err(format!("operands without an operator: {operands:?}"));
    }
    Ok(ops)
}

fn header_version(bytes: &[u8]) -> Result<String, String> {
    let line = bytes.split(|&b| b == b'\n').next().unwrap_or_default();
    let line = std::str::from_utf8(line).map_err(|_| "header not ASCII")?;
    line.strip_prefix("%PDF-")
        .map(str::to_string)
        .ok_or_else(|| format!("no %PDF- header: {line:?}"))
}

fn startxref(bytes: &[u8]) -> Result<usize, String> {
    let tail = &bytes[bytes.len().saturating_sub(64)..];
    let text = String::from_utf8_lossy(tail);
    let after = text.rsplit_once("startxref").ok_or("no startxref")?.1;
    let mut lines = after.split_whitespace();
    let offset = lines.next().ok_or("no xref offset")?;
    if lines.next() != Some("%%EOF") {
        return Err("file does not end with %%EOF".into());
    }
    offset
        .parse()
        .map_err(|_| format!("bad startxref {offset:?}"))
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C' | b'\0')
}

fn is_delimiter(byte: u8) -> bool {
    b"()<>[]{}/%".contains(&byte)
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn at(bytes: &'a [u8], pos: usize) -> Self {
        Lexer { bytes, pos }
    }

    fn skip_space(&mut self) {
        while let Some(&byte) = self.bytes.get(self.pos) {
            if is_space(byte) {
                self.pos += 1;
            } else if byte == b'%' {
                while self
                    .bytes
                    .get(self.pos)
                    .is_some_and(|&b| b != b'\n' && b != b'\r')
                {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.pos + count;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or("unexpected end of data")?;
        self.pos = end;
        Ok(slice)
    }

    fn eol(&mut self) -> Result<(), String> {
        match self.bytes.get(self.pos..self.pos + 2) {
            Some(b"\r\n") => self.pos += 2,
            _ if self.bytes.get(self.pos) == Some(&b'\n') => self.pos += 1,
            _ => return Err("stream keyword not followed by an end of line".into()),
        }
        Ok(())
    }

    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|&b| !is_space(b) && !is_delimiter(b))
        {
            self.pos += 1;
        }
        &self.bytes[start..self.pos]
    }

    fn peek_keyword(&mut self, keyword: &str) -> bool {
        self.skip_space();
        let saved = self.pos;
        let found = self.word() == keyword.as_bytes();
        self.pos = saved;
        found
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<(), String> {
        self.skip_space();
        let at = self.pos;
        let word = self.word();
        if word != keyword.as_bytes() {
            return Err(format!(
                "expected {keyword} at {at}, found {:?}",
                String::from_utf8_lossy(word)
            ));
        }
        Ok(())
    }

    fn int(&mut self) -> Result<i64, String> {
        match self.value()? {
            Value::Int(value) => Ok(value),
            other => Err(format!("expected an integer, found {other:?}")),
        }
    }

    fn peek_value(&mut self) -> Result<Option<Value>, String> {
        self.skip_space();
        if self.pos >= self.bytes.len() {
            return Ok(None);
        }
        let saved = self.pos;
        let value = self.value()?;
        self.pos = saved;
        Ok(Some(value))
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_space();
        let byte = *self.bytes.get(self.pos).ok_or("unexpected end of data")?;
        match byte {
            b'/' => {
                self.pos += 1;
                Ok(Value::Name(decode_name(self.word())?))
            }
            b'(' => self.literal_string(),
            b'<' if self.bytes.get(self.pos + 1) == Some(&b'<') => self.dictionary(),
            b'<' => self.hex_string(),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    if self.bytes.get(self.pos) == Some(&b']') {
                        self.pos += 1;
                        return Ok(Value::Array(items));
                    }
                    items.push(self.value()?);
                }
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.number_or_ref(),
            _ if is_delimiter(byte) => {
                Err(format!("unexpected {:?} at {}", byte as char, self.pos))
            }
            _ => {
                let word = String::from_utf8_lossy(self.word()).into_owned();
                Ok(match word.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "null" => Value::Null,
                    _ => Value::Operator(word),
                })
            }
        }
    }

    fn number_or_ref(&mut self) -> Result<Value, String> {
        let first = self.number()?;
        let Value::Int(number) = first else {
            return Ok(first);
        };
        let saved = self.pos;
        self.skip_space();
        if let Ok(Value::Int(generation)) = self.number() {
            self.skip_space();
            if self.word() == b"R" {
                return Ok(Value::Ref(number as u32, generation as u16));
            }
        }
        self.pos = saved;
        Ok(first)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| b"+-.0123456789".contains(b))
        {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        if let Ok(int) = text.parse::<i64>() {
            return Ok(Value::Int(int));
        }
        text.parse::<f64>()
            .map(Value::Real)
            .map_err(|_| format!("bad number {text:?} at {start}"))
    }

    fn dictionary(&mut self) -> Result<Value, String> {
        self.pos += 2;
        let mut dict = BTreeMap::new();
        loop {
            self.skip_space();
            if self.bytes.get(self.pos..self.pos + 2) == Some(b">>") {
                self.pos += 2;
                return Ok(Value::Dict(dict));
            }
            let Value::Name(key) = self.value()? else {
                return Err(format!("dictionary key is not a name at {}", self.pos));
            };
            let value = self.value()?;
            dict.insert(key, value);
        }
    }

    fn hex_string(&mut self) -> Result<Value, String> {
        self.pos += 1;
        let mut digits = Vec::new();
        loop {
            let byte = *self.bytes.get(self.pos).ok_or("unterminated hex string")?;
            self.pos += 1;
            match byte {
                b'>' => break,
                _ if is_space(byte) => {}
                _ => digits.push((byte as char).to_digit(16).ok_or("bad hex digit")? as u8),
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(0);
        }
        Ok(Value::Str(
            digits
                .chunks(2)
                .map(|pair| pair[0] << 4 | pair[1])
                .collect(),
        ))
    }

    fn literal_string(&mut self) -> Result<Value, String> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut depth = 1;
        loop {
            let byte = *self.bytes.get(self.pos).ok_or("unterminated string")?;
            self.pos += 1;
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Value::Str(out));
                    }
                }
                b'\\' => {
                    self.escape(&mut out)?;
                    continue;
                }
                b'\r' => {
                    if self.bytes.get(self.pos) == Some(&b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                    continue;
                }
                _ => {}
            }
            out.push(byte);
        }
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        let byte = *self.bytes.get(self.pos).ok_or("unterminated escape")?;
        self.pos += 1;
        match byte {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(b'\x08'),
            b'f' => out.push(b'\x0C'),
            b'0'..=b'7' => {
                let mut code = (byte - b'0') as u32;
                for _ in 0..2 {
                    match self.bytes.get(self.pos) {
                        Some(&digit @ b'0'..=b'7') => {
                            code = code * 8 + (digit - b'0') as u32;
                            self.pos += 1;
                        }
                        _ => break,
                    }
                }
                out.push(code as u8);
            }
            b'\n' => {}
            b'\r' => {
                if self.bytes.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
            }
            other => out.push(other),
        }
        Ok(())
    }
}

fn decode_name(raw: &[u8]) -> Result<String, String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'#' {
            let hex = raw
                .get(index + 1..index + 3)
                .ok_or("truncated #xx in name")?;
            let hex = std::str::from_utf8(hex).map_err(|_| "bad #xx")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| format!("bad #{hex}"))?);
            index += 3;
        } else {
            out.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "name is not UTF-8".into())
}
