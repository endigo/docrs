//! M6 parity: difflib port, text diffs and pending changes (compare cases
//! join here as compare lands).

use std::collections::BTreeMap;
use std::path::Path;

use docrs::difflib::{SequenceMatcher, Tag};
use docrs::textdiff::{pending_changes, text_diff};
use docrs::{Document, Limits, View};
use serde_json::{Value, json};

use crate::{hostile, oracle};

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn tag_name(tag: Tag) -> &'static str {
    match tag {
        Tag::Equal => "equal",
        Tag::Replace => "replace",
        Tag::Delete => "delete",
        Tag::Insert => "insert",
    }
}

fn sequence(rng: &mut XorShift, len: usize, alphabet: u64) -> Vec<String> {
    (0..len)
        .map(|_| format!("t{}", rng.below(alphabet)))
        .collect()
}

pub fn run(python: &Path) -> Result<bool, String> {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let mut pairs: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for round in 0..400 {
        let len = match round % 4 {
            0 => rng.below(12) as usize,
            1 => rng.below(80) as usize,
            2 => 190 + rng.below(40) as usize,
            _ => 400 + rng.below(1100) as usize,
        };
        let alphabet = [2, 5, 20, 300][round % 4];
        let a = sequence(&mut rng, len, alphabet);
        // b: a with random edits so matches are plentiful.
        let mut b = a.clone();
        for _ in 0..rng.below(1 + len as u64 / 4 + 3) {
            let op = rng.below(3);
            let at = if b.is_empty() {
                0
            } else {
                rng.below(b.len() as u64) as usize
            };
            match op {
                0 if !b.is_empty() => {
                    b.remove(at);
                }
                1 => b.insert(at, format!("t{}", rng.below(alphabet))),
                _ if !b.is_empty() => b[at] = format!("t{}", rng.below(alphabet)),
                _ => {}
            }
        }
        pairs.push((a, b));
    }
    let mut requests = vec![json!({"op": "difflib", "pairs": pairs})];
    let mut expected: Vec<(String, Value)> = Vec::new();
    for rel in hostile::all_fixtures() {
        if rel.contains("/corrupt/") {
            continue;
        }
        let path = hostile::fixtures_dir().join(&rel);
        let doc = Document::open_path(&path, &Limits::default()).map_err(|e| e.to_string())?;
        requests.push(json!({"op": "pending_changes", "path": path}));
        expected.push((
            format!("pending:{rel}"),
            pending_changes(&doc).map_err(|e| e.to_string())?.to_json(),
        ));
    }
    let redline = hostile::fixtures_dir().join("generated/redline");
    let pairs_of_docs = [
        ("compare-original.docx", "compare-revised.docx"),
        ("multiround.docx", "multiround-accepted.docx"),
    ];
    for (a, b) in pairs_of_docs {
        for view in [View::Current, View::Original, View::All] {
            let (pa, pb) = (redline.join(a), redline.join(b));
            let da = Document::open_path(&pa, &Limits::default()).map_err(|e| e.to_string())?;
            let db = Document::open_path(&pb, &Limits::default()).map_err(|e| e.to_string())?;
            requests.push(json!({"op": "text_diff", "a": pa, "b": pb, "view": view.as_str()}));
            let diff = text_diff(
                &da,
                &pa.display().to_string(),
                &db,
                &pb.display().to_string(),
                view,
            )
            .map_err(|e| e.to_string())?;
            expected.push((
                format!("text_diff:{a}->{b}:{}", view.as_str()),
                diff.to_json(),
            ));
        }
    }
    let results = oracle::run(python, &requests)?;
    let mut summary: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let python_opcodes = results[0]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for ((a, b), py) in pairs.iter().zip(python_opcodes.iter()) {
        let rust: Vec<Value> = SequenceMatcher::new(a, b)
            .opcodes()
            .into_iter()
            .map(|(tag, i1, i2, j1, j2)| json!([tag_name(tag), i1, i2, j1, j2]))
            .collect();
        if Value::Array(rust) == *py {
            *summary.entry("difflib_match").or_default() += 1;
        } else {
            failures.push(json!({"case": "difflib", "a_len": a.len(), "b_len": b.len()}));
        }
    }
    for ((name, rust), py) in expected.iter().zip(results[1..].iter()) {
        if *rust == py["diff"] {
            *summary
                .entry(if name.starts_with("pending") {
                    "pending_match"
                } else {
                    "text_diff_match"
                })
                .or_default() += 1;
            if rust["changed_line_count"].as_u64().unwrap_or(0) > 0 {
                *summary.entry("non_empty_diffs").or_default() += 1;
            }
        } else {
            failures.push(json!({"case": name, "rust": rust, "python": py}));
        }
    }
    println!(
        "m6-parity: {}",
        serde_json::to_string(&summary).unwrap_or_default()
    );
    for failure in failures.iter().take(10) {
        let mut text = failure.to_string();
        text.truncate(1200);
        println!("  FAIL {text}");
    }
    Ok(failures.is_empty())
}

use docrs::compare::{CompareOptions, Granularity, compare};
use docrs::package::{zip_read, zip_write};
use docrs::xml::{self as pxml, NodeData, NodeId, XmlTree};

fn rebuild(bytes: &[u8], part: &str, tree: &XmlTree) -> Option<Vec<u8>> {
    let contents = zip_read::read_archive(bytes, &Limits::default()).ok()?;
    let members: Vec<(String, Vec<u8>)> = contents
        .members
        .into_iter()
        .map(|m| {
            if m.name == part {
                (m.name, pxml::serialize(tree))
            } else {
                (m.name, m.data)
            }
        })
        .collect();
    zip_write::write_archive(
        members.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
        &Limits::default(),
    )
    .ok()
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn body_children(tree: &XmlTree, local: &str) -> Vec<NodeId> {
    let body = tree
        .child_elements(tree.root())
        .find(|n| tree.is(*n, W, "body"));
    body.map(|b| {
        tree.child_elements(b)
            .filter(|n| tree.is(*n, W, local))
            .collect()
    })
    .unwrap_or_default()
}

fn text_nodes(tree: &XmlTree, within: NodeId) -> Vec<NodeId> {
    tree.descendants(within)
        .filter(|n| matches!(tree.data(*n), NodeData::Text(t) if !t.trim().is_empty()))
        .filter(|n| tree.parent(*n).is_some_and(|p| tree.is(p, W, "t")))
        .collect()
}

fn variants(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![("identical".to_string(), bytes.to_vec())];
    let Ok(doc) = Document::open_bytes(bytes, &Limits::default()) else {
        return out;
    };
    let part = doc.package().main_part_name().to_string();
    let Ok(base) = doc.package().xml(&part).cloned() else {
        return out;
    };
    let paragraphs: Vec<NodeId> = body_children(&base, "p")
        .into_iter()
        .filter(|p| !text_nodes(&base, *p).is_empty())
        .collect();
    let mut push = |label: &str, tree: XmlTree| {
        if let Some(bytes) = rebuild(bytes, &part, &tree) {
            out.push((label.to_string(), bytes));
        }
    };
    if let Some(first) = paragraphs.first() {
        let mut tree = base.clone();
        let node = text_nodes(&tree, *first)[0];
        let text = tree.text(node).unwrap_or("").to_string();
        let changed = match text.split_once(' ') {
            Some((head, tail)) => format!("{head} revised {tail}"),
            None => format!("{text} revised"),
        };
        tree.set_text(node, changed).ok();
        push("word-edit", tree);
        let mut tree = base.clone();
        if tree.detach(*first).is_ok() {
            push("delete-paragraph", tree);
        }
        let mut tree = base.clone();
        let copy = tree.deep_clone(*first);
        if let (Some(parent), Some(index)) = (tree.parent(*first), tree.index_in_parent(*first))
            && tree.insert(parent, index + 1, copy).is_ok()
        {
            push("duplicate-paragraph", tree);
        }
    }
    if paragraphs.len() >= 2 {
        let mut tree = base.clone();
        for p in [paragraphs[0], paragraphs[paragraphs.len() - 1]] {
            let node = text_nodes(&tree, p)[0];
            let text = format!("{} and more", tree.text(node).unwrap_or(""));
            tree.set_text(node, text).ok();
        }
        push("two-edits", tree);
        let mut tree = base.clone();
        let second = paragraphs[1];
        if let (Some(parent), Some(index)) = (
            tree.parent(paragraphs[0]),
            tree.index_in_parent(paragraphs[0]),
        ) && tree.detach(second).is_ok()
            && tree.insert(parent, index, second).is_ok()
        {
            push("swap-paragraphs", tree);
        }
    }
    if let Some(table) = body_children(&base, "tbl").first().copied() {
        let rows: Vec<NodeId> = base
            .child_elements(table)
            .filter(|n| base.is(*n, W, "tr"))
            .collect();
        if let Some(node) = text_nodes(&base, table).first().copied() {
            let mut tree = base.clone();
            let text = format!("{} edited", tree.text(node).unwrap_or(""));
            tree.set_text(node, text).ok();
            push("table-cell-edit", tree);
        }
        if rows.len() >= 2 {
            let mut tree = base.clone();
            if tree.detach(rows[1]).is_ok() {
                push("table-row-delete", tree);
            }
            let mut tree = base.clone();
            let copy = tree.deep_clone(rows[0]);
            if tree
                .insert(table, tree.index_in_parent(rows[0]).unwrap_or(0) + 1, copy)
                .is_ok()
            {
                push("table-row-duplicate", tree);
            }
        }
    }
    out
}

fn compare_story_parts_equivalent(a: &Path, b: &Path) -> bool {
    let (Ok(da), Ok(db)) = (Document::open(a), Document::open(b)) else {
        return false;
    };
    let sa = da.stories();
    let sb = db.stories();
    if sa.len() != sb.len() {
        return false;
    }
    sa.iter().zip(sb.iter()).all(|(x, y)| {
        x.name == y.name
            && match (da.package().xml(&x.part), db.package().xml(&y.part)) {
                (Ok(tx), Ok(ty)) => pxml::trees_equivalent(tx, ty),
                _ => false,
            }
    })
}

pub fn run_compare(python: &Path) -> Result<bool, String> {
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let date = "2024-01-02T03:04:05Z";
    let mut requests = Vec::new();
    let mut plans: Vec<(String, Value, std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut inputs: Vec<(String, std::path::PathBuf)> = hostile::all_fixtures()
        .into_iter()
        .filter(|rel| !rel.contains("/corrupt/") && !rel.contains("large"))
        .map(|rel| {
            let path = hostile::fixtures_dir().join(&rel);
            (rel, path)
        })
        .collect();
    inputs.sort();
    for (index, (rel, path)) in inputs.iter().enumerate() {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let mut pairs: Vec<(String, std::path::PathBuf)> = Vec::new();
        for (variant_index, (label, revised)) in variants(&bytes).into_iter().enumerate() {
            let revised_path = work.path().join(format!("v-{index}-{variant_index}.docx"));
            std::fs::write(&revised_path, revised).map_err(|e| e.to_string())?;
            pairs.push((label, revised_path));
        }
        if rel.ends_with("compare-original.docx") {
            pairs.push((
                "upstream-pair".into(),
                hostile::fixtures_dir().join("generated/redline/compare-revised.docx"),
            ));
        }
        for (pair_index, (label, revised_path)) in pairs.into_iter().enumerate() {
            let has_revisions = Document::open_bytes(&bytes, &Limits::default())
                .ok()
                .and_then(|d| d.revisions().ok())
                .is_some_and(|r| !r.is_empty());
            let modes: &[Option<&str>] = if has_revisions {
                &[None, Some("accept"), Some("reject")]
            } else {
                &[None]
            };
            let runs: Vec<(&str, Option<&str>)> = ["word", "block"]
                .iter()
                .flat_map(|g| modes.iter().map(move |m| (*g, *m)))
                .collect();
            for (granularity, mode) in runs {
                let tag = format!("{granularity}-{}", mode.unwrap_or("none"));
                let name = format!("{rel}#{label}#{tag}");
                let rust_out = work
                    .path()
                    .join(format!("r-{index}-{pair_index}-{tag}.docx"));
                let python_out = work
                    .path()
                    .join(format!("p-{index}-{pair_index}-{tag}.docx"));
                let options = CompareOptions {
                    author: "Comparer".into(),
                    date: Some(docrs::clock::Timestamp::parse(date).map_err(|e| e.to_string())?),
                    granularity: if granularity == "word" {
                        Granularity::Word
                    } else {
                        Granularity::Block
                    },
                    materialize: match mode {
                        Some("accept") => Some(docrs::compare::Materialize::Accept),
                        Some("reject") => Some(docrs::compare::Materialize::Reject),
                        _ => None,
                    },
                    // Upstream redlines protected originals unasked (and
                    // reports `document_protection_present`).
                    acknowledge_protection: true,
                };
                let revised_bytes = std::fs::read(&revised_path).map_err(|e| e.to_string())?;
                let rust = match compare(&bytes, &revised_bytes, &options, &Limits::default()) {
                    Ok(result) => {
                        let mut value = result.to_json();
                        if let Some(findings) = value["findings"].as_array_mut() {
                            findings.retain(|f| f["kind"] != "algebra_unverified");
                        }
                        match result.document.save(&rust_out) {
                            Ok(_) => json!({"outcome": "ok", "result": value}),
                            Err(err) => {
                                json!({"outcome": "error", "category": err.code(), "reason": err.reason()})
                            }
                        }
                    }
                    Err(err) => {
                        json!({"outcome": "error", "category": err.code(), "reason": err.reason(), "message": err.message()})
                    }
                };
                requests.push(json!({"op": "compare", "original": path, "revised": revised_path, "author": "Comparer", "date": date, "granularity": granularity, "materialize": mode, "out": python_out}));
                plans.push((name, rust, rust_out, python_out));
            }
        }
    }
    let results = oracle::run(python, &requests)?;
    let mut summary: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for ((name, rust, rust_out, python_out), py) in plans.iter().zip(results.iter()) {
        let verdict = if rust["outcome"] == "ok" && py["outcome"] == "ok" {
            if rust["result"] != py["result"] {
                "report_mismatch"
            } else if !compare_story_parts_equivalent(rust_out, python_out) {
                "redline_mismatch"
            } else {
                "match_redline"
            }
        } else if rust["outcome"] == "error"
            && py["outcome"] == "error"
            && rust["category"] == py["category"]
        {
            "match_refusal"
        } else {
            "outcome_mismatch"
        };
        *summary.entry(verdict.to_string()).or_default() += 1;
        if rust["outcome"] == "ok" && (name.ends_with("-accept") || name.ends_with("-reject")) {
            *summary.entry("materialized_redlines".into()).or_default() += 1;
        }
        if rust["outcome"] == "ok" {
            let count = rust["result"]["revision_count"].as_u64().unwrap_or(0);
            *summary
                .entry(if count == 0 {
                    "zero_revision_results".into()
                } else {
                    "redlines_with_revisions".into()
                })
                .or_default() += 1;
        }
        if !verdict.starts_with("match") {
            failures.push(json!({"case": name, "verdict": verdict, "rust": rust, "python": py}));
        }
    }
    println!(
        "compare-parity: {}",
        serde_json::to_string(&summary).unwrap_or_default()
    );
    for failure in failures.iter().take(12) {
        let mut text = failure.to_string();
        text.truncate(1500);
        println!("  FAIL {text}");
    }
    Ok(failures.is_empty())
}
