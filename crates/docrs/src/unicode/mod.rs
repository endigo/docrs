//! Python-compatible Unicode predicates and folding.
//!
//! The upstream oracle's text semantics are defined by CPython 3.13
//! (Unicode 15.1). Rust's `char::is_whitespace` and `to_lowercase` differ from
//! Python's `str.isspace` and `str.casefold` (e.g. `ß → ss`, `ſ → s`,
//! U+001C..U+001F count as space in Python), so this module uses tables
//! generated from the pinned interpreter (`tools/gen_unicode_tables.py`).
//!
//! Text offsets reported to callers are Unicode scalar positions, matching
//! Python string indexing; [`ScalarIndex`] maps them to UTF-8 byte offsets.

mod tables;

pub use tables::UNICODE_VERSION;

/// Python `str.isspace()` for one character (also `re` `\s` on `str`).
pub fn is_py_space(c: char) -> bool {
    tables::ISSPACE.binary_search(&(c as u32)).is_ok()
}

/// Python `str.casefold()` of one character, appended to `out`.
pub fn casefold_char_into(c: char, out: &mut String) {
    match tables::CASEFOLD.binary_search_by_key(&(c as u32), |(cp, _)| *cp) {
        Ok(index) => out.push_str(tables::CASEFOLD[index].1),
        Err(_) => out.push(c),
    }
}

/// Python `str.casefold()`.
pub fn casefold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        casefold_char_into(c, &mut out);
    }
    out
}

/// Python `str.strip()` (no arguments).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python `str.lower()` is not table-pinned here; ASCII lowering is used
/// only where upstream compares ASCII-only identifiers.
pub fn ascii_lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// Maps Unicode scalar positions to UTF-8 byte offsets of one string.
#[derive(Debug, Clone)]
pub struct ScalarIndex {
    /// Byte offset of each scalar, plus the total length at the end.
    offsets: Vec<usize>,
}

impl ScalarIndex {
    /// Index `text`.
    pub fn new(text: &str) -> Self {
        let mut offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        offsets.push(text.len());
        ScalarIndex { offsets }
    }

    /// Number of scalars.
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    /// True for the empty string.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Byte offset of scalar position `position` (`position <= len`).
    pub fn byte(&self, position: usize) -> usize {
        self.offsets[position]
    }

    /// Scalar position of a byte offset on a character boundary.
    pub fn scalar(&self, byte: usize) -> Option<usize> {
        self.offsets.binary_search(&byte).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casefold_matches_python_expansions() {
        assert_eq!(casefold("Straße"), "strasse");
        assert_eq!(casefold("ſ"), "s");
        assert_eq!(casefold("ΣΑΣ"), "σασ");
        assert_eq!(casefold("ﬁ"), "fi");
        assert_eq!(casefold("İ"), "i\u{307}");
        assert_eq!(casefold("ᠮᠣᠩᠭᠣᠯ"), "ᠮᠣᠩᠭᠣᠯ");
    }

    #[test]
    fn python_space_predicate() {
        for c in [
            '\t', '\n', '\u{b}', '\u{c}', '\r', '\u{1c}', '\u{1f}', ' ', '\u{85}', '\u{a0}',
            '\u{3000}',
        ] {
            assert!(is_py_space(c), "{c:?}");
        }
        // Mongolian vowel separator and ZWSP are not Python whitespace.
        assert!(!is_py_space('\u{180e}'));
        assert!(!is_py_space('\u{200b}'));
    }

    #[test]
    fn scalar_index_maps_emoji_and_combining() {
        let text = "a😀e\u{301}ᠮ";
        let index = ScalarIndex::new(text);
        assert_eq!(index.len(), 5);
        assert_eq!(index.byte(1), 1);
        assert_eq!(index.byte(2), 5);
        assert_eq!(&text[index.byte(2)..index.byte(4)], "e\u{301}");
        assert_eq!(index.scalar(5), Some(2));
        assert_eq!(index.scalar(2), None);
    }
}
