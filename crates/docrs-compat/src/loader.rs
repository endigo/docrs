//! Independent-loader smoke: LibreOffice headless PDF conversion.
//!
//! The check passes only when a LibreOffice binary actually starts and
//! produces a non-empty PDF for every input. A launcher that exists but
//! cannot run (e.g. pointing at a removed application bundle) is reported as
//! an environment failure, never as a pass.

use std::path::{Path, PathBuf};
use std::process::Command;

fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(explicit) = std::env::var("DOCRS_SOFFICE") {
        out.push(PathBuf::from(explicit));
    }
    for name in ["soffice", "libreoffice"] {
        if let Ok(path) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path) {
                let candidate = dir.join(name);
                if candidate.exists() {
                    out.push(candidate);
                }
            }
        }
    }
    out.push(PathBuf::from(
        "/Applications/LibreOffice.app/Contents/MacOS/soffice",
    ));
    out
}

/// The LibreOffice binary the gate would use, or why none works.
pub fn available() -> Result<PathBuf, String> {
    working_soffice()
}

fn working_soffice() -> Result<PathBuf, String> {
    let mut problems = Vec::new();
    for candidate in candidates() {
        match Command::new(&candidate).arg("--version").output() {
            Ok(output)
                if output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("LibreOffice") =>
            {
                return Ok(candidate);
            }
            Ok(output) => problems.push(format!(
                "{}: exit {:?}: {}",
                candidate.display(),
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(err) => problems.push(format!("{}: {err}", candidate.display())),
        }
    }
    Err(format!(
        "no working LibreOffice found: {}",
        problems.join("; ")
    ))
}

pub fn smoke(require: bool, files: &[PathBuf]) -> Result<bool, String> {
    let soffice = match working_soffice() {
        Ok(path) => path,
        Err(message) if require => return Err(format!("loader gate NOT satisfied: {message}")),
        Err(message) => {
            println!("loader smoke SKIPPED (not a pass): {message}");
            return Ok(true);
        }
    };
    let out = tempfile::tempdir().map_err(|e| e.to_string())?;
    let generated;
    let files = if files.is_empty() {
        generated = representative_outputs(out.path())?;
        generated.as_slice()
    } else {
        files
    };
    let mut ok = true;
    for file in files {
        let status = Command::new(&soffice)
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(out.path())
            .arg(file)
            .output()
            .map_err(|e| e.to_string())?;
        let pdf = out
            .path()
            .join(Path::new(file.file_stem().unwrap_or_default()).with_extension("pdf"));
        let converted = status.status.success()
            && std::fs::metadata(&pdf)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
        println!(
            "  {} {}",
            if converted { "converted" } else { "FAILED   " },
            file.display()
        );
        ok &= converted;
    }
    Ok(ok)
}

/// Outputs of every shipped workflow on frozen fixtures (no-op save,
/// replacement, tracked replacement, comment, accept-all, compare redline,
/// append), written to `dir` for conversion.
fn representative_outputs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    use docrs::composition::AppendOptions;
    use docrs::{Document, FindOptions, Limits, ReplaceOptions};

    let fixtures = crate::hostile::fixtures_dir();
    let open = |rel: &str| {
        Document::open_path(fixtures.join(rel), &Limits::default()).map_err(|e| e.to_string())
    };
    let date = Some(docrs::clock::Timestamp::from_unix(1_704_164_645));
    let fragmented = "generated/feature-isolated/fragmented-runs.docx";
    let mut outputs: Vec<(&str, Document)> = Vec::new();
    outputs.push(("noop", open("generated/minimal-clean/minimal.docx")?));
    let mut replaced = open(fragmented)?;
    let span = replaced
        .find_one("payment terms", &FindOptions::default())
        .map_err(|e| e.to_string())?;
    replaced
        .replace(&span, "payment schedule", &ReplaceOptions::default())
        .map_err(|e| e.to_string())?;
    outputs.push(("replace", replaced));
    let mut tracked = open(fragmented)?;
    let span = tracked
        .find_one("payment terms", &FindOptions::default())
        .map_err(|e| e.to_string())?;
    let options = ReplaceOptions {
        tracked: true,
        author: Some("Loader".into()),
        date,
        preserve_revision: false,
    };
    tracked
        .replace(&span, "payment schedule", &options)
        .map_err(|e| e.to_string())?;
    outputs.push(("tracked", tracked));
    let mut commented = open(fragmented)?;
    let span = commented
        .find_one("late surcharge", &FindOptions::default())
        .map_err(|e| e.to_string())?;
    commented
        .add_comment(&span, "Check the rate.", "Loader", None, date)
        .map_err(|e| e.to_string())?;
    outputs.push(("comment", commented));
    let mut accepted = open("generated/redline/multiround.docx")?;
    let mut revisions = accepted.revisions().map_err(|e| e.to_string())?;
    revisions
        .accept_all(&mut accepted, None)
        .map_err(|e| e.to_string())?;
    outputs.push(("accepted", accepted));
    let read = |rel: &str| std::fs::read(fixtures.join(rel)).map_err(|e| e.to_string());
    let options = docrs::compare::CompareOptions {
        author: "Loader".into(),
        date,
        granularity: docrs::compare::Granularity::Word,
        materialize: None,
    };
    let redline = docrs::compare::compare(
        &read("generated/redline/compare-original.docx")?,
        &read("generated/redline/compare-revised.docx")?,
        &options,
        &Limits::default(),
    )
    .map_err(|e| e.to_string())?;
    outputs.push(("redline", redline.document));
    let mut appended = open("generated/minimal-clean/minimal.docx")?;
    appended
        .append_document(&open(fragmented)?, &AppendOptions::default())
        .map_err(|e| e.to_string())?;
    outputs.push(("append", appended));
    let mut paths = Vec::new();
    for (name, document) in outputs {
        let path = dir.join(format!("{name}.docx"));
        document.save(&path).map_err(|e| e.to_string())?;
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_outputs_build_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let outputs = representative_outputs(dir.path()).unwrap();
        assert_eq!(outputs.len(), 7);
        for path in outputs {
            docrs::Document::open_path(&path, &docrs::Limits::default())
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        }
    }
}
