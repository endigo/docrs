//! Bookmark enumeration, creation and deletion — upstream `bookmarks.py`.
//!
//! Creation wraps an exact [`Span`]; ids are unique across every story;
//! deletion removes only the markers — never the text — and refuses while a
//! field instruction (or an internal hyperlink) still references the name.
//!
//! Adapted from paper-docx (MIT); see `NOTICE.md`.

use serde::Serialize;

use crate::fields::fieldcode;
use crate::links::{
    add_next, add_previous, isolate_edge_runs, require_live_span, validate_span_fresh,
};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::search::Span;
use crate::story::{is_direct_run_child, project_run_child, stories};
use crate::txn::Txn;
use crate::unicode::{casefold, is_py_space};
use crate::wml::{W, W14, is_w, iter_nodes, w_attr};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

#[path = "p1_pyclass.rs"]
pub(crate) mod pyclass;

use pyclass::{is_py_alnum, is_py_alpha, is_py_decimal, is_py_word};

/// One bookmark (upstream `BookmarkInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkInfo {
    /// `w:name`.
    pub name: String,
    /// `w:id` (`-1` when missing).
    pub bookmark_id: i64,
    /// Story name.
    pub story: String,
    /// Visible (current-view) text between the markers.
    pub text: String,
}

#[derive(Serialize)]
struct BookmarkJson<'a> {
    name: &'a str,
    id: i64,
    story: &'a str,
    text: &'a str,
    is_point: bool,
}

impl BookmarkInfo {
    /// A point bookmark wraps no text.
    pub fn is_point(&self) -> bool {
        self.text.is_empty()
    }

    /// Upstream `to_dict()`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(BookmarkJson {
            name: &self.name,
            id: self.bookmark_id,
            story: &self.story,
            text: &self.text,
            is_point: self.is_point(),
        })
        .unwrap_or_default()
    }
}

/// Python `int(text)` for decimal strings (whitespace, sign, `_`
/// separators and Unicode decimal digits accepted).
pub(crate) fn py_int(raw: &str) -> Option<i64> {
    let trimmed = raw.trim_matches(is_py_space);
    let (negative, body) = match trimmed.chars().next() {
        Some('-') => (true, &trimmed[1..]),
        Some('+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut value: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        if !is_py_decimal(c) {
            return None;
        }
        let digit = decimal_value(c)?;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(if negative { -value } else { value })
}

/// Numeric value of a Unicode decimal digit: `Nd` characters come in
/// complete, contiguous 0–9 sets, so the offset from the start of the
/// contiguous decimal run, modulo ten, is the digit value.
fn decimal_value(c: char) -> Option<i64> {
    if c.is_ascii_digit() {
        return Some(c as i64 - '0' as i64);
    }
    let cp = c as u32;
    let mut start = cp;
    while start > 0 && char::from_u32(start - 1).is_some_and(is_py_decimal) {
        start -= 1;
    }
    Some(((cp - start) % 10) as i64)
}

fn int_or_invalid(raw: &str) -> Result<i64> {
    py_int(raw).ok_or_else(|| {
        Error::invalid_argument(
            "non_numeric_bookmark_id",
            format!(
                "invalid literal for int() with base 10: {}",
                crate::controls::py_repr(raw)
            ),
        )
    })
}

const CONTENT_REVISION_WRAPPERS: &[(&str, &str)] = &[
    (W, "ins"),
    (W, "del"),
    (W, "moveFrom"),
    (W, "moveTo"),
    (W14, "conflictIns"),
    (W14, "conflictDel"),
];
const CURRENT_HIDDEN_REVISION_WRAPPERS: &[(&str, &str)] =
    &[(W, "del"), (W, "moveFrom"), (W14, "conflictDel")];

fn is_any(tree: &XmlTree, node: NodeId, names: &[(&str, &str)]) -> bool {
    names.iter().any(|(ns, local)| tree.is(node, ns, local))
}

/// Upstream `_revision_scope`: revision wrappers around `node`, outermost
/// first.
fn revision_scope(tree: &XmlTree, node: NodeId) -> Vec<NodeId> {
    let mut scope: Vec<NodeId> = tree
        .ancestors(node)
        .filter(|a| is_any(tree, *a, CONTENT_REVISION_WRAPPERS))
        .collect();
    scope.reverse();
    scope
}

fn bookmark_text(tree: &XmlTree, start: NodeId, raw_id: Option<&str>) -> String {
    let Some(raw_id) = raw_id else {
        return String::new();
    };
    let mut pieces = String::new();
    let mut collecting = false;
    let mut last_paragraph: Option<NodeId> = None;
    for node in iter_nodes(tree, tree.root()) {
        if node == start {
            collecting = true;
            continue;
        }
        if !collecting {
            continue;
        }
        if is_w(tree, node, "bookmarkEnd") && w_attr(tree, node, "id") == Some(raw_id) {
            break;
        }
        if !is_direct_run_child(tree, node)
            || is_w(tree, node, "delText")
            || is_w(tree, node, "instrText")
        {
            continue;
        }
        if revision_scope(tree, node)
            .iter()
            .any(|w| is_any(tree, *w, CURRENT_HIDDEN_REVISION_WRAPPERS))
        {
            continue;
        }
        let projection = project_run_child(tree, node);
        if projection.barrier || projection.text.is_empty() {
            continue;
        }
        let paragraph = tree.ancestors(node).find(|a| is_w(tree, *a, "p"));
        if last_paragraph.is_some() && paragraph != last_paragraph {
            pieces.push('\n');
        }
        last_paragraph = paragraph;
        pieces.push_str(&projection.text);
    }
    pieces
}

/// A `w:bookmarkStart` with its parsed id.
struct BookmarkStart<'t> {
    story: String,
    tree: &'t XmlTree,
    element: NodeId,
    id: i64,
}

impl BookmarkStart<'_> {
    fn name(&self) -> &str {
        w_attr(self.tree, self.element, "name").unwrap_or("")
    }
}

/// Every `w:bookmarkStart` across every story, document order (refusing a
/// non-numeric id).
fn bookmark_starts(document: &Document) -> Result<Vec<BookmarkStart<'_>>> {
    let mut found = Vec::new();
    for story in stories(document) {
        let tree = document.package().xml(&story.part)?;
        for element in iter_nodes(tree, tree.root()).filter(|n| is_w(tree, *n, "bookmarkStart")) {
            let id = match w_attr(tree, element, "id") {
                Some(raw) => int_or_invalid(raw)?,
                None => -1,
            };
            found.push(BookmarkStart {
                story: story.name.clone(),
                tree,
                element,
                id,
            });
        }
    }
    Ok(found)
}

/// Every bookmark across every story, document order (upstream
/// `list_bookmarks`).
pub fn list_bookmarks(document: &Document) -> Result<Vec<BookmarkInfo>> {
    Ok(bookmark_starts(document)?
        .into_iter()
        .map(|start| BookmarkInfo {
            name: start.name().to_string(),
            bookmark_id: start.id,
            text: bookmark_text(
                start.tree,
                start.element,
                w_attr(start.tree, start.element, "id"),
            ),
            story: start.story,
        })
        .collect())
}

/// Word's bookmark-name rule (upstream `_NAME_RE`,
/// `^[^\W\d_][\w]{0,39}$` on a Python `str`, where `$` also matches before
/// one trailing newline).
fn is_word_legal_name(name: &str) -> bool {
    let body = name.strip_suffix('\n').unwrap_or(name);
    let mut chars = body.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_py_word(first) || is_py_decimal(first) || first == '_' {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    rest.len() <= 39 && rest.iter().all(|c| is_py_word(*c))
}

fn next_bookmark_id(document: &Document) -> Result<i64> {
    let mut highest = 0;
    for story in stories(document) {
        let tree = document.package().xml(&story.part)?;
        for node in iter_nodes(tree, tree.root()) {
            if (is_w(tree, node, "bookmarkStart") || is_w(tree, node, "bookmarkEnd"))
                && let Some(raw) = w_attr(tree, node, "id")
            {
                highest = highest.max(int_or_invalid(raw)?);
            }
        }
    }
    Ok(highest + 1)
}

/// Upstream `_revision_wrapper_between`.
fn revision_wrapper_between(tree: &XmlTree, left: NodeId, right: NodeId) -> bool {
    let mut inside = false;
    for node in iter_nodes(tree, tree.root()) {
        if node == left {
            inside = true;
        }
        if inside && is_any(tree, node, CONTENT_REVISION_WRAPPERS) {
            return true;
        }
        if node == right {
            return !inside;
        }
    }
    true
}

/// Bookmark exactly `span`'s text under `name` (upstream
/// `create_bookmark`).
///
/// Splits boundary runs so the markers wrap the span's characters exactly.
/// On success the span stays usable (its offsets follow the split), as
/// upstream's span object does. Refuses a protected document, a foreign or
/// stale span, a Word-illegal or already used (case-insensitive) name, a
/// field result, non-run content and pending-revision boundaries.
pub fn create_bookmark(
    document: &mut Document,
    span: &mut Span,
    name: &str,
) -> Result<BookmarkInfo> {
    let (staged, info, offsets) = {
        let doc: &Document = document;
        require_live_span(doc, span)?;
        refuse_if_protected(doc, "create a bookmark", OperationClass::BodyContent)?;
        if !is_word_legal_name(name) {
            return Err(Error::invalid_argument(
                "illegal_bookmark_name",
                format!(
                    "bookmark name {} is not Word-legal (start with a letter; letters, digits and underscores only; max 40 chars)",
                    crate::controls::py_repr(name)
                ),
            ));
        }
        let folded = casefold(name);
        if bookmark_starts(doc)?
            .iter()
            .any(|start| casefold(start.name()) == folded)
        {
            return Err(Error::unsupported(
                "duplicate_bookmark_name",
                format!(
                    "a bookmark named {} already exists; bookmark names are document-unique",
                    crate::controls::py_repr(name)
                ),
            ));
        }
        validate_span_fresh(doc, span)?;
        if span.in_field {
            return Err(Error::unsupported(
                "span_in_field_result",
                "the span lies inside a field result; a bookmark planted there is destroyed the next time the field updates — bookmark the field's source text instead",
            ));
        }
        if span.atoms.iter().any(|atom| atom.run.is_none()) {
            return Err(Error::unsupported(
                "span_outside_runs",
                "the span includes content outside ordinary runs; bookmark a plain text range",
            ));
        }
        let tree = doc.package().xml(&span.part)?;
        let first_run = span.atoms[0].run.unwrap_or(span.atoms[0].element);
        let last_run = span.atoms[span.atoms.len() - 1]
            .run
            .unwrap_or(span.atoms[span.atoms.len() - 1].element);
        if revision_scope(tree, first_run) != revision_scope(tree, last_run) {
            return Err(Error::unsupported(
                "crosses_revision_scopes",
                "bookmark endpoints cross pending revision scopes; resolve the revision or bookmark text within one revision scope. Nothing was changed",
            ));
        }
        if revision_wrapper_between(tree, first_run, last_run) {
            return Err(Error::unsupported(
                "crosses_revision_content",
                "bookmark text crosses pending revision content; resolve the revision first. Nothing was changed",
            ));
        }
        let bookmark_id = next_bookmark_id(doc)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&span.part)?;
        let offsets = isolate_edge_runs(tree, span)?;
        let start = tree.create_element(first_run, W, "bookmarkStart")?;
        add_previous(tree, first_run, start)?;
        tree.set_attr(start, Some(W), "id", &bookmark_id.to_string())?;
        tree.set_attr(start, Some(W), "name", name)?;
        let end = tree.create_element(last_run, W, "bookmarkEnd")?;
        add_next(tree, last_run, end)?;
        tree.set_attr(end, Some(W), "id", &bookmark_id.to_string())?;
        let info = BookmarkInfo {
            name: name.to_string(),
            bookmark_id,
            story: span.story.clone(),
            text: span.text.clone(),
        };
        (txn.finish()?, info, offsets)
    };
    document.commit(staged)?;
    span.start_offset = offsets.0;
    span.end_offset = offsets.1;
    Ok(info)
}

/// Bookmark identifiers of a formula field (`= ...`), read with Python's
/// character classes (upstream `_fieldcode`'s `str.isalpha`/`isalnum`).
fn formula_operands(instruction: &str, names: &[&str]) -> Vec<String> {
    let wanted: Vec<String> = names.iter().map(|n| casefold(n)).collect();
    let chars: Vec<char> = instruction.chars().collect();
    let mut out = Vec::new();
    let mut index = 0;
    let mut quoted = false;
    while index < chars.len() {
        let c = chars[index];
        if c == '"' {
            quoted = !quoted;
            index += 1;
            continue;
        }
        if quoted || !(is_py_alpha(c) || c == '_') {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < chars.len() && (is_py_alnum(chars[index]) || chars[index] == '_') {
            index += 1;
        }
        let value: String = chars[start..index].iter().collect();
        let mut following = index;
        while following < chars.len() && is_py_space(chars[following]) {
            following += 1;
        }
        if wanted.contains(&casefold(&value))
            && (following >= chars.len() || chars[following] != '(')
        {
            out.push(value);
        }
    }
    out
}

/// Upstream `bookmark_operands(instruction, implicit_names=names)`.
fn bookmark_operands(instruction: &str, names: &[&str]) -> Result<Vec<String>> {
    // Tokenized first: an unterminated quoted operand refuses in a formula too.
    let operands = fieldcode::bookmark_operands(instruction, names)?;
    if instruction.trim_start_matches(is_py_space).starts_with('=') {
        return Ok(formula_operands(instruction, names));
    }
    Ok(operands.into_iter().map(|operand| operand.value).collect())
}

/// Upstream `_iter_field_instructions`: concatenated instruction text per
/// complex field.
fn field_instructions(tree: &XmlTree) -> Vec<String> {
    crate::composition::iter_field_instructions(tree, tree.root())
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

fn field_references(document: &Document, name: &str) -> Result<usize> {
    let target = casefold(name);
    let names = [name];
    let mut count = 0;
    for story in stories(document) {
        let tree = document.package().xml(&story.part)?;
        for instruction in field_instructions(tree) {
            if bookmark_operands(&instruction, &names)?
                .iter()
                .any(|operand| casefold(operand) == target)
            {
                count += 1;
            }
        }
        for node in iter_nodes(tree, tree.root()).filter(|n| is_w(tree, *n, "fldSimple")) {
            if let Some(instr) = w_attr(tree, node, "instr").filter(|i| !i.is_empty())
                && bookmark_operands(instr, &names)?
                    .iter()
                    .any(|operand| casefold(operand) == target)
            {
                count += 1;
            }
        }
        for link in iter_nodes(tree, tree.root()).filter(|n| is_w(tree, *n, "hyperlink")) {
            if w_attr(tree, link, "anchor").is_some_and(|anchor| casefold(anchor) == target) {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Remove `name`'s markers, leaving the text in place (upstream
/// `delete_bookmark`).
///
/// Refuses while a field instruction or internal hyperlink references the
/// name, a protected document, an unknown or case-ambiguous name, malformed
/// marker ids anywhere in a story, and markers in different revision
/// scopes.
pub fn delete_bookmark(document: &mut Document, name: &str) -> Result<()> {
    let staged = {
        let doc: &Document = document;
        refuse_if_protected(doc, "delete a bookmark", OperationClass::BodyContent)?;
        let folded = casefold(name);
        let mut matching: Vec<String> = Vec::new();
        for story in stories(doc) {
            let tree = doc.package().xml(&story.part)?;
            for start in iter_nodes(tree, tree.root()).filter(|n| is_w(tree, *n, "bookmarkStart")) {
                let actual = w_attr(tree, start, "name").unwrap_or("").to_string();
                if casefold(&actual) == folded && !matching.contains(&actual) {
                    matching.push(actual);
                }
            }
        }
        if matching.len() > 1 {
            return Err(Error::unsupported(
                "case_ambiguous_bookmark",
                format!(
                    "bookmark name {} is case-ambiguous; nothing was changed",
                    crate::controls::py_repr(name)
                ),
            ));
        }
        let resolved = matching.pop().unwrap_or_else(|| name.to_string());
        let referencing = field_references(doc, &resolved)?;
        if referencing > 0 {
            return Err(Error::unsupported(
                "bookmark_referenced",
                format!(
                    "bookmark {} is referenced by {referencing} field instruction(s); update or remove those fields first",
                    crate::controls::py_repr(name)
                ),
            ));
        }
        let resolved_folded = casefold(&resolved);
        let mut pairs: Vec<(String, NodeId, NodeId)> = Vec::new();
        for story in stories(doc) {
            let tree = doc.package().xml(&story.part)?;
            let mut starts: Vec<(i64, NodeId)> = Vec::new();
            let mut ends: Vec<(i64, NodeId)> = Vec::new();
            for (local, destination) in [("bookmarkStart", &mut starts), ("bookmarkEnd", &mut ends)]
            {
                for marker in iter_nodes(tree, tree.root()).filter(|n| is_w(tree, *n, local)) {
                    let raw = w_attr(tree, marker, "id");
                    let id = raw.and_then(py_int).unwrap_or(-1);
                    if id < 0 || destination.iter().any(|(existing, _)| *existing == id) {
                        return Err(Error::unsupported(
                            "malformed_bookmark_ids",
                            format!(
                                "bookmark markers have missing, non-numeric, or duplicate w:id {}; nothing was changed",
                                raw.map(crate::controls::py_repr)
                                    .unwrap_or_else(|| "None".into())
                            ),
                        ));
                    }
                    destination.push((id, marker));
                }
            }
            let mut start_ids: Vec<i64> = starts.iter().map(|(id, _)| *id).collect();
            let mut end_ids: Vec<i64> = ends.iter().map(|(id, _)| *id).collect();
            start_ids.sort_unstable();
            end_ids.sort_unstable();
            if start_ids != end_ids {
                return Err(Error::unsupported(
                    "unpaired_bookmark_markers",
                    "bookmark start/end markers are not one-to-one; nothing was changed",
                ));
            }
            for (id, start) in &starts {
                if casefold(w_attr(tree, *start, "name").unwrap_or("")) == resolved_folded {
                    let end = ends
                        .iter()
                        .find(|(end_id, _)| end_id == id)
                        .map(|(_, end)| *end)
                        .ok_or_else(|| Error::internal("bookmark_pair", "paired end vanished"))?;
                    pairs.push((story.part.clone(), *start, end));
                }
            }
        }
        if pairs.is_empty() {
            return Err(Error::refusal(
                RefusalKind::TargetNotFound,
                "no_bookmark",
                format!(
                    "no bookmark named {} exists",
                    crate::controls::py_repr(name)
                ),
            ));
        }
        for (part, start, end) in &pairs {
            let tree = doc.package().xml(part)?;
            if revision_scope(tree, *start) != revision_scope(tree, *end) {
                return Err(Error::unsupported(
                    "markers_cross_revision_scopes",
                    format!(
                        "bookmark {} has markers in different revision scopes; deleting it could leave a dangling marker. Nothing was changed",
                        crate::controls::py_repr(name)
                    ),
                ));
            }
        }
        let mut txn = Txn::new(doc);
        for (part, start, end) in &pairs {
            let tree = txn.tree_mut(part)?;
            crate::controls::remove_with_tail(tree, *end)?;
            crate::controls::remove_with_tail(tree, *start)?;
        }
        txn.finish()?
    };
    document.commit(staged)
}

impl Document {
    /// Every bookmark (see [`list_bookmarks`]).
    pub fn bookmarks(&self) -> Result<Vec<BookmarkInfo>> {
        list_bookmarks(self)
    }

    /// Bookmark `span` as `name` (see [`create_bookmark`]).
    pub fn create_bookmark(&mut self, span: &mut Span, name: &str) -> Result<BookmarkInfo> {
        create_bookmark(self, span, name)
    }

    /// Delete bookmark `name` (see [`delete_bookmark`]).
    pub fn delete_bookmark(&mut self, name: &str) -> Result<()> {
        delete_bookmark(self, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_python_regex() {
        assert!(is_word_legal_name("KeyPhrase"));
        assert!(is_word_legal_name("Äbc_1"));
        assert!(is_word_legal_name("Name\n"));
        assert!(!is_word_legal_name("1starts"));
        assert!(!is_word_legal_name("_hidden"));
        assert!(!is_word_legal_name("has space"));
        assert!(!is_word_legal_name(""));
        assert!(!is_word_legal_name(&"x".repeat(41)));
        assert!(is_word_legal_name(&"x".repeat(40)));
    }

    #[test]
    fn python_int_parsing() {
        assert_eq!(py_int("05"), Some(5));
        assert_eq!(py_int(" +7 "), Some(7));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int("\u{0665}"), Some(5));
        assert_eq!(py_int("bad"), None);
        assert_eq!(py_int(""), None);
    }

    #[test]
    fn operands_follow_upstream() {
        let names = ["DefinedTerm"];
        let found = |instr: &str| bookmark_operands(instr, &names).unwrap();
        assert_eq!(found(" REF DefinedTerm \\h "), vec!["DefinedTerm"]);
        assert_eq!(found(" DefinedTerm \\* MERGEFORMAT "), vec!["DefinedTerm"]);
        assert!(found(" QUOTE \"literal REF DefinedTerm text\" ").is_empty());
        assert_eq!(found(" =ROUND(DefinedTerm,0) "), vec!["DefinedTerm"]);
        assert_eq!(
            found(" HYPERLINK \\l \"DefinedTerm\" "),
            vec!["DefinedTerm"]
        );
        assert!(bookmark_operands(" REF \"open", &names).is_err());
    }
}
