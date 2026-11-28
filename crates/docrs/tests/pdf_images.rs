//! Image XObjects read back by the test reader: every PNG colour type and
//! depth, JPEG passed through, GIF and BMP decoded, budgets and hostile
//! files. Set `DOCRS_IMAGE_CASES=DIR` to keep each case's PDF next to its
//! source for `tools/pdf/image_check.swift`.

#[allow(dead_code)]
#[path = "support/pdf.rs"]
mod reader;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;

use docrs::ErrorKind;
use docrs::pdf::{Info, PdfLimits, image_document_for_tests};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use proptest::prelude::*;
use reader::{Dict, Pdf, Value};

const PRODUCER: &str = "docrs-test";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("pdf/images").join(name)).unwrap()
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn document_with(
    images: &[(&str, &[u8])],
    limits: &PdfLimits,
) -> docrs::Result<(Vec<u8>, Vec<Vec<&'static str>>)> {
    let info = Info {
        title: Some("Images".into()),
        ..Info::default()
    };
    image_document_for_tests(images, &info, PRODUCER, limits)
}

fn read(data: &[u8]) -> docrs::Result<(Vec<u8>, Vec<&'static str>)> {
    document_with(&[("word/media/image1", data)], &PdfLimits::default())
        .map(|(pdf, mut warnings)| (pdf, warnings.remove(0)))
}

/// A case's PDF, checked by qpdf and kept for the PDFKit comparison.
fn case(name: &str, source: &[u8], extension: &str) -> (Pdf, Vec<&'static str>) {
    let (bytes, warnings) = read(source).unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(bytes, read(source).unwrap().0, "{name}: not deterministic");
    qpdf_check(name, &bytes);
    if let Some(dir) = std::env::var_os("DOCRS_IMAGE_CASES") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.pdf")), &bytes).unwrap();
        std::fs::write(dir.join(format!("{name}.{extension}")), source).unwrap();
    }
    (Pdf::parse(&bytes).unwrap(), warnings)
}

fn qpdf_check(name: &str, bytes: &[u8]) {
    let found = Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        let required = std::env::var_os("CI").is_some() && !cfg!(windows);
        assert!(!required, "qpdf must be installed in CI");
        eprintln!("SKIPPED: qpdf is not installed; `brew install qpdf` to run this check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.pdf"));
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{name}: {report}");
    assert!(
        report.contains("No syntax or stream encoding errors found"),
        "{name}: {report}"
    );
}

/// The image the page draws, and its soft mask.
fn xobject(pdf: &Pdf) -> (&Dict, Option<&Dict>) {
    let page = pdf.pages().unwrap()[0];
    let resources = pdf.page_attr(page, "Resources").unwrap().dict().unwrap();
    let xobjects = pdf.get(resources, "XObject").dict().unwrap();
    assert_eq!(xobjects.len(), 1);
    let image = pdf
        .resolve(xobjects.values().next().unwrap())
        .dict()
        .unwrap();
    let mask = pdf.get(image, "SMask").dict();
    (image, mask)
}

fn stream<'a>(pdf: &'a Pdf, dict: &'a Dict) -> &'a Value {
    pdf.objects
        .values()
        .find(|value| matches!(value, Value::Stream(d, _) if std::ptr::eq(d, dict)))
        .unwrap()
}

fn number(pdf: &Pdf, dict: &Dict, key: &str) -> f64 {
    pdf.get(dict, key).number().unwrap()
}

fn name<'a>(pdf: &'a Pdf, dict: &'a Dict, key: &str) -> &'a str {
    match pdf.get(dict, key) {
        Value::Name(name) => name,
        other => panic!("/{key} is {other:?}"),
    }
}

/// The page's image as RGBA, decoding what the test reader cannot: PNG
/// predictors and indexed colour.
fn rgba(pdf: &Pdf) -> (u32, u32, Vec<[u8; 4]>) {
    let (image, mask) = xobject(pdf);
    let (width, height) = (
        number(pdf, image, "Width") as u32,
        number(pdf, image, "Height") as u32,
    );
    assert_eq!(number(pdf, image, "BitsPerComponent"), 8.0);
    let mut data = pdf.stream_data(stream(pdf, image)).unwrap();
    let (space, palette) = match pdf.get(image, "ColorSpace") {
        Value::Name(name) => (name.clone(), Vec::new()),
        Value::Array(items) => {
            let Value::Str(palette) = pdf.resolve(&items[3]) else {
                panic!("palette {items:?}")
            };
            ("Indexed".to_string(), palette.clone())
        }
        other => panic!("colour space {other:?}"),
    };
    let components = match space.as_str() {
        "DeviceGray" | "Indexed" => 1,
        "DeviceRGB" => 3,
        other => panic!("{other}"),
    };
    if let Some(parms) = pdf.get(image, "DecodeParms").dict() {
        assert_eq!(number(pdf, parms, "Predictor"), 15.0);
        assert_eq!(number(pdf, parms, "Colors"), components as f64);
        assert_eq!(number(pdf, parms, "Columns"), f64::from(width));
        data = unpredict(&data, width as usize * components, components);
    }
    assert_eq!(data.len(), (width * height) as usize * components);
    let alpha = mask.map(|mask| {
        assert_eq!(name(pdf, mask, "ColorSpace"), "DeviceGray");
        assert_eq!(number(pdf, mask, "Width"), f64::from(width));
        pdf.stream_data(stream(pdf, mask)).unwrap()
    });
    let pixels = (0..(width * height) as usize)
        .map(|i| {
            let a = alpha.as_ref().map_or(255, |alpha| alpha[i]);
            match space.as_str() {
                "DeviceGray" => [data[i], data[i], data[i], a],
                "DeviceRGB" => [data[i * 3], data[i * 3 + 1], data[i * 3 + 2], a],
                _ => {
                    let at = usize::from(data[i]) * 3;
                    [palette[at], palette[at + 1], palette[at + 2], a]
                }
            }
        })
        .collect();
    (width, height, pixels)
}

/// PNG row filters undone, written apart from the library's decoder.
fn unpredict(data: &[u8], row: usize, bpp: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for line in data.chunks(row + 1) {
        let start = out.len();
        for i in 0..row {
            let a = if i >= bpp { out[start + i - bpp] } else { 0 };
            let b = if start >= row {
                out[start - row + i]
            } else {
                0
            };
            let c = if start >= row && i >= bpp {
                out[start - row + i - bpp]
            } else {
                0
            };
            let x = line[i + 1];
            out.push(match line[0] {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                _ => x.wrapping_add(paeth(a, b, c)),
            });
        }
    }
    out
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

// PNG encoder.

const GREY: u8 = 0;
const RGB: u8 = 2;
const PALETTE: u8 = 3;
const GREY_ALPHA: u8 = 4;
const RGBA: u8 = 6;

#[derive(Clone)]
struct Png {
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    interlaced: bool,
    /// `channels` samples per pixel, at `depth` bits.
    samples: Vec<u16>,
    palette: Vec<u8>,
    transparency: Option<Vec<u8>>,
    /// Chunks before the image data, and after it.
    extra: Vec<([u8; 4], Vec<u8>)>,
    after: Vec<([u8; 4], Vec<u8>)>,
}

fn channels(color: u8) -> usize {
    match color {
        RGB => 3,
        GREY_ALPHA => 2,
        RGBA => 4,
        _ => 1,
    }
}

impl Png {
    /// A pattern touching every sample value at `depth` bits.
    fn pattern(width: u32, height: u32, depth: u8, color: u8, interlaced: bool) -> Png {
        let max = (1u32 << depth) - 1;
        let channels = channels(color);
        let entries = if color == PALETTE {
            1u32 << depth.min(4)
        } else {
            0
        };
        let samples = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                (0..channels as u32).map(move |c| {
                    let value = x * 37 + y * 101 + c * 59 + x * y * 3;
                    if color == PALETTE {
                        (value % entries) as u16
                    } else if (color == GREY_ALPHA && c == 1) || (color == RGBA && c == 3) {
                        // Opaque, clear and in between.
                        [max, 0, max / 2, max / 3][((x + y) % 4) as usize] as u16
                    } else {
                        (value.wrapping_mul(2_654_435_761) % (max + 1)) as u16
                    }
                })
            })
            .collect();
        let palette = (0..entries)
            .flat_map(|i| [(i * 70) as u8, (255 - i * 15) as u8, (i * 31 % 256) as u8])
            .collect();
        Png {
            width,
            height,
            depth,
            color,
            interlaced,
            samples,
            palette,
            transparency: None,
            extra: Vec::new(),
            after: Vec::new(),
        }
    }

    fn sample(&self, x: u32, y: u32, c: usize) -> u16 {
        self.samples[(y * self.width + x) as usize * channels(self.color) + c]
    }

    fn to_byte(&self, value: u16) -> u8 {
        match self.depth {
            16 => (value >> 8) as u8,
            8 => value as u8,
            depth => (u32::from(value) * 255 / ((1 << depth) - 1)) as u8,
        }
    }

    /// What each pixel should look like, 8 bits per channel.
    fn expected(&self) -> Vec<[u8; 4]> {
        let key = |n: usize| -> Option<Vec<u16>> {
            self.transparency
                .as_ref()
                .filter(|_| self.color != PALETTE)
                .map(|t| {
                    (0..n)
                        .map(|i| u16::from_be_bytes([t[i * 2], t[i * 2 + 1]]))
                        .collect()
                })
        };
        let pixel = |x: u32, y: u32| -> [u8; 4] {
            let s = |c| self.sample(x, y, c);
            let b = |c| self.to_byte(s(c));
            match self.color {
                GREY => {
                    let clear = key(1).is_some_and(|k| k[0] == s(0));
                    [b(0), b(0), b(0), if clear { 0 } else { 255 }]
                }
                RGB => {
                    let clear = key(3).is_some_and(|k| k == [s(0), s(1), s(2)]);
                    [b(0), b(1), b(2), if clear { 0 } else { 255 }]
                }
                PALETTE => {
                    let i = usize::from(s(0));
                    let alpha = self.transparency.as_ref().and_then(|t| t.get(i).copied());
                    [
                        self.palette[i * 3],
                        self.palette[i * 3 + 1],
                        self.palette[i * 3 + 2],
                        alpha.unwrap_or(255),
                    ]
                }
                GREY_ALPHA => [b(0), b(0), b(0), b(1)],
                _ => [b(0), b(1), b(2), b(3)],
            }
        };
        (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| pixel(x, y)))
            .collect()
    }

    fn encode(&self) -> Vec<u8> {
        self.encode_with(&self.compressed())
    }

    /// The file around `compressed` image data.
    fn encode_with(&self, compressed: &[u8]) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&self.width.to_be_bytes());
        ihdr.extend_from_slice(&self.height.to_be_bytes());
        ihdr.extend_from_slice(&[self.depth, self.color, 0, 0, u8::from(self.interlaced)]);
        let mut out = b"\x89PNG\r\n\x1A\n".to_vec();
        chunk(&mut out, b"IHDR", &ihdr);
        for (kind, body) in &self.extra {
            chunk(&mut out, kind, body);
        }
        if !self.palette.is_empty() {
            chunk(&mut out, b"PLTE", &self.palette);
        }
        if let Some(t) = &self.transparency {
            chunk(&mut out, b"tRNS", t);
        }
        let (first, second) = compressed.split_at(compressed.len() / 2);
        chunk(&mut out, b"IDAT", first);
        chunk(&mut out, b"IDAT", second);
        for (kind, body) in &self.after {
            chunk(&mut out, kind, body);
        }
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn compressed(&self) -> Vec<u8> {
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&self.filtered()).unwrap();
        z.finish().unwrap()
    }

    /// The image data before compression, every row filtered, the filters
    /// taken in turn.
    fn filtered(&self) -> Vec<u8> {
        let mut raw = Vec::new();
        let passes: &[(u32, u32, u32, u32)] = if self.interlaced {
            &[
                (0, 0, 8, 8),
                (4, 0, 8, 8),
                (0, 4, 4, 8),
                (2, 0, 4, 4),
                (0, 2, 2, 4),
                (1, 0, 2, 2),
                (0, 1, 1, 2),
            ]
        } else {
            &[(0, 0, 1, 1)]
        };
        let channels = channels(self.color);
        let bpp = (channels * usize::from(self.depth)).div_ceil(8);
        let mut row_number = 0;
        for &(x0, y0, dx, dy) in passes {
            let mut previous: Vec<u8> = Vec::new();
            for y in (y0..self.height).step_by(dy as usize) {
                let xs: Vec<u32> = (x0..self.width).step_by(dx as usize).collect();
                if xs.is_empty() {
                    continue;
                }
                let mut bits = BitPacker::default();
                for &x in &xs {
                    for c in 0..channels {
                        bits.push(self.sample(x, y, c), self.depth);
                    }
                }
                let line = bits.finish();
                let filter = (row_number % 5) as u8;
                row_number += 1;
                raw.push(filter);
                for i in 0..line.len() {
                    let a = if i >= bpp { line[i - bpp] } else { 0 };
                    let b = previous.get(i).copied().unwrap_or(0);
                    let c = if i >= bpp {
                        previous.get(i - bpp).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    let predicted = match filter {
                        0 => 0,
                        1 => a,
                        2 => b,
                        3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                        _ => paeth(a, b, c),
                    };
                    raw.push(line[i].wrapping_sub(predicted));
                }
                previous = line;
            }
        }
        raw
    }
}

#[derive(Default)]
struct BitPacker {
    out: Vec<u8>,
    bits: usize,
}

impl BitPacker {
    fn push(&mut self, value: u16, depth: u8) {
        match depth {
            16 => self.out.extend_from_slice(&value.to_be_bytes()),
            8 => self.out.push(value as u8),
            _ => {
                if self.bits.is_multiple_of(8) {
                    self.out.push(0);
                }
                let shift = 8 - usize::from(depth) - self.bits % 8;
                *self.out.last_mut().unwrap() |= (value as u8) << shift;
                self.bits += usize::from(depth);
            }
        }
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(body);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

fn assert_pixels(name: &str, pdf: &Pdf, width: u32, height: u32, expected: &[[u8; 4]]) {
    let (w, h, pixels) = rgba(pdf);
    assert_eq!((w, h), (width, height), "{name}");
    for (i, (got, want)) in pixels.iter().zip(expected).enumerate() {
        // A clear pixel's colour does not show.
        if want[3] == 0 && got[3] == 0 {
            continue;
        }
        assert_eq!(got, want, "{name}: pixel {i}");
    }
}

#[test]
fn every_png_colour_type_and_depth_decodes_or_passes_through() {
    let kinds: &[(u8, &[u8])] = &[
        (GREY, &[1, 2, 4, 8, 16]),
        (RGB, &[8, 16]),
        (PALETTE, &[1, 2, 4, 8]),
        (GREY_ALPHA, &[8, 16]),
        (RGBA, &[8, 16]),
    ];
    for &(color, depths) in kinds {
        for &depth in depths {
            for interlaced in [false, true] {
                let png = Png::pattern(13, 11, depth, color, interlaced);
                let name = format!(
                    "png-c{color}-d{depth}{}",
                    if interlaced { "-i" } else { "" }
                );
                let (pdf, warnings) = case(&name, &png.encode(), "png");
                assert!(warnings.is_empty(), "{name}: {warnings:?}");
                let (image, mask) = xobject(&pdf);
                let passed = depth == 8 && matches!(color, GREY | RGB | PALETTE) && !interlaced;
                assert_eq!(
                    pdf.get(image, "DecodeParms").dict().is_some(),
                    passed,
                    "{name}"
                );
                let alpha = matches!(color, GREY_ALPHA | RGBA);
                assert_eq!(mask.is_some(), alpha, "{name}");
                assert_pixels(&name, &pdf, 13, 11, &png.expected());
            }
        }
    }
}

#[test]
fn a_passed_through_png_keeps_its_compressed_data() {
    let png = Png::pattern(16, 9, 8, RGB, false);
    let file = png.encode();
    let (pdf, _) = case("png-passthrough", &file, "png");
    let (image, _) = xobject(&pdf);
    let (_, raw) = pdf.raw_stream(stream(&pdf, image)).unwrap();
    let mut inflated = Vec::new();
    ZlibDecoder::new(raw).read_to_end(&mut inflated).unwrap();
    assert_eq!(name(&pdf, image, "Filter"), "FlateDecode");
    assert_eq!(inflated.len(), 9 * (16 * 3 + 1));
    let idat: Vec<u8> = file
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"IDAT")
        .flat_map(|(at, _)| {
            let len = u32::from_be_bytes(file[at - 4..at].try_into().unwrap()) as usize;
            file[at + 4..at + 4 + len].to_vec()
        })
        .collect();
    assert_eq!(raw, idat.as_slice());
}

#[test]
fn png_transparency_becomes_a_soft_mask() {
    let mut palette = Png::pattern(12, 8, 4, PALETTE, false);
    palette.transparency = Some(vec![0, 128, 255, 40]);
    let mut grey = Png::pattern(12, 8, 8, GREY, false);
    grey.transparency = Some(vec![0, grey.samples[5] as u8]);
    let mut rgb = Png::pattern(12, 8, 16, RGB, true);
    let key: Vec<u8> = (0..3)
        .flat_map(|c| rgb.sample(2, 1, c).to_be_bytes())
        .collect();
    rgb.transparency = Some(key);
    for (name, png) in [
        ("png-palette-trns", palette),
        ("png-grey-trns", grey),
        ("png-rgb16-trns", rgb),
    ] {
        let (pdf, _) = case(name, &png.encode(), "png");
        let (image, mask) = xobject(&pdf);
        assert!(mask.is_some(), "{name}");
        assert!(pdf.get(image, "DecodeParms").dict().is_none(), "{name}");
        let expected = png.expected();
        assert!(expected.iter().any(|p| p[3] == 0), "{name}");
        assert_pixels(name, &pdf, png.width, png.height, &expected);
    }
}

#[test]
fn an_rgba_png_with_every_pixel_opaque_has_no_mask() {
    let mut png = Png::pattern(8, 8, 8, RGBA, false);
    for pixel in png.samples.chunks_mut(4) {
        pixel[3] = 255;
    }
    let (pdf, _) = case("png-opaque-rgba", &png.encode(), "png");
    assert!(xobject(&pdf).1.is_none());
    assert_pixels("png-opaque-rgba", &pdf, 8, 8, &png.expected());
}

#[test]
fn a_transparent_logo_from_imageio_keeps_its_edges() {
    let (pdf, warnings) = case("logo", &fixture("logo.png"), "png");
    assert!(warnings.is_empty());
    let (width, height, pixels) = rgba(&pdf);
    assert_eq!((width, height), (48, 32));
    let alphas: Vec<u8> = pixels.iter().map(|p| p[3]).collect();
    assert!(alphas.contains(&0) && alphas.contains(&255));
    assert!(
        alphas.iter().any(|&a| a > 0 && a < 255),
        "anti-aliased edges"
    );
}

#[test]
fn a_png_profile_or_animation_is_reported() {
    let mut png = Png::pattern(8, 8, 8, RGB, false);
    png.extra
        .push((*b"iCCP", b"sRGB\0\0x\x9c\x03\0\0\0\0\x01".to_vec()));
    let (_, warnings) = case("png-profiled", &png.encode(), "png");
    assert_eq!(warnings, ["pdf_image_profile_ignored"]);
    let mut png = Png::pattern(8, 8, 8, RGB, false);
    let frame = |sequence: u32| {
        let mut body = sequence.to_be_bytes().to_vec();
        for value in [8u32, 8, 0, 0] {
            body.extend_from_slice(&value.to_be_bytes());
        }
        body.extend_from_slice(&[0, 1, 0, 2, 0, 0]);
        (*b"fcTL", body)
    };
    png.extra = vec![(*b"acTL", vec![0, 0, 0, 2, 0, 0, 0, 0]), frame(0)];
    let mut second = 2u32.to_be_bytes().to_vec();
    second.extend(Png::pattern(8, 8, 8, RGB, true).compressed());
    png.after = vec![frame(1), (*b"fdAT", second)];
    let (pdf, warnings) = case("png-animated", &png.encode(), "png");
    assert_eq!(warnings, ["pdf_image_animated"]);
    assert_pixels("png-animated", &pdf, 8, 8, &png.expected());
}

fn malformed(data: &[u8]) -> String {
    let err = read(data).unwrap_err();
    assert_eq!(err.reason(), "pdf_image_malformed", "{err}");
    err.to_string()
}

fn unsupported(data: &[u8]) -> String {
    let err = read(data).unwrap_err();
    assert_eq!(err.reason(), "pdf_image_unsupported", "{err}");
    err.to_string()
}

#[test]
fn a_damaged_png_is_a_typed_error() {
    let file = Png::pattern(10, 10, 8, RGBA, true).encode();
    let mut bad_crc = file.clone();
    bad_crc[30] ^= 1;
    assert!(malformed(&bad_crc).contains("CRC"));
    let mut bad_filter = Png::pattern(4, 2, 8, GREY, false);
    bad_filter.depth = 8;
    let mut raw = vec![7, 1, 2, 3, 4, 0, 1, 2, 3, 4];
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&raw).unwrap();
    let mut file = b"\x89PNG\r\n\x1A\n".to_vec();
    chunk(&mut file, b"IHDR", &[0, 0, 0, 4, 0, 0, 0, 2, 8, 0, 0, 0, 0]);
    chunk(&mut file, b"IDAT", &z.finish().unwrap());
    chunk(&mut file, b"IEND", &[]);
    assert!(malformed(&file).contains("unknown PNG filter"));
    raw.truncate(5);
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&raw[1..]).unwrap();
    let mut short = b"\x89PNG\r\n\x1A\n".to_vec();
    chunk(
        &mut short,
        b"IHDR",
        &[0, 0, 0, 4, 0, 0, 0, 2, 8, 0, 0, 0, 0],
    );
    chunk(&mut short, b"IDAT", &z.finish().unwrap());
    chunk(&mut short, b"IEND", &[]);
    assert!(malformed(&short).contains("ends early"));
    let mut bad_depth = b"\x89PNG\r\n\x1A\n".to_vec();
    chunk(
        &mut bad_depth,
        b"IHDR",
        &[0, 0, 0, 4, 0, 0, 0, 2, 4, 2, 0, 0, 0],
    );
    assert!(malformed(&bad_depth).contains("header"));
    let mut no_palette = Png::pattern(4, 4, 8, PALETTE, false);
    no_palette.palette.clear();
    assert!(malformed(&no_palette.encode()).contains("palette"));
    let mut outside = Png::pattern(4, 4, 4, PALETTE, false);
    outside.palette.truncate(6);
    outside.transparency = Some(vec![0]);
    assert!(malformed(&outside.encode()).contains("outside its palette"));
}

#[test]
fn truncated_files_are_typed_errors_at_every_length() {
    let files = [
        Png::pattern(9, 7, 2, GREY, true).encode(),
        Png::pattern(9, 7, 8, RGB, false).encode(),
        fixture("photo-rgb.jpg"),
        fixture("animated.gif"),
        gif(&Gif::pattern(9, 7, true)),
        bmp(&Bmp::pattern(9, 7, 8, false)),
    ];
    for file in files {
        for len in 0..file.len() {
            match read(&file[..len]) {
                Ok(_) => {}
                Err(err) => assert!(matches!(err.kind(), ErrorKind::Refusal(_)), "{len}: {err}"),
            }
        }
    }
}

#[test]
fn huge_dimensions_hit_the_budget_before_any_allocation() {
    let mut png = b"\x89PNG\r\n\x1A\n".to_vec();
    chunk(
        &mut png,
        b"IHDR",
        &[
            0x7F, 0xFF, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 8, 6, 0, 0, 0,
        ],
    );
    let mut jpeg = fixture("photo-rgb.jpg");
    let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    jpeg[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    let mut gif = b"GIF89a\xFF\xFF\xFF\xFF\x80\x00\x00\0\0\0\xFF\xFF\xFF".to_vec();
    gif.extend_from_slice(b"\x2C\0\0\0\0\x01\0\x01\0\0\x02\x02\x4C\x01\0\x3B");
    let mut bmp = bmp(&Bmp::pattern(2, 2, 24, false));
    bmp[18..26].copy_from_slice(&[0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    for file in [png, jpeg, gif, bmp] {
        let err = read(&file).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ResourceLimit, "{err}");
        assert_eq!(err.reason(), "max_image_pixels");
    }
}

#[test]
fn a_hostile_header_with_trusted_limits_fails_on_its_data() {
    let mut png = b"\x89PNG\r\n\x1A\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 1, 0, 0, 0, 1, 0, 0, 8, 6, 0, 0, 0]);
    chunk(&mut png, b"IDAT", &[0x78, 0x9C, 0x03, 0, 0, 0, 0, 1]);
    chunk(&mut png, b"IEND", &[]);
    let mut gif = b"GIF89a\xFF\xFF\xFF\xFF\x80\x00\x00\0\0\0\xFF\xFF\xFF".to_vec();
    gif.extend_from_slice(b"\x2C\0\0\0\0\xFF\xFF\xFF\xFF\0\x02\x02\x4C\x01\0\x3B");
    for file in [png, gif] {
        let err = document_with(&[("word/media/x", &file)], &PdfLimits::trusted()).unwrap_err();
        assert_eq!(err.reason(), "pdf_image_malformed", "{err}");
    }
}

#[test]
fn each_part_is_one_xobject_and_the_document_budget_spans_them() {
    let first = Png::pattern(10, 10, 8, RGB, false).encode();
    let second = Png::pattern(10, 10, 8, GREY, false).encode();
    let images: [(&str, &[u8]); 3] = [
        ("word/media/a.png", &first),
        ("word/media/b.png", &second),
        ("word/media/a.png", &first),
    ];
    let (bytes, _) = document_with(&images, &PdfLimits::default()).unwrap();
    let pdf = Pdf::parse(&bytes).unwrap();
    let images = pdf
        .objects
        .values()
        .filter(|v| matches!(v.dict().map(|d| pdf.get(d, "Subtype")), Some(Value::Name(n)) if n == "Image"))
        .count();
    assert_eq!(images, 2);
    assert_eq!(pdf.pages().unwrap().len(), 3);
    let mut limits = PdfLimits::default();
    limits.max_document_pixels = 150;
    let err = document_with(&images_of(&first, &second), &limits).unwrap_err();
    assert_eq!(err.reason(), "max_document_pixels");
    assert!(document_with(&images_of(&first, &first)[..1], &limits).is_ok());
}

fn images_of<'a>(first: &'a [u8], second: &'a [u8]) -> [(&'static str, &'a [u8]); 2] {
    [("word/media/a.png", first), ("word/media/b.png", second)]
}

// JPEG.

#[test]
fn grey_rgb_and_progressive_jpegs_pass_through() {
    for (photo, space) in [
        ("photo-grey", "DeviceGray"),
        ("photo-rgb", "DeviceRGB"),
        ("photo-progressive", "DeviceRGB"),
    ] {
        let file = fixture(&format!("{photo}.jpg"));
        let (pdf, warnings) = case(&format!("jpeg-{photo}"), &file, "jpg");
        assert!(warnings.is_empty(), "{photo}: {warnings:?}");
        let (image, mask) = xobject(&pdf);
        assert!(mask.is_none());
        assert_eq!(name_of(&pdf, image), space);
        assert_eq!(name(&pdf, image, "Filter"), "DCTDecode");
        assert_eq!(
            (number(&pdf, image, "Width"), number(&pdf, image, "Height")),
            (48.0, 32.0)
        );
        assert_eq!(
            pdf.raw_stream(stream(&pdf, image)).unwrap().1,
            file.as_slice()
        );
        assert_eq!(pdf.get(image, "Decode"), &Value::Null);
    }
}

fn name_of<'a>(pdf: &'a Pdf, image: &'a Dict) -> &'a str {
    name(pdf, image, "ColorSpace")
}

#[test]
fn an_adobe_cmyk_jpeg_is_drawn_with_inverted_decode() {
    let (pdf, warnings) = case("jpeg-photo-cmyk", &fixture("photo-cmyk.jpg"), "jpg");
    assert!(warnings.is_empty());
    let (image, _) = xobject(&pdf);
    assert_eq!(name_of(&pdf, image), "DeviceCMYK");
    let decode: Vec<f64> = match pdf.get(image, "Decode") {
        Value::Array(items) => items.iter().map(|v| v.number().unwrap()).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(decode, [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
}

#[test]
fn a_profiled_jpeg_warns_and_is_drawn_without_its_profile() {
    let (pdf, warnings) = case("jpeg-photo-p3", &fixture("photo-p3.jpg"), "jpg");
    assert_eq!(warnings, ["pdf_image_profile_ignored"]);
    assert_eq!(name_of(&pdf, xobject(&pdf).0), "DeviceRGB");
}

#[test]
fn jpegs_the_engine_does_not_draw_are_refused_as_unsupported() {
    let cmyk = fixture("photo-cmyk.jpg");
    let adobe = cmyk.windows(5).position(|w| w == b"Adobe").unwrap();
    let mut ycck = cmyk.clone();
    ycck[adobe + 11] = 2;
    assert!(unsupported(&ycck).contains("YCCK"));
    let mut plain_cmyk = cmyk.clone();
    plain_cmyk[adobe] = b'X';
    assert!(unsupported(&plain_cmyk).contains("YCCK"));
    let rgb = fixture("photo-rgb.jpg");
    let sof = rgb.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let mut twelve = rgb.clone();
    twelve[sof + 4] = 12;
    assert!(unsupported(&twelve).contains("12-bit"));
    let mut arithmetic = rgb.clone();
    arithmetic[sof + 1] = 0xC9;
    assert!(unsupported(&arithmetic).contains("arithmetic"));
    let mut no_height = rgb.clone();
    no_height[sof + 5..sof + 7].copy_from_slice(&[0, 0]);
    assert!(unsupported(&no_height).contains("height"));
    let mut no_frame = rgb[..2].to_vec();
    no_frame.extend_from_slice(&rgb[rgb.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap()..]);
    assert!(malformed(&no_frame).contains("frame"));
}

// GIF encoder.

struct Gif {
    width: u16,
    height: u16,
    palette: Vec<u8>,
    /// Frame left, top, width, height.
    frame: (u16, u16, u16, u16),
    indices: Vec<u8>,
    transparent: Option<u8>,
    interlaced: bool,
    local: bool,
    frames: usize,
}

impl Gif {
    fn pattern(width: u16, height: u16, interlaced: bool) -> Gif {
        let indices = (0..u32::from(width) * u32::from(height))
            .map(|i| ((i * 7 + i / u32::from(width) * 3) % 16) as u8)
            .collect();
        Gif {
            width,
            height,
            palette: (0..16u8)
                .flat_map(|i| [i * 16, 255 - i * 16, i * 9])
                .collect(),
            frame: (0, 0, width, height),
            indices,
            transparent: None,
            interlaced,
            local: false,
            frames: 1,
        }
    }

    fn expected(&self) -> Vec<[u8; 4]> {
        let (left, top, fw, fh) = self.frame;
        let mut out = vec![[0, 0, 0, 0]; usize::from(self.width) * usize::from(self.height)];
        for y in 0..fh {
            for x in 0..fw {
                let index = self.indices[usize::from(y) * usize::from(fw) + usize::from(x)];
                if Some(index) == self.transparent {
                    continue;
                }
                let at = usize::from(index) * 3;
                let p = &self.palette[at..at + 3];
                out[usize::from(top + y) * usize::from(self.width) + usize::from(left + x)] =
                    [p[0], p[1], p[2], 255];
            }
        }
        out
    }
}

fn gif(spec: &Gif) -> Vec<u8> {
    let size_bits = (spec.palette.len() / 3)
        .next_power_of_two()
        .trailing_zeros()
        .max(1) as u8
        - 1;
    let mut table = spec.palette.clone();
    table.resize(3 << (size_bits + 1), 0);
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&spec.width.to_le_bytes());
    out.extend_from_slice(&spec.height.to_le_bytes());
    out.extend_from_slice(&[if spec.local { 0 } else { 0x80 | size_bits }, 0, 0]);
    if !spec.local {
        out.extend_from_slice(&table);
    }
    out.extend_from_slice(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\0\0\0");
    let (left, top, fw, fh) = spec.frame;
    let rows: Vec<u16> = if spec.interlaced {
        [(0, 8), (4, 8), (2, 4), (1, 2)]
            .iter()
            .flat_map(|&(s, step)| (s..fh).step_by(step))
            .collect()
    } else {
        (0..fh).collect()
    };
    let stored: Vec<u8> = rows
        .iter()
        .flat_map(|&y| spec.indices[usize::from(y) * usize::from(fw)..][..usize::from(fw)].to_vec())
        .collect();
    for _ in 0..spec.frames {
        if let Some(t) = spec.transparent {
            out.extend_from_slice(&[0x21, 0xF9, 4, 1, 10, 0, t, 0]);
        }
        out.push(0x2C);
        for value in [left, top, fw, fh] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        let mut flags = if spec.interlaced { 0x40 } else { 0 };
        if spec.local {
            flags |= 0x80 | size_bits;
        }
        out.push(flags);
        if spec.local {
            out.extend_from_slice(&table);
        }
        let largest = u32::from(stored.iter().copied().max().unwrap_or(0));
        let min = (size_bits + 1)
            .max(2)
            .max((32 - largest.leading_zeros()) as u8);
        out.push(min);
        for block in lzw(&stored, min).chunks(255) {
            out.push(block.len() as u8);
            out.extend_from_slice(block);
        }
        out.push(0);
    }
    out.push(0x3B);
    out
}

/// GIF's LZW, written apart from the library's decoder.
fn lzw(indices: &[u8], min: u8) -> Vec<u8> {
    let clear = 1u16 << min;
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut put = |code: u16, width: u8, out: &mut Vec<u8>| {
        acc |= u32::from(code) << bits;
        bits += u32::from(width);
        while bits >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            bits -= 8;
        }
    };
    let mut table: HashMap<Vec<u8>, u16> = HashMap::new();
    let reset = |table: &mut HashMap<Vec<u8>, u16>| {
        table.clear();
        for i in 0..clear {
            table.insert(vec![i as u8], i);
        }
    };
    reset(&mut table);
    let (mut next, mut width) = (clear + 2, min + 1);
    put(clear, width, &mut out);
    let mut current: Vec<u8> = Vec::new();
    for &index in indices {
        let mut longer = current.clone();
        longer.push(index);
        if table.contains_key(&longer) {
            current = longer;
            continue;
        }
        put(table[&current], width, &mut out);
        if next == 4096 {
            put(clear, width, &mut out);
            reset(&mut table);
            (next, width) = (clear + 2, min + 1);
        } else {
            table.insert(longer, next);
            if next == 1 << width && width < 12 {
                width += 1;
            }
            next += 1;
        }
        current = vec![index];
    }
    if !current.is_empty() {
        put(table[&current], width, &mut out);
    }
    put(clear + 1, width, &mut out);
    if bits > 0 {
        out.push(acc as u8);
    }
    out
}

#[test]
fn gifs_decode_with_their_palette_interlace_and_transparency() {
    let mut transparent = Gif::pattern(20, 12, false);
    transparent.transparent = Some(3);
    let mut local = Gif::pattern(20, 12, true);
    local.local = true;
    let mut framed = Gif::pattern(20, 12, false);
    framed.frame = (4, 3, 10, 6);
    framed.indices.truncate(60);
    let mut noisy = Gif::pattern(200, 150, false);
    noisy.palette = (0..=255u8)
        .flat_map(|i| [i, i.wrapping_mul(7), 255 - i])
        .collect();
    noisy.indices = (0..200u32 * 150)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    for (name, spec) in [
        ("gif-plain", Gif::pattern(20, 12, false)),
        ("gif-interlaced-local", local),
        ("gif-transparent", transparent),
        ("gif-framed", framed),
        ("gif-noisy", noisy),
    ] {
        let (pdf, warnings) = case(name, &gif(&spec), "gif");
        assert!(warnings.is_empty(), "{name}");
        assert_pixels(
            name,
            &pdf,
            u32::from(spec.width),
            u32::from(spec.height),
            &spec.expected(),
        );
    }
}

#[test]
fn an_animated_gif_is_its_first_frame_with_a_warning() {
    let mut spec = Gif::pattern(16, 8, false);
    spec.frames = 3;
    let (pdf, warnings) = case("gif-three-frames", &gif(&spec), "gif");
    assert_eq!(warnings, ["pdf_image_animated"]);
    assert_pixels("gif-three-frames", &pdf, 16, 8, &spec.expected());
    let (pdf, warnings) = case("gif-animated", &fixture("animated.gif"), "gif");
    assert_eq!(warnings, ["pdf_image_animated"]);
    let (width, height, pixels) = rgba(&pdf);
    assert_eq!((width, height), (48, 32));
    // ImageIO's first frame: a red square from x 4 to 19, rows 8 to 23
    // counted from the bottom.
    let at = |x: u32, y: u32| pixels[(y * width + x) as usize];
    assert!(
        at(10, 15)[0] > 200 && at(10, 15)[1] < 80,
        "{:?}",
        at(10, 15)
    );
    assert!(
        at(30, 15)[3] == 0 || at(30, 15)[0] < 200,
        "{:?}",
        at(30, 15)
    );
}

#[test]
fn a_damaged_gif_is_a_typed_error() {
    let mut outside = Gif::pattern(4, 4, false);
    outside.palette.truncate(6);
    assert!(malformed(&gif(&outside)).contains("outside its colour table"));
    let mut bad_frame = Gif::pattern(8, 8, false);
    bad_frame.frame = (4, 4, 8, 8);
    assert!(malformed(&gif(&bad_frame)).contains("outside the image"));
    let mut file = gif(&Gif::pattern(8, 8, false));
    let data = file.iter().rposition(|&b| b == 0x2C).unwrap() + 10;
    file[data] = 1;
    assert!(malformed(&file).contains("code size"));
}

// BMP encoder.

struct Bmp {
    width: i32,
    height: i32,
    bits: u16,
    top_down: bool,
    rgb: Vec<[u8; 3]>,
    alpha: Option<Vec<u8>>,
    palette: Vec<[u8; 3]>,
    indices: Vec<u8>,
}

impl Bmp {
    fn pattern(width: i32, height: i32, bits: u16, top_down: bool) -> Bmp {
        let count = (width * height) as usize;
        let rgb: Vec<[u8; 3]> = (0..count)
            .map(|i| {
                [
                    (i * 13) as u8,
                    (i * 29 + 7) as u8,
                    (255 - i * 3 % 256) as u8,
                ]
            })
            .collect();
        let entries = if bits <= 8 { 1usize << bits.min(4) } else { 0 };
        let palette: Vec<[u8; 3]> = (0..entries)
            .map(|i| [(i * 50) as u8, (i * 17) as u8, (200 - i * 9) as u8])
            .collect();
        let indices = (0..count)
            .map(|i| ((i * 5 + i / 3) % entries.max(1)) as u8)
            .collect();
        Bmp {
            width,
            height,
            bits,
            top_down,
            rgb,
            alpha: None,
            palette,
            indices,
        }
    }

    fn expected(&self) -> Vec<[u8; 4]> {
        (0..(self.width * self.height) as usize)
            .map(|i| {
                let [r, g, b] = if self.bits <= 8 {
                    self.palette[usize::from(self.indices[i])]
                } else {
                    self.rgb[i]
                };
                [r, g, b, self.alpha.as_ref().map_or(255, |a| a[i])]
            })
            .collect()
    }
}

/// A BMP with a 40-byte header, or a 124-byte one with bit fields when it
/// has alpha.
fn bmp(spec: &Bmp) -> Vec<u8> {
    let stride = (spec.width as usize * usize::from(spec.bits)).div_ceil(32) * 4;
    let header: u32 = if spec.alpha.is_some() { 124 } else { 40 };
    let palette_bytes = spec.palette.len() * 4;
    let offset = 14 + header as usize + palette_bytes;
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&((offset + stride * spec.height as usize) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(&spec.width.to_le_bytes());
    let height = if spec.top_down {
        -spec.height
    } else {
        spec.height
    };
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&spec.bits.to_le_bytes());
    let compression: u32 = if spec.alpha.is_some() { 3 } else { 0 };
    out.extend_from_slice(&compression.to_le_bytes());
    out.extend_from_slice(&[0; 12]);
    out.extend_from_slice(&(spec.palette.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    if spec.alpha.is_some() {
        for mask in [0x00FF_0000u32, 0xFF00, 0xFF, 0xFF00_0000] {
            out.extend_from_slice(&mask.to_le_bytes());
        }
        out.resize(14 + 124, 0);
    }
    for [r, g, b] in &spec.palette {
        out.extend_from_slice(&[*b, *g, *r, 0]);
    }
    for stored in 0..spec.height {
        let y = if spec.top_down {
            stored
        } else {
            spec.height - 1 - stored
        };
        let mut row = BitPacker::default();
        for x in 0..spec.width {
            let i = (y * spec.width + x) as usize;
            match spec.bits {
                24 | 32 => {
                    let [r, g, b] = spec.rgb[i];
                    row.out.extend_from_slice(&[b, g, r]);
                    if spec.bits == 32 {
                        row.out.push(spec.alpha.as_ref().map_or(0, |a| a[i]));
                    }
                }
                bits => row.push(u16::from(spec.indices[i]), bits as u8),
            }
        }
        let mut row = row.finish();
        row.resize(stride, 0);
        out.extend_from_slice(&row);
    }
    out
}

#[test]
fn bmps_decode_bottom_up_and_top_down_at_every_depth() {
    let mut alpha = Bmp::pattern(9, 5, 32, true);
    alpha.alpha = Some((0..45).map(|i| (i * 40 % 256) as u8).collect());
    let cases = [
        ("bmp-24", Bmp::pattern(9, 5, 24, false)),
        ("bmp-24-top-down", Bmp::pattern(9, 5, 24, true)),
        ("bmp-8", Bmp::pattern(9, 5, 8, false)),
        ("bmp-4", Bmp::pattern(9, 5, 4, false)),
        ("bmp-1", Bmp::pattern(9, 5, 1, true)),
        ("bmp-32", Bmp::pattern(9, 5, 32, false)),
        ("bmp-32-alpha", alpha),
    ];
    for (name, spec) in cases {
        let (pdf, warnings) = case(name, &bmp(&spec), "bmp");
        assert!(warnings.is_empty());
        assert_eq!(xobject(&pdf).1.is_some(), spec.alpha.is_some(), "{name}");
        assert_pixels(
            name,
            &pdf,
            spec.width as u32,
            spec.height as u32,
            &spec.expected(),
        );
    }
}

#[test]
fn compressed_and_16_bit_bmps_are_unsupported() {
    let mut rle = bmp(&Bmp::pattern(4, 4, 8, false));
    rle[30] = 1;
    assert!(unsupported(&rle).contains("run-length"));
    let mut sixteen = bmp(&Bmp::pattern(4, 4, 24, false));
    sixteen[28] = 16;
    assert!(unsupported(&sixteen).contains("16-bit"));
    let mut png_inside = bmp(&Bmp::pattern(4, 4, 24, false));
    png_inside[30] = 5;
    assert!(unsupported(&png_inside).contains("compressed"));
    let mut outside = Bmp::pattern(4, 4, 8, false);
    outside.indices[3] = 200;
    assert!(malformed(&bmp(&outside)).contains("outside its palette"));
}

/// A 1 × 1 GIF whose LZW data fills the table with KwKwK codes, then repeats
/// the longest code for 4 MiB: each code would walk a 4,000-link chain.
fn lzw_chain_bomb() -> Vec<u8> {
    let mut codes = Vec::new();
    let mut put = BitWriter::default();
    let (mut next, mut width) = (6u32, 3u32);
    put.code(0, width);
    while next < 4096 {
        put.code(next, width);
        next += 1;
        if next == 1 << width && width < 12 {
            width += 1;
        }
    }
    while put.out.len() < 4 << 20 {
        put.code(4095, width);
    }
    codes.extend(put.out);
    let mut out = b"GIF89a\x01\0\x01\0\x81\0\0".to_vec();
    out.extend_from_slice(&[0, 0, 0, 255, 255, 255, 1, 2, 3, 4, 5, 6]);
    out.extend_from_slice(b"\x2C\0\0\0\0\x01\0\x01\0\0\x02");
    for block in codes.chunks(255) {
        out.push(block.len() as u8);
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&[0, 0x3B]);
    out
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    bits: u32,
}

impl BitWriter {
    fn code(&mut self, code: u32, width: u32) {
        self.acc |= code << self.bits;
        self.bits += width;
        while self.bits >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.bits -= 8;
        }
    }
}

#[test]
fn gif_decoding_stops_at_the_frame_last_pixel() {
    let bomb = lzw_chain_bomb();
    assert!(bomb.len() > 4 << 20);
    let start = std::time::Instant::now();
    let (pdf, _) = case("gif-lzw-bomb", &bomb, "gif");
    assert!(start.elapsed().as_secs_f64() < 2.0, "{:?}", start.elapsed());
    let (width, height, pixels) = rgba(&pdf);
    assert_eq!((width, height, pixels), (1, 1, vec![[0, 0, 0, 255]]));
}

#[test]
fn bit_field_masks_must_be_separate_runs() {
    let mut spec = Bmp::pattern(1, 1, 32, false);
    spec.alpha = Some(vec![255]);
    spec.rgb = vec![[255, 255, 255]];
    for masks in [
        [0x8000_0001u32, 0xFF00, 0xFF, 0],
        [0xFF_0000, 0xFF_FF00, 0xFF, 0],
        [0xF0F0_0000, 0xFF00, 0xFF, 0],
    ] {
        let mut file = bmp(&spec);
        file[70..74].copy_from_slice(&[0xFF; 4]);
        for (i, mask) in masks.iter().enumerate() {
            file[54 + i * 4..58 + i * 4].copy_from_slice(&mask.to_le_bytes());
        }
        assert!(malformed(&file).contains("masks"), "{masks:x?}");
    }
}

#[test]
fn passed_through_png_data_must_end_with_the_last_row() {
    let png = Png::pattern(1, 1, 8, GREY, false);
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(&[0, 7]).unwrap();
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..512 {
        z.write_all(&zeros).unwrap();
    }
    let bomb = png.encode_with(&z.finish().unwrap());
    let start = std::time::Instant::now();
    assert!(malformed(&bomb).contains("runs past its last row"));
    assert!(start.elapsed().as_secs_f64() < 1.0, "{:?}", start.elapsed());
    let mut checksum = png.encode_with(&{
        let mut data = png.compressed();
        let last = data.len() - 1;
        data[last] ^= 1;
        data
    });
    repair_crcs(&mut checksum);
    assert!(malformed(&checksum).contains("corrupt"));
}

#[test]
fn a_passed_through_palette_index_must_be_in_the_palette() {
    let mut png = Png::pattern(4, 4, 8, PALETTE, false);
    png.palette.truncate(3);
    assert!(malformed(&png.encode()).contains("outside its palette"));
}

#[test]
fn a_jpeg_needs_its_components_and_scan_data() {
    let rgb = fixture("photo-rgb.jpg");
    let sos = rgb.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let length = usize::from(u16::from_be_bytes([rgb[sos + 2], rgb[sos + 3]]));
    let scan_header = sos + 2 + length;
    assert!(malformed(&rgb[..scan_header]).contains("no image data"));
    let mut eoi = rgb[..scan_header].to_vec();
    eoi.extend_from_slice(&[0xFF, 0xD9]);
    assert!(malformed(&eoi).contains("no image data"));
    let sof = rgb.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let mut listed = rgb.clone();
    listed[sof + 9] = 4;
    assert!(malformed(&listed).contains("components"));
}

#[test]
fn a_gif_with_an_empty_logical_screen_takes_its_frame_size() {
    let spec = Gif::pattern(20, 12, false);
    let mut file = gif(&spec);
    file[6..10].copy_from_slice(&[0; 4]);
    let (pdf, _) = case("gif-zero-screen", &file, "gif");
    assert_pixels("gif-zero-screen", &pdf, 20, 12, &spec.expected());
}

/// After an intended change to the output, regenerate the file with
/// `DOCRS_BLESS=1 cargo test -p docrs --test pdf_images output_matches_the_golden_file`,
/// then run `qpdf --check` on it and review the diff.
#[test]
fn output_matches_the_golden_file() {
    let (bytes, _) = read(&fixture("logo.png")).unwrap();
    let path = fixtures().join("pdf/image-logo.pdf");
    if std::env::var_os("DOCRS_BLESS").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    assert!(
        std::fs::read(&path).unwrap() == bytes,
        "differs from {}",
        path.display()
    );
}

fn seeds() -> Vec<Vec<u8>> {
    let mut fields = Bmp::pattern(6, 5, 32, false);
    fields.alpha = Some(vec![128; 30]);
    let mut noisy = Gif::pattern(60, 40, false);
    noisy.palette = (0..=255u8).flat_map(|i| [i, i, i]).collect();
    noisy.indices = (0..2400u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    vec![
        Png::pattern(6, 5, 2, PALETTE, true).encode(),
        Png::pattern(6, 5, 16, RGBA, false).encode(),
        fixture("photo-progressive.jpg"),
        fixture("photo-cmyk.jpg"),
        fixture("animated.gif"),
        gif(&Gif::pattern(6, 5, true)),
        gif(&noisy),
        bmp(&Bmp::pattern(6, 5, 4, false)),
        bmp(&fields),
    ]
}

/// Each PNG chunk's CRC recomputed, so that edits reach what is inside.
fn repair_crcs(file: &mut [u8]) {
    let mut at = 8;
    while at + 12 <= file.len() {
        let len = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        let Some(end) = (at + 8)
            .checked_add(len)
            .filter(|end| end + 4 <= file.len())
        else {
            return;
        };
        let crc = crc32fast::hash(&file[at + 4..end]);
        file[end..end + 4].copy_from_slice(&crc.to_be_bytes());
        at = end + 4;
    }
}

const COLOUR_DEPTHS: [(u8, u8); 15] = [
    (GREY, 1),
    (GREY, 2),
    (GREY, 4),
    (GREY, 8),
    (GREY, 16),
    (RGB, 8),
    (RGB, 16),
    (PALETTE, 1),
    (PALETTE, 2),
    (PALETTE, 4),
    (PALETTE, 8),
    (GREY_ALPHA, 8),
    (GREY_ALPHA, 16),
    (RGBA, 8),
    (RGBA, 16),
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Damaged images never panic: they are drawn, or refused with a typed
    /// error.
    #[test]
    fn damaged_images_never_panic(
        seed in 0usize..9,
        edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        cut in any::<prop::sample::Index>(),
    ) {
        let mut file = seeds().swap_remove(seed);
        for (at, value) in edits {
            let at = at.index(file.len());
            file[at] = value;
        }
        let len = cut.index(file.len() + 1);
        file.truncate(len.max(file.len() * 3 / 4));
        if file.starts_with(b"\x89PNG") {
            repair_crcs(&mut file);
        }
        if let Err(err) = read(&file) {
            prop_assert!(!matches!(err.kind(), ErrorKind::Internal), "{err}");
        }
    }

    /// PNGs damaged inside their compressed data, with valid chunks: rows,
    /// filters, palettes and header fields that disagree with the data.
    #[test]
    fn damaged_png_rows_never_panic(
        kind in 0usize..15,
        width in 1u32..20,
        height in 1u32..20,
        interlaced in any::<bool>(),
        edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..6),
        header in proptest::option::of((0usize..5, any::<u8>())),
        cut in any::<prop::sample::Index>(),
    ) {
        let (color, depth) = COLOUR_DEPTHS[kind];
        let png = Png::pattern(width, height, depth, color, interlaced);
        let mut raw = png.filtered();
        for (at, value) in edits {
            let at = at.index(raw.len());
            raw[at] = value;
        }
        raw.truncate(cut.index(raw.len() + 1).max(raw.len() * 7 / 8));
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&raw).unwrap();
        let mut file = png.encode_with(&z.finish().unwrap());
        if let Some((field, value)) = header {
            file[16 + 8 + field] = value;
            repair_crcs(&mut file);
        }
        if let Err(err) = read(&file) {
            prop_assert!(!matches!(err.kind(), ErrorKind::Internal), "{err}");
        }
    }
}
