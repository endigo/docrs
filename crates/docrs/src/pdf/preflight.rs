//! The scan behind [`preflight`]: every construct of the body, the headers
//! and footers the sections show, and the footnotes and endnotes, sorted
//! into drawn, not drawn yet, and unsupported in v1.

use std::collections::HashMap;

use super::{MAX_LOCATIONS, NOT_YET_DRAWN, PdfLocation, PdfReport, PdfWarning, description};
use crate::layout::model::{StoryRef, on_off, section_properties, story_references};
use crate::package::rels::{Relationship, rt};
use crate::txn::Txn;
use crate::wml::{A, M, PIC, R, W, WP, first_choice_children, paragraph_text, w_attr, w_child};
use crate::xml::{NodeData, NodeId, XmlTree};
use crate::{Document, Result};

const VML: &str = "urn:schemas-microsoft-com:vml";
const OFFICE: &str = "urn:schemas-microsoft-com:office:office";
const WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPC: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas";
const CHART_URIS: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "http://schemas.microsoft.com/office/drawing/2014/chartex",
];
const DIAGRAM_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
/// Word names its watermark shapes with these prefixes.
const WATERMARK_PREFIXES: &[&str] = &["PowerPlusWaterMarkObject", "WordPictureWatermark"];
const LINK_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

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

/// Subjects the painter draws; each painter PR adds its own.
const DRAWN: &[&str] = &[];

/// Quoted input kept per warning, and its length.
const MAX_SAMPLES: usize = 5;
const SAMPLE_CHARS: usize = 60;

/// What a PDF of `document` would leave out or change. Reads only.
pub fn preflight(document: &Document) -> Result<PdfReport> {
    let txn = Txn::new(document);
    let main = document.package().main_part_name();
    let tree = txn.tree(main)?;
    let body = w_child(tree, tree.root(), "body");
    let mut scan = Scan::new(&txn);
    scan.story(main, tree, body.unwrap_or(tree.root()))?;
    let settings = match txn.part_related_by(main, rt::SETTINGS)? {
        Some(part) => Some(txn.tree(&part)?),
        None => None,
    };
    let setting = |name: &str| {
        settings.is_some_and(|s| w_child(s, s.root(), name).is_some_and(|n| on_off(s, n)))
    };
    if let Some(body) = body {
        for part in shown_stories(&txn, main, tree, body, setting("evenAndOddHeaders"))? {
            let story = txn.tree(&part)?;
            scan.story(&part, story, story.root())?;
        }
    }
    for rel in [rt::FOOTNOTES, rt::ENDNOTES] {
        if let Some(part) = txn.part_related_by(main, rel)? {
            scan.notes(&part, txn.tree(&part)?)?;
        }
    }
    if setting("autoHyphenation") {
        scan.note_at(LAYOUT, "automatic hyphenation", None, None);
    }
    Ok(scan.finish())
}

/// The headers and footers some page shows: each section's own, else the
/// previous section's of the same type; first-page ones only with
/// `w:titlePg`, even-page ones only with `w:evenAndOddHeaders`.
fn shown_stories(
    txn: &Txn<'_>,
    main: &str,
    tree: &XmlTree,
    body: NodeId,
    even_pages: bool,
) -> Result<Vec<String>> {
    let rels = txn.resolved_relationships(main)?;
    let mut current: HashMap<(&str, String), &str> = HashMap::new();
    let mut shown: Vec<String> = Vec::new();
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
                    && !shown.iter().any(|s| s == part)
                {
                    shown.push(part.to_string());
                }
            }
        }
    }
    Ok(shown)
}

struct Entry {
    code: &'static str,
    subject: &'static str,
    count: usize,
    locations: Vec<PdfLocation>,
    samples: Vec<String>,
}

/// A complex field: its instruction so far, and whether the instruction
/// is still open (before `separate`).
struct Field {
    instruction: String,
    open: bool,
}

struct Scan<'s, 'd> {
    txn: &'s Txn<'d>,
    part: String,
    rels: Vec<Relationship>,
    ordinal: usize,
    /// Subjects noted once per paragraph, for the current one.
    in_paragraph: Vec<(&'static str, &'static str)>,
    fields: Vec<Field>,
    entries: Vec<Entry>,
}

impl<'s, 'd> Scan<'s, 'd> {
    fn new(txn: &'s Txn<'d>) -> Self {
        Scan {
            txn,
            part: String::new(),
            rels: Vec::new(),
            ordinal: 0,
            in_paragraph: Vec::new(),
            fields: Vec::new(),
            entries: Vec::new(),
        }
    }

    fn story(&mut self, part: &str, tree: &XmlTree, root: NodeId) -> Result<()> {
        self.enter(part)?;
        self.walk(tree, root)
    }

    /// Footnotes and endnotes, without the separators Word draws itself.
    fn notes(&mut self, part: &str, tree: &XmlTree) -> Result<()> {
        self.enter(part)?;
        for note in tree.child_elements(tree.root()) {
            if w_attr(tree, note, "type").is_none_or(|t| t == "normal") {
                self.walk(tree, note)?;
            }
        }
        Ok(())
    }

    fn enter(&mut self, part: &str) -> Result<()> {
        self.part = part.to_string();
        self.rels = self.txn.relationships(part)?;
        self.ordinal = 0;
        self.in_paragraph.clear();
        self.fields.clear();
        Ok(())
    }

    fn walk(&mut self, tree: &XmlTree, node: NodeId) -> Result<()> {
        for child in first_choice_children(tree, node) {
            if self.visit(tree, child)? {
                self.walk(tree, child)?;
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
                self.note(EQUATION, "equation");
                false
            }
            Some(WP) if local == "anchor" => self.anchor(tree, node),
            Some(A) if local == "graphicData" => self.graphic(tree, node),
            Some(PIC) if local == "pic" => {
                let blip = tree
                    .descendants(node)
                    .find(|&n| tree.is(n, A, "blip"))
                    .and_then(|blip| tree.attr(blip, Some(R), "embed"));
                self.image(blip)?;
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
            Some(VML) => self.vml(tree, node, local)?,
            _ => true,
        })
    }

    fn wordml(&mut self, tree: &XmlTree, node: NodeId, local: &str) -> bool {
        let parent = tree.parent(node).and_then(|p| tree.local_name(p));
        let val = w_attr(tree, node, "val");
        let on = on_off(tree, node);
        match local {
            "p" => {
                self.ordinal += 1;
                self.in_paragraph.clear();
            }
            "t" => {
                self.text(&text_of(tree, node));
                return false;
            }
            "sym" => self.note_once(NOT_YET_DRAWN, "text"),
            "tab" if parent == Some("r") => self.note_once(NOT_YET_DRAWN, "tab"),
            "br" if matches!(w_attr(tree, node, "type"), Some("page" | "column")) => {
                self.note(NOT_YET_DRAWN, "page break")
            }
            "pageBreakBefore" if on => self.note(NOT_YET_DRAWN, "page break"),
            "numPr"
                if w_child(tree, node, "numId").and_then(|n| w_attr(tree, n, "val"))
                    != Some("0") =>
            {
                self.note(NOT_YET_DRAWN, "list")
            }
            "tbl" => self.note(NOT_YET_DRAWN, "table"),
            "tblpPr" => self.note(LAYOUT, "floating tables"),
            "hyperlink" => self.hyperlink(tree, node),
            "footnoteReference" => self.note(NOT_YET_DRAWN, "footnote"),
            "endnoteReference" => self.note(NOT_YET_DRAWN, "endnote"),
            "fldSimple" => self.field(w_attr(tree, node, "instr").unwrap_or("")),
            "fldChar" => self.field_char(w_attr(tree, node, "fldCharType")),
            "instrText" => {
                if let Some(field) = self.fields.last_mut().filter(|f| f.open) {
                    field.instruction.push_str(&text_of(tree, node));
                }
                return false;
            }
            "pBdr" | "tblBorders" | "tcBorders" | "bdr" => self.note(NOT_YET_DRAWN, "border"),
            "shd"
                if w_attr(tree, node, "fill").is_some_and(|f| f != "auto")
                    || !matches!(val, None | Some("clear" | "nil")) =>
            {
                self.note(NOT_YET_DRAWN, "shading")
            }
            "highlight" if val.is_some_and(|v| v != "none") => self.note(NOT_YET_DRAWN, "shading"),
            "ins" | "del" | "moveFrom" | "moveTo" | "cellIns" | "cellDel" | "cellMerge" => {
                self.note(REVISIONS, "tracked change")
            }
            "rPrChange" | "pPrChange" | "sectPrChange" | "tblPrChange" | "tblGridChange"
            | "trPrChange" | "tcPrChange" | "tblPrExChange" | "numberingChange" => {
                self.note(REVISIONS, "tracked change");
                return false;
            }
            "commentReference" => self.note(REVISIONS, "comment"),
            "object" => {
                self.note(OBJECT, "OLE object");
                return false;
            }
            "framePr" => match w_attr(tree, node, "dropCap") {
                Some("drop" | "margin") => self.note(LAYOUT, "drop caps"),
                _ => self.note(LAYOUT, "frames"),
            },
            "textDirection" if parent == Some("tcPr") && !horizontal(val) => {
                self.note(LAYOUT, "rotated cell text")
            }
            "cols" if columns(tree, node) > 1 => self.note(COLUMNS, "multi-column section"),
            "lnNumType" => self.note(LAYOUT, "line numbers"),
            "pgBorders" => self.note(LAYOUT, "page borders"),
            "vAlign" if parent == Some("sectPr") && !matches!(val, None | Some("top")) => {
                self.note(LAYOUT, "vertical page alignment")
            }
            "bidi" | "rtl" if on => self.note_once(TEXT, "right-to-left text"),
            "delText" | "delInstrText" => return false,
            _ => {}
        }
        true
    }

    fn text(&mut self, text: &str) {
        if !text.is_empty() {
            self.note_once(NOT_YET_DRAWN, "text");
        }
        for ch in text.chars() {
            if let Some(script) = unsupported_script(ch) {
                self.note_once(TEXT, script);
            }
        }
    }

    fn hyperlink(&mut self, tree: &XmlTree, node: NodeId) {
        let target = tree
            .attr(node, Some(R), "id")
            .and_then(|id| self.rels.iter().find(|rel| rel.id == id))
            .map(|rel| rel.target.clone());
        match target {
            Some(target) if !target.starts_with('#') => match scheme(&target) {
                Some(scheme) if LINK_SCHEMES.contains(&scheme.as_str()) => {
                    self.note(NOT_YET_DRAWN, "link")
                }
                scheme => {
                    let sample = scheme.map_or_else(|| "relative".to_string(), |s| format!("{s}:"));
                    self.note_with(LINK, "link", sample);
                }
            },
            _ if w_attr(tree, node, "anchor").is_some() || target.is_some() => {
                self.note(NOT_YET_DRAWN, "link")
            }
            _ => {}
        }
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

    fn field(&mut self, instruction: &str) {
        let command = instruction
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        match command.as_str() {
            "DATE" => self.note(FIELD, "DATE"),
            "TIME" => self.note(FIELD, "TIME"),
            "PRINTDATE" => self.note(FIELD, "PRINTDATE"),
            _ => self.note(NOT_YET_DRAWN, "field"),
        }
    }

    /// A floating object: a watermark, else its wrapping.
    fn anchor(&mut self, tree: &XmlTree, node: NodeId) -> bool {
        let name = tree
            .first_child_named(node, WP, "docPr")
            .and_then(|d| tree.attr(d, None, "name"));
        if is_watermark(name) {
            self.note(WATERMARK, "watermark");
            return false;
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
            _ => return true,
        }
        false
    }

    fn vml(&mut self, tree: &XmlTree, node: NodeId, local: &str) -> Result<bool> {
        let shapes = [
            "shape",
            "rect",
            "roundrect",
            "oval",
            "line",
            "polyline",
            "arc",
            "curve",
            "group",
            "image",
        ];
        if !shapes.contains(&local) {
            return Ok(local != "shapetype");
        }
        let find = |name: &str| tree.descendants(node).find(|&n| tree.is(n, VML, name));
        if is_watermark(tree.attr(node, None, "id")) {
            self.note(WATERMARK, "watermark");
        } else if tree.attr(node, Some(OFFICE), "hr") == Some("t") {
            self.note(NOT_YET_DRAWN, "horizontal rule");
        } else if let Some(text_box) = find("textbox") {
            self.text_box(tree, text_box);
        } else if let Some(data) = find("imagedata") {
            self.image(tree.attr(data, Some(R), "id"))?;
        } else {
            self.note(SHAPE, "shape");
        }
        Ok(false)
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
    fn image(&mut self, id: Option<&str>) -> Result<()> {
        let part = match id {
            Some(id) => self.txn.part_related_by_id(&self.part, id)?,
            None => None,
        };
        let format = part.as_deref().and_then(|part| {
            let content_type = self.txn.content_type(part).unwrap_or_default();
            unsupported_image(&content_type, part)
        });
        match format {
            Some(format) => self.note(IMAGE, format),
            None => self.note(NOT_YET_DRAWN, "image"),
        }
        Ok(())
    }

    fn note(&mut self, code: &'static str, subject: &'static str) {
        let location = Some(self.location());
        self.note_at(code, subject, location, None);
    }

    fn note_with(&mut self, code: &'static str, subject: &'static str, sample: String) {
        let location = Some(self.location());
        self.note_at(code, subject, location, Some(sample));
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
        location: Option<PdfLocation>,
        sample: Option<String>,
    ) {
        if code == NOT_YET_DRAWN && DRAWN.contains(&subject) {
            return;
        }
        let index = match self
            .entries
            .iter()
            .position(|e| e.code == code && e.subject == subject)
        {
            Some(index) => index,
            None => {
                self.entries.push(Entry {
                    code,
                    subject,
                    count: 0,
                    locations: Vec::new(),
                    samples: Vec::new(),
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[index];
        entry.count += 1;
        if let Some(location) = location.filter(|_| entry.locations.len() < MAX_LOCATIONS) {
            entry.locations.push(location);
        }
        if let Some(sample) = sample.filter(|_| entry.samples.len() < MAX_SAMPLES) {
            entry
                .samples
                .push(sample.chars().take(SAMPLE_CHARS).collect());
        }
    }

    fn location(&self) -> PdfLocation {
        PdfLocation {
            story: self.part.clone(),
            ordinal: self.ordinal,
            page: None,
        }
    }

    fn finish(self) -> PdfReport {
        let warnings = self
            .entries
            .into_iter()
            .map(|entry| {
                let mut message = format!(
                    "{} ({}): {}",
                    entry.subject,
                    entry.count,
                    description(entry.code)
                );
                if !entry.samples.is_empty() {
                    let quoted: Vec<String> =
                        entry.samples.iter().map(|s| format!("\"{s}\"")).collect();
                    message.push_str(&format!(" ({})", quoted.join(", ")));
                }
                PdfWarning {
                    code: entry.code,
                    subject: entry.subject,
                    message,
                    locations: entry.locations,
                    count: entry.count,
                }
            })
            .collect();
        PdfReport { warnings }
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

fn is_watermark(name: Option<&str>) -> bool {
    name.is_some_and(|name| WATERMARK_PREFIXES.iter().any(|p| name.starts_with(p)))
}

/// `w:textDirection` values that keep text horizontal.
fn horizontal(value: Option<&str>) -> bool {
    matches!(value, None | Some("lrTb" | "tb" | "lrTbV" | "tbV"))
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

/// The lower-cased scheme of an absolute URI; `None` for a relative one.
fn scheme(target: &str) -> Option<String> {
    let (scheme, _) = target.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
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
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F => {
            Some("combining marks")
        }
        0x0590..=0x08FF
        | 0x200F
        | 0x202B
        | 0x202E
        | 0x2067
        | 0xFB1D..=0xFDFF
        | 0xFE70..=0xFEFC
        | 0x10800..=0x10FFF
        | 0x1E800..=0x1EFFF => Some("right-to-left text"),
        0x0900..=0x0FFF
        | 0x1000..=0x109F
        | 0x1780..=0x18AF
        | 0x1900..=0x1AAF
        | 0x1B00..=0x1C4F
        | 0xA800..=0xA8FF
        | 0xA980..=0xAAFF
        | 0x11000..=0x11FFF => Some("complex script"),
        0x1100..=0x11FF
        | 0x2E80..=0x2FFF
        | 0x3000..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF => Some("CJK text"),
        _ => None,
    }
}
