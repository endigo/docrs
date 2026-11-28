//! GIF (GIF89a): the first frame, on the logical screen, with its
//! transparent colour as alpha. Later frames are only counted.

use super::{ANIMATED, Budget, Image, Space, byte, bytes, le16, malformed, plane};
use crate::{Error, Result};

const MAX_CODES: usize = 4096;

/// The first image and the extension before it.
struct Frame<'a> {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    interlaced: bool,
    palette: &'a [u8],
    transparent: Option<u8>,
    min_code: u8,
}

pub(super) fn read(data: &[u8], budget: &Budget) -> Result<Image> {
    let width = u32::from(le16(data, 6)?);
    let height = u32::from(le16(data, 8)?);
    budget.check(width, height)?;
    let flags = byte(data, 10)?;
    let mut at = 13;
    let global = if flags & 0x80 != 0 {
        let palette = bytes(data, at, 3 << ((flags & 7) + 1))?;
        at += palette.len();
        palette
    } else {
        &[]
    };
    let (frame, data_at) = first_frame(data, at, global)?;
    if frame.left + frame.width > width || frame.top + frame.height > height {
        return Err(malformed("the first GIF frame lies outside the image"));
    }
    let (indices, after) = decompress(data, data_at, &frame)?;
    let mut image = compose(width, height, &frame, &indices)?;
    if later_frame(data, after) {
        image.warn(ANIMATED);
    }
    Ok(image)
}

/// The first image descriptor, and where its LZW data starts.
fn first_frame<'a>(data: &'a [u8], mut at: usize, global: &'a [u8]) -> Result<(Frame<'a>, usize)> {
    let mut transparent = None;
    loop {
        match byte(data, at)? {
            0x21 => {
                let label = byte(data, at + 1)?;
                if label == 0xF9 {
                    let control = bytes(data, at + 2, 6)?;
                    transparent = (byte(control, 1)? & 1 != 0).then_some(byte(control, 4)?);
                }
                at = skip_blocks(data, at + 2)?;
            }
            0x2C => {
                let flags = byte(data, at + 9)?;
                let mut next = at + 10;
                let palette = if flags & 0x80 != 0 {
                    let local = bytes(data, next, 3 << ((flags & 7) + 1))?;
                    next += local.len();
                    local
                } else {
                    global
                };
                if palette.is_empty() {
                    return Err(malformed("the GIF has no colour table"));
                }
                let frame = Frame {
                    left: u32::from(le16(data, at + 1)?),
                    top: u32::from(le16(data, at + 3)?),
                    width: u32::from(le16(data, at + 5)?),
                    height: u32::from(le16(data, at + 7)?),
                    interlaced: flags & 0x40 != 0,
                    palette,
                    transparent,
                    min_code: byte(data, next)?,
                };
                return Ok((frame, next + 1));
            }
            _ => return Err(malformed("the GIF has no image")),
        }
    }
}

/// Past a run of data sub-blocks starting at `at`, and its terminator.
fn skip_blocks(data: &[u8], mut at: usize) -> Result<usize> {
    loop {
        let size = usize::from(byte(data, at)?);
        at += 1;
        if size == 0 {
            return Ok(at);
        }
        bytes(data, at, size)?;
        at += size;
    }
}

/// Whether another image follows; a file cut short after the first frame
/// is drawn as if it ended there.
fn later_frame(data: &[u8], mut at: usize) -> bool {
    loop {
        match data.get(at) {
            Some(0x2C) => return true,
            Some(0x21) => match skip_blocks(data, at + 2) {
                Ok(next) => at = next,
                Err(_) => return false,
            },
            _ => return false,
        }
    }
}

/// The frame's colour indices in stored row order, and where its data ends.
fn decompress(data: &[u8], mut at: usize, frame: &Frame) -> Result<(Vec<u8>, usize)> {
    if !(2..=8).contains(&frame.min_code) {
        return Err(malformed("an invalid GIF code size"));
    }
    let total = frame.width as usize * frame.height as usize;
    let mut out = Vec::new();
    let mut lzw = Lzw::new(frame.min_code);
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut ended = false;
    loop {
        let size = usize::from(byte(data, at)?);
        at += 1;
        if size == 0 {
            break;
        }
        for &value in bytes(data, at, size)? {
            if ended {
                break;
            }
            held |= u32::from(value) << bits;
            bits += 8;
            while !ended && bits >= u32::from(lzw.width) {
                let code = (held & ((1 << lzw.width) - 1)) as u16;
                held >>= lzw.width;
                bits -= u32::from(lzw.width);
                ended = !lzw.code(code, &mut out, total)?;
            }
        }
        at += size;
    }
    if out.len() < total {
        return Err(malformed("the GIF image data ends early"));
    }
    Ok((out, at))
}

/// GIF's variable-width LZW: each code is a prefix code plus one index.
struct Lzw {
    min_code: u8,
    width: u8,
    next: u16,
    previous: Option<u16>,
    prefix: Vec<u16>,
    suffix: Vec<u8>,
    first: Vec<u8>,
    stack: Vec<u8>,
}

impl Lzw {
    fn new(min_code: u8) -> Self {
        let roots = 1u16 << min_code;
        let mut lzw = Lzw {
            min_code,
            width: min_code + 1,
            next: roots + 2,
            previous: None,
            prefix: vec![0; MAX_CODES],
            suffix: vec![0; MAX_CODES],
            first: vec![0; MAX_CODES],
            stack: Vec::with_capacity(MAX_CODES),
        };
        for root in 0..roots {
            lzw.set(root, 0, root as u8, root as u8);
        }
        lzw
    }

    fn clear(&self) -> u16 {
        1 << self.min_code
    }

    fn set(&mut self, code: u16, prefix: u16, suffix: u8, first: u8) {
        let at = usize::from(code);
        if let (Some(p), Some(s), Some(f)) = (
            self.prefix.get_mut(at),
            self.suffix.get_mut(at),
            self.first.get_mut(at),
        ) {
            (*p, *s, *f) = (prefix, suffix, first);
        }
    }

    fn first_of(&self, code: u16) -> u8 {
        self.first.get(usize::from(code)).copied().unwrap_or(0)
    }

    /// Takes one code, writing its indices; `false` at the end code.
    fn code(&mut self, code: u16, out: &mut Vec<u8>, total: usize) -> Result<bool> {
        let clear = self.clear();
        if code == clear {
            self.width = self.min_code + 1;
            self.next = clear + 2;
            self.previous = None;
            return Ok(true);
        }
        if code == clear + 1 {
            return Ok(false);
        }
        let Some(previous) = self.previous else {
            if code >= clear {
                return Err(malformed("the GIF image data is corrupt"));
            }
            self.emit(code, out, total)?;
            self.previous = Some(code);
            return Ok(true);
        };
        if code > self.next || (code == self.next && usize::from(self.next) >= MAX_CODES) {
            return Err(malformed("the GIF image data is corrupt"));
        }
        let first = if code == self.next {
            self.first_of(previous)
        } else {
            self.first_of(code)
        };
        if usize::from(self.next) < MAX_CODES {
            self.set(self.next, previous, first, self.first_of(previous));
            self.next += 1;
            if self.next == 1 << self.width && self.width < 12 {
                self.width += 1;
            }
        }
        self.emit(code, out, total)?;
        self.previous = Some(code);
        Ok(true)
    }

    /// Appends the indices of `code` to `out`, up to `total`; memory grows
    /// with the data, not with the size the header claims.
    fn emit(&mut self, mut code: u16, out: &mut Vec<u8>, total: usize) -> Result<()> {
        self.stack.clear();
        while code >= self.clear() {
            let at = usize::from(code);
            self.stack.push(self.suffix.get(at).copied().unwrap_or(0));
            code = self.prefix.get(at).copied().unwrap_or(0);
        }
        self.stack.push(code as u8);
        let room = total.saturating_sub(out.len()).min(self.stack.len());
        out.try_reserve(room)
            .map_err(|_| Error::out_of_memory(room))?;
        out.extend(self.stack.iter().rev().take(room));
        Ok(())
    }
}

/// The frame drawn onto the logical screen, which it fits; outside it is
/// transparent.
fn compose(width: u32, height: u32, frame: &Frame, indices: &[u8]) -> Result<Image> {
    let mut color = plane(width, height, 3)?;
    let mut alpha = plane(width, height, 1)?;
    let rows = stored_rows(frame.height, frame.interlaced);
    for (stored, &row) in rows.iter().enumerate() {
        let y = frame.top + row;
        for column in 0..frame.width {
            let x = frame.left + column;
            let index = indices
                .get(stored * frame.width as usize + column as usize)
                .copied()
                .unwrap_or(0);
            if frame.transparent == Some(index) {
                continue;
            }
            let entry = bytes(frame.palette, usize::from(index) * 3, 3)
                .map_err(|_| malformed("a GIF pixel outside its colour table"))?;
            let pixel = y as usize * width as usize + x as usize;
            let slot = color
                .get_mut(pixel * 3..pixel * 3 + 3)
                .zip(alpha.get_mut(pixel))
                .ok_or_else(|| {
                    Error::internal(
                        "pdf_image_pixel_outside",
                        "a decoded pixel outside its image",
                    )
                })?;
            slot.0.copy_from_slice(entry);
            *slot.1 = u8::MAX;
        }
    }
    Ok(Image::decoded(
        width,
        height,
        Space::Rgb,
        color,
        Some(alpha),
    ))
}

/// The image row each stored row is: in order, or in GIF's four
/// interlaced passes.
fn stored_rows(height: u32, interlaced: bool) -> Vec<u32> {
    if !interlaced {
        return (0..height).collect();
    }
    [(0, 8), (4, 8), (2, 4), (1, 2)]
        .iter()
        .flat_map(|&(start, step)| (start..height).step_by(step))
        .collect()
}
