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
}

impl NumberFormat {
    /// `w:pgNumType w:fmt`; formats layout does not write are arabic.
    pub(crate) fn of_section(fmt: Option<&str>) -> NumberFormat {
        match fmt {
            Some("lowerRoman") => NumberFormat::LowerRoman,
            Some("upperRoman") => NumberFormat::UpperRoman,
            Some("lowerLetter") => NumberFormat::LowerLetter,
            Some("upperLetter") => NumberFormat::UpperLetter,
            _ => NumberFormat::Arabic,
        }
    }

    fn of_switch(value: &str) -> Option<NumberFormat> {
        Some(match value {
            "roman" => NumberFormat::LowerRoman,
            "ROMAN" => NumberFormat::UpperRoman,
            "alphabetic" => NumberFormat::LowerLetter,
            "ALPHABETIC" => NumberFormat::UpperLetter,
            _ if value.eq_ignore_ascii_case("arabic") => NumberFormat::Arabic,
            _ => return None,
        })
    }

    fn or(self, fallback: NumberFormat) -> NumberFormat {
        if self == NumberFormat::Section {
            fallback
        } else {
            self
        }
    }

    /// Roman numerals past 3999 and letters or numerals for 0 and below are
    /// written in arabic.
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
            NumberFormat::LowerLetter | NumberFormat::UpperLetter if n >= 1 => {
                let base = if self == NumberFormat::UpperLetter {
                    b'A'
                } else {
                    b'a'
                };
                // Word repeats the letter: y, z, aa, bb.
                let letter = char::from(base + ((n - 1) % 26) as u8);
                std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect()
            }
            _ => n.to_string(),
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

/// A field instruction read: its kind when layout computes it, and the
/// warning it gives.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Instruction {
    pub kind: Option<FieldKind>,
    pub warning: Option<&'static str>,
}

/// Reads `instruction` (a `w:instrText` run's text or `w:fldSimple`'s
/// `w:instr`).
pub(crate) fn instruction(code: &str, properties: &Properties) -> Instruction {
    let Ok(tokens) = fieldcode::tokens(code) else {
        return Instruction::default();
    };
    let Some((command, rest)) = tokens.split_first().filter(|(c, _)| !c.quoted) else {
        return Instruction::default();
    };
    let command = command.value.to_ascii_uppercase();
    let mut format = NumberFormat::Section;
    let mut operands = Vec::new();
    let mut warning = None;
    let mut rest = rest.iter();
    while let Some(token) = rest.next() {
        if token.quoted || !token.value.starts_with('\\') {
            operands.push(token.value.as_str());
            continue;
        }
        match token.value.as_str() {
            "\\*" => {
                if let Some(f) = rest.next().and_then(|t| NumberFormat::of_switch(&t.value)) {
                    format = f;
                }
            }
            "\\@" | "\\#" => {
                rest.next();
            }
            s if s.eq_ignore_ascii_case("\\p") => warning = Some(FIELD_SWITCH_IGNORED),
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
                kind: None,
                warning: Some(FIELD_CACHED),
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
    Instruction {
        kind: Some(kind),
        warning,
    }
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
        for property in tree.child_elements(tree.root()) {
            if let Some(name) = tree.attr(property, None, "name") {
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
    pub pieces: Range<u32>,
    pub pen: super::model::FieldPen,
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
    fn write(self, format: NumberFormat) -> String {
        format.or(self.format).write(self.shown)
    }
}

/// What the previous pass found; empty before the first.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Values {
    pub pages: Option<u32>,
    pub section_pages: Vec<u32>,
    pub bookmarks: HashMap<String, Number>,
    /// The page of each body paragraph holding a page field.
    pub paragraphs: HashMap<NodeId, Number>,
}

impl Values {
    /// What a pass gave: its page count and numbers, each bookmark's page
    /// and the page of each body paragraph in `page_fields`.
    pub(crate) fn of(
        pages: &Pages,
        bookmarks: &HashMap<String, u32>,
        placed: &[Placed],
        page_fields: &HashSet<NodeId>,
    ) -> Values {
        let number = |page: u32| pages.numbers.get(page as usize - 1).copied();
        let mut section_pages = Vec::new();
        for page in &pages.numbers {
            if section_pages.len() <= page.section {
                section_pages.resize(page.section + 1, 0);
            }
            section_pages[page.section] += 1;
        }
        let mut paragraphs = HashMap::new();
        for entry in placed {
            if let Some(node) = entry.node.filter(|n| page_fields.contains(n))
                && let Some(number) = number(entry.page)
            {
                paragraphs.entry(node).or_insert(number);
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
    /// `None` keeps the stored result.
    fn value(&self, kind: &FieldKind, node: Option<NodeId>) -> Option<String> {
        let values = self.values;
        let page = || {
            self.page
                .or_else(|| node.and_then(|n| values.paragraphs.get(&n).copied()))
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
    for span in &mut para.fields {
        let start = (i64::from(span.pieces.start) + shift) as u32;
        let end = (i64::from(span.pieces.end) + shift) as u32;
        span.pieces = start..end;
        let Some(text) = context.value(&span.kind, para.node) else {
            continue;
        };
        let pen = &span.pen;
        let pieces: Vec<Piece> = text
            .chars()
            .map(|ch| Piece::Char {
                ch,
                width: pen.measurer.width(ch),
                metric: pen.measurer.metric,
                look: pen.look,
            })
            .collect();
        let added = pieces.len() as u32;
        para.pieces.splice(start as usize..end as usize, pieces);
        shift += i64::from(added) - i64::from(end - start);
        span.pieces = start..start + added;
    }
}

/// Whether any of `blocks` has a field layout computes.
pub(crate) fn any(blocks: &[Block]) -> bool {
    spans(blocks).next().is_some()
}

/// A warning layout gives, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutWarning {
    pub code: &'static str,
    pub story: StoryId,
    pub page: Option<u32>,
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
        assert_eq!(read.warning, Some(FIELD_SWITCH_IGNORED));
        assert_eq!(kind(" PAGEREF _Toc1 \\h ").warning, None);
    }

    #[test]
    fn dates_keep_their_stored_value_and_warn() {
        for code in [" DATE \\@ \"d MMMM yyyy\" ", "TIME", "PRINTDATE"] {
            assert_eq!(
                kind(code),
                Instruction {
                    kind: None,
                    warning: Some(FIELD_CACHED)
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
