//! End-to-end CLI acceptance (contract checks 1, 3 and 5 through a real
//! process).

use std::path::{Path, PathBuf};
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
    assert_eq!(roles.len(), 41);
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
    // This template has no distinct first page.
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
    let output = bin().args(["replace", "--bogus"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--bogus"));
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

fn tiptap_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/markdown/tiptap")
        .join(name)
}

#[test]
fn each_source_format_has_its_command() {
    let dir = tempfile::tempdir().unwrap();
    let convert = |command: &str, input: &Path, out: &Path| {
        bin()
            .args(["--json", command])
            .arg(input)
            .arg("--reference")
            .arg(examples("reference.docx"))
            .arg("--out")
            .arg(out)
            .output()
            .unwrap()
    };
    let outline_of = |out: &Path| {
        let outline = bin().args(["outline"]).arg(out).output().unwrap();
        String::from_utf8_lossy(&outline.stdout).into_owned()
    };

    let from_json = dir.path().join("json.docx");
    let output = convert("from-json", &tiptap_fixture("core.json"), &from_json);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report = json_of(&output);
    assert_eq!(report["schema"], "docrs_from_json");
    assert_eq!(report["markdown"]["counts"]["tables"], 2);
    assert_eq!(
        report["markdown"]["warnings"][0]["code"],
        "remote_image_skipped"
    );
    let text = outline_of(&from_json);
    assert!(text.contains("Advice") && text.contains("After"), "{text}");

    let from_html = dir.path().join("html.docx");
    let output = convert("from-html", &tiptap_fixture("core.html"), &from_html);
    let report = json_of(&output);
    assert_eq!(report["schema"], "docrs_from_html", "{report}");
    assert_eq!(report["markdown"]["counts"]["tables"], 2);
    assert_eq!(outline_of(&from_html), text);

    // The command decides the format, not the file name.
    let renamed = dir.path().join("page.txt");
    std::fs::copy(tiptap_fixture("core.json"), &renamed).unwrap();
    let output = convert("from-json", &renamed, &dir.path().join("renamed.docx"));
    assert!(output.status.success(), "{}", json_of(&output));
    let output = convert(
        "from-markdown",
        &tiptap_fixture("core.json"),
        &dir.path().join("compat.docx"),
    );
    let report = json_of(&output);
    assert_eq!(report["schema"], "docrs_from_markdown", "{report}");
    assert_eq!(report["markdown"]["counts"]["tables"], 2, "{report}");

    for command in [
        &["from-tiptap", "-"][..],
        &["from-markdown", "-", "--from", "tiptap"],
    ] {
        let out = dir.path().join("stdin.docx");
        let mut child = bin()
            .args(command)
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
            .write_all(br#"{"type":"doc","content":[{"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"From JSON"}]}]}"#)
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(outline_of(&out).contains("From JSON"), "{command:?}");
    }

    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "[1, 2]").unwrap();
    let output = convert("from-json", &bad, &dir.path().join("bad.docx"));
    assert!(!output.status.success());
    let error = json_of(&output);
    assert_eq!(error["error"]["reason"], "tiptap_json_invalid", "{error}");
    let output = bin()
        .args(["--json", "from-markdown", "--from", "docbook"])
        .arg(&bad)
        .arg("--out")
        .arg(dir.path().join("bad.docx"))
        .output()
        .unwrap();
    let error = json_of(&output);
    assert_eq!(error["error"]["reason"], "invalid_format", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tiptap"),
        "{error}"
    );
}

#[test]
fn replace_refuses_nth_with_all() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.docx");
    std::fs::copy(fixture(FRAGMENTED), &input).unwrap();
    let before = std::fs::read(&input).unwrap();
    let out = bin()
        .args(["--json", "replace"])
        .arg(&input)
        .args([
            "--find", "payment", "--with", "x", "--all", "--nth", "2", "--out",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["error"]["reason"], "argument_conflict");
    assert_eq!(std::fs::read(&input).unwrap(), before);
}

#[test]
fn layout_lines_prints_one_json_object_a_line() {
    let output = bin()
        .args(["layout", "--lines"])
        .arg(fixture(FRAGMENTED))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut records = stdout
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap());
    let table = records.next().unwrap();
    let looks = table["looks"].as_array().unwrap();
    assert!(table["links"].is_array());
    let lines: Vec<serde_json::Value> = records.collect();
    assert!(!lines.is_empty());
    for line in &lines {
        assert_eq!(line["page"], 1);
        assert_eq!(line["story"], "body");
        for key in ["x", "y", "width", "baseline"] {
            assert!(line[key].is_number(), "{key}: {line}");
        }
        assert!(line["baseline"].as_f64() > line["y"].as_f64());
        let runs = line["runs"].as_array().unwrap();
        let text: String = runs.iter().map(|r| r["text"].as_str().unwrap()).collect();
        assert_eq!(text, line["text"].as_str().unwrap());
        for run in runs {
            let look = &looks[run["look"].as_u64().unwrap() as usize];
            assert!(look["size"].as_f64().unwrap() > 0.0, "{look}");
        }
    }
    assert!(
        lines
            .iter()
            .any(|l| !l["text"].as_str().unwrap().is_empty())
    );
}

#[test]
fn layout_lines_ends_quietly_when_its_reader_stops() {
    use std::io::{BufRead, BufReader};
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("long.md");
    let docx = dir.path().join("long.docx");
    std::fs::write(&source, "A line of text.\n\n".repeat(3000)).unwrap();
    let status = bin()
        .arg("from-markdown")
        .arg(&source)
        .arg("--out")
        .arg(&docx)
        .status()
        .unwrap();
    assert!(status.success());
    let mut child = bin()
        .args(["layout", "--lines"])
        .arg(&docx)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(first.starts_with('{'), "{first}");
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const INTER_DIR: &str = "tests/fixtures/fonts/inter";

/// A font directory: Inter, a text file, a `.ttf` that is not a font and
/// a sparse `.otf` over the size cap.
fn font_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let inter = repo(INTER_DIR).join("Inter[opsz,wght].ttf");
    std::fs::copy(inter, dir.path().join("Inter.ttf")).unwrap();
    std::fs::write(dir.path().join("OFL.txt"), "licence").unwrap();
    std::fs::write(dir.path().join("broken.TTF"), "not a font").unwrap();
    let big = std::fs::File::create(dir.path().join("huge.otf")).unwrap();
    big.set_len(65 << 20).unwrap();
    dir
}

fn skipped(report: &serde_json::Value, key: &str) -> Vec<(String, String)> {
    report[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let path = std::path::Path::new(item["path"].as_str().unwrap());
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, item["reason"].as_str().unwrap().to_string())
        })
        .collect()
}

#[test]
fn fonts_lists_faces_and_the_directory_files_it_skips() {
    let dir = font_dir();
    let output = bin()
        .args(["--json", "fonts", "--font-dir"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report = json_of(&output);
    assert_eq!(report["schema"], "docrs_fonts");
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0]["faces"],
        serde_json::json!([{"family": "Inter", "bold": false, "italic": false, "style": "regular", "variable": true, "cff": false, "embeddable": true, "subsettable": true}])
    );
    assert_eq!(
        skipped(&report, "skipped"),
        [
            ("broken.TTF".into(), "font_format_unsupported".into()),
            ("huge.otf".into(), "font_file_too_large".into()),
        ]
    );

    let output = bin()
        .args(["--json", "fonts"])
        .arg(repo(INTER_DIR).join("OFL.txt"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        json_of(&output)["error"]["reason"],
        "font_format_unsupported"
    );
}

#[test]
fn a_supplied_font_measures_the_contents_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let convert = |fonts: &[&std::ffi::OsStr]| {
        let output = bin()
            .args(["--json", "from-markdown"])
            .arg(repo("tests/fixtures/pdf/sources/toc.md"))
            .arg("--reference")
            .arg(repo("tests/fixtures/pdf/references/inter.docx"))
            .arg("--out")
            .arg(dir.path().join("toc.docx"))
            .args(fonts)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        json_of(&output)
    };
    let without = convert(&[]);
    assert_eq!(
        without["markdown"]["toc"]["missing_fonts"],
        serde_json::json!(["Inter"])
    );
    assert!(without.get("fonts_skipped").is_none());
    let fonts = font_dir();
    let with = convert(&["--font-dir".as_ref(), fonts.path().as_os_str()]);
    assert_eq!(
        with["markdown"]["toc"]["missing_fonts"],
        serde_json::json!([])
    );
    assert_eq!(skipped(&with, "fonts_skipped").len(), 2);

    let output = bin()
        .args(["--json", "layout"])
        .arg(dir.path().join("toc.docx"))
        .arg("--font")
        .arg(repo(INTER_DIR).join("OFL.txt"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn pdf_preflight_prints_the_report() {
    let output = bin()
        .args(["--json", "pdf", "--preflight"])
        .arg(fixture("generated/feature-isolated/textbox.docx"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    let codes: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"pdf_text_box_skipped"), "{codes:?}");
    assert_eq!(report["set_aside"], serde_json::json!([]));
    let help = bin().arg("--help").output().unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(
        help.contains("pdf") && help.contains("experimental"),
        "{help}"
    );
}

/// The memo converted into the Inter reference, as a .docx in `dir`.
fn inter_memo(dir: &Path) -> PathBuf {
    let docx = dir.join("memo.docx");
    let output = bin()
        .arg("from-markdown")
        .arg(repo("tests/fixtures/pdf/sources/memo.md"))
        .arg("--reference")
        .arg(repo("tests/fixtures/pdf/references/inter.docx"))
        .arg("--out")
        .arg(&docx)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    docx
}

fn is_pdf(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(b"%PDF-1.7"))
}

#[test]
fn pdf_writes_the_document_with_the_fonts_given() {
    let dir = tempfile::tempdir().unwrap();
    let docx = inter_memo(dir.path());
    let pdf = dir.path().join("memo.pdf");
    let output = bin()
        .args(["--json", "pdf"])
        .arg(&docx)
        .arg("--out")
        .arg(&pdf)
        .arg("--font-dir")
        .arg(repo(INTER_DIR))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = json_of(&output);
    assert_eq!(report["schema"], "docrs_pdf");
    assert_eq!(report["pdf"]["pages"], 2);
    assert!(is_pdf(&pdf));

    let missing = dir.path().join("missing.pdf");
    let output = bin()
        .args(["--json", "pdf"])
        .arg(&docx)
        .arg("--out")
        .arg(&missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let error = &json_of(&output)["error"];
    assert_eq!(error["reason"], "pdf_font_program_missing");
    assert_eq!(error["details"]["missing_fonts"][0]["family"], "Inter");
    assert!(!missing.exists());

    let output = bin().arg("pdf").arg(&docx).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "--out or --preflight");
}

#[test]
fn pdf_refuses_to_write_over_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let docx = inter_memo(dir.path());
    let before = std::fs::read(&docx).unwrap();
    let hard = dir.path().join("hard.pdf");
    std::fs::hard_link(&docx, &hard).unwrap();
    let through_new_directory = dir.path().join("new").join("..").join("memo.docx");
    let mut outs = vec![docx.clone(), hard, through_new_directory];
    #[cfg(unix)]
    {
        let soft = dir.path().join("soft.pdf");
        std::os::unix::fs::symlink(&docx, &soft).unwrap();
        outs.push(soft);
        // Dangling until the write would create `nodir`.
        let dangling = dir.path().join("dangling.pdf");
        std::os::unix::fs::symlink("nodir/../memo.docx", &dangling).unwrap();
        outs.push(dangling);
    }
    for out in outs {
        let output = bin()
            .args(["--json", "pdf"])
            .arg(&docx)
            .arg("--out")
            .arg(&out)
            .arg("--font-dir")
            .arg(repo(INTER_DIR))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", out.display());
        assert_eq!(json_of(&output)["error"]["reason"], "output_is_input");
        assert_eq!(std::fs::read(&docx).unwrap(), before);
    }
    let reference = dir.path().join("reference.docx");
    std::fs::copy(repo("tests/fixtures/pdf/references/inter.docx"), &reference).unwrap();
    let kept = std::fs::read(&reference).unwrap();
    let output = bin()
        .args(["--json", "from-markdown"])
        .arg(repo("tests/fixtures/pdf/sources/memo.md"))
        .arg("--reference")
        .arg(&reference)
        .args(["--format", "pdf", "--out"])
        .arg(dir.path().join("other").join("..").join("reference.docx"))
        .arg("--font-dir")
        .arg(repo(INTER_DIR))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_of(&output)["error"]["reason"], "output_is_input");
    assert_eq!(std::fs::read(&reference).unwrap(), kept);
    for made in ["new", "nodir", "other"] {
        assert!(!dir.path().join(made).exists(), "a refusal left {made}/");
    }
}

#[test]
fn each_convert_command_writes_pdf_with_format_pdf() {
    let dir = tempfile::tempdir().unwrap();
    let sources = [
        ("from-markdown", repo("tests/fixtures/pdf/sources/memo.md")),
        ("from-html", dir.path().join("page.html")),
        ("from-json", dir.path().join("doc.json")),
    ];
    std::fs::write(
        &sources[1].1,
        "<h1>Title</h1><p>Some <strong>bold</strong> text.</p>",
    )
    .unwrap();
    std::fs::write(
        &sources[2].1,
        r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Hello"}]}]}"#,
    )
    .unwrap();
    for (command, source) in &sources {
        let out = dir.path().join(format!("{command}.pdf"));
        let output = bin()
            .args(["--json", command])
            .arg(source)
            .arg("--reference")
            .arg(repo("tests/fixtures/pdf/references/inter.docx"))
            .args(["--format", "pdf", "--out"])
            .arg(&out)
            .arg("--font-dir")
            .arg(repo(INTER_DIR))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{command}: {output:?}");
        let report = json_of(&output);
        assert!(report["pdf"]["pages"].as_u64().unwrap() >= 1, "{report}");
        assert!(report.get("save").is_none());
        assert!(is_pdf(&out), "{command}");
    }
    let docx = dir.path().join("named.pdf");
    let output = bin()
        .arg("from-html")
        .arg(&sources[1].1)
        .arg("--out")
        .arg(&docx)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(
        std::fs::read(&docx).unwrap().starts_with(b"PK"),
        "the extension never picks the format"
    );
    let output = bin()
        .arg("from-html")
        .arg(&sources[1].1)
        .args(["--format", "pdf", "--strict", "--out"])
        .arg(dir.path().join("strict.pdf"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "no font program for the default fonts"
    );
}
