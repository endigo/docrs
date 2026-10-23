//! Property tests: no panics on hostile bytes, and no partial commits under
//! random edit sequences.

mod support;

use docrs::package::zip_read::read_archive;
use docrs::{Document, FindOptions, Limits, ReplaceOptions, View};
use proptest::prelude::*;
use support::*;

fn corpus() -> Vec<Vec<u8>> {
    vec![
        build_zip(&minimal_members()),
        fixture("generated/minimal-clean/minimal.docx"),
        fixture("generated/feature-isolated/fragmented-runs.docx"),
    ]
}

proptest! {
    // DOCRS_PROPTEST_CASES overrides the default (e.g. 400 for release checks).
    #![proptest_config(ProptestConfig { cases: std::env::var("DOCRS_PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(96), .. ProptestConfig::default() })]

    /// Arbitrary byte flips, truncations and insertions never panic: the
    /// guard either opens the package or refuses with a typed error.
    #[test]
    fn corrupted_archives_never_panic(which in 0usize..3, edits in prop::collection::vec((any::<u16>(), any::<u8>(), 0u8..3), 1..8)) {
        let mut bytes = corpus()[which].clone();
        for (position, value, kind) in edits {
            if bytes.is_empty() { break; }
            let at = position as usize % bytes.len();
            match kind {
                0 => bytes[at] = value,
                1 => bytes.truncate(at),
                _ => bytes.insert(at, value),
            }
        }
        let _ = read_archive(&bytes, &Limits::default());
        if let Ok(doc) = Document::open_bytes(&bytes, &Limits::default()) {
            let _ = doc.outline(View::All);
            let _ = doc.find_text("a", &FindOptions::default());
        }
    }

    #[test]
    fn arbitrary_xml_never_panics(text in "(<[a-z:]{0,4}( [a-z:]{1,3}=\"[^\"<]{0,4}\")?/?>|</[a-z:]{0,4}>|&[a-z#0-9]{0,4};|[a-z <>&\"'\\]]){0,40}") {
        let _ = docrs::xml::parse_str(&text);
    }

    /// Random replacements either commit (and the result reparses) or
    /// refuse leaving the package byte-identical.
    #[test]
    fn random_replacements_commit_fully_or_not_at_all(
        steps in prop::collection::vec((0usize..12, 0usize..6, "[a-zA-Z ]{0,6}"), 1..6)
    ) {
        let input = fixture("generated/feature-isolated/fragmented-runs.docx");
        let mut doc = Document::open_bytes(&input, &Limits::default()).unwrap();
        let mut spans = Vec::new();
        for (start, len, with) in steps {
            let text: String = doc.blocks(View::Current).unwrap().iter().map(|b| b.text.clone()).collect::<Vec<_>>().join("\n");
            let chars: Vec<char> = text.chars().collect();
            if chars.is_empty() { break; }
            let start = start % chars.len();
            let end = (start + len.max(1)).min(chars.len());
            let needle: String = chars[start..end].iter().collect();
            let found = doc.find_text(&needle, &FindOptions::default()).unwrap_or_default();
            let Some(span) = found.into_iter().next() else { continue };
            spans.push(span.clone());
            let before = doc.patch_bytes().unwrap().0;
            match doc.replace(&span, &with, &ReplaceOptions::default()) {
                Ok(report) => {
                    let (after, _) = doc.patch_bytes().unwrap();
                    prop_assert!(Document::open_bytes(&after, &Limits::default()).is_ok());
                    if !report.changed { prop_assert_eq!(after, before); }
                }
                Err(err) => {
                    prop_assert!(err.is_refusal() || err.kind() == docrs::ErrorKind::InvalidArgument, "{}", err);
                    prop_assert_eq!(doc.patch_bytes().unwrap().0, before);
                }
            }
            // Old spans may be stale, never silently applied to moved text.
            for old in &spans {
                let snapshot = doc.patch_bytes().unwrap().0;
                if doc.replace(old, "Q", &ReplaceOptions::default()).is_err() {
                    prop_assert_eq!(doc.patch_bytes().unwrap().0, snapshot);
                }
            }
        }
    }
}
