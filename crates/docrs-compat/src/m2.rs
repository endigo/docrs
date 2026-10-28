//! M2 read/locate parity: outlines and searches against the oracle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use docrs::story::BlockKind;
use docrs::{Document, FindOptions, Limits, MatchPolicy, View};
use serde_json::{Value, json};

use crate::{hostile, oracle, report};

const VIEWS: [View; 3] = [View::Current, View::Original, View::All];

fn take_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn last_chars(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(n)).collect()
}

type Needles = BTreeSet<(
    String,
    &'static str,
    &'static str,
    Option<usize>,
    Option<String>,
)>;

fn add(out: &mut Needles, needle: String, view: View, policy: MatchPolicy) {
    if !needle.is_empty() {
        out.insert((needle, view.as_str(), policy.as_str(), None, None));
    }
}

fn needles(doc: &Document, budget: usize) -> Vec<Value> {
    let mut out: Needles = BTreeSet::new();
    let fixed = [
        "\u{FFFC}", "\t", " ", "e", "the", "ß", "SS", "\n", "  ", "-", "'", "\"", "a b", "Ü", "ᠮ",
    ];
    for view in VIEWS {
        for policy in [MatchPolicy::Exact, MatchPolicy::Normalized] {
            for needle in fixed {
                add(&mut out, needle.to_string(), view, policy);
            }
        }
        let Ok(blocks) = doc.blocks(view) else {
            continue;
        };
        let mut words: Vec<String> = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            let tokens: Vec<&str> = block.text.split_whitespace().collect();
            for token in tokens.iter().filter(|t| t.chars().count() >= 2) {
                words.push(token.to_string());
            }
            for pair in tokens.windows(2).take(3) {
                add(
                    &mut out,
                    format!("{} {}", pair[0], pair[1]),
                    view,
                    MatchPolicy::Exact,
                );
                add(
                    &mut out,
                    format!("{}  {}", pair[0].to_uppercase(), pair[1]),
                    view,
                    MatchPolicy::Normalized,
                );
            }
            if block.kind == BlockKind::Paragraph {
                add(
                    &mut out,
                    take_chars(&block.text, 12),
                    view,
                    MatchPolicy::Exact,
                );
                add(
                    &mut out,
                    last_chars(&block.text, 8),
                    view,
                    MatchPolicy::Exact,
                );
                if let Some(next) = blocks.get(i + 1)
                    && next.story == block.story
                    && !block.text.is_empty()
                    && !next.text.is_empty()
                {
                    let crossing = format!(
                        "{}\n{}",
                        last_chars(&block.text, 5),
                        take_chars(&next.text, 5)
                    );
                    add(&mut out, crossing.clone(), view, MatchPolicy::Exact);
                    add(
                        &mut out,
                        crossing.replace('\n', " "),
                        view,
                        MatchPolicy::Normalized,
                    );
                }
            }
            if out.len() > budget {
                break;
            }
        }
        words.sort();
        words.dedup();
        for word in words.iter().take(budget / 6) {
            add(&mut out, word.clone(), view, MatchPolicy::Exact);
            add(&mut out, word.to_uppercase(), view, MatchPolicy::Normalized);
        }
        if let Some(word) = words.first() {
            out.insert((word.clone(), view.as_str(), "exact", Some(1), None));
            out.insert((word.clone(), view.as_str(), "exact", Some(2), None));
            if let Some(context) = words.get(words.len() / 2) {
                out.insert((
                    word.clone(),
                    view.as_str(),
                    "exact",
                    None,
                    Some(context.clone()),
                ));
                out.insert((
                    word.clone(),
                    view.as_str(),
                    "normalized",
                    None,
                    Some(context.to_uppercase()),
                ));
            }
        }
    }
    out.into_iter()
        .take(budget)
        .map(|(needle, view, policy, nth, near)| json!({"needle": needle, "view": view, "match": policy, "nth": nth, "near": near}))
        .collect()
}

fn rust_find(doc: &Document, query: &Value) -> Value {
    let options = FindOptions {
        policy: MatchPolicy::parse(query["match"].as_str().unwrap_or("exact"))
            .unwrap_or(MatchPolicy::Exact),
        view: View::parse(query["view"].as_str().unwrap_or("current")).unwrap_or(View::Current),
        story: query["story"].as_str().map(str::to_string),
        nth: query["nth"].as_u64().map(|n| n as usize),
        near: query["near"].as_str().map(str::to_string),
    };
    match doc.find_text(query["needle"].as_str().unwrap_or(""), &options) {
        Ok(spans) => {
            json!({"outcome": "ok", "spans": spans.iter().map(|s| s.to_json()).collect::<Vec<_>>()})
        }
        Err(err) => json!({"outcome": "error", "category": err.code(), "reason": err.reason()}),
    }
}

pub fn run(python: &Path, report_path: Option<&Path>) -> Result<bool, String> {
    let mut requests = Vec::new();
    let mut expected: Vec<(String, &'static str, Value)> = Vec::new();
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut inputs = hostile::fixture_inputs(&["/corrupt/"]);
    inputs.extend(hostile::materialize(
        work.path(),
        hostile::synthetic_documents(),
    )?);
    for (rel, path) in inputs {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let doc =
            Document::open_bytes(&bytes, &Limits::default()).map_err(|e| format!("{rel}: {e}"))?;
        let mut outlines = serde_json::Map::new();
        for view in VIEWS {
            let outline = doc
                .outline(view)
                .map(|o| o.to_json())
                .unwrap_or_else(|e| json!({"error": e.code()}));
            outlines.insert(view.as_str().to_string(), outline);
        }
        requests
            .push(json!({"op": "outline", "path": path, "views": ["current", "original", "all"]}));
        expected.push((rel.clone(), "outline", Value::Object(outlines)));

        let budget = if rel.contains("large") { 12 } else { 160 };
        let queries = needles(&doc, budget);
        let results: Vec<Value> = queries.iter().map(|q| rust_find(&doc, q)).collect();
        requests.push(json!({"op": "find_many", "path": path, "queries": queries}));
        expected.push((
            rel.clone(),
            "find",
            json!({"queries": queries, "results": results}),
        ));
    }
    let oracle_results = oracle::run(python, &requests)?;
    let mut summary: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failures: Vec<Value> = Vec::new();
    for ((rel, kind, rust), python) in expected.iter().zip(oracle_results.iter()) {
        if *kind == "outline" {
            for view in ["current", "original", "all"] {
                let ok = python["outlines"][view] == rust[view];
                *summary
                    .entry(if ok {
                        "outline_match"
                    } else {
                        "outline_mismatch"
                    })
                    .or_default() += 1;
                if !ok {
                    failures.push(json!({"fixture": rel, "view": view, "rust": rust[view], "python": python["outlines"][view]}));
                }
            }
        } else {
            let queries = rust["queries"].as_array().cloned().unwrap_or_default();
            let rust_results = rust["results"].as_array().cloned().unwrap_or_default();
            let python_results = python["results"].as_array().cloned().unwrap_or_default();
            for ((query, r), p) in queries
                .iter()
                .zip(rust_results.iter())
                .zip(python_results.iter())
            {
                let ok = if r["outcome"] == "ok" {
                    r == p
                } else {
                    p["outcome"] == "error" && p["category"] == r["category"]
                };
                *summary
                    .entry(if ok { "find_match" } else { "find_mismatch" })
                    .or_default() += 1;
                let spans = r["spans"].as_array().map(Vec::len).unwrap_or(0);
                if spans > 0 {
                    *summary.entry("queries_with_matches").or_default() += 1;
                    *summary.entry("spans_compared").or_default() += spans;
                    if query["match"] == "normalized" {
                        *summary
                            .entry("normalized_queries_with_matches")
                            .or_default() += 1;
                    }
                }
                if r["outcome"] != "ok" {
                    *summary.entry("error_outcomes").or_default() += 1;
                }
                let crossing = r["spans"]
                    .as_array()
                    .map(|v| v.iter().filter(|s| s["crosses_paragraphs"] == true).count())
                    .unwrap_or(0);
                *summary.entry("crossing_spans").or_default() += crossing;
                if !ok {
                    failures.push(json!({"fixture": rel, "query": query, "rust": r, "python": p}));
                }
            }
        }
    }
    println!(
        "m2-parity: {}",
        serde_json::to_string(&summary).unwrap_or_default()
    );
    report::print_failures("MISMATCH ", &failures, 12, 900);
    report::write_json(
        report_path,
        &json!({"summary": summary, "failures": failures}),
    )?;
    Ok(failures.is_empty())
}
