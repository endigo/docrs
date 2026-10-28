//! Search normalization with source mapping (upstream `_normalize.py` and
//! `search._normalized_with_map`).
//!
//! Normalization folds typographic punctuation and exotic spaces to ASCII,
//! deletes soft hyphens, collapses every run of Python whitespace to one
//! ASCII space and applies Python's full case folding (`ß → ss`). It is used
//! only for matching and anchor hashing; document text is never rewritten.
//!
//! [`normalized_with_map`] returns, for every normalized character, the
//! scalar index of the source character it came from, so expansions such as
//! `ß → ss` map both output characters back to one source position.

use sha2::{Digest, Sha256};

use crate::unicode::{casefold_char_into, is_py_space};

/// Upstream `NORMALIZE_CHARS` translation (`None` deletes the character).
fn translate(c: char) -> Option<Option<char>> {
    Some(match c {
        '\u{2018}' | '\u{2019}' => Some('\''),
        '\u{201C}' | '\u{201D}' => Some('"'),
        '\u{2013}' | '\u{2014}' | '\u{2212}' => Some('-'),
        '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\u{2009}' => Some(' '),
        '\u{00AD}' => None,
        '\t' => Some(' '),
        '\r' => Some('\n'),
        _ => return None,
    })
}

fn translated(c: char) -> Option<char> {
    match translate(c) {
        Some(mapped) => mapped,
        None => Some(c),
    }
}

/// Upstream `normalize_text` (translate, collapse `\s+`, casefold).
pub fn normalize_text(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_space = false;
    for c in value.chars() {
        let Some(c) = translated(c) else { continue };
        if is_py_space(c) {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }
    let mut out = String::with_capacity(collapsed.len());
    for c in collapsed.chars() {
        casefold_char_into(c, &mut out);
    }
    out
}

/// Normalized text plus, per normalized scalar, the source scalar index.
pub fn normalized_with_map(value: &str) -> (String, Vec<usize>) {
    let mut text = String::with_capacity(value.len());
    let mut map: Vec<usize> = Vec::with_capacity(value.len());
    let mut buffer = String::new();
    let mut previous_space = false;
    for (index, c) in value.chars().enumerate() {
        let Some(mut out) = translated(c) else {
            continue;
        };
        if is_py_space(out) {
            out = ' ';
        }
        buffer.clear();
        casefold_char_into(out, &mut buffer);
        for folded in buffer.chars() {
            if folded == ' ' {
                if previous_space {
                    continue;
                }
                previous_space = true;
            } else {
                previous_space = false;
            }
            text.push(folded);
            map.push(index);
        }
    }
    (text, map)
}

/// First 8 hex digits of SHA-256 over the normalized text (upstream
/// `content_hash`).
pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(normalize_text(text).as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether a policy-space interval maps to whole source characters.
pub fn source_aligned(start: usize, end: usize, map: &[usize]) -> bool {
    (start == 0 || map[start - 1] != map[start]) && (end == map.len() || map[end - 1] != map[end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_maps_back_to_one_source_char() {
        let (text, map) = normalized_with_map("Straße\u{00A0}\u{00A0}Ünd\u{00AD}er");
        assert_eq!(text, "strasse ünder");
        assert_eq!(map, vec![0, 1, 2, 3, 4, 4, 5, 6, 8, 9, 10, 12, 13]);
        assert!(!source_aligned(4, 5, &map));
        assert!(source_aligned(3, 6, &map));
    }

    #[test]
    fn normalize_text_matches_reference_rules() {
        assert_eq!(
            normalize_text("“Payment” — Terms\t\r\nNow"),
            "\"payment\" - terms now"
        );
        assert_eq!(normalize_text("a\u{1c}b"), "a b");
        assert_eq!(content_hash("Hello"), content_hash("hello"));
        assert_eq!(content_hash("Hello"), "2cf24dba");
    }
}
