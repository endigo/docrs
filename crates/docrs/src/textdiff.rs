//! Human-readable semantic text diff (upstream `_textdiff.py`).
//!
//! Per-story unified diffs of `[kind] text` block lines (no block index in
//! the label, so one insertion does not shift every later line).
//! [`pending_changes`] diffs one document's `original` view against its
//! `current` view: what accepting every revision would change.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::difflib::unified_diff;
use crate::story::{BlockKind, View};
use crate::{Document, Result};

/// One story's unified diff (empty when unchanged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoryTextDiff {
    /// Story name.
    pub story: String,
    /// Unified diff lines.
    pub diff_lines: Vec<String>,
}

/// Per-story unified diffs of visible block text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextDiff {
    /// Every story present in either document, sorted.
    pub stories: Vec<StoryTextDiff>,
}

impl TextDiff {
    /// `+`/`-` lines excluding the file headers.
    pub fn changed_line_count(&self) -> usize {
        self.stories
            .iter()
            .flat_map(|story| story.diff_lines.iter())
            .filter(|line| {
                (line.starts_with('+') || line.starts_with('-'))
                    && !line.starts_with("+++")
                    && !line.starts_with("---")
            })
            .count()
    }

    /// No story changed.
    pub fn is_empty(&self) -> bool {
        self.stories.iter().all(|story| story.diff_lines.is_empty())
    }

    /// Upstream `to_dict()` (`schema: paper_text_diff`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_text_diff",
            "version": 1,
            "changed_line_count": self.changed_line_count(),
            "stories": self.stories,
        })
    }
}

fn story_lines(document: &Document, view: View) -> Result<BTreeMap<String, Vec<String>>> {
    let mut lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for block in document.blocks(view)? {
        let kind = match block.kind {
            BlockKind::Paragraph => "paragraph",
            BlockKind::Table => "table",
        };
        lines
            .entry(block.story.clone())
            .or_default()
            .push(format!("[{kind}] {}", block.text));
    }
    Ok(lines)
}

fn diff(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
    from: &str,
    to: &str,
) -> TextDiff {
    let empty = Vec::new();
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    TextDiff {
        stories: names
            .into_iter()
            .map(|story| StoryTextDiff {
                story: story.clone(),
                diff_lines: unified_diff(
                    before.get(story).unwrap_or(&empty),
                    after.get(story).unwrap_or(&empty),
                    &format!("{from}:{story}"),
                    &format!("{to}:{story}"),
                    3,
                ),
            })
            .collect(),
    }
}

/// What changed textually between two documents (labels name them in the
/// diff headers, as upstream uses the paths).
pub fn text_diff(
    a: &Document,
    a_label: &str,
    b: &Document,
    b_label: &str,
    view: View,
) -> Result<TextDiff> {
    Ok(diff(
        &story_lines(a, view)?,
        &story_lines(b, view)?,
        a_label,
        b_label,
    ))
}

/// What the pending revisions would change if all were accepted.
pub fn pending_changes(document: &Document) -> Result<TextDiff> {
    Ok(diff(
        &story_lines(document, View::Original)?,
        &story_lines(document, View::Current)?,
        "original",
        "current",
    ))
}
