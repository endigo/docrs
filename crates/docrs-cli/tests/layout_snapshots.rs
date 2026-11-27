//! `docrs layout --json` for every reference template, every example
//! document, the conversions in `tools/pdf/corpus.tsv` and the PDF fixtures
//! in `tests/fixtures/pdf/cases.tsv`, compared with the snapshots under
//! `tests/fixtures/layout/`. `DOCRS_UPDATE_SNAPSHOTS=1` rewrites them, and
//! the manifests' hashes.

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
    let mut text = String::from("{\n");
    let last = object.len() - 1;
    for (i, (key, value)) in object.iter().enumerate() {
        let comma = if i == last { "" } else { "," };
        match value.as_array().filter(|_| key == "paragraphs") {
            Some(items) => {
                writeln!(text, "  {key:?}: [").unwrap();
                for (j, item) in items.iter().enumerate() {
                    let sep = if j + 1 == items.len() { "" } else { "," };
                    writeln!(text, "    {item}{sep}").unwrap();
                }
                writeln!(text, "  ]{comma}").unwrap();
            }
            None => writeln!(text, "  {key:?}: {value}{comma}").unwrap(),
        }
    }
    text.push_str("}\n");
    Ok(text)
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

#[test]
fn layout_matches_the_snapshots() {
    let root = root();
    let update = std::env::var_os(UPDATE).is_some_and(|v| v == "1");
    let snapshots = root.join("tests/fixtures/layout");
    let work = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    let mut actual: BTreeMap<PathBuf, Result<String, String>> = BTreeMap::new();

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

    let manifest = root.join("tools/pdf/corpus.tsv");
    let mut rewritten = String::from("case\ttemplate\ttemplate_sha256\tinput\tinput_sha256\n");
    for row in tsv(&manifest) {
        let [case, template, template_hash, input, input_hash] = &row[..] else {
            panic!("{}: a row has five columns: {row:?}", manifest.display());
        };
        let (template_path, input_path) = (root.join(template), root.join(input));
        let (template_now, input_now) = (sha256(&template_path), sha256(&input_path));
        for (file, recorded, now) in [
            (template, template_hash, &template_now),
            (input, input_hash, &input_now),
        ] {
            if recorded != now && !update {
                failures.push(format!(
                    "tools/pdf/corpus.tsv: {case}: {file} hashes {now}, the manifest says {recorded}"
                ));
            }
        }
        writeln!(
            rewritten,
            "{case}\t{template}\t{template_now}\t{input}\t{input_now}"
        )
        .unwrap();
        let out = work.path().join(format!("{case}.docx"));
        let result = convert(&input_path, &template_path, &out, &["--toc"])
            .and_then(|()| layout(&out, None));
        actual.insert(snapshots.join(format!("corpus/{case}.json")), result);
    }

    for row in tsv(&root.join("tests/fixtures/pdf/cases.tsv")) {
        let [case, source, reference, font, options] = &row[..] else {
            panic!("tests/fixtures/pdf/cases.tsv: a row has five columns: {row:?}");
        };
        let flags: Vec<&str> = options.split(' ').filter(|o| *o != "-").collect();
        let font = (font != "-").then(|| root.join(font));
        let out = work.path().join(format!("pdf-{case}.docx"));
        let result = convert(&root.join(source), &root.join(reference), &out, &flags)
            .and_then(|()| layout(&out, font.as_deref()));
        actual.insert(snapshots.join(format!("pdf/{case}.json")), result);
    }

    let shown = |path: &Path| {
        path.strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut stale = Vec::new();
    files_under(&snapshots, &mut stale);
    stale.retain(|path| {
        path.extension().is_some_and(|ext| ext == "json") && !actual.contains_key(path)
    });

    if update {
        std::fs::write(&manifest, rewritten).unwrap();
        for path in &stale {
            std::fs::remove_file(path).unwrap();
        }
    } else {
        for path in &stale {
            failures.push(format!("{}: no document makes this snapshot", shown(path)));
        }
    }
    for (path, result) in &actual {
        let text = match result {
            Ok(text) => text,
            Err(err) => {
                failures.push(format!("{}: {err}", shown(path)));
                continue;
            }
        };
        let recorded = std::fs::read_to_string(path).ok();
        if recorded.as_deref() == Some(text.as_str()) {
            continue;
        }
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
            continue;
        }
        failures.push(match recorded {
            None => format!("{}: no snapshot", shown(path)),
            Some(recorded) => {
                let line = recorded
                    .lines()
                    .zip(text.lines())
                    .position(|(a, b)| a != b)
                    .unwrap_or(recorded.lines().count().min(text.lines().count()));
                format!(
                    "{}: differs from line {}\n    snapshot: {}\n    now:      {}",
                    shown(path),
                    line + 1,
                    recorded.lines().nth(line).unwrap_or("(end)"),
                    text.lines().nth(line).unwrap_or("(end)"),
                )
            }
        });
    }
    assert!(
        failures.is_empty(),
        "{} layout snapshot(s) differ. If the change is intended, rerun with {UPDATE}=1 \
         and review the diff under tests/fixtures/layout/.\n{}",
        failures.len(),
        failures.join("\n")
    );
}
