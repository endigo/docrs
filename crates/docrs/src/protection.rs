//! Document-protection awareness (upstream `protection.py`).
//!
//! Word's Restrict Editing writes `w:documentProtection` into the settings
//! part. Mutating operations consult it per mode *and* per operation class
//! (the matrix below, measured in Word upstream except where noted) and
//! refuse with `document_protected`. Protection is advisory: the single
//! override is [`crate::Document::acknowledge_protection`], an in-memory flag
//! that is never persisted. This crate never strips the setting.

use serde::Serialize;

use crate::package::rels::rt;
use crate::wml::{w_attr, w_child};
use crate::{Document, Error, RefusalKind, Result};

/// Kind of edit being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationClass {
    /// Adding or editing comments.
    Comment,
    /// Setting a form-field / control value.
    FormFieldValue,
    /// Changing body content.
    BodyContent,
}

/// What `w:documentProtection` declares (report-only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtectionStatus {
    /// Raw `w:edit` token, if any.
    pub edit: Option<String>,
    /// `w:formatting` restriction.
    pub formatting: bool,
    /// Enforcement is active for an edit or formatting restriction.
    pub enforced: bool,
    /// The in-memory override flag.
    pub acknowledged: bool,
}

impl ProtectionStatus {
    /// Enforced and not acknowledged (ignores the operation class).
    pub fn blocks_paper_edits(&self) -> bool {
        self.enforced && !self.acknowledged
    }
}

fn is_on(value: Option<&str>) -> bool {
    matches!(
        value.map(str::to_lowercase).as_deref(),
        Some("1" | "true" | "on")
    )
}

struct Declared {
    edit: Option<String>,
    formatting: bool,
    enforcement: bool,
}

fn declared(document: &Document) -> Result<Option<Declared>> {
    let package = document.package();
    let Some(settings) = package.part_related_by(package.main_part_name(), rt::SETTINGS)? else {
        return Ok(None);
    };
    let tree = package.xml(&settings)?;
    let Some(element) = w_child(tree, tree.root(), "documentProtection") else {
        return Ok(None);
    };
    Ok(Some(Declared {
        edit: w_attr(tree, element, "edit").map(str::to_string),
        formatting: is_on(w_attr(tree, element, "formatting")),
        enforcement: is_on(w_attr(tree, element, "enforcement")),
    }))
}

/// Protection state of `document`.
pub fn status(document: &Document) -> Result<ProtectionStatus> {
    let acknowledged = document.protection_acknowledged();
    Ok(match declared(document)? {
        None => ProtectionStatus {
            edit: None,
            formatting: false,
            enforced: false,
            acknowledged,
        },
        Some(declared) => ProtectionStatus {
            enforced: (!matches!(declared.edit.as_deref(), None | Some("none"))
                || declared.formatting)
                && declared.enforcement,
            edit: declared.edit,
            formatting: declared.formatting,
            acknowledged,
        },
    })
}

/// Upstream `_PROTECTION_MATRIX`; `None` mode is the formatting-only row.
/// Two cells are unmeasured upstream and ship permissive: body content under
/// `trackedChanges` and under formatting-only protection.
fn permits(mode: Option<&str>, class: OperationClass) -> bool {
    use OperationClass::*;
    match (mode, class) {
        (Some("readOnly"), _) => false,
        (Some("comments"), Comment) => true,
        (Some("comments"), _) => false,
        (Some("trackedChanges"), FormFieldValue) => false,
        (Some("trackedChanges"), _) => true,
        (Some("forms"), FormFieldValue) => true,
        (Some("forms"), _) => false,
        (None, _) => true,
        // An unrecognised restriction is not a licence.
        (Some(_), _) => false,
    }
}

/// Refuse with `document_protected` when the restriction forbids `class`.
pub fn refuse_if_protected(
    document: &Document,
    operation: &str,
    class: OperationClass,
) -> Result<()> {
    if document.protection_acknowledged() {
        return Ok(());
    }
    let Some(declared) = declared(document)? else {
        return Ok(());
    };
    let unrestricted_edit = matches!(declared.edit.as_deref(), None | Some("none"));
    if (unrestricted_edit && !declared.formatting) || !declared.enforcement {
        return Ok(());
    }
    let mode = if unrestricted_edit {
        None
    } else {
        declared.edit.as_deref()
    };
    if permits(mode, class) {
        return Ok(());
    }
    let restriction = match mode {
        None => "formatting-only".to_string(),
        Some(edit) => format!("{edit:?} editing"),
    };
    Err(Error::refusal(
        RefusalKind::DocumentProtected,
        "restrict_editing",
        format!(
            "cannot {operation}: this document is under Restrict Editing ({restriction} protection, \
             w:documentProtection), and Word's own UI would not permit this edit either. The document \
             itself is fine — this is a policy restriction, not a defect in the file. Protection is \
             advisory, not security: if the edit is intended, acknowledge protection first; this \
             library never removes the protection setting itself"
        ),
    ))
}

/// `w:edit` tokens [`set_protection`] accepts.
pub const EDIT_MODES: &[&str] = &["readOnly", "comments", "trackedChanges", "forms"];

/// Elements that follow `w:documentProtection` in `CT_Settings`
/// (upstream `_DOCUMENT_PROTECTION_SUCCESSORS`), as `(namespace, local)`.
const DOCUMENT_PROTECTION_SUCCESSORS: &[(&str, &str)] = &[
    (W_NS, "autoFormatOverride"),
    (W_NS, "styleLockTheme"),
    (W_NS, "styleLockQFSet"),
    (W_NS, "defaultTabStop"),
    (W_NS, "autoHyphenation"),
    (W_NS, "consecutiveHyphenLimit"),
    (W_NS, "hyphenationZone"),
    (W_NS, "doNotHyphenateCaps"),
    (W_NS, "showEnvelope"),
    (W_NS, "summaryLength"),
    (W_NS, "clickAndTypeStyle"),
    (W_NS, "defaultTableStyle"),
    (W_NS, "evenAndOddHeaders"),
    (W_NS, "bookFoldRevPrinting"),
    (W_NS, "bookFoldPrinting"),
    (W_NS, "bookFoldPrintingSheets"),
    (W_NS, "drawingGridHorizontalSpacing"),
    (W_NS, "drawingGridVerticalSpacing"),
    (W_NS, "displayHorizontalDrawingGridEvery"),
    (W_NS, "displayVerticalDrawingGridEvery"),
    (W_NS, "doNotUseMarginsForDrawingGridOrigin"),
    (W_NS, "drawingGridHorizontalOrigin"),
    (W_NS, "drawingGridVerticalOrigin"),
    (W_NS, "doNotShadeFormData"),
    (W_NS, "noPunctuationKerning"),
    (W_NS, "characterSpacingControl"),
    (W_NS, "printTwoOnOne"),
    (W_NS, "strictFirstAndLastChars"),
    (W_NS, "noLineBreaksAfter"),
    (W_NS, "noLineBreaksBefore"),
    (W_NS, "savePreviewPicture"),
    (W_NS, "doNotValidateAgainstSchema"),
    (W_NS, "saveInvalidXml"),
    (W_NS, "ignoreMixedContent"),
    (W_NS, "alwaysShowPlaceholderText"),
    (W_NS, "doNotDemarcateInvalidXml"),
    (W_NS, "saveXmlDataOnly"),
    (W_NS, "useXSLTWhenSaving"),
    (W_NS, "saveThroughXslt"),
    (W_NS, "showXMLTags"),
    (W_NS, "alwaysMergeEmptyNamespace"),
    (W_NS, "updateFields"),
    (W_NS, "hdrShapeDefaults"),
    (W_NS, "footnotePr"),
    (W_NS, "endnotePr"),
    (W_NS, "compat"),
    (W_NS, "docVars"),
    (W_NS, "rsids"),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/math",
        "mathPr",
    ),
    (W_NS, "attachedSchema"),
    (W_NS, "themeFontLang"),
    (W_NS, "clrSchemeMapping"),
    (W_NS, "doNotIncludeSubdocsInStats"),
    (W_NS, "doNotAutoCompressPictures"),
    (W_NS, "forceUpgrade"),
    (W_NS, "captions"),
    (W_NS, "readModeInkLockDown"),
    (W_NS, "smartTagType"),
    (
        "http://schemas.openxmlformats.org/schemaLibrary/2006/main",
        "schemaLibrary",
    ),
    (W_NS, "shapeDefaults"),
    (W_NS, "doNotEmbedSmartTags"),
    (W_NS, "decimalSymbol"),
    (W_NS, "listSeparator"),
];

const W_NS: &str = crate::wml::W;

/// Turn Restrict Editing on (upstream `set_protection`): writes
/// `w:documentProtection` with `w:edit` and `w:enforcement="1"` (creating
/// the settings part if needed) and clears any acknowledgment. Never strips
/// a setting.
pub fn set_protection(document: &mut Document, edit: &str) -> Result<ProtectionStatus> {
    if !EDIT_MODES.contains(&edit) {
        return Err(Error::invalid_argument(
            "invalid_edit_mode",
            format!("edit must be one of {EDIT_MODES:?}, got {edit:?}"),
        ));
    }
    let mut txn = crate::txn::Txn::new(document);
    let settings = crate::parts::ensure_settings_part(&mut txn)?;
    let tree = txn.tree_mut(&settings)?;
    let root = tree.root();
    let element = match w_child(tree, root, "documentProtection") {
        Some(existing) => existing,
        None => {
            let element = tree.create_element(root, W_NS, "documentProtection")?;
            let position = tree.children(root).iter().position(|child| {
                DOCUMENT_PROTECTION_SUCCESSORS
                    .iter()
                    .any(|(ns, local)| tree.is(*child, ns, local))
            });
            let index = position.unwrap_or(tree.children(root).len());
            tree.insert(root, index, element)?;
            element
        }
    };
    tree.set_attr(element, Some(W_NS), "edit", edit)?;
    tree.set_attr(element, Some(W_NS), "enforcement", "1")?;
    let staged = txn.finish()?;
    document.commit(staged)?;
    document.set_protection_acknowledged(false);
    status(document)
}
