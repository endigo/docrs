//! Append one document to another, reconciling styles, numbering, media,
//! hyperlinks and bookmarks, and print the composition report.
//!
//! ```text
//! cargo run -p docrs --example compose -- base.docx addition.docx out.docx
//! ```

use docrs::composition::{AppendOptions, StylesMode};
use docrs::{Document, Limits};

fn main() -> docrs::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [base, addition, out] = args.as_slice() else {
        eprintln!("usage: compose BASE.docx ADDITION.docx OUT.docx");
        std::process::exit(2);
    };
    let mut document = Document::open_path(base, &Limits::default())?;
    let source = Document::open_path(addition, &Limits::default())?;
    let options = AppendOptions {
        styles: StylesMode::ImportRenamed,
        ..AppendOptions::default()
    };
    let report = document.append_document(&source, &options)?;
    println!("{}", report.to_json());
    document.save(out)?;
    Ok(())
}
