//! Where the painted lines put links, bookmarks and headings, for
//! [`Navigation`].

use std::collections::HashSet;

use super::nav::{LinkId, Navigation, Rect};
use crate::layout::flow::{Line, Point};
use crate::layout::model::{Model, Para, Piece, StoryId};

/// The navigation of the pages painted so far.
#[derive(Default)]
pub(super) struct Places {
    links: bool,
    outline: bool,
    pub(super) navigation: Navigation,
    /// The hyperlink the last linked piece painted belongs to.
    open: Option<Occurrence>,
    occurrences: usize,
    lines: usize,
    /// A repeated header row paints its paragraphs again.
    outlined: HashSet<*const Para>,
}

/// One hyperlink in the document: consecutive pieces of a paragraph with
/// the same link.
struct Occurrence {
    para: *const Para,
    piece: u32,
    link: u32,
    id: LinkId,
}

/// A line painted on `page`, `at` its column's left and its top from the
/// top of a page `height` tall.
pub(super) struct Painted<'a> {
    pub page: usize,
    pub height: f64,
    pub at: Point,
    pub para: &'a Para,
    pub line: &'a Line,
}

impl Places {
    pub(super) fn new(links: bool, outline: bool) -> Places {
        Places {
            links,
            outline,
            ..Places::default()
        }
    }

    /// Page 1's text area, where `_top` goes.
    pub(super) fn first_page(&mut self, left: f64, top: f64) {
        if self.links {
            self.navigation.text_area(left, top);
        }
    }

    pub(super) fn line(&mut self, model: &Model, painted: &Painted) {
        if self.links {
            self.links(model, painted);
            self.bookmarks(painted);
        }
        if self.outline {
            self.heading(painted);
        }
        self.lines += 1;
    }

    /// One rectangle per hyperlink on the line, from its top to its bottom
    /// and from its first piece's x to the next piece's, so the spaces
    /// between its words and their justified gaps are inside it.
    fn links(&mut self, model: &Model, painted: &Painted) {
        let Painted { para, line, .. } = *painted;
        let (origins, _) = line.origins(para);
        let first = line.pieces.start;
        let pieces = para
            .pieces
            .get(first as usize..line.pieces.end as usize)
            .unwrap_or_default();
        let mut parts: Vec<(LinkId, u32, f64, f64)> = Vec::new();
        for (at, piece) in pieces.iter().enumerate() {
            let Some(link) = link_of(model, piece) else {
                continue;
            };
            let id = self.occurrence(para, first + at as u32, link);
            let Some(end) = piece_end(line, &origins, first, at, piece) else {
                continue;
            };
            match parts.last_mut() {
                Some(part) if part.0 == id => part.3 = end,
                _ => parts.push((id, link, origins[at], end)),
            }
        }
        let top = painted.height - painted.at.y;
        for (id, link, from, to) in parts {
            let rect = Rect {
                left: painted.at.x + from,
                bottom: top - line.height,
                right: painted.at.x + to,
                top,
            };
            self.navigation.link(
                id,
                painted.page,
                self.lines,
                rect,
                &model.links[link as usize],
            );
        }
    }

    /// The hyperlink piece `piece` of `para` belongs to: the open one when
    /// it continues it.
    fn occurrence(&mut self, para: &Para, piece: u32, link: u32) -> LinkId {
        let para = para as *const Para;
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.para == para && open.link == link && open.piece + 1 == piece)
        {
            open.piece = piece;
            return open.id;
        }
        let id = LinkId(self.occurrences);
        self.occurrences += 1;
        self.open = Some(Occurrence {
            para,
            piece,
            link,
            id,
        });
        id
    }

    /// The bookmarks before a piece of the line, and on its last line those
    /// after the paragraph's text, at the line's top where its text starts.
    fn bookmarks(&mut self, painted: &Painted) {
        let Painted { para, line, .. } = *painted;
        let last = line.pieces.end as usize >= para.pieces.len();
        let left = painted.at.x + line.label.unwrap_or(line.start);
        let top = painted.height - painted.at.y;
        for bookmark in &para.bookmarks {
            if line.pieces.contains(&bookmark.piece) || (last && bookmark.piece >= line.pieces.end)
            {
                self.navigation
                    .destination(&bookmark.name, painted.page, left, top);
            }
        }
    }

    /// A body paragraph with an outline level, at its first line's top.
    fn heading(&mut self, painted: &Painted) {
        let Painted { para, line, .. } = *painted;
        let Some(level) = para.outline else {
            return;
        };
        if para.story != StoryId::Body
            || line.pieces.start != 0
            || !self.outlined.insert(para as *const Para)
        {
            return;
        }
        let top = painted.height - painted.at.y;
        self.navigation
            .heading(level + 1, &title(para), painted.page, top);
    }
}

/// The link of a piece that can be part of one.
fn link_of(model: &Model, piece: &Piece) -> Option<u32> {
    match piece {
        Piece::Char { look, .. } | Piece::Tab { look, .. } | Piece::LineBreak { look, .. } => {
            model.looks[*look as usize].link
        }
        Piece::PageBreak | Piece::Object { .. } => None,
    }
}

/// Where a character or tab ends: the next piece's x, else its own end.
fn piece_end(line: &Line, origins: &[f64], first: u32, at: usize, piece: &Piece) -> Option<f64> {
    let own = match piece {
        Piece::Char { width, .. } => origins[at] + width,
        Piece::Tab { .. } => line
            .tabs
            .iter()
            .find(|tab| tab.piece == first + at as u32)
            .map_or(origins[at], |tab| tab.to),
        _ => return None,
    };
    Some(origins.get(at + 1).copied().unwrap_or(own))
}

/// A heading's text with its list label, as the outline shows it.
fn title(para: &Para) -> String {
    let text = |pieces: &[Piece]| -> String {
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Char { ch, .. } => *ch,
                _ => ' ',
            })
            .collect()
    };
    match &para.label {
        Some(label) => format!("{} {}", text(&label.pieces), text(&para.pieces)),
        None => text(&para.pieces),
    }
}
