//! Where the painted lines put links, bookmarks and headings, for
//! [`Navigation`].

use std::collections::HashSet;

use super::nav::{LinkId, Navigation, Rect};
use crate::layout::flow::{Line, Point};
use crate::layout::model::{Align, Model, Para, Piece, StoryId};

/// Word's link rectangles reach this far, in points, past the text's ends
/// (measured in Word for Mac's PDFs).
const LINK_MARGIN: f64 = 2.16;

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

    /// One rectangle per hyperlink on the line, from its first piece to
    /// the end of its last drawn one, the spaces between its words and
    /// their justified gaps included. As Word's, it reaches
    /// [`LINK_MARGIN`] past each end, to the right edge of the text when
    /// the link goes on onto the next line, and on the paragraph's last
    /// line down through its space after.
    fn links(&mut self, model: &Model, painted: &Painted) {
        let Painted { para, line, .. } = *painted;
        let (origins, _) = line.origins(para);
        let first = line.pieces.start;
        let pieces = para
            .pieces
            .get(first as usize..line.pieces.end as usize)
            .unwrap_or_default();
        let drawn = pieces.iter().rposition(|piece| match piece {
            Piece::Char { ch, .. } => *ch != ' ',
            Piece::Tab { .. } | Piece::Object { .. } => true,
            Piece::LineBreak { .. } | Piece::PageBreak => false,
        });
        let mut parts: Vec<(LinkId, u32, f64, f64)> = Vec::new();
        for (at, piece) in pieces.iter().enumerate() {
            let Some(link) = link_of(model, piece) else {
                continue;
            };
            let id = self.occurrence(para, first + at as u32, link);
            let end = match piece_end(line, &origins, first, at, piece) {
                Some(end) if drawn.is_some_and(|last| at <= last) => end,
                _ => continue,
            };
            match parts.last_mut() {
                Some(part) if part.0 == id => part.3 = end,
                _ => parts.push((id, link, origins[at], end)),
            }
        }
        let continued = continued_link(model, para, line);
        let top = painted.height - painted.at.y;
        let after = if line.pieces.end as usize >= para.pieces.len() {
            para.after_spacing()
        } else {
            0.0
        };
        let count = parts.len();
        for (index, (id, link, from, to)) in parts.into_iter().enumerate() {
            let to = if index + 1 == count && continued == Some(link) {
                text_right(para, line)
            } else {
                to
            };
            let rect = Rect {
                left: painted.at.x + from - LINK_MARGIN,
                bottom: top - line.height - after,
                right: painted.at.x + to + LINK_MARGIN,
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

/// The link that runs from the end of `line` onto the next line.
fn continued_link(model: &Model, para: &Para, line: &Line) -> Option<u32> {
    let end = line.pieces.end as usize;
    let last = link_of(model, para.pieces.get(end.checked_sub(1)?)?)?;
    let next = link_of(model, para.pieces.get(end)?)?;
    (last == next).then_some(last)
}

/// The x, from the column's left, where `line`'s text area ends: past its
/// content by the slack its alignment did not put before it.
fn text_right(para: &Para, line: &Line) -> f64 {
    let before = match para.align {
        Align::Center => line.slack / 2.0,
        Align::Right => line.slack,
        _ => 0.0,
    };
    line.start + line.width + line.slack - before
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

/// Where a character or tab ends.
fn piece_end(line: &Line, origins: &[f64], first: u32, at: usize, piece: &Piece) -> Option<f64> {
    Some(match piece {
        Piece::Char { width, .. } => origins[at] + width,
        Piece::Tab { .. } => line
            .tabs
            .iter()
            .find(|tab| tab.piece == first + at as u32)
            .map_or(origins[at], |tab| tab.to),
        _ => return None,
    })
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
