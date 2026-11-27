//! Page layout estimation: where Word for Mac breaks lines and pages, from
//! font metrics and the document's formatting, without rendering. Used to
//! write the page numbers of a table of contents.
//!
//! The estimate covers paragraphs (line breaking with tabs, indents, list
//! labels and line spacing), paragraph spacing with contextual spacing,
//! page breaks, keep-with-next, keep-lines, widow/orphan control, tables
//! (rows as tall as their tallest cell, split between lines, repeated
//! header rows), inline pictures, and the header and footer heights that
//! push the body down. Floating objects, footnotes, columns and multiple
//! sections with different page setups are not modelled.

mod flow;
pub mod fonts;
mod model;

use std::collections::{HashMap, HashSet};

pub use flow::Placed;
#[cfg(test)]
pub(crate) use flow::TEST_LAYOUT_WORK;
pub use fonts::{Face, FontBook};

use fonts::{FontError, Parsed};

use crate::Result;
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::wml::{R, w_attr, w_child, w_children};
use crate::xml::NodeId;
use model::StoryId;

/// The estimated layout of a document's body.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Every paragraph (and table row end) in Word's order.
    pub placed: Vec<Placed>,
    /// Physical page count.
    pub pages: u32,
    /// The number the first page shows (`w:pgNumType w:start`, else 1).
    pub first_page_number: i64,
    /// Font families with no metrics, substituted, and families whose
    /// embedded fonts went unread because the document's fonts exceeded
    /// the parse budget.
    pub missing_fonts: Vec<String>,
    /// Families measured from fonts embedded in the document.
    pub embedded_fonts: Vec<String>,
    bookmarks: HashMap<String, u32>,
}

impl Layout {
    /// The physical page (from 1) holding the paragraph that starts
    /// bookmark `name`.
    pub fn bookmark_page(&self, name: &str) -> Option<u32> {
        self.bookmarks.get(name).copied()
    }

    /// The page number shown on physical page `page`.
    pub fn page_number(&self, page: u32) -> i64 {
        self.first_page_number + i64::from(page) - 1
    }
}

/// Embedded fonts of the document (`w:embedRegular` and friends in the
/// font table), deobfuscated and measured within one budget, and the
/// families whose fonts the budget left unread.
fn embedded_fonts(txn: &Txn<'_>, main: &str) -> Result<(Vec<Face>, Vec<String>)> {
    let (mut out, mut unread) = (Vec::new(), Vec::new());
    let Some(table) = txn.part_related_by(main, rt::FONT_TABLE)? else {
        return Ok((out, unread));
    };
    let tree = txn.tree(&table)?;
    let rels = txn.resolved_relationships(&table)?;
    let mut budget = fonts::FILE_BUDGET;
    let mut copies: HashMap<&str, Vec<u8>> = HashMap::new();
    let mut parsed: HashMap<(&str, Option<[u8; 16]>), Parsed<Face>> = HashMap::new();
    for font in w_children(tree, tree.root(), "font") {
        let Some(name) = w_attr(tree, font, "name") else {
            continue;
        };
        for (local, bold, italic) in [
            ("embedRegular", false, false),
            ("embedBold", true, false),
            ("embedItalic", false, true),
            ("embedBoldItalic", true, true),
        ] {
            let Some(embed) = w_child(tree, font, local) else {
                continue;
            };
            let Some(id) = tree.attr(embed, Some(R), "id") else {
                continue;
            };
            let Some((_, part)) = rels.iter().find(|(rel, _)| rel.id == id) else {
                continue;
            };
            let key = match w_attr(tree, embed, "fontKey") {
                Some(key) => match fonts::font_key(key) {
                    Some(key) => Some(key),
                    None => continue,
                },
                None => None,
            };
            let face = parsed.entry((part.as_str(), key)).or_insert_with(|| {
                let unreadable = Err(FontError::Malformed("embedded font part"));
                let Some(bytes) = txn.binary(part) else {
                    return unreadable;
                };
                let Some(key) = key else {
                    return Face::parse_within(&bytes, name, bold, italic, &mut budget);
                };
                let copy = copies.entry(part).or_insert_with(|| bytes.to_vec());
                fonts::xor_font_key(copy, &key);
                let face = Face::parse_within(copy, name, bold, italic, &mut budget);
                fonts::xor_font_key(copy, &key);
                face
            });
            match face {
                Ok(face) => out.push(face.restyled(name, bold, italic)),
                Err(FontError::TooLarge) => unread.push(name.to_string()),
                Err(_) => {}
            }
        }
    }
    Ok((out, unread))
}

fn model_of(
    txn: &Txn<'_>,
    main: &str,
    extra: Vec<Face>,
) -> Result<(model::Model, FontBook, Vec<String>, Vec<String>)> {
    let (embedded, unread) = embedded_fonts(txn, main)?;
    let mut embedded_names: Vec<String> = embedded.iter().map(|f| f.family.clone()).collect();
    embedded_names.dedup();
    let mut own = embedded;
    own.extend(extra);
    let mut book = FontBook::new(own);
    let model = model::build(txn, main, &mut book)?;
    Ok((model, book, embedded_names, unread))
}

/// Estimate the layout of `main`'s body. `fonts` are extra font files the
/// caller has (measured locally, never shipped); the document's embedded
/// fonts take precedence over them and over the bundled table.
pub fn layout(txn: &Txn<'_>, main: &str, extra: Vec<Face>) -> Result<Layout> {
    let (model, book, embedded_names, unread) = model_of(txn, main, extra)?;
    let mut by_node: HashMap<(StoryId, NodeId), Vec<String>> = HashMap::new();
    collect_bookmarks(&model.blocks, &mut by_node);
    let mut placed = Vec::new();
    let pages = flow::paginate(&model, &mut placed)?;
    let mut bookmarks = HashMap::new();
    for entry in &placed {
        if let Some(names) = entry.node.and_then(|n| by_node.get(&(StoryId::Body, n))) {
            for name in names {
                bookmarks.entry(name.clone()).or_insert(entry.page);
            }
        }
    }
    Ok(Layout {
        placed,
        pages,
        first_page_number: model
            .sections
            .first()
            .and_then(|s| s.page_start)
            .unwrap_or(1),
        missing_fonts: missing_fonts(&book.missing, unread),
        embedded_fonts: embedded_names,
        bookmarks,
    })
}

fn missing_fonts(substituted: &[String], unread: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = substituted.iter().map(|m| m.to_lowercase()).collect();
    let mut out = substituted.to_vec();
    out.extend(
        unread
            .into_iter()
            .filter(|family| seen.insert(family.to_lowercase())),
    );
    out
}

/// A laid-out line (`docrs layout --lines`). Experimental.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LineRecord {
    /// Physical page, from 1.
    pub page: u32,
    pub story: String,
    /// Points from the page's top-left corner, as are `baseline` and `tabs`.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub baseline: f64,
    /// Where each tab moves from and to.
    pub tabs: Vec<(f64, f64)>,
    /// The width left over before alignment; a justified line spreads it
    /// over its `gaps`.
    pub slack: f64,
    pub gaps: u32,
    /// Ends the paragraph, or in a line or page break.
    pub last: bool,
    /// `w:jc`'s value: `left`, `center`, `right`, `both`, `distribute`…
    pub align: &'static str,
    pub text: String,
}

/// Every line of `main`'s body. Experimental.
#[doc(hidden)]
pub fn line_records(txn: &Txn<'_>, main: &str, extra: Vec<Face>) -> Result<Vec<LineRecord>> {
    let (model, ..) = model_of(txn, main, extra)?;
    let mut dump = LineDump::default();
    flow::paginate(&model, &mut dump)?;
    Ok(dump.lines)
}

#[derive(Default)]
struct LineDump {
    page: u32,
    lines: Vec<LineRecord>,
}

impl flow::Sink for LineDump {
    fn page(&mut self, page: &flow::PageStart) -> Result<()> {
        self.page = page.number;
        Ok(())
    }

    fn line(&mut self, at: flow::Point, para: &model::Para, line: &flow::Line) -> Result<()> {
        let pieces = para
            .pieces
            .get(line.pieces.start as usize..line.pieces.end as usize)
            .unwrap_or_default();
        let text = pieces
            .iter()
            .filter_map(|piece| match piece {
                model::Piece::Char { ch, .. } => Some(*ch),
                model::Piece::Tab { .. } => Some('\t'),
                _ => None,
            })
            .collect();
        let story = match para.story {
            StoryId::Body => "body".to_string(),
            StoryId::Header(n) => format!("header{n}"),
            StoryId::Footer(n) => format!("footer{n}"),
        };
        self.lines.push(LineRecord {
            page: self.page,
            story,
            x: at.x + line.start,
            y: at.y,
            width: line.width,
            height: line.height,
            baseline: at.y + line.baseline,
            tabs: line
                .tabs
                .iter()
                .map(|tab| (at.x + tab.from, at.x + tab.to))
                .collect(),
            slack: line.slack,
            gaps: line.gaps,
            last: line.last,
            align: para.align.name(),
            text,
        });
        Ok(())
    }
}

fn collect_bookmarks(blocks: &[model::Block], out: &mut HashMap<(StoryId, NodeId), Vec<String>>) {
    for block in blocks {
        match block {
            model::Block::Para(para) => {
                if let (Some(node), false) = (para.node, para.bookmarks.is_empty()) {
                    out.insert((para.story, node), para.bookmarks.clone());
                }
            }
            model::Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_bookmarks(&cell.blocks, out);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::model::{Block, Para, StoryId};
    use super::*;

    #[test]
    fn bookmarks_are_keyed_by_story_and_node() {
        // Node ids are per part: a header paragraph can share one with a
        // body paragraph.
        let node = Some(NodeId(3));
        let para = |story, name: &str| {
            Block::Para(Para {
                story,
                node,
                bookmarks: vec![name.to_string()],
                ..Para::default()
            })
        };
        let blocks = [
            para(StoryId::Header(1), "inHeader"),
            para(StoryId::Body, "inBody"),
        ];
        let mut map = HashMap::new();
        collect_bookmarks(&blocks, &mut map);
        assert_eq!(map[&(StoryId::Body, NodeId(3))], ["inBody"]);
        assert_eq!(map[&(StoryId::Header(1), NodeId(3))], ["inHeader"]);
    }
}
