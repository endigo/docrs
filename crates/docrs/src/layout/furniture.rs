//! Headers and footers per page: which story each page shows, laid out at
//! the section's text width with the page's field values, and the body
//! area they leave.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::fields::{self, Context, LayoutWarning, Number, Values};
use super::flow::{CellLayout, Drawn, EPSILON, Item, Point, RowSlice, Sink, story_layout};
use super::model::{Model, Page, Section, Slots};
use crate::Result;

/// A story overflows its half of the page.
pub(crate) const STORY_OVERFLOW: &str = "pdf_story_overflow";

/// The body keeps at least this much of the page's height.
const MIN_BODY: f64 = 36.0;

/// The page being opened.
#[derive(Debug, Clone, Copy)]
pub(super) struct PageAt {
    pub page: u32,
    pub section: usize,
    /// The section's first page.
    pub first: bool,
}

/// The page's body area, from the page top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Body {
    pub top: f64,
    pub bottom: f64,
}

/// What a pass gives the next: every page's number and the warnings.
#[derive(Debug, Clone, Default)]
pub(super) struct Pages {
    pub count: u32,
    pub numbers: Vec<Number>,
    pub warnings: Vec<LayoutWarning>,
}

pub(super) struct Furniture<'m> {
    model: &'m Model,
    values: &'m Values,
    /// Stories without live fields, laid out once per text width.
    laid_out: HashMap<(usize, u64), CellLayout<'m>>,
    numbers: Vec<Number>,
    warnings: Vec<LayoutWarning>,
}

impl<'m> Furniture<'m> {
    pub(super) fn new(model: &'m Model, values: &'m Values) -> Self {
        Furniture {
            model,
            values,
            laid_out: HashMap::new(),
            numbers: Vec::new(),
            warnings: model.warnings.clone(),
        }
    }

    pub(super) fn finish(self) -> Pages {
        Pages {
            count: self.numbers.len() as u32,
            numbers: self.numbers,
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
        let page = &section.page;
        let context = Context {
            values: self.values,
            page: Some(number),
        };
        let header = self.pick(section.headers, at, number);
        let footer = self.pick(section.footers, at, number);
        let header = self.place(header, Side::Header, at, &context, sink, charge)?;
        let footer = self.place(footer, Side::Footer, at, &context, sink, charge)?;
        // Word pushes the body down (up) when the header (footer) runs past
        // the top (bottom) margin.
        let top = page.top.max(page.header + header);
        let bottom = page.height - page.bottom.max(page.footer + footer);
        Ok(Body {
            top,
            bottom: bottom.max(top + MIN_BODY),
        })
    }

    fn number(&self, at: PageAt, section: &Section) -> Number {
        let shown = match (self.numbers.last(), at.first, section.page_start) {
            (_, true, Some(start)) | (None, _, Some(start)) => start,
            (Some(previous), ..) => previous.shown.saturating_add(1),
            (None, ..) => 1,
        };
        Number {
            shown,
            format: section.number_format,
            section: at.section,
        }
    }

    /// The first page's story with a distinct first page, an even page's
    /// with distinct even pages, else the default; none when the section
    /// and those before it name none of that type.
    fn pick(&self, slots: Slots, at: PageAt, number: Number) -> Option<usize> {
        let section = &self.model.sections[at.section];
        if at.first && section.title_page {
            slots.first
        } else if self.model.even_and_odd && number.shown % 2 == 0 {
            slots.even
        } else {
            slots.default
        }
    }

    /// Emit a story's lines and give its height. A story keeps to its half
    /// of the page less half the body's least height: lines past that are
    /// not drawn.
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
        let room = (page.height / 2.0 - MIN_BODY / 2.0 - side.distance(page)).max(0.0);
        let bottom = |drawn: &Drawn<'_>| {
            drawn.offset
                + match &drawn.item {
                    Item::Line(_, line) => line.height,
                    Item::Row(_, _, height) => *height,
                }
        };
        let clipped = layout.content > room + EPSILON;
        let height = if clipped {
            layout
                .drawn
                .iter()
                .map(bottom)
                .filter(|b| *b <= room + EPSILON)
                .fold(0.0, f64::max)
        } else {
            layout.content
        };
        let top = match side {
            Side::Header => page.header,
            Side::Footer => page.height - page.footer - height,
        };
        let left = page.left + page.gutter;
        for drawn in layout
            .drawn
            .iter()
            .filter(|d| bottom(d) <= height + EPSILON)
        {
            let point = Point {
                x: left + drawn.x,
                y: top + drawn.offset,
            };
            charge(1)?;
            match &drawn.item {
                Item::Line(para, line) => sink.line(point, para, line)?,
                Item::Row(table, row, height) => {
                    sink.row(point, table, *row, &RowSlice::whole(*height))?
                }
            }
        }
        if clipped {
            self.warnings.push(LayoutWarning {
                code: STORY_OVERFLOW,
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
    Footer,
}

impl Side {
    /// The story's distance from the paper's edge.
    fn distance(self, page: &Page) -> f64 {
        match self {
            Side::Header => page.header,
            Side::Footer => page.footer,
        }
    }
}
