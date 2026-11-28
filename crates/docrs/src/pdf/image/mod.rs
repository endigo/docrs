//! Pictures as image XObjects. JPEG is passed through; PNG is passed through
//! when PDF can read its data as it is, and decoded otherwise; GIF and BMP
//! are decoded. Decoded images become 8-bit colour plus a soft mask.

#![warn(clippy::indexing_slicing)]

mod bmp;
mod gif;
mod jpeg;
mod png;
#[cfg(any(test, feature = "test-support"))]
mod sample;

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use super::limits::PdfLimits;
use super::writer::{Dict, Object, Ref, Writer, count, name};
use crate::{Error, Result};

#[cfg(any(test, feature = "test-support"))]
pub(crate) use self::sample::image_document;

/// A format the engine does not draw, or a variant of one it does.
pub(crate) const UNSUPPORTED: &str = "pdf_image_unsupported";
/// An embedded colour profile, which the image is drawn without.
pub(crate) const PROFILE_IGNORED: &str = "pdf_image_profile_ignored";
/// An animation, drawn as its first frame.
pub(crate) const ANIMATED: &str = "pdf_image_animated";

/// A decoded or passed-through image, ready to write.
#[derive(Debug)]
pub(crate) struct Image {
    width: u32,
    height: u32,
    space: Space,
    samples: Samples,
    /// Adobe's CMYK JPEGs store each value inverted.
    inverted: bool,
    /// One 8-bit opacity per pixel, row by row from the top.
    alpha: Option<Vec<u8>>,
    /// The codes of what the PDF shows differently, each once.
    warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// RGB triples.
    Indexed(Vec<u8>),
}

#[derive(Debug)]
enum Samples {
    /// A JPEG file as it is.
    Dct(Vec<u8>),
    /// PNG image data as it is: zlib, each row behind a PNG filter byte.
    Png(Vec<u8>),
    /// 8 bits per component, row by row from the top.
    Raw(Vec<u8>),
}

impl Space {
    fn components(&self) -> u32 {
        match self {
            Space::Gray | Space::Indexed(_) => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }

    fn object(&self) -> Result<Object> {
        Ok(match self {
            Space::Gray => name("DeviceGray"),
            Space::Rgb => name("DeviceRGB"),
            Space::Cmyk => name("DeviceCMYK"),
            Space::Indexed(palette) => vec![
                name("Indexed"),
                name("DeviceRGB"),
                count((palette.len() / 3).saturating_sub(1))?,
                Object::Bytes(palette.clone()),
            ]
            .into(),
        })
    }
}

impl Image {
    /// Reads an image, recognising its format by its bytes, within the
    /// pixel budgets of `limits` for a document that has decoded `used`.
    pub(crate) fn read(data: &[u8], limits: &PdfLimits, used: u64) -> Result<Image> {
        let budget = Budget { limits, used };
        match format(data) {
            Format::Png => png::read(data, &budget),
            Format::Jpeg => jpeg::read(data, &budget),
            Format::Gif => gif::read(data, &budget),
            Format::Bmp => bmp::read(data, &budget),
            Format::Other(what) => Err(unsupported(what)),
        }
    }

    fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Writes the image XObject, and its soft mask before it.
    pub(crate) fn write(&self, writer: &mut Writer) -> Result<Ref> {
        let mut dict = self.xobject(&self.space)?;
        if self.inverted {
            let decode: Vec<Object> = (0..self.space.components())
                .flat_map(|_| [1.into(), 0.into()])
                .collect();
            dict.set("Decode", decode);
        }
        if let Some(alpha) = &self.alpha {
            let mask = writer.alloc()?;
            writer.stream(mask, self.xobject(&Space::Gray)?, alpha)?;
            dict.set("SMask", mask);
        }
        let at = writer.alloc()?;
        match &self.samples {
            Samples::Dct(data) => writer.stream_raw(at, dict, Some("DCTDecode"), data)?,
            Samples::Png(data) => {
                let parms = Dict::new()
                    .with("Predictor", 15)
                    .with("Colors", self.space.components())
                    .with("BitsPerComponent", 8)
                    .with("Columns", self.width);
                writer.stream_raw(
                    at,
                    dict.with("DecodeParms", parms),
                    Some("FlateDecode"),
                    data,
                )?
            }
            Samples::Raw(data) => writer.stream(at, dict, data)?,
        }
        Ok(at)
    }

    fn xobject(&self, space: &Space) -> Result<Dict> {
        Ok(Dict::new()
            .with("Type", name("XObject"))
            .with("Subtype", name("Image"))
            .with("Width", self.width)
            .with("Height", self.height)
            .with("ColorSpace", space.object()?)
            .with("BitsPerComponent", 8))
    }

    /// An image of decoded 8-bit samples, without a mask where every pixel
    /// is opaque.
    fn decoded(
        width: u32,
        height: u32,
        space: Space,
        samples: Vec<u8>,
        alpha: Option<Vec<u8>>,
    ) -> Image {
        Image {
            width,
            height,
            space,
            samples: Samples::Raw(samples),
            inverted: false,
            alpha: alpha.filter(|alpha| alpha.iter().any(|&a| a != u8::MAX)),
            warnings: Vec::new(),
        }
    }

    fn warn(&mut self, code: &'static str) {
        if !self.warnings.contains(&code) {
            self.warnings.push(code);
        }
    }
}

enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Other(&'static str),
}

/// The format by its signature; never by a file name.
fn format(data: &[u8]) -> Format {
    let text = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let text = text.trim_ascii_start();
    if data.starts_with(b"\x89PNG\r\n\x1A\n") {
        Format::Png
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Format::Jpeg
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Format::Gif
    } else if data.starts_with(b"BM") {
        Format::Bmp
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Format::Other("a TIFF image")
    } else if data.starts_with(&[1, 0, 0, 0]) && data.get(40..44) == Some(b" EMF") {
        Format::Other("an EMF image")
    } else if data.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A])
        || data.starts_with(&[1, 0, 9, 0])
        || data.starts_with(&[2, 0, 9, 0])
    {
        Format::Other("a WMF image")
    } else if text.starts_with(b"<") {
        Format::Other("an SVG or other XML image")
    } else {
        Format::Other("an image in an unrecognised format")
    }
}

/// The images of one document: one XObject per media part.
#[derive(Default)]
pub(crate) struct ImageResources {
    placed: BTreeMap<String, Placed>,
    /// Decoded pixels of every image written.
    pixels: u64,
}

/// An image written once, to draw wherever its part is used.
#[derive(Debug, Clone)]
pub(crate) struct Placed {
    /// The name in a page's `/XObject` resources.
    pub(crate) name: String,
    pub(crate) xobject: Ref,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) warnings: Vec<&'static str>,
}

impl ImageResources {
    /// Media part `part`, holding `data`: read and written the first time,
    /// within the per-image and per-document pixel budgets.
    pub(crate) fn add(
        &mut self,
        writer: &mut Writer,
        part: &str,
        data: &[u8],
        limits: &PdfLimits,
    ) -> Result<&Placed> {
        let number = self.placed.len() + 1;
        match self.placed.entry(part.to_string()) {
            Entry::Occupied(placed) => Ok(placed.into_mut()),
            Entry::Vacant(slot) => {
                let image = Image::read(data, limits, self.pixels)?;
                let xobject = image.write(writer)?;
                self.pixels += image.pixels();
                Ok(slot.insert(Placed {
                    name: format!("Im{number}"),
                    xobject,
                    width: image.width,
                    height: image.height,
                    warnings: image.warnings,
                }))
            }
        }
    }

    /// The `/XObject` resource dictionary.
    pub(crate) fn resources(&self) -> Dict {
        self.placed.values().fold(Dict::new(), |dict, placed| {
            dict.with(&placed.name, placed.xobject)
        })
    }
}

/// The pixels an image may decode to: its own budget, and what is left of
/// the document's.
struct Budget<'a> {
    limits: &'a PdfLimits,
    used: u64,
}

impl Budget<'_> {
    /// Checks a size read from a header, before anything is decoded.
    fn check(&self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(malformed(format!("the image is {width} × {height} pixels")));
        }
        let pixels = u64::from(width) * u64::from(height);
        if pixels > self.limits.max_image_pixels {
            return Err(Error::limit(
                "max_image_pixels",
                self.limits.max_image_pixels,
                pixels,
            ));
        }
        let total = self.used.saturating_add(pixels);
        if total > self.limits.max_document_pixels {
            return Err(Error::limit(
                "max_document_pixels",
                self.limits.max_document_pixels,
                total,
            ));
        }
        Ok(())
    }
}

/// `components` zeroed bytes per pixel of a `width` × `height` image.
fn plane(width: u32, height: u32, components: usize) -> Result<Vec<u8>> {
    let len = usize::try_from(width)
        .ok()
        .zip(usize::try_from(height).ok())
        .and_then(|(width, height)| width.checked_mul(height))
        .and_then(|pixels| pixels.checked_mul(components))
        .ok_or(Error::out_of_memory(usize::MAX))?;
    zeroed(len)
}

fn zeroed(len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| Error::out_of_memory(len))?;
    out.resize(len, 0);
    Ok(out)
}

fn copied(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(data.len())
        .map_err(|_| Error::out_of_memory(data.len()))?;
    out.extend_from_slice(data);
    Ok(out)
}

fn malformed(what: impl std::fmt::Display) -> Error {
    Error::malformed("pdf_image_malformed", format!("malformed image: {what}"))
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::unsupported(UNSUPPORTED, format!("the PDF engine does not draw {what}"))
}

/// `len` bytes at `at`, or a malformed-image error naming the truncation.
fn bytes(data: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    at.checked_add(len)
        .and_then(|end| data.get(at..end))
        .ok_or_else(|| malformed("it ends early"))
}

fn byte(data: &[u8], at: usize) -> Result<u8> {
    data.get(at)
        .copied()
        .ok_or_else(|| malformed("it ends early"))
}

fn be16(data: &[u8], at: usize) -> Result<u16> {
    let b = bytes(data, at, 2)?;
    Ok(u16::from_be_bytes([byte(b, 0)?, byte(b, 1)?]))
}

fn be32(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from(be16(data, at)?) << 16 | u32::from(be16(data, at + 2)?))
}

fn le16(data: &[u8], at: usize) -> Result<u16> {
    Ok(be16(data, at)?.swap_bytes())
}

fn le32(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from(le16(data, at)?) | u32::from(le16(data, at + 2)?) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_comes_from_the_bytes() {
        let unsupported = |data: &[u8]| {
            let err = Image::read(data, &PdfLimits::default(), 0).unwrap_err();
            assert_eq!(err.reason(), UNSUPPORTED, "{err}");
            err.to_string()
        };
        assert!(unsupported(b"II*\0rest").contains("TIFF"));
        assert!(unsupported(b"MM\0*rest").contains("TIFF"));
        let mut emf = vec![1, 0, 0, 0];
        emf.resize(40, 0);
        emf.extend_from_slice(b" EMF");
        assert!(unsupported(&emf).contains("EMF"));
        assert!(unsupported(&[0xD7, 0xCD, 0xC6, 0x9A, 0, 0]).contains("WMF"));
        assert!(unsupported(b"\xEF\xBB\xBF  <svg/>").contains("SVG"));
        assert!(unsupported(b"plain text").contains("unrecognised"));
    }

    #[test]
    fn budgets_are_checked_with_the_document_total() {
        let limits = PdfLimits {
            max_image_pixels: 100,
            max_document_pixels: 150,
        };
        let budget = |used| Budget {
            limits: &limits,
            used,
        };
        assert!(budget(0).check(10, 10).is_ok());
        let err = budget(0).check(10, 11).unwrap_err();
        assert_eq!(err.reason(), "max_image_pixels");
        let err = budget(60).check(10, 10).unwrap_err();
        assert_eq!(err.reason(), "max_document_pixels");
        assert_eq!(err.limit_exceeded().unwrap().observed, 160);
        let err = budget(0).check(0, 5).unwrap_err();
        assert_eq!(err.reason(), "pdf_image_malformed");
    }
}
