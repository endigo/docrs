use super::props::{FontRef, PPr, RPr, read_ppr, read_rpr};
use super::table::TableCtx;
use super::{Block, Builder, Label, LineRule, LineSpacing, Metric, Para, Piece};
use crate::layout::fonts::FontKind;
use crate::units::{EMU_PER_PT, EMU_PER_PX, PT_PER_INCH, TWIPS_PER_PT};
use crate::wml::{W, w_attr, w_child};
use crate::xml::{NodeData, NodeId, XmlTree};

const DRAWINGML_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const VML: &str = "urn:schemas-microsoft-com:vml";

/// Word shows nothing of a field, result included, while any enclosing
/// field is still in its code.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FieldPart {
    Code,
    Result,
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
                "tbl" => out.push(Block::Table(self.table(tree, child))),
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
        let mut fields: Vec<FieldPart> = Vec::new();
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
        fields: &mut Vec<FieldPart>,
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
        fields: &mut Vec<FieldPart>,
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
            let in_instruction = fields.contains(&FieldPart::Code);
            if tree.local_name(child) != Some("t") {
                previous = None;
            }
            match tree.local_name(child).unwrap_or("") {
                "fldChar" => match w_attr(tree, child, "fldCharType") {
                    Some("begin") => fields.push(FieldPart::Code),
                    Some("separate") => {
                        if let Some(last) = fields.last_mut() {
                            *last = FieldPart::Result;
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
                                        / EMU_PER_PT as f64
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
