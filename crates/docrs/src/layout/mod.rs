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

use std::collections::HashMap;

pub use flow::Placed;
pub use fonts::{Face, FontBook};

use crate::Result;
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::NodeId;

/// The estimated layout of a document's body.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Every paragraph (and table row end) in Word's order.
    pub placed: Vec<Placed>,
    /// Physical page count.
    pub pages: u32,
    /// The number the first page shows (`w:pgNumType w:start`, else 1).
    pub first_page_number: i64,
    /// Font families with no metrics, substituted.
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
/// font table), deobfuscated and measured.
fn embedded_fonts(txn: &Txn<'_>, main: &str) -> Result<Vec<Face>> {
    let mut out = Vec::new();
    let Some(table) = txn.part_related_by(main, rt::FONT_TABLE)? else {
        return Ok(out);
    };
    let tree = txn.tree(&table)?;
    let rels = txn.relationships(&table)?;
    let table_uri = format!("/{}", table.trim_start_matches('/'));
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
            let Some(id) = tree.attr(embed, Some(crate::authoring::R), "id") else {
                continue;
            };
            let Some(rel) = rels.iter().find(|r| r.id == id) else {
                continue;
            };
            let target = rel.target_part_name(crate::package::rels::base_uri(&table_uri));
            let Some(part) = txn.resolve(&target) else {
                continue;
            };
            let Some(bytes) = txn.binary(&part) else {
                continue;
            };
            let data = match w_attr(tree, embed, "fontKey") {
                Some(key) => match fonts::deobfuscate(&bytes, key) {
                    Some(data) => data,
                    None => continue,
                },
                None => bytes.to_vec(),
            };
            if let Some(face) = Face::parse(&data, name, bold, italic) {
                out.push(face);
            }
        }
    }
    Ok(out)
}

/// Estimate the layout of `main`'s body. `fonts` are extra font files the
/// caller has (measured locally, never shipped); the document's embedded
/// fonts take precedence over them and over the bundled table.
pub fn layout(txn: &Txn<'_>, main: &str, extra: Vec<Face>) -> Result<Layout> {
    let embedded = embedded_fonts(txn, main)?;
    let mut embedded_names: Vec<String> = embedded.iter().map(|f| f.family.clone()).collect();
    embedded_names.dedup();
    let mut own = embedded;
    own.extend(extra);
    let mut book = FontBook::new(own);
    let model = model::build(txn, main, &mut book)?;
    let mut by_node: HashMap<NodeId, Vec<String>> = HashMap::new();
    collect_bookmarks(&model.blocks, &mut by_node);
    let (placed, pages) = flow::paginate(&model);
    let mut bookmarks = HashMap::new();
    for entry in &placed {
        if let Some(names) = entry.node.and_then(|n| by_node.get(&n)) {
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
        missing_fonts: book.missing.clone(),
        embedded_fonts: embedded_names,
        bookmarks,
    })
}

fn collect_bookmarks(blocks: &[model::Block], out: &mut HashMap<NodeId, Vec<String>>) {
    for block in blocks {
        match block {
            model::Block::Para(para) => {
                if let (Some(node), false) = (para.node, para.bookmarks.is_empty()) {
                    out.insert(node, para.bookmarks.clone());
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
