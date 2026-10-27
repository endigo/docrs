//! Which reference-document style each Markdown element takes.
//!
//! Every element has a *role*. A role looks its style up by name in the
//! reference document, trying pandoc's name first (so a pandoc reference
//! document works unchanged) and then the nearest Word built-in. When none
//! exists the converter either adds a definition (the default) or refuses
//! ([`StylePolicy::Strict`]); body-text roles always end at the document's
//! default paragraph style, so they never need either. Roles resolve on
//! first use, so a conversion only touches the styles it needs.

use std::collections::{BTreeMap, BTreeSet};

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
    /// The spacing lifted onto the body style, for the variants that follow it.
    body_spacing: Option<Spacing>,
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
            body_spacing: None,
            resolved: BTreeMap::new(),
            missing: Vec::new(),
        }
    }

    /// Reads the sample once and names the styles part.
    fn prepare(&mut self, txn: &mut Txn<'_>) -> Result<String> {
        if self.sample.is_none() {
            self.sample = Some(Sample::read(txn)?);
        }
        if let Some(part) = &self.part {
            return Ok(part.clone());
        }
        let part = crate::parts::ensure_styles_part(txn)?;
        self.part = Some(part.clone());
        Ok(part)
    }

    /// Gives a `List Bullet` or `List Number` style (`id`) the template
    /// never writes text in the font and size of the template's own
    /// bulleted (`bullets`) or numbered sample paragraphs, which sit in
    /// another style, so the exported list looks like the template's.
    pub(crate) fn restyle_list(
        &mut self,
        txn: &mut Txn<'_>,
        id: &str,
        bullets: bool,
    ) -> Result<()> {
        if self.prefer_own {
            return Ok(());
        }
        let part = self.prepare(txn)?;
        let sample = self.sample.as_ref().expect("sample read above");
        if sample.text_styles.contains(&Some(id.to_string())) {
            return Ok(());
        }
        let Some(look) = sample.list_looks.get(&bullets) else {
            return Ok(());
        };
        let styles = txn.tree(&part)?;
        if *look == run_look(&rpr_layers(styles, style_by_id(styles, id))).xml() {
            return Ok(());
        }
        restyle(txn, &part, id, look)
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
        let part = self.prepare(txn)?;
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
                let prefer_own = self.prefer_own;
                let resolved = &self.resolved;
                let by_name = |names: &[String]| {
                    names.iter().find_map(|name| {
                        let style = find_style(tree, name, role.kind())?;
                        // A body-text style the template writes no text in
                        // is not the template's body: a converter's spacer
                        // Body Text while the default style carries the
                        // prose, or a First Paragraph the prose never uses.
                        let unused = role.falls_back_to_default()
                            && !prefer_own
                            && Some(style) != default
                            && w_attr(tree, style, "styleId").is_some_and(|id| {
                                sample.unused(name, id)
                                    && !resolved
                                        .values()
                                        .any(|r| r.created && r.style_id.as_deref() == Some(id))
                            });
                        (!unused).then_some(style)
                    })
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
                let lift_body = role == Role::Body && sample.unstyled_body(tree).is_some();
                match found_style {
                    Some(style) => Some(found(tree, style, default)),
                    None if lift_body && self.policy == StylePolicy::Create => None,
                    None if role.falls_back_to_default() && !self.prefer_own => Some(
                        self.resolved
                            .get(&Role::Body)
                            .filter(|_| role != Role::Body)
                            .map(|body| RoleStyle {
                                created: false,
                                ..body.clone()
                            })
                            .unwrap_or_else(|| RoleStyle {
                                name: default
                                    .map(|d| display_name(tree, d))
                                    .unwrap_or_else(|| "Normal".to_string()),
                                style_id: None,
                                created: false,
                            }),
                    ),
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
                    let mut body_spacing = None;
                    let lifted = match role {
                        Role::Body => {
                            let tree = txn.tree(&part)?;
                            body_spacing = sample.body_spacing(tree, None);
                            sample.unstyled_body(tree)
                        }
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
                    let created = create(txn, &part, role, body, next, lifted)?;
                    if role == Role::Body {
                        self.body_spacing = body_spacing;
                    }
                    created
                }
            },
        };
        // An existing style whose sample paragraphs show another look by hand
        // takes that look, so the converted text looks like the template's own.
        if !style.created && !self.prefer_own {
            let sample = self.sample.as_ref().expect("sample read above");
            let look = match role.heading_level() {
                Some(level) => sample.heading_looks.get(&level).cloned().or_else(|| {
                    let id = style.style_id.as_deref()?;
                    sample.heading_font_to_follow(txn.tree(&part).ok()?, level, id)
                }),
                None if role.falls_back_to_default() => {
                    self.body_role_look(txn.tree(&part)?, role, &style)
                }
                None => None,
            };
            if let (Some(look), Some(id)) = (look, style.style_id.as_deref()) {
                restyle(txn, &part, id, &look)?;
            }
            if let (true, Some(id)) = (role.falls_back_to_default(), style.style_id.as_deref())
                && let Some(spacing) = self.body_role_spacing(txn.tree(&part)?, role, &style)
            {
                respace(txn, &part, id, &spacing)?;
                if role == Role::Body {
                    self.body_spacing = Some(spacing);
                }
            }
        }
        let id = style.style_id.clone();
        self.resolved.insert(role, style);
        Ok(id)
    }

    /// The spacing an existing style of a body-text role takes: for the
    /// body, the prose's; for a variant wearing only the default style's
    /// spacing, what the body took.
    fn body_role_spacing(&self, tree: &XmlTree, role: Role, style: &RoleStyle) -> Option<Spacing> {
        let sample = self.sample.as_ref()?;
        let id = style.style_id.as_deref()?;
        if role == Role::Body {
            return sample.body_spacing(tree, Some(id));
        }
        let body = self.resolved.get(&Role::Body)?;
        let body_id = body.style_id.as_deref();
        if body_id == Some(id) || !wears_default(tree, id, body_id, own_spacing) {
            return None;
        }
        self.body_spacing.clone()
    }

    /// The look an existing style of a body-text role takes: the one its own
    /// sample paragraphs show, or, for a variant (First Paragraph, Compact)
    /// wearing only the default style's look, the body's.
    fn body_role_look(&self, tree: &XmlTree, role: Role, style: &RoleStyle) -> Option<String> {
        let sample = self.sample.as_ref()?;
        let id = style.style_id.as_deref()?;
        if let Some(look) = sample.body_looks.get(&style.style_id) {
            return Some(look.clone());
        }
        if role == Role::Body {
            return None;
        }
        let body = self.resolved.get(&Role::Body)?;
        let body_id = body.style_id.as_deref();
        if body_id == Some(id) || !wears_default(tree, id, body_id, own_look) {
            return None;
        }
        let key = if body.created {
            None
        } else {
            body.style_id.clone()
        };
        sample.body_looks.get(&key).cloned()
    }
}

fn style_by_id(tree: &XmlTree, id: &str) -> Option<NodeId> {
    w_children(tree, tree.root(), "style").find(|s| w_attr(tree, *s, "styleId") == Some(id))
}

/// Whether a paragraph style shows only the default style's look in the
/// respect `own` tests: neither it nor a parent short of the default has
/// one of its own, and it does not descend from `body`, which hands its
/// look down anyway.
fn wears_default(
    tree: &XmlTree,
    id: &str,
    body: Option<&str>,
    own: fn(&XmlTree, NodeId) -> bool,
) -> bool {
    let default = default_style(tree, Kind::Paragraph);
    let mut seen: Vec<String> = Vec::new();
    let mut next = Some(id.to_string());
    while let Some(id) = next.take() {
        if Some(id.as_str()) == body || seen.contains(&id) {
            return false;
        }
        let Some(style) = style_by_id(tree, &id) else {
            break;
        };
        if Some(style) == default {
            break;
        }
        if own(tree, style) {
            return false;
        }
        seen.push(id);
        next = w_child(tree, style, "basedOn")
            .and_then(|b| w_attr(tree, b, "val"))
            .map(str::to_string);
    }
    true
}

/// A font, size or colour in the style's own run properties.
fn own_look(tree: &XmlTree, style: NodeId) -> bool {
    w_child(tree, style, "rPr").is_some_and(|rpr| {
        ["rFonts", "sz", "color"]
            .iter()
            .any(|local| w_child(tree, rpr, local).is_some())
    })
}

/// Paragraph spacing in the style's own paragraph properties.
fn own_spacing(tree: &XmlTree, style: NodeId) -> bool {
    w_child(tree, style, "pPr").is_some_and(|ppr| w_child(tree, ppr, "spacing").is_some())
}

/// Paragraph spacing as Word resolves it, in twips; `line` is in 240ths
/// of a line under the `auto` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spacing {
    before: String,
    after: String,
    line: String,
    line_rule: String,
}

impl Default for Spacing {
    fn default() -> Spacing {
        Spacing {
            before: "0".to_string(),
            after: "0".to_string(),
            line: "240".to_string(),
            line_rule: "auto".to_string(),
        }
    }
}

impl Spacing {
    /// Lays one `w:spacing` element over this spacing; a `line` brings its
    /// rule along (`auto` when it names none).
    fn apply(&mut self, tree: &XmlTree, spacing: Option<NodeId>) {
        let Some(spacing) = spacing else {
            return;
        };
        if let Some(before) = w_attr(tree, spacing, "before") {
            self.before = before.to_string();
        }
        if let Some(after) = w_attr(tree, spacing, "after") {
            self.after = after.to_string();
        }
        if let Some(line) = w_attr(tree, spacing, "line") {
            self.line = line.to_string();
            self.line_rule = w_attr(tree, spacing, "lineRule")
                .unwrap_or("auto")
                .to_string();
        }
    }

    fn xml(&self) -> String {
        format!(
            "<w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{}\" w:lineRule=\"{}\"/>",
            xml_escape(&self.before),
            xml_escape(&self.after),
            xml_escape(&self.line),
            xml_escape(&self.line_rule)
        )
    }

    fn describe(&self) -> Value {
        let twips = |value: &str| value.parse::<f64>().ok();
        serde_json::json!({
            "before_pt": twips(&self.before).map(|t| t / 20.0),
            "after_pt": twips(&self.after).map(|t| t / 20.0),
            "line": twips(&self.line).map(|l| if self.line_rule == "auto" { l / 240.0 } else { l / 20.0 }),
            "line_rule": self.line_rule,
        })
    }
}

/// A style and the styles it is based on, nearest first.
fn style_chain(styles: &XmlTree, style: Option<NodeId>) -> Vec<NodeId> {
    let mut chain = Vec::new();
    let mut current = style;
    while let Some(node) = current.filter(|node| !chain.contains(node)) {
        chain.push(node);
        current = w_child(styles, node, "basedOn")
            .and_then(|b| w_attr(styles, b, "val"))
            .and_then(|id| style_by_id(styles, id));
    }
    chain
}

/// The spacing a paragraph style gives: the document defaults under the
/// style's ancestors, the style itself last (`None`: the defaults alone).
fn style_spacing(styles: &XmlTree, style: Option<NodeId>) -> Spacing {
    let mut spacing = Spacing::default();
    spacing.apply(
        styles,
        w_child(styles, styles.root(), "docDefaults")
            .and_then(|d| w_child(styles, d, "pPrDefault"))
            .and_then(|d| w_child(styles, d, "pPr"))
            .and_then(|p| w_child(styles, p, "spacing")),
    );
    for node in style_chain(styles, style).into_iter().rev() {
        spacing.apply(
            styles,
            w_child(styles, node, "pPr").and_then(|p| w_child(styles, p, "spacing")),
        );
    }
    spacing
}

/// A font a run property names: by name, or through the theme.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Font {
    Named(String),
    Theme(String),
}

/// The font and size a paragraph's text resolves to.
struct RunLook {
    ascii: Option<Font>,
    hansi: Option<Font>,
    size: Option<String>,
}

impl RunLook {
    /// `w:rFonts` and `w:sz` for this look; empty when nothing is set.
    fn xml(&self) -> String {
        let mut attrs = String::new();
        for (slot, font) in [("ascii", &self.ascii), ("hAnsi", &self.hansi)] {
            match font {
                Some(Font::Named(name)) => {
                    attrs.push_str(&format!(" w:{slot}=\"{}\"", xml_escape(name)));
                }
                Some(Font::Theme(theme)) => {
                    attrs.push_str(&format!(" w:{slot}Theme=\"{}\"", xml_escape(theme)));
                }
                None => {}
            }
        }
        let mut out = String::new();
        if !attrs.is_empty() {
            out.push_str(&format!("<w:rFonts{attrs}/>"));
        }
        if let Some(size) = &self.size {
            out.push_str(&format!("<w:sz w:val=\"{}\"/>", xml_escape(size)));
        }
        out
    }
}

/// The run properties a paragraph style gives, nearest first: the style's
/// own, each style it is based on, then the document defaults.
fn rpr_layers(styles: &XmlTree, style: Option<NodeId>) -> Vec<(&XmlTree, NodeId)> {
    style_chain(styles, style)
        .into_iter()
        .filter_map(|node| w_child(styles, node, "rPr"))
        .chain(
            w_child(styles, styles.root(), "docDefaults")
                .and_then(|d| w_child(styles, d, "rPrDefault"))
                .and_then(|d| w_child(styles, d, "rPr")),
        )
        .map(|rpr| (styles, rpr))
        .collect()
}

/// The font and size `layers` of run properties resolve to, nearest first,
/// as Word does: each font slot from the nearest `w:rFonts` that sets it,
/// where a theme attribute beats the name on the same element.
fn run_look(layers: &[(&XmlTree, NodeId)]) -> RunLook {
    let font = |slot: &str| {
        let theme = format!("{slot}Theme");
        layers.iter().find_map(|(tree, rpr)| {
            let fonts = w_child(tree, *rpr, "rFonts")?;
            w_attr(tree, fonts, &theme)
                .map(|value| Font::Theme(value.to_string()))
                .or_else(|| w_attr(tree, fonts, slot).map(|name| Font::Named(name.to_string())))
        })
    };
    RunLook {
        ascii: font("ascii"),
        hansi: font("hAnsi"),
        size: layers.iter().find_map(|(tree, rpr)| {
            w_child(tree, *rpr, "sz").and_then(|sz| w_attr(tree, sz, "val").map(str::to_string))
        }),
    }
}

/// Writes `spacing` into the style's paragraph properties in full, so no
/// inherited value shows through.
fn respace(txn: &mut Txn<'_>, part: &str, id: &str, spacing: &Spacing) -> Result<()> {
    let tree = txn.tree_mut(part)?;
    let Some(style) = style_by_id(tree, id) else {
        return Ok(());
    };
    let ppr = crate::oxml::get_or_add(tree, style, "pPr", crate::authoring::STYLE_SEQ)?;
    if let Some(old) = w_child(tree, ppr, "spacing") {
        crate::authoring::remove(tree, old)?;
    }
    let wrapper = import_fragment(
        tree,
        ppr,
        &format!("<w:pPr xmlns:w=\"{W}\">{}</w:pPr>", spacing.xml()),
    )?;
    let element = tree
        .child_elements(wrapper)
        .next()
        .expect("the spacing element");
    tree.detach(element)?;
    crate::oxml::insert_before_successors(
        tree,
        ppr,
        element,
        crate::oxml::successors(crate::oxml::PPR_SEQ, "spacing"),
    )
}

/// The value carrying the most weight, when one beats every other.
fn plurality<T: PartialEq + Clone>(values: impl Iterator<Item = (T, usize)>) -> Option<T> {
    let mut weights: Vec<(T, usize)> = Vec::new();
    for (value, weight) in values {
        match weights.iter_mut().find(|(seen, _)| *seen == value) {
            Some(seen) => seen.1 += weight,
            None => weights.push((value, weight)),
        }
    }
    weights.sort_by_key(|(_, weight)| std::cmp::Reverse(*weight));
    match weights.as_slice() {
        [(value, most), rest @ ..] if rest.first().is_none_or(|(_, second)| second < most) => {
            Some(value.clone())
        }
        _ => None,
    }
}

fn restyle(txn: &mut Txn<'_>, part: &str, id: &str, look: &str) -> Result<()> {
    let tree = txn.tree_mut(part)?;
    let Some(style) = style_by_id(tree, id) else {
        return Ok(());
    };
    let rpr = crate::oxml::get_or_add(tree, style, "rPr", crate::authoring::STYLE_SEQ)?;
    let wrapper = import_fragment(tree, rpr, &format!("<w:rPr xmlns:w=\"{W}\">{look}</w:rPr>"))?;
    for child in tree.child_elements(wrapper).collect::<Vec<_>>() {
        let local = tree.local_name(child).unwrap_or("").to_string();
        let attrs: Vec<(String, String)> = tree
            .element(child)
            .map(|element| {
                element
                    .attrs
                    .iter()
                    .filter(|attr| attr.ns.map(|ns| tree.uri(ns)) == Some(W))
                    .map(|attr| (attr.name.local.to_string(), attr.value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let old = w_child(tree, rpr, &local);
        // The sample's attributes override the style's one by one; the rest
        // stay. A font or colour set by name drops the theme attribute Word
        // would otherwise prefer, and the other way round, short of what the
        // sample sets itself.
        if let (Some(old), false) = (old, attrs.is_empty()) {
            for (name, _) in &attrs {
                for shadowed in shadowed_by(&local, name) {
                    tree.remove_attr(old, Some(W), shadowed);
                }
            }
            for (name, value) in &attrs {
                tree.set_attr(old, Some(W), name, value)?;
            }
            tree.detach(child)?;
            continue;
        }
        if let Some(old) = old {
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

fn shadowed_by(element: &str, attribute: &str) -> &'static [&'static str] {
    match (element, attribute) {
        ("rFonts", "ascii") => &["asciiTheme"],
        ("rFonts", "asciiTheme") => &["ascii"],
        ("rFonts", "hAnsi") => &["hAnsiTheme"],
        ("rFonts", "hAnsiTheme") => &["hAnsi"],
        ("rFonts", "eastAsia") => &["eastAsiaTheme"],
        ("rFonts", "eastAsiaTheme") => &["eastAsia"],
        ("rFonts", "cs") => &["cstheme"],
        ("rFonts", "cstheme") => &["cs"],
        ("color", "val") | ("u", "color") => &["themeColor", "themeTint", "themeShade"],
        // `w:color` keeps its `w:val`, which the schema requires: Word
        // prefers the theme colour when both are set anyway.
        ("u", "themeColor") => &["color"],
        _ => &[],
    }
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
    /// The run look carrying the most characters of a style's prose
    /// paragraphs, by style id (`None`: the default style), when it is set
    /// by hand and beats every other look, the style's own included;
    /// emphasis left out.
    body_looks: BTreeMap<Option<String>, String>,
    /// The spacing each prose paragraph shows, as Word resolves it, with
    /// the paragraph's characters, by style id (`None`: the default style).
    body_spacings: BTreeMap<Option<String>, Vec<(Spacing, usize)>>,
    /// Styles of the top-level paragraphs that carry text, prose or not.
    text_styles: BTreeSet<Option<String>>,
    /// The font and size, as Word resolves them, carrying the most
    /// characters of the bulleted (`true`) and numbered sample paragraphs.
    list_looks: BTreeMap<bool, String>,
    /// Heading styles by id: named `heading N` or carrying an outline level.
    heading_styles: BTreeMap<String, u8>,
    table_style: Option<String>,
    table: Option<Lifted>,
    table_look: Vec<(String, bool)>,
}

/// Characters from which a sample paragraph is prose the body look and
/// spacing can be read from, rather than a letterhead or label line.
const PROSE_CHARS: usize = 40;

/// `CT_TcBorders`, `CT_TblBorders` and `CT_PBdr` child order, strict and
/// transitional names together.
const BORDERS_SEQ: &[&str] = &[
    "top", "start", "left", "bottom", "end", "right", "insideH", "insideV", "tl2br", "tr2bl",
    "between", "bar",
];

/// `CT_TcMar` and `CT_TblCellMar` child order.
const MARGINS_SEQ: &[&str] = &["top", "start", "left", "bottom", "end", "right"];

/// The schema order of a properties element's children, for the elements a
/// look is copied from; `None` keeps the template's order.
fn child_sequence(local: &str) -> Option<&'static [&'static str]> {
    Some(match local {
        "pPr" => crate::oxml::PPR_SEQ,
        "rPr" => crate::oxml::RPR_SEQ,
        "tcPr" => crate::authoring::TCPR_SEQ,
        "tblPr" => crate::authoring::TBLPR_SEQ,
        "tcBorders" | "tblBorders" | "pBdr" => BORDERS_SEQ,
        "tcMar" | "tblCellMar" => MARGINS_SEQ,
        _ => return None,
    })
}

/// The `w:` children of `node` in schema order, whatever order the template
/// wrote them in; children the schema does not name follow, in their order.
fn ordered_w_children(tree: &XmlTree, node: NodeId) -> Vec<NodeId> {
    let mut children: Vec<NodeId> = tree
        .child_elements(node)
        .filter(|c| tree.ns_uri(*c) == Some(W))
        .collect();
    if let Some(sequence) = tree.local_name(node).and_then(child_sequence) {
        children.sort_by_key(|child| {
            let local = tree.local_name(*child).unwrap_or("");
            sequence
                .iter()
                .position(|item| *item == local)
                .unwrap_or(sequence.len())
        });
    }
    children
}

/// The `w:` children of `node` that `admit` lets through, as
/// WordprocessingML in schema order.
fn w_xml(tree: &XmlTree, node: NodeId, admit: impl Fn(&str) -> bool) -> String {
    let mut out = String::new();
    for child in ordered_w_children(tree, node) {
        if admit(tree.local_name(child).unwrap_or("")) {
            write_w(tree, child, &mut out);
        }
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
    let children = ordered_w_children(tree, node);
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
    let out = w_xml(tree, rpr, |local| {
        LOOK_RPR.contains(&local) && !skip.contains(&local)
    });
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
    /// Whether the template writes no prose in this style while its prose
    /// is elsewhere: in the default style, or, for a First Paragraph, in
    /// any style at all. Only prose counts as use: templates built from
    /// Pandoc's sample reference carry one short line per style ("First
    /// Paragraph."), a label rather than use, and a Body Text holding only
    /// the steps of a closing section is not the body.
    fn unused(&self, name: &str, id: &str) -> bool {
        let elsewhere = if name.eq_ignore_ascii_case("First Paragraph") {
            !self.body_spacings.is_empty()
        } else {
            self.body_spacings.contains_key(&None)
        };
        elsewhere && !self.body_spacings.contains_key(&Some(id.to_string()))
    }

    /// The font an unused heading style still on the theme's font follows:
    /// the one the practice set by name on the nearest higher level it does
    /// use. A heading the template uses, or one with a named font anywhere
    /// in its chain, is left as it is.
    fn heading_font_to_follow(&self, styles: &XmlTree, level: u8, id: &str) -> Option<String> {
        let used = |id: &str| self.text_styles.contains(&Some(id.to_string()));
        let look = |id: &str| run_look(&rpr_layers(styles, style_by_id(styles, id)));
        if used(id) || !matches!(look(id).ascii, Some(Font::Theme(_))) {
            return None;
        }
        let above = (1..level).rev().find_map(|above| {
            self.heading_styles
                .iter()
                .find(|(id, l)| **l == above && used(id))
                .map(|(id, _)| id.as_str())
        })?;
        let named = |font: Option<Font>| font.filter(|font| matches!(font, Font::Named(_)));
        let above = look(above);
        Some(
            RunLook {
                ascii: Some(named(above.ascii)?),
                hansi: named(above.hansi),
                size: None,
            }
            .xml(),
        )
    }

    /// The spacing the body style takes from the prose, the unstyled
    /// paragraphs' and the style's own, when it differs from what the style
    /// gives; `body_style` `None` is the default style, or a style to be
    /// created on it.
    fn body_spacing(&self, styles: &XmlTree, body_style: Option<&str>) -> Option<Spacing> {
        let node = match body_style {
            Some(id) => Some(style_by_id(styles, id)?),
            None => default_style(styles, Kind::Paragraph),
        };
        let own = style_spacing(styles, node);
        let buckets = [None, body_style.map(str::to_string)];
        let prose = buckets
            .iter()
            .take(if body_style.is_some() { 2 } else { 1 })
            .filter_map(|style| self.body_spacings.get(style))
            .flatten()
            .map(|(spacing, chars)| (spacing.clone(), *chars));
        plurality(prose).filter(|spacing| *spacing != own)
    }

    /// What the unstyled paragraphs show that the default style does not,
    /// for a Body Text added on it: their look and their spacing.
    fn unstyled_body(&self, styles: &XmlTree) -> Option<Lifted> {
        let rpr = self.body_looks.get(&None).cloned();
        let spacing = self.body_spacing(styles, None);
        if rpr.is_none() && spacing.is_none() {
            return None;
        }
        Some(Lifted {
            rpr: rpr.unwrap_or_default(),
            ppr: spacing.map(|spacing| spacing.xml()).unwrap_or_default(),
            // Named apart from a Body Text the template keeps unused.
            name: find_style(styles, "Body Text", Kind::Paragraph)
                .is_some()
                .then(|| "Template Body Text".to_string()),
            ..Lifted::default()
        })
    }

    /// What a conversion lifts from the sample content, for a report;
    /// `body_style` is the existing style the body takes (`None`: the
    /// default, or the style added on it).
    pub(crate) fn summary(&self, styles: Option<&XmlTree>, body_style: Option<&str>) -> Value {
        serde_json::json!({
            "body": self.body_looks.get(&body_style.map(str::to_string)).map(|rpr| describe_rpr(rpr)),
            "spacing": styles
                .and_then(|styles| self.body_spacing(styles, body_style))
                .map(|spacing| spacing.describe()),
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
        let mut body_runs: BTreeMap<Option<String>, Vec<(String, usize)>> = BTreeMap::new();
        let mut default_style: Option<String> = None;
        let mut tables: Vec<SampleTable> = Vec::new();
        let mut list_runs: BTreeMap<bool, Vec<(String, usize)>> = BTreeMap::new();
        let related = |rel_type: &str| -> Result<Option<&XmlTree>> {
            match package.part_related_by(package.main_part_name(), rel_type)? {
                Some(name) => Ok(Some(package.xml(&name)?)),
                None => Ok(None),
            }
        };
        let styles_tree = related(crate::package::rels::rt::STYLES)?;
        let numbering_tree = related(crate::package::rels::rt::NUMBERING)?;
        if let Some(styles) = styles_tree {
            for style in w_children(styles, styles.root(), "style") {
                let Some(id) = w_attr(styles, style, "styleId") else {
                    continue;
                };
                if matches!(w_attr(styles, style, "default"), Some("1" | "true" | "on"))
                    && w_attr(styles, style, "type").unwrap_or("paragraph") == "paragraph"
                {
                    default_style = Some(id.to_string());
                }
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
                    sample.heading_styles.insert(id.to_string(), level);
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
            let style_id = ppr
                .and_then(|p| w_child(tree, p, "pStyle"))
                .and_then(|s| w_attr(tree, s, "val"))
                .filter(|id| Some(*id) != default_style.as_deref())
                .map(str::to_string);
            let listed = ppr.and_then(|p| w_child(tree, p, "numPr")).is_some();
            let first_run = w_children(tree, block, "r")
                .find(|r| !crate::authoring::run_text(tree, *r).trim().is_empty());
            let first_rpr = first_run.and_then(|r| w_child(tree, r, "rPr"));
            if first_run.is_some() {
                sample.text_styles.insert(style_id.clone());
            }
            let chars: usize = w_children(tree, block, "r")
                .map(|r| crate::authoring::run_text(tree, r).chars().count())
                .sum();
            let outline = ppr
                .and_then(|p| w_child(tree, p, "outlineLvl"))
                .and_then(|o| w_attr(tree, o, "val"))
                .and_then(|v| v.parse::<u8>().ok());
            let heading_level = ppr
                .and_then(|p| w_child(tree, p, "pStyle"))
                .and_then(|s| w_attr(tree, s, "val"))
                .and_then(|id| sample.heading_styles.get(id).copied());
            if let (Some(level), true) = (heading_level, outline.is_none()) {
                if first_run.is_some() {
                    heading_runs.entry(level).or_default().push(
                        first_rpr
                            .and_then(|r| look_rpr(tree, r, &[]))
                            .unwrap_or_default(),
                    );
                }
                continue;
            }
            if listed && outline.is_none() && first_run.is_some() {
                let num_pr = ppr.and_then(|p| w_child(tree, p, "numPr"));
                let value = |local: &str| {
                    num_pr
                        .and_then(|n| w_child(tree, n, local))
                        .and_then(|v| w_attr(tree, v, "val"))
                        .and_then(|v| v.parse::<i64>().ok())
                };
                if let (Some(styles), Some(numbering)) = (styles_tree, numbering_tree)
                    && let Some(num_id) = value("numId").filter(|n| *n != 0)
                    && let Some(format) =
                        super::render::level_format(numbering, num_id, value("ilvl").unwrap_or(0))
                    && format != "none"
                {
                    let style = style_id
                        .as_deref()
                        .or(default_style.as_deref())
                        .and_then(|id| style_by_id(styles, id));
                    let mut layers: Vec<(&XmlTree, NodeId)> =
                        first_rpr.map(|rpr| (tree, rpr)).into_iter().collect();
                    layers.extend(rpr_layers(styles, style));
                    list_runs
                        .entry(format == "bullet")
                        .or_default()
                        .push((run_look(&layers).xml(), chars));
                }
                continue;
            }
            match outline {
                Some(level) if level < 9 => {
                    sample.headings.entry(level + 1).or_insert_with(|| Lifted {
                        ppr: ppr
                            .map(|p| {
                                w_xml(tree, p, |local| {
                                    !matches!(
                                        local,
                                        "pStyle" | "numPr" | "rPr" | "sectPr" | "pPrChange"
                                    )
                                })
                            })
                            .unwrap_or_default(),
                        rpr: first_rpr
                            .and_then(|r| look_rpr(tree, r, &[]))
                            .unwrap_or_default(),
                        ..Lifted::default()
                    });
                }
                None if !listed && first_run.is_some() && chars >= PROSE_CHARS => {
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
                    let look = candidate
                        .and_then(|r| look_rpr(tree, r, EMPHASIS_RPR))
                        .unwrap_or_default();
                    let mut spacing = styles_tree
                        .map(|styles| {
                            let node = style_id
                                .as_deref()
                                .or(default_style.as_deref())
                                .and_then(|id| style_by_id(styles, id));
                            style_spacing(styles, node)
                        })
                        .unwrap_or_default();
                    spacing.apply(tree, ppr.and_then(|p| w_child(tree, p, "spacing")));
                    sample
                        .body_spacings
                        .entry(style_id.clone())
                        .or_default()
                        .push((spacing, chars));
                    body_runs.entry(style_id).or_default().push((look, chars));
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
        // A plurality by characters: a tie, or plain text in the lead,
        // leaves the style as it is.
        for (style, runs) in body_runs {
            if let Some(look) = plurality(runs.into_iter()).filter(|look| !look.is_empty()) {
                sample.body_looks.insert(style, look);
            }
        }
        for (bullets, runs) in list_runs {
            if let Some(look) = plurality(runs.into_iter()).filter(|look| !look.is_empty()) {
                sample.list_looks.insert(bullets, look);
            }
        }
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
const LOOK_TBLPR: &[&str] = &["tblBorders", "shd", "tblCellMar"];

fn row_look(tree: &XmlTree, tr: NodeId) -> (String, String) {
    match w_child(tree, tr, "tc") {
        Some(tc) => cell_look(tree, tc),
        None => (String::new(), String::new()),
    }
}

/// `(cell look, run look)`: borders, shading and margins; font, colour and
/// emphasis of the first run (the paragraph mark's when the cell is empty).
fn cell_look(tree: &XmlTree, tc: NodeId) -> (String, String) {
    let cell = w_child(tree, tc, "tcPr")
        .map(|tc_pr| w_xml(tree, tc_pr, |local| LOOK_TCPR.contains(&local)))
        .unwrap_or_default();
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
    let props = tbl_pr
        .map(|tbl_pr| w_xml(tree, tbl_pr, |local| LOOK_TBLPR.contains(&local)))
        .unwrap_or_default();
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
    sides.sort_by_key(|(side, _)| {
        BORDERS_SEQ
            .iter()
            .position(|o| o == side)
            .unwrap_or(BORDERS_SEQ.len())
    });
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
