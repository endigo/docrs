//! The OPC package: guarded reads, the relationship graph, lazily parsed XML
//! parts, patch saves and package diffs.
//!
//! A [`Package`] retains the original archive bytes and each member's
//! original inflated bytes. XML parts are parsed into [`XmlTree`] arenas on
//! demand (eagerly for the part kinds upstream parses on open). Nothing is
//! re-serialized unless it was modified, and a modified part whose result is
//! semantically identical to its original gets the original bytes back.
//!
//! **Compatibility deviation (preservation):** upstream rebuilds the package
//! from the relationship graph on save, silently dropping members that no
//! relationship reaches. This port keeps every member, so unknown or orphaned
//! parts survive a save untouched.

pub mod content_types;
pub mod diagnose;
pub mod diff;
pub mod names;
pub mod patch;
pub mod rels;
pub mod save;
pub mod zip_read;
pub mod zip_write;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use content_types::{ContentTypes, content_type_matches, parse_content_types};
use rels::{Relationship, is_relationship_type, parse_relationships, rels_member_for, rt};

use crate::unicode::casefold;
use crate::xml::{self, XmlTree};
use crate::{Error, Limits, Result};

/// Content types referenced by package validation.
pub mod ct {
    /// WordprocessingML main document.
    pub const WML_DOCUMENT_MAIN: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
    /// Core properties.
    pub const OPC_CORE_PROPERTIES: &str =
        "application/vnd.openxmlformats-package.core-properties+xml";
    /// Relationships.
    pub const OPC_RELATIONSHIPS: &str = super::rels::RELS_CONTENT_TYPE;
    /// Styles.
    pub const WML_STYLES: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
    /// SpreadsheetML styles.
    pub const SML_STYLES: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";
    /// Settings.
    pub const WML_SETTINGS: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
    /// Numbering.
    pub const WML_NUMBERING: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
    /// Font table.
    pub const WML_FONT_TABLE: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml";
    /// Header.
    pub const WML_HEADER: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
    /// Footer.
    pub const WML_FOOTER: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";
    /// Footnotes.
    pub const WML_FOOTNOTES: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
    /// Endnotes.
    pub const WML_ENDNOTES: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml";
    /// Comments.
    pub const WML_COMMENTS: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
    /// PresentationML comments.
    pub const PML_COMMENTS: &str =
        "application/vnd.openxmlformats-officedocument.presentationml.comments+xml";
    /// SpreadsheetML comments.
    pub const SML_COMMENTS: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml";
    /// Theme.
    pub const OFC_THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
    /// Comments extended (w15).
    pub const WML_COMMENTS_EXTENDED: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml";
    /// Comment IDs (w16cid).
    pub const WML_COMMENTS_IDS: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsIds+xml";
    /// Comments extensible (w16cex).
    pub const WML_COMMENTS_EXTENSIBLE: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtensible+xml";

    /// Content types upstream parses as XML when the package opens
    /// (`PartFactory.part_type_for` XmlPart classes).
    pub const EAGER_XML: &[&str] = &[
        OPC_CORE_PROPERTIES,
        WML_COMMENTS,
        WML_DOCUMENT_MAIN,
        WML_ENDNOTES,
        WML_FOOTER,
        WML_FOOTNOTES,
        WML_HEADER,
        WML_NUMBERING,
        WML_SETTINGS,
        WML_STYLES,
        WML_COMMENTS_EXTENDED,
        WML_COMMENTS_IDS,
        WML_COMMENTS_EXTENSIBLE,
    ];
}

const KNOWN_RELATIONSHIP_CONTENT_TYPES: &[(&str, &[&str])] = &[
    (rt::CORE_PROPERTIES, &[ct::OPC_CORE_PROPERTIES]),
    (rt::STYLES, &[ct::WML_STYLES, ct::SML_STYLES]),
    (rt::SETTINGS, &[ct::WML_SETTINGS]),
    (rt::NUMBERING, &[ct::WML_NUMBERING]),
    (rt::FONT_TABLE, &[ct::WML_FONT_TABLE]),
    (rt::HEADER, &[ct::WML_HEADER]),
    (rt::FOOTER, &[ct::WML_FOOTER]),
    (rt::FOOTNOTES, &[ct::WML_FOOTNOTES]),
    (rt::ENDNOTES, &[ct::WML_ENDNOTES]),
    (
        rt::COMMENTS,
        &[ct::WML_COMMENTS, ct::PML_COMMENTS, ct::SML_COMMENTS],
    ),
    (rt::THEME, &[ct::OFC_THEME]),
];

/// True for member names holding XML by OPC convention.
pub fn is_xml_part_name(name: &str) -> bool {
    name.ends_with(".xml") || name.ends_with(".rels")
}

/// One package member.
#[derive(Debug, Clone)]
pub struct Part {
    name: String,
    original: Option<Arc<[u8]>>,
    content_type: Option<String>,
    tree: OnceLock<XmlTree>,
    replaced: Option<Vec<u8>>,
    dirty: bool,
    removed: bool,
    reachable: bool,
    generation: u64,
    incarnation: u32,
}

impl Part {
    /// Member name (part name without the leading `/`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Declared content type, when the content-types item covers it.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// Bytes the member had when the package was opened: `None` for a part
    /// added in this session under a new name. A part added under the name
    /// of a removed member keeps that member's bytes here, so a patch save
    /// can tell an unchanged re-addition from a change.
    pub fn original(&self) -> Option<&[u8]> {
        self.original.as_deref()
    }

    /// Whether the part changed in memory since open.
    pub fn is_modified(&self) -> bool {
        self.dirty || self.replaced.is_some() || self.removed || self.original.is_none()
    }

    /// Whether a relationship chain from the package reaches this part.
    pub fn is_reachable(&self) -> bool {
        self.reachable
    }

    /// Commit generation of this part (bumped on each committed change).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How many times this member's name was removed and added again; the
    /// part's tree carries the same value ([`XmlTree::incarnation`]).
    pub fn incarnation(&self) -> u32 {
        self.incarnation
    }

    /// Current bytes (serialized when the tree was modified).
    pub fn current_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        if let Some(bytes) = &self.replaced {
            return std::borrow::Cow::Borrowed(bytes);
        }
        if self.dirty
            && let Some(tree) = self.tree.get()
        {
            return std::borrow::Cow::Owned(xml::serialize(tree));
        }
        std::borrow::Cow::Borrowed(self.original.as_deref().unwrap_or(&[]))
    }
}

/// A validated OPC package.
#[derive(Debug, Clone)]
pub struct Package {
    source: Option<Arc<[u8]>>,
    parts: Vec<Part>,
    index: HashMap<String, usize>,
    fold_index: HashMap<String, usize>,
    directories: Vec<String>,
    content_types: ContentTypes,
    main_part: String,
    limits: Limits,
}

impl Package {
    /// Validate and load `bytes` as a WordprocessingML package.
    pub fn open(bytes: &[u8], limits: &Limits) -> Result<Package> {
        Self::open_with(bytes, limits, zip_read::Tolerance::default()).map(|(package, _)| package)
    }

    /// [`Package::open`] with a [`zip_read::Tolerance`]; also returns what it
    /// set aside. A package that needed any tolerance keeps no source bytes,
    /// so even an unchanged save writes a clean archive.
    pub fn open_with(
        bytes: &[u8],
        limits: &Limits,
        tolerance: zip_read::Tolerance,
    ) -> Result<(Package, Vec<String>)> {
        let contents = zip_read::read_archive_with(bytes, limits, tolerance)?;
        let set_aside = contents.set_aside;
        let mut index = HashMap::new();
        let mut fold_index = HashMap::new();
        let mut parts = Vec::with_capacity(contents.members.len());
        for (position, member) in contents.members.into_iter().enumerate() {
            index.insert(member.name.clone(), position);
            fold_index.insert(casefold(&member.name), position);
            parts.push(Part {
                name: member.name,
                original: Some(Arc::from(member.data.into_boxed_slice())),
                content_type: None,
                tree: OnceLock::new(),
                replaced: None,
                dirty: false,
                removed: false,
                reachable: false,
                generation: 0,
                incarnation: 0,
            });
        }
        let mut package = Package {
            source: set_aside
                .is_empty()
                .then(|| Arc::from(bytes.to_vec().into_boxed_slice())),
            parts,
            index,
            fold_index,
            directories: contents.directories,
            content_types: ContentTypes::default(),
            main_part: String::new(),
            limits: limits.clone(),
        };
        package.load_graph()?;
        Ok((package, set_aside))
    }

    /// Actual member name matching `part_name` case-insensitively
    /// (`/`-prefixed or not), as Word and upstream resolve targets.
    pub fn resolve_member(&self, part_name: &str) -> Option<&str> {
        self.live_member_by_fold(part_name)
            .map(|index| self.parts[index].name.as_str())
    }

    /// Add a new XML part (transaction commit). The content-types item must
    /// already declare it.
    pub(crate) fn add_xml_part(
        &mut self,
        name: &str,
        content_type: &str,
        tree: XmlTree,
    ) -> Result<()> {
        let cell = OnceLock::new();
        let _ = cell.set(tree);
        self.place_new_part(Part {
            name: name.to_string(),
            original: None,
            content_type: Some(content_type.to_string()),
            tree: cell,
            replaced: None,
            dirty: true,
            removed: false,
            reachable: true,
            generation: 1,
            incarnation: 0,
        })
    }

    /// Add a new binary (non-XML) part such as an image (transaction
    /// commit). The content-types item must already declare it.
    pub(crate) fn add_binary_part(
        &mut self,
        name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<()> {
        self.place_new_part(Part {
            name: name.to_string(),
            original: None,
            content_type: Some(content_type.to_string()),
            tree: OnceLock::new(),
            replaced: Some(bytes),
            dirty: false,
            removed: false,
            reachable: true,
            generation: 1,
            incarnation: 0,
        })
    }

    /// Install a part added by a commit. A name no live part holds (case
    /// folded) is free: a removed member of exactly that name gets its slot
    /// back, keeping its original bytes and archive position so the patch
    /// save sees a change (or, with identical bytes, no change) rather than a
    /// removal plus an addition.
    fn place_new_part(&mut self, mut part: Part) -> Result<()> {
        if self.live_member_by_fold(&part.name).is_some() {
            return Err(Error::internal(
                "duplicate_part",
                format!("part {} already exists", part.name),
            ));
        }
        let folded = casefold(&part.name);
        match self.index.get(&part.name).copied() {
            Some(position) => {
                let slot = &mut self.parts[position];
                part.original = slot.original.take();
                part.generation = slot.generation + 1;
                // A new incarnation of the name: handles into the removed
                // member must not pass for this one.
                part.incarnation = slot.incarnation.wrapping_add(1);
                if let Some(tree) = part.tree.get_mut() {
                    tree.set_incarnation(part.incarnation);
                }
                *slot = part;
                self.fold_index.insert(folded, position);
            }
            None => {
                let position = self.parts.len();
                self.index.insert(part.name.clone(), position);
                self.fold_index.insert(folded, position);
                self.parts.push(part);
            }
        }
        Ok(())
    }

    /// Mark a member removed (transaction commit); it is left out of every
    /// listing and of the saved package, and its name is free for a later
    /// [`Package::add_xml_part`] or [`Package::add_binary_part`].
    pub(crate) fn remove_part(&mut self, name: &str) {
        if let Some(index) = self.index.get(name).copied() {
            self.parts[index].removed = true;
            self.parts[index].generation += 1;
        }
    }

    /// Check that a staged commit can be applied without failing part-way:
    /// new parts must not collide with live parts (other than those the same
    /// commit removes first) or each other, replaced trees must name existing
    /// parts, and a staged `[Content_Types].xml` must parse. Returns the
    /// parsed content types for [`Package::set_content_types`].
    pub(crate) fn preflight_commit<'a>(
        &self,
        new_parts: impl IntoIterator<Item = &'a str>,
        replaced: impl IntoIterator<Item = &'a str>,
        removed: &BTreeSet<String>,
        content_types: Option<&XmlTree>,
    ) -> Result<Option<ContentTypes>> {
        let mut seen = HashSet::new();
        for name in new_parts {
            let member = name.strip_prefix('/').unwrap_or(name);
            let taken = self
                .live_member_by_fold(name)
                .is_some_and(|index| !removed.contains(&self.parts[index].name));
            if taken || !seen.insert(casefold(member)) {
                return Err(Error::internal(
                    "duplicate_part",
                    format!("part {name} already exists"),
                ));
            }
        }
        for name in replaced {
            if !self.index.contains_key(name) {
                return Err(Error::internal(
                    "unknown_part",
                    format!("no part named {name:?}"),
                ));
            }
        }
        content_types
            .map(|tree| parse_content_types(&xml::serialize(tree)))
            .transpose()
    }

    /// Install content types validated by [`Package::preflight_commit`].
    pub(crate) fn set_content_types(&mut self, content_types: ContentTypes) {
        self.content_types = content_types;
    }

    fn member_by_fold(&self, part_name: &str) -> Option<usize> {
        let member = part_name.strip_prefix('/').unwrap_or(part_name);
        self.fold_index.get(&casefold(member)).copied()
    }

    fn live_member_by_fold(&self, part_name: &str) -> Option<usize> {
        self.member_by_fold(part_name)
            .filter(|index| !self.parts[*index].removed)
    }

    /// Validate content types and the relationship graph (upstream
    /// `PackageReader.from_file`), then eagerly parse the XML part kinds
    /// upstream parses on open.
    fn load_graph(&mut self) -> Result<()> {
        let content_types_index = self.member_by_fold("/[Content_Types].xml").ok_or_else(|| {
            Error::malformed(
                "missing_content_types",
                "required part '[Content_Types].xml' is missing",
            )
        })?;
        let types = parse_content_types(self.parts[content_types_index].original().unwrap_or(&[]))?;
        self.content_types = types;

        let package_rels = self.relationships_at("/")?;
        let office: Vec<&Relationship> = package_rels
            .iter()
            .filter(|rel| is_relationship_type(&rel.rel_type, rt::OFFICE_DOCUMENT))
            .collect();
        if office.len() > 1 {
            return Err(Error::malformed(
                "multiple_office_documents",
                "package contains multiple officeDocument relationships",
            ));
        }
        if office.first().is_some_and(|rel| rel.external) {
            return Err(Error::malformed(
                "external_office_document",
                "package officeDocument relationship is external",
            ));
        }

        // Depth-first walk mirroring `_walk_phys_parts`.
        let mut visited: HashSet<String> = HashSet::new();
        let mut reached: Vec<(usize, String)> = Vec::new();
        let mut stack: Vec<(String, Vec<Relationship>, usize)> =
            vec![("/".to_string(), package_rels.clone(), 0)];
        while let Some((source, relationships, position)) = stack.pop() {
            if position >= relationships.len() {
                continue;
            }
            let rel = relationships[position].clone();
            stack.push((source.clone(), relationships, position + 1));
            if rel.external {
                continue;
            }
            let part_name = rel.target_part_name(rels::base_uri(&source));
            let member = self.member_by_fold(&part_name).ok_or_else(|| {
                Error::malformed(
                    "broken_relationship_target",
                    format!(
                        "relationship {:?} targets missing package part {part_name}",
                        rel.id
                    ),
                )
            })?;
            let content_type = self.content_types.lookup(&part_name).ok_or_else(|| {
                Error::malformed(
                    "undeclared_content_type",
                    format!(
                        "relationship {:?} targets {part_name}, which has no declared content type",
                        rel.id
                    ),
                )
            })?;
            validate_target_content_type(&rel.rel_type, content_type, &part_name)?;
            if !visited.insert(casefold(&part_name)) {
                continue;
            }
            reached.push((member, content_type.to_string()));
            let child_rels = self.relationships_at(&part_name)?;
            stack.push((part_name, child_rels, 0));
        }
        for (member, content_type) in reached {
            self.parts[member].content_type = Some(content_type);
            self.parts[member].reachable = true;
        }
        // Content types for members no relationship reaches (informational).
        for position in 0..self.parts.len() {
            if self.parts[position].content_type.is_none() {
                let name = format!("/{}", self.parts[position].name);
                self.parts[position].content_type =
                    self.content_types.lookup(&name).map(str::to_string);
            }
        }

        let main = package_rels
            .iter()
            .find(|rel| rel.rel_type == rt::OFFICE_DOCUMENT && !rel.external)
            .ok_or_else(|| {
                Error::malformed(
                    "missing_main_document",
                    "no relationship of type officeDocument in the package relationships",
                )
            })?;
        let main_name = main.target_part_name("/");
        let main_index = self.member_by_fold(&main_name).ok_or_else(|| {
            Error::malformed(
                "broken_relationship_target",
                format!("main document part {main_name} is missing"),
            )
        })?;
        self.main_part = self.parts[main_index].name.clone();

        for position in 0..self.parts.len() {
            let eager = self.parts[position]
                .content_type
                .as_deref()
                .is_some_and(|ct| ct::EAGER_XML.contains(&ct));
            if eager && self.parts[position].reachable {
                let name = self.parts[position].name.clone();
                self.xml(&name)?;
            }
        }
        Ok(())
    }

    /// Relationships whose source is `part_name` (`/` for the package),
    /// parsed from the current bytes of the relationships part.
    pub fn relationships_at(&self, part_name: &str) -> Result<Vec<Relationship>> {
        let rels_member = rels_member_for(part_name);
        match self.member_by_fold(&rels_member) {
            Some(index) if !self.parts[index].removed => {
                let bytes = self.parts[index].current_bytes();
                parse_relationships(&bytes, rels::base_uri(part_name))
            }
            _ => Ok(Vec::new()),
        }
    }

    /// The member a relationship of `rel_type` from `source` targets.
    pub fn part_related_by(&self, source: &str, rel_type: &str) -> Result<Option<String>> {
        let source_name = rels::source_part_name(source);
        let rels = self.relationships_at(&source_name)?;
        Ok(
            rels::single_target(&rels, rel_type, &source_name)?.and_then(|target| {
                self.member_by_fold(&target)
                    .map(|index| self.parts[index].name.clone())
            }),
        )
    }

    /// Main document member name (e.g. `word/document.xml`).
    pub fn main_part_name(&self) -> &str {
        &self.main_part
    }

    /// Limits this package was opened with.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Original archive bytes.
    pub fn source_bytes(&self) -> Option<&[u8]> {
        self.source.as_deref()
    }

    /// Parsed content types (as opened).
    pub fn content_types(&self) -> &ContentTypes {
        &self.content_types
    }

    /// Inert directory records of the source archive.
    pub fn directories(&self) -> &[String] {
        &self.directories
    }

    /// Members in archive order (removed parts excluded).
    pub fn parts(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(|part| !part.removed)
    }

    /// Member by exact name.
    pub fn part(&self, name: &str) -> Option<&Part> {
        self.index
            .get(name)
            .map(|index| &self.parts[*index])
            .filter(|part| !part.removed)
    }

    /// Parsed XML for member `name` (parsed on first use).
    pub fn xml(&self, name: &str) -> Result<&XmlTree> {
        let index = *self
            .index
            .get(name)
            .ok_or_else(|| Error::internal("unknown_part", format!("no part named {name:?}")))?;
        let part = &self.parts[index];
        if let Some(tree) = part.tree.get() {
            return Ok(tree);
        }
        let bytes = part.current_bytes();
        let mut tree = xml::parse(&bytes, &self.limits).map_err(|err| err.into_error(name))?;
        tree.set_incarnation(part.incarnation);
        let _ = part.tree.set(tree);
        part.tree
            .get()
            .ok_or_else(|| Error::internal("tree_cell", "parsed tree was not retained"))
    }

    /// Replace the parsed tree of `name` (transaction commit).
    pub fn commit_tree(&mut self, name: &str, mut tree: XmlTree) -> Result<()> {
        let index = *self
            .index
            .get(name)
            .ok_or_else(|| Error::internal("unknown_part", format!("no part named {name:?}")))?;
        let part = &mut self.parts[index];
        if tree.incarnation() != part.incarnation {
            // Not a copy of the member's own tree: handles into the old one
            // must not pass for the new one.
            part.incarnation = part.incarnation.wrapping_add(1);
            tree.set_incarnation(part.incarnation);
        }
        part.tree = OnceLock::new();
        let _ = part.tree.set(tree);
        part.dirty = true;
        part.generation += 1;
        Ok(())
    }

    /// Replace a part's raw bytes (binary replacement).
    pub fn commit_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<()> {
        let index = *self
            .index
            .get(name)
            .ok_or_else(|| Error::internal("unknown_part", format!("no part named {name:?}")))?;
        let part = &mut self.parts[index];
        part.tree = OnceLock::new();
        part.replaced = Some(bytes);
        part.dirty = false;
        part.generation += 1;
        // New content from bytes is a new arena for any handle into the old.
        part.incarnation = part.incarnation.wrapping_add(1);
        Ok(())
    }

    /// Whether anything changed in memory since open.
    pub fn is_modified(&self) -> bool {
        self.parts.iter().any(Part::is_modified)
    }

    /// Patch-save to bytes (see [`patch::patch_bytes`]).
    pub fn patch_bytes(&self) -> Result<(Vec<u8>, patch::PatchReport)> {
        patch::patch_bytes(self)
    }

    pub(crate) fn raw_parts(&self) -> &[Part] {
        &self.parts
    }
}

fn validate_target_content_type(rel_type: &str, content_type: &str, part_name: &str) -> Result<()> {
    for (expected_type, expected_content) in KNOWN_RELATIONSHIP_CONTENT_TYPES {
        if !is_relationship_type(rel_type, expected_type) {
            continue;
        }
        if expected_content
            .iter()
            .any(|item| content_type_matches(content_type, item))
        {
            return Ok(());
        }
        return Err(Error::malformed(
            "relationship_content_type_mismatch",
            format!(
                "relationship type {rel_type:?} targets {part_name} with invalid content type {content_type:?}"
            ),
        ));
    }
    Ok(())
}
