//! `[Content_Types].xml` validation and lookup (upstream
//! `_zipguard._parse_content_types`, `pkgreader._ContentTypeMap`).

use std::collections::{HashMap, HashSet};

use crate::unicode::{casefold, py_strip};
use crate::xml::{self, NodeData, XmlTree};
use crate::{Error, Limits, Result};

/// The content-types namespace.
pub const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// Parsed content-type declarations, in document order.
///
/// Lookups go through an index built when the declarations are parsed, so
/// the public lists are for reading: pushing to them does not change what
/// [`ContentTypes::lookup`] answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentTypes {
    /// `(extension, content type)` from `Default` elements.
    pub defaults: Vec<(String, String)>,
    /// `(part name, content type)` from `Override` elements.
    pub overrides: Vec<(String, String)>,
    /// Lower-cased keys of the declarations above, built by
    /// [`parse_content_types`].
    index: LookupIndex,
}

/// Lower-cased extension or part name → position of its first declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LookupIndex {
    defaults: HashMap<String, usize>,
    overrides: HashMap<String, usize>,
}

fn malformed(reason: &'static str, message: impl Into<String>) -> Error {
    Error::malformed(reason, message)
}

pub(crate) fn parse_tree(data: &[u8]) -> Result<XmlTree> {
    // Content types are validated with default XML limits regardless of the
    // caller's profile: they are small by construction.
    xml::parse(data, &Limits::default()).map_err(|err| match err {
        xml::XmlError::Malformed {
            reason: "dtd_prohibited",
            ..
        } => malformed(
            "content_types_dtd",
            "[Content_Types].xml contains a prohibited DTD",
        ),
        xml::XmlError::Malformed { .. } => malformed(
            "content_types_malformed",
            "[Content_Types].xml is malformed",
        ),
        xml::XmlError::Limit(error) => error.context("[Content_Types].xml"),
    })
}

/// Validate `[Content_Types].xml` bytes.
pub fn parse_content_types(data: &[u8]) -> Result<ContentTypes> {
    let tree = parse_tree(data)?;
    let root = tree.root();
    if !tree.is(root, CT_NS, "Types") {
        return Err(malformed(
            "content_types_root",
            "[Content_Types].xml has an unexpected root element",
        ));
    }
    let mut types = ContentTypes::default();
    let mut override_keys: HashSet<String> = HashSet::new();
    for child in tree.children(root) {
        let NodeData::Element(_) = tree.data(*child) else {
            continue;
        };
        if tree.is(*child, CT_NS, "Default") {
            let extension = tree.attr(*child, None, "Extension");
            let content_type = tree.attr(*child, None, "ContentType");
            let (Some(extension), Some(content_type)) = (extension, content_type) else {
                return Err(invalid_default());
            };
            if extension.is_empty()
                || extension.contains('.')
                || extension.contains('/')
                || extension != py_strip(extension)
                || content_type.is_empty()
                || content_type != py_strip(content_type)
            {
                return Err(invalid_default());
            }
            let key = extension.to_lowercase();
            if types.index.defaults.contains_key(&key) {
                return Err(malformed(
                    "content_types_duplicate_default",
                    format!(
                        "[Content_Types].xml declares the extension {extension:?} in more than one Default \
                         element; an extension may carry only one Default declaration, so the package does \
                         not bind it to a single content type and Word refuses to open it. Delete the \
                         duplicate Default so {extension:?} is declared exactly once"
                    ),
                ));
            }
            types.index.defaults.insert(key, types.defaults.len());
            types
                .defaults
                .push((extension.to_string(), content_type.to_string()));
            continue;
        }
        if tree.is(*child, CT_NS, "Override") {
            let part_name = tree.attr(*child, None, "PartName");
            let content_type = tree.attr(*child, None, "ContentType");
            let (Some(part_name), Some(content_type)) = (part_name, content_type) else {
                return Err(invalid_override());
            };
            if part_name.is_empty()
                || !part_name.starts_with('/')
                || part_name == "/"
                || part_name != py_strip(part_name)
                || content_type.is_empty()
                || content_type != py_strip(content_type)
            {
                return Err(invalid_override());
            }
            if !override_keys.insert(casefold(part_name)) {
                return Err(malformed(
                    "content_types_ambiguous_override",
                    "[Content_Types].xml contains an ambiguous Override declaration",
                ));
            }
            types
                .index
                .overrides
                .entry(part_name.to_lowercase())
                .or_insert(types.overrides.len());
            types
                .overrides
                .push((part_name.to_string(), content_type.to_string()));
            continue;
        }
        return Err(malformed(
            "content_types_unsupported_declaration",
            "[Content_Types].xml contains an unsupported declaration",
        ));
    }
    Ok(types)
}

fn invalid_default() -> Error {
    malformed(
        "content_types_invalid_default",
        "[Content_Types].xml contains an invalid Default declaration",
    )
}

fn invalid_override() -> Error {
    malformed(
        "content_types_invalid_override",
        "[Content_Types].xml contains an invalid Override declaration",
    )
}

/// Python `posixpath.splitext` extension (without the dot) of a part name.
pub fn extension(part_name: &str) -> &str {
    let filename = part_name.rsplit('/').next().unwrap_or(part_name);
    let stem_start = filename.len() - filename.trim_start_matches('.').len();
    match filename[stem_start..].rfind('.') {
        Some(dot) => &filename[stem_start + dot + 1..],
        None => "",
    }
}

impl ContentTypes {
    /// Content type for `part_name` (`/`-prefixed): override first, then the
    /// extension default; both case-insensitive (the first matching
    /// declaration wins).
    pub fn lookup(&self, part_name: &str) -> Option<&str> {
        if let Some(position) = self.index.overrides.get(&part_name.to_lowercase()) {
            return Some(&self.overrides[*position].1);
        }
        self.index
            .defaults
            .get(&extension(part_name).to_lowercase())
            .map(|position| self.defaults[*position].1.as_str())
    }
}

/// Normalized MIME type without parameters (upstream `media_type`).
pub fn media_type(content_type: &str) -> String {
    casefold(py_strip(content_type.split(';').next().unwrap_or("")))
}

/// Upstream `content_type_matches`.
pub fn content_type_matches(actual: &str, expected: &str) -> bool {
    media_type(actual) == media_type(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(body: &str) -> Result<ContentTypes> {
        parse_content_types(format!("<Types xmlns=\"{CT_NS}\">{body}</Types>").as_bytes())
    }

    #[test]
    fn splitext_semantics() {
        assert_eq!(extension("/word/document.xml"), "xml");
        assert_eq!(extension("/_rels/.rels"), "");
        assert_eq!(extension("/a/b.c.d"), "d");
        assert_eq!(extension("/a/..b"), "");
        assert_eq!(extension("/a/b"), "");
    }

    #[test]
    fn validates_declarations() {
        assert!(types("<Default Extension=\"xml\" ContentType=\"application/xml\"/>").is_ok());
        assert!(types("<Default Extension=\".xml\" ContentType=\"a\"/>").is_err());
        assert!(types("<Default Extension=\"xml \" ContentType=\"a\"/>").is_err());
        assert!(types("<Default Extension=\"xml\" ContentType=\"a\"/><Default Extension=\"XML\" ContentType=\"b\"/>").is_err());
        assert!(types("<Override PartName=\"word/x.xml\" ContentType=\"a\"/>").is_err());
        assert!(types("<Override PartName=\"/\" ContentType=\"a\"/>").is_err());
        assert!(types("<Override PartName=\"/A.xml\" ContentType=\"a\"/><Override PartName=\"/a.XML\" ContentType=\"a\"/>").is_err());
        assert!(types("<Other/>").is_err());
        assert!(types("<!-- ok --><?pi x?>").is_ok());
        let err = parse_content_types(b"<!DOCTYPE Types><Types/>").unwrap_err();
        assert_eq!(err.reason(), "content_types_dtd");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let parsed = types(
            "<Default Extension=\"XML\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/word/Document.xml\" ContentType=\"main\"/>",
        )
        .unwrap();
        assert_eq!(parsed.lookup("/word/document.xml"), Some("main"));
        assert_eq!(parsed.lookup("/word/styles.xml"), Some("application/xml"));
        assert_eq!(parsed.lookup("/word/media/a.png"), None);
    }
}
