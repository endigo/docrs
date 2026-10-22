//! Front matter: the YAML (`---`) or TOML (`+++`) block at the top of a
//! Markdown file. Only the subset documents use for metadata is read —
//! scalars, quoted strings, block scalars and flat lists; anything else is
//! ignored rather than guessed at.

use std::collections::BTreeMap;

use super::ast::MetadataKind;

/// A front-matter value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Scalar(String),
    List(Vec<String>),
}

impl Value {
    fn items(&self) -> Vec<String> {
        match self {
            Value::Scalar(text) => vec![text.clone()],
            Value::List(items) => items.clone(),
        }
    }
}

/// Document metadata recognized from front matter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub date: Option<String>,
    pub abstract_: Option<String>,
    pub keywords: Vec<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub lang: Option<String>,
    pub toc: Option<bool>,
    pub toc_title: Option<String>,
    pub toc_depth: Option<i64>,
}

pub(crate) fn metadata(kind: MetadataKind, text: &str) -> Metadata {
    let fields = match kind {
        MetadataKind::Yaml => parse_yaml(text),
        MetadataKind::Toml => parse_toml(text),
    };
    let scalar = |key: &str| match fields.get(key) {
        Some(Value::Scalar(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::List(items)) if !items.is_empty() => Some(items.join(", ")),
        _ => None,
    };
    let flag = |key: &str| {
        scalar(key).and_then(|v| match v.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        })
    };
    Metadata {
        title: scalar("title"),
        subtitle: scalar("subtitle"),
        authors: fields
            .get("author")
            .or_else(|| fields.get("authors"))
            .map(Value::items)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| !a.is_empty())
            .collect(),
        date: scalar("date"),
        abstract_: scalar("abstract"),
        keywords: fields
            .get("keywords")
            .or_else(|| fields.get("tags"))
            .map(|v| match v {
                Value::Scalar(text) => text
                    .split(',')
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .collect(),
                Value::List(items) => items.clone(),
            })
            .unwrap_or_default(),
        subject: scalar("subject"),
        description: scalar("description"),
        category: scalar("category"),
        lang: scalar("lang").or_else(|| scalar("language")),
        toc: flag("toc").or_else(|| flag("table-of-contents")),
        toc_title: scalar("toc-title"),
        toc_depth: scalar("toc-depth").and_then(|d| d.parse().ok()),
    }
}

fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        let inner = &raw[1..raw.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return raw[1..raw.len() - 1].replace("''", "'");
    }
    strip_comment(raw).to_string()
}

/// A plain (unquoted) scalar ends at ` #`.
fn strip_comment(raw: &str) -> &str {
    match raw.find(" #") {
        Some(index) => raw[..index].trim_end(),
        None => raw,
    }
}

fn inline_list(raw: &str) -> Option<Vec<String>> {
    let inner = raw.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                current.push(c);
            }
            (Some(q), _) if c == q => {
                quote = None;
                current.push(c);
            }
            (None, ',') => {
                items.push(unquote(&current));
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        items.push(unquote(&current));
    }
    Some(items.into_iter().filter(|i| !i.is_empty()).collect())
}

/// Leading ASCII spaces: YAML indents with spaces only, so a tab or a
/// non-ASCII space (U+00A0, U+3000, …) starts the content. Always a char
/// boundary of `line`.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Top-level keys of a YAML mapping.
pub(crate) fn parse_yaml(text: &str) -> BTreeMap<String, Value> {
    let lines: Vec<&str> = text.lines().collect();
    let mut fields = BTreeMap::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || indent_of(line) > 0 {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches('"').trim_matches('\'').to_string();
        let rest = rest.trim();
        // Indented continuation lines belong to this key.
        let start = index;
        while index < lines.len() && (lines[index].trim().is_empty() || indent_of(lines[index]) > 0)
        {
            index += 1;
        }
        let block = &lines[start..index];
        let value = if rest.starts_with('|') || rest.starts_with('>') {
            let folded = rest.starts_with('>');
            let indent = block
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| indent_of(l))
                .min()
                .unwrap_or(0);
            // Blank lines may be shorter than the indentation, or blank with
            // other white space, so each drops at most its own spaces.
            let body: Vec<&str> = block
                .iter()
                .map(|l| &l[indent_of(l).min(indent)..])
                .collect();
            let joined = if folded { fold(&body) } else { body.join("\n") };
            Value::Scalar(joined.trim_end().to_string())
        } else if let Some(items) = inline_list(rest) {
            Value::List(items)
        } else if rest.is_empty() {
            let items: Vec<String> = block
                .iter()
                .filter_map(|l| l.trim_start().strip_prefix('-'))
                .map(|item| {
                    let item = item.trim();
                    // `- name: X` (a mapping item): keep the name.
                    match item.split_once(':') {
                        Some((k, v)) if k.trim() == "name" => unquote(v),
                        _ => unquote(item),
                    }
                })
                .filter(|i| !i.is_empty())
                .collect();
            if items.is_empty() {
                // Plain multi-line scalar.
                let joined = block.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
                Value::Scalar(joined.trim().to_string())
            } else {
                Value::List(items)
            }
        } else {
            let mut scalar = unquote(rest);
            // Continuation lines of a plain scalar fold with spaces.
            for l in block.iter().filter(|l| !l.trim().is_empty()) {
                scalar.push(' ');
                scalar.push_str(l.trim());
            }
            Value::Scalar(scalar)
        };
        fields.insert(key, value);
    }
    fields
}

/// YAML `>` folding: single newlines become spaces, blank lines newlines.
fn fold(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut previous_blank = true;
    for line in lines {
        if line.trim().is_empty() {
            out.push('\n');
            previous_blank = true;
        } else {
            if !previous_blank {
                out.push(' ');
            }
            out.push_str(line.trim_end());
            previous_blank = false;
        }
    }
    out
}

/// Top-level `key = value` pairs of TOML front matter.
pub(crate) fn parse_toml(text: &str) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').to_string();
        let value = match inline_list(value) {
            Some(items) => Value::List(items),
            None => Value::Scalar(unquote(value)),
        };
        fields.insert(key, value);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_subset() {
        let meta = metadata(
            MetadataKind::Yaml,
            "title: \"A: B\"\nsubtitle: plain # comment\nauthor:\n  - Ann\n  - name: Bo\n\
             keywords: [x, 'y z']\nabstract: |\n  Line one\n  line two\ntoc: true\ntoc-depth: 2\n",
        );
        assert_eq!(meta.title.as_deref(), Some("A: B"));
        assert_eq!(meta.subtitle.as_deref(), Some("plain"));
        assert_eq!(meta.authors, vec!["Ann", "Bo"]);
        assert_eq!(meta.keywords, vec!["x", "y z"]);
        assert_eq!(meta.abstract_.as_deref(), Some("Line one\nline two"));
        assert_eq!(meta.toc, Some(true));
        assert_eq!(meta.toc_depth, Some(2));
    }

    #[test]
    fn folded_scalars_and_toml() {
        let meta = metadata(MetadataKind::Yaml, "abstract: >\n  one\n  two\n\n  three\n");
        assert_eq!(meta.abstract_.as_deref(), Some("one two\nthree"));
        let meta = metadata(
            MetadataKind::Toml,
            "title = \"T\"\nauthor = [\"A\", \"B\"]\ntoc = false\n",
        );
        assert_eq!(meta.title.as_deref(), Some("T"));
        assert_eq!(meta.authors, vec!["A", "B"]);
        assert_eq!(meta.toc, Some(false));
    }

    #[test]
    fn indentation_is_ascii_spaces_only() {
        // A no-break space is content: it ends the block scalar and its
        // line, with no key, is ignored.
        let meta = metadata(MetadataKind::Yaml, "abstract: |\n x\n\u{a0}y\ntitle: T\n");
        assert_eq!(meta.abstract_.as_deref(), Some("x"));
        assert_eq!(meta.title.as_deref(), Some("T"));
        // Inside the block it is kept, after the spaces.
        let meta = metadata(
            MetadataKind::Yaml,
            "abstract: >\n  one\n  \u{a0}two\n\u{3000}\n  three\n",
        );
        assert_eq!(meta.abstract_.as_deref(), Some("one \u{a0}two\nthree"));
    }
}
