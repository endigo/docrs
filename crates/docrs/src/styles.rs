//! Read side of the style collection (python-docx `styles` package as
//! shipped by upstream: `styles/__init__.py` (`BabelFish`), `styles.py`,
//! `style.py`, `latent.py` and `oxml/styles.py`; MIT, see `NOTICE.md`).
//!
//! [`Styles`] is an owned snapshot of the styles part: lookup by UI name
//! (with the special-case `BabelFish` aliases) and by style id, the default
//! style per type, base and next styles, the on/off display properties and
//! the latent-style defaults and exceptions. Reading never creates or edits
//! a part: a document without a styles part reads the bundled default
//! template (upstream would add that part to the package as a side effect).
//!
//! Writing styles belongs to the authoring packages; they reuse the
//! `pub(crate)` element readers at the bottom of this module.

use serde::Serialize;

use crate::package::rels::rt;
use crate::unicode::py_strip;
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, Result};

/// The default styles part (upstream `templates/default-styles.xml`).
const DEFAULT_STYLES: &[u8] = include_bytes!("../assets/default-styles.xml");

/// Special-case UI ↔ internal style names (upstream `BabelFish`).
pub const STYLE_ALIASES: &[(&str, &str)] = &[
    ("Caption", "caption"),
    ("Footer", "footer"),
    ("Header", "header"),
    ("Heading 1", "heading 1"),
    ("Heading 2", "heading 2"),
    ("Heading 3", "heading 3"),
    ("Heading 4", "heading 4"),
    ("Heading 5", "heading 5"),
    ("Heading 6", "heading 6"),
    ("Heading 7", "heading 7"),
    ("Heading 8", "heading 8"),
    ("Heading 9", "heading 9"),
];

/// Internal (styles.xml) name for a UI name, e.g. `heading 1` for
/// `Heading 1` (upstream `BabelFish.ui2internal`).
pub fn ui_to_internal(ui_name: &str) -> &str {
    STYLE_ALIASES
        .iter()
        .find(|(ui, _)| *ui == ui_name)
        .map(|(_, internal)| *internal)
        .unwrap_or(ui_name)
}

/// UI name for an internal name, e.g. `Heading 1` for `heading 1`
/// (upstream `BabelFish.internal2ui`).
pub fn internal_to_ui(internal_name: &str) -> &str {
    STYLE_ALIASES
        .iter()
        .find(|(_, internal)| *internal == internal_name)
        .map(|(ui, _)| *ui)
        .unwrap_or(internal_name)
}

/// Style id python-docx derives from an internal name (upstream
/// `oxml.styles.styleId_from_name`).
pub fn style_id_from_name(name: &str) -> String {
    match name {
        "caption" => "Caption".to_string(),
        "heading 1" | "heading 2" | "heading 3" | "heading 4" | "heading 5" | "heading 6"
        | "heading 7" | "heading 8" | "heading 9" => {
            format!("Heading{}", &name["heading ".len()..])
        }
        _ => name.replace(' ', ""),
    }
}

/// `WD_STYLE_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleType {
    /// `paragraph` (1).
    Paragraph,
    /// `character` (2).
    Character,
    /// `table` (3).
    Table,
    /// `numbering` (4, `WD_STYLE_TYPE.LIST`).
    Numbering,
}

impl StyleType {
    /// All types in `WD_STYLE_TYPE` order.
    pub const ALL: [StyleType; 4] = [
        StyleType::Paragraph,
        StyleType::Character,
        StyleType::Table,
        StyleType::Numbering,
    ];

    /// The `w:type` token.
    pub fn as_xml(self) -> &'static str {
        match self {
            StyleType::Paragraph => "paragraph",
            StyleType::Character => "character",
            StyleType::Table => "table",
            StyleType::Numbering => "numbering",
        }
    }

    /// Parse a `w:type` token (`WD_STYLE_TYPE.from_xml`).
    pub fn from_xml(token: &str) -> Result<StyleType> {
        match token {
            "paragraph" => Ok(StyleType::Paragraph),
            "character" => Ok(StyleType::Character),
            "table" => Ok(StyleType::Table),
            "numbering" => Ok(StyleType::Numbering),
            _ => Err(Error::invalid_argument(
                "invalid_style_type",
                format!("WD_STYLE_TYPE has no XML mapping for {token:?}"),
            )),
        }
    }
}

/// `ST_OnOff` as python-docx reads it: `1`/`true`/`on` are true. Tokens
/// outside the schema read as false (python-docx raises `InvalidXmlError`).
pub(crate) fn on_off(value: &str) -> bool {
    matches!(value, "1" | "true" | "on")
}

/// Python `int(text)` for `ST_DecimalNumber` (surrounding whitespace, sign
/// and digit-group underscores allowed).
pub(crate) fn py_int(value: &str) -> Option<i64> {
    let trimmed = py_strip(value);
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let parsed: i64 = digits.replace('_', "").parse().ok()?;
    Some(if negative { -parsed } else { parsed })
}

fn decimal(value: &str, what: &str) -> Result<i64> {
    py_int(value).ok_or_else(|| {
        Error::invalid_argument(
            "invalid_decimal_number",
            format!("invalid literal for int() with base 10: {value:?} ({what})"),
        )
    })
}

/// `CT_OnOff` child value: absent → `None`; present without `w:val` → true.
fn on_off_child(tree: &XmlTree, parent: NodeId, local: &str) -> Option<bool> {
    w_child(tree, parent, local).map(|child| w_attr(tree, child, "val").is_none_or(on_off))
}

/// One `w:style` definition (python-docx `BaseStyle` and subclasses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Style {
    /// `w:styleId`.
    pub style_id: Option<String>,
    /// UI name (`BabelFish.internal2ui` of `w:name/@w:val`).
    pub name: Option<String>,
    /// Raw `w:name/@w:val`.
    pub internal_name: Option<String>,
    /// Style type (`paragraph` when `w:type` is absent, as `BaseStyle.type`).
    pub style_type: StyleType,
    /// `w:type` was present.
    pub type_declared: bool,
    /// Not marked `w:customStyle`.
    pub builtin: bool,
    /// `w:semiHidden`.
    pub hidden: bool,
    /// `w:locked`.
    pub locked: bool,
    /// `w:uiPriority`.
    pub priority: Option<i64>,
    /// `w:qFormat`.
    pub quick_style: bool,
    /// `w:unhideWhenUsed`.
    pub unhide_when_used: bool,
    /// `w:default` is on.
    pub is_default: bool,
    /// Raw `w:basedOn/@w:val`.
    pub based_on: Option<String>,
    /// Raw `w:next/@w:val`.
    pub next: Option<String>,
    /// Position among the `w:style` children of `w:styles`.
    pub index: usize,
}

/// `w:lsdException` (python-docx `_LatentStyle`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LatentStyle {
    /// UI name.
    pub name: String,
    /// Raw `w:name`.
    pub internal_name: String,
    /// `w:semiHidden` (tri-state).
    pub hidden: Option<bool>,
    /// `w:locked` (tri-state).
    pub locked: Option<bool>,
    /// `w:uiPriority`.
    pub priority: Option<i64>,
    /// `w:qFormat` (tri-state).
    pub quick_style: Option<bool>,
    /// `w:unhideWhenUsed` (tri-state).
    pub unhide_when_used: Option<bool>,
}

/// `w:latentStyles` (python-docx `LatentStyles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LatentStyles {
    /// `w:defUIPriority`.
    pub default_priority: Option<i64>,
    /// `w:defSemiHidden` (false when absent).
    pub default_to_hidden: bool,
    /// `w:defLockedState`.
    pub default_to_locked: bool,
    /// `w:defQFormat`.
    pub default_to_quick_style: bool,
    /// `w:defUnhideWhenUsed`.
    pub default_to_unhide_when_used: bool,
    /// `w:count`.
    pub load_count: Option<i64>,
    /// Exceptions in document order.
    pub exceptions: Vec<LatentStyle>,
}

impl LatentStyles {
    /// The exception for `name` (UI or internal name; upstream
    /// `LatentStyles[name]`).
    pub fn get(&self, name: &str) -> Option<&LatentStyle> {
        let internal = ui_to_internal(name);
        self.exceptions
            .iter()
            .find(|style| style.internal_name == internal)
    }

    /// Number of exceptions.
    pub fn len(&self) -> usize {
        self.exceptions.len()
    }

    /// No exceptions.
    pub fn is_empty(&self) -> bool {
        self.exceptions.is_empty()
    }
}

/// The document's style collection (python-docx `Styles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Styles {
    /// Package member read, or `None` when the bundled default was used
    /// because the document has no styles part.
    pub part: Option<String>,
    styles: Vec<Style>,
    latent: Option<LatentStyles>,
}

impl Styles {
    /// Every style in document order.
    pub fn iter(&self) -> impl Iterator<Item = &Style> {
        self.styles.iter()
    }

    /// Number of styles.
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// No styles.
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// Upstream `name in styles`: a style whose internal name is the
    /// internal form of `name`.
    pub fn contains(&self, name: &str) -> bool {
        let internal = ui_to_internal(name);
        self.styles
            .iter()
            .any(|style| style.internal_name.as_deref() == Some(internal))
    }

    /// The first style named `name` (UI name; internal aliases applied).
    pub fn by_name(&self, name: &str) -> Option<&Style> {
        let internal = ui_to_internal(name);
        self.styles
            .iter()
            .find(|style| style.internal_name.as_deref() == Some(internal))
    }

    /// The first style with `w:styleId == style_id`.
    pub fn by_id(&self, style_id: &str) -> Option<&Style> {
        self.styles
            .iter()
            .find(|style| style.style_id.as_deref() == Some(style_id))
    }

    /// Upstream `styles[key]`: by name, else (deprecated upstream) by id.
    pub fn get(&self, key: &str) -> Option<&Style> {
        self.by_name(key).or_else(|| self.by_id(key))
    }

    /// Default style of `style_type`: the last `w:default` style of that
    /// declared type (upstream `Styles.default`).
    pub fn default_style(&self, style_type: StyleType) -> Option<&Style> {
        self.styles.iter().rfind(|style| {
            style.type_declared && style.style_type == style_type && style.is_default
        })
    }

    /// Upstream `Styles.get_by_id`: the style of `style_type` with
    /// `style_id`, else the default for the type.
    pub fn get_by_id(&self, style_id: Option<&str>, style_type: StyleType) -> Option<&Style> {
        let Some(style_id) = style_id.filter(|id| !id.is_empty()) else {
            return self.default_style(style_type);
        };
        match self.by_id(style_id) {
            Some(style) if style.type_declared && style.style_type == style_type => Some(style),
            _ => self.default_style(style_type),
        }
    }

    /// Upstream `Styles.get_style_id(name, style_type)`: the id of the style
    /// named `name`, or `None` when it is the type's default. Refuses an
    /// unknown name or a style of another type.
    pub fn get_style_id(&self, name: &str, style_type: StyleType) -> Result<Option<String>> {
        let Some(style) = self.get(name) else {
            return Err(Error::invalid_argument(
                "unknown_style",
                format!("no style with name {name:?}"),
            ));
        };
        if style.style_type != style_type {
            return Err(Error::invalid_argument(
                "style_type_mismatch",
                format!(
                    "assigned style is type {}, need type {}",
                    style.style_type.as_xml(),
                    style_type.as_xml()
                ),
            ));
        }
        if self
            .default_style(style_type)
            .is_some_and(|default| default.index == style.index)
        {
            return Ok(None);
        }
        Ok(style.style_id.clone())
    }

    /// `style.base_style`: the existing style named by `w:basedOn`.
    pub fn base_style(&self, style: &Style) -> Option<&Style> {
        style.based_on.as_deref().and_then(|id| self.by_id(id))
    }

    /// `style.next_paragraph_style` (paragraph and table styles): the
    /// `w:next` style when it exists and is a declared paragraph style,
    /// otherwise `style` itself.
    pub fn next_paragraph_style<'a>(&'a self, style: &'a Style) -> &'a Style {
        match style.next.as_deref().and_then(|id| self.by_id(id)) {
            Some(next) if next.type_declared && next.style_type == StyleType::Paragraph => next,
            _ => style,
        }
    }

    /// `w:latentStyles`, when present (reading never adds it).
    pub fn latent_styles(&self) -> Option<&LatentStyles> {
        self.latent.as_ref()
    }
}

fn read_style(tree: &XmlTree, element: NodeId, index: usize) -> Result<Style> {
    let raw_type = w_attr(tree, element, "type");
    let style_type = match raw_type {
        Some(token) => StyleType::from_xml(token)?,
        None => StyleType::Paragraph,
    };
    let internal_name = w_child(tree, element, "name")
        .and_then(|name| w_attr(tree, name, "val"))
        .map(str::to_string);
    let priority = match w_child(tree, element, "uiPriority").and_then(|p| w_attr(tree, p, "val")) {
        Some(value) => Some(decimal(value, "w:uiPriority")?),
        None => None,
    };
    Ok(Style {
        style_id: w_attr(tree, element, "styleId").map(str::to_string),
        name: internal_name
            .as_deref()
            .map(|name| internal_to_ui(name).to_string()),
        internal_name,
        style_type,
        type_declared: raw_type.is_some(),
        builtin: !w_attr(tree, element, "customStyle").is_some_and(on_off),
        hidden: on_off_child(tree, element, "semiHidden").unwrap_or(false),
        locked: on_off_child(tree, element, "locked").unwrap_or(false),
        priority,
        quick_style: on_off_child(tree, element, "qFormat").unwrap_or(false),
        unhide_when_used: on_off_child(tree, element, "unhideWhenUsed").unwrap_or(false),
        is_default: w_attr(tree, element, "default").is_some_and(on_off),
        based_on: w_child(tree, element, "basedOn")
            .and_then(|b| w_attr(tree, b, "val"))
            .map(str::to_string),
        next: w_child(tree, element, "next")
            .and_then(|n| w_attr(tree, n, "val"))
            .map(str::to_string),
        index,
    })
}

fn read_latent(tree: &XmlTree, element: NodeId) -> Result<LatentStyles> {
    let decimal_attr = |name: &str| -> Result<Option<i64>> {
        match w_attr(tree, element, name) {
            Some(value) => Ok(Some(decimal(value, name)?)),
            None => Ok(None),
        }
    };
    let flag = |name: &str| w_attr(tree, element, name).is_some_and(on_off);
    let mut exceptions = Vec::new();
    for exception in w_children(tree, element, "lsdException") {
        let tri = |name: &str| w_attr(tree, exception, name).map(on_off);
        let priority = match w_attr(tree, exception, "uiPriority") {
            Some(value) => Some(decimal(value, "w:uiPriority")?),
            None => None,
        };
        let raw_name = w_attr(tree, exception, "name").unwrap_or("");
        exceptions.push(LatentStyle {
            name: internal_to_ui(raw_name).to_string(),
            internal_name: raw_name.to_string(),
            hidden: tri("semiHidden"),
            locked: tri("locked"),
            priority,
            quick_style: tri("qFormat"),
            unhide_when_used: tri("unhideWhenUsed"),
        });
    }
    Ok(LatentStyles {
        default_priority: decimal_attr("defUIPriority")?,
        default_to_hidden: flag("defSemiHidden"),
        default_to_locked: flag("defLockedState"),
        default_to_quick_style: flag("defQFormat"),
        default_to_unhide_when_used: flag("defUnhideWhenUsed"),
        load_count: decimal_attr("count")?,
        exceptions,
    })
}

/// Snapshot of `tree` (a `w:styles` root).
pub(crate) fn read_styles(tree: &XmlTree, part: Option<String>) -> Result<Styles> {
    let root = tree.root();
    let styles = style_elements(tree)
        .into_iter()
        .enumerate()
        .map(|(index, element)| read_style(tree, element, index))
        .collect::<Result<Vec<_>>>()?;
    let latent = match w_child(tree, root, "latentStyles") {
        Some(element) => Some(read_latent(tree, element)?),
        None => None,
    };
    Ok(Styles {
        part,
        styles,
        latent,
    })
}

/// The document's styles (see [`Styles`]).
pub fn styles(document: &Document) -> Result<Styles> {
    match styles_part(document)? {
        Some(part) => read_styles(document.package().xml(&part)?, Some(part)),
        None => read_styles(&default_styles_tree()?, None),
    }
}

impl Document {
    /// The style collection (read-only snapshot; see [`Styles`]).
    pub fn styles(&self) -> Result<Styles> {
        styles(self)
    }
}

/// Member of the styles part related to the main document, if any.
pub(crate) fn styles_part(document: &Document) -> Result<Option<String>> {
    let package = document.package();
    package.part_related_by(package.main_part_name(), rt::STYLES)
}

/// The bundled default styles part, parsed like python-docx templates.
pub(crate) fn default_styles_tree() -> Result<XmlTree> {
    crate::parts::template(DEFAULT_STYLES)
}

/// Direct `w:style` children of the `w:styles` root.
pub(crate) fn style_elements(tree: &XmlTree) -> Vec<NodeId> {
    w_children(tree, tree.root(), "style").collect()
}

/// First `w:style` with `w:styleId == style_id` (`CT_Styles.get_by_id`).
pub(crate) fn style_element_by_id(tree: &XmlTree, style_id: &str) -> Option<NodeId> {
    style_elements(tree)
        .into_iter()
        .find(|style| w_attr(tree, *style, "styleId") == Some(style_id))
}

/// First `w:style` whose `w:name/@w:val` is `internal_name`
/// (`CT_Styles.get_by_name`).
pub(crate) fn style_element_by_name(tree: &XmlTree, internal_name: &str) -> Option<NodeId> {
    style_elements(tree).into_iter().find(|style| {
        w_child(tree, *style, "name").and_then(|name| w_attr(tree, name, "val"))
            == Some(internal_name)
    })
}

/// `w:styleId` → element, later duplicates winning (upstream
/// `formatting._styles_index`).
pub(crate) fn styles_index(tree: &XmlTree) -> std::collections::HashMap<String, NodeId> {
    let mut index = std::collections::HashMap::new();
    for style in style_elements(tree) {
        if let Some(id) = w_attr(tree, style, "styleId").filter(|id| !id.is_empty()) {
            index.insert(id.to_string(), style);
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn babel_fish_round_trips_aliases() {
        assert_eq!(ui_to_internal("Heading 1"), "heading 1");
        assert_eq!(internal_to_ui("heading 9"), "Heading 9");
        assert_eq!(internal_to_ui("caption"), "Caption");
        assert_eq!(ui_to_internal("Body Text"), "Body Text");
        assert_eq!(ui_to_internal("heading 1"), "heading 1");
    }

    #[test]
    fn style_ids_follow_python_docx() {
        assert_eq!(style_id_from_name("heading 3"), "Heading3");
        assert_eq!(style_id_from_name("caption"), "Caption");
        assert_eq!(style_id_from_name("Body Text 2"), "BodyText2");
    }

    #[test]
    fn python_int_accepts_whitespace_sign_and_underscores() {
        assert_eq!(py_int(" 42 "), Some(42));
        assert_eq!(py_int("+4_2"), Some(42));
        assert_eq!(py_int("-7"), Some(-7));
        assert_eq!(py_int("4__2"), None);
        assert_eq!(py_int("x"), None);
        assert_eq!(py_int(""), None);
    }
}
