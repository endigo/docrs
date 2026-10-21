//! # docrs
//!
//! A preservation-first DOCX editing core. Portions are adapted from
//! paper-docx (MIT; see `NOTICE.md`).
//!
//! The original package and XML arenas are the preservation authority:
//! unchanged parts keep their original bytes, unknown markup and namespace
//! scopes survive edits, and every mutating operation either commits
//! completely or refuses with the document unchanged.

/// The README's examples, compiled as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

pub mod anchor;
pub mod authoring;
pub mod blocks;
pub mod bookmarks;
pub mod clock;
pub mod commentops;
pub mod comments;
pub mod compare;
pub mod composition;
pub mod controls;
pub mod coreprops;
pub mod difflib;
pub mod document;
pub mod drawing;
pub mod error;
pub mod fields;
pub mod formatting;
pub mod image;
pub mod layout;
pub mod limits;
pub mod links;
#[cfg(feature = "markdown")]
pub mod markdown;
pub mod normalize;
pub mod notes;
pub mod numbering;
pub mod oxml;
pub mod package;
pub mod parts;
pub mod protection;
pub mod replace;
pub mod revision;
pub mod search;
pub mod sections;
pub mod story;
pub mod styles;
pub mod styles_write;
pub mod tableops;
pub mod textdiff;
pub mod txn;
pub mod unicode;
pub mod wml;
pub mod xml;

pub use document::{Document, DocumentId};
pub use error::{Error, ErrorKind, LimitExceeded, RefusalKind, Result};
pub use limits::Limits;
pub use package::zip_read::Tolerance;
pub use replace::{ReplaceAllOptions, ReplaceAllReport, ReplaceOptions, ReplaceReport};
pub use search::{FindOptions, MatchPolicy, Span};
pub use story::{Block, BlockKind, Outline, View};
