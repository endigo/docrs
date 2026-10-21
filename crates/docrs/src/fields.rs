//! Field authoring — formulas, never values (upstream `fields.py` and
//! `_fieldcode.py`, MIT; see `NOTICE.md`).
//!
//! Authors PAGE/NUMPAGES/DATE simple fields, REF/PAGEREF cross-references,
//! SEQ captions and the TOC complex field. Every inserted field carries
//! placeholder result text and arms `w:updateFields`: this crate never
//! computes a field's value.

use crate::anchor::{
    BlockTarget, locate_paragraph, require_current_view, resolve_anchor_paragraph,
};
use crate::blocks::{insert_after, refuse_paragraph_in_open_field};
use crate::numbering::{py_int, set_p_style};
use crate::oxml::{add_t, append, set_w, w_el};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::story;
use crate::txn::Txn;
use crate::wml::{W, w_attr, w_child};
use crate::xml::{NodeId, XML_NS, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// `CT_Settings` children that follow `w:updateFields`.
const UPDATE_FIELDS_SUCCESSORS: &[(&str, &str)] = &[
    (W, "hdrShapeDefaults"),
    (W, "footnotePr"),
    (W, "endnotePr"),
    (W, "compat"),
    (W, "docVars"),
    (W, "rsids"),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/math",
        "mathPr",
    ),
    (W, "attachedSchema"),
    (W, "themeFontLang"),
    (W, "clrSchemeMapping"),
    (W, "doNotIncludeSubdocsInStats"),
    (W, "doNotAutoCompressPictures"),
    (W, "forceUpgrade"),
    (W, "captions"),
    (W, "readModeInkLockDown"),
    (W, "smartTagType"),
    (
        "http://schemas.openxmlformats.org/schemaLibrary/2006/main",
        "schemaLibrary",
    ),
    (W, "shapeDefaults"),
    (W, "doNotEmbedSmartTags"),
    (W, "decimalSymbol"),
    (W, "listSeparator"),
];

/// Reference kinds for [`add_reference_field`].
pub const REFERENCE_KINDS: &[&str] = &["number", "page", "text"];

/// Upstream `_set_update_fields_on_open` (creates the settings part if
/// needed).
pub(crate) fn set_update_fields_on_open(txn: &mut Txn<'_>) -> Result<()> {
    let settings = crate::parts::ensure_settings_part(txn)?;
    let tree = txn.tree_mut(&settings)?;
    let root = tree.root();
    if let Some(existing) = w_child(tree, root, "updateFields") {
        return set_w(tree, existing, "val", "true");
    }
    let element = w_el(tree, root, "updateFields")?;
    set_w(tree, element, "val", "true")?;
    let position = tree.children(root).iter().position(|child| {
        UPDATE_FIELDS_SUCCESSORS
            .iter()
            .any(|(ns, local)| tree.is(*child, ns, local))
    });
    match position {
        Some(index) => tree.insert(root, index, element),
        None => append(tree, root, element),
    }
}

/// Upstream `_simple_field`.
fn simple_field(
    tree: &mut XmlTree,
    scope: NodeId,
    instr: &str,
    placeholder: &str,
) -> Result<NodeId> {
    let field = w_el(tree, scope, "fldSimple")?;
    set_w(tree, field, "instr", instr)?;
    let run = w_el(tree, field, "r")?;
    add_t(tree, run, placeholder)?;
    append(tree, field, run)?;
    Ok(field)
}

struct Target {
    part: String,
    element: NodeId,
}

fn paragraph_target(
    document: &Document,
    paragraph: BlockTarget<'_>,
    operation: &str,
) -> Result<Target> {
    let located = locate_paragraph(document, paragraph)?;
    require_current_view(paragraph)?;
    refuse_if_protected(document, operation, OperationClass::BodyContent)?;
    Ok(Target {
        part: located.part,
        element: located.element,
    })
}

fn append_simple_field(
    document: &mut Document,
    paragraph: BlockTarget<'_>,
    instr: &str,
    placeholder: &str,
) -> Result<()> {
    let target = paragraph_target(document, paragraph, "insert a field")?;
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&target.part)?;
    let field = simple_field(tree, target.element, instr, placeholder)?;
    append(tree, target.element, field)?;
    set_update_fields_on_open(&mut txn)?;
    let staged = txn.finish()?;
    document.commit(staged)
}

/// Append a PAGE field (placeholder `1`).
pub fn add_page_number_field(document: &mut Document, paragraph: BlockTarget<'_>) -> Result<()> {
    append_simple_field(document, paragraph, " PAGE ", "1")
}

/// Append a NUMPAGES field (placeholder `1`).
pub fn add_page_count_field(document: &mut Document, paragraph: BlockTarget<'_>) -> Result<()> {
    append_simple_field(document, paragraph, " NUMPAGES ", "1")
}

/// Append a DATE field; `date_format` is Word's `\@` picture (double quotes
/// removed). The result stays the `(date)` placeholder.
pub fn add_date_field(
    document: &mut Document,
    paragraph: BlockTarget<'_>,
    date_format: Option<&str>,
) -> Result<()> {
    let instr = match date_format.filter(|f| !f.is_empty()) {
        Some(format) => format!(" DATE \\@ \"{}\" ", format.replace('"', "")),
        None => " DATE ".to_string(),
    };
    append_simple_field(document, paragraph, &instr, "(date)")
}

/// Bookmark names as upstream `list_bookmarks` reports them (every
/// `w:bookmarkStart` in every story; a malformed id is upstream's raw
/// `ValueError`).
fn bookmark_names(document: &Document) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for story in story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        for node in tree.descendants(tree.root()) {
            if !tree.is(node, W, "bookmarkStart") {
                continue;
            }
            if let Some(raw) = w_attr(tree, node, "id")
                && py_int(raw).is_none()
            {
                return Err(Error::invalid_argument(
                    "malformed_bookmark_id",
                    format!("invalid literal for int() with base 10: {raw:?}"),
                ));
            }
            names.push(w_attr(tree, node, "name").unwrap_or("").to_string());
        }
    }
    Ok(names)
}

/// Append a cross-reference to `bookmark`: its text (`"text"`), page
/// (`"page"`) or paragraph number (`"number"`).
pub fn add_reference_field(
    document: &mut Document,
    paragraph: BlockTarget<'_>,
    bookmark: &str,
    kind: &str,
) -> Result<()> {
    let instr = match kind {
        "text" => format!(" REF {bookmark} \\h "),
        "page" => format!(" PAGEREF {bookmark} \\h "),
        "number" => format!(" REF {bookmark} \\r \\h "),
        other => {
            return Err(Error::invalid_argument(
                "invalid_reference_kind",
                format!("kind must be one of {REFERENCE_KINDS:?}, got {other:?}"),
            ));
        }
    };
    let target = paragraph_target(document, paragraph, "insert a field")?;
    if !bookmark_names(document)?
        .iter()
        .any(|name| name == bookmark)
    {
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "bookmark_not_found",
            format!("no bookmark named {bookmark:?} exists to cross-reference"),
        ));
    }
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&target.part)?;
    let field = simple_field(tree, target.element, &instr, "(reference)")?;
    append(tree, target.element, field)?;
    set_update_fields_on_open(&mut txn)?;
    let staged = txn.finish()?;
    document.commit(staged)
}

/// Python `str.isalnum` for one character (approximated by Unicode
/// alphabetic/numeric properties).
fn py_isalnum(c: char) -> bool {
    c.is_alphanumeric()
}

/// Append a SEQ caption (`Figure 1`) to `paragraph` and set its style id to
/// `Caption` (not checked against the styles part, as upstream).
pub fn add_caption(
    document: &mut Document,
    paragraph: BlockTarget<'_>,
    label: &str,
    description: &str,
) -> Result<()> {
    if label.is_empty() {
        return Err(Error::invalid_argument(
            "empty_label",
            "label must be a non-empty caption name",
        ));
    }
    let target = paragraph_target(document, paragraph, "insert a caption")?;
    let seq: String = label.chars().filter(|c| py_isalnum(*c)).collect();
    let seq_name = if seq.is_empty() {
        "Figure".to_string()
    } else {
        seq
    };
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&target.part)?;
    let p = target.element;
    set_p_style(tree, p, Some("Caption"))?;
    let prefix = w_el(tree, p, "r")?;
    add_t(tree, prefix, &format!("{label} "))?;
    append(tree, p, prefix)?;
    let field = simple_field(tree, p, &format!(" SEQ {seq_name} \\* ARABIC "), "1")?;
    append(tree, p, field)?;
    if !description.is_empty() {
        let suffix = w_el(tree, p, "r")?;
        add_t(tree, suffix, &format!(": {description}"))?;
        append(tree, p, suffix)?;
    }
    set_update_fields_on_open(&mut txn)?;
    let staged = txn.finish()?;
    document.commit(staged)
}

pub(crate) fn fld_char_run(
    tree: &mut XmlTree,
    scope: NodeId,
    kind: &str,
    dirty: bool,
) -> Result<NodeId> {
    let run = w_el(tree, scope, "r")?;
    let fld_char = w_el(tree, run, "fldChar")?;
    set_w(tree, fld_char, "fldCharType", kind)?;
    if dirty {
        set_w(tree, fld_char, "dirty", "true")?;
    }
    append(tree, run, fld_char)?;
    Ok(run)
}

/// Insert a TOC complex field in a new paragraph after `anchor` (main body
/// only), marked dirty; `levels` maps to `\o "low-high"`.
pub fn insert_toc_after(
    document: &mut Document,
    anchor: BlockTarget<'_>,
    levels: (i64, i64),
) -> Result<()> {
    let (low, high) = levels;
    if !(1 <= low && low <= high && high <= 9) {
        return Err(Error::invalid_argument(
            "invalid_toc_levels",
            format!("levels must satisfy 1 <= low <= high <= 9, got ({low}, {high})"),
        ));
    }
    let located = resolve_anchor_paragraph(document, anchor)?;
    if located.story != "word/document.xml" {
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "toc_outside_body",
            format!(
                "a TOC inserts into the main document body (anchor is in {})",
                located.story
            ),
        ));
    }
    {
        let tree = document.package().xml(&located.part)?;
        refuse_paragraph_in_open_field(tree, &located.story, located.element, true)?;
    }
    let mut txn = Txn::new(document);
    let tree = txn.tree_mut(&located.part)?;
    let scope = located.element;
    let paragraph = w_el(tree, scope, "p")?;
    let begin = fld_char_run(tree, paragraph, "begin", true)?;
    let instr_run = w_el(tree, paragraph, "r")?;
    let instr = w_el(tree, instr_run, "instrText")?;
    tree.set_attr(instr, Some(XML_NS), "space", "preserve")?;
    let text = tree.create_text(format!(" TOC \\o \"{low}-{high}\" \\h \\z \\u "));
    tree.insert(instr, 0, text)?;
    append(tree, instr_run, instr)?;
    let separate = fld_char_run(tree, paragraph, "separate", false)?;
    let placeholder = w_el(tree, paragraph, "r")?;
    add_t(
        tree,
        placeholder,
        "Table of contents placeholder \u{2014} update fields to build it.",
    )?;
    let end = fld_char_run(tree, paragraph, "end", false)?;
    for piece in [begin, instr_run, separate, placeholder, end] {
        append(tree, paragraph, piece)?;
    }
    insert_after(tree, located.element, &[paragraph])?;
    set_update_fields_on_open(&mut txn)?;
    let staged = txn.finish()?;
    document.commit(staged)
}

impl Document {
    /// See [`add_page_number_field`].
    pub fn add_page_number_field(&mut self, paragraph: BlockTarget<'_>) -> Result<()> {
        add_page_number_field(self, paragraph)
    }

    /// See [`add_page_count_field`].
    pub fn add_page_count_field(&mut self, paragraph: BlockTarget<'_>) -> Result<()> {
        add_page_count_field(self, paragraph)
    }

    /// See [`add_date_field`].
    pub fn add_date_field(
        &mut self,
        paragraph: BlockTarget<'_>,
        date_format: Option<&str>,
    ) -> Result<()> {
        add_date_field(self, paragraph, date_format)
    }

    /// See [`add_reference_field`].
    pub fn add_reference_field(
        &mut self,
        paragraph: BlockTarget<'_>,
        bookmark: &str,
        kind: &str,
    ) -> Result<()> {
        add_reference_field(self, paragraph, bookmark, kind)
    }

    /// See [`add_caption`].
    pub fn add_caption(
        &mut self,
        paragraph: BlockTarget<'_>,
        label: &str,
        description: &str,
    ) -> Result<()> {
        add_caption(self, paragraph, label, description)
    }

    /// See [`insert_toc_after`].
    pub fn insert_toc_after(&mut self, anchor: BlockTarget<'_>, levels: (i64, i64)) -> Result<()> {
        insert_toc_after(self, anchor, levels)
    }
}

/// Minimal, position-preserving parser for bookmark-bearing Word field codes
/// (upstream `_fieldcode.py`). Offsets are scalar positions.
pub mod fieldcode {
    use crate::unicode::{casefold, is_py_space};
    use crate::{Error, RefusalKind, Result};

    /// Field commands Word recognizes (an implicit bookmark reference is a
    /// first token that is *not* one of these).
    pub const FIELD_COMMANDS: &[&str] = &[
        "ADDRESSBLOCK",
        "ADVANCE",
        "ASK",
        "AUTHOR",
        "AUTONUM",
        "AUTONUMLGL",
        "AUTONUMOUT",
        "AUTOTEXT",
        "AUTOTEXTLIST",
        "BARCODE",
        "BIBLIOGRAPHY",
        "BIDIOUTLINE",
        "CITATION",
        "COMMENTS",
        "COMPARE",
        "CREATEDATE",
        "DATABASE",
        "DATE",
        "DISPLAYBARCODE",
        "DOCVARIABLE",
        "DOCPROPERTY",
        "EDITTIME",
        "EMBED",
        "EQ",
        "FILENAME",
        "FILESIZE",
        "FILLIN",
        "FORMCHECKBOX",
        "FORMDROPDOWN",
        "FORMTEXT",
        "GLOSSARY",
        "GOTOBUTTON",
        "GREETINGLINE",
        "HYPERLINK",
        "IF",
        "INCLUDEPICTURE",
        "INCLUDETEXT",
        "INDEX",
        "INFO",
        "KEYWORDS",
        "LASTSAVEDBY",
        "LINK",
        "LISTNUM",
        "MACROBUTTON",
        "MERGEBARCODE",
        "MERGEFIELD",
        "MERGEREC",
        "MERGESEQ",
        "NEXT",
        "NEXTIF",
        "NOTEREF",
        "NUMCHARS",
        "NUMPAGES",
        "NUMWORDS",
        "PAGE",
        "PAGEREF",
        "PRINT",
        "PRINTDATE",
        "PRIVATE",
        "QUOTE",
        "RD",
        "REF",
        "REVNUM",
        "SAVEDATE",
        "SECTION",
        "SECTIONPAGES",
        "SEQ",
        "SET",
        "SKIPIF",
        "STYLEREF",
        "SUBJECT",
        "SYMBOL",
        "TA",
        "TC",
        "TEMPLATE",
        "TIME",
        "TITLE",
        "TOA",
        "TOC",
        "USERADDRESS",
        "USERINITIALS",
        "USERNAME",
        "XE",
    ];

    /// One token of a field instruction.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FieldOperand {
        /// Unquoted value.
        pub value: String,
        /// Scalar start offset (inclusive, quotes included).
        pub start: usize,
        /// Scalar end offset (exclusive).
        pub end: usize,
        /// Written in double quotes.
        pub quoted: bool,
    }

    impl FieldOperand {
        /// Render `value` in this operand's position (quoted when the
        /// original was or when `value` contains whitespace).
        pub fn render(&self, value: &str) -> String {
            let quoted = self.quoted || value.chars().any(is_py_space);
            if !quoted {
                return value.to_string();
            }
            format!("\"{}\"", value.replace('"', "\\\""))
        }
    }

    fn tokens(instruction: &str) -> Result<Vec<FieldOperand>> {
        let chars: Vec<char> = instruction.chars().collect();
        let mut out = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            while index < chars.len() && is_py_space(chars[index]) {
                index += 1;
            }
            if index >= chars.len() {
                break;
            }
            let start = index;
            if chars[index] != '"' {
                while index < chars.len() && !is_py_space(chars[index]) {
                    index += 1;
                }
                out.push(FieldOperand {
                    value: chars[start..index].iter().collect(),
                    start,
                    end: index,
                    quoted: false,
                });
                continue;
            }
            index += 1;
            let mut value = String::new();
            let mut closed = false;
            while index < chars.len() {
                let c = chars[index];
                if c == '"' {
                    index += 1;
                    out.push(FieldOperand {
                        value: std::mem::take(&mut value),
                        start,
                        end: index,
                        quoted: true,
                    });
                    closed = true;
                    break;
                }
                if c == '\\' && index + 1 < chars.len() && chars[index + 1] == '"' {
                    value.push('"');
                    index += 2;
                    continue;
                }
                value.push(c);
                index += 1;
            }
            if !closed {
                return Err(Error::refusal(
                    RefusalKind::UnsupportedStructure,
                    "unterminated_field_operand",
                    "field instruction contains an unterminated quoted operand; nothing was changed",
                ));
            }
        }
        Ok(out)
    }

    /// The operand following `command` when the instruction starts with it.
    pub fn command_operand(instruction: &str, command: &str) -> Result<Option<FieldOperand>> {
        let tokens = tokens(instruction)?;
        if tokens.len() < 2
            || tokens[0].quoted
            || tokens[0].value.to_uppercase() != command.to_uppercase()
        {
            return Ok(None);
        }
        let operand = tokens[1].clone();
        Ok(if !operand.quoted && operand.value.starts_with('\\') {
            None
        } else {
            Some(operand)
        })
    }

    /// Bookmark operands of `instruction` (REF/PAGEREF/NOTEREF/GOTOBUTTON
    /// operand, HYPERLINK `\l`, TOC `\b`, formula identifiers and implicit
    /// references among `implicit_names`).
    pub fn bookmark_operands(
        instruction: &str,
        implicit_names: &[&str],
    ) -> Result<Vec<FieldOperand>> {
        let tokens = tokens(instruction)?;
        let command = match tokens.first() {
            Some(first) if !first.quoted => first.value.to_uppercase(),
            _ => String::new(),
        };
        if instruction.trim_start_matches(is_py_space).starts_with('=') {
            return Ok(formula_bookmark_operands(instruction, implicit_names));
        }
        if matches!(
            command.as_str(),
            "REF" | "PAGEREF" | "NOTEREF" | "GOTOBUTTON"
        ) {
            return Ok(command_operand(instruction, &command)?
                .into_iter()
                .collect());
        }
        let switch = match command.as_str() {
            "HYPERLINK" => Some("\\l"),
            "TOC" => Some("\\b"),
            _ => None,
        };
        if let Some(switch) = switch {
            let mut out = Vec::new();
            for (index, token) in tokens
                .iter()
                .enumerate()
                .take(tokens.len().saturating_sub(1))
            {
                let next = &tokens[index + 1];
                if !token.quoted
                    && casefold(&token.value) == switch
                    && (next.quoted || !next.value.starts_with('\\'))
                {
                    out.push(next.clone());
                }
            }
            return Ok(out);
        }
        let implicit: Vec<String> = implicit_names.iter().map(|name| casefold(name)).collect();
        if let Some(first) = tokens.first()
            && !first.quoted
            && !FIELD_COMMANDS.contains(&command.as_str())
            && implicit.contains(&casefold(&first.value))
        {
            return Ok(vec![first.clone()]);
        }
        Ok(Vec::new())
    }

    fn is_identifier_start(c: char) -> bool {
        c.is_alphabetic() || c == '_'
    }

    fn is_identifier_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    fn formula_bookmark_operands(instruction: &str, names: &[&str]) -> Vec<FieldOperand> {
        let wanted: Vec<String> = names.iter().map(|name| casefold(name)).collect();
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
            if quoted || !is_identifier_start(c) {
                index += 1;
                continue;
            }
            let start = index;
            index += 1;
            while index < chars.len() && is_identifier_char(chars[index]) {
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
                out.push(FieldOperand {
                    value,
                    start,
                    end: index,
                    quoted: false,
                });
            }
        }
        out
    }

    fn rewrite(instruction: &str, mut replacements: Vec<(FieldOperand, String)>) -> String {
        replacements.sort_by_key(|(operand, _)| std::cmp::Reverse(operand.start));
        let mut chars: Vec<char> = instruction.chars().collect();
        for (operand, replacement) in replacements {
            let rendered: Vec<char> = operand.render(&replacement).chars().collect();
            chars.splice(operand.start..operand.end, rendered);
        }
        chars.into_iter().collect()
    }

    fn lookup<'a>(renames: &'a [(&str, &str)], value: &str) -> Option<&'a str> {
        let folded = casefold(value);
        // Python dict semantics: a later key with the same fold wins.
        renames
            .iter()
            .rev()
            .find(|(old, _)| casefold(old) == folded)
            .map(|(_, new)| *new)
    }

    /// Rename bookmark operands (`renames` maps old → new, case-insensitive).
    pub fn rewrite_bookmark_operands(
        instruction: &str,
        renames: &[(&str, &str)],
    ) -> Result<String> {
        let names: Vec<&str> = renames.iter().map(|(old, _)| *old).collect();
        let replacements = bookmark_operands(instruction, &names)?
            .into_iter()
            .filter_map(|operand| {
                lookup(renames, &operand.value).map(|new| (operand, new.to_string()))
            })
            .collect();
        Ok(rewrite(instruction, replacements))
    }

    /// Rename the operand of `command` when it is among `renames`.
    pub fn rewrite_command_operand(
        instruction: &str,
        command: &str,
        renames: &[(&str, &str)],
    ) -> Result<String> {
        let Some(operand) = command_operand(instruction, command)? else {
            return Ok(instruction.to_string());
        };
        match lookup(renames, &operand.value) {
            Some(new) => Ok(rewrite(instruction, vec![(operand, new.to_string())])),
            None => Ok(instruction.to_string()),
        }
    }
}
