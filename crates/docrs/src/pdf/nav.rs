//! Links, named destinations and the outline: gathered while pages are
//! painted, written with the pages.
//!
//! Internal links go to named destinations, so a link may name a bookmark
//! that a later page sets. `_top` is built in: the top of page 1's text
//! area, as in Word's PDF. Only `http`, `https`, `mailto` and `tel` URIs
//! are written; no Launch, JavaScript or file action ever is.

use std::collections::{BTreeMap, BTreeSet};

use super::writer::{Dict, Object, Ref, Writer, count, name, text};
use super::{PdfWarning, Tally};
use crate::layout::Link;
use crate::{Error, Result};

pub(super) const LINK_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

const SCHEME_SKIPPED: &str = "pdf_link_scheme_skipped";
const TARGET_MISSING: &str = "pdf_link_target_missing";

/// Parts of one link on one line closer than this, in points, are one rect.
const ADJACENT: f64 = 0.5;
/// Entries per leaf of the `/Dests` name tree.
const NAME_TREE_LEAF: usize = 64;

/// A rectangle in PDF user space, in points from the bottom left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

/// One occurrence of a hyperlink, numbered by the painter: every part of it,
/// on every line, has the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LinkId(pub usize);

impl Rect {
    fn touches(&self, next: &Rect) -> bool {
        next.left <= self.right + ADJACENT && self.left <= next.right + ADJACENT
    }

    fn union(&self, other: &Rect) -> Rect {
        Rect {
            left: self.left.min(other.left),
            bottom: self.bottom.min(other.bottom),
            right: self.right.max(other.right),
            top: self.top.max(other.top),
        }
    }
}

/// Word's PDF destinations start this far left of the text, in points.
const DEST_INDENT: f64 = 2.16;
/// The anchor of Word's "Top of the Document" link, which needs no bookmark.
const DOCUMENT_START: &str = "_top";

/// A point to go to: a page (from 0) and a height on it.
#[derive(Debug, Clone, Copy)]
struct Place {
    page: usize,
    top: f64,
}

/// Where a bookmark or `_top` goes: `left` is where the text starts.
#[derive(Debug, Clone, Copy)]
struct Spot {
    page: usize,
    left: f64,
    top: f64,
}

#[derive(Debug, PartialEq)]
enum Target {
    /// Allowed and 7-bit ASCII.
    Uri(String),
    Anchor(String),
}

#[derive(Debug)]
struct Annotation {
    id: LinkId,
    page: usize,
    line: usize,
    rect: Rect,
    target: Target,
}

#[derive(Debug)]
struct Destination {
    /// [`dest_key`] of the first name.
    key: String,
    spot: Spot,
}

#[derive(Debug)]
struct Heading {
    level: u8,
    title: String,
    place: Place,
}

/// What the painter reports for navigation.
#[derive(Debug, Default)]
pub struct Navigation {
    links: Vec<Annotation>,
    /// By [`bookmark_key`].
    destinations: BTreeMap<String, Destination>,
    headings: Vec<Heading>,
    /// Page 1's text area, for `_top`.
    start: Option<Spot>,
    /// Links already counted in a warning.
    warned: BTreeSet<LinkId>,
    tally: Tally,
}

/// The objects [`Navigation::write`] made, for the pages and the catalog.
pub(super) struct Written {
    /// Each page's `/Annots`, empty when it has none.
    pub(super) annots: Vec<Vec<Object>>,
    /// `/Names`, `/Outlines` and `/PageMode`, as far as they apply.
    pub(super) catalog: Vec<(&'static str, Object)>,
    pub(super) warnings: Vec<PdfWarning>,
}

impl Navigation {
    /// Part `rect` of link `id` on `line` (the painter's line index) of
    /// `page`. A part touching the previous part of the same link on the
    /// same line extends its rectangle.
    pub fn link(&mut self, id: LinkId, page: usize, line: usize, rect: Rect, target: &Link) {
        let Some(target) = self.target(id, target) else {
            return;
        };
        let continued = self.links.last_mut().filter(|last| {
            (last.id, last.page, last.line) == (id, page, line) && last.rect.touches(&rect)
        });
        match continued {
            Some(last) => last.rect = last.rect.union(&rect),
            None => self.links.push(Annotation {
                id,
                page,
                line,
                rect,
                target,
            }),
        }
    }

    /// A bookmark on the line of `page` whose text starts at `left` and
    /// whose top is `top`; the first of a name, in any case, wins.
    pub fn destination(&mut self, name: &str, page: usize, left: f64, top: f64) {
        self.destinations
            .entry(bookmark_key(name))
            .or_insert_with(|| Destination {
                key: dest_key(name),
                spot: Spot { page, left, top },
            });
    }

    /// The left and top of page 1's text area, where `_top` goes. Without
    /// it, `_top` goes to the top of page 1.
    pub fn text_area(&mut self, left: f64, top: f64) {
        self.start = Some(Spot { page: 0, left, top });
    }

    /// A heading of outline `level` (1 is the top). A blank title has no
    /// outline item.
    pub fn heading(&mut self, level: u8, title: &str, page: usize, top: f64) {
        let title = outline_title(title);
        if !title.is_empty() {
            self.headings.push(Heading {
                level,
                title,
                place: Place { page, top },
            });
        }
    }

    fn target(&mut self, id: LinkId, link: &Link) -> Option<Target> {
        match link {
            Link::Anchor(anchor) => Some(Target::Anchor(anchor.clone())),
            Link::Uri(uri) => {
                let uri = uri.trim();
                match scheme(uri) {
                    Some(scheme) if LINK_SCHEMES.contains(&scheme.as_str()) => {
                        Some(Target::Uri(ascii_uri(uri)))
                    }
                    scheme => {
                        let sample = scheme.map_or_else(|| "relative".into(), |s| format!("{s}:"));
                        self.warn(id, SCHEME_SKIPPED, sample);
                        None
                    }
                }
            }
        }
    }

    /// Counts each link once.
    fn warn(&mut self, id: LinkId, code: &'static str, sample: String) {
        if self.warned.insert(id) {
            self.tally.note(code, "link", None, Some(sample));
        }
    }

    /// Writes the annotations, the `/Dests` name tree and the outline for
    /// `pages`, each `(page, height)`.
    pub(super) fn write(mut self, writer: &mut Writer, pages: &[(Ref, f64)]) -> Result<Written> {
        let page_ref = |page: usize| {
            pages.get(page).map(|&(at, _)| at).ok_or_else(|| {
                Error::internal(
                    "pdf_nav_page",
                    format!("navigation names page {page} of {}", pages.len()),
                )
            })
        };
        let mut annots = vec![Vec::new(); pages.len()];
        for link in std::mem::take(&mut self.links) {
            let page = page_ref(link.page)?;
            let action = match &link.target {
                Target::Uri(uri) => Dict::new().with("S", name("URI")).with("URI", text(uri)),
                Target::Anchor(anchor) => {
                    let goes = if is_document_start(anchor) {
                        match self.start {
                            Some(spot) => spot_destination(page_ref(0)?, spot),
                            None => destination(page_ref(0)?, pages[0].1),
                        }
                    } else if let Some(dest) = self.destinations.get(&bookmark_key(anchor)) {
                        text(&dest.key)
                    } else {
                        self.warn(link.id, TARGET_MISSING, anchor.clone());
                        continue;
                    };
                    Dict::new().with("S", name("GoTo")).with("D", goes)
                }
            };
            let at = writer.alloc()?;
            writer.dict(
                at,
                Dict::new()
                    .with("Type", name("Annot"))
                    .with("Subtype", name("Link"))
                    .with("Rect", rect(&link.rect))
                    .with("Border", vec![0.into(), 0.into(), 0.into()])
                    .with("P", page)
                    .with("A", action),
            )?;
            annots[link.page].push(at.into());
        }
        let mut catalog = Vec::new();
        if !self.destinations.is_empty() {
            let dests = self.write_dests(writer, &page_ref)?;
            catalog.push(("Names", Dict::new().with("Dests", dests).into()));
        }
        if !self.headings.is_empty() {
            let outline = self.write_outline(writer, &page_ref)?;
            catalog.push(("Outlines", outline.into()));
            catalog.push(("PageMode", name("UseOutlines")));
        }
        Ok(Written {
            annots,
            catalog,
            warnings: self.tally.finish(),
        })
    }

    /// A root holding the names, or leaves under it when there are many.
    fn write_dests(
        &self,
        writer: &mut Writer,
        page_ref: &impl Fn(usize) -> Result<Ref>,
    ) -> Result<Ref> {
        let mut sorted: Vec<&Destination> = self.destinations.values().collect();
        sorted.sort_by(|a, b| a.key.cmp(&b.key));
        let mut entries = Vec::with_capacity(sorted.len());
        for dest in sorted {
            let place = spot_destination(page_ref(dest.spot.page)?, dest.spot);
            entries.push((dest.key.as_str(), place));
        }
        let names = |entries: &[(&str, Object)]| -> Vec<Object> {
            entries
                .iter()
                .flat_map(|(key, dest)| [text(key), dest.clone()])
                .collect()
        };
        let root = writer.alloc()?;
        if entries.len() <= NAME_TREE_LEAF {
            writer.dict(root, Dict::new().with("Names", names(&entries)))?;
            return Ok(root);
        }
        let mut kids = Vec::new();
        for leaf in entries.chunks(NAME_TREE_LEAF) {
            let at = writer.alloc()?;
            let limits = vec![text(leaf[0].0), text(leaf[leaf.len() - 1].0)];
            writer.dict(
                at,
                Dict::new()
                    .with("Limits", limits)
                    .with("Names", names(leaf)),
            )?;
            kids.push(at.into());
        }
        writer.dict(root, Dict::new().with("Kids", kids))?;
        Ok(root)
    }

    /// Every item open; a heading deeper than the one before it is its
    /// child whatever levels it skips.
    fn write_outline(
        &self,
        writer: &mut Writer,
        page_ref: &impl Fn(usize) -> Result<Ref>,
    ) -> Result<Ref> {
        let parents = outline_parents(&self.headings);
        let mut descendants = vec![0usize; self.headings.len()];
        for item in (0..self.headings.len()).rev() {
            if let Some(parent) = parents[item] {
                descendants[parent] += 1 + descendants[item];
            }
        }
        let mut top = Vec::new();
        let mut kids = vec![Vec::new(); self.headings.len()];
        for (item, parent) in parents.iter().enumerate() {
            match parent {
                Some(parent) => kids[*parent].push(item),
                None => top.push(item),
            }
        }
        let root = writer.alloc()?;
        let refs = (0..self.headings.len())
            .map(|_| writer.alloc())
            .collect::<Result<Vec<Ref>>>()?;
        let ends = |dict: Dict, items: &[usize]| match (items.first(), items.last()) {
            (Some(&first), Some(&last)) => dict.with("First", refs[first]).with("Last", refs[last]),
            _ => dict,
        };
        let outlines = Dict::new()
            .with("Type", name("Outlines"))
            .with("Count", count(self.headings.len())?);
        writer.dict(root, ends(outlines, &top))?;
        let levels = std::iter::once((root, &top)).chain(refs.iter().copied().zip(&kids));
        for (parent, siblings) in levels {
            for (index, &item) in siblings.iter().enumerate() {
                let heading = &self.headings[item];
                let place = destination(page_ref(heading.place.page)?, heading.place.top);
                let mut dict = Dict::new()
                    .with("Title", text(&heading.title))
                    .with("Parent", parent)
                    .with("Dest", place);
                if index > 0 {
                    dict.set("Prev", refs[siblings[index - 1]]);
                }
                if let Some(&next) = siblings.get(index + 1) {
                    dict.set("Next", refs[next]);
                }
                if !kids[item].is_empty() {
                    dict.set("Count", count(descendants[item])?);
                }
                writer.dict(refs[item], ends(dict, &kids[item]))?;
            }
        }
        Ok(root)
    }
}

/// Each heading's parent: the nearest earlier heading of a lower level.
fn outline_parents(headings: &[Heading]) -> Vec<Option<usize>> {
    let mut open: Vec<usize> = Vec::new();
    headings
        .iter()
        .enumerate()
        .map(|(item, heading)| {
            while open
                .last()
                .is_some_and(|&above| headings[above].level >= heading.level)
            {
                open.pop();
            }
            let parent = open.last().copied();
            open.push(item);
            parent
        })
        .collect()
}

/// Word matches bookmark names without regard to case.
pub(super) fn bookmark_key(name: &str) -> String {
    name.to_lowercase()
}

/// The lower-cased scheme of an absolute URI; `None` for a relative one.
pub(super) fn scheme(target: &str) -> Option<String> {
    let (scheme, _) = target.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// `/URI` is 7-bit ASCII (ISO 32000-1 12.6.4.7): other bytes, controls and
/// spaces are percent-encoded.
fn ascii_uri(uri: &str) -> String {
    let mut out = String::with_capacity(uri.len());
    for byte in uri.bytes() {
        if byte.is_ascii_graphic() {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Line breaks, tabs and other controls become spaces, runs of spaces one.
fn outline_title(title: &str) -> String {
    title
        .split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A destination's name in the PDF: the bookmark name with `%`, controls
/// and non-ASCII bytes percent-encoded. Readers order name tree keys
/// differently once they are not ASCII (qpdf decodes them as text).
fn dest_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        if matches!(byte, b' '..=b'~') && byte != b'%' {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Whether `anchor` is Word's built-in "Top of the Document".
pub(super) fn is_document_start(anchor: &str) -> bool {
    bookmark_key(anchor) == DOCUMENT_START
}

/// `[page /XYZ x top 0]`, as Word writes a bookmark's destination.
fn spot_destination(page: Ref, spot: Spot) -> Object {
    vec![
        page.into(),
        name("XYZ"),
        (spot.left - DEST_INDENT).into(),
        spot.top.into(),
        0.into(),
    ]
    .into()
}

fn destination(page: Ref, top: f64) -> Object {
    vec![
        page.into(),
        name("XYZ"),
        Object::Null,
        top.into(),
        Object::Null,
    ]
    .into()
}

fn rect(rect: &Rect) -> Object {
    vec![
        rect.left.into(),
        rect.bottom.into(),
        rect.right.into(),
        rect.top.into(),
    ]
    .into()
}
