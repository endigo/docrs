//! The scan behind [`preflight`]: every construct of the body, the headers
//! and footers the sections show, and the footnotes and endnotes, with the
//! formatting their styles give them, sorted into drawn, not drawn yet, and
//! unsupported in v1.

use std::collections::{HashMap, HashSet};

use super::nav::{LINK_SCHEMES, bookmark_key, is_document_start, scheme};
use super::{FOOTNOTE_LAYOUT_UNSUPPORTED, NOT_YET_DRAWN, PdfLocation, PdfReport, Tally};
use crate::layout::model::{
    CAMBRIA_MATH, Formatting, Frame, Link, NoteKind, NotePosition, NoteSettings, ParaFormat, RPr,
    StoryRef, TableCtx, any_border, field_link, horizontal, linear_text, linearize, on_off,
    section_properties, shaded, story_references,
};
use crate::package::rels::{Relationship, base_uri, rt, source_part_name};
use crate::txn::Txn;
use crate::wml::{A, M, PIC, R, W, WP, first_choice_children, paragraph_text, w_attr, w_child};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, Result};

const VML: &str = "urn:schemas-microsoft-com:vml";
const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
const OFFICE: &str = "urn:schemas-microsoft-com:office:office";
const WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPC: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas";
const PICTURE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const CHART_URIS: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "http://schemas.microsoft.com/office/drawing/2014/chartex",
];
const DIAGRAM_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
/// Word names its watermark shapes with these prefixes.
const WATERMARK_PREFIXES: &[&str] = &["PowerPlusWaterMarkObject", "WordPictureWatermark"];

const CHART: &str = "pdf_chart_skipped";
const SMARTART: &str = "pdf_smartart_skipped";
const OBJECT: &str = "pdf_object_skipped";
const SHAPE: &str = "pdf_shape_skipped";
const TEXT_BOX: &str = "pdf_text_box_skipped";
const WATERMARK: &str = "pdf_watermark_skipped";
const COLUMNS: &str = "pdf_columns_ignored";
const WRAP: &str = "pdf_wrap_ignored";
const EQUATION: &str = "pdf_equation_linearized";
const IMAGE: &str = "pdf_image_unsupported";
const TEXT: &str = "pdf_text_unsupported";
const LAYOUT: &str = "pdf_layout_feature_ignored";
const REVISIONS: &str = "pdf_revisions_flattened";
const FIELD: &str = "pdf_field_cached";
const LINK: &str = "pdf_link_scheme_skipped";
const LINK_TARGET: &str = "pdf_link_target_missing";
const JUSTIFY: &str = "pdf_justification_unsupported";
const APPROXIMATED: &str = "pdf_decoration_approximated";

const RTL: &str = "right-to-left text";

/// Subjects the painter draws; each painter PR adds its own.
const DRAWN: &[&str] = &["text", "tab", "line break", "page break", "list"];

/// What a PDF of `document` would leave out or change. Reads only.
pub fn preflight(document: &Document) -> Result<PdfReport> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name();
    let tree = txn.tree(main)?;
    let body = w_child(tree, tree.root(), "body");
    let formatting = Formatting::read(
        related(&txn, main, rt::STYLES)?,
        related(&txn, main, rt::NUMBERING)?,
        related(&txn, main, rt::THEME)?,
    );
    let mut scan = Scan::new(&txn, formatting);
    scan.story(main, "body", tree, body.unwrap_or(tree.root()))?;
    let settings = related(&txn, main, rt::SETTINGS)?;
    let setting = |name: &str| {
        settings.is_some_and(|s| w_child(s, s.root(), name).is_some_and(|n| on_off(s, n)))
    };
    if let Some(body) = body {
        for (part, kind) in shown_stories(&txn, main, tree, body, setting("evenAndOddHeaders"))? {
            let story = txn.tree(&part)?;
            scan.story(&part, kind, story, story.root())?;
        }
    }
    for (rel, kind) in [(rt::FOOTNOTES, "footnotes"), (rt::ENDNOTES, "endnotes")] {
        if let Some(part) = txn.part_related_by(main, rel)? {
            scan.notes(&part, kind, txn.tree(&part)?)?;
        }
    }
    if setting("autoHyphenation") {
        scan.note_at(LAYOUT, "automatic hyphenation", false, None);
    }
    if let Some(body) = body {
        scan.note_placement(tree, body, settings);
        scan.nested_notes(&txn, main, tree, body)?;
    }
    Ok(scan.finish())
}

/// [`preflight`], with the work it took: children listed and relationships
/// read (`test-support` feature).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn preflight_work_for_tests(document: &Document) -> Result<(PdfReport, u64)> {
    let before = crate::xml::tree::work();
    let report = preflight(document)?;
    Ok((report, crate::xml::tree::work() - before))
}

fn related<'t>(txn: &'t Txn<'_>, main: &str, rel: &str) -> Result<Option<&'t XmlTree>> {
    match txn.part_related_by(main, rel)? {
        Some(part) => Ok(Some(txn.tree(&part)?)),
        None => Ok(None),
    }
}

/// The headers and footers some page shows, with their kind: each
/// section's own, else the previous section's of the same type; first-page
/// ones only with `w:titlePg`, even-page ones only with
/// `w:evenAndOddHeaders`.
fn shown_stories(
    txn: &Txn<'_>,
    main: &str,
    tree: &XmlTree,
    body: NodeId,
    even_pages: bool,
) -> Result<Vec<(String, &'static str)>> {
    let rels = txn.resolved_relationships(main)?;
    let mut current: HashMap<(&str, String), &str> = HashMap::new();
    let mut shown: Vec<(String, &'static str)> = Vec::new();
    for sect in section_properties(tree, body) {
        for StoryRef { kind, which, part } in story_references(tree, sect, &rels) {
            current.insert((kind, which), part);
        }
        let first_page = w_child(tree, sect, "titlePg").is_some_and(|t| on_off(tree, t));
        let pages = [
            ("default", true),
            ("first", first_page),
            ("even", even_pages),
        ];
        for kind in ["header", "footer"] {
            for (which, _) in pages.iter().filter(|(_, on)| *on) {
                if let Some(part) = current.get(&(kind, which.to_string()))
                    && !shown.iter().any(|(s, _)| s == part)
                {
                    shown.push((part.to_string(), kind));
                }
            }
        }
    }
    Ok(shown)
}

/// A complex field: its instruction so far, and whether the instruction
/// is still open (before `separate`).
struct Field {
    instruction: String,
    open: bool,
}

/// A table being walked: its style context and the rows seen.
struct TableScan {
    ctx: TableCtx,
    look_first_row: bool,
    rows: usize,
}

struct Scan<'s, 'd> {
    txn: &'s Txn<'d>,
    formatting: Formatting,
    part: String,
    kind: &'static str,
    /// The current part's relationships by id.
    rels: HashMap<String, Relationship>,
    ordinal: usize,
    /// Subjects noted once per paragraph, for the current one.
    in_paragraph: Vec<(&'static str, &'static str)>,
    para_rpr: RPr,
    tables: Vec<TableScan>,
    fields: Vec<Field>,
    /// By [`bookmark_key`].
    bookmarks: HashSet<String>,
    /// Links to bookmarks, checked once every story is read.
    anchors: Vec<(String, PdfLocation)>,
    tally: Tally,
}

impl<'s, 'd> Scan<'s, 'd> {
    fn new(txn: &'s Txn<'d>, formatting: Formatting) -> Self {
        Scan {
            txn,
            formatting,
            part: String::new(),
            kind: "body",
            rels: HashMap::new(),
            ordinal: 0,
            in_paragraph: Vec::new(),
            para_rpr: RPr::default(),
            tables: Vec::new(),
            fields: Vec::new(),
            bookmarks: HashSet::new(),
            anchors: Vec::new(),
            tally: Tally::default(),
        }
    }

    fn story(
        &mut self,
        part: &str,
        kind: &'static str,
        tree: &XmlTree,
        root: NodeId,
    ) -> Result<()> {
        self.enter(part, kind)?;
        self.walk(tree, root)?;
        self.close_fields();
        Ok(())
    }

    /// Footnotes and endnotes, without the separators Word draws itself.
    fn notes(&mut self, part: &str, kind: &'static str, tree: &XmlTree) -> Result<()> {
        self.enter(part, kind)?;
        for note in tree.child_elements(tree.root()) {
            if w_attr(tree, note, "type").is_none_or(|t| t == "normal") {
                self.walk(tree, note)?;
            }
        }
        self.close_fields();
        Ok(())
    }

    /// Notes the layout puts only at the foot of the page or after the last
    /// block, in one column.
    /// Notes referenced only from another note: layout lays out the notes
    /// the body refers to.
    fn nested_notes(
        &mut self,
        txn: &Txn<'_>,
        main: &str,
        tree: &XmlTree,
        body: NodeId,
    ) -> Result<()> {
        let kinds = ["footnoteReference", "endnoteReference"];
        let references = |tree: &XmlTree, root: NodeId| -> Vec<(&'static str, String)> {
            tree.descendants(root)
                .filter_map(|n| {
                    let kind = kinds.into_iter().find(|k| tree.is(n, W, k))?;
                    Some((kind, w_attr(tree, n, "id")?.to_string()))
                })
                .collect()
        };
        let cited = references(tree, body);
        for rel in [rt::FOOTNOTES, rt::ENDNOTES] {
            let Some(notes) = related(txn, main, rel)? else {
                continue;
            };
            let nested = references(notes, notes.root())
                .into_iter()
                .filter(|reference| !cited.contains(reference))
                .count();
            for _ in 0..nested {
                self.note_at(
                    FOOTNOTE_LAYOUT_UNSUPPORTED,
                    "notes referenced only from a note",
                    false,
                    None,
                );
            }
        }
        Ok(())
    }

    fn note_placement(&mut self, tree: &XmlTree, body: NodeId, settings: Option<&XmlTree>) {
        let referenced = |local: &str| tree.descendants(body).any(|n| tree.is(n, W, local));
        let document = NoteSettings::document(settings);
        let sects = section_properties(tree, body);
        let mut sections: Vec<NoteSettings> =
            sects.iter().map(|s| document.section(tree, *s)).collect();
        if sections.is_empty() {
            sections.push(document);
        }
        let placed = |kind: NoteKind, position: NotePosition| {
            sections.iter().any(|s| s.get(kind).position == position)
        };
        let mut misplaced = Vec::new();
        if referenced("footnoteReference") {
            if placed(NoteKind::Footnote, NotePosition::BeneathText) {
                misplaced.push("footnotes beneath the text");
            }
            if sects.iter().any(|s| footnote_columns(tree, *s) > 1) {
                misplaced.push("footnote columns");
            }
        }
        if referenced("endnoteReference")
            && sects.len() > 1
            && placed(NoteKind::Endnote, NotePosition::SectionEnd)
        {
            misplaced.push("endnotes at section ends");
        }
        for subject in misplaced {
            self.note_at(FOOTNOTE_LAYOUT_UNSUPPORTED, subject, false, None);
        }
    }

    fn enter(&mut self, part: &str, kind: &'static str) -> Result<()> {
        self.part = part.to_string();
        self.kind = kind;
        self.rels = self
            .txn
            .relationships(part)?
            .into_iter()
            .map(|rel| (rel.id.clone(), rel))
            .collect();
        self.ordinal = 0;
        self.in_paragraph.clear();
        self.para_rpr = RPr::default();
        self.tables.clear();
        self.fields.clear();
        Ok(())
    }

    fn walk(&mut self, tree: &XmlTree, node: NodeId) -> Result<()> {
        for child in first_choice_children(tree, node) {
            if self.visit(tree, child)? {
                self.walk(tree, child)?;
                if tree.is(child, W, "tbl") {
                    self.tables.pop();
                }
            }
        }
        Ok(())
    }

    /// Note what `node` is; true when its children still need a look.
    fn visit(&mut self, tree: &XmlTree, node: NodeId) -> Result<bool> {
        let Some(local) = tree.local_name(node) else {
            return Ok(false);
        };
        Ok(match tree.ns_uri(node) {
            Some(W) => self.wordml(tree, node, local),
            Some(M) if local == "oMath" => {
                if !deleted(tree, node) {
                    self.equation(&linear_text(&linearize(tree, node, CAMBRIA_MATH)));
                }
                false
            }
            Some(WP) if matches!(local, "inline" | "anchor") => self.drawing(tree, node),
            Some(A) if local == "graphicData" => self.graphic(tree, node),
            Some(PIC) if local == "pic" => {
                let blip = tree
                    .descendants(node)
                    .find(|&n| tree.is(n, A, "blip"))
                    .and_then(|blip| tree.attr(blip, Some(R), "embed"));
                self.image(blip);
                false
            }
            Some(WPS) if local == "wsp" => {
                match tree.descendants(node).find(|&n| tree.is(n, WPS, "txbx")) {
                    Some(text_box) => self.text_box(tree, text_box),
                    None => self.note(SHAPE, "shape"),
                }
                false
            }
            Some(WPG) if local == "wgp" => {
                self.note(SHAPE, "shape group");
                false
            }
            Some(WPC) if local == "wpc" => {
                self.note(SHAPE, "drawing canvas");
                false
            }
            Some(VML) => {
                if local != "shapetype" {
                    self.vml(tree, node);
                }
                false
            }
            _ => true,
        })
    }

    fn wordml(&mut self, tree: &XmlTree, node: NodeId, local: &str) -> bool {
        let parent = tree.parent(node).and_then(|p| tree.local_name(p));
        let val = w_attr(tree, node, "val");
        let on = on_off(tree, node);
        match local {
            "p" => self.paragraph(tree, node),
            "r" => self.run(tree, node),
            "tbl" => self.table(tree, node),
            "tr" => {
                if let Some(table) = self.tables.last_mut() {
                    table.rows += 1;
                    table.ctx.first_row = table.rows == 1 && table.look_first_row;
                }
            }
            "t" => {
                self.text(&text_of(tree, node));
                return false;
            }
            "sym" => self.note_once(NOT_YET_DRAWN, "symbol"),
            "noBreakHyphen" => self.note_once(NOT_YET_DRAWN, "no-break hyphen"),
            "softHyphen" => self.note_once(NOT_YET_DRAWN, "soft hyphen"),
            "sectPr" if parent == Some("pPr") => self.note(NOT_YET_DRAWN, "section break"),
            "tab" if parent == Some("r") => self.note_once(NOT_YET_DRAWN, "tab"),
            "ptab" => self.note_once(NOT_YET_DRAWN, "positional tab"),
            "br" if matches!(w_attr(tree, node, "type"), Some("page" | "column")) => {
                self.note(NOT_YET_DRAWN, "page break")
            }
            "br" | "cr" => self.note_once(NOT_YET_DRAWN, "line break"),
            "tblpPr" => self.note(LAYOUT, "floating tables"),
            "tblBorders" | "tcBorders" if any_border(tree, node) => {
                self.note(NOT_YET_DRAWN, "border")
            }
            "shd" if matches!(parent, Some("tblPr" | "tblPrEx" | "tcPr")) && shaded(tree, node) => {
                self.note(NOT_YET_DRAWN, "shading")
            }
            "bidiVisual" if on => self.note(TEXT, RTL),
            "bidi" if parent == Some("sectPr") && on => self.note(TEXT, RTL),
            "bdo" | "dir" if val == Some("rtl") => self.note_once(TEXT, RTL),
            "textDirection" if parent == Some("tcPr") && !horizontal(val) => {
                self.note(LAYOUT, "rotated cell text")
            }
            "textDirection" if parent == Some("sectPr") && !horizontal(val) => {
                self.note(LAYOUT, "rotated text")
            }
            "hyperlink" => self.hyperlink(tree, node),
            "bookmarkStart" => {
                if let Some(name) = w_attr(tree, node, "name") {
                    self.bookmarks.insert(bookmark_key(name));
                }
            }
            "footnoteReference" | "footnoteRef" => self.note(NOT_YET_DRAWN, "footnote"),
            "endnoteReference" | "endnoteRef" => self.note(NOT_YET_DRAWN, "endnote"),
            "fldSimple" => self.field(w_attr(tree, node, "instr").unwrap_or("")),
            "fldChar" => self.field_char(w_attr(tree, node, "fldCharType")),
            "instrText" => {
                if let Some(field) = self.fields.last_mut().filter(|f| f.open) {
                    field.instruction.push_str(&text_of(tree, node));
                }
                return false;
            }
            "pgNum" => self.note(NOT_YET_DRAWN, "field"),
            "dayShort" | "dayLong" | "monthShort" | "monthLong" | "yearShort" | "yearLong" => {
                self.note(FIELD, "DATE")
            }
            "ins" | "del" | "moveFrom" | "moveTo" | "cellIns" | "cellDel" | "cellMerge" => {
                self.note(REVISIONS, "tracked change")
            }
            "rPrChange" | "pPrChange" | "sectPrChange" | "tblPrChange" | "tblGridChange"
            | "trPrChange" | "tcPrChange" | "tblPrExChange" | "numberingChange" => {
                self.note(REVISIONS, "tracked change");
                return false;
            }
            "commentReference" => self.note(REVISIONS, "comment"),
            "object" | "control" | "altChunk" => {
                let subject = match local {
                    "object" => "OLE object",
                    "control" => "ActiveX control",
                    _ => "imported document",
                };
                self.note(OBJECT, subject);
                return false;
            }
            "contentPart" => {
                self.note(SHAPE, "ink");
                return false;
            }
            "ruby" => self.note_once(TEXT, "ruby text"),
            "cols" if columns(tree, node) > 1 => self.note(COLUMNS, "multi-column section"),
            "lnNumType" => self.note(LAYOUT, "line numbers"),
            "pgBorders" => self.note(LAYOUT, "page borders"),
            "vAlign" if parent == Some("sectPr") && val == Some("both") => {
                self.note(LAYOUT, "vertical page justification")
            }
            "delText" | "delInstrText" => return false,
            _ => {}
        }
        true
    }

    /// A paragraph's effective properties: docDefaults, the table style,
    /// the style chain, the numbering level and direct formatting.
    fn paragraph(&mut self, tree: &XmlTree, p: NodeId) {
        self.ordinal += 1;
        self.in_paragraph.clear();
        let table = self.tables.last().map(|t| &t.ctx);
        let ParaFormat { ppr, rpr, .. } = self.formatting.paragraph(tree, p, table);
        if ppr.page_break_before == Some(true) {
            self.note(NOT_YET_DRAWN, "page break");
        }
        if ppr.num.as_ref().is_some_and(|(id, _)| id != "0") {
            self.note(NOT_YET_DRAWN, "list");
        }
        if ppr.barred() {
            self.note(NOT_YET_DRAWN, "border");
        }
        if ppr.border_approximated() {
            self.note(APPROXIMATED, "paragraph border");
        }
        match ppr.frame {
            Some(Frame::DropCap) => self.note(LAYOUT, "drop caps"),
            Some(Frame::Frame) => self.note(LAYOUT, "frames"),
            None => {}
        }
        if ppr.bidi == Some(true) {
            self.note_once(TEXT, RTL);
        }
        if ppr.rotated == Some(true) {
            self.note(LAYOUT, "rotated text");
        }
        if ppr.mirror == Some(true) {
            self.note(LAYOUT, "mirrored indents");
        }
        if ppr.justified_otherwise() {
            self.note(JUSTIFY, "distributed justification");
        }
        self.para_rpr = rpr;
    }

    fn run(&mut self, tree: &XmlTree, r: NodeId) {
        let rpr = self.formatting.run(tree, r, &self.para_rpr);
        if rpr.rtl == Some(true) {
            self.note_once(TEXT, RTL);
        }
        if rpr.bordered() {
            self.note_once(NOT_YET_DRAWN, "border");
        }
    }

    /// A table and what its style draws; direct table properties are
    /// noted as the walk reaches them.
    fn table(&mut self, tree: &XmlTree, tbl: NodeId) {
        self.note(NOT_YET_DRAWN, "table");
        let (style, look_first_row) = self.formatting.table(tree, tbl);
        let marks = self.formatting.table_marks(style.as_deref());
        if marks.border {
            self.note(NOT_YET_DRAWN, "border");
        }
        if marks.shading {
            self.note(NOT_YET_DRAWN, "shading");
        }
        if marks.floating {
            self.note(LAYOUT, "floating tables");
        }
        if marks.bidi {
            self.note(TEXT, RTL);
        }
        self.tables.push(TableScan {
            ctx: TableCtx {
                style,
                first_row: false,
            },
            look_first_row,
            rows: 0,
        });
    }

    fn text(&mut self, text: &str) {
        if !text.is_empty() {
            self.note_once(NOT_YET_DRAWN, "text");
            if matches!(self.kind, "header" | "footer") {
                self.note_once(NOT_YET_DRAWN, self.kind);
            }
        }
        self.scripts(text);
    }

    /// An equation, quoted as the line of text the PDF shows.
    fn equation(&mut self, linear: &str) {
        self.note_with(EQUATION, "equation", linear.to_string());
        self.scripts(linear);
    }

    fn scripts(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\u{AD}' {
                self.note_once(NOT_YET_DRAWN, "soft hyphen");
            }
            if let Some(script) = unsupported_script(ch) {
                self.note_once(TEXT, script);
            }
        }
    }

    fn hyperlink(&mut self, tree: &XmlTree, node: NodeId) {
        let target = tree
            .attr(node, Some(R), "id")
            .and_then(|id| self.rels.get(id))
            .map(|rel| rel.target.clone());
        match target {
            Some(target) => self.link(&target),
            None => {
                if let Some(anchor) = w_attr(tree, node, "anchor") {
                    self.anchor(anchor);
                }
            }
        }
    }

    /// A link target: a bookmark, an allowed scheme, or skipped.
    fn link(&mut self, target: &str) {
        let target = target.trim();
        if target.starts_with('#') {
            return self.note(NOT_YET_DRAWN, "link");
        }
        match scheme(target) {
            Some(scheme) if LINK_SCHEMES.contains(&scheme.as_str()) => {
                self.note(NOT_YET_DRAWN, "link")
            }
            scheme => {
                let sample = scheme.map_or_else(|| "relative".to_string(), |s| format!("{s}:"));
                self.note_with(LINK, "link", sample);
            }
        }
    }

    fn anchor(&mut self, anchor: &str) {
        self.note(NOT_YET_DRAWN, "link");
        let location = self.location();
        self.anchors.push((anchor.to_string(), location));
    }

    fn field_char(&mut self, kind: Option<&str>) {
        match kind {
            Some("begin") => self.fields.push(Field {
                instruction: String::new(),
                open: true,
            }),
            Some("separate") => self.close_instruction(),
            Some("end") => {
                self.close_instruction();
                self.fields.pop();
            }
            _ => {}
        }
    }

    fn close_instruction(&mut self) {
        if let Some(field) = self.fields.last_mut().filter(|f| f.open) {
            field.open = false;
            let instruction = std::mem::take(&mut field.instruction);
            self.field(&instruction);
        }
    }

    /// Fields a story leaves without `separate` or `end`.
    fn close_fields(&mut self) {
        while !self.fields.is_empty() {
            self.close_instruction();
            self.fields.pop();
        }
    }

    fn field(&mut self, instruction: &str) {
        let command = instruction
            .split(|c: char| c.is_whitespace() || c == '\\')
            .find(|token| !token.is_empty())
            .unwrap_or("")
            .to_ascii_uppercase();
        match command.as_str() {
            "DATE" => self.note(FIELD, "DATE"),
            "TIME" => self.note(FIELD, "TIME"),
            "PRINTDATE" => self.note(FIELD, "PRINTDATE"),
            // Drawn as saved until the PDF computes its own page numbers.
            "TOC" => self.note(FIELD, "TOC"),
            "PAGEREF" => self.note(FIELD, "PAGEREF"),
            "HYPERLINK" => {
                self.note(NOT_YET_DRAWN, "field");
                match field_link(instruction) {
                    Some(Link::Uri(target)) => self.link(&target),
                    Some(Link::Anchor(anchor)) => self.anchor(&anchor),
                    // The target is a nested field's result, unknown here.
                    _ => self.note_with(LINK, "link", "computed".to_string()),
                }
            }
            _ => self.note(NOT_YET_DRAWN, "field"),
        }
    }

    /// An inline or floating object: a watermark, else its click links and
    /// its wrapping.
    fn drawing(&mut self, tree: &XmlTree, node: NodeId) -> bool {
        let name = tree
            .first_child_named(node, WP, "docPr")
            .and_then(|d| tree.attr(d, None, "name"));
        if is_watermark(name) {
            self.note(WATERMARK, "watermark");
            return false;
        }
        let clicks: Vec<String> = tree
            .descendants(node)
            .filter(|&n| tree.is(n, A, "hlinkClick"))
            .filter_map(|n| tree.attr(n, Some(R), "id"))
            .filter_map(|id| self.rels.get(id).map(|rel| rel.target.clone()))
            .collect();
        for target in clicks {
            self.link(&target);
        }
        let wraps = ["wrapSquare", "wrapTight", "wrapThrough", "wrapTopAndBottom"];
        if wraps
            .iter()
            .any(|wrap| tree.first_child_named(node, WP, wrap).is_some())
        {
            self.note(WRAP, "floating object");
        }
        true
    }

    fn graphic(&mut self, tree: &XmlTree, node: NodeId) -> bool {
        match tree.attr(node, None, "uri") {
            Some(uri) if CHART_URIS.contains(&uri) => self.note(CHART, "chart"),
            Some(DIAGRAM_URI) => self.note(SMARTART, "SmartArt"),
            Some(PICTURE_URI | WPS | WPG | WPC) => return true,
            _ => self.note(SHAPE, "drawing"),
        }
        false
    }

    /// A VML shape. Its text box or picture counts first, so a rule that
    /// holds either is not mistaken for a plain rule.
    fn vml(&mut self, tree: &XmlTree, node: NodeId) {
        for href in tree
            .descendants(node)
            .filter(|&n| tree.ns_uri(n) == Some(VML))
            .filter_map(|n| tree.attr(n, None, "href"))
        {
            self.link(href);
        }
        let find = |name: &str| tree.descendants(node).find(|&n| tree.is(n, VML, name));
        if is_watermark(tree.attr(node, None, "id")) {
            self.note(WATERMARK, "watermark");
        } else if let Some(text_box) = find("textbox") {
            self.text_box(tree, text_box);
        } else if let Some(data) = find("imagedata") {
            self.image(tree.attr(data, Some(R), "id"));
        } else if tree.attr(node, Some(OFFICE), "hr") == Some("t") {
            self.note(NOT_YET_DRAWN, "horizontal rule");
        } else {
            self.note(SHAPE, "shape");
        }
    }

    fn text_box(&mut self, tree: &XmlTree, text_box: NodeId) {
        let text: Vec<String> = tree
            .descendants(text_box)
            .filter(|&n| tree.is(n, W, "p"))
            .map(|p| paragraph_text(tree, p))
            .filter(|t| !t.trim().is_empty())
            .collect();
        self.note_with(TEXT_BOX, "text box", text.join(" "));
    }

    /// A picture by its relationship: TIFF, EMF and WMF are unsupported.
    fn image(&mut self, id: Option<&str>) {
        let source = source_part_name(&self.part);
        let base = base_uri(&source);
        let format = id
            .and_then(|id| self.rels.get(id))
            .filter(|rel| !rel.external)
            .and_then(|rel| self.txn.resolve(&rel.target_part_name(base)))
            .and_then(|part| {
                let content_type = self.txn.content_type(&part).unwrap_or_default();
                unsupported_image(&content_type, &part)
            });
        match format {
            Some(format) => self.note(IMAGE, format),
            None => self.note(NOT_YET_DRAWN, "image"),
        }
    }

    fn note(&mut self, code: &'static str, subject: &'static str) {
        self.note_at(code, subject, true, None);
    }

    fn note_with(&mut self, code: &'static str, subject: &'static str, sample: String) {
        self.note_at(code, subject, true, Some(sample));
    }

    fn note_once(&mut self, code: &'static str, subject: &'static str) {
        if !self.in_paragraph.contains(&(code, subject)) {
            self.in_paragraph.push((code, subject));
            self.note(code, subject);
        }
    }

    fn note_at(
        &mut self,
        code: &'static str,
        subject: &'static str,
        located: bool,
        sample: Option<String>,
    ) {
        if code == NOT_YET_DRAWN && DRAWN.contains(&subject) {
            return;
        }
        let location = located.then(|| self.location());
        self.tally.note(code, subject, location, sample);
    }

    fn location(&self) -> PdfLocation {
        PdfLocation {
            story: self.part.clone(),
            ordinal: self.ordinal,
            page: None,
            kind: self.kind,
        }
    }

    fn finish(mut self) -> PdfReport {
        for (anchor, location) in std::mem::take(&mut self.anchors) {
            if !is_document_start(&anchor) && !self.bookmarks.contains(&bookmark_key(&anchor)) {
                self.tally
                    .note(LINK_TARGET, "link", Some(location), Some(anchor));
            }
        }
        PdfReport {
            warnings: self.tally.finish(),
            pages: None,
        }
    }
}

fn text_of(tree: &XmlTree, node: NodeId) -> String {
    tree.children(node)
        .iter()
        .filter_map(|&child| match tree.data(child) {
            NodeData::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Inside a tracked deletion, which the PDF leaves out.
fn deleted(tree: &XmlTree, node: NodeId) -> bool {
    std::iter::successors(tree.parent(node), |&n| tree.parent(n))
        .any(|n| tree.is(n, W, "del") || tree.is(n, W, "moveFrom"))
}

fn is_watermark(name: Option<&str>) -> bool {
    name.is_some_and(|name| WATERMARK_PREFIXES.iter().any(|p| name.starts_with(p)))
}

/// `w15:footnoteColumns`.
fn footnote_columns(tree: &XmlTree, sect: NodeId) -> u32 {
    tree.first_child_named(sect, W15, "footnoteColumns")
        .and_then(|c| tree.attr(c, Some(W), "val"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

fn columns(tree: &XmlTree, cols: NodeId) -> usize {
    let declared = w_attr(tree, cols, "num")
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    let listed = tree
        .child_elements(cols)
        .filter(|&c| tree.is(c, W, "col"))
        .count();
    declared.max(listed)
}

fn unsupported_image(content_type: &str, part: &str) -> Option<&'static str> {
    let extension = part
        .rsplit_once('.')
        .map_or("", |(_, e)| e)
        .to_ascii_lowercase();
    if content_type.contains("tiff") || matches!(extension.as_str(), "tif" | "tiff") {
        Some("TIFF image")
    } else if content_type.contains("emf") || extension == "emf" {
        Some("EMF image")
    } else if content_type.contains("wmf") || extension == "wmf" {
        Some("WMF image")
    } else {
        None
    }
}

/// The v1 engine draws Latin text without shaping.
fn unsupported_script(ch: char) -> Option<&'static str> {
    match u32::from(ch) {
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x1AB0..=0x1AFF
        | 0x1CD0..=0x1CFF
        | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF
        | 0xFE20..=0xFE2F
        | 0x1D165..=0x1D169
        | 0x1D16D..=0x1D172
        | 0x1D17B..=0x1D182
        | 0x1D185..=0x1D18B
        | 0x1D1AA..=0x1D1AD
        | 0x1D242..=0x1D244 => Some("combining marks"),
        0x0590..=0x08FF
        | 0x200F
        | 0x202B
        | 0x202E
        | 0x2067
        | 0xFB1D..=0xFDFF
        | 0xFE70..=0xFEFC
        | 0x10800..=0x10FFF
        | 0x1E800..=0x1EFFF => Some(RTL),
        0x0900..=0x0FFF
        | 0x1000..=0x109F
        | 0x1700..=0x18AF
        | 0x1900..=0x1AAF
        | 0x1B00..=0x1C4F
        | 0xA800..=0xA8FF
        | 0xA980..=0xAAFF
        | 0xABC0..=0xABFF
        | 0x11000..=0x11FFF => Some("complex script"),
        0x1100..=0x11FF
        | 0x2E80..=0x2FFF
        | 0x3000..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF => Some("CJK text"),
        // Joiners, emoji presentation, skin tones, flags and tag sequences
        // need shaping to form one glyph.
        0x200D | 0xFE0F | 0x1F1E6..=0x1F1FF | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F => {
            Some("emoji sequence")
        }
        _ => None,
    }
}
