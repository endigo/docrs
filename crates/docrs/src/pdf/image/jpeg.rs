//! JPEG, passed through as `DCTDecode` once its frame header is read.

use super::{
    Budget, Image, PROFILE_IGNORED, Samples, Space, be16, byte, bytes, copied, malformed,
    unsupported,
};
use crate::Result;

const SOS: u8 = 0xDA;

struct Frame {
    width: u32,
    height: u32,
    components: u8,
}

pub(super) fn read(data: &[u8], budget: &Budget) -> Result<Image> {
    let mut frame = None;
    let mut adobe = None;
    let mut profile = false;
    let mut at = 2;
    loop {
        let (marker, next) = marker(data, at)?;
        at = next;
        match marker {
            SOS => break,
            0xD9 => return Err(malformed("the JPEG ends before its image data")),
            0x01 | 0xD0..=0xD8 => continue,
            _ => {}
        }
        let length = usize::from(be16(data, at)?);
        if length < 2 {
            return Err(malformed("a JPEG segment shorter than its length field"));
        }
        let segment = bytes(data, at + 2, length - 2)?;
        at += length;
        match marker {
            0xC0..=0xC2 if frame.is_none() => frame = Some(read_frame(segment)?),
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(unsupported(
                    "a lossless, hierarchical or arithmetic-coded JPEG",
                ));
            }
            0xE2 if segment.starts_with(b"ICC_PROFILE\0") => profile = true,
            0xEE if segment.starts_with(b"Adobe") => adobe = Some(byte(segment, 11)?),
            _ => {}
        }
    }
    let frame = frame.ok_or_else(|| malformed("the JPEG has no frame header"))?;
    budget.check(frame.width, frame.height)?;
    let scan = at.checked_add(usize::from(be16(data, at)?));
    if scan
        .and_then(|scan| data.get(scan..))
        .is_none_or(|rest| rest.is_empty() || rest.starts_with(&[0xFF, 0xD9]))
    {
        return Err(malformed(
            "the JPEG has no image data after its scan header",
        ));
    }
    // Adobe's CMYK (transform 0) is the one four-component JPEG tested
    // against a real encoder; its values are stored inverted.
    let space = match (frame.components, adobe) {
        (1, _) => Space::Gray,
        (3, _) => Space::Rgb,
        (4, Some(0)) => Space::Cmyk,
        (4, _) => {
            return Err(unsupported(
                "a YCCK JPEG, or a CMYK one without Adobe's marker",
            ));
        }
        (other, _) => return Err(unsupported(format!("a JPEG with {other} components"))),
    };
    let mut image = Image {
        width: frame.width,
        height: frame.height,
        inverted: space == Space::Cmyk,
        space,
        samples: Samples::Dct(copied(data)?),
        alpha: None,
        warnings: Vec::new(),
    };
    if profile {
        image.warn(PROFILE_IGNORED);
    }
    Ok(image)
}

/// The marker at `at` and where its segment starts; fill bytes before a
/// marker are skipped.
fn marker(data: &[u8], mut at: usize) -> Result<(u8, usize)> {
    if byte(data, at)? != 0xFF {
        return Err(malformed("a JPEG segment does not start with a marker"));
    }
    while byte(data, at)? == 0xFF {
        at += 1;
    }
    Ok((byte(data, at)?, at + 1))
}

fn read_frame(segment: &[u8]) -> Result<Frame> {
    let precision = byte(segment, 0)?;
    if precision != 8 {
        return Err(unsupported(format!("a {precision}-bit JPEG")));
    }
    let height = u32::from(be16(segment, 1)?);
    if height == 0 {
        return Err(unsupported("a JPEG whose height follows its first scan"));
    }
    let components = byte(segment, 5)?;
    if segment.len() != 6 + 3 * usize::from(components) {
        return Err(malformed(
            "the JPEG frame header does not list its components",
        ));
    }
    Ok(Frame {
        width: u32::from(be16(segment, 3)?),
        height,
        components,
    })
}
