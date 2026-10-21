//! M3 safe-replacement parity: generated replace cases run through Rust and
//! the oracle on fresh documents, comparing outcomes, reports, changed-part
//! sets and the semantic content of the edited parts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use docrs::package::zip_read::read_archive;
use docrs::xml::{self, trees_equivalent};
use docrs::{Document, FindOptions, Limits, MatchPolicy, ReplaceAllOptions, ReplaceOptions, View};
use serde_json::{Value, json};

use crate::{hostile, oracle};

fn word_counts(doc: &Document) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    if let Ok(blocks) = doc.blocks(View::Current) {
        for block in blocks {
            for word in block.text.split_whitespace() {
                let word: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
                if word.chars().count() >= 3 {
                    *counts.entry(word).or_default() += 1;
                }
            }
        }
    }
    counts
}

fn count_matches(doc: &Document, needle: &str) -> usize {
    doc.find_text(needle, &FindOptions::default())
        .map(|v| v.len())
        .unwrap_or(0)
}

fn cases_for(doc: &Document, budget: usize) -> Vec<Value> {
    let mut cases: Vec<Value> = Vec::new();
    let counts = word_counts(doc);
    let unique: Vec<&String> = counts
        .keys()
        .filter(|w| count_matches(doc, w) == 1)
        .take(6)
        .collect();
    let ambiguous: Vec<&String> = counts
        .keys()
        .filter(|w| count_matches(doc, w) >= 2)
        .take(3)
        .collect();
    for word in &unique {
        let chars: Vec<char> = word.chars().collect();
        let middle: String = if chars.len() >= 3 {
            std::iter::once(chars[0])
                .chain(std::iter::once('Q'))
                .chain(chars[2..].iter().copied())
                .collect()
        } else {
            format!("{word}Q")
        };
        for with in [
            format!("{word}X"),
            format!("Z{word}"),
            word.to_uppercase(),
            String::new(),
            middle,
            format!("{word} inserted text"),
        ] {
            cases.push(json!({"needle": word, "with": with}));
        }
        cases.push(json!({"needle": word, "with": word, "again": format!("{word}2")}));
        cases
            .push(json!({"needle": word, "with": format!("{word}1"), "again": format!("{word}2")}));
        cases.push(json!({"needle": word.to_uppercase(), "with": format!("{word}N"), "match": "normalized"}));
    }
    for word in &ambiguous {
        cases.push(json!({"needle": word, "with": format!("{word}A")}));
        cases.push(json!({"needle": word, "with": format!("{word}A"), "nth": 2}));
        cases.push(json!({"needle": word, "with": format!("{word}!"), "all": true}));
        cases.push(json!({"needle": word, "with": format!("{word} "), "all": true}));
        cases.push(
            json!({"needle": word.to_lowercase(), "with": "x", "all": true, "match": "normalized"}),
        );
    }
    if let Ok(blocks) = doc.blocks(View::Current) {
        for pair in blocks.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.story != b.story || a.text.chars().count() < 6 || b.text.chars().count() < 6 {
                continue;
            }
            let tail: String = a.text.chars().skip(a.text.chars().count() - 4).collect();
            let head: String = b.text.chars().take(4).collect();
            let needle = format!("{tail}\n{head}");
            if count_matches(doc, &needle) != 1 {
                continue;
            }
            cases.push(json!({"needle": needle, "with": format!("{tail}\nQ{}", head.chars().skip(1).collect::<String>())}));
            cases.push(json!({"needle": needle, "with": format!("{}Q {head}", tail.chars().take(3).collect::<String>())}));
            cases.push(json!({"needle": needle, "with": "joined"}));
            break;
        }
        for block in &blocks {
            let words: Vec<&str> = block.text.split(' ').collect();
            if let Some(pair) = words
                .windows(2)
                .find(|p| p[0].chars().count() >= 2 && p[1].chars().count() >= 2)
            {
                let needle = format!("{} {}", pair[0], pair[1]);
                if count_matches(doc, &needle) == 1 {
                    cases.push(
                        json!({"needle": needle, "with": format!("{} {}x", pair[0], pair[1])}),
                    );
                    cases
                        .push(json!({"needle": needle, "with": format!("{}{}", pair[0], pair[1])}));
                    break;
                }
            }
        }
    }
    // Tracked variants of every generated case (M5): same targets, real
    // w:del / w:ins revisions with a fixed author and date.
    let untracked: Vec<Value> = cases.clone();
    for case in untracked {
        let mut tracked = case.clone();
        tracked["tracked"] = json!(true);
        tracked["author"] = json!("Reviewer");
        tracked["date"] = json!("2024-01-02T03:04:05Z");
        cases.push(tracked);
    }
    cases.push(json!({"needle": "\u{FFFC}", "with": "x"}));
    cases.push(json!({"needle": "e", "with": "line\nbreak"}));
    cases.truncate(budget);
    cases
}

pub(crate) fn rust_case(doc_bytes: &[u8], case: &Value, out: &Path) -> Value {
    let run = || -> docrs::Result<Value> {
        let mut doc = Document::open_bytes(doc_bytes, &Limits::default())?;
        if case["acknowledge"] == true {
            doc.acknowledge_protection()?;
        }
        let policy = MatchPolicy::parse(case["match"].as_str().unwrap_or("exact"))?;
        let view = View::parse(case["view"].as_str().unwrap_or("current"))?;
        let needle = case["needle"].as_str().unwrap_or("");
        let with = case["with"].as_str().unwrap_or("");
        let replace_options = ReplaceOptions {
            tracked: case["tracked"] == true,
            author: case["author"].as_str().map(str::to_string),
            date: match case["date"].as_str() {
                Some(text) => Some(docrs::clock::Timestamp::parse(text)?),
                None => None,
            },
            preserve_revision: case["preserve_revision"] == true,
        };
        let mut payload = if case["all"] == true {
            let options = ReplaceAllOptions {
                policy,
                view,
                replace: replace_options.clone(),
                ..ReplaceAllOptions::default()
            };
            let report = doc.replace_all(needle, with, &options)?;
            let mut value = report.to_json();
            if let Some(results) = value["results"].as_array_mut() {
                for result in results {
                    if let Some(object) = result.as_object_mut() {
                        object.remove("changed");
                        object.remove("version");
                    }
                }
            }
            if let Some(refused) = value["refused"].as_array_mut() {
                for item in refused {
                    *item = json!({"story": item["story"], "anchor": item["anchor"], "error": item["error"]});
                }
            }
            value
        } else {
            let options = FindOptions {
                policy,
                view,
                nth: case["nth"].as_u64().map(|n| n as usize),
                ..FindOptions::default()
            };
            let span = doc.find_one(needle, &options)?;
            let report = doc.replace(&span, with, &replace_options)?;
            let mut value = report.to_json();
            value.as_object_mut().map(|o| o.remove("changed"));
            if let Some(again) = case["again"].as_str() {
                let outcome = match doc.replace(&span, again, &replace_options) {
                    Ok(_) => "ok".to_string(),
                    Err(err) => err.code().to_string(),
                };
                value["again"] = json!(outcome);
            }
            value
        };
        let patch = doc.save(out)?;
        payload.as_object_mut().map(|o| o.remove("version"));
        Ok(json!({"outcome": "ok", "result": payload, "patch": patch.to_json()}))
    };
    match run() {
        Ok(value) => value,
        Err(err) => {
            json!({"outcome": "error", "category": err.code(), "reason": err.reason(), "message": err.message()})
        }
    }
}

pub(crate) fn normalize_python_result(result: &Value, all: bool) -> Value {
    let mut value = result.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("version");
    }
    if all {
        if let Some(refused) = value["refused"].as_array_mut() {
            for item in refused {
                *item = json!({"story": item["story"], "anchor": item["anchor"], "error": item["error"]});
            }
        }
        if let Some(results) = value["results"].as_array_mut() {
            for item in results {
                item.as_object_mut().map(|o| o.remove("version"));
            }
        }
    }
    value
}

pub(crate) fn parts_equivalent(
    rust_out: &Path,
    python_out: &Path,
    parts: &[String],
) -> Result<bool, String> {
    let a = read_archive(
        &std::fs::read(rust_out).map_err(|e| e.to_string())?,
        &Limits::default(),
    )
    .map_err(|e| e.to_string())?;
    let b = read_archive(
        &std::fs::read(python_out).map_err(|e| e.to_string())?,
        &Limits::default(),
    )
    .map_err(|e| e.to_string())?;
    for part in parts {
        let (Some(x), Some(y)) = (a.get(part), b.get(part)) else {
            return Ok(false);
        };
        let (Ok(x), Ok(y)) = (
            xml::parse(x, &Limits::default()),
            xml::parse(y, &Limits::default()),
        ) else {
            return Ok(false);
        };
        if !trees_equivalent(&x, &y) {
            return Ok(false);
        }
    }
    let rust_doc = Document::open(rust_out).map_err(|e| e.to_string())?;
    let python_doc = Document::open(python_out).map_err(|e| e.to_string())?;
    let ro = rust_doc
        .outline(View::All)
        .map_err(|e| e.to_string())?
        .to_json();
    let po = python_doc
        .outline(View::All)
        .map_err(|e| e.to_string())?
        .to_json();
    Ok(ro == po)
}

/// (name, input, cases, rust results, rust outputs) for one document.
type Plan = (String, PathBuf, Vec<Value>, Vec<Value>, Vec<PathBuf>);

pub fn run(python: &Path, report: Option<&Path>) -> Result<bool, String> {
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut inputs: Vec<(String, PathBuf)> = hostile::all_fixtures()
        .into_iter()
        .filter(|rel| !rel.contains("/corrupt/") && !rel.contains("large"))
        .map(|rel| {
            let path = hostile::fixtures_dir().join(&rel);
            (rel, path)
        })
        .collect();
    for (name, bytes) in hostile::synthetic_documents()
        .into_iter()
        .chain(hostile::m3_documents())
    {
        let path = work.path().join(format!(
            "{}.docx",
            crate::opsrun::case_safe_file_stem(&name)
        ));
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        inputs.push((name, path));
    }

    let mut requests = Vec::new();
    let mut plans: Vec<Plan> = Vec::new();
    for (index, (name, path)) in inputs.iter().enumerate() {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let doc =
            Document::open_bytes(&bytes, &Limits::default()).map_err(|e| format!("{name}: {e}"))?;
        let mut cases = cases_for(&doc, 120);
        let extra = hostile::m3_extra_cases(name);
        for case in &extra {
            if case.get("deviation").is_some() || case.get("acknowledge").is_some() {
                continue;
            }
            let mut tracked = case.clone();
            tracked["tracked"] = json!(true);
            tracked["author"] = json!("Reviewer");
            tracked["date"] = json!("2024-01-02T03:04:05Z");
            cases.push(tracked);
        }
        cases.extend(extra);
        let mut rust_results = Vec::new();
        let mut rust_outs = Vec::new();
        for (case_index, case) in cases.iter_mut().enumerate() {
            let rust_out = work.path().join(format!("r-{index}-{case_index}.docx"));
            let python_out = work.path().join(format!("p-{index}-{case_index}.docx"));
            case["out"] = json!(python_out);
            rust_results.push(rust_case(&bytes, case, &rust_out));
            rust_outs.push(rust_out);
        }
        requests.push(json!({"op": "replace_cases", "path": path, "cases": cases}));
        plans.push((name.clone(), path.clone(), cases, rust_results, rust_outs));
    }
    let oracle_results = oracle::run(python, &requests)?;
    let mut summary: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failures: Vec<Value> = Vec::new();
    let mut outcomes: BTreeSet<String> = BTreeSet::new();
    for ((name, _path, cases, rust_results, rust_outs), python) in
        plans.iter().zip(oracle_results.iter())
    {
        let python_results = python["results"].as_array().cloned().unwrap_or_default();
        for (((case, rust), py), rust_out) in cases
            .iter()
            .zip(rust_results)
            .zip(python_results.iter())
            .zip(rust_outs)
        {
            let all = case["all"] == true;
            let verdict = if rust["outcome"] == "ok" && py["outcome"] == "ok" {
                let rust_result = &rust["result"];
                let py_result = normalize_python_result(&py["result"], all);
                let mut rust_cmp = rust_result.clone();
                rust_cmp.as_object_mut().map(|o| o.remove("schema"));
                let mut py_cmp = py_result.clone();
                py_cmp.as_object_mut().map(|o| o.remove("schema"));
                let changed_r: BTreeSet<String> = rust["patch"]["changed_parts"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let changed_p: BTreeSet<String> = py["patch"]["changed_parts"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let python_out = PathBuf::from(case["out"].as_str().unwrap_or(""));
                let parts: Vec<String> = changed_r.iter().cloned().collect();
                let equivalent = parts_equivalent(rust_out, &python_out, &parts).unwrap_or(false);
                outcomes.insert("ok".into());
                if rust_cmp != py_cmp {
                    "report_mismatch"
                } else if changed_r != changed_p {
                    "changed_parts_mismatch"
                } else if !equivalent {
                    "output_mismatch"
                } else {
                    "match"
                }
            } else if rust["outcome"] == "error" && py["outcome"] == "error" {
                outcomes.insert(format!(
                    "refused:{}",
                    rust["category"].as_str().unwrap_or("")
                ));
                if rust["category"] == py["category"] {
                    "match"
                } else {
                    "category_mismatch"
                }
            } else {
                "outcome_mismatch"
            };
            let verdict = if verdict != "match"
                && case.get("deviation").is_some()
                && rust["outcome"] == "error"
            {
                "deviation"
            } else {
                verdict
            };
            if case["tracked"] == true {
                let key = if rust["outcome"] == "ok" {
                    "tracked_ok"
                } else {
                    "tracked_refused"
                };
                *summary.entry(key).or_default() += 1;
                if rust["result"]["revision_ids"]
                    .as_array()
                    .is_some_and(|ids| !ids.is_empty())
                {
                    *summary.entry("tracked_with_revisions").or_default() += 1;
                }
            }
            *summary.entry(verdict).or_default() += 1;
            if verdict != "match" && verdict != "deviation" {
                failures.push(json!({"doc": name, "case": case, "verdict": verdict, "rust": rust, "python": py}));
            }
        }
    }
    // Every Rust output must reopen in the oracle too.
    let mut reopen_requests = Vec::new();
    for (_, _, _, rust_results, rust_outs) in &plans {
        for (result, out) in rust_results.iter().zip(rust_outs) {
            if result["outcome"] == "ok" {
                reopen_requests.push(json!({"op": "open", "path": out}));
            }
        }
    }
    let reopened = oracle::run(python, &reopen_requests)?;
    for (request, result) in reopen_requests.iter().zip(reopened.iter()) {
        let ok = result["outcome"] == "ok";
        *summary
            .entry(if ok {
                "python_reopens_rust_output"
            } else {
                "python_reopen_failed"
            })
            .or_default() += 1;
        if !ok {
            failures.push(json!({"verdict": "python_reopen_failed", "path": request["path"], "python": result}));
        }
    }
    println!(
        "m3-parity: {}",
        serde_json::to_string(&summary).unwrap_or_default()
    );
    println!(
        "  outcome kinds covered: {}",
        outcomes.into_iter().collect::<Vec<_>>().join(", ")
    );
    for failure in failures.iter().take(15) {
        let mut text = failure.to_string();
        if text.len() > 1400 {
            text.truncate(1400);
            text.push('…');
        }
        println!("  {text}");
    }
    if let Some(path) = report {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&json!({"summary": summary, "failures": failures}))
                .unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(failures.is_empty())
}
