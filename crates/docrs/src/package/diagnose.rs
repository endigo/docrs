//! Typed triage of a file before opening it (upstream `diagnose`).
//!
//! Returns a [`PackageDiagnosis`] instead of failing, so untrusted input can
//! be classified first. `kind` names what the file is (`docx`, `dotx`,
//! `docm`, `dotm`, `xlsx`, `pptx`, `opc-unknown`, `not-a-zip`,
//! `encrypted-or-legacy-binary`, `unsafe-archive`, `corrupt-zip`, `missing`,
//! and the port-specific `resource-limit`).

use std::path::Path;

use serde::Serialize;

use super::content_types::CT_NS;
use super::rels::parse_relationships;
use super::zip_read::{self, CONTENT_TYPES_NAME};
use crate::xml::{self, NodeData};
use crate::{ErrorKind, Limits};

const CFB_MAGIC: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";

/// Diagnosis result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageDiagnosis {
    /// The diagnosed path.
    pub path: String,
    /// Whether this crate can open it as a WordprocessingML document.
    pub readable: bool,
    /// What the file actually is.
    pub kind: String,
    /// Why it is not readable (empty when readable).
    pub problems: Vec<String>,
}

impl PackageDiagnosis {
    /// JSON (`schema: paper_diagnosis`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "paper_diagnosis",
            "version": 1,
            "path": self.path,
            "readable": self.readable,
            "kind": self.kind,
            "problems": self.problems,
        })
    }
}

/// Diagnose the file at `path`.
pub fn diagnose_path(path: &Path, limits: &Limits) -> PackageDiagnosis {
    let label = path.display().to_string();
    if !path.is_file() {
        return result(&label, false, "missing", vec!["file does not exist".into()]);
    }
    match std::fs::read(path) {
        Ok(bytes) => diagnose_bytes(&label, &bytes, limits),
        Err(err) => result(
            &label,
            false,
            "missing",
            vec![format!("file cannot be read: {err}")],
        ),
    }
}

fn result(path: &str, readable: bool, kind: &str, problems: Vec<String>) -> PackageDiagnosis {
    PackageDiagnosis {
        path: path.to_string(),
        readable,
        kind: kind.to_string(),
        problems,
    }
}

/// Diagnose in-memory bytes labeled `path`.
pub fn diagnose_bytes(path: &str, bytes: &[u8], limits: &Limits) -> PackageDiagnosis {
    if bytes.starts_with(CFB_MAGIC) {
        return result(
            path,
            false,
            "encrypted-or-legacy-binary",
            vec![
                "OLE compound file: either a password-protected Office document (decrypt it first) or a \
                 legacy binary .doc (convert to .docx)"
                    .into(),
            ],
        );
    }
    if !bytes.starts_with(b"PK") {
        if zip_read::has_end_record(bytes) {
            return result(
                path,
                false,
                "unsafe-archive",
                vec![
                    "a ZIP archive is present but does not begin at the start of the file; Word cannot open \
                     a .docx with anything in front of the package. Extract the archive to a file of its own \
                     and open that"
                        .into(),
                ],
            );
        }
        return result(
            path,
            false,
            "not-a-zip",
            vec!["not a ZIP archive, so not an OPC package".into()],
        );
    }
    let contents = match zip_read::read_archive(bytes, limits) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::ResourceLimit => {
            return result(
                path,
                false,
                "resource-limit",
                vec![err.message().to_string()],
            );
        }
        Err(err)
            if matches!(
                err.reason(),
                "zip_corrupt_extra_field" | "zip_unsupported_version" | "zip_invalid_name_encoding"
            ) =>
        {
            return result(
                path,
                false,
                "corrupt-zip",
                vec![format!("ZIP structure is damaged: {}", err.message())],
            );
        }
        Err(err) => {
            return result(
                path,
                false,
                "unsafe-archive",
                vec![err.message().to_string()],
            );
        }
    };

    let mut problems: Vec<String> = [CONTENT_TYPES_NAME, "_rels/.rels"]
        .iter()
        .filter(|name| contents.get(name).is_none())
        .map(|name| format!("required part {name:?} is missing"))
        .collect();
    let mut main_part = "word/document.xml".to_string();
    if let Some(rels) = contents.get("_rels/.rels")
        && let Ok(rels) = parse_relationships(rels, "/")
    {
        let mut sorted = rels.clone();
        sorted
            .sort_by(|a, b| (&a.id, &a.rel_type, &a.target).cmp(&(&b.id, &b.rel_type, &b.target)));
        if let Some(rel) = sorted
            .iter()
            .find(|rel| rel.rel_type.ends_with("/officeDocument") && !rel.external)
        {
            main_part = rel.target.trim_start_matches('/').to_string();
        }
    }
    let mut main_type: Option<String> = None;
    if let Some(types) = contents.get(CONTENT_TYPES_NAME) {
        match xml::parse(types, limits) {
            Ok(tree) => {
                let wanted = format!("/{main_part}");
                for node in tree.descendants(tree.root()) {
                    if matches!(tree.data(node), NodeData::Element(_))
                        && tree.is(node, CT_NS, "Override")
                        && tree.attr(node, None, "PartName") == Some(wanted.as_str())
                    {
                        main_type = tree.attr(node, None, "ContentType").map(str::to_string);
                    }
                }
            }
            Err(err) => problems.push(format!(
                "[Content_Types].xml is unparseable or unsafe: {err}"
            )),
        }
    }
    if contents.get(&main_part).is_none() {
        for (marker, kind) in [("xl/", "xlsx"), ("ppt/", "pptx")] {
            if contents
                .members
                .iter()
                .any(|member| member.name.starts_with(marker))
            {
                return result(
                    path,
                    false,
                    kind,
                    vec!["an OPC package, but not a WordprocessingML one".into()],
                );
            }
        }
        let mut all = vec![format!("no {main_part} main document part")];
        all.extend(problems);
        return result(path, false, "opc-unknown", all);
    }
    let kind = match main_type.as_deref() {
        None => "docx",
        Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
        ) => "docx",
        Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
        ) => "dotx",
        Some("application/vnd.ms-word.document.macroEnabled.main+xml") => "docm",
        Some("application/vnd.ms-word.template.macroEnabledTemplate.main+xml") => "dotm",
        Some(_) => "opc-unknown",
    };
    match kind {
        "docm" | "dotm" => problems.push(format!(
            "macro-enabled Office file ({kind}); this library opens only plain .docx — resave without macros"
        )),
        "dotx" => problems.push("Word template (.dotx); this library opens only .docx — resave as a document".into()),
        "opc-unknown" => problems.push(format!("unrecognized main-part content type {:?}", main_type.unwrap_or_default())),
        _ => {}
    }
    result(path, problems.is_empty(), kind, problems)
}
