//! Lossless, namespace-aware XML: parse, arena, serialize, compare.

pub mod chars;
pub mod decode;
pub mod equiv;
pub mod parse;
pub mod tree;
pub mod write;

pub use decode::SourceEncoding;
pub use equiv::trees_equivalent;
pub use parse::parse;
pub use tree::{Attr, Element, NodeData, NodeId, NsDecl, NsId, QName, XmlDecl, XmlTree};
#[cfg(feature = "markdown")]
pub(crate) use write::escape_attr_to_string;
pub use write::{serialize, serialize_node};

use crate::Error;

/// The `xml` prefix namespace.
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
/// The `xmlns` namespace (never declarable).
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
/// Markup Compatibility (ECMA-376 Part 3).
pub const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
/// XML Schema instance.
pub const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// Failure to parse an XML part.
#[derive(Debug)]
pub enum XmlError {
    /// The bytes are not well-formed, namespace-valid XML this crate accepts.
    Malformed {
        /// Stable sub-code.
        reason: &'static str,
        /// Human detail including line and column.
        message: String,
    },
    /// A resource budget was exceeded while parsing.
    Limit(Error),
}

impl XmlError {
    pub(crate) fn malformed(reason: &'static str, message: impl Into<String>) -> Self {
        XmlError::Malformed {
            reason,
            message: message.into(),
        }
    }

    /// Convert to a crate error for part `part`: malformed XML becomes a
    /// `malformed_package` refusal; budgets stay `resource_limit`.
    pub fn into_error(self, part: &str) -> Error {
        match self {
            XmlError::Malformed { reason, message } => Error::malformed(
                xml_reason(reason),
                format!("XML part {part:?} is not well-formed: {message}"),
            ),
            XmlError::Limit(error) => error.context(format!("parsing XML part {part:?}")),
        }
    }
}

fn xml_reason(reason: &'static str) -> &'static str {
    match reason {
        "dtd_prohibited" => "xml_dtd_prohibited",
        "unsupported_encoding" => "xml_unsupported_encoding",
        _ => "unparseable_xml_part",
    }
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XmlError::Malformed { reason, message } => write!(f, "{reason}: {message}"),
            XmlError::Limit(error) => write!(f, "{error}"),
        }
    }
}

/// Parse a standalone XML string with default limits (test/convenience helper).
pub fn parse_str(text: &str) -> Result<XmlTree, XmlError> {
    parse(text.as_bytes(), &crate::Limits::default())
}
