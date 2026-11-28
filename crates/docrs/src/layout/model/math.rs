//! Office Math as one line of text, after Word's linear format, so that an
//! equation is measured and drawn as the same string.

use super::look::VertAlign;
use crate::layout::fonts::be16;
use crate::wml::{M, W};
use crate::xml::{NodeData, NodeId, XmlTree};

/// Nesting followed before the rest of an equation is read as its text.
const MAX_DEPTH: usize = 64;

/// A display stacked taller than this many text lines is laid out one
/// line tall, so that it stays on its page.
const MAX_LEVELS: f64 = 16.0;

/// A MATH table's script geometry, in ems of the base size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MathScripts {
    /// A script's size, and a script's within a script, as shares of the
    /// base size.
    pub script: f64,
    pub script_script: f64,
    /// How far a superscript is raised and a subscript lowered.
    pub up: f64,
    pub down: f64,
    /// The space after a script.
    pub after: f64,
}

/// Cambria Math's MATH table, read from the copy in Word for Mac 16.115's
/// fonts (2048 units per em). Word sets every equation in Cambria Math, so
/// this is Word's script rule whatever font draws the equation.
pub(crate) const CAMBRIA_MATH: MathScripts = MathScripts {
    script: 0.73,
    script_script: 0.60,
    up: 750.0 / 2048.0,
    down: 418.0 / 2048.0,
    after: 85.0 / 2048.0,
};

impl MathScripts {
    /// A font's MATH table (OpenType `MathConstants`), when its script
    /// sizes are percentages, its shifts within an em and its space after a
    /// script within half an em.
    pub(crate) fn read(table: &[u8], units_per_em: f64) -> Option<MathScripts> {
        let constants = usize::from(be16(table, 4)?);
        let value = |at: usize| be16(table, constants + at).map(|v| f64::from(v as i16));
        let percent = |at| {
            value(at)
                .filter(|p| (1.0..=100.0).contains(p))
                .map(|p| p / 100.0)
        };
        // The value records after the four header fields: 4 bytes each.
        let record = |index: usize, most: f64| {
            value(8 + 4 * index)
                .map(|v| v / units_per_em)
                .filter(|ems| (0.0..=most).contains(ems))
        };
        Some(MathScripts {
            script: percent(0)?,
            script_script: percent(2)?,
            down: record(4, 1.0)?,
            up: record(7, 1.0)?,
            after: record(13, 0.5)?,
        })
    }

    /// A script's size `level`s deep, as a share of the equation's.
    fn size(self, level: i32) -> f64 {
        match level {
            0 => 1.0,
            1 => self.script,
            _ => self.script_script,
        }
    }

    /// How far a script is raised (negative: lowered), in ems of its base.
    fn shift(self, vert: VertAlign) -> f64 {
        match vert {
            VertAlign::Superscript => self.up,
            _ => -self.down,
        }
    }
}

/// A character of the linear form, the run whose properties it takes
/// (`None` for the characters that spell the structure), and where it
/// sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MathChar {
    pub ch: char,
    pub run: Option<NodeId>,
    pub role: Role,
    /// Its size as a share of its run's.
    pub scale: f64,
    /// How far it is raised, in ems of its run's size.
    pub rise: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Role {
    /// Drawn, and part of the linear text.
    Shown,
    /// Part of the linear text only: a script's `^` or `_`, and brackets
    /// that only group what a raised or lowered script already sets apart.
    Grouping,
    /// Drawn only: the space Word sets around an operator, in ems.
    Space(f64),
}

/// The linear form of `math` (an `m:oMath` or any part of one).
pub(crate) fn linearize(tree: &XmlTree, math: NodeId, scripts: MathScripts) -> Vec<MathChar> {
    let mut line = Line {
        tree,
        out: Vec::new(),
        run: None,
        depth: 0,
        scripts,
        level: 0,
        scale: 1.0,
        rise: 0.0,
        in_script: false,
        argument: 0,
    };
    line.children(math);
    line.out
}

/// The linear text, `^` and `_` included: what a reader extracts and a
/// warning quotes.
pub(crate) fn linear_text(chars: &[MathChar]) -> String {
    chars
        .iter()
        .filter(|c| !matches!(c.role, Role::Space(_)))
        .map(|c| c.ch)
        .collect()
}

/// How many text lines tall Word stacks `math` in a display: a fraction
/// stacks its numerator on its denominator, a matrix or equation array
/// its rows, an n-ary operator or a limit its limits at script size;
/// stacks nest by adding. Past [`MAX_LEVELS`], one line.
pub(crate) fn levels(tree: &XmlTree, math: NodeId, scripts: MathScripts) -> f64 {
    let levels = stack(tree, math, scripts, 0);
    if levels > MAX_LEVELS { 1.0 } else { levels }
}

fn stack(tree: &XmlTree, node: NodeId, scripts: MathScripts, depth: usize) -> f64 {
    if depth == MAX_DEPTH {
        return 1.0;
    }
    let child = |local: &str| tree.first_child_named(node, M, local);
    let of = |node: Option<NodeId>| node.map_or(0.0, |n| stack(tree, n, scripts, depth + 1));
    let shown = |node: Option<NodeId>| {
        node.filter(|&n| has_text(tree, n)).map_or(0.0, |n| {
            scripts.size(1) * stack(tree, n, scripts, depth + 1)
        })
    };
    let rows = |row: &str| {
        tree.child_elements(node)
            .filter(|&r| tree.is(r, M, row))
            .map(|r| stack(tree, r, scripts, depth + 1))
            .sum::<f64>()
    };
    if tree.ns_uri(node) != Some(M) {
        return most(tree, node, scripts, depth);
    }
    match tree.local_name(node).unwrap_or("") {
        "f" if !matches!(property(tree, node, "fPr", "type"), Some("lin" | "skw")) => {
            of(child("num")).max(1.0) + of(child("den")).max(1.0)
        }
        "nary" => {
            let hidden = |name| flag(tree, node, "naryPr", name);
            let sub = if hidden("subHide") {
                0.0
            } else {
                shown(child("sub"))
            };
            let sup = if hidden("supHide") {
                0.0
            } else {
                shown(child("sup"))
            };
            of(child("e")).max(1.0 + sub + sup)
        }
        "limLow" | "limUpp" => of(child("e")).max(1.0) + shown(child("lim")),
        "m" => rows("mr"),
        "eqArr" => rows("e"),
        "sSub" | "sSup" | "sSubSup" | "sPre" => of(child("e")).max(1.0),
        _ => most(tree, node, scripts, depth),
    }
}

/// The tallest of `node`'s parts, a line at least.
fn most(tree: &XmlTree, node: NodeId, scripts: MathScripts, depth: usize) -> f64 {
    tree.child_elements(node)
        .map(|c| stack(tree, c, scripts, depth + 1))
        .fold(1.0, f64::max)
}

fn has_text(tree: &XmlTree, node: NodeId) -> bool {
    tree.descendants(node)
        .any(|n| matches!(tree.data(n), NodeData::Text(t) if !t.is_empty()))
}

/// The space Word sets on each side of a binary operator (a medium math
/// space) or a relation (a thick one), in ems.
fn operator_space(ch: char) -> Option<f64> {
    match ch {
        '+' | '-' | '−' | '±' | '∓' | '×' | '·' | '÷' | '∗' | '∘' | '∙' | '⊕' | '⊗' | '∪' | '∩'
        | '∧' | '∨' => Some(4.0 / 18.0),
        '=' | '≠' | '<' | '>' | '≤' | '≥' | '≈' | '≡' | '∼' | '≃' | '≅' | '∝' | '→' | '←' | '↔'
        | '⇒' | '⇐' | '⇔' | '∈' | '∉' | '⊂' | '⊃' | '⊆' | '⊇' => Some(5.0 / 18.0),
        _ => None,
    }
}

/// After these an operator is a sign, not between two operands.
fn opens(ch: char) -> bool {
    operator_space(ch).is_some()
        || matches!(ch, '(' | '[' | '{' | '⟨' | '/' | '&' | '@' | ',' | ' ')
}

/// The `m:val` of `node`'s `properties/name`.
fn property<'t>(tree: &'t XmlTree, node: NodeId, properties: &str, name: &str) -> Option<&'t str> {
    let properties = tree.first_child_named(node, M, properties)?;
    let property = tree.first_child_named(properties, M, name)?;
    tree.attr(property, Some(M), "val")
}

/// An on/off property: on when present, unless its `m:val` says off.
fn flag(tree: &XmlTree, node: NodeId, properties: &str, name: &str) -> bool {
    tree.first_child_named(node, M, properties)
        .and_then(|p| tree.first_child_named(p, M, name))
        .is_some_and(|property| {
            !matches!(
                tree.attr(property, Some(M), "val"),
                Some("0" | "off" | "false")
            )
        })
}

struct Line<'t> {
    tree: &'t XmlTree,
    out: Vec<MathChar>,
    run: Option<NodeId>,
    depth: usize,
    scripts: MathScripts,
    /// How many scripts deep the next character is.
    level: i32,
    scale: f64,
    rise: f64,
    /// In a script or a limit, where Word sets no operator spacing.
    in_script: bool,
    /// Where the argument being written starts, which may yet take an
    /// opening bracket.
    argument: usize,
}

impl<'t> Line<'t> {
    fn children(&mut self, node: NodeId) {
        for child in self.tree.child_elements(node) {
            self.element(child);
        }
    }

    fn element(&mut self, node: NodeId) {
        if self.depth == MAX_DEPTH {
            return self.plain(node);
        }
        self.depth += 1;
        let tree = self.tree;
        match (tree.ns_uri(node), tree.local_name(node).unwrap_or("")) {
            (Some(M | W), "r") => {
                let outer = self.run.replace(node);
                self.children(node);
                self.run = outer;
            }
            (Some(M | W), "t") => self.text(node),
            (Some(W), "del" | "moveFrom") => {}
            (Some(M), "f") => self.fraction(node),
            (Some(M), "sSub") => {
                self.base(self.child(node, "e"));
                let start = self.out.len();
                self.raised(VertAlign::Subscript, self.child(node, "sub"));
                self.space_after_scripts(start);
            }
            (Some(M), "sSup") => {
                self.base(self.child(node, "e"));
                let start = self.out.len();
                self.raised(VertAlign::Superscript, self.child(node, "sup"));
                self.space_after_scripts(start);
            }
            (Some(M), "sSubSup") => {
                self.base(self.child(node, "e"));
                let start = self.out.len();
                self.raised(VertAlign::Subscript, self.child(node, "sub"));
                self.raised(VertAlign::Superscript, self.child(node, "sup"));
                self.space_after_scripts(start);
            }
            (Some(M), "sPre") => {
                let start = self.out.len();
                self.script('_', self.child(node, "sub"));
                self.script('^', self.child(node, "sup"));
                if self.out.len() > start {
                    self.insert(start, '(', Role::Shown);
                    self.push(')');
                }
                self.operand(self.child(node, "e"), Role::Shown);
            }
            (Some(M), "rad") => self.radical(node),
            (Some(M), "d") => self.delimiter(node),
            (Some(M), "nary") => self.nary(node),
            (Some(M), "acc") => {
                self.operand(self.child(node, "e"), Role::Shown);
                self.push_char(self.char_prop(node, "accPr", "chr", '\u{302}'));
            }
            (Some(M), "bar") => {
                self.operand(self.child(node, "e"), Role::Shown);
                let top = self.prop(node, "barPr", "pos") == Some("top");
                self.push(if top { '\u{305}' } else { '\u{332}' });
            }
            (Some(M), "groupChr") => {
                self.push_char(self.char_prop(node, "groupChrPr", "chr", '\u{23DF}'));
                self.operand(self.child(node, "e"), Role::Shown);
            }
            (Some(M), "func") => {
                if let Some(name) = self.child(node, "fName") {
                    self.children(name);
                }
                self.after(' ', Role::Shown, |line| {
                    line.operand(line.child(node, "e"), Role::Shown)
                });
            }
            (Some(M), "limLow") => {
                self.operand(self.child(node, "e"), Role::Shown);
                self.script('_', self.child(node, "lim"));
            }
            (Some(M), "limUpp") => {
                self.operand(self.child(node, "e"), Role::Shown);
                self.script('^', self.child(node, "lim"));
            }
            (Some(M), "m") => {
                let rows: Vec<NodeId> = self.named(node, "mr").collect();
                self.array(rows, |line, row| {
                    let cells: Vec<NodeId> = line.named(row, "e").collect();
                    line.joined(cells, Some('&'));
                });
            }
            (Some(M), "eqArr") => {
                let rows: Vec<NodeId> = self.named(node, "e").collect();
                self.array(rows, |line, row| line.children(row));
            }
            _ => self.children(node),
        }
        self.depth -= 1;
    }

    /// `num/den`, the slash after `m:type`.
    fn fraction(&mut self, f: NodeId) {
        let slash = match self.prop(f, "fPr", "type") {
            Some("noBar") => '¦',
            Some("skw") => '⊘',
            Some("lin") => '∕',
            _ => '/',
        };
        self.operand(self.child(f, "num"), Role::Shown);
        self.push(slash);
        self.operand(self.child(f, "den"), Role::Shown);
    }

    /// `√x`, `∛x`, `∜x`, else `√(n&x)`.
    fn radical(&mut self, rad: NodeId) {
        let start = self.out.len();
        if !self.flag(rad, "radPr", "degHide")
            && let Some(degree) = self.child(rad, "deg")
        {
            self.in_script(|line| line.children(degree));
        }
        let degree: String = self.out[start..].iter().map(|c| c.ch).collect();
        let root = match degree.as_str() {
            "" => '√',
            "3" => '∛',
            "4" => '∜',
            _ => {
                self.insert(start, '√', Role::Shown);
                self.insert(start + 1, '(', Role::Shown);
                self.push('&');
                if let Some(body) = self.child(rad, "e") {
                    self.children(body);
                }
                return self.push(')');
            }
        };
        self.out.truncate(start);
        self.push(root);
        self.operand(self.child(rad, "e"), Role::Shown);
    }

    /// Its arguments between `m:begChr` and `m:endChr`, apart by `m:sepChr`.
    fn delimiter(&mut self, d: NodeId) {
        let open = self.char_prop(d, "dPr", "begChr", '(');
        let close = self.char_prop(d, "dPr", "endChr", ')');
        let separator = self.char_prop(d, "dPr", "sepChr", '|');
        self.push_char(open);
        let arguments: Vec<NodeId> = self.named(d, "e").collect();
        self.joined(arguments, separator);
        self.push_char(close);
    }

    /// `∑_(i=1)^n x`. Word puts `▒` before the operand; a space reads better
    /// and every text font has one.
    fn nary(&mut self, nary: NodeId) {
        self.push_char(self.char_prop(nary, "naryPr", "chr", '∫'));
        if !self.flag(nary, "naryPr", "subHide") {
            self.script('_', self.child(nary, "sub"));
        }
        if !self.flag(nary, "naryPr", "supHide") {
            self.script('^', self.child(nary, "sup"));
        }
        self.after(' ', Role::Shown, |line| {
            if let Some(body) = line.child(nary, "e") {
                line.children(body);
            }
        });
    }

    /// `■(a&b@c&d)`: rows apart by `@`.
    fn array(&mut self, rows: Vec<NodeId>, mut row: impl FnMut(&mut Self, NodeId)) {
        self.push('■');
        self.push('(');
        for (at, each) in rows.into_iter().enumerate() {
            if at > 0 {
                self.push('@');
            }
            row(self, each);
        }
        self.push(')');
    }

    fn joined(&mut self, arguments: Vec<NodeId>, separator: Option<char>) {
        for (at, argument) in arguments.into_iter().enumerate() {
            if at > 0 {
                self.push_char(separator);
            }
            self.children(argument);
        }
    }

    /// A limit or other script the linear text spells out: `mark` and the
    /// argument, or nothing for an empty one.
    fn script(&mut self, mark: char, argument: Option<NodeId>) {
        self.in_script(|line| {
            line.after(mark, Role::Shown, |line| {
                line.operand(argument, Role::Shown)
            })
        });
    }

    /// A subscript or superscript drawn smaller and lowered or raised, its
    /// `_` or `^` and grouping brackets in the linear text only.
    fn raised(&mut self, vert: VertAlign, argument: Option<NodeId>) {
        let mark = if vert == VertAlign::Superscript {
            '^'
        } else {
            '_'
        };
        let outer = (self.level, self.scale, self.rise);
        self.rise += self.scripts.shift(vert) * self.scale;
        self.level += 1;
        self.scale = self.scripts.size(self.level);
        self.in_script(|line| {
            line.after(mark, Role::Grouping, |line| {
                line.operand(argument, Role::Grouping)
            })
        });
        (self.level, self.scale, self.rise) = outer;
    }

    /// The space a font's MATH table sets after the scripts written since
    /// `start`, if any.
    fn space_after_scripts(&mut self, start: usize) {
        let space = self.scripts.after;
        if space > 0.0 && self.out.len() > start {
            self.push_as(' ', Role::Space(space));
        }
    }

    /// A script's base: its brackets only group when it is one run.
    fn base(&mut self, base: Option<NodeId>) {
        let tree = self.tree;
        let one_run = base.is_some_and(|base| {
            let mut content = self.content(base);
            matches!((content.next(), content.next()), (Some(only), None) if tree.is(only, M, "r"))
        });
        let brackets = if one_run { Role::Grouping } else { Role::Shown };
        self.operand(base, brackets);
    }

    fn in_script(&mut self, write: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.in_script, true);
        write(self);
        self.in_script = outer;
    }

    /// `first` and what `write` adds, or nothing when it adds nothing.
    fn after(&mut self, first: char, role: Role, write: impl FnOnce(&mut Self)) {
        let start = self.out.len();
        self.push_as(first, role);
        write(self);
        if self.out.len() == start + 1 {
            self.out.pop();
        }
    }

    /// The argument, in brackets unless it reads as one unit.
    fn operand(&mut self, argument: Option<NodeId>, brackets: Role) {
        let Some(argument) = argument else {
            return;
        };
        let start = self.out.len();
        let outer = std::mem::replace(&mut self.argument, start);
        self.children(argument);
        self.argument = outer;
        let mut written = self.out[start..]
            .iter()
            .filter(|c| !matches!(c.role, Role::Space(_)));
        let unit = written.clone().nth(1).is_none()
            || written.all(|c| c.ch.is_alphanumeric())
            || self.one_group(argument);
        if !unit {
            self.insert(start, '(', brackets);
            self.push_as(')', brackets);
        }
    }

    /// The argument is one delimiter, matrix or equation array, which
    /// brackets itself.
    fn one_group(&self, argument: NodeId) -> bool {
        let tree = self.tree;
        let mut content = self.content(argument);
        match (content.next(), content.next()) {
            (Some(only), None) => {
                tree.ns_uri(only) == Some(M)
                    && matches!(tree.local_name(only), Some("d" | "m" | "eqArr"))
            }
            _ => false,
        }
    }

    /// An argument's parts, without its properties.
    fn content(&self, argument: NodeId) -> impl Iterator<Item = NodeId> + 't {
        let tree = self.tree;
        tree.child_elements(argument)
            .filter(move |&c| !tree.local_name(c).unwrap_or("").ends_with("Pr"))
    }

    /// Text past [`MAX_DEPTH`]: the runs' text in document order.
    fn plain(&mut self, node: NodeId) {
        let tree = self.tree;
        for t in tree.descendants(node) {
            if tree.local_name(t) == Some("t") && matches!(tree.ns_uri(t), Some(M | W)) {
                self.text(t);
            }
        }
    }

    fn text(&mut self, t: NodeId) {
        let ordinary = self.in_script || self.ordinary_run();
        for child in self.tree.children(t) {
            if let NodeData::Text(text) = self.tree.data(*child) {
                for ch in text.chars() {
                    match operator_space(ch).filter(|_| !ordinary && self.follows_operand()) {
                        Some(space) => {
                            self.push_as(' ', Role::Space(space));
                            self.push_run(ch);
                            self.push_as(' ', Role::Space(space));
                        }
                        None => self.push_run(ch),
                    }
                }
            }
        }
    }

    /// The run is `m:nor` text, set as written.
    fn ordinary_run(&self) -> bool {
        let tree = self.tree;
        self.run
            .and_then(|r| tree.first_child_named(r, M, "rPr"))
            .is_some_and(|pr| tree.first_child_named(pr, M, "nor").is_some())
    }

    fn follows_operand(&self) -> bool {
        self.out[self.argument..]
            .iter()
            .rev()
            .find(|c| !matches!(c.role, Role::Space(_)))
            .is_some_and(|c| !opens(c.ch))
    }

    fn push_run(&mut self, ch: char) {
        let run = self.run;
        self.out.push(self.at(ch, run, Role::Shown));
    }

    fn push(&mut self, ch: char) {
        self.push_as(ch, Role::Shown);
    }

    fn push_as(&mut self, ch: char, role: Role) {
        self.out.push(self.at(ch, None, role));
    }

    fn push_char(&mut self, ch: Option<char>) {
        if let Some(ch) = ch {
            self.push(ch);
        }
    }

    fn insert(&mut self, at: usize, ch: char, role: Role) {
        self.out.insert(at, self.at(ch, None, role));
    }

    fn at(&self, ch: char, run: Option<NodeId>, role: Role) -> MathChar {
        MathChar {
            ch,
            run,
            role,
            scale: self.scale,
            rise: self.rise,
        }
    }

    fn child(&self, node: NodeId, local: &str) -> Option<NodeId> {
        self.tree.first_child_named(node, M, local)
    }

    fn named<'a>(&'a self, node: NodeId, local: &'a str) -> impl Iterator<Item = NodeId> + 'a {
        let tree = self.tree;
        tree.child_elements(node)
            .filter(move |&c| tree.is(c, M, local))
    }

    fn prop(&self, node: NodeId, properties: &str, name: &str) -> Option<&'t str> {
        property(self.tree, node, properties, name)
    }

    fn flag(&self, node: NodeId, properties: &str, name: &str) -> bool {
        flag(self.tree, node, properties, name)
    }

    /// A character property (`ST_Char`, one character): `default` when
    /// absent, none when empty.
    fn char_prop(&self, node: NodeId, properties: &str, name: &str, default: char) -> Option<char> {
        match self.prop(node, properties, name) {
            Some(value) => value.chars().next(),
            None => Some(default),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse_str;

    /// Cambria Math's scripts without the space after them, so that the
    /// drawn text shows only operator spacing.
    const PLAIN: MathScripts = MathScripts {
        after: 0.0,
        ..CAMBRIA_MATH
    };

    fn chars(math: &str) -> Vec<MathChar> {
        chars_with(math, PLAIN)
    }

    fn chars_with(math: &str, scripts: MathScripts) -> Vec<MathChar> {
        let tree = parse_str(&format!(
            "<m:oMath xmlns:m=\"{M}\" xmlns:w=\"{W}\">{math}</m:oMath>"
        ))
        .unwrap();
        linearize(&tree, tree.root(), scripts)
    }

    fn linear(math: &str) -> String {
        linear_text(&chars(math))
    }

    /// What layout draws: no grouping, operator spacing as spaces.
    fn drawn(math: &str) -> String {
        chars(math)
            .iter()
            .filter(|c| c.role != Role::Grouping)
            .map(|c| c.ch)
            .collect()
    }

    fn levels_of(math: &str) -> f64 {
        levels_with(math, CAMBRIA_MATH)
    }

    fn levels_with(math: &str, scripts: MathScripts) -> f64 {
        let tree = parse_str(&format!(
            "<m:oMath xmlns:m=\"{M}\" xmlns:w=\"{W}\">{math}</m:oMath>"
        ))
        .unwrap();
        levels(&tree, tree.root(), scripts)
    }

    fn r(text: &str) -> String {
        format!("<m:r><m:t>{text}</m:t></m:r>")
    }

    fn arg(name: &str, content: &str) -> String {
        format!("<m:{name}>{content}</m:{name}>")
    }

    fn el(name: &str, props: &str, args: &[(&str, &str)]) -> String {
        let args: String = args.iter().map(|(n, c)| arg(n, c)).collect();
        format!("<m:{name}>{props}{args}</m:{name}>")
    }

    #[test]
    fn runs_keep_their_text_and_their_run() {
        let runs = chars(&format!("{}{}", r("a"), r("b")));
        assert_eq!(linear_text(&runs), "ab");
        assert!(
            runs.iter()
                .all(|c| c.run.is_some() && c.role == Role::Shown)
        );
        assert_ne!(runs[0].run, runs[1].run);
        let fraction = chars(&el("f", "", &[("num", &r("a")), ("den", &r("b"))]));
        assert_eq!(fraction[1].run, None, "the slash spells the structure");
    }

    #[test]
    fn scripts_are_drawn_small_and_raised_without_their_marks() {
        let math = el("sSup", "", &[("e", &r("a")), ("sup", &r("2"))])
            + &r("+")
            + &el("sSub", "", &[("e", &r("b")), ("sub", &r("i"))]);
        assert_eq!(linear(&math), "a^2+b_i");
        assert_eq!(drawn(&math), "a2 + bi");
        let at = |ch: char| {
            let c = chars(&math).into_iter().find(|c| c.ch == ch).unwrap();
            (c.scale, c.rise)
        };
        let MathScripts {
            script,
            script_script,
            up,
            down,
            ..
        } = CAMBRIA_MATH;
        assert_eq!(at('a'), (1.0, 0.0));
        assert_eq!(at('2'), (script, up));
        assert_eq!(at('i'), (script, -down));
        let nested = el(
            "sSup",
            "",
            &[
                ("e", &r("x")),
                ("sup", &el("sSup", "", &[("e", &r("2")), ("sup", &r("k"))])),
            ],
        );
        assert_eq!(linear(&nested), "x^(2^k)");
        assert_eq!(drawn(&nested), "x2k");
        let k = chars(&nested).into_iter().find(|c| c.ch == 'k').unwrap();
        assert_eq!((k.scale, k.rise), (script_script, up + script * up));
    }

    #[test]
    fn brackets_that_only_group_are_not_drawn() {
        let power = |base: &str, sup: &str| el("sSup", "", &[("e", base), ("sup", sup)]);
        let exp = power(&r("e"), &r("-x"));
        assert_eq!((linear(&exp), drawn(&exp)), ("e^(-x)".into(), "e-x".into()));
        let run = power(&r("n+1"), &r("2"));
        assert_eq!(
            (linear(&run), drawn(&run)),
            ("(n+1)^2".into(), "n + 12".into())
        );
        let fraction = el("f", "", &[("num", &r("a")), ("den", &r("b"))]);
        let built = power(&fraction, &r("2"));
        assert_eq!(
            (linear(&built), drawn(&built)),
            ("(a/b)^2".into(), "(a/b)2".into()),
            "a base of more than one run keeps its brackets"
        );
    }

    #[test]
    fn operators_are_spaced_between_operands_only() {
        let math = r("-x+y=1");
        assert_eq!(drawn(&math), "-x + y = 1");
        let spaces: Vec<f64> = chars(&math)
            .iter()
            .filter_map(|c| match c.role {
                Role::Space(ems) => Some(ems),
                _ => None,
            })
            .collect();
        assert_eq!(spaces, [4.0 / 18.0, 4.0 / 18.0, 5.0 / 18.0, 5.0 / 18.0]);
        let script = el("sSup", "", &[("e", &r("x")), ("sup", &r("a+b"))]);
        assert_eq!(drawn(&script), "xa+b", "none in a script");
        let limit = el(
            "nary",
            r#"<m:naryPr><m:chr m:val="∑"/></m:naryPr>"#,
            &[("sub", &r("i=1")), ("sup", &r("n")), ("e", &r("i"))],
        );
        assert_eq!(drawn(&limit), "∑_(i=1)^n i", "nor in a limit");
        let ordinary = r#"<m:r><m:rPr><m:nor/></m:rPr><m:t>a+b</m:t></m:r>"#;
        assert_eq!(drawn(ordinary), "a+b", "nor in ordinary text");
        let grouped = el("d", "", &[("e", &r("-1"))]) + &r("&amp;-2");
        assert_eq!(drawn(&grouped), "(-1)&-2", "after an opening bracket or &");
        let operand = el(
            "nary",
            r#"<m:naryPr><m:chr m:val="∑"/></m:naryPr>"#,
            &[("sub", ""), ("sup", ""), ("e", &r("-x"))],
        );
        assert_eq!(drawn(&operand), "∑ -x", "at the start of an operand");
        let squared = r("y") + &el("sSup", "", &[("e", &r("-x")), ("sup", &r("2"))]);
        assert_eq!(linear(&squared), "y(-x)^2");
        assert_eq!(drawn(&squared), "y-x2", "after a bracket that only groups");
    }

    #[test]
    fn scripts_take_their_geometry_and_the_space_after_them_from_a_math_table() {
        let geometry = MathScripts {
            script: 0.5,
            script_script: 0.25,
            up: 0.4,
            down: 0.1,
            after: 0.05,
        };
        let nested = el(
            "sSubSup",
            "",
            &[
                ("e", &r("x")),
                ("sub", &r("i")),
                ("sup", &el("sSup", "", &[("e", &r("2")), ("sup", &r("k"))])),
            ],
        ) + &r("+1");
        let chars = chars_with(&nested, geometry);
        assert_eq!(linear_text(&chars), linear(&nested), "the same linear text");
        let at = |ch: char| {
            let c = chars.iter().find(|c| c.ch == ch).unwrap();
            (c.scale, c.rise)
        };
        assert_eq!(at('i'), (0.5, -0.1));
        assert_eq!(at('2'), (0.5, 0.4));
        assert_eq!(at('k'), (0.25, 0.4 + 0.5 * 0.4), "script-script size");
        let spaces: Vec<f64> = chars
            .iter()
            .filter_map(|c| match c.role {
                Role::Space(ems) => Some(ems),
                _ => None,
            })
            .collect();
        assert_eq!(
            spaces,
            [0.05, 0.05, 4.0 / 18.0, 4.0 / 18.0],
            "after the inner and the outer scripts, then around +"
        );
        let sum = el(
            "nary",
            "",
            &[("sub", &r("a")), ("sup", &r("b")), ("e", &r("f"))],
        );
        assert_eq!(levels_with(&sum, geometry), 1.0 + 2.0 * 0.5);
    }

    /// A MATH table with these constants: script sizes, then the
    /// subscript drop, superscript rise and space after, in font units.
    fn math_table(values: [i16; 5]) -> Vec<u8> {
        let mut table = vec![0, 1, 0, 0, 0, 10, 0, 0, 0, 0];
        let mut constants = vec![0u8; 8 + 4 * 14];
        for (at, value) in [0, 2, 8 + 4 * 4, 8 + 4 * 7, 8 + 4 * 13]
            .into_iter()
            .zip(values)
        {
            constants[at..at + 2].copy_from_slice(&value.to_be_bytes());
        }
        table.extend(&constants);
        table
    }

    #[test]
    fn a_math_table_gives_its_script_geometry() {
        let table = math_table([73, 60, 418, 750, 85]);
        assert_eq!(MathScripts::read(&table, 2048.0), Some(CAMBRIA_MATH));
        assert_eq!(MathScripts::read(&table[..40], 2048.0), None, "cut short");
        let read = |values| MathScripts::read(&math_table(values), 2048.0);
        assert_eq!(read([0, 60, 418, 750, 85]), None, "not a percentage");
    }

    #[test]
    fn a_hostile_math_table_is_not_read() {
        let read = |values| MathScripts::read(&math_table(values), 2048.0);
        assert_eq!(read([73, 60, 418, i16::MAX, 85]), None, "a rise of 16 ems");
        assert_eq!(read([73, 60, -418, 750, 85]), None, "a negative drop");
        assert_eq!(
            read([73, 60, 418, 750, 1100]),
            None,
            "over half an em after"
        );
        assert_eq!(read([73, 101, 418, 750, 85]), None);
        assert!(
            read([73, 60, 2048, 2048, 1024]).is_some(),
            "the bounds hold"
        );
    }

    #[test]
    fn a_display_taller_than_sixteen_lines_is_one_line() {
        let rows = |n: usize| {
            let rows: Vec<(&str, &str)> =
                std::iter::repeat_n(("e", "<m:r><m:t>x</m:t></m:r>"), n).collect();
            el("eqArr", "", &rows)
        };
        assert_eq!(levels_of(&rows(16)), 16.0);
        assert_eq!(levels_of(&rows(17)), 1.0);
        assert_eq!(levels_of(&rows(200)), 1.0);
    }

    #[test]
    fn character_properties_take_their_first_character() {
        let separator = "|".repeat(10_000);
        let args: Vec<(&str, &str)> =
            std::iter::repeat_n(("e", "<m:r><m:t>a</m:t></m:r>"), 100).collect();
        let props = format!(
            r#"<m:dPr><m:begChr m:val="(("/><m:sepChr m:val="{separator}"/><m:endChr m:val="))"/></m:dPr>"#
        );
        let text = linear(&el("d", &props, &args));
        assert_eq!(text.len(), 2 + 100 + 99);
        assert_eq!(
            linear(&el(
                "acc",
                r#"<m:accPr><m:chr m:val="^^"/></m:accPr>"#,
                &[("e", &r("x"))]
            )),
            "x^"
        );
    }

    #[test]
    fn displays_stack_as_word_does() {
        let fraction = |num: &str| el("f", "", &[("num", num), ("den", &r("2"))]);
        let script = CAMBRIA_MATH.script;
        assert_eq!(levels_of(&r("a+b")), 1.0);
        assert_eq!(levels_of(&fraction(&r("n"))), 2.0);
        assert_eq!(levels_of(&fraction(&fraction(&r("n")))), 3.0, "stacks nest");
        assert_eq!(
            levels_of(&el(
                "f",
                r#"<m:fPr><m:type m:val="lin"/></m:fPr>"#,
                &[("num", &r("a")), ("den", &r("b"))]
            )),
            1.0,
            "a linear fraction does not stack"
        );
        let sum = |props: &str| {
            el(
                "nary",
                props,
                &[("sub", &r("i=1")), ("sup", &r("n")), ("e", &r("i"))],
            )
        };
        assert_eq!(levels_of(&sum("")), 1.0 + 2.0 * script);
        assert_eq!(
            levels_of(&sum(r#"<m:naryPr><m:supHide m:val="1"/></m:naryPr>"#)),
            1.0 + script
        );
        let limit = el("limLow", "", &[("e", &r("lim")), ("lim", &r("x→0"))]);
        assert_eq!(levels_of(&limit), 1.0 + script);
        let row =
            |cells: &[&str]| arg("mr", &cells.iter().map(|c| arg("e", c)).collect::<String>());
        let matrix = format!(
            "<m:m>{}{}</m:m>",
            row(&[&r("a"), &r("b")]),
            row(&[&fraction(&r("c")), &r("d")])
        );
        assert_eq!(
            levels_of(&matrix),
            3.0,
            "each row as tall as its tallest cell"
        );
        assert_eq!(
            levels_of(&el("eqArr", "", &[("e", &r("x")), ("e", &r("y"))])),
            2.0
        );
        let squared = el("sSup", "", &[("e", &fraction(&r("n"))), ("sup", &r("2"))]);
        assert_eq!(levels_of(&squared), 2.0, "scripts stack nothing");
        assert_eq!(levels_of(&(r("y=") + &fraction(&r("n")))), 2.0);
    }

    #[test]
    fn fractions() {
        let f = |props: &str, num: &str, den: &str| {
            linear(&el("f", props, &[("num", num), ("den", den)]))
        };
        assert_eq!(f("", &r("a"), &r("b")), "a/b");
        assert_eq!(f("", &r("n+1"), &r("2")), "(n+1)/2");
        assert_eq!(
            f(
                r#"<m:fPr><m:type m:val="noBar"/></m:fPr>"#,
                &r("n"),
                &r("k")
            ),
            "n¦k"
        );
    }

    #[test]
    fn subscripts_and_superscripts() {
        assert_eq!(
            linear(&el("sSub", "", &[("e", &r("x")), ("sub", &r("i"))])),
            "x_i"
        );
        assert_eq!(
            linear(&el("sSup", "", &[("e", &r("x")), ("sup", &r("2"))])),
            "x^2"
        );
        assert_eq!(
            linear(&el(
                "sSubSup",
                "",
                &[("e", &r("x")), ("sub", &r("i")), ("sup", &r("2"))]
            )),
            "x_i^2"
        );
        assert_eq!(
            linear(&el("sSup", "", &[("e", &r("e")), ("sup", &r("-x"))])),
            "e^(-x)"
        );
        assert_eq!(
            linear(&el(
                "sPre",
                "",
                &[("sub", &r("1")), ("sup", &r("2")), ("e", &r("F"))]
            )),
            "(_1^2)F"
        );
    }

    #[test]
    fn radicals() {
        let hidden = r#"<m:radPr><m:degHide m:val="1"/></m:radPr>"#;
        let rad = |props: &str, deg: &str, body: &str| {
            linear(&el("rad", props, &[("deg", deg), ("e", body)]))
        };
        assert_eq!(rad(hidden, "", &r("x")), "√x");
        assert_eq!(rad("", "", &r("x+1")), "√(x+1)");
        assert_eq!(rad("", &r("3"), &r("x")), "∛x");
        assert_eq!(rad("", &r("4"), &r("x")), "∜x");
        assert_eq!(rad("", &r("n"), &r("x")), "√(n&x)");
    }

    #[test]
    fn delimiters() {
        let d = |props: &str, args: &[&str]| {
            let args: Vec<(&str, &str)> = args.iter().map(|a| ("e", *a)).collect();
            linear(&el("d", props, &args))
        };
        assert_eq!(d("", &[&r("a+b")]), "(a+b)");
        assert_eq!(
            d(
                r#"<m:dPr><m:begChr m:val="["/><m:endChr m:val="]"/></m:dPr>"#,
                &[&r("x")]
            ),
            "[x]"
        );
        assert_eq!(
            d(
                r#"<m:dPr><m:begChr m:val="|"/><m:endChr m:val="|"/></m:dPr>"#,
                &[&r("x")]
            ),
            "|x|"
        );
        assert_eq!(
            d(
                r#"<m:dPr><m:begChr m:val="⟨"/><m:sepChr m:val=","/><m:endChr m:val="⟩"/></m:dPr>"#,
                &[&r("a"), &r("b")]
            ),
            "⟨a,b⟩"
        );
        assert_eq!(d("", &[&r("a"), &r("b")]), "(a|b)");
        assert_eq!(
            d(
                r#"<m:dPr><m:begChr m:val="{"/><m:endChr m:val=""/></m:dPr>"#,
                &[&r("x")]
            ),
            "{x"
        );
    }

    #[test]
    fn n_ary_operators() {
        let sum = r#"<m:naryPr><m:chr m:val="∑"/><m:limLoc m:val="undOvr"/></m:naryPr>"#;
        assert_eq!(
            linear(&el(
                "nary",
                sum,
                &[("sub", &r("i=1")), ("sup", &r("n")), ("e", &r("i"))]
            )),
            "∑_(i=1)^n i"
        );
        assert_eq!(
            linear(&el(
                "nary",
                "",
                &[("sub", &r("a")), ("sup", &r("b")), ("e", &r("f"))]
            )),
            "∫_a^b f"
        );
        let hidden = r#"<m:naryPr><m:chr m:val="∏"/><m:subHide m:val="1"/><m:supHide m:val="1"/></m:naryPr>"#;
        assert_eq!(
            linear(&el(
                "nary",
                hidden,
                &[("sub", ""), ("sup", ""), ("e", &r("x"))]
            )),
            "∏ x"
        );
    }

    #[test]
    fn accents_and_bars() {
        assert_eq!(linear(&el("acc", "", &[("e", &r("x"))])), "x\u{302}");
        assert_eq!(
            linear(&el(
                "acc",
                r#"<m:accPr><m:chr m:val="⃗"/></m:accPr>"#,
                &[("e", &r("v"))]
            )),
            "v\u{20D7}"
        );
        assert_eq!(
            linear(&el(
                "bar",
                r#"<m:barPr><m:pos m:val="top"/></m:barPr>"#,
                &[("e", &r("z"))]
            )),
            "z\u{305}"
        );
        assert_eq!(linear(&el("bar", "", &[("e", &r("a+b"))])), "(a+b)\u{332}");
    }

    #[test]
    fn functions() {
        let sin = r#"<m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>sin</m:t></m:r>"#;
        assert_eq!(
            linear(&el("func", "", &[("fName", sin), ("e", &r("θ"))])),
            "sin θ"
        );
        assert_eq!(
            linear(&el("func", "", &[("fName", sin), ("e", &r("2x"))])),
            "sin 2x"
        );
        assert_eq!(
            linear(&el("func", "", &[("fName", sin), ("e", &r("x+y"))])),
            "sin (x+y)"
        );
    }

    #[test]
    fn matrices_and_equation_arrays() {
        let row = |a: &str, b: &str| arg("mr", &(arg("e", &r(a)) + &arg("e", &r(b))));
        let matrix =
            el("m", "", &[]).replace("</m:m>", &(row("a", "b") + &row("c", "d") + "</m:m>"));
        let bracketed = el(
            "d",
            r#"<m:dPr><m:begChr m:val="["/><m:endChr m:val="]"/></m:dPr>"#,
            &[("e", &matrix)],
        );
        assert_eq!(linear(&bracketed), "[■(a&b@c&d)]");
        assert_eq!(
            linear(&el("eqArr", "", &[("e", &r("x=1")), ("e", &r("y=2"))])),
            "■(x=1@y=2)"
        );
    }

    #[test]
    fn boxes_group_characters_and_limits() {
        assert_eq!(linear(&el("box", "<m:boxPr/>", &[("e", &r("a=b"))])), "a=b");
        assert_eq!(
            linear(&el("groupChr", "", &[("e", &r("a+b"))])),
            "\u{23DF}(a+b)"
        );
        let lim = r#"<m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>lim</m:t></m:r>"#;
        assert_eq!(
            linear(&el("limLow", "", &[("e", lim), ("lim", &r("x→0"))])),
            "lim_(x→0)"
        );
        assert_eq!(
            linear(&el("limUpp", "", &[("e", &r("x")), ("lim", &r("def"))])),
            "x^def"
        );
    }

    #[test]
    fn unknown_elements_give_their_text_in_order_and_deletions_none() {
        let math = format!(
            "{}<m:phant><m:e>{}</m:e></m:phant><w:ins>{}</w:ins><w:del><m:r><w:delText>q</w:delText></m:r></w:del>",
            r("a"),
            r("b"),
            r("c")
        );
        assert_eq!(linear(&math), "abc");
    }

    #[test]
    fn a_fraction_inside_a_radical_inside_a_subscript() {
        let hidden = r#"<m:radPr><m:degHide m:val="1"/></m:radPr>"#;
        let fraction = el("f", "", &[("num", &r("a")), ("den", &r("b"))]);
        let root = el("rad", hidden, &[("deg", ""), ("e", &fraction)]);
        assert_eq!(
            linear(&el("sSub", "", &[("e", &r("x")), ("sub", &root)])),
            "x_(√(a/b))"
        );
    }

    #[test]
    fn deep_nesting_reads_as_text() {
        let mut math = r("x");
        for _ in 0..100 {
            math = el("sSup", "", &[("e", &math), ("sup", &r("2"))]);
        }
        let text = linear(&math);
        assert!(text.starts_with('('), "{text}");
        assert_eq!(text.matches('2').count(), 100);
    }
}
