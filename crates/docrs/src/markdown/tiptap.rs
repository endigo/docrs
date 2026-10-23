//! TipTap document JSON (`editor.getJSON()`, ProseMirror's document
//! format) folded into the converter's blocks.
//!
//! The document is a tree of nodes, `{"type": "doc", "content": [...]}`,
//! each node `{"type", "attrs"?, "content"?, "marks"?, "text"?}`. The node
//! and mark names of Tiptap 3's own extensions map onto the same tree the
//! Markdown and HTML sources produce, so a document converts the same from
//! its JSON as from its `getHTML()`: paragraphs and headings with
//! `textAlign`, bullet, ordered and task lists, block quotes, code blocks,
//! rules, hard breaks, images (a picture standing alone in its paragraph
//! aligns it by its own `textAlign`), tables with `colspan`, `rowspan` and
//! `colwidth`, the marks bold, italic, underline, strike, code, subscript,
//! superscript, link, highlight and textStyle (colour, font, size,
//! background), inline and block math, details, mentions, YouTube embeds
//! (as links), a table of contents and page breaks. An editor's default
//! alignment (`left`, `start`) is not written, so the reference document's
//! styles keep theirs.
//!
//! Nothing is dropped silently: a node type with no Word equivalent keeps
//! its text (`tiptap_node_dropped`), a mark type keeps its text without the
//! formatting (`tiptap_mark_dropped`), and a formatting attribute with no
//! equivalent is ignored (`tiptap_attribute_dropped`). Attributes that carry
//! no formatting (ids, classes, an app's own data) are ignored like HTML's
//! `class`.
//!
//! Refused, with nothing changed: a source that is not JSON, a root that is
//! not a `doc` node and a node without a `type` (`tiptap_json_invalid`), and
//! arrays and objects nested deeper than [`MAX_NESTING_DEPTH`]
//! (`markdown_nesting_too_deep`; a node and its content are two levels).

use serde_json::{Map, Value};

use super::ast::{
    Align, Block, CellSpan, Inline, ListItem, MAX_NESTING_DEPTH, Parsed, TABLE_CLAMPED_MESSAGE,
    warn_once,
};
use super::html::{
    HtmlCell, HtmlRow, SpanFormat, aligned, assemble_table, css_align, css_color, css_pixels,
    first_family, half_points,
};
use crate::{Error, Result};

/// The line every warning of a JSON conversion is filed under; the document
/// has no lines of its own (editor HTML is treated the same).
const LINE: usize = 1;

/// Parse a TipTap document into blocks. Refuses what the module docs list.
pub(crate) fn parse(source: &str) -> Result<Parsed> {
    let depth = nesting(source);
    if depth > MAX_NESTING_DEPTH {
        return Err(Error::limit(
            "markdown_nesting_too_deep",
            MAX_NESTING_DEPTH as u64,
            depth as u64,
        )
        .context(format!(
            "the JSON nests deeper than {MAX_NESTING_DEPTH} levels of arrays and objects; nothing was changed"
        )));
    }
    let value: Value = serde_json::from_str(source)
        .map_err(|err| invalid(format!("the source is not JSON: {err}")))?;
    let root = read(&value, "the root")?;
    if root.kind != "doc" {
        return Err(invalid(format!(
            "the root must be a node of type \"doc\", got {:?}",
            root.kind
        )));
    }
    let mut converter = Converter::default();
    let blocks = converter.blocks(root.content, false, "doc")?;
    let mut warnings = Vec::new();
    for name in converter.unknown_nodes {
        warn_once(
            &mut warnings,
            "tiptap_node_dropped",
            format!("node type {name:?} has no Word equivalent; its text is kept"),
            Some(LINE),
        );
    }
    for name in converter.unknown_marks {
        warn_once(
            &mut warnings,
            "tiptap_mark_dropped",
            format!("mark type {name:?} has no Word equivalent; its text is kept without it"),
            Some(LINE),
        );
    }
    for what in converter.dropped {
        warn_once(
            &mut warnings,
            "tiptap_attribute_dropped",
            format!("attribute {what} has no Word equivalent and was ignored"),
            Some(LINE),
        );
    }
    if converter.narrowed_table {
        warn_once(
            &mut warnings,
            "table_columns_clamped",
            TABLE_CLAMPED_MESSAGE.to_string(),
            Some(LINE),
        );
    }
    Ok(Parsed {
        blocks,
        warnings,
        html: true,
        ..Parsed::default()
    })
}

fn invalid(message: String) -> Error {
    Error::invalid_argument("tiptap_json_invalid", message)
}

/// The deepest nesting of arrays and objects in `source`. Cheap enough to run
/// before parsing, and serde_json refuses the same depth on its own.
fn nesting(source: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);
    for byte in source.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// A node as read: ProseMirror's `{type, attrs, content, marks, text}`.
/// Marks have the same shape without content.
struct Node<'a> {
    kind: &'a str,
    attrs: Option<&'a Map<String, Value>>,
    content: &'a [Value],
    marks: &'a [Value],
    text: Option<&'a str>,
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn read<'a>(value: &'a Value, place: &str) -> Result<Node<'a>> {
    let Some(object) = value.as_object() else {
        return Err(invalid(format!(
            "{place}: a node must be an object with a \"type\", got {}",
            kind_of(value)
        )));
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Err(invalid(format!("{place}: a node has no \"type\"")));
    };
    Ok(Node {
        kind,
        attrs: object.get("attrs").and_then(Value::as_object),
        content: object
            .get("content")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice),
        marks: object
            .get("marks")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice),
        text: object.get("text").and_then(Value::as_str),
    })
}

fn inside(kind: &str) -> String {
    format!("inside a {kind:?} node")
}

impl<'a> Node<'a> {
    /// An attribute that is set. `null` and `""` count as unset: Tiptap
    /// writes `null` for a default (`textAlign`) and `""` for a style a
    /// text has none of (`textStyle`'s `color`, `highlight`'s `color`).
    fn attr(&self, name: &str) -> Option<&'a Value> {
        self.attrs?
            .get(name)
            .filter(|value| !value.is_null() && value.as_str().is_none_or(|s| !s.trim().is_empty()))
    }

    fn text_attr(&self, name: &str) -> Option<&'a str> {
        self.attr(name)?.as_str()
    }

    fn number_attr(&self, name: &str) -> Option<f64> {
        match self.attr(name)? {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
        .filter(|number| number.is_finite())
    }

    /// A cell span: at least 1, and no wider than HTML's spans are read.
    fn span_attr(&self, name: &str) -> usize {
        self.number_attr(name)
            .map_or(1, |number| (number as i64).clamp(1, 1000) as usize)
    }

    fn flag_attr(&self, name: &str) -> Option<bool> {
        match self.attr(name)? {
            Value::Bool(flag) => Some(*flag),
            Value::String(text) => Some(text == "true"),
            _ => None,
        }
    }
}

fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "hardBreak" | "image" | "mention" | "emoji" | "inlineMath"
    )
}

/// `pageBreak`, `page-break`, `page_break`, … as apps and Tiptap Pages name
/// their page-break node.
fn is_page_break(kind: &str) -> bool {
    kind.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .eq_ignore_ascii_case("pagebreak")
}

fn refs(values: &[Value]) -> Vec<&Value> {
    values.iter().collect()
}

fn paragraph(inlines: Vec<Inline>, plain: bool) -> Block {
    if plain {
        Block::Plain(inlines)
    } else {
        Block::Paragraph(inlines)
    }
}

/// Join a link onto an equal link before it: an editor writes one link as a
/// text node per change of formatting.
fn push(out: &mut Vec<Inline>, inline: Inline) {
    match (out.last_mut(), inline) {
        (Some(Inline::Text(last)), Inline::Text(text)) => last.push_str(&text),
        (
            Some(Inline::Link {
                url,
                title,
                content,
            }),
            Inline::Link {
                url: next_url,
                title: next_title,
                content: next_content,
            },
        ) if *url == next_url && *title == next_title => {
            for inline in next_content {
                push(content, inline);
            }
        }
        (_, inline) => out.push(inline),
    }
}

#[derive(Default)]
struct Converter {
    unknown_nodes: Vec<String>,
    unknown_marks: Vec<String>,
    dropped: Vec<String>,
    narrowed_table: bool,
}

impl Converter {
    fn note_node(&mut self, kind: &str) {
        if !self.unknown_nodes.iter().any(|known| known == kind) {
            self.unknown_nodes.push(kind.to_string());
        }
    }

    fn note_mark(&mut self, kind: &str) {
        if !self.unknown_marks.iter().any(|known| known == kind) {
            self.unknown_marks.push(kind.to_string());
        }
    }

    fn drop_attr(&mut self, attribute: &str, on: &str) {
        let note = format!("{attribute} on {on}");
        if !self.dropped.contains(&note) {
            self.dropped.push(note);
        }
    }

    /// The alignment a block asks for. The editor's default (`null` in
    /// Tiptap 3, `left` before) is not an instruction, so the reference
    /// document's styles keep their own.
    fn align(&mut self, node: &Node) -> Option<Align> {
        let value = node.text_attr("textAlign")?;
        match value.trim().to_ascii_lowercase().as_str() {
            "left" | "start" => None,
            _ => {
                let align = css_align(value);
                if align.is_none() {
                    self.drop_attr("textAlign", node.kind);
                }
                align
            }
        }
    }

    /// `own`, or for a picture standing alone its own `textAlign` (Tiptap's
    /// TextAlign extension configured for `image`).
    fn with_pictures(
        &mut self,
        own: Option<Align>,
        content: &[&Value],
        parent: &str,
    ) -> Option<Align> {
        let mut align = own;
        for value in content {
            // `inlines` read the same nodes already, so none can be refused here.
            let Some(picture) = read(value, &inside(parent))
                .ok()
                .filter(|node| node.kind == "image")
            else {
                continue;
            };
            let wanted = self.align(&picture);
            if own.is_none() && content.len() == 1 {
                align = wanted;
            } else if wanted.is_some() && wanted != align {
                self.drop_attr("textAlign", "image");
            }
        }
        align
    }

    /// A picture with an alignment of its own (Tiptap's block image) is a
    /// paragraph of its own.
    fn blocks(&mut self, nodes: &[Value], plain: bool, parent: &str) -> Result<Vec<Block>> {
        let mut out = Vec::new();
        let mut pending: Vec<&Value> = Vec::new();
        for value in nodes {
            let node = read(value, &inside(parent))?;
            if is_inline(node.kind) {
                let align = (node.kind == "image").then(|| self.align(&node)).flatten();
                if align.is_none() {
                    pending.push(value);
                    continue;
                }
                self.flush(&mut pending, plain, parent, &mut out)?;
                let picture = paragraph(vec![self.image(&node)], plain);
                out.extend(aligned(align, vec![picture]));
                continue;
            }
            self.flush(&mut pending, plain, parent, &mut out)?;
            self.block(&node, plain, &mut out)?;
        }
        self.flush(&mut pending, plain, parent, &mut out)?;
        Ok(out)
    }

    fn flush(
        &mut self,
        pending: &mut Vec<&Value>,
        plain: bool,
        parent: &str,
        out: &mut Vec<Block>,
    ) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let inlines = self.inlines(pending, parent)?;
        pending.clear();
        let blank = inlines
            .iter()
            .all(|inline| matches!(inline, Inline::Text(text) if text.trim().is_empty()));
        if !blank {
            out.push(paragraph(inlines, plain));
        }
        Ok(())
    }

    fn block(&mut self, node: &Node, plain: bool, out: &mut Vec<Block>) -> Result<()> {
        let kind = node.kind;
        match kind {
            "paragraph" => {
                let content = refs(node.content);
                let inlines = self.inlines(&content, kind)?;
                let align = self.align(node);
                let align = self.with_pictures(align, &content, kind);
                out.extend(aligned(align, vec![paragraph(inlines, plain)]));
            }
            "heading" => {
                let level = node
                    .number_attr("level")
                    .map_or(1, |level| level.clamp(1.0, 9.0) as u8);
                let content = refs(node.content);
                let heading = Block::Heading {
                    level,
                    id: node.text_attr("id").map(str::to_string),
                    content: self.inlines(&content, kind)?,
                };
                let align = self.align(node);
                let align = self.with_pictures(align, &content, kind);
                out.extend(aligned(align, vec![heading]));
            }
            "blockquote" => {
                let blocks = self.blocks(node.content, false, kind)?;
                let align = self.align(node);
                out.push(Block::Quote {
                    alert: None,
                    blocks: aligned(align, blocks),
                });
            }
            "codeBlock" => out.push(Block::Code {
                language: node
                    .text_attr("language")
                    .map(str::trim)
                    .filter(|language| !language.is_empty())
                    .map(str::to_string),
                text: self.raw_text(node.content, kind)?,
            }),
            "horizontalRule" => out.push(Block::Rule),
            "bulletList" | "orderedList" | "taskList" => {
                let list = self.list(node)?;
                let align = self.align(node);
                out.extend(aligned(align, vec![list]));
            }
            "listItem" | "taskItem" => {
                let item = self.item(node)?;
                match out.last_mut() {
                    Some(Block::List {
                        start: None,
                        items,
                        tight,
                    }) if *tight => items.push(item),
                    _ => out.push(Block::List {
                        start: None,
                        tight: true,
                        items: vec![item],
                    }),
                }
            }
            "table" => {
                let table = self.table(node)?;
                out.push(table);
            }
            "details" => {
                for child_value in node.content {
                    let child = read(child_value, &inside(kind))?;
                    match child.kind {
                        "detailsSummary" => self.summary(&child, plain, out)?,
                        "detailsContent" => {
                            out.extend(self.blocks(child.content, plain, child.kind)?);
                        }
                        _ => out.extend(self.blocks(
                            std::slice::from_ref(child_value),
                            plain,
                            kind,
                        )?),
                    }
                }
            }
            "detailsSummary" => self.summary(node, plain, out)?,
            "detailsContent" | "doc" => out.extend(self.blocks(node.content, plain, kind)?),
            "blockMath" => {
                if let Some(latex) = node.text_attr("latex") {
                    out.push(Block::DisplayMath(latex.to_string()));
                }
            }
            "tableOfContents" => out.push(Block::Toc),
            // An embedded video has no place in Word; its address does.
            "youtube" => {
                if let Some(src) = node.text_attr("src") {
                    out.push(paragraph(
                        vec![Inline::Link {
                            url: src.to_string(),
                            title: String::new(),
                            content: vec![Inline::Text(src.to_string())],
                        }],
                        plain,
                    ));
                }
            }
            _ if is_page_break(kind) => out.push(Block::PageBreak),
            other => {
                self.note_node(other);
                out.extend(self.blocks(node.content, plain, other)?);
            }
        }
        Ok(())
    }

    fn summary(&mut self, node: &Node, plain: bool, out: &mut Vec<Block>) -> Result<()> {
        let content = refs(node.content);
        let inlines = self.inlines(&content, node.kind)?;
        if !inlines.is_empty() {
            let align = self.with_pictures(None, &content, node.kind);
            let summary = paragraph(vec![Inline::Strong(inlines)], plain);
            out.extend(aligned(align, vec![summary]));
        }
        Ok(())
    }

    fn raw_text(&mut self, nodes: &[Value], parent: &str) -> Result<String> {
        let mut text = String::new();
        for value in nodes {
            let node = read(value, &inside(parent))?;
            match node.kind {
                "text" => text.push_str(node.text.unwrap_or("")),
                "hardBreak" => text.push('\n'),
                other => {
                    self.note_node(other);
                    text.push_str(&self.raw_text(node.content, other)?);
                }
            }
        }
        Ok(text)
    }

    fn inlines(&mut self, nodes: &[&Value], parent: &str) -> Result<Vec<Inline>> {
        let mut out: Vec<Inline> = Vec::new();
        for value in nodes {
            let node = read(value, &inside(parent))?;
            match node.kind {
                "text" => {
                    let text = node.text.unwrap_or("");
                    if !text.is_empty() {
                        for inline in self.marked(text, node.marks, node.kind)? {
                            push(&mut out, inline);
                        }
                    }
                }
                "hardBreak" => out.push(Inline::HardBreak),
                "image" => {
                    let image = self.image(&node);
                    out.push(image);
                }
                // As the editor shows it.
                "mention" => {
                    let label = node
                        .text_attr("label")
                        .or_else(|| node.text_attr("id"))
                        .unwrap_or("");
                    let prefix = node.text_attr("mentionSuggestionChar").unwrap_or("@");
                    let text = format!("{prefix}{label}");
                    for inline in self.marked(&text, node.marks, node.kind)? {
                        push(&mut out, inline);
                    }
                }
                // The glyph is the editor's to look up; the name stays.
                "emoji" => {
                    self.note_node(node.kind);
                    if let Some(name) = node.text_attr("name") {
                        push(&mut out, Inline::Text(format!(":{name}:")));
                    }
                }
                "inlineMath" => {
                    if let Some(latex) = node.text_attr("latex") {
                        out.push(Inline::Math(latex.to_string()));
                    }
                }
                other => {
                    self.note_node(other);
                    out.extend(self.inlines(&refs(node.content), other)?);
                }
            }
        }
        Ok(out)
    }

    /// `text` with its `marks` applied, nested as the HTML source nests the
    /// same formatting.
    fn marked(&mut self, text: &str, marks: &[Value], on: &str) -> Result<Vec<Inline>> {
        let mut format = SpanFormat::default();
        let mut code = false;
        let mut link: Option<(String, String)> = None;
        let mut highlight_given = false;
        for value in marks {
            let mark = read(value, &format!("a mark of a {on:?} node"))?;
            match mark.kind {
                "bold" => format.bold = true,
                "italic" => format.italic = true,
                "underline" => format.underline = true,
                "strike" => format.strike = true,
                "superscript" => format.superscript = true,
                "subscript" => format.subscript = true,
                "code" => code = true,
                // `target`, `rel` and `class` are the page's concern, not
                // the document's.
                "link" => {
                    link = mark.text_attr("href").map(|href| {
                        (
                            href.to_string(),
                            mark.text_attr("title").unwrap_or("").to_string(),
                        )
                    });
                }
                "highlight" => match mark.text_attr("color") {
                    None => format.default_highlight = true,
                    Some(color) => match css_color(color) {
                        Some(color) => {
                            format.style.highlight = color;
                            highlight_given = true;
                        }
                        None => self.drop_attr("color", "highlight"),
                    },
                },
                "textStyle" => {
                    for (name, value) in mark.attrs.into_iter().flatten() {
                        let text = match value {
                            Value::String(text) if !text.trim().is_empty() => text.clone(),
                            Value::Number(number) => number.to_string(),
                            _ => continue,
                        };
                        match name.as_str() {
                            "color" => match css_color(&text) {
                                Some(Some(color)) => format.style.color = Some(color),
                                Some(None) => {}
                                None => self.drop_attr("color", "textStyle"),
                            },
                            "fontFamily" => {
                                format.style.font =
                                    first_family(&text).or(format.style.font.take());
                            }
                            "fontSize" => match css_pixels(&text) {
                                Some(px) => format.style.size = Some(half_points(px)),
                                None => self.drop_attr("fontSize", "textStyle"),
                            },
                            "backgroundColor" => match css_color(&text) {
                                Some(color) => {
                                    if !highlight_given {
                                        format.style.highlight = color;
                                    }
                                    highlight_given = true;
                                }
                                None => self.drop_attr("backgroundColor", "textStyle"),
                            },
                            "lineHeight" => self.drop_attr("lineHeight", "textStyle"),
                            // An app's own attribute.
                            _ => {}
                        }
                    }
                }
                other => self.note_mark(other),
            }
        }
        if highlight_given {
            format.default_highlight = false;
        }
        let content = vec![if code {
            Inline::Code(text.to_string())
        } else {
            Inline::Text(text.to_string())
        }];
        let mut content = format.wrap(content);
        if let Some((url, title)) = link {
            content = vec![Inline::Link {
                url,
                title,
                content,
            }];
        }
        Ok(content)
    }

    fn image(&mut self, node: &Node) -> Inline {
        let mut size = [None, None];
        for (slot, name) in size.iter_mut().zip(["width", "height"]) {
            *slot = match node.attr(name) {
                None => None,
                Some(Value::Number(number)) => {
                    number.as_f64().filter(|n| n.is_finite() && *n > 0.0)
                }
                Some(Value::String(text)) => {
                    let px = css_pixels(text);
                    if px.is_none() && text.trim() != "auto" {
                        self.drop_attr(name, "image");
                    }
                    px
                }
                Some(_) => {
                    self.drop_attr(name, "image");
                    None
                }
            };
        }
        Inline::Image {
            url: node.text_attr("src").unwrap_or("").to_string(),
            title: node.text_attr("title").unwrap_or("").to_string(),
            alt: vec![Inline::Text(
                node.text_attr("alt").unwrap_or("").to_string(),
            )],
            line: LINE,
            width: size[0],
            height: size[1],
        }
    }

    fn list(&mut self, node: &Node) -> Result<Block> {
        let ordered = node.kind == "orderedList";
        if ordered
            && let Some(style) = node.text_attr("type")
            && style != "1"
        {
            self.drop_attr("type", node.kind);
        }
        let mut items = Vec::new();
        for value in node.content {
            let child = read(value, &inside(node.kind))?;
            items.push(match child.kind {
                "listItem" | "taskItem" => self.item(&child)?,
                _ => ListItem {
                    task: None,
                    blocks: self.blocks(std::slice::from_ref(value), true, node.kind)?,
                },
            });
        }
        let tight = items
            .iter()
            .all(|item| !item.blocks.iter().any(|b| matches!(b, Block::Paragraph(_))));
        Ok(Block::List {
            start: ordered.then(|| {
                node.number_attr("start")
                    .filter(|start| *start >= 0.0)
                    .map_or(1, |start| start as u64)
            }),
            tight,
            items,
        })
    }

    fn item(&mut self, node: &Node) -> Result<ListItem> {
        let task = (node.kind == "taskItem").then(|| node.flag_attr("checked").unwrap_or(false));
        Ok(ListItem {
            task,
            blocks: self.blocks(node.content, true, node.kind)?,
        })
    }

    fn table(&mut self, node: &Node) -> Result<Block> {
        let mut rows: Vec<HtmlRow> = Vec::new();
        for row_value in node.content {
            let row = read(row_value, &inside(node.kind))?;
            if row.kind != "tableRow" {
                let cell = HtmlCell {
                    blocks: self.blocks(std::slice::from_ref(row_value), true, node.kind)?,
                    span: CellSpan::default(),
                    colwidth: vec![None],
                };
                rows.push(HtmlRow {
                    header: false,
                    cells: vec![cell],
                });
                continue;
            }
            let mut cells = Vec::new();
            let mut all_header = true;
            for cell_value in row.content {
                let cell = read(cell_value, &inside(row.kind))?;
                if cell.kind != "tableCell" && cell.kind != "tableHeader" {
                    all_header = false;
                    cells.push(HtmlCell {
                        blocks: self.blocks(std::slice::from_ref(cell_value), true, row.kind)?,
                        span: CellSpan::default(),
                        colwidth: vec![None],
                    });
                    continue;
                }
                all_header &= cell.kind == "tableHeader";
                let span = CellSpan {
                    colspan: cell.span_attr("colspan"),
                    rowspan: cell.span_attr("rowspan"),
                    valign: None,
                };
                let mut colwidth: Vec<Option<f64>> = match cell.attr("colwidth") {
                    Some(Value::Array(widths)) => widths
                        .iter()
                        .map(|width| width.as_f64().filter(|w| w.is_finite() && *w > 0.0))
                        .collect(),
                    Some(Value::Number(width)) => {
                        vec![width.as_f64().filter(|w| w.is_finite() && *w > 0.0)]
                    }
                    _ => Vec::new(),
                };
                if colwidth.is_empty() && span.colspan == 1 {
                    colwidth.push(None);
                }
                // Tiptap 3.32's cell `align`, as `<td align>`.
                let align = cell.text_attr("align").and_then(css_align);
                let blocks = self.blocks(cell.content, true, cell.kind)?;
                cells.push(HtmlCell {
                    blocks: aligned(align, blocks),
                    span,
                    colwidth,
                });
            }
            rows.push(HtmlRow {
                header: all_header && !cells.is_empty(),
                cells,
            });
        }
        let (table, narrowed) = assemble_table(rows, Vec::new(), None);
        self.narrowed_table |= narrowed;
        Ok(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_counts_brackets_outside_strings() {
        assert_eq!(
            nesting(r#"{"type":"doc","content":[{"type":"paragraph"}]}"#),
            3
        );
        assert_eq!(nesting(r#"{"text":"[[{{\"}}]]"}"#), 1);
        assert_eq!(nesting(r#"{"text":"a\\"}"#), 1);
        assert_eq!(nesting("]]]}}}"), 0);
        assert_eq!(nesting(&"[".repeat(500)), 500);
    }

    #[test]
    fn marks_nest_like_html_spans() {
        let parsed = parse(
            r##"{"type":"doc","content":[{"type":"paragraph","attrs":{"textAlign":"left"},"content":[
                {"type":"text","text":"a","marks":[{"type":"bold"},{"type":"link","attrs":{"href":"https://e.org"}}]},
                {"type":"text","text":"b","marks":[{"type":"link","attrs":{"href":"https://e.org"}}]},
                {"type":"text","text":" c","marks":[{"type":"textStyle","attrs":{"color":"#ff0000","lineHeight":"2"}},{"type":"highlight","attrs":{"color":null}}]}]}]}"##,
        )
        .unwrap();
        let [Block::Paragraph(inlines)] = parsed.blocks.as_slice() else {
            panic!("{:?}", parsed.blocks);
        };
        assert_eq!(inlines.len(), 2, "{inlines:?}");
        assert!(
            matches!(&inlines[0], Inline::Link { url, content, .. }
                if url == "https://e.org" && content.len() == 2
                    && matches!(&content[0], Inline::Strong(_))
                    && matches!(&content[1], Inline::Text(t) if t == "b")),
            "{inlines:?}"
        );
        assert!(
            matches!(&inlines[1], Inline::Styled(style, inner)
                if style.color.as_deref() == Some("FF0000")
                    && matches!(inner.as_slice(), [Inline::Highlight(_)])),
            "{inlines:?}"
        );
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.warnings[0].code, "tiptap_attribute_dropped");
        assert!(
            parsed.warnings[0]
                .message
                .contains("lineHeight on textStyle")
        );
    }

    #[test]
    fn structure_is_checked() {
        for (source, needle) in [
            ("[]", "the root"),
            ("{\"type\":\"paragraph\"}", "\"doc\""),
            (
                "{\"type\":\"doc\",\"content\":[1]}",
                "inside a \"doc\" node",
            ),
            (
                "{\"type\":\"doc\",\"content\":[{\"attrs\":{}}]}",
                "has no \"type\"",
            ),
            (
                "{\"type\":\"doc\",\"content\":[{\"type\":\"text\",\"text\":\"x\",\"marks\":[{}]}]}",
                "a mark of a \"text\" node",
            ),
            ("nope", "not JSON"),
        ] {
            let err = parse(source).unwrap_err();
            assert_eq!(err.reason(), "tiptap_json_invalid", "{source}");
            assert!(
                err.message().contains(needle),
                "{source}: {}",
                err.message()
            );
        }
    }
}
