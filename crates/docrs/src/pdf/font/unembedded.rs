//! Fonts referenced by name and not embedded (`allow_unembedded`): simple
//! TrueType fonts in `WinAnsiEncoding` whose `/Widths` are the measuring
//! face's advances, so a viewer that has the font draws each glyph where
//! layout placed it, and one that lacks it substitutes.

use std::sync::Arc;

use crate::Result;
use crate::layout::fonts::Face;
use crate::pdf::writer::{Dict, Object, Ref, Writer, name};

pub(crate) struct Unembedded {
    family: String,
    bold: bool,
    italic: bool,
    face: Arc<Face>,
}

/// `WinAnsiEncoding` codes 0x80-0x9F (ISO 32000-1 Annex D); 0: none.
const WIN_ANSI_HIGH: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
    0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

/// `ch`'s code in `WinAnsiEncoding`.
pub(crate) fn code(ch: char) -> Option<u8> {
    let value = ch as u32;
    match value {
        0x20..=0x7E | 0xA0..=0xFF => Some(value as u8),
        0 => None,
        _ => WIN_ANSI_HIGH
            .iter()
            .position(|unit| u32::from(*unit) == value)
            .map(|at| 0x80 + at as u8),
    }
}

fn char_of(code: u8) -> Option<char> {
    match code {
        0x20..=0x7E | 0xA0..=0xFF => Some(char::from(code)),
        0x80..=0x9F => char::from_u32(u32::from(WIN_ANSI_HIGH[usize::from(code - 0x80)]))
            .filter(|ch| *ch != '\0'),
        _ => None,
    }
}

const FIRST: u8 = 0x20;
const LAST: u8 = 0xFF;

impl Unembedded {
    pub(crate) fn new(family: &str, bold: bool, italic: bool, face: Arc<Face>) -> Self {
        Unembedded {
            family: family.to_string(),
            bold,
            italic,
            face,
        }
    }

    pub(crate) fn is(&self, family: &str, bold: bool, italic: bool) -> bool {
        self.family == family && self.bold == bold && self.italic == italic
    }

    /// The width a code is drawn with, in thousandths of an em.
    pub(crate) fn width(&self, code: u8) -> f64 {
        char_of(code).map_or(0.0, |ch| (self.face.advance(ch) * 1000.0).round())
    }

    /// ISO 32000-1 9.6.3: a TrueType font's name with its spaces removed,
    /// then a comma and its style.
    fn base_font(&self) -> String {
        let family: String = self.family.chars().filter(|c| !c.is_whitespace()).collect();
        match (self.bold, self.italic) {
            (false, false) => family,
            (true, false) => format!("{family},Bold"),
            (false, true) => format!("{family},Italic"),
            (true, true) => format!("{family},BoldItalic"),
        }
    }

    pub(crate) fn write(&self, writer: &mut Writer) -> Result<Ref> {
        let base_font = self.base_font();
        let scale = |value: f64| (value * 1000.0).round();
        let (ascent, descent) = (scale(self.face.ascent()), -scale(self.face.descent()));
        let mut flags = NONSYMBOLIC;
        if self.italic {
            flags |= ITALIC;
        }
        let descriptor = writer.alloc()?;
        writer.dict(
            descriptor,
            Dict::new()
                .with("Type", name("FontDescriptor"))
                .with("FontName", name(&base_font))
                .with("Flags", flags)
                .with(
                    "FontBBox",
                    [0.0, descent, 1000.0, ascent].map(Object::from).to_vec(),
                )
                .with("ItalicAngle", if self.italic { -12.0 } else { 0.0 })
                .with("Ascent", ascent)
                .with("Descent", descent)
                .with("CapHeight", (ascent * 0.7).round())
                .with("StemV", if self.bold { 140_i64 } else { 80 }),
        )?;
        let widths: Vec<Object> = (FIRST..=LAST).map(|code| self.width(code).into()).collect();
        let font = writer.alloc()?;
        writer.dict(
            font,
            Dict::new()
                .with("Type", name("Font"))
                .with("Subtype", name("TrueType"))
                .with("BaseFont", name(&base_font))
                .with("FirstChar", i64::from(FIRST))
                .with("LastChar", i64::from(LAST))
                .with("Widths", widths)
                .with("Encoding", name("WinAnsiEncoding"))
                .with("FontDescriptor", descriptor),
        )?;
        Ok(font)
    }
}

/// Font descriptor flags (ISO 32000-1 Table 123).
const NONSYMBOLIC: u32 = 1 << 5;
const ITALIC: u32 = 1 << 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_ansi_codes_round_trip() {
        for ch in ['A', '~', '\u{A0}', 'é', '€', '\u{2019}', '\u{2022}', 'Ÿ'] {
            assert_eq!(code(ch).and_then(char_of), Some(ch), "{ch:?}");
        }
        assert_eq!(code('\u{2610}'), None);
        assert_eq!(code('\u{81}'), None);
        assert_eq!(char_of(0x81), None);
    }
}
