//! End-to-end CLI acceptance (contract checks 1, 3 and 5 through a real
//! process).

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_docrs"))
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/upstream")
        .join(rel)
}

const FRAGMENTED: &str = "generated/feature-isolated/fragmented-runs.docx";

#[test]
fn replace_writes_only_the_intended_correction() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.docx");
    std::fs::copy(fixture(FRAGMENTED), &input).unwrap();
    let out = dir.path().join("out.docx");
    let status = bin()
        .args(["--json", "replace"])
        .arg(&input)
        .args([
            "--find",
            "payment terms",
            "--with",
            "payment schedule",
            "--out",
        ])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(
        report["save"]["changed_parts"],
        serde_json::json!(["word/document.xml"])
    );

    let diff = bin()
        .args(["--json", "diff"])
        .arg(&input)
        .arg(&out)
        .output()
        .unwrap();
    let diff: serde_json::Value = serde_json::from_slice(&diff.stdout).unwrap();
    assert_eq!(diff["added"], serde_json::json!([]));
    assert_eq!(diff["removed"], serde_json::json!([]));
    assert_eq!(diff["changed"].as_array().unwrap().len(), 1);

    let found = bin()
        .args(["--json", "find"])
        .arg(&out)
        .arg("payment schedule")
        .output()
        .unwrap();
    let found: serde_json::Value = serde_json::from_slice(&found.stdout).unwrap();
    assert_eq!(found["count"], 1);
    let gone = bin()
        .arg("find")
        .arg(&out)
        .arg("payment terms")
        .status()
        .unwrap();
    assert_eq!(gone.code(), Some(3));
}

#[test]
fn refusal_preserves_an_existing_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    std::fs::write(&out, b"sentinel bytes").unwrap();
    let output = bin()
        .args(["--json", "replace"])
        .arg(fixture(FRAGMENTED))
        .args(["--find", "hr", "--with", "hour", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "ambiguous_target");
    assert_eq!(std::fs::read(&out).unwrap(), b"sentinel bytes");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1);
}

#[test]
fn same_path_save_and_corrupt_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.docx");
    std::fs::copy(fixture(FRAGMENTED), &path).unwrap();
    let status = bin()
        .arg("replace")
        .arg(&path)
        .args(["--find", "late surcharge", "--with", "late fee", "--out"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    let found = bin()
        .arg("find")
        .arg(&path)
        .arg("late fee")
        .status()
        .unwrap();
    assert!(found.success());

    let corrupt = bin()
        .args(["--json", "outline"])
        .arg(fixture("generated/corrupt/broken-rel.docx"))
        .output()
        .unwrap();
    assert_eq!(corrupt.status.code(), Some(3));
    let error: serde_json::Value = serde_json::from_slice(&corrupt.stdout).unwrap();
    assert_eq!(error["error"]["reason"], "broken_relationship_target");
}

fn json_of(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)))
}

#[test]
fn resolve_accepts_every_revision_like_the_ground_truth() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("accepted.docx");
    let listed = bin()
        .args(["--json", "revisions"])
        .arg(fixture("generated/redline/multiround.docx"))
        .output()
        .unwrap();
    let pending = json_of(&listed)["revisions"].as_array().unwrap().len();
    assert!(pending > 0);
    let resolved = bin()
        .args(["--json", "resolve"])
        .arg(fixture("generated/redline/multiround.docx"))
        .arg("--accept")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(resolved.status.success());
    assert_eq!(json_of(&resolved)["count"], pending);
    let after = bin()
        .args(["--json", "revisions"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(json_of(&after)["revisions"], serde_json::json!([]));
    // Visible text equals the upstream accepted ground truth.
    let diff = bin()
        .args(["--json", "text-diff"])
        .arg(&out)
        .arg(fixture("generated/redline/multiround-accepted.docx"))
        .output()
        .unwrap();
    let diff = json_of(&diff);
    for story in diff["stories"].as_array().unwrap() {
        assert_eq!(story["diff_lines"], serde_json::json!([]), "{story}");
    }
}

#[test]
fn append_reconciles_into_the_body_only() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("appended.docx");
    let output = bin()
        .args(["--json", "append"])
        .arg(fixture("generated/minimal-clean/minimal.docx"))
        .arg(fixture(FRAGMENTED))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = json_of(&output);
    assert_eq!(report["composition"]["schema"], "paper_composition");
    assert_eq!(
        report["save"]["changed_parts"],
        serde_json::json!(["word/document.xml"])
    );
    let found = bin()
        .arg("find")
        .arg(&out)
        .arg("payment terms")
        .status()
        .unwrap();
    assert!(found.success());
}

#[test]
fn composition_refusals_preserve_an_existing_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    std::fs::write(&out, b"sentinel bytes").unwrap();
    // Ambiguous destination anchor.
    let ambiguous = bin()
        .args(["--json", "insert-from"])
        .arg(fixture("generated/minimal-clean/minimal.docx"))
        .arg(fixture(FRAGMENTED))
        .args(["--start", "payment terms", "--anchor", "paragraph", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(ambiguous.status.code(), Some(3));
    assert_eq!(json_of(&ambiguous)["error"]["code"], "ambiguous_target");
    // Protected destination without acknowledgment.
    let protected = bin()
        .args(["--json", "append"])
        .arg(fixture(
            "generated/feature-isolated/protected-readonly.docx",
        ))
        .arg(fixture(FRAGMENTED))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(protected.status.code(), Some(3));
    assert_eq!(json_of(&protected)["error"]["code"], "document_protected");
    // Invalid option token.
    let invalid = bin()
        .args(["append"])
        .arg(fixture("generated/minimal-clean/minimal.docx"))
        .arg(fixture(FRAGMENTED))
        .args(["--styles", "merge", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(std::fs::read(&out).unwrap(), b"sentinel bytes");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1);
}

fn examples(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/markdown")
        .join(rel)
}

#[test]
fn from_markdown_converts_the_showcase_with_the_reference() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("showcase.docx");
    let output = bin()
        .args(["--json", "from-markdown"])
        .arg(examples("showcase.md"))
        .arg("--reference")
        .arg(examples("reference.docx"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = json_of(&output);
    assert_eq!(report["schema"], "docrs_from_markdown");
    let counts = &report["markdown"]["counts"];
    assert_eq!(counts["tables"], 2);
    assert_eq!(
        counts["images"], 3,
        "images resolve next to the Markdown file"
    );
    assert_eq!(counts["footnotes"], 2);
    assert_eq!(report["markdown"]["warnings"], serde_json::json!([]));
    assert_eq!(report["markdown"]["created_styles"], serde_json::json!([]));
    assert_eq!(
        report["markdown"]["styles"]["code_block"]["name"],
        "Source Code"
    );
    // The output reopens and reads back through the CLI.
    let outline = bin().args(["outline"]).arg(&out).output().unwrap();
    assert!(outline.status.success());
    assert!(String::from_utf8_lossy(&outline.stdout).contains("Quarterly Platform Review"));
}

#[test]
fn from_markdown_reads_stdin_and_fills_a_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("memo.docx");
    let mut child = bin()
        .args([
            "from-markdown",
            "-",
            "--placeholder",
            "{{content}}",
            "--reference",
        ])
        .arg(examples("letterhead.docx"))
        .arg("--out")
        .arg(&out)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"## Summary\n\nBody text.\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let outline = bin().args(["outline"]).arg(&out).output().unwrap();
    let text = String::from_utf8_lossy(&outline.stdout).to_string();
    let memo = text.find("MEMORANDUM").unwrap();
    let summary = text.find("Summary").unwrap();
    let footer = text.find("Confidential").unwrap();
    assert!(memo < summary && summary < footer, "{text}");
    assert!(!text.contains("{{content}}"));
}

#[test]
fn from_markdown_refusals_keep_the_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let markdown = dir.path().join("in.md");
    std::fs::write(&markdown, "```\ncode\n```\n\n![x](missing.png)\n").unwrap();
    let out = dir.path().join("out.docx");
    std::fs::write(&out, b"sentinel").unwrap();
    for (flag, code) in [("--strict-styles", 3), ("--strict", 3)] {
        let output = bin()
            .args(["from-markdown"])
            .arg(&markdown)
            .arg(flag)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{flag}");
        assert_eq!(std::fs::read(&out).unwrap(), b"sentinel", "{flag}");
    }
    let output = bin()
        .args(["from-markdown"])
        .arg(&markdown)
        .args(["--style", "nonsense=Normal", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(std::fs::read(&out).unwrap(), b"sentinel");
}

#[test]
fn markdown_reference_and_styles_commands() {
    let dir = tempfile::tempdir().unwrap();
    let reference = dir.path().join("reference.docx");
    let output = bin()
        .args(["markdown-reference", "--out"])
        .arg(&reference)
        .output()
        .unwrap();
    assert!(output.status.success());
    let styles = bin()
        .args(["--json", "markdown-styles"])
        .arg(&reference)
        .output()
        .unwrap();
    let report = json_of(&styles);
    let roles = report["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 40);
    let missing: Vec<&str> = roles
        .iter()
        .filter(|role| role["style"].is_null())
        .map(|role| role["role"].as_str().unwrap())
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
    let blank = bin().args(["markdown-styles"]).output().unwrap();
    let text = String::from_utf8_lossy(&blank.stdout);
    assert!(text.contains("code_block          missing"), "{text}");
}

#[test]
fn from_markdown_sets_aside_package_quirks_word_tolerates() {
    // The fixture whose ZIP directory entries hold empty compressed
    // streams: refused by the guarded reader, set aside for a reference.
    let reference = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/markdown/templates/docx-template-missing-pandoc-styles.docx");
    let dir = tempfile::tempdir().unwrap();
    let markdown = dir.path().join("in.md");
    std::fs::write(&markdown, "# Advice\n\nBody.\n").unwrap();
    let out = dir.path().join("out.docx");
    let run = |extra: &[&str]| {
        bin()
            .args(["--json", "from-markdown"])
            .arg(&markdown)
            .arg("--reference")
            .arg(&reference)
            .args(extra)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap()
    };
    let output = run(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = json_of(&output);
    assert_eq!(report["reference_set_aside"].as_array().unwrap().len(), 2);
    // The output itself opens under the strict reader.
    assert!(
        bin()
            .args(["outline"])
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success()
    );

    std::fs::remove_file(&out).unwrap();
    let output = run(&["--strict-package"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(!out.exists());
    // Cover handling is selectable; this template has no distinct first page.
    let output = run(&["--cover-page"]);
    assert!(output.status.success());
    assert!(json_of(&output)["markdown"]["cover_page"].is_null());
    assert!(!run(&["--cover-page", "--no-cover-page"]).status.success());
}

#[test]
fn replace_all_reports_the_count_and_refuses_when_nothing_matches() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    let output = bin()
        .args(["--json", "replace"])
        .arg(fixture(FRAGMENTED))
        .args([
            "--find",
            "payment terms",
            "--with",
            "payment schedule",
            "--all",
        ])
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report = json_of(&output);
    assert_eq!(report["replaced_count"], 1);
    assert_eq!(report["refused_count"], 0);
    assert_eq!(report["edit"]["replaced_count"], 1);

    // No match: a refusal (exit 3) like a single replace, and an existing
    // output file is left as it was.
    std::fs::write(&out, b"sentinel bytes").unwrap();
    for json in [true, false] {
        let mut command = bin();
        if json {
            command.arg("--json");
        }
        let output = command
            .arg("replace")
            .arg(fixture(FRAGMENTED))
            .args(["--find", "no such text", "--with", "x", "--all", "--out"])
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3));
        if json {
            let error = json_of(&output);
            assert_eq!(error["error"]["code"], "target_not_found");
            assert_eq!(error["error"]["reason"], "no_match");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("nothing replaced"), "{stderr}");
        }
        assert_eq!(std::fs::read(&out).unwrap(), b"sentinel bytes");
    }
}

#[test]
fn argument_errors_are_json_under_json() {
    for args in [
        &["--json", "replace", "x.docx", "--bogus"][..],
        &["replace", "x.docx", "--nth", "abc", "--json"],
        &["--json"],
        &["--json", "no-such-command"],
    ] {
        let output = bin().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error = json_of(&output);
        assert_eq!(error["schema_version"], 1, "{args:?}");
        assert_eq!(error["error"]["code"], "invalid_argument", "{args:?}");
        assert_eq!(error["error"]["refusal"], false, "{args:?}");
    }
    // Without --json, clap's own text on stderr.
    let output = bin().args(["replace", "--bogus"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--bogus"));
    // Help and version still print to stdout and succeed.
    for flag in ["--help", "--version"] {
        let output = bin().args(["--json", flag]).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("docrs"));
    }
}

#[test]
fn an_unreadable_font_is_an_io_error() {
    let output = bin()
        .args(["--json", "layout"])
        .arg(fixture(FRAGMENTED))
        .args(["--font", "/no/such/font.ttf"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5));
    let error = json_of(&output);
    assert_eq!(error["error"]["code"], "io");
    assert_eq!(error["error"]["reason"], "font_unreadable");
}

#[test]
fn a_panic_is_an_internal_error() {
    // The DOCRS_TEST_PANIC hook exists in debug builds only.
    if !cfg!(debug_assertions) {
        return;
    }
    let output = bin()
        .args(["--json", "diagnose"])
        .arg(fixture(FRAGMENTED))
        .env("DOCRS_TEST_PANIC", "1")
        .env_remove("RUST_BACKTRACE")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(70));
    let error = json_of(&output);
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["reason"], "internal_panic");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("panic requested by DOCRS_TEST_PANIC"),
        "{message}"
    );
    // The quiet hook: no "thread 'main' panicked" report.
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = bin()
        .arg("diagnose")
        .arg(fixture(FRAGMENTED))
        .env("DOCRS_TEST_PANIC", "1")
        .env_remove("RUST_BACKTRACE")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(70));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("docrs: internal (internal_panic)"),
        "{stderr}"
    );
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");
}
