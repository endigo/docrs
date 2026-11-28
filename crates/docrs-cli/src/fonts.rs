//! Font files given with `--font` and `--font-dir`.

use std::path::{Path, PathBuf};

use docrs::layout::{FontInfo, FontKind, FontSet};
use docrs::{Error, Result};
use serde_json::{Value, json};

use crate::FontArgs;

/// The fonts read, each file's faces, and the directory files skipped.
pub struct Loaded {
    pub set: FontSet,
    pub files: Vec<(PathBuf, Vec<FontInfo>)>,
    pub skipped: Vec<(PathBuf, Error)>,
}

impl Loaded {
    /// The `fonts_skipped` JSON entries.
    pub fn skipped_json(&self) -> Vec<Value> {
        self.skipped
            .iter()
            .map(|(path, error)| {
                json!({"path": path, "reason": error.reason(), "message": error.message()})
            })
            .collect()
    }

    /// One line per skipped file.
    pub fn skipped_text(&self) -> String {
        self.skipped
            .iter()
            .map(|(path, error)| {
                format!("\n  font skipped: {}: {}", path.display(), error.message())
            })
            .collect()
    }

    /// Adds `fonts_skipped` to a JSON report when a file was skipped.
    pub fn report_skipped(&self, value: &mut Value) {
        if !self.skipped.is_empty() {
            value["fonts_skipped"] = json!(self.skipped_json());
        }
    }
}

/// A `--font-dir` file larger than this is skipped unread.
const MAX_DIR_FONT_BYTES: u64 = 64 << 20;

const FONT_EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

/// Reads `--font` files, failing on the first bad one, and the font files
/// of each `--font-dir`, skipping those that cannot be read, are too large
/// or are not supported.
pub fn load(args: &FontArgs) -> Result<Loaded> {
    let mut builder = FontSet::builder();
    let mut loaded = Loaded {
        set: FontSet::default(),
        files: Vec::new(),
        skipped: Vec::new(),
    };
    for path in &args.fonts {
        let faces = read_font(path).and_then(|bytes| {
            builder
                .add(bytes)
                .map_err(|err| Error::invalid_argument(err.reason(), in_file(path, &err)))
        })?;
        loaded.files.push((path.clone(), faces));
    }
    for dir in &args.font_dirs {
        for path in dir_files(dir)? {
            let read = within_cap(&path).and_then(|()| read_font(&path));
            match read.and_then(|bytes| builder.add(bytes)) {
                Ok(faces) => loaded.files.push((path, faces)),
                Err(err) => loaded.skipped.push((path, err)),
            }
        }
    }
    for fallback in &args.fallbacks {
        let (kind, family) = fallback_of(fallback)?;
        builder.fallback(kind, family);
    }
    if args.metric_aliases {
        builder.metric_aliases();
    }
    loaded.set = builder.build();
    Ok(loaded)
}

/// `KIND=FAMILY` of `--fallback`.
fn fallback_of(arg: &str) -> Result<(FontKind, &str)> {
    let (kind, family) = arg.split_once('=').unwrap_or((arg, ""));
    let kind = match kind.trim() {
        "serif" => FontKind::Serif,
        "sans" => FontKind::Sans,
        "mono" => FontKind::Mono,
        _ => return Err(bad_fallback(arg)),
    };
    match family.trim() {
        "" => Err(bad_fallback(arg)),
        family => Ok((kind, family)),
    }
}

fn bad_fallback(arg: &str) -> Error {
    Error::invalid_argument(
        "fallback_invalid",
        format!("--fallback {arg}: expected serif, sans or mono, then = and a family"),
    )
}

fn in_file(path: &Path, err: &Error) -> String {
    format!("font {}: {}", path.display(), err.message())
}

fn within_cap(path: &Path) -> Result<()> {
    let size = std::fs::metadata(path).map_err(|err| {
        let message = format!("reading font {}: {err}", path.display());
        Error::io("font_unreadable", message, err)
    })?;
    if size.len() > MAX_DIR_FONT_BYTES {
        return Err(Error::invalid_argument(
            "font_file_too_large",
            format!(
                "{} MiB, over the {} MiB a --font-dir file may have",
                size.len() >> 20,
                MAX_DIR_FONT_BYTES >> 20
            ),
        ));
    }
    Ok(())
}

fn read_font(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|err| {
        let message = format!("reading font {}: {err}", path.display());
        Error::io("font_unreadable", message, err)
    })
}

/// The directory's font files by extension, sorted, hidden files left out.
fn dir_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let unreadable = |err| {
        let message = format!("reading font directory {}: {err}", dir.display());
        Error::io("font_dir_unreadable", message, err)
    };
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(unreadable)? {
        let path = entry.map_err(unreadable)?.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        let font = path.extension().is_some_and(|extension| {
            FONT_EXTENSIONS.contains(&extension.to_string_lossy().to_ascii_lowercase().as_str())
        });
        if font && !hidden && !path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// `regular`, `bold`, `italic` or `bold italic`.
pub fn style(face: &FontInfo) -> &'static str {
    match (face.bold, face.italic) {
        (false, false) => "regular",
        (true, false) => "bold",
        (false, true) => "italic",
        (true, true) => "bold italic",
    }
}
