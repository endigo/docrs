//! Release-gate properties (M7): random mixed edit sequences commit fully or
//! not at all, normalization source maps stay aligned, malformed
//! relationships and arbitrary namespace prefixes never panic and never
//! break round trips.

mod support;

use docrs::clock::Timestamp;
use docrs::composition::{AppendOptions, HeadersFrom, SectionStart, StylesMode};
use docrs::normalize::{normalize_text, normalized_with_map};
use docrs::{Document, ErrorKind, FindOptions, Limits, MatchPolicy, ReplaceOptions, View};
use proptest::prelude::*;
use support::*;

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: std::env::var("DOCRS_PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64),
        ..ProptestConfig::default()
    }
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// Characters that exercise every normalization rule: case-fold
/// expansions, typographic folds, exotic and collapsed spaces, deletions,
/// combining marks, astral scalars and Mongolian.
const POOL: &[char] = &[
    'a', 'b', 'S', 's', 'ß', 'ſ', 'İ', 'ﬁ', 'Σ', 'ς', 'é', 'e', '\u{0301}', '😀', 'ᠠ', ' ',
    '\u{00A0}', '\u{2009}', '\u{3000}', '\u{00AD}', '\u{2019}', '\'', '\u{201C}', '"', '\u{2014}',
    '-', '.',
];

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

fn document_with_text(text: &str) -> Document {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\"><w:body><w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p></w:body></w:document>",
        xml_escape(text)
    );
    Document::open_bytes(&minimal_package_with(&xml), &Limits::default()).unwrap()
}

/// One step of a random edit sequence.
#[derive(Debug, Clone)]
enum Edit {
    AddParagraph(String),
    Replace {
        at: usize,
        len: usize,
        with: String,
        tracked: bool,
    },
    Comment {
        at: usize,
        len: usize,
    },
    AcceptAll,
    RejectAll,
    Append {
        continuous: bool,
        renamed: bool,
    },
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        "[a-z ]{0,8}".prop_map(Edit::AddParagraph),
        (0usize..400, 1usize..8, "[a-zA-Z ]{0,6}", any::<bool>()).prop_map(
            |(at, len, with, tracked)| Edit::Replace {
                at,
                len,
                with,
                tracked
            }
        ),
        (0usize..400, 1usize..8).prop_map(|(at, len)| Edit::Comment { at, len }),
        Just(Edit::AcceptAll),
        Just(Edit::RejectAll),
        (any::<bool>(), any::<bool>()).prop_map(|(continuous, renamed)| Edit::Append {
            continuous,
            renamed
        }),
    ]
}

/// A span over `len` characters at `at` of the visible text, if that text is
/// found uniquely enough to address.
fn span_at(doc: &Document, at: usize, len: usize) -> Option<docrs::Span> {
    let text: String = doc
        .blocks(View::Current)
        .ok()?
        .iter()
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let start = at % chars.len();
    let end = (start + len).min(chars.len());
    let needle: String = chars[start..end].iter().collect();
    doc.find_text(&needle, &FindOptions::default())
        .ok()?
        .into_iter()
        .next()
}

fn apply(doc: &mut Document, source: &Document, step: &Edit) -> docrs::Result<()> {
    let date = Some(Timestamp::from_unix(1_704_164_645));
    match step {
        Edit::AddParagraph(text) => doc.add_paragraph(text, None).map(|_| ()),
        Edit::Replace {
            at,
            len,
            with,
            tracked,
        } => {
            let Some(span) = span_at(doc, *at, *len) else {
                return Ok(());
            };
            let options = ReplaceOptions {
                tracked: *tracked,
                author: tracked.then(|| "Fuzz".to_string()),
                date,
                preserve_revision: false,
            };
            doc.replace(&span, with, &options).map(|_| ())
        }
        Edit::Comment { at, len } => {
            let Some(span) = span_at(doc, *at, *len) else {
                return Ok(());
            };
            doc.add_comment(&span, "note", "Fuzz", None, date)
                .map(|_| ())
        }
        Edit::AcceptAll => doc.revisions()?.accept_all(doc, None).map(|_| ()),
        Edit::RejectAll => doc.revisions()?.reject_all(doc, None).map(|_| ()),
        Edit::Append {
            continuous,
            renamed,
        } => {
            let options = AppendOptions {
                section: if *continuous {
                    SectionStart::Continuous
                } else {
                    SectionStart::NewPage
                },
                styles: if *renamed {
                    StylesMode::ImportRenamed
                } else {
                    StylesMode::MatchByName
                },
                headers: HeadersFrom::Destination,
            };
            doc.append_document(source, &options).map(|_| ())
        }
    }
}

proptest! {
    #![proptest_config(config())]

    /// Random sequences across replacement (tracked and untracked),
    /// comments, revision resolution and composition: every step either
    /// commits a package that reopens, or fails with a typed, non-internal
    /// error leaving the package byte-identical.
    #[test]
    fn mixed_edit_sequences_commit_fully_or_not_at_all(
        fixture_index in 0usize..3,
        steps in prop::collection::vec(edit(), 1..7),
    ) {
        let fixtures = [
            "generated/feature-isolated/fragmented-runs.docx",
            "generated/feature-isolated/tracked-ins-del.docx",
            "generated/feature-isolated/comments.docx",
        ];
        let mut doc = Document::open_bytes(&fixture(fixtures[fixture_index]), &Limits::default()).unwrap();
        let source = Document::open_bytes(&fixture("generated/minimal-clean/minimal.docx"), &Limits::default()).unwrap();
        let source_before = source.patch_bytes().unwrap().0;
        for step in &steps {
            let before = doc.patch_bytes().unwrap().0;
            match apply(&mut doc, &source, step) {
                Ok(()) => {
                    let (after, _) = doc.patch_bytes().unwrap();
                    let reopened = Document::open_bytes(&after, &Limits::default());
                    prop_assert!(reopened.is_ok(), "{:?}: {:?}", step, reopened.err());
                    prop_assert!(reopened.unwrap().outline(View::All).is_ok());
                }
                Err(err) => {
                    prop_assert!(err.kind() != ErrorKind::Internal, "{:?}: {}", step, err);
                    prop_assert_eq!(doc.patch_bytes().unwrap().0, before, "{:?}", step);
                }
            }
        }
        // The composition source is never modified.
        prop_assert_eq!(source.patch_bytes().unwrap().0, source_before);
    }

    /// The normalization map is total, monotone and in range, and agrees
    /// with `normalize_text`.
    #[test]
    fn normalization_maps_every_scalar_back_to_its_source(chars in prop::collection::vec(prop::sample::select(POOL), 0..24)) {
        let text: String = chars.iter().collect();
        let (normalized, map) = normalized_with_map(&text);
        prop_assert_eq!(&normalized, &normalize_text(&text));
        prop_assert_eq!(map.len(), normalized.chars().count());
        prop_assert!(map.windows(2).all(|w| w[0] <= w[1]));
        prop_assert!(map.iter().all(|&i| i < chars.len()));
    }

    /// Normalized search over arbitrary Unicode finds every substring of
    /// the paragraph, and each span's source text normalizes to the query.
    #[test]
    fn normalized_spans_map_to_whole_source_characters(
        chars in prop::collection::vec(prop::sample::select(POOL), 1..20),
        start in 0usize..20,
        len in 1usize..8,
    ) {
        let text: String = chars.iter().collect();
        let start = start % chars.len();
        let end = (start + len).min(chars.len());
        let query: String = chars[start..end].iter().collect();
        let wanted = normalize_text(&query);
        prop_assume!(!wanted.trim().is_empty());
        let doc = document_with_text(&text);
        let options = FindOptions { policy: MatchPolicy::Normalized, ..FindOptions::default() };
        match doc.find_text(&query, &options) {
            Ok(spans) => {
                prop_assert!(!spans.is_empty(), "query {:?} not found in {:?}", query, text);
                for span in &spans {
                    prop_assert_eq!(normalize_text(span.text()), wanted.clone(), "span {:?} in {:?}", span.text(), text);
                    prop_assert!(text.contains(span.text()));
                }
            }
            Err(err) => prop_assert!(false, "{} query {:?} text {:?}", err, query, text),
        }
    }

    /// Hostile relationship parts (odd targets, ids, types and modes) never
    /// panic; a package that opens also saves and reopens.
    #[test]
    fn malformed_relationships_never_panic(
        entries in prop::collection::vec((
            prop::sample::select(vec!["rId1", "rId2", "rId1 ", "", "r:Id", "rId\u{0}"]),
            prop::sample::select(vec![
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles",
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink",
                "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
                "",
            ]),
            prop::sample::select(vec![
                "word/document.xml", "/word/document.xml", "./word/document.xml", "word/../word/document.xml",
                "word/missing.xml", "../outside.xml", "word/document%2Exml", "WORD/DOCUMENT.XML",
                "http://example.com/", "", "word/", "word/document.xml#frag",
            ]),
            prop::sample::select(vec!["", " TargetMode=\"External\"", " TargetMode=\"Internal\"", " TargetMode=\"bogus\""]),
        ), 0..4),
        on_package in any::<bool>(),
    ) {
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
        if !on_package {
            xml.push_str("<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>");
        }
        for (id, kind, target, mode) in &entries {
            xml.push_str(&format!(
                "<Relationship Id=\"{}\" Type=\"{kind}\" Target=\"{}\"{mode}/>",
                xml_escape(id).replace('\u{0}', "&#0;"),
                xml_escape(target)
            ));
        }
        xml.push_str("</Relationships>");
        let mut members = minimal_members();
        if on_package {
            members[1] = RawMember::new("_rels/.rels", xml.as_bytes());
        } else {
            members.push(RawMember::new("word/_rels/document.xml.rels", xml.as_bytes()));
        }
        let bytes = build_zip(&members);
        if let Ok(mut doc) = Document::open_bytes(&bytes, &Limits::default()) {
            let _ = doc.outline(View::All);
            let (saved, _) = doc.patch_bytes().unwrap();
            prop_assert_eq!(&saved, &bytes, "no-op save must be byte-identical");
            if doc.add_paragraph("added", None).is_ok() {
                let (edited, _) = doc.patch_bytes().unwrap();
                prop_assert!(Document::open_bytes(&edited, &Limits::default()).is_ok());
            }
        }
    }

    /// Any prefix (and redundant nested redeclarations) for the
    /// WordprocessingML namespace: search, replacement and save/reopen work
    /// and the no-op save is byte-identical.
    #[test]
    fn arbitrary_namespace_prefixes_round_trip(
        prefix in "[a-z][a-z0-9]{0,4}",
        redeclare in any::<bool>(),
        word in "[a-z]{3,8}",
    ) {
        prop_assume!(!prefix.starts_with("xml"));
        // The word is searched for in "alpha {word} omega"; a word inside
        // "alpha" or "omega" ("pha") would match twice.
        prop_assume!(!"alpha".contains(word.as_str()) && !"omega".contains(word.as_str()));
        let p = &prefix;
        let inner = if redeclare { format!(" xmlns:{p}=\"{W}\"") } else { String::new() };
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><{p}:document xmlns:{p}=\"{W}\"><{p}:body><{p}:p{inner}><{p}:r><{p}:t>alpha {word} omega</{p}:t></{p}:r></{p}:p></{p}:body></{p}:document>"
        );
        let bytes = minimal_package_with(&xml);
        let mut doc = Document::open_bytes(&bytes, &Limits::default()).unwrap();
        prop_assert_eq!(doc.patch_bytes().unwrap().0, bytes);
        let span = doc.find_one(&word, &FindOptions::default()).unwrap();
        doc.replace(&span, "REPLACED", &ReplaceOptions::default()).unwrap();
        let (edited, report) = doc.patch_bytes().unwrap();
        prop_assert_eq!(report.changed_parts, vec!["word/document.xml".to_string()]);
        let reopened = Document::open_bytes(&edited, &Limits::default()).unwrap();
        prop_assert_eq!(reopened.find_text("alpha REPLACED omega", &FindOptions::default()).unwrap().len(), 1);
        let edited_xml = String::from_utf8(
            docrs::xml::serialize(reopened.package().xml("word/document.xml").unwrap()),
        ).unwrap();
        let opening = format!("<{p}:document xmlns:{p}=");
        prop_assert!(edited_xml.contains(&opening));
    }
}
