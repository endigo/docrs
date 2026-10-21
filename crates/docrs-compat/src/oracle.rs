//! Invocation of the pinned Python oracle (`tools/oracle/oracle.py`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Refuse to compare against an oracle checkout at the wrong revision.
pub fn check_revision(expected: &str) -> Result<(), String> {
    let checkout = repo_root().join(".reference/paper-docx");
    let output = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git for the oracle checkout: {e}"))?;
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // An abbreviated expected revision must still be unambiguous, and a
    // failed `git` (empty output) never matches.
    if !output.status.success() || expected.len() < 7 || !head.starts_with(expected) {
        return Err(format!(
            "oracle checkout {} is at {head:?}, expected {expected}",
            checkout.display()
        ));
    }
    Ok(())
}

/// Run `requests` through the oracle; results come back in order.
///
/// Requests are independent, so they are dealt round-robin to parallel
/// oracle processes (`DOCRS_ORACLE_JOBS`, default one per core).
pub fn run(python: &Path, requests: &[Value]) -> Result<Vec<Value>, String> {
    let python = if python.is_relative() {
        repo_root().join(python)
    } else {
        python.to_path_buf()
    };
    // Each process pays the interpreter and import start-up, so keep a few
    // requests per process.
    let jobs = std::env::var("DOCRS_ORACLE_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, requests.len().div_ceil(8).max(1));
    let mut batches: Vec<Vec<&Value>> = vec![Vec::new(); jobs];
    for (index, request) in requests.iter().enumerate() {
        batches[index % jobs].push(request);
    }
    let outputs: Vec<Result<Vec<Value>, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = batches
            .iter()
            .map(|batch| scope.spawn(|| run_batch(&python, batch)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("oracle thread panicked".into()))
            })
            .collect()
    });
    let mut batches = Vec::with_capacity(jobs);
    for output in outputs {
        batches.push(output?.into_iter());
    }
    let results: Vec<Value> = (0..requests.len())
        .filter_map(|index| batches[index % jobs].next())
        .collect();
    if results.len() != requests.len() {
        return Err(format!(
            "oracle returned {} results for {} requests",
            results.len(),
            requests.len()
        ));
    }
    Ok(results)
}

fn run_batch(python: &Path, requests: &[&Value]) -> Result<Vec<Value>, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let request_path = dir.path().join("requests.json");
    std::fs::write(
        &request_path,
        serde_json::to_vec(requests).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let script = repo_root().join("tools/oracle/oracle.py");
    let output = Command::new(python)
        .arg("-I")
        .arg(&script)
        .arg(&request_path)
        .output()
        .map_err(|e| format!("cannot run oracle {}: {e}", python.display()))?;
    if !output.status.success() {
        return Err(format!(
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let results: Vec<Value> =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("oracle output: {e}"))?;
    if results.len() != requests.len() {
        return Err(format!(
            "oracle returned {} results for {} requests",
            results.len(),
            requests.len()
        ));
    }
    Ok(results)
}
