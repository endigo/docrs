//! Part-by-part package comparison (upstream `diff_package`).

use std::collections::BTreeMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::patch::parts_semantically_equal;
use super::{is_xml_part_name, zip_read};
use crate::{Limits, Result};

/// One byte-changed part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartDiff {
    /// Member name.
    pub part: String,
    /// `"xml"` or `"binary"`.
    pub kind: &'static str,
    /// SHA-256 of the first package's bytes.
    pub before_sha256: String,
    /// SHA-256 of the second package's bytes.
    pub after_sha256: String,
    /// False when the bytes differ but the XML means the same.
    pub semantic_change: bool,
}

/// Comparison of two packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageDiff {
    /// Members only in the second package (sorted).
    pub added: Vec<String>,
    /// Members only in the first package (sorted).
    pub removed: Vec<String>,
    /// Byte-changed members present in both (sorted).
    pub changed: Vec<PartDiff>,
    /// Members whose bytes are identical.
    pub byte_identical_count: usize,
}

impl PackageDiff {
    /// Names of semantically changed parts.
    pub fn semantic_changed_parts(&self) -> Vec<&str> {
        self.changed
            .iter()
            .filter(|item| item.semantic_change)
            .map(|item| item.part.as_str())
            .collect()
    }

    /// Same parts with the same meaning.
    pub fn is_semantically_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.semantic_changed_parts().is_empty()
    }

    /// JSON (`schema: paper_package_diff`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_package_diff",
            "version": 1,
            "added": self.added,
            "removed": self.removed,
            "changed": self.changed,
            "byte_identical_count": self.byte_identical_count,
            "is_semantically_empty": self.is_semantically_empty(),
        })
    }
}

/// SHA-256 hex digest.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Diff two archives (both pass the package guard first).
pub fn diff_packages(a: &[u8], b: &[u8], limits: &Limits) -> Result<PackageDiff> {
    let a = zip_read::read_archive(a, limits)?;
    let b = zip_read::read_archive(b, limits)?;
    let a_order: Vec<&str> = a.members.iter().map(|m| m.name.as_str()).collect();
    let b_order: Vec<&str> = b.members.iter().map(|m| m.name.as_str()).collect();
    let a_map: BTreeMap<&str, &[u8]> = a
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.data.as_slice()))
        .collect();
    let b_map: BTreeMap<&str, &[u8]> = b
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.data.as_slice()))
        .collect();

    let added = b_map
        .keys()
        .filter(|name| !a_map.contains_key(*name))
        .map(|s| s.to_string())
        .collect();
    let removed = a_map
        .keys()
        .filter(|name| !b_map.contains_key(*name))
        .map(|s| s.to_string())
        .collect();
    let mut changed = Vec::new();
    let mut identical = 0;
    for (name, before) in &a_map {
        let Some(after) = b_map.get(name) else {
            continue;
        };
        if before == after {
            identical += 1;
            continue;
        }
        let (kind, semantic) = if is_xml_part_name(name) {
            (
                "xml",
                !parts_semantically_equal(name, before, after, &a_order, &b_order, limits),
            )
        } else {
            ("binary", true)
        };
        changed.push(PartDiff {
            part: name.to_string(),
            kind,
            before_sha256: sha256_hex(before),
            after_sha256: sha256_hex(after),
            semantic_change: semantic,
        });
    }
    Ok(PackageDiff {
        added,
        removed,
        changed,
        byte_identical_count: identical,
    })
}
