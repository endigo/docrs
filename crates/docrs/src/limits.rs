//! Explicit resource budgets.
//!
//! Upstream paper-docx removed its archive size and member-count ceilings after
//! Word was measured opening documents past all of them, so upstream refuses
//! nothing for size alone — except where lxml's own (non-`huge_tree`) parser
//! limits apply: element depth above 256 and single text nodes above
//! 10,000,000 bytes fail to parse there.
//!
//! This port holds whole packages in memory, so it bounds work explicitly.
//! Every budget failure is reported as [`crate::ErrorKind::ResourceLimit`]
//! naming the field below — a *policy* outcome distinct from malformed input.
//! [`Limits::default`] mirrors lxml's effective XML limits and sets generous
//! package ceilings; [`Limits::trusted`] is for local, trusted input.
//!
//! These defaults are a documented policy difference from upstream, not a
//! claim that larger documents are invalid.

/// Resource budgets applied while reading, editing and writing packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Physical size of the input archive.
    pub max_archive_bytes: u64,
    /// Central-directory records (members plus directory entries).
    pub max_entries: u64,
    /// Inflated size of any single member, declared or actual.
    pub max_part_bytes: u64,
    /// Inflated size of all members together, declared or actual.
    pub max_total_part_bytes: u64,
    /// Maximum element nesting depth of one XML part (lxml default: 256).
    pub max_xml_depth: u32,
    /// Nodes (elements, text, comments, PIs) in one XML part.
    pub max_xml_nodes: u64,
    /// Attributes on one element.
    pub max_xml_attributes: u32,
    /// Namespace declarations on one element.
    pub max_namespace_declarations: u32,
    /// Bytes in one text node (lxml default: 10,000,000).
    pub max_text_node_bytes: u64,
    /// Characters produced by search normalization for one story.
    pub max_normalized_chars: u64,
    /// Serialized output archive size.
    pub max_output_bytes: u64,
    /// Abstract work units for package/text diff algorithms.
    pub max_diff_work: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_archive_bytes: 1 << 30,
            max_entries: 65_535,
            max_part_bytes: 512 << 20,
            max_total_part_bytes: 2 << 30,
            max_xml_depth: 256,
            max_xml_nodes: 50_000_000,
            max_xml_attributes: 10_000,
            max_namespace_declarations: 10_000,
            max_text_node_bytes: 10_000_000,
            max_normalized_chars: 200_000_000,
            // The writer does not emit ZIP64; stay strictly below its limits.
            max_output_bytes: 0xFFFF_FFFE,
            max_diff_work: 100_000_000,
        }
    }
}

impl Limits {
    /// A profile for a serverless function with 256 MB of memory (Vercel's
    /// smallest). Together with a caller's own caps on the source text and
    /// image bytes, a conversion stays under about 200 MB of process memory
    /// (measured in Node 24 with the WebAssembly build). Ordinary templates
    /// (well under 2 MB, under 20,000 XML nodes) are far inside it.
    pub fn serverless() -> Self {
        Limits {
            max_archive_bytes: 8 << 20,
            max_entries: 1_000,
            // Also bounds each picture in the output, which is validated
            // with these budgets.
            max_part_bytes: 12 << 20,
            max_total_part_bytes: 32 << 20,
            max_xml_depth: 256,
            // The in-memory tree costs a few hundred bytes per node.
            max_xml_nodes: 150_000,
            max_xml_attributes: 1_000,
            max_namespace_declarations: 1_000,
            max_text_node_bytes: 1 << 20,
            max_normalized_chars: 10_000_000,
            max_output_bytes: 32 << 20,
            max_diff_work: 10_000_000,
        }
    }

    /// A profile for trusted local input: only structural format limits apply.
    pub fn trusted() -> Self {
        Limits {
            max_archive_bytes: u64::MAX,
            max_entries: u64::MAX,
            max_part_bytes: u64::MAX,
            max_total_part_bytes: u64::MAX,
            max_xml_depth: 100_000,
            max_xml_nodes: u64::MAX,
            max_xml_attributes: u32::MAX,
            max_namespace_declarations: u32::MAX,
            max_text_node_bytes: u64::MAX,
            max_normalized_chars: u64::MAX,
            max_output_bytes: 0xFFFF_FFFE,
            max_diff_work: u64::MAX,
        }
    }

    pub(crate) fn check(limit: &'static str, allowed: u64, observed: u64) -> crate::Result<()> {
        if observed > allowed {
            Err(crate::Error::limit(limit, allowed, observed))
        } else {
            Ok(())
        }
    }
}
