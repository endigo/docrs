//! Propose a tracked change with a comment, then list pending revisions.
//!
//! ```text
//! cargo run -p docrs --example review -- in.docx "payment terms" "payment schedule" out.docx
//! ```

use docrs::{Document, FindOptions, Limits, ReplaceOptions};

fn main() -> docrs::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, find, with, out] = args.as_slice() else {
        eprintln!("usage: review IN.docx FIND WITH OUT.docx");
        std::process::exit(2);
    };
    let mut document = Document::open_path(input, &Limits::default())?;
    let span = document.find_one(find, &FindOptions::default())?;
    document.add_comment(
        &span,
        "Proposed wording change.",
        "Reviewer",
        Some("R"),
        None,
    )?;
    // The span was consumed by the comment's commit; locate the text again.
    let span = document.find_one(find, &FindOptions::default())?;
    let options = ReplaceOptions {
        tracked: true,
        author: Some("Reviewer".into()),
        ..ReplaceOptions::default()
    };
    document.replace(&span, with, &options)?;
    for revision in document.revisions()?.iter() {
        println!(
            "{} by {:?}: {:?}",
            revision.revision_type, revision.author, revision.text
        );
    }
    document.save(out)?;
    Ok(())
}
