//! Command implementations.

use std::io::Write;
use std::path::Path;

use docrs::markdown::{InputFormat, MarkdownOptions, Placement, StylePolicy};
use docrs::package::diagnose::diagnose_path;
use docrs::package::diff::diff_packages;
use docrs::{
    Document, Error, FindOptions, Limits, MatchPolicy, ReplaceAllOptions, ReplaceOptions, Result,
    View,
};
use serde_json::{Value, json};

use crate::{Command, ConvertArgs, FontArgs, OutputFormat, fonts, paragraphs};

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|err| Error::io("read_failed", format!("reading {}", path.display()), err))
}

fn emit(json_mode: bool, value: Value, human: impl FnOnce() -> String) {
    if json_mode {
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            object.insert("schema_version".into(), json!(1));
        }
        println!("{value}");
    } else {
        println!("{}", human());
    }
}

fn emit_composition(
    json_mode: bool,
    schema: &str,
    report: &docrs::composition::CompositionReport,
    patch: &docrs::package::patch::PatchReport,
    out: &Path,
) {
    let value = json!({"schema": schema, "version": 1, "composition": report.to_json(), "save": patch.to_json(), "out": out});
    emit(json_mode, value, || {
        let mut text = format!(
            "wrote {} ({} blocks inserted; changed: {})",
            out.display(),
            report.inserted_blocks,
            patch.changed_parts.join(", ")
        );
        for finding in &report.findings {
            text.push_str(&format!("\n  note: {}: {}", finding.kind, finding.detail));
        }
        text
    });
}

/// `--cover-page` / `--no-cover-page`; `None` lets the conversion detect one.
fn cover_page_option(cover_page: bool, no_cover_page: bool) -> Option<bool> {
    match (cover_page, no_cover_page) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

/// `ROLE=Style Name` pairs.
fn style_map(
    pairs: &[String],
) -> Result<std::collections::BTreeMap<docrs::markdown::Role, String>> {
    let mut map = std::collections::BTreeMap::new();
    for pair in pairs {
        let Some((role, name)) = pair.split_once('=') else {
            return Err(Error::invalid_argument(
                "invalid_style_mapping",
                format!("--style expects ROLE=STYLE, got {pair:?}"),
            ));
        };
        let role = docrs::markdown::Role::from_key(role.trim())?;
        map.insert(role, name.trim().trim_matches('"').to_string());
    }
    Ok(map)
}

pub fn run(command: &Command, limits: &Limits, json_mode: bool) -> Result<u8> {
    match command {
        Command::Diagnose { file } => {
            let diagnosis = diagnose_path(file, limits);
            let readable = diagnosis.readable;
            emit(json_mode, diagnosis.to_json(), || {
                let mut out = format!(
                    "{}: {} ({})",
                    diagnosis.path,
                    if readable { "readable" } else { "NOT readable" },
                    diagnosis.kind
                );
                for problem in &diagnosis.problems {
                    out.push_str(&format!("\n  - {problem}"));
                }
                out
            });
            Ok(if readable { 0 } else { 3 })
        }
        Command::Diff { a, b } => {
            let diff = diff_packages(&read(a)?, &read(b)?, limits)?;
            emit(json_mode, diff.to_json(), || {
                let mut out = String::new();
                for name in &diff.added {
                    out.push_str(&format!("added    {name}\n"));
                }
                for name in &diff.removed {
                    out.push_str(&format!("removed  {name}\n"));
                }
                for item in &diff.changed {
                    let label = if item.semantic_change {
                        "changed "
                    } else {
                        "rewrote "
                    };
                    out.push_str(&format!("{label} {} ({})\n", item.part, item.kind));
                }
                out.push_str(&format!(
                    "{} byte-identical; semantically {}",
                    diff.byte_identical_count,
                    if diff.is_semantically_empty() {
                        "identical"
                    } else {
                        "different"
                    }
                ));
                out
            });
            Ok(0)
        }
        Command::Outline { input, view } => {
            let document = Document::open_path(input, limits)?;
            let outline = document.outline(View::parse(view)?)?;
            emit(json_mode, outline.to_json(), || {
                let mut out = String::new();
                for block in &outline.blocks {
                    out.push_str(&format!(
                        "{}#{} [{:?}] {}\n",
                        block.story, block.index, block.kind, block.text
                    ));
                }
                let blind: Vec<String> = outline
                    .blind_region_counts
                    .iter()
                    .filter(|(_, count)| **count > 0)
                    .map(|(key, count)| format!("{key}={count}"))
                    .collect();
                out.push_str(&format!(
                    "blind regions: {}",
                    if blind.is_empty() {
                        "none".into()
                    } else {
                        blind.join(", ")
                    }
                ));
                out
            });
            Ok(0)
        }
        Command::Find {
            input,
            text,
            normalized,
            view,
            story,
        } => {
            let document = Document::open_path(input, limits)?;
            let options = FindOptions {
                policy: if *normalized {
                    MatchPolicy::Normalized
                } else {
                    MatchPolicy::Exact
                },
                view: View::parse(view)?,
                story: story.clone(),
                ..FindOptions::default()
            };
            let spans = document.find_text(text, &options)?;
            let value = json!({"schema": "paper_find", "version": 1, "count": spans.len(), "spans": spans.iter().map(|s| s.to_json()).collect::<Vec<_>>()});
            emit(json_mode, value, || {
                let mut out = format!("{} match(es)", spans.len());
                for span in &spans {
                    out.push_str(&format!(
                        "\n  {}#{} at {}: {:?}",
                        span.story(),
                        span.anchor().index,
                        span.raw_start(),
                        span.text()
                    ));
                }
                out
            });
            Ok(if spans.is_empty() { 3 } else { 0 })
        }
        Command::Replace {
            input,
            find,
            with_text,
            out,
            all,
            normalized,
            nth,
            story,
            acknowledge_protection,
        } => {
            let mut document = Document::open_path(input, limits)?;
            if *acknowledge_protection {
                document.acknowledge_protection()?;
            }
            let policy = if *normalized {
                MatchPolicy::Normalized
            } else {
                MatchPolicy::Exact
            };
            let (edit, replaced, refused) = if *all {
                let options = ReplaceAllOptions {
                    policy,
                    story: story.clone(),
                    ..ReplaceAllOptions::default()
                };
                let report = document.replace_all(find, with_text, &options)?;
                if report.replaced_count == 0 {
                    // Nothing to write: refuse like a single replace does.
                    return Err(nothing_replaced(find, &report.refused));
                }
                (
                    report.to_json(),
                    report.replaced_count,
                    report.refused.len(),
                )
            } else {
                let options = FindOptions {
                    policy,
                    story: story.clone(),
                    nth: *nth,
                    ..FindOptions::default()
                };
                let span = document.find_one(find, &options)?;
                let edit = document
                    .replace(&span, with_text, &ReplaceOptions::default())?
                    .to_json();
                (edit, 1, 0)
            };
            let patch = document.save(out)?;
            let value = json!({"schema": "docrs_replace", "version": 1, "replaced_count": replaced, "refused_count": refused, "edit": edit, "save": patch.to_json(), "out": out});
            emit(json_mode, value, || {
                format!(
                    "wrote {} ({replaced} replaced, {refused} refused; changed: {})",
                    out.display(),
                    if patch.changed_parts.is_empty() {
                        "nothing".into()
                    } else {
                        patch.changed_parts.join(", ")
                    }
                )
            });
            Ok(0)
        }
        Command::TextDiff { a, b, view } => {
            let first = Document::open_path(a, limits)?;
            let second = Document::open_path(b, limits)?;
            let diff = docrs::textdiff::text_diff(
                &first,
                &a.display().to_string(),
                &second,
                &b.display().to_string(),
                View::parse(view)?,
            )?;
            emit(json_mode, diff.to_json(), || {
                diff.stories
                    .iter()
                    .flat_map(|story| story.diff_lines.iter().cloned())
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(0)
        }
        Command::Pending { input } => {
            let document = Document::open_path(input, limits)?;
            let diff = docrs::textdiff::pending_changes(&document)?;
            emit(json_mode, diff.to_json(), || {
                if diff.is_empty() {
                    "no pending text changes".to_string()
                } else {
                    diff.stories
                        .iter()
                        .flat_map(|story| story.diff_lines.iter().cloned())
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
            Ok(0)
        }
        Command::Compare {
            original,
            revised,
            author,
            date,
            block,
            materialize,
            acknowledge_protection,
            out,
        } => {
            let options = docrs::compare::CompareOptions {
                author: author.clone(),
                date: match date {
                    Some(text) => Some(docrs::clock::Timestamp::parse(text)?),
                    None => None,
                },
                granularity: if *block {
                    docrs::compare::Granularity::Block
                } else {
                    docrs::compare::Granularity::Word
                },
                materialize: match materialize.as_deref() {
                    None => None,
                    Some("accept") => Some(docrs::compare::Materialize::Accept),
                    Some("reject") => Some(docrs::compare::Materialize::Reject),
                    Some(other) => {
                        return Err(Error::invalid_argument(
                            "invalid_materialize",
                            format!("materialize must be 'accept' or 'reject', got {other:?}"),
                        ));
                    }
                },
                acknowledge_protection: *acknowledge_protection,
            };
            let result =
                docrs::compare::compare(&read(original)?, &read(revised)?, &options, limits)?;
            let patch = result.document.save(out)?;
            let value = json!({"schema": "docrs_compare", "version": 1, "compare": result.to_json(), "save": patch.to_json(), "out": out});
            emit(json_mode, value, || {
                format!(
                    "wrote {} ({} revisions, accept/reject verified)",
                    out.display(),
                    result.revision_count
                )
            });
            Ok(0)
        }
        Command::Revisions { input } => {
            let document = Document::open_path(input, limits)?;
            let revisions = document.revisions()?;
            emit(json_mode, revisions.to_json(), || {
                let mut out = format!("{} pending revisions", revisions.len());
                for revision in revisions.iter() {
                    let item = revision.to_json();
                    out.push_str(&format!(
                        "\n  {} by {} in {}: {:?}",
                        item["revision_type"].as_str().unwrap_or("?"),
                        item["author"].as_str().unwrap_or("?"),
                        item["story"].as_str().unwrap_or("?"),
                        item["text"].as_str().unwrap_or(""),
                    ));
                }
                out
            });
            Ok(0)
        }
        Command::Resolve {
            input,
            accept,
            reject: _,
            author,
            out,
            acknowledge_protection,
        } => {
            let mut document = Document::open_path(input, limits)?;
            if *acknowledge_protection {
                document.acknowledge_protection()?;
            }
            let mut revisions = document.revisions()?;
            let count = if *accept {
                revisions.accept_all(&mut document, author.as_deref())?
            } else {
                revisions.reject_all(&mut document, author.as_deref())?
            };
            let patch = document.save(out)?;
            let action = if *accept { "accepted" } else { "rejected" };
            let value = json!({"schema": "docrs_resolve", "version": 1, "action": action, "count": count, "save": patch.to_json(), "out": out});
            emit(json_mode, value, || {
                format!("wrote {} ({count} revisions {action})", out.display())
            });
            Ok(0)
        }
        Command::Append {
            input,
            source,
            styles,
            section,
            headers,
            out,
            acknowledge_protection,
        } => {
            let options = docrs::composition::AppendOptions {
                section: docrs::composition::SectionStart::parse(section)?,
                styles: docrs::composition::StylesMode::parse(styles)?,
                headers: docrs::composition::HeadersFrom::parse(headers)?,
            };
            let mut document = Document::open_path(input, limits)?;
            let source = Document::open_path(source, limits)?;
            if *acknowledge_protection {
                document.acknowledge_protection()?;
            }
            let report = document.append_document(&source, &options)?;
            let patch = document.save(out)?;
            emit_composition(json_mode, "docrs_append", &report, &patch, out);
            Ok(0)
        }
        Command::InsertFrom {
            input,
            source,
            start,
            anchor,
            end,
            count,
            exclude_start,
            exclude_end,
            styles,
            out,
            acknowledge_protection,
        } => {
            let options = docrs::composition::InsertBlocksOptions {
                end_anchor: end.as_deref().map(docrs::anchor::BlockTarget::Text),
                count: *count,
                styles: docrs::composition::StylesMode::parse(styles)?,
                include_start: !*exclude_start,
                include_end: !*exclude_end,
            };
            let mut document = Document::open_path(input, limits)?;
            let source = Document::open_path(source, limits)?;
            if *acknowledge_protection {
                document.acknowledge_protection()?;
            }
            let report = document.insert_blocks_from(
                &source,
                docrs::anchor::BlockTarget::Text(start),
                docrs::anchor::BlockTarget::Text(anchor),
                &options,
            )?;
            let patch = document.save(out)?;
            emit_composition(json_mode, "docrs_insert_from", &report, &patch, out);
            Ok(0)
        }
        Command::FromMarkdown { source, from } => {
            let format = markdown_input_format(&source.input, from.as_deref())?;
            convert(source, format, "docrs_from_markdown", limits, json_mode)
        }
        Command::FromHtml(source) => convert(
            source,
            InputFormat::Html,
            "docrs_from_html",
            limits,
            json_mode,
        ),
        Command::FromJson(source) => convert(
            source,
            InputFormat::Tiptap,
            "docrs_from_json",
            limits,
            json_mode,
        ),
        Command::MarkdownReference { out } => {
            let document = docrs::markdown::reference_document()?;
            let patch = document.save(out)?;
            emit(
                json_mode,
                json!({"schema": "docrs_markdown_reference", "version": 1, "save": patch.to_json(), "out": out}),
                || {
                    format!(
                        "wrote {} (restyle it in Word, then pass it as --reference)",
                        out.display()
                    )
                },
            );
            Ok(0)
        }
        Command::Layout { file, fonts, lines } => {
            let (document, _) = open_reference(Some(file), limits, false)?;
            let fonts = fonts::load(fonts)?;
            let txn = docrs::txn::Txn::new(&document);
            let main = document.package().main_part_name().to_string();
            if *lines {
                let round = |v: f64| (v * 100.0).round() / 100.0;
                for (path, err) in &fonts.skipped {
                    eprintln!("docrs: font skipped: {}: {}", path.display(), err.message());
                }
                let closed = std::cell::Cell::new(false);
                let out = std::cell::RefCell::new(std::io::stdout().lock());
                let write = |value: serde_json::Value| {
                    writeln!(out.borrow_mut(), "{value}").map_err(|err| {
                        closed.set(err.kind() == std::io::ErrorKind::BrokenPipe);
                        Error::io("write_failed", format!("writing a line: {err}"), err)
                    })
                };
                let result = docrs::layout::records_with(
                    &txn,
                    &main,
                    &fonts.set,
                    |looks, links| {
                        let looks: Vec<_> = looks.iter().map(|l| look_json(l, round)).collect();
                        let links: Vec<_> = links.iter().map(link_json).collect();
                        write(json!({"looks": looks, "links": links}))
                    },
                    |record| write(record_json(&record, round)),
                );
                return match result {
                    Err(_) if closed.get() => Ok(0),
                    result => result.map(|()| 0),
                };
            }
            let layout = docrs::layout::layout_with(&txn, &main, &fonts.set)?;
            let tree = txn.tree(&main)?;
            let contents = tree
                .child_elements(tree.root())
                .find(|&n| docrs::wml::is_w(tree, n, "body"))
                .map(|body| paragraphs::contents_paragraphs(tree, body))
                .unwrap_or_default();
            let mut value = json!({
                "schema": "docrs_layout",
                "version": 1,
                "pages": layout.pages,
                "missing_fonts": layout.missing_fonts,
                "embedded_fonts": layout.embedded_fonts,
                "paragraphs": layout.placed.iter().map(|p| json!({
                    "page": p.page,
                    "y": (p.y * 100.0).round() / 100.0,
                    "row_end": p.row_end,
                    "text": p.node.filter(|_| !p.row_end).map(|n| paragraphs::shown_text(tree, n).chars().take(40).collect::<String>()),
                    "toc": p.node.is_some_and(|n| contents.contains(&n)),
                    "heading": p.node.filter(|_| !p.row_end).is_some_and(|n| tree.child_elements(n).any(|c| tree.local_name(c) == Some("bookmarkStart"))),
                })).collect::<Vec<_>>(),
            });
            fonts.report_skipped(&mut value);
            let text = serde_json::to_string_pretty(&value).unwrap_or_default();
            emit(json_mode, value, || text);
            Ok(0)
        }
        Command::Fonts { files, font_dirs } => {
            let loaded = fonts::load(&FontArgs {
                fonts: files.clone(),
                font_dirs: font_dirs.clone(),
            })?;
            let value = json!({
                "schema": "docrs_fonts",
                "version": 1,
                "files": loaded.files.iter().map(|(path, faces)| json!({
                    "path": path,
                    "faces": faces.iter().map(|face| json!({
                        "family": face.family,
                        "bold": face.bold,
                        "italic": face.italic,
                        "style": fonts::style(face),
                        "variable": face.variable,
                        "cff": face.cff,
                        "embeddable": face.embeddable,
                        "subsettable": face.subsettable,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "skipped": loaded.skipped_json(),
            });
            emit(json_mode, value, || fonts_text(&loaded));
            Ok(0)
        }
        Command::Pdf {
            input,
            out,
            preflight,
            strict,
            fonts,
        } => {
            let (document, set_aside) = open_reference(Some(input), limits, false)?;
            let out = match out {
                Some(out) if !preflight => out,
                _ => {
                    let report = docrs::pdf::preflight(&document)?;
                    let mut value = report.to_json();
                    value["set_aside"] = json!(set_aside);
                    emit(json_mode, value, || preflight_text(&report, &set_aside));
                    return Ok(0);
                }
            };
            refuse_overwriting(out, &[input])?;
            let fonts = fonts::load(fonts)?;
            let report = write_pdf(&document, &fonts, *strict, limits, out)?;
            let mut value = json!({"schema": "docrs_pdf", "version": 1, "pdf": report.to_json(), "set_aside": set_aside, "out": out});
            fonts.report_skipped(&mut value);
            emit(json_mode, value, || {
                let mut text = pdf_text(&report, out);
                text.push_str(&fonts.skipped_text());
                text
            });
            Ok(0)
        }
        Command::MarkdownInspect {
            reference,
            styles,
            page_numbers,
            cover_page,
            no_cover_page,
            strict_package,
        } => {
            let (document, set_aside) = open_reference(Some(reference), limits, *strict_package)?;
            let options = docrs::markdown::MarkdownOptions {
                style_map: style_map(styles)?,
                page_numbers: *page_numbers,
                cover_page: cover_page_option(*cover_page, *no_cover_page),
                ..Default::default()
            };
            let mut inspection = docrs::markdown::inspect(&document, &options)?;
            inspection["reference_set_aside"] = json!(set_aside);
            let value = json!({
                "schema": "docrs_markdown_inspect",
                "version": 1,
                "inspection": inspection,
            });
            let text = serde_json::to_string_pretty(&value["inspection"]).unwrap_or_default();
            emit(json_mode, value, || text);
            Ok(0)
        }
        Command::MarkdownStyles {
            reference,
            styles,
            strict_package,
        } => {
            let (document, _) = open_reference(reference.as_deref(), limits, *strict_package)?;
            let roles = docrs::markdown::role_styles(&document, &style_map(styles)?)?;
            let value = json!({
                "schema": "docrs_markdown_styles",
                "version": 1,
                "roles": roles.iter().map(|(role, style)| json!({
                    "role": role.key(),
                    "candidates": role.candidates(),
                    "style": style,
                })).collect::<Vec<_>>(),
            });
            emit(json_mode, value, || {
                roles
                    .iter()
                    .map(|(role, style)| match style {
                        Some(style) => format!("{:<19} {}", role.key(), style.name),
                        None => format!(
                            "{:<19} missing (tries {}; added on conversion unless --strict-styles)",
                            role.key(),
                            role.candidates().join(", ")
                        ),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(0)
        }
        Command::Roundtrip { input, out } => {
            let document = Document::open_path(input, limits)?;
            let report = document.save(out)?;
            emit(json_mode, report.to_json(), || {
                format!(
                    "wrote {} ({})",
                    out.display(),
                    if report.verbatim_copy {
                        "verbatim copy"
                    } else {
                        "patched"
                    }
                )
            });
            Ok(0)
        }
    }
}

/// Renders `document` as PDF with `fonts` and writes it to `out` atomically.
fn write_pdf(
    document: &Document,
    fonts: &fonts::Loaded,
    strict: bool,
    limits: &Limits,
    out: &Path,
) -> Result<docrs::pdf::PdfReport> {
    let mut options = docrs::pdf::PdfOptions::default();
    options.fonts = fonts.set.clone();
    options.strict = strict;
    if *limits == Limits::trusted() {
        options.limits = docrs::pdf::PdfLimits::trusted();
    }
    let (bytes, report) = docrs::pdf::render(document, &options)?;
    docrs::package::save::write_atomic(out, &bytes, None)?;
    Ok(report)
}

/// Refuses an output that is one of `inputs`, through a symbolic or hard
/// link, or a directory the write would create, too. Creates nothing.
fn refuse_overwriting(out: &Path, inputs: &[&Path]) -> Result<()> {
    let destination = docrs::package::save::destination(out)?;
    let Some(target) = landing(&destination) else {
        return Ok(());
    };
    for input in inputs {
        if same_file::is_same_file(input, &target).unwrap_or(false) {
            return Err(Error::invalid_argument(
                "output_is_input",
                format!(
                    "{} is {}; write the PDF elsewhere",
                    out.display(),
                    input.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Where a write to `path` lands once it has created the directories it
/// needs: its longest existing ancestor resolved, then the rest applied
/// as names, since a directory the write creates is no symbolic link.
fn landing(path: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let absolute = std::path::absolute(path).ok()?;
    let mut components = absolute.components();
    let mut existing = std::path::PathBuf::new();
    let mut missing = None;
    for component in components.by_ref() {
        let next = existing.join(component);
        if std::fs::metadata(&next).is_err() {
            missing = Some(component);
            break;
        }
        existing = next;
    }
    let mut out = existing.canonicalize().ok()?;
    for component in missing.into_iter().chain(components) {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
            _ => {}
        }
    }
    Some(out)
}

fn pdf_text(report: &docrs::pdf::PdfReport, out: &Path) -> String {
    let mut text = format!(
        "wrote {} ({} pages, experimental)",
        out.display(),
        report.pages.unwrap_or(0)
    );
    for warning in &report.warnings {
        text.push_str(&format!(
            "\n  warning: {}: {}",
            warning.code, warning.message
        ));
    }
    text
}

fn preflight_text(report: &docrs::pdf::PdfReport, set_aside: &[String]) -> String {
    let mut lines: Vec<String> = report
        .warnings
        .iter()
        .map(|warning| format!("{}: {}", warning.code, warning.message))
        .collect();
    if lines.is_empty() {
        lines.push("nothing a PDF would leave out".to_string());
    }
    lines.extend(set_aside.iter().map(|item| format!("  package: {item}")));
    lines.join("\n")
}

/// The refusal for a `replace --all` that replaced nothing: no match, or
/// every match refused (the first refusal's kind and reason).
/// The format `from-markdown` reads: what the hidden `--from` names, else
/// HTML for an .html/.htm file and TipTap JSON for a .json file, as before
/// `from-html` and `from-json` existed, else Markdown.
fn markdown_input_format(input: &Path, from: Option<&str>) -> Result<InputFormat> {
    let extension = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match from {
        Some("markdown" | "md") => Ok(InputFormat::Markdown),
        Some("html") => Ok(InputFormat::Html),
        Some("tiptap" | "json") => Ok(InputFormat::Tiptap),
        Some(other) => Err(Error::invalid_argument(
            "invalid_format",
            format!("--from must be markdown, html or tiptap, got {other:?}"),
        )),
        None => Ok(match extension.as_deref() {
            Some("html" | "htm") => InputFormat::Html,
            Some("json") => InputFormat::Tiptap,
            _ => InputFormat::Markdown,
        }),
    }
}

/// `from-markdown`, `from-html` and `from-json`: `source` read as `format`
/// into the reference document, reported under `schema`.
fn convert(
    source: &ConvertArgs,
    format: InputFormat,
    schema: &str,
    limits: &Limits,
    json_mode: bool,
) -> Result<u8> {
    let ConvertArgs {
        input,
        reference,
        out,
        format: output_format,
        placeholder,
        append,
        styles,
        strict_styles,
        strict,
        resource_path,
        toc,
        no_toc,
        toc_depth,
        toc_title,
        shift_heading_level,
        smart,
        no_title_block,
        no_properties,
        no_captions,
        converter_lists,
        toc_page_numbers,
        no_toc_numbers,
        page_numbers,
        cover_page,
        no_cover_page,
        acknowledge_protection,
        strict_package,
        fonts,
    } = source;
    let fonts = fonts::load(fonts)?;
    let text = if input.as_os_str() == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .map_err(|err| Error::io("read_failed", "reading standard input", err))?;
        text
    } else {
        String::from_utf8(read(input)?).map_err(|_| {
            Error::invalid_argument(
                "markdown_not_utf8",
                format!("{} is not UTF-8 text", input.display()),
            )
        })?
    };
    // The library reads no image file without a directory; the
    // command reads them beside the source file (standard input: the
    // current directory).
    let resource_dir = resource_path.clone().or_else(|| {
        Some(if input.as_os_str() == "-" {
            std::path::PathBuf::from(".")
        } else {
            input.parent().map(Path::to_path_buf).unwrap_or_default()
        })
    });
    let options = MarkdownOptions {
        placement: match (placeholder, append) {
            (Some(marker), _) => Placement::Placeholder(marker.clone()),
            (None, true) => Placement::Append,
            (None, false) => Placement::ReplaceBody,
        },
        style_policy: if *strict_styles {
            StylePolicy::Strict
        } else {
            StylePolicy::Create
        },
        style_map: style_map(styles)?,
        resource_dir,
        smart_punctuation: *smart,
        toc: if *toc {
            Some(true)
        } else if *no_toc {
            Some(false)
        } else {
            None
        },
        toc_depth: *toc_depth,
        toc_title: toc_title.clone(),
        heading_offset: *shift_heading_level,
        title_block: !no_title_block,
        set_properties: !no_properties,
        figure_captions: !no_captions && format == InputFormat::Markdown,
        strict: *strict,
        template_lists: !converter_lists,
        toc_page_numbers: *toc_page_numbers,
        toc_numbers: !no_toc_numbers,
        page_numbers: *page_numbers,
        cover_page: cover_page_option(*cover_page, *no_cover_page),
        images: Default::default(),
        now: None,
    };
    let (mut document, set_aside) = open_reference(reference.as_deref(), limits, *strict_package)?;
    if *acknowledge_protection {
        document.acknowledge_protection()?;
    }
    let report =
        docrs::markdown::insert_source_with(&mut document, &text, format, &options, &fonts.set)?;
    let mut value = json!({"schema": schema, "version": 1, "markdown": report.to_json(), "reference_set_aside": set_aside, "out": out});
    let pdf = match output_format {
        OutputFormat::Docx => {
            value["save"] = document.save(out)?.to_json();
            None
        }
        OutputFormat::Pdf => {
            let inputs: Vec<&Path> = [Some(input.as_path()), reference.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            refuse_overwriting(out, &inputs)?;
            let pdf = write_pdf(&document, &fonts, *strict, limits, out)?;
            value["pdf"] = pdf.to_json();
            Some(pdf)
        }
    };
    fonts.report_skipped(&mut value);
    emit(json_mode, value, || {
        let c = &report.counts;
        let mut text = format!(
            "wrote {}{} ({} headings, {} paragraphs, {} list items, {} tables, {} images, {} footnotes, {} links)",
            out.display(),
            pdf.as_ref().map_or(String::new(), |pdf| format!(
                " as PDF ({} pages, experimental)",
                pdf.pages.unwrap_or(0)
            )),
            c.headings,
            c.paragraphs,
            c.list_items,
            c.tables,
            c.images,
            c.footnotes,
            c.links
        );
        for item in &set_aside {
            text.push_str(&format!("\n  reference package: {item}"));
        }
        text.push_str(&fonts.skipped_text());
        if !report.created_styles.is_empty() {
            text.push_str(&format!(
                "\n  added styles missing from the reference: {}",
                report.created_styles.join(", ")
            ));
        }
        for warning in &report.warnings {
            match warning.line {
                Some(line) => {
                    text.push_str(&format!("\n  warning (line {line}): {}", warning.message))
                }
                None => text.push_str(&format!("\n  warning: {}", warning.message)),
            }
        }
        text
    });
    Ok(0)
}

/// Each file's faces, a line each, then the files skipped.
fn fonts_text(loaded: &fonts::Loaded) -> String {
    let mut text = String::new();
    for (path, faces) in &loaded.files {
        text.push_str(&format!("{}\n", path.display()));
        for face in faces {
            let mut notes = Vec::new();
            if face.cff {
                notes.push("CFF outlines");
            }
            if face.variable {
                notes.push("variable: measured at its default instance");
            }
            if !face.embeddable {
                notes.push("its licence forbids embedding");
            } else if !face.subsettable {
                notes.push("its licence forbids subsetting");
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join("; "))
            };
            text.push_str(&format!(
                "  {:<28} {}{notes}\n",
                face.family,
                fonts::style(face)
            ));
        }
    }
    for (path, err) in &loaded.skipped {
        text.push_str(&format!("skipped {}: {}\n", path.display(), err.message()));
    }
    text.trim_end().to_string()
}

fn nothing_replaced(find: &str, refused: &[docrs::replace::RefusedMatch]) -> Error {
    use docrs::RefusalKind;
    let Some(first) = refused.first() else {
        return Error::refusal(
            RefusalKind::TargetNotFound,
            "no_match",
            format!("no match for {find:?} in any story part; nothing replaced"),
        );
    };
    let kind = [
        RefusalKind::MalformedPackage,
        RefusalKind::AmbiguousTarget,
        RefusalKind::TargetNotFound,
        RefusalKind::UnsupportedStructure,
        RefusalKind::BoundaryViolation,
        RefusalKind::RelationshipPolicy,
        RefusalKind::DocumentProtected,
    ]
    .into_iter()
    .find(|kind| kind.code() == first.code)
    .unwrap_or(RefusalKind::UnsupportedStructure);
    Error::refusal(
        kind,
        first.reason,
        format!(
            "all {} match(es) for {find:?} were refused; nothing replaced (first, in {}: {})",
            refused.len(),
            first.story,
            first.message
        ),
    )
}

/// Open a Markdown reference document. It is only read, so irregularities
/// Word tolerates are set aside unless `strict` asks for the guarded refusal.
fn open_reference(
    path: Option<&Path>,
    limits: &Limits,
    strict: bool,
) -> Result<(Document, Vec<String>)> {
    let tolerance = if strict {
        docrs::Tolerance::default()
    } else {
        docrs::Tolerance::word()
    };
    match path {
        Some(path) => Document::open_path_with(path, limits, tolerance),
        None => Ok((Document::new(), Vec::new())),
    }
}

/// A `layout --lines` line, or a row part (with a `row` key).
fn record_json(record: &docrs::layout::Record, round: impl Fn(f64) -> f64) -> serde_json::Value {
    match record {
        docrs::layout::Record::Line(line) => {
            let runs: Vec<_> = line
                .runs
                .iter()
                .map(|run| json!({"text": run.text, "look": run.look}))
                .collect();
            json!({
                "page": line.page,
                "story": line.story,
                "x": round(line.x),
                "y": round(line.y),
                "width": round(line.width),
                "height": round(line.height),
                "baseline": round(line.baseline),
                "tabs": line.tabs.iter().map(|(from, to)| [round(*from), round(*to)]).collect::<Vec<_>>(),
                "leaders": line.leaders.iter().map(|leader| leader.as_ref().map(|run| json!({
                    "text": run.text.chars().next().map(String::from),
                    "x": run.origins.first().map(|x| round(*x)),
                    "count": run.origins.len(),
                }))).collect::<Vec<_>>(),
                "slack": round(line.slack),
                "gaps": line.gaps,
                "last": line.last,
                "align": line.align,
                "text": line.text,
                "runs": runs,
            })
        }
        docrs::layout::Record::Row(row) => {
            let cells: Vec<_> = row
                .cells
                .iter()
                .map(|cell| {
                    json!({
                        "x": round(cell.x),
                        "width": round(cell.width),
                        "from": round(cell.from),
                        "to": round(cell.to),
                    })
                })
                .collect();
            json!({
                "row": row.row,
                "page": row.page,
                "story": row.story,
                "x": round(row.x),
                "y": round(row.y),
                "width": round(row.width),
                "height": round(row.height),
                "first": row.first,
                "last": row.last,
                "repeat": row.repeat,
                "cells": cells,
            })
        }
        _ => serde_json::Value::Null,
    }
}

fn look_json(look: &docrs::layout::Appearance, round: impl Fn(f64) -> f64) -> serde_json::Value {
    let hex = |rgb: Option<docrs::layout::Rgb>| rgb.map(|rgb| rgb.to_string());
    json!({
        "metric": look.metric,
        "size": round(look.size),
        "rise": round(look.rise),
        "colour": look.colour.to_string(),
        "bold": look.bold,
        "italic": look.italic,
        "underline": look.underline.map(|u| json!({"style": u.style.name(), "colour": u.colour.to_string()})),
        "strike": look.strike.name(),
        "highlight": hex(look.highlight),
        "shade": hex(look.shade),
        "caps": look.caps.name(),
        "spacing": round(look.spacing),
        "scale": round(look.scale),
        "link": look.link,
    })
}

fn link_json(link: &docrs::layout::Link) -> serde_json::Value {
    match link {
        docrs::layout::Link::Uri(uri) => json!({"uri": uri}),
        docrs::layout::Link::Anchor(anchor) => json!({"anchor": anchor}),
        _ => serde_json::Value::Null,
    }
}
