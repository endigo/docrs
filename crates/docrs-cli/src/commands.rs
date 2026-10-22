//! Command implementations.

use std::path::Path;

use docrs::package::diagnose::diagnose_path;
use docrs::package::diff::diff_packages;
use docrs::{
    Document, Error, FindOptions, Limits, MatchPolicy, ReplaceAllOptions, ReplaceOptions, Result,
    View,
};
use serde_json::{Value, json};

use crate::Command;

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
            // Success is published only after the output is committed.
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
        Command::FromMarkdown {
            input,
            from,
            reference,
            out,
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
        } => {
            use docrs::markdown::{InputFormat, MarkdownOptions, Placement, StylePolicy};
            let format = match from.as_deref() {
                Some("markdown" | "md") => InputFormat::Markdown,
                Some("html") => InputFormat::Html,
                Some("tiptap" | "json") => InputFormat::Tiptap,
                Some(other) => {
                    return Err(Error::invalid_argument(
                        "invalid_format",
                        format!("--from must be markdown, html or tiptap, got {other:?}"),
                    ));
                }
                None => match input.extension().and_then(|e| e.to_str()) {
                    Some(ext)
                        if ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm") =>
                    {
                        InputFormat::Html
                    }
                    Some(ext) if ext.eq_ignore_ascii_case("json") => InputFormat::Tiptap,
                    _ => InputFormat::Markdown,
                },
            };
            let markdown = if input.as_os_str() == "-" {
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
            // command reads them beside the Markdown (standard input: the
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
                cover_page: match (cover_page, no_cover_page) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                },
                // Images resolve on disk next to the Markdown file.
                images: Default::default(),
                now: None,
            };
            let (mut document, set_aside) =
                open_reference(reference.as_deref(), limits, *strict_package)?;
            if *acknowledge_protection {
                document.acknowledge_protection()?;
            }
            let report =
                docrs::markdown::insert_source(&mut document, &markdown, format, &options)?;
            let patch = document.save(out)?;
            let value = json!({"schema": "docrs_from_markdown", "version": 1, "markdown": report.to_json(), "reference_set_aside": set_aside, "save": patch.to_json(), "out": out});
            emit(json_mode, value, || {
                let c = &report.counts;
                let mut text = format!(
                    "wrote {} ({} headings, {} paragraphs, {} list items, {} tables, {} images, {} footnotes, {} links)",
                    out.display(),
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
                if !report.created_styles.is_empty() {
                    text.push_str(&format!(
                        "\n  added styles missing from the reference: {}",
                        report.created_styles.join(", ")
                    ));
                }
                for warning in &report.warnings {
                    match warning.line {
                        Some(line) => text
                            .push_str(&format!("\n  warning (line {line}): {}", warning.message)),
                        None => text.push_str(&format!("\n  warning: {}", warning.message)),
                    }
                }
                text
            });
            Ok(0)
        }
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
        Command::Layout { file, fonts } => {
            let (document, _) = open_reference(Some(file), limits, false)?;
            let mut faces = Vec::new();
            for path in fonts {
                let data = std::fs::read(path).map_err(|err| {
                    Error::io(
                        "font_unreadable",
                        format!("reading font {}: {err}", path.display()),
                        err,
                    )
                })?;
                faces.extend(docrs::layout::Face::parse_file(&data));
            }
            let txn = docrs::txn::Txn::new(&document);
            let main = document.package().main_part_name().to_string();
            let layout = docrs::layout::layout(&txn, &main, faces)?;
            let tree = txn.tree(&main)?;
            let value = json!({
                "schema": "docrs_layout",
                "version": 1,
                "pages": layout.pages,
                "missing_fonts": layout.missing_fonts,
                "embedded_fonts": layout.embedded_fonts,
                "paragraphs": layout.placed.iter().map(|p| json!({
                    "page": p.page,
                    "y": (p.y * 100.0).round() / 100.0,
                    "row_end": p.row_end,
                    "text": p.node.filter(|_| !p.row_end).map(|n| tree.text_content(n).chars().take(40).collect::<String>()),
                    "heading": p.node.filter(|_| !p.row_end).is_some_and(|n| tree.child_elements(n).any(|c| tree.local_name(c) == Some("bookmarkStart"))),
                })).collect::<Vec<_>>(),
            });
            let text = serde_json::to_string_pretty(&value).unwrap_or_default();
            emit(json_mode, value, || text);
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
                cover_page: match (cover_page, no_cover_page) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                },
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

/// The refusal for a `replace --all` that replaced nothing: no match, or
/// every match refused (the first refusal's kind and reason).
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
