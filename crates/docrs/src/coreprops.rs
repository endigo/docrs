//! Core document properties (`docProps/core.xml`; python-docx
//! `opc/coreprops.py`, `oxml/coreprops.py`, `opc/parts/coreprops.py` and
//! `OpcPackage._core_properties_part`; MIT, see `NOTICE.md`).
//!
//! [`Document::core_properties`] reads all fifteen Dublin Core properties
//! with upstream's semantics: text properties are the element's leading
//! text (lxml `.text`) or `""`, dates are parsed as W3CDTF (invalid strings
//! read as `None`), `revision` is a non-negative integer (anything else
//! reads as `0`). Reading never creates a part: for a package without core
//! properties it reports the values upstream's default part would hold
//! (`title` "Word Document", `last_modified_by` "python-docx", revision
//! one) with `modified` `None` and `part` `None` (upstream creates the part on
//! first access and stamps `modified` with the current time).
//!
//! [`Document::set_core_properties`] applies assignments atomically. When
//! the package has no core-properties part it first creates
//! `docProps/core.xml` exactly like `CorePropertiesPart.default` (with the
//! caller's `now`, or the current time, as `modified`) and relates it from
//! the package root.

use crate::clock::Timestamp;
use crate::package::content_types::CT_NS;
use crate::package::ct;
use crate::package::rels::rt;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::{RelTarget, Txn};
use crate::wml::{leading_text, set_leading_text};
use crate::xml::{self, NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// `cp:` (OPC core properties).
pub const CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
/// `dc:` (Dublin Core elements).
pub const DC: &str = "http://purl.org/dc/elements/1.1/";
/// `dcterms:` (Dublin Core terms).
pub const DCTERMS: &str = "http://purl.org/dc/terms/";
/// `xsi:`.
pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

const PART_NAME: &str = "docProps/core.xml";

/// One of the fifteen core properties (upstream `CoreProperties` names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreProperty {
    /// `author` (`dc:creator`).
    Author,
    /// `category` (`cp:category`).
    Category,
    /// `comments` (`dc:description`).
    Comments,
    /// `content_status` (`cp:contentStatus`).
    ContentStatus,
    /// `created` (`dcterms:created`).
    Created,
    /// `identifier` (`dc:identifier`).
    Identifier,
    /// `keywords` (`cp:keywords`).
    Keywords,
    /// `language` (`dc:language`).
    Language,
    /// `last_modified_by` (`cp:lastModifiedBy`).
    LastModifiedBy,
    /// `last_printed` (`cp:lastPrinted`).
    LastPrinted,
    /// `modified` (`dcterms:modified`).
    Modified,
    /// `revision` (`cp:revision`).
    Revision,
    /// `subject` (`dc:subject`).
    Subject,
    /// `title` (`dc:title`).
    Title,
    /// `version` (`cp:version`).
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Date,
    Revision,
}

impl CoreProperty {
    /// Every property in upstream attribute order.
    pub const ALL: [CoreProperty; 15] = [
        CoreProperty::Author,
        CoreProperty::Category,
        CoreProperty::Comments,
        CoreProperty::ContentStatus,
        CoreProperty::Created,
        CoreProperty::Identifier,
        CoreProperty::Keywords,
        CoreProperty::Language,
        CoreProperty::LastModifiedBy,
        CoreProperty::LastPrinted,
        CoreProperty::Modified,
        CoreProperty::Revision,
        CoreProperty::Subject,
        CoreProperty::Title,
        CoreProperty::Version,
    ];

    /// Upstream property name (`last_modified_by`, …).
    pub fn name(self) -> &'static str {
        match self {
            CoreProperty::Author => "author",
            CoreProperty::Category => "category",
            CoreProperty::Comments => "comments",
            CoreProperty::ContentStatus => "content_status",
            CoreProperty::Created => "created",
            CoreProperty::Identifier => "identifier",
            CoreProperty::Keywords => "keywords",
            CoreProperty::Language => "language",
            CoreProperty::LastModifiedBy => "last_modified_by",
            CoreProperty::LastPrinted => "last_printed",
            CoreProperty::Modified => "modified",
            CoreProperty::Revision => "revision",
            CoreProperty::Subject => "subject",
            CoreProperty::Title => "title",
            CoreProperty::Version => "version",
        }
    }

    /// From an upstream property name.
    pub fn from_name(name: &str) -> Result<CoreProperty> {
        CoreProperty::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| {
                Error::invalid_argument(
                    "unknown_core_property",
                    format!("CoreProperties has no property {name:?}"),
                )
            })
    }

    /// `(namespace, local name)` of the property element.
    pub fn element(self) -> (&'static str, &'static str) {
        match self {
            CoreProperty::Author => (DC, "creator"),
            CoreProperty::Category => (CP, "category"),
            CoreProperty::Comments => (DC, "description"),
            CoreProperty::ContentStatus => (CP, "contentStatus"),
            CoreProperty::Created => (DCTERMS, "created"),
            CoreProperty::Identifier => (DC, "identifier"),
            CoreProperty::Keywords => (CP, "keywords"),
            CoreProperty::Language => (DC, "language"),
            CoreProperty::LastModifiedBy => (CP, "lastModifiedBy"),
            CoreProperty::LastPrinted => (CP, "lastPrinted"),
            CoreProperty::Modified => (DCTERMS, "modified"),
            CoreProperty::Revision => (CP, "revision"),
            CoreProperty::Subject => (DC, "subject"),
            CoreProperty::Title => (DC, "title"),
            CoreProperty::Version => (CP, "version"),
        }
    }

    fn kind(self) -> Kind {
        match self {
            CoreProperty::Created | CoreProperty::LastPrinted | CoreProperty::Modified => {
                Kind::Date
            }
            CoreProperty::Revision => Kind::Revision,
            _ => Kind::Text,
        }
    }
}

/// A value assigned to a core property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreValue {
    /// A string (text properties).
    Text(String),
    /// An instant (date properties; written as `YYYY-MM-DDTHH:MM:SSZ`).
    Date(Timestamp),
    /// An integer (`revision`; text properties store `str(value)`).
    Int(i64),
}

/// One property assignment for [`Document::set_core_properties`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorePropertyChange {
    /// The property.
    pub property: CoreProperty,
    /// Its new value.
    pub value: CoreValue,
}

impl CorePropertyChange {
    /// `property = value`.
    pub fn new(property: CoreProperty, value: CoreValue) -> Self {
        CorePropertyChange { property, value }
    }
}

/// Snapshot of the core properties (upstream `CoreProperties` getters).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoreProperties {
    /// Package member read; `None` when the package has no core-properties
    /// part (the values are then upstream's defaults, see the module docs).
    pub part: Option<String>,
    /// `author`.
    pub author: String,
    /// `category`.
    pub category: String,
    /// `comments`.
    pub comments: String,
    /// `content_status`.
    pub content_status: String,
    /// `created`.
    #[serde(serialize_with = "ser_date")]
    pub created: Option<Timestamp>,
    /// `identifier`.
    pub identifier: String,
    /// `keywords`.
    pub keywords: String,
    /// `language`.
    pub language: String,
    /// `last_modified_by`.
    pub last_modified_by: String,
    /// `last_printed`.
    #[serde(serialize_with = "ser_date")]
    pub last_printed: Option<Timestamp>,
    /// `modified`.
    #[serde(serialize_with = "ser_date")]
    pub modified: Option<Timestamp>,
    /// `revision`.
    pub revision: i64,
    /// `subject`.
    pub subject: String,
    /// `title`.
    pub title: String,
    /// `version`.
    pub version: String,
}

fn ser_date<S: serde::Serializer>(
    value: &Option<Timestamp>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match value {
        Some(ts) => serializer.serialize_str(&ts.to_xml()),
        None => serializer.serialize_none(),
    }
}

impl CoreProperties {
    /// The text value of a text property (`""` for date/revision properties).
    pub fn text(&self, property: CoreProperty) -> &str {
        match property {
            CoreProperty::Author => &self.author,
            CoreProperty::Category => &self.category,
            CoreProperty::Comments => &self.comments,
            CoreProperty::ContentStatus => &self.content_status,
            CoreProperty::Identifier => &self.identifier,
            CoreProperty::Keywords => &self.keywords,
            CoreProperty::Language => &self.language,
            CoreProperty::LastModifiedBy => &self.last_modified_by,
            CoreProperty::Subject => &self.subject,
            CoreProperty::Title => &self.title,
            CoreProperty::Version => &self.version,
            _ => "",
        }
    }

    /// The value of a date property (`None` for other properties).
    pub fn date(&self, property: CoreProperty) -> Option<Timestamp> {
        match property {
            CoreProperty::Created => self.created,
            CoreProperty::LastPrinted => self.last_printed,
            CoreProperty::Modified => self.modified,
            _ => None,
        }
    }
}

fn child(tree: &XmlTree, property: CoreProperty) -> Option<NodeId> {
    let (ns, local) = property.element();
    tree.first_child_named(tree.root(), ns, local)
}

fn text_of(tree: &XmlTree, property: CoreProperty) -> String {
    child(tree, property)
        .map(|element| leading_text(tree, element))
        .unwrap_or_default()
}

fn date_of(tree: &XmlTree, property: CoreProperty) -> Option<Timestamp> {
    let element = child(tree, property)?;
    parse_w3cdtf(&leading_text(tree, element))
}

fn revision_of(tree: &XmlTree) -> i64 {
    child(tree, CoreProperty::Revision)
        .and_then(|element| crate::styles::py_int(&leading_text(tree, element)))
        .filter(|value| *value >= 0)
        .unwrap_or(0)
}

/// Snapshot of a `cp:coreProperties` tree.
pub(crate) fn read_core_properties(tree: &XmlTree, part: Option<String>) -> CoreProperties {
    CoreProperties {
        part,
        author: text_of(tree, CoreProperty::Author),
        category: text_of(tree, CoreProperty::Category),
        comments: text_of(tree, CoreProperty::Comments),
        content_status: text_of(tree, CoreProperty::ContentStatus),
        created: date_of(tree, CoreProperty::Created),
        identifier: text_of(tree, CoreProperty::Identifier),
        keywords: text_of(tree, CoreProperty::Keywords),
        language: text_of(tree, CoreProperty::Language),
        last_modified_by: text_of(tree, CoreProperty::LastModifiedBy),
        last_printed: date_of(tree, CoreProperty::LastPrinted),
        modified: date_of(tree, CoreProperty::Modified),
        revision: revision_of(tree),
        subject: text_of(tree, CoreProperty::Subject),
        title: text_of(tree, CoreProperty::Title),
        version: text_of(tree, CoreProperty::Version),
    }
}

/// A field of a `strptime` template: alternatives tried in order, each a
/// sequence of character classes (CPython `_strptime` regexes).
type Alternatives = &'static [&'static [&'static str]];

const DIGITS: &str = "0123456789";
const YEAR: Alternatives = &[&[DIGITS, DIGITS, DIGITS, DIGITS]];
const MONTH: Alternatives = &[&["1", "012"], &["0", "123456789"], &["123456789"]];
const DAY: Alternatives = &[
    &["3", "01"],
    &["12", DIGITS],
    &["0", "123456789"],
    &["123456789"],
    &[" ", "123456789"],
];
const HOUR: Alternatives = &[&["2", "0123"], &["01", DIGITS], &[DIGITS]];
const MINUTE: Alternatives = &[&["012345", DIGITS], &[DIGITS]];
const SECOND: Alternatives = &[&["6", "01"], &["012345", DIGITS], &[DIGITS]];

enum Token {
    Field(Alternatives),
    Literal(char),
}

/// `re.match` of `tokens` at the start of `text` (backtracking into
/// earlier alternatives only when a later token fails; no end anchor).
/// Records the field values and the consumed length.
fn match_tokens(tokens: &[Token], text: &[char], out: &mut Vec<i64>, end: &mut usize) -> bool {
    let Some((first, rest)) = tokens.split_first() else {
        *end = text.len();
        return true;
    };
    match first {
        Token::Literal(lit) => {
            // CPython compiles the format with re.IGNORECASE.
            match text.first() {
                Some(c) if c.to_lowercase().eq(lit.to_lowercase()) => {
                    match_tokens(rest, &text[1..], out, end)
                }
                _ => false,
            }
        }
        Token::Field(alternatives) => {
            for alternative in alternatives.iter() {
                if text.len() < alternative.len() {
                    continue;
                }
                let matched = alternative
                    .iter()
                    .zip(text.iter())
                    .all(|(class, c)| class.contains(*c));
                if !matched {
                    continue;
                }
                let digits: String = text[..alternative.len()]
                    .iter()
                    .filter(|c| c.is_ascii_digit())
                    .collect();
                let value: i64 = digits.parse().unwrap_or(0);
                out.push(value);
                if match_tokens(rest, &text[alternative.len()..], out, end) {
                    return true;
                }
                out.pop();
            }
            false
        }
    }
}

/// `datetime.strptime(text, template)` for the four W3CDTF templates;
/// `None` on `ValueError`.
fn strptime(text: &[char], template: &str) -> Option<Timestamp> {
    let tokens: Vec<Token> = match template {
        "datetime" => vec![
            Token::Field(YEAR),
            Token::Literal('-'),
            Token::Field(MONTH),
            Token::Literal('-'),
            Token::Field(DAY),
            Token::Literal('T'),
            Token::Field(HOUR),
            Token::Literal(':'),
            Token::Field(MINUTE),
            Token::Literal(':'),
            Token::Field(SECOND),
        ],
        "date" => vec![
            Token::Field(YEAR),
            Token::Literal('-'),
            Token::Field(MONTH),
            Token::Literal('-'),
            Token::Field(DAY),
        ],
        "month" => vec![Token::Field(YEAR), Token::Literal('-'), Token::Field(MONTH)],
        _ => vec![Token::Field(YEAR)],
    };
    let mut fields = Vec::new();
    let mut remaining = 0;
    // "unconverted data remains" is a ValueError.
    if !match_tokens(&tokens, text, &mut fields, &mut remaining) || remaining != 0 {
        return None;
    }
    let get = |i: usize, default: i64| fields.get(i).copied().unwrap_or(default);
    let (year, month, day) = (get(0, 1900), get(1, 1), get(2, 1));
    let (hour, minute, second) = (get(3, 0), get(4, 0), get(5, 0));
    // `datetime(...)` rejects year 0 and leap seconds (60/61).
    if year < 1 || second > 59 {
        return None;
    }
    Timestamp::from_utc(
        year,
        month as u32,
        day as u32,
        hour as u32,
        minute as u32,
        second as u32,
    )
    .ok()
}

/// Upstream `CT_CoreProperties._parse_W3CDTF_to_datetime` (`None` where
/// upstream raises `ValueError`, which `_datetime_of_element` swallows).
pub fn parse_w3cdtf(text: &str) -> Option<Timestamp> {
    let chars: Vec<char> = text.chars().collect();
    let split = chars.len().min(19);
    let (parseable, offset) = chars.split_at(split);
    let mut parsed = None;
    // Every template is tried; a later success wins (no `break` upstream).
    for template in ["datetime", "date", "month", "year"] {
        if let Some(ts) = strptime(parseable, template) {
            parsed = Some(ts);
        }
    }
    let parsed = parsed?;
    if offset.len() != 6 {
        return Some(parsed);
    }
    // `([+-])(\d\d):(\d\d)` matched at the start; the sign is inverted.
    let digit = |i: usize| offset[i].to_digit(10).map(i64::from);
    if !matches!(offset[0], '+' | '-') || offset[3] != ':' {
        return None;
    }
    let hours = digit(1)? * 10 + digit(2)?;
    let minutes = digit(4)? * 10 + digit(5)?;
    let sign = if offset[0] == '+' { -1 } else { 1 };
    let shifted = parsed.unix() + sign * (hours * 3600 + minutes * 60);
    // `datetime + timedelta` overflows outside years 1..=9999.
    let min = Timestamp::from_utc(1, 1, 1, 0, 0, 0).ok()?.unix();
    let max = Timestamp::from_utc(9999, 12, 31, 23, 59, 59).ok()?.unix();
    if shifted < min || shifted > max {
        return None;
    }
    Some(Timestamp::from_unix(shifted))
}

/// Text characters lxml accepts (XML 1.0 `Char`).
fn xml_compatible(text: &str) -> bool {
    text.chars().all(|c| {
        matches!(c, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
    })
}

/// Python `str(datetime)` of an aware UTC instant.
fn py_str_datetime(ts: Timestamp) -> String {
    let xml = ts.to_xml();
    format!("{} {}+00:00", &xml[..10], &xml[11..19])
}

/// The XML text a change writes, validated like the upstream setter.
fn rendered(change: &CorePropertyChange) -> Result<String> {
    let property = change.property;
    match property.kind() {
        Kind::Text => {
            let text = match &change.value {
                CoreValue::Text(text) => text.clone(),
                CoreValue::Int(n) => n.to_string(),
                CoreValue::Date(ts) => py_str_datetime(*ts),
            };
            if text.chars().count() > 255 {
                return Err(Error::invalid_argument(
                    "core_property_too_long",
                    format!(
                        "exceeded 255 char limit for property {}, got {} characters",
                        property.name(),
                        text.chars().count()
                    ),
                ));
            }
            if !xml_compatible(&text) {
                return Err(Error::invalid_argument(
                    "xml_incompatible_text",
                    "All strings must be XML compatible: Unicode or ASCII, no NULL bytes or control characters",
                ));
            }
            Ok(text)
        }
        Kind::Date => match &change.value {
            CoreValue::Date(ts) => Ok(ts.to_xml()),
            _ => Err(Error::invalid_argument(
                "core_property_requires_datetime",
                format!(
                    "property {} requires <type 'datetime.datetime'> object",
                    property.name()
                ),
            )),
        },
        Kind::Revision => match &change.value {
            CoreValue::Int(n) if *n >= 1 => Ok(n.to_string()),
            other => Err(Error::invalid_argument(
                "invalid_revision",
                format!("revision property requires positive int, got {other:?}"),
            )),
        },
    }
}

/// `_get_or_add` + `.text = value` (+ the `xsi:type` of created/modified).
pub(crate) fn write(tree: &mut XmlTree, property: CoreProperty, text: &str) -> Result<()> {
    let root = tree.root();
    let (ns, local) = property.element();
    let element = match tree.first_child_named(root, ns, local) {
        Some(existing) => existing,
        None => {
            // ZeroOrOne with no successors: appended.
            let element = tree.create_element(root, ns, local)?;
            let end = tree.children(root).len();
            tree.insert(root, end, element)?;
            element
        }
    };
    set_leading_text(tree, element, text)?;
    if matches!(property, CoreProperty::Created | CoreProperty::Modified) {
        let prefix = tree
            .prefix_for(element, DCTERMS)
            .flatten()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "dcterms".to_string());
        tree.set_attr(element, Some(XSI), "type", &format!("{prefix}:W3CDTF"))?;
    }
    Ok(())
}

/// `CT_CoreProperties.new()` followed by `CorePropertiesPart.default`'s
/// assignments.
fn default_part(now: Timestamp) -> Result<XmlTree> {
    let mut tree = xml::parse_str(&format!(
        "<cp:coreProperties xmlns:cp=\"{CP}\" xmlns:dc=\"{DC}\" xmlns:dcterms=\"{DCTERMS}\"/>"
    ))
    .map_err(|err| Error::internal("coreprops_template", err.to_string()))?;
    write(&mut tree, CoreProperty::Title, "Word Document")?;
    write(&mut tree, CoreProperty::LastModifiedBy, "python-docx")?;
    write(&mut tree, CoreProperty::Revision, "1")?;
    write(&mut tree, CoreProperty::Modified, &now.to_xml())?;
    Ok(tree)
}

fn existing_part(txn: &Txn<'_>) -> Result<Option<String>> {
    txn.part_related_by("/", rt::CORE_PROPERTIES)
}

/// Upstream `OpcPackage._core_properties_part` inside `txn`.
pub(crate) fn ensure_part(txn: &mut Txn<'_>, now: Option<Timestamp>) -> Result<String> {
    if let Some(part) = existing_part(txn)? {
        return Ok(part);
    }
    if txn.resolve(PART_NAME).is_some() {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "core_properties_partname_collision",
            format!(
                "{PART_NAME} exists but is not related as the core-properties part; refusing to create a second part with that name"
            ),
        ));
    }
    // A stale `Override` for the missing part (Word leaves them behind)
    // would become ambiguous next to the new one; python-docx rewrites the
    // content types from the parts it holds, so it keeps exactly one.
    {
        let types = txn.tree_mut("[Content_Types].xml")?;
        let root = types.root();
        let stale: Vec<NodeId> = types
            .child_elements(root)
            .filter(|node| types.is(*node, CT_NS, "Override"))
            .filter(|node| {
                types
                    .attr(*node, None, "PartName")
                    .is_some_and(|name| name.eq_ignore_ascii_case(&format!("/{PART_NAME}")))
            })
            .collect();
        for node in stale {
            types.detach(node)?;
        }
    }
    let tree = default_part(now.unwrap_or_else(Timestamp::now))?;
    txn.add_xml_part(PART_NAME, ct::OPC_CORE_PROPERTIES, tree)?;
    txn.relate_to("/", rt::CORE_PROPERTIES, RelTarget::Part(PART_NAME))?;
    Ok(PART_NAME.to_string())
}

/// The document's core properties (see the module docs).
pub fn core_properties(document: &Document) -> Result<CoreProperties> {
    let package = document.package();
    match package.part_related_by("/", rt::CORE_PROPERTIES)? {
        Some(part) => Ok(read_core_properties(package.xml(&part)?, Some(part))),
        None => Ok(CoreProperties {
            part: None,
            author: String::new(),
            category: String::new(),
            comments: String::new(),
            content_status: String::new(),
            created: None,
            identifier: String::new(),
            keywords: String::new(),
            language: String::new(),
            last_modified_by: "python-docx".into(),
            last_printed: None,
            modified: None,
            revision: 1,
            subject: String::new(),
            title: "Word Document".into(),
            version: String::new(),
        }),
    }
}

/// Assign core properties in order, atomically (see the module docs).
/// `now` stamps `modified` of a newly created part (`None`: the current
/// time). Every value is validated before anything is staged.
pub fn set_core_properties(
    document: &mut Document,
    changes: &[CorePropertyChange],
    now: Option<Timestamp>,
) -> Result<CoreProperties> {
    let texts = changes.iter().map(rendered).collect::<Result<Vec<_>>>()?;
    refuse_if_protected(document, "set core properties", OperationClass::BodyContent)?;
    let mut txn = Txn::new(document);
    let part = ensure_part(&mut txn, now)?;
    {
        let tree = txn.tree_mut(&part)?;
        for (change, text) in changes.iter().zip(texts.iter()) {
            write(tree, change.property, text)?;
        }
    }
    let staged = txn.finish()?;
    document.commit(staged)?;
    core_properties(document)
}

impl Document {
    /// Core document properties (read-only snapshot; never creates a part).
    pub fn core_properties(&self) -> Result<CoreProperties> {
        core_properties(self)
    }

    /// Assign core properties (see [`set_core_properties`]).
    pub fn set_core_properties(
        &mut self,
        changes: &[CorePropertyChange],
        now: Option<Timestamp>,
    ) -> Result<CoreProperties> {
        set_core_properties(self, changes, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(text: &str) -> Option<String> {
        parse_w3cdtf(text).map(|t| t.to_xml())
    }

    #[test]
    fn w3cdtf_parsing_follows_upstream() {
        assert_eq!(ts("2003"), Some("2003-01-01T00:00:00Z".into()));
        assert_eq!(ts("2003-12"), Some("2003-12-01T00:00:00Z".into()));
        assert_eq!(ts("2003-12-31"), Some("2003-12-31T00:00:00Z".into()));
        assert_eq!(
            ts("2003-12-31T10:14:55Z"),
            Some("2003-12-31T10:14:55Z".into())
        );
        assert_eq!(
            ts("2012-11-17T11:07:40-05:30"),
            Some("2012-11-17T16:37:40Z".into())
        );
        assert_eq!(
            ts("2003-12-31T10:14:55+01:00"),
            Some("2003-12-31T09:14:55Z".into())
        );
        assert_eq!(ts("2003-1-5"), Some("2003-01-05T00:00:00Z".into()));
        assert_eq!(ts("2003-02-30"), None);
        assert_eq!(ts("foobar"), None);
        assert_eq!(ts(""), None);
        assert_eq!(
            ts("2003-12-31t10:14:55"),
            Some("2003-12-31T10:14:55Z".into())
        );
        assert_eq!(ts("2003-12-31T10:14:55+0100x"), None);
    }
}
