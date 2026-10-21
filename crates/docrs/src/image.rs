//! Image characterization (upstream `docx/image/*`, python-docx, MIT; see
//! `NOTICE.md`).
//!
//! [`Image::from_blob`] recognizes BMP, GIF, JPEG (JFIF and Exif), PNG and
//! TIFF by the same signatures, at the same offsets and in the same order as
//! upstream `docx.image.SIGNATURES`, and reads pixel size and resolution with
//! the same header walks (`bmp.py`, `gif.py`, `jpeg.py`, `png.py`,
//! `tiff.py`), including Python's `round` (ties to even) and `int()`
//! truncation in the derived values.
//!
//! Failures are [`crate::ErrorKind::InvalidArgument`] (the caller supplied
//! unusable image bytes) with a stable reason; [`upstream_exception`] names
//! the exception class upstream raises for the same input.

use serde::Serialize;

use crate::{Error, Result};

const EMUS_PER_INCH: f64 = 914_400.0;

/// The upstream exception class for an image error `reason`
/// (`UnrecognizedImageError`, `UnexpectedEndOfFileError`, ...).
pub fn upstream_exception(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "unrecognized_image" => "UnrecognizedImageError",
        "unexpected_end_of_file" => "UnexpectedEndOfFileError",
        "invalid_image_stream" => "InvalidImageStreamError",
        "jpeg_unexpected_end_of_file" => "Exception",
        "missing_jpeg_marker" => "KeyError",
        "truncated_struct" => "error",
        "invalid_utf8_string" => "UnicodeDecodeError",
        "zero_division" => "ZeroDivisionError",
        "non_numeric_value" => "TypeError",
        _ => return None,
    })
}

fn err(reason: &'static str, message: impl Into<String>) -> Error {
    Error::invalid_argument(reason, message)
}

fn eof() -> Error {
    err(
        "unexpected_end_of_file",
        "unexpected end of file while reading the image stream",
    )
}

/// Python `round()` for floats (ties to even), then `int()`.
fn py_round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// Recognized image format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG with a JFIF (APP0) header.
    Jfif,
    /// JPEG with an Exif (APP1) header.
    Exif,
    /// GIF87a / GIF89a.
    Gif,
    /// TIFF (either byte order).
    Tiff,
    /// Windows bitmap.
    Bmp,
}

impl ImageFormat {
    /// MIME content type (upstream `MIME_TYPE`).
    pub fn content_type(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jfif | ImageFormat::Exif => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Tiff => "image/tiff",
            ImageFormat::Bmp => "image/bmp",
        }
    }

    /// Default filename extension (`default_ext`).
    pub fn default_ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jfif | ImageFormat::Exif => "jpg",
            ImageFormat::Gif => "gif",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Bmp => "bmp",
        }
    }
}

/// A characterized image (upstream `Image` created from an anonymous stream).
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    blob: Vec<u8>,
    format: ImageFormat,
    px_width: Option<i64>,
    px_height: Option<i64>,
    horz_dpi: i64,
    vert_dpi: i64,
    sha1: String,
    filename: Option<String>,
}

impl Image {
    /// Characterize `blob` (upstream `Image.from_blob`).
    pub fn from_blob(blob: &[u8]) -> Result<Image> {
        let format = detect(blob)?;
        let header = match format {
            ImageFormat::Png => png(blob)?,
            ImageFormat::Jfif | ImageFormat::Exif => jpeg(blob, format)?,
            ImageFormat::Gif => gif(blob)?,
            ImageFormat::Tiff => {
                let tiff = Tiff::parse(blob)?;
                Header {
                    px_width: tiff.px_width(),
                    px_height: tiff.px_height(),
                    horz_dpi: tiff.dpi(TAG_X_RESOLUTION)?,
                    vert_dpi: tiff.dpi(TAG_Y_RESOLUTION)?,
                }
            }
            ImageFormat::Bmp => bmp(blob)?,
        };
        Ok(Image {
            blob: blob.to_vec(),
            format,
            px_width: header.px_width,
            px_height: header.px_height,
            horz_dpi: header.horz_dpi,
            vert_dpi: header.vert_dpi,
            sha1: sha1_hex(blob),
            filename: None,
        })
    }

    /// Load and characterize an image file (upstream `Image.from_file` with
    /// a path): `filename` is the path's base name and `ext` its extension.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Image> {
        let path = path.as_ref();
        let blob = std::fs::read(path).map_err(|err| {
            Error::io(
                "read_image",
                format!("cannot read image {}", path.display()),
                err,
            )
        })?;
        let mut image = Image::from_blob(&blob)?;
        // The platform's base name, as `os.path.basename` (`ntpath` on Windows).
        image.filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        Ok(image)
    }

    /// The image bytes.
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// Recognized format.
    pub fn format(&self) -> ImageFormat {
        self.format
    }

    /// MIME content type.
    pub fn content_type(&self) -> &'static str {
        self.format.content_type()
    }

    /// Extension (no leading period) of [`Image::filename`]: from the file
    /// name when loaded with [`Image::from_file`], otherwise canonical for the
    /// format.
    pub fn ext(&self) -> &str {
        match &self.filename {
            Some(name) => py_splitext_ext(name),
            None => self.format.default_ext(),
        }
    }

    /// Original file name for [`Image::from_file`], otherwise the generic
    /// `image.<ext>` upstream uses for stream input.
    pub fn filename(&self) -> String {
        match &self.filename {
            Some(name) => name.clone(),
            None => format!("image.{}", self.ext()),
        }
    }

    /// Horizontal pixel count (`None` when a TIFF lacks an integer
    /// `ImageWidth`).
    pub fn px_width(&self) -> Option<i64> {
        self.px_width
    }

    /// Vertical pixel count.
    pub fn px_height(&self) -> Option<i64> {
        self.px_height
    }

    /// Horizontal dots per inch.
    pub fn horz_dpi(&self) -> i64 {
        self.horz_dpi
    }

    /// Vertical dots per inch.
    pub fn vert_dpi(&self) -> i64 {
        self.vert_dpi
    }

    /// Lowercase hex SHA-1 of the bytes.
    pub fn sha1(&self) -> &str {
        &self.sha1
    }

    /// Native width in EMU (`Inches(px_width / horz_dpi)`).
    pub fn width(&self) -> Result<i64> {
        native(self.px_width, self.horz_dpi)
    }

    /// Native height in EMU (`Inches(px_height / vert_dpi)`).
    pub fn height(&self) -> Result<i64> {
        native(self.px_height, self.vert_dpi)
    }

    /// Upstream `scaled_dimensions(width, height)` in EMU: both given are
    /// returned as is; one given scales the other preserving aspect ratio
    /// (Python `round`); neither gives the native size.
    pub fn scaled_dimensions(&self, width: Option<i64>, height: Option<i64>) -> Result<(i64, i64)> {
        match (width, height) {
            (None, None) => Ok((self.width()?, self.height()?)),
            (Some(width), Some(height)) => Ok((width, height)),
            (None, Some(height)) => {
                let native_height = self.height()?;
                if native_height == 0 {
                    return Err(err("zero_division", "image has zero native height"));
                }
                let factor = height as f64 / native_height as f64;
                Ok((py_round(self.width()? as f64 * factor), height))
            }
            (Some(width), None) => {
                let native_width = self.width()?;
                if native_width == 0 {
                    return Err(err("zero_division", "image has zero native width"));
                }
                let factor = width as f64 / native_width as f64;
                Ok((width, py_round(self.height()? as f64 * factor)))
            }
        }
    }

    /// Characterization report (`content_type`, pixel size, dpi, `ext`,
    /// `filename`, native `width`/`height` in EMU or `null`, `sha1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "content_type": self.content_type(),
            "px_width": self.px_width,
            "px_height": self.px_height,
            "horz_dpi": self.horz_dpi,
            "vert_dpi": self.vert_dpi,
            "ext": self.ext(),
            "filename": self.filename(),
            "width": self.width().ok(),
            "height": self.height().ok(),
            "sha1": self.sha1,
        })
    }
}

/// `os.path.splitext(filename)[1][1:]` (leading dots do not start an
/// extension).
pub(crate) fn py_splitext_ext(filename: &str) -> &str {
    let stem_start = filename.len() - filename.trim_start_matches('.').len();
    match filename[stem_start..].rfind('.') {
        Some(dot) => &filename[stem_start + dot + 1..],
        None => "",
    }
}

fn native(px: Option<i64>, dpi: i64) -> Result<i64> {
    let Some(px) = px else {
        return Err(err(
            "non_numeric_value",
            "image pixel dimension is not available",
        ));
    };
    if dpi == 0 {
        return Err(err("zero_division", "image resolution is zero dpi"));
    }
    Ok(((px as f64 / dpi as f64) * EMUS_PER_INCH) as i64)
}

/// Upstream `SIGNATURES`, checked in order against the first 32 bytes.
fn detect(blob: &[u8]) -> Result<ImageFormat> {
    let header = &blob[..blob.len().min(32)];
    let signatures: &[(ImageFormat, usize, &[u8])] = &[
        (ImageFormat::Png, 0, b"\x89PNG\r\n\x1a\n"),
        (ImageFormat::Jfif, 6, b"JFIF"),
        (ImageFormat::Exif, 6, b"Exif"),
        (ImageFormat::Gif, 0, b"GIF87a"),
        (ImageFormat::Gif, 0, b"GIF89a"),
        (ImageFormat::Tiff, 0, b"MM\x00*"),
        (ImageFormat::Tiff, 0, b"II*\x00"),
        (ImageFormat::Bmp, 0, b"BM"),
    ];
    for (format, offset, signature) in signatures {
        let end = (offset + signature.len()).min(header.len());
        let start = (*offset).min(end);
        if &header[start..end] == *signature {
            return Ok(*format);
        }
    }
    Err(err(
        "unrecognized_image",
        "the image stream could not be recognized as BMP, GIF, JPEG, PNG or TIFF",
    ))
}

struct Header {
    px_width: Option<i64>,
    px_height: Option<i64>,
    horz_dpi: i64,
    vert_dpi: i64,
}

/// Upstream `StreamReader`: reads at `base_offset + base + offset`.
#[derive(Clone, Copy)]
struct Reader<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl<'a> Reader<'a> {
    fn bytes(&self, at: u64, count: u64) -> Result<&'a [u8]> {
        let start = usize::try_from(at).map_err(|_| eof())?;
        let count = usize::try_from(count).map_err(|_| eof())?;
        let end = start.checked_add(count).ok_or_else(eof)?;
        self.data.get(start..end).ok_or_else(eof)
    }

    fn byte(&self, at: u64) -> Result<u8> {
        Ok(self.bytes(at, 1)?[0])
    }

    fn short(&self, at: u64) -> Result<u16> {
        let b = self.bytes(at, 2)?;
        Ok(if self.big_endian {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    }

    fn long(&self, at: u64) -> Result<u32> {
        let b = self.bytes(at, 4)?;
        let array = [b[0], b[1], b[2], b[3]];
        Ok(if self.big_endian {
            u32::from_be_bytes(array)
        } else {
            u32::from_le_bytes(array)
        })
    }

    fn string(&self, at: u64, count: u64) -> Result<String> {
        let bytes = self.bytes(at, count)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| err("invalid_utf8_string", "image header string is not UTF-8"))
    }
}

fn bmp(blob: &[u8]) -> Result<Header> {
    let reader = Reader {
        data: blob,
        big_endian: false,
    };
    let px_width = reader.long(0x12)?;
    let px_height = reader.long(0x16)?;
    let horz = reader.long(0x26)?;
    let vert = reader.long(0x2A)?;
    let dpi = |px_per_meter: u32| {
        if px_per_meter == 0 {
            96
        } else {
            py_round(px_per_meter as f64 * 0.0254)
        }
    };
    Ok(Header {
        px_width: Some(px_width.into()),
        px_height: Some(px_height.into()),
        horz_dpi: dpi(horz),
        vert_dpi: dpi(vert),
    })
}

fn gif(blob: &[u8]) -> Result<Header> {
    let bytes = blob.get(6..10).ok_or_else(|| {
        err(
            "truncated_struct",
            "GIF logical screen descriptor is truncated",
        )
    })?;
    Ok(Header {
        px_width: Some(u16::from_le_bytes([bytes[0], bytes[1]]).into()),
        px_height: Some(u16::from_le_bytes([bytes[2], bytes[3]]).into()),
        horz_dpi: 72,
        vert_dpi: 72,
    })
}

fn png(blob: &[u8]) -> Result<Header> {
    let reader = Reader {
        data: blob,
        big_endian: true,
    };
    let mut ihdr: Option<(u32, u32)> = None;
    let mut phys: Option<(u32, u32, u8)> = None;
    let mut chunk_offset: u64 = 8;
    loop {
        let data_len = reader.long(chunk_offset)?;
        let chunk_type = reader.string(chunk_offset + 4, 4)?;
        let data_offset = chunk_offset + 8;
        match chunk_type.as_str() {
            "IHDR" => {
                let value = (reader.long(data_offset)?, reader.long(data_offset + 4)?);
                ihdr.get_or_insert(value);
            }
            "pHYs" => {
                let value = (
                    reader.long(data_offset)?,
                    reader.long(data_offset + 4)?,
                    reader.byte(data_offset + 8)?,
                );
                phys.get_or_insert(value);
            }
            _ => {}
        }
        if chunk_type == "IEND" {
            break;
        }
        chunk_offset += 4 + 4 + u64::from(data_len) + 4;
    }
    let (px_width, px_height) =
        ihdr.ok_or_else(|| err("invalid_image_stream", "no IHDR chunk in PNG image"))?;
    let dpi = |units: u8, per_unit: u32| {
        if units == 1 && per_unit != 0 {
            py_round(per_unit as f64 * 0.0254)
        } else {
            72
        }
    };
    let (horz_dpi, vert_dpi) = match phys {
        None => (72, 72),
        Some((horz, vert, units)) => (dpi(units, horz), dpi(units, vert)),
    };
    Ok(Header {
        px_width: Some(px_width.into()),
        px_height: Some(px_height.into()),
        horz_dpi,
        vert_dpi,
    })
}

// ---------------------------------------------------------------- JPEG

const SOS: u8 = 0xDA;
const EOI: u8 = 0xD9;
const APP0: u8 = 0xE0;
const APP1: u8 = 0xE1;
const SOF_CODES: &[u8] = &[
    0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
];
const STANDALONE: &[u8] = &[
    0x01, 0xD8, 0xD9, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7,
];

enum Marker {
    App0 { horz_dpi: i64, vert_dpi: i64 },
    App1 { horz_dpi: i64, vert_dpi: i64 },
    Sof { px_width: u16, px_height: u16 },
    Other,
}

fn jpeg_eof() -> Error {
    err(
        "jpeg_unexpected_end_of_file",
        "unexpected end of file while scanning JPEG markers",
    )
}

/// `_MarkerFinder.next(start)`: `(marker_code, segment_offset)`.
fn next_marker(blob: &[u8], start: u64) -> Result<(u8, u64)> {
    let read = |position: u64| -> Result<u8> {
        usize::try_from(position)
            .ok()
            .and_then(|p| blob.get(p).copied())
            .ok_or_else(jpeg_eof)
    };
    let mut position = start;
    loop {
        while read(position)? != 0xFF {
            position += 1;
        }
        position += 1;
        let mut byte = read(position)?;
        while byte == 0xFF {
            position += 1;
            byte = read(position)?;
        }
        if byte == 0x00 {
            continue;
        }
        return Ok((byte, position + 1));
    }
}

fn jpeg(blob: &[u8], format: ImageFormat) -> Result<Header> {
    let reader = Reader {
        data: blob,
        big_endian: true,
    };
    let mut markers: Vec<(u8, Marker)> = Vec::new();
    let mut start = 0u64;
    loop {
        let (code, offset) = next_marker(blob, start)?;
        let (segment_length, marker) = if code == APP0 {
            let length = reader.short(offset)?;
            let units = reader.byte(offset + 9)?;
            let x_density = reader.short(offset + 10)?;
            let y_density = reader.short(offset + 12)?;
            let dpi = |density: u16| match units {
                1 => i64::from(density),
                2 => py_round(f64::from(density) * 2.54),
                _ => 72,
            };
            (
                length,
                Marker::App0 {
                    horz_dpi: dpi(x_density),
                    vert_dpi: dpi(y_density),
                },
            )
        } else if code == APP1 {
            let length = reader.short(offset)?;
            let signature_start = usize::try_from(offset + 2).unwrap_or(usize::MAX);
            let signature = blob
                .get(signature_start..)
                .map(|rest| &rest[..rest.len().min(6)])
                .unwrap_or(&[]);
            if signature != b"Exif\x00\x00" {
                (
                    length,
                    Marker::App1 {
                        horz_dpi: 72,
                        vert_dpi: 72,
                    },
                )
            } else {
                // `stream.read(segment_length - 8)`: a negative count reads
                // the rest of the stream.
                let from = usize::try_from(offset + 8).unwrap_or(usize::MAX);
                let rest = blob.get(from..).unwrap_or(&[]);
                let wanted = i64::from(length) - 8;
                let segment = if wanted < 0 {
                    rest
                } else {
                    &rest[..rest.len().min(wanted as usize)]
                };
                let tiff = Tiff::parse(segment)?;
                (
                    length,
                    Marker::App1 {
                        horz_dpi: tiff.dpi(TAG_X_RESOLUTION)?,
                        vert_dpi: tiff.dpi(TAG_Y_RESOLUTION)?,
                    },
                )
            }
        } else if SOF_CODES.contains(&code) {
            let length = reader.short(offset)?;
            let px_height = reader.short(offset + 3)?;
            let px_width = reader.short(offset + 5)?;
            (
                length,
                Marker::Sof {
                    px_width,
                    px_height,
                },
            )
        } else if STANDALONE.contains(&code) {
            (0, Marker::Other)
        } else {
            (reader.short(offset)?, Marker::Other)
        };
        markers.push((code, marker));
        if code == SOS || code == EOI {
            break;
        }
        start = offset + u64::from(segment_length);
    }
    let sof = markers
        .iter()
        .find_map(|(_, marker)| match marker {
            Marker::Sof {
                px_width,
                px_height,
            } => Some((*px_width, *px_height)),
            _ => None,
        })
        .ok_or_else(|| {
            err(
                "missing_jpeg_marker",
                "no start of frame (SOFn) marker in image",
            )
        })?;
    let dpi = markers
        .iter()
        .find_map(|(_, marker)| match (format, marker) {
            (ImageFormat::Jfif, Marker::App0 { horz_dpi, vert_dpi })
            | (ImageFormat::Exif, Marker::App1 { horz_dpi, vert_dpi }) => {
                Some((*horz_dpi, *vert_dpi))
            }
            _ => None,
        });
    let (horz_dpi, vert_dpi) = dpi.ok_or_else(|| {
        err(
            "missing_jpeg_marker",
            if format == ImageFormat::Jfif {
                "no APP0 marker in image"
            } else {
                "no APP1 marker in image"
            },
        )
    })?;
    Ok(Header {
        px_width: Some(sof.0.into()),
        px_height: Some(sof.1.into()),
        horz_dpi,
        vert_dpi,
    })
}

// ---------------------------------------------------------------- TIFF

const TAG_IMAGE_WIDTH: u16 = 0x0100;
const TAG_IMAGE_LENGTH: u16 = 0x0101;
const TAG_X_RESOLUTION: u16 = 0x011A;
const TAG_Y_RESOLUTION: u16 = 0x011B;
const TAG_RESOLUTION_UNIT: u16 = 0x0128;

/// A parsed IFD entry value (Python int, float or str).
#[derive(Debug, Clone, PartialEq)]
enum TiffValue {
    Int(i64),
    Float(f64),
    Str,
}

impl TiffValue {
    fn equals(&self, number: i64) -> bool {
        match self {
            TiffValue::Int(value) => *value == number,
            TiffValue::Float(value) => *value == number as f64,
            TiffValue::Str => false,
        }
    }
}

struct Tiff {
    entries: Vec<(u16, TiffValue)>,
}

impl Tiff {
    fn parse(data: &[u8]) -> Result<Tiff> {
        let big_endian = data.get(..2) == Some(b"MM");
        let reader = Reader { data, big_endian };
        let ifd0 = u64::from(reader.long(4)?);
        let count = reader.short(ifd0)?;
        let mut entries: Vec<(u16, TiffValue)> = Vec::new();
        for index in 0..u64::from(count) {
            let offset = ifd0 + 2 + index * 12;
            let field_type = reader.short(offset + 2)?;
            let tag = reader.short(offset)?;
            let value_count = reader.long(offset + 4)?;
            let value_offset = u64::from(reader.long(offset + 8)?);
            let value = match field_type {
                2 => {
                    let wanted = i64::from(value_count) - 1;
                    if wanted < 0 {
                        return Err(err(
                            "truncated_struct",
                            "TIFF ASCII entry has a zero value count",
                        ));
                    }
                    reader.string(value_offset, wanted as u64)?;
                    TiffValue::Str
                }
                3 if value_count == 1 => TiffValue::Int(reader.short(offset + 8)?.into()),
                4 if value_count == 1 => TiffValue::Int(reader.long(offset + 8)?.into()),
                5 if value_count == 1 => {
                    let numerator = reader.long(value_offset)?;
                    let denominator = reader.long(value_offset + 4)?;
                    if denominator == 0 {
                        return Err(err(
                            "zero_division",
                            "TIFF rational entry has a zero denominator",
                        ));
                    }
                    TiffValue::Float(f64::from(numerator) / f64::from(denominator))
                }
                _ => TiffValue::Str,
            };
            match entries.iter_mut().find(|(existing, _)| *existing == tag) {
                Some(slot) => slot.1 = value,
                None => entries.push((tag, value)),
            }
        }
        Ok(Tiff { entries })
    }

    fn get(&self, tag: u16) -> Option<&TiffValue> {
        self.entries
            .iter()
            .find(|(existing, _)| *existing == tag)
            .map(|(_, value)| value)
    }

    fn dimension(&self, tag: u16) -> Option<i64> {
        match self.get(tag) {
            Some(TiffValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    fn px_width(&self) -> Option<i64> {
        self.dimension(TAG_IMAGE_WIDTH)
    }

    fn px_height(&self) -> Option<i64> {
        self.dimension(TAG_IMAGE_LENGTH)
    }

    /// `_TiffParser._dpi`.
    fn dpi(&self, tag: u16) -> Result<i64> {
        let Some(dots) = self.get(tag) else {
            return Ok(72);
        };
        let unit = self
            .get(TAG_RESOLUTION_UNIT)
            .cloned()
            .unwrap_or(TiffValue::Int(2));
        if unit.equals(1) {
            return Ok(72);
        }
        let inches = unit.equals(2);
        match dots {
            TiffValue::Int(value) if inches => Ok(*value),
            TiffValue::Int(value) => Ok(py_round(*value as f64 * 2.54)),
            TiffValue::Float(value) if inches => Ok(py_round(*value)),
            TiffValue::Float(value) => Ok(py_round(*value * 2.54)),
            TiffValue::Str => Err(err(
                "non_numeric_value",
                "TIFF resolution entry is not numeric",
            )),
        }
    }
}

// ---------------------------------------------------------------- SHA-1

/// Lowercase hex SHA-1 digest (upstream `hashlib.sha1(blob).hexdigest()`).
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(&[b'a'; 1000]),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }

    #[test]
    fn python_rounding_ties_to_even() {
        assert_eq!(py_round(2.5), 2);
        assert_eq!(py_round(3.5), 4);
        assert_eq!(py_round(71.9994), 72);
    }
}
