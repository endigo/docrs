//! Shared JSON operation cases (`tests/cases/*.json`, see `docs/cases.md`).
//!
//! Each file: `{"schema_version": 1, "id", "fixture", "upstream": [...],
//! "cases": [{"needle", "with", "all"?, "match"?, "nth"?, "again"?,
//! "expect": {"outcome": "ok"|"error", "category"?, "changed_parts"?,
//! "reopen_contains"?, "reopen_absent"?}, "deviation"?}]}`. Every case runs on
//! a fresh document in Rust (checked against `expect`) and in the oracle
//! (checked against Rust), and refusals must leave an existing output
//! file untouched.

use std::path::{Path, PathBuf};

use docrs::{Document, FindOptions};
use serde_json::{Value, json};

use crate::{hostile, m3, opsrun, oracle, report};

fn contains(path: &Path, needle: &str) -> bool {
    Document::open(path)
        .ok()
        .and_then(|doc| doc.find_text(needle, &FindOptions::default()).ok())
        .is_some_and(|spans| !spans.is_empty())
}

fn check_expect(case: &Value, rust: &Value, out: &Path, sentinel: &[u8]) -> Vec<String> {
    let mut problems = Vec::new();
    let expect = &case["expect"];
    if expect.is_null() {
        return problems;
    }
    if expect["outcome"] != rust["outcome"] {
        problems.push(format!(
            "outcome {} != expected {}",
            rust["outcome"], expect["outcome"]
        ));
        return problems;
    }
    if !expect["category"].is_null() && expect["category"] != rust["category"] {
        problems.push(format!(
            "category {} != expected {}",
            rust["category"], expect["category"]
        ));
    }
    if rust["outcome"] == "error" {
        if std::fs::read(out).ok().as_deref() != Some(sentinel) {
            problems.push("refusal modified the existing output file".into());
        }
        return problems;
    }
    if !expect["changed_parts"].is_null()
        && expect["changed_parts"] != rust["patch"]["changed_parts"]
    {
        problems.push(format!(
            "changed_parts {} != expected {}",
            rust["patch"]["changed_parts"], expect["changed_parts"]
        ));
    }
    for needle in expect["reopen_contains"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !contains(out, needle) {
            problems.push(format!("reopened output lacks {needle:?}"));
        }
    }
    for needle in expect["reopen_absent"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if contains(out, needle) {
            problems.push(format!("reopened output still contains {needle:?}"));
        }
    }
    problems
}

/// (id, cases, rust results, rust outputs) for one case file.
type CaseRun = (String, Vec<Value>, Vec<Value>, Vec<PathBuf>);

pub fn run(dir: &Path, python: Option<&Path>, report_path: Option<&Path>) -> Result<bool, String> {
    let dir = if dir.is_relative() {
        oracle::repo_root().join(dir)
    } else {
        dir.to_path_buf()
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let sentinel = b"pre-existing output sentinel".to_vec();
    let mut requests = Vec::new();
    let mut runs: Vec<CaseRun> = Vec::new();
    let mut failures: Vec<Value> = Vec::new();
    let mut total = 0usize;
    for (file_index, file) in files.iter().enumerate() {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let spec: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        if spec["schema_version"] != 1 {
            return Err(format!("{}: schema_version must be 1", file.display()));
        }
        let id = spec["id"].as_str().unwrap_or("?").to_string();
        let fixture = spec["fixture"]
            .as_str()
            .ok_or_else(|| format!("{id}: missing fixture"))?;
        let input = if let Some(name) = fixture.strip_prefix("synthetic:") {
            let (_, bytes) = hostile::synthetic_documents()
                .into_iter()
                .chain(hostile::m3_documents())
                .find(|(n, _)| n == name)
                .ok_or_else(|| format!("{id}: unknown synthetic {name}"))?;
            let path = work
                .path()
                .join(format!("{}.docx", crate::opsrun::case_safe_file_stem(name)));
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            path
        } else {
            hostile::fixtures_dir().join(fixture)
        };
        let bytes = std::fs::read(&input).map_err(|e| format!("{id}: {e}"))?;
        let mut cases: Vec<Value> = spec["cases"].as_array().cloned().unwrap_or_default();
        let mut rust_results = Vec::new();
        let mut outs = Vec::new();
        for (case_index, case) in cases.iter_mut().enumerate() {
            total += 1;
            let rust_out = work
                .path()
                .join(format!("case-{file_index}-{case_index}.docx"));
            std::fs::write(&rust_out, &sentinel).map_err(|e| e.to_string())?;
            let result = m3::rust_case(&bytes, case, &rust_out);
            for problem in check_expect(case, &result, &rust_out, &sentinel) {
                failures.push(json!({"id": id, "case": case, "problem": problem}));
            }
            case["out"] = json!(
                work.path()
                    .join(format!("oracle-{file_index}-{case_index}.docx"))
            );
            rust_results.push(result);
            outs.push(rust_out);
        }
        requests.push(json!({"op": "replace_cases", "path": input, "cases": cases}));
        runs.push((id, cases, rust_results, outs));
    }
    let mut oracle_checked = 0usize;
    if let Some(python) = python {
        let results = oracle::run(python, &requests)?;
        for ((id, cases, rust_results, outs), oracle_result) in runs.iter().zip(results.iter()) {
            let python_results = oracle_result["results"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for (((case, rust), py), out) in cases
                .iter()
                .zip(rust_results)
                .zip(python_results.iter())
                .zip(outs)
            {
                oracle_checked += 1;
                let agree = if rust["outcome"] == "ok" && py["outcome"] == "ok" {
                    let parts: Vec<String> = opsrun::names(&rust["patch"]["changed_parts"])
                        .into_iter()
                        .collect();
                    m3::reports_equal(rust, py, case["all"] == true)
                        && m3::parts_equivalent(
                            out,
                            Path::new(case["out"].as_str().unwrap_or("")),
                            &parts,
                        )
                        .unwrap_or(false)
                } else {
                    rust["outcome"] == py["outcome"] && rust["category"] == py["category"]
                };
                if !agree && case.get("deviation").is_none() {
                    failures.push(json!({"id": id, "case": case, "problem": "rust and oracle disagree", "rust": rust, "python": py}));
                }
            }
        }
    }
    println!(
        "cases: {} files, {total} cases, {oracle_checked} oracle-compared, {} failures",
        files.len(),
        failures.len()
    );
    for failure in &failures {
        println!("  FAIL {failure}");
    }
    report::write_json(report_path, &json!({"failures": failures}))?;
    Ok(failures.is_empty())
}
