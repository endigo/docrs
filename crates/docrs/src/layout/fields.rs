//! Fields layout shows with their current value: page numbers, page counts,
//! page references and document properties. Every other field shows the
//! result stored in the document; nothing is fetched or evaluated.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::flow::Placed;
use super::furniture::Pages;
use super::model::{Block, Model, Para, Piece, StoryId};
use crate::coreprops::{CoreProperty, read_core_properties};
use crate::fields::fieldcode;
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::xml::NodeId;

const CUSTOM_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";

/// A date or time field keeps its stored value.
pub(crate) const FIELD_CACHED: &str = "pdf_field_cached";
/// A field switch layout does not apply.
pub(crate) const FIELD_SWITCH_IGNORED: &str = "pdf_field_switch_ignored";
/// A number format layout does not write is written in arabic.
pub(crate) const FIELD_FORMAT_APPROXIMATED: &str = "pdf_field_format_approximated";

/// Letters are written up to this many repeats (zz…z, 780), arabic past it.
const MAX_LETTER_REPEATS: i64 = 30;

/// How a page number is written (`\*` switches, `w:pgNumType w:fmt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum NumberFormat {
    /// The section's format for a page number, else arabic.
    #[default]
    Section,
    Arabic,
    LowerRoman,
    UpperRoman,
    LowerLetter,
    UpperLetter,
    /// `- 1 -`.
    NumberInDash,
    /// At least two digits: `01`.
    DecimalZero,
}

impl NumberFormat {
    /// `w:pgNumType w:fmt`; `None` for a format layout does not write
    /// (words, ordinals, other scripts), which it writes in arabic.
    pub(crate) fn of_section(fmt: Option<&str>) -> Option<NumberFormat> {
        Some(match fmt {
            None | Some("decimal") => NumberFormat::Arabic,
            Some("lowerRoman") => NumberFormat::LowerRoman,
            Some("upperRoman") => NumberFormat::UpperRoman,
            Some("lowerLetter") => NumberFormat::LowerLetter,
            Some("upperLetter") => NumberFormat::UpperLetter,
            Some("numberInDash") => NumberFormat::NumberInDash,
            Some("decimalZero") => NumberFormat::DecimalZero,
            Some(_) => return None,
        })
    }

    /// A `\*` switch's format; `Err` for a number format layout does not
    /// write, `Ok(None)` for a text one (case, `MERGEFORMAT`).
    fn of_switch(value: &str) -> Result<Option<NumberFormat>, ()> {
        Ok(Some(match value {
            "roman" => NumberFormat::LowerRoman,
            "ROMAN" => NumberFormat::UpperRoman,
            "alphabetic" => NumberFormat::LowerLetter,
            "ALPHABETIC" => NumberFormat::UpperLetter,
            _ if value.eq_ignore_ascii_case("arabic") => NumberFormat::Arabic,
            _ if ["mergeformat", "charformat"]
                .iter()
                .any(|text| value.eq_ignore_ascii_case(text)) =>
            {
                return Ok(None);
            }
            _ => return Err(()),
        }))
    }

    fn or(self, fallback: NumberFormat) -> NumberFormat {
        if self == NumberFormat::Section {
            fallback
        } else {
            self
        }
    }

    /// Roman numerals past 3999, letters past 30 repeats, and letters or
    /// numerals for 0 and below are written in arabic.
    pub(crate) fn write(self, n: i64) -> String {
        match self {
            NumberFormat::LowerRoman | NumberFormat::UpperRoman if (1..4000).contains(&n) => {
                let roman = roman(n);
                if self == NumberFormat::UpperRoman {
                    roman.to_uppercase()
                } else {
                    roman
                }
            }
            NumberFormat::LowerLetter | NumberFormat::UpperLetter
                if (1..=26 * MAX_LETTER_REPEATS).contains(&n) =>
            {
                let base = if self == NumberFormat::UpperLetter {
                    b'A'
                } else {
                    b'a'
                };
                // Word repeats the letter: y, z, aa, bb.
                let letter = char::from(base + ((n - 1) % 26) as u8);
                std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect()
            }
            NumberFormat::NumberInDash => format!("- {n} -"),
            NumberFormat::DecimalZero if (0..10).contains(&n) => format!("0{n}"),
            _ => n.to_string(),
        }
    }
}

/// A `\\*` case switch, applied to the value as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Case {
    #[default]
    AsIs,
    Upper,
    Lower,
    /// `FirstCap` and `Caps`: a page number is one word.
    Capital,
}

impl Case {
    fn of_switch(value: &str) -> Option<Case> {
        let is = |name: &str| value.eq_ignore_ascii_case(name);
        Some(if is("upper") {
            Case::Upper
        } else if is("lower") {
            Case::Lower
        } else if is("firstcap") || is("caps") {
            Case::Capital
        } else {
            return None;
        })
    }

    pub(crate) fn apply(self, text: String) -> String {
        match self {
            Case::AsIs => text,
            Case::Upper => text.to_uppercase(),
            Case::Lower => text.to_lowercase(),
            Case::Capital => {
                let mut chars = text.chars();
                chars.next().map_or(String::new(), |first| {
                    first.to_uppercase().chain(chars).collect()
                })
            }
        }
    }
}

fn roman(mut n: i64) -> String {
    const DIGITS: [(i64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, digits) in DIGITS {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    out
}

/// A field whose value layout computes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FieldKind {
    Page(NumberFormat),
    NumPages(NumberFormat),
    SectionPages(NumberFormat),
    PageRef {
        bookmark: String,
        format: NumberFormat,
    },
    /// A document property, read when the model is built.
    Text(String),
}

/// A field instruction read: its kind when layout computes it, the case
/// its value is written in, and the warnings it gives with their subjects.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Instruction {
    pub kind: Option<FieldKind>,
    pub case: Case,
    pub warnings: Vec<(&'static str, &'static str)>,
}

/// An instruction's words: quoted operands as written, and unquoted ones
/// split before each switch, so `PAGE\\*roman` reads as `PAGE \\* roman`.
fn words(code: &str) -> Option<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for token in fieldcode::tokens(code).ok()? {
        if token.quoted {
            out.push((token.value, true));
            continue;
        }
        let mut rest = token.value.as_str();
        while !rest.is_empty() {
            let end = match rest.strip_prefix('\\') {
                Some(after) => 1 + after.chars().next().map_or(0, char::len_utf8),
                None => rest.find('\\').unwrap_or(rest.len()),
            };
            out.push((rest[..end].to_string(), false));
            rest = &rest[end..];
        }
    }
    Some(out)
}

/// The command of `code`, upper-cased, when its first word is one.
fn command(words: &[(String, bool)]) -> Option<String> {
    words
        .first()
        .filter(|(_, quoted)| !quoted)
        .map(|(word, _)| word.to_ascii_uppercase())
}

/// Reads `instruction` (a `w:instrText` run's text or `w:fldSimple`'s
/// `w:instr`).
pub(crate) fn instruction(code: &str, properties: &Properties) -> Instruction {
    let Some(words) = words(code) else {
        return Instruction::default();
    };
    let Some(command) = command(&words) else {
        return Instruction::default();
    };
    let mut format = NumberFormat::Section;
    let mut case = Case::AsIs;
    let mut operands = Vec::new();
    let mut warnings = Vec::new();
    let mut rest = words[1..].iter();
    while let Some((word, quoted)) = rest.next() {
        if *quoted || !word.starts_with('\\') {
            operands.push(word.as_str());
            continue;
        }
        match word.as_str() {
            "\\*" => {
                let Some((value, _)) = rest.next() else {
                    continue;
                };
                if let Some(switch) = Case::of_switch(value) {
                    case = switch;
                    continue;
                }
                match NumberFormat::of_switch(value) {
                    Ok(Some(f)) => format = f,
                    Err(()) => {
                        format = NumberFormat::Arabic;
                        warnings.push((FIELD_FORMAT_APPROXIMATED, "page field"));
                    }
                    Ok(None) => {}
                }
            }
            "\\#" => {
                rest.next();
                warnings.push((FIELD_FORMAT_APPROXIMATED, "page field"));
            }
            "\\@" => {
                rest.next();
            }
            s if s.eq_ignore_ascii_case("\\p") => {
                warnings.push((FIELD_SWITCH_IGNORED, "page reference"))
            }
            _ => {}
        }
    }
    let kind = match (command.as_str(), operands.as_slice()) {
        ("PAGE", []) => FieldKind::Page(format),
        ("NUMPAGES", []) => FieldKind::NumPages(format),
        ("SECTIONPAGES", []) => FieldKind::SectionPages(format),
        ("PAGEREF", [bookmark, ..]) => FieldKind::PageRef {
            bookmark: bookmark.to_string(),
            format,
        },
        ("DATE" | "TIME" | "PRINTDATE", _) => {
            return Instruction {
                warnings: cached(&command)
                    .map(|s| (FIELD_CACHED, s))
                    .into_iter()
                    .collect(),
                ..Instruction::default()
            };
        }
        // With an operand, these set the property instead.
        ("TITLE" | "AUTHOR" | "SUBJECT" | "KEYWORDS", []) => {
            match properties.get(&command.to_ascii_lowercase()) {
                Some(value) => FieldKind::Text(value.clone()),
                None => return Instruction::default(),
            }
        }
        ("DOCPROPERTY", [name, ..]) => match properties.get(&name.to_ascii_lowercase()) {
            Some(value) => FieldKind::Text(value.clone()),
            None => return Instruction::default(),
        },
        _ => return Instruction::default(),
    };
    if let FieldKind::Text(_) = kind {
        warnings.clear();
    }
    Instruction {
        kind: Some(kind),
        case,
        warnings,
    }
}

/// The page and date fields a PDF shows as saved when layout does not
/// compute them, as subjects.
fn cached(command: &str) -> Option<&'static str> {
    [
        "PAGE",
        "NUMPAGES",
        "SECTIONPAGES",
        "PAGEREF",
        "DATE",
        "TIME",
        "PRINTDATE",
    ]
    .into_iter()
    .find(|known| *known == command)
}

/// Whether the field begun by `node` (`w:fldChar` or `w:fldSimple`) is
/// locked: Word keeps its result and does not update it.
pub(crate) fn locked(tree: &crate::xml::XmlTree, node: NodeId) -> bool {
    crate::wml::w_attr(tree, node, "fldLock").is_some_and(crate::styles::on_off)
}

/// How a PDF shows a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shown {
    /// Computed from the PDF's pages (a page reference once its bookmark
    /// is placed).
    Computed,
    /// Its saved result, though Word would compute it: the subject.
    Cached(&'static str),
    /// Its saved result; the PDF engine does not draw it yet.
    Other,
}

/// How a PDF shows the field with instruction `code`, as layout reads it.
#[cfg_attr(
    not(feature = "pdf"),
    allow(dead_code, reason = "the PDF preflight asks")
)]
pub(crate) fn shown(code: &str) -> Shown {
    let read = instruction(code, &Properties::new());
    if matches!(read.kind, Some(ref kind) if !matches!(kind, FieldKind::Text(_))) {
        return Shown::Computed;
    }
    let command = code
        .split(|c: char| c.is_whitespace() || c == '\\')
        .find(|word| !word.is_empty())
        .unwrap_or("")
        .to_ascii_uppercase();
    cached(&command).map_or(Shown::Other, Shown::Cached)
}

/// Document properties by lower-cased name: the core properties under
/// their field names, then the custom ones.
pub(crate) type Properties = HashMap<String, String>;

pub(crate) fn properties(txn: &Txn<'_>) -> Properties {
    let mut out = Properties::new();
    if let Some(tree) = related(txn, rt::CORE_PROPERTIES) {
        let core = read_core_properties(tree, None);
        for (name, property) in [
            ("title", CoreProperty::Title),
            ("author", CoreProperty::Author),
            ("subject", CoreProperty::Subject),
            ("keywords", CoreProperty::Keywords),
            ("comments", CoreProperty::Comments),
            ("category", CoreProperty::Category),
            ("lastsavedby", CoreProperty::LastModifiedBy),
        ] {
            out.insert(name.to_string(), core.text(property).to_string());
        }
    }
    if let Some(tree) = related(txn, CUSTOM_PROPERTIES) {
        // Word formats dates, booleans and the like; only text and numbers
        // are shown as written.
        const PLAIN: &[&str] = &[
            "lpwstr", "lpstr", "bstr", "i1", "i2", "i4", "i8", "int", "ui1", "ui2", "ui4", "ui8",
            "uint", "r4", "r8", "decimal",
        ];
        for property in tree.child_elements(tree.root()) {
            let plain = tree
                .child_elements(property)
                .next()
                .and_then(|value| tree.local_name(value))
                .is_some_and(|kind| PLAIN.contains(&kind));
            if let (Some(name), true) = (tree.attr(property, None, "name"), plain) {
                out.entry(name.to_lowercase())
                    .or_insert_with(|| tree.text_content(property));
            }
        }
    }
    out
}

fn related<'t>(txn: &'t Txn<'_>, rel: &str) -> Option<&'t crate::xml::XmlTree> {
    let part = txn.part_related_by("/", rel).ok().flatten()?;
    txn.tree(&part).ok()
}

/// A live field's result in its paragraph, and the pen that writes it.
#[derive(Debug, Clone)]
pub(crate) struct FieldSpan {
    pub kind: FieldKind,
    pub case: Case,
    pub pieces: Range<u32>,
    pub pen: super::model::FieldPen,
}

impl FieldSpan {
    /// Every character the value can show, as its case writes it; a page
    /// number in its section's format may show any of `sections`'. A
    /// property's value is shown already, so it has none here.
    #[cfg(feature = "pdf")]
    pub(crate) fn value_chars(&self, sections: &[NumberFormat]) -> Vec<char> {
        let format = match &self.kind {
            FieldKind::Page(format)
            | FieldKind::NumPages(format)
            | FieldKind::SectionPages(format)
            | FieldKind::PageRef { format, .. } => *format,
            FieldKind::Text(_) => return Vec::new(),
        };
        let formats: Vec<NumberFormat> = if format == NumberFormat::Section {
            sections.to_vec()
        } else {
            vec![format]
        };
        // Arabic digits for every format: each writes some numbers in arabic.
        let mut chars: Vec<char> = ('0'..='9').collect();
        for format in formats {
            match format {
                NumberFormat::LowerRoman => chars.extend("ivxlcdm".chars()),
                NumberFormat::UpperRoman => chars.extend("IVXLCDM".chars()),
                NumberFormat::LowerLetter => chars.extend('a'..='z'),
                NumberFormat::UpperLetter => chars.extend('A'..='Z'),
                NumberFormat::NumberInDash => chars.extend(['-', ' ']),
                _ => {}
            }
        }
        let mut chars: Vec<char> = match self.case {
            Case::AsIs => chars,
            Case::Upper => chars.iter().flat_map(|c| c.to_uppercase()).collect(),
            Case::Lower => chars.iter().flat_map(|c| c.to_lowercase()).collect(),
            Case::Capital => chars
                .iter()
                .flat_map(|c| std::iter::once(*c).chain(c.to_uppercase()))
                .collect(),
        };
        chars.sort_unstable();
        chars.dedup();
        chars
    }
}

/// The page a value is computed for: its shown number, its section's
/// format and its section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Number {
    pub shown: i64,
    pub format: NumberFormat,
    pub section: usize,
}

impl Number {
    pub(crate) fn write(self, format: NumberFormat) -> String {
        format.or(self.format).write(self.shown)
    }
}

/// What the previous pass found; empty before the first.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Values {
    pub pages: Option<u32>,
    pub section_pages: Vec<u32>,
    pub bookmarks: HashMap<String, Number>,
    /// The pages of each body paragraph holding a page field: each line's
    /// first piece and its page, in order.
    pub paragraphs: HashMap<NodeId, Vec<(u32, Number)>>,
}

/// The page of each line of the body's paragraphs: the line's first piece
/// and its physical page, in order.
pub(crate) type LinePages = HashMap<NodeId, Vec<(u32, u32)>>;

/// The entry of the line holding piece `at`: the last starting at or
/// before it, else the first.
pub(crate) fn line_at<T: Copy>(lines: &[(u32, T)], at: u32) -> Option<T> {
    let holding = lines.partition_point(|(start, _)| *start <= at);
    lines.get(holding.saturating_sub(1)).map(|(_, page)| *page)
}

impl Values {
    /// What a pass gave: its page count and numbers, each bookmark's page
    /// and the page of each line of the body paragraphs in `page_fields`.
    pub(crate) fn of(
        pages: &Pages,
        bookmarks: &HashMap<String, u32>,
        placed: &[Placed],
        lines: &LinePages,
        page_fields: &HashSet<NodeId>,
    ) -> Values {
        let number = |page: u32| pages.numbers.get(page as usize - 1).copied();
        let mut section_pages = Vec::new();
        let counted = (1..).zip(&pages.numbers);
        for (_, page) in counted.filter(|(n, _)| !pages.blank.contains(n)) {
            if section_pages.len() <= page.section {
                section_pages.resize(page.section + 1, 0);
            }
            section_pages[page.section] += 1;
        }
        let mut paragraphs = HashMap::new();
        for entry in placed {
            if let Some(node) = entry.node.filter(|n| page_fields.contains(n))
                && let Some(first) = number(entry.page)
            {
                paragraphs.entry(node).or_insert_with(|| {
                    let by_line = lines.get(&node).into_iter().flatten();
                    let by_line: Vec<(u32, Number)> = by_line
                        .filter_map(|(start, page)| Some((*start, number(*page)?)))
                        .collect();
                    if by_line.is_empty() {
                        vec![(0, first)]
                    } else {
                        by_line
                    }
                });
            }
        }
        Values {
            pages: Some(pages.count),
            section_pages,
            bookmarks: bookmarks
                .iter()
                .filter_map(|(name, page)| Some((name.clone(), number(*page)?)))
                .collect(),
            paragraphs,
        }
    }
}

/// Whether one pass's values can change the next: any computed field but
/// a document property, and a page number outside the body (its page is
/// known as it is laid out).
pub(crate) fn feeds_back(model: &Model) -> bool {
    let feeds = |blocks: &[Block], body: bool| {
        spans(blocks).any(|span| match span.kind {
            FieldKind::Text(_) => false,
            FieldKind::Page(_) => body,
            _ => true,
        })
    };
    feeds(&model.blocks, true) || model.stories.iter().any(|s| feeds(&s.blocks, false))
}

/// The body paragraphs holding a page number field.
pub(crate) fn page_field_nodes(blocks: &[Block]) -> HashSet<NodeId> {
    let mut out = HashSet::new();
    each_para(blocks, &mut |para| {
        if para
            .fields
            .iter()
            .any(|f| matches!(f.kind, FieldKind::Page(_) | FieldKind::SectionPages(_)))
            && let Some(node) = para.node
        {
            out.insert(node);
        }
    });
    out
}

fn spans(blocks: &[Block]) -> impl Iterator<Item = FieldSpan> {
    let mut out = Vec::new();
    each_para(blocks, &mut |para| out.extend(para.fields.iter().cloned()));
    out.into_iter()
}

fn each_para(blocks: &[Block], f: &mut dyn FnMut(&Para)) {
    for block in blocks {
        match block {
            Block::Para(para) => f(para),
            Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        each_para(&cell.blocks, f);
                    }
                }
            }
        }
    }
}

/// Values for one story: `page` for a header or footer, else each body
/// paragraph's own from the previous pass.
pub(crate) struct Context<'v> {
    pub values: &'v Values,
    pub page: Option<Number>,
}

impl Context<'_> {
    /// The value of a field at piece `at` of its paragraph `node`, as the
    /// previous pass laid it out; `None` keeps the stored result.
    fn value(&self, kind: &FieldKind, node: Option<NodeId>, at: u32) -> Option<String> {
        let values = self.values;
        let page = || {
            self.page.or_else(|| {
                let lines = values.paragraphs.get(&node?)?;
                line_at(lines, at)
            })
        };
        match kind {
            FieldKind::Page(format) => page().map(|p| p.write(*format)),
            FieldKind::NumPages(format) => values
                .pages
                .map(|n| format.or(NumberFormat::Arabic).write(n.into())),
            FieldKind::SectionPages(format) => page()
                .and_then(|p| values.section_pages.get(p.section))
                .map(|n| format.or(NumberFormat::Arabic).write((*n).into())),
            FieldKind::PageRef { bookmark, format } => {
                values.bookmarks.get(bookmark).map(|p| p.write(*format))
            }
            FieldKind::Text(text) => Some(text.clone()),
        }
    }
}

/// Write each live field's value in place of its result.
pub(crate) fn show(blocks: &mut [Block], context: &Context<'_>) {
    for block in blocks {
        match block {
            Block::Para(para) => show_para(para, context),
            Block::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        show(&mut cell.blocks, context);
                    }
                }
            }
        }
    }
}

fn show_para(para: &mut Para, context: &Context<'_>) {
    let mut shift: i64 = 0;
    let mut done = 0;
    for span in &mut para.fields {
        let (start, end) = (
            i64::from(span.pieces.start) + shift,
            i64::from(span.pieces.end) + shift,
        );
        // Spans are in order and apart; one that is not keeps its result.
        if start < done || end < start || end > para.pieces.len() as i64 {
            continue;
        }
        // Where the previous pass laid the field out.
        let laid = span.pieces.start;
        let (start, end) = (start as u32, end as u32);
        span.pieces = start..end;
        done = i64::from(end);
        let Some(text) = context.value(&span.kind, para.node, laid) else {
            continue;
        };
        let text = span.case.apply(text);
        let pieces: Vec<Piece> = text.chars().map(|ch| span.pen.piece(ch)).collect();
        let added = pieces.len() as u32;
        para.pieces.splice(start as usize..end as usize, pieces);
        let moved = i64::from(added) - i64::from(end - start);
        for equation in para
            .equations
            .iter_mut()
            .filter(|e| e.pieces.start >= end as usize)
        {
            let by = |at: usize| (at as i64 + moved) as usize;
            equation.pieces = by(equation.pieces.start)..by(equation.pieces.end);
        }
        for bookmark in para.bookmarks.iter_mut().filter(|b| b.piece >= end) {
            bookmark.piece = (i64::from(bookmark.piece) + moved) as u32;
        }
        shift += moved;
        span.pieces = start..start + added;
        done = i64::from(start + added);
    }
}

/// Whether any of `blocks` has a field whose value depends on the page.
pub(crate) fn per_page(blocks: &[Block]) -> bool {
    spans(blocks).any(|span| !matches!(span.kind, FieldKind::Text(_)))
}

/// A warning layout gives, what it is about, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutWarning {
    pub code: &'static str,
    pub subject: &'static str,
    pub story: StoryId,
    pub page: Option<u32>,
}

/// Whether a page field of `blocks` writes its number in its section's
/// format.
pub(crate) fn takes_section_format(blocks: &[Block]) -> bool {
    spans(blocks).any(|span| {
        matches!(
            span.kind,
            FieldKind::Page(NumberFormat::Section)
                | FieldKind::PageRef {
                    format: NumberFormat::Section,
                    ..
                }
        )
    })
}

/// Whether `blocks` have a page number or page reference field, in any
/// format.
pub(crate) fn takes_page_number(blocks: &[Block]) -> bool {
    spans(blocks).any(|span| matches!(span.kind, FieldKind::Page(_) | FieldKind::PageRef { .. }))
}

/// A page reference whose bookmark `values` do not place keeps its saved
/// result: one warning per story of `stories` holding one.
pub(crate) fn unplaced_references<'b>(
    stories: impl IntoIterator<Item = (StoryId, &'b [Block])>,
    values: &Values,
) -> Vec<LayoutWarning> {
    stories
        .into_iter()
        .filter(|(_, blocks)| {
            spans(blocks).any(|span| match &span.kind {
                FieldKind::PageRef { bookmark, .. } => !values.bookmarks.contains_key(bookmark),
                _ => false,
            })
        })
        .map(|(story, _)| LayoutWarning {
            code: FIELD_CACHED,
            subject: "PAGEREF",
            story,
            page: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(code: &str) -> Instruction {
        let properties = Properties::from([
            ("title".to_string(), "Annual report".to_string()),
            ("client".to_string(), "Acme".to_string()),
        ]);
        instruction(code, &properties)
    }

    #[test]
    fn page_fields_take_their_format_switch() {
        assert_eq!(
            kind(" PAGE \\* roman \\* MERGEFORMAT ").kind,
            Some(FieldKind::Page(NumberFormat::LowerRoman))
        );
        assert_eq!(
            kind("NUMPAGES \\* ALPHABETIC").kind,
            Some(FieldKind::NumPages(NumberFormat::UpperLetter))
        );
        assert_eq!(
            kind("page").kind,
            Some(FieldKind::Page(NumberFormat::Section))
        );
        assert_eq!(
            kind("SECTIONPAGES \\* Arabic").kind,
            Some(FieldKind::SectionPages(NumberFormat::Arabic))
        );
    }

    #[test]
    fn numbers_are_written_in_arabic_roman_and_letters() {
        let write = |format: NumberFormat, n| format.write(n);
        assert_eq!(write(NumberFormat::Arabic, 12), "12");
        assert_eq!(write(NumberFormat::LowerRoman, 1994), "mcmxciv");
        assert_eq!(write(NumberFormat::UpperRoman, 9), "IX");
        assert_eq!(write(NumberFormat::UpperRoman, 4000), "4000");
        assert_eq!(write(NumberFormat::LowerLetter, 1), "a");
        assert_eq!(write(NumberFormat::LowerLetter, 28), "bb");
        assert_eq!(write(NumberFormat::UpperLetter, 26), "Z");
        assert_eq!(write(NumberFormat::LowerLetter, 0), "0");
    }

    #[test]
    fn page_references_warn_of_the_position_switch() {
        let read = kind(" PAGEREF _Toc1 \\h \\p ");
        assert_eq!(
            read.kind,
            Some(FieldKind::PageRef {
                bookmark: "_Toc1".to_string(),
                format: NumberFormat::Section
            })
        );
        assert_eq!(read.warnings, [(FIELD_SWITCH_IGNORED, "page reference")]);
        assert!(kind(" PAGEREF _Toc1 \\h ").warnings.is_empty());
    }

    #[test]
    fn a_switch_written_against_a_word_is_read() {
        assert_eq!(
            kind("PAGE\\* roman").kind,
            Some(FieldKind::Page(NumberFormat::LowerRoman))
        );
        assert_eq!(
            kind("PAGE \\*ROMAN").kind,
            Some(FieldKind::Page(NumberFormat::UpperRoman))
        );
    }

    #[test]
    fn a_case_switch_applies_to_the_written_number() {
        let read = kind(" PAGE \\* roman \\* Upper ");
        assert_eq!(read.kind, Some(FieldKind::Page(NumberFormat::LowerRoman)));
        assert_eq!(read.case.apply(NumberFormat::LowerRoman.write(14)), "XIV");
        assert_eq!(kind("PAGE \\* FirstCap").case.apply("xiv".into()), "Xiv");
        assert_eq!(kind("PAGE \\* Lower").case.apply("XIV".into()), "xiv");
        assert!(kind("PAGE \\* Upper").warnings.is_empty());
    }

    #[test]
    fn section_formats_in_dashes_and_two_digits_are_written() {
        let of = |fmt| NumberFormat::of_section(Some(fmt));
        assert_eq!(of("numberInDash").unwrap().write(3), "- 3 -");
        assert_eq!(of("decimalZero").unwrap().write(3), "03");
        assert_eq!(of("decimalZero").unwrap().write(12), "12");
        assert_eq!(of("decimal"), Some(NumberFormat::Arabic));
        assert_eq!(NumberFormat::of_section(None), Some(NumberFormat::Arabic));
        for fmt in ["cardinalText", "ordinal", "ordinalText", "hex", "chicago"] {
            assert_eq!(of(fmt), None, "{fmt}");
        }
    }

    /// The preflight and layout read a field alike: each probe of the
    /// PR 73 review.
    #[test]
    fn a_pdf_shows_a_field_computed_only_where_layout_computes_it() {
        for (code, want) in [
            (" PAGE ", Shown::Computed),
            (" PAGE\\* roman ", Shown::Computed),
            (" PAGE \\* roman \\* Upper ", Shown::Computed),
            (" NUMPAGES ", Shown::Computed),
            (" SECTIONPAGES \\* Arabic ", Shown::Computed),
            (" PAGEREF far \\h ", Shown::Computed),
            (" PAGE \\* Ordinal ", Shown::Computed),
            (" PAGE extra ", Shown::Cached("PAGE")),
            (" PAGE \" ", Shown::Cached("PAGE")),
            (" PAGEREF ", Shown::Cached("PAGEREF")),
            (" PAGEREF \\h ", Shown::Cached("PAGEREF")),
            (" NUMPAGES 3 ", Shown::Cached("NUMPAGES")),
            (" DATE \\@ \"d MMMM\" ", Shown::Cached("DATE")),
            (" TIME ", Shown::Cached("TIME")),
            (" PRINTDATE ", Shown::Cached("PRINTDATE")),
            (" REF _Ref1 \\h ", Shown::Other),
            (" TITLE ", Shown::Other),
            (" SEQ Figure ", Shown::Other),
        ] {
            assert_eq!(shown(code), want, "{code}");
        }
    }

    #[test]
    fn dates_keep_their_stored_value_and_warn() {
        for (code, subject) in [
            (" DATE \\@ \"d MMMM yyyy\" ", "DATE"),
            ("TIME", "TIME"),
            ("PRINTDATE", "PRINTDATE"),
        ] {
            assert_eq!(
                kind(code),
                Instruction {
                    warnings: vec![(FIELD_CACHED, subject)],
                    ..Instruction::default()
                }
            );
        }
    }

    #[test]
    fn properties_are_read_and_nothing_else_is_computed() {
        assert_eq!(
            kind(" TITLE ").kind,
            Some(FieldKind::Text("Annual report".to_string()))
        );
        assert_eq!(
            kind(" DOCPROPERTY Client ").kind,
            Some(FieldKind::Text("Acme".to_string()))
        );
        for code in [
            " TITLE \"New title\" ",
            " DOCPROPERTY Missing ",
            " AUTHOR ",
            " LINK Excel.Sheet.12 \"https://example.com/a.xlsx\" ",
            " INCLUDETEXT \"https://example.com/a.docx\" ",
            " HYPERLINK \"https://example.com\" ",
            " REF _Ref1 \\h ",
        ] {
            assert_eq!(kind(code), Instruction::default(), "{code}");
        }
    }
}
