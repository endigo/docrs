//! Font metrics for layout: advance widths and Word for Mac's single line
//! height (hhea ascender + descender + line gap) per face.
//!
//! Three sources, best first: fonts embedded in the document
//! (`word/fonts/*.odttf`, read with [`Face::parse`]), the caller's
//! [`FontSet`], and the bundled table `font_metrics.bin` measured from the
//! fonts Word for Mac uses (`tools/layout/make_font_metrics.py`; numbers
//! only, no glyph data). A family found in none of them falls back to a
//! face of the same kind (serif, sans, monospace) and is reported.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::{Arc, OnceLock};

use crate::error::{Error, Result};

/// One face's metrics, in font units.
#[derive(Debug, Clone)]
pub struct Face {
    /// Family name as documents name it (`Calibri Light`, `Aptos`).
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// A symbol font: characters U+0020-00FF are read from U+F020-F0FF.
    symbol: bool,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
    line_gap: f64,
    average_width: f64,
    codepoints: Arc<[u32]>,
    widths: Arc<[u16]>,
    /// Each code point's glyph; empty for the bundled table.
    glyphs: Arc<[u16]>,
    /// Pair kerning between `KERN_CODEPOINTS`, in font units, sorted.
    kerning: Arc<[KernPair]>,
    file: Option<Arc<FontFile>>,
}

/// A font file's bytes and where one face's table directory starts in it.
pub struct FontFile {
    data: Arc<[u8]>,
    /// An embedded font's key: `data` is kept obfuscated, so that a part
    /// embedded under many keys is held once.
    key: Option<[u8; 16]>,
    offset: usize,
}

impl FontFile {
    /// The font file, deobfuscated.
    pub fn data(&self) -> Cow<'_, [u8]> {
        match &self.key {
            None => Cow::Borrowed(&self.data),
            Some(key) => {
                let mut data = self.data.to_vec();
                xor_font_key(&mut data, key);
                Cow::Owned(data)
            }
        }
    }

    /// Offset of the face's table directory (0 outside a collection).
    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl std::fmt::Debug for FontFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontFile")
            .field("bytes", &self.data.len())
            .field("offset", &self.offset)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontError {
    Malformed(&'static str),
    Unsupported(&'static str),
    CmapUnsorted,
    CmapReversedRange,
    CmapOutOfRange,
    TooLarge,
}

impl FontError {
    fn into_error(self) -> Error {
        let (reason, message) = match self {
            FontError::Malformed(table) => (
                "font_malformed",
                format!("font table {table} is missing or malformed"),
            ),
            FontError::Unsupported(message) => ("font_format_unsupported", message.to_string()),
            FontError::CmapUnsorted => (
                "font_cmap_invalid",
                "cmap ranges are unsorted or overlap".into(),
            ),
            FontError::CmapReversedRange => (
                "font_cmap_invalid",
                "a cmap range ends before it starts".into(),
            ),
            FontError::CmapOutOfRange => (
                "font_cmap_invalid",
                "a cmap range maps past the last code point or glyph".into(),
            ),
            FontError::TooLarge => (
                "font_too_large",
                "the font maps or kerns too many entries".into(),
            ),
        };
        Error::invalid_argument(reason, message)
    }
}

pub(crate) type Parsed<T> = std::result::Result<T, FontError>;

type KernPair = ((u32, u32), i16);

/// At most one mapping per Unicode code point.
const MAX_MAPPINGS: usize = 0x11_0000;

/// Cmap entries and kern records one font file, or one document's embedded
/// fonts, may examine.
pub(crate) const FILE_BUDGET: usize = 4 * MAX_MAPPINGS;

/// Code points kerning is kept between (printable ASCII and the
/// typographic quotes and dashes; as `tools/layout/make_font_metrics.py`).
const KERN_CODEPOINTS: [u32; 103] = {
    let mut out = [0u32; 103];
    let mut index = 0;
    while index < 95 {
        out[index] = 0x20 + index as u32;
        index += 1;
    }
    let extra = [0xA0, 0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2026];
    let mut k = 0;
    while k < extra.len() {
        out[95 + k] = extra[k];
        k += 1;
    }
    out
};

impl Face {
    /// Advance width of `ch` in ems (the average width when the face has
    /// no glyph for it).
    pub fn advance(&self, ch: char) -> f64 {
        let width = match self.mapped(ch) {
            Some(index) if self.widths[index] != u16::MAX => f64::from(self.widths[index]),
            _ => self.average_width,
        };
        width / self.units_per_em
    }

    /// The glyph `ch` maps to, when the face was read from a font file.
    pub fn glyph(&self, ch: char) -> Option<u16> {
        self.glyphs.get(self.mapped(ch)?).copied()
    }

    /// The font file the face was read from, for subsetting.
    pub fn file(&self) -> Option<&FontFile> {
        self.file.as_deref()
    }

    fn mapped(&self, ch: char) -> Option<usize> {
        let mut code = ch as u32;
        if self.symbol && (0x20..0x100).contains(&code) {
            code |= 0xF000;
        }
        self.codepoints.binary_search(&code).ok()
    }

    /// Kerning between `left` and `right` in ems (0 when the face has no
    /// pair for them).
    pub fn kern(&self, left: char, right: char) -> f64 {
        match self
            .kerning
            .binary_search_by_key(&(left as u32, right as u32), |(key, _)| *key)
        {
            Ok(index) => f64::from(self.kerning[index].1) / self.units_per_em,
            Err(_) => 0.0,
        }
    }

    /// Single line height in ems (Word for Mac: hhea ascender + descender
    /// + line gap).
    pub fn line_height(&self) -> f64 {
        (self.ascender + self.descender + self.line_gap) / self.units_per_em
    }

    /// Ascent plus line gap in ems: Word for Mac sets the gap above the
    /// text, so on a line mixing fonts it adds to the ascent.
    pub fn ascent(&self) -> f64 {
        (self.ascender + self.line_gap) / self.units_per_em
    }

    /// Metrics read from a TrueType/OpenType font file (one face of a
    /// collection), keeping the advances of every mapped code point.
    pub fn parse(data: &[u8], family: &str, bold: bool, italic: bool) -> Option<Face> {
        Face::try_parse(data, family, bold, italic).ok()
    }

    /// [`Face::parse`], saying why a face could not be read
    /// (`invalid_argument`: `font_malformed`, `font_cmap_invalid` or
    /// `font_too_large`).
    pub fn try_parse(data: &[u8], family: &str, bold: bool, italic: bool) -> Result<Face> {
        let mut budget = FILE_BUDGET;
        Ttf::new(data)
            .and_then(|ttf| Face::from_ttf(&ttf, family, bold, italic, &mut budget))
            .map_err(FontError::into_error)
    }

    /// The first face of an embedded font, read from `plain` (`part`
    /// deobfuscated with `key`) and keeping `part`.
    pub(crate) fn parse_embedded(
        plain: &[u8],
        part: &Arc<[u8]>,
        key: Option<[u8; 16]>,
        family: &str,
        budget: &mut usize,
    ) -> Parsed<Face> {
        let offset = face_view(plain, 0).ok_or(FontError::Malformed("ttcf"))?;
        let face = Face::from_ttf(&Ttf::new_at(plain, offset)?, family, false, false, budget)?;
        Ok(face.with_file(FontFile {
            data: part.clone(),
            key,
            offset,
        }))
    }

    fn with_file(self, file: FontFile) -> Face {
        Face {
            file: Some(Arc::new(file)),
            ..self
        }
    }

    pub(crate) fn restyled(&self, family: &str, bold: bool, italic: bool) -> Face {
        Face {
            family: family.to_string(),
            bold,
            italic,
            ..self.clone()
        }
    }

    fn from_ttf(
        ttf: &Ttf<'_>,
        family: &str,
        bold: bool,
        italic: bool,
        budget: &mut usize,
    ) -> Parsed<Face> {
        let head = ttf.table(b"head").ok_or(FontError::Malformed("head"))?;
        let units_per_em = be16(head, 18)
            .filter(|u| *u != 0)
            .ok_or(FontError::Malformed("head"))?;
        let units_per_em = f64::from(units_per_em);
        let hhea = ttf.table(b"hhea").ok_or(FontError::Malformed("hhea"))?;
        let field = |at| be16(hhea, at).ok_or(FontError::Malformed("hhea"));
        let ascender = f64::from(field(4)? as i16);
        let descender = -f64::from(field(6)? as i16);
        let line_gap = f64::from(field(8)? as i16);
        let metrics = usize::from(field(34)?);
        let hmtx = ttf
            .table(b"hmtx")
            .filter(|hmtx| metrics != 0 && hmtx.len() / 4 >= metrics)
            .ok_or(FontError::Malformed("hmtx"))?;
        let average_width = ttf
            .table(b"OS/2")
            .and_then(|os2| be16(os2, 2))
            .map_or(units_per_em / 2.0, |w| f64::from(w as i16).max(1.0));
        let glyphs = ttf
            .table(b"maxp")
            .and_then(|maxp| be16(maxp, 4))
            .map_or(0x1_0000, u32::from);
        let mut mapping = cmap(
            ttf.table(b"cmap").ok_or(FontError::Malformed("cmap"))?,
            glyphs,
            budget,
        )?;
        let mapping_is_symbol = !mapping.iter().any(|(c, _)| *c == 0x41)
            && mapping.iter().any(|(c, _)| (0xF020..0xF100).contains(c));
        mapping.sort_unstable();
        mapping.dedup_by_key(|(c, _)| *c);
        let advance = |glyph: u16| {
            let index = usize::from(glyph).min(metrics - 1);
            be16(hmtx, index * 4).map_or(0, |width| width.min(u16::MAX - 1))
        };
        let kerning = match ttf.table(b"kern") {
            Some(table) => kern_pairs(table, &mapping, budget)?,
            None => Vec::new(),
        };
        Ok(Face {
            family: family.to_string(),
            bold,
            italic,
            symbol: mapping_is_symbol,
            units_per_em,
            ascender,
            descender,
            line_gap,
            average_width,
            codepoints: mapping.iter().map(|(c, _)| *c).collect(),
            widths: mapping.iter().map(|(_, glyph)| advance(*glyph)).collect(),
            glyphs: mapping.iter().map(|(_, glyph)| *glyph).collect(),
            kerning: kerning.into(),
            file: None,
        })
    }
}

/// The classic `kern` table's horizontal format 0 pairs (Microsoft
/// version 0 and Apple version 1 layouts) between `KERN_CODEPOINTS`.
fn kern_pairs(table: &[u8], mapping: &[(u32, u16)], budget: &mut usize) -> Parsed<Vec<KernPair>> {
    let mut by_glyph: HashMap<u16, Vec<u32>> = HashMap::new();
    for (c, glyph) in mapping {
        if KERN_CODEPOINTS.contains(c) {
            by_glyph.entry(*glyph).or_default().push(*c);
        }
    }
    let mut by_pair: HashMap<(u16, u16), i16> = HashMap::new();
    let mut records = table.len() / 6;
    let mut exhausted = false;
    let mut read = |at: usize| -> Option<()> {
        let pairs = usize::from(be16(table, at)?);
        for index in 0..pairs {
            records = records.checked_sub(1)?;
            let Some(left_budget) = budget.checked_sub(1) else {
                exhausted = true;
                return None;
            };
            *budget = left_budget;
            let record = at.checked_add(8 + 6 * index)?;
            let (left, right) = (be16(table, record)?, be16(table, record + 2)?);
            let value = be16(table, record + 4)? as i16;
            if value != 0 && by_glyph.contains_key(&left) && by_glyph.contains_key(&right) {
                by_pair.entry((left, right)).or_insert(value);
            }
        }
        Some(())
    };
    if be16(table, 0) == Some(0) {
        let count = usize::from(be16(table, 2).unwrap_or(0));
        let mut at = 4;
        for _ in 0..count {
            let (Some(length), Some(coverage)) = (be16(table, at + 2), be16(table, at + 4)) else {
                break;
            };
            if coverage >> 8 == 0 && coverage & 1 != 0 && coverage & 6 == 0 {
                read(at + 6);
            }
            at += usize::from(length).max(6);
            if at >= table.len() {
                break;
            }
        }
    } else if be32(table, 0) == Some(0x0001_0000) {
        let count = be32(table, 4).unwrap_or(0).min(64);
        let mut at = 8;
        for _ in 0..count {
            let (Some(length), Some(coverage)) = (be32(table, at), be16(table, at + 4)) else {
                break;
            };
            if coverage & 0xFF == 0 && coverage & 0xE000 == 0 {
                read(at + 8);
            }
            match at.checked_add((length as usize).max(8)) {
                Some(next) if next < table.len() => at = next,
                _ => break,
            }
        }
    }
    if exhausted {
        return Err(FontError::TooLarge);
    }
    let pairs: usize = by_pair
        .keys()
        .map(|(left, right)| by_glyph[left].len() * by_glyph[right].len())
        .sum();
    *budget = budget.checked_sub(pairs).ok_or(FontError::TooLarge)?;
    let mut out = Vec::with_capacity(pairs);
    for ((left, right), value) in by_pair {
        for l in &by_glyph[&left] {
            for r in &by_glyph[&right] {
                out.push(((*l, *r), value));
            }
        }
    }
    out.sort_unstable_by_key(|(key, _)| *key);
    Ok(out)
}

impl Face {
    /// Every face of a font file (one, or each of a collection), named and
    /// styled the way documents name fonts: the family, with a style other
    /// than regular, bold, italic or bold italic appended. Faces that cannot
    /// be read are skipped.
    pub fn parse_file(data: &[u8]) -> Vec<Face> {
        let mut budget = FILE_BUDGET;
        read_faces(&Arc::from(data), &mut budget)
            .into_iter()
            .filter_map(|read| read.ok().map(|named| named.face))
            .collect()
    }
}

/// A face read from a font file, with the other names it answers to.
struct Named {
    face: Face,
    info: FontInfo,
    /// Full name (name id 4) and PostScript name (id 6).
    names: [Option<String>; 2],
}

fn read_faces(data: &Arc<[u8]>, budget: &mut usize) -> Vec<Parsed<Named>> {
    let count = match data.get(..4) {
        Some(b"ttcf") => be32(data, 8).unwrap_or(0) as usize,
        Some(b"true" | b"OTTO" | [0, 1, 0, 0]) => 1,
        Some(b"wOFF" | b"wOF2") => {
            return vec![Err(FontError::Unsupported(
                "WOFF and WOFF2 fonts are not supported; supply the TTF or OTF file",
            ))];
        }
        Some(b"typ1" | b"%!PS" | b"%!Fo" | [0x80, 0x01, ..]) => {
            return vec![Err(FontError::Unsupported(
                "Type 1 fonts are not supported",
            ))];
        }
        _ => {
            return vec![Err(FontError::Unsupported(
                "not a TrueType or OpenType font (TTF, OTF, TTC or OTC)",
            ))];
        }
    };
    (0..count.min(64))
        .map(|index| read_face(data, index, budget))
        .collect()
}

fn read_face(data: &Arc<[u8]>, index: usize, budget: &mut usize) -> Parsed<Named> {
    let offset = face_view(data, index).ok_or(FontError::Malformed("ttcf"))?;
    if !matches!(
        be32(data, offset),
        Some(0x0001_0000 | 0x7472_7565 | 0x4F54_544F)
    ) {
        return Err(FontError::Malformed("table directory"));
    }
    let ttf = Ttf::new_at(data, offset)?;
    if ttf.table(b"CFF2").is_some() {
        return Err(FontError::Unsupported("CFF2 fonts are not supported"));
    }
    if ttf.table(b"glyf").is_none() && ttf.table(b"CFF ").is_none() {
        return Err(FontError::Unsupported(
            "bitmap-only fonts are not supported",
        ));
    }
    let [Some(family), Some(subfamily), full, postscript] =
        ttf.table(b"name").map(names).unwrap_or_default()
    else {
        return Err(FontError::Malformed("name"));
    };
    let (family, bold, italic) = document_style(family, &subfamily);
    let face = Face::from_ttf(&ttf, &family, bold, italic, budget)?;
    let info = FontInfo {
        family,
        bold,
        italic,
        variable: ttf.table(b"fvar").is_some(),
        embeddable: embeddable(&ttf),
    };
    Ok(Named {
        face: face.with_file(FontFile {
            data: data.clone(),
            key: None,
            offset,
        }),
        info,
        names: [full, postscript],
    })
}

/// The family and style a document names a face by, from its name table's
/// family and subfamily.
fn document_style(family: String, subfamily: &str) -> (String, bool, bool) {
    let words: Vec<&str> = subfamily
        .split_whitespace()
        .map(|w| if w == "Oblique" { "Italic" } else { w })
        .collect();
    let italic = words.contains(&"Italic");
    let rest: Vec<&str> = words.into_iter().filter(|w| *w != "Italic").collect();
    let bold = rest == ["Bold"];
    let plain = matches!(
        rest.as_slice(),
        [] | ["Regular"] | ["Roman"] | ["Plain"] | ["Book"] | ["Normal"] | ["Bold"]
    );
    let extra = rest.join(" ");
    let family = if plain || family.ends_with(&extra) {
        family
    } else {
        format!("{family} {extra}")
    };
    (family, bold, italic)
}

/// OS/2 `fsType`: restricted licence embedding (bits 0-3 equal to 2) or
/// bitmap embedding only (bit 9) forbid embedding the outlines.
fn embeddable(ttf: &Ttf<'_>) -> bool {
    let fs_type = ttf.table(b"OS/2").and_then(|os2| be16(os2, 8)).unwrap_or(0);
    fs_type & 0x000F != 0x0002 && fs_type & 0x0200 == 0
}

/// Offset of face `index`'s table directory.
fn face_view(data: &[u8], index: usize) -> Option<usize> {
    if data.get(..4)? == b"ttcf" {
        Some(be32(data, 12 + 4 * index)? as usize)
    } else {
        (index == 0).then_some(0)
    }
}

/// Name ids 1, 2, 4 and 6 (family, subfamily, full and PostScript name;
/// Windows English, else Macintosh Roman).
fn names(table: &[u8]) -> [Option<String>; 4] {
    const IDS: [u16; 4] = [1, 2, 4, 6];
    let mut out: [Option<String>; 4] = Default::default();
    let mut mac: [Option<String>; 4] = Default::default();
    let (Some(count), Some(strings)) = (be16(table, 2), be16(table, 4)) else {
        return out;
    };
    for index in 0..usize::from(count) {
        let at = 6 + 12 * index;
        let (Some(platform), Some(language), Some(id), Some(length), Some(offset)) = (
            be16(table, at),
            be16(table, at + 4),
            be16(table, at + 6),
            be16(table, at + 8),
            be16(table, at + 10),
        ) else {
            continue;
        };
        let Some(slot) = IDS.iter().position(|wanted| *wanted == id) else {
            continue;
        };
        let start = usize::from(strings) + usize::from(offset);
        let Some(raw) = table.get(start..start + usize::from(length)) else {
            continue;
        };
        if platform == 3 && (language == 0x409 || language == 0) && out[slot].is_none() {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            out[slot] = Some(String::from_utf16_lossy(&units));
        } else if platform == 1 && language == 0 && mac[slot].is_none() {
            mac[slot] = Some(raw.iter().map(|b| char::from(*b)).collect());
        }
    }
    for (name, mac) in out.iter_mut().zip(mac) {
        if name.is_none() {
            *name = mac;
        }
    }
    out
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The table directory of the first face of a font file.
struct Ttf<'a> {
    data: &'a [u8],
    tables: Vec<([u8; 4], usize, usize)>,
}

impl<'a> Ttf<'a> {
    fn new(data: &'a [u8]) -> Parsed<Ttf<'a>> {
        Ttf::new_at(
            data,
            face_view(data, 0).ok_or(FontError::Malformed("ttcf"))?,
        )
    }

    fn new_at(data: &'a [u8], offset: usize) -> Parsed<Ttf<'a>> {
        let malformed = FontError::Malformed("table directory");
        let directory = offset.checked_add(12).ok_or(malformed)?;
        let count = usize::from(be16(data, offset + 4).ok_or(malformed)?);
        if data.len().saturating_sub(directory) / 16 < count {
            return Err(malformed);
        }
        let mut tables = Vec::with_capacity(count);
        for index in 0..count {
            let record = directory + 16 * index;
            let read = || -> Option<([u8; 4], usize, usize)> {
                let tag: [u8; 4] = data.get(record..record + 4)?.try_into().ok()?;
                let start = be32(data, record + 8)? as usize;
                let length = be32(data, record + 12)? as usize;
                data.get(start..start.checked_add(length)?)?;
                Some((tag, start, length))
            };
            tables.push(read().ok_or(malformed)?);
        }
        Ok(Ttf { data, tables })
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        let (_, start, length) = self.tables.iter().find(|(t, _, _)| t == tag)?;
        self.data.get(*start..start + length)
    }
}

/// The Unicode cmap (format 4 or 12): `(code point, glyph)`.
fn cmap(table: &[u8], glyphs: u32, budget: &mut usize) -> Parsed<Vec<(u32, u16)>> {
    let malformed = FontError::Malformed("cmap");
    let field16 = |at| be16(table, at).ok_or(malformed);
    let field32 = |at| be32(table, at).ok_or(malformed);
    let count = usize::from(field16(2)?);
    let mut best: Option<(u8, usize, u16)> = None;
    for index in 0..count {
        let platform = field16(4 + 8 * index)?;
        let encoding = field16(6 + 8 * index)?;
        let offset = field32(8 + 8 * index)? as usize;
        let format = field16(offset)?;
        let rank = match (platform, encoding) {
            (3, 10) | (0, 4) => 3,
            (3, 1) | (0, 3) => 2,
            (3, 0) => 1,
            _ => 0,
        };
        if matches!(format, 4 | 12) && best.is_none_or(|(r, _, _)| rank > r) {
            best = Some((rank, offset, format));
        }
    }
    let (_, offset, format) = best.ok_or(malformed)?;
    let mut out = Vec::new();
    let mut charge = || -> Parsed<()> {
        *budget = budget.checked_sub(1).ok_or(FontError::TooLarge)?;
        Ok(())
    };
    if format == 4 {
        let length = usize::from(field16(offset + 2)?);
        let sub = table.get(offset..offset + length).ok_or(malformed)?;
        let field = |at| be16(sub, at).ok_or(malformed);
        let seg_x2 = usize::from(field(6)?);
        if seg_x2 == 0 || seg_x2 % 2 != 0 || 16 + 4 * seg_x2 > length {
            return Err(malformed);
        }
        let segments = seg_x2 / 2;
        let (ends, starts) = (14, 16 + seg_x2);
        let (deltas, ranges) = (starts + seg_x2, starts + 2 * seg_x2);
        if field(ends + seg_x2 - 2)? != 0xFFFF {
            return Err(malformed);
        }
        let mut previous_end: Option<u16> = None;
        for s in 0..segments {
            let end = field(ends + 2 * s)?;
            let start = field(starts + 2 * s)?;
            let delta = field(deltas + 2 * s)?;
            let range_offset = usize::from(field(ranges + 2 * s)?);
            if start > end {
                return Err(FontError::CmapReversedRange);
            }
            if previous_end.is_some_and(|p| end < p) {
                return Err(FontError::CmapUnsorted);
            }
            previous_end = Some(end);
            for c in start..=end {
                charge()?;
                if c == 0xFFFF {
                    continue;
                }
                let glyph = match range_offset {
                    0 => c.wrapping_add(delta),
                    _ => match field(ranges + 2 * s + range_offset + 2 * usize::from(c - start))? {
                        0 => 0,
                        g => g.wrapping_add(delta),
                    },
                };
                if glyph == 0 {
                    continue;
                }
                if u32::from(glyph) >= glyphs {
                    return Err(FontError::CmapOutOfRange);
                }
                push(&mut out, u32::from(c), glyph)?;
            }
        }
    } else {
        let length = field32(offset + 4)? as usize;
        let sub = offset
            .checked_add(length)
            .and_then(|end| table.get(offset..end))
            .ok_or(malformed)?;
        let field32 = |at| be32(sub, at).ok_or(malformed);
        let groups = field32(12)? as usize;
        let needed = groups.checked_mul(12).and_then(|n| n.checked_add(16));
        if needed.is_none_or(|n| n > length) {
            return Err(malformed);
        }
        let mut previous_end: Option<u32> = None;
        for g in 0..groups {
            let at = 16 + 12 * g;
            let start = field32(at)?;
            let end = field32(at + 4)?;
            let glyph = field32(at + 8)?;
            if start > end {
                return Err(FontError::CmapReversedRange);
            }
            if previous_end.is_some_and(|p| start <= p) {
                return Err(FontError::CmapUnsorted);
            }
            previous_end = Some(end);
            if end > 0x10_FFFF || u64::from(glyph) + u64::from(end - start) >= u64::from(glyphs) {
                return Err(FontError::CmapOutOfRange);
            }
            for c in start..=end {
                charge()?;
                push(&mut out, c, (glyph + (c - start)) as u16)?;
            }
        }
    }
    Ok(out)
}

fn push(out: &mut Vec<(u32, u16)>, c: u32, glyph: u16) -> Parsed<()> {
    if out.len() >= MAX_MAPPINGS {
        return Err(FontError::TooLarge);
    }
    out.try_reserve(1).map_err(|_| FontError::TooLarge)?;
    out.push((c, glyph));
    Ok(())
}

fn builtin() -> &'static [Arc<Face>] {
    static FACES: OnceLock<Vec<Arc<Face>>> = OnceLock::new();
    FACES.get_or_init(|| read_table(include_bytes!("font_metrics.bin")).unwrap_or_default())
}

fn read_table(packed: &[u8]) -> Option<Vec<Arc<Face>>> {
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(packed)
        .read_to_end(&mut data)
        .ok()?;
    let le16 = |at: usize| Some(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]));
    if data.get(..4)? != b"DCFM" || le16(4)? != 3 {
        return None;
    }
    let set_count = usize::from(le16(6)?);
    let mut at = 8;
    let mut sets: Vec<Arc<[u32]>> = Vec::with_capacity(set_count);
    for _ in 0..set_count {
        let count = usize::from(le16(at)?);
        at += 2;
        let mut codepoints = Vec::with_capacity(count);
        for _ in 0..count {
            codepoints.push(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
            at += 4;
        }
        sets.push(codepoints.into());
    }
    let kern_count = usize::from(*data.get(at)?);
    at += 1;
    let mut kern_codepoints = Vec::with_capacity(kern_count);
    for _ in 0..kern_count {
        kern_codepoints.push(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
        at += 4;
    }
    let faces = usize::from(le16(at)?);
    at += 2;
    let mut out = Vec::with_capacity(faces);
    for _ in 0..faces {
        let length = usize::from(*data.get(at)?);
        let family = String::from_utf8(data.get(at + 1..at + 1 + length)?.to_vec()).ok()?;
        at += 1 + length;
        let flags = *data.get(at)?;
        let codepoints = sets.get(usize::from(*data.get(at + 1)?))?.clone();
        at += 2;
        let units_per_em = f64::from(le16(at)?);
        let ascender = f64::from(le16(at + 2)? as i16);
        let descender = f64::from(le16(at + 4)? as i16);
        let line_gap = f64::from(le16(at + 6)? as i16);
        let average_width = f64::from(le16(at + 8)?).max(1.0);
        at += 10;
        let mut widths = Vec::with_capacity(codepoints.len());
        for _ in 0..codepoints.len() {
            widths.push(le16(at)?);
            at += 2;
        }
        let pairs = usize::from(le16(at)?);
        at += 2;
        let mut kerning = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let left = *kern_codepoints.get(usize::from(*data.get(at)?))?;
            let right = *kern_codepoints.get(usize::from(*data.get(at + 1)?))?;
            kerning.push(((left, right), le16(at + 2)? as i16));
            at += 4;
        }
        kerning.sort_unstable_by_key(|(key, _)| *key);
        out.push(Arc::new(Face {
            family,
            bold: flags & 1 != 0,
            italic: flags & 2 != 0,
            symbol: flags & 4 != 0,
            units_per_em,
            ascender,
            descender,
            line_gap,
            average_width,
            codepoints,
            widths: widths.into(),
            glyphs: Arc::new([]),
            kerning: kerning.into(),
            file: None,
        }));
    }
    Some(out)
}

/// How Word for Mac substitutes a font it does not have (measured): a font
/// the font table declares roman, or does not declare at all, is laid out
/// in Cambria; any other declared font in Calibri.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    /// Declared `w:family="roman"`, or not in the font table.
    Serif,
    /// Declared with another family, or none.
    Sans,
    /// Declared fixed pitch (Word still substitutes Calibri).
    Mono,
}

/// One face a [`FontSet`] provides, named as a document names it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontInfo {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Has variation axes; layout measures its default instance.
    pub variable: bool,
    /// The font's `fsType` allows embedding its outlines.
    pub embeddable: bool,
}

/// Font files the caller supplies to lay out with. Immutable and cheap to
/// clone; build one with [`FontSet::builder`].
#[derive(Debug, Clone, Default)]
pub struct FontSet(Arc<FontSetInner>);

#[derive(Debug, Default)]
struct FontSetInner {
    faces: Vec<Arc<Face>>,
    infos: Vec<FontInfo>,
    index: HashMap<String, [Option<usize>; 4]>,
    aliases: HashMap<String, String>,
}

impl FontSet {
    pub fn builder() -> FontSetBuilder {
        FontSetBuilder {
            faces: Vec::new(),
            aliases: HashMap::new(),
            budget: FILE_BUDGET,
        }
    }

    /// Every face, in the order added.
    pub fn faces(&self) -> &[FontInfo] {
        &self.0.infos
    }

    /// A set of already measured faces, found by family only.
    pub(crate) fn from_faces(faces: Vec<Face>) -> FontSet {
        let infos = faces
            .iter()
            .map(|face| FontInfo {
                family: face.family.clone(),
                bold: face.bold,
                italic: face.italic,
                variable: false,
                embeddable: false,
            })
            .collect();
        let faces: Vec<Arc<Face>> = faces.into_iter().map(Arc::new).collect();
        FontSet(Arc::new(FontSetInner {
            index: index(&faces),
            faces,
            infos,
            aliases: HashMap::new(),
        }))
    }

    fn resolve<'k>(&'k self, key: &'k str) -> &'k str {
        self.0.aliases.get(key).map_or(key, String::as_str)
    }
}

/// Collects font files for a [`FontSet`]. Every file read shares one parse
/// budget.
pub struct FontSetBuilder {
    faces: Vec<Named>,
    aliases: HashMap<String, String>,
    budget: usize,
}

impl FontSetBuilder {
    /// Adds every face of a TTF, OTF, TTC or OTC file. Refuses the whole
    /// file, adding nothing, when a face cannot be read (`invalid_argument`:
    /// `font_format_unsupported`, `font_malformed`, `font_cmap_invalid` or
    /// `font_too_large`).
    pub fn add(&mut self, bytes: Vec<u8>) -> Result<Vec<FontInfo>> {
        let faces = read_faces(&bytes.into(), &mut self.budget)
            .into_iter()
            .collect::<Parsed<Vec<Named>>>()
            .and_then(|faces| {
                if faces.is_empty() {
                    Err(FontError::Malformed("ttcf"))
                } else {
                    Ok(faces)
                }
            })
            .map_err(FontError::into_error)?;
        let infos = faces.iter().map(|named| named.info.clone()).collect();
        self.faces.extend(faces);
        Ok(infos)
    }

    /// Lays out `family` with `to`'s faces (from this set, else the bundled
    /// table) unless the document embeds `family`.
    pub fn alias(&mut self, family: &str, to: &str) -> &mut Self {
        self.aliases
            .insert(family.trim().to_lowercase(), to.trim().to_lowercase());
        self
    }

    pub fn build(self) -> FontSet {
        let (faces, infos, names): (Vec<_>, Vec<_>, Vec<_>) = self
            .faces
            .into_iter()
            .map(|named| (Arc::new(named.face), named.info, named.names))
            .collect();
        let mut index = index(&faces);
        let families: HashSet<String> = index.keys().cloned().collect();
        for (at, names) in names.iter().enumerate() {
            for key in names.iter().flatten().map(|name| name.to_lowercase()) {
                if !families.contains(&key) {
                    index.entry(key).or_default()[slot(&faces[at])].get_or_insert(at);
                }
            }
        }
        FontSet(Arc::new(FontSetInner {
            faces,
            infos,
            index,
            aliases: self.aliases,
        }))
    }
}

/// Faces by family for one layout: the document's embedded fonts first,
/// then the caller's [`FontSet`], then the bundled table.
#[derive(Default)]
pub struct FontBook {
    own: Vec<Arc<Face>>,
    own_index: HashMap<String, [Option<usize>; 4]>,
    fonts: FontSet,
    /// Lower-cased family → the faces found for it, per (bold, italic).
    cache: HashMap<String, Option<FaceSet>>,
    /// Families that had to be substituted, as the document names them.
    pub missing: Vec<String>,
    missing_seen: HashSet<String>,
}

fn slot(face: &Face) -> usize {
    usize::from(face.bold) | (usize::from(face.italic) << 1)
}

/// Lower-cased family → the first face of each (bold, italic) slot.
fn index(faces: &[Arc<Face>]) -> HashMap<String, [Option<usize>; 4]> {
    let mut out: HashMap<String, [Option<usize>; 4]> = HashMap::new();
    for (at, face) in faces.iter().enumerate() {
        out.entry(face.family.to_lowercase()).or_default()[slot(face)].get_or_insert(at);
    }
    out
}

fn builtin_index() -> &'static HashMap<String, [Option<usize>; 4]> {
    static INDEX: OnceLock<HashMap<String, [Option<usize>; 4]>> = OnceLock::new();
    INDEX.get_or_init(|| index(builtin()))
}

#[derive(Clone, Copy)]
enum Source {
    Own,
    Fonts,
    Builtin,
}

#[derive(Clone, Copy)]
struct FaceSet {
    source: Source,
    faces: [Option<usize>; 4],
}

impl FontBook {
    /// A font book with `own` faces taking precedence over the table.
    pub fn new(own: Vec<Face>) -> FontBook {
        FontBook::with_fonts(own, &FontSet::default())
    }

    /// A font book with `own` faces first, then `fonts`, then the table.
    pub fn with_fonts(own: Vec<Face>, fonts: &FontSet) -> FontBook {
        let own: Vec<Arc<Face>> = own.into_iter().map(Arc::new).collect();
        FontBook {
            own_index: index(&own),
            own,
            fonts: fonts.clone(),
            ..FontBook::default()
        }
    }

    fn set(&mut self, family: &str) -> Option<FaceSet> {
        let mut key = family.trim().to_lowercase();
        // Names Word resolves to another font.
        if key == "times" {
            key = "times new roman".to_string();
        }
        if let Some(found) = self.cache.get(&key) {
            return *found;
        }
        let resolved = self.fonts.resolve(&key);
        let found = [
            (Source::Own, self.own_index.get(&key)),
            (Source::Fonts, self.fonts.0.index.get(resolved)),
            (Source::Builtin, builtin_index().get(resolved)),
        ]
        .into_iter()
        .find_map(|(source, faces)| {
            faces.map(|faces| FaceSet {
                source,
                faces: *faces,
            })
        });
        self.cache.insert(key, found);
        found
    }
    /// Whether metrics exist for `family`.
    pub fn knows(&mut self, family: &str) -> bool {
        self.set(family).is_some()
    }

    /// The face for `family` in a style: the exact style, else the nearest
    /// one the family has (Word synthesizes the rest); a family with no
    /// metrics takes `kind`'s substitute and is recorded in `missing`.
    pub fn face(&mut self, family: &str, bold: bool, italic: bool, kind: FontKind) -> Arc<Face> {
        let set = match self.set(family) {
            Some(set) => set,
            None => {
                if !family.is_empty() && self.missing_seen.insert(family.to_ascii_lowercase()) {
                    self.missing.push(family.to_string());
                }
                let substitute = match kind {
                    FontKind::Serif => "Cambria",
                    FontKind::Sans | FontKind::Mono => "Calibri",
                };
                self.set(substitute).expect("bundled substitute fonts")
            }
        };
        let wanted = usize::from(bold) | (usize::from(italic) << 1);
        let order = [wanted, wanted & 1, wanted & 2, 0, 1, 2, 3];
        let index = order
            .iter()
            .find_map(|slot| set.faces[*slot])
            .expect("a face set has a face");
        match set.source {
            Source::Own => self.own[index].clone(),
            Source::Fonts => self.fonts.0.faces[index].clone(),
            Source::Builtin => builtin()[index].clone(),
        }
    }
}

/// Deobfuscate an embedded font (`.odttf`, ECMA-376 Part 1 §17.8.1): the
/// first 32 bytes are XORed with the font key GUID's bytes, reversed.
pub fn deobfuscate(data: &[u8], key: &str) -> Option<Vec<u8>> {
    let key = font_key(key)?;
    let mut out = data.to_vec();
    xor_font_key(&mut out, &key);
    Some(out)
}

/// The font key GUID's bytes, reversed, as they are XORed.
pub(crate) fn font_key(key: &str) -> Option<[u8; 16]> {
    let hex: String = key.chars().filter(char::is_ascii_hexdigit).collect();
    if hex.len() != 32 {
        return None;
    }
    let mut guid = [0u8; 16];
    for (index, byte) in guid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).ok()?;
    }
    guid.reverse();
    Some(guid)
}

/// XORs the first 32 bytes with `key`; applying it twice restores `data`.
pub(crate) fn xor_font_key(data: &mut [u8], key: &[u8; 16]) {
    for (index, byte) in data.iter_mut().take(32).enumerate() {
        *byte ^= key[index % 16];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_table_reads() {
        let faces = builtin();
        assert!(faces.len() > 100);
        let mut book = FontBook::default();
        let arial = book.face("Arial", false, false, FontKind::Sans);
        // Arial: 'a' is 1139/2048 em; single line 1.1499 em.
        assert!((arial.advance('a') - 1139.0 / 2048.0).abs() < 1e-9);
        assert!((arial.line_height() - 2355.0 / 2048.0).abs() < 1e-9);
        assert!(book.knows("calibri") && book.knows("Aptos"));
        assert!(book.missing.is_empty());
        book.face("No Such Font", true, false, FontKind::Serif);
        assert_eq!(book.missing, ["No Such Font"]);
        // Arial kerns "AV" by -152/2048 em; "ab" not at all.
        assert!((arial.kern('A', 'V') + 152.0 / 2048.0).abs() < 1e-9);
        assert_eq!(arial.kern('a', 'b'), 0.0);
    }

    #[test]
    fn deobfuscation_xors_the_first_32_bytes() {
        let key = "{01234567-89AB-CDEF-0123-456789ABCDEF}";
        let data = vec![0u8; 40];
        let out = deobfuscate(&data, key).unwrap();
        assert_eq!(out[0], 0xEF);
        assert_eq!(out[15], 0x01);
        assert_eq!(out[16], 0xEF);
        assert_eq!(&out[32..], &[0u8; 8]);
    }
}

#[cfg(test)]
mod coverage {
    use super::*;

    #[test]
    fn common_document_fonts_are_measured() {
        let mut book = FontBook::default();
        for family in [
            "Arial",
            "Calibri",
            "Calibri Light",
            "Cambria",
            "Aptos",
            "Aptos Display",
            "Consolas",
            "Symbol",
            "Liberation Sans",
            "Lucida Bright",
            "Segoe UI",
            "Tahoma",
            "Trebuchet MS",
            "Times New Roman",
            "Courier New",
            "Webdings",
            "Wingdings",
            "MS Gothic",
        ] {
            assert!(book.knows(family), "{family}");
        }
        let gothic = book.face("MS Gothic", false, false, FontKind::Sans);
        for ch in ['\u{2610}', '\u{2611}', '\u{2612}'] {
            assert!((gothic.advance(ch) - 1.0).abs() < 1e-9, "{ch}");
        }
        assert!((gothic.advance('A') - 0.5).abs() < 1e-9);
        assert!((gothic.line_height() - 1.0).abs() < 1e-9);
        // Symbol's bullet (U+F0B7, written as U+00B7 too) has a width.
        let symbol = book.face("Symbol", false, false, FontKind::Serif);
        assert!(symbol.advance('\u{F0B7}') > 0.2);
        assert!((symbol.advance('\u{B7}') - symbol.advance('\u{F0B7}')).abs() < 1e-9);
    }
}

#[cfg(test)]
mod hostile {
    use super::*;
    use proptest::prelude::*;
    use std::time::{Duration, Instant};

    pub(super) type Tables = Vec<([u8; 4], Vec<u8>)>;

    pub(super) fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0; 6]);
        let mut at = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in tables {
            out.extend(tag);
            out.extend([0; 4]);
            out.extend((at as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            body.extend(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            at = 12 + 16 * tables.len() + body.len();
        }
        out.extend(body);
        out
    }

    pub(super) fn be(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    /// A Windows English name table holding `records` (name id, text).
    pub(super) fn name_table(records: &[(u16, &str)]) -> Vec<u8> {
        let texts: Vec<Vec<u8>> = records
            .iter()
            .map(|(_, text)| text.encode_utf16().flat_map(u16::to_be_bytes).collect())
            .collect();
        let mut out = be(&[0, records.len() as u16, 6 + 12 * records.len() as u16]);
        let mut offset = 0;
        for ((id, _), text) in records.iter().zip(&texts) {
            out.extend(be(&[3, 1, 0x409, *id, text.len() as u16, offset]));
            offset += text.len() as u16;
        }
        out.extend(texts.concat());
        out
    }

    fn face_tables(glyphs: u16, cmap: Vec<u8>, kern: Option<Vec<u8>>) -> Vec<u8> {
        face_with_metrics(glyphs, cmap, kern, 3)
    }

    fn face_with_metrics(
        glyphs: u16,
        cmap: Vec<u8>,
        kern: Option<Vec<u8>>,
        metrics: u16,
    ) -> Vec<u8> {
        let mut tables = base_tables(glyphs, cmap, metrics);
        tables.push((*b"name", name_table(&[(1, "Test"), (2, "Regular")])));
        if let Some(kern) = kern {
            tables.push((*b"kern", kern));
        }
        sfnt(&tables)
    }

    /// Every table but `name`: glyph g advances 500 + 100 g of 1000 units.
    pub(super) fn base_tables(glyphs: u16, cmap: Vec<u8>, metrics: u16) -> Tables {
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let mut hhea = vec![0; 36];
        hhea[4..6].copy_from_slice(&800u16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&metrics.to_be_bytes());
        let mut maxp = vec![0, 0, 0x50, 0];
        maxp.extend(glyphs.to_be_bytes());
        let hmtx: Vec<u8> = (0..metrics).flat_map(|g| be(&[500 + 100 * g, 0])).collect();
        vec![
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"maxp", maxp),
            (*b"hmtx", hmtx),
            (*b"cmap", cmap),
            (*b"glyf", Vec::new()),
        ]
    }

    pub(super) fn format4(segments: &[(u16, u16, u16)]) -> Vec<u8> {
        let segments: Vec<_> = segments.iter().map(|&(s, e, d)| (s, e, d, 0)).collect();
        format4_with(&segments, &[])
    }

    /// Segments are (start, end, delta, idRangeOffset).
    pub(super) fn format4_with(segments: &[(u16, u16, u16, u16)], glyph_ids: &[u16]) -> Vec<u8> {
        let n = segments.len();
        let length = (16 + 8 * n + 2 * glyph_ids.len()) as u16;
        let mut out = be(&[0, 1, 3, 1, 0, 12, 4, length, 0, 2 * n as u16, 0, 0, 0]);
        out.extend(segments.iter().flat_map(|s| s.1.to_be_bytes()));
        out.extend(be(&[0]));
        out.extend(segments.iter().flat_map(|s| s.0.to_be_bytes()));
        out.extend(segments.iter().flat_map(|s| s.2.to_be_bytes()));
        out.extend(segments.iter().flat_map(|s| s.3.to_be_bytes()));
        out.extend(be(glyph_ids));
        out
    }

    pub(super) fn format12(groups: impl ExactSizeIterator<Item = (u32, u32, u32)>) -> Vec<u8> {
        let mut out = be(&[0, 1, 3, 10, 0, 12, 12, 0]);
        out.extend((16 + 12 * groups.len() as u32).to_be_bytes());
        out.extend([0; 4]);
        out.extend((groups.len() as u32).to_be_bytes());
        for (start, end, glyph) in groups {
            out.extend(start.to_be_bytes());
            out.extend(end.to_be_bytes());
            out.extend(glyph.to_be_bytes());
        }
        out
    }

    fn full_unicode_cmap() -> Vec<u8> {
        format12(
            (0..0x11_0000u32)
                .step_by(0xFFFF)
                .map(|start| (start, (start + 0xFFFE).min(0x10_FFFF), 0)),
        )
    }

    pub(super) fn rebase(font: &[u8], base: usize) -> Vec<u8> {
        let mut font = font.to_vec();
        for index in 0..usize::from(be16(&font, 4).unwrap()) {
            let at = 12 + 16 * index + 8;
            let offset = be32(&font, at).unwrap() as usize + base;
            font[at..at + 4].copy_from_slice(&(offset as u32).to_be_bytes());
        }
        font
    }

    /// A collection of `count` directory entries, entry `i` pointing at
    /// `faces[i % faces.len()]`.
    pub(super) fn collection(faces: &[Vec<u8>], count: usize) -> Vec<u8> {
        let mut out = b"ttcf".to_vec();
        out.extend(be(&[1, 0]));
        out.extend((count as u32).to_be_bytes());
        let mut base = 12 + 4 * count;
        let mut offsets = Vec::new();
        for face in faces {
            offsets.push(base);
            base += face.len();
        }
        for index in 0..count {
            out.extend((offsets[index % faces.len()] as u32).to_be_bytes());
        }
        for (face, offset) in faces.iter().zip(offsets) {
            out.extend(rebase(face, offset));
        }
        out
    }

    fn read(font: &[u8], budget: &mut usize) -> Parsed<Face> {
        Face::from_ttf(&Ttf::new(font)?, "Test", false, false, budget)
    }

    fn refusal(font: &[u8]) -> Option<FontError> {
        let mut budget = FILE_BUDGET;
        read(font, &mut budget).err()
    }

    pub(super) fn ab_cmap() -> Vec<u8> {
        format4(&[(0x41, 0x42, 1u16.wrapping_sub(0x41)), (0xFFFF, 0xFFFF, 1)])
    }

    fn valid() -> Vec<u8> {
        let kern = {
            let mut kern = be(&[0, 1, 0, 6 + 8 + 6, 1, 1, 0, 0, 0]);
            kern.extend(be(&[1, 2, (-50i16) as u16]));
            kern
        };
        face_tables(3, ab_cmap(), Some(kern))
    }

    fn quickly<T>(run: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let out = run();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        out
    }

    #[test]
    fn a_small_valid_font_reads() {
        let face = Face::try_parse(&valid(), "Test", false, false).unwrap();
        assert!((face.advance('A') - 0.6).abs() < 1e-9);
        assert!((face.advance('B') - 0.7).abs() < 1e-9);
        assert!((face.kern('A', 'B') + 0.05).abs() < 1e-9);
        assert!((face.line_height() - 1.0).abs() < 1e-9);
        let faces = Face::parse_file(&valid());
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].family, "Test");
    }

    #[test]
    fn repeated_format_12_groups_are_refused_quickly() {
        let font = face_tables(
            0xFFFF,
            format12((0..0x1_0000).map(|_| (0, 0xFFFE, 0))),
            None,
        );
        assert!(font.len() > 700 * 1024);
        assert_eq!(quickly(|| refusal(&font)), Some(FontError::CmapUnsorted));
        assert!(quickly(|| Face::parse_file(&font)).is_empty());
        let error = Face::try_parse(&font, "Test", false, false).unwrap_err();
        assert_eq!(
            (error.code(), error.reason()),
            ("invalid_argument", "font_cmap_invalid")
        );
    }

    #[test]
    fn inconsistent_format_12_groups_are_refused() {
        let parse = |groups: &[(u32, u32, u32)]| {
            refusal(&face_tables(3, format12(groups.iter().copied()), None))
        };
        assert_eq!(parse(&[(0x41, 0x42, 1)]), None);
        assert_eq!(
            parse(&[(0x42, 0x41, 1)]),
            Some(FontError::CmapReversedRange)
        );
        assert_eq!(
            parse(&[(0x42, 0x42, 1), (0x41, 0x41, 2)]),
            Some(FontError::CmapUnsorted)
        );
        assert_eq!(
            parse(&[(0x41, 0x42, 1), (0x42, 0x43, 2)]),
            Some(FontError::CmapUnsorted)
        );
        assert_eq!(parse(&[(0x41, 0x43, 1)]), Some(FontError::CmapOutOfRange));
        assert_eq!(
            parse(&[(0x11_0000, 0x11_0000, 1)]),
            Some(FontError::CmapOutOfRange)
        );
    }

    #[test]
    fn repeated_format_4_segments_are_refused_quickly() {
        let mut segments = vec![(0, 0xFFFE, 0); 7999];
        segments.push((0xFFFF, 0xFFFF, 1));
        let font = face_tables(0xFFFF, format4(&segments), None);
        assert_eq!(quickly(|| refusal(&font)), Some(FontError::TooLarge));
        assert!(quickly(|| Face::parse_file(&font)).is_empty());
        let too_long = face_tables(0xFFFF, format4(&vec![(0, 0xFFFE, 0); 0x7FFF]), None);
        assert_eq!(
            quickly(|| refusal(&too_long)),
            Some(FontError::Malformed("cmap"))
        );
    }

    #[test]
    fn a_format_4_subtable_must_be_whole() {
        let malformed = Some(FontError::Malformed("cmap"));
        let patched = |at: usize, value: u16| {
            let mut cmap = ab_cmap();
            cmap[12 + at..14 + at].copy_from_slice(&value.to_be_bytes());
            refusal(&face_tables(3, cmap, None))
        };
        let length = (ab_cmap().len() - 12) as u16;
        assert_eq!(patched(2, length), None);
        assert_eq!(patched(2, length + 2), malformed);
        assert_eq!(patched(2, length - 2), malformed);
        assert_eq!(patched(6, 3), malformed);
        assert_eq!(patched(6, 0), malformed);
        let to_a = 1u16.wrapping_sub(0x41);
        assert_eq!(
            refusal(&face_tables(3, format4(&[(0x41, 0x42, to_a)]), None)),
            malformed
        );
    }

    #[test]
    fn a_format_4_glyph_index_must_stay_in_its_subtable() {
        let segments = [(0x41, 0x42, 0, 4), (0xFFFF, 0xFFFF, 1, 0)];
        let mut budget = FILE_BUDGET;
        let face = read(
            &face_tables(3, format4_with(&segments, &[1, 2]), None),
            &mut budget,
        )
        .unwrap();
        assert!((face.advance('B') - 0.7).abs() < 1e-9);
        let mut short = format4_with(&segments, &[1]);
        short.extend(be(&[2]));
        assert_eq!(
            refusal(&face_tables(3, short, None)),
            Some(FontError::Malformed("cmap"))
        );
    }

    #[test]
    fn inconsistent_format_4_segments_are_refused() {
        let parse = |segments: &[(u16, u16, u16)]| {
            let mut segments = segments.to_vec();
            segments.push((0xFFFF, 0xFFFF, 1));
            refusal(&face_tables(3, format4(&segments), None))
        };
        let to_a = 1u16.wrapping_sub(0x41);
        assert_eq!(
            parse(&[(0x42, 0x41, to_a)]),
            Some(FontError::CmapReversedRange)
        );
        assert_eq!(
            parse(&[(0x41, 0x42, to_a), (0x40, 0x41, to_a)]),
            Some(FontError::CmapUnsorted)
        );
        assert_eq!(
            parse(&[(0x41, 0x43, to_a)]),
            Some(FontError::CmapOutOfRange)
        );
    }

    #[test]
    fn format_4_segments_may_share_one_code_point() {
        let to_a = 1u16.wrapping_sub(0x41);
        let segments = [
            (0x41, 0x42, to_a),
            (0x42, 0x42, 1u16.wrapping_sub(0x42)),
            (0xFFFF, 0xFFFF, 1),
        ];
        let mut budget = FILE_BUDGET;
        let face = read(&face_tables(3, format4(&segments), None), &mut budget).unwrap();
        assert!((face.advance('A') - 0.6).abs() < 1e-9);
        assert!((face.advance('B') - 0.6).abs() < 1e-9);
    }

    #[test]
    fn a_format_12_subtable_must_be_whole() {
        let patched = |length: u32| {
            let mut cmap = format12([(0x41, 0x42, 1)].into_iter());
            cmap[16..20].copy_from_slice(&length.to_be_bytes());
            refusal(&face_tables(3, cmap, None))
        };
        assert_eq!(patched(28), None);
        assert_eq!(patched(27), Some(FontError::Malformed("cmap")));
        assert_eq!(patched(32), Some(FontError::Malformed("cmap")));
    }

    #[test]
    fn a_collection_skips_its_inconsistent_face() {
        let bad = face_tables(3, format12([(0x42, 0x41, 1)].into_iter()), None);
        let faces = Face::parse_file(&collection(&[bad, valid()], 2));
        assert_eq!(faces.len(), 1);
        assert!((faces[0].advance('A') - 0.6).abs() < 1e-9);
    }

    #[test]
    fn a_face_with_an_empty_hmtx_is_refused_before_its_cmap() {
        let face = face_with_metrics(0xFFFF, full_unicode_cmap(), None, 0);
        let mut budget = FILE_BUDGET;
        assert_eq!(
            read(&face, &mut budget).err(),
            Some(FontError::Malformed("hmtx"))
        );
        assert_eq!(budget, FILE_BUDGET);
        let ttc = collection(&[face], 64);
        assert!(ttc.len() < 800, "{}", ttc.len());
        assert!(quickly(|| Face::parse_file(&ttc)).is_empty());
    }

    #[test]
    fn a_collection_sharing_one_full_cmap_stops_at_the_file_budget() {
        let ttc = collection(&[face_tables(0xFFFF, full_unicode_cmap(), None)], 64);
        let faces = quickly(|| Face::parse_file(&ttc));
        assert_eq!(faces.len(), FILE_BUDGET / MAX_MAPPINGS);
        assert!(faces.iter().all(|face| face.widths.len() == MAX_MAPPINGS));
    }

    #[test]
    fn a_refused_face_is_charged_for_the_entries_it_read() {
        let mut groups: Vec<_> = (0..0x10_0000u32)
            .step_by(0xFFFF)
            .map(|start| (start, start + 0xFFFE, 0))
            .collect();
        groups.push((0, 0, 0));
        let face = face_tables(0xFFFF, format12(groups.into_iter()), None);
        let mut budget = FILE_BUDGET;
        assert_eq!(
            read(&face, &mut budget).err(),
            Some(FontError::CmapUnsorted)
        );
        assert!(FILE_BUDGET - budget >= 0x10_0000);
        let ttc = collection(&[face, valid()], 64);
        assert!(quickly(|| Face::parse_file(&ttc)).len() < 32);
    }

    #[test]
    fn a_kern_pair_listed_twice_takes_its_first_value() {
        let mut kern = be(&[0, 1, 0, 6 + 8 + 12, 1, 2, 0, 0, 0]);
        kern.extend(be(&[1, 2, (-50i16) as u16, 1, 2, (-30i16) as u16]));
        let mut budget = FILE_BUDGET;
        let face = read(&face_tables(3, ab_cmap(), Some(kern)), &mut budget).unwrap();
        assert!((face.kern('A', 'B') + 0.05).abs() < 1e-9);
    }

    #[test]
    fn overlapping_kern_subtables_are_read_quickly() {
        let mut kern = be(&[0, 0xFFFF]);
        for _ in 0..0xFFFF {
            kern.extend(be(&[0xFFFF, 6, 1]));
        }
        let font = face_tables(3, ab_cmap(), Some(kern));
        let mut budget = FILE_BUDGET;
        let face = quickly(|| read(&font, &mut budget)).unwrap();
        assert_eq!(face.kern('A', 'B'), 0.0);
    }

    proptest! {
        #[test]
        fn mutated_fonts_never_panic(
            edits in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..16),
            cut in any::<usize>(),
        ) {
            let mut font = valid();
            for (at, byte) in edits {
                let at = at % font.len();
                font[at] = byte;
            }
            font.truncate(font.len() - cut % 8);
            let _ = Face::try_parse(&font, "Test", false, false);
            let _ = Face::parse_file(&font);
        }
    }
}

#[cfg(test)]
mod font_set {
    use super::hostile::{Tables, ab_cmap, base_tables, collection, format12, name_table, sfnt};
    use super::*;

    fn font(style: &str, extra: Tables) -> Vec<u8> {
        let mut tables = base_tables(3, ab_cmap(), 3);
        let (full, postscript) = (format!("Brand {style}"), format!("Brand-{style}"));
        tables.push((
            *b"name",
            name_table(&[(1, "Brand"), (2, style), (4, &full), (6, &postscript)]),
        ));
        tables.extend(extra);
        sfnt(&tables)
    }

    fn os2(fs_type: u16) -> ([u8; 4], Vec<u8>) {
        let mut table = vec![0; 78];
        table[2..4].copy_from_slice(&550u16.to_be_bytes());
        table[8..10].copy_from_slice(&fs_type.to_be_bytes());
        (*b"OS/2", table)
    }

    fn set_of(files: &[Vec<u8>]) -> FontSet {
        let mut builder = FontSet::builder();
        for file in files {
            builder.add(file.clone()).unwrap();
        }
        builder.build()
    }

    fn refusal(file: Vec<u8>) -> (&'static str, &'static str) {
        let error = FontSet::builder().add(file).unwrap_err();
        (error.code(), error.reason())
    }

    #[test]
    fn faces_are_found_by_family_full_and_postscript_name() {
        let fonts = set_of(&[font("Regular", vec![]), font("Bold", vec![])]);
        let styles: Vec<_> = fonts.faces().iter().map(|f| (&*f.family, f.bold)).collect();
        assert_eq!(styles, [("Brand", false), ("Brand", true)]);
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        for name in ["brand", "Brand Bold", "BRAND-BOLD", "Brand-Regular"] {
            assert!(book.knows(name), "{name}");
        }
        let bold = book.face("Brand", true, false, FontKind::Sans);
        assert!(bold.bold && (bold.advance('A') - 0.6).abs() < 1e-9);
        assert!(book.face("Brand Bold", false, false, FontKind::Sans).bold);
        assert!(!book.face("Brand", false, false, FontKind::Sans).bold);
        assert!(book.missing.is_empty());
    }

    #[test]
    fn a_face_keeps_its_glyphs_and_file() {
        let regular = font("Regular", vec![]);
        let bold = font("Bold", vec![]);
        let ttc = collection(&[regular.clone(), bold], 2);
        let mut builder = FontSet::builder();
        let added = builder.add(ttc.clone()).unwrap();
        assert_eq!(added.len(), 2);
        let mut book = FontBook::with_fonts(Vec::new(), &builder.build());
        let face = book.face("Brand", true, false, FontKind::Sans);
        assert_eq!(
            (face.glyph('A'), face.glyph('B'), face.glyph('C')),
            (Some(1), Some(2), None)
        );
        let file = face.file().unwrap();
        assert_eq!(*file.data(), *ttc);
        assert_eq!(file.offset(), 20 + regular.len());
        let regular_face = book.face("Brand", false, false, FontKind::Sans);
        assert_eq!(regular_face.file().unwrap().offset(), 20);
    }

    #[test]
    fn a_variable_font_is_accepted_and_flagged() {
        let fonts = set_of(&[font("Regular", vec![(*b"fvar", vec![0; 16])])]);
        assert!(fonts.faces()[0].variable);
        assert!(!set_of(&[font("Regular", vec![])]).faces()[0].variable);
    }

    #[test]
    fn fs_type_decides_embedding() {
        for (fs_type, embeddable) in [
            (0x0000, true),
            (0x0002, false),
            (0x0004, true),
            (0x0008, true),
            (0x000A, true),
            (0x0200, false),
            (0x0208, false),
        ] {
            let fonts = set_of(&[font("Regular", vec![os2(fs_type)])]);
            assert_eq!(fonts.faces()[0].embeddable, embeddable, "{fs_type:#06x}");
        }
    }

    #[test]
    fn an_alias_lays_a_family_out_in_another() {
        let mut builder = FontSet::builder();
        builder.add(font("Regular", vec![])).unwrap();
        builder
            .alias("Calibri", "Brand")
            .alias("Helvetica", "Arial");
        let fonts = builder.build();
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        assert_eq!(
            book.face("calibri", false, false, FontKind::Sans).family,
            "Brand"
        );
        assert_eq!(
            book.face("Helvetica", false, false, FontKind::Sans).family,
            "Arial"
        );
        let embedded = Face::try_parse(&font("Regular", vec![]), "Calibri", false, false).unwrap();
        let mut book = FontBook::with_fonts(vec![embedded], &fonts);
        assert_eq!(
            book.face("Calibri", false, false, FontKind::Sans).family,
            "Calibri"
        );
    }

    #[test]
    fn invalid_bytes_are_refused_and_add_nothing() {
        let bad = sfnt(&{
            let mut tables = base_tables(3, format12([(0x42, 0x41, 1)].into_iter()), 3);
            tables.push((*b"name", name_table(&[(1, "Bad"), (2, "Regular")])));
            tables
        });
        let mut builder = FontSet::builder();
        let error = builder
            .add(collection(&[font("Regular", vec![]), bad], 2))
            .unwrap_err();
        assert_eq!(error.reason(), "font_cmap_invalid");
        builder.add(font("Bold", vec![])).unwrap();
        let fonts = builder.build();
        assert_eq!(fonts.faces().len(), 1);
        assert!(fonts.faces()[0].bold);

        let unsupported = ("invalid_argument", "font_format_unsupported");
        let malformed = ("invalid_argument", "font_malformed");
        let mut woff = font("Regular", vec![]);
        woff[..4].copy_from_slice(b"wOF2");
        assert_eq!(refusal(woff), unsupported);
        assert_eq!(refusal(b"%!PS-AdobeFont-1.0: Brand".to_vec()), unsupported);
        assert_eq!(refusal(vec![0x80, 0x01, 0, 0, 0, 0]), unsupported);
        assert_eq!(refusal(b"plain text".to_vec()), unsupported);
        assert_eq!(
            refusal(font("Regular", vec![(*b"CFF2", vec![0; 8])])),
            unsupported
        );
        let mut bitmap = font("Regular", vec![]);
        let glyf = bitmap.windows(4).position(|w| w == b"glyf").unwrap();
        bitmap[glyf..glyf + 4].copy_from_slice(b"EBDT");
        assert_eq!(refusal(bitmap), unsupported);
        assert_eq!(refusal(font("Regular", vec![])[..40].to_vec()), malformed);
        assert_eq!(refusal(collection(&[], 0)), malformed);
        let mut nameless = font("Regular", vec![]);
        let name = nameless.windows(4).position(|w| w == b"name").unwrap();
        nameless[name..name + 4].copy_from_slice(b"xxxx");
        assert_eq!(refusal(nameless), malformed);
    }

    #[test]
    fn otf_outlines_are_measured() {
        let mut tables = base_tables(3, ab_cmap(), 3);
        tables.retain(|(tag, _)| tag != b"glyf");
        tables.push((*b"CFF ", vec![1, 0, 4, 1]));
        tables.push((*b"name", name_table(&[(1, "Brand"), (2, "Regular")])));
        let mut otf = sfnt(&tables);
        otf[..4].copy_from_slice(b"OTTO");
        let mut book = FontBook::with_fonts(Vec::new(), &set_of(&[otf]));
        assert!(
            (book
                .face("Brand", false, false, FontKind::Sans)
                .advance('B')
                - 0.7)
                .abs()
                < 1e-9
        );
    }
}
