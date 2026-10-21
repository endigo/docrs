//! Atomic filesystem saves (upstream `_write_bytes_atomically`).
//!
//! The output bytes are computed and validated *before* the filesystem is
//! touched. They are then written to a same-directory temporary file,
//! flushed and `fsync`ed, given the destination's permission bits, and
//! renamed over the destination (`rename(2)` on Unix, `MoveFileEx` with
//! replace semantics on Windows). Any failure before the rename leaves an
//! existing destination byte-for-byte intact and removes the temporary file.
//!
//! A destination symlink is followed: the link's target is replaced and the
//! link itself is kept. If the link changes while the save runs, the save
//! refuses before replacing anything.
//!
//! A failed save does not undo an earlier successful in-memory edit; the two
//! are separate operations.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, Result};

/// Failure-injection points for tests (`fault-injection` feature).
#[cfg(feature = "fault-injection")]
pub mod fault {
    use std::cell::Cell;

    /// Where an injected failure fires.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SavePoint {
        /// After the temporary file is created, before any byte is written.
        AfterTempCreate,
        /// After the bytes are written and synced, before permissions.
        AfterWrite,
        /// Immediately before the atomic rename.
        BeforeRename,
    }

    thread_local! {
        static FAIL_AT: Cell<Option<SavePoint>> = const { Cell::new(None) };
    }

    /// Arm a one-shot failure at `point` on this thread.
    pub fn fail_at(point: SavePoint) {
        FAIL_AT.with(|cell| cell.set(Some(point)));
    }

    /// Disarm.
    pub fn clear() {
        FAIL_AT.with(|cell| cell.set(None));
    }

    pub(crate) fn check(point: SavePoint) -> std::io::Result<()> {
        FAIL_AT.with(|cell| {
            if cell.get() == Some(point) {
                cell.set(None);
                Err(std::io::Error::other(format!(
                    "injected failure at {point:?}"
                )))
            } else {
                Ok(())
            }
        })
    }
}

#[cfg(feature = "fault-injection")]
use fault::SavePoint;

#[cfg(feature = "fault-injection")]
fn inject(point: SavePoint) -> std::io::Result<()> {
    fault::check(point)
}

#[cfg(not(feature = "fault-injection"))]
#[derive(Debug, Clone, Copy)]
enum SavePoint {
    AfterTempCreate,
    AfterWrite,
    BeforeRename,
}

#[cfg(not(feature = "fault-injection"))]
fn inject(_point: SavePoint) -> std::io::Result<()> {
    Ok(())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn io(reason: &'static str, message: impl Into<String>, err: std::io::Error) -> Error {
    Error::io(reason, message, err)
}

/// Resolve a symlink chain without requiring the final target to exist
/// (Python `os.path.realpath`, non-strict).
fn resolve_link(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&current)?;
                current = if target.is_absolute() {
                    target
                } else {
                    current
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(target)
                };
            }
            _ => return Ok(current),
        }
    }
    Err(std::io::Error::other("too many levels of symbolic links"))
}

#[cfg(unix)]
fn link_identity(meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn link_identity(meta: &fs::Metadata) -> (u64, u64) {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    (meta.len(), modified)
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_of(_meta: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn apply_mode(file: &File, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_mode(_file: &File, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Permission bits of an existing file (Unix), used for new destinations.
pub fn file_mode(path: &Path) -> Option<u32> {
    fs::metadata(path).ok().and_then(|meta| mode_of(&meta))
}

/// Atomically replace `path` with `data`.
///
/// An existing destination keeps its permission bits; a new one receives
/// `new_file_mode` (Unix) or the platform default.
pub fn write_atomic(path: &Path, data: &[u8], new_file_mode: Option<u32>) -> Result<()> {
    let link_state = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let destination = resolve_link(path).map_err(|err| {
                io(
                    "symlink_resolution",
                    format!("cannot resolve symlink {}", path.display()),
                    err,
                )
            })?;
            Some((link_identity(&meta), destination))
        }
        _ => None,
    };
    let destination = link_state
        .as_ref()
        .map(|(_, target)| target.clone())
        .unwrap_or_else(|| path.to_path_buf());
    let mode = fs::metadata(&destination)
        .ok()
        .and_then(|meta| mode_of(&meta))
        .or(new_file_mode);

    let directory = match destination.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&directory).map_err(|err| {
        io(
            "create_directory",
            format!("cannot create {}", directory.display()),
            err,
        )
    })?;
    let base = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            Error::invalid_argument(
                "invalid_output_path",
                format!("{} has no file name", path.display()),
            )
        })?;

    let (temp_path, mut file) = create_temp(&directory, &base)?;
    let result = (|| -> Result<()> {
        inject(SavePoint::AfterTempCreate)
            .map_err(|err| io("write_failed", "temporary file", err))?;
        file.write_all(data)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|err| {
                io(
                    "write_failed",
                    format!("writing {}", temp_path.display()),
                    err,
                )
            })?;
        inject(SavePoint::AfterWrite).map_err(|err| io("write_failed", "temporary file", err))?;
        if let Some(mode) = mode {
            apply_mode(&file, mode)
                .map_err(|err| io("permissions", "setting output permissions", err))?;
        }
        // Close before renaming. (On WebAssembly `File` is an inert stub
        // with no `Drop`; saving to a path is unsupported there anyway.)
        #[allow(clippy::drop_non_drop)]
        drop(file);
        if let Some((identity, target)) = &link_state {
            let current = fs::symlink_metadata(path).map_err(|err| {
                io(
                    "symlink_changed",
                    "destination symlink changed during save; nothing was replaced",
                    err,
                )
            })?;
            let unchanged = current.file_type().is_symlink()
                && link_identity(&current) == *identity
                && resolve_link(path).ok().as_ref() == Some(target);
            if !unchanged {
                return Err(io(
                    "symlink_changed",
                    "destination symlink changed during save; nothing was replaced",
                    std::io::Error::other("symlink changed"),
                ));
            }
        }
        inject(SavePoint::BeforeRename).map_err(|err| io("rename_failed", "before rename", err))?;
        fs::rename(&temp_path, &destination).map_err(|err| {
            io(
                "rename_failed",
                format!("replacing {}", destination.display()),
                err,
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
        return result;
    }
    sync_directory(&directory);
    Ok(())
}

fn create_temp(directory: &Path, base: &str) -> Result<(PathBuf, File)> {
    let pid = std::process::id();
    let mut last_error = None;
    for _ in 0..64 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let candidate = directory.join(format!("{base}.{pid:x}{nanos:x}{counter:x}.partial"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => last_error = Some(err),
            Err(err) => {
                return Err(io(
                    "temp_create_failed",
                    format!("creating a temporary file in {}", directory.display()),
                    err,
                ));
            }
        }
    }
    Err(io(
        "temp_create_failed",
        "could not create a unique temporary file",
        last_error.unwrap_or_else(|| std::io::Error::other("exhausted temporary names")),
    ))
}

#[cfg(unix)]
fn sync_directory(directory: &Path) {
    if let Ok(dir) = File::open(directory) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) {}
