//! Font metrics for layout: advance widths and Word for Mac's single line
//! height (hhea ascender + descender + line gap) per face.
//!
//! Three sources, best first: fonts embedded in the document
//! (`word/fonts/*.odttf`, read with [`Face::parse`]), fonts the caller
//! supplies, and the bundled table `font_metrics.bin` measured from the
//! fonts Word for Mac uses (`tools/layout/make_font_metrics.py`; numbers
//! only, no glyph data). A family found in none of them falls back to a
//! face of the same kind (serif, sans, monospace) and is reported.

use std::collections::HashMap;
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
    widths: Vec<u16>,
    /// Pair kerning between `KERN_CODEPOINTS`, in font units, sorted.
    kerning: Vec<((u32, u32), i16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontError {
    Malformed(&'static str),
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

type Parsed<T> = std::result::Result<T, FontError>;

type KernPair = ((u32, u32), i16);

/// At most one mapping per Unicode code point.
const MAX_MAPPINGS: usize = 0x11_0000;

/// Cmap entries and kern records one file may examine, across its faces.
const FILE_BUDGET: usize = 4 * MAX_MAPPINGS;

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
        let mut code = ch as u32;
        if self.symbol && (0x20..0x100).contains(&code) {
            code |= 0xF000;
        }
        let width = match self.codepoints.binary_search(&code) {
            Ok(index) if self.widths[index] != u16::MAX => f64::from(self.widths[index]),
            _ => self.average_width,
        };
        width / self.units_per_em
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
        let codepoints: Arc<[u32]> = mapping.iter().map(|(c, _)| *c).collect();
        let widths: Vec<u16> = mapping.iter().map(|(_, glyph)| advance(*glyph)).collect();
        let symbol = mapping_is_symbol;
        Ok(Face {
            family: family.to_string(),
            bold,
            italic,
            symbol,
            units_per_em,
            ascender,
            descender,
            line_gap,
            average_width,
            codepoints,
            widths,
            kerning,
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
    /// than regular, bold, italic or bold italic appended.
    pub fn parse_file(data: &[u8]) -> Vec<Face> {
        let count = if data.get(..4) == Some(b"ttcf") {
            be32(data, 8).unwrap_or(0) as usize
        } else {
            1
        };
        let mut out = Vec::new();
        let mut budget = FILE_BUDGET;
        for index in 0..count.min(64) {
            let Some(view) = face_view(data, index) else {
                continue;
            };
            let Some(names) = Ttf::new_at(data, view)
                .ok()
                .and_then(|t| t.table(b"name").map(names))
            else {
                continue;
            };
            let (Some(family), Some(subfamily)) = (names.0, names.1) else {
                continue;
            };
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
            if let Ok(face) = Ttf::new_at(data, view)
                .and_then(|t| Face::from_ttf(&t, &family, bold, italic, &mut budget))
            {
                out.push(face);
            }
        }
        out
    }
}

/// Offset of face `index`'s table directory.
fn face_view(data: &[u8], index: usize) -> Option<usize> {
    if data.get(..4)? == b"ttcf" {
        Some(be32(data, 12 + 4 * index)? as usize)
    } else {
        (index == 0).then_some(0)
    }
}

/// (family, subfamily) from a name table (ids 1 and 2; Windows English,
/// else Macintosh Roman).
fn names(table: &[u8]) -> (Option<String>, Option<String>) {
    let mut out: [Option<String>; 2] = [None, None];
    let mut mac: [Option<String>; 2] = [None, None];
    let Some(count) = be16(table, 2) else {
        return (None, None);
    };
    let Some(strings) = be16(table, 4) else {
        return (None, None);
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
        if !(1..=2).contains(&id) {
            continue;
        }
        let start = usize::from(strings) + usize::from(offset);
        let Some(raw) = table.get(start..start + usize::from(length)) else {
            continue;
        };
        let slot = usize::from(id - 1);
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
    let [family, subfamily] = out;
    let [mac_family, mac_subfamily] = mac;
    (family.or(mac_family), subfamily.or(mac_subfamily))
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
            if previous_end.is_some_and(|p| start <= p) {
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
        let groups = field32(offset + 12)? as usize;
        let last = groups
            .checked_mul(12)
            .and_then(|n| n.checked_add(offset + 16))
            .ok_or(malformed)?;
        if last > table.len() {
            return Err(malformed);
        }
        let mut previous_end: Option<u32> = None;
        for g in 0..groups {
            let at = offset + 16 + 12 * g;
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
            widths,
            kerning,
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

/// Faces by family for one layout: the document's embedded and the
/// caller's fonts first, then the bundled table.
#[derive(Default)]
pub struct FontBook {
    own: Vec<Arc<Face>>,
    /// Lower-cased family → indices into `own` (negative-free) or the
    /// bundled table, per (bold, italic).
    cache: HashMap<String, Option<FaceSet>>,
    /// Families that had to be substituted, as the document names them.
    pub missing: Vec<String>,
}

#[derive(Clone, Copy)]
struct FaceSet {
    own: bool,
    faces: [Option<usize>; 4],
}

impl FontBook {
    /// A font book with `own` faces taking precedence over the table.
    pub fn new(own: Vec<Face>) -> FontBook {
        FontBook {
            own: own.into_iter().map(Arc::new).collect(),
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
        let find = |faces: &[Arc<Face>]| {
            let mut set = [None; 4];
            for (index, face) in faces.iter().enumerate() {
                if face.family.to_lowercase() == key {
                    let slot = usize::from(face.bold) | (usize::from(face.italic) << 1);
                    set[slot].get_or_insert(index);
                }
            }
            set.iter().any(Option::is_some).then_some(set)
        };
        let found = find(&self.own)
            .map(|faces| FaceSet { own: true, faces })
            .or_else(|| find(builtin()).map(|faces| FaceSet { own: false, faces }));
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
                if !family.is_empty()
                    && !self.missing.iter().any(|m| m.eq_ignore_ascii_case(family))
                {
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
        if set.own {
            self.own[index].clone()
        } else {
            builtin()[index].clone()
        }
    }
}

/// Deobfuscate an embedded font (`.odttf`, ECMA-376 Part 1 §17.8.1): the
/// first 32 bytes are XORed with the font key GUID's bytes, reversed.
pub fn deobfuscate(data: &[u8], key: &str) -> Option<Vec<u8>> {
    let hex: String = key.chars().filter(char::is_ascii_hexdigit).collect();
    if hex.len() != 32 {
        return None;
    }
    let mut guid = [0u8; 16];
    for (index, byte) in guid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).ok()?;
    }
    guid.reverse();
    let mut out = data.to_vec();
    for (index, byte) in out.iter_mut().take(32).enumerate() {
        *byte ^= guid[index % 16];
    }
    Some(out)
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

    fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0; 6]);
        let mut at = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in tables {
            out.extend(*tag);
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

    fn be(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
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
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let mut hhea = vec![0; 36];
        hhea[4..6].copy_from_slice(&800u16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&metrics.to_be_bytes());
        let mut maxp = vec![0, 0, 0x50, 0];
        maxp.extend(glyphs.to_be_bytes());
        let hmtx: Vec<u8> = (0..metrics).flat_map(|g| be(&[500 + 100 * g, 0])).collect();
        let utf16 =
            |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
        let (family, style) = (utf16("Test"), utf16("Regular"));
        let mut name = be(&[0, 2, 6 + 24]);
        name.extend(be(&[3, 1, 0x409, 1, family.len() as u16, 0]));
        name.extend(be(&[
            3,
            1,
            0x409,
            2,
            style.len() as u16,
            family.len() as u16,
        ]));
        name.extend(family);
        name.extend(style);
        let mut tables = vec![
            (b"head", head),
            (b"hhea", hhea),
            (b"maxp", maxp),
            (b"hmtx", hmtx),
            (b"cmap", cmap),
            (b"name", name),
        ];
        if let Some(kern) = kern {
            tables.push((b"kern", kern));
        }
        sfnt(&tables)
    }

    fn format4(segments: &[(u16, u16, u16)]) -> Vec<u8> {
        let segments: Vec<_> = segments.iter().map(|&(s, e, d)| (s, e, d, 0)).collect();
        format4_with(&segments, &[])
    }

    /// Segments are (start, end, delta, idRangeOffset).
    fn format4_with(segments: &[(u16, u16, u16, u16)], glyph_ids: &[u16]) -> Vec<u8> {
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

    fn format12(groups: impl ExactSizeIterator<Item = (u32, u32, u32)>) -> Vec<u8> {
        let mut out = be(&[0, 1, 3, 10, 0, 12, 12, 0]);
        out.extend([0; 8]);
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

    fn rebase(font: &[u8], base: usize) -> Vec<u8> {
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
    fn collection(faces: &[Vec<u8>], count: usize) -> Vec<u8> {
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
        read(font, &mut FILE_BUDGET.clone()).err()
    }

    fn ab_cmap() -> Vec<u8> {
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
        assert_eq!(quickly(|| refusal(&font)), Some(FontError::CmapUnsorted));
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
        let face = read(
            &face_tables(3, format4_with(&segments, &[1, 2]), None),
            &mut FILE_BUDGET.clone(),
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
            parse(&[(0x41, 0x42, to_a), (0x42, 0x42, to_a)]),
            Some(FontError::CmapUnsorted)
        );
        assert_eq!(
            parse(&[(0x41, 0x43, to_a)]),
            Some(FontError::CmapOutOfRange)
        );
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
        assert!(
            faces
                .iter()
                .all(|face| face.widths.capacity() == MAX_MAPPINGS)
        );
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
    fn overlapping_kern_subtables_are_read_quickly() {
        let mut kern = be(&[0, 0xFFFF]);
        for _ in 0..0xFFFF {
            kern.extend(be(&[0xFFFF, 6, 1]));
        }
        let font = face_tables(3, ab_cmap(), Some(kern));
        let face = quickly(|| read(&font, &mut FILE_BUDGET.clone())).unwrap();
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
