//! Pages of images, for tests: each image fills a page its pixel size in
//! points.

use super::ImageResources;
use crate::Result;
use crate::pdf::limits::PdfLimits;
use crate::pdf::meta::Info;
use crate::pdf::writer::{Dict, Writer, write_name, write_number};

/// One page per `(part, bytes)`, an image written once per part, and each
/// page's image warnings.
pub(crate) fn image_document(
    images: &[(&str, &[u8])],
    info: &Info,
    producer: &str,
    limits: &PdfLimits,
) -> Result<(Vec<u8>, Vec<Vec<&'static str>>)> {
    let mut writer = Writer::new(limits.max_output_bytes)?;
    let mut resources = ImageResources::default();
    let mut pages = Vec::new();
    let mut warnings = Vec::new();
    for (part, data) in images {
        let placed = resources.add(&mut writer, part, data, limits)?;
        let (width, height) = (f64::from(placed.width), f64::from(placed.height));
        let mut content = b"q ".to_vec();
        write_number(&mut content, width)?;
        content.extend_from_slice(b" 0 0 ");
        write_number(&mut content, height)?;
        content.extend_from_slice(b" 0 0 cm ");
        write_name(&mut content, &placed.name)?;
        content.extend_from_slice(b" Do Q\n");
        pages.push(((width, height), content));
        warnings.push(placed.warnings.clone());
    }
    let resources = Dict::new().with("XObject", resources.resources());
    let pdf = crate::pdf::write_document(writer, &pages, resources, info, producer)?;
    Ok((pdf, warnings))
}
