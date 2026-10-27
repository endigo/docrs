//! The headers and footers the output's section shows (its own and those
//! inherited from earlier sections), and the pictures they cannot show
//! whole.

use std::collections::BTreeMap;

use crate::Result;
use crate::authoring::{R, WP, section_properties};
use crate::txn::Txn;
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::XmlTree;

const EMU_PER_TWIP: i64 = 635;
const EMU_PER_CM: f64 = 360_000.0;

/// The header and footer references the final section shows, its own and
/// those it inherits (Word reuses the previous section's header of a type
/// the section does not define): `(element, type, relationship id,
/// inherited)`.
pub(crate) fn effective_references(
    tree: &XmlTree,
) -> Result<Vec<(&'static str, String, String, bool)>> {
    let sections = section_properties(tree)?;
    let mut latest: BTreeMap<(&'static str, String), (String, bool)> = BTreeMap::new();
    for (index, sect_pr) in sections.iter().enumerate() {
        let own = index + 1 == sections.len();
        for local in ["headerReference", "footerReference"] {
            for reference in w_children(tree, *sect_pr, local) {
                let kind = w_attr(tree, reference, "type")
                    .unwrap_or("default")
                    .to_string();
                if let Some(r_id) = tree.attr(reference, Some(R), "id") {
                    latest.insert((local, kind), (r_id.to_string(), !own));
                }
            }
        }
    }
    Ok(latest
        .into_iter()
        .map(|((local, kind), (r_id, inherited))| (local, kind, r_id, inherited))
        .collect())
}

/// A header or footer the final section shows.
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
    let rels = txn.relationships(main)?;
    let main_uri = format!("/{}", main.trim_start_matches('/'));
    let base = crate::package::rels::base_uri(&main_uri);
    let mut out = Vec::new();
    for (local, kind, r_id, inherited) in effective_references(tree)? {
        let part = rels
            .iter()
            .find(|rel| rel.id == r_id && !rel.external)
            .map(|rel| rel.target_part_name(base))
            .and_then(|name| txn.resolve(&name));
        out.push(Story {
            story: if local == "headerReference" {
                "header"
            } else {
                "footer"
            },
            kind,
            part,
            inherited,
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

pub(crate) fn oversized_pictures(txn: &Txn<'_>, main: &str) -> Result<Vec<OversizedPicture>> {
    let tree = txn.tree(main)?;
    let Some(last) = section_properties(tree)?.last().copied() else {
        return Ok(Vec::new());
    };
    let pg_sz = w_child(tree, last, "pgSz");
    let twips = |name: &str, fallback: i64| {
        pg_sz
            .and_then(|p| w_attr(tree, p, name))
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(fallback, |v| v as i64)
    };
    let page_width = twips("w", 12240) * EMU_PER_TWIP;
    let page_height = twips("h", 15840) * EMU_PER_TWIP;
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
