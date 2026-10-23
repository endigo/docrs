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
        Face::from_ttf(&Ttf::new(data)?, family, bold, italic)
    }

    fn from_ttf(ttf: &Ttf<'_>, family: &str, bold: bool, italic: bool) -> Option<Face> {
        let head = ttf.table(b"head")?;
        let units_per_em = f64::from(be16(head, 18)?);
        let hhea = ttf.table(b"hhea")?;
        let ascender = f64::from(be16(hhea, 4)? as i16);
        let descender = -f64::from(be16(hhea, 6)? as i16);
        let line_gap = f64::from(be16(hhea, 8)? as i16);
        let metrics = usize::from(be16(hhea, 34)?);
        let hmtx = ttf.table(b"hmtx")?;
        let average_width = ttf
            .table(b"OS/2")
            .and_then(|os2| be16(os2, 2))
            .map_or(units_per_em / 2.0, |w| f64::from(w as i16).max(1.0));
        let mut mapping = cmap(ttf.table(b"cmap")?)?;
        let mapping_is_symbol = !mapping.iter().any(|(c, _)| *c == 0x41)
            && mapping.iter().any(|(c, _)| (0xF020..0xF100).contains(c));
        mapping.sort_unstable();
        mapping.dedup_by_key(|(c, _)| *c);
        let advance = |glyph: u16| {
            let index = usize::from(glyph).min(metrics.saturating_sub(1));
            be16(hmtx, index * 4)
        };
        let kerning = ttf
            .table(b"kern")
            .map(|table| kern_pairs(table, &mapping))
            .unwrap_or_default();
        let mut codepoints = Vec::with_capacity(mapping.len());
        let mut widths = Vec::with_capacity(mapping.len());
        for (c, glyph) in mapping {
            if let Some(width) = advance(glyph) {
                codepoints.push(c);
                widths.push(width.min(u16::MAX - 1));
            }
        }
        let symbol = mapping_is_symbol;
        Some(Face {
            family: family.to_string(),
            bold,
            italic,
            symbol,
            units_per_em,
            ascender,
            descender,
            line_gap,
            average_width,
            codepoints: codepoints.into(),
            widths,
            kerning,
        })
    }
}

/// The classic `kern` table's horizontal format 0 pairs (Microsoft
/// version 0 and Apple version 1 layouts) between `KERN_CODEPOINTS`.
fn kern_pairs(table: &[u8], mapping: &[(u32, u16)]) -> Vec<((u32, u32), i16)> {
    let mut by_glyph: HashMap<u16, Vec<u32>> = HashMap::new();
    for (c, glyph) in mapping {
        if KERN_CODEPOINTS.contains(c) {
            by_glyph.entry(*glyph).or_default().push(*c);
        }
    }
    let mut out = Vec::new();
    let mut read = |at: usize| -> Option<()> {
        let pairs = usize::from(be16(table, at)?);
        for index in 0..pairs {
            let record = at + 8 + 6 * index;
            let (left, right) = (be16(table, record)?, be16(table, record + 2)?);
            let value = be16(table, record + 4)? as i16;
            if value == 0 {
                continue;
            }
            if let (Some(ls), Some(rs)) = (by_glyph.get(&left), by_glyph.get(&right)) {
                for l in ls {
                    for r in rs {
                        out.push(((*l, *r), value));
                    }
                }
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
            at += (length as usize).max(8);
        }
    }
    out.sort_unstable_by_key(|(key, _)| *key);
    out.dedup_by_key(|(key, _)| *key);
    out
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
        for index in 0..count.min(64) {
            let Some(view) = face_view(data, index) else {
                continue;
            };
            let Some(names) = Ttf::new_at(data, view).and_then(|t| t.table(b"name").map(names))
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
            if let Some(face) =
                Ttf::new_at(data, view).and_then(|t| Face::from_ttf(&t, &family, bold, italic))
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
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// The table directory of the first face of a font file.
struct Ttf<'a> {
    data: &'a [u8],
    tables: Vec<([u8; 4], usize, usize)>,
}

impl<'a> Ttf<'a> {
    fn new(data: &'a [u8]) -> Option<Ttf<'a>> {
        Ttf::new_at(data, face_view(data, 0)?)
    }

    fn new_at(data: &'a [u8], offset: usize) -> Option<Ttf<'a>> {
        let count = usize::from(be16(data, offset + 4)?);
        let mut tables = Vec::with_capacity(count);
        for index in 0..count {
            let record = offset + 12 + 16 * index;
            let tag: [u8; 4] = data.get(record..record + 4)?.try_into().ok()?;
            let start = be32(data, record + 8)? as usize;
            let length = be32(data, record + 12)? as usize;
            data.get(start..start.checked_add(length)?)?;
            tables.push((tag, start, length));
        }
        Some(Ttf { data, tables })
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        let (_, start, length) = self.tables.iter().find(|(t, _, _)| t == tag)?;
        self.data.get(*start..start + length)
    }
}

/// The Unicode cmap (format 4 or 12): `(code point, glyph)`.
fn cmap(table: &[u8]) -> Option<Vec<(u32, u16)>> {
    let count = usize::from(be16(table, 2)?);
    let mut best: Option<(u8, usize, u16)> = None;
    for index in 0..count {
        let platform = be16(table, 4 + 8 * index)?;
        let encoding = be16(table, 6 + 8 * index)?;
        let offset = be32(table, 8 + 8 * index)? as usize;
        let format = be16(table, offset)?;
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
    let (_, offset, format) = best?;
    let mut out = Vec::new();
    if format == 4 {
        let segments = usize::from(be16(table, offset + 6)?) / 2;
        let ends = offset + 14;
        let starts = ends + 2 * segments + 2;
        let deltas = starts + 2 * segments;
        let ranges = deltas + 2 * segments;
        for s in 0..segments {
            let end = be16(table, ends + 2 * s)?;
            let start = be16(table, starts + 2 * s)?;
            let delta = be16(table, deltas + 2 * s)?;
            let range_offset = usize::from(be16(table, ranges + 2 * s)?);
            if start > end {
                continue;
            }
            for c in start..=end {
                if c == 0xFFFF {
                    continue;
                }
                let glyph = if range_offset == 0 {
                    c.wrapping_add(delta)
                } else {
                    let at = ranges + 2 * s + range_offset + 2 * usize::from(c - start);
                    match be16(table, at) {
                        Some(0) | None => 0,
                        Some(g) => g.wrapping_add(delta),
                    }
                };
                if glyph != 0 {
                    out.push((u32::from(c), glyph));
                }
            }
        }
    } else {
        let groups = be32(table, offset + 12)? as usize;
        for g in 0..groups.min(1 << 16) {
            let at = offset + 16 + 12 * g;
            let start = be32(table, at)?;
            let end = be32(table, at + 4)?.min(start.saturating_add(0x10000));
            let glyph = be32(table, at + 8)?;
            for c in start..=end {
                let id = glyph + (c - start);
                if let Ok(id) = u16::try_from(id) {
                    out.push((c, id));
                }
            }
        }
    }
    Some(out)
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
        ] {
            assert!(book.knows(family), "{family}");
        }
        // Symbol's bullet (U+F0B7, written as U+00B7 too) has a width.
        let symbol = book.face("Symbol", false, false, FontKind::Serif);
        assert!(symbol.advance('\u{F0B7}') > 0.2);
        assert!((symbol.advance('\u{B7}') - symbol.advance('\u{F0B7}')).abs() < 1e-9);
    }
}
