//! Headers and footers per page: which story each page shows, laid out at
//! the section's text width with the page's field values, and the body
//! area they leave.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::fields::{self, Context, LayoutWarning, Number, Values};
use super::flow::{CellLayout, Drawn, EPSILON, Item, Point, Sink, story_layout};
use super::model::{Model, Section, Slots};
use crate::Result;

/// A story overflows its half of the page.
pub(crate) const STORY_OVERFLOW: &str = "pdf_story_overflow";

/// The body keeps at least this much of the page's height.
const MIN_BODY: f64 = 36.0;

/// A story's lines start less than half the page less this from its edge.
const STORY_CAP_MARGIN: f64 = 10.0;

/// The page being opened.
#[derive(Debug, Clone, Copy)]
pub(super) struct PageAt {
    pub page: u32,
    pub section: usize,
    /// The section's first page.
    pub first: bool,
    /// Added before an odd or even section start: Word draws nothing on it.
    pub blank: bool,
}

/// The page's body area, from the page top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Body {
    pub top: f64,
    pub bottom: f64,
}

/// What a pass gives the next: every page's number and the warnings.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pages {
    pub count: u32,
    pub numbers: Vec<Number>,
    /// Blank pages added before odd and even starts, which no section counts.
    pub blank: Vec<u32>,
    pub warnings: Vec<LayoutWarning>,
}

pub(super) struct Furniture<'m> {
    model: &'m Model,
    values: &'m Values,
    /// Stories without live fields, laid out once per text width.
    laid_out: HashMap<(usize, u64), CellLayout<'m>>,
    numbers: Vec<Number>,
    blank: Vec<u32>,
    warnings: Vec<LayoutWarning>,
}

impl<'m> Furniture<'m> {
    pub(super) fn new(model: &'m Model, values: &'m Values) -> Self {
        Furniture {
            model,
            values,
            laid_out: HashMap::new(),
            numbers: Vec::new(),
            blank: Vec::new(),
            warnings: model.warnings.clone(),
        }
    }

    pub(super) fn finish(self) -> Pages {
        Pages {
            count: self.numbers.len() as u32,
            numbers: self.numbers,
            blank: self.blank,
            warnings: self.warnings,
        }
    }

    /// Number the page, lay out and emit its header and footer, and give
    /// the body area they leave. `charge` counts work.
    pub(super) fn page<S: Sink>(
        &mut self,
        at: PageAt,
        sink: &mut S,
        charge: &mut dyn FnMut(u64) -> Result<()>,
    ) -> Result<Body> {
        let section = &self.model.sections[at.section];
        let number = self.number(at, section);
        self.numbers.push(number);
        if at.blank {
            self.blank.push(at.page);
        }
        let page = &section.page;
        let context = Context {
            values: self.values,
            page: Some(number),
        };
        let header = self.pick(section.headers, at, number);
        let footer = self.pick(section.footers, at, number);
        let header = self.place(header, Side::Header, at, &context, sink, charge)?;
        // Word pushes the body down (up) when the header (footer) runs past
        // the top (bottom) margin. A footer leaves the body at least its
        // least height below the header.
        let top = page.top.max(page.header + header);
        let room = page.height - page.footer - top - MIN_BODY;
        let footer = self.place(footer, Side::Footer(room), at, &context, sink, charge)?;
        let bottom = page.height - page.bottom.max(page.footer + footer);
        Ok(Body {
            top,
            bottom: bottom.max(top + MIN_BODY),
        })
    }

    fn number(&self, at: PageAt, section: &Section) -> Number {
        Number {
            shown: self.next_number(at.first, section),
            format: section.number_format,
            section: at.section,
        }
    }

    /// The number the next page shows: its section's start on the
    /// section's `first` page, else one more than the page before.
    pub(super) fn next_number(&self, first: bool, section: &Section) -> i64 {
        match (self.numbers.last(), first, section.page_start) {
            (_, true, Some(start)) | (None, _, Some(start)) => start,
            (Some(previous), ..) => previous.shown.saturating_add(1),
            (None, ..) => 1,
        }
    }

    /// The first page's story with a distinct first page, an even page's
    /// with distinct even pages, else the default; none when the section
    /// and those before it name none of that type.
    fn pick(&self, slots: Slots, at: PageAt, number: Number) -> Option<usize> {
        let section = &self.model.sections[at.section];
        if at.blank {
            None
        } else if at.first && section.title_page {
            slots.first
        } else if self.model.even_and_odd && number.shown % 2 == 0 {
            slots.even
        } else {
            slots.default
        }
    }

    /// Emit a story's lines and give its height, at most a footer's room. Lines from
    /// half the page less 10 pt past the story's edge on are not drawn: a
    /// header's below the header distance, a footer's above its bottom
    /// (measured from its top, as for a header); nor are a footer's lines
    /// past its room.
    fn place<S: Sink>(
        &mut self,
        story: Option<usize>,
        side: Side,
        at: PageAt,
        context: &Context<'_>,
        sink: &mut S,
        charge: &mut dyn FnMut(u64) -> Result<()>,
    ) -> Result<f64> {
        let Some(index) = story else {
            return Ok(0.0);
        };
        let model = self.model;
        let page = &model.sections[at.section].page;
        let story = &model.stories[index];
        let shown;
        let owned;
        let layout: &CellLayout<'_> = if story.fields {
            let mut blocks = story.blocks.clone();
            fields::show(&mut blocks, context);
            shown = blocks;
            owned = story_layout(&shown, &model.metrics, page.text_width, model.default_tab);
            charge(owned.work)?;
            &owned
        } else {
            match self.laid_out.entry((index, page.text_width.to_bits())) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let layout = story_layout(
                        &story.blocks,
                        &model.metrics,
                        page.text_width,
                        model.default_tab,
                    );
                    charge(layout.work)?;
                    entry.insert(layout)
                }
            }
        };
        // Word for Mac draws a header's lines while their top is less than
        // half the page less 10 pt below the header distance, whatever the
        // margins; the body then starts below the last one's space after.
        let cap = page.height / 2.0 - STORY_CAP_MARGIN;
        let room = match side {
            Side::Header => f64::INFINITY,
            Side::Footer(room) => room.max(0.0),
        };
        let bottom = |drawn: &Drawn<'_>, after: bool| {
            drawn.offset
                + match &drawn.item {
                    Item::Line(para, line) if after && line.last => {
                        line.height + para.after_spacing()
                    }
                    Item::Line(_, line) => line.height,
                    Item::Row(_, _, slice) => slice.height,
                }
        };
        let shown =
            |drawn: &&Drawn<'_>| drawn.offset < cap && bottom(drawn, false) <= room + EPSILON;
        let clipped = layout.drawn.iter().any(|d| !shown(&d));
        let height = if clipped {
            layout
                .drawn
                .iter()
                .filter(shown)
                .map(|drawn| bottom(drawn, true))
                .fold(0.0, f64::max)
        } else {
            layout.content
        }
        .min(room);
        let top = match side {
            Side::Header => page.header,
            Side::Footer(_) => page.height - page.footer - height,
        };
        let left = page.left + page.gutter;
        for drawn in layout.drawn.iter().filter(shown) {
            let point = Point {
                x: left + drawn.x,
                y: top + drawn.offset,
            };
            charge(1)?;
            match &drawn.item {
                Item::Line(para, line) => sink.line(point, para, line)?,
                Item::Row(table, row, slice) => sink.row(point, table, *row, slice)?,
            }
        }
        if clipped {
            self.warnings.push(LayoutWarning {
                code: STORY_OVERFLOW,
                subject: story.id.kind(),
                story: story.id,
                page: Some(at.page),
            });
        }
        Ok(height)
    }
}

#[derive(Clone, Copy)]
enum Side {
    Header,
    /// The most height it may take.
    Footer(f64),
}
