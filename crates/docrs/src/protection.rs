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
use crate::sections::insert_settings_child;
use crate::wml::{W, w_attr, w_child};
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
    let status = status(document)?;
    if !status.blocks_paper_edits() {
        return Ok(());
    }
    let mode = status.edit.as_deref().filter(|edit| *edit != "none");
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
            let element = tree.create_element(root, W, "documentProtection")?;
            insert_settings_child(tree, root, element, "documentProtection")?;
            element
        }
    };
    tree.set_attr(element, Some(W), "edit", edit)?;
    tree.set_attr(element, Some(W), "enforcement", "1")?;
    let staged = txn.finish()?;
    document.commit(staged)?;
    document.set_protection_acknowledged(false);
    status(document)
}
