//! The document as layout sees it: paragraphs with resolved formatting and
//! measured text, tables with their grids, and the page geometry.
//!
//! Formatting is resolved as Word does: document defaults, then the table
//! style (in tables), the paragraph style chain, the numbering level's
//! paragraph properties and direct formatting; run properties likewise
//! through the character style. Within one style chain the nearest value
//! wins; the toggle properties (bold, italic, caps) XOR across the table,
//! paragraph and character styles, and direct formatting is absolute.

use std::collections::HashMap;
use std::rc::Rc;

use super::fonts::{FontBook, FontKind};
use crate::Result;
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeData, NodeId, XmlTree};

const TWIP: f64 = 1.0 / 20.0;
const DRAWINGML_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const VML: &str = "urn:schemas-microsoft-com:vml";

/// Page geometry in points.
#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub height: f64,
    pub top: f64,
    pub bottom: f64,
    pub header: f64,
    pub footer: f64,
    /// Text width between the margins.
    pub text_width: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineRule {
    /// `line` is 240ths of the single line height.
    #[default]
    Auto,
    /// Exactly `line` twips.
    Exact,
    /// At least `line` twips.
    AtLeast,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TabKind {
    Left,
    Right,
    Center,
    Decimal,
    Clear,
}

/// Font metrics of a text piece, shared by index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metric {
    /// Single line height in points.
    pub height: f64,
    /// Ascent plus line gap in points.
    pub ascent: f64,
}

#[derive(Debug, Clone)]
pub enum Piece {
    Char { ch: char, width: f64, metric: u32 },
    Tab { metric: u32 },
    LineBreak { metric: u32 },
    PageBreak,
    Object { width: f64, height: f64 },
}

#[derive(Debug, Clone)]
pub struct Label {
    pub width: f64,
    pub metric: u32,
    /// What follows the label: a tab (true) or a space/nothing.
    pub tab: bool,
    pub space: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Para {
    pub node: Option<NodeId>,
    pub pieces: Vec<Piece>,
    pub before: f64,
    pub after: f64,
    pub before_auto: bool,
    pub after_auto: bool,
    pub line: f64,
    pub line_rule: LineRule,
    pub left: f64,
    pub right: f64,
    /// First-line indent (negative: hanging).
    pub first: f64,
    pub keep_next: bool,
    pub keep_lines: bool,
    pub page_break_before: bool,
    pub widow_control: bool,
    pub contextual: bool,
    /// Justified: Word lets the spaces of a line shrink to fit it.
    pub justified: bool,
    pub style: String,
    pub tabs: Vec<(f64, TabKind)>,
    pub label: Option<Label>,
    /// The paragraph mark's metric (empty paragraphs, the last line).
    pub mark: u32,
    pub bookmarks: Vec<String>,
    /// Ends a section (`w:sectPr` in its properties): the next section's
    /// index.
    pub section_break: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub width: f64,
    pub margins: [f64; 4],
    pub blocks: Vec<Block>,
    /// 0: none, 1: restart, 2: continue.
    pub vmerge: u8,
}

#[derive(Debug, Clone, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub height: Option<(f64, bool)>,
    /// Width of the border above the row (the table's top border, else
    /// the inside one), and below it for the last row: Word adds both to
    /// the row's height.
    pub border_top: f64,
    pub border_bottom: f64,
    pub cant_split: bool,
    pub header: bool,
    pub node: Option<NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone)]
pub enum Block {
    Para(Para),
    Table(Table),
}

/// A section's page setup and header/footer heights.
#[derive(Debug, Clone)]
pub struct Section {
    pub page: Page,
    /// Default header and footer content heights.
    pub header_height: f64,
    pub footer_height: f64,
    /// First-page ones, when the section has a distinct first page.
    pub first_header_height: Option<f64>,
    pub first_footer_height: Option<f64>,
    /// `w:pgNumType w:start`.
    pub page_start: Option<i64>,
    /// Starts on the same page (`continuous`).
    pub continuous: bool,
}

/// One document's layout input.
pub struct Model {
    pub sections: Vec<Section>,
    pub blocks: Vec<Block>,
    pub metrics: Vec<Metric>,
    pub default_tab: f64,
}

#[derive(Debug, Clone, Default)]
struct PPr {
    before: Option<f64>,
    after: Option<f64>,
    before_auto: Option<bool>,
    after_auto: Option<bool>,
    line: Option<(f64, LineRule)>,
    left: Option<f64>,
    right: Option<f64>,
    first: Option<f64>,
    keep_next: Option<bool>,
    keep_lines: Option<bool>,
    page_break_before: Option<bool>,
    widow_control: Option<bool>,
    contextual: Option<bool>,
    justified: Option<bool>,
    tabs: Vec<(f64, TabKind)>,
    num: Option<(String, Option<u8>)>,
    ilvl: Option<u8>,
}

impl PPr {
    fn merge(&mut self, other: &PPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f.clone(); } )* };
        }
        take!(
            before,
            after,
            before_auto,
            after_auto,
            line,
            left,
            right,
            first,
            keep_next,
            keep_lines,
            page_break_before,
            widow_control,
            contextual,
            justified,
            num,
            ilvl
        );
        for (pos, kind) in &other.tabs {
            self.tabs.retain(|(p, _)| (p - pos).abs() > 0.01);
            if *kind != TabKind::Clear {
                self.tabs.push((*pos, *kind));
            }
        }
    }
}

#[derive(Debug, Clone)]
enum FontRef {
    Name(String),
    Theme(bool),
}

#[derive(Debug, Clone, Default)]
struct RPr {
    ascii: Option<FontRef>,
    hansi: Option<FontRef>,
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    caps: Option<bool>,
    small_caps: Option<bool>,
    vanish: Option<bool>,
    spacing: Option<f64>,
    scale: Option<f64>,
    vert: Option<bool>,
    /// Kerning from this size (points) up; 0 is off.
    kern: Option<f64>,
}

impl RPr {
    fn merge(&mut self, other: &RPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f.clone(); } )* };
        }
        take!(
            ascii, hansi, size, bold, italic, caps, small_caps, vanish, spacing, scale, vert, kern
        );
    }

    /// Style layering: toggles XOR with `other`'s, the rest is replaced.
    fn layer(&mut self, other: &RPr) {
        let toggles = [
            (self.bold, other.bold),
            (self.italic, other.italic),
            (self.caps, other.caps),
            (self.small_caps, other.small_caps),
        ];
        self.merge(other);
        let xor = |(mine, theirs): (Option<bool>, Option<bool>)| match (mine, theirs) {
            (Some(a), Some(b)) => Some(a ^ b),
            (a, b) => b.or(a),
        };
        self.bold = xor(toggles[0]);
        self.italic = xor(toggles[1]);
        self.caps = xor(toggles[2]);
        self.small_caps = xor(toggles[3]);
    }
}

fn on_off(tree: &XmlTree, node: NodeId) -> bool {
    !matches!(
        w_attr(tree, node, "val"),
        Some("0" | "false" | "off" | "none")
    )
}

fn twips(tree: &XmlTree, node: NodeId, name: &str) -> Option<f64> {
    w_attr(tree, node, name)
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v * TWIP)
}

fn read_ppr(tree: &XmlTree, ppr: Option<NodeId>) -> PPr {
    let mut out = PPr::default();
    let Some(ppr) = ppr else {
        return out;
    };
    for child in tree.child_elements(ppr) {
        let Some(local) = tree.local_name(child) else {
            continue;
        };
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        match local {
            "spacing" => {
                out.before = twips(tree, child, "before");
                out.after = twips(tree, child, "after");
                out.before_auto = w_attr(tree, child, "beforeAutospacing")
                    .map(|v| !matches!(v, "0" | "false" | "off"));
                out.after_auto = w_attr(tree, child, "afterAutospacing")
                    .map(|v| !matches!(v, "0" | "false" | "off"));
                if let Some(line) = w_attr(tree, child, "line").and_then(|v| v.parse::<f64>().ok())
                {
                    let rule = match w_attr(tree, child, "lineRule") {
                        Some("exact") => LineRule::Exact,
                        Some("atLeast") => LineRule::AtLeast,
                        _ => LineRule::Auto,
                    };
                    out.line = Some((line, rule));
                }
            }
            "ind" => {
                out.left = twips(tree, child, "left").or_else(|| twips(tree, child, "start"));
                out.right = twips(tree, child, "right").or_else(|| twips(tree, child, "end"));
                if let Some(hanging) = twips(tree, child, "hanging") {
                    out.first = Some(-hanging);
                } else if let Some(first) = twips(tree, child, "firstLine") {
                    out.first = Some(first);
                }
            }
            "keepNext" => out.keep_next = Some(on_off(tree, child)),
            "keepLines" => out.keep_lines = Some(on_off(tree, child)),
            "pageBreakBefore" => out.page_break_before = Some(on_off(tree, child)),
            "widowControl" => out.widow_control = Some(on_off(tree, child)),
            "contextualSpacing" => out.contextual = Some(on_off(tree, child)),
            "jc" => {
                out.justified = Some(matches!(
                    w_attr(tree, child, "val"),
                    Some(
                        "both"
                            | "distribute"
                            | "lowKashida"
                            | "mediumKashida"
                            | "highKashida"
                            | "thaiDistribute"
                    )
                ))
            }
            "tabs" => {
                for tab in w_children(tree, child, "tab") {
                    let Some(pos) = twips(tree, tab, "pos") else {
                        continue;
                    };
                    let kind = match w_attr(tree, tab, "val") {
                        Some("right" | "end") => TabKind::Right,
                        Some("center") => TabKind::Center,
                        Some("decimal") => TabKind::Decimal,
                        Some("clear") => TabKind::Clear,
                        _ => TabKind::Left,
                    };
                    out.tabs.push((pos, kind));
                }
            }
            "numPr" => {
                let ilvl = w_child(tree, child, "ilvl")
                    .and_then(|n| w_attr(tree, n, "val"))
                    .and_then(|v| v.parse().ok());
                if let Some(id) = w_child(tree, child, "numId").and_then(|n| w_attr(tree, n, "val"))
                {
                    out.num = Some((id.to_string(), ilvl));
                }
                out.ilvl = ilvl;
            }
            _ => {}
        }
    }
    out
}

fn read_rpr(tree: &XmlTree, rpr: Option<NodeId>) -> RPr {
    let mut out = RPr::default();
    let Some(rpr) = rpr else {
        return out;
    };
    for child in tree.child_elements(rpr) {
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        match tree.local_name(child).unwrap_or("") {
            "rFonts" => {
                let theme = |name: &str| {
                    w_attr(tree, child, name).map(|v| FontRef::Theme(v.starts_with("major")))
                };
                let named =
                    |name: &str| w_attr(tree, child, name).map(|v| FontRef::Name(v.to_string()));
                out.ascii = theme("asciiTheme").or_else(|| named("ascii"));
                out.hansi = theme("hAnsiTheme").or_else(|| named("hAnsi"));
            }
            "sz" => {
                out.size = w_attr(tree, child, "val")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| v / 2.0)
            }
            "b" => out.bold = Some(on_off(tree, child)),
            "i" => out.italic = Some(on_off(tree, child)),
            "caps" => out.caps = Some(on_off(tree, child)),
            "smallCaps" => out.small_caps = Some(on_off(tree, child)),
            "vanish" => out.vanish = Some(on_off(tree, child)),
            "spacing" => out.spacing = twips(tree, child, "val"),
            "kern" => {
                out.kern = w_attr(tree, child, "val")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|half| half / 2.0)
            }
            "w" => {
                out.scale = w_attr(tree, child, "val")
                    .and_then(|v| v.trim_end_matches('%').parse::<f64>().ok())
            }
            "vertAlign" => {
                out.vert = Some(matches!(
                    w_attr(tree, child, "val"),
                    Some("superscript" | "subscript")
                ))
            }
            _ => {}
        }
    }
    out
}

struct StyleDef {
    based_on: Option<String>,
    ppr: PPr,
    rpr: RPr,
    /// Table styles: whole-table cell margins and borders, first-row look.
    cell_margins: [Option<f64>; 4],
    borders: [Option<f64>; 3],
    first_row: Option<(PPr, RPr)>,
    /// Position in the styles part.
    order: usize,
    /// A table style with table properties of its own (`w:tblPr` content
    /// or conditional formatting).
    table_props: bool,
}

struct Styles {
    defs: HashMap<String, StyleDef>,
    default_para: Option<String>,
    default_table: Option<String>,
    default_ppr: PPr,
    default_rpr: RPr,
    chains: HashMap<String, Rc<(PPr, RPr)>>,
}

/// Border widths in points from `w:tblBorders` or `w:tcBorders`: top,
/// bottom and inside horizontal (`none` and `nil` are 0).
fn borders(tree: &XmlTree, node: Option<NodeId>) -> [Option<f64>; 3] {
    let mut out = [None; 3];
    if let Some(node) = node {
        for (slot, name) in out.iter_mut().zip(["top", "bottom", "insideH"]) {
            *slot = w_child(tree, node, name).map(|side| match w_attr(tree, side, "val") {
                Some("none" | "nil") | None => 0.0,
                Some(_) => {
                    w_attr(tree, side, "sz")
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(2.0)
                        / 8.0
                }
            });
        }
    }
    out
}

fn margins(tree: &XmlTree, mar: Option<NodeId>) -> [Option<f64>; 4] {
    let mut out = [None; 4];
    if let Some(mar) = mar {
        for (index, names) in [
            ["top", "top"],
            ["bottom", "bottom"],
            ["left", "start"],
            ["right", "end"],
        ]
        .iter()
        .enumerate()
        {
            let side = w_child(tree, mar, names[0]).or_else(|| w_child(tree, mar, names[1]));
            out[index] = side.and_then(|s| twips(tree, s, "w"));
        }
    }
    out
}

impl Styles {
    fn read(tree: Option<&XmlTree>) -> Styles {
        let mut styles = Styles {
            defs: HashMap::new(),
            default_para: None,
            default_table: None,
            default_ppr: PPr::default(),
            default_rpr: RPr::default(),
            chains: HashMap::new(),
        };
        let Some(tree) = tree else {
            return styles;
        };
        let root = tree.root();
        if let Some(defaults) = w_child(tree, root, "docDefaults") {
            let rpr = w_child(tree, defaults, "rPrDefault").and_then(|d| w_child(tree, d, "rPr"));
            let ppr = w_child(tree, defaults, "pPrDefault").and_then(|d| w_child(tree, d, "pPr"));
            styles.default_rpr = read_rpr(tree, rpr);
            styles.default_ppr = read_ppr(tree, ppr);
        }
        for (order, style) in w_children(tree, root, "style").enumerate() {
            let Some(id) = w_attr(tree, style, "styleId") else {
                continue;
            };
            let kind = w_attr(tree, style, "type").unwrap_or("paragraph");
            let default = matches!(w_attr(tree, style, "default"), Some("1" | "true" | "on"));
            if default && kind == "paragraph" && styles.default_para.is_none() {
                styles.default_para = Some(id.to_string());
            }
            if default && kind == "table" && styles.default_table.is_none() {
                styles.default_table = Some(id.to_string());
            }
            let tbl_ppr = w_child(tree, style, "tblPr");
            let first_row = w_children(tree, style, "tblStylePr")
                .find(|s| w_attr(tree, *s, "type") == Some("firstRow"))
                .map(|s| {
                    (
                        read_ppr(tree, w_child(tree, s, "pPr")),
                        read_rpr(tree, w_child(tree, s, "rPr")),
                    )
                });
            styles.defs.insert(
                id.to_string(),
                StyleDef {
                    based_on: w_child(tree, style, "basedOn")
                        .and_then(|b| w_attr(tree, b, "val"))
                        .map(str::to_string),
                    ppr: read_ppr(tree, w_child(tree, style, "pPr")),
                    rpr: read_rpr(tree, w_child(tree, style, "rPr")),
                    cell_margins: margins(
                        tree,
                        tbl_ppr.and_then(|t| w_child(tree, t, "tblCellMar")),
                    ),
                    borders: borders(tree, tbl_ppr.and_then(|t| w_child(tree, t, "tblBorders"))),
                    first_row,
                    order,
                    table_props: tbl_ppr.is_some_and(|t| tree.child_elements(t).next().is_some())
                        || w_child(tree, style, "tblStylePr").is_some(),
                },
            );
        }
        styles
    }

    /// A style's properties with its `basedOn` chain (nearest wins).
    fn chain(&mut self, id: &str) -> Rc<(PPr, RPr)> {
        if let Some(found) = self.chains.get(id) {
            return found.clone();
        }
        let mut ids = Vec::new();
        let mut current = Some(id.to_string());
        while let Some(cur) = current {
            if ids.contains(&cur) || ids.len() > 32 {
                break;
            }
            current = self.defs.get(&cur).and_then(|d| d.based_on.clone());
            ids.push(cur);
        }
        let mut ppr = PPr::default();
        let mut rpr = RPr::default();
        for cur in ids.iter().rev() {
            if let Some(def) = self.defs.get(cur) {
                ppr.merge(&def.ppr);
                rpr.merge(&def.rpr);
            }
        }
        let out = Rc::new((ppr, rpr));
        self.chains.insert(id.to_string(), out.clone());
        out
    }

    /// A table style's cell margins and borders with its `basedOn` chain.
    fn table_look(&self, id: Option<&str>) -> ([Option<f64>; 4], [Option<f64>; 3]) {
        let mut margins = [None; 4];
        let mut borders = [None; 3];
        let mut current = id.map(str::to_string);
        let mut seen = 0;
        while let Some(cur) = current {
            let Some(def) = self.defs.get(&cur) else {
                break;
            };
            for (slot, value) in margins.iter_mut().zip(def.cell_margins) {
                if slot.is_none() {
                    *slot = value;
                }
            }
            for (slot, value) in borders.iter_mut().zip(def.borders) {
                if slot.is_none() {
                    *slot = value;
                }
            }
            // Word reads a table style with properties of its own before a
            // base style defined after it: the base adds nothing, and the
            // cell margins it leaves unset are 0.
            let later = def
                .based_on
                .as_ref()
                .and_then(|b| self.defs.get(b))
                .is_some_and(|base| base.order > def.order);
            if later && def.table_props {
                for slot in &mut margins {
                    slot.get_or_insert(0.0);
                }
                break;
            }
            current = def.based_on.clone();
            seen += 1;
            if seen > 32 {
                break;
            }
        }
        (margins, borders)
    }
}

struct Level {
    format: String,
    text: String,
    start: i64,
    ppr: PPr,
    rpr: RPr,
    suffix: String,
}

/// A `w:num`: its abstractNumId, level overrides and start overrides.
type Num = (String, HashMap<u8, Level>, HashMap<u8, i64>);

#[derive(Default)]
struct Numbering {
    nums: HashMap<String, Num>,
    abstracts: HashMap<String, HashMap<u8, Level>>,
    counters: HashMap<String, [i64; 9]>,
}

fn read_level(tree: &XmlTree, lvl: NodeId) -> Level {
    let val = |name: &str| w_child(tree, lvl, name).and_then(|n| w_attr(tree, n, "val"));
    Level {
        format: val("numFmt").unwrap_or("decimal").to_string(),
        text: val("lvlText").unwrap_or("").to_string(),
        start: val("start").and_then(|v| v.parse().ok()).unwrap_or(1),
        ppr: read_ppr(tree, w_child(tree, lvl, "pPr")),
        rpr: read_rpr(tree, w_child(tree, lvl, "rPr")),
        suffix: val("suff").unwrap_or("tab").to_string(),
    }
}

impl Numbering {
    fn read(tree: Option<&XmlTree>) -> Numbering {
        let mut out = Numbering::default();
        let Some(tree) = tree else {
            return out;
        };
        let root = tree.root();
        for abs in w_children(tree, root, "abstractNum") {
            let Some(id) = w_attr(tree, abs, "abstractNumId") else {
                continue;
            };
            let mut levels = HashMap::new();
            for lvl in w_children(tree, abs, "lvl") {
                if let Some(ilvl) = w_attr(tree, lvl, "ilvl").and_then(|v| v.parse().ok()) {
                    levels.insert(ilvl, read_level(tree, lvl));
                }
            }
            out.abstracts.insert(id.to_string(), levels);
        }
        for num in w_children(tree, root, "num") {
            let Some(id) = w_attr(tree, num, "numId") else {
                continue;
            };
            let abs = w_child(tree, num, "abstractNumId")
                .and_then(|n| w_attr(tree, n, "val"))
                .unwrap_or("")
                .to_string();
            let mut levels = HashMap::new();
            let mut starts = HashMap::new();
            for o in w_children(tree, num, "lvlOverride") {
                let Some(ilvl) = w_attr(tree, o, "ilvl").and_then(|v| v.parse().ok()) else {
                    continue;
                };
                if let Some(start) = w_child(tree, o, "startOverride")
                    .and_then(|s| w_attr(tree, s, "val"))
                    .and_then(|v| v.parse().ok())
                {
                    starts.insert(ilvl, start);
                }
                if let Some(lvl) = w_child(tree, o, "lvl") {
                    levels.insert(ilvl, read_level(tree, lvl));
                }
            }
            out.nums.insert(id.to_string(), (abs, levels, starts));
        }
        out
    }

    fn level(&self, num: &str, ilvl: u8) -> Option<&Level> {
        let (abs, overrides, _) = self.nums.get(num)?;
        overrides
            .get(&ilvl)
            .or_else(|| self.abstracts.get(abs).and_then(|levels| levels.get(&ilvl)))
    }

    /// Advance the counters and return the label text.
    fn label(&mut self, num: &str, ilvl: u8) -> Option<String> {
        let (abs, _, starts) = self.nums.get(num)?;
        let (abs, starts) = (abs.clone(), starts.clone());
        let level = self.level(num, ilvl)?;
        let text = level.text.clone();
        let format = level.format.clone();
        let mut starts_by_level = [1i64; 9];
        for (i, slot) in starts_by_level.iter_mut().enumerate() {
            *slot = starts
                .get(&(i as u8))
                .copied()
                .or_else(|| self.level(num, i as u8).map(|l| l.start))
                .unwrap_or(1);
        }
        let counters = self.counters.entry(abs).or_insert_with(|| {
            let mut c = [0; 9];
            for (i, v) in c.iter_mut().enumerate() {
                *v = starts_by_level[i] - 1;
            }
            c
        });
        let level_index = usize::from(ilvl.min(8));
        counters[level_index] += 1;
        for deeper in level_index + 1..9 {
            counters[deeper] = starts_by_level[deeper] - 1;
        }
        let values = *counters;
        if format == "bullet" {
            return Some(text);
        }
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%'
                && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
            {
                chars.next();
                let index = (d as usize).saturating_sub(1).min(8);
                out.push_str(&format_number(values[index], &format));
                continue;
            }
            out.push(c);
        }
        Some(out)
    }
}

fn format_number(value: i64, format: &str) -> String {
    match format {
        "lowerLetter" | "upperLetter" => {
            let n = value.max(1) - 1;
            let letter = (b'a' + (n % 26) as u8) as char;
            let s: String = std::iter::repeat_n(letter, (n / 26 + 1) as usize).collect();
            if format == "upperLetter" {
                s.to_uppercase()
            } else {
                s
            }
        }
        "lowerRoman" | "upperRoman" => {
            let s = roman(value);
            if format == "upperRoman" {
                s.to_uppercase()
            } else {
                s
            }
        }
        "decimalZero" => format!("{value:02}"),
        "none" => String::new(),
        _ => value.to_string(),
    }
}

fn roman(mut value: i64) -> String {
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (n, s) in table {
        while value >= n {
            out.push_str(s);
            value -= n;
        }
    }
    out
}

struct Theme {
    major: Option<String>,
    minor: Option<String>,
}

fn read_theme(tree: Option<&XmlTree>) -> Theme {
    const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    let mut theme = Theme {
        major: None,
        minor: None,
    };
    if let Some(tree) = tree {
        for node in tree.descendants(tree.root()) {
            for (local, slot) in [
                ("majorFont", &mut theme.major),
                ("minorFont", &mut theme.minor),
            ] {
                if tree.is(node, A, local) && slot.is_none() {
                    *slot = tree
                        .first_child_named(node, A, "latin")
                        .and_then(|l| tree.attr(l, None, "typeface"))
                        .filter(|t| !t.is_empty())
                        .map(str::to_string);
                }
            }
        }
    }
    theme
}

/// Font kinds named by the font table (`w:family`).
fn read_font_kinds(tree: Option<&XmlTree>) -> HashMap<String, FontKind> {
    let mut out = HashMap::new();
    if let Some(tree) = tree {
        for font in w_children(tree, tree.root(), "font") {
            let Some(name) = w_attr(tree, font, "name") else {
                continue;
            };
            let fixed =
                w_child(tree, font, "pitch").and_then(|p| w_attr(tree, p, "val")) == Some("fixed");
            let kind = match w_child(tree, font, "family").and_then(|f| w_attr(tree, f, "val")) {
                _ if fixed => FontKind::Mono,
                Some("modern") => FontKind::Mono,
                Some("roman") => FontKind::Serif,
                _ => FontKind::Sans,
            };
            out.insert(name.to_lowercase(), kind);
        }
    }
    out
}

pub(super) struct Builder<'a> {
    styles: Styles,
    numbering: Numbering,
    theme: Theme,
    kinds: HashMap<String, FontKind>,
    pub fonts: &'a mut FontBook,
    pub metrics: Vec<Metric>,
    metric_index: HashMap<(u64, u64), u32>,
    section_widths: Vec<f64>,
    sections_seen: usize,
}

fn part<'t>(txn: &'t Txn<'_>, main: &str, rel: &str) -> Option<&'t XmlTree> {
    let name = txn.part_related_by(main, rel).ok().flatten()?;
    txn.tree(&name).ok()
}

/// Build the layout model of `main`'s body.
pub(super) fn build(txn: &Txn<'_>, main: &str, fonts: &mut FontBook) -> Result<Model> {
    let tree = txn.tree(main)?;
    let mut builder = Builder {
        styles: Styles::read(part(txn, main, rt::STYLES)),
        numbering: Numbering::read(part(txn, main, rt::NUMBERING)),
        theme: read_theme(part(txn, main, rt::THEME)),
        kinds: read_font_kinds(part(txn, main, rt::FONT_TABLE)),
        fonts,
        metrics: Vec::new(),
        metric_index: HashMap::new(),
        section_widths: Vec::new(),
        sections_seen: 0,
    };
    let default_tab = part(txn, main, rt::SETTINGS)
        .and_then(|s| w_child(s, s.root(), "defaultTabStop").and_then(|d| twips(s, d, "val")))
        .unwrap_or(36.0);
    let body = w_child(tree, tree.root(), "body");
    // Section properties in body order: those ending a section inside a
    // paragraph, then the body's own (the last section).
    let mut sects: Vec<NodeId> = Vec::new();
    if let Some(body) = body {
        for node in tree.descendants(body) {
            if tree.is(node, W, "sectPr") {
                let in_ppr = tree.parent(node).is_some_and(|p| tree.is(p, W, "pPr"));
                let in_body = tree.parent(node) == Some(body);
                if in_ppr || in_body {
                    sects.push(node);
                }
            }
        }
    }
    let mut sections = Vec::new();
    let mut inherited: HashMap<(&'static str, String), f64> = HashMap::new();
    let main_uri = format!("/{}", main.trim_start_matches('/'));
    let rels = txn.relationships(main)?;
    for sect in &sects {
        let page = page_of(tree, Some(*sect));
        for reference in tree.child_elements(*sect) {
            let kind = match tree.local_name(reference) {
                Some("headerReference") => "header",
                Some("footerReference") => "footer",
                _ => continue,
            };
            let which = w_attr(tree, reference, "type")
                .unwrap_or("default")
                .to_string();
            let Some(id) = tree.attr(reference, Some(crate::authoring::R), "id") else {
                continue;
            };
            let Some(rel) = rels.iter().find(|r| r.id == id) else {
                continue;
            };
            let Some(name) =
                txn.resolve(&rel.target_part_name(crate::package::rels::base_uri(&main_uri)))
            else {
                continue;
            };
            let Ok(story) = txn.tree(&name) else {
                continue;
            };
            let mut story_blocks = Vec::new();
            builder.blocks(story, story.root(), None, false, &mut story_blocks);
            let height = super::flow::stack_height(
                &story_blocks,
                &builder.metrics,
                page.text_width,
                default_tab,
            );
            inherited.insert((kind, which), height);
        }
        let get =
            |kind: &'static str, which: &str| inherited.get(&(kind, which.to_string())).copied();
        let first_page = w_child(tree, *sect, "titlePg").is_some_and(|t| on_off(tree, t));
        sections.push(Section {
            page,
            header_height: get("header", "default").unwrap_or(0.0),
            footer_height: get("footer", "default").unwrap_or(0.0),
            first_header_height: first_page.then(|| get("header", "first").unwrap_or(0.0)),
            first_footer_height: first_page.then(|| get("footer", "first").unwrap_or(0.0)),
            page_start: w_child(tree, *sect, "pgNumType")
                .and_then(|n| w_attr(tree, n, "start"))
                .and_then(|v| v.parse().ok()),
            continuous: w_child(tree, *sect, "type").and_then(|t| w_attr(tree, t, "val"))
                == Some("continuous"),
        });
    }
    if sections.is_empty() {
        sections.push(Section {
            page: page_of(tree, None),
            header_height: 0.0,
            footer_height: 0.0,
            first_header_height: None,
            first_footer_height: None,
            page_start: None,
            continuous: false,
        });
    }
    let mut blocks = Vec::new();
    if let Some(body) = body {
        builder.section_widths = sections.iter().map(|s| s.page.text_width).collect();
        builder.blocks(tree, body, None, false, &mut blocks);
    }
    Ok(Model {
        sections,
        blocks,
        metrics: builder.metrics,
        default_tab,
    })
}

fn page_of(tree: &XmlTree, sect: Option<NodeId>) -> Page {
    let size = sect.and_then(|s| w_child(tree, s, "pgSz"));
    let mar = sect.and_then(|s| w_child(tree, s, "pgMar"));
    let get = |node: Option<NodeId>, name: &str, default: f64| {
        node.and_then(|n| twips(tree, n, name)).unwrap_or(default)
    };
    let width = get(size, "w", 612.0);
    let height = get(size, "h", 792.0);
    let left = get(mar, "left", 72.0);
    let right = get(mar, "right", 72.0);
    let gutter = get(mar, "gutter", 0.0);
    Page {
        height,
        top: get(mar, "top", 72.0),
        bottom: get(mar, "bottom", 72.0),
        header: get(mar, "header", 36.0),
        footer: get(mar, "footer", 36.0),
        text_width: (width - left - right - gutter).max(36.0),
    }
}

#[derive(Clone)]
struct TableCtx {
    style: Option<String>,
    first_row: bool,
}

impl Builder<'_> {
    fn metric(&mut self, height: f64, ascent: f64) -> u32 {
        let key = (height.to_bits(), ascent.to_bits());
        if let Some(index) = self.metric_index.get(&key) {
            return *index;
        }
        let index = self.metrics.len() as u32;
        self.metrics.push(Metric { height, ascent });
        self.metric_index.insert(key, index);
        index
    }

    fn blocks(
        &mut self,
        tree: &XmlTree,
        parent: NodeId,
        table: Option<&TableCtx>,
        _nested: bool,
        out: &mut Vec<Block>,
    ) {
        for child in tree.child_elements(parent) {
            if tree.ns_uri(child) != Some(W) {
                continue;
            }
            match tree.local_name(child).unwrap_or("") {
                "p" => out.push(Block::Para(self.paragraph(tree, child, table))),
                "tbl" => out.push(Block::Table(self.table(tree, child))),
                "sdt" => {
                    if let Some(content) = w_child(tree, child, "sdtContent") {
                        self.blocks(tree, content, table, _nested, out);
                    }
                }
                "customXml" | "ins" | "moveTo" => self.blocks(tree, child, table, _nested, out),
                _ => {}
            }
        }
    }

    fn table(&mut self, tree: &XmlTree, tbl: NodeId) -> Table {
        let tbl_pr = w_child(tree, tbl, "tblPr");
        let style = tbl_pr
            .and_then(|p| w_child(tree, p, "tblStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string)
            .or_else(|| self.styles.default_table.clone());
        let look_first_row = tbl_pr
            .and_then(|p| w_child(tree, p, "tblLook"))
            .map(|l| {
                w_attr(tree, l, "firstRow")
                    .map(|v| v == "1" || v == "true")
                    .unwrap_or_else(|| {
                        w_attr(tree, l, "val")
                            .and_then(|v| u32::from_str_radix(v, 16).ok())
                            .is_some_and(|v| v & 0x20 != 0)
                    })
            })
            .unwrap_or(true);
        let (mut table_margins, mut table_borders) = self.styles.table_look(style.as_deref());
        let direct = margins(tree, tbl_pr.and_then(|p| w_child(tree, p, "tblCellMar")));
        for (slot, value) in table_margins.iter_mut().zip(direct) {
            if value.is_some() {
                *slot = value;
            }
        }
        let direct = borders(tree, tbl_pr.and_then(|p| w_child(tree, p, "tblBorders")));
        for (slot, value) in table_borders.iter_mut().zip(direct) {
            if value.is_some() {
                *slot = value;
            }
        }
        let [table_top, table_bottom, inside] = table_borders.map(|b| b.unwrap_or(0.0));
        let row_count = w_children(tree, tbl, "tr").count();
        // Explicit bottom borders of the previous row's cells.
        let mut above_next = 0.0f64;
        let grid: Vec<f64> = w_child(tree, tbl, "tblGrid")
            .map(|g| {
                w_children(tree, g, "gridCol")
                    .map(|c| twips(tree, c, "w").unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();
        let mut rows = Vec::new();
        for (row_index, tr) in w_children(tree, tbl, "tr").enumerate() {
            let tr_pr = w_child(tree, tr, "trPr");
            let height = tr_pr
                .and_then(|p| w_child(tree, p, "trHeight"))
                .and_then(|h| {
                    twips(tree, h, "val").map(|v| (v, w_attr(tree, h, "hRule") == Some("exact")))
                });
            let flag = |name: &str| {
                tr_pr
                    .and_then(|p| w_child(tree, p, name))
                    .is_some_and(|n| on_off(tree, n))
            };
            let ctx = TableCtx {
                style: style.clone(),
                first_row: row_index == 0 && look_first_row,
            };
            let mut cells = Vec::new();
            let mut column = 0;
            let last = row_index + 1 == row_count;
            let mut border_top = above_next;
            let mut border_bottom = 0.0f64;
            above_next = 0.0;
            for tc in w_children(tree, tr, "tc") {
                let tc_pr = w_child(tree, tc, "tcPr");
                let [top, bottom, _] =
                    borders(tree, tc_pr.and_then(|p| w_child(tree, p, "tcBorders")));
                border_top =
                    border_top.max(top.unwrap_or(if row_index == 0 { table_top } else { inside }));
                if last {
                    border_bottom = border_bottom.max(bottom.unwrap_or(table_bottom));
                } else if let Some(bottom) = bottom {
                    above_next = above_next.max(bottom);
                }
                let span = tc_pr
                    .and_then(|p| w_child(tree, p, "gridSpan"))
                    .and_then(|s| w_attr(tree, s, "val"))
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1);
                let width: f64 = if grid.len() >= column + span {
                    grid[column..column + span].iter().sum()
                } else {
                    tc_pr
                        .and_then(|p| w_child(tree, p, "tcW"))
                        .and_then(|w| twips(tree, w, "w"))
                        .unwrap_or(72.0)
                };
                column += span;
                let mut cell_margins = table_margins;
                let own = margins(tree, tc_pr.and_then(|p| w_child(tree, p, "tcMar")));
                for (slot, value) in cell_margins.iter_mut().zip(own) {
                    if value.is_some() {
                        *slot = value;
                    }
                }
                let vmerge = match tc_pr.and_then(|p| w_child(tree, p, "vMerge")) {
                    None => 0,
                    Some(v) if w_attr(tree, v, "val") == Some("restart") => 1,
                    Some(_) => 2,
                };
                let mut blocks = Vec::new();
                self.blocks(tree, tc, Some(&ctx), true, &mut blocks);
                cells.push(Cell {
                    width,
                    margins: [
                        cell_margins[0].unwrap_or(0.0),
                        cell_margins[1].unwrap_or(0.0),
                        cell_margins[2].unwrap_or(5.4),
                        cell_margins[3].unwrap_or(5.4),
                    ],
                    blocks,
                    vmerge,
                });
            }
            rows.push(Row {
                cells,
                height,
                border_top,
                border_bottom,
                cant_split: flag("cantSplit"),
                header: flag("tblHeader"),
                node: Some(tr),
            });
        }
        Table { rows }
    }

    /// Effective paragraph and paragraph-mark run properties.
    fn para_props(&mut self, style: &str, direct: &PPr, table: Option<&TableCtx>) -> (PPr, RPr) {
        let mut ppr = self.styles.default_ppr.clone();
        let mut rpr = self.styles.default_rpr.clone();
        if let Some(table) = table
            && let Some(id) = &table.style
        {
            let chain = self.styles.chain(id);
            ppr.merge(&chain.0);
            rpr.layer(&chain.1);
            if table.first_row
                && let Some((p, r)) = self.styles.defs.get(id).and_then(|d| d.first_row.clone())
            {
                ppr.merge(&p);
                rpr.layer(&r);
            }
        }
        let chain = self.styles.chain(style);
        ppr.merge(&chain.0);
        rpr.layer(&chain.1);
        // Numbering's indents sit between the style and direct formatting.
        let num = direct.num.clone().or_else(|| ppr.num.clone());
        if let Some((id, ilvl)) = num {
            let ilvl = direct.ilvl.or(ilvl).or(ppr.ilvl).unwrap_or(0);
            if let Some(level) = self.numbering.level(&id, ilvl) {
                let level_ppr = level.ppr.clone();
                ppr.merge(&level_ppr);
            }
        }
        ppr.merge(direct);
        (ppr, rpr)
    }

    fn font_name(&self, font: &Option<FontRef>) -> String {
        match font {
            Some(FontRef::Name(name)) => name.clone(),
            Some(FontRef::Theme(major)) => {
                let theme = if *major {
                    &self.theme.major
                } else {
                    &self.theme.minor
                };
                theme.clone().unwrap_or_else(|| "Calibri".to_string())
            }
            None => "Times New Roman".to_string(),
        }
    }

    /// (advance in points for `ch`, metric index) under `rpr`.
    fn measurer(&mut self, rpr: &RPr) -> Measurer {
        let size = rpr.size.unwrap_or(10.0);
        let small = if rpr.vert == Some(true) {
            size * 2.0 / 3.0
        } else {
            size
        };
        let bold = rpr.bold.unwrap_or(false);
        let italic = rpr.italic.unwrap_or(false);
        let ascii = self.font_name(&rpr.ascii);
        let hansi = rpr
            .hansi
            .as_ref()
            .map_or_else(|| ascii.clone(), |f| self.font_name(&Some(f.clone())));
        // Not in the font table: Word substitutes as for a roman font.
        let kind = |name: &str| {
            self.kinds
                .get(&name.to_lowercase())
                .copied()
                .unwrap_or(FontKind::Serif)
        };
        let (ascii_kind, hansi_kind) = (kind(&ascii), kind(&hansi));
        let face = self.fonts.face(&ascii, bold, italic, ascii_kind);
        let face_hansi = if hansi.eq_ignore_ascii_case(&ascii) {
            face.clone()
        } else {
            self.fonts.face(&hansi, bold, italic, hansi_kind)
        };
        let metric = self.metric(face.line_height() * size, face.ascent() * size);
        Measurer {
            ascii: face,
            hansi: face_hansi,
            size: small,
            caps: rpr.caps.unwrap_or(false),
            small_caps: rpr.small_caps.unwrap_or(false),
            spacing: rpr.spacing.unwrap_or(0.0),
            scale: rpr.scale.unwrap_or(100.0) / 100.0,
            kern: rpr.kern.is_some_and(|from| from > 0.0 && size >= from),
            metric,
        }
    }

    fn paragraph(&mut self, tree: &XmlTree, p: NodeId, table: Option<&TableCtx>) -> Para {
        let ppr_node = w_child(tree, p, "pPr");
        let direct = read_ppr(tree, ppr_node);
        let style = ppr_node
            .and_then(|pp| w_child(tree, pp, "pStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
            .map(str::to_string)
            .or_else(|| self.styles.default_para.clone())
            .unwrap_or_default();
        let (ppr, para_rpr) = self.para_props(&style, &direct, table);
        let mut mark_rpr = para_rpr.clone();
        mark_rpr.merge(&read_rpr(
            tree,
            ppr_node.and_then(|pp| w_child(tree, pp, "rPr")),
        ));
        let mark = self.measurer(&mark_rpr).metric;
        let mut para = Para {
            node: Some(p),
            before: ppr.before.unwrap_or(0.0),
            after: ppr.after.unwrap_or(0.0),
            before_auto: ppr.before_auto.unwrap_or(false),
            after_auto: ppr.after_auto.unwrap_or(false),
            line: ppr.line.map_or(240.0, |l| l.0),
            line_rule: ppr.line.map_or(LineRule::Auto, |l| l.1),
            left: ppr.left.unwrap_or(0.0),
            right: ppr.right.unwrap_or(0.0),
            first: ppr.first.unwrap_or(0.0),
            keep_next: ppr.keep_next.unwrap_or(false),
            keep_lines: ppr.keep_lines.unwrap_or(false),
            page_break_before: ppr.page_break_before.unwrap_or(false),
            // Word for Mac applies widow control unless it is turned off.
            widow_control: ppr.widow_control.unwrap_or(true),
            contextual: ppr.contextual.unwrap_or(false),
            justified: ppr.justified.unwrap_or(false),
            style: style.clone(),
            tabs: ppr.tabs.clone(),
            mark,
            section_break: None,
            ..Para::default()
        };
        para.tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let num = direct.num.clone().or_else(|| ppr.num.clone());
        if let Some((id, ilvl)) = num.filter(|(id, _)| id != "0") {
            let ilvl = direct.ilvl.or(ilvl).or(ppr.ilvl).unwrap_or(0);
            if let Some(text) = self.numbering.label(&id, ilvl) {
                let level = self.numbering.level(&id, ilvl);
                let suffix = level.map_or("tab".to_string(), |l| l.suffix.clone());
                let level_rpr = level.map(|l| l.rpr.clone()).unwrap_or_default();
                let mut label_rpr = mark_rpr.clone();
                label_rpr.merge(&level_rpr);
                let measurer = self.measurer(&label_rpr);
                let width: f64 = text.chars().map(|c| measurer.width(c)).sum();
                let space = if suffix == "space" {
                    measurer.width(' ')
                } else {
                    0.0
                };
                para.label = Some(Label {
                    width,
                    metric: measurer.metric,
                    tab: suffix == "tab",
                    space,
                });
            }
        }
        let mut fields: Vec<bool> = Vec::new();
        self.inline(tree, p, &para_rpr, &mut para, &mut fields);
        if table.is_none()
            && ppr_node
                .and_then(|pp| w_child(tree, pp, "sectPr"))
                .is_some()
        {
            self.sections_seen += 1;
            para.section_break = Some(self.sections_seen);
        }
        para
    }

    fn inline(
        &mut self,
        tree: &XmlTree,
        parent: NodeId,
        para_rpr: &RPr,
        para: &mut Para,
        fields: &mut Vec<bool>,
    ) {
        for child in tree.child_elements(parent) {
            if tree.ns_uri(child) != Some(W) {
                continue;
            }
            match tree.local_name(child).unwrap_or("") {
                "r" => self.run(tree, child, para_rpr, para, fields),
                "hyperlink" | "smartTag" | "customXml" | "ins" | "moveTo" | "fldSimple" | "dir"
                | "bdo" => self.inline(tree, child, para_rpr, para, fields),
                "sdt" => {
                    if let Some(content) = w_child(tree, child, "sdtContent") {
                        self.inline(tree, content, para_rpr, para, fields);
                    }
                }
                "bookmarkStart" => {
                    if let Some(name) = w_attr(tree, child, "name") {
                        para.bookmarks.push(name.to_string());
                    }
                }
                _ => {}
            }
        }
        // Math: measured as its text in the paragraph font.
        for child in tree.child_elements(parent) {
            if tree.local_name(child) == Some("oMath")
                || tree.local_name(child) == Some("oMathPara")
            {
                let text = tree.text_content(child);
                let measurer = self.measurer(para_rpr);
                for ch in text.chars() {
                    para.pieces.push(Piece::Char {
                        ch,
                        width: measurer.width(ch),
                        metric: measurer.metric,
                    });
                }
            }
        }
    }

    fn run(
        &mut self,
        tree: &XmlTree,
        r: NodeId,
        para_rpr: &RPr,
        para: &mut Para,
        fields: &mut Vec<bool>,
    ) {
        let rpr_node = w_child(tree, r, "rPr");
        let direct = read_rpr(tree, rpr_node);
        let mut rpr = para_rpr.clone();
        if let Some(char_style) = rpr_node
            .and_then(|n| w_child(tree, n, "rStyle"))
            .and_then(|s| w_attr(tree, s, "val"))
        {
            let chain = self.styles.chain(char_style);
            rpr.layer(&chain.1);
        }
        rpr.merge(&direct);
        if rpr.vanish == Some(true) {
            return;
        }
        let mut measurer: Option<Measurer> = None;
        // The run's previous character, for kerning.
        let mut previous: Option<char> = None;
        for child in tree.child_elements(r) {
            if tree.ns_uri(child) != Some(W) {
                if tree.local_name(child) == Some("AlternateContent") {
                    // Choose the fallback's VML size if any; inline content
                    // rarely uses this.
                }
                continue;
            }
            let in_instruction = fields.last() == Some(&true);
            if tree.local_name(child) != Some("t") {
                previous = None;
            }
            match tree.local_name(child).unwrap_or("") {
                "fldChar" => match w_attr(tree, child, "fldCharType") {
                    Some("begin") => fields.push(true),
                    Some("separate") => {
                        if let Some(last) = fields.last_mut() {
                            *last = false;
                        }
                    }
                    Some("end") => {
                        fields.pop();
                    }
                    _ => {}
                },
                _ if in_instruction => {}
                "t" => {
                    let m = measurer.get_or_insert_with(|| self.measurer(&rpr));
                    for node in tree.children(child) {
                        if let NodeData::Text(text) = tree.data(*node) {
                            for ch in text.chars() {
                                if let Some(left) = previous {
                                    let kern = m.kern(left, ch);
                                    if let (true, Some(Piece::Char { width, .. })) =
                                        (kern != 0.0, para.pieces.last_mut())
                                    {
                                        *width += kern;
                                    }
                                }
                                previous = Some(ch);
                                para.pieces.push(Piece::Char {
                                    ch,
                                    width: m.width(ch),
                                    metric: m.metric,
                                });
                            }
                        }
                    }
                }
                "tab" | "ptab" => {
                    let m = measurer.get_or_insert_with(|| self.measurer(&rpr));
                    para.pieces.push(Piece::Tab { metric: m.metric });
                }
                "br" => match w_attr(tree, child, "type") {
                    Some("page" | "column") => para.pieces.push(Piece::PageBreak),
                    _ => {
                        let m = measurer.get_or_insert_with(|| self.measurer(&rpr));
                        para.pieces.push(Piece::LineBreak { metric: m.metric });
                    }
                },
                "cr" => {
                    let m = measurer.get_or_insert_with(|| self.measurer(&rpr));
                    para.pieces.push(Piece::LineBreak { metric: m.metric });
                }
                "noBreakHyphen" | "sym" | "footnoteReference" | "endnoteReference" => {
                    let m = measurer.get_or_insert_with(|| self.measurer(&rpr));
                    let ch = if tree.local_name(child) == Some("sym") {
                        '0'
                    } else {
                        '\u{2011}'
                    };
                    let width = if ch == '\u{2011}' {
                        m.width('-')
                    } else {
                        m.width(ch)
                    };
                    para.pieces.push(Piece::Char {
                        ch,
                        width,
                        metric: m.metric,
                    });
                }
                "drawing" => {
                    for node in tree.descendants(child) {
                        if tree.is(node, DRAWINGML_WP, "inline") {
                            if let Some(extent) =
                                tree.first_child_named(node, DRAWINGML_WP, "extent")
                            {
                                let emu = |name: &str| {
                                    tree.attr(extent, None, name)
                                        .and_then(|v| v.parse::<f64>().ok())
                                        .unwrap_or(0.0)
                                        / 12700.0
                                };
                                para.pieces.push(Piece::Object {
                                    width: emu("cx"),
                                    height: emu("cy"),
                                });
                            }
                            break;
                        }
                    }
                }
                "pict" | "object" => {
                    // VML: the horizontal rule and sized shapes.
                    for node in tree.descendants(child) {
                        if tree.ns_uri(node) == Some(VML)
                            && let Some(style) = tree.attr(node, None, "style")
                        {
                            let (width, height) = vml_size(style);
                            if height > 0.0 {
                                let hr = tree.attr(
                                    node,
                                    Some("urn:schemas-microsoft-com:office:office"),
                                    "hr",
                                ) == Some("t");
                                para.pieces.push(Piece::Object {
                                    width: if hr { 0.0 } else { width },
                                    height,
                                });
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn vml_size(style: &str) -> (f64, f64) {
    let mut width = 0.0;
    let mut height = 0.0;
    for decl in style.split(';') {
        let mut parts = decl.splitn(2, ':');
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let value = value.trim();
        let number: f64 = value
            .trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .parse()
            .unwrap_or(0.0);
        let points = if value.ends_with("in") {
            number * 72.0
        } else if value.ends_with("px") {
            number * 0.75
        } else if value.ends_with("cm") {
            number * 72.0 / 2.54
        } else if value.ends_with("mm") {
            number * 72.0 / 25.4
        } else {
            number
        };
        match key.trim() {
            "width" => width = points,
            "height" => height = points,
            _ => {}
        }
    }
    (width, height)
}

/// Width of characters under one run's properties.
pub(super) struct Measurer {
    ascii: std::sync::Arc<super::fonts::Face>,
    hansi: std::sync::Arc<super::fonts::Face>,
    size: f64,
    caps: bool,
    small_caps: bool,
    spacing: f64,
    scale: f64,
    /// Pair kerning on (`w:kern` at or below the size).
    kern: bool,
    pub metric: u32,
}

impl Measurer {
    pub(super) fn width(&self, ch: char) -> f64 {
        let face = if (ch as u32) < 0x80 {
            &self.ascii
        } else {
            &self.hansi
        };
        let (ch, size) = if self.caps || (self.small_caps && ch.is_lowercase()) {
            let upper = ch.to_uppercase().next().unwrap_or(ch);
            let size = if self.small_caps && ch.is_lowercase() && !self.caps {
                self.size * 0.8
            } else {
                self.size
            };
            (upper, size)
        } else {
            (ch, self.size)
        };
        face.advance(ch) * size * self.scale + self.spacing
    }

    /// The kerning `left` takes before `right` (points), when kerning is on
    /// and both are set in the same face.
    pub(super) fn kern(&self, left: char, right: char) -> f64 {
        let face = |ch: char| {
            if (ch as u32) < 0x80 {
                &self.ascii
            } else {
                &self.hansi
            }
        };
        if !self.kern
            || !std::sync::Arc::ptr_eq(face(left), face(right))
            || self.caps
            || self.small_caps
        {
            return 0.0;
        }
        face(left).kern(left, right) * self.size * self.scale
    }
}
