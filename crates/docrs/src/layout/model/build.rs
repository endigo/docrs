use super::formatting::ParaFormat;
use std::sync::Arc;

use super::look::{Appearance, Caps, Link, ParaBorders, Strike, VertAlign, is_ascii_slot};
use super::props::{FontRef, RPr, length, number, read_rpr};
use super::table::TableCtx;
use super::{Block, Builder, FieldPen, Label, LineRule, LineSpacing, Metric, Para, Piece, RunFace};
use crate::fields::fieldcode;
use crate::layout::fields::{FieldKind, FieldSpan, LayoutWarning, instruction};
use crate::layout::fonts::{Face, FontKind};
use crate::layout::variation::Style;
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
    live: Option<LiveField>,
}

/// A field layout computes, open: where its result starts in its
/// paragraph's pieces and fields.
struct LiveField {
    kind: FieldKind,
    piece: u32,
    span: usize,
    pen: FieldPen,
}

impl LiveField {
    fn new(kind: FieldKind, para: &Para, pen: FieldPen) -> LiveField {
        LiveField {
            kind,
            piece: para.pieces.len() as u32,
            span: para.fields.len(),
            pen,
        }
    }

    /// Close it at the paragraph's last piece, in place of the fields
    /// nested in its result. One that ends inside an earlier field's result
    /// (fields closed out of order) keeps its stored result.
    fn close(self, para: &mut Para) {
        para.fields.truncate(self.span);
        if para
            .fields
            .last()
            .is_some_and(|span| span.pieces.end > self.piece)
        {
            return;
        }
        para.fields.push(FieldSpan {
            kind: self.kind,
            pieces: self.piece..para.pieces.len() as u32,
            pen: self.pen,
        });
    }
}

/// A run's size and drawn baseline shift under `w:vertAlign` in `face`.
fn script(size: f64, vert: Option<VertAlign>, face: &Face) -> (f64, f64) {
    // Word draws the face's OS/2 script size, to the nearest half point (halves up).
    let small = |superscript| (face.script_size(superscript) * size * 2.0).round() / 2.0;
    match vert.unwrap_or_default() {
        VertAlign::Baseline => (size, 0.0),
        // Word for Mac's shifts at 11 pt, approximate at other sizes (it rises
        // 0.35 to 0.415 em); its rule is still open.
        VertAlign::Superscript => (small(true), size * 0.41),
        VertAlign::Subscript => (small(false), -size * 0.087),
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
        let (_, shift) = script(rpr.size.unwrap_or(10.0), rpr.vert, &measurer.ascii);
        let on = |toggle: Option<bool>| toggle.unwrap_or(false);
        let look = Appearance {
            metric: measurer.metric,
            ascii: measurer.faces[0],
            hansi: measurer.faces[1],
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
            caps: measurer.caps,
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

    /// Reads a field's instruction where its result shows: its warning,
    /// and its kind when layout computes it.
    fn field_kind(&mut self, code: &str) -> Option<FieldKind> {
        let read = instruction(code, &self.properties);
        for code in read.warnings {
            let warning = LayoutWarning {
                code,
                story: self.story,
                page: None,
            };
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning);
            }
        }
        read.kind
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

    fn field_pen(&mut self, rpr: &RPr, link: Option<u32>) -> FieldPen {
        let measurer = self.measurer(rpr);
        let look = self.look(rpr, &measurer, link);
        FieldPen { measurer, look }
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

    fn font_name(&self, font: &Option<FontRef>) -> String {
        match font {
            Some(FontRef::Name(name)) => name.clone(),
            Some(FontRef::Theme(major)) => {
                let theme = if *major {
                    &self.fmt.theme.major
                } else {
                    &self.fmt.theme.minor
                };
                theme.clone().unwrap_or_else(|| "Calibri".to_string())
            }
            None => "Times New Roman".to_string(),
        }
    }

    fn measurer(&mut self, rpr: &RPr) -> Measurer {
        let size = rpr.size.unwrap_or(10.0);
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
        let style = Style::of(bold, italic);
        let face = self.fonts.face(&ascii, bold, italic, ascii_kind);
        let face_hansi = if hansi.eq_ignore_ascii_case(&ascii) {
            face.clone()
        } else {
            self.fonts.face(&hansi, bold, italic, hansi_kind)
        };
        let metric = self.metric(face.line_height() * size, face.ascent() * size);
        let (small, _) = script(size, rpr.vert, &face);
        let faces = [
            self.run_face(ascii, style, &face),
            self.run_face(hansi, style, &face_hansi),
        ];
        Measurer {
            ascii: face,
            hansi: face_hansi,
            faces,
            size: small,
            caps: Caps::of(rpr.caps.unwrap_or(false), rpr.small_caps.unwrap_or(false)),
            bold,
            spacing: rpr.spacing.unwrap_or(0.0),
            scale: rpr.scale.unwrap_or(100.0) / 100.0,
            kern: rpr.kern.is_some_and(|from| from > 0.0 && size >= from),
            metric,
        }
    }

    fn run_face(&mut self, family: String, style: Style, face: &Arc<Face>) -> u32 {
        let key = (family, style);
        if let Some(index) = self.face_index.get(&key) {
            return *index;
        }
        let index = self.faces.len() as u32;
        self.faces.push(RunFace {
            family: key.0.clone(),
            bold: style.weight >= 600,
            italic: style.italic,
            face: face.clone(),
        });
        self.face_index.insert(key, index);
        index
    }

    fn paragraph(&mut self, tree: &XmlTree, p: NodeId, table: Option<&TableCtx>) -> Para {
        let ppr_node = w_child(tree, p, "pPr");
        let ParaFormat {
            style,
            direct,
            ppr,
            rpr: para_rpr,
        } = self.fmt.paragraph(tree, p, table);
        let mut mark_rpr = para_rpr.clone();
        mark_rpr.merge(&read_rpr(
            tree,
            ppr_node.and_then(|pp| w_child(tree, pp, "rPr")),
            &self.fmt.theme.palette,
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
            if let Some(text) = self.fmt.numbering.label(&id, ilvl) {
                let level = self.fmt.numbering.level(&id, ilvl);
                let suffix = level.map_or("tab".to_string(), |l| l.suffix.clone());
                let level_rpr = level.map(|l| l.rpr.clone()).unwrap_or_default();
                let mut label_rpr = mark_rpr.clone();
                label_rpr.merge(&level_rpr);
                let measurer = self.measurer(&label_rpr);
                let look = self.look(&label_rpr, &measurer, None);
                let pieces: Vec<Piece> = text
                    .chars()
                    .map(|ch| Piece::Char {
                        ch,
                        width: measurer.width(ch),
                        metric: measurer.metric,
                        look,
                    })
                    .collect();
                let width = pieces
                    .iter()
                    .map(|piece| match piece {
                        Piece::Char { width, .. } => *width,
                        _ => 0.0,
                    })
                    .sum();
                let space = if suffix == "space" {
                    measurer.width(' ')
                } else {
                    0.0
                };
                para.label = Some(Label {
                    pieces,
                    width,
                    metric: measurer.metric,
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
                    let shown = !fields.iter().any(|f| f.part == FieldPart::Code);
                    let live = match w_attr(tree, child, "instr") {
                        Some(code) if name == "fldSimple" && shown => self.field_kind(code),
                        _ => None,
                    };
                    let outer = self.link;
                    self.link = target.map(|t| self.link(t)).or(outer);
                    let live = live.map(|kind| {
                        let rpr = match w_child(tree, child, "r") {
                            Some(r) => self.fmt.run(tree, r, para_rpr),
                            None => para_rpr.clone(),
                        };
                        LiveField::new(kind, para, self.field_pen(&rpr, self.link))
                    });
                    self.inline(tree, child, para_rpr, para, fields);
                    if let Some(live) = live {
                        live.close(para);
                    }
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
        let rpr = self.fmt.run(tree, r, para_rpr);
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
                        live: None,
                    }),
                    Some("separate") => {
                        let Some((last, outer)) = fields.split_last_mut() else {
                            continue;
                        };
                        last.part = FieldPart::Result;
                        let target = field_link(&last.code);
                        last.link = target.map(|t| self.link(t));
                        if outer.iter().any(|f| f.part == FieldPart::Code) {
                            continue;
                        }
                        let code = std::mem::take(&mut last.code);
                        if let Some(kind) = self.field_kind(&code) {
                            let pen = self.field_pen(&rpr, link);
                            last.live = Some(LiveField::new(kind, para, pen));
                        }
                    }
                    Some("end") => {
                        if let Some(live) = fields.pop().and_then(|f| f.live) {
                            live.close(para);
                        }
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
                "pgNum" => {
                    let pen = self.field_pen(&rpr, link);
                    LiveField::new(FieldKind::Page(Default::default()), para, pen).close(para);
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
pub(crate) fn field_link(code: &str) -> Option<Link> {
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

/// The advance Word for Mac adds to each glyph of a bold run set in a face
/// that is not bold, in ems (measured, 11 pt Liberation Serif).
const SYNTHETIC_BOLD_ADVANCE: f64 = 0.018;

#[derive(Debug, Clone)]
pub(crate) struct Measurer {
    ascii: Arc<Face>,
    hansi: Arc<Face>,
    /// Into `Model.faces`, for `ascii` and `hansi`.
    faces: [u32; 2],
    size: f64,
    caps: Caps,
    /// The run is bold: a face that is not is drawn emboldened.
    bold: bool,
    spacing: f64,
    scale: f64,
    /// Pair kerning on (`w:kern` at or below the size).
    kern: bool,
    pub metric: u32,
}

impl Measurer {
    fn face(&self, ch: char) -> &Arc<Face> {
        if is_ascii_slot(ch) {
            &self.ascii
        } else {
            &self.hansi
        }
    }

    pub(crate) fn width(&self, ch: char) -> f64 {
        let face = self.face(ch);
        let (ch, share) = self.caps.apply(ch);
        let size = self.size * share;
        let emboldened = if self.bold && !face.bold {
            size * SYNTHETIC_BOLD_ADVANCE
        } else {
            0.0
        };
        face.advance(ch) * size * self.scale + self.spacing + emboldened
    }

    /// The kerning `left` takes before `right` (points), when kerning is on
    /// and both are set in the same face.
    pub(super) fn kern(&self, left: char, right: char) -> f64 {
        let face = |ch: char| self.face(ch);
        if !self.kern || !Arc::ptr_eq(face(left), face(right)) || self.caps != Caps::None {
            return 0.0;
        }
        face(left).kern(left, right) * self.size * self.scale
    }
}
