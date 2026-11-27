//! A strict PDF reader for tests: classic xref files only. Written apart from
//! the library's writer so that a shared mistake cannot pass unnoticed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use flate2::read::ZlibDecoder;

pub type Dict = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Value>),
    Dict(Dict),
    Ref(u32, u16),
    Stream(Dict, Vec<u8>),
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

    pub fn dict(&self) -> Option<&Dict> {
        match self {
            Value::Dict(dict) | Value::Stream(dict, _) => Some(dict),
            _ => None,
        }
    }
}

/// One content stream operation. An inline image is `BI` with its
/// parameters and data as the two operands.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub operator: String,
    pub operands: Vec<Value>,
}

pub struct Pdf {
    pub bytes: Vec<u8>,
    pub version: String,
    /// The `startxref` offset: the newest xref section.
    pub xref_at: usize,
    /// `(object number, byte offset)` for every in-use object, all sections merged.
    pub xref: Vec<(u32, usize)>,
    pub trailer: Dict,
    pub objects: BTreeMap<u32, Value>,
}

impl Pdf {
    pub fn parse(bytes: &[u8]) -> Result<Pdf, String> {
        let version = header_version(bytes)?;
        let xref_at = startxref(bytes)?;
        let mut sections = Vec::new();
        let mut next = Some(xref_at);
        let mut seen = BTreeSet::new();
        while let Some(at) = next {
            if !seen.insert(at) {
                return Err(format!("/Prev loops back to {at}"));
            }
            let (entries, trailer) = xref_section(bytes, at)?;
            next = match trailer.get("Prev") {
                None => None,
                Some(Value::Int(prev)) => Some(usize::try_from(*prev).map_err(|_| "bad /Prev")?),
                Some(other) => return Err(format!("bad /Prev {other:?}")),
            };
            sections.push((entries, trailer));
        }
        let mut offsets = BTreeMap::new();
        for (entries, _) in sections.iter().rev() {
            for &(number, offset) in entries {
                match offset {
                    Some(offset) => offsets.insert(number, offset),
                    None => offsets.remove(&number),
                };
            }
        }
        let mut pdf = Pdf {
            bytes: bytes.to_vec(),
            version,
            xref_at,
            xref: offsets.into_iter().collect(),
            trailer: sections.swap_remove(0).1,
            objects: BTreeMap::new(),
        };
        for &(number, _) in &pdf.xref {
            let object = pdf.load(number)?;
            pdf.objects.insert(number, object);
        }
        Ok(pdf)
    }

    fn offset_of(&self, number: u32) -> Result<usize, String> {
        self.xref
            .iter()
            .find(|&&(n, _)| n == number)
            .map(|&(_, offset)| offset)
            .ok_or_else(|| format!("object {number} is not in the xref table"))
    }

    fn load(&self, number: u32) -> Result<Value, String> {
        let mut lexer = Lexer::new(&self.bytes, self.offset_of(number)?);
        let found = lexer.int()?;
        lexer.int()?;
        lexer.expect_keyword("obj")?;
        if found != i64::from(number) {
            return Err(format!("xref says object {number} but found {found}"));
        }
        let value = lexer.value()?;
        if !lexer.peek_keyword("stream") {
            lexer.expect_keyword("endobj")?;
            return Ok(value);
        }
        let Value::Dict(dict) = value else {
            return Err(format!("stream {number} has no dictionary"));
        };
        lexer.expect_keyword("stream")?;
        lexer
            .eol()
            .ok_or("stream keyword not followed by an end of line")?;
        let length = self.stream_length(&dict)?;
        let data = lexer.take(length)?.to_vec();
        lexer.eol();
        if !lexer.rest().starts_with(b"endstream") {
            return Err(format!(
                "stream {number}: /Length {length} does not end at endstream"
            ));
        }
        lexer.expect_keyword("endstream")?;
        lexer.expect_keyword("endobj")?;
        Ok(Value::Stream(dict, data))
    }

    fn stream_length(&self, dict: &Dict) -> Result<usize, String> {
        let length = match dict.get("Length").ok_or("stream without /Length")? {
            Value::Ref(number, _) => self.load(*number)?,
            direct => direct.clone(),
        };
        match length {
            Value::Int(length) => usize::try_from(length).map_err(|_| "negative /Length".into()),
            other => Err(format!("bad /Length {other:?}")),
        }
    }

    pub fn resolve<'a>(&'a self, value: &'a Value) -> &'a Value {
        match value {
            Value::Ref(number, _) => self.objects.get(number).unwrap_or(&Value::Null),
            other => other,
        }
    }

    pub fn get<'a>(&'a self, dict: &'a Dict, key: &str) -> &'a Value {
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

    pub fn catalog(&self) -> &Dict {
        self.get(&self.trailer, "Root")
            .dict()
            .expect("/Root dictionary")
    }

    /// Leaf page dictionaries in document order.
    pub fn pages(&self) -> Vec<&Dict> {
        let mut out = Vec::new();
        self.collect_pages(self.get(self.catalog(), "Pages"), &mut out);
        out
    }

    fn collect_pages<'a>(&'a self, node: &'a Value, out: &mut Vec<&'a Dict>) {
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

    /// A page attribute, inherited through `/Parent` when the page lacks it.
    pub fn page_attr<'a>(&'a self, page: &'a Dict, key: &str) -> &'a Value {
        let mut node = page;
        loop {
            if node.contains_key(key) {
                return self.get(node, key);
            }
            match self.get(node, "Parent").dict() {
                Some(parent) => node = parent,
                None => return &Value::Null,
            }
        }
    }

    pub fn media_boxes(&self) -> Vec<[f64; 4]> {
        self.pages()
            .into_iter()
            .map(|page| {
                let Value::Array(items) = self.page_attr(page, "MediaBox") else {
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
    pub fn info(&self) -> Result<BTreeMap<String, String>, String> {
        let Some(info) = self.get(&self.trailer, "Info").dict() else {
            return Ok(BTreeMap::new());
        };
        info.iter()
            .filter_map(|(key, value)| match self.resolve(value) {
                Value::Str(bytes) => Some(decode_text(bytes).map(|text| (key.clone(), text))),
                _ => None,
            })
            .collect()
    }
}

/// A text string: UTF-16BE after a BOM, otherwise PDFDocEncoding, of which
/// only the part that agrees with ASCII is accepted.
pub fn decode_text(bytes: &[u8]) -> Result<String, String> {
    if let [0xFE, 0xFF, rest @ ..] = bytes {
        if rest.len() % 2 == 1 {
            return Err("odd-length UTF-16 text".into());
        }
        let units: Vec<u16> = rest
            .chunks(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16(&units).map_err(|_| "invalid UTF-16 text".into());
    }
    bytes
        .iter()
        .map(|&byte| match byte {
            b' '..=b'~' | b'\t' | b'\n' | b'\r' => Ok(char::from(byte)),
            other => Err(format!("PDFDocEncoding byte {other:#04x} outside ASCII")),
        })
        .collect()
}

const OPERATORS: &[&str] = &[
    "b", "B", "b*", "B*", "BDC", "BMC", "BT", "BX", "c", "cm", "CS", "cs", "d", "d0", "d1", "Do",
    "DP", "EMC", "ET", "EX", "f", "F", "f*", "G", "g", "gs", "h", "i", "j", "J", "K", "k", "l",
    "m", "M", "MP", "n", "q", "Q", "re", "RG", "rg", "ri", "s", "S", "SC", "sc", "SCN", "scn",
    "sh", "T*", "Tc", "Td", "TD", "Tf", "Tj", "TJ", "TL", "Tm", "Tr", "Ts", "Tw", "Tz", "v", "w",
    "W", "W*", "y", "'", "\"",
];

/// Operations of a content stream; unknown operators, references and
/// exponents are errors.
pub fn tokenize_content(data: &[u8]) -> Result<Vec<Op>, String> {
    let mut lexer = Lexer::new(data, 0);
    lexer.content = true;
    let mut ops = Vec::new();
    let mut operands = Vec::new();
    while !lexer.at_end() {
        match lexer.value()? {
            Value::Operator(operator) if operator == "BI" => {
                if !operands.is_empty() {
                    return Err("operands before BI".into());
                }
                ops.push(Op {
                    operator,
                    operands: lexer.inline_image()?,
                });
            }
            Value::Operator(operator) if OPERATORS.contains(&operator.as_str()) => ops.push(Op {
                operator,
                operands: std::mem::take(&mut operands),
            }),
            Value::Operator(operator) => return Err(format!("unknown operator {operator:?}")),
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

type Entries = Vec<(u32, Option<usize>)>;

/// One xref section and its trailer; a free entry's offset is `None`.
fn xref_section(bytes: &[u8], at: usize) -> Result<(Entries, Dict), String> {
    let mut lexer = Lexer::new(bytes, at);
    lexer.expect_keyword("xref")?;
    let mut entries = Vec::new();
    while !lexer.peek_keyword("trailer") {
        let first = u32::try_from(lexer.int()?).map_err(|_| "bad subsection start")?;
        let count = u32::try_from(lexer.int()?).map_err(|_| "bad subsection count")?;
        lexer
            .eol()
            .ok_or("subsection header without an end of line")?;
        for number in first..first + count {
            let entry = lexer.take(20)?;
            let (offset, generation, in_use) = xref_entry(entry)
                .ok_or_else(|| format!("bad xref entry {:?}", String::from_utf8_lossy(entry)))?;
            if number == 0 && (in_use || generation != 65_535) {
                return Err("xref entry 0 is not free with generation 65535".into());
            }
            entries.push((number, in_use.then_some(offset)));
        }
    }
    lexer.expect_keyword("trailer")?;
    let Value::Dict(trailer) = lexer.value()? else {
        return Err("trailer is not a dictionary".into());
    };
    Ok((entries, trailer))
}

/// `\d{10} \d{5} [nf]` and a two-byte end of line (7.5.4).
fn xref_entry(entry: &[u8]) -> Option<(usize, u32, bool)> {
    let digits = |field: &[u8]| -> Option<usize> {
        if !field.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(field).ok()?.parse().ok()
    };
    let in_use = match entry[17] {
        b'n' => true,
        b'f' => false,
        _ => return None,
    };
    let eol = matches!(&entry[18..20], b" \n" | b" \r" | b"\r\n");
    if entry[10] != b' ' || entry[16] != b' ' || !eol {
        return None;
    }
    let generation = u32::try_from(digits(&entry[11..16])?).ok()?;
    Some((digits(&entry[..10])?, generation, in_use))
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C' | b'\0')
}

fn is_delimiter(byte: u8) -> bool {
    b"()<>[]{}/%".contains(&byte)
}

fn is_regular(byte: u8) -> bool {
    !is_space(byte) && !is_delimiter(byte)
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// Content stream syntax: no `N G R` references.
    content: bool,
}

impl<'a> Lexer<'a> {
    fn new(bytes: &'a [u8], pos: usize) -> Self {
        Lexer {
            bytes,
            pos,
            content: false,
        }
    }

    fn rest(&self) -> &'a [u8] {
        &self.bytes[self.pos.min(self.bytes.len())..]
    }

    fn at_end(&mut self) -> bool {
        self.skip_space();
        self.pos >= self.bytes.len()
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

    /// Consumes one end of line (CR LF, LF or CR).
    fn eol(&mut self) -> Option<()> {
        let len = match self.rest() {
            [b'\r', b'\n', ..] => 2,
            [b'\n' | b'\r', ..] => 1,
            _ => return None,
        };
        self.pos += len;
        Some(())
    }

    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.bytes.get(self.pos).is_some_and(|&b| is_regular(b)) {
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
        self.skip_space();
        match self.number()? {
            Value::Int(value) => Ok(value),
            other => Err(format!("expected an integer, found {other:?}")),
        }
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
            b'+' | b'-' | b'.' | b'0'..=b'9' if self.content => self.number(),
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
        if self.rest().first().is_some_and(u8::is_ascii_digit)
            && let Ok(Value::Int(generation)) = self.number()
        {
            self.skip_space();
            if self.word() == b"R" {
                let number = u32::try_from(number).map_err(|_| "bad object number")?;
                let generation = u16::try_from(generation).map_err(|_| "bad generation")?;
                return Ok(Value::Ref(number, generation));
            }
        }
        self.pos = saved;
        Ok(first)
    }

    /// `[+-]?\d*\.?\d*` with a digit, ended by a space or a delimiter.
    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        let token = self.word();
        let text = std::str::from_utf8(token).map_err(|_| "number not ASCII")?;
        let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
        let well_formed = unsigned.bytes().any(|b| b.is_ascii_digit())
            && unsigned.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && unsigned.bytes().filter(|&b| b == b'.').count() <= 1;
        if !well_formed {
            return Err(format!("bad number {text:?} at {start}"));
        }
        if let Ok(int) = text.parse::<i64>() {
            return Ok(Value::Int(int));
        }
        text.parse::<f64>()
            .map(Value::Real)
            .map_err(|_| format!("bad number {text:?} at {start}"))
    }

    fn dictionary(&mut self) -> Result<Value, String> {
        self.pos += 2;
        let dict = self.entries(|lexer| {
            lexer.skip_space();
            let end = lexer.rest().starts_with(b">>");
            if end {
                lexer.pos += 2;
            }
            end
        })?;
        Ok(Value::Dict(dict))
    }

    /// Key/value pairs until `end` consumes the closing token.
    fn entries(&mut self, mut end: impl FnMut(&mut Self) -> bool) -> Result<Dict, String> {
        let mut dict = Dict::new();
        while !end(self) {
            let Value::Name(key) = self.value()? else {
                return Err(format!("dictionary key is not a name at {}", self.pos));
            };
            let value = self.value()?;
            if dict.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate key /{key}"));
            }
        }
        Ok(dict)
    }

    /// After `BI`: parameters, `ID`, one space, data, then `EI` (8.9.7).
    fn inline_image(&mut self) -> Result<Vec<Value>, String> {
        let params = self.entries(|lexer| {
            let end = lexer.peek_keyword("ID");
            if end {
                lexer.word();
            }
            end
        })?;
        if !self.rest().first().copied().is_some_and(is_space) {
            return Err("ID not followed by a space".into());
        }
        self.pos += 1;
        let data = self.rest();
        let end = (1..data.len())
            .find(|&i| {
                is_space(data[i - 1])
                    && data[i..].starts_with(b"EI")
                    && data.get(i + 2).is_none_or(|&b| is_space(b))
            })
            .ok_or("inline image without EI")?;
        self.pos += end + 2;
        Ok(vec![
            Value::Dict(params),
            Value::Str(data[..end - 1].to_vec()),
        ])
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
                .map(|pair| (pair[0] << 4) | pair[1])
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
                let mut code = u32::from(byte - b'0');
                for _ in 0..2 {
                    match self.bytes.get(self.pos) {
                        Some(&digit @ b'0'..=b'7') => {
                            code = code * 8 + u32::from(digit - b'0');
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
            let byte = u8::from_str_radix(hex, 16).map_err(|_| format!("bad #{hex}"))?;
            if byte == 0 {
                return Err("NUL in a name".into());
            }
            out.push(byte);
            index += 3;
        } else {
            out.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "name is not UTF-8".into())
}
