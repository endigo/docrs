//! `docrs layout --json` against `tests/fixtures/layout/`, and the PDF
//! fixtures' baselines against Word's (tools/pdf/README.md).

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

struct Fixture {
    case: String,
    docx: PathBuf,
    font: Option<PathBuf>,
}

/// Each PDF fixture converted into `work`.
fn fixtures(root: &Path, work: &Path) -> Vec<(Fixture, Result<(), String>)> {
    tsv(&root.join("tests/fixtures/pdf/cases.tsv"))
        .into_iter()
        .map(|row| {
            let [case, source, reference, font, options] = &row[..] else {
                panic!("tests/fixtures/pdf/cases.tsv: a row has five columns: {row:?}");
            };
            let flags: Vec<&str> = options.split(' ').filter(|o| *o != "-").collect();
            let fixture = Fixture {
                case: case.clone(),
                docx: work.join(format!("pdf-{case}.docx")),
                font: (font != "-").then(|| root.join(font)),
            };
            let converted = convert(
                &root.join(source),
                &root.join(reference),
                &fixture.docx,
                &flags,
            );
            (fixture, converted)
        })
        .collect()
}

fn fixture_snapshots(root: &Path, snapshots: &Path, work: &Path, actual: &mut Snapshots) {
    for (fixture, converted) in fixtures(root, work) {
        let result = converted.and_then(|()| layout(&fixture.docx, fixture.font.as_deref()));
        actual.insert(snapshots.join(format!("pdf/{}.json", fixture.case)), result);
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

const BASELINE_TOLERANCE: f64 = 0.3;

/// What a fixture's baselines are held to: the lines that must match one
/// of Word's, the residual allowed and the last page compared.
struct BaselineCheck {
    case: &'static str,
    matched: usize,
    within: f64,
    last_page: u32,
}

const fn check(case: &'static str, matched: usize) -> BaselineCheck {
    BaselineCheck {
        case,
        matched,
        within: BASELINE_TOLERANCE,
        last_page: u32::MAX,
    }
}

const BASELINE_CHECKS: [BaselineCheck; 10] = [
    check("memo-arial", 47),
    check("memo-times-new-roman", 44),
    check("memo-inter", 49),
    check("lists-arial", 24),
    // With its page-2 header and footer.
    check("cover-page", 12),
    check("memo-aptos", 46),
    // Every page's header and footer, and the body.
    check("stories-arial", 115),
    // Only a row's first cell matches Word's line, which joins the row.
    // Word's header row is 0.54 pt taller, which moves every line below it.
    BaselineCheck {
        within: 0.65,
        ..check("table-times-new-roman", 15)
    },
    // From page 2, Word sets the first heading 19 pt lower (its 24 pt
    // space before less the previous paragraph's 5 pt after; a separate
    // PR). 15 of toc-inter's 16 page-1 lines match Word's.
    BaselineCheck {
        last_page: 1,
        ..check("toc-arial", 16)
    },
    BaselineCheck {
        last_page: 1,
        ..check("toc-inter", 15)
    },
];

fn letters(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

struct WordLine {
    page: u64,
    letters: String,
    baseline: f64,
}

fn word_lines(path: &Path) -> Vec<WordLine> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|page| page["lines"].as_array().unwrap())
        .filter_map(|line| {
            Some(WordLine {
                page: line["page"].as_u64()?,
                letters: letters(line["text"].as_str()?),
                baseline: line["baseline"].as_f64()?,
            })
        })
        .collect()
}

fn laid_out_lines(fixture: &Fixture) -> Result<Vec<serde_json::Value>, String> {
    let mut args = vec![
        std::ffi::OsStr::new("layout"),
        fixture.docx.as_os_str(),
        std::ffi::OsStr::new("--lines"),
    ];
    if let Some(font) = &fixture.font {
        args.extend([std::ffi::OsStr::new("--font"), font.as_os_str()]);
    }
    let stdout = docrs(&args)?;
    String::from_utf8_lossy(&stdout)
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).map_err(|err| err.to_string()))
        .collect()
}

/// Our baseline less Word's for each line with text up to `last_page`,
/// headers and footers included.
/// Word's line is the nearest on the page with the same text, the same
/// after a list label, or starting with it (Word joins a row's cells).
fn baseline_residuals(ours: &[serde_json::Value], word: &[WordLine], last_page: u32) -> Vec<f64> {
    let mut used = vec![false; word.len()];
    let mut residuals = Vec::new();
    for line in ours {
        let (Some(page), Some(text), Some(baseline)) = (
            line["page"].as_u64(),
            line["text"].as_str(),
            line["baseline"].as_f64(),
        ) else {
            continue;
        };
        let text = letters(text);
        if text.is_empty() || page > u64::from(last_page) {
            continue;
        }
        let whole = |w: &WordLine| w.letters.ends_with(&text) && w.letters.len() <= text.len() + 3;
        let nearest = word
            .iter()
            .enumerate()
            .filter(|(i, w)| {
                w.page == page && ((!used[*i] && whole(w)) || w.letters.starts_with(&text))
            })
            .min_by(|(_, a), (_, b)| {
                (a.baseline - baseline)
                    .abs()
                    .total_cmp(&(b.baseline - baseline).abs())
            });
        if let Some((i, w)) = nearest {
            used[i] = whole(w);
            residuals.push(baseline - w.baseline);
        }
    }
    residuals
}

#[test]
fn baselines_match_words() {
    let root = root();
    let work = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for (fixture, converted) in fixtures(&root, work.path()) {
        let case = fixture.case.as_str();
        let ours = match converted.and_then(|()| laid_out_lines(&fixture)) {
            Ok(ours) => ours,
            Err(err) => {
                failures.push(format!("{case}: {err}"));
                continue;
            }
        };
        let word = word_lines(&root.join(format!("tests/fixtures/pdf/{case}/word.lines.json")));
        let Some(expected) = BASELINE_CHECKS.iter().find(|c| c.case == case) else {
            failures.push(format!("{case}: not in BASELINE_CHECKS"));
            continue;
        };
        let residuals = baseline_residuals(&ours, &word, expected.last_page);
        let worst = residuals
            .iter()
            .copied()
            .fold(0.0_f64, |a, r| a.max(r.abs()));
        println!(
            "{case}: {} lines matched, worst residual {worst:.2} pt",
            residuals.len()
        );
        if residuals.len() < expected.matched {
            failures.push(format!(
                "{case}: {} lines matched one of Word's, {} expected",
                residuals.len(),
                expected.matched
            ));
        }
        if worst > expected.within + 1e-9 {
            failures.push(format!(
                "{case}: a baseline {worst:.2} pt from Word's (allowed {})",
                expected.within
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
