//! docrs-compat: test-only compatibility harness.
//!
//! * `kernel-parity` — runs the frozen fixture corpus plus a generated
//!   hostile-archive corpus through the Rust core and the pinned Python oracle
//!   and compares open outcomes, no-op patch saves, diagnoses and package
//!   diffs. Intentional deviations are listed in [`DEVIATIONS`]; anything
//!   else that differs fails the run.
//! * `cases` — runs shared JSON operation cases (`tests/cases/*.json`).
//! * `loader-smoke` — requires a *working* LibreOffice and converts outputs
//!   to PDF; a broken launcher fails clearly rather than passing.

mod cases;
mod hostile;
mod loader;
mod m2;
mod m3;
mod m6;
mod ops;
mod opsrun;
mod oracle;
mod report;
mod synthetic;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use docrs::package::diagnose::diagnose_bytes;
use docrs::package::diff::{diff_packages, sha256_hex};
use docrs::package::{zip_read, zip_write};
use docrs::xml::{self, NodeData};
use docrs::{Document, ErrorKind, Limits};
use serde_json::{Value, json};

pub const UPSTREAM_REV: &str = "3c0f9a56c73105564a6a44661c6d058a0d3fc800";

/// Cases where Rust intentionally differs from the oracle, keyed
/// `case@op`, with the reason.
const DEVIATIONS: &[(&str, &str)] = &[
    (
        "document-internal-dtd@open",
        "Rust refuses any DOCTYPE in a package part (ISO 29500 forbids it; no entity semantics); \
         upstream's oxml parser accepts an internal subset in story parts",
    ),
    (
        "document-internal-dtd@diagnose",
        "diagnosis does not parse story XML in either implementation; Rust's open-time DTD refusal \
         is the deviation recorded under @open",
    ),
    (
        "xml-depth-257@open",
        "both refuse; Rust reports resource_limit(max_xml_depth) where lxml raises XMLSyntaxError \
         'Excessive depth' (policy outcome, not malformed evidence)",
    ),
    (
        "orphan-member@noop_patch_save",
        "upstream save rebuilds the package from the relationship graph and drops members no \
         relationship reaches; Rust preserves every member (no-op save is the original archive)",
    ),
    (
        "escaped-space-name@noop_patch_save",
        "same as orphan-member: the extra member is unreachable and upstream drops it on save",
    ),
    (
        "rel-case-mismatch-target@noop_patch_save",
        "upstream renames a member to the case its relationship uses (removing word/document.xml, \
         adding WORD/Document.XML); Rust keeps the original member name and bytes",
    ),
];

fn is_deviation(name: &str, op: &str) -> bool {
    DEVIATIONS
        .iter()
        .any(|(key, _)| *key == format!("{name}@{op}"))
}

#[derive(Parser)]
#[command(
    name = "docrs-compat",
    about = "Test-only compatibility harness for docrs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compare the Rust kernel with the pinned Python oracle.
    KernelParity {
        /// Oracle interpreter.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
        /// Expected upstream revision of the oracle checkout.
        #[arg(long, default_value = UPSTREAM_REV)]
        upstream_rev: String,
        /// Write the full JSON report here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Compare outlines and searches (M2) with the oracle over all fixtures.
    M2Parity {
        /// Oracle interpreter.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
        /// Write the full JSON report here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Compare safe replacement (M3) with the oracle.
    M3Parity {
        /// Oracle interpreter.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
        /// Write the full JSON report here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Compare difflib, text diffs, pending changes (and compare) with the oracle.
    M6Parity {
        /// Oracle interpreter.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
    },
    /// Run shared multi-step ops cases (`tests/ops`) through Rust and the oracle.
    Ops {
        /// Case directory.
        #[arg(long, default_value = "tests/ops")]
        dir: PathBuf,
        /// Oracle interpreter.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
        /// Skip the oracle.
        #[arg(long)]
        no_oracle: bool,
        /// Only case files whose path contains this text.
        #[arg(long)]
        filter: Option<String>,
        /// Write the full JSON report here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Run shared JSON operation cases through Rust (and the oracle).
    Cases {
        /// Case directory.
        #[arg(long, default_value = "tests/cases")]
        dir: PathBuf,
        /// Oracle interpreter; omit with --no-oracle.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
        /// Skip the oracle.
        #[arg(long)]
        no_oracle: bool,
        /// Write the full JSON report here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Every parity suite in dependency order (kernel, m2, m3, m6, cases,
    /// ops), then the independent-loader gate; fails if any gate fails.
    Check {
        /// Oracle interpreter.
        #[arg(long, default_value = ".venv/bin/python")]
        python: PathBuf,
        /// Expected upstream revision of the oracle checkout.
        #[arg(long, default_value = UPSTREAM_REV)]
        upstream_rev: String,
        /// Treat an unavailable LibreOffice as a failure (release mode).
        #[arg(long)]
        require_libreoffice: bool,
    },
    /// Time open/outline/find/replace/save on one file.
    Bench {
        /// Input document.
        file: PathBuf,
        /// Output path.
        out: PathBuf,
    },
    /// Independent-loader smoke test (requires a working LibreOffice).
    LoaderSmoke {
        /// Fail unless LibreOffice actually converts every file.
        #[arg(long)]
        require_libreoffice: bool,
        /// Files to convert (defaults to generated kernel outputs).
        files: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::KernelParity {
            python,
            upstream_rev,
            report,
        } => kernel_parity(&python, &upstream_rev, report.as_deref()),
        Command::Ops {
            dir,
            python,
            no_oracle,
            filter,
            report,
        } => opsrun::run(
            &dir,
            if no_oracle {
                None
            } else {
                Some(python.as_path())
            },
            report.as_deref(),
            filter.as_deref(),
        ),
        Command::M6Parity { python } => {
            m6::run(&python).and_then(|ok| Ok(ok & m6::run_compare(&python)?))
        }
        Command::M2Parity { python, report } => m2::run(&python, report.as_deref()),
        Command::M3Parity { python, report } => m3::run(&python, report.as_deref()),
        Command::Cases {
            dir,
            python,
            no_oracle,
            report,
        } => cases::run(
            &dir,
            if no_oracle {
                None
            } else {
                Some(python.as_path())
            },
            report.as_deref(),
        ),
        Command::Check {
            python,
            upstream_rev,
            require_libreoffice,
        } => check(&python, &upstream_rev, require_libreoffice),
        Command::Bench { file, out } => bench(&file, &out),
        Command::LoaderSmoke {
            require_libreoffice,
            files,
        } => loader::smoke(require_libreoffice, &files),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn check(python: &Path, upstream_rev: &str, require_libreoffice: bool) -> Result<bool, String> {
    type Gate<'a> = (&'static str, Box<dyn Fn() -> Result<bool, String> + 'a>);
    let gates: Vec<Gate<'_>> = vec![
        (
            "kernel-parity",
            Box::new(|| kernel_parity(python, upstream_rev, None)),
        ),
        ("m2-parity", Box::new(|| m2::run(python, None))),
        ("m3-parity", Box::new(|| m3::run(python, None))),
        (
            "m6-parity",
            Box::new(|| m6::run(python).and_then(|ok| Ok(ok & m6::run_compare(python)?))),
        ),
        (
            "cases",
            Box::new(|| cases::run(Path::new("tests/cases"), Some(python), None)),
        ),
        (
            "ops",
            Box::new(|| opsrun::run(Path::new("tests/ops"), Some(python), None, None)),
        ),
    ];
    let mut verdicts = Vec::new();
    for (name, gate) in &gates {
        println!("== {name}");
        let verdict = match gate() {
            Ok(true) => "pass".to_string(),
            Ok(false) => "FAIL".to_string(),
            Err(message) => format!("ERROR ({message})"),
        };
        verdicts.push((*name, verdict));
    }
    println!("== loader-smoke");
    let loader_verdict = match loader::working_soffice() {
        Err(message) if !require_libreoffice => {
            println!("  {message}");
            "UNVERIFIED (no working LibreOffice; not a pass)".to_string()
        }
        _ => match loader::smoke(require_libreoffice, &[]) {
            Ok(true) => "pass".to_string(),
            Ok(false) => "FAIL".to_string(),
            Err(message) => format!("ERROR ({message})"),
        },
    };
    let loader_unverified = loader_verdict.starts_with("UNVERIFIED");
    verdicts.push(("loader-smoke", loader_verdict));
    println!("== summary (upstream {upstream_rev})");
    for (name, verdict) in &verdicts {
        println!("  {name}: {verdict}");
    }
    if loader_unverified {
        println!(
            "  (oracle gates only: the independent-loader gate is unverified on this machine)"
        );
    }
    Ok(verdicts
        .iter()
        .all(|(name, verdict)| verdict == "pass" || (*name == "loader-smoke" && loader_unverified)))
}

fn rust_open(bytes: &[u8]) -> Value {
    match Document::open_bytes(bytes, &Limits::default()) {
        Ok(_) => json!({"outcome": "ok"}),
        Err(err) => json!({
            "outcome": "error",
            "category": err.code(),
            "reason": err.reason(),
            "refusal": err.is_refusal(),
            "resource_limit": err.kind() == ErrorKind::ResourceLimit,
        }),
    }
}

fn compare_open(name: &str, rust: &Value, python: &Value) -> &'static str {
    let r_ok = rust["outcome"] == "ok";
    let p_ok = python["outcome"] == "ok";
    let deviation = is_deviation(name, "open");
    if r_ok && p_ok {
        return "match";
    }
    if !r_ok && !p_ok {
        if rust["category"] == python["category"] {
            return "match";
        }
        if rust["category"] == "malformed_package" && python["category"] == "non_refusal_error" {
            // Upstream leaks a raw exception (BadZipFile, KeyError, lxml
            // XMLSyntaxError, NotImplementedError); Rust classifies it.
            return "classified";
        }
        if deviation {
            return "deviation";
        }
        return "mismatch";
    }
    if deviation { "deviation" } else { "mismatch" }
}

fn edit_first_text(bytes: &[u8], mutate: bool) -> Option<Vec<u8>> {
    let contents = zip_read::read_archive(bytes, &Limits::default()).ok()?;
    let doc = Document::open_bytes(bytes, &Limits::default()).ok()?;
    let main = doc.package().main_part_name().to_string();
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    for member in contents.members {
        if member.name == main {
            let mut tree = xml::parse(&member.data, &Limits::default()).ok()?;
            if mutate {
                let node = tree
                    .descendants(tree.root())
                    .find(|n| matches!(tree.data(*n), NodeData::Text(t) if !t.trim().is_empty()))?;
                let text = format!("{} [edited]", tree.text(node)?);
                tree.set_text(node, text).ok()?;
            }
            members.push((member.name, xml::serialize(&tree)));
        } else {
            members.push((member.name, member.data));
        }
    }
    zip_write::write_archive(
        members.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
        &Limits::default(),
    )
    .ok()
}

fn normalize_diff(diff: &Value) -> Value {
    let changed: Vec<Value> = diff["changed"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| json!({"part": item["part"], "kind": item["kind"], "semantic_change": item["semantic_change"]}))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "added": diff["added"],
        "removed": diff["removed"],
        "changed": changed,
        "byte_identical_count": diff["byte_identical_count"],
        "is_semantically_empty": diff["is_semantically_empty"],
    })
}

fn kernel_parity(
    python: &Path,
    upstream_rev: &str,
    report_path: Option<&Path>,
) -> Result<bool, String> {
    oracle::check_revision(upstream_rev)?;
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let root = work.path();
    std::fs::create_dir_all(root.join("hostile")).map_err(|e| e.to_string())?;
    let mut items = hostile::materialize(&root.join("hostile"), hostile::corpus())?;
    items.extend(
        hostile::fixture_inputs(&[])
            .into_iter()
            .map(|(rel, path)| (format!("fixture:{rel}"), path)),
    );

    let mut requests: Vec<Value> = Vec::new();
    let mut plan: Vec<(String, &'static str, Value)> = Vec::new();
    for (index, (name, path)) in items.iter().enumerate() {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        requests.push(json!({"op": "open", "path": path}));
        plan.push((name.clone(), "open", rust_open(&bytes)));
        requests.push(json!({"op": "diagnose", "path": path}));
        let diagnosis = diagnose_bytes(&path.display().to_string(), &bytes, &Limits::default());
        plan.push((
            name.clone(),
            "diagnose",
            json!({"kind": diagnosis.kind, "readable": diagnosis.readable}),
        ));
        if let Ok(doc) = Document::open_bytes(&bytes, &Limits::default()) {
            let out = root.join(format!("noop-{index}.docx"));
            requests.push(json!({"op": "noop_patch_save", "path": path, "out": out}));
            let (patched, report) = doc.patch_bytes().map_err(|e| e.to_string())?;
            plan.push((name.clone(), "noop_patch_save", json!({"out_sha256": sha256_hex(&patched), "verbatim_copy": report.verbatim_copy, "input_sha256": sha256_hex(&bytes)})));
        }
        if name.starts_with("fixture:") && !name.contains("/corrupt/") {
            for (label, mutate) in [("edited", true), ("reserialized", false)] {
                if let Some(variant) = edit_first_text(&bytes, mutate) {
                    let variant_path = root.join(format!("{label}-{index}.docx"));
                    std::fs::write(&variant_path, &variant).map_err(|e| e.to_string())?;
                    requests.push(json!({"op": "diff", "a": path, "b": variant_path}));
                    let diff = diff_packages(&bytes, &variant, &Limits::default())
                        .map_err(|e| e.to_string())?;
                    plan.push((
                        format!("{name}#{label}"),
                        "diff",
                        normalize_diff(&diff.to_json()),
                    ));
                }
            }
        }
    }

    let results = oracle::run(python, &requests)?;
    let mut summary: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut rows: Vec<Value> = Vec::new();
    let mut failed = false;
    for ((name, op, rust), python_result) in plan.iter().zip(results.iter()) {
        let verdict: &'static str = match *op {
            "open" => compare_open(name, rust, python_result),
            "diagnose" => {
                let p = &python_result["diagnosis"];
                if p["kind"] == rust["kind"] && p["readable"] == rust["readable"] {
                    "match"
                } else if python_result["outcome"] == "error" && rust["readable"] == false {
                    // Upstream `diagnose` itself crashes; Rust returns a diagnosis.
                    "classified"
                } else if is_deviation(name, "diagnose") {
                    "deviation"
                } else {
                    "mismatch"
                }
            }
            "noop_patch_save" => {
                let same = python_result["outcome"] == "ok"
                    && python_result["out_sha256"] == rust["out_sha256"]
                    && rust["out_sha256"] == rust["input_sha256"];
                if same {
                    "match"
                } else if is_deviation(name, "noop_patch_save")
                    && rust["out_sha256"] == rust["input_sha256"]
                {
                    "deviation"
                } else {
                    "mismatch"
                }
            }
            "diff"
                if python_result["outcome"] == "ok"
                    && normalize_diff(&python_result["diff"]) == *rust =>
            {
                "match"
            }
            _ => "mismatch",
        };
        *summary.entry(verdict).or_default() += 1;
        if verdict == "mismatch" {
            failed = true;
        }
        rows.push(json!({"case": name, "op": op, "verdict": verdict, "rust": rust, "python": python_result}));
    }

    println!("kernel-parity against upstream {upstream_rev}");
    for row in &rows {
        let verdict = row["verdict"].as_str().unwrap_or("");
        if verdict != "match" {
            println!(
                "  {verdict:<10} {:<10} {:<48} rust={} python={}",
                row["op"].as_str().unwrap_or(""),
                row["case"].as_str().unwrap_or(""),
                report::clipped(row["rust"].to_string(), 160),
                report::clipped(row["python"].to_string(), 160)
            );
        }
    }
    println!(
        "summary: {}",
        serde_json::to_string(&summary).unwrap_or_default()
    );
    for (case, why) in DEVIATIONS {
        println!("  documented deviation {case}: {why}");
    }
    report::write_json(
        report_path,
        &json!({"schema_version": 1, "upstream_rev": upstream_rev, "summary": summary, "rows": rows}),
    )?;
    Ok(!failed)
}

/// `bench FILE OUT`: open, outline, find, replace, save timings (release).
pub fn bench(path: &Path, out: &Path) -> Result<bool, String> {
    use std::time::Instant;
    let t0 = Instant::now();
    let mut doc = Document::open_path(path, &Limits::default()).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    let outline = doc
        .outline(docrs::View::Current)
        .map_err(|e| e.to_string())?;
    let t2 = Instant::now();
    let spans = doc
        .find_text("filler", &docrs::FindOptions::default())
        .map_err(|e| e.to_string())?;
    let t3 = Instant::now();
    let span = spans.last().ok_or("no matches")?.clone();
    doc.replace(&span, "FILLER", &docrs::ReplaceOptions::default())
        .map_err(|e| e.to_string())?;
    let t4 = Instant::now();
    doc.save(out).map_err(|e| e.to_string())?;
    let t5 = Instant::now();
    let s = |a: Instant, b: Instant| (b - a).as_secs_f64();
    println!(
        "rust open={:.3}s outline({})={:.3}s find({})={:.3}s replace={:.3}s save={:.3}s total={:.3}s",
        s(t0, t1),
        outline.blocks.len(),
        s(t1, t2),
        spans.len(),
        s(t2, t3),
        s(t3, t4),
        s(t4, t5),
        s(t0, t5)
    );
    Ok(true)
}
