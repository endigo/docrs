//! Comment thread operations: anchored text, replies, resolution, deletion
//! (upstream `commentops.py`, MIT; see `NOTICE.md`).
//!
//! Word models threading and resolution outside `word/comments.xml`, in
//! `word/commentsExtended.xml`: one `w15:commentEx` per comment, keyed by
//! the `w14:paraId` of the comment's last paragraph, carrying `w15:done`
//! (resolved) and `w15:paraIdParent` (reply-of). Modern identity rows live in
//! `word/commentsIds.xml` (`w16cid`) and `word/commentsExtensible.xml`
//! (`w16cex`). Every mutation validates first and stages all writes in one
//! [`Txn`], so refusals change nothing.

use serde::Serialize;

use crate::clock::Timestamp;
use crate::comments::{self, Comment};
use crate::package::{ct, rels::rt};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::revision::{W15, iter_w, py_int, py_opt_repr, py_repr, w_local};
use crate::story::{self, is_direct_run_child, project_run_child};
use crate::txn::{RelTarget, Txn};
use crate::wml::{W, W14, w_attr};
use crate::xml::{self, NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// `commentsExtended` content type.
pub const COMMENTS_EXTENDED_CONTENT_TYPE: &str = ct::WML_COMMENTS_EXTENDED;
/// `commentsExtended` relationship type.
pub const COMMENTS_EXTENDED_RELATIONSHIP_TYPE: &str =
    "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
/// `commentsIds` content type.
pub const COMMENTS_IDS_CONTENT_TYPE: &str = ct::WML_COMMENTS_IDS;
/// `commentsIds` relationship type.
pub const COMMENTS_IDS_RELATIONSHIP_TYPE: &str =
    "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds";
/// `commentsExtensible` content type.
pub const COMMENTS_EXTENSIBLE_CONTENT_TYPE: &str = ct::WML_COMMENTS_EXTENSIBLE;
/// `commentsExtensible` relationship type.
pub const COMMENTS_EXTENSIBLE_RELATIONSHIP_TYPE: &str =
    "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible";

pub(crate) const W16CID: &str = "http://schemas.microsoft.com/office/word/2016/wordml/cid";
pub(crate) const W16CEX: &str = "http://schemas.microsoft.com/office/word/2018/wordml/cex";

const COMMENTS_PARTNAME: &str = "word/comments.xml";
const COMMENTS_EXTENDED_PARTNAME: &str = "word/commentsExtended.xml";
const COMMENTS_IDS_PARTNAME: &str = "word/commentsIds.xml";
const COMMENTS_EXTENSIBLE_PARTNAME: &str = "word/commentsExtensible.xml";

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

fn not_found(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::TargetNotFound, reason, message)
}

fn main_of(txn: &Txn<'_>) -> String {
    txn.document().package().main_part_name().to_string()
}

fn occupied(txn: &Txn<'_>, name: &str) -> bool {
    txn.resolve(name).is_some()
}

/// The related comments part, refusing an ambiguous relationship with
/// `purpose` in the message.
pub(crate) fn comments_part(txn: &Txn<'_>, purpose: &str) -> Result<Option<String>> {
    match txn.part_related_by(&main_of(txn), rt::COMMENTS) {
        Ok(part) => Ok(part),
        Err(err) if err.reason() == "ambiguous_relationship" => Err(unsupported(
            "ambiguous_relationship",
            format!(
                "multiple comments relationships make {purpose} ambiguous; nothing was changed"
            ),
        )),
        Err(err) => Err(err),
    }
}

fn is_live(txn: &Txn<'_>, part: &str) -> bool {
    txn.content_type(part)
        .is_some_and(|ct| ct::EAGER_XML.contains(&ct.as_str()))
}

/// Upstream `_preflight_comment_add`.
pub(crate) fn preflight_comment_add(txn: &Txn<'_>) -> Result<()> {
    let part = match comments_part(txn, "comment authoring")? {
        Some(part) => part,
        None => {
            if occupied(txn, COMMENTS_PARTNAME) {
                return Err(unsupported(
                    "comments_part_name_taken",
                    "a package part already occupies /word/comments.xml without the document comments \
                     relationship; nothing was changed",
                ));
            }
            return Ok(());
        }
    };
    if !is_live(txn, &part) || !txn.tree(&part).is_ok_and(|t| t.is(t.root(), W, "comments")) {
        return Err(unsupported(
            "comments_part_not_live",
            "the comments relationship does not target a live w:comments part; nothing was changed",
        ));
    }
    let tree = txn.tree(&part)?;
    let mut seen = Vec::new();
    for comment in tree.child_elements(tree.root()) {
        if w_local(tree, comment) != Some("comment") {
            continue;
        }
        let raw = w_attr(tree, comment, "id");
        let Some(id) = raw.and_then(py_int) else {
            return Err(unsupported(
                "malformed_comment_id",
                format!(
                    "malformed comment w:id={}; nothing was changed",
                    py_opt_repr(raw)
                ),
            ));
        };
        if seen.contains(&id) {
            return Err(unsupported(
                "duplicate_comment_id",
                format!("duplicate comment w:id={id}; nothing was changed"),
            ));
        }
        seen.push(id);
    }
    Ok(())
}

/// Upstream `_preflight_comment_range` (the protection gate included).
pub(crate) fn preflight_comment_range(
    document: &Document,
    tree: &XmlTree,
    first_run: NodeId,
    last_run: NodeId,
    operation: &str,
) -> Result<()> {
    refuse_if_protected(document, operation, OperationClass::Comment)?;
    let nodes: Vec<NodeId> = crate::wml::iter_nodes(tree, tree.root()).collect();
    let position = |id: NodeId| nodes.iter().position(|node| *node == id);
    let (Some(first), Some(last)) = (position(first_run), position(last_run)) else {
        return Err(not_found(
            "stale_comment_range",
            "comment range runs are stale or detached; reacquire the span",
        ));
    };
    if first > last {
        return Err(unsupported(
            "reversed_comment_range",
            "comment range endpoints are reversed; nothing was changed",
        ));
    }
    let interval = &nodes[first..=last];
    let selected: Vec<NodeId> = interval
        .iter()
        .copied()
        .filter(|node| w_local(tree, *node) == Some("r"))
        .collect();
    if selected
        .iter()
        .any(|run| iter_w(tree, *run, &["fldChar"]).next().is_some())
    {
        return Err(unsupported(
            "comment_across_field",
            "cannot anchor a comment across a complex field boundary; nothing was changed",
        ));
    }
    let mut depth = 0usize;
    for node in &nodes {
        if w_local(tree, *node) == Some("fldChar") {
            match w_attr(tree, *node, "fldCharType") {
                Some("begin") => depth += 1,
                Some("end") if depth > 0 => depth -= 1,
                _ => {}
            }
        }
        if depth > 0 && selected.contains(node) {
            return Err(unsupported(
                "comment_inside_field",
                "cannot anchor a comment inside a complex field; nothing was changed",
            ));
        }
    }
    for run in &selected {
        if tree
            .ancestors(*run)
            .any(|ancestor| w_local(tree, ancestor) == Some("fldSimple"))
        {
            return Err(unsupported(
                "comment_inside_field",
                "cannot anchor a comment inside a field; nothing was changed",
            ));
        }
    }
    let mut controls: Vec<NodeId> = Vec::new();
    for node in interval {
        for candidate in std::iter::once(*node).chain(tree.ancestors(*node)) {
            if w_local(tree, candidate) == Some("sdt") && !controls.contains(&candidate) {
                controls.push(candidate);
            }
        }
    }
    for control in controls {
        refuse_control_write_restrictions(tree, control)?;
    }
    Ok(())
}

/// Upstream `controls._refuse_control_write_restrictions`.
fn refuse_control_write_restrictions(tree: &XmlTree, sdt: NodeId) -> Result<()> {
    let Some(sdt_pr) = tree.first_child_named(sdt, W, "sdtPr") else {
        return Ok(());
    };
    if tree.first_child_named(sdt_pr, W, "dataBinding").is_some() {
        return Err(unsupported(
            "data_bound_control",
            "control is data-bound; edits could be discarded on sync",
        ));
    }
    let locks: Vec<NodeId> = crate::wml::w_children(tree, sdt_pr, "lock").collect();
    if locks.len() > 1 {
        return Err(unsupported(
            "multiple_locks",
            "control has multiple w:lock declarations; nothing was changed",
        ));
    }
    if locks.first().is_some_and(|lock| {
        matches!(
            w_attr(tree, *lock, "val"),
            Some("contentLocked" | "sdtContentLocked")
        )
    }) {
        return Err(unsupported(
            "locked_control",
            "control content is locked; nothing was changed",
        ));
    }
    Ok(())
}

/// Upstream `_validate_para_id`: an 8-digit hex id, upper-cased.
pub(crate) fn validate_para_id(raw: Option<&str>, attribute: &str) -> Result<String> {
    match raw {
        Some(value)
            if value.chars().count() == 8 && value.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            Ok(value.to_ascii_uppercase())
        }
        _ => Err(unsupported(
            "malformed_para_id",
            format!(
                "malformed {attribute}={}; nothing was changed",
                py_opt_repr(raw)
            ),
        )),
    }
}

/// Upstream `_last_paragraph`: the last `w:p` anywhere inside the comment.
pub(crate) fn last_paragraph(tree: &XmlTree, comment: NodeId) -> Option<NodeId> {
    iter_w(tree, comment, &["p"]).last()
}

fn require_last_paragraph(tree: &XmlTree, comment: NodeId) -> Result<NodeId> {
    last_paragraph(tree, comment).ok_or_else(|| {
        unsupported(
            "comment_without_paragraph",
            "comment has no paragraph to key on",
        )
    })
}

fn existing_para_ids(tree: &XmlTree) -> Result<Vec<u32>> {
    let mut values = Vec::new();
    for paragraph in iter_w(tree, tree.root(), &["p"]) {
        if let Some(raw) = tree.attr(paragraph, Some(W14), "paraId") {
            let value = validate_para_id(Some(raw), "w14:paraId")?;
            values.push(u32::from_str_radix(&value, 16).unwrap_or(0));
        }
    }
    let mut sorted = values.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != values.len() {
        return Err(unsupported(
            "duplicate_para_id",
            "duplicate w14:paraId values make comment threading ambiguous; nothing was changed",
        ));
    }
    Ok(values)
}

/// Upstream `_ensure_para_id`.
pub(crate) fn ensure_para_id(
    txn: &mut Txn<'_>,
    comments: &str,
    paragraph: NodeId,
) -> Result<String> {
    let tree = txn.tree(comments)?;
    if let Some(existing) = tree.attr(paragraph, Some(W14), "paraId") {
        return validate_para_id(Some(existing), "w14:paraId");
    }
    let next = u64::from(
        existing_para_ids(tree)?
            .into_iter()
            .max()
            .unwrap_or(0x1000_0000),
    ) + 1;
    // A w14:paraId is non-zero and below 0x80000000.
    if next >= 0x8000_0000 {
        return Err(Error::invalid_argument(
            "para_id_exhausted",
            format!(
                "the highest w14:paraId in the comments part is {:08X}; the next one would reach \
                 0x80000000, which Word does not accept. Nothing was changed",
                next - 1
            ),
        ));
    }
    let para_id = format!("{next:08X}");
    txn.tree_mut(comments)?
        .set_attr(paragraph, Some(W14), "paraId", &para_id)?;
    Ok(para_id)
}

fn new_part_from(template: &str) -> Result<XmlTree> {
    xml::parse_str(template).map_err(|err| Error::internal("comment_template", err.to_string()))
}

/// Upstream `_comments_extended_root`.
pub(crate) fn comments_extended_root(txn: &mut Txn<'_>, create: bool) -> Result<Option<String>> {
    let existing = validate_comments_extended(txn)?;
    if existing.is_some() || !create {
        return Ok(existing);
    }
    if occupied(txn, COMMENTS_EXTENDED_PARTNAME) {
        return Err(unsupported(
            "comments_extended_name_taken",
            "a package part already occupies /word/commentsExtended.xml without the commentsExtended \
             relationship; nothing was changed",
        ));
    }
    let tree = new_part_from(&format!(
        "<w15:commentsEx xmlns:w15=\"{W15}\" xmlns:w14=\"{W14}\"/>"
    ))?;
    txn.add_xml_part(
        COMMENTS_EXTENDED_PARTNAME,
        COMMENTS_EXTENDED_CONTENT_TYPE,
        tree,
    )?;
    let main = main_of(txn);
    txn.relate_to(
        &main,
        COMMENTS_EXTENDED_RELATIONSHIP_TYPE,
        RelTarget::Part(COMMENTS_EXTENDED_PARTNAME),
    )?;
    Ok(Some(COMMENTS_EXTENDED_PARTNAME.to_string()))
}

/// The validating half of `_comments_extended_root(create=False)`.
pub(crate) fn validate_comments_extended(txn: &Txn<'_>) -> Result<Option<String>> {
    let part = match txn.part_related_by(&main_of(txn), COMMENTS_EXTENDED_RELATIONSHIP_TYPE) {
        Ok(part) => part,
        Err(err) if err.reason() == "ambiguous_relationship" => {
            return Err(unsupported(
                "ambiguous_relationship",
                "multiple commentsExtended relationships make comment threading ambiguous; nothing was changed",
            ));
        }
        Err(err) => return Err(err),
    };
    let Some(part) = part else { return Ok(None) };
    if !is_live(txn, &part) {
        return Err(unsupported(
            "comments_extended_opaque",
            "commentsExtended part loaded as an opaque blob; the package registration is missing, and writing \
             to a parsed copy would be silently lost on save",
        ));
    }
    let tree = txn.tree(&part)?;
    if !tree.is(tree.root(), W15, "commentsEx") {
        return Err(unsupported(
            "comments_extended_root",
            "commentsExtended has an unexpected root element; nothing was changed",
        ));
    }
    let mut seen: Vec<String> = Vec::new();
    for entry in tree.child_elements(tree.root()) {
        if !tree.is(entry, W15, "commentEx") {
            continue;
        }
        let para_id = validate_para_id(tree.attr(entry, Some(W15), "paraId"), "w15:paraId")?;
        if let Some(parent) = tree.attr(entry, Some(W15), "paraIdParent") {
            validate_para_id(Some(parent), "w15:paraIdParent")?;
        }
        if seen.contains(&para_id) {
            return Err(unsupported(
                "duplicate_para_id",
                "duplicate w15:paraId values make comment threading ambiguous; nothing was changed",
            ));
        }
        seen.push(para_id);
    }
    Ok(Some(part))
}

struct IdentityPart {
    rel_type: &'static str,
    partname: &'static str,
    content_type: &'static str,
    ns: &'static str,
    root: &'static str,
    prefix: &'static str,
}

const IDS_PART: IdentityPart = IdentityPart {
    rel_type: COMMENTS_IDS_RELATIONSHIP_TYPE,
    partname: COMMENTS_IDS_PARTNAME,
    content_type: COMMENTS_IDS_CONTENT_TYPE,
    ns: W16CID,
    root: "commentsIds",
    prefix: "w16cid",
};

const EXTENSIBLE_PART: IdentityPart = IdentityPart {
    rel_type: COMMENTS_EXTENSIBLE_RELATIONSHIP_TYPE,
    partname: COMMENTS_EXTENSIBLE_PARTNAME,
    content_type: COMMENTS_EXTENSIBLE_CONTENT_TYPE,
    ns: W16CEX,
    root: "commentsExtensible",
    prefix: "w16cex",
};

/// Upstream `_xml_part_root`.
fn xml_part_root(txn: &mut Txn<'_>, spec: &IdentityPart, create: bool) -> Result<Option<String>> {
    let shown = format!("/{}", spec.partname);
    let part = match txn.part_related_by(&main_of(txn), spec.rel_type) {
        Ok(part) => part,
        Err(err) if err.reason() == "ambiguous_relationship" => {
            return Err(unsupported(
                "ambiguous_relationship",
                format!(
                    "multiple {shown} relationships make comment identity ambiguous; nothing was changed"
                ),
            ));
        }
        Err(err) => return Err(err),
    };
    if let Some(part) = part {
        let ok = is_live(txn, &part)
            && txn
                .tree(&part)
                .is_ok_and(|t| t.is(t.root(), spec.ns, spec.root));
        if !ok {
            return Err(unsupported(
                "identity_part_root",
                format!("{shown} has an unexpected root; nothing was changed"),
            ));
        }
        return Ok(Some(part));
    }
    if !create {
        return Ok(None);
    }
    if occupied(txn, spec.partname) {
        return Err(unsupported(
            "identity_part_name_taken",
            format!(
                "a package part already occupies {shown} without the expected relationship; nothing was changed"
            ),
        ));
    }
    let tree = new_part_from(&format!(
        "<{p}:{r} xmlns:{p}=\"{ns}\"/>",
        p = spec.prefix,
        r = spec.root,
        ns = spec.ns
    ))?;
    txn.add_xml_part(spec.partname, spec.content_type, tree)?;
    let main = main_of(txn);
    txn.relate_to(&main, spec.rel_type, RelTarget::Part(spec.partname))?;
    Ok(Some(spec.partname.to_string()))
}

fn append_entry(
    tree: &mut XmlTree,
    ns: &str,
    local: &str,
    attrs: &[(&str, &str)],
) -> Result<NodeId> {
    let root = tree.root();
    let entry = tree.create_element(root, ns, local)?;
    let end = tree.children(root).len();
    tree.insert(root, end, entry)?;
    for (name, value) in attrs {
        tree.set_attr(entry, Some(ns), name, value)?;
    }
    Ok(entry)
}

/// Upstream `_ensure_comment_identity`.
pub(crate) fn ensure_comment_identity(
    txn: &mut Txn<'_>,
    comments: &str,
    comment: NodeId,
) -> Result<()> {
    let last = require_last_paragraph(txn.tree(comments)?, comment)?;
    let para_id = ensure_para_id(txn, comments, last)?;
    let ids = xml_part_root(txn, &IDS_PART, true)?
        .ok_or_else(|| Error::internal("comments_ids", "commentsIds part missing"))?;
    let (existing, used) = {
        let tree = txn.tree(&ids)?;
        let rows: Vec<NodeId> = tree
            .child_elements(tree.root())
            .filter(|entry| tree.is(*entry, W16CID, "commentId"))
            .collect();
        let upper = |entry: NodeId, name: &str| {
            tree.attr(entry, Some(W16CID), name)
                .unwrap_or("")
                .to_ascii_uppercase()
        };
        let existing = rows
            .iter()
            .copied()
            .find(|entry| upper(*entry, "paraId") == para_id)
            .map(|entry| {
                let durable = upper(entry, "durableId");
                if durable.is_empty() {
                    para_id.clone()
                } else {
                    durable
                }
            });
        let used: Vec<String> = rows
            .iter()
            .map(|entry| upper(*entry, "durableId"))
            .collect();
        (existing, used)
    };
    let durable_id = match existing {
        Some(durable) => durable,
        None => {
            let mut durable = para_id.clone();
            let mut nonce = u32::from_str_radix(&para_id, 16).unwrap_or(0);
            while used.contains(&durable) {
                nonce = nonce.wrapping_add(1);
                durable = format!("{nonce:08X}");
            }
            append_entry(
                txn.tree_mut(&ids)?,
                W16CID,
                "commentId",
                &[("paraId", &para_id), ("durableId", &durable)],
            )?;
            durable
        }
    };
    let cex = xml_part_root(txn, &EXTENSIBLE_PART, true)?
        .ok_or_else(|| Error::internal("comments_extensible", "commentsExtensible part missing"))?;
    let present = {
        let tree = txn.tree(&cex)?;
        tree.child_elements(tree.root()).any(|entry| {
            tree.is(entry, W16CEX, "commentExtensible")
                && tree
                    .attr(entry, Some(W16CEX), "durableId")
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    == durable_id
        })
    };
    if !present {
        append_entry(
            txn.tree_mut(&cex)?,
            W16CEX,
            "commentExtensible",
            &[("durableId", &durable_id)],
        )?;
    }
    Ok(())
}

/// Upstream `_preflight_comments_extended_write`.
pub(crate) fn preflight_comments_extended_write(txn: &Txn<'_>) -> Result<()> {
    let root = validate_comments_extended(txn)?;
    if root.is_none() && occupied(txn, COMMENTS_EXTENDED_PARTNAME) {
        return Err(unsupported(
            "comments_extended_name_taken",
            "a package part already occupies /word/commentsExtended.xml without the commentsExtended \
             relationship; nothing was changed",
        ));
    }
    Ok(())
}

/// Upstream `_retarget_comment_identity`.
fn retarget_comment_identity(txn: &mut Txn<'_>, previous: &str, current: &str) -> Result<()> {
    let Some(ids) = xml_part_root(txn, &IDS_PART, false)? else {
        return Ok(());
    };
    let wanted = previous.to_ascii_uppercase();
    let target = {
        let tree = txn.tree(&ids)?;
        tree.child_elements(tree.root()).find(|entry| {
            tree.is(*entry, W16CID, "commentId")
                && tree
                    .attr(*entry, Some(W16CID), "paraId")
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    == wanted
        })
    };
    if let Some(entry) = target {
        txn.tree_mut(&ids)?
            .set_attr(entry, Some(W16CID), "paraId", current)?;
    }
    Ok(())
}

/// Upstream `_remove_comment_identity_rows`.
pub(crate) fn remove_comment_identity_rows(
    txn: &mut Txn<'_>,
    _main: &str,
    para_ids: &[String],
) -> Result<()> {
    let wanted: Vec<String> = para_ids
        .iter()
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let ids = xml_part_root(txn, &IDS_PART, false)?;
    let mut durable_ids: Vec<String> = Vec::new();
    if let Some(ids) = ids {
        let doomed: Vec<NodeId> = {
            let tree = txn.tree(&ids)?;
            tree.child_elements(tree.root())
                .filter(|entry| tree.is(*entry, W16CID, "commentId"))
                .filter(|entry| {
                    let para = tree
                        .attr(*entry, Some(W16CID), "paraId")
                        .unwrap_or("")
                        .to_ascii_uppercase();
                    if wanted.contains(&para) {
                        let durable = tree
                            .attr(*entry, Some(W16CID), "durableId")
                            .unwrap_or("")
                            .to_ascii_uppercase();
                        if !durable.is_empty() {
                            durable_ids.push(durable);
                        }
                        true
                    } else {
                        false
                    }
                })
                .collect()
        };
        let tree = txn.tree_mut(&ids)?;
        for entry in doomed {
            tree.detach(entry)?;
        }
    }
    let cex = xml_part_root(txn, &EXTENSIBLE_PART, false)?;
    let Some(cex) = cex else { return Ok(()) };
    if durable_ids.is_empty() {
        return Ok(());
    }
    let doomed: Vec<NodeId> = {
        let tree = txn.tree(&cex)?;
        tree.child_elements(tree.root())
            .filter(|entry| {
                tree.is(*entry, W16CEX, "commentExtensible")
                    && durable_ids.contains(
                        &tree
                            .attr(*entry, Some(W16CEX), "durableId")
                            .unwrap_or("")
                            .to_ascii_uppercase(),
                    )
            })
            .collect()
    };
    let tree = txn.tree_mut(&cex)?;
    for entry in doomed {
        tree.detach(entry)?;
    }
    Ok(())
}

/// Upstream `_migrate_comment_extension`.
pub(crate) fn migrate_comment_extension(
    txn: &mut Txn<'_>,
    comments: &str,
    comment: NodeId,
    previous_last: NodeId,
) -> Result<()> {
    let current_last = require_last_paragraph(txn.tree(comments)?, comment)?;
    if current_last == previous_last {
        return Ok(());
    }
    let Some(previous_raw) = txn
        .tree(comments)?
        .attr(previous_last, Some(W14), "paraId")
        .map(str::to_string)
    else {
        return Ok(());
    };
    let previous_id = validate_para_id(Some(&previous_raw), "w14:paraId")?;
    let current_id = ensure_para_id(txn, comments, current_last)?;
    retarget_comment_identity(txn, &previous_id, &current_id)?;
    let Some(root) = comments_extended_root(txn, false)? else {
        return Ok(());
    };
    let (own, children) = {
        let tree = txn.tree(&root)?;
        let own = entry_for(tree, &previous_id)?;
        let mut children = Vec::new();
        for entry in tree.child_elements(tree.root()) {
            if !tree.is(entry, W15, "commentEx") {
                continue;
            }
            if let Some(parent) = tree.attr(entry, Some(W15), "paraIdParent")
                && validate_para_id(Some(parent), "w15:paraIdParent")? == previous_id
            {
                children.push(entry);
            }
        }
        (own, children)
    };
    let tree = txn.tree_mut(&root)?;
    if let Some(entry) = own {
        tree.set_attr(entry, Some(W15), "paraId", &current_id)?;
    }
    for entry in children {
        tree.set_attr(entry, Some(W15), "paraIdParent", &current_id)?;
    }
    Ok(())
}

/// Upstream `_entry_for(create=False)`.
fn entry_for(tree: &XmlTree, para_id: &str) -> Result<Option<NodeId>> {
    let normalized = validate_para_id(Some(para_id), "paragraph id")?;
    for entry in tree.child_elements(tree.root()) {
        if !tree.is(entry, W15, "commentEx") {
            continue;
        }
        let existing = validate_para_id(tree.attr(entry, Some(W15), "paraId"), "w15:paraId")?;
        if existing == normalized {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

/// Upstream `_entry_for(create=True)`.
fn entry_for_create(txn: &mut Txn<'_>, root: &str, para_id: &str) -> Result<NodeId> {
    if let Some(entry) = entry_for(txn.tree(root)?, para_id)? {
        return Ok(entry);
    }
    let normalized = validate_para_id(Some(para_id), "paragraph id")?;
    append_entry(
        txn.tree_mut(root)?,
        W15,
        "commentEx",
        &[("paraId", &normalized), ("done", "0")],
    )
}

// ---------------------------------------------------------------------------
// Ownership
// ---------------------------------------------------------------------------

/// Upstream `require_comment_owner`: the comment's element in the live
/// comments part.
pub(crate) fn comment_element(txn: &Txn<'_>, comment: &Comment) -> Result<(String, NodeId)> {
    let expected = comments_part(txn, "comment ownership")?;
    let foreign = || {
        Error::refusal(
            RefusalKind::BoundaryViolation,
            "foreign_comment",
            "comment belongs to a different document; select the comment from the document passed to this operation",
        )
    };
    let Some(expected) = expected else {
        return Err(foreign());
    };
    if comment.document != txn.document().id() || comment.part != expected {
        return Err(foreign());
    }
    let tree = txn.tree(&expected)?;
    if !tree.contains(comment.element) || tree.parent(comment.element) != Some(tree.root()) {
        return Err(not_found(
            "stale_comment",
            "comment is stale: its comment was removed from the document",
        ));
    }
    Ok((expected, comment.element))
}

// ---------------------------------------------------------------------------
// Public operations
// ---------------------------------------------------------------------------

fn last_para_id(tree: &XmlTree, comment: NodeId) -> Result<Option<String>> {
    let last = require_last_paragraph(tree, comment)?;
    Ok(tree.attr(last, Some(W14), "paraId").map(str::to_string))
}

/// Whether `comment`'s thread is marked resolved (`w15:done`).
pub fn is_resolved(document: &Document, comment: &Comment) -> Result<bool> {
    let mut txn = Txn::new(document);
    let (part, element) = comment_element(&txn, comment)?;
    let Some(root) = comments_extended_root(&mut txn, false)? else {
        return Ok(false);
    };
    let Some(para_id) = last_para_id(txn.tree(&part)?, element)?.filter(|p| !p.is_empty()) else {
        return Ok(false);
    };
    let tree = txn.tree(&root)?;
    Ok(entry_for(tree, &para_id)?
        .is_some_and(|entry| matches!(tree.attr(entry, Some(W15), "done"), Some("1" | "true"))))
}

/// Mark `comment`'s thread resolved (or reopen it with `resolved = false`).
pub fn resolve(document: &mut Document, comment: &Comment, resolved: bool) -> Result<()> {
    let staged = {
        let mut txn = Txn::new(document);
        let (part, element) = comment_element(&txn, comment)?;
        refuse_if_protected(document, "resolve a comment", OperationClass::Comment)?;
        preflight_comments_extended_write(&txn)?;
        let last = require_last_paragraph(txn.tree(&part)?, element)?;
        let para_id = ensure_para_id(&mut txn, &part, last)?;
        let root = comments_extended_root(&mut txn, true)?
            .ok_or_else(|| Error::internal("comments_extended", "commentsExtended missing"))?;
        let entry = entry_for_create(&mut txn, &root, &para_id)?;
        txn.tree_mut(&root)?.set_attr(
            entry,
            Some(W15),
            "done",
            if resolved { "1" } else { "0" },
        )?;
        txn.finish()?
    };
    document.commit(staged)
}

/// The id of the comment `comment` replies to, or `None` for a thread start.
pub fn parent_of(document: &Document, comment: &Comment) -> Result<Option<i64>> {
    let mut txn = Txn::new(document);
    parent_of_in(&mut txn, comment)
}

fn parent_of_in(txn: &mut Txn<'_>, comment: &Comment) -> Result<Option<i64>> {
    let (part, element) = comment_element(txn, comment)?;
    let Some(root) = comments_extended_root(txn, false)? else {
        return Ok(None);
    };
    let Some(para_id) = last_para_id(txn.tree(&part)?, element)?.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let raw_parent = {
        let tree = txn.tree(&root)?;
        entry_for(tree, &para_id)?.and_then(|entry| {
            tree.attr(entry, Some(W15), "paraIdParent")
                .map(str::to_string)
        })
    };
    let Some(raw_parent) = raw_parent.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let parent_para = validate_para_id(Some(&raw_parent), "w15:paraIdParent")?;
    for candidate in comments::comments_in(txn, &part)?.iter() {
        let (cpart, celement) = comment_element(txn, candidate)?;
        let raw = last_para_id(txn.tree(&cpart)?, celement)?;
        if let Some(raw) = raw
            && validate_para_id(Some(&raw), "w14:paraId")? == parent_para
        {
            return Ok(Some(candidate.comment_id));
        }
    }
    Ok(None)
}

/// Upstream `_anchor_elements`: (start, end, reference run) in the body.
fn anchor_elements(
    tree: &XmlTree,
    comment_id: i64,
) -> Result<(Option<NodeId>, Option<NodeId>, Option<NodeId>)> {
    let body = tree
        .first_child_named(tree.root(), W, "body")
        .ok_or_else(|| unsupported("no_body", "document has no body"))?;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut references = Vec::new();
    for node in iter_w(
        tree,
        body,
        &["commentRangeStart", "commentRangeEnd", "commentReference"],
    ) {
        let raw = w_attr(tree, node, "id");
        let Some(id) = raw.and_then(py_int) else {
            return Err(unsupported(
                "malformed_comment_id",
                format!(
                    "malformed {} comment marker w:id={}; nothing was changed",
                    tree.local_name(node).unwrap_or(""),
                    py_opt_repr(raw)
                ),
            ));
        };
        if id != comment_id {
            continue;
        }
        match w_local(tree, node) {
            Some("commentRangeStart") => starts.push(node),
            Some("commentRangeEnd") => ends.push(node),
            _ => references.push(tree.parent(node).unwrap_or(node)),
        }
    }
    if starts.len() > 1 || ends.len() > 1 || references.len() > 1 {
        return Err(unsupported(
            "duplicate_comment_anchor",
            format!("comment {comment_id} has duplicate anchor markers; nothing was changed"),
        ));
    }
    Ok((
        starts.first().copied(),
        ends.first().copied(),
        references.first().copied(),
    ))
}

/// The document text `comment` is anchored to (from its range marks).
pub fn anchored_text(document: &Document, comment: &Comment) -> Result<String> {
    let txn = Txn::new(document);
    anchored_text_in(&txn, comment)
}

fn anchored_text_in(txn: &Txn<'_>, comment: &Comment) -> Result<String> {
    comment_element(txn, comment)?;
    let main = main_of(txn);
    let tree = txn.tree(&main)?;
    let (start, end, _) = anchor_elements(tree, comment.comment_id)?;
    let (Some(start), Some(end)) = (start, end) else {
        return Err(not_found(
            "comment_without_anchor",
            format!(
                "comment {} has no range anchor in the body",
                comment.comment_id
            ),
        ));
    };
    let mut pieces = String::new();
    let mut inside = false;
    let mut last_paragraph: Option<NodeId> = None;
    for node in crate::wml::iter_nodes(tree, tree.root()) {
        if node == start {
            inside = true;
            continue;
        }
        if node == end {
            break;
        }
        if !inside || !is_direct_run_child(tree, node) {
            continue;
        }
        if matches!(w_local(tree, node), Some("delText" | "instrText")) {
            continue;
        }
        let projection = project_run_child(tree, node);
        if projection.barrier || projection.text.is_empty() {
            continue;
        }
        let paragraph = tree
            .ancestors(node)
            .find(|a| w_local(tree, *a) == Some("p"));
        if last_paragraph.is_some() && paragraph != last_paragraph {
            pieces.push('\n');
        }
        last_paragraph = paragraph;
        pieces.push_str(&projection.text);
    }
    Ok(pieces)
}

/// Validate `text`/`author`/`initials` for a new comment (upstream
/// `_validate_xml_characters` and the `author is required` check).
pub(crate) fn validate_comment_arguments(
    text: &str,
    author: &str,
    initials: Option<&str>,
) -> Result<()> {
    if author.is_empty() {
        return Err(Error::invalid_argument(
            "author_required",
            "author is required",
        ));
    }
    validate_xml_characters(text, "text")?;
    validate_xml_characters(author, "author")?;
    if let Some(initials) = initials {
        validate_xml_characters(initials, "initials")?;
    }
    Ok(())
}

/// Upstream `_validate_xml_characters`.
pub(crate) fn validate_xml_characters(value: &str, argument: &str) -> Result<()> {
    let mut invalid: Vec<String> = value
        .chars()
        .filter(|c| {
            let code = *c as u32;
            !(matches!(code, 0x09 | 0x0A | 0x0D)
                || (0x20..=0xD7FF).contains(&code)
                || (0xE000..=0xFFFD).contains(&code)
                || (0x10000..=0x10FFFF).contains(&code))
        })
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    invalid.sort();
    invalid.dedup();
    if invalid.is_empty() {
        return Ok(());
    }
    let shown: Vec<String> = invalid.iter().map(|value| py_repr(value)).collect();
    Err(Error::invalid_argument(
        "invalid_xml_characters",
        format!(
            "{argument} contains character(s) XML 1.0 cannot represent: [{}]",
            shown.join(", ")
        ),
    ))
}

fn insert_after(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "insertion anchor is detached"))?;
    let index = tree.index_in_parent(anchor).unwrap_or(0) + 1;
    tree.insert(parent, index, node)
}

fn insert_before(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "insertion anchor is detached"))?;
    let index = tree.index_in_parent(anchor).unwrap_or(0);
    tree.insert(parent, index, node)
}

/// A detached `w:commentRangeStart`/`End` with `w:id`.
pub(crate) fn range_marker(
    tree: &mut XmlTree,
    scope: NodeId,
    local: &str,
    id: i64,
) -> Result<NodeId> {
    let marker = tree.create_element(scope, W, local)?;
    tree.set_attr(marker, Some(W), "id", &id.to_string())?;
    Ok(marker)
}

/// A detached reference run (`CT_R._new_comment_reference_run`).
pub(crate) fn reference_run(tree: &mut XmlTree, scope: NodeId, id: i64) -> Result<NodeId> {
    let run = tree.create_element(scope, W, "r")?;
    let r_pr = tree.create_element(run, W, "rPr")?;
    tree.insert(run, 0, r_pr)?;
    let style = tree.create_element(r_pr, W, "rStyle")?;
    tree.insert(r_pr, 0, style)?;
    tree.set_attr(style, Some(W), "val", "CommentReference")?;
    let reference = tree.create_element(run, W, "commentReference")?;
    let end = tree.children(run).len();
    tree.insert(run, end, reference)?;
    tree.set_attr(reference, Some(W), "id", &id.to_string())?;
    Ok(run)
}

/// Insert `commentRangeStart` before `first`, then `commentRangeEnd` and a
/// reference run after `last` (upstream `insert_comment_range_*`).
pub(crate) fn mark_comment_range(
    tree: &mut XmlTree,
    first: NodeId,
    last: NodeId,
    id: i64,
) -> Result<()> {
    let start = range_marker(tree, first, "commentRangeStart", id)?;
    insert_before(tree, first, start)?;
    let run = reference_run(tree, last, id)?;
    insert_after(tree, last, run)?;
    let end = range_marker(tree, last, "commentRangeEnd", id)?;
    insert_after(tree, last, end)?;
    Ok(())
}

/// Add a threaded reply to `comment` with its own anchor triple around the
/// same text; returns the new comment.
pub fn reply(
    document: &mut Document,
    comment: &Comment,
    text: &str,
    author: &str,
    initials: Option<&str>,
    date: Option<Timestamp>,
) -> Result<Comment> {
    validate_comment_arguments(text, author, initials)?;
    let (staged, part, element) = {
        let mut txn = Txn::new(document);
        let (part, parent_elm) = comment_element(&txn, comment)?;
        refuse_if_protected(document, "reply to a comment", OperationClass::Comment)?;
        let main = main_of(&txn);
        let (start, end, reference) = {
            let tree = txn.tree(&main)?;
            anchor_elements(tree, comment.comment_id)?
        };
        let (Some(start), Some(end), Some(reference)) = (start, end, reference) else {
            return Err(not_found(
                "comment_without_anchor",
                format!(
                    "comment {} has no range anchor to thread onto",
                    comment.comment_id
                ),
            ));
        };
        let anchored: Vec<NodeId> = {
            let tree = txn.tree(&main)?;
            let ordered: Vec<NodeId> = crate::wml::iter_nodes(tree, tree.root()).collect();
            let start_at = ordered.iter().position(|n| *n == start);
            let end_at = ordered.iter().position(|n| *n == end);
            match (start_at, end_at) {
                (Some(s), Some(e)) if s < e => ordered[s + 1..e]
                    .iter()
                    .copied()
                    .filter(|n| w_local(tree, *n) == Some("r"))
                    .collect(),
                _ => {
                    return Err(unsupported(
                        "stale_comment_range",
                        "comment range markers are stale, reversed, or detached; nothing was changed",
                    ));
                }
            }
        };
        if anchored.is_empty() {
            return Err(unsupported(
                "empty_comment_range",
                "comment range contains no live runs to anchor a reply; nothing was changed",
            ));
        }
        preflight_comment_range(
            document,
            txn.tree(&main)?,
            anchored[0],
            anchored[anchored.len() - 1],
            "reply to a comment",
        )?;
        preflight_comment_add(&txn)?;
        preflight_comments_extended_write(&txn)?;
        let parent_last = require_last_paragraph(txn.tree(&part)?, parent_elm)?;
        let parent_para_id = ensure_para_id(&mut txn, &part, parent_last)?;
        let (cpart, new_elm, new_id) = comments::stage_add_comment(
            &mut txn,
            text,
            author,
            Some(initials.unwrap_or("")),
            None,
        )?;
        let date = date.unwrap_or_else(Timestamp::now).to_xml();
        txn.tree_mut(&cpart)?
            .set_attr(new_elm, Some(W), "date", &date)?;
        let new_last = require_last_paragraph(txn.tree(&cpart)?, new_elm)?;
        let new_para_id = ensure_para_id(&mut txn, &cpart, new_last)?;
        {
            let tree = txn.tree_mut(&main)?;
            let new_start = range_marker(tree, start, "commentRangeStart", new_id)?;
            insert_after(tree, start, new_start)?;
            let new_end = range_marker(tree, end, "commentRangeEnd", new_id)?;
            insert_before(tree, end, new_end)?;
            let run = reference_run(tree, reference, new_id)?;
            insert_after(tree, reference, run)?;
        }
        let root = comments_extended_root(&mut txn, true)?
            .ok_or_else(|| Error::internal("comments_extended", "commentsExtended missing"))?;
        entry_for_create(&mut txn, &root, &parent_para_id)?;
        let reply_entry = entry_for_create(&mut txn, &root, &new_para_id)?;
        txn.tree_mut(&root)?
            .set_attr(reply_entry, Some(W15), "paraIdParent", &parent_para_id)?;
        (txn.finish()?, cpart, new_elm)
    };
    document.commit(staged)?;
    comments::comment_at(document, &part, element)
}

/// One row of [`comment_thread`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadEntry {
    /// `w:id`.
    pub comment_id: i64,
    /// `w:author`.
    pub author: String,
    /// Comment body text.
    pub text: String,
    /// `w15:done`.
    pub resolved: bool,
    /// Parent comment id for replies.
    pub parent_id: Option<i64>,
    /// Anchored document text, when the comment has a body range.
    pub anchored_text: Option<String>,
}

/// Every comment with its thread state.
pub fn comment_thread(document: &Document) -> Result<Vec<ThreadEntry>> {
    let mut txn = Txn::new(document);
    preflight_comment_add(&txn)?;
    let Some(part) = comments_part(&txn, "comment inspection")? else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for comment in comments::comments_in(&txn, &part)?.iter() {
        let anchor = match anchored_text_in(&txn, comment) {
            Ok(text) => Some(text),
            Err(err) if err.refusal_kind() == Some(RefusalKind::TargetNotFound) => None,
            Err(err) => return Err(err),
        };
        let resolved = is_resolved(document, comment)?;
        let parent_id = parent_of_in(&mut txn, comment)?;
        entries.push(ThreadEntry {
            comment_id: comment.comment_id,
            author: comment.author.clone(),
            text: comment.text.clone(),
            resolved,
            parent_id,
            anchored_text: anchor,
        });
    }
    Ok(entries)
}

fn thread_ids_to_delete(txn: &mut Txn<'_>, part: &str, comment: &Comment) -> Result<Vec<i64>> {
    let mut wanted = vec![comment.comment_id];
    let all = comments::comments_in(txn, part)?;
    let mut grew = true;
    while grew {
        grew = false;
        for candidate in all.iter() {
            if wanted.contains(&candidate.comment_id) {
                continue;
            }
            if let Some(parent) = parent_of_in(txn, candidate)?
                && wanted.contains(&parent)
            {
                wanted.push(candidate.comment_id);
                grew = true;
            }
        }
    }
    Ok(wanted)
}

/// Delete `comment` and its replies, removing their range marks.
pub fn delete_comment(document: &mut Document, comment: &Comment) -> Result<()> {
    let stories = story::stories(document);
    let staged = {
        let mut txn = Txn::new(document);
        let (part, _) = comment_element(&txn, comment)?;
        refuse_if_protected(document, "delete a comment", OperationClass::Comment)?;
        preflight_comment_add(&txn)?;
        preflight_comments_extended_write(&txn)?;
        let ids = thread_ids_to_delete(&mut txn, &part, comment)?;
        let mut para_ids: Vec<String> = Vec::new();
        for item in comments::comments_in(&txn, &part)?.iter() {
            if !ids.contains(&item.comment_id) {
                continue;
            }
            let (cpart, element) = comment_element(&txn, item)?;
            if let Some(raw) = last_para_id(txn.tree(&cpart)?, element)?.filter(|r| !r.is_empty()) {
                para_ids.push(validate_para_id(Some(&raw), "w14:paraId")?);
            }
        }
        for story in &stories {
            let doomed: Vec<NodeId> = {
                let tree = txn.tree(&story.part)?;
                iter_w(
                    tree,
                    tree.root(),
                    &["commentRangeStart", "commentRangeEnd", "commentReference"],
                )
                .filter(|node| {
                    w_attr(tree, *node, "id")
                        .and_then(py_int)
                        .is_some_and(|id| ids.contains(&id))
                })
                .collect()
            };
            if doomed.is_empty() {
                continue;
            }
            let tree = txn.tree_mut(&story.part)?;
            for node in doomed {
                let parent = tree.parent(node);
                tree.detach(node)?;
                if let Some(parent) = parent {
                    let empty_run = w_local(tree, parent) == Some("r")
                        && crate::wml::node_children(tree, parent)
                            .all(|child| w_local(tree, child) == Some("rPr"));
                    if empty_run && tree.parent(parent).is_some() {
                        tree.detach(parent)?;
                    }
                }
            }
        }
        let doomed: Vec<NodeId> = {
            let tree = txn.tree(&part)?;
            tree.child_elements(tree.root())
                .filter(|elm| w_local(tree, *elm) == Some("comment"))
                .filter(|elm| {
                    w_attr(tree, *elm, "id")
                        .and_then(py_int)
                        .is_some_and(|id| ids.contains(&id))
                })
                .collect()
        };
        {
            let tree = txn.tree_mut(&part)?;
            for elm in doomed {
                tree.detach(elm)?;
            }
        }
        if let Some(extended) = comments_extended_root(&mut txn, false)? {
            let doomed: Vec<NodeId> = {
                let tree = txn.tree(&extended)?;
                tree.child_elements(tree.root())
                    .filter(|entry| tree.is(*entry, W15, "commentEx"))
                    .filter(|entry| {
                        let hit = |name: &str| {
                            tree.attr(*entry, Some(W15), name)
                                .filter(|v| !v.is_empty())
                                .is_some_and(|v| para_ids.contains(&v.to_ascii_uppercase()))
                        };
                        hit("paraId") || hit("paraIdParent")
                    })
                    .collect()
            };
            let tree = txn.tree_mut(&extended)?;
            for entry in doomed {
                tree.detach(entry)?;
            }
        }
        let main = main_of(&txn);
        remove_comment_identity_rows(&mut txn, &main, &para_ids)?;
        txn.finish()?
    };
    document.commit(staged)
}

impl Document {
    /// The thread state of every comment (upstream `comment_thread`).
    pub fn comment_thread(&self) -> Result<Vec<ThreadEntry>> {
        comment_thread(self)
    }
}
