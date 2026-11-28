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

/// An in-use xref entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub number: u32,
    pub generation: u16,
    pub offset: usize,
}

pub struct Pdf {
    pub bytes: Vec<u8>,
    pub version: String,
    /// The `startxref` offset: the newest xref section.
    pub xref_at: usize,
    /// Every in-use object, all sections merged, by number.
    pub xref: Vec<Entry>,
    pub trailer: Dict,
    pub objects: BTreeMap<u32, Value>,
}

impl Pdf {
    pub fn parse(bytes: &[u8]) -> Result<Pdf, String> {
        let version = header_version(bytes)?;
        let xref_at = last_startxref(bytes)?;
        let mut sections = Vec::new();
        let mut next = Some(xref_at);
        let mut seen = BTreeSet::new();
        while let Some(at) = next {
            if !seen.insert(at) {
                return Err(format!("/Prev loops back to {at}"));
            }
            let section = xref_section(bytes, at, sections.is_empty())?;
            next = match section.trailer.get("Prev") {
                None => None,
                Some(Value::Int(prev)) => Some(usize::try_from(*prev).map_err(|_| "bad /Prev")?),
                Some(other) => return Err(format!("bad /Prev {other:?}")),
            };
            sections.push(section);
        }
        let mut entries = BTreeMap::new();
        for section in sections.iter().rev() {
            for &(number, entry) in &section.entries {
                entries.insert(number, entry);
            }
        }
        let trailer = sections.swap_remove(0).trailer;
        let highest = entries.keys().next_back().copied().unwrap_or(0);
        if trailer.get("Size") != Some(&Value::Int(i64::from(highest) + 1)) {
            return Err(format!(
                "/Size {:?} does not match the highest object {highest}",
                trailer.get("Size")
            ));
        }
        let mut pdf = Pdf {
            bytes: bytes.to_vec(),
            version,
            xref_at,
            xref: entries.into_values().flatten().collect(),
            trailer,
            objects: BTreeMap::new(),
        };
        for entry in pdf.xref.clone() {
            let object = pdf.load(entry, &mut BTreeSet::new())?;
            pdf.objects.insert(entry.number, object);
        }
        Ok(pdf)
    }

    fn entry(&self, number: u32) -> Result<Entry, String> {
        self.xref
            .iter()
            .find(|entry| entry.number == number)
            .copied()
            .ok_or_else(|| format!("object {number} is not in the xref table"))
    }

    fn load(&self, entry: Entry, loading: &mut BTreeSet<u32>) -> Result<Value, String> {
        let number = entry.number;
        if !loading.insert(number) {
            return Err(format!("object {number} refers to itself"));
        }
        let mut lexer = Lexer::new(&self.bytes, entry.offset);
        let found = (lexer.int()?, lexer.int()?);
        lexer.expect_keyword("obj")?;
        if found != (i64::from(number), i64::from(entry.generation)) {
            return Err(format!(
                "xref says object {number} {} but found {} {}",
                entry.generation, found.0, found.1
            ));
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
        if !lexer.rest().starts_with(b"\r\n") && !lexer.rest().starts_with(b"\n") {
            return Err("stream keyword not followed by CR LF or LF".into());
        }
        lexer.eol();
        let length = self.stream_length(&dict, loading)?;
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

    fn stream_length(&self, dict: &Dict, loading: &mut BTreeSet<u32>) -> Result<usize, String> {
        let length = match dict.get("Length").ok_or("stream without /Length")? {
            Value::Ref(number, _) => self.load(self.entry(*number)?, loading)?,
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

    /// A stream's dictionary and bytes as stored, whatever its filter.
    pub fn raw_stream<'a>(&'a self, value: &'a Value) -> Result<(&'a Dict, &'a [u8]), String> {
        match self.resolve(value) {
            Value::Stream(dict, raw) => Ok((dict, raw)),
            other => Err(format!("not a stream: {other:?}")),
        }
    }

    /// A stream's data, inflated when its filter is `/FlateDecode`.
    pub fn stream_data(&self, value: &Value) -> Result<Vec<u8>, String> {
        let (dict, raw) = self.raw_stream(value)?;
        match self.get(dict, "Filter") {
            Value::Null => Ok(raw.to_vec()),
            Value::Name(filter) if filter == "FlateDecode" => {
                let mut out = Vec::new();
                ZlibDecoder::new(raw)
                    .read_to_end(&mut out)
                    .map_err(|err| format!("inflate: {err}"))?;
                Ok(out)
            }
            other => Err(format!("unsupported filter {other:?}")),
        }
    }

    pub fn catalog(&self) -> Result<&Dict, String> {
        self.get(&self.trailer, "Root")
            .dict()
            .ok_or_else(|| "/Root is not a dictionary".into())
    }

    /// Leaf page dictionaries in document order; every `/Count` must match.
    pub fn pages(&self) -> Result<Vec<&Dict>, String> {
        let root = self
            .catalog()?
            .get("Pages")
            .ok_or("catalog without /Pages")?;
        let mut out = Vec::new();
        self.collect_pages(root, &mut BTreeSet::new(), &mut out)?;
        Ok(out)
    }

    fn collect_pages<'a>(
        &'a self,
        node: &'a Value,
        visited: &mut BTreeSet<u32>,
        out: &mut Vec<&'a Dict>,
    ) -> Result<(), String> {
        let Value::Ref(number, _) = node else {
            return Err(format!("page tree node is not a reference: {node:?}"));
        };
        if !visited.insert(*number) {
            return Err(format!("page tree visits object {number} twice"));
        }
        let dict = self
            .resolve(node)
            .dict()
            .ok_or_else(|| format!("page tree node {number} is not a dictionary"))?;
        match self.get(dict, "Type") {
            Value::Name(kind) if kind == "Pages" => {
                let Value::Array(kids) = self.get(dict, "Kids") else {
                    return Err(format!("/Kids of {number} is not an array"));
                };
                let before = out.len();
                for kid in kids {
                    self.collect_pages(kid, visited, out)?;
                }
                let leaves = (out.len() - before) as f64;
                if self.get(dict, "Count").number() != Some(leaves) {
                    return Err(format!("/Count of {number} is not its {leaves} pages"));
                }
            }
            Value::Name(kind) if kind == "Page" => out.push(dict),
            other => return Err(format!("page tree node {number} has /Type {other:?}")),
        }
        Ok(())
    }

    /// A page attribute, inherited through `/Parent` when the page lacks it.
    pub fn page_attr<'a>(&'a self, page: &'a Dict, key: &str) -> Result<&'a Value, String> {
        let mut node = page;
        let mut visited = BTreeSet::new();
        loop {
            if node.contains_key(key) {
                return Ok(self.get(node, key));
            }
            match node.get("Parent") {
                None => return Ok(&Value::Null),
                Some(Value::Ref(number, _)) if visited.insert(*number) => {
                    node = self
                        .objects
                        .get(number)
                        .and_then(Value::dict)
                        .ok_or_else(|| format!("/Parent {number} is not a dictionary"))?;
                }
                Some(other) => return Err(format!("bad or cyclic /Parent {other:?}")),
            }
        }
    }

    pub fn media_boxes(&self) -> Result<Vec<[f64; 4]>, String> {
        self.pages()?
            .into_iter()
            .map(|page| {
                let Value::Array(items) = self.page_attr(page, "MediaBox")? else {
                    return Err("page without /MediaBox".to_string());
                };
                let numbers: Option<Vec<f64>> =
                    items.iter().map(|v| self.resolve(v).number()).collect();
                numbers
                    .and_then(|numbers| numbers.try_into().ok())
                    .ok_or_else(|| format!("bad /MediaBox {items:?}"))
            })
            .collect()
    }

    /// The page (from 0) a reference names.
    pub fn page_index(&self, page: &Value) -> Result<usize, String> {
        let Value::Ref(number, _) = page else {
            return Err(format!("page is not a reference: {page:?}"));
        };
        let pages = self.pages()?;
        let target = self.objects.get(number).and_then(Value::dict);
        pages
            .iter()
            .position(|&dict| Some(dict) == target)
            .ok_or_else(|| format!("object {number} is not a page"))
    }

    /// A name tree's entries in order. Keys must ascend across the whole
    /// tree, and each kid's `/Limits` must hold its first and last key.
    pub fn name_tree<'a>(&'a self, root: &'a Value) -> Result<Vec<(Vec<u8>, &'a Value)>, String> {
        let mut out = Vec::new();
        self.collect_names(root, true, &mut BTreeSet::new(), &mut out)?;
        if out.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err("name tree keys are not strictly ascending".into());
        }
        Ok(out)
    }

    fn collect_names<'a>(
        &'a self,
        node: &'a Value,
        is_root: bool,
        visited: &mut BTreeSet<u32>,
        out: &mut Vec<(Vec<u8>, &'a Value)>,
    ) -> Result<(), String> {
        if let Value::Ref(number, _) = node
            && !visited.insert(*number)
        {
            return Err(format!("name tree visits object {number} twice"));
        }
        let dict = self
            .resolve(node)
            .dict()
            .ok_or("name tree node is not a dictionary")?;
        let before = out.len();
        if let Value::Array(names) = self.get(dict, "Names") {
            if names.len() % 2 != 0 {
                return Err("/Names has an odd length".into());
            }
            for pair in names.chunks(2) {
                let Value::Str(key) = self.resolve(&pair[0]) else {
                    return Err(format!("name tree key is not a string: {:?}", pair[0]));
                };
                out.push((key.clone(), self.resolve(&pair[1])));
            }
        }
        if let Value::Array(kids) = self.get(dict, "Kids") {
            for kid in kids {
                self.collect_names(kid, false, visited, out)?;
            }
        }
        match self.get(dict, "Limits") {
            Value::Null if is_root => {}
            Value::Array(limits) if !is_root => {
                let [Value::Str(first), Value::Str(last)] = limits.as_slice() else {
                    return Err(format!("bad /Limits {limits:?}"));
                };
                let keys = &out[before..];
                if keys.first().map(|k| &k.0) != Some(first)
                    || keys.last().map(|k| &k.0) != Some(last)
                {
                    return Err("/Limits are not the node's first and last keys".into());
                }
            }
            other => return Err(format!("/Limits {other:?} on a root: {is_root}")),
        }
        Ok(())
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

/// Operand shapes (ISO 32000-1 Annex A): `n` number, `N` name, `s` string,
/// `a` array, `p` name or dictionary; `*` one to four numbers, `+` numbers,
/// a name, or numbers and then a name.
const OPERATORS: &[(&str, &str)] = &[
    ("b", ""),
    ("B", ""),
    ("b*", ""),
    ("B*", ""),
    ("BDC", "Np"),
    ("BMC", "N"),
    ("BT", ""),
    ("BX", ""),
    ("c", "nnnnnn"),
    ("cm", "nnnnnn"),
    ("CS", "N"),
    ("cs", "N"),
    ("d", "an"),
    ("d0", "nn"),
    ("d1", "nnnnnn"),
    ("Do", "N"),
    ("DP", "Np"),
    ("EMC", ""),
    ("ET", ""),
    ("EX", ""),
    ("f", ""),
    ("F", ""),
    ("f*", ""),
    ("G", "n"),
    ("g", "n"),
    ("gs", "N"),
    ("h", ""),
    ("i", "n"),
    ("j", "n"),
    ("J", "n"),
    ("K", "nnnn"),
    ("k", "nnnn"),
    ("l", "nn"),
    ("m", "nn"),
    ("M", "n"),
    ("MP", "N"),
    ("n", ""),
    ("q", ""),
    ("Q", ""),
    ("re", "nnnn"),
    ("RG", "nnn"),
    ("rg", "nnn"),
    ("ri", "N"),
    ("s", ""),
    ("S", ""),
    ("SC", "*"),
    ("sc", "*"),
    ("SCN", "+"),
    ("scn", "+"),
    ("sh", "N"),
    ("T*", ""),
    ("Tc", "n"),
    ("Td", "nn"),
    ("TD", "nn"),
    ("Tf", "Nn"),
    ("Tj", "s"),
    ("TJ", "a"),
    ("TL", "n"),
    ("Tm", "nnnnnn"),
    ("Tr", "n"),
    ("Ts", "n"),
    ("Tw", "n"),
    ("Tz", "n"),
    ("v", "nnnn"),
    ("w", "n"),
    ("W", ""),
    ("W*", ""),
    ("y", "nnnn"),
    ("'", "s"),
    ("\"", "nns"),
];

fn fits(shape: u8, operand: &Value) -> bool {
    matches!(
        (shape, operand),
        (b'n', Value::Int(_) | Value::Real(_))
            | (b'N', Value::Name(_))
            | (b's', Value::Str(_))
            | (b'a', Value::Array(_))
            | (b'p', Value::Name(_) | Value::Dict(_))
    )
}

fn operands_fit(shape: &str, operands: &[Value]) -> bool {
    let numbers = |values: &[Value]| values.iter().all(|value| fits(b'n', value));
    match shape {
        "*" => (1..=4).contains(&operands.len()) && numbers(operands),
        "+" => match operands {
            [before @ .., Value::Name(_)] => numbers(before),
            _ => !operands.is_empty() && numbers(operands),
        },
        _ => {
            shape.len() == operands.len()
                && shape
                    .bytes()
                    .zip(operands)
                    .all(|(shape, operand)| fits(shape, operand))
        }
    }
}

/// Operations of a content stream. Unknown operators, wrong operands,
/// references, exponents and unbalanced `q`/`Q` or `BT`/`ET` are errors.
pub fn tokenize_content(data: &[u8]) -> Result<Vec<Op>, String> {
    let mut lexer = Lexer::new(data, 0);
    lexer.content = true;
    let mut ops = Vec::new();
    let mut operands = Vec::new();
    let mut saved_states = 0_usize;
    let mut in_text = false;
    while !lexer.at_end() {
        let operator = match lexer.value()? {
            Value::Operator(operator) => operator,
            operand => {
                operands.push(operand);
                continue;
            }
        };
        if operator == "BI" {
            if !operands.is_empty() {
                return Err("operands before BI".into());
            }
            let operands = lexer.inline_image()?;
            ops.push(Op { operator, operands });
            continue;
        }
        let shape = OPERATORS
            .iter()
            .find(|(name, _)| *name == operator)
            .map(|&(_, shape)| shape)
            .ok_or_else(|| format!("unknown operator {operator:?}"))?;
        if !operands_fit(shape, &operands) {
            return Err(format!("{operator} with operands {operands:?}"));
        }
        match operator.as_str() {
            "q" => saved_states += 1,
            "Q" => {
                saved_states = saved_states
                    .checked_sub(1)
                    .ok_or("Q without a matching q")?
            }
            "BT" if in_text => return Err("BT inside a text object".into()),
            "BT" => in_text = true,
            "ET" if !in_text => return Err("ET without BT".into()),
            "ET" => in_text = false,
            _ => {}
        }
        ops.push(Op {
            operator,
            operands: std::mem::take(&mut operands),
        });
    }
    if !operands.is_empty() {
        return Err(format!("operands without an operator: {operands:?}"));
    }
    if saved_states > 0 || in_text {
        return Err("content ends inside q or BT".into());
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

/// The offset in the file's last `startxref` line.
fn last_startxref(bytes: &[u8]) -> Result<usize, String> {
    let at = bytes
        .windows(9)
        .rposition(|window| window == b"startxref")
        .ok_or("no startxref")?;
    Lexer::new(bytes, at).startxref()
}

type Entries = Vec<(u32, Option<Entry>)>;

struct Section {
    entries: Entries,
    trailer: Dict,
}

/// One xref section, its trailer and the `startxref` line after it; a free
/// entry is `None`. The newest section must end the file.
fn xref_section(bytes: &[u8], at: usize, newest: bool) -> Result<Section, String> {
    let mut lexer = Lexer::new(bytes, at);
    lexer.expect_keyword("xref")?;
    let mut entries = Vec::new();
    while !lexer.peek_keyword("trailer") {
        let first = u32::try_from(lexer.int()?).map_err(|_| "bad subsection start")?;
        let count = u32::try_from(lexer.int()?).map_err(|_| "bad subsection count")?;
        let end = first.checked_add(count).ok_or("subsection past u32")?;
        lexer
            .eol()
            .ok_or("subsection header without an end of line")?;
        for number in first..end {
            let raw = lexer.take(20)?;
            let (offset, generation, in_use) = xref_entry(raw)
                .ok_or_else(|| format!("bad xref entry {:?}", String::from_utf8_lossy(raw)))?;
            if number == 0 && (in_use || generation != 65_535) {
                return Err("xref entry 0 is not free with generation 65535".into());
            }
            let generation = u16::try_from(generation).map_err(|_| "xref generation past 65535")?;
            let entry = in_use.then_some(Entry {
                number,
                generation,
                offset,
            });
            entries.push((number, entry));
        }
    }
    lexer.expect_keyword("trailer")?;
    let Value::Dict(trailer) = lexer.value()? else {
        return Err("trailer is not a dictionary".into());
    };
    lexer.skip_whitespace();
    if lexer.startxref()? != at {
        return Err(format!(
            "the startxref after the trailer at {at} points elsewhere"
        ));
    }
    if newest && !lexer.rest().iter().all(|&b| is_space(b)) {
        return Err("data after %%EOF".into());
    }
    Ok(Section { entries, trailer })
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

    fn skip_whitespace(&mut self) {
        while self.rest().first().copied().is_some_and(is_space) {
            self.pos += 1;
        }
    }

    fn skip_space(&mut self) {
        loop {
            self.skip_whitespace();
            if self.rest().first() != Some(&b'%') {
                return;
            }
            while self
                .rest()
                .first()
                .is_some_and(|&b| b != b'\n' && b != b'\r')
            {
                self.pos += 1;
            }
        }
    }

    /// `startxref`, an end of line, the offset, an end of line and `%%EOF`.
    fn startxref(&mut self) -> Result<usize, String> {
        let malformed = || "malformed startxref line".to_string();
        if !self.rest().starts_with(b"startxref") {
            return Err(malformed());
        }
        self.pos += 9;
        self.eol().ok_or_else(malformed)?;
        let digits = self.word();
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(malformed());
        }
        let offset = std::str::from_utf8(digits)
            .unwrap()
            .parse()
            .map_err(|_| malformed())?;
        self.eol().ok_or_else(malformed)?;
        if !self.rest().starts_with(b"%%EOF") {
            return Err(malformed());
        }
        self.pos += 5;
        Ok(offset)
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

    /// `[+-]?\d*\.?\d*` with a digit, ended by a space or a delimiter. An
    /// integer must fit in i64.
    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        let token = self.word();
        let text = std::str::from_utf8(token).map_err(|_| "number not ASCII")?;
        let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
        let dots = unsigned.bytes().filter(|&b| b == b'.').count();
        let well_formed = unsigned.bytes().any(|b| b.is_ascii_digit())
            && unsigned.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && dots <= 1;
        let bad = || format!("bad number {text:?} at {start}");
        if !well_formed {
            return Err(bad());
        }
        if dots == 0 {
            return text.parse().map(Value::Int).map_err(|_| bad());
        }
        text.parse().map(Value::Real).map_err(|_| bad())
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
