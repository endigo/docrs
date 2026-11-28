//! Paragraph borders and shading: the room borders take beside the text,
//! and the box each paragraph draws on a page.
//!
//! Consecutive paragraphs with equal borders and indents form one group:
//! the top border above the first, `between` above each of the others, the
//! bottom border below the last.

use crate::layout::model::{Block, Border, Para, Rgb};

/// Word for Mac draws a paragraph's box this far outside its indents
/// (measured: borders and fills alike, with or without side borders).
const OUTSET: f64 = 1.44;

/// `para` shares `previous`'s box.
fn joins(previous: &Para, para: &Para) -> bool {
    previous.borders == para.borders && previous.left == para.left && previous.right == para.right
}

fn para_at(blocks: &[Block], index: usize) -> Option<&Para> {
    match blocks.get(index)? {
        Block::Para(para) => Some(para),
        Block::Table(_) => None,
    }
}

/// How `blocks[index]` joins the paragraphs before and after it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Joins {
    pub previous: bool,
    pub next: bool,
}

impl Joins {
    pub(super) fn of(blocks: &[Block], index: usize) -> Joins {
        let Some(para) = para_at(blocks, index) else {
            return Joins::default();
        };
        let previous = index.checked_sub(1).and_then(|i| para_at(blocks, i));
        Joins {
            previous: previous.is_some_and(|previous| joins(previous, para)),
            next: para_at(blocks, index + 1).is_some_and(|next| joins(para, next)),
        }
    }

    /// The border above `para`'s text: the group's top, or `between`.
    pub(super) fn top(self, para: &Para) -> Option<Border> {
        let borders = para.borders.as_deref()?;
        if self.previous {
            borders.between
        } else {
            borders.top
        }
    }

    /// The group's bottom border, below its last paragraph's text.
    pub(super) fn bottom(self, para: &Para) -> Option<Border> {
        para.borders.as_deref().filter(|_| !self.next)?.bottom
    }

    /// The room the line above `para`'s text takes, and where in it the
    /// line starts. Word gives `between` its space both above and below
    /// (measured: Times New Roman 12 pt, ½ pt line, space 4, after 6:
    /// pitch 28.32 for a 13.8 pt line).
    fn lead(self, para: &Para) -> (f64, f64) {
        match self.top(para) {
            Some(border) if self.previous => (border.extent() + border.space, border.space),
            Some(border) => (border.extent(), 0.0),
            None => (0.0, 0.0),
        }
    }

    /// The room the borders take above and below `para`'s text.
    pub(super) fn extents(self, para: &Para) -> (f64, f64) {
        let bottom = self.bottom(para).map_or(0.0, |b| b.extent());
        (self.lead(para).0, bottom)
    }
}

/// A paragraph's borders and shading on one page, points from the page's
/// top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ParaBox {
    /// The outer edges of the lines drawn around the text.
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub top_line: Option<Border>,
    pub bottom_line: Option<Border>,
    pub left_line: Option<Border>,
    pub right_line: Option<Border>,
    /// Where the side lines start and end: Word runs them through the
    /// spacing the paragraph shares with its neighbours (measured).
    pub sides: (f64, f64),
    pub fill: Option<Fill>,
}

/// A paragraph's shading: its colour, its left and right edges, and its
/// bottom, which reaches the next box when the next paragraph joins this
/// one and is shaded too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fill {
    pub colour: Rgb,
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
}

impl ParaBox {
    /// The box of `para`'s lines from `top` to `bottom`, the room its
    /// lines above and below take included: the top or between line when
    /// `opening` its first line, the bottom line when `closing` its last.
    /// `column`: the column's left edge and width. `None` when it has no
    /// borders and no shading.
    pub(super) fn new(
        para: &Para,
        column: (f64, f64),
        (top, bottom): (f64, f64),
        joins: Joins,
        (opening, closing): (bool, bool),
    ) -> Option<ParaBox> {
        if para.borders.is_none() && para.shade.is_none() {
            return None;
        }
        let borders = para.borders.as_deref();
        let left_line = borders.and_then(|b| b.left);
        let right_line = borders.and_then(|b| b.right);
        let (x, width) = column;
        let text = (x + para.left + para.first.min(0.0), x + width - para.right);
        let side = |line: Option<Border>| OUTSET + line.map_or(0.0, |b| b.extent());
        let (left, right) = (text.0 - side(left_line), text.1 + side(right_line));
        // Word fills up to the side lines in a box closed above or below,
        // else only to the outset (measured).
        let closed = borders.is_some_and(|b| b.top.is_some() || b.bottom.is_some());
        let inner = |line: Option<Border>| line.map_or(0.0, |b| b.thickness());
        let fill = para.shade.map(|colour| Fill {
            colour,
            left: if closed {
                left + inner(left_line)
            } else {
                text.0 - OUTSET
            },
            right: if closed {
                right - inner(right_line)
            } else {
                text.1 + OUTSET
            },
            bottom,
        });
        let offset = if opening { joins.lead(para).1 } else { 0.0 };
        Some(ParaBox {
            left,
            right,
            top: top + offset,
            bottom,
            top_line: joins.top(para).filter(|_| opening),
            bottom_line: joins.bottom(para).filter(|_| closing),
            left_line,
            right_line,
            sides: (top, bottom),
            fill,
        })
    }

    /// Run the side lines up through `share` of the spacing above, unless a
    /// top border closes the box there.
    pub(super) fn reach_up(&mut self, share: f64, joined: bool) {
        if joined || self.top_line.is_none() {
            self.sides.0 -= share;
        }
    }

    /// The paragraph that follows on this page, `gap` below this box, of
    /// which `after` is this paragraph's space after. The fill reaches a
    /// shaded paragraph that joins, down to its between line; the side lines
    /// run through `after`, but end with the fill of a shaded paragraph that
    /// ends its group (measured).
    pub(super) fn reach_down(&mut self, after: f64, gap: f64, next: Option<&Para>) {
        match (&mut self.fill, next) {
            (Some(fill), Some(next)) if next.shade.is_some() => {
                let between = next.borders.as_deref().and_then(|b| b.between);
                fill.bottom += gap + between.map_or(0.0, |b| b.space);
                self.sides.1 = self.bottom + gap;
            }
            (None, _) | (Some(_), Some(_)) if self.bottom_line.is_none() => self.sides.1 += after,
            _ => {}
        }
    }

    /// The box `shift` lower down the page.
    pub(super) fn moved_down(self, shift: f64) -> ParaBox {
        ParaBox {
            top: self.top + shift,
            bottom: self.bottom + shift,
            sides: (self.sides.0 + shift, self.sides.1 + shift),
            fill: self.fill.map(|fill| Fill {
                bottom: fill.bottom + shift,
                ..fill
            }),
            ..self
        }
    }

    /// End the box, its side lines and its fill by `limit`.
    pub(super) fn end_by(&mut self, limit: f64) {
        self.bottom = self.bottom.min(limit);
        self.sides.1 = self.sides.1.min(limit);
        if let Some(fill) = &mut self.fill {
            fill.bottom = fill.bottom.min(limit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{METRICS, model, para};
    use super::super::{Line, PageStart, Point, Sink, Values, cell_layout, paginate};
    use super::*;
    use crate::Result;
    use crate::layout::model::{BorderStyle, Colour, ParaBorders};

    /// Each line's page and top, and each box with its page.
    #[derive(Default)]
    struct Drawn {
        page: u32,
        lines: Vec<(u32, f64)>,
        boxes: Vec<(u32, ParaBox)>,
    }

    impl Sink for Drawn {
        fn page(&mut self, page: &PageStart) -> Result<()> {
            self.page = page.number;
            Ok(())
        }

        fn line(&mut self, at: Point, _: &Para, _: &Line) -> Result<()> {
            self.lines.push((self.page, at.y));
            Ok(())
        }

        fn para_box(&mut self, boxed: &ParaBox) -> Result<()> {
            self.boxes.push((self.page, *boxed));
            Ok(())
        }
    }

    /// Lines from 10 points below the page top to 10 above its bottom.
    fn draw(paras: Vec<Para>, page_height: f64) -> Drawn {
        let mut drawn = Drawn::default();
        let blocks = paras.into_iter().map(Block::Para).collect();
        paginate(&model(blocks, page_height), &Values::default(), &mut drawn).unwrap();
        drawn
    }

    fn with(borders: ParaBorders, para: Para) -> Para {
        Para {
            borders: Some(Box::new(borders)),
            ..para
        }
    }

    /// Tops and bottoms.
    fn spans(drawn: &Drawn) -> Vec<(u32, f64, f64)> {
        drawn
            .boxes
            .iter()
            .map(|(page, b)| (*page, b.top, b.bottom))
            .collect()
    }

    fn line(width: f64, space: f64) -> Border {
        Border {
            style: BorderStyle::Single,
            width,
            space,
            colour: Colour::Auto,
        }
    }

    fn bordered(borders: ParaBorders) -> Block {
        Block::Para(Para {
            borders: Some(Box::new(borders)),
            ..Para::default()
        })
    }

    #[test]
    fn equal_borders_join_into_one_group() {
        let boxed = ParaBorders {
            top: Some(line(1.0, 2.0)),
            bottom: Some(line(0.5, 4.0)),
            between: Some(line(0.25, 1.0)),
            ..ParaBorders::default()
        };
        let blocks = [bordered(boxed), bordered(boxed), bordered(boxed)];
        let extents: Vec<_> = (0..3)
            .map(|i| match &blocks[i] {
                Block::Para(para) => Joins::of(&blocks, i).extents(para),
                Block::Table(_) => unreachable!(),
            })
            .collect();
        assert_eq!(extents, [(3.0, 0.0), (2.25, 0.0), (2.25, 4.5)]);
    }

    #[test]
    fn different_borders_or_indents_keep_their_own_box() {
        let rule = ParaBorders {
            bottom: Some(line(0.75, 1.0)),
            ..ParaBorders::default()
        };
        let thicker = ParaBorders {
            bottom: Some(line(1.5, 1.0)),
            ..ParaBorders::default()
        };
        let mut indented = bordered(rule);
        if let Block::Para(para) = &mut indented {
            para.left = 36.0;
        }
        let blocks = [bordered(rule), bordered(thicker), indented, bordered(rule)];
        for index in 0..blocks.len() {
            assert_eq!(Joins::of(&blocks, index), Joins::default(), "{index}");
        }
    }

    #[test]
    fn the_box_sits_outside_the_indents_and_the_side_borders() {
        let para = Para {
            left: 24.0,
            right: 12.0,
            borders: Some(Box::new(ParaBorders {
                left: Some(line(2.25, 8.0)),
                ..ParaBorders::default()
            })),
            ..Para::default()
        };
        let boxed = ParaBox::new(
            &para,
            (60.0, 400.0),
            (100.0, 120.0),
            Joins::default(),
            (true, true),
        )
        .unwrap();
        assert!((boxed.left - (60.0 + 24.0 - 8.0 - 2.25 - OUTSET)).abs() < 1e-9);
        assert!((boxed.right - (460.0 - 12.0 + OUTSET)).abs() < 1e-9);
        assert_eq!((boxed.top, boxed.sides), (100.0, (100.0, 120.0)));
        assert!(
            ParaBox::new(
                &Para::default(),
                (0.0, 100.0),
                (0.0, 1.0),
                Joins::default(),
                (true, true)
            )
            .is_none()
        );
    }

    #[test]
    fn borders_take_their_width_and_space_beside_the_text() {
        let plain = draw(vec![para(1, 20.0), para(1, 20.0)], 60.0);
        assert_eq!(plain.lines, [(1, 10.0), (1, 20.0)]);
        assert!(plain.boxes.is_empty());
        let ruled = ParaBorders {
            bottom: Some(line(1.0, 2.0)),
            ..ParaBorders::default()
        };
        let topped = ParaBorders {
            top: Some(line(0.5, 1.0)),
            ..ParaBorders::default()
        };
        let drawn = draw(
            vec![with(ruled, para(1, 20.0)), with(topped, para(1, 20.0))],
            60.0,
        );
        assert_eq!(drawn.lines, [(1, 10.0), (1, 24.5)]);
        assert_eq!(spans(&drawn), [(1, 10.0, 23.0), (1, 23.0, 34.5)]);
        assert_eq!(drawn.boxes[0].1.bottom_line, ruled.bottom);
        assert_eq!(drawn.boxes[1].1.top_line, topped.top);
    }

    #[test]
    fn a_group_draws_its_top_between_and_bottom_once_and_its_sides_throughout() {
        let boxed = ParaBorders {
            top: Some(line(1.0, 2.0)),
            bottom: Some(line(1.0, 2.0)),
            between: Some(line(0.5, 1.0)),
            left: Some(line(0.5, 4.0)),
            ..ParaBorders::default()
        };
        let member = || {
            with(
                boxed,
                Para {
                    after: 4.0,
                    ..para(1, 20.0)
                },
            )
        };
        let drawn = draw(vec![member(), member(), member()], 100.0);
        assert_eq!(drawn.lines, [(1, 13.0), (1, 29.5), (1, 46.0)]);
        assert_eq!(
            spans(&drawn),
            [(1, 10.0, 23.0), (1, 28.0, 39.5), (1, 44.5, 59.0)]
        );
        let lines: Vec<_> = drawn
            .boxes
            .iter()
            .map(|(_, b)| (b.top_line, b.bottom_line))
            .collect();
        assert_eq!(
            lines,
            [
                (boxed.top, None),
                (boxed.between, None),
                (boxed.between, boxed.bottom)
            ]
        );
        let sides: Vec<_> = drawn.boxes.iter().map(|(_, b)| b.sides).collect();
        assert_eq!(sides, [(10.0, 27.0), (27.0, 43.5), (43.5, 59.0)]);
    }

    #[test]
    fn a_split_paragraph_draws_a_box_per_page_open_at_the_break() {
        let boxed = ParaBorders {
            top: Some(line(1.0, 2.0)),
            bottom: Some(line(1.0, 2.0)),
            ..ParaBorders::default()
        };
        let drawn = draw(vec![with(boxed, para(12, 45.0))], 60.0);
        assert_eq!(
            drawn.lines,
            [
                (1, 13.0),
                (1, 23.0),
                (1, 33.0),
                (2, 10.0),
                (2, 20.0),
                (2, 30.0)
            ]
        );
        assert_eq!(spans(&drawn), [(1, 10.0, 43.0), (2, 10.0, 43.0)]);
        let lines: Vec<_> = drawn
            .boxes
            .iter()
            .map(|(_, b)| (b.top_line, b.bottom_line))
            .collect();
        assert_eq!(lines, [(boxed.top, None), (None, boxed.bottom)]);
    }

    #[test]
    fn a_group_split_between_paragraphs_carries_on_with_between() {
        let boxed = ParaBorders {
            top: Some(line(1.0, 2.0)),
            bottom: Some(line(1.0, 2.0)),
            between: Some(line(0.5, 1.0)),
            ..ParaBorders::default()
        };
        let drawn = draw(
            vec![with(boxed, para(6, 45.0)), with(boxed, para(2, 45.0))],
            60.0,
        );
        assert_eq!(drawn.lines, [(1, 13.0), (1, 23.0), (1, 33.0), (2, 12.5)]);
        assert_eq!(spans(&drawn), [(1, 10.0, 43.0), (2, 11.0, 25.5)]);
        assert_eq!(drawn.boxes[1].1.top_line, boxed.between);
    }

    #[test]
    fn a_fill_runs_through_the_gap_to_a_shaded_paragraph_that_joins() {
        let grey = Rgb {
            r: 0xF5,
            g: 0xF5,
            b: 0xF5,
        };
        let code = || Para {
            shade: Some(grey),
            after: 10.0,
            ..para(1, 20.0)
        };
        let drawn = draw(vec![code(), code(), para(1, 20.0)], 60.0);
        let fills: Vec<_> = drawn
            .boxes
            .iter()
            .map(|(_, b)| b.fill.map(|f| (f.colour, f.bottom)))
            .collect();
        assert_eq!(fills, [Some((grey, 30.0)), Some((grey, 40.0))]);
        assert_eq!(spans(&drawn), [(1, 10.0, 20.0), (1, 30.0, 40.0)]);
    }

    #[test]
    fn side_lines_run_on_from_a_shaded_paragraph_to_an_unshaded_one_that_joins() {
        let boxed = ParaBorders {
            top: Some(line(1.0, 2.0)),
            bottom: Some(line(1.0, 2.0)),
            between: Some(line(0.5, 1.0)),
            left: Some(line(0.5, 4.0)),
            right: Some(line(0.5, 4.0)),
        };
        let grey = Rgb {
            r: 0xF5,
            g: 0xF5,
            b: 0xF5,
        };
        let member = |shade| {
            with(
                boxed,
                Para {
                    shade,
                    after: 6.0,
                    ..para(1, 20.0)
                },
            )
        };
        let drawn = draw(
            vec![
                member(Some(grey)),
                member(None),
                member(Some(grey)),
                member(Some(grey)),
            ],
            200.0,
        );
        let sides: Vec<_> = drawn.boxes.iter().map(|(_, b)| b.sides).collect();
        for pair in sides.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "{sides:?}");
        }
    }

    #[test]
    fn side_lines_take_the_spacing_above_that_is_not_the_previous_paragraphs() {
        let quote = ParaBorders {
            left: Some(line(2.25, 8.0)),
            ..ParaBorders::default()
        };
        let drawn = draw(
            vec![
                Para {
                    after: 4.0,
                    ..para(1, 20.0)
                },
                with(
                    quote,
                    Para {
                        before: 5.0,
                        after: 5.0,
                        ..para(1, 20.0)
                    },
                ),
            ],
            60.0,
        );
        assert_eq!(drawn.lines, [(1, 10.0), (1, 25.0)]);
        assert_eq!(drawn.boxes[0].1.sides, (24.0, 35.0));
    }

    #[test]
    fn borders_add_to_the_height_of_a_header_or_cell() {
        let ruled = ParaBorders {
            bottom: Some(line(0.75, 1.0)),
            ..ParaBorders::default()
        };
        let blocks = [with(ruled, para(1, 20.0)), with(ruled, para(1, 20.0))].map(Block::Para);
        let layout = cell_layout(&blocks, &METRICS, 100.0, 36.0, false);
        assert_eq!(layout.content, 21.75);
    }
}
