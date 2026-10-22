//! Raw HTML blocks in Markdown, for the subset editors and HTML-to-Markdown
//! tools emit: paragraphs, headings, lists, quotes, rules, line breaks,
//! inline emphasis and links, and tables whose cells hold paragraphs or
//! lists. Anything else is reported by tag name and its text is kept.

use super::ast::{
    Align, Block, CellSpan, Inline, ListItem, MAX_NESTING_DEPTH, MAX_TABLE_COLUMNS, RunStyle, Slot,
    TableLayout, VAlign, place, too_deep,
};
use crate::Result;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    Close(String),
    Text(String),
}

fn decode_entities(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest[1..].find(';').map(|e| e + 1).filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(html: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        if rest.starts_with('<')
            && let Some(end) = rest.find('>')
        {
            let inner = rest[1..end].trim();
            rest = &rest[end + 1..];
            if let Some(name) = inner.strip_prefix('/') {
                tokens.push(Token::Close(name.trim().to_ascii_lowercase()));
                continue;
            }
            if inner.starts_with('!') || inner.starts_with('?') {
                continue;
            }
            let self_closing = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
            let name = inner[..name_end].to_ascii_lowercase();
            // Script, style and title text is not markup: it runs to the
            // element's end tag.
            let raw = !self_closing && RAW_TEXT.contains(&name.as_str());
            tokens.push(Token::Open {
                attrs: attributes(&inner[name_end..]),
                name: name.clone(),
                self_closing,
            });
            if raw {
                let end = find_ignore_case(rest, &format!("</{name}")).unwrap_or(rest.len());
                if end > 0 {
                    tokens.push(Token::Text(rest[..end].to_string()));
                }
                rest = &rest[end..];
            }
            continue;
        }
        let end = rest.find('<').filter(|i| *i > 0).unwrap_or(rest.len());
        tokens.push(Token::Text(decode_entities(&rest[..end])));
        rest = &rest[end..];
    }
    tokens
}

/// Elements whose content is text up to their end tag.
const RAW_TEXT: &[&str] = &["script", "style", "title"];

/// Byte offset of the first ASCII-case-insensitive match of `needle`.
fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn attributes(text: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let value = if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (value, remaining) = match after.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let close = after[1..].find(q).map_or(after.len(), |i| i + 1);
                    (
                        &after[1..close.min(after.len())],
                        after.get(close + 1..).unwrap_or(""),
                    )
                }
                _ => {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            rest = remaining.trim_start();
            decode_entities(value)
        } else {
            String::new()
        };
        if !name.is_empty() {
            attrs.push((name, value));
        }
    }
    attrs
}

/// An element still open while building the tree: name, attributes,
/// children so far.
type OpenElement = (String, Vec<(String, String)>, Vec<Node>);

const VOID: &[&str] = &[
    "br", "hr", "img", "col", "wbr", "meta", "link", "input", "base", "basefont", "param", "area",
];

/// Whether `tag` is a void element (it has no content or end tag).
pub(crate) fn is_void(tag: &str) -> bool {
    VOID.contains(&tag)
}

/// Elements browsers do not display (the HTML standard's rendering rules
/// give them `display: none`, and `noscript` with scripting on): they are
/// dropped with their content.
const HIDDEN: &[&str] = &[
    "head", "title", "style", "script", "template", "noscript", "noembed", "noframes", "datalist",
    "rp", "meta", "link", "base", "basefont", "param", "area",
];

/// `name` as listed in [`HIDDEN`], when browsers do not display it.
pub(crate) fn hidden_tag(name: &str) -> Option<&'static str> {
    HIDDEN.iter().find(|hidden| **hidden == name).copied()
}

/// A parsed element tree.
#[derive(Debug, Clone)]
enum Node {
    Element {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<Node>,
    },
    Text(String),
}

/// The element tree of `tokens`, nested inside `depth` levels of the
/// surrounding document; refused past [`MAX_NESTING_DEPTH`].
fn tree(tokens: Vec<Token>, depth: usize) -> Result<Vec<Node>> {
    // Stack of open elements; the root collects top-level nodes.
    let mut stack: Vec<OpenElement> = vec![(String::new(), Vec::new(), Vec::new())];
    for token in tokens {
        match token {
            Token::Text(text) => stack.last_mut().expect("root").2.push(Node::Text(text)),
            Token::Open {
                name,
                attrs,
                self_closing,
            } => {
                // Implied end tags: a new <p>, <li>, <tr>, <td>, <th> closes
                // an open sibling of the same family.
                let closes: &[&str] = match name.as_str() {
                    "p" => &["p"],
                    "li" => &["li", "p"],
                    "tr" => &["tr", "td", "th", "p"],
                    "td" | "th" => &["td", "th", "p"],
                    _ => &[],
                };
                if let Some(position) = stack
                    .iter()
                    .rposition(|(n, _, _)| closes.contains(&n.as_str()))
                {
                    let barrier = stack.iter().rposition(|(n, _, _)| {
                        matches!(n.as_str(), "table" | "ul" | "ol" | "tbody" | "thead")
                    });
                    if barrier.is_none_or(|b| b < position) {
                        while stack.len() > position {
                            close_top(&mut stack);
                        }
                        // `<p><li>`: the item closes a paragraph that never
                        // had content; browsers draw nothing for it.
                        let parent = &mut stack.last_mut().expect("root").2;
                        if name == "li"
                            && matches!(parent.last(), Some(Node::Element { name, children, .. })
                                if name == "p" && children.is_empty())
                        {
                            parent.pop();
                        }
                    }
                }
                if self_closing || VOID.contains(&name.as_str()) {
                    stack.last_mut().expect("root").2.push(Node::Element {
                        name,
                        attrs,
                        children: Vec::new(),
                    });
                } else {
                    // Every open element but the root is a level.
                    if depth + stack.len() > MAX_NESTING_DEPTH {
                        return Err(too_deep(depth + stack.len()));
                    }
                    stack.push((name, attrs, Vec::new()));
                }
            }
            Token::Close(name) => {
                if let Some(position) = stack.iter().rposition(|(n, _, _)| *n == name)
                    && position > 0
                {
                    while stack.len() > position {
                        close_top(&mut stack);
                    }
                }
            }
        }
    }
    while stack.len() > 1 {
        close_top(&mut stack);
    }
    Ok(stack.pop().expect("root").2)
}

fn close_top(stack: &mut Vec<OpenElement>) {
    let (name, attrs, children) = stack.pop().expect("an open element");
    stack.last_mut().expect("parent").2.push(Node::Element {
        name,
        attrs,
        children,
    });
}

/// Conversion state: what had no mapping.
#[derive(Default)]
struct Converter {
    /// Tag names with no Word equivalent (their text is kept).
    unknown: Vec<String>,
    /// CSS declarations with no Word equivalent, as `property on <tag>`.
    styles: Vec<String>,
    /// Elements browsers do not display, dropped with their content.
    skipped: Vec<String>,
    /// A table was narrowed to Word's column limit.
    narrowed_table: bool,
    /// Source line of the HTML (for image warnings).
    line: usize,
}

const INLINE: &[&str] = &[
    "a", "b", "strong", "i", "em", "cite", "u", "ins", "s", "del", "strike", "code", "kbd", "samp",
    "tt", "sup", "sub", "mark", "span", "font", "br", "small", "big", "abbr", "img",
];

fn is_inline(node: &Node) -> bool {
    match node {
        Node::Text(_) => true,
        Node::Element { name, .. } => INLINE.contains(&name.as_str()),
    }
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// `style` declarations as `(property, value)`, the property lower-cased.
fn declarations(attrs: &[(String, String)]) -> Vec<(String, String)> {
    let Some(style) = attr(attrs, "style") else {
        return Vec::new();
    };
    style
        .split(';')
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            let value = value.trim();
            let value = value.strip_suffix("!important").unwrap_or(value).trim();
            (!property.is_empty() && !value.is_empty()).then(|| (property, value.to_string()))
        })
        .collect()
}

const NAMED_COLORS: &[(&str, &str)] = &[
    ("black", "000000"),
    ("white", "FFFFFF"),
    ("red", "FF0000"),
    ("green", "008000"),
    ("blue", "0000FF"),
    ("yellow", "FFFF00"),
    ("orange", "FFA500"),
    ("purple", "800080"),
    ("gray", "808080"),
    ("grey", "808080"),
    ("silver", "C0C0C0"),
    ("maroon", "800000"),
    ("olive", "808000"),
    ("lime", "00FF00"),
    ("aqua", "00FFFF"),
    ("cyan", "00FFFF"),
    ("teal", "008080"),
    ("navy", "000080"),
    ("fuchsia", "FF00FF"),
    ("magenta", "FF00FF"),
    ("pink", "FFC0CB"),
    ("brown", "A52A2A"),
    ("gold", "FFD700"),
    ("darkred", "8B0000"),
    ("darkgreen", "006400"),
    ("darkblue", "00008B"),
    ("lightgray", "D3D3D3"),
    ("lightgrey", "D3D3D3"),
    ("darkgray", "A9A9A9"),
    ("darkgrey", "A9A9A9"),
];

/// A CSS colour as `RRGGBB`. `Some(None)` for values that set no colour of
/// their own (`inherit`, `transparent`, …); `None` when unreadable.
pub(crate) fn css_color(value: &str) -> Option<Option<String>> {
    let value = value.trim().to_ascii_lowercase();
    if matches!(
        value.as_str(),
        "inherit"
            | "initial"
            | "unset"
            | "revert"
            | "currentcolor"
            | "transparent"
            | "auto"
            | "none"
    ) {
        return Some(None);
    }
    if let Some(hex) = value.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let rgb: String = match hex.len() {
            3 | 4 => hex.chars().take(3).flat_map(|c| [c, c]).collect(),
            6 | 8 => hex[..6].to_string(),
            _ => return None,
        };
        return Some(Some(rgb.to_ascii_uppercase()));
    }
    if let Some(args) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = args
            .split([',', ' ', '/'])
            .filter(|part| !part.is_empty())
            .collect();
        if !(3..=4).contains(&parts.len()) {
            return None;
        }
        let number = |part: &str, scale: f64| -> Option<f64> {
            match part.strip_suffix('%') {
                Some(percent) => percent.parse::<f64>().ok().map(|p| p / 100.0 * scale),
                None => part.parse::<f64>().ok(),
            }
        };
        if let Some(alpha) = parts.get(3)
            && number(alpha, 1.0)? <= 0.0
        {
            return Some(None);
        }
        let mut rgb = String::new();
        for part in &parts[..3] {
            let channel = number(part, 255.0)?.round().clamp(0.0, 255.0) as u8;
            rgb.push_str(&format!("{channel:02X}"));
        }
        return Some(Some(rgb));
    }
    NAMED_COLORS
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, rgb)| Some((*rgb).to_string()))
}

/// The first family of a CSS `font-family` list; a generic family becomes a
/// font every Word installation has.
pub(crate) fn first_family(value: &str) -> Option<String> {
    let first = value
        .split(',')
        .next()?
        .trim()
        .trim_matches(['"', '\''])
        .trim();
    let generic = match first.to_ascii_lowercase().as_str() {
        "" | "inherit" | "initial" | "unset" | "revert" => return None,
        "serif" | "ui-serif" => "Times New Roman",
        "sans-serif" | "ui-sans-serif" | "system-ui" | "-apple-system" => "Arial",
        "monospace" | "ui-monospace" => "Courier New",
        "cursive" => "Comic Sans MS",
        _ => return Some(first.to_string()),
    };
    Some(generic.to_string())
}

/// A CSS length in pixels (`200`, `200px`, `150pt`, `2in`, `5cm`); `None`
/// for relative units (`%`, `em`) and anything unreadable.
pub(crate) fn css_pixels(value: &str) -> Option<f64> {
    let value = value.trim().to_ascii_lowercase();
    let (number, factor) = [
        ("px", 1.0),
        ("pt", 96.0 / 72.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
    ]
    .iter()
    .find_map(|(unit, factor)| value.strip_suffix(unit).map(|n| (n, *factor)))
    .unwrap_or((value.as_str(), 1.0));
    number
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n * factor)
}

/// A column width: CSS pixels, or a percentage of the table's width
/// (negative, so the two stay apart until the table's width is known).
fn css_column_width(value: &str) -> Option<f64> {
    match value.trim().strip_suffix('%') {
        Some(percent) => percent
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite() && *p > 0.0)
            .map(|p| -p),
        None => css_pixels(value),
    }
}

pub(crate) fn css_align(value: &str) -> Option<Align> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(Align::Left),
        "center" | "middle" => Some(Align::Center),
        "right" | "end" => Some(Align::Right),
        "justify" => Some(Align::Justify),
        _ => None,
    }
}

fn css_valign(value: &str) -> Option<VAlign> {
    match value.trim().to_ascii_lowercase().as_str() {
        "top" => Some(VAlign::Top),
        "middle" | "center" => Some(VAlign::Center),
        "bottom" => Some(VAlign::Bottom),
        _ => None,
    }
}

/// Which sides of an element ask for a page break: the editor's
/// `data-page-break` marker, CSS `page-break-before/after: always` and
/// `break-before/after: page`. `(before, after)`.
fn page_break_sides(attrs: &[(String, String)]) -> (bool, bool) {
    let mut sides = (attr(attrs, "data-page-break").is_some(), false);
    for (property, value) in declarations(attrs) {
        let value = value.to_ascii_lowercase();
        match property.as_str() {
            "page-break-before" if value == "always" => sides.0 = true,
            "break-before" if value == "page" => sides.0 = true,
            "page-break-after" if value == "always" => sides.1 = true,
            "break-after" if value == "page" => sides.1 = true,
            _ => {}
        }
    }
    sides
}

/// Whether `nodes` hold no text (only whitespace and line breaks).
fn is_blank(nodes: &[Node]) -> bool {
    nodes.iter().all(|node| match node {
        Node::Text(text) => text.trim().is_empty(),
        Node::Element { name, children, .. } => {
            name == "br" || (name != "img" && is_blank(children))
        }
    })
}

/// `blocks` with `align` applied to each one that has text of its own.
pub(crate) fn aligned(align: Option<Align>, blocks: Vec<Block>) -> Vec<Block> {
    let Some(align) = align else {
        return blocks;
    };
    blocks
        .into_iter()
        .map(|block| match block {
            Block::Paragraph(_)
            | Block::Plain(_)
            | Block::Heading { .. }
            | Block::List { .. }
            | Block::Quote { .. }
            | Block::DefinitionList(_) => Block::Aligned(align, Box::new(block)),
            other => other,
        })
        .collect()
}

/// Run formatting a span asks for through `style` (and `<font>` / `<mark>`
/// attributes), with the declarations that have no Word equivalent.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpanFormat {
    pub style: RunStyle,
    /// `<mark>` with no colour of its own: Word's yellow highlight.
    pub default_highlight: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub superscript: bool,
    pub subscript: bool,
    /// `property on <tag>` for each declaration that was ignored.
    pub dropped: Vec<String>,
}

/// A font size in CSS pixels as Word's half-points.
pub(crate) fn half_points(px: f64) -> u32 {
    ((px * 1.5).round() as u32).clamp(2, 3276)
}

pub(crate) fn span_format(tag: &str, attrs: &[(String, String)]) -> SpanFormat {
    let mut format = SpanFormat::default();
    let drop = |format: &mut SpanFormat, property: &str| {
        format.dropped.push(format!("{property} on <{tag}>"));
    };
    let mut highlight_given = false;
    if tag == "font" {
        match attr(attrs, "color").map(css_color) {
            Some(Some(color)) => format.style.color = color,
            Some(None) => drop(&mut format, "color"),
            None => {}
        }
        if let Some(face) = attr(attrs, "face") {
            format.style.font = first_family(face);
        }
        if attr(attrs, "size").is_some() {
            drop(&mut format, "size");
        }
    }
    if tag == "mark"
        && let Some(color) = attr(attrs, "data-color")
    {
        match css_color(color) {
            Some(color) => {
                format.style.highlight = color;
                highlight_given = true;
            }
            None => drop(&mut format, "data-color"),
        }
    }
    for (property, value) in declarations(attrs) {
        let lower = value.to_ascii_lowercase();
        match property.as_str() {
            "color" => match css_color(&value) {
                Some(Some(color)) => format.style.color = Some(color),
                Some(None) => {}
                None => drop(&mut format, &property),
            },
            "background-color" | "background" => match css_color(&value) {
                Some(color) => {
                    if !highlight_given {
                        format.style.highlight = color;
                    }
                    highlight_given = true;
                }
                None => drop(&mut format, &property),
            },
            "font-family" => format.style.font = first_family(&value).or(format.style.font),
            "font-size" => match css_pixels(&value) {
                Some(px) => format.style.size = Some(half_points(px)),
                None => drop(&mut format, &property),
            },
            "font-weight" => {
                format.bold = matches!(lower.as_str(), "bold" | "bolder")
                    || lower.parse::<u32>().is_ok_and(|weight| weight >= 600);
            }
            "font-style" => format.italic = lower.contains("italic") || lower.contains("oblique"),
            "text-decoration" | "text-decoration-line" => {
                format.underline = lower.contains("underline");
                format.strike = lower.contains("line-through");
            }
            "vertical-align" => match lower.as_str() {
                "super" => format.superscript = true,
                "sub" => format.subscript = true,
                "baseline" => {}
                _ => drop(&mut format, &property),
            },
            _ => drop(&mut format, &property),
        }
    }
    format.default_highlight = tag == "mark" && !highlight_given;
    format
}

impl SpanFormat {
    /// `content` with this formatting applied.
    pub(crate) fn wrap(self, content: Vec<Inline>) -> Vec<Inline> {
        let mut content = content;
        if self.default_highlight {
            content = vec![Inline::Highlight(content)];
        }
        if self.style != RunStyle::default() {
            content = vec![Inline::Styled(self.style, content)];
        }
        for (on, wrap) in [
            (self.bold, Inline::Strong as fn(Vec<Inline>) -> Inline),
            (self.italic, Inline::Emphasis),
            (self.underline, Inline::Underline),
            (self.strike, Inline::Strike),
            (self.superscript, Inline::Superscript),
            (self.subscript, Inline::Subscript),
        ] {
            if on {
                content = vec![wrap(content)];
            }
        }
        content
    }
}

/// The formatting of an inline start tag written in Markdown
/// (`<span style="color: red">`).
pub(crate) fn tag_format(html: &str) -> Option<SpanFormat> {
    match tokenize(html).into_iter().next()? {
        Token::Open { name, attrs, .. } => Some(span_format(&name, &attrs)),
        _ => None,
    }
}

/// A table cell as an editor wrote it: its blocks, spans and widths.
pub(crate) struct HtmlCell {
    pub blocks: Vec<Block>,
    pub span: CellSpan,
    /// TipTap's `colwidth` (one width per spanned column).
    pub colwidth: Vec<Option<f64>>,
}

pub(crate) struct HtmlRow {
    pub header: bool,
    pub cells: Vec<HtmlCell>,
}

impl Converter {
    fn note(&mut self, name: &str) {
        if !self.unknown.iter().any(|u| u == name) {
            self.unknown.push(name.to_string());
        }
    }

    /// Whether `name` is an element browsers do not display; it is noted
    /// as skipped.
    fn skip(&mut self, name: &str) -> bool {
        let Some(tag) = hidden_tag(name) else {
            return false;
        };
        if !self.skipped.iter().any(|s| s == tag) {
            self.skipped.push(tag.to_string());
        }
        true
    }

    fn note_style(&mut self, tag: &str, property: &str) {
        let note = format!("{property} on <{tag}>");
        if !self.styles.contains(&note) {
            self.styles.push(note);
        }
    }

    /// The alignment a block element asks for (`style="text-align: …"`,
    /// `align`); declarations other than `handled` ones are reported.
    fn block_align(
        &mut self,
        tag: &str,
        attrs: &[(String, String)],
        handled: &[&str],
    ) -> Option<Align> {
        let mut align = attr(attrs, "align").and_then(css_align);
        for (property, value) in declarations(attrs) {
            match property.as_str() {
                "text-align" => align = css_align(&value).or(align),
                "page-break-before" | "page-break-after" | "break-before" | "break-after" => {}
                other if handled.contains(&other) => {}
                _ => self.note_style(tag, &property),
            }
        }
        align
    }

    fn image(&mut self, attrs: &[(String, String)]) -> Inline {
        let mut size = [("width", None), ("height", None)];
        for (name, value) in size.iter_mut() {
            if let Some(given) = attr(attrs, name) {
                *value = css_pixels(given);
                if value.is_none() && given.trim() != "auto" {
                    self.note_style("img", name);
                }
            }
        }
        for (property, value) in declarations(attrs) {
            match property.as_str() {
                "width" | "height" => {
                    let slot = &mut size[usize::from(property == "height")].1;
                    match css_pixels(&value) {
                        Some(px) => *slot = Some(px),
                        None if value.trim() == "auto" => {}
                        None => self.note_style("img", &property),
                    }
                }
                // Pictures never exceed the text width anyway.
                "max-width" | "max-height" | "display" => {}
                _ => self.note_style("img", &property),
            }
        }
        Inline::Image {
            url: attr(attrs, "src").unwrap_or("").to_string(),
            title: attr(attrs, "title").unwrap_or("").to_string(),
            alt: vec![Inline::Text(attr(attrs, "alt").unwrap_or("").to_string())],
            line: self.line,
            width: size[0].1,
            height: size[1].1,
        }
    }

    fn inlines(&mut self, nodes: &[Node]) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::new();
        for node in nodes {
            match node {
                Node::Text(text) => {
                    // HTML collapses whitespace runs.
                    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    let lead = text.starts_with(char::is_whitespace) && !collapsed.is_empty();
                    let trail = text.ends_with(char::is_whitespace) && !collapsed.is_empty();
                    let mut piece = String::new();
                    if lead || (collapsed.is_empty() && !text.is_empty()) {
                        piece.push(' ');
                    }
                    piece.push_str(&collapsed);
                    if trail && !collapsed.is_empty() {
                        piece.push(' ');
                    }
                    if !piece.is_empty() {
                        match out.last_mut() {
                            Some(Inline::Text(last)) => last.push_str(&piece),
                            _ => out.push(Inline::Text(piece)),
                        }
                    }
                }
                Node::Element { name, .. } if self.skip(name) => {}
                Node::Element {
                    name,
                    attrs,
                    children,
                } => {
                    let inline = match name.as_str() {
                        "br" => {
                            out.push(Inline::HardBreak);
                            continue;
                        }
                        "img" => {
                            let image = self.image(attrs);
                            out.push(image);
                            continue;
                        }
                        "b" | "strong" => vec![Inline::Strong(self.inlines(children))],
                        "i" | "em" | "cite" => vec![Inline::Emphasis(self.inlines(children))],
                        "u" | "ins" => vec![Inline::Underline(self.inlines(children))],
                        "s" | "del" | "strike" => vec![Inline::Strike(self.inlines(children))],
                        "sup" => vec![Inline::Superscript(self.inlines(children))],
                        "sub" => vec![Inline::Subscript(self.inlines(children))],
                        "code" | "kbd" | "samp" | "tt" => {
                            let content = self.inlines(children);
                            vec![Inline::Code(super::ast::inline_text(&content))]
                        }
                        "a" => vec![Inline::Link {
                            url: attr(attrs, "href").unwrap_or("").to_string(),
                            title: attr(attrs, "title").unwrap_or("").to_string(),
                            content: self.inlines(children),
                        }],
                        "mark" | "span" | "font" | "small" | "big" | "abbr" => {
                            self.inlines(children)
                        }
                        other => {
                            self.note(other);
                            self.inlines(children)
                        }
                    };
                    let format = span_format(name, attrs);
                    for dropped in &format.dropped {
                        if !self.styles.contains(dropped) {
                            self.styles.push(dropped.clone());
                        }
                    }
                    for inline in format.wrap(inline) {
                        match (out.last_mut(), inline) {
                            (Some(Inline::Text(last)), Inline::Text(text)) => last.push_str(&text),
                            (_, inline) => out.push(inline),
                        }
                    }
                }
            }
        }
        out
    }

    /// Block content of `nodes`; loose inline runs become paragraphs (or
    /// plain blocks when `plain`).
    fn blocks(&mut self, nodes: &[Node], plain: bool) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending: Vec<Node> = Vec::new();
        let flush = |this: &mut Converter, pending: &mut Vec<Node>, out: &mut Vec<Block>| {
            if pending.is_empty() {
                return;
            }
            let inlines = this.inlines(pending);
            pending.clear();
            let blank = inlines
                .iter()
                .all(|i| matches!(i, Inline::Text(t) if t.trim().is_empty()));
            if !blank {
                let inlines = trim_inlines(inlines);
                out.push(if plain {
                    Block::Plain(inlines)
                } else {
                    Block::Paragraph(inlines)
                });
            }
        };
        for node in nodes {
            if is_inline(node) {
                pending.push(node.clone());
                continue;
            }
            flush(self, &mut pending, &mut out);
            let Node::Element {
                name,
                attrs,
                children,
            } = node
            else {
                continue;
            };
            if self.skip(name) {
                continue;
            }
            let (break_before, break_after) = page_break_sides(attrs);
            if (break_before || break_after) && is_blank(children) {
                out.push(Block::PageBreak);
                continue;
            }
            if break_before {
                out.push(Block::PageBreak);
            }
            match name.as_str() {
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "figure"
                | "figcaption" | "center" => {
                    // The editor wraps each picture in a `div` carrying its
                    // alignment.
                    let align = attr(attrs, "data-image-align")
                        .and_then(css_align)
                        .or(self.block_align(name, attrs, &[]))
                        .or((name == "center").then_some(Align::Center));
                    let has_blocks = children.iter().any(|c| !is_inline(c));
                    let inner = if has_blocks {
                        self.blocks(children, plain)
                    } else {
                        let inlines = trim_inlines(self.inlines(children));
                        vec![if plain {
                            Block::Plain(inlines)
                        } else {
                            Block::Paragraph(inlines)
                        }]
                    };
                    out.extend(aligned(align, inner));
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let align = self.block_align(name, attrs, &[]);
                    let heading = Block::Heading {
                        level: name[1..].parse().unwrap_or(1),
                        id: attr(attrs, "id").map(str::to_string),
                        content: trim_inlines(self.inlines(children)),
                    };
                    out.extend(aligned(align, vec![heading]));
                }
                "blockquote" => {
                    let align = self.block_align(name, attrs, &[]);
                    let blocks = self.blocks(children, false);
                    out.push(Block::Quote {
                        alert: None,
                        blocks: aligned(align, blocks),
                    });
                }
                "pre" => {
                    self.block_align(name, attrs, &[]);
                    out.push(Block::Code {
                        language: None,
                        text: super::ast::inline_text(&self.inlines_raw(children)),
                    });
                }
                "hr" => {
                    self.block_align(name, attrs, &[]);
                    out.push(Block::Rule);
                }
                "ul" | "ol" => {
                    let align = self.block_align(name, attrs, &[]);
                    let list = self.list(name == "ol", attrs, children);
                    out.extend(aligned(align, vec![list]));
                }
                // An item outside any list (`<p><li>…</li></p>`) is drawn as
                // a bullet; consecutive ones form one list.
                "li" => {
                    let Block::List { items, .. } =
                        self.list(false, &[], std::slice::from_ref(node))
                    else {
                        unreachable!("list() returns a list")
                    };
                    match out.last_mut() {
                        Some(Block::List {
                            start: None,
                            items: previous,
                            tight,
                        }) if *tight => previous.extend(items),
                        _ => out.push(Block::List {
                            start: None,
                            tight: true,
                            items,
                        }),
                    }
                }
                "table" => out.extend(self.table(attrs, children)),
                "dl" => out.push(self.definition_list(children)),
                "html" | "body" | "thead" | "tbody" | "tfoot" => {
                    out.extend(self.blocks(children, plain));
                }
                other => {
                    self.note(other);
                    out.extend(self.blocks(children, plain));
                }
            }
            if break_after {
                out.push(Block::PageBreak);
            }
        }
        flush(self, &mut pending, &mut out);
        out
    }

    /// Preformatted text: whitespace kept.
    fn inlines_raw(&mut self, nodes: &[Node]) -> Vec<Inline> {
        let mut out = Vec::new();
        for node in nodes {
            match node {
                Node::Text(text) => out.push(Inline::Text(text.clone())),
                Node::Element { name, .. } if name == "br" => out.push(Inline::Text("\n".into())),
                Node::Element { name, .. } if self.skip(name) => {}
                Node::Element { children, .. } => out.extend(self.inlines_raw(children)),
            }
        }
        out
    }

    fn list(&mut self, ordered: bool, attrs: &[(String, String)], children: &[Node]) -> Block {
        // TipTap task lists: `<li data-checked="true">`.
        let tasks = attr(attrs, "data-type") == Some("taskList");
        let mut items = Vec::new();
        for child in children {
            if let Node::Element {
                name,
                attrs,
                children,
            } = child
                && name == "li"
            {
                let align = self.block_align(name, attrs, &[]);
                let task = (tasks || attr(attrs, "data-checked").is_some())
                    .then(|| attr(attrs, "data-checked") == Some("true"));
                // A task item's checkbox `label` holds no text.
                let children: Vec<Node> = if task.is_some() {
                    children
                        .iter()
                        .filter(|c| !matches!(c, Node::Element { name, .. } if name == "label"))
                        .flat_map(|c| match c {
                            Node::Element { name, children, .. } if name == "div" => {
                                children.clone()
                            }
                            other => vec![other.clone()],
                        })
                        .collect()
                } else {
                    children.clone()
                };
                let blocks = self.blocks(&children, true);
                items.push(ListItem {
                    task,
                    blocks: aligned(align, blocks),
                });
            }
        }
        let tight = items
            .iter()
            .all(|item| !item.blocks.iter().any(|b| matches!(b, Block::Paragraph(_))));
        Block::List {
            start: ordered.then(|| {
                attr(attrs, "start")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1)
            }),
            tight,
            items,
        }
    }

    /// A table, preceded by its `caption` (as a `Table:` caption paragraph).
    fn table(&mut self, attrs: &[(String, String)], children: &[Node]) -> Vec<Block> {
        let mut rows: Vec<HtmlRow> = Vec::new();
        let mut widths: Vec<Option<f64>> = Vec::new();
        let mut caption = Vec::new();
        self.collect_rows(children, false, &mut rows, &mut widths, &mut caption);
        let mut table_width = attr(attrs, "width").and_then(css_pixels);
        for (property, value) in declarations(attrs) {
            match property.as_str() {
                "width" => table_width = css_pixels(&value).or(table_width),
                "min-width" | "max-width" | "table-layout" | "border-collapse" => {}
                _ => self.note_style("table", &property),
            }
        }
        let (table, narrowed) = assemble_table(rows, widths, table_width);
        self.narrowed_table |= narrowed;
        let mut out = Vec::new();
        if !caption.is_empty() {
            let mut inlines = vec![Inline::Text("Table: ".into())];
            inlines.extend(trim_inlines(caption));
            out.push(Block::Paragraph(inlines));
        }
        out.push(table);
        out
    }

    fn collect_rows(
        &mut self,
        nodes: &[Node],
        in_head: bool,
        rows: &mut Vec<HtmlRow>,
        widths: &mut Vec<Option<f64>>,
        caption: &mut Vec<Inline>,
    ) {
        for node in nodes {
            let Node::Element {
                name,
                attrs,
                children,
            } = node
            else {
                continue;
            };
            if self.skip(name) {
                continue;
            }
            match name.as_str() {
                "thead" => self.collect_rows(children, true, rows, widths, caption),
                "tbody" | "tfoot" => self.collect_rows(children, false, rows, widths, caption),
                "caption" => caption.extend(self.inlines(children)),
                "colgroup" => self.collect_rows(children, in_head, rows, widths, caption),
                "col" => {
                    let mut width = attr(attrs, "width").and_then(css_column_width);
                    for (property, value) in declarations(attrs) {
                        match property.as_str() {
                            "width" => width = css_column_width(&value).or(width),
                            // TipTap's floor for columns the user never sized.
                            "min-width" => {}
                            _ => self.note_style("col", &property),
                        }
                    }
                    // A span widens the table no further than Word's last
                    // column.
                    let room = MAX_TABLE_COLUMNS.saturating_sub(widths.len()).max(1);
                    let span = attr(attrs, "span")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, room);
                    widths.extend(std::iter::repeat_n(width, span));
                }
                "tr" => {
                    // Cells inherit the row's alignment.
                    let row_align = self.block_align(name, attrs, &[]);
                    let mut cells = Vec::new();
                    let mut all_th = true;
                    for cell in children {
                        let Node::Element {
                            name,
                            attrs,
                            children,
                        } = cell
                        else {
                            continue;
                        };
                        if name != "td" && name != "th" {
                            continue;
                        }
                        all_th &= name == "th";
                        let count = |key: &str| {
                            attr(attrs, key)
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(1)
                                .clamp(1, 1000)
                        };
                        let mut align = attr(attrs, "align").and_then(css_align).or(row_align);
                        let mut valign = attr(attrs, "valign").and_then(css_valign);
                        let mut width = attr(attrs, "width").and_then(css_column_width);
                        for (property, value) in declarations(attrs) {
                            match property.as_str() {
                                "text-align" => align = css_align(&value).or(align),
                                "vertical-align" => valign = css_valign(&value).or(valign),
                                "width" => width = css_column_width(&value).or(width),
                                "min-width" | "max-width" => {}
                                _ => self.note_style(name, &property),
                            }
                        }
                        let span = CellSpan {
                            colspan: count("colspan"),
                            rowspan: count("rowspan"),
                            valign,
                        };
                        let mut colwidth: Vec<Option<f64>> = attr(attrs, "colwidth")
                            .map(|list| {
                                list.split(',')
                                    .map(|w| w.trim().parse::<f64>().ok().filter(|w| *w > 0.0))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if colwidth.is_empty() && span.colspan == 1 {
                            colwidth.push(width);
                        }
                        let blocks = self.blocks(children, true);
                        cells.push(HtmlCell {
                            blocks: aligned(align, blocks),
                            span,
                            colwidth,
                        });
                    }
                    rows.push(HtmlRow {
                        header: in_head || (all_th && !cells.is_empty()),
                        cells,
                    });
                }
                _ => self.collect_rows(children, in_head, rows, widths, caption),
            }
        }
    }

    fn definition_list(&mut self, children: &[Node]) -> Block {
        let mut entries: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        for child in children {
            if let Node::Element { name, children, .. } = child {
                match name.as_str() {
                    "dt" => entries.push((trim_inlines(self.inlines(children)), Vec::new())),
                    "dd" => {
                        let blocks = self.blocks(children, false);
                        match entries.last_mut() {
                            Some((_, definitions)) => definitions.push(blocks),
                            None => entries.push((Vec::new(), vec![blocks])),
                        }
                    }
                    _ => {}
                }
            }
        }
        Block::DefinitionList(entries)
    }
}

/// The table block of `rows` as an editor wrote them: leading header rows
/// repeat, spans are placed on the grid (narrowed to Word's 63 columns),
/// each cell's `colwidth` fills the grid columns `widths` (from `colgroup`)
/// left open, and percentage widths are of `table_width`. Also whether the
/// table was narrowed.
pub(crate) fn assemble_table(
    mut rows: Vec<HtmlRow>,
    mut widths: Vec<Option<f64>>,
    table_width: Option<f64>,
) -> (Block, bool) {
    let header_rows = rows.iter().take_while(|row| row.header).count();
    let narrowed = narrow(&mut rows);
    let spans: Vec<Vec<CellSpan>> = rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.span).collect())
        .collect();
    let (placed, columns) = place(&spans);
    widths.resize(columns, None);
    // TipTap's `colwidth` fills the columns `colgroup` left open.
    for (row, slots) in rows.iter().zip(&placed) {
        for slot in slots {
            let Slot::Cell { index, col, .. } = *slot else {
                continue;
            };
            for (offset, width) in row.cells[index].colwidth.iter().enumerate() {
                if let Some(entry) = widths.get_mut(col + offset)
                    && entry.is_none()
                {
                    *entry = *width;
                }
            }
        }
    }
    // Percentages are of the table's width; without one they are
    // proportions like any other.
    for width in widths.iter_mut().flatten() {
        if *width < 0.0 {
            *width = match table_width {
                Some(table) => -*width / 100.0 * table,
                None => -*width,
            };
        }
    }
    let table = Block::Table {
        aligns: vec![Align::None; columns],
        head: Vec::new(),
        rows: rows
            .into_iter()
            .map(|row| row.cells.into_iter().map(|cell| cell.blocks).collect())
            .collect(),
        layout: Some(Box::new(TableLayout {
            spans,
            header_rows,
            widths,
            table_width,
        })),
    };
    (table, narrowed)
}

/// Fit a table into Word's 63 columns: spans reaching past the last column
/// are cut back (as [`place`] places them), and cells that would start past
/// it move their content into the cell before them in the row, or into the
/// cell covering that column from a row above. Returns whether anything
/// changed.
fn narrow(rows: &mut [HtmlRow]) -> bool {
    let mut changed = false;
    // A row's 64th cell and later start past the last column whatever the
    // spans before them do, so they move into the 63rd before the grid is
    // laid out: a long row over many short ones then costs rows × 63
    // slots, not rows × cells.
    for row in rows.iter_mut() {
        if row.cells.len() > MAX_TABLE_COLUMNS {
            let overflow: Vec<Block> = row
                .cells
                .drain(MAX_TABLE_COLUMNS..)
                .flat_map(|cell| cell.blocks)
                .collect();
            row.cells[MAX_TABLE_COLUMNS - 1].blocks.extend(overflow);
            changed = true;
        }
    }
    let spans: Vec<Vec<CellSpan>> = rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.span).collect())
        .collect();
    let (placed, _) = place(&spans);
    // `(row, cell index)` moves into `(row, cell index)`.
    let mut moves: Vec<((usize, usize), (usize, usize))> = Vec::new();
    for (row, slots) in placed.iter().enumerate() {
        let mut target = None;
        for slot in slots {
            match *slot {
                Slot::Cell {
                    index, col, span, ..
                } if col < MAX_TABLE_COLUMNS => {
                    changed |= span.colspan < spans[row][index].colspan;
                    target = Some((row, index));
                }
                Slot::Continue { col, .. } if col < MAX_TABLE_COLUMNS => {
                    // The cell above that starts this column's span.
                    target = (0..row).rev().find_map(|above| {
                        placed[above].iter().find_map(|slot| match *slot {
                            Slot::Cell { index, col: c, .. } if c == col => Some((above, index)),
                            _ => None,
                        })
                    });
                }
                Slot::Cell { index, .. } => {
                    if let Some(target) = target {
                        moves.push(((row, index), target));
                    }
                }
                _ => {}
            }
        }
    }
    if moves.is_empty() {
        return changed;
    }
    for &((row, index), (to_row, to_index)) in &moves {
        let blocks = std::mem::take(&mut rows[row].cells[index].blocks);
        rows[to_row].cells[to_index].blocks.extend(blocks);
    }
    // Remove the emptied cells, last first so the indexes stay valid.
    for &((row, index), _) in moves.iter().rev() {
        rows[row].cells.remove(index);
    }
    true
}

/// Leading and trailing whitespace of a paragraph is not content, nor is a
/// final line break (browsers draw `<p>a<br></p>` as one line).
fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    if matches!(inlines.last(), Some(Inline::HardBreak)) {
        inlines.pop();
    }
    if let Some(Inline::Text(first)) = inlines.first_mut() {
        *first = first.trim_start().to_string();
    }
    if let Some(Inline::Text(last)) = inlines.last_mut() {
        *last = last.trim_end().to_string();
    }
    inlines.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
    inlines
}

/// What an HTML conversion could not map: tag names (their text is kept),
/// CSS declarations (`property on <tag>`), elements browsers do not display
/// (dropped with their content) and whether a table was narrowed to Word's
/// column limit.
#[derive(Debug, Default)]
pub(crate) struct Unmapped {
    pub tags: Vec<String>,
    pub styles: Vec<String>,
    pub skipped: Vec<String>,
    pub narrowed_table: bool,
}

/// Blocks for an HTML fragment starting on source `line`, nested inside
/// `depth` levels of Markdown, and what had no mapping. Refuses elements
/// nested past [`MAX_NESTING_DEPTH`].
pub(crate) fn convert(html: &str, line: usize, depth: usize) -> Result<(Vec<Block>, Unmapped)> {
    let mut converter = Converter {
        line,
        ..Converter::default()
    };
    let nodes = tree(tokenize(html), depth)?;
    let blocks = converter.blocks(&nodes, false);
    Ok((
        blocks,
        Unmapped {
            tags: converter.unknown,
            styles: converter.styles,
            skipped: converter.skipped,
            narrowed_table: converter.narrowed_table,
        },
    ))
}

/// Whether `html` is one empty paragraph (`<p></p>`, `<p><br></p>`):
/// an intentional blank line.
pub(crate) fn is_empty_paragraph(html: &str) -> bool {
    let Ok(nodes) = tree(tokenize(html.trim()), 0) else {
        return false;
    };
    matches!(nodes.as_slice(), [Node::Element { name, children, .. }]
    if name == "p" && children.iter().all(|c| match c {
        Node::Text(t) => t.trim().is_empty(),
        Node::Element { name, .. } => name == "br",
    }))
}

/// Whether `html` is nothing but one empty element asking for a page break
/// (`<div data-page-break="">`, `<div style="page-break-after: always">`).
pub(crate) fn is_page_break_element(html: &str) -> bool {
    let Ok(nodes) = tree(tokenize(html.trim()), 0) else {
        return false;
    };
    matches!(nodes.as_slice(), [Node::Element { attrs, children, .. }]
        if page_break_sides(attrs) != (false, false) && is_blank(children))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(html: &str) -> (Vec<Block>, Unmapped) {
        convert(html, 1, 0).unwrap()
    }

    #[test]
    fn tables_with_lists_in_cells() {
        let (blocks, unmapped) = read(
            "<table><tbody><tr><th>Step</th><th align=\"right\">Checks</th></tr><tr><td><p>Rollover</p></td>\
             <td><ul><li><p>Confirm the closing balance</p></li><li><p>Confirm the insurance cover</p></li></ul></td></tr></tbody></table>",
        );
        assert!(unmapped.tags.is_empty(), "{unmapped:?}");
        let [Block::Table { rows, layout, .. }] = blocks.as_slice() else {
            panic!("{blocks:?}")
        };
        let layout = layout.as_ref().expect("an HTML table has a layout");
        assert_eq!(layout.header_rows, 1);
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0][1][0], Block::Aligned(Align::Right, _)));
        assert_eq!(
            rows[1][0],
            vec![Block::Plain(vec![Inline::Text("Rollover".into())])]
        );
        assert!(
            matches!(&rows[1][1][0], Block::List { start: None, items, .. } if items.len() == 2)
        );
    }

    #[test]
    fn inline_markup_entities_and_implied_ends() {
        let (blocks, _) =
            read("<p>A &amp; <b>B</b><br>c<p>d &#8212; <a href=\"https://x.y\">e</a>");
        assert_eq!(
            blocks[0],
            Block::Paragraph(vec![
                Inline::Text("A & ".into()),
                Inline::Strong(vec![Inline::Text("B".into())]),
                Inline::HardBreak,
                Inline::Text("c".into()),
            ])
        );
        assert!(matches!(&blocks[1], Block::Paragraph(i) if i.len() == 2));
    }

    #[test]
    fn empty_paragraphs_and_unknown_tags() {
        assert!(is_empty_paragraph("<p></p>"));
        assert!(is_empty_paragraph("<p><br></p>\n"));
        assert!(!is_empty_paragraph("<p>x</p>"));
        let (blocks, unmapped) = read("<details><summary>s</summary>body</details>");
        assert_eq!(unmapped.tags, vec!["details", "summary"]);
        assert!(!blocks.is_empty());
        // A trailing line break draws nothing.
        let (blocks, _) = read("<p>a<br></p><p><br></p>");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![Inline::Text("a".into())]),
                Block::Paragraph(Vec::new())
            ]
        );
    }

    #[test]
    fn colours_fonts_and_highlights() {
        assert_eq!(css_color("#dc2626"), Some(Some("DC2626".into())));
        assert_eq!(css_color("#abc"), Some(Some("AABBCC".into())));
        assert_eq!(css_color("rgb(220, 38, 38)"), Some(Some("DC2626".into())));
        assert_eq!(css_color("rgba(0 0 0 / 0)"), Some(None));
        assert_eq!(css_color("inherit"), Some(None));
        assert_eq!(css_color("Red"), Some(Some("FF0000".into())));
        assert_eq!(css_color("var(--x)"), None);
        assert_eq!(
            first_family("\"Courier New\", monospace").as_deref(),
            Some("Courier New")
        );
        assert_eq!(first_family("monospace").as_deref(), Some("Courier New"));
        let (blocks, unmapped) = read(
            "<p><span style=\"color: #dc2626;\">red</span><mark data-color=\"#fef08a\" \
             style=\"background-color: #fef08a; color: inherit;\">hi</mark><mark>plain</mark>\
             <span style=\"font-size: 16px; letter-spacing: 2px\">big</span></p>",
        );
        assert_eq!(unmapped.styles, vec!["letter-spacing on <span>"]);
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert!(
            matches!(&inlines[0], Inline::Styled(style, _) if style.color.as_deref() == Some("DC2626"))
        );
        assert!(
            matches!(&inlines[1], Inline::Styled(style, _) if style.highlight.as_deref() == Some("FEF08A") && style.color.is_none())
        );
        assert!(matches!(&inlines[2], Inline::Highlight(_)));
        assert!(matches!(&inlines[3], Inline::Styled(style, _) if style.size == Some(24)));
    }

    #[test]
    fn alignment_images_and_page_breaks() {
        let (blocks, unmapped) = read(
            "<h2 style=\"text-align: center;\">T</h2><p style=\"text-align: justify;\">J</p>\
             <div data-image-align=\"right\" style=\"text-align: right;\"><img src=\"a.png\" alt=\"A\" width=\"200\" height=\"80\"></div>\
             <div data-page-break=\"\" class=\"rich-text-page-break\"></div><p style=\"page-break-before: always\">P</p>",
        );
        assert!(
            unmapped.styles.is_empty() && unmapped.tags.is_empty(),
            "{unmapped:?}"
        );
        assert!(
            matches!(&blocks[0], Block::Aligned(Align::Center, b) if matches!(**b, Block::Heading { level: 2, .. }))
        );
        assert!(matches!(&blocks[1], Block::Aligned(Align::Justify, _)));
        let Block::Aligned(Align::Right, image) = &blocks[2] else {
            panic!("{blocks:?}")
        };
        assert!(matches!(&**image, Block::Paragraph(i)
            if matches!(&i[..], [Inline::Image { width: Some(w), height: Some(h), .. }] if *w == 200.0 && *h == 80.0)));
        assert_eq!(blocks[3], Block::PageBreak);
        assert_eq!(blocks[4], Block::PageBreak);
        assert!(matches!(&blocks[5], Block::Paragraph(_)));
        assert!(is_page_break_element("<div data-page-break=\"\"></div>"));
        assert!(!is_page_break_element(
            "<p>x</p><div data-page-break></div>"
        ));
    }

    #[test]
    fn spans_and_editor_column_widths() {
        let (blocks, _) = read(
            "<table style=\"width: 520px;\"><colgroup><col style=\"width: 240px;\"><col style=\"min-width: 25px;\"><col style=\"width: 140px;\"></colgroup>\
             <tbody><tr><th colwidth=\"240\">A</th><th colspan=\"2\" colwidth=\"0,140\">B</th></tr>\
             <tr><td rowspan=\"2\" style=\"vertical-align: middle\">C</td><td>D</td><td>E</td></tr><tr><td>F</td><td>G</td></tr></tbody></table>",
        );
        let [Block::Table { layout, .. }] = blocks.as_slice() else {
            panic!("{blocks:?}")
        };
        let layout = layout.as_ref().unwrap();
        assert_eq!(layout.widths, vec![Some(240.0), None, Some(140.0)]);
        let (percent, _) = read(
            "<table><colgroup><col style=\"width:70%\"><col width=\"30%\"></colgroup><tr><td>a</td><td>b</td></tr></table>",
        );
        let [
            Block::Table {
                layout: Some(percent),
                ..
            },
        ] = percent.as_slice()
        else {
            panic!("{percent:?}")
        };
        assert_eq!(percent.widths, vec![Some(70.0), Some(30.0)]);
        assert_eq!(layout.table_width, Some(520.0));
        let (grid, columns) = place(&layout.spans);
        assert_eq!(columns, 3);
        assert!(
            matches!(grid[1][0], Slot::Cell { merges_down: true, span, .. } if span.valign == Some(VAlign::Center))
        );
        assert!(matches!(grid[2][0], Slot::Continue { col: 0, .. }));
        assert!(matches!(
            grid[2][1],
            Slot::Cell {
                index: 0,
                col: 1,
                ..
            }
        ));
        // A ragged row is padded to the grid.
        let (grid, columns) = place(&[vec![CellSpan::default(); 3], vec![CellSpan::default()]]);
        assert_eq!(columns, 3);
        assert!(matches!(grid[1][2], Slot::Empty { col: 2 }));
    }
}
