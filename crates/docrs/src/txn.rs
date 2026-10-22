//! Multi-part copy-on-write transactions.
//!
//! A [`Txn`] reads through to the live package and stages every write: a
//! modified part is cloned on first write (node IDs retained), new parts and
//! relationships are recorded, and `[Content_Types].xml` is edited the way
//! python-docx would describe the new parts. Nothing touches the document
//! until [`Document::commit`] applies a [`Staged`] set, after every staged
//! tree was validated; a refusal or error simply drops the transaction.

use std::collections::{BTreeMap, BTreeSet};

use crate::package::content_types::{CT_NS, extension};
use crate::package::rels::{
    RELS_CONTENT_TYPE, RELS_NS, Relationship, base_uri, relationships_from_tree, relative_ref,
    rels_member_for,
};
use crate::unicode::casefold;
use crate::xml::{self, XmlTree};
use crate::{Document, Error, Result};

/// python-docx `default_content_types`: `(extension, content type)` pairs it
/// writes as `Default` entries.
const DEFAULT_CONTENT_TYPES: &[(&str, &str)] = &[
    (
        "bin",
        "application/vnd.openxmlformats-officedocument.presentationml.printerSettings",
    ),
    (
        "bin",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings",
    ),
    (
        "bin",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.printerSettings",
    ),
    ("bmp", "image/bmp"),
    ("emf", "image/x-emf"),
    ("fntdata", "application/x-fontdata"),
    ("gif", "image/gif"),
    ("jpe", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("rels", RELS_CONTENT_TYPE),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("wdp", "image/vnd.ms-photo"),
    ("wmf", "image/x-wmf"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    ("xml", "application/xml"),
];

const CONTENT_TYPES: &str = "[Content_Types].xml";

/// Relationship target.
#[derive(Debug, Clone, Copy)]
pub enum RelTarget<'a> {
    /// An internal part (member name).
    Part(&'a str),
    /// An external URI.
    External(&'a str),
}

/// A staged change set ready for [`Document::commit`].
#[derive(Debug, Default)]
pub struct Staged {
    trees: BTreeMap<String, XmlTree>,
    new_parts: Vec<(String, String)>,
    binaries: BTreeMap<String, Vec<u8>>,
    removed: BTreeSet<String>,
}

impl Staged {
    /// Parts that will change or be added.
    pub fn touched_parts(&self) -> Vec<&str> {
        self.trees
            .keys()
            .chain(self.binaries.keys())
            .map(String::as_str)
            .collect()
    }

    /// Nothing staged.
    pub fn is_empty(&self) -> bool {
        self.trees.is_empty() && self.binaries.is_empty() && self.removed.is_empty()
    }

    /// Existing parts the commit removes (a name the change set adds back
    /// is not removed).
    pub fn removed_parts(&self) -> Vec<&str> {
        self.removed
            .iter()
            .filter(|name| !self.new_parts.iter().any(|(new, _)| new == *name))
            .map(String::as_str)
            .collect()
    }
}

/// A transaction over one document.
pub struct Txn<'d> {
    doc: &'d Document,
    trees: BTreeMap<String, XmlTree>,
    new_parts: Vec<(String, String)>,
    binaries: BTreeMap<String, Vec<u8>>,
    /// Existing members removed by this transaction.
    removed: BTreeSet<String>,
}

impl<'d> Txn<'d> {
    /// Begin a transaction.
    pub fn new(doc: &'d Document) -> Self {
        Txn {
            doc,
            trees: BTreeMap::new(),
            new_parts: Vec::new(),
            binaries: BTreeMap::new(),
            removed: BTreeSet::new(),
        }
    }

    /// The document being edited.
    pub fn document(&self) -> &'d Document {
        self.doc
    }

    /// Actual member name for `name` (case-insensitive, staged parts included).
    pub fn resolve(&self, name: &str) -> Option<String> {
        let bare = name.trim_start_matches('/');
        let folded = casefold(bare);
        if let Some((existing, _)) = self.new_parts.iter().find(|(n, _)| casefold(n) == folded) {
            return Some(existing.clone());
        }
        self.doc
            .package()
            .resolve_member(bare)
            .filter(|member| !self.removed.contains(*member))
            .map(str::to_string)
    }

    /// Remove an existing part: its member, its relationships part and its
    /// `[Content_Types].xml` override go when the change set commits.
    /// Relationships that target it are the caller's to remove. A part added
    /// in this transaction cannot be removed; a removed name can be added
    /// again.
    pub fn remove_part(&mut self, name: &str) -> Result<()> {
        let Some(member) = self.resolve(name) else {
            return Ok(());
        };
        if self.new_parts.iter().any(|(n, _)| *n == member) {
            return Err(Error::internal(
                "remove_new_part",
                format!("part {member} was added in this transaction"),
            ));
        }
        if member == CONTENT_TYPES || member == self.doc.package().main_part_name() {
            return Err(Error::internal(
                "remove_required_part",
                format!("part {member} cannot be removed"),
            ));
        }
        let rels_member = rels_member_for(&format!("/{member}"));
        let tree = self.tree_mut(CONTENT_TYPES)?;
        let root = tree.root();
        let part_name = casefold(&format!("/{member}"));
        let overrides: Vec<_> = tree
            .child_elements(root)
            .filter(|node| tree.is(*node, CT_NS, "Override"))
            .filter(|node| {
                tree.attr(*node, None, "PartName")
                    .is_some_and(|p| casefold(p) == part_name)
            })
            .collect();
        for node in overrides {
            tree.detach(node)?;
        }
        self.trees.remove(&member);
        self.binaries.remove(&member);
        self.removed.insert(member);
        if let Some(rels) = self.resolve(&rels_member) {
            self.trees.remove(&rels);
            self.removed.insert(rels);
        }
        Ok(())
    }

    /// Current tree of `part` (staged or live).
    pub fn tree(&self, part: &str) -> Result<&XmlTree> {
        match self.trees.get(part) {
            Some(tree) => Ok(tree),
            None => self.doc.package().xml(part),
        }
    }

    /// Mutable staged tree of `part` (cloned on first write).
    pub fn tree_mut(&mut self, part: &str) -> Result<&mut XmlTree> {
        if !self.trees.contains_key(part) {
            let tree = self.doc.package().xml(part)?.clone();
            self.trees.insert(part.to_string(), tree);
        }
        self.trees
            .get_mut(part)
            .ok_or_else(|| Error::internal("txn_tree", "staged tree vanished"))
    }

    /// Content type of `part` as it will be after commit.
    pub fn content_type(&self, part: &str) -> Option<String> {
        if let Some((_, ct)) = self.new_parts.iter().find(|(n, _)| n == part) {
            return Some(ct.clone());
        }
        self.doc
            .package()
            .part(part)
            .and_then(|p| p.content_type())
            .map(str::to_string)
    }

    /// Relationships from `source` (`/` or a member name).
    pub fn relationships(&self, source: &str) -> Result<Vec<Relationship>> {
        let source_name = source_part_name(source);
        let rels_member = rels_member_for(&source_name);
        match self.resolve(&rels_member) {
            Some(member) => relationships_from_tree(self.tree(&member)?, base_uri(&source_name)),
            None => Ok(Vec::new()),
        }
    }

    /// Member targeted by the single internal relationship of exact
    /// `rel_type` from `source` (upstream `part_related_by`).
    pub fn part_related_by(&self, source: &str, rel_type: &str) -> Result<Option<String>> {
        let source_name = source_part_name(source);
        let matching: Vec<Relationship> = self
            .relationships(source)?
            .into_iter()
            .filter(|rel| !rel.external && rel.rel_type == rel_type)
            .collect();
        match matching.as_slice() {
            [] => Ok(None),
            [rel] => Ok(self.resolve(&rel.target_part_name(base_uri(&source_name)))),
            _ => Err(Error::refusal(
                crate::RefusalKind::UnsupportedStructure,
                "ambiguous_relationship",
                format!(
                    "multiple {rel_type} relationships from {source_name} make the target ambiguous"
                ),
            )),
        }
    }

    /// Upstream `next_partname("/word/header%d.xml")` with `{}` as the
    /// number placeholder (member names, no leading slash).
    pub fn next_partname(&self, template: &str) -> String {
        let count = self.doc.package().parts().count() + self.new_parts.len();
        // A member this transaction removes keeps its name out of the
        // numbering (it can still be added back by name), so the chosen name
        // does not depend on removals staged earlier in the same change set.
        (1..=count + 1)
            .map(|n| template.replace("{}", &n.to_string()))
            .find(|candidate| {
                self.resolve(candidate).is_none()
                    && self.doc.package().resolve_member(candidate).is_none()
            })
            .unwrap_or_else(|| template.replace("{}", &(count + 1).to_string()))
    }

    /// Add a new XML part and describe it in `[Content_Types].xml`.
    pub fn add_xml_part(&mut self, name: &str, content_type: &str, tree: XmlTree) -> Result<()> {
        if self.resolve(name).is_some() {
            return Err(Error::internal(
                "duplicate_part",
                format!("part {name} already exists"),
            ));
        }
        self.declare_content_type(name, content_type)?;
        self.trees.insert(name.to_string(), tree);
        self.new_parts
            .push((name.to_string(), content_type.to_string()));
        Ok(())
    }

    /// Add a new binary part (e.g. an image) and describe it in
    /// `[Content_Types].xml` with the same Default/Override rule as XML parts.
    pub fn add_binary_part(
        &mut self,
        name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<()> {
        if self.resolve(name).is_some() {
            return Err(Error::internal(
                "duplicate_part",
                format!("part {name} already exists"),
            ));
        }
        self.declare_content_type(name, content_type)?;
        self.binaries.insert(name.to_string(), bytes);
        self.new_parts
            .push((name.to_string(), content_type.to_string()));
        Ok(())
    }

    /// Current bytes of a part (staged binary or live), `None` when absent.
    pub fn binary(&self, part: &str) -> Option<std::borrow::Cow<'_, [u8]>> {
        if let Some(bytes) = self.binaries.get(part) {
            return Some(std::borrow::Cow::Borrowed(bytes.as_slice()));
        }
        self.doc.package().part(part).map(|p| p.current_bytes())
    }

    fn declare_content_type(&mut self, name: &str, content_type: &str) -> Result<()> {
        let ext = extension(&format!("/{name}")).to_lowercase();
        let tree = self.tree_mut(CONTENT_TYPES)?;
        let root = tree.root();
        let existing_default: Option<String> = tree
            .child_elements(root)
            .filter(|node| tree.is(*node, CT_NS, "Default"))
            .find(|node| {
                tree.attr(*node, None, "Extension")
                    .is_some_and(|e| e.to_lowercase() == ext)
            })
            .and_then(|node| tree.attr(node, None, "ContentType").map(str::to_string));
        if existing_default.as_deref() == Some(content_type) {
            return Ok(());
        }
        let as_default = existing_default.is_none()
            && DEFAULT_CONTENT_TYPES.contains(&(ext.as_str(), content_type));
        let element = if as_default {
            let element = tree.create_element(root, CT_NS, "Default")?;
            tree.set_attr(element, None, "Extension", &ext)?;
            element
        } else {
            let element = tree.create_element(root, CT_NS, "Override")?;
            tree.set_attr(element, None, "PartName", &format!("/{name}"))?;
            element
        };
        tree.set_attr(element, None, "ContentType", content_type)?;
        let end = tree.children(root).len();
        tree.insert(root, end, element)
    }

    /// Upstream `relate_to`: reuse a matching relationship or add one with
    /// the lowest unused `rIdN`. Returns the rId.
    pub fn relate_to(
        &mut self,
        source: &str,
        rel_type: &str,
        target: RelTarget<'_>,
    ) -> Result<String> {
        let source_name = source_part_name(source);
        let base = base_uri(&source_name).to_string();
        let existing = self.relationships(source)?;
        let matches = |rel: &Relationship| {
            if rel.rel_type != rel_type {
                return false;
            }
            match target {
                RelTarget::External(url) => rel.external && rel.target == url,
                RelTarget::Part(member) => {
                    !rel.external
                        && casefold(rel.target_part_name(&base).trim_start_matches('/'))
                            == casefold(member)
                }
            }
        };
        if let Some(rel) = existing.iter().find(|rel| matches(rel)) {
            return Ok(rel.id.clone());
        }
        let r_id = (1..=existing.len() + 1)
            .map(|n| format!("rId{n}"))
            .find(|candidate| existing.iter().all(|rel| rel.id != *candidate))
            .unwrap_or_else(|| format!("rId{}", existing.len() + 1));
        let (target_ref, external) = match target {
            RelTarget::External(url) => (url.to_string(), true),
            RelTarget::Part(member) => (relative_ref(&format!("/{member}"), &base), false),
        };
        let rels_member = rels_member_for(&source_name);
        let member = match self.resolve(&rels_member) {
            Some(member) => member,
            None => {
                let tree = xml::parse_str(&format!("<Relationships xmlns=\"{RELS_NS}\"/>"))
                    .map_err(|err| Error::internal("rels_template", err.to_string()))?;
                self.add_xml_part(&rels_member, RELS_CONTENT_TYPE, tree)?;
                rels_member
            }
        };
        let tree = self.tree_mut(&member)?;
        let root = tree.root();
        let rel = tree.create_element(root, RELS_NS, "Relationship")?;
        tree.set_attr(rel, None, "Id", &r_id)?;
        tree.set_attr(rel, None, "Type", rel_type)?;
        tree.set_attr(rel, None, "Target", &target_ref)?;
        if external {
            tree.set_attr(rel, None, "TargetMode", "External")?;
        }
        let end = tree.children(root).len();
        tree.insert(root, end, rel)?;
        Ok(r_id)
    }

    /// Validate every staged tree and hand back the change set.
    pub fn finish(self) -> Result<Staged> {
        for (part, tree) in &self.trees {
            let bytes = xml::serialize(tree);
            let reparsed = xml::parse(&bytes, &crate::Limits::trusted()).map_err(|err| {
                Error::internal(
                    "staged_reparse",
                    format!("staged {part} does not reparse: {err}"),
                )
            })?;
            if !xml::trees_equivalent(tree, &reparsed) {
                return Err(Error::internal(
                    "staged_roundtrip",
                    format!("staged {part} does not round-trip"),
                ));
            }
        }
        Ok(Staged {
            trees: self.trees,
            new_parts: self.new_parts,
            binaries: self.binaries,
            removed: self.removed,
        })
    }
}

fn source_part_name(source: &str) -> String {
    if source == "/" {
        "/".to_string()
    } else {
        format!("/{}", source.trim_start_matches('/'))
    }
}

impl Document {
    /// Apply a validated change set. Every staged tree replaces its live
    /// tree (or becomes a new part) together; content types are reloaded.
    pub(crate) fn commit(&mut self, staged: Staged) -> Result<()> {
        let Staged {
            mut trees,
            new_parts,
            mut binaries,
            removed,
        } = staged;
        // Every fallible check runs before the first mutation, so a failure
        // leaves the package exactly as it was.
        if let Some((name, _)) = new_parts
            .iter()
            .find(|(name, _)| !binaries.contains_key(name) && !trees.contains_key(name))
        {
            return Err(Error::internal(
                "txn_new_part",
                format!("new part {name} lost its tree"),
            ));
        }
        let replaced: Vec<&str> = trees
            .keys()
            .filter(|name| !new_parts.iter().any(|(new, _)| new == *name))
            .map(String::as_str)
            .collect();
        let content_types = self.package().preflight_commit(
            new_parts.iter().map(|(name, _)| name.as_str()),
            replaced,
            &removed,
            trees.get(CONTENT_TYPES),
        )?;
        // Removals go first so a removed name can be added back.
        for name in &removed {
            self.package_mut().remove_part(name);
        }
        for (name, content_type) in &new_parts {
            if let Some(bytes) = binaries.remove(name) {
                self.package_mut()
                    .add_binary_part(name, content_type, bytes)?;
                continue;
            }
            if let Some(tree) = trees.remove(name) {
                self.package_mut().add_xml_part(name, content_type, tree)?;
            }
        }
        for (name, tree) in trees {
            self.package_mut().commit_tree(&name, tree)?;
        }
        if let Some(content_types) = content_types {
            self.package_mut().set_content_types(content_types);
        }
        Ok(())
    }

    /// [`Document::commit`] for tests only (`test-support` feature).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn commit_for_tests(&mut self, staged: Staged) -> Result<()> {
        self.commit(staged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_commit_leaves_package_unchanged() {
        let mut doc = Document::new();
        let (before, _) = doc.patch_bytes().unwrap();
        let mut txn = Txn::new(&doc);
        let main = txn.resolve("word/document.xml").unwrap();
        let tree = txn.tree_mut(&main).unwrap();
        let root = tree.root();
        tree.set_local_name(root, "documentRenamed").unwrap();
        let types = txn.tree_mut(CONTENT_TYPES).unwrap();
        let root = types.root();
        types.set_local_name(root, "NotTypes").unwrap();
        let staged = txn.finish().unwrap();
        assert!(doc.commit(staged).is_err());
        assert!(!doc.package().is_modified());
        assert_eq!(doc.patch_bytes().unwrap().0, before);
    }
}
