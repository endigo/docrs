//! PDF export. **Experimental**: exempt from semantic versioning until the
//! release that completes it. There is no public API yet.

// The writer's full surface is for the renderer, which lands later.
#![allow(dead_code)]

mod meta;
mod writer;

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use meta::Info;
#[cfg(not(feature = "test-support"))]
use meta::Info;

use writer::{Dict, Object, Writer, name};

use crate::{Limits, Result};

/// A file of empty pages, `(width, height)` in points.
pub(crate) fn blank_document(
    pages: &[(f64, f64)],
    info: &Info,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let mut writer = Writer::new(limits.max_output_bytes)?;
    let catalog = writer.alloc();
    let tree = writer.alloc();
    let mut kids = Vec::with_capacity(pages.len());
    for &(width, height) in pages {
        let page = writer.alloc();
        let contents = writer.alloc();
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
            .with("Count", pages.len() as i64)
            .with("Kids", kids)
            .with("Resources", Dict::new()),
    )?;
    writer.dict(
        catalog,
        Dict::new()
            .with("Type", name("Catalog"))
            .with("Pages", tree),
    )?;
    let info_ref = writer.alloc();
    writer.dict(info_ref, info.dict()?)?;
    let id = meta::file_id(writer.body());
    writer.finish(
        Dict::new()
            .with("Root", catalog)
            .with("Info", info_ref)
            .with("ID", id),
    )
}

/// [`blank_document`] for tests only (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn blank_document_for_tests(
    pages: &[(f64, f64)],
    info: &Info,
    limits: &Limits,
) -> Result<Vec<u8>> {
    blank_document(pages, info, limits)
}
