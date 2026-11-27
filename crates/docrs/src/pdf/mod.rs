//! PDF export. **Experimental**: exempt from semantic versioning until the
//! release that completes it. There is no public API yet. Output is
//! deterministic for a given build.

#![cfg_attr(not(feature = "test-support"), allow(dead_code))]

mod meta;
mod writer;

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use meta::Info;

use writer::{Dict, Object, Writer, count, name};

use crate::{Error, Limits, Result};

/// Page side limits in points (ISO 32000-1 Annex C).
const MIN_PAGE_SIDE: f64 = 3.0;
const MAX_PAGE_SIDE: f64 = 14_400.0;

/// A file of empty pages, `(width, height)` in points.
pub(crate) fn blank_document(
    pages: &[(f64, f64)],
    info: &meta::Info,
    producer: &str,
    limits: &Limits,
) -> Result<Vec<u8>> {
    check_pages(pages)?;
    let mut writer = Writer::new(limits.max_output_bytes)?;
    let catalog = writer.alloc()?;
    let tree = writer.alloc()?;
    let mut kids = Vec::with_capacity(pages.len());
    for &(width, height) in pages {
        let page = writer.alloc()?;
        let contents = writer.alloc()?;
        writer.stream(contents, Dict::new(), b"")?;
        let media_box: Vec<Object> = vec![0.0.into(), 0.0.into(), width.into(), height.into()];
        writer.dict(
            page,
            Dict::new()
                .with("Type", name("Page"))
                .with("Parent", tree)
                .with("MediaBox", media_box)
                .with("Contents", contents),
        )?;
        kids.push(page.into());
    }
    writer.dict(
        tree,
        Dict::new()
            .with("Type", name("Pages"))
            .with("Count", count(pages.len())?)
            .with("Kids", kids)
            .with("Resources", Dict::new()),
    )?;
    writer.dict(
        catalog,
        Dict::new()
            .with("Type", name("Catalog"))
            .with("Pages", tree),
    )?;
    let info_ref = writer.alloc()?;
    writer.dict(info_ref, info.dict(producer)?)?;
    let id = meta::file_id(writer.body());
    writer.finish(
        Dict::new()
            .with("Root", catalog)
            .with("Info", info_ref)
            .with("ID", id),
    )
}

fn check_pages(pages: &[(f64, f64)]) -> Result<()> {
    if pages.is_empty() {
        return Err(Error::invalid_argument(
            "pdf_no_pages",
            "a PDF needs at least one page",
        ));
    }
    for &(width, height) in pages {
        for side in [width, height] {
            if !(MIN_PAGE_SIDE..=MAX_PAGE_SIDE).contains(&side) {
                return Err(Error::invalid_argument(
                    "pdf_page_size",
                    format!(
                        "page {width} × {height} pt: each side must be {MIN_PAGE_SIDE} to {MAX_PAGE_SIDE} pt"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// [`blank_document`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn blank_document_for_tests(
    pages: &[(f64, f64)],
    info: &Info,
    producer: &str,
    limits: &Limits,
) -> Result<Vec<u8>> {
    blank_document(pages, info, producer, limits)
}
