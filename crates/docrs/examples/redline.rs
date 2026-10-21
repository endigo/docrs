//! Produce a tracked-changes redline turning ORIGINAL into REVISED. The
//! result is returned only after accepting it is proved to give REVISED and
//! rejecting it to give ORIGINAL.
//!
//! ```text
//! cargo run -p docrs --example redline -- original.docx revised.docx out.docx
//! ```

use docrs::Limits;
use docrs::compare::{CompareOptions, Granularity, compare};

fn main() -> docrs::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [original, revised, out] = args.as_slice() else {
        eprintln!("usage: redline ORIGINAL.docx REVISED.docx OUT.docx");
        std::process::exit(2);
    };
    let read = |path: &str| {
        std::fs::read(path)
            .map_err(|err| docrs::Error::io("read_failed", format!("reading {path}"), err))
    };
    let options = CompareOptions {
        author: "docrs".into(),
        date: None,
        granularity: Granularity::Word,
        materialize: None,
        acknowledge_protection: false,
    };
    let result = compare(
        &read(original)?,
        &read(revised)?,
        &options,
        &Limits::default(),
    )?;
    println!(
        "{} revisions (accept/reject verified)",
        result.revision_count
    );
    result.document.save(out)?;
    Ok(())
}
