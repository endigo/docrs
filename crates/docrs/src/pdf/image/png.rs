//! PNG (ISO/IEC 15948). Passed through when PDF's Flate predictor reads the
//! data as it is: 8-bit grey, RGB or palette, no transparency, not
//! interlaced. Everything else is decoded into 8-bit colour plus alpha.

use std::io::{self, Read};

use flate2::read::ZlibDecoder;

use super::{
    ANIMATED, Budget, Image, PROFILE_IGNORED, Samples, Space, be32, byte, bytes, malformed,
    too_large, unsupported, zeroed,
};
use crate::{Error, Result};

const GREY: u8 = 0;
const RGB: u8 = 2;
const PALETTE: u8 = 3;
const GREY_ALPHA: u8 = 4;
const RGBA: u8 = 6;

struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    interlaced: bool,
}

/// What the chunks hold, in file order.
#[derive(Default)]
struct Chunks<'a> {
    palette: Option<&'a [u8]>,
    transparency: Option<&'a [u8]>,
    data: Vec<u8>,
    profile: bool,
    animated: bool,
}

pub(super) fn read(data: &[u8], budget: &Budget) -> Result<Image> {
    let (header, mut chunks) = read_chunks(data, budget)?;
    let mut image = if header.depth == 8
        && matches!(header.color, GREY | RGB | PALETTE)
        && chunks.transparency.is_none()
        && !header.interlaced
    {
        let stream = check_rows(&header, &chunks)?;
        chunks.data.truncate(stream);
        Image {
            width: header.width,
            height: header.height,
            space: match header.color {
                GREY => Space::Gray,
                RGB => Space::Rgb,
                _ => Space::Indexed(chunks.palette.unwrap_or_default().to_vec()),
            },
            samples: Samples::Png(chunks.data),
            inverted: false,
            alpha: None,
            warnings: Vec::new(),
        }
    } else {
        decode(&header, &chunks)?
    };
    if chunks.profile {
        image.warn(PROFILE_IGNORED);
    }
    if chunks.animated {
        image.warn(ANIMATED);
    }
    Ok(image)
}

fn read_chunks<'a>(data: &'a [u8], budget: &Budget) -> Result<(Header, Chunks<'a>)> {
    let mut at = 8;
    let mut header = None;
    let mut chunks = Chunks::default();
    loop {
        let length = usize::try_from(be32(data, at)?).map_err(|_| malformed("a huge chunk"))?;
        let kind = bytes(data, at + 4, 4)?;
        let body = bytes(data, at + 8, length)?;
        let stored = be32(data, at + 8 + length)?;
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(body);
        let intact = crc.finalize() == stored;
        at += 12 + length;
        if header.is_none() && kind != b"IHDR" {
            return Err(malformed("the PNG does not start with IHDR"));
        }
        let critical = kind.first().is_some_and(u8::is_ascii_uppercase);
        if !intact {
            // As libpng and browsers do: a damaged ancillary chunk is
            // treated as absent.
            if critical {
                return Err(malformed("a PNG chunk fails its CRC"));
            }
            continue;
        }
        match kind {
            b"IHDR" if header.is_none() => {
                let read = read_header(body)?;
                budget.check(read.width, read.height)?;
                header = Some(read);
            }
            b"PLTE" => chunks.palette = Some(body),
            b"tRNS" => chunks.transparency = Some(body),
            b"IDAT" => {
                chunks
                    .data
                    .try_reserve(body.len())
                    .map_err(|_| Error::out_of_memory(format_args!("{} bytes", body.len())))?;
                chunks.data.extend_from_slice(body);
            }
            b"iCCP" => chunks.profile = true,
            b"acTL" => chunks.animated = true,
            b"IEND" => break,
            _ if critical => return Err(malformed("an unknown critical PNG chunk")),
            _ => {}
        }
    }
    let header = header.ok_or_else(|| malformed("the PNG has no IHDR"))?;
    check_palette(&header, &chunks)?;
    Ok((header, chunks))
}

fn read_header(body: &[u8]) -> Result<Header> {
    let header = Header {
        width: be32(body, 0)?,
        height: be32(body, 4)?,
        depth: byte(body, 8)?,
        color: byte(body, 9)?,
        interlaced: byte(body, 12)? == 1,
    };
    let depth_allowed: &[u8] = match header.color {
        GREY => &[1, 2, 4, 8, 16],
        PALETTE => &[1, 2, 4, 8],
        RGB | GREY_ALPHA | RGBA => &[8, 16],
        _ => &[],
    };
    if body.len() != 13
        || !depth_allowed.contains(&header.depth)
        || byte(body, 10)? != 0
        || byte(body, 11)? != 0
        || byte(body, 12)? > 1
        || header.width > i32::MAX as u32
        || header.height > i32::MAX as u32
    {
        return Err(malformed("the PNG header is invalid"));
    }
    Ok(header)
}

fn check_palette(header: &Header, chunks: &Chunks) -> Result<()> {
    if header.color != PALETTE {
        return Ok(());
    }
    let entries = chunks.palette.map_or(0, |p| p.len() / 3);
    let fits = chunks
        .palette
        .is_some_and(|p| p.len() % 3 == 0 && entries >= 1 && entries <= 1 << header.depth);
    if !fits || chunks.transparency.is_some_and(|t| t.len() > entries) {
        return Err(malformed("the PNG palette is missing or invalid"));
    }
    Ok(())
}

/// Each pass of the image: where it starts, its step and its size.
struct Pass {
    x: u32,
    y: u32,
    dx: u32,
    dy: u32,
    width: u32,
    height: u32,
}

const ADAM7: [(u32, u32, u32, u32); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

fn passes(header: &Header) -> Vec<Pass> {
    let steps: &[(u32, u32, u32, u32)] = if header.interlaced {
        &ADAM7
    } else {
        &[(0, 0, 1, 1)]
    };
    let span = |size: u32, start: u32, step: u32| (size.saturating_sub(start)).div_ceil(step);
    steps
        .iter()
        .map(|&(x, y, dx, dy)| Pass {
            x,
            y,
            dx,
            dy,
            width: span(header.width, x, dx),
            height: span(header.height, y, dy),
        })
        .filter(|pass| pass.width > 0 && pass.height > 0)
        .collect()
}

fn channels(color: u8) -> usize {
    match color {
        RGB => 3,
        GREY_ALPHA => 2,
        RGBA => 4,
        _ => 1,
    }
}

/// Bytes in one row of `width` pixels, without the filter byte.
fn row_bytes(header: &Header, width: u32) -> Result<usize> {
    usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(channels(header.color) * usize::from(header.depth)))
        .map(|bits| bits.div_ceil(8))
        .ok_or_else(too_large)
}

/// The inflated size of every pass, filter bytes included.
fn inflated_size(header: &Header) -> Result<usize> {
    passes(header).iter().try_fold(0usize, |total, pass| {
        let rows = usize::try_from(pass.height).ok();
        row_bytes(header, pass.width)?
            .checked_add(1)
            .zip(rows)
            .and_then(|(row, rows)| row.checked_mul(rows))
            .and_then(|size| total.checked_add(size))
            .ok_or_else(too_large)
    })
}

/// Deflate expands data at most about 1,032 times: data shorter than that
/// share of the rows cannot hold them, and is refused before anything is
/// allocated for them.
const MAX_DEFLATE_RATIO: u64 = 1_033;

/// The longest row decoded, filter byte excluded: 4 MiB, which is 262,144
/// pixels of 16-bit RGBA and 4,194,304 of 8-bit grey.
const MAX_ROW_BYTES: usize = 4 << 20;

/// Inflates and unfilters each row of every pass in turn, holding two rows
/// at a time; returns the decoder after the last row.
fn each_row<'a>(
    header: &Header,
    data: &'a [u8],
    mut visit: impl FnMut(&Pass, u32, &[u8]) -> Result<()>,
) -> Result<ZlibDecoder<&'a [u8]>> {
    let size = inflated_size(header)?;
    if (data.len() as u64).saturating_mul(MAX_DEFLATE_RATIO) < size as u64 {
        return Err(malformed("the PNG image data ends early"));
    }
    let widest = row_bytes(header, header.width)?;
    if widest > MAX_ROW_BYTES {
        return Err(unsupported(format!(
            "a PNG with rows of more than {MAX_ROW_BYTES} bytes"
        )));
    }
    let mut line = zeroed(widest + 1)?;
    let mut previous = zeroed(widest)?;
    let bpp = (channels(header.color) * usize::from(header.depth)).div_ceil(8);
    let mut decoder = ZlibDecoder::new(data);
    for pass in passes(header) {
        let length = row_bytes(header, pass.width)?;
        let (line, previous) = line
            .get_mut(..=length)
            .zip(previous.get_mut(..length))
            .ok_or_else(|| Error::internal("pdf_image_row", "a PNG pass wider than the image"))?;
        previous.fill(0);
        for row in 0..pass.height {
            decoder.read_exact(line).map_err(|err| match err.kind() {
                io::ErrorKind::UnexpectedEof => malformed("the PNG image data ends early"),
                _ => malformed("the PNG image data is corrupt"),
            })?;
            let (&mut filter, line) = line
                .split_first_mut()
                .ok_or_else(|| malformed("an empty PNG row"))?;
            unfilter(filter, line, previous, bpp)?;
            visit(&pass, row, line)?;
            previous.copy_from_slice(line);
        }
    }
    Ok(decoder)
}

/// Checks the passed-through data as decoding would, without keeping it:
/// every row with a known filter, every index in the palette, and nothing
/// after the last row. Returns the length of the zlib stream, so the PDF
/// holds no more than the image.
fn check_rows(header: &Header, chunks: &Chunks) -> Result<usize> {
    let entries = chunks.palette.map_or(0, |palette| palette.len() / 3);
    let mut decoder = each_row(header, &chunks.data, |_, _, line| {
        if header.color == PALETTE && line.iter().any(|&index| usize::from(index) >= entries) {
            return Err(malformed("a PNG pixel outside its palette"));
        }
        Ok(())
    })?;
    match decoder.read(&mut [0u8; 1]) {
        Ok(0) => usize::try_from(decoder.total_in()).map_err(|_| too_large()),
        Ok(_) => Err(malformed("the PNG image data runs past its last row")),
        Err(_) => Err(malformed("the PNG image data is corrupt")),
    }
}

/// What a decoded pixel becomes.
struct Target<'a> {
    header: &'a Header,
    palette: &'a [u8],
    transparency: Option<&'a [u8]>,
    components: usize,
    /// Rows from the top, as many as the data has reached.
    color: Vec<u8>,
    alpha: Vec<u8>,
}

fn decode(header: &Header, chunks: &Chunks) -> Result<Image> {
    let grey = matches!(header.color, GREY | GREY_ALPHA);
    let components = if grey { 1 } else { 3 };
    let mut target = Target {
        header,
        palette: chunks.palette.unwrap_or_default(),
        transparency: chunks.transparency,
        components,
        color: Vec::new(),
        alpha: Vec::new(),
    };
    each_row(header, &chunks.data, |pass, row, line| {
        let y = pass.y + row * pass.dy;
        target.cover(y + 1)?;
        (0..pass.width)
            .try_for_each(|column| target.put(pass.x + column * pass.dx, y, line, column))
    })?;
    target.cover(header.height)?;
    let space = if grey { Space::Gray } else { Space::Rgb };
    Ok(Image::decoded(
        header.width,
        header.height,
        space,
        target.color,
        Some(target.alpha),
    ))
}

fn unfilter(filter: u8, line: &mut [u8], previous: &[u8], bpp: usize) -> Result<()> {
    if filter > 4 {
        return Err(malformed("an unknown PNG filter"));
    }
    for at in 0..line.len() {
        let back = |row: &[u8]| at.checked_sub(bpp).and_then(|i| row.get(i)).copied();
        let a = back(line).unwrap_or(0);
        let b = previous.get(at).copied().unwrap_or(0);
        let c = back(previous).unwrap_or(0);
        let predicted = match filter {
            1 => a,
            2 => b,
            3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        if let Some(value) = line.get_mut(at) {
            *value = value.wrapping_add(predicted);
        }
    }
    Ok(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

impl Target<'_> {
    /// Grows the planes to hold the first `rows` rows, so that memory follows
    /// the data read and not the size the header claims.
    fn cover(&mut self, rows: u32) -> Result<()> {
        let pixels = |rows: u32| {
            (self.header.width as usize)
                .checked_mul(rows.min(self.header.height) as usize)
                .ok_or_else(too_large)
        };
        let (len, full) = (pixels(rows)?, pixels(self.header.height)?);
        let scale = |n: usize| n.checked_mul(self.components).ok_or_else(too_large);
        grow(&mut self.alpha, len, full)?;
        grow(&mut self.color, scale(len)?, scale(full)?)
    }

    /// Pixel `column` of an unfiltered `line`, placed at `x`, `y`.
    fn put(&mut self, x: u32, y: u32, line: &[u8], column: u32) -> Result<()> {
        let header = self.header;
        let channels = channels(header.color);
        let mut samples = [0u16; 4];
        for (channel, sample) in samples.iter_mut().enumerate().take(channels) {
            *sample = sample_at(line, column as usize * channels + channel, header.depth);
        }
        let [first, second, third, fourth] = samples;
        let depth_mask = ((1u32 << header.depth) - 1) as u16;
        let opaque = |key: &[u16]| {
            let matches = self.transparency.is_some_and(|t| {
                key.iter()
                    .enumerate()
                    .all(|(i, &v)| be16_at(t, i).map(|k| k & depth_mask) == Some(v))
            });
            if matches { 0 } else { u8::MAX }
        };
        let (color, alpha): ([u8; 3], u8) = match header.color {
            GREY => {
                let v = to_byte(first, header.depth);
                ([v; 3], opaque(&[first]))
            }
            RGB => (
                [first, second, third].map(|v| to_byte(v, header.depth)),
                opaque(&[first, second, third]),
            ),
            PALETTE => {
                let index = usize::from(first);
                let entry = bytes(self.palette, index * 3, 3)
                    .map_err(|_| malformed("a PNG pixel outside its palette"))?;
                let alpha = self
                    .transparency
                    .and_then(|t| t.get(index))
                    .copied()
                    .unwrap_or(u8::MAX);
                ([0, 1, 2].map(|i| entry.get(i).copied().unwrap_or(0)), alpha)
            }
            GREY_ALPHA => (
                [to_byte(first, header.depth); 3],
                to_byte(second, header.depth),
            ),
            _ => (
                [first, second, third].map(|v| to_byte(v, header.depth)),
                to_byte(fourth, header.depth),
            ),
        };
        let pixel = y as usize * header.width as usize + x as usize;
        for (i, value) in color.iter().take(self.components).enumerate() {
            set(&mut self.color, pixel * self.components + i, *value)?;
        }
        set(&mut self.alpha, pixel, alpha)
    }
}

/// `plane` zero-extended to `len`, its capacity at most doubling and never
/// past `full`.
fn grow(plane: &mut Vec<u8>, len: usize, full: usize) -> Result<()> {
    if plane.len() >= len {
        return Ok(());
    }
    if plane.capacity() < len {
        let capacity = len.max(plane.capacity() * 2).min(full);
        plane
            .try_reserve_exact(capacity - plane.len())
            .map_err(|_| Error::out_of_memory(format_args!("{capacity} bytes")))?;
    }
    plane.resize(len, 0);
    Ok(())
}

fn set(plane: &mut [u8], at: usize, value: u8) -> Result<()> {
    let slot = plane.get_mut(at).ok_or_else(|| {
        Error::internal(
            "pdf_image_pixel_outside",
            "a decoded pixel outside its image",
        )
    })?;
    *slot = value;
    Ok(())
}

/// Sample `index` of a row of `depth`-bit samples, packed from the high bit.
fn sample_at(line: &[u8], index: usize, depth: u8) -> u16 {
    let byte = |at: usize| line.get(at).copied().unwrap_or(0);
    match depth {
        16 => u16::from_be_bytes([byte(index * 2), byte(index * 2 + 1)]),
        8 => byte(index).into(),
        _ => {
            let bit = index * usize::from(depth);
            let shift = 8 - usize::from(depth) - bit % 8;
            u16::from(byte(bit / 8) >> shift) & ((1 << depth) - 1)
        }
    }
}

/// A `tRNS` grey or RGB value: two bytes per channel.
fn be16_at(data: &[u8], index: usize) -> Option<u16> {
    let pair = data.get(index * 2..index * 2 + 2)?;
    Some(u16::from_be_bytes([*pair.first()?, *pair.get(1)?]))
}

fn to_byte(value: u16, depth: u8) -> u8 {
    match depth {
        16 => (value >> 8) as u8,
        8 => value as u8,
        _ => (u32::from(value) * 255 / ((1 << depth) - 1)) as u8,
    }
}
