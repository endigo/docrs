//! The headers and footers the output's section shows (the template's page
//! 1 on page 1, its body pages' after), and the pictures they cannot show
//! whole.

use std::collections::BTreeMap;

use crate::Result;
use crate::authoring::{R, WP, section_properties};
use crate::txn::Txn;
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

use crate::units::EMU_PER_TWIP;

const EMU_PER_CM: f64 = crate::units::EMU_PER_CM as f64;

/// A header or footer reference the output's section shows.
pub(crate) struct Reference {
    /// `headerReference` or `footerReference`.
    pub element: &'static str,
    /// `default`, `first` or `even`.
    pub kind: String,
    pub r_id: String,
    /// Not one the template's last section defines itself.
    pub inherited: bool,
}

/// What the output's single section shows in its headers and footers.
pub(crate) struct OutputSection {
    pub references: Vec<Reference>,
    /// Whether it has a distinct first page (`w:titlePg`).
    pub title_page: bool,
}

const STORIES: [&str; 2] = ["headerReference", "footerReference"];

fn references(
    tree: &XmlTree,
    sect_pr: NodeId,
) -> impl Iterator<Item = (&'static str, String, String)> + '_ {
    STORIES.into_iter().flat_map(move |local| {
        w_children(tree, sect_pr, local).filter_map(move |reference| {
            let kind = w_attr(tree, reference, "type")
                .unwrap_or("default")
                .to_string();
            let r_id = tree.attr(reference, Some(R), "id")?.to_string();
            Some((local, kind, r_id))
        })
    })
}

/// Whether the section has a distinct first page (`w:titlePg` on).
pub(crate) fn title_page(tree: &XmlTree, sect_pr: NodeId) -> bool {
    w_child(tree, sect_pr, "titlePg")
        .is_some_and(|t| !matches!(w_attr(tree, t, "val"), Some("0" | "false" | "off")))
}

/// The headers and footers the output's single section shows. Page 1 shows
/// what the template's page 1 shows: the first section's first-page header
/// and footer when it has a distinct first page, else its default ones. The
/// other pages show the last section's default ones, inherited from an
/// earlier section when it lacks them (Word reuses the previous section's
/// header of a type the section does not define). When page 1 differs from
/// the other pages, the section has a distinct first page with page 1's
/// header and footer in the first-page slots.
pub(crate) fn output_section(tree: &XmlTree) -> Result<OutputSection> {
    let sections = section_properties(tree)?;
    let (Some(first), Some(last)) = (sections.first(), sections.last()) else {
        return Ok(OutputSection {
            references: Vec::new(),
            title_page: false,
        });
    };
    let mut latest: BTreeMap<(&'static str, String), String> = BTreeMap::new();
    for sect_pr in &sections {
        for (local, kind, r_id) in references(tree, *sect_pr) {
            latest.insert((local, kind), r_id);
        }
    }
    let page_one_kind = if title_page(tree, *first) {
        "first"
    } else {
        "default"
    };
    let page_one: BTreeMap<&'static str, String> = references(tree, *first)
        .filter(|(_, kind, _)| kind == page_one_kind)
        .map(|(local, _, r_id)| (local, r_id))
        .collect();
    let distinct = STORIES
        .iter()
        .any(|local| page_one.get(local) != latest.get(&(*local, "default".to_string())));
    latest.retain(|(_, kind), _| kind != "first");
    if distinct {
        for (local, r_id) in page_one {
            latest.insert((local, "first".to_string()), r_id);
        }
    }
    let own: Vec<(&'static str, String, String)> = references(tree, *last).collect();
    Ok(OutputSection {
        references: latest
            .into_iter()
            .map(|((element, kind), r_id)| Reference {
                inherited: !own.contains(&(element, kind.clone(), r_id.clone())),
                element,
                kind,
                r_id,
            })
            .collect(),
        title_page: distinct,
    })
}

/// A header or footer the output's section shows.
pub(crate) struct Story {
    /// `header` or `footer`.
    pub story: &'static str,
    /// `default`, `first` or `even`.
    pub kind: String,
    /// Its part, when the relationship leads to one in the package.
    pub part: Option<String>,
    /// Taken over from an earlier section.
    pub inherited: bool,
}

pub(crate) fn stories(txn: &Txn<'_>, main: &str) -> Result<Vec<Story>> {
    let tree = txn.tree(main)?;
    let rels = txn.resolved_relationships(main)?;
    let mut out = Vec::new();
    for reference in output_section(tree)?.references {
        let part = rels
            .iter()
            .find(|(rel, _)| rel.id == reference.r_id)
            .map(|(_, member)| member.clone());
        out.push(Story {
            story: if reference.element == "headerReference" {
                "header"
            } else {
                "footer"
            },
            kind: reference.kind,
            part,
            inherited: reference.inherited,
        });
    }
    Ok(out)
}

/// A picture a header or footer cannot show whole. An inline picture sits
/// in the story's text flow and pushes or clips: wider than the page, or
/// taller than half of it (taller than the page in a first-page header,
/// which holds the cover of templates that have one). An anchored picture
/// is placed by position and may cover the page, as a letterhead
/// background does: wider or taller than the page.
pub(crate) struct OversizedPicture {
    story: &'static str,
    kind: String,
    width: i64,
    height: i64,
    page_width: i64,
    page_height: i64,
    height_limit: i64,
}

impl OversizedPicture {
    /// Plain language a practice can act on.
    pub(crate) fn message(&self) -> String {
        let cm = |emu: i64| format!("{:.1}", emu as f64 / EMU_PER_CM);
        let wide = self.width > self.page_width;
        let whole_page = self.height_limit == self.page_height;
        let fault = match (wide, self.height > self.height_limit, whole_page) {
            (true, true, true) => "larger than the page",
            (true, true, false) => "wider than the page and taller than half of it",
            (true, false, _) => "wider than the page",
            (false, _, true) => "taller than the page",
            (false, _, false) => "taller than half the page",
        };
        let story = match (self.story, self.kind.as_str()) {
            (story, "default") => story.to_string(),
            (story, "first") => format!("first-page {story}"),
            (story, "even") => format!("even-page {story}"),
            (story, kind) => format!("{kind} {story}"),
        };
        format!(
            "The {story} has a picture {} cm wide and {} cm tall, {fault} ({} by {} cm). Word hides what does not fit, but it can break PDFs and previews. Resize or remove it in the template.",
            cm(self.width),
            cm(self.height),
            cm(self.page_width),
            cm(self.page_height)
        )
    }
}

/// Page width and height of a section in twips (`w:pgSz`), US Letter
/// without one.
pub(crate) fn page_size(tree: &XmlTree, sect_pr: Option<NodeId>) -> (i64, i64) {
    let pg_sz = sect_pr.and_then(|s| w_child(tree, s, "pgSz"));
    let twips = |name: &str, fallback: i64| {
        pg_sz
            .and_then(|p| w_attr(tree, p, name))
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(fallback, |v| v as i64)
    };
    (twips("w", 12240), twips("h", 15840))
}

pub(crate) fn oversized_pictures(txn: &Txn<'_>, main: &str) -> Result<Vec<OversizedPicture>> {
    let tree = txn.tree(main)?;
    let Some(last) = section_properties(tree)?.last().copied() else {
        return Ok(Vec::new());
    };
    let (page_width, page_height) = page_size(tree, Some(last));
    let page_width = page_width * EMU_PER_TWIP;
    let page_height = page_height * EMU_PER_TWIP;
    let mut out = Vec::new();
    for story in stories(txn, main)? {
        let Some(part) = &story.part else {
            continue;
        };
        let inline_limit = if story.story == "header" && story.kind == "first" {
            page_height
        } else {
            page_height / 2
        };
        let tree = txn.tree(part)?;
        for node in tree.descendants(tree.root()) {
            let anchored = tree.is(node, WP, "anchor");
            if !(anchored || tree.is(node, WP, "inline")) {
                continue;
            }
            let height_limit = if anchored { page_height } else { inline_limit };
            let Some(extent) = tree
                .child_elements(node)
                .find(|c| tree.is(*c, WP, "extent"))
            else {
                continue;
            };
            let emu = |name: &str| {
                tree.attr(extent, None, name)
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0)
            };
            let (width, height) = (emu("cx"), emu("cy"));
            if width > page_width || height > height_limit {
                out.push(OversizedPicture {
                    story: story.story,
                    kind: story.kind.clone(),
                    width,
                    height,
                    page_width,
                    page_height,
                    height_limit,
                });
            }
        }
    }
    Ok(out)
}
