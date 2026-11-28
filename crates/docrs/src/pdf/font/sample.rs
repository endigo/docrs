//! A page of text lines in embedded fonts, for tests: one glyph per
//! character through the cmap, kerned from `kern` as layout measures.

use std::sync::Arc;

use super::{FontResources, Style};
use crate::layout::fonts::Face;
use crate::pdf::meta::Info;
use crate::pdf::writer::{Dict, Writer, write_hex, write_name, write_number};
use crate::{Limits, Result};

const A4: (f64, f64) = (595.2756, 841.8898);
const MARGIN: f64 = 56.0;

/// One line of [`text_document`].
#[derive(Debug, Clone)]
pub struct SampleLine<'a> {
    pub font: &'a [u8],
    pub index: u32,
    pub style: Style,
    pub size: f64,
    /// Each glyph with the text it draws, as [`glyphs_by_cmap`] gives them.
    pub glyphs: Vec<(u16, String)>,
}

/// Each character's glyph in face `index`'s Unicode cmap, glyph 0 for a
/// character it does not map.
pub(crate) fn glyphs_by_cmap(font: &[u8], index: u32, text: &str) -> Result<Vec<(u16, String)>> {
    let face = Face::read_at(&Arc::from(font), index as usize)?;
    Ok(text
        .chars()
        .map(|c| (face.glyph(c).unwrap_or(0), c.to_string()))
        .collect())
}

/// An A4 page with each line in its own face, from the top margin down.
pub(crate) fn text_document(
    lines: &[SampleLine],
    info: &Info,
    producer: &str,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let mut writer = Writer::new(limits.max_output_bytes)?;
    let mut fonts = FontResources::default();
    let mut content = Vec::new();
    let mut baseline = A4.1 - MARGIN;
    for line in lines {
        let face = fonts.add_face(Arc::from(line.font), line.index, Some(line.style))?;
        let metrics = Face::read_at(&Arc::from(line.font), line.index as usize)?;
        baseline -= line.size * 1.5;
        content.extend_from_slice(b"BT ");
        write_name(&mut content, &face.resource_name())?;
        content.push(b' ');
        write_number(&mut content, line.size)?;
        content.extend_from_slice(b" Tf ");
        write_number(&mut content, MARGIN)?;
        content.push(b' ');
        write_number(&mut content, baseline)?;
        content.extend_from_slice(b" Td [");
        let mut previous: Option<char> = None;
        for (glyph, text) in &line.glyphs {
            if let (Some(left), Some(right)) = (previous, text.chars().next()) {
                let kern = metrics.kern(left, right);
                if kern != 0.0 {
                    write_number(&mut content, -kern * 1000.0)?;
                }
            }
            write_hex(
                &mut content,
                &fonts.use_glyph(face, *glyph, text)?.to_be_bytes(),
            );
            previous = text.chars().last();
        }
        content.extend_from_slice(b"] TJ ET\n");
    }
    let resources = fonts
        .finish(&mut writer)?
        .into_iter()
        .fold(Dict::new(), |fonts, (name, font)| fonts.with(&name, font));
    let contents = super::super::page_content(&mut writer, &content)?;
    super::super::write_document(
        writer,
        &[(A4, contents)],
        Dict::new().with("Font", resources),
        info,
        producer,
    )
}
