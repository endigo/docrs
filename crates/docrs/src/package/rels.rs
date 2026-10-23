//! Relationship parts (upstream `pkgreader._validate_relationship_records`,
//! `_SerializedRelationships`, `_rels.py`, `PackURI`).

use std::collections::HashMap;

use crate::xml::chars::is_ncname;
use crate::xml::{self, NodeData};

/// Relationships content type.
pub const RELS_CONTENT_TYPE: &str = "application/vnd.openxmlformats-package.relationships+xml";
use crate::{Error, Limits, Result};

/// The package-relationships namespace.
pub const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const TRANSITIONAL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
const STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships/";

/// Relationship type URIs (transitional spelling).
pub mod rt {
    /// Main document.
    pub const OFFICE_DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    /// Core properties.
    pub const CORE_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    /// Styles.
    pub const STYLES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    /// Settings.
    pub const SETTINGS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
    /// Numbering.
    pub const NUMBERING: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
    /// Font table.
    pub const FONT_TABLE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable";
    /// Header.
    pub const HEADER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
    /// Footer.
    pub const FOOTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
    /// Footnotes.
    pub const FOOTNOTES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
    /// Endnotes.
    pub const ENDNOTES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
    /// Comments.
    pub const COMMENTS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
    /// Theme.
    pub const THEME: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
    /// Image.
    pub const IMAGE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
    /// Hyperlink.
    pub const HYPERLINK: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
}

/// Upstream `is_relationship_type`: transitional and strict spellings match.
pub fn is_relationship_type(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }
    let suffix = expected
        .strip_prefix(TRANSITIONAL)
        .or_else(|| expected.strip_prefix(STRICT));
    match suffix {
        Some(suffix) => {
            actual.strip_prefix(TRANSITIONAL) == Some(suffix)
                || actual.strip_prefix(STRICT) == Some(suffix)
        }
        None => false,
    }
}

/// One relationship record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    /// `Id`.
    pub id: String,
    /// `Type`.
    pub rel_type: String,
    /// `Target` as written.
    pub target: String,
    /// `TargetMode="External"`.
    pub external: bool,
}

impl Relationship {
    /// Absolute part name of an internal target resolved against `base_uri`.
    pub fn target_part_name(&self, base_uri: &str) -> String {
        resolve_target(base_uri, &self.target)
    }
}

/// Python `posixpath.abspath(posixpath.join(base, reference))`.
pub fn resolve_target(base_uri: &str, reference: &str) -> String {
    let joined = if reference.starts_with('/') {
        reference.to_string()
    } else if base_uri.ends_with('/') {
        format!("{base_uri}{reference}")
    } else {
        format!("{base_uri}/{reference}")
    };
    normpath(&joined)
}

/// Python `posixpath.normpath` for an absolute path.
fn normpath(path: &str) -> String {
    let leading = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("{leading}{}", parts.join("/"))
}

/// Upstream `PackURI.relative_ref`: `part_name` relative to `base_uri`.
pub fn relative_ref(part_name: &str, base_uri: &str) -> String {
    if base_uri == "/" {
        return part_name.trim_start_matches('/').to_string();
    }
    let target: Vec<&str> = part_name.split('/').filter(|s| !s.is_empty()).collect();
    let base: Vec<&str> = base_uri.split('/').filter(|s| !s.is_empty()).collect();
    let common = target
        .iter()
        .zip(base.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<&str> = std::iter::repeat_n("..", base.len() - common).collect();
    parts.extend(&target[common..]);
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Directory portion of a part name (`PackURI.baseURI`).
pub fn base_uri(part_name: &str) -> &str {
    match part_name.rfind('/') {
        Some(0) => "/",
        Some(index) => &part_name[..index],
        None => "",
    }
}

/// Member name of the relationships part for `part_name` (`/` = package).
pub fn rels_member_for(part_name: &str) -> String {
    if part_name == "/" {
        return "_rels/.rels".to_string();
    }
    let base = base_uri(part_name);
    let filename = &part_name[part_name.rfind('/').map(|i| i + 1).unwrap_or(0)..];
    let base = base.trim_start_matches('/');
    if base.is_empty() {
        format!("_rels/{filename}.rels")
    } else {
        format!("{base}/_rels/{filename}.rels")
    }
}

fn malformed(reason: &'static str, message: impl Into<String>) -> Error {
    Error::malformed(reason, message)
}

/// Validate and parse a relationships part.
pub fn parse_relationships(data: &[u8], base: &str) -> Result<Vec<Relationship>> {
    let tree = xml::parse(data, &Limits::default()).map_err(|err| match err {
        xml::XmlError::Malformed {
            reason: "dtd_prohibited",
            ..
        } => malformed(
            "relationships_dtd",
            format!("relationships for {base:?} contain a prohibited DTD"),
        ),
        xml::XmlError::Malformed { .. } => malformed(
            "relationships_malformed",
            format!("relationships for {base:?} are malformed"),
        ),
        xml::XmlError::Limit(error) => error.context(format!("relationships for {base:?}")),
    })?;
    relationships_from_tree(&tree, base)
}

/// Validate and read relationships from a parsed relationships part.
pub fn relationships_from_tree(tree: &xml::XmlTree, base: &str) -> Result<Vec<Relationship>> {
    let root = tree.root();
    if !tree.is(root, RELS_NS, "Relationships") {
        return Err(malformed(
            "relationships_root",
            format!("relationships for {base:?} have an unexpected root element"),
        ));
    }
    let mut out: Vec<Relationship> = Vec::new();
    let mut ids: HashMap<&str, usize> = HashMap::new();
    for child in tree.children(root) {
        let NodeData::Element(_) = tree.data(*child) else {
            continue;
        };
        if !tree.is(*child, RELS_NS, "Relationship") {
            return Err(malformed(
                "relationships_unexpected_element",
                format!("relationships for {base:?} contain an unexpected element"),
            ));
        }
        let id = tree.attr(*child, None, "Id");
        let Some(id) = id.filter(|id| is_ncname(id)) else {
            return Err(malformed(
                "relationship_invalid_id",
                format!(
                    "relationship Id {:?} is not a valid XML ID",
                    id.unwrap_or("")
                ),
            ));
        };
        let target = tree.attr(*child, None, "Target");
        if let Some(first) = ids.get(id).map(|position| &out[*position]) {
            let second = target.unwrap_or("");
            let message = if first.target == second {
                format!(
                    "relationships for {base:?} declare Id {id:?} twice for the same target {second:?}; a \
                     relationships part may use each Id only once, so Word refuses to open the package \
                     even though the two declarations agree. Delete the repeated Relationship element \
                     and keep one"
                )
            } else {
                format!(
                    "relationships for {base:?} point Id {id:?} at two different targets, {:?} and \
                     {second:?}; nothing in the package says which target the Id means, so Word refuses \
                     to open it. Give the second relationship an Id of its own and repoint whatever \
                     refers to it",
                    first.target
                )
            };
            return Err(malformed("relationship_duplicate_id", message));
        }
        let rel_type = tree.attr(*child, None, "Type").unwrap_or("");
        let target = target.unwrap_or("");
        if rel_type.is_empty() || target.is_empty() {
            return Err(malformed(
                "relationship_missing_field",
                format!("relationship {id:?} is missing Type or Target"),
            ));
        }
        let external = match tree.attr(*child, None, "TargetMode").unwrap_or("Internal") {
            "Internal" => false,
            "External" => true,
            _ => {
                return Err(malformed(
                    "relationship_invalid_target_mode",
                    format!("relationship {id:?} has invalid TargetMode"),
                ));
            }
        };
        ids.insert(id, out.len());
        out.push(Relationship {
            id: id.to_string(),
            rel_type: rel_type.to_string(),
            target: target.to_string(),
            external,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_resolution_matches_posixpath() {
        assert_eq!(
            resolve_target("/word", "document.xml"),
            "/word/document.xml"
        );
        assert_eq!(resolve_target("/word", "../media/x.png"), "/media/x.png");
        assert_eq!(
            resolve_target("/", "word/document.xml"),
            "/word/document.xml"
        );
        assert_eq!(
            resolve_target("/word", "/word/document.xml"),
            "/word/document.xml"
        );
        assert_eq!(resolve_target("/word", "//a//b"), "//a/b");
        assert_eq!(resolve_target("/word", "../../../x"), "/x");
        assert_eq!(resolve_target("/word", "http://x/y"), "/word/http:/x/y");
    }

    #[test]
    fn rels_member_names() {
        assert_eq!(rels_member_for("/"), "_rels/.rels");
        assert_eq!(
            rels_member_for("/word/document.xml"),
            "word/_rels/document.xml.rels"
        );
        assert_eq!(rels_member_for("/x.xml"), "_rels/x.xml.rels");
    }

    #[test]
    fn strict_and_transitional_types_match() {
        assert!(is_relationship_type(
            "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
            rt::OFFICE_DOCUMENT
        ));
        assert!(!is_relationship_type(rt::STYLES, rt::OFFICE_DOCUMENT));
    }

    #[test]
    fn duplicate_ids_and_modes_refuse() {
        let rels = |body: &str| {
            parse_relationships(
                format!("<Relationships xmlns=\"{RELS_NS}\">{body}</Relationships>").as_bytes(),
                "/",
            )
        };
        assert!(rels("<Relationship Id=\"rId1\" Type=\"t\" Target=\"a\"/>").is_ok());
        assert_eq!(
            rels("<Relationship Id=\"rId1\" Type=\"t\" Target=\"a\"/><Relationship Id=\"rId1\" Type=\"t\" Target=\"a\"/>")
                .unwrap_err()
                .reason(),
            "relationship_duplicate_id"
        );
        assert!(rels("<Relationship Id=\"1\" Type=\"t\" Target=\"a\"/>").is_err());
        assert!(rels("<Relationship Id=\"r\" Type=\"t\" Target=\"a\" TargetMode=\"x\"/>").is_err());
        assert!(rels("<Relationship Id=\"r\" Type=\"\" Target=\"a\"/>").is_err());
        assert!(rels("<Other/>").is_err());
    }
}
