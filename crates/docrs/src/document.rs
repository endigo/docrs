//! The document handle.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::package::patch::PatchReport;
use crate::package::zip_read::Tolerance;
use crate::package::{Package, ct, save};
use crate::{Error, Limits, Result};

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

/// The blank-document package (upstream `templates/default.docx`, MIT; see
/// `NOTICE.md`).
const DEFAULT_DOCX: &[u8] = include_bytes!("../assets/default.docx");

/// Session-local identity of one opened document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);

/// An opened WordprocessingML document.
///
/// The document owns its package and XML arenas. Editing takes `&mut self`,
/// so the safe API cannot mutate a document concurrently.
#[derive(Debug)]
pub struct Document {
    id: DocumentId,
    package: Package,
    source_path: Option<PathBuf>,
    /// Consumption tokens of spans used by successful text-changing edits.
    consumed: HashSet<u64>,
    /// In-memory protection override (never persisted).
    protection_acknowledged: bool,
}

impl Document {
    /// Open package bytes with explicit limits.
    pub fn open_bytes(bytes: &[u8], limits: &Limits) -> Result<Document> {
        Self::open_bytes_with(bytes, limits, Tolerance::default()).map(|(document, _)| document)
    }

    /// Open package bytes, tolerating the irregularities `tolerance` names.
    /// Returns what was set aside (empty when the package needed nothing).
    pub fn open_bytes_with(
        bytes: &[u8],
        limits: &Limits,
        tolerance: Tolerance,
    ) -> Result<(Document, Vec<String>)> {
        let (package, set_aside) = Package::open_with(bytes, limits, tolerance)?;
        let main = package
            .part(package.main_part_name())
            .ok_or_else(|| Error::internal("main_part", "main part vanished"))?;
        if main.content_type() != Some(ct::WML_DOCUMENT_MAIN) {
            return Err(Error::invalid_argument(
                "not_a_word_document",
                format!(
                    "the package is not a Word document: main part content type is {:?}",
                    main.content_type().unwrap_or("")
                ),
            ));
        }
        let document = Document {
            id: DocumentId(NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed)),
            package,
            source_path: None,
            consumed: HashSet::new(),
            protection_acknowledged: false,
        };
        Ok((document, set_aside))
    }

    /// Open a file with explicit limits.
    pub fn open_path(path: impl AsRef<Path>, limits: &Limits) -> Result<Document> {
        Self::open_path_with(path, limits, Tolerance::default()).map(|(document, _)| document)
    }

    /// [`Document::open_path`] with a [`Tolerance`]; see
    /// [`Document::open_bytes_with`].
    pub fn open_path_with(
        path: impl AsRef<Path>,
        limits: &Limits,
        tolerance: Tolerance,
    ) -> Result<(Document, Vec<String>)> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .map_err(|err| Error::io("read_failed", format!("reading {}", path.display()), err))?;
        let (mut document, set_aside) = Self::open_bytes_with(&bytes, limits, tolerance)?;
        document.source_path = Some(path.to_path_buf());
        Ok((document, set_aside))
    }

    /// Open a file with default limits.
    pub fn open(path: impl AsRef<Path>) -> Result<Document> {
        Self::open_path(path, &Limits::default())
    }

    /// A new blank document (upstream `Document()` with no argument).
    pub fn new() -> Document {
        Self::open_bytes(DEFAULT_DOCX, &Limits::default()).expect("bundled default.docx opens")
    }

    /// Session-local identity.
    pub fn id(&self) -> DocumentId {
        self.id
    }

    /// The package.
    pub fn package(&self) -> &Package {
        &self.package
    }

    pub(crate) fn package_mut(&mut self) -> &mut Package {
        &mut self.package
    }

    pub(crate) fn is_consumed(&self, token: u64) -> bool {
        self.consumed.contains(&token)
    }

    pub(crate) fn consume(&mut self, token: u64) {
        self.consumed.insert(token);
    }

    /// Whether protection was acknowledged for this open document.
    pub fn protection_acknowledged(&self) -> bool {
        self.protection_acknowledged
    }

    pub(crate) fn set_protection_acknowledged(&mut self, value: bool) {
        self.protection_acknowledged = value;
    }

    /// Turn Restrict Editing on (see [`crate::protection::set_protection`]).
    pub fn set_protection(&mut self, edit: &str) -> Result<crate::protection::ProtectionStatus> {
        crate::protection::set_protection(self, edit)
    }

    /// The `w:documentProtection` state (report-only).
    pub fn protection_status(&self) -> Result<crate::protection::ProtectionStatus> {
        crate::protection::status(self)
    }

    /// Explicitly override protection for this open document (in memory
    /// only; the setting is never removed). Returns the overridden status.
    pub fn acknowledge_protection(&mut self) -> Result<crate::protection::ProtectionStatus> {
        let status = crate::protection::status(self)?;
        self.protection_acknowledged = true;
        Ok(status)
    }

    /// Raw package mutation for tests only (`test-support` feature).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn package_mut_for_tests(&mut self) -> &mut Package {
        &mut self.package
    }

    /// Path the document was opened from, if any.
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// Patch-save to bytes. With no changes this is the original archive.
    pub fn patch_bytes(&self) -> Result<(Vec<u8>, PatchReport)> {
        self.package.patch_bytes()
    }

    /// Patch-save atomically to `path` (which may be the source path).
    ///
    /// An existing destination keeps its permission bits; a new one inherits
    /// the source file's. Nothing on disk changes unless the complete output
    /// was produced and validated.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<PatchReport> {
        let path = path.as_ref();
        let (bytes, report) = self.patch_bytes()?;
        let mode = self
            .source_path
            .as_deref()
            .and_then(save::file_mode)
            .or(Some(0o644));
        save::write_atomic(path, &bytes, mode)?;
        Ok(report)
    }
}

impl Document {
    /// Story parts in traversal order.
    pub fn stories(&self) -> Vec<crate::story::Story> {
        crate::story::stories(self)
    }

    /// Every block of every story under `view`.
    pub fn blocks(&self, view: crate::View) -> Result<Vec<crate::Block>> {
        crate::story::iter_blocks(self, view)
    }

    /// Inspection snapshot.
    pub fn outline(&self, view: crate::View) -> Result<crate::Outline> {
        crate::story::outline(self, view)
    }

    /// Every match of `needle`.
    pub fn find_text(
        &self,
        needle: &str,
        options: &crate::FindOptions,
    ) -> Result<Vec<crate::Span>> {
        crate::search::find_text(self, needle, options)
    }

    /// The single match of `needle`, or `target_not_found` / `ambiguous_target`.
    pub fn find_one(&self, needle: &str, options: &crate::FindOptions) -> Result<crate::Span> {
        crate::search::find_one(self, needle, options)
    }

    /// Replace one span's text, untracked, inside one proved owner region.
    ///
    /// Refusals (`ambiguous` alignment, boundary crossings, fields, controls,
    /// revisions, bookmarks, protection, stale or foreign spans) leave the
    /// document and the span unchanged and reusable. A successful
    /// text-changing replacement consumes the span and every clone of it.
    /// Identical text returns `changed: false` and consumes nothing.
    pub fn replace(
        &mut self,
        span: &crate::Span,
        new_text: &str,
        options: &crate::replace::ReplaceOptions,
    ) -> Result<crate::replace::ReplaceReport> {
        crate::replace::replace(self, span, new_text, options)
    }

    /// Replace every match in one atomic pass: per-match refusals are
    /// reported and the rest proceed; a stale match aborts the whole batch
    /// with nothing changed.
    pub fn replace_all(
        &mut self,
        needle: &str,
        new_text: &str,
        options: &crate::replace::ReplaceAllOptions,
    ) -> Result<crate::replace::ReplaceAllReport> {
        crate::replace::replace_all(self, needle, new_text, options)
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}
