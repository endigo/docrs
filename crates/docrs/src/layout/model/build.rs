use super::look::{Appearance, Caps, Link, ParaBorders, Strike, VertAlign};
use super::props::{FontRef, PPr, RPr, length, number, read_ppr, read_rpr};
use super::table::TableCtx;
use super::{Block, Builder, Label, LineRule, LineSpacing, Metric, Para, Piece};
use crate::fields::fieldcode;
use crate::layout::fonts::FontKind;
use crate::units::{EMU_PER_PT, EMU_PER_PX, PT_PER_INCH, TWIPS_PER_PT};
use crate::wml::{R, W, w_attr, w_child};
use crate::xml::{NodeData, NodeId, XmlTree};

const DRAWINGML_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const VML: &str = "urn:schemas-microsoft-com:vml";

/// Tables nested deeper are laid out as their paragraphs.
const MAX_TABLE_DEPTH: usize = 32;

/// Word shows nothing of a field, result included, while any enclosing
/// field is still in its code.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FieldPart {
    Code,
    Result,
}

struct Field {
    part: FieldPart,
    code: String,
    /// A HYPERLINK field's target, for its result.
    link: Option<u32>,
}

/// A run's drawn size and baseline shift under `w:vertAlign`. Word for Mac
/// 16.115 at Arial 11 pt raises 0.41 em and lowers 0.087 em, not Arial's
/// OS/2 offsets (0.477 and 0.138 em).
fn script(size: f64, vert: Option<VertAlign>) -> (f64, f64) {
    match vert.unwrap_or_default() {
        VertAlign::Baseline => (size, 0.0),
        VertAlign::Superscript => (size * 2.0 / 3.0, size * 0.41),
        VertAlign::Subscript => (size * 2.0 / 3.0, -size * 0.087),
    }
}

/// A run's measurer and its look for the link it is in.
struct Pen {
    measurer: Measurer,
    link: Option<u32>,
    look: u32,
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

    fn look(&mut self, rpr: &RPr, measurer: &Measurer, link: Option<u32>) -> u32 {
        let (_, shift) = script(rpr.size.unwrap_or(10.0), rpr.vert);
        let on = |toggle: Option<bool>| toggle.unwrap_or(false);
        let look = Appearance {
            metric: measurer.metric,
            size: measurer.size,
            rise: rpr.position.unwrap_or(0.0) + shift,
            colour: rpr.colour.unwrap_or_default(),
            bold: on(rpr.bold),
            italic: on(rpr.italic),
            underline: rpr.underline.flatten(),
            strike: if on(rpr.dstrike) {
                Strike::Double
            } else if on(rpr.strike) {
                Strike::Single
            } else {
                Strike::None
            },
            highlight: rpr.highlight.flatten(),
            shade: rpr.shd.flatten(),
            caps: if measurer.caps {
                Caps::All
            } else if measurer.small_caps {
                Caps::Small
            } else {
                Caps::None
            },
            spacing: measurer.spacing,
            scale: measurer.scale,
            link,
        };
        let next = self.looks.len() as u32;
        *self.looks.entry(look).or_insert(next)
    }

    fn link(&mut self, link: Link) -> u32 {
        if let Some(index) = self.link_index.get(&link) {
            return *index;
        }
        let index = self.links.len() as u32;
        self.links.push(link.clone());
        self.link_index.insert(link, index);
        index
    }

    /// `w:hyperlink`'s target: its relationship's, with its bookmark as the
    /// fragment, else the bookmark.
    fn hyperlink_target(&self, tree: &XmlTree, hyperlink: NodeId) -> Option<Link> {
        let uri = tree
            .attr(hyperlink, Some(R), "id")
            .and_then(|id| self.hyperlinks.get(id));
        let anchor = w_attr(tree, hyperlink, "anchor");
        match (uri, anchor) {
            (Some(uri), Some(anchor)) => Some(Link::Uri(format!("{uri}#{anchor}"))),
            (Some(uri), None) => Some(Link::Uri(uri.clone())),
            (None, anchor) => anchor.map(|a| Link::Anchor(a.to_string())),
        }
    }

    /// The pen for `rpr`, made on first use and given the look for `link`.
    fn pen<'p>(&mut self, pen: &'p mut Option<Pen>, rpr: &RPr, link: Option<u32>) -> &'p Pen {
        let stale = pen.as_ref().is_none_or(|p| p.link != link);
        if stale {
            let measurer = match pen.take() {
                Some(old) => old.measurer,
                None => self.measurer(rpr),
            };
            let look = self.look(rpr, &measurer, link);
            *pen = Some(Pen {
                measurer,
                link,
                look,
            });
        }
        pen.as_ref().expect("made above")
    }

    pub(super) fn blocks(
        &mut self,
        tree: &XmlTree,
        parent: NodeId,
        table: Option<&TableCtx>,
        out: &mut Vec<Block>,
    ) {
        for child in tree.child_elements(parent) {
            if tree.ns_uri(child) != Some(W) {
                continue;
            }
            match tree.local_name(child).unwrap_or("") {
                "p" => out.push(Block::Para(self.paragraph(tree, child, table))),
                "tbl" if self.depth < MAX_TABLE_DEPTH => {
                    self.depth += 1;
                    let table = self.table(tree, child);
                    self.depth -= 1;
                    out.push(Block::Table(table));
                }
                "tbl" => self.flattened(tree, child, table, out),
                "sdt" => {
                    if let Some(content) = w_child(tree, child, "sdtContent") {
                        self.blocks(tree, content, table, out);
                    }
                }
                "customXml" | "ins" | "moveTo" => self.blocks(tree, child, table, out),
                _ => {}
            }
        }
    }

    /// A table nested too deep: its paragraphs, and those of the tables in
    /// it, as paragraphs of the enclosing cell.
    fn flattened(
        &mut self,
        tree: &XmlTree,
        tbl: NodeId,
        table: Option<&TableCtx>,
        out: &mut Vec<Block>,
    ) {
        let mut stack = vec![tbl];
        while let Some(node) = stack.pop() {
            if tree.ns_uri(node) != Some(W) {
                continue;
            }
            match tree.local_name(node).unwrap_or("") {
                "p" => out.push(Block::Para(self.paragraph(tree, node, table))),
                "tbl" | "tr" | "tc" | "sdt" | "sdtContent" | "customXml" | "ins" | "moveTo" => {
                    let children: Vec<NodeId> = tree.child_elements(node).collect();
                    stack.extend(children.into_iter().rev());
                }
                _ => {}
            }
        }
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
                && let Some((p, r)) = self.styles.defs.get(id).and_then(|d| d.first_row.as_ref())
            {
                ppr.merge(p);
                rpr.layer(r);
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
                ppr.merge(&level.ppr);
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

    fn measurer(&mut self, rpr: &RPr) -> Measurer {
        let size = rpr.size.unwrap_or(10.0);
        let (small, _) = script(size, rpr.vert);
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
        let direct = read_ppr(tree, ppr_node, &self.theme.palette);
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
            &self.theme.palette,
        ));
        let mark = self.measurer(&mark_rpr).metric;
        let mut para = Para {
            story: self.story,
            node: Some(p),
            before: ppr.before.unwrap_or(0.0),
            after: ppr.after.unwrap_or(0.0),
            before_auto: ppr.before_auto.unwrap_or(false),
            after_auto: ppr.after_auto.unwrap_or(false),
            line: match ppr.line {
                None => LineSpacing::Multiple(1.0),
                Some((line, LineRule::Auto)) => LineSpacing::Multiple(line / 240.0),
                Some((line, LineRule::Exact)) => LineSpacing::Exact(line / TWIPS_PER_PT as f64),
                Some((line, LineRule::AtLeast)) => LineSpacing::AtLeast(line / TWIPS_PER_PT as f64),
            },
            left: ppr.left.unwrap_or(0.0),
            right: ppr.right.unwrap_or(0.0),
            first: ppr.first.unwrap_or(0.0),
            keep_next: ppr.keep_next.unwrap_or(false),
            keep_lines: ppr.keep_lines.unwrap_or(false),
            page_break_before: ppr.page_break_before.unwrap_or(false),
            // Word for Mac applies widow control unless it is turned off.
            widow_control: ppr.widow_control.unwrap_or(true),
            contextual: ppr.contextual.unwrap_or(false),
            align: ppr.align.unwrap_or_default(),
            style: style.clone(),
            tabs: ppr.tabs.clone(),
            mark,
            section_break: None,
            shade: ppr.shd.flatten(),
            borders: ParaBorders::from_sides(ppr.borders.map(Option::flatten)),
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
                    look: self.look(&label_rpr, &measurer, None),
                    tab: suffix == "tab",
                    space,
                });
            }
        }
        let mut fields = Vec::new();
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
        fields: &mut Vec<Field>,
    ) {
        for child in tree.child_elements(parent) {
            if tree.ns_uri(child) != Some(W) {
                continue;
            }
            match tree.local_name(child).unwrap_or("") {
                "r" => self.run(tree, child, para_rpr, para, fields),
                name @ ("hyperlink" | "fldSimple") => {
                    let target = if name == "hyperlink" {
                        self.hyperlink_target(tree, child)
                    } else {
                        w_attr(tree, child, "instr").and_then(field_link)
                    };
                    let outer = self.link;
                    self.link = target.map(|t| self.link(t)).or(outer);
                    self.inline(tree, child, para_rpr, para, fields);
                    self.link = outer;
                }
                "smartTag" | "customXml" | "ins" | "moveTo" | "dir" | "bdo" => {
                    self.inline(tree, child, para_rpr, para, fields)
                }
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
                let look = self.look(para_rpr, &measurer, self.link);
                for ch in text.chars() {
                    para.pieces.push(Piece::Char {
                        ch,
                        width: measurer.width(ch),
                        metric: measurer.metric,
                        look,
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
        fields: &mut Vec<Field>,
    ) {
        let rpr_node = w_child(tree, r, "rPr");
        let direct = read_rpr(tree, rpr_node, &self.theme.palette);
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
        let mut pen: Option<Pen> = None;
        let mut previous: Option<char> = None;
        for child in tree.child_elements(r) {
            if tree.ns_uri(child) != Some(W) {
                if tree.local_name(child) == Some("AlternateContent") {
                    // Choose the fallback's VML size if any; inline content
                    // rarely uses this.
                }
                continue;
            }
            let in_instruction = fields.iter().any(|f| f.part == FieldPart::Code);
            let link = fields.iter().rev().find_map(|f| f.link).or(self.link);
            if tree.local_name(child) != Some("t") {
                previous = None;
            }
            match tree.local_name(child).unwrap_or("") {
                "fldChar" => match w_attr(tree, child, "fldCharType") {
                    Some("begin") => fields.push(Field {
                        part: FieldPart::Code,
                        code: String::new(),
                        link: None,
                    }),
                    Some("separate") => {
                        if let Some(last) = fields.last_mut() {
                            last.part = FieldPart::Result;
                            let target = field_link(&last.code);
                            last.link = target.map(|t| self.link(t));
                        }
                    }
                    Some("end") => {
                        fields.pop();
                    }
                    _ => {}
                },
                "instrText" => {
                    if let Some(field) = fields.last_mut().filter(|f| f.part == FieldPart::Code) {
                        field.code.push_str(&tree.text_content(child));
                    }
                }
                _ if in_instruction => {}
                "t" => {
                    let pen = self.pen(&mut pen, &rpr, link);
                    let m = &pen.measurer;
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
                                    look: pen.look,
                                });
                            }
                        }
                    }
                }
                "tab" | "ptab" => {
                    let pen = self.pen(&mut pen, &rpr, link);
                    para.pieces.push(Piece::Tab {
                        metric: pen.measurer.metric,
                        look: pen.look,
                    });
                }
                "br" => match w_attr(tree, child, "type") {
                    Some("page" | "column") => para.pieces.push(Piece::PageBreak),
                    _ => {
                        let pen = self.pen(&mut pen, &rpr, link);
                        para.pieces.push(Piece::LineBreak {
                            metric: pen.measurer.metric,
                            look: pen.look,
                        });
                    }
                },
                "cr" => {
                    let pen = self.pen(&mut pen, &rpr, link);
                    para.pieces.push(Piece::LineBreak {
                        metric: pen.measurer.metric,
                        look: pen.look,
                    });
                }
                "noBreakHyphen" | "sym" | "footnoteReference" | "endnoteReference" => {
                    let pen = self.pen(&mut pen, &rpr, link);
                    let m = &pen.measurer;
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
                        look: pen.look,
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
                                        .and_then(number)
                                        .map_or(0.0, |emu| length(emu / EMU_PER_PT as f64))
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

/// A HYPERLINK field's target: its URI (with `\l`'s bookmark as the
/// fragment), else the bookmark.
fn field_link(code: &str) -> Option<Link> {
    let tokens = fieldcode::tokens(code).ok()?;
    let (command, rest) = tokens.split_first()?;
    if command.quoted || !command.value.eq_ignore_ascii_case("HYPERLINK") {
        return None;
    }
    let (mut uri, mut anchor) = (None, None);
    let mut rest = rest.iter();
    while let Some(token) = rest.next() {
        if token.quoted || !token.value.starts_with('\\') {
            uri = uri.or(Some(token.value.clone()));
            continue;
        }
        match token.value.to_ascii_lowercase().as_str() {
            "\\l" => anchor = rest.next().map(|t| t.value.clone()),
            "\\o" | "\\t" => {
                rest.next();
            }
            _ => {}
        }
    }
    match (uri, anchor) {
        (Some(uri), Some(anchor)) => Some(Link::Uri(format!("{uri}#{anchor}"))),
        (Some(uri), None) => Some(Link::Uri(uri)),
        (None, anchor) => anchor.map(Link::Anchor),
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
        let number =
            number(value.trim_end_matches(|c: char| c.is_ascii_alphabetic())).unwrap_or(0.0);
        let points = if value.ends_with("in") {
            number * PT_PER_INCH as f64
        } else if value.ends_with("px") {
            number * (EMU_PER_PX as f64 / EMU_PER_PT as f64)
        } else if value.ends_with("cm") {
            number * PT_PER_INCH as f64 / 2.54
        } else if value.ends_with("mm") {
            number * PT_PER_INCH as f64 / 25.4
        } else {
            number
        };
        let points = length(points);
        match key.trim() {
            "width" => width = points,
            "height" => height = points,
            _ => {}
        }
    }
    (width, height)
}

pub(super) struct Measurer {
    ascii: std::sync::Arc<crate::layout::fonts::Face>,
    hansi: std::sync::Arc<crate::layout::fonts::Face>,
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
