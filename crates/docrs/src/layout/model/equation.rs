//! Equations in the layout model: each one's linear form in its runs'
//! looks, with subscripts and superscripts set small and lowered or
//! raised, and a display equation as tall as Word stacks it.

use std::collections::HashMap;

use super::build::{DEFAULT_SIZE, Measurer};
use super::math::{CAMBRIA_MATH, MathScripts, Role, levels, linear_text, linearize};
use super::props::{FontRef, RPr};
use super::{Align, Builder, Equation, Para, Piece};
use crate::layout::fonts::FontKind;
use crate::wml::{M, W};
use crate::xml::{NodeId, XmlTree};

/// Word's math font, drawn only when the document or the caller supplies it.
const MATH_FONT: &str = "Cambria Math";

/// A display equation's pieces in its paragraph, the line break after them
/// included, and its alignment.
pub(super) struct Display {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) align: Align,
}

/// What a math pen sets: its run (`None`: the structure), whether it is a
/// letter, and its scale and rise.
type PenKey = (Option<NodeId>, bool, u64, u64);

struct MathPen {
    measurer: Measurer,
    look: u32,
    /// The measurer's metric, raised or stacked.
    metric: u32,
}

/// The paragraph's only content is this `m:oMath`, which Word then sets as
/// a display equation.
pub(super) fn lone_equation(tree: &XmlTree, p: NodeId) -> Option<NodeId> {
    let mut content = tree.child_elements(p).filter(|&c| {
        !(tree.ns_uri(c) == Some(W)
            && matches!(
                tree.local_name(c),
                Some("pPr" | "bookmarkStart" | "bookmarkEnd" | "proofErr")
            ))
    });
    match (content.next(), content.next()) {
        (Some(only), None) if tree.is(only, M, "oMath") => Some(only),
        _ => None,
    }
}

impl Builder<'_> {
    /// An equation as its linear form, `levels` text lines tall.
    pub(super) fn equation(
        &mut self,
        tree: &XmlTree,
        math: NodeId,
        levels: f64,
        para_rpr: &RPr,
        para: &mut Para,
    ) {
        let chars = linearize(tree, math, self.scripts());
        let start = para.pieces.len();
        let mut pens: HashMap<PenKey, MathPen> = HashMap::new();
        for c in &chars {
            let (ch, run) = match c.role {
                Role::Grouping => continue,
                Role::Space(_) => (' ', None),
                Role::Shown => (c.ch, c.run),
            };
            let key = (run, ch.is_alphabetic(), c.scale.to_bits(), c.rise.to_bits());
            let pen = pens
                .entry(key)
                .or_insert_with(|| self.math_pen(tree, key, levels, para_rpr));
            let width = match c.role {
                Role::Space(ems) => ems * pen.measurer.size,
                _ => pen.measurer.width(ch),
            };
            para.pieces.push(Piece::Char {
                ch,
                width,
                metric: pen.metric,
                look: pen.look,
            });
        }
        para.equations.push(Equation {
            pieces: start..para.pieces.len(),
            text: linear_text(&chars),
        });
    }

    /// `m:oMathPara`, aligned as its `m:jc` says (Word's default centres it).
    pub(super) fn math_para(
        &mut self,
        tree: &XmlTree,
        math_para: NodeId,
        para_rpr: &RPr,
        para: &mut Para,
    ) {
        let jc = tree
            .first_child_named(math_para, M, "oMathParaPr")
            .and_then(|pr| tree.first_child_named(pr, M, "jc"))
            .and_then(|jc| tree.attr(jc, Some(M), "val"));
        let align = match jc {
            Some("left") => Align::Left,
            Some("right") => Align::Right,
            _ => Align::Center,
        };
        let equations = tree
            .child_elements(math_para)
            .filter(|&c| tree.is(c, M, "oMath"))
            .collect();
        self.display_equation(tree, equations, align, para_rpr, para);
    }

    /// Display equations, each on a line of its own.
    pub(super) fn display_equation(
        &mut self,
        tree: &XmlTree,
        equations: Vec<NodeId>,
        align: Align,
        para_rpr: &RPr,
        para: &mut Para,
    ) {
        let start = para.pieces.len();
        for math in equations {
            if !matches!(para.pieces.last(), None | Some(Piece::LineBreak { .. })) {
                self.math_break(para_rpr, para);
            }
            let levels = levels(tree, math, self.scripts());
            self.equation(tree, math, levels, para_rpr, para);
        }
        self.math_break(para_rpr, para);
        self.display = Some(Display {
            start,
            end: para.pieces.len(),
            align,
        });
    }

    /// Cambria Math's script geometry: from the supplied font's MATH table
    /// when there is one, else as Word's copy has it.
    fn scripts(&mut self) -> MathScripts {
        if !self.fonts.supplies(MATH_FONT) {
            return CAMBRIA_MATH;
        }
        self.fonts
            .face(MATH_FONT, false, false, FontKind::Serif)
            .math_scripts()
            .unwrap_or(CAMBRIA_MATH)
    }

    fn math_break(&mut self, para_rpr: &RPr, para: &mut Para) {
        let measurer = self.measurer(para_rpr);
        let look = self.look(para_rpr, &measurer, self.link);
        para.pieces.push(Piece::LineBreak {
            metric: measurer.metric,
            look,
        });
    }

    /// A script's pen is smaller and raised or lowered, and its line
    /// extent moves with it; a base pen of a display stands `levels` lines
    /// tall, the extra shared above and below.
    fn math_pen(&mut self, tree: &XmlTree, key: PenKey, levels: f64, para_rpr: &RPr) -> MathPen {
        let (run, letter, scale, rise) = key;
        let (scale, rise) = (f64::from_bits(scale), f64::from_bits(rise));
        let mut rpr = self.math_rpr(tree, run, letter, para_rpr);
        let size = rpr.size.unwrap_or(DEFAULT_SIZE);
        let rise = rise * size;
        rpr.size = Some(size * scale);
        rpr.position = Some(rpr.position.unwrap_or(0.0) + rise);
        let measurer = self.measurer(&rpr);
        let look = self.look(&rpr, &measurer, self.link);
        let m = self.metrics[measurer.metric as usize];
        let (ascent, below) = if scale == 1.0 && rise == 0.0 {
            let extra = (levels - 1.0).max(0.0) * m.height / 2.0;
            (m.ascent + extra, m.height - m.ascent + extra)
        } else {
            (
                (m.ascent + rise).max(0.0),
                (m.height - m.ascent - rise).max(0.0),
            )
        };
        let metric = self.metric(ascent + below, ascent);
        MathPen {
            measurer,
            look,
            metric,
        }
    }

    /// A math run's properties over the paragraph's, with its `m:sty`
    /// (letters slanted when it has none, as Word does) unless `m:nor`
    /// makes it ordinary text.
    fn math_rpr(
        &mut self,
        tree: &XmlTree,
        run: Option<NodeId>,
        letter: bool,
        para_rpr: &RPr,
    ) -> RPr {
        let mut rpr = match run {
            Some(run) => self.fmt.run(tree, run, para_rpr),
            None => para_rpr.clone(),
        };
        rpr.ascii = self.math_font(&rpr.ascii, &para_rpr.ascii);
        rpr.hansi = self.math_font(&rpr.hansi, &para_rpr.hansi);
        let math_rpr = run.and_then(|run| tree.first_child_named(run, M, "rPr"));
        if math_rpr.is_some_and(|pr| tree.first_child_named(pr, M, "nor").is_some()) {
            return rpr;
        }
        let sty = math_rpr
            .and_then(|pr| tree.first_child_named(pr, M, "sty"))
            .and_then(|sty| tree.attr(sty, Some(M), "val"));
        let (bold, italic) = match sty {
            Some("p") => (false, false),
            Some("b") => (true, false),
            Some("i") => (false, true),
            Some("bi") => (true, true),
            _ => (false, letter && run.is_some()),
        };
        rpr.bold = Some(bold);
        rpr.italic = Some(italic);
        rpr
    }

    /// The font a math run's slot is laid out in: the one it names when
    /// that is supplied, else Cambria Math when supplied, else the
    /// paragraph's, so that an equation never needs a font its paragraph
    /// does not.
    fn math_font(
        &mut self,
        named: &Option<FontRef>,
        paragraph: &Option<FontRef>,
    ) -> Option<FontRef> {
        if named != paragraph && self.fonts.supplies(&self.font_name(named)) {
            return named.clone();
        }
        if self.fonts.supplies(MATH_FONT) {
            return Some(FontRef::Name(MATH_FONT.to_string()));
        }
        paragraph.clone()
    }
}
