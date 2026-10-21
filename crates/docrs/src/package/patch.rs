//! Narrow "patch" saves (upstream `patch_save` and its comparison rules).
//!
//! * A save in which nothing changed returns the original archive bytes
//!   verbatim (no-op round trip is byte-identical).
//! * Otherwise every member keeps its original bytes unless it changed; a
//!   modified XML part that is semantically identical to its original is
//!   restored to the original bytes. Output is a deterministic ZIP in the
//!   original member order with added members appended.
//! * The finished output is re-read through the package guard before it is
//!   returned, so a save never produces a package this crate would refuse.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::content_types::CT_NS;
use super::rels::RELS_NS;
use super::{Package, is_xml_part_name, zip_read, zip_write};
use crate::xml::{self, NodeData};
use crate::{Limits, Result};

/// Outcome of a patch save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PatchReport {
    /// Modified parts whose result was semantically identical and therefore
    /// got their original bytes back.
    pub restored_parts: Vec<String>,
    /// Parts written with new bytes.
    pub changed_parts: Vec<String>,
    /// Parts not present in the original.
    pub added_parts: Vec<String>,
    /// Original parts no longer present.
    pub removed_parts: Vec<String>,
    /// The output is the original archive, byte for byte.
    pub verbatim_copy: bool,
}

impl PatchReport {
    /// JSON (`schema: paper_patch_save`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_patch_save",
            "version": 1,
            "restored_parts": self.restored_parts,
            "changed_parts": self.changed_parts,
            "added_parts": self.added_parts,
            "removed_parts": self.removed_parts,
            "verbatim_copy": self.verbatim_copy,
        })
    }
}

/// Serialize `package` as a patch of its source archive.
pub fn patch_bytes(package: &Package) -> Result<(Vec<u8>, PatchReport)> {
    let limits = package.limits();
    let original_names: Vec<&str> = package
        .raw_parts()
        .iter()
        .filter(|part| part.original.is_some())
        .map(|part| part.name.as_str())
        .collect();
    let candidate_names: Vec<&str> = package.parts().map(|part| part.name.as_str()).collect();

    let mut restored = Vec::new();
    let mut changed = Vec::new();
    let mut added = BTreeSet::new();
    let mut removed = BTreeSet::new();
    let mut output: Vec<(String, std::borrow::Cow<'_, [u8]>)> = Vec::new();
    for part in package.raw_parts() {
        if part.removed {
            if part.original.is_some() {
                removed.insert(part.name.clone());
            }
            continue;
        }
        let current = part.current_bytes();
        match part.original() {
            None => {
                added.insert(part.name.clone());
                output.push((part.name.clone(), current));
            }
            Some(original) => {
                if *current == *original {
                    output.push((part.name.clone(), current));
                } else if is_xml_part_name(&part.name)
                    && parts_semantically_equal(
                        &part.name,
                        original,
                        &current,
                        &original_names,
                        &candidate_names,
                        limits,
                    )
                {
                    restored.push(part.name.clone());
                    output.push((part.name.clone(), std::borrow::Cow::Borrowed(original)));
                } else {
                    changed.push(part.name.clone());
                    output.push((part.name.clone(), current));
                }
            }
        }
    }

    let verbatim = added.is_empty() && removed.is_empty() && changed.is_empty();
    let bytes = match (verbatim, package.source_bytes()) {
        (true, Some(source)) => source.to_vec(),
        _ => zip_write::write_archive(
            output
                .iter()
                .map(|(name, data)| (name.as_str(), data.as_ref())),
            limits,
        )?,
    };
    zip_read::validate_archive(
        &bytes,
        &Limits {
            max_archive_bytes: u64::MAX,
            ..limits.clone()
        },
    )
    .map_err(|err| err.context("the serialized package failed validation"))?;
    Ok((
        bytes,
        PatchReport {
            restored_parts: restored,
            changed_parts: changed,
            added_parts: added.into_iter().collect(),
            removed_parts: removed.into_iter().collect(),
            verbatim_copy: verbatim && package.source_bytes().is_some(),
        },
    ))
}

/// Part-type-aware equivalence; anything unparseable is conservatively
/// unequal.
pub fn parts_semantically_equal(
    name: &str,
    a: &[u8],
    b: &[u8],
    a_names: &[&str],
    b_names: &[&str],
    limits: &Limits,
) -> bool {
    if name == zip_read::CONTENT_TYPES_NAME {
        return match (
            effective_content_types(a, a_names, limits),
            effective_content_types(b, b_names, limits),
        ) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
    }
    if name.ends_with(".rels") {
        return match (
            relationship_multiset(a, limits),
            relationship_multiset(b, limits),
        ) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
    }
    xml_equivalent(a, b, limits).unwrap_or(false)
}

/// Upstream `xml_equivalent` (`None` when either side fails to parse).
pub fn xml_equivalent(a: &[u8], b: &[u8], limits: &Limits) -> Option<bool> {
    let x = xml::parse(a, limits).ok()?;
    let y = xml::parse(b, limits).ok()?;
    Some(xml::trees_equivalent(&x, &y))
}

type RelTuple = (String, String, String, String);

/// Sorted multiset of `(Id, Type, Target, TargetMode)` over every
/// `Relationship` element at any depth.
fn relationship_multiset(data: &[u8], limits: &Limits) -> Option<Vec<RelTuple>> {
    let tree = xml::parse(data, limits).ok()?;
    let mut out: Vec<RelTuple> = tree
        .descendants(tree.root())
        .filter(|node| tree.is(*node, RELS_NS, "Relationship"))
        .map(|node| {
            let get = |name: &str| tree.attr(node, None, name).unwrap_or("").to_string();
            let mode = tree
                .attr(node, None, "TargetMode")
                .unwrap_or("Internal")
                .to_string();
            (get("Id"), get("Type"), get("Target"), mode)
        })
        .collect();
    out.sort();
    Some(out)
}

pub(crate) type EffectiveTypes = (BTreeMap<String, String>, BTreeMap<String, String>);

/// `(active defaults, overrides)`; defaults are restricted to extensions in
/// use, computed exactly as upstream does (`name.rsplit(".", 1)[-1].lower()`
/// over member names containing a dot).
pub(crate) fn effective_content_types(
    data: &[u8],
    names: &[&str],
    limits: &Limits,
) -> Option<EffectiveTypes> {
    let in_use: BTreeSet<String> = names
        .iter()
        .filter(|name| name.contains('.'))
        .map(|name| name.rsplit('.').next().unwrap_or("").to_lowercase())
        .collect();
    let tree = xml::parse(data, limits).ok()?;
    let mut defaults = BTreeMap::new();
    let mut overrides = BTreeMap::new();
    for node in tree.descendants(tree.root()) {
        if !matches!(tree.data(node), NodeData::Element(_)) {
            continue;
        }
        if tree.is(node, CT_NS, "Default") {
            let ext = tree
                .attr(node, None, "Extension")
                .unwrap_or("")
                .to_lowercase();
            let ct = tree
                .attr(node, None, "ContentType")
                .unwrap_or("")
                .to_string();
            defaults.insert(ext, ct);
        } else if tree.is(node, CT_NS, "Override") {
            let part = tree.attr(node, None, "PartName").unwrap_or("").to_string();
            let ct = tree
                .attr(node, None, "ContentType")
                .unwrap_or("")
                .to_string();
            overrides.insert(part, ct);
        }
    }
    defaults.retain(|ext, _| in_use.contains(ext));
    Some((defaults, overrides))
}
