//! Windows bitmaps without compression: 1, 4 and 8 bits through a palette,
//! 24-bit BGR and 32-bit BGRX or bit fields, bottom-up or top-down.

use super::{Budget, Image, Space, byte, bytes, le16, le32, malformed, plane, unsupported};
use crate::{Error, Result};

const FILE_HEADER: usize = 14;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

struct Header {
    width: u32,
    height: u32,
    top_down: bool,
    bits: u16,
    /// Red, green, blue and alpha masks of a 32-bit pixel.
    masks: [u32; 4],
    palette: Vec<[u8; 3]>,
    data: usize,
}

pub(super) fn read(data: &[u8], budget: &Budget) -> Result<Image> {
    let header = read_header(data)?;
    budget.check(header.width, header.height)?;
    let stride = usize::try_from(header.width)
        .ok()
        .and_then(|w| w.checked_mul(usize::from(header.bits)))
        .map(|bits| bits.div_ceil(32) * 4)
        .ok_or(Error::out_of_memory(usize::MAX))?;
    let rows = bytes(
        data,
        header.data,
        stride
            .checked_mul(header.height as usize)
            .ok_or(Error::out_of_memory(usize::MAX))?,
    )?;
    let mut color = plane(header.width, header.height, 3)?;
    let mut alpha = plane(header.width, header.height, 1)?;
    for (stored, row) in rows.chunks_exact(stride).enumerate() {
        let y = if header.top_down {
            stored
        } else {
            header.height as usize - 1 - stored
        };
        for x in 0..header.width as usize {
            let (rgb, a) = header.pixel(row, x)?;
            let pixel = y * header.width as usize + x;
            let slot = color
                .get_mut(pixel * 3..pixel * 3 + 3)
                .zip(alpha.get_mut(pixel))
                .ok_or_else(|| {
                    Error::internal(
                        "pdf_image_pixel_outside",
                        "a decoded pixel outside its image",
                    )
                })?;
            slot.0.copy_from_slice(&rgb);
            *slot.1 = a;
        }
    }
    // Many 32-bit writers leave an alpha channel of zeros: those are opaque.
    let alpha = (alpha.iter().any(|&a| a != 0)).then_some(alpha);
    Ok(Image::decoded(
        header.width,
        header.height,
        Space::Rgb,
        color,
        alpha,
    ))
}

fn read_header(data: &[u8]) -> Result<Header> {
    let offset = le32(data, 10)? as usize;
    let size = le32(data, FILE_HEADER)? as usize;
    let (width, height, bits, compression, colors, entry) = match size {
        12 => (
            i32::from(le16(data, 18)?),
            i32::from(le16(data, 20)?),
            le16(data, 24)?,
            BI_RGB,
            0,
            3,
        ),
        40.. => (
            le32(data, 18)? as i32,
            le32(data, 22)? as i32,
            le16(data, 28)?,
            le32(data, 30)?,
            le32(data, 46)?,
            4,
        ),
        _ => return Err(unsupported(format!("a BMP with a {size}-byte header"))),
    };
    match (compression, bits) {
        (BI_RGB, 1 | 4 | 8 | 24 | 32) | (BI_BITFIELDS, 32) => {}
        (1 | 2, _) => return Err(unsupported("a run-length encoded BMP")),
        (BI_RGB | BI_BITFIELDS, _) => return Err(unsupported(format!("a {bits}-bit BMP"))),
        _ => return Err(unsupported("a compressed BMP")),
    }
    if width <= 0 || height == 0 || height == i32::MIN {
        return Err(malformed(format!("the BMP is {width} × {height} pixels")));
    }
    let masks = if compression == BI_BITFIELDS {
        let alpha = if size >= 56 { le32(data, 66)? } else { 0 };
        [le32(data, 54)?, le32(data, 58)?, le32(data, 62)?, alpha]
    } else {
        [0xFF_0000, 0xFF00, 0xFF, 0]
    };
    let palette_at = FILE_HEADER
        + size
        + if size == 40 && compression == BI_BITFIELDS {
            12
        } else {
            0
        };
    let palette = if bits <= 8 {
        let count = match colors {
            0 => 1 << bits,
            n => n.min(1 << bits) as usize,
        };
        bytes(data, palette_at, count * entry)?
            .chunks_exact(entry)
            .map(|bgr| [2, 1, 0].map(|i| bgr.get(i).copied().unwrap_or(0)))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Header {
        width: width.unsigned_abs(),
        height: height.unsigned_abs(),
        top_down: height < 0,
        bits,
        masks,
        palette,
        data: offset,
    })
}

impl Header {
    /// Pixel `x` of a stored row: its colour and opacity.
    fn pixel(&self, row: &[u8], x: usize) -> Result<([u8; 3], u8)> {
        match self.bits {
            24 => {
                let bgr = bytes(row, x * 3, 3)?;
                Ok(([byte(bgr, 2)?, byte(bgr, 1)?, byte(bgr, 0)?], u8::MAX))
            }
            32 => {
                let value = le32(row, x * 4)?;
                let [r, g, b, a] = self.masks.map(|mask| channel(value, mask));
                Ok(([r, g, b], a))
            }
            bits => {
                let bit = x * usize::from(bits);
                let shift = 8 - usize::from(bits) - bit % 8;
                let index = (byte(row, bit / 8)? >> shift) & ((1u16 << bits) - 1) as u8;
                let rgb = self
                    .palette
                    .get(usize::from(index))
                    .ok_or_else(|| malformed("a BMP pixel outside its palette"))?;
                Ok((*rgb, u8::MAX))
            }
        }
    }
}

/// The 8-bit value of the bits `mask` selects.
fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let width = (mask >> shift).count_ones();
    let raw = (value & mask) >> shift;
    if width >= 8 {
        (raw >> (width - 8)) as u8
    } else {
        (raw * 255 / ((1 << width) - 1)) as u8
    }
}
