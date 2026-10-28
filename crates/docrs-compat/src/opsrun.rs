//! Shared ops cases (`tests/ops/*.json`): multi-step authoring scenarios run
//! on fresh documents through Rust adapters (`ops/`) and the oracle's
//! dynamic dispatcher, then compared step by step and part by part.
//!
//! Case file: `{"schema_version": 1, "id", "fixture", "upstream": [...],
//! "cases": [{"name", "steps": [{"op": "module.function", "args": [...],
//! "kwargs": {...}}], "expect": {"steps": [{"outcome", "category"?}],
//! "changed_parts"?, "added_parts"?, "reopen_contains"?, "reopen_absent"?},
//! "deviation"?}]}`.
//!
//! Comparison: per-step outcome, refusal category and returned value; the
//! changed and added part sets; every changed or added part's meaning
//! (relationships by resolved target, content types by the effective type of
//! each member, other XML by semantic equivalence); identical reopened
//! outlines; and the oracle must reopen every Rust output.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use docrs::package::content_types::{ContentTypes, parse_content_types};
use docrs::package::rels::{base_uri, parse_relationships};
use docrs::package::zip_read::{ZipContents, read_archive};
use docrs::xml::{self, trees_equivalent};
use docrs::{Document, FindOptions, Limits, View};
use serde_json::{Value, json};

use crate::ops::{self, Step};
use crate::{hostile, oracle, report};

fn strip_volatile(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "message")
                .map(|(k, v)| (k.clone(), strip_volatile(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_volatile).collect()),
        other => other.clone(),
    }
}

/// Run one case through Rust.
pub fn rust_run(bytes: &[u8], steps: &[Value], out: &Path) -> Value {
    let mut doc = match Document::open_bytes(bytes, &Limits::default()) {
        Ok(doc) => doc,
        Err(err) => {
            return json!({"outcome": "error", "category": err.code(), "reason": err.reason()});
        }
    };
    let mut results = Vec::new();
    for step in steps {
        let name = step["op"].as_str().unwrap_or("");
        let empty = serde_json::Map::new();
        let args = step["args"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let kwargs = step["kwargs"].as_object().unwrap_or(&empty);
        let Some(adapter) = ops::lookup(name) else {
            results.push(json!({"outcome": "unsupported_op", "op": name}));
            continue;
        };
        let call = Step { args, kwargs };
        results.push(match adapter(&mut doc, &call) {
            Ok(value) => json!({"outcome": "ok", "value": value}),
            Err(err) => json!({"outcome": "error", "category": err.code(), "reason": err.reason(), "message": err.message()}),
        });
    }
    match doc.save(out) {
        Ok(patch) => json!({"outcome": "ok", "steps": results, "patch": patch.to_json()}),
        Err(err) => {
            json!({"outcome": "error", "category": err.code(), "reason": err.reason(), "steps": results})
        }
    }
}

/// The strings of a JSON array, as a set.
pub(crate) fn names(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn rel_multiset(data: &[u8], member: &str) -> Option<Vec<(String, String, String, bool)>> {
    let source = member
        .replace("_rels/", "")
        .trim_end_matches(".rels")
        .to_string();
    let source = if source.is_empty() || source == "." {
        "/".to_string()
    } else {
        format!("/{source}")
    };
    let base = base_uri(&source).to_string();
    let rels = parse_relationships(data, &base).ok()?;
    let mut out: Vec<(String, String, String, bool)> = rels
        .into_iter()
        .map(|rel| {
            let target = if rel.external {
                rel.target.clone()
            } else {
                rel.target_part_name(&base).to_lowercase()
            };
            (rel.id, rel.rel_type, target, rel.external)
        })
        .collect();
    out.sort();
    Some(out)
}

fn effective_type(types: &ContentTypes, member: &str) -> Option<String> {
    types.lookup(&format!("/{member}")).map(str::to_string)
}

fn part_equivalent(name: &str, a: &ZipContents, b: &ZipContents) -> bool {
    let (Some(x), Some(y)) = (a.get(name), b.get(name)) else {
        return false;
    };
    if name.ends_with(".rels") {
        return rel_multiset(x, name) == rel_multiset(y, name);
    }
    if name == "[Content_Types].xml" {
        let (Ok(tx), Ok(ty)) = (parse_content_types(x), parse_content_types(y)) else {
            return false;
        };
        return a
            .members
            .iter()
            .filter(|m| b.get(&m.name).is_some())
            .all(|m| effective_type(&tx, &m.name) == effective_type(&ty, &m.name));
    }
    match (
        xml::parse(x, &Limits::default()),
        xml::parse(y, &Limits::default()),
    ) {
        (Ok(tx), Ok(ty)) => trees_equivalent(&tx, &ty),
        _ => x == y,
    }
}

fn contains(path: &Path, needle: &str) -> bool {
    Document::open(path)
        .ok()
        .and_then(|doc| {
            doc.find_text(
                needle,
                &FindOptions {
                    view: View::All,
                    ..FindOptions::default()
                },
            )
            .ok()
        })
        .is_some_and(|spans| !spans.is_empty())
}

fn compare(
    case: &Value,
    rust: &Value,
    python: &Value,
    rust_out: &Path,
    python_out: &Path,
) -> Vec<String> {
    let mut problems = Vec::new();
    if rust["outcome"] != "ok" {
        problems.push(format!("rust run failed: {rust}"));
        return problems;
    }
    let expect = &case["expect"];
    let rust_steps = rust["steps"].as_array().cloned().unwrap_or_default();
    for (index, step) in rust_steps.iter().enumerate() {
        if step["outcome"] == "unsupported_op" {
            problems.push(format!("step {index}: no Rust adapter for {}", step["op"]));
        }
        let expected = &expect["steps"][index];
        if !expected.is_null() {
            if expected["outcome"] != step["outcome"] {
                problems.push(format!(
                    "step {index}: outcome {} != expected {}",
                    step["outcome"], expected["outcome"]
                ));
            }
            if !expected["category"].is_null() && expected["category"] != step["category"] {
                problems.push(format!(
                    "step {index}: category {} != expected {}",
                    step["category"], expected["category"]
                ));
            }
        }
    }
    for key in ["changed_parts", "added_parts"] {
        if !expect[key].is_null() && names(&expect[key]) != names(&rust["patch"][key]) {
            problems.push(format!(
                "{key} {} != expected {}",
                rust["patch"][key], expect[key]
            ));
        }
    }
    for needle in expect["reopen_contains"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !contains(rust_out, needle) {
            problems.push(format!("reopened output lacks {needle:?}"));
        }
    }
    for needle in expect["reopen_absent"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if contains(rust_out, needle) {
            problems.push(format!("reopened output still contains {needle:?}"));
        }
    }
    if python.is_null() || case.get("deviation").is_some() {
        return problems;
    }
    if python["outcome"] != "ok" {
        problems.push(format!("oracle run failed: {python}"));
        return problems;
    }
    let python_steps = python["steps"].as_array().cloned().unwrap_or_default();
    for (index, (r, p)) in rust_steps.iter().zip(python_steps.iter()).enumerate() {
        if r["outcome"] != p["outcome"] {
            problems.push(format!(
                "step {index}: rust {} vs oracle {} ({})",
                r["outcome"],
                p["outcome"],
                strip_volatile(p)
            ));
        } else if r["outcome"] == "error" {
            if r["category"] != p["category"] {
                problems.push(format!(
                    "step {index}: category rust {} vs oracle {} ({})",
                    r["category"], p["category"], p["message"]
                ));
            }
        } else if strip_volatile(&r["value"]) != strip_volatile(&p["value"]) {
            problems.push(format!(
                "step {index}: value rust {} vs oracle {}",
                r["value"], p["value"]
            ));
        }
    }
    for key in ["changed_parts", "added_parts"] {
        if names(&rust["patch"][key]) != names(&python["patch"][key]) {
            problems.push(format!(
                "{key}: rust {} vs oracle {}",
                rust["patch"][key], python["patch"][key]
            ));
        }
    }
    let read = |path: &Path| {
        std::fs::read(path)
            .ok()
            .and_then(|b| read_archive(&b, &Limits::default()).ok())
    };
    match (read(rust_out), read(python_out)) {
        (Some(a), Some(b)) => {
            for name in
                names(&rust["patch"]["changed_parts"]).union(&names(&rust["patch"]["added_parts"]))
            {
                if !part_equivalent(name, &a, &b) {
                    problems.push(format!(
                        "part {name} differs semantically from the oracle output"
                    ));
                }
            }
            let outline = |path: &Path| {
                Document::open(path)
                    .ok()
                    .and_then(|d| d.outline(View::All).ok())
                    .map(|o| o.to_json())
            };
            if outline(rust_out) != outline(python_out) {
                problems.push("reopened outlines differ".into());
            }
        }
        _ => problems.push("could not read outputs".into()),
    }
    problems
}

pub fn run(
    dir: &Path,
    python: Option<&Path>,
    report_path: Option<&Path>,
    filter: Option<&str>,
) -> Result<bool, String> {
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
        .filter(|p| filter.is_none_or(|f| p.to_string_lossy().contains(f)))
        .collect();
    files.sort();
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut planned: Vec<(String, Value, Value, PathBuf, PathBuf)> = Vec::new();
    let mut requests = Vec::new();
    for (file_index, file) in files.iter().enumerate() {
        let spec: Value =
            serde_json::from_str(&std::fs::read_to_string(file).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{}: {e}", file.display()))?;
        if spec["schema_version"] != 1 {
            return Err(format!("{}: schema_version must be 1", file.display()));
        }
        let id = spec["id"].as_str().unwrap_or("?").to_string();
        let fixture = spec["fixture"]
            .as_str()
            .ok_or_else(|| format!("{id}: missing fixture"))?;
        let input = resolve_fixture(fixture, work.path())?;
        let bytes = std::fs::read(&input).map_err(|e| format!("{id}: {e}"))?;
        for (case_index, case) in spec["cases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let name = format!(
                "{id}/{}",
                case["name"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| case_index.to_string())
            );
            let rust_out = work
                .path()
                .join(format!("r-{file_index}-{case_index}.docx"));
            let python_out = work
                .path()
                .join(format!("p-{file_index}-{case_index}.docx"));
            let steps = case["steps"].as_array().cloned().unwrap_or_default();
            let rust = rust_run(&bytes, &steps, &rust_out);
            requests.push(json!({"op": "ops", "path": input, "out": python_out, "steps": steps}));
            planned.push((name, case, rust, rust_out, python_out));
        }
    }
    let python_results = match python {
        Some(python) => oracle::run(python, &requests)?,
        None => vec![Value::Null; planned.len()],
    };
    let mut summary: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut reopen = Vec::new();
    for ((name, case, rust, rust_out, python_out), py) in planned.iter().zip(python_results.iter())
    {
        let problems = compare(case, rust, py, rust_out, python_out);
        let verdict = if !problems.is_empty() {
            "fail"
        } else if case.get("deviation").is_some() {
            "deviation"
        } else {
            "pass"
        };
        *summary.entry(verdict).or_default() += 1;
        if !problems.is_empty() {
            failures.push(json!({"case": name, "problems": problems, "rust": rust, "python": py}));
        }
        if rust["outcome"] == "ok" {
            reopen.push(json!({"op": "open", "path": rust_out}));
        }
    }
    if let Some(python) = python {
        for (request, result) in reopen.iter().zip(oracle::run(python, &reopen)?.iter()) {
            if result["outcome"] == "ok" {
                *summary.entry("oracle_reopens").or_default() += 1;
            } else {
                failures.push(json!({"case": request["path"], "problems": ["oracle cannot reopen the Rust output"], "python": result}));
            }
        }
    }
    println!(
        "ops: {} files, {}",
        files.len(),
        serde_json::to_string(&summary).unwrap_or_default()
    );
    for failure in &failures {
        let text = format!("  FAIL {} :: {}", failure["case"], failure["problems"]);
        println!("{}", report::clipped(text, 1500));
    }
    report::write_json(
        report_path,
        &json!({"summary": summary, "failures": failures}),
    )?;
    Ok(failures.is_empty())
}

/// A file stem that stays distinct on case-insensitive filesystems: each
/// uppercase ASCII letter becomes `^` plus its lowercase form.
pub(crate) fn case_safe_file_stem(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('^');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `generated/...` fixtures or `synthetic:<name>` documents.
pub fn resolve_fixture(fixture: &str, work: &Path) -> Result<PathBuf, String> {
    if let Some(name) = fixture.strip_prefix("synthetic:") {
        let bytes = hostile::synthetic_document(name)
            .ok_or_else(|| format!("unknown synthetic document {name}"))?;
        let path = work.join(format!("{}.docx", case_safe_file_stem(name)));
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        return Ok(path);
    }
    if fixture == "new" {
        let path = work.join("new.docx");
        Document::new().save(&path).map_err(|e| e.to_string())?;
        return Ok(path);
    }
    Ok(hostile::fixtures_dir().join(fixture))
}
