//! The font program a PDF embeds: a subset, an instance, or the whole face.

use subsetter::{GlyphRemapper, Tag};

use super::face::{FaceInfo, Outlines, Variation};
use crate::layout::fonts::{FontError, Ttf, be16};
use crate::{Error, Result};

/// The subset of `remapper`'s glyphs (and the glyphs they are built from)
/// at `variation`. A face that forbids subsetting keeps every glyph at its
/// own id, and is embedded as it is when no instance has to be made.
pub(super) fn embed(
    data: &[u8],
    index: u32,
    info: &FaceInfo,
    variation: &Variation,
    remapper: &GlyphRemapper,
) -> Result<Vec<u8>> {
    if info.subsettable {
        return subset(data, index, info, variation, remapper);
    }
    if variation.is_empty() && info.outlines != Outlines::Cff2 {
        return standalone(data, index, info.outlines);
    }
    let every_glyph = GlyphRemapper::new_from_glyphs_sorted(&(0..info.glyphs).collect::<Vec<_>>());
    subset(data, index, info, variation, &every_glyph)
}

fn subset(
    data: &[u8],
    index: u32,
    info: &FaceInfo,
    variation: &Variation,
    remapper: &GlyphRemapper,
) -> Result<Vec<u8>> {
    let coordinates: Vec<(Tag, f32)> = variation
        .iter()
        .map(|(tag, value)| (Tag::new(tag), *value))
        .collect();
    subsetter::subset_with_variations(data, index, &coordinates, remapper).map_err(|err| {
        Error::invalid_argument(
            "pdf_font_subset_failed",
            format!("{} could not be subset for embedding: {err}", info.label),
        )
    })
}

/// One face of a collection as a font file of its own.
fn standalone(data: &[u8], index: u32, outlines: Outlines) -> Result<Vec<u8>> {
    if data.get(..4) != Some(b"ttcf") {
        return Ok(data.to_vec());
    }
    let ttf = Ttf::face(data, index).map_err(FontError::into_error)?;
    let mut tables: Vec<([u8; 4], &[u8])> = ttf.tables().collect();
    tables.sort_by_key(|(tag, _)| *tag);
    let count = tables.len() as u16;
    let selector = count.checked_ilog2().unwrap_or(0) as u16;
    let search_range = 16 << selector;
    let version: &[u8; 4] = match outlines {
        Outlines::TrueType => &[0, 1, 0, 0],
        Outlines::Cff | Outlines::Cff2 => b"OTTO",
    };
    let mut out = version.to_vec();
    for field in [count, search_range, selector, count * 16 - search_range] {
        out.extend_from_slice(&field.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    for (tag, table) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(table).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(table.len() as u32).to_be_bytes());
        offset += table.len().next_multiple_of(4);
    }
    for (_, table) in &tables {
        out.extend_from_slice(table);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    Ok(out)
}

fn checksum(table: &[u8]) -> u32 {
    table.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// Advance widths of an embedded program, by glyph id, in thousandths of an em.
pub(super) struct Advances<'a> {
    hmtx: &'a [u8],
    metrics: usize,
    scale: f64,
}

impl<'a> Advances<'a> {
    pub(super) fn read(program: &'a [u8]) -> Result<Advances<'a>> {
        let ttf = Ttf::face(program, 0).map_err(FontError::into_error)?;
        let malformed = |table| FontError::Malformed(table).into_error();
        let units_per_em = ttf
            .table(b"head")
            .and_then(|head| be16(head, 18))
            .filter(|units| *units > 0)
            .ok_or_else(|| malformed("head"))?;
        let metrics = ttf
            .table(b"hhea")
            .and_then(|hhea| be16(hhea, 34))
            .map(usize::from)
            .ok_or_else(|| malformed("hhea"))?;
        let hmtx = ttf
            .table(b"hmtx")
            .filter(|hmtx| metrics > 0 && hmtx.len() / 4 >= metrics)
            .ok_or_else(|| malformed("hmtx"))?;
        Ok(Advances {
            hmtx,
            metrics,
            scale: 1000.0 / f64::from(units_per_em),
        })
    }

    pub(super) fn width(&self, glyph: u16) -> f64 {
        let at = usize::from(glyph).min(self.metrics - 1) * 4;
        be16(self.hmtx, at).map_or(0.0, |width| f64::from(width) * self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_pad_the_last_word_with_zeros() {
        assert_eq!(checksum(&[0, 0, 0, 1, 0, 0, 0, 2]), 3);
        assert_eq!(checksum(&[1]), 0x0100_0000);
        assert_eq!(checksum(&[0xFF; 8]), 0xFFFF_FFFE);
    }
}
