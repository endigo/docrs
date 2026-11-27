//! `docrs layout --json` against `tests/fixtures/layout/` (tools/pdf/README.md).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const UPDATE: &str = "DOCRS_UPDATE_SNAPSHOTS";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn docrs(args: &[&std::ffi::OsStr]) -> Result<Vec<u8>, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_docrs"))
        .arg("--json")
        .args(args)
        .output()
        .map_err(|err| err.to_string())?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "docrs {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn layout(docx: &Path, font: Option<&Path>) -> Result<String, String> {
    let mut args = vec![std::ffi::OsStr::new("layout"), docx.as_os_str()];
    if let Some(font) = font {
        args.extend([std::ffi::OsStr::new("--font"), font.as_os_str()]);
    }
    let stdout = docrs(&args)?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&stdout).map_err(|err| err.to_string())?;
    let object = value.as_object_mut().ok_or("layout output is an object")?;
    object.remove("schema_version");
    Ok(snapshot_text(object))
}

fn snapshot_text(object: &serde_json::Map<String, serde_json::Value>) -> String {
    let fields: Vec<String> = object
        .iter()
        .map(|(key, value)| match value.as_array() {
            Some(items) if key == "paragraphs" && !items.is_empty() => {
                let rows: Vec<String> = items.iter().map(|item| format!("    {item}")).collect();
                format!("  {key:?}: [\n{}\n  ]", rows.join(",\n"))
            }
            _ => format!("  {key:?}: {value}"),
        })
        .collect();
    format!("{{\n{}\n}}\n", fields.join(",\n"))
}

fn convert(input: &Path, reference: &Path, out: &Path, flags: &[&str]) -> Result<(), String> {
    let mut args = vec![
        std::ffi::OsStr::new("from-markdown"),
        input.as_os_str(),
        std::ffi::OsStr::new("--reference"),
        reference.as_os_str(),
        std::ffi::OsStr::new("--out"),
        out.as_os_str(),
    ];
    args.extend(flags.iter().map(std::ffi::OsStr::new));
    docrs(&args).map(|_| ())
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn docx_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "docx"))
        .collect();
    out.sort();
    out
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().into_owned()
}

fn tsv(path: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

type Snapshots = BTreeMap<PathBuf, Result<String, String>>;

fn document_snapshots(root: &Path, snapshots: &Path, actual: &mut Snapshots) {
    let templates = root.join("tests/fixtures/markdown/templates");
    for (dir, into) in [
        (templates.clone(), "templates"),
        (templates.join("v2"), "templates/v2"),
        (root.join("examples/markdown"), "examples"),
    ] {
        for docx in docx_in(&dir) {
            let snapshot = snapshots.join(into).join(format!("{}.json", stem(&docx)));
            actual.insert(snapshot, layout(&docx, None));
        }
    }
}

/// The corpus manifest with today's hashes, and the hashes that changed.
fn corpus_snapshots(
    root: &Path,
    snapshots: &Path,
    work: &Path,
    actual: &mut Snapshots,
) -> (String, Vec<String>) {
    let mut manifest = String::from("case\ttemplate\ttemplate_sha256\tinput\tinput_sha256\n");
    let mut changed = Vec::new();
    for row in tsv(&root.join("tools/pdf/corpus.tsv")) {
        let [case, template, template_hash, input, input_hash] = &row[..] else {
            panic!("tools/pdf/corpus.tsv: a row has five columns: {row:?}");
        };
        let (template_path, input_path) = (root.join(template), root.join(input));
        let (template_now, input_now) = (sha256(&template_path), sha256(&input_path));
        for (file, recorded, now) in [
            (template, template_hash, &template_now),
            (input, input_hash, &input_now),
        ] {
            if recorded != now {
                changed.push(format!(
                    "tools/pdf/corpus.tsv: {case}: {file} hashes {now}, the manifest says {recorded}"
                ));
            }
        }
        writeln!(
            manifest,
            "{case}\t{template}\t{template_now}\t{input}\t{input_now}"
        )
        .unwrap();
        let out = work.join(format!("{case}.docx"));
        let result = convert(&input_path, &template_path, &out, &["--toc"])
            .and_then(|()| layout(&out, None));
        actual.insert(snapshots.join(format!("corpus/{case}.json")), result);
    }
    (manifest, changed)
}

fn fixture_snapshots(root: &Path, snapshots: &Path, work: &Path, actual: &mut Snapshots) {
    for row in tsv(&root.join("tests/fixtures/pdf/cases.tsv")) {
        let [case, source, reference, font, options] = &row[..] else {
            panic!("tests/fixtures/pdf/cases.tsv: a row has five columns: {row:?}");
        };
        let flags: Vec<&str> = options.split(' ').filter(|o| *o != "-").collect();
        let font = (font != "-").then(|| root.join(font));
        let out = work.join(format!("pdf-{case}.docx"));
        let result = convert(&root.join(source), &root.join(reference), &out, &flags)
            .and_then(|()| layout(&out, font.as_deref()));
        actual.insert(snapshots.join(format!("pdf/{case}.json")), result);
    }
}

fn stale_snapshots(snapshots: &Path, actual: &Snapshots) -> Vec<PathBuf> {
    let mut stale = Vec::new();
    files_under(snapshots, &mut stale);
    stale.retain(|path| {
        path.extension().is_some_and(|ext| ext == "json") && !actual.contains_key(path)
    });
    stale
}

fn difference_summary(recorded: &str, now: &str) -> String {
    let (recorded, now): (Vec<&str>, Vec<&str>) =
        (recorded.lines().collect(), now.lines().collect());
    let pairs = recorded.iter().zip(&now);
    let changed =
        pairs.clone().filter(|(a, b)| a != b).count() + recorded.len().abs_diff(now.len());
    let line = pairs
        .clone()
        .position(|(a, b)| a != b)
        .unwrap_or(recorded.len().min(now.len()));
    format!(
        "{changed} line(s) differ, the first is line {}\n    snapshot: {}\n    now:      {}",
        line + 1,
        recorded.get(line).unwrap_or(&"(end)"),
        now.get(line).unwrap_or(&"(end)"),
    )
}

#[test]
fn layout_matches_the_snapshots() {
    let root = root();
    let update = std::env::var_os(UPDATE).is_some_and(|v| v == "1");
    let snapshots = root.join("tests/fixtures/layout");
    let work = tempfile::tempdir().unwrap();
    let mut actual = Snapshots::new();
    document_snapshots(&root, &snapshots, &mut actual);
    let (manifest, mut failures) = corpus_snapshots(&root, &snapshots, work.path(), &mut actual);
    fixture_snapshots(&root, &snapshots, work.path(), &mut actual);
    let stale = stale_snapshots(&snapshots, &actual);
    let shown = |path: &Path| {
        path.strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
    };

    if update {
        failures.clear();
        std::fs::write(root.join("tools/pdf/corpus.tsv"), manifest).unwrap();
        for path in &stale {
            std::fs::remove_file(path).unwrap();
        }
    } else {
        for path in &stale {
            failures.push(format!("{}: no document makes this snapshot", shown(path)));
        }
    }
    for (path, result) in &actual {
        let now = match result {
            Ok(now) => now,
            Err(err) => {
                failures.push(format!("{}: {err}", shown(path)));
                continue;
            }
        };
        let recorded = std::fs::read_to_string(path).ok();
        if recorded.as_deref() == Some(now.as_str()) {
            continue;
        }
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, now).unwrap();
            continue;
        }
        let problem = match recorded {
            None => "no snapshot".to_string(),
            Some(recorded) => difference_summary(&recorded, now),
        };
        failures.push(format!("{}: {problem}", shown(path)));
    }
    assert!(
        failures.is_empty(),
        "{} layout snapshot(s) differ. If the change is intended, rerun with {UPDATE}=1 \
         and review the diff under tests/fixtures/layout/.\n{}",
        failures.len(),
        failures.join("\n")
    );
}
