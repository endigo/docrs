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

use super::model::MathScripts;
use super::variation::{Fvar, Style, optical_label};
use crate::error::{Error, Result};
use crate::txn::Txn;

/// One face's metrics, in font units.
#[derive(Debug, Clone)]
pub struct Face {
    /// Family name as documents name it (`Calibri Light`, `Aptos`).
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// A symbol font: characters U+0020-00FF are read from U+F020-F0FF.
    symbol: bool,
    /// Its `fsType` allows embedding its outlines.
    embeddable: bool,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
    line_gap: f64,
    average_width: f64,
    /// OS/2 `ySuperscriptYSize` and `ySubscriptYSize`, in ems.
    script_sizes: [f64; 2],
    codepoints: Arc<[u32]>,
    widths: Arc<[u16]>,
    /// Each code point's glyph; empty for the bundled table.
    glyphs: Arc<[u16]>,
    /// Pair kerning between `KERN_CODEPOINTS`, in font units, sorted.
    kerning: Arc<[KernPair]>,
    /// `post`'s underline and `OS/2`'s strikeout; `None` when the face
    /// gives none.
    underline: Option<Stroke>,
    strikeout: Option<Stroke>,
    /// Its MATH table's script geometry, for a math font read from a file.
    math: Option<MathScripts>,
    file: Option<Arc<FontFile>>,
    /// The style a variable face was instanced at; `None`: its default.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    instance: Option<Style>,
}

/// A decoration line in ems: its centre above the baseline (negative:
/// below) and its thickness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stroke {
    pub offset: f64,
    pub thickness: f64,
}

/// The thickest decoration line a face may ask for, in ems.
const MAX_STROKE: f64 = 0.2;

impl Stroke {
    /// `top` of a line `thickness` thick, in font units, as `post` and
    /// `OS/2` give them; `None` for a missing or empty line, one thicker
    /// than [`MAX_STROKE`], or one whose top is outside `tops` (ems). The
    /// default instance's values: `MVAR` deltas are not applied.
    fn read(
        top: Option<u16>,
        thickness: Option<u16>,
        units_per_em: f64,
        tops: std::ops::RangeInclusive<f64>,
    ) -> Option<Stroke> {
        let top = f64::from(top? as i16) / units_per_em;
        let thickness = f64::from(thickness? as i16) / units_per_em;
        (thickness > 0.0 && thickness <= MAX_STROKE && tops.contains(&top)).then(|| Stroke {
            offset: top - thickness / 2.0,
            thickness,
        })
    }
}

/// Arial's lines, for faces that give none (Word check pending).
const UNDERLINE: Stroke = Stroke {
    offset: -0.142_578,
    thickness: 0.073_242,
};
const STRIKEOUT: Stroke = Stroke {
    offset: 0.233_887,
    thickness: 0.049_805,
};

/// The font program a face was read from, and where the face's table
/// directory starts in it.
pub struct FontFile {
    source: FileSource,
    offset: usize,
    index: u32,
}

enum FileSource {
    Bytes(Arc<[u8]>),
    /// A font the document embeds, read again from its part when needed.
    Part {
        name: String,
        key: Option<[u8; 16]>,
    },
}

impl FontFile {
    /// The font file. An embedded font is read from its part through `txn`,
    /// which must be over the document the face was laid out from. A keyed
    /// font is deobfuscated into a copy of the part on every call, so a
    /// writer reads each (part, key) once.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(crate) fn read<'a>(&'a self, txn: &'a Txn<'_>) -> Option<Cow<'a, [u8]>> {
        match &self.source {
            FileSource::Bytes(bytes) => Some(Cow::Borrowed(bytes)),
            FileSource::Part { name, key: None } => txn.binary(name),
            FileSource::Part {
                name,
                key: Some(key),
            } => {
                let mut bytes = txn.binary(name)?.into_owned();
                xor_font_key(&mut bytes, key);
                Some(Cow::Owned(bytes))
            }
        }
    }

    /// Offset of the face's table directory (0 outside a collection).
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The face's index in its collection (0 outside one).
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The file's bytes, unless it is a part of the document.
    pub(crate) fn bytes(&self) -> Option<&Arc<[u8]>> {
        match &self.source {
            FileSource::Bytes(bytes) => Some(bytes),
            FileSource::Part { .. } => None,
        }
    }
}

impl std::fmt::Debug for FontFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("FontFile");
        match &self.source {
            FileSource::Bytes(bytes) => out.field("bytes", &bytes.len()),
            FileSource::Part { name, key } => {
                out.field("part", name).field("keyed", &key.is_some())
            }
        };
        out.field("offset", &self.offset).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontError {
    Malformed(&'static str),
    Unnamed,
    Unsupported(&'static str),
    CmapUnsorted,
    CmapReversedRange,
    CmapOutOfRange,
    TooLarge,
}

impl FontError {
    pub(crate) fn into_error(self) -> Error {
        let (reason, message) = match self {
            FontError::Malformed(table) => (
                "font_malformed",
                format!("font table {table} is missing or malformed"),
            ),
            FontError::Unnamed => (
                "font_malformed",
                "the font names no family or style (name ids 1 and 2)".into(),
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

    /// Its MATH table's script geometry, if it has one.
    pub(crate) fn math_scripts(&self) -> Option<MathScripts> {
        self.math
    }

    /// The font file the face was read from, for subsetting.
    pub fn file(&self) -> Option<&FontFile> {
        self.file.as_deref()
    }

    /// Read from a font file whose licence allows embedding its outlines.
    pub(crate) fn embeddable(&self) -> bool {
        self.file.is_some() && self.embeddable
    }

    /// Draws `ch` from a program a PDF may embed.
    pub(crate) fn draws(&self, ch: char) -> bool {
        self.embeddable() && self.glyph(ch).is_some()
    }

    /// The style a variable face's advances were measured at, which the
    /// PDF embeds; `None`: the font's default instance.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    pub(crate) fn instance(&self) -> Option<Style> {
        self.instance
    }

    /// This face at the instance `style` selects ([`Fvar::variation`]),
    /// with the advances `subsetter` writes for it; `None` for a static
    /// face, a document's own font, or the default instance.
    fn instanced(&self, style: Style) -> Option<Face> {
        use skrifa::{GlyphId, MetadataProvider, instance::Size};
        let file = self.file.as_deref()?;
        let data = file.bytes()?;
        let fvar = Ttf::new_at(data, file.offset).ok()?.table(b"fvar")?;
        let variation = Fvar::read(fvar).variation(style);
        if variation.is_empty() {
            return None;
        }
        let font = skrifa::FontRef::from_index(data, file.index).ok()?;
        let location = font.axes().location(
            variation
                .iter()
                .map(|(tag, value)| (skrifa::Tag::new(tag), *value)),
        );
        let metrics = font.glyph_metrics(Size::unscaled(), &location);
        let widths = self
            .glyphs
            .iter()
            .map(|glyph| {
                metrics
                    .advance_width(GlyphId::new(u32::from(*glyph)))
                    .map_or(0, |width| (width.round() as u16).min(u16::MAX - 1))
            })
            .collect();
        let axis = |tag: &[u8; 4]| variation.iter().find(|(t, _)| t == tag).map(|(_, v)| *v);
        Some(Face {
            bold: self.bold || axis(b"wght").is_some_and(|weight| weight >= 600.0),
            italic: self.italic || axis(b"ital").is_some() || axis(b"slnt").is_some(),
            widths,
            instance: Some(style),
            ..self.clone()
        })
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

    /// The underline of this face.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    pub(crate) fn underline(&self) -> Stroke {
        self.underline.unwrap_or(UNDERLINE)
    }

    /// The strikethrough line of this face.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter reads it")
    )]
    pub(crate) fn strikeout(&self) -> Stroke {
        self.strikeout.unwrap_or(STRIKEOUT)
    }

    /// Single line height in ems (Word for Mac: hhea ascender + descender
    /// + line gap).
    pub fn line_height(&self) -> f64 {
        (self.ascender + self.descender + self.line_gap) / self.units_per_em
    }

    /// Superscript (`true`) or subscript size in ems.
    pub(crate) fn script_size(&self, superscript: bool) -> f64 {
        self.script_sizes[usize::from(!superscript)]
    }

    /// Ascent plus line gap in ems: Word for Mac sets the gap above the
    /// text, so on a line mixing fonts it adds to the ascent.
    pub fn ascent(&self) -> f64 {
        (self.ascender + self.line_gap) / self.units_per_em
    }

    /// The hhea descender in ems, positive below the baseline.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF writer reads it")
    )]
    pub(crate) fn descent(&self) -> f64 {
        self.descender / self.units_per_em
    }

    /// Metrics read from a TrueType/OpenType font file (one face of a
    /// collection), keeping the advances of every mapped code point.
    pub fn parse(data: &[u8], family: &str, bold: bool, italic: bool) -> Option<Face> {
        Face::try_parse(data, family, bold, italic).ok()
    }

    /// [`Face::parse`], saying why a face could not be read
    /// (`invalid_argument`: `font_malformed`, `font_cmap_invalid`,
    /// `font_too_large`, or `font_format_unsupported` for a cmap without
    /// format 4 or 12).
    pub fn try_parse(data: &[u8], family: &str, bold: bool, italic: bool) -> Result<Face> {
        let mut budget = FILE_BUDGET;
        Ttf::face(data, 0)
            .and_then(|ttf| Face::from_ttf(&ttf, family, bold, italic, &mut budget))
            .map_err(FontError::into_error)
    }

    /// The first face of an embedded font, read from `plain` (part `part`
    /// deobfuscated with `key`).
    pub(crate) fn parse_embedded(
        plain: &[u8],
        part: &str,
        key: Option<[u8; 16]>,
        family: &str,
        budget: &mut usize,
    ) -> Parsed<Face> {
        let offset = face_view(plain, 0).ok_or(FontError::Malformed("ttcf"))?;
        let face = Face::from_ttf(&Ttf::new_at(plain, offset)?, family, false, false, budget)?;
        let name = part.to_string();
        Ok(face.with_file(FontFile {
            source: FileSource::Part { name, key },
            offset,
            index: 0,
        }))
    }

    /// Face `index` of a font file, read as a font set reads it, within
    /// one file's parse budget.
    #[cfg(feature = "pdf")]
    pub(crate) fn read_at(data: &Arc<[u8]>, index: usize) -> Result<Face> {
        let mut budget = FILE_BUDGET;
        read_face(data, index, &mut budget)
            .map(|named| named.face)
            .map_err(FontError::into_error)
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
        let os2 = ttf.table(b"OS/2");
        let post = ttf.table(b"post");
        let average_width = os2
            .and_then(|os2| be16(os2, 2))
            .map_or(units_per_em / 2.0, |w| f64::from(w as i16).max(1.0));
        let script_size = |at| {
            os2.and_then(|os2| be16(os2, at))
                .map(|size| f64::from(size as i16) / units_per_em)
                .filter(|size| *size > 0.0 && *size <= 1.0)
                .unwrap_or(SCRIPT_SIZE)
        };
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
            embeddable: licence(ttf).0,
            units_per_em,
            ascender,
            descender,
            line_gap,
            average_width,
            script_sizes: [script_size(20), script_size(12)],
            codepoints: mapping.iter().map(|(c, _)| *c).collect(),
            widths: mapping.iter().map(|(_, glyph)| advance(*glyph)).collect(),
            glyphs: mapping.iter().map(|(_, glyph)| *glyph).collect(),
            kerning: kerning.into(),
            underline: post.and_then(|post| {
                Stroke::read(be16(post, 8), be16(post, 10), units_per_em, -0.5..=0.0)
            }),
            strikeout: os2.and_then(|os2| {
                Stroke::read(be16(os2, 28), be16(os2, 26), units_per_em, 0.0..=1.0)
            }),
            math: ttf
                .table(b"MATH")
                .and_then(|table| MathScripts::read(table, units_per_em)),
            file: None,
            instance: None,
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
    /// Every face of a font file (one, or each of the first 64 of a
    /// collection), named and styled the way documents name fonts: the
    /// family, with a style other than regular, bold, italic or bold italic
    /// appended. Faces that cannot be read are skipped.
    pub fn parse_file(data: &[u8]) -> Vec<Face> {
        let mut budget = FILE_BUDGET;
        read_faces(&Arc::from(data), &mut budget)
            .into_iter()
            .filter_map(|read| read.ok().map(|named| named.face))
            .collect()
    }
}

/// The script size of Arial, Times New Roman, Georgia and Courier New
/// (1331/2048 em), for a face whose OS/2 table gives none.
const SCRIPT_SIZE: f64 = 1331.0 / 2048.0;

/// Faces read from one collection; the rest are left unread.
const MAX_COLLECTION_FACES: usize = 64;

/// A face read from a font file, with the other names it answers to.
struct Named {
    face: Face,
    info: FontInfo,
    /// Full name (name id 4), PostScript name (id 6), and the name Word
    /// for Mac gives a variable font.
    names: [Option<String>; 3],
    /// Why a font set refuses the face, though its metrics read.
    unsupported: Option<&'static str>,
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
    (0..count.min(MAX_COLLECTION_FACES))
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
    let [Some(family), Some(subfamily), full, postscript] = ttf
        .table(b"name")
        .map(|table| names(table, [1, 2, 4, 6]))
        .unwrap_or_default()
    else {
        return Err(FontError::Unnamed);
    };
    let cff = ttf.table(b"CFF ").is_some();
    let unsupported = if ttf.table(b"CFF2").is_some() {
        Some("CFF2 fonts are not supported")
    } else if ttf.table(b"glyf").is_none() && !cff {
        Some("bitmap-only fonts are not supported")
    } else {
        None
    };
    let (family, bold, italic) = document_style(family, &subfamily);
    let word_family = word_family(&ttf, &family);
    let face = Face::from_ttf(&ttf, &family, bold, italic, budget)
        .map_err(|error| unsupported.map_or(error, FontError::Unsupported))?;
    let (embeddable, subsettable) = licence(&ttf);
    let info = FontInfo {
        family,
        bold,
        italic,
        variable: ttf.table(b"fvar").is_some(),
        cff,
        embeddable,
        subsettable,
        word_family: word_family.clone(),
    };
    Ok(Named {
        face: face.with_file(FontFile {
            source: FileSource::Bytes(data.clone()),
            offset,
            index: index as u32,
        }),
        info,
        names: [full, postscript, word_family],
        unsupported,
    })
}

/// The family Word for Mac lists a variable font under, when it is not the
/// family: the family and the default optical size's label.
fn word_family(ttf: &Ttf<'_>, family: &str) -> Option<String> {
    let fvar = Fvar::read(ttf.table(b"fvar")?);
    let label = optical_label(&fvar, ttf.table(b"STAT")?)?;
    let [label] = names(ttf.table(b"name")?, [label]);
    Some(format!("{family} {}", label?))
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

/// OS/2 `fsType` as (embeddable, subsettable). Restricted licence (bit 1)
/// yields to editable or preview and print (bits 2, 3), as versions before
/// 3 allow; bitmap only (bit 9) and no subsetting (bit 8) exist from
/// version 2.
pub(crate) fn licence(ttf: &Ttf<'_>) -> (bool, bool) {
    let Some(os2) = ttf.table(b"OS/2") else {
        return (true, true);
    };
    let fs_type = be16(os2, 8).unwrap_or(0);
    let defined = |bit: u16| be16(os2, 0).is_some_and(|version| version >= 2) && fs_type & bit != 0;
    let restricted = fs_type & 0x000E == 0x0002;
    (!restricted && !defined(0x0200), !defined(0x0100))
}

/// Offset of face `index`'s table directory.
fn face_view(data: &[u8], index: usize) -> Option<usize> {
    if data.get(..4)? == b"ttcf" {
        Some(be32(data, 12 + 4 * index)? as usize)
    } else {
        (index == 0).then_some(0)
    }
}

/// Names `ids` from a name table: Windows English, else Unicode, else
/// Macintosh Roman.
pub(crate) fn names<const N: usize>(table: &[u8], ids: [u16; N]) -> [Option<String>; N] {
    let mut best: [Option<(u8, String)>; N] = std::array::from_fn(|_| None);
    let (Some(count), Some(strings)) = (be16(table, 2), be16(table, 4)) else {
        return std::array::from_fn(|_| None);
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
        let rank = match (platform, language) {
            (3, 0x409 | 0) => 3,
            (0, _) => 2,
            (1, 0) => 1,
            _ => continue,
        };
        let Some(slot) = ids.iter().position(|wanted| *wanted == id) else {
            continue;
        };
        if best[slot].as_ref().is_some_and(|(kept, _)| *kept >= rank) {
            continue;
        }
        let start = usize::from(strings) + usize::from(offset);
        let Some(raw) = table.get(start..start + usize::from(length)) else {
            continue;
        };
        let text = if platform == 1 {
            raw.iter().map(|b| char::from(*b)).collect()
        } else {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        };
        best[slot] = Some((rank, text));
    }
    best.map(|name| name.map(|(_, text)| text))
}

pub(crate) fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

pub(crate) fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The table directory of one face of a font file.
pub(crate) struct Ttf<'a> {
    data: &'a [u8],
    tables: Vec<([u8; 4], usize, usize)>,
}

impl<'a> Ttf<'a> {
    /// Face `index` of a font file or collection.
    pub(crate) fn face(data: &'a [u8], index: u32) -> Parsed<Ttf<'a>> {
        let view = face_view(data, index as usize).ok_or(FontError::Malformed("ttcf"))?;
        Ttf::new_at(data, view)
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

    pub(crate) fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        let (_, start, length) = self.tables.iter().find(|(t, _, _)| t == tag)?;
        self.data.get(*start..start + length)
    }

    /// The tables in directory order, as `(tag, data)`.
    #[cfg(feature = "pdf")]
    pub(crate) fn tables(&self) -> impl Iterator<Item = ([u8; 4], &'a [u8])> + '_ {
        self.tables
            .iter()
            .map(|&(tag, start, length)| (tag, &self.data[start..start + length]))
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
    let (_, offset, format) = best.ok_or(match count {
        0 => malformed,
        _ => FontError::Unsupported("the font has no Unicode cmap (format 4 or 12)"),
    })?;
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
            embeddable: false,
            units_per_em,
            ascender,
            descender,
            line_gap,
            average_width,
            // The table has no OS/2 script sizes; every measured Microsoft font's is this.
            script_sizes: [SCRIPT_SIZE; 2],
            codepoints,
            widths: widths.into(),
            glyphs: Arc::new([]),
            kerning: kerning.into(),
            underline: None,
            strikeout: None,
            math: None,
            file: None,
            instance: None,
        }));
    }
    Some(out)
}

/// How Word for Mac substitutes a font it does not have (measured): a font
/// the font table declares roman, or does not declare at all, is laid out
/// in Cambria; any other declared font in Calibri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Has CFF outlines (an OpenType `.otf`), not TrueType ones.
    pub cff: bool,
    /// The font's `fsType` allows embedding its outlines.
    pub embeddable: bool,
    /// The font's `fsType` allows embedding a subset.
    pub subsettable: bool,
    /// The name Word for Mac gives a variable font, when it is not
    /// `family` ("Fraunces 9pt"); the face answers to both.
    pub word_family: Option<String>,
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
    aliases: HashMap<String, Alias>,
    /// Lower-cased family per [`FontKind`].
    fallbacks: [Option<String>; 3],
}

#[derive(Debug, Clone)]
struct Alias {
    to: String,
    /// One of [`METRIC_ALIASES`].
    tested: bool,
}

/// Families whose advances equal those of Word's font over every code point
/// both map but U+2010 (`metric_aliases_match_words_fonts`).
const METRIC_ALIASES: &[(&str, &str)] = &[
    ("arial", "liberation sans"),
    ("times new roman", "liberation serif"),
    ("times", "liberation serif"),
    ("courier new", "liberation mono"),
];

impl FontSet {
    pub fn builder() -> FontSetBuilder {
        FontSetBuilder {
            faces: Vec::new(),
            infos: Vec::new(),
            names: Vec::new(),
            aliases: HashMap::new(),
            fallbacks: Default::default(),
            budget: FILE_BUDGET,
        }
    }

    /// Every face, in the order added.
    pub fn faces(&self) -> &[FontInfo] {
        &self.0.infos
    }

    /// Whether a face answers to `name` (family, full, PostScript or Word's
    /// name), ignoring case.
    pub fn provides(&self, name: &str) -> bool {
        self.0.index.contains_key(&name.trim().to_lowercase())
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
                cff: false,
                embeddable: false,
                subsettable: false,
                word_family: None,
            })
            .collect();
        let faces: Vec<Arc<Face>> = faces.into_iter().map(Arc::new).collect();
        FontSet(Arc::new(FontSetInner {
            index: index(&faces),
            faces,
            infos,
            ..FontSetInner::default()
        }))
    }
}

/// Collects font files for a [`FontSet`].
#[derive(Debug)]
pub struct FontSetBuilder {
    faces: Vec<Arc<Face>>,
    infos: Vec<FontInfo>,
    /// Each face's other names ([`Named::names`]).
    names: Vec<[Option<String>; 3]>,
    aliases: HashMap<String, Alias>,
    fallbacks: [Option<String>; 3],
    budget: usize,
}

impl FontSetBuilder {
    /// Adds every face of a TTF, OTF, TTC or OTC file (the first 64 of a
    /// collection). Refuses the whole file, adding nothing, when a face
    /// cannot be read (`invalid_argument`: `font_format_unsupported`,
    /// `font_malformed`, `font_cmap_invalid` or `font_too_large`). The
    /// files added share one parse budget, which a refused file spends too.
    pub fn add(&mut self, bytes: impl Into<Arc<[u8]>>) -> Result<Vec<FontInfo>> {
        let faces = read_faces(&bytes.into(), &mut self.budget)
            .into_iter()
            .map(|read| read.and_then(supported))
            .collect::<Parsed<Vec<Named>>>()
            .and_then(|faces| {
                if faces.is_empty() {
                    Err(FontError::Malformed("ttcf"))
                } else {
                    Ok(faces)
                }
            })
            .map_err(FontError::into_error)?;
        let infos: Vec<FontInfo> = faces.iter().map(|named| named.info.clone()).collect();
        for named in faces {
            self.faces.push(Arc::new(named.face));
            self.infos.push(named.info);
            self.names.push(named.names);
        }
        Ok(infos)
    }

    /// Lays out `family` with `to`'s faces (from this set, else the bundled
    /// table) when neither the document nor this set has faces of
    /// `family`. Aliases are single-hop: `to` is not itself aliased. An
    /// alias to a family found nowhere is ignored. A PDF drawn through one
    /// reports it (`pdf_font_substituted`).
    pub fn alias(&mut self, family: &str, to: &str) -> &mut Self {
        self.aliases.insert(
            family.trim().to_lowercase(),
            Alias {
                to: to.trim().to_lowercase(),
                tested: false,
            },
        );
        self
    }

    /// Adds the aliases whose advances equal those of Word's font, so a
    /// PDF drawn through them breaks lines as Word does: Arial to
    /// Liberation Sans, Times New Roman and Times to Liberation Serif,
    /// Courier New to Liberation Mono. Off by default; aliases already set
    /// are kept. One exception: Word's Arial and Times New Roman draw
    /// U+2010 HYPHEN wider than the Liberation fonts do.
    pub fn metric_aliases(&mut self) -> &mut Self {
        for (family, to) in METRIC_ALIASES {
            self.aliases.entry(family.to_string()).or_insert(Alias {
                to: to.to_string(),
                tested: true,
            });
        }
        self
    }

    /// For a PDF ([`LayoutTarget::Pdf`]), lays out and draws in `family`'s
    /// faces from this set a family of `kind` that neither the document
    /// nor this set provides (nor an alias), and draws in it a character
    /// the run's face lacks; the PDF reports it (`pdf_font_substituted`).
    /// Word's layout ignores it, so a .docx's contents page numbers keep
    /// the metrics of the fonts Word has. Symbol fonts never fall back:
    /// their characters stand for others.
    pub fn fallback(&mut self, kind: FontKind, family: &str) -> &mut Self {
        self.fallbacks[kind as usize] = Some(family.trim().to_lowercase());
        self
    }

    pub fn build(&self) -> FontSet {
        let mut index = index(&self.faces);
        let families: HashSet<String> = index.keys().cloned().collect();
        for (at, names) in self.names.iter().enumerate() {
            for key in names.iter().flatten().map(|name| name.to_lowercase()) {
                if !families.contains(&key) {
                    index.entry(key).or_default()[slot(&self.faces[at])].get_or_insert(at);
                }
            }
        }
        FontSet(Arc::new(FontSetInner {
            faces: self.faces.clone(),
            infos: self.infos.clone(),
            index,
            aliases: self.aliases.clone(),
            fallbacks: self.fallbacks.clone(),
        }))
    }
}

fn supported(named: Named) -> Parsed<Named> {
    match named.unsupported {
        Some(message) => Err(FontError::Unsupported(message)),
        None => Ok(named),
    }
}

/// What a layout measures for. Experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum LayoutTarget {
    /// Word's pages (the contents page numbers of a .docx): a family Word
    /// has keeps the bundled table's metrics, and declared fallbacks are
    /// ignored.
    #[default]
    Word,
    /// A PDF, which draws with the faces that measure it: declared
    /// fallbacks apply, and a face whose licence forbids embedding counts
    /// as missing unless it is referenced by name (`allow_unembedded`).
    Pdf { allow_unembedded: bool },
}

/// Faces by family for one layout: the document's embedded fonts first,
/// then the caller's [`FontSet`] (the family's own faces, an alias, and
/// for a PDF the declared fallback for its kind), then the bundled table.
#[derive(Default)]
pub struct FontBook {
    own: Vec<Arc<Face>>,
    own_index: HashMap<String, [Option<usize>; 4]>,
    fonts: FontSet,
    target: LayoutTarget,
    /// Lower-cased family and kind → how its faces were found.
    cache: HashMap<(String, FontKind), Option<(Found, Slots)>>,
    /// Families that had to be substituted, as the document names them.
    pub missing: Vec<String>,
    missing_seen: HashSet<String>,
    /// Variable faces by address, at each style asked of them.
    instances: HashMap<(usize, Style), Arc<Face>>,
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

/// Whether the bundled table measures `family` (its names are public).
#[cfg_attr(not(feature = "pdf"), allow(dead_code, reason = "PDF reports redact"))]
pub(crate) fn bundled(family: &str) -> bool {
    builtin_index().contains_key(canonical(&family.trim().to_lowercase()))
}

/// Fonts whose characters stand for others, so no other font draws them
/// as they are.
pub(crate) fn symbol_font(family: &str) -> bool {
    matches!(
        family.trim().to_lowercase().as_str(),
        "symbol" | "wingdings" | "wingdings 2" | "wingdings 3" | "webdings" | "ms gothic"
    )
}

#[derive(Clone, Copy)]
enum Source {
    Own,
    Fonts,
    Builtin,
}

/// A family's face per (bold, italic) slot, and where each comes from.
type Slots = [Option<(Source, usize)>; 4];

fn tagged(source: Source, faces: Option<&[Option<usize>; 4]>) -> Option<Slots> {
    faces.map(|faces| faces.map(|face| face.map(|at| (source, at))))
}

/// How a family's faces were found.
#[derive(Clone, Copy)]
enum Found {
    Direct,
    Alias { tested: bool },
    Fallback,
    Builtin,
    Substitute,
}

/// Where the face that measures a run comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "pdf"),
    allow(dead_code, reason = "the PDF painter reads it")
)]
pub(crate) enum FaceSource {
    /// A font the document embeds.
    Document,
    /// The caller's face of the family.
    Supplied,
    /// An alias's face; `tested`: one of the metric aliases.
    Alias { tested: bool },
    /// The declared fallback for the family's kind.
    Fallback,
    /// A face of another family, for a character the run's face lacks.
    Glyph,
    /// The bundled table: metrics without a font program.
    Measured,
    /// Word's substitute for a family found nowhere.
    Substitute,
}

/// A run's face and where it comes from.
#[derive(Clone)]
pub(crate) struct Resolved {
    pub face: Arc<Face>,
    pub source: FaceSource,
    /// Drawn in another style of the family, though Word's fonts have this
    /// one (the bundled table).
    pub style_missing: bool,
    /// The family's own face was passed over: its licence forbids
    /// embedding it in a PDF.
    pub restricted: bool,
}

/// Names Word resolves to another font.
fn canonical(key: &str) -> &str {
    match key {
        "times" => "times new roman",
        _ => key,
    }
}

fn builtin_slots(key: &str) -> Option<Slots> {
    tagged(Source::Builtin, builtin_index().get(canonical(key)))
}

/// The slot to draw `wanted` from: it, else the nearest style the family
/// has (Word synthesizes the rest).
fn pick(set: &Slots, wanted: usize) -> (Source, usize) {
    [wanted, wanted & 1, wanted & 2, 0, 1, 2, 3]
        .iter()
        .find_map(|slot| set[*slot])
        .expect("a face set has a face")
}

fn wanted(style: Style) -> usize {
    usize::from(style.weight >= 600) | (usize::from(style.italic) << 1)
}

impl FontBook {
    /// A font book with `own` faces taking precedence over the table.
    pub fn new(own: Vec<Face>) -> FontBook {
        FontBook::with_fonts(own, &FontSet::default())
    }

    /// A font book with `own` faces first, then `fonts`, then the table.
    pub(crate) fn with_fonts(own: Vec<Face>, fonts: &FontSet) -> FontBook {
        let own: Vec<Arc<Face>> = own.into_iter().map(Arc::new).collect();
        FontBook {
            own_index: index(&own),
            own,
            fonts: fonts.clone(),
            ..FontBook::default()
        }
    }

    /// This book laying out for `target`.
    pub(crate) fn with_target(self, target: LayoutTarget) -> FontBook {
        FontBook { target, ..self }
    }

    /// The faces of `key` itself: the document's, with the styles it lacks
    /// filled from the supplied ones.
    fn direct(&self, key: &str) -> Option<Slots> {
        let own = self.drawable(self.own_slots(key));
        match (own, self.supplied(key)) {
            (Some(own), Some(supplied)) => Some(std::array::from_fn(|at| own[at].or(supplied[at]))),
            (own, supplied) => own.or(supplied),
        }
    }

    fn own_slots(&self, key: &str) -> Option<Slots> {
        tagged(Source::Own, self.own_index.get(canonical(key)))
    }

    fn supplied_slots(&self, key: &str) -> Option<Slots> {
        tagged(Source::Fonts, self.fonts.0.index.get(canonical(key)))
    }

    fn supplied(&self, key: &str) -> Option<Slots> {
        self.drawable(self.supplied_slots(key))
    }

    /// For a PDF that embeds every font, `slots` without the faces whose
    /// licence forbids it; `None` when none is left.
    fn drawable(&self, slots: Option<Slots>) -> Option<Slots> {
        let mut slots = slots?;
        if self.target
            == (LayoutTarget::Pdf {
                allow_unembedded: false,
            })
        {
            for slot in &mut slots {
                if slot.is_some_and(|(from, at)| self.forbids_embedding(from, at)) {
                    *slot = None;
                }
            }
        }
        slots.iter().any(Option::is_some).then_some(slots)
    }

    fn forbids_embedding(&self, from: Source, at: usize) -> bool {
        let face = self.face_at(from, at);
        face.file().is_some() && !face.embeddable()
    }

    /// `key` has a face of its own that [`FontBook::drawable`] passes over.
    fn restricted(&self, key: &str) -> bool {
        let passed_over = |slots: Option<Slots>| {
            slots.is_some_and(|slots| {
                slots
                    .iter()
                    .flatten()
                    .any(|(from, at)| self.forbids_embedding(*from, *at))
            })
        };
        self.target
            == (LayoutTarget::Pdf {
                allow_unembedded: false,
            })
            && (passed_over(self.own_slots(key)) || passed_over(self.supplied_slots(key)))
    }

    /// An alias's faces: supplied ones, else (an alias the caller set) the
    /// table's.
    fn alias(&self, key: &str) -> Option<(Found, Slots)> {
        let alias = self.fonts.0.aliases.get(key)?;
        let measured = || builtin_slots(&alias.to).filter(|_| !alias.tested);
        let slots = self.supplied(&alias.to).or_else(measured)?;
        Some((
            Found::Alias {
                tested: alias.tested,
            },
            slots,
        ))
    }

    /// For a PDF, the declared fallbacks, `kind`'s first; none for a
    /// symbol font.
    fn fallbacks(&self, key: &str, kind: FontKind) -> Vec<Slots> {
        if self.target == LayoutTarget::Word || symbol_font(key) {
            return Vec::new();
        }
        let declared = &self.fonts.0.fallbacks;
        std::iter::once(kind as usize)
            .chain(0..declared.len())
            .filter_map(|at| declared[at].as_deref())
            .filter_map(|family| self.supplied(family))
            .collect()
    }

    fn found(&mut self, family: &str, kind: FontKind) -> Option<(Found, Slots)> {
        let key = family.trim().to_lowercase();
        if let Some(found) = self.cache.get(&(key.clone(), kind)) {
            return *found;
        }
        let declared = self.fonts.0.fallbacks[kind as usize].as_deref();
        let for_pdf = self.target != LayoutTarget::Word;
        let found = self
            .direct(&key)
            .map(|slots| (Found::Direct, slots))
            .or_else(|| self.alias(&key))
            .or_else(|| {
                let fallback = declared.filter(|_| for_pdf && !symbol_font(&key))?;
                Some((Found::Fallback, self.supplied(fallback)?))
            })
            .or_else(|| builtin_slots(&key).map(|slots| (Found::Builtin, slots)));
        self.cache.insert((key, kind), found);
        found
    }

    /// Whether metrics exist for `family` (a declared fallback aside).
    pub fn knows(&mut self, family: &str) -> bool {
        let key = family.trim().to_lowercase();
        self.direct(&key).is_some() || self.alias(&key).is_some() || builtin_slots(&key).is_some()
    }

    /// Whether the document embeds `family` or the caller supplied it.
    pub(crate) fn supplies(&mut self, family: &str) -> bool {
        let key = family.trim().to_lowercase();
        let slots = self
            .direct(&key)
            .or_else(|| self.alias(&key).map(|(_, slots)| slots));
        slots.is_some_and(|slots| {
            slots
                .iter()
                .flatten()
                .any(|(source, _)| !matches!(source, Source::Builtin))
        })
    }

    /// The face for `family` in a style: the exact style, else the nearest
    /// one the family has (Word synthesizes the rest); a family with no
    /// metrics takes `kind`'s substitute and is recorded in `missing`.
    pub fn face(&mut self, family: &str, bold: bool, italic: bool, kind: FontKind) -> Arc<Face> {
        self.resolve(family, Style::of(bold, italic), kind).face
    }

    /// [`FontBook::face`] for a run drawn at `style.size`, which picks a
    /// variable face's optical size, with where the face comes from.
    pub(crate) fn resolve(&mut self, family: &str, style: Style, kind: FontKind) -> Resolved {
        let wanted = wanted(style);
        let (found, set) = match self.found(family, kind) {
            Some(found) => found,
            None => (Found::Substitute, self.substitute(family, kind)),
        };
        let (from, at) = pick(&set, wanted);
        let face = self.styled(self.face_at(from, at), style);
        let source = match found {
            Found::Direct if matches!(from, Source::Own) => FaceSource::Document,
            Found::Direct => FaceSource::Supplied,
            Found::Alias { tested } => FaceSource::Alias { tested },
            Found::Fallback => FaceSource::Fallback,
            Found::Builtin => FaceSource::Measured,
            Found::Substitute => FaceSource::Substitute,
        };
        let synthesized = slot(&face) != wanted;
        let key = family.trim().to_lowercase();
        let style_missing = synthesized
            && matches!(found, Found::Direct | Found::Alias { .. })
            && builtin_slots(&key).is_some_and(|slots| slots[wanted].is_some());
        Resolved {
            face,
            source,
            style_missing,
            restricted: self.restricted(&key),
        }
    }

    /// `kind`'s substitute for `family`, which is recorded as missing.
    fn substitute(&mut self, family: &str, kind: FontKind) -> Slots {
        if !family.is_empty() && self.missing_seen.insert(family.to_ascii_lowercase()) {
            self.missing.push(family.to_string());
        }
        let substitute = match kind {
            FontKind::Serif => "cambria",
            FontKind::Sans | FontKind::Mono => "calibri",
        };
        self.direct(substitute)
            .or_else(|| builtin_slots(substitute))
            .expect("bundled substitute fonts")
    }

    fn face_at(&self, source: Source, at: usize) -> Arc<Face> {
        match source {
            Source::Own => self.own[at].clone(),
            Source::Fonts => self.fonts.0.faces[at].clone(),
            Source::Builtin => builtin()[at].clone(),
        }
    }

    /// The first face after `family`'s own that draws `ch` from a program
    /// a PDF may embed: a supplied face of the family, its alias's, then
    /// the declared fallbacks, `kind`'s first.
    pub(crate) fn glyph_fallback(
        &mut self,
        family: &str,
        style: Style,
        kind: FontKind,
        ch: char,
    ) -> Option<Arc<Face>> {
        let key = family.trim().to_lowercase();
        let sets: Vec<Slots> = [
            self.supplied(&key),
            self.alias(&key).map(|(_, slots)| slots),
        ]
        .into_iter()
        .flatten()
        .chain(self.fallbacks(&key, kind))
        .collect();
        self.first_drawing(&sets, style, ch)
    }

    /// The first declared fallback that draws `ch`.
    #[cfg_attr(
        not(feature = "pdf"),
        allow(dead_code, reason = "the PDF painter calls it")
    )]
    pub(crate) fn declared_drawing(&mut self, style: Style, ch: char) -> Option<Arc<Face>> {
        let sets = self.fallbacks("", FontKind::Serif);
        self.first_drawing(&sets, style, ch)
    }

    fn first_drawing(&mut self, sets: &[Slots], style: Style, ch: char) -> Option<Arc<Face>> {
        for set in sets {
            let (from, at) = pick(set, wanted(style));
            let face = self.styled(self.face_at(from, at), style);
            if face.draws(ch) {
                return Some(face);
            }
        }
        None
    }

    /// A variable face at the instance `style` selects, so a run is
    /// measured with the advances the PDF embeds for it.
    fn styled(&mut self, face: Arc<Face>, style: Style) -> Arc<Face> {
        let key = (Arc::as_ptr(&face) as usize, style);
        if let Some(instance) = self.instances.get(&key) {
            return instance.clone();
        }
        let instance = face.instanced(style).map_or(face, Arc::new);
        self.instances.insert(key, instance.clone());
        instance
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
    fn a_decoration_line_out_of_bounds_is_left_to_the_default() {
        let read = |top: i16, thickness: i16| {
            Stroke::read(Some(top as u16), Some(thickness as u16), 1000.0, -0.5..=0.0)
        };
        assert_eq!(
            read(-100, 50),
            Some(Stroke {
                offset: -0.125,
                thickness: 0.05
            })
        );
        assert_eq!(read(-100, 0), None);
        assert_eq!(read(-100, 900), None);
        assert_eq!(read(-2000, 50), None);
        assert_eq!(read(300, 50), None);
    }

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
        Face::from_ttf(&Ttf::face(font, 0)?, "Test", false, false, budget)
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
    use super::hostile::{
        Tables, ab_cmap, base_tables, be, collection, format12, name_table, sfnt,
    };
    use super::*;

    fn font(style: &str, extra: Tables) -> Vec<u8> {
        family_font("Brand", style, extra)
    }

    fn family_font(family: &str, style: &str, extra: Tables) -> Vec<u8> {
        let mut tables = base_tables(3, ab_cmap(), 3);
        let (full, postscript) = (format!("{family} {style}"), format!("{family}-{style}"));
        tables.push((
            *b"name",
            name_table(&[(1, family), (2, style), (4, &full), (6, &postscript)]),
        ));
        tables.extend(extra);
        sfnt(&tables)
    }

    fn os2(version: u16, fs_type: u16) -> ([u8; 4], Vec<u8>) {
        let mut table = vec![0; 96];
        table[0..2].copy_from_slice(&version.to_be_bytes());
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
        let document = crate::Document::new();
        let txn = Txn::new(&document);
        assert_eq!(*file.read(&txn).unwrap(), *ttc);
        assert_eq!(file.offset(), 20 + regular.len());
        let regular_face = book.face("Brand", false, false, FontKind::Sans);
        assert_eq!(regular_face.file().unwrap().offset(), 20);
    }

    #[test]
    fn script_sizes_are_read_from_os2_or_fall_back_to_1331_of_2048() {
        let face = |sizes: Option<(i16, i16)>| {
            let extra = sizes.map(|(sup, sub)| {
                let (tag, mut table) = os2(4, 0);
                table[12..14].copy_from_slice(&sub.to_be_bytes());
                table[20..22].copy_from_slice(&sup.to_be_bytes());
                (tag, table)
            });
            let face = &Face::parse_file(&font("Regular", extra.into_iter().collect()))[0];
            [face.script_size(true), face.script_size(false)]
        };
        assert_eq!(face(Some((600, 550))), [0.6, 0.55]);
        for absurd in [None, Some((0, -300)), Some((1001, i16::MIN))] {
            assert_eq!(face(absurd), [SCRIPT_SIZE; 2], "{absurd:?}");
        }
        let arial = FontBook::default().face("Arial", false, false, FontKind::Sans);
        assert_eq!(arial.script_size(true), 1331.0 / 2048.0);
    }

    #[test]
    fn a_variable_font_is_accepted_and_flagged() {
        let fonts = set_of(&[font("Regular", vec![(*b"fvar", vec![0; 16])])]);
        assert!(fonts.faces()[0].variable);
        assert!(!set_of(&[font("Regular", vec![])]).faces()[0].variable);
    }

    #[test]
    fn fs_type_is_read_per_os2_version() {
        for (version, fs_type, embeddable, subsettable) in [
            (4, 0x0000, true, true),
            (4, 0x0002, false, true),
            (4, 0x0003, false, true),
            (4, 0x0004, true, true),
            (4, 0x0008, true, true),
            (1, 0x000A, true, true),
            (4, 0x0100, true, false),
            (4, 0x0200, false, true),
            (2, 0x0308, false, false),
            (1, 0x0300, true, true),
        ] {
            let fonts = set_of(&[font("Regular", vec![os2(version, fs_type)])]);
            let face = &fonts.faces()[0];
            assert_eq!(
                (face.embeddable, face.subsettable),
                (embeddable, subsettable),
                "version {version}, {fs_type:#06x}"
            );
        }
    }

    #[test]
    fn an_alias_lays_a_family_out_in_another() {
        let mut builder = FontSet::builder();
        builder.add(font("Regular", vec![])).unwrap();
        let fonts = builder
            .alias("Calibri", "Brand")
            .alias("Helvetica", "Arial")
            .build();
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
    fn an_alias_to_a_family_found_nowhere_is_ignored() {
        let fonts = FontSet::builder()
            .alias("Calibri", "Not Loaded")
            .alias("Arial", "Not Loaded")
            .alias("Cambria", "Not Loaded")
            .build();
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        assert!(book.knows("Arial") && book.knows("Calibri"));
        let arial = book.face("Arial", false, false, FontKind::Sans);
        assert_eq!(arial.family, "Arial");
        assert_eq!(
            book.face("No Such Font", false, false, FontKind::Sans)
                .family,
            "Calibri"
        );
        assert_eq!(
            book.face("No Such Serif", false, false, FontKind::Serif)
                .family,
            "Cambria"
        );
        assert_eq!(book.missing, ["No Such Font", "No Such Serif"]);
    }

    #[test]
    fn a_substitute_ignores_aliases() {
        let mut builder = FontSet::builder();
        builder.add(font("Regular", vec![])).unwrap();
        let fonts = builder.alias("Calibri", "Brand").build();
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        assert_eq!(
            book.face("No Such Font", false, false, FontKind::Sans)
                .family,
            "Calibri"
        );
    }

    #[test]
    fn aliases_apply_before_times_is_read_as_times_new_roman() {
        let mut builder = FontSet::builder();
        builder.add(font("Regular", vec![])).unwrap();
        let fonts = builder
            .alias("Times", "Brand")
            .alias("Foo", "Times")
            .build();
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        assert_eq!(
            book.face("Times", false, false, FontKind::Serif).family,
            "Brand"
        );
        assert_eq!(
            book.face("Foo", false, false, FontKind::Serif).family,
            "Times New Roman"
        );
        assert!(book.missing.is_empty());
        let mut plain = FontBook::default();
        assert_eq!(
            plain.face("Times", false, false, FontKind::Serif).family,
            "Times New Roman"
        );
    }

    #[test]
    fn embedded_and_supplied_faces_merge_per_style() {
        let regular = Face::try_parse(&font("Regular", vec![]), "Brand", false, false).unwrap();
        let bold = Face::try_parse(&font("Bold", vec![]), "Brand", true, false).unwrap();
        let mut book = FontBook::with_fonts(vec![regular], &FontSet::from_faces(vec![bold]));
        assert!(book.face("Brand", true, false, FontKind::Sans).bold);
        assert!(!book.face("Brand", false, false, FontKind::Sans).bold);
        assert!(!book.face("Brand", false, true, FontKind::Sans).bold);
    }

    #[test]
    fn unicode_platform_names_are_read() {
        let mut tables = base_tables(3, ab_cmap(), 3);
        let mut name = name_table(&[(1, "Brand"), (2, "Bold")]);
        for record in [6, 18] {
            name[record..record + 2].copy_from_slice(&0u16.to_be_bytes());
            name[record + 2..record + 4].copy_from_slice(&3u16.to_be_bytes());
        }
        tables.push((*b"name", name));
        let fonts = set_of(&[sfnt(&tables)]);
        assert_eq!(
            (&*fonts.faces()[0].family, fonts.faces()[0].bold),
            ("Brand", true)
        );
    }

    #[test]
    fn parse_file_keeps_faces_a_font_set_refuses() {
        let cff2 = font("Regular", vec![(*b"CFF2", vec![0; 8])]);
        let mut bitmap = font("Regular", vec![]);
        let glyf = bitmap.windows(4).position(|w| w == b"glyf").unwrap();
        bitmap[glyf..glyf + 4].copy_from_slice(b"EBDT");
        for file in [cff2, bitmap.clone()] {
            assert_eq!(Face::parse_file(&file).len(), 1);
            assert_eq!(refusal(file).1, "font_format_unsupported");
        }
        let head = bitmap.windows(4).position(|w| w == b"head").unwrap();
        bitmap[head..head + 4].copy_from_slice(b"bhed");
        assert!(Face::parse_file(&bitmap).is_empty());
        let error = FontSet::builder().add(bitmap).unwrap_err();
        assert_eq!(error.reason(), "font_format_unsupported");
        assert!(error.message().contains("bitmap-only"), "{error}");
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
        let error = FontSet::builder().add(nameless).unwrap_err();
        assert_eq!(error.reason(), "font_malformed");
        assert!(error.message().contains("names no family"), "{error}");
        let mut format13 = base_tables(3, Vec::new(), 3);
        format13[4].1 = be(&[0, 1, 3, 10, 0, 12, 13, 0, 0, 0, 28, 0, 0, 0, 0, 0, 0, 0, 1]);
        format13.push((*b"name", name_table(&[(1, "Brand"), (2, "Regular")])));
        assert_eq!(refusal(sfnt(&format13)), unsupported);
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

    fn resolve(book: &mut FontBook, family: &str, bold: bool, kind: FontKind) -> Resolved {
        book.resolve(family, Style::of(bold, false), kind)
    }

    #[test]
    fn a_declared_fallback_stands_in_for_a_family_found_nowhere() {
        let mut builder = FontSet::builder();
        builder.add(font("Regular", vec![])).unwrap();
        let fonts = builder.fallback(FontKind::Sans, "brand ").build();
        let mut word = FontBook::with_fonts(Vec::new(), &fonts);
        let calibri = resolve(&mut word, "Calibri", false, FontKind::Sans);
        assert_eq!(
            (calibri.face.family.as_str(), calibri.source),
            ("Calibri", FaceSource::Measured),
            "Word's pages keep Word's metrics"
        );
        let mut book = FontBook::with_fonts(Vec::new(), &fonts).with_target(PDF);
        let calibri = resolve(&mut book, "Calibri", false, FontKind::Sans);
        assert_eq!(
            (calibri.face.family.as_str(), calibri.source),
            ("Brand", FaceSource::Fallback)
        );
        let serif = resolve(&mut book, "Calibri", false, FontKind::Serif);
        assert_eq!(
            (serif.face.family.as_str(), serif.source),
            ("Calibri", FaceSource::Measured)
        );
        let symbol = resolve(&mut book, "Symbol", false, FontKind::Sans);
        assert_eq!(
            symbol.source,
            FaceSource::Measured,
            "symbol fonts never fall back"
        );
        assert!(!book.knows("No Such Font"));
        let nowhere = resolve(&mut book, "No Such Font", false, FontKind::Serif);
        assert_eq!(nowhere.source, FaceSource::Substitute);
        assert_eq!(book.missing, ["No Such Font"]);
    }

    const PDF: LayoutTarget = LayoutTarget::Pdf {
        allow_unembedded: false,
    };

    #[test]
    fn a_face_whose_licence_forbids_embedding_is_passed_over_for_a_pdf() {
        let restricted = font("Regular", vec![os2(4, 0x0002)]);
        let fonts = {
            let mut builder = FontSet::builder();
            builder.add(restricted).unwrap();
            builder
                .add(family_font("Other", "Regular", vec![]))
                .unwrap();
            builder.fallback(FontKind::Sans, "Other").build()
        };
        let mut word = FontBook::with_fonts(Vec::new(), &fonts);
        let brand = resolve(&mut word, "Brand", false, FontKind::Sans);
        assert_eq!(
            (brand.face.family.as_str(), brand.restricted),
            ("Brand", false)
        );
        let mut pdf = FontBook::with_fonts(Vec::new(), &fonts).with_target(PDF);
        let brand = resolve(&mut pdf, "Brand", false, FontKind::Sans);
        assert_eq!(
            (brand.face.family.as_str(), brand.source, brand.restricted),
            ("Other", FaceSource::Fallback, true)
        );
        let unembedded = LayoutTarget::Pdf {
            allow_unembedded: true,
        };
        let mut named = FontBook::with_fonts(Vec::new(), &fonts).with_target(unembedded);
        let brand = resolve(&mut named, "Brand", false, FontKind::Sans);
        assert_eq!(
            (brand.face.family.as_str(), brand.source),
            ("Brand", FaceSource::Supplied)
        );
    }

    #[test]
    fn a_supplied_family_comes_before_its_alias_and_metric_aliases_need_their_fonts() {
        let mut builder = FontSet::builder();
        builder.add(font("Regular", vec![])).unwrap();
        let fonts = builder.alias("Brand", "Arial").metric_aliases().build();
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        assert_eq!(
            resolve(&mut book, "Brand", false, FontKind::Sans).source,
            FaceSource::Supplied
        );
        let arial = resolve(&mut book, "Arial", false, FontKind::Sans);
        assert_eq!(
            (arial.face.family.as_str(), arial.source),
            ("Arial", FaceSource::Measured)
        );
        let mut builder = FontSet::builder();
        builder
            .add(family_font("Liberation Sans", "Regular", vec![]))
            .unwrap();
        let fonts = builder
            .metric_aliases()
            .alias("Courier New", "Liberation Sans")
            .build();
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        let arial = resolve(&mut book, "Arial", false, FontKind::Sans);
        assert_eq!(
            (arial.face.family.as_str(), arial.source),
            ("Liberation Sans", FaceSource::Alias { tested: true })
        );
        let courier = resolve(&mut book, "Courier New", false, FontKind::Mono);
        assert_eq!(
            courier.source,
            FaceSource::Alias { tested: false },
            "set before"
        );
    }

    #[test]
    fn a_style_words_fonts_have_but_the_set_lacks_is_noted() {
        let fonts = set_of(&[
            family_font("Arial", "Regular", vec![]),
            family_font("Brand", "Regular", vec![]),
        ]);
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        let arial = resolve(&mut book, "Arial", true, FontKind::Sans);
        assert!(arial.style_missing && !arial.face.bold);
        assert!(!resolve(&mut book, "Arial", false, FontKind::Sans).style_missing);
        let brand = resolve(&mut book, "Brand", true, FontKind::Sans);
        assert!(
            !brand.style_missing,
            "Word synthesizes a style the family lacks"
        );
    }

    fn inter() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/fonts/inter/Inter[opsz,wght].ttf"
        );
        std::fs::read(path).unwrap()
    }

    /// `font` with the elidable flag of its default `opsz` label (format
    /// 1, on axis 0, at 14) cleared.
    fn default_opsz_not_elidable(font: &[u8]) -> Vec<u8> {
        let stat = Ttf::face(font, 0).unwrap().table(b"STAT").unwrap();
        let start = stat.as_ptr() as usize - font.as_ptr() as usize;
        let offsets = be32(stat, 14).unwrap() as usize;
        let mut out = font.to_vec();
        for index in 0..usize::from(be16(stat, 12).unwrap()) {
            let at = offsets + usize::from(be16(stat, offsets + 2 * index).unwrap());
            let record = (be16(stat, at), be16(stat, at + 2), be32(stat, at + 8));
            if record == (Some(1), Some(0), Some(14 << 16)) {
                out[start + at + 5] &= !0x02;
            }
        }
        out
    }

    #[test]
    fn a_variable_font_answers_to_the_name_word_gives_it() {
        let inter = inter();
        let plain = set_of(std::slice::from_ref(&inter));
        assert_eq!(
            plain.faces()[0].word_family,
            None,
            "Inter's 14pt is elidable"
        );
        let fonts = set_of(&[default_opsz_not_elidable(&inter)]);
        assert_eq!(fonts.faces()[0].word_family.as_deref(), Some("Inter 14pt"));
        let mut book = FontBook::with_fonts(Vec::new(), &fonts);
        for name in ["Inter 14pt", "inter 14PT", "Inter"] {
            let found = resolve(&mut book, name, false, FontKind::Sans);
            assert_eq!(
                (found.face.family.as_str(), found.source),
                ("Inter", FaceSource::Supplied),
                "{name}"
            );
        }
    }

    #[test]
    fn metric_aliases_match_words_fonts() {
        let mut differ = Vec::new();
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/fonts/liberation"
        );
        for (family, to) in METRIC_ALIASES {
            let file = format!(
                "{dir}/{}-Regular.ttf",
                to.replace("liberation ", "Liberation").replace(' ', "")
            );
            let alias = Face::parse_file(&std::fs::read(&file).unwrap()).remove(0);
            let slots = builtin_slots(family).unwrap_or_else(|| panic!("{family}"));
            let word = &builtin()[slots[0].unwrap().1];
            let mut compared = 0;
            for code in word.codepoints.iter() {
                let Some(ch) = char::from_u32(*code).filter(|ch| alias.glyph(*ch).is_some()) else {
                    continue;
                };
                let expected = word.advance(ch);
                if (alias.advance(ch) - expected).abs() >= 1e-9 {
                    differ.push((*family, *code));
                }
                compared += 1;
            }
            assert!(compared > 200, "{family}: {compared} code points");
        }
        // Word's Arial and Times New Roman draw U+2010 HYPHEN wider than
        // their hyphen-minus; the Liberation fonts do not.
        let hyphen = [
            ("arial", 0x2010),
            ("times new roman", 0x2010),
            ("times", 0x2010),
        ];
        assert_eq!(differ, hyphen);
    }
}
