//! Image characterization regressions (expected values frozen from the
//! pinned oracle, `docx.image.image.Image.from_blob`). Inputs are tiny
//! images generated from code (see `tests/ops/m4-image-characterize.json`).

mod support;

use docrs::image::{Image, ImageFormat, upstream_exception};
use support::{PNG_150DPI, hex};

const PNG_NO_IHDR: &str =
    "89504e470d0a1a0a0000000c49444154789c63606060000000040001f61738550000000049454e44ae426082";
const PNG_TRUNCATED: &str = "89504e470d0a1a0a0000000d4948445200000002000000020802000000fdd49a730000000b49444154789c636000030000070001b286acf4";
const BMP_ZERO_PPM: &str = "424d4e000000000000003600000028000000020000000300000001001800000000001800000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
const BMP_300DPI: &str = "424d4e0000000000000036000000280000000400000002000000010018000000000018000000232e0000232e00000000000000000000000000000000000000000000000000000000000000000000";
const GIF_5X7: &str = "47494638396105000700800000000000ffffff2c00000000010001000002024401003b";
const GIF_TRUNCATED: &str = "47494638396105";
const JFIF_72: &str = "ffd8ffe000104a46494600010101004800480000ffdb00040000ffc0000b080008001001011100ffda0008010100003f001234ff0056ffd9";
const JFIF_CM: &str = "ffd8ffe000104a464946000101020076003b0000ffdb00040000ffc0000b080014000a01011100ffda0008010100003f001234ff0056ffd9";
const JFIF_NO_SOF: &str =
    "ffd8ffe000104a46494600010101004800480000ffdb00040000ffda0008010100003f001234ff0056ffd9";
const EXIF_CM: &str = "ffd8ffe1004a4578696600004d4d002a000000080003011a00050000000100000032011b0005000000010000003a0128000300000001000300000000000000000076000000010000003b00000001ffc0000b08000a001401011100ffda0008010100003f00ffd9";
const TIFF_LE: &str = "49492a000800000005000001030001000000800200000101030001000000e00100001a010500010000004a0000001b010500010000005200000028010300010000000200000000000000c800000001000000c800000001000000";
const TIFF_ZERO_DEN: &str =
    "49492a000800000002000001030001000000080000001a0105000100000026000000000000002c01000000000000";
const TIFF_ASCII_ZERO: &str =
    "49492a000800000002000001030001000000080000000e010200000000002600000000000000";
const TIFF_ASCII_RES: &str = "49492a000800000003000001030001000000080000000101030001000000080000001a010200030000003200000000000000616200";
const TIFF_NO_WIDTH: &str = "49492a000800000001001a010500010000001a000000000000004800000001000000";

fn image(text: &str) -> Image {
    Image::from_blob(&hex(text)).unwrap()
}

fn shape(image: &Image) -> (Option<i64>, Option<i64>, i64, i64) {
    (
        image.px_width(),
        image.px_height(),
        image.horz_dpi(),
        image.vert_dpi(),
    )
}

#[test]
fn characterizes_each_format_like_upstream() {
    let png = image(PNG_150DPI);
    assert_eq!(png.format(), ImageFormat::Png);
    assert_eq!(png.content_type(), "image/png");
    assert_eq!(png.filename(), "image.png");
    assert_eq!(shape(&png), (Some(3), Some(2), 150, 150));
    assert_eq!(png.sha1(), "adb85078924eef555aad97e88628dfff5f84408b");

    let bmp = image(BMP_ZERO_PPM);
    assert_eq!((bmp.content_type(), bmp.ext()), ("image/bmp", "bmp"));
    assert_eq!(shape(&bmp), (Some(2), Some(3), 96, 96));
    assert_eq!(shape(&image(BMP_300DPI)), (Some(4), Some(2), 300, 300));

    let gif = image(GIF_5X7);
    assert_eq!((gif.content_type(), gif.ext()), ("image/gif", "gif"));
    assert_eq!(shape(&gif), (Some(5), Some(7), 72, 72));

    let jfif = image(JFIF_CM);
    assert_eq!(
        (jfif.format(), jfif.content_type(), jfif.ext()),
        (ImageFormat::Jfif, "image/jpeg", "jpg")
    );
    assert_eq!(shape(&jfif), (Some(10), Some(20), 300, 150));
    assert_eq!(shape(&image(JFIF_72)), (Some(16), Some(8), 72, 72));

    let exif = image(EXIF_CM);
    assert_eq!(exif.format(), ImageFormat::Exif);
    assert_eq!(shape(&exif), (Some(20), Some(10), 300, 150));

    let tiff = image(TIFF_LE);
    assert_eq!((tiff.content_type(), tiff.ext()), ("image/tiff", "tiff"));
    assert_eq!(shape(&tiff), (Some(640), Some(480), 200, 200));
    assert_eq!(shape(&image(TIFF_NO_WIDTH)), (None, None, 72, 72));
}

#[test]
fn native_and_scaled_dimensions_follow_python_arithmetic() {
    let cases = [
        (PNG_150DPI, (18288, 12192), (1500, 1000), (777, 518)),
        (JFIF_CM, (30480, 121920), (250, 1000), (777, 3108)),
        (TIFF_LE, (2926080, 2194560), (1333, 1000), (777, 583)),
        (BMP_300DPI, (12192, 6096), (2000, 1000), (777, 388)),
    ];
    for (bytes, native, by_height, by_width) in cases {
        let image = image(bytes);
        assert_eq!((image.width().unwrap(), image.height().unwrap()), native);
        assert_eq!(image.scaled_dimensions(None, None).unwrap(), native);
        assert_eq!(
            image.scaled_dimensions(None, Some(1000)).unwrap(),
            by_height
        );
        assert_eq!(image.scaled_dimensions(Some(777), None).unwrap(), by_width);
        assert_eq!(image.scaled_dimensions(Some(5), Some(6)).unwrap(), (5, 6));
    }
    let missing = image(TIFF_NO_WIDTH);
    assert_eq!(missing.width().unwrap_err().reason(), "non_numeric_value");
}

#[test]
fn malformed_streams_map_to_upstream_exception_classes() {
    let cases = [
        (
            PNG_NO_IHDR,
            "invalid_image_stream",
            "InvalidImageStreamError",
        ),
        (
            PNG_TRUNCATED,
            "unexpected_end_of_file",
            "UnexpectedEndOfFileError",
        ),
        (GIF_TRUNCATED, "truncated_struct", "error"),
        (JFIF_NO_SOF, "missing_jpeg_marker", "KeyError"),
        (&JFIF_72[..40], "jpeg_unexpected_end_of_file", "Exception"),
        (TIFF_ZERO_DEN, "zero_division", "ZeroDivisionError"),
        (TIFF_ASCII_ZERO, "truncated_struct", "error"),
        (TIFF_ASCII_RES, "non_numeric_value", "TypeError"),
        (
            "6e6f7420616e20696d616765",
            "unrecognized_image",
            "UnrecognizedImageError",
        ),
        ("", "unrecognized_image", "UnrecognizedImageError"),
    ];
    for (bytes, reason, class) in cases {
        let err = Image::from_blob(&hex(bytes)).unwrap_err();
        assert_eq!(err.code(), "invalid_argument", "{reason}");
        assert_eq!(err.reason(), reason);
        assert_eq!(upstream_exception(err.reason()), Some(class));
    }
}

#[test]
fn signature_order_and_offsets_match_upstream() {
    // "BM" alone is recognized as BMP and then runs out of bytes.
    let err = Image::from_blob(b"BM").unwrap_err();
    assert_eq!(err.reason(), "unexpected_end_of_file");
    // JFIF is detected at offset 6 regardless of what precedes it.
    let mut odd = hex(JFIF_72);
    odd[0] = 0;
    assert_eq!(Image::from_blob(&odd).unwrap().format(), ImageFormat::Jfif);
    // A big-endian TIFF signature.
    let be = hex("4d4d002a000000080001010000030000000100400000");
    assert_eq!(Image::from_blob(&be).unwrap().px_width(), Some(64));
}
