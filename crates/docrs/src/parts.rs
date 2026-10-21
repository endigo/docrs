//! On-demand creation of standard parts, as python-docx does when an
//! operation touches a part the package lacks (templates from upstream
//! `src/docx/templates`, MIT; see `NOTICE.md`).

use crate::package::ct;
use crate::package::rels::rt;
use crate::txn::{RelTarget, Txn};
use crate::xml;
use crate::{Error, Result};

const DEFAULT_SETTINGS: &[u8] = include_bytes!("../assets/default-settings.xml");
const DEFAULT_STYLES: &[u8] = include_bytes!("../assets/default-styles.xml");

/// Parse a bundled template the way python-docx does (`remove_blank_text`):
/// whitespace-only text between elements is dropped. Used only for
/// templates, never for document content.
pub fn template(bytes: &[u8]) -> Result<xml::XmlTree> {
    let mut tree = xml::parse(bytes, &crate::Limits::default())
        .map_err(|err| Error::internal("template", err.to_string()))?;
    let blanks: Vec<xml::NodeId> = tree
        .descendants(tree.root())
        .filter(|node| matches!(tree.data(*node), xml::NodeData::Text(text) if text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))))
        .filter(|node| tree.parent(*node).is_some_and(|parent| tree.child_elements(parent).next().is_some()))
        .collect();
    for node in blanks {
        tree.detach(node)?;
    }
    Ok(tree)
}

/// The settings part related to the main document, created (as
/// `word/settings.xml` from the default template) when missing.
pub fn ensure_settings_part(txn: &mut Txn<'_>) -> Result<String> {
    ensure_part(
        txn,
        rt::SETTINGS,
        "word/settings.xml",
        ct::WML_SETTINGS,
        DEFAULT_SETTINGS,
    )
}

/// The styles part related to the main document, created (as
/// `word/styles.xml` from the default template) when missing.
pub fn ensure_styles_part(txn: &mut Txn<'_>) -> Result<String> {
    ensure_part(
        txn,
        rt::STYLES,
        "word/styles.xml",
        ct::WML_STYLES,
        DEFAULT_STYLES,
    )
}

/// Upstream `DocumentPart._<kind>_part` pattern: the related part of
/// `rel_type`, or a new part at `name` from `template_bytes` related to the
/// main document.
pub fn ensure_part(
    txn: &mut Txn<'_>,
    rel_type: &str,
    name: &str,
    content_type: &str,
    template_bytes: &[u8],
) -> Result<String> {
    let main = txn.document().package().main_part_name().to_string();
    if let Some(existing) = txn.part_related_by(&main, rel_type)? {
        return Ok(existing);
    }
    txn.add_xml_part(name, content_type, template(template_bytes)?)?;
    txn.relate_to(&main, rel_type, RelTarget::Part(name))?;
    Ok(name.to_string())
}
