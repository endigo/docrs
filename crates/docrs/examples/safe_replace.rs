//! Replace one exact occurrence and save only the changed part.
//!
//! ```text
//! cargo run -p docrs --example safe_replace -- in.docx "payment terms" "payment schedule" out.docx
//! ```

use docrs::{Document, FindOptions, Limits, ReplaceOptions};

fn main() -> docrs::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, find, with, out] = args.as_slice() else {
        eprintln!("usage: safe_replace IN.docx FIND WITH OUT.docx");
        std::process::exit(2);
    };
    let mut document = Document::open_path(input, &Limits::default())?;
    // `find_one` refuses when the text is missing or ambiguous.
    let span = document.find_one(find, &FindOptions::default())?;
    let report = document.replace(&span, with, &ReplaceOptions::default())?;
    let saved = document.save(out)?;
    println!(
        "replaced {:?} with {:?}; changed parts: {:?}",
        report.deleted_text, report.inserted_text, saved.changed_parts
    );
    Ok(())
}
