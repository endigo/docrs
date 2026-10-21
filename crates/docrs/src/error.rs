//! Error contract.
//!
//! Every fallible operation returns [`Error`]. Its [`ErrorKind`] separates the
//! outcome classes the port must keep distinct:
//!
//! * [`ErrorKind::Refusal`] — a *safe refusal*. The operation changed nothing:
//!   the in-memory document, every live handle and every file on disk are
//!   exactly as they were. The seven [`RefusalKind`]s mirror upstream's
//!   `PaperRefusal` subclasses (`MalformedPackageError` included).
//! * [`ErrorKind::InvalidArgument`] — the caller passed something unusable
//!   (upstream raises `TypeError`/`ValueError`). Also leaves state unchanged.
//! * [`ErrorKind::ResourceLimit`] — an explicit [`crate::Limits`] budget was
//!   exceeded. This is a *policy* outcome, never evidence that the input is
//!   malformed; [`LimitExceeded`] names the budget.
//! * [`ErrorKind::Io`] — the filesystem failed. A failed save never replaces
//!   an existing destination, but it does not undo an earlier successful edit.
//! * [`ErrorKind::Internal`] — an invariant inside this crate failed (a bug).
//!
//! Machine-readable identity is [`Error::code`] (stable, snake_case) plus
//! [`Error::reason`] (a finer stable sub-code). Human messages are context
//! only and are not part of the compatibility contract.

use std::fmt;

use serde::Serialize;

/// The safe-refusal categories (upstream `PaperRefusal` subclasses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
    /// The file is not a package this library can safely read
    /// (`MalformedPackageError`).
    MalformedPackage,
    /// The target matches more than one location (`AmbiguousTargetError`).
    AmbiguousTarget,
    /// No location matches, or a captured handle went stale
    /// (`TargetNotFoundError`).
    TargetNotFound,
    /// The target involves structure the operation does not safely support
    /// (`UnsupportedStructureError`).
    UnsupportedStructure,
    /// The operation would cross a structural boundary
    /// (`BoundaryViolationError`).
    BoundaryViolation,
    /// The operation would create or modify a relationship unsafely
    /// (`RelationshipPolicyError`).
    RelationshipPolicy,
    /// The document enforces an editing restriction (`DocumentProtectedError`).
    DocumentProtected,
}

impl RefusalKind {
    /// Stable machine code.
    pub fn code(self) -> &'static str {
        match self {
            RefusalKind::MalformedPackage => "malformed_package",
            RefusalKind::AmbiguousTarget => "ambiguous_target",
            RefusalKind::TargetNotFound => "target_not_found",
            RefusalKind::UnsupportedStructure => "unsupported_structure",
            RefusalKind::BoundaryViolation => "boundary_violation",
            RefusalKind::RelationshipPolicy => "relationship_policy",
            RefusalKind::DocumentProtected => "document_protected",
        }
    }

    /// The upstream Python exception class this category corresponds to.
    pub fn upstream_class(self) -> &'static str {
        match self {
            RefusalKind::MalformedPackage => "MalformedPackageError",
            RefusalKind::AmbiguousTarget => "AmbiguousTargetError",
            RefusalKind::TargetNotFound => "TargetNotFoundError",
            RefusalKind::UnsupportedStructure => "UnsupportedStructureError",
            RefusalKind::BoundaryViolation => "BoundaryViolationError",
            RefusalKind::RelationshipPolicy => "RelationshipPolicyError",
            RefusalKind::DocumentProtected => "DocumentProtectedError",
        }
    }
}

/// Outcome class of an [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Safe refusal: nothing changed.
    Refusal(RefusalKind),
    /// Unusable caller argument: nothing changed.
    InvalidArgument,
    /// An explicit resource budget was exceeded: nothing changed.
    ResourceLimit,
    /// Filesystem failure.
    Io,
    /// Internal invariant failure (a bug in this crate).
    Internal,
}

/// Details of an exceeded [`crate::Limits`] budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LimitExceeded {
    /// The `Limits` field name, e.g. `"max_part_bytes"`.
    pub limit: &'static str,
    /// The configured budget.
    pub allowed: u64,
    /// The observed amount (a lower bound when work stopped early).
    pub observed: u64,
}

/// The single error type of this crate.
pub struct Error {
    kind: ErrorKind,
    reason: &'static str,
    message: String,
    limit: Option<LimitExceeded>,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    /// A safe refusal of `kind` with a stable `reason` sub-code.
    pub fn refusal(kind: RefusalKind, reason: &'static str, message: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Refusal(kind),
            reason,
            message: message.into(),
            limit: None,
            source: None,
        }
    }

    /// Shorthand for a [`RefusalKind::MalformedPackage`] refusal.
    pub fn malformed(reason: &'static str, message: impl Into<String>) -> Self {
        Self::refusal(RefusalKind::MalformedPackage, reason, message)
    }

    /// An invalid caller argument.
    pub fn invalid_argument(reason: &'static str, message: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::InvalidArgument,
            reason,
            message: message.into(),
            limit: None,
            source: None,
        }
    }

    /// An exceeded resource budget.
    pub fn limit(limit: &'static str, allowed: u64, observed: u64) -> Self {
        Error {
            kind: ErrorKind::ResourceLimit,
            reason: limit,
            message: format!(
                "resource budget {limit} exceeded: allowed {allowed}, observed at least {observed}; \
                 this is a configured policy limit, not evidence the input is malformed"
            ),
            limit: Some(LimitExceeded {
                limit,
                allowed,
                observed,
            }),
            source: None,
        }
    }

    /// A filesystem failure with context.
    pub fn io(reason: &'static str, message: impl Into<String>, source: std::io::Error) -> Self {
        Error {
            kind: ErrorKind::Io,
            reason,
            message: message.into(),
            limit: None,
            source: Some(Box::new(source)),
        }
    }

    /// An internal invariant failure.
    pub fn internal(reason: &'static str, message: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Internal,
            reason,
            message: message.into(),
            limit: None,
            source: None,
        }
    }

    /// The outcome class.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The refusal category, when this is a safe refusal.
    pub fn refusal_kind(&self) -> Option<RefusalKind> {
        match self.kind {
            ErrorKind::Refusal(kind) => Some(kind),
            _ => None,
        }
    }

    /// True for every outcome that guarantees nothing changed.
    pub fn is_refusal(&self) -> bool {
        matches!(self.kind, ErrorKind::Refusal(_))
    }

    /// Stable top-level machine code.
    pub fn code(&self) -> &'static str {
        match self.kind {
            ErrorKind::Refusal(kind) => kind.code(),
            ErrorKind::InvalidArgument => "invalid_argument",
            ErrorKind::ResourceLimit => "resource_limit",
            ErrorKind::Io => "io",
            ErrorKind::Internal => "internal",
        }
    }

    /// Stable finer-grained sub-code.
    pub fn reason(&self) -> &'static str {
        self.reason
    }

    /// Human-readable context (not a stable contract).
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A message with nothing taken from the input (no text, names, paths
    /// or values): the codes, and a budget's numbers. For logs that must not
    /// hold client data.
    pub fn redacted_message(&self) -> String {
        match &self.limit {
            Some(limit) => format!(
                "resource budget {} exceeded: allowed {}, observed at least {}",
                limit.limit, limit.allowed, limit.observed
            ),
            None => format!("{} ({})", self.code(), self.reason),
        }
    }

    /// Budget details for [`ErrorKind::ResourceLimit`].
    pub fn limit_exceeded(&self) -> Option<&LimitExceeded> {
        self.limit.as_ref()
    }

    /// Prefix the human message with more context, keeping identity.
    pub fn context(mut self, context: impl fmt::Display) -> Self {
        self.message = format!("{context}: {}", self.message);
        self
    }

    /// JSON diagnostic (`schema_version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "error": {
                "code": self.code(),
                "reason": self.reason,
                "message": self.message,
                "refusal": self.is_refusal(),
                "limit": self.limit,
            }
        })
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.code())
            .field("reason", &self.reason)
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.code(), self.reason, self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Crate result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
