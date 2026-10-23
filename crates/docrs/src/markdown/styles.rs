//! Which reference-document style each Markdown element takes.
//!
//! Every element has a *role*. A role looks its style up by name in the
//! reference document, trying pandoc's name first (so a pandoc reference
//! document works unchanged) and then the nearest Word built-in. When none
//! exists the converter either adds a definition (the default) or refuses
//! ([`StylePolicy::Strict`]); body-text roles always end at the document's
//! default paragraph style, so they never need either. Roles resolve on
//! first use, so a conversion only touches the styles it needs.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::numbering::import_fragment;
use crate::styles::{internal_to_ui, style_id_from_name, ui_to_internal};
use crate::txn::Txn;
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Error, Result};

/// What to do when the reference document lacks a style for a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StylePolicy {
    /// Add a pandoc-compatible definition (recorded in the report).
    #[default]
    Create,
    /// Refuse the conversion and name the missing styles; nothing changes.
    Strict,
}

/// The style roles Markdown elements take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Front-matter `title`.
    Title,
    /// Front-matter `subtitle`.
    Subtitle,
    /// Front-matter `author`.
    Author,
    /// Front-matter `date`.
    Date,
    /// Front-matter `abstract`.
    Abstract,
    /// `#` heading.
    Heading1,
    /// `##` heading.
    Heading2,
    /// `###` heading.
    Heading3,
    /// `####` heading.
    Heading4,
    /// `#####` heading.
    Heading5,
    /// `######` heading.
    Heading6,
    /// Level 7 (reachable with a heading offset).
    Heading7,
    /// Level 8 (reachable with a heading offset).
    Heading8,
    /// Level 9 (reachable with a heading offset).
    Heading9,
    /// Ordinary paragraphs.
    Body,
    /// The first paragraph after a heading or other non-paragraph block.
    FirstParagraph,
    /// Items of tight lists and table cells.
    Compact,
    /// `>` block quotes and alerts.
    BlockQuote,
    /// Fenced and indented code blocks.
    CodeBlock,
    /// `` `code` `` spans (character style).
    InlineCode,
    /// Link text (character style).
    Hyperlink,
    /// Footnote paragraphs.
    FootnoteText,
    /// Footnote marks (character style).
    FootnoteReference,
    /// Definition-list terms.
    DefinitionTerm,
    /// Definition-list definitions.
    Definition,
    /// The paragraph holding a stand-alone image.
    Figure,
    /// Caption under a stand-alone image (its alt text).
    ImageCaption,
    /// `Table: …` caption of a table.
    TableCaption,
    /// Title above a table of contents.
    TocHeading,
    /// Tables (table style).
    Table,
    /// Paragraphs inside table cells (derived from `Compact` with the
    /// table style's paragraph spacing; LibreOffice calls it Table Contents).
    TableContents,
    /// Table of contents entries, level 1.
    Toc1,
    /// Level 2 entries.
    Toc2,
    /// Level 3 entries.
    Toc3,
    /// Level 4 entries.
    Toc4,
    /// Level 5 entries.
    Toc5,
    /// Level 6 entries.
    Toc6,
    /// Level 7 entries.
    Toc7,
    /// Level 8 entries.
    Toc8,
    /// Level 9 entries.
    Toc9,
}

/// Style kinds a role can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Paragraph,
    Character,
    Table,
}

impl Kind {
    fn xml(self) -> &'static str {
        match self {
            Kind::Paragraph => "paragraph",
            Kind::Character => "character",
            Kind::Table => "table",
        }
    }
}

impl Role {
    /// Every role, in report order.
    pub const ALL: [Role; 40] = [
        Role::Title,
        Role::Subtitle,
        Role::Author,
        Role::Date,
        Role::Abstract,
        Role::Heading1,
        Role::Heading2,
        Role::Heading3,
        Role::Heading4,
        Role::Heading5,
        Role::Heading6,
        Role::Heading7,
        Role::Heading8,
        Role::Heading9,
        Role::Body,
        Role::FirstParagraph,
        Role::Compact,
        Role::BlockQuote,
        Role::CodeBlock,
        Role::InlineCode,
        Role::Hyperlink,
        Role::FootnoteText,
        Role::FootnoteReference,
        Role::DefinitionTerm,
        Role::Definition,
        Role::Figure,
        Role::ImageCaption,
        Role::TableCaption,
        Role::TocHeading,
        Role::Table,
        Role::TableContents,
        Role::Toc1,
        Role::Toc2,
        Role::Toc3,
        Role::Toc4,
        Role::Toc5,
        Role::Toc6,
        Role::Toc7,
        Role::Toc8,
        Role::Toc9,
    ];

    /// Stable key (`heading1`, `code_block`, …) used by style maps.
    pub fn key(self) -> &'static str {
        match self {
            Role::Title => "title",
            Role::Subtitle => "subtitle",
            Role::Author => "author",
            Role::Date => "date",
            Role::Abstract => "abstract",
            Role::Heading1 => "heading1",
            Role::Heading2 => "heading2",
            Role::Heading3 => "heading3",
            Role::Heading4 => "heading4",
            Role::Heading5 => "heading5",
            Role::Heading6 => "heading6",
            Role::Heading7 => "heading7",
            Role::Heading8 => "heading8",
            Role::Heading9 => "heading9",
            Role::Body => "body",
            Role::FirstParagraph => "first_paragraph",
            Role::Compact => "compact",
            Role::BlockQuote => "block_quote",
            Role::CodeBlock => "code_block",
            Role::InlineCode => "inline_code",
            Role::Hyperlink => "hyperlink",
            Role::FootnoteText => "footnote_text",
            Role::FootnoteReference => "footnote_reference",
            Role::DefinitionTerm => "definition_term",
            Role::Definition => "definition",
            Role::Figure => "figure",
            Role::ImageCaption => "image_caption",
            Role::TableCaption => "table_caption",
            Role::TocHeading => "toc_heading",
            Role::Table => "table",
            Role::TableContents => "table_contents",
            Role::Toc1 => "toc1",
            Role::Toc2 => "toc2",
            Role::Toc3 => "toc3",
            Role::Toc4 => "toc4",
            Role::Toc5 => "toc5",
            Role::Toc6 => "toc6",
            Role::Toc7 => "toc7",
            Role::Toc8 => "toc8",
            Role::Toc9 => "toc9",
        }
    }

    /// The role with key `key`.
    pub fn from_key(key: &str) -> Result<Role> {
        Role::ALL
            .into_iter()
            .find(|role| role.key() == key)
            .ok_or_else(|| {
                let keys: Vec<&str> = Role::ALL.iter().map(|r| r.key()).collect();
                Error::invalid_argument(
                    "unknown_style_role",
                    format!("unknown style role {key:?}; roles are {}", keys.join(", ")),
                )
            })
    }

    /// The heading role for `level` (clamped to 1–9).
    pub fn heading(level: u8) -> Role {
        match level {
            0 | 1 => Role::Heading1,
            2 => Role::Heading2,
            3 => Role::Heading3,
            4 => Role::Heading4,
            5 => Role::Heading5,
            6 => Role::Heading6,
            7 => Role::Heading7,
            8 => Role::Heading8,
            _ => Role::Heading9,
        }
    }

    /// The contents-entry role for `level` (clamped to 1–9).
    pub fn toc(level: u8) -> Role {
        match level {
            0 | 1 => Role::Toc1,
            2 => Role::Toc2,
            3 => Role::Toc3,
            4 => Role::Toc4,
            5 => Role::Toc5,
            6 => Role::Toc6,
            7 => Role::Toc7,
            8 => Role::Toc8,
            _ => Role::Toc9,
        }
    }

    fn toc_level(self) -> Option<u8> {
        Some(match self {
            Role::Toc1 => 1,
            Role::Toc2 => 2,
            Role::Toc3 => 3,
            Role::Toc4 => 4,
            Role::Toc5 => 5,
            Role::Toc6 => 6,
            Role::Toc7 => 7,
            Role::Toc8 => 8,
            Role::Toc9 => 9,
            _ => return None,
        })
    }

    fn heading_level(self) -> Option<u8> {
        Some(match self {
            Role::Heading1 => 1,
            Role::Heading2 => 2,
            Role::Heading3 => 3,
            Role::Heading4 => 4,
            Role::Heading5 => 5,
            Role::Heading6 => 6,
            Role::Heading7 => 7,
            Role::Heading8 => 8,
            Role::Heading9 => 9,
            _ => return None,
        })
    }

    pub(crate) fn kind(self) -> Kind {
        match self {
            Role::InlineCode | Role::Hyperlink | Role::FootnoteReference => Kind::Character,
            Role::Table => Kind::Table,
            _ => Kind::Paragraph,
        }
    }

    /// Style names tried in order (UI names): pandoc's first, then Word's.
    pub fn candidates(self) -> Vec<String> {
        let names: &[&str] = match self {
            Role::Title => &["Title"],
            Role::Subtitle => &["Subtitle"],
            Role::Author => &["Author"],
            Role::Date => &["Date"],
            Role::Abstract => &["Abstract"],
            // Some firms keep body formatting on a custom "Paragraph" style.
            Role::Body => &["Body Text", "Paragraph"],
            Role::FirstParagraph => &["First Paragraph", "Body Text", "Paragraph"],
            Role::Compact => &["Compact", "Body Text", "Paragraph"],
            Role::BlockQuote => &["Block Text", "Quote"],
            Role::CodeBlock => &["Source Code", "HTML Preformatted"],
            Role::InlineCode => &["Verbatim Char", "HTML Code"],
            Role::Hyperlink => &["Hyperlink"],
            Role::FootnoteText => &["Footnote Text"],
            Role::FootnoteReference => &["Footnote Reference"],
            Role::DefinitionTerm => &["Definition Term"],
            Role::Definition => &["Definition"],
            Role::Figure => &["Captioned Figure", "Figure"],
            Role::ImageCaption => &["Image Caption", "Caption"],
            Role::TableCaption => &["Table Caption", "Caption"],
            Role::TocHeading => &["TOC Heading"],
            Role::Table => &["Table", "Table Grid"],
            Role::TableContents => &["Table Contents"],
            toc if toc.toc_level().is_some() => {
                return vec![format!("TOC {}", toc.toc_level().unwrap_or(1))];
            }
            heading => {
                let level = heading.heading_level().unwrap_or(1);
                return vec![format!("Heading {level}")];
            }
        };
        names.iter().map(|n| n.to_string()).collect()
    }

    fn falls_back_to_default(self) -> bool {
        matches!(self, Role::Body | Role::FirstParagraph | Role::Compact)
    }
}

/// How a role was satisfied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RoleStyle {
    /// UI name of the style used.
    pub name: String,
    /// Its style id; `None` writes no explicit style (the default style).
    pub style_id: Option<String>,
    /// Whether the conversion added the definition.
    pub created: bool,
}

fn find_style(tree: &XmlTree, name: &str, kind: Kind) -> Option<NodeId> {
    let wanted = ui_to_internal(name).to_lowercase();
    w_children(tree, tree.root(), "style").find(|style| {
        w_attr(tree, *style, "type").unwrap_or("paragraph") == kind.xml()
            && w_child(tree, *style, "name")
                .and_then(|n| w_attr(tree, n, "val"))
                .is_some_and(|v| v.to_lowercase() == wanted)
    })
}

fn default_style(tree: &XmlTree, kind: Kind) -> Option<NodeId> {
    w_children(tree, tree.root(), "style").find(|style| {
        w_attr(tree, *style, "type").unwrap_or("paragraph") == kind.xml()
            && matches!(w_attr(tree, *style, "default"), Some("1" | "true" | "on"))
    })
}

fn display_name(tree: &XmlTree, style: NodeId) -> String {
    let internal = w_child(tree, style, "name")
        .and_then(|n| w_attr(tree, n, "val"))
        .unwrap_or("");
    ui_name(internal)
}

/// The name Word shows for an internal style name (built-ins such as
/// `footnote text` are stored in lower case).
fn ui_name(internal: &str) -> String {
    if let Some(level) = internal.strip_prefix("toc ") {
        return format!("TOC {level}");
    }
    const LOWER_BUILTINS: &[&str] = &[
        "footnote text",
        "footnote reference",
        "endnote text",
        "endnote reference",
        "annotation text",
        "annotation reference",
    ];
    if LOWER_BUILTINS.contains(&internal) {
        return internal
            .split(' ')
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    internal_to_ui(internal).to_string()
}

/// Role → style resolution for one conversion.
pub(crate) struct Resolver {
    part: Option<String>,
    overrides: BTreeMap<Role, String>,
    policy: StylePolicy,
    /// Add each role's own (first-candidate) style even when a fallback
    /// exists, so a starter reference document has every style separately.
    pub(crate) prefer_own: bool,
    sample: Option<Sample>,
    pub(crate) resolved: BTreeMap<Role, RoleStyle>,
    pub(crate) missing: Vec<String>,
}

impl Resolver {
    /// `w:tblLook` flags of the reference document's representative sample
    /// table (empty before the sample is read or when it has none).
    pub(crate) fn sample_table_look(&self) -> &[(String, bool)] {
        self.sample
            .as_ref()
            .map(|sample| sample.table_look.as_slice())
            .unwrap_or(&[])
    }

    pub(crate) fn new(overrides: BTreeMap<Role, String>, policy: StylePolicy) -> Resolver {
        Resolver {
            part: None,
            overrides,
            policy,
            prefer_own: false,
            sample: None,
            resolved: BTreeMap::new(),
            missing: Vec::new(),
        }
    }

    /// Style id for `role` (`None`: the default style, or a role left
    /// missing under the strict policy — the conversion then refuses).
    pub(crate) fn id(&mut self, txn: &mut Txn<'_>, role: Role) -> Result<Option<String>> {
        if let Some(style) = self.resolved.get(&role) {
            return Ok(style.style_id.clone());
        }
        if self.missing.iter().any(|m| m.starts_with(role.key())) {
            return Ok(None);
        }
        // Body-text variants follow the body role (which may be lifted
        // from the template's sample text).
        if matches!(role, Role::FirstParagraph | Role::Compact) {
            self.id(txn, Role::Body)?;
        }
        if role == Role::TableContents {
            self.id(txn, Role::Compact)?;
            self.id(txn, Role::Table)?;
        }
        if self.sample.is_none() {
            self.sample = Some(Sample::read(txn)?);
        }
        let part = match &self.part {
            Some(part) => part.clone(),
            None => {
                let part = crate::parts::ensure_styles_part(txn)?;
                self.part = Some(part.clone());
                part
            }
        };
        let style = {
            let tree = txn.tree(&part)?;
            let default = default_style(tree, Kind::Paragraph);
            if let Some(name) = self.overrides.get(&role) {
                let Some(style) = find_style(tree, name, role.kind()) else {
                    return Err(Error::invalid_argument(
                        "mapped_style_missing",
                        format!(
                            "the style map sends {} to {name:?}, but the reference document has no {} style with that name",
                            role.key(),
                            role.kind().xml()
                        ),
                    ));
                };
                Some(found(tree, style, default))
            } else {
                let candidates = role.candidates();
                let candidates = if self.prefer_own {
                    &candidates[..1]
                } else {
                    &candidates[..]
                };
                let sample = self.sample.as_ref().expect("sample read above");
                let by_name = |names: &[String]| {
                    names
                        .iter()
                        .find_map(|name| find_style(tree, name, role.kind()))
                };
                let found_style = if role == Role::Table && !self.prefer_own {
                    // The sample tables beat the style names: Word lets nobody
                    // create a style called "Table", so a template's is often
                    // an empty leftover while its sample tables carry the look.
                    let sample_style = sample.table_style.as_deref().and_then(|id| {
                        w_children(tree, tree.root(), "style").find(|s| {
                            w_attr(tree, *s, "styleId") == Some(id)
                                && w_attr(tree, *s, "type") == Some("table")
                        })
                    });
                    match (&sample.table, sample_style) {
                        (Some(_), _) => None,
                        (None, Some(style)) => Some(style),
                        (None, None) => by_name(candidates),
                    }
                } else {
                    by_name(candidates)
                };
                // A custom paragraph style carrying the heading's outline
                // level ("Section Heading") is the template's heading.
                let found_style = found_style.or_else(|| {
                    let level = role.heading_level()?;
                    (!self.prefer_own).then_some(())?;
                    w_children(tree, tree.root(), "style").find(|style| {
                        w_attr(tree, *style, "type").unwrap_or("paragraph") == "paragraph"
                            && w_child(tree, *style, "pPr")
                                .and_then(|ppr| w_child(tree, ppr, "outlineLvl"))
                                .and_then(|o| w_attr(tree, o, "val"))
                                == Some(&(level - 1).to_string())
                    })
                });
                let lift_body = role == Role::Body && sample.body_rpr.is_some();
                match found_style {
                    Some(style) => Some(found(tree, style, default)),
                    None if lift_body && self.policy == StylePolicy::Create => None,
                    None if role.falls_back_to_default() && !self.prefer_own => Some(RoleStyle {
                        name: default
                            .map(|d| display_name(tree, d))
                            .unwrap_or_else(|| "Normal".to_string()),
                        style_id: None,
                        created: false,
                    }),
                    None => None,
                }
            }
        };
        let style = match style {
            Some(style) => style,
            None => match self.policy {
                StylePolicy::Strict => {
                    self.missing.push(format!(
                        "{} (tried {})",
                        role.key(),
                        role.candidates().join(", ")
                    ));
                    return Ok(None);
                }
                StylePolicy::Create => {
                    let body = match role.kind() {
                        Kind::Paragraph
                            if role.heading_level().is_none()
                                && !matches!(role, Role::Title | Role::Subtitle | Role::Body) =>
                        {
                            self.id(txn, Role::Body)?
                        }
                        _ => None,
                    };
                    let next = match role {
                        Role::Title | Role::Subtitle | Role::TocHeading => {
                            self.id(txn, Role::Body)?
                        }
                        heading if heading.heading_level().is_some() => self.id(txn, Role::Body)?,
                        _ => None,
                    };
                    let sample = self.sample.as_ref().expect("sample read above");
                    let mut body = body;
                    let lifted = match role {
                        Role::Body => sample.body_rpr.clone().map(|rpr| Lifted {
                            rpr,
                            ..Lifted::default()
                        }),
                        Role::Table => sample.table.clone().map(|mut lifted| {
                            let tree = txn.tree(&part).ok();
                            let exists = |name: &str| {
                                tree.is_some_and(|tree| {
                                    find_style(tree, name, Kind::Table).is_some()
                                })
                            };
                            if exists("Table") {
                                let name = (1..)
                                    .map(|n| match n {
                                        1 => "Template Table".to_string(),
                                        n => format!("Template Table {n}"),
                                    })
                                    .find(|name| !exists(name))
                                    .expect("an unused style name");
                                lifted.name = Some(name);
                            }
                            lifted
                        }),
                        Role::TableContents => {
                            body = self
                                .resolved
                                .get(&Role::Compact)
                                .and_then(|c| c.style_id.clone())
                                .or(body);
                            let table = self
                                .resolved
                                .get(&Role::Table)
                                .and_then(|t| t.style_id.clone());
                            Some(Lifted {
                                ppr: table_cell_spacing(txn.tree(&part)?, table.as_deref()),
                                ..Lifted::default()
                            })
                        }
                        heading if heading.heading_level().is_some() => heading
                            .heading_level()
                            .and_then(|level| sample.headings.get(&level).cloned()),
                        // Word's own TOC Heading: Heading 1's look, kept out
                        // of the contents.
                        Role::TocHeading if !self.prefer_own => {
                            let heading = self.id(txn, Role::heading(1))?;
                            heading.map(|heading| Lifted {
                                ppr: "<w:outlineLvl w:val=\"9\"/>".to_string(),
                                based_on: Some(heading),
                                ..Lifted::default()
                            })
                        }
                        // A deeper contents level follows the first, so
                        // the whole contents share the template's look.
                        toc if toc.toc_level().is_some_and(|level| level > 1) => {
                            let level = i64::from(toc.toc_level().unwrap_or(2));
                            let first = self.id(txn, Role::Toc1)?;
                            match first {
                                Some(first) => {
                                    let left = style_indent_left(txn.tree(&part)?, &first);
                                    Some(Lifted {
                                        ppr: format!(
                                            "<w:ind w:left=\"{}\"/>",
                                            left + 220 * (level - 1)
                                        ),
                                        based_on: Some(first),
                                        ..Lifted::default()
                                    })
                                }
                                None => None,
                            }
                        }
                        _ => None,
                    };
                    // Headings, titles and contents entries normally sit on
                    // the default style; when the body font had to be lifted
                    // from the template's sample text they take it too.
                    let lifted_body = self
                        .resolved
                        .get(&Role::Body)
                        .filter(|b| b.created)
                        .and_then(|b| b.style_id.clone());
                    if role.toc_level().is_some() {
                        body = lifted_body.clone();
                    }
                    if role.heading_level().is_some()
                        || matches!(role, Role::Title | Role::Subtitle | Role::TocHeading)
                    {
                        self.id(txn, Role::Body)?;
                        body = self
                            .resolved
                            .get(&Role::Body)
                            .filter(|b| b.created)
                            .and_then(|b| b.style_id.clone());
                    }
                    create(txn, &part, role, body, next, lifted)?
                }
            },
        };
        // An existing heading style whose sample paragraphs all show
        // another look by hand takes that look, so the converted headings
        // look like the template's own.
        if let (Some(level), Some(id), false) =
            (role.heading_level(), style.style_id.clone(), style.created)
        {
            let look = self
                .sample
                .as_ref()
                .and_then(|sample| sample.heading_looks.get(&level).cloned());
            if let Some(look) = look.filter(|_| !self.prefer_own) {
                restyle(txn, &part, &id, &look)?;
            }
        }
        let id = style.style_id.clone();
        self.resolved.insert(role, style);
        Ok(id)
    }
}

/// `CT_Style` child order.
const STYLE_SEQ: &[&str] = &[
    "name",
    "aliases",
    "basedOn",
    "next",
    "link",
    "autoRedefine",
    "hidden",
    "uiPriority",
    "semiHidden",
    "unhideWhenUsed",
    "qFormat",
    "locked",
    "personal",
    "personalCompose",
    "personalReply",
    "rsid",
    "pPr",
    "rPr",
    "tblPr",
    "trPr",
    "tcPr",
    "tblStylePr",
];

fn restyle(txn: &mut Txn<'_>, part: &str, id: &str, look: &str) -> Result<()> {
    let tree = txn.tree_mut(part)?;
    let root = tree.root();
    let Some(style) =
        w_children(tree, root, "style").find(|s| w_attr(tree, *s, "styleId") == Some(id))
    else {
        return Ok(());
    };
    let rpr = crate::oxml::get_or_add(tree, style, "rPr", STYLE_SEQ)?;
    let wrapper = import_fragment(tree, rpr, &format!("<w:rPr xmlns:w=\"{W}\">{look}</w:rPr>"))?;
    for child in tree.child_elements(wrapper).collect::<Vec<_>>() {
        let local = tree.local_name(child).unwrap_or("").to_string();
        if let Some(old) = w_child(tree, rpr, &local) {
            crate::authoring::remove(tree, old)?;
        }
        tree.detach(child)?;
        crate::oxml::insert_before_successors(
            tree,
            rpr,
            child,
            crate::oxml::successors(crate::oxml::RPR_SEQ, &local),
        )?;
    }
    Ok(())
}

fn found(tree: &XmlTree, style: NodeId, default: Option<NodeId>) -> RoleStyle {
    RoleStyle {
        name: display_name(tree, style),
        style_id: if Some(style) == default {
            None
        } else {
            w_attr(tree, style, "styleId").map(str::to_string)
        },
        created: false,
    }
}

/// A style body taken from the reference document's sample content:
/// `pPr`, `rPr` and table-style children, as WordprocessingML.
#[derive(Debug, Clone, Default)]
pub(crate) struct Lifted {
    pub ppr: String,
    pub rpr: String,
    pub extra: String,
    /// Style name to create under instead of the role's own.
    pub name: Option<String>,
    /// Style to base the creation on instead of the role's default.
    pub based_on: Option<String>,
}

/// Branding the reference document shows only in its sample body (the
/// body the conversion replaces): headings made with direct outline
/// levels, body text formatted by hand, the sample table's style or its
/// direct formatting. Read from the document as opened.
#[derive(Debug, Default)]
pub(crate) struct Sample {
    headings: BTreeMap<u8, Lifted>,
    /// The run look at least 60% of a Heading N style's sample paragraphs set
    /// by hand: what the template shows for that heading, whatever its style
    /// says.
    heading_looks: BTreeMap<u8, String>,
    body_rpr: Option<String>,
    table_style: Option<String>,
    table: Option<Lifted>,
    table_look: Vec<(String, bool)>,
}

fn w_xml(tree: &XmlTree, node: NodeId, skip: &[&str]) -> String {
    let mut out = String::new();
    for child in tree.child_elements(node) {
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        let local = tree.local_name(child).unwrap_or("");
        if skip.contains(&local) {
            continue;
        }
        write_w(tree, child, &mut out);
    }
    out
}

fn write_w(tree: &XmlTree, node: NodeId, out: &mut String) {
    let local = tree.local_name(node).unwrap_or("");
    out.push_str(&format!("<w:{local}"));
    if let Some(element) = tree.element(node) {
        for attr in &element.attrs {
            if attr.ns.map(|ns| tree.uri(ns)) == Some(W) {
                out.push_str(&format!(
                    " w:{}=\"{}\"",
                    attr.name.local,
                    xml_escape(&attr.value)
                ));
            }
        }
    }
    let children: Vec<NodeId> = tree
        .child_elements(node)
        .filter(|c| tree.ns_uri(*c) == Some(W))
        .collect();
    if children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in children {
        write_w(tree, child, out);
    }
    out.push_str(&format!("</w:{local}>"));
}

const LOOK_RPR: &[&str] = &[
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "color",
    "sz",
    "szCs",
    "u",
];

/// Emphasis a sample sentence may carry that is not the body look.
const EMPHASIS_RPR: &[&str] = &["b", "bCs", "i", "iCs", "u"];

fn look_rpr(tree: &XmlTree, rpr: NodeId, skip: &[&str]) -> Option<String> {
    let mut out = String::new();
    for child in tree.child_elements(rpr) {
        let local = tree.local_name(child).unwrap_or("");
        if tree.ns_uri(child) == Some(W) && LOOK_RPR.contains(&local) && !skip.contains(&local) {
            write_w(tree, child, &mut out);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn describe_rpr(rpr: &str) -> Value {
    let attr = |element: &str, name: &str| -> Option<String> {
        let start = rpr.find(&format!("<w:{element} "))?;
        let tag = &rpr[start..start + rpr[start..].find('>')?];
        let key = format!(" w:{name}=\"");
        let at = tag.find(&key)? + key.len();
        Some(tag[at..at + tag[at..].find('"')?].to_string())
    };
    let toggle = |element: &str| -> Value {
        if rpr.contains(&format!("<w:{element}/>")) {
            return Value::Bool(true);
        }
        match attr(element, "val") {
            Some(val) => Value::Bool(!matches!(val.as_str(), "0" | "false" | "off")),
            None => Value::Null,
        }
    };
    serde_json::json!({
        "font": attr("rFonts", "ascii"),
        "size_pt": attr("sz", "val").and_then(|v| v.parse::<f64>().ok()).map(|half| half / 2.0),
        "color": attr("color", "val"),
        "bold": toggle("b"),
        "italic": toggle("i"),
    })
}

impl Sample {
    /// What a conversion lifts from the sample content, for a report.
    pub(crate) fn summary(&self) -> Value {
        serde_json::json!({
            "body": self.body_rpr.as_deref().map(describe_rpr),
            "headings": self
                .headings
                .iter()
                .map(|(level, lifted)| (*level, lifted.rpr.as_str()))
                .chain(self.heading_looks.iter().map(|(level, look)| (*level, look.as_str())))
                .map(|(level, rpr)| (format!("heading_{level}"), describe_rpr(rpr)))
                .collect::<serde_json::Map<String, Value>>(),
            "table": {
                "style_id": self.table_style,
                "direct_formatting_lifted": self.table.is_some(),
                "look": self
                    .table_look
                    .iter()
                    .map(|(flag, on)| (flag.clone(), Value::Bool(*on)))
                    .collect::<serde_json::Map<String, Value>>(),
            },
        })
    }

    pub(crate) fn read(txn: &Txn<'_>) -> Result<Sample> {
        let package = txn.document().package();
        let tree = package.xml(package.main_part_name())?;
        let mut sample = Sample::default();
        let Some(body) = w_child(tree, tree.root(), "body") else {
            return Ok(sample);
        };
        let mut body_candidates: Vec<String> = Vec::new();
        let mut tables: Vec<SampleTable> = Vec::new();
        let mut heading_styles: BTreeMap<String, u8> = BTreeMap::new();
        if let Some(styles) =
            package.part_related_by(package.main_part_name(), crate::package::rels::rt::STYLES)?
        {
            let styles = package.xml(&styles)?;
            for style in w_children(styles, styles.root(), "style") {
                let Some(id) = w_attr(styles, style, "styleId") else {
                    continue;
                };
                let by_name = w_child(styles, style, "name")
                    .and_then(|n| w_attr(styles, n, "val"))
                    .and_then(|name| {
                        name.to_ascii_lowercase()
                            .strip_prefix("heading ")
                            .and_then(|n| n.trim().parse::<u8>().ok())
                    });
                let by_outline = w_child(styles, style, "pPr")
                    .and_then(|p| w_child(styles, p, "outlineLvl"))
                    .and_then(|o| w_attr(styles, o, "val"))
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|level| *level < 9)
                    .map(|level| level + 1);
                if let Some(level) = by_name.or(by_outline).filter(|l| (1..=9).contains(l)) {
                    heading_styles.insert(id.to_string(), level);
                }
            }
        }
        let mut heading_runs: BTreeMap<u8, Vec<String>> = BTreeMap::new();
        for block in tree.child_elements(body) {
            if tree.is(block, W, "tbl") {
                tables.push(read_table(tree, block));
                continue;
            }
            if !tree.is(block, W, "p") {
                continue;
            }
            let ppr = w_child(tree, block, "pPr");
            let styled = ppr.and_then(|p| w_child(tree, p, "pStyle")).is_some();
            let listed = ppr.and_then(|p| w_child(tree, p, "numPr")).is_some();
            let first_rpr = w_children(tree, block, "r")
                .find(|r| !crate::authoring::run_text(tree, *r).trim().is_empty())
                .and_then(|r| w_child(tree, r, "rPr"));
            let outline = ppr
                .and_then(|p| w_child(tree, p, "outlineLvl"))
                .and_then(|o| w_attr(tree, o, "val"))
                .and_then(|v| v.parse::<u8>().ok());
            let heading_level = ppr
                .and_then(|p| w_child(tree, p, "pStyle"))
                .and_then(|s| w_attr(tree, s, "val"))
                .and_then(|id| heading_styles.get(id).copied());
            if let (Some(level), true) = (heading_level, outline.is_none()) {
                let has_text = w_children(tree, block, "r")
                    .any(|r| !crate::authoring::run_text(tree, r).trim().is_empty());
                if has_text {
                    heading_runs.entry(level).or_default().push(
                        first_rpr
                            .and_then(|r| look_rpr(tree, r, &[]))
                            .unwrap_or_default(),
                    );
                }
                continue;
            }
            match outline {
                Some(level) if level < 9 => {
                    sample.headings.entry(level + 1).or_insert_with(|| Lifted {
                        ppr: ppr
                            .map(|p| {
                                w_xml(tree, p, &["pStyle", "numPr", "rPr", "sectPr", "pPrChange"])
                            })
                            .unwrap_or_default(),
                        rpr: first_rpr
                            .and_then(|r| look_rpr(tree, r, &[]))
                            .unwrap_or_default(),
                        ..Lifted::default()
                    });
                }
                None if !styled && !listed => {
                    // Text over 14 pt or bold is a title, not body text.
                    let candidate = first_rpr.filter(|r| {
                        let size = w_child(tree, *r, "sz")
                            .and_then(|s| w_attr(tree, s, "val"))
                            .and_then(|v| v.parse::<i64>().ok());
                        let has = |l: &str| w_child(tree, *r, l).is_some();
                        (has("rFonts") || has("sz") || has("color"))
                            && size.is_none_or(|s| s <= 28)
                            && !has("b")
                    });
                    if let Some(rpr) = candidate.and_then(|r| look_rpr(tree, r, EMPHASIS_RPR)) {
                        body_candidates.push(rpr);
                    }
                }
                _ => {}
            }
        }
        // Among the tables of the commonest style, the group sharing a header
        // look wins: layout tables and one-offs lose to the data tables.
        let style = most_common(tables.iter().map(|t| t.style.clone()));
        if let Some(style) = style {
            let same_style: Vec<&SampleTable> =
                tables.iter().filter(|t| t.style == style).collect();
            let header = most_common(
                same_style
                    .iter()
                    .filter(|t| t.rows >= 2)
                    .map(|t| t.header.clone()),
            );
            let group: Vec<&SampleTable> = match &header {
                Some(header) => same_style
                    .iter()
                    .copied()
                    .filter(|t| t.rows >= 2 && t.header == *header)
                    .collect(),
                None => same_style,
            };
            if let Some(representative) = group.first() {
                sample.table_look = representative.look.clone();
                sample.table = lift_table(&group).map(|mut lifted| {
                    lifted.based_on = style.clone();
                    lifted
                });
                sample.table_style = style;
            }
        }
        for (level, looks) in heading_runs {
            if let Some(look) = most_common(looks.iter().cloned()) {
                let count = looks.iter().filter(|l| **l == look).count();
                if !look.is_empty() && count * 5 >= looks.len() * 3 {
                    sample.heading_looks.insert(level, look);
                }
            }
        }
        sample.body_rpr = body_candidates
            .iter()
            .max_by_key(|candidate| {
                let count = body_candidates.iter().filter(|c| c == candidate).count();
                let first = body_candidates
                    .iter()
                    .position(|c| c == *candidate)
                    .unwrap_or(0);
                (count, std::cmp::Reverse(first))
            })
            .cloned();
        Ok(sample)
    }
}

#[derive(Debug, Clone)]
struct SampleTable {
    style: Option<String>,
    props: String,
    rows: usize,
    header: (String, String),
    /// `(cell look, run look)` of every body cell (rows after the first).
    body_cells: Vec<(String, String)>,
    /// First-cell looks of rows 1 and 2 (banding).
    row_firsts: Vec<(String, String)>,
    look: Vec<(String, bool)>,
}

fn most_common<T: PartialEq + Clone>(values: impl Iterator<Item = T>) -> Option<T> {
    let values: Vec<T> = values.collect();
    values
        .iter()
        .enumerate()
        .max_by_key(|(index, value)| {
            let count = values.iter().filter(|v| v == value).count();
            (count, std::cmp::Reverse(*index))
        })
        .map(|(_, value)| value.clone())
}

/// `w:tblLook` flags of a table, from the attributes Word 2010 writes or
/// else decoded from the older hexadecimal `w:val`.
fn table_look(tree: &XmlTree, tbl: NodeId) -> Vec<(String, bool)> {
    const FLAGS: [(&str, u32); 6] = [
        ("firstRow", 0x20),
        ("lastRow", 0x40),
        ("firstColumn", 0x80),
        ("lastColumn", 0x100),
        ("noHBand", 0x200),
        ("noVBand", 0x400),
    ];
    let Some(look) = w_child(tree, tbl, "tblPr").and_then(|p| w_child(tree, p, "tblLook")) else {
        return Vec::new();
    };
    let bits = w_attr(tree, look, "val").and_then(|v| u32::from_str_radix(v, 16).ok());
    FLAGS
        .iter()
        .filter_map(|(name, bit)| {
            let on = match w_attr(tree, look, name) {
                Some(value) => Some(matches!(value, "1" | "true" | "on")),
                None => bits.map(|bits| bits & bit != 0),
            }?;
            Some((name.to_string(), on))
        })
        .collect()
}

const LOOK_TCPR: &[&str] = &["tcBorders", "shd", "tcMar"];

fn row_look(tree: &XmlTree, tr: NodeId) -> (String, String) {
    match w_child(tree, tr, "tc") {
        Some(tc) => cell_look(tree, tc),
        None => (String::new(), String::new()),
    }
}

/// `(cell look, run look)`: borders, shading and margins; font, colour and
/// emphasis of the first run (the paragraph mark's when the cell is empty).
fn cell_look(tree: &XmlTree, tc: NodeId) -> (String, String) {
    let mut cell = String::new();
    if let Some(tc_pr) = w_child(tree, tc, "tcPr") {
        for child in tree.child_elements(tc_pr) {
            let local = tree.local_name(child).unwrap_or("");
            if tree.ns_uri(child) == Some(W) && LOOK_TCPR.contains(&local) {
                write_w(tree, child, &mut cell);
            }
        }
    }
    let paragraphs: Vec<NodeId> = w_children(tree, tc, "p").collect();
    let run = paragraphs
        .iter()
        .flat_map(|p| w_children(tree, *p, "r").collect::<Vec<_>>())
        .find(|r| !crate::authoring::run_text(tree, *r).trim().is_empty())
        .and_then(|r| w_child(tree, r, "rPr"))
        .or_else(|| {
            paragraphs
                .first()
                .and_then(|p| w_child(tree, *p, "pPr"))
                .and_then(|p| w_child(tree, p, "rPr"))
        });
    let run = run.and_then(|r| look_rpr(tree, r, &[])).unwrap_or_default();
    (cell, run)
}

fn carries_look(xml: &str) -> bool {
    let value = |element: &str, attr: &str| {
        let key = format!("w:{attr}=\"");
        element
            .find(&key)
            .map(|at| &element[at + key.len()..])
            .and_then(|rest| rest.split('"').next())
            .map(str::to_string)
    };
    xml.split('<').any(|element| {
        let local = element
            .trim_start_matches("w:")
            .split([' ', '/', '>'])
            .next()
            .unwrap_or("");
        match local {
            "shd" => value(element, "fill").is_some_and(|fill| {
                !matches!(fill.to_ascii_uppercase().as_str(), "AUTO" | "FFFFFF")
            }),
            "color" => value(element, "val").is_some_and(|val| !val.eq_ignore_ascii_case("auto")),
            "b" => value(element, "val").is_none_or(|val| !matches!(val.as_str(), "0" | "false")),
            "top" | "left" | "bottom" | "right" | "start" | "end" | "insideH" | "insideV" => {
                let val = value(element, "val").unwrap_or_default();
                let size = value(element, "sz").and_then(|sz| sz.parse::<u32>().ok());
                !matches!(val.as_str(), "nil" | "none" | "") && size.is_none_or(|sz| sz > 0)
            }
            _ => false,
        }
    })
}

fn read_table(tree: &XmlTree, tbl: NodeId) -> SampleTable {
    let tbl_pr = w_child(tree, tbl, "tblPr");
    let style = tbl_pr
        .and_then(|p| w_child(tree, p, "tblStyle"))
        .and_then(|s| w_attr(tree, s, "val"))
        .map(str::to_string);
    let mut props = String::new();
    for child in tbl_pr.into_iter().flat_map(|p| tree.child_elements(p)) {
        let local = tree.local_name(child).unwrap_or("");
        if tree.ns_uri(child) == Some(W) && matches!(local, "tblBorders" | "shd" | "tblCellMar") {
            write_w(tree, child, &mut props);
        }
    }
    let rows: Vec<NodeId> = w_children(tree, tbl, "tr").collect();
    let header = rows
        .first()
        .map(|tr| row_look(tree, *tr))
        .unwrap_or_default();
    let body_cells = rows
        .iter()
        .skip(1)
        .flat_map(|tr| w_children(tree, *tr, "tc").collect::<Vec<_>>())
        .map(|tc| cell_look(tree, tc))
        .collect();
    let row_firsts = rows
        .iter()
        .skip(1)
        .take(2)
        .map(|tr| row_look(tree, *tr))
        .collect();
    SampleTable {
        style,
        props,
        rows: rows.len(),
        header,
        body_cells,
        row_firsts,
        look: table_look(tree, tbl),
    }
}

fn border_sides(xml: &str, container: &str) -> Vec<(String, String)> {
    let open = format!("<w:{container}>");
    let close = format!("</w:{container}>");
    let Some(start) = xml.find(&open) else {
        return Vec::new();
    };
    let inner = &xml[start + open.len()..];
    let inner = &inner[..inner.find(&close).unwrap_or(inner.len())];
    inner
        .split("<w:")
        .filter(|piece| !piece.is_empty())
        .filter_map(|piece| {
            let side = piece.split([' ', '/', '>']).next()?.to_string();
            let attrs = piece.split_once(' ').map_or("", |(_, rest)| rest);
            let attrs = attrs.trim_end_matches('>').trim_end_matches('/').trim();
            Some((side, attrs.to_string()))
        })
        .collect()
}

/// Word draws a table style's row and column lines from `tblBorders`
/// (`insideH`, `insideV` and the outer sides), not from a whole-table
/// `tcBorders`.
fn cell_borders_as_table_borders(cell: &str, table: &str) -> String {
    let mut sides: Vec<(String, String)> = border_sides(table, "tblBorders");
    let mut set = |side: &str, attrs: &str| {
        sides.retain(|(s, _)| s != side);
        sides.push((side.to_string(), attrs.to_string()));
    };
    for (side, attrs) in border_sides(cell, "tcBorders") {
        match side.as_str() {
            "bottom" => {
                set("bottom", &attrs);
                set("insideH", &attrs);
            }
            "top" => {
                set("top", &attrs);
                set("insideH", &attrs);
            }
            "left" | "start" => {
                set("left", &attrs);
                set("insideV", &attrs);
            }
            "right" | "end" => {
                set("right", &attrs);
                set("insideV", &attrs);
            }
            _ => {}
        }
    }
    if sides.is_empty() {
        return String::new();
    }
    // CT_TblBorders order.
    let order = [
        "top", "left", "start", "bottom", "right", "end", "insideH", "insideV",
    ];
    sides.sort_by_key(|(side, _)| order.iter().position(|o| o == side).unwrap_or(order.len()));
    let mut out = String::from("<w:tblBorders>");
    for (side, attrs) in sides {
        out.push_str(&format!("<w:{side} {attrs}/>"));
    }
    out.push_str("</w:tblBorders>");
    out
}

/// The look a group of alike sample tables adds by hand; `None` when nothing
/// visible is added.
fn lift_table(group: &[&SampleTable]) -> Option<Lifted> {
    let representative = group.first()?;
    let mut props = representative.props.clone();
    let header = &representative.header;
    let body_cell = most_common(
        group
            .iter()
            .flat_map(|t| t.body_cells.iter().map(|c| c.0.clone())),
    );
    let body_run = most_common(group.iter().flat_map(|t| {
        t.body_cells
            .iter()
            .filter(|c| !c.1.is_empty())
            .map(|c| c.1.clone())
    }));
    // Shading alone separates banded rows; borders and fonts are shared.
    let shading = |cell: &str| {
        cell.find("<w:shd ")
            .map(|at| cell[at..at + cell[at..].find("/>").unwrap_or(0) + 2].to_string())
    };
    let without_shading = |cell: &str| match shading(cell) {
        Some(shd) => cell.replacen(&shd, "", 1),
        None => cell.to_string(),
    };
    let (whole_tcpr, whole_rpr, conditional) = match (body_cell, body_run) {
        (None, _) => (header.0.clone(), header.1.clone(), String::new()),
        (Some(body_cell), body_run) => {
            let body_run = body_run.unwrap_or_default();
            let mut conditional = String::new();
            let firsts = &representative.row_firsts;
            let banded = firsts.len() == 2
                && shading(&firsts[1].0) != shading(&firsts[0].0)
                && without_shading(&firsts[1].0) == without_shading(&firsts[0].0);
            let whole_tcpr = if banded {
                for (kind, row) in [("band1Horz", &firsts[0].0), ("band2Horz", &firsts[1].0)] {
                    if let Some(shd) = shading(row) {
                        conditional.push_str(&format!(
                            "<w:tblStylePr w:type=\"{kind}\"><w:tblPr/><w:tcPr>{shd}</w:tcPr></w:tblStylePr>"
                        ));
                    }
                }
                without_shading(&body_cell)
            } else {
                body_cell.clone()
            };
            if header.0 != body_cell || header.1 != body_run {
                conditional.push_str("<w:tblStylePr w:type=\"firstRow\">");
                if !header.1.is_empty() {
                    conditional.push_str(&format!("<w:rPr>{}</w:rPr>", header.1));
                }
                conditional.push_str("<w:tblPr/>");
                if !header.0.is_empty() {
                    conditional.push_str(&format!("<w:tcPr>{}</w:tcPr>", header.0));
                }
                conditional.push_str("</w:tblStylePr>");
            }
            (whole_tcpr, body_run, conditional)
        }
    };
    let whole_tcpr = if whole_tcpr.contains("<w:tcBorders>") {
        let table_borders = cell_borders_as_table_borders(&whole_tcpr, &props);
        if let Some(start) = props.find("<w:tblBorders>") {
            let end = props
                .find("</w:tblBorders>")
                .map_or(props.len(), |e| e + 15);
            props.replace_range(start..end, "");
        }
        props.insert_str(0, &table_borders);
        let start = whole_tcpr.find("<w:tcBorders>").unwrap_or(0);
        let end = whole_tcpr
            .find("</w:tcBorders>")
            .map_or(whole_tcpr.len(), |e| e + 14);
        let mut rest = whole_tcpr.clone();
        rest.replace_range(start..end, "");
        rest
    } else {
        whole_tcpr
    };
    // Word reads a table style's cell padding from `tblCellMar`, not `tcMar`.
    let whole_tcpr = match (whole_tcpr.find("<w:tcMar>"), whole_tcpr.find("</w:tcMar>")) {
        (Some(start), Some(end)) => {
            let end = end + "</w:tcMar>".len();
            let margins =
                whole_tcpr[start + "<w:tcMar>".len()..end - "</w:tcMar>".len()].to_string();
            if let Some(old) = props.find("<w:tblCellMar>") {
                let old_end = props[old..]
                    .find("</w:tblCellMar>")
                    .map_or(props.len(), |e| old + e + "</w:tblCellMar>".len());
                props.replace_range(old..old_end, "");
            }
            props.push_str(&format!("<w:tblCellMar>{margins}</w:tblCellMar>"));
            let mut rest = whole_tcpr.clone();
            rest.replace_range(start..end, "");
            rest
        }
        _ => whole_tcpr,
    };
    // CT_TblPrBase order: tblStyleRowBandSize precedes tblBorders, shd and tblCellMar.
    if conditional.contains("Horz\"") {
        props.insert_str(0, "<w:tblStyleRowBandSize w:val=\"1\"/>");
    }
    let mut extra = String::new();
    if !props.is_empty() {
        extra.push_str(&format!("<w:tblPr>{props}</w:tblPr>"));
    }
    if !whole_tcpr.is_empty() {
        extra.push_str(&format!("<w:tcPr>{whole_tcpr}</w:tcPr>"));
    }
    extra.push_str(&conditional);
    // Only a visible addition is worth a style of its own (Pandoc's
    // zero-width header rule, for instance, is not).
    if !carries_look(&extra) && !carries_look(&whole_rpr) {
        return None;
    }
    Some(Lifted {
        rpr: whole_rpr,
        extra,
        ..Lifted::default()
    })
}

/// Header-row text colour from `table_style` (or a style it is based on)
/// that the cell paragraph style would hide: in Word a paragraph style's
/// colour beats a table style's, so white header text set by the table
/// style turns body-coloured on the header fill unless written on the runs.
pub(crate) fn header_color_override(
    tree: &XmlTree,
    table_style: Option<&str>,
    cell_style: Option<&str>,
) -> Option<String> {
    let by_id = |id: &str| {
        w_children(tree, tree.root(), "style").find(|s| w_attr(tree, *s, "styleId") == Some(id))
    };
    let chain = |start: Option<NodeId>| {
        let mut out = Vec::new();
        let mut current = start;
        while let Some(style) = current {
            if out.contains(&style) || out.len() > 16 {
                break;
            }
            out.push(style);
            current = w_child(tree, style, "basedOn")
                .and_then(|b| w_attr(tree, b, "val"))
                .and_then(by_id);
        }
        out
    };
    let color = |rpr: Option<NodeId>| {
        rpr.and_then(|r| w_child(tree, r, "color"))
            .and_then(|c| w_attr(tree, c, "val"))
            .map(str::to_string)
    };
    let header = chain(table_style.and_then(by_id))
        .into_iter()
        .find_map(|style| {
            w_children(tree, style, "tblStylePr")
                .find(|c| w_attr(tree, *c, "type") == Some("firstRow"))
                .and_then(|c| color(w_child(tree, c, "rPr")))
        })?;
    let cell_start = cell_style
        .and_then(by_id)
        .or_else(|| default_style(tree, Kind::Paragraph));
    let paragraph = chain(cell_start)
        .into_iter()
        .find_map(|style| color(w_child(tree, style, "rPr")))?;
    (!paragraph.eq_ignore_ascii_case(&header)).then_some(header)
}

fn table_cell_spacing(tree: &XmlTree, table_style: Option<&str>) -> String {
    let style = table_style.and_then(|id| {
        w_children(tree, tree.root(), "style").find(|s| w_attr(tree, *s, "styleId") == Some(id))
    });
    let spacing = style.and_then(|style| {
        let whole = w_children(tree, style, "tblStylePr")
            .find(|c| w_attr(tree, *c, "type") == Some("wholeTable"))
            .and_then(|c| w_child(tree, c, "pPr"))
            .and_then(|p| w_child(tree, p, "spacing"));
        whole.or_else(|| w_child(tree, style, "pPr").and_then(|p| w_child(tree, p, "spacing")))
    });
    let mut out = String::new();
    match spacing {
        Some(spacing) => write_w(tree, spacing, &mut out),
        None => out.push_str(
            "<w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>",
        ),
    }
    out.push_str("<w:contextualSpacing/>");
    out
}

/// Name a created style is written under (Word's internal name for
/// built-ins, so Word recognizes them).
fn internal_name(role: Role) -> String {
    match role {
        Role::FootnoteText => "footnote text".to_string(),
        Role::FootnoteReference => "footnote reference".to_string(),
        toc if toc.toc_level().is_some() => format!("toc {}", toc.toc_level().unwrap_or(1)),
        other => ui_to_internal(&other.candidates()[0]).to_string(),
    }
}

fn unique_id(tree: &XmlTree, base: &str) -> String {
    let taken = |id: &str| {
        w_children(tree, tree.root(), "style").any(|s| w_attr(tree, s, "styleId") == Some(id))
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|id| !taken(id))
        .expect("an unused style id")
}

/// Formatting of a created definition: `(pPr, rPr, table extras)` bodies,
/// children in schema order.
fn definition(role: Role) -> (&'static str, &'static str, &'static str) {
    match role {
        Role::Title => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"480\" w:after=\"240\"/><w:jc w:val=\"center\"/>",
            "<w:b/><w:bCs/><w:color w:val=\"17365D\"/><w:sz w:val=\"36\"/><w:szCs w:val=\"36\"/>",
            "",
        ),
        Role::Subtitle => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"240\" w:after=\"240\"/><w:jc w:val=\"center\"/>",
            "<w:color w:val=\"17365D\"/><w:sz w:val=\"30\"/><w:szCs w:val=\"30\"/>",
            "",
        ),
        Role::Author | Role::Date => (
            "<w:keepNext/><w:keepLines/><w:jc w:val=\"center\"/>",
            "",
            "",
        ),
        Role::Abstract => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"300\" w:after=\"300\"/><w:ind w:left=\"720\" w:right=\"720\"/>",
            "<w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>",
            "",
        ),
        Role::Heading1 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"480\" w:after=\"0\"/><w:outlineLvl w:val=\"0\"/>",
            "<w:b/><w:bCs/><w:color w:val=\"2F5496\"/><w:sz w:val=\"32\"/><w:szCs w:val=\"32\"/>",
            "",
        ),
        Role::Heading2 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"1\"/>",
            "<w:b/><w:bCs/><w:color w:val=\"2F5496\"/><w:sz w:val=\"28\"/><w:szCs w:val=\"28\"/>",
            "",
        ),
        Role::Heading3 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"2\"/>",
            "<w:b/><w:bCs/><w:color w:val=\"2F5496\"/><w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/>",
            "",
        ),
        Role::Heading4 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"3\"/>",
            "<w:b/><w:bCs/><w:i/><w:iCs/><w:color w:val=\"2F5496\"/>",
            "",
        ),
        Role::Heading5 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"4\"/>",
            "<w:i/><w:iCs/><w:color w:val=\"2F5496\"/>",
            "",
        ),
        Role::Heading6 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"5\"/>",
            "<w:color w:val=\"1F3763\"/>",
            "",
        ),
        Role::Heading7 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"6\"/>",
            "<w:i/><w:iCs/><w:color w:val=\"1F3763\"/>",
            "",
        ),
        Role::Heading8 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"7\"/>",
            "<w:color w:val=\"272727\"/><w:sz w:val=\"21\"/><w:szCs w:val=\"21\"/>",
            "",
        ),
        Role::Heading9 => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"200\" w:after=\"0\"/><w:outlineLvl w:val=\"8\"/>",
            "<w:i/><w:iCs/><w:color w:val=\"272727\"/><w:sz w:val=\"21\"/><w:szCs w:val=\"21\"/>",
            "",
        ),
        Role::BlockQuote => (
            "<w:pBdr><w:left w:val=\"single\" w:sz=\"18\" w:space=\"8\" w:color=\"BFBFBF\"/></w:pBdr><w:spacing w:before=\"100\" w:after=\"100\"/><w:ind w:left=\"480\" w:right=\"480\"/>",
            "<w:color w:val=\"404040\"/>",
            "",
        ),
        Role::CodeBlock => (
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F5F5F5\"/><w:spacing w:before=\"0\" w:after=\"200\" w:line=\"240\" w:lineRule=\"auto\"/>",
            "<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/><w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>",
            "",
        ),
        Role::InlineCode => (
            "",
            "<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/><w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F2F2F2\"/>",
            "",
        ),
        Role::Hyperlink => ("", "<w:color w:val=\"0563C1\"/><w:u w:val=\"single\"/>", ""),
        Role::FootnoteText => (
            "<w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>",
            "<w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>",
            "",
        ),
        Role::FootnoteReference => ("", "<w:vertAlign w:val=\"superscript\"/>", ""),
        Role::DefinitionTerm => (
            "<w:keepNext/><w:keepLines/><w:spacing w:after=\"0\"/>",
            "<w:b/><w:bCs/>",
            "",
        ),
        Role::Definition => ("<w:ind w:left=\"720\"/>", "", ""),
        Role::Figure => ("<w:keepNext/><w:jc w:val=\"center\"/>", "", ""),
        Role::ImageCaption | Role::TableCaption => (
            "<w:spacing w:before=\"0\" w:after=\"200\"/><w:jc w:val=\"center\"/>",
            "<w:i/><w:iCs/><w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>",
            "",
        ),
        Role::TocHeading => (
            "<w:keepNext/><w:keepLines/><w:spacing w:before=\"480\" w:after=\"120\"/>",
            "<w:b/><w:bCs/><w:color w:val=\"2F5496\"/><w:sz w:val=\"32\"/><w:szCs w:val=\"32\"/>",
            "",
        ),
        Role::Table => (
            "",
            "",
            "<w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"8\" w:space=\"0\" w:color=\"404040\"/>\
             <w:bottom w:val=\"single\" w:sz=\"8\" w:space=\"0\" w:color=\"404040\"/>\
             <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BFBFBF\"/></w:tblBorders>\
             <w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr>\
             <w:tblStylePr w:type=\"firstRow\"><w:rPr><w:b/><w:bCs/></w:rPr><w:tblPr/><w:tcPr><w:tcBorders>\
             <w:bottom w:val=\"single\" w:sz=\"8\" w:space=\"0\" w:color=\"404040\"/></w:tcBorders></w:tcPr></w:tblStylePr>",
        ),
        Role::Compact => ("<w:spacing w:before=\"36\" w:after=\"36\"/>", "", ""),
        Role::Body | Role::FirstParagraph => ("", "", ""),
        // Spacing comes from the table style (see `table_cell_spacing`).
        Role::TableContents => ("", "", ""),
        Role::Toc1 => ("<w:spacing w:after=\"100\"/>", "", ""),
        Role::Toc2 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"220\"/>",
            "",
            "",
        ),
        Role::Toc3 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"440\"/>",
            "",
            "",
        ),
        Role::Toc4 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"660\"/>",
            "",
            "",
        ),
        Role::Toc5 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"880\"/>",
            "",
            "",
        ),
        Role::Toc6 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"1100\"/>",
            "",
            "",
        ),
        Role::Toc7 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"1320\"/>",
            "",
            "",
        ),
        Role::Toc8 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"1540\"/>",
            "",
            "",
        ),
        Role::Toc9 => (
            "<w:spacing w:after=\"100\"/><w:ind w:left=\"1760\"/>",
            "",
            "",
        ),
    }
}

/// The left indent paragraph style `id` sets itself, in twips.
fn style_indent_left(tree: &XmlTree, id: &str) -> i64 {
    w_children(tree, tree.root(), "style")
        .find(|style| w_attr(tree, *style, "styleId") == Some(id))
        .and_then(|style| w_child(tree, style, "pPr"))
        .and_then(|ppr| w_child(tree, ppr, "ind"))
        .and_then(|ind| w_attr(tree, ind, "left").or_else(|| w_attr(tree, ind, "start")))
        .and_then(|left| left.parse().ok())
        .unwrap_or(0)
}

fn create(
    txn: &mut Txn<'_>,
    part: &str,
    role: Role,
    body: Option<String>,
    next: Option<String>,
    lifted: Option<Lifted>,
) -> Result<RoleStyle> {
    let custom_name = lifted.as_ref().and_then(|l| l.name.clone());
    let lifted_base = lifted.as_ref().and_then(|l| l.based_on.clone());
    let internal = custom_name.clone().unwrap_or_else(|| internal_name(role));
    let builtin = custom_name.is_none()
        && !matches!(
            role,
            Role::Author
                | Role::Date
                | Role::Abstract
                | Role::FirstParagraph
                | Role::Compact
                | Role::CodeBlock
                | Role::InlineCode
                | Role::DefinitionTerm
                | Role::Definition
                | Role::Figure
                | Role::ImageCaption
                | Role::TableCaption
                | Role::TableContents
        );
    let tree = txn.tree(part)?;
    // Word's ids for built-ins are the UI name without spaces
    // (`FootnoteText`), which `style_id_from_name` gives for UI names.
    let id = unique_id(tree, &style_id_from_name(&ui_name(&internal)));
    let kind = role.kind();
    let based_on = lifted_base.or(body).or_else(|| {
        default_style(tree, kind).and_then(|d| w_attr(tree, d, "styleId").map(str::to_string))
    });
    let (ppr, rpr, extra) = definition(role);
    let (ppr, rpr, extra): (String, String, String) = match lifted {
        Some(lifted) => (lifted.ppr, lifted.rpr, lifted.extra),
        None => (ppr.to_string(), rpr.to_string(), extra.to_string()),
    };
    let mut xml = format!(
        "<w:style xmlns:w=\"{W}\" w:type=\"{}\" w:styleId=\"{}\"{}>",
        kind.xml(),
        xml_escape(&id),
        if builtin { "" } else { " w:customStyle=\"1\"" }
    );
    xml.push_str(&format!("<w:name w:val=\"{}\"/>", xml_escape(&internal)));
    if let Some(based_on) = based_on.filter(|b| *b != id) {
        xml.push_str(&format!("<w:basedOn w:val=\"{}\"/>", xml_escape(&based_on)));
    }
    if let Some(next) = next {
        xml.push_str(&format!("<w:next w:val=\"{}\"/>", xml_escape(&next)));
    }
    let priority = if role.heading_level().is_some() {
        9
    } else if role.toc_level().is_some() {
        39
    } else {
        99
    };
    xml.push_str(&format!("<w:uiPriority w:val=\"{priority}\"/>"));
    if kind != Kind::Table {
        xml.push_str("<w:qFormat/>");
    }
    if !ppr.is_empty() {
        xml.push_str(&format!("<w:pPr>{ppr}</w:pPr>"));
    }
    if !rpr.is_empty() {
        xml.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
    }
    xml.push_str(&extra);
    xml.push_str("</w:style>");
    let tree = txn.tree_mut(part)?;
    let root = tree.root();
    let style = import_fragment(tree, root, &xml)?;
    crate::oxml::append(tree, root, style)?;
    Ok(RoleStyle {
        name: ui_name(&internal),
        style_id: Some(id),
        created: true,
    })
}

pub(crate) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
