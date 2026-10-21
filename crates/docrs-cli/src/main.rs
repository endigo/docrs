//! `docrs` command-line interface.
//!
//! Exit codes: 0 success; 2 invalid usage or argument; 3 safe refusal
//! (nothing changed); 4 resource budget exceeded; 5 I/O failure; 70 internal
//! error. With `--json`, stdout carries one JSON object with
//! `schema_version: 1`; errors are JSON on stdout as well.
//!
//! Mutating commands select and edit within one process and publish success
//! only after the output file has been atomically committed.

mod commands;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use docrs::{Error, ErrorKind};

#[derive(Parser)]
#[command(
    name = "docrs",
    version,
    about = "Preservation-first DOCX inspection and safe editing"
)]
pub struct Cli {
    /// Emit machine-readable JSON (schema_version 1).
    #[arg(long, global = true)]
    json: bool,
    /// Use the trusted-input resource profile instead of the default budgets.
    #[arg(long, global = true)]
    trusted: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Classify a file before opening it (never fails on bad input).
    Diagnose {
        /// File to diagnose.
        file: std::path::PathBuf,
    },
    /// Part-by-part semantic diff of two packages.
    Diff {
        /// First package.
        a: std::path::PathBuf,
        /// Second package.
        b: std::path::PathBuf,
    },
    /// Every block of every story, plus blind-region counts.
    Outline {
        /// Input package.
        input: std::path::PathBuf,
        /// Text layer: current, original or all.
        #[arg(long, default_value = "current")]
        view: String,
    },
    /// Locate text (exact by default) across every story.
    Find {
        /// Input package.
        input: std::path::PathBuf,
        /// Text to find.
        text: String,
        /// Use normalized (case/typography/whitespace-folded) matching.
        #[arg(long)]
        normalized: bool,
        /// Text layer: current, original or all.
        #[arg(long, default_value = "current")]
        view: String,
        /// Restrict to one story (e.g. word/document.xml).
        #[arg(long)]
        story: Option<String>,
    },
    /// Replace one exact occurrence (or all with --all) and save atomically.
    Replace {
        /// Input package.
        input: std::path::PathBuf,
        /// Text to find.
        #[arg(long)]
        find: String,
        /// Replacement text.
        #[arg(long = "with")]
        with_text: String,
        /// Output path (may equal the input).
        #[arg(long)]
        out: std::path::PathBuf,
        /// Replace every match (per-match refusals are reported).
        #[arg(long)]
        all: bool,
        /// Use normalized matching.
        #[arg(long)]
        normalized: bool,
        /// Select the nth match (1-based) instead of requiring uniqueness.
        #[arg(long)]
        nth: Option<usize>,
        /// Restrict to one story.
        #[arg(long)]
        story: Option<String>,
        /// Proceed despite Restrict Editing (the setting is kept).
        #[arg(long)]
        acknowledge_protection: bool,
    },
    /// Unified diff of visible block text between two documents.
    TextDiff {
        /// First document.
        a: std::path::PathBuf,
        /// Second document.
        b: std::path::PathBuf,
        /// Text layer: current, original or all.
        #[arg(long, default_value = "current")]
        view: String,
    },
    /// What accepting every pending revision would change (original vs current).
    Pending {
        /// Input document.
        input: std::path::PathBuf,
    },
    /// Produce a verified tracked-changes redline turning ORIGINAL into REVISED.
    Compare {
        /// Original document.
        original: std::path::PathBuf,
        /// Revised document.
        revised: std::path::PathBuf,
        /// Revision author.
        #[arg(long)]
        author: String,
        /// Revision date (ISO 8601 with zone; default now).
        #[arg(long)]
        date: Option<String>,
        /// Use whole-block granularity instead of word level.
        #[arg(long)]
        block: bool,
        /// Resolve pending input revisions first: accept or reject.
        #[arg(long)]
        materialize: Option<String>,
        /// Output path for the redline.
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// Pending tracked changes, in document order.
    Revisions {
        /// Input document.
        input: std::path::PathBuf,
    },
    /// Accept or reject every pending revision (optionally one author's).
    Resolve {
        /// Input document.
        input: std::path::PathBuf,
        /// Accept the revisions.
        #[arg(long, conflicts_with = "reject", required_unless_present = "reject")]
        accept: bool,
        /// Reject the revisions.
        #[arg(long)]
        reject: bool,
        /// Only revisions by this author.
        #[arg(long)]
        author: Option<String>,
        /// Output path (may equal the input).
        #[arg(long)]
        out: std::path::PathBuf,
        /// Proceed despite Restrict Editing (the setting is kept).
        #[arg(long)]
        acknowledge_protection: bool,
    },
    /// Append SOURCE's body (styles, numbering, media, links, bookmarks reconciled).
    Append {
        /// Destination document.
        input: std::path::PathBuf,
        /// Document whose body is appended.
        source: std::path::PathBuf,
        /// Style reconciliation: match_by_name or import_renamed.
        #[arg(long, default_value = "match_by_name")]
        styles: String,
        /// Section start: new_page or continuous.
        #[arg(long, default_value = "new_page")]
        section: String,
        /// Letterhead: destination or source.
        #[arg(long, default_value = "destination")]
        headers: String,
        /// Output path (may equal the input).
        #[arg(long)]
        out: std::path::PathBuf,
        /// Proceed despite Restrict Editing (the setting is kept).
        #[arg(long)]
        acknowledge_protection: bool,
    },
    /// Copy a range of SOURCE's body blocks after the paragraph containing --anchor.
    InsertFrom {
        /// Destination document.
        input: std::path::PathBuf,
        /// Document the blocks come from.
        source: std::path::PathBuf,
        /// Exact text in the source paragraph that starts the range.
        #[arg(long)]
        start: String,
        /// Exact text in the destination paragraph to insert after.
        #[arg(long)]
        anchor: String,
        /// Exact text in the source paragraph that ends the range.
        #[arg(long, conflicts_with = "count")]
        end: Option<String>,
        /// Number of blocks to copy when there is no --end.
        #[arg(long, default_value_t = 1)]
        count: i64,
        /// Leave out the start block.
        #[arg(long)]
        exclude_start: bool,
        /// Leave out the end block (requires --end).
        #[arg(long, requires = "end")]
        exclude_end: bool,
        /// Style reconciliation: match_by_name or import_renamed.
        #[arg(long, default_value = "match_by_name")]
        styles: String,
        /// Output path (may equal the input).
        #[arg(long)]
        out: std::path::PathBuf,
        /// Proceed despite Restrict Editing (the setting is kept).
        #[arg(long)]
        acknowledge_protection: bool,
    },
    /// Convert Markdown (or editor HTML) into a .docx styled by a reference document.
    FromMarkdown {
        /// Markdown or HTML file (`-` reads standard input).
        input: std::path::PathBuf,
        /// Source format: markdown or html (default: html for .html/.htm
        /// files, else markdown).
        #[arg(long)]
        from: Option<String>,
        /// Reference document supplying styles, lists, page setup, headers
        /// and footers (default: the bundled blank document).
        #[arg(long)]
        reference: Option<std::path::PathBuf>,
        /// Output path.
        #[arg(long)]
        out: std::path::PathBuf,
        /// Replace only the body paragraph reading exactly this text.
        #[arg(long, conflicts_with = "append")]
        placeholder: Option<String>,
        /// Append after the reference document's body instead of replacing it.
        #[arg(long)]
        append: bool,
        /// Map a style role to an existing style, e.g. code_block="My Code" (repeatable).
        #[arg(long = "style", value_name = "ROLE=STYLE")]
        styles: Vec<String>,
        /// Refuse when the reference document lacks a style (default: add it).
        #[arg(long)]
        strict_styles: bool,
        /// Refuse instead of warning when something cannot be converted faithfully.
        #[arg(long)]
        strict: bool,
        /// Directory relative image paths resolve against (default: the Markdown file's).
        #[arg(long)]
        resource_path: Option<std::path::PathBuf>,
        /// Add a table of contents (overrides front matter `toc`).
        #[arg(long, conflicts_with = "no_toc")]
        toc: bool,
        /// Never add a table of contents from front matter.
        #[arg(long)]
        no_toc: bool,
        /// Heading levels in the table of contents (1-9).
        #[arg(long)]
        toc_depth: Option<i64>,
        /// Title above the table of contents.
        #[arg(long)]
        toc_title: Option<String>,
        /// Add N to every heading level (-1 makes `##` a Heading 1).
        #[arg(long, allow_hyphen_values = true, default_value_t = 0)]
        shift_heading_level: i8,
        /// Typographic quotes, dashes and ellipses.
        #[arg(long)]
        smart: bool,
        /// Do not render the front-matter title block.
        #[arg(long)]
        no_title_block: bool,
        /// Do not copy front matter into the document properties.
        #[arg(long)]
        no_properties: bool,
        /// Do not caption stand-alone images with their alt text (HTML
        /// input never adds captions).
        #[arg(long)]
        no_captions: bool,
        /// Ignore the reference's List Bullet / List Number numbering and
        /// use the converter's own list definitions.
        #[arg(long)]
        converter_lists: bool,
        /// Ask Word to update the table of contents on open (it prompts).
        #[arg(long)]
        toc_page_numbers: bool,
        /// Leave the table of contents without the page numbers this
        /// conversion computes by estimating Word's layout.
        #[arg(long)]
        no_toc_numbers: bool,
        /// Add a centred page number footer when the reference has none.
        #[arg(long)]
        page_numbers: bool,
        /// Keep the reference's first page as a cover: the content starts on
        /// page 2 (default: only when a cover is detected).
        #[arg(long, conflicts_with = "no_cover_page")]
        cover_page: bool,
        /// Start the content on page 1, turning a detected cover's distinct
        /// first page off.
        #[arg(long)]
        no_cover_page: bool,
        /// Proceed despite Restrict Editing on the reference (the setting is kept).
        #[arg(long)]
        acknowledge_protection: bool,
        /// Refuse a reference package with irregularities Word tolerates
        /// (Office `[trash]` members, data in a ZIP directory entry) instead
        /// of setting them aside.
        #[arg(long)]
        strict_package: bool,
    },
    /// Write a starter reference document showing every Markdown style.
    MarkdownReference {
        /// Output path.
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// Show which style each Markdown role takes from a reference document.
    MarkdownStyles {
        /// Reference document (default: the bundled blank document).
        reference: Option<std::path::PathBuf>,
        /// Map a style role to an existing style (repeatable).
        #[arg(long = "style", value_name = "ROLE=STYLE")]
        styles: Vec<String>,
        /// Refuse a reference package with irregularities Word tolerates.
        #[arg(long)]
        strict_package: bool,
    },
    /// Estimate where Word for Mac breaks pages: each paragraph's page and
    /// position, headings' pages, fonts measured (JSON).
    Layout {
        /// The .docx file.
        file: std::path::PathBuf,
        /// A font file to measure with (repeatable), for fonts the bundled
        /// table lacks; it is read locally and never stored.
        #[arg(long = "font", value_name = "FILE")]
        fonts: Vec<std::path::PathBuf>,
    },
    /// What a conversion will take from a reference document: role styles,
    /// lifted branding, the section kept, headers and footers, cover page,
    /// compatibility mode and problems in plain language (JSON).
    MarkdownInspect {
        /// Reference document.
        reference: std::path::PathBuf,
        /// Map a style role to an existing style (repeatable).
        #[arg(long = "style", value_name = "ROLE=STYLE")]
        styles: Vec<String>,
        /// Inspect as for a conversion with page numbers.
        #[arg(long)]
        page_numbers: bool,
        /// Inspect as for a conversion forcing a cover page.
        #[arg(long, conflicts_with = "no_cover_page")]
        cover_page: bool,
        /// Inspect as for a conversion turning a cover page off.
        #[arg(long)]
        no_cover_page: bool,
        /// Refuse a reference package with irregularities Word tolerates.
        #[arg(long)]
        strict_package: bool,
    },
    /// Open and patch-save without edits (byte-identical when unchanged).
    Roundtrip {
        /// Input package.
        input: std::path::PathBuf,
        /// Output path.
        #[arg(long)]
        out: std::path::PathBuf,
    },
}

pub fn exit_code(err: &Error) -> u8 {
    match err.kind() {
        ErrorKind::Refusal(_) => 3,
        ErrorKind::InvalidArgument => 2,
        ErrorKind::ResourceLimit => 4,
        ErrorKind::Io => 5,
        ErrorKind::Internal => 70,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let limits = if cli.trusted {
        docrs::Limits::trusted()
    } else {
        docrs::Limits::default()
    };
    match commands::run(&cli.command, &limits, cli.json) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            if cli.json {
                println!("{}", err.to_json());
            } else {
                eprintln!("docrs: {err}");
            }
            ExitCode::from(exit_code(&err))
        }
    }
}
