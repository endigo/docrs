//! The parsed Markdown tree written as WordprocessingML inside one
//! transaction.
//!
//! Blocks are built as detached elements scoped to their final parent and
//! attached at the end, so the body is only touched once everything built.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use super::ast::{
    AlertKind, Align, Block, Inline, ListItem, Parsed, RunStyle, Slot, TableLayout, VAlign,
    inline_text, place,
};
use super::frontmatter::Metadata;
use super::styles::{Resolver, Role};
use super::{MarkdownOptions, MarkdownReport, MarkdownWarning, Placement};
use crate::authoring::{
    A, R, WP, add_picture_named, append_run_content, block_width, body, clear_except, new_tbl,
    paragraph_text, set_table_style_id,
};
use crate::coreprops::{CoreProperty, ensure_part as ensure_core_part, write as write_core};
use crate::fields::{fld_char_run, set_update_fields_on_open};
use crate::image::Image;
use crate::numbering::{
    add_next, add_previous, get_or_create_numbering, import_fragment, next_free_ids,
};
use crate::oxml::{add_t, append, insert_before_successors, set_w, w_el};
use crate::package::rels::rt;
use crate::txn::{RelTarget, Txn};
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XML_NS, XmlTree};
use crate::{Error, RefusalKind, Result};

/// Width used when the document has no section properties (6in in EMU).
const FALLBACK_BLOCK_WIDTH: i64 = 5_486_400;
const EMU_PER_TWIP: i64 = 635;
const EMU_PER_PX: f64 = 9525.0;
const TWIPS_PER_PX: f64 = EMU_PER_PX / EMU_PER_TWIP as f64;
/// The editor's narrowest column (Tiptap's `cellMinWidth`).
const MIN_COLUMN_PX: f64 = 25.0;
const VML_NS: &str = "urn:schemas-microsoft-com:vml";
const OFFICE_NS: &str = "urn:schemas-microsoft-com:office:office";
/// Left indent per list level, in twips (with a 360 hanging indent).
const LIST_INDENT: i64 = 720;
const BULLET_NAME: &str = "PaperMarkdownBullet";
const DECIMAL_NAME: &str = "PaperMarkdownDecimal";

/// Run formatting accumulated from enclosing inline spans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Fmt {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    superscript: bool,
    subscript: bool,
    highlight: bool,
    code: bool,
    link: bool,
    /// Editor colours and font (the innermost setting wins).
    style: RunStyle,
}

/// Flattened inline content of one paragraph.
#[derive(Debug, Clone)]
enum Piece {
    Text(Fmt, String),
    /// LaTeX source of inline math.
    Math(String),
    Break(Fmt),
    Image {
        url: String,
        title: String,
        alt: String,
        line: usize,
        width: Option<f64>,
        height: Option<f64>,
    },
    Note {
        label: String,
        line: usize,
    },
    Link {
        url: String,
        title: String,
        pieces: Vec<Piece>,
    },
}

fn push_text(out: &mut Vec<Piece>, fmt: &Fmt, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Piece::Text(last_fmt, last)) = out.last_mut()
        && last_fmt == fmt
    {
        last.push_str(text);
        return;
    }
    out.push(Piece::Text(fmt.clone(), text.to_string()));
}

fn flatten(inlines: &[Inline], fmt: &Fmt, out: &mut Vec<Piece>) {
    let with = |change: fn(&mut Fmt)| {
        let mut changed = fmt.clone();
        change(&mut changed);
        changed
    };
    for inline in inlines {
        match inline {
            Inline::Text(text) => push_text(out, fmt, text),
            Inline::Code(text) => push_text(out, &with(|f| f.code = true), text),
            Inline::Math(text) => out.push(Piece::Math(text.clone())),
            Inline::Emphasis(children) => flatten(children, &with(|f| f.italic = true), out),
            Inline::Strong(children) => flatten(children, &with(|f| f.bold = true), out),
            Inline::Strike(children) => flatten(children, &with(|f| f.strike = true), out),
            Inline::Superscript(children) => {
                flatten(children, &with(|f| f.superscript = true), out)
            }
            Inline::Subscript(children) => flatten(children, &with(|f| f.subscript = true), out),
            Inline::Underline(children) => flatten(children, &with(|f| f.underline = true), out),
            Inline::Highlight(children) => flatten(children, &with(|f| f.highlight = true), out),
            Inline::Styled(style, children) => {
                let mut inner = fmt.clone();
                let own = &mut inner.style;
                own.color = style.color.clone().or(own.color.take());
                own.highlight = style.highlight.clone().or(own.highlight.take());
                own.font = style.font.clone().or(own.font.take());
                own.size = style.size.or(own.size);
                flatten(children, &inner, out);
            }
            Inline::Link {
                url,
                title,
                content,
            } => {
                let mut pieces = Vec::new();
                flatten(content, &with(|f| f.link = true), &mut pieces);
                out.push(Piece::Link {
                    url: url.clone(),
                    title: title.clone(),
                    pieces,
                });
            }
            Inline::Image {
                url,
                title,
                alt,
                line,
                width,
                height,
            } => out.push(Piece::Image {
                url: url.clone(),
                title: title.clone(),
                alt: inline_text(alt),
                line: *line,
                width: *width,
                height: *height,
            }),
            Inline::FootnoteRef { label, line } => out.push(Piece::Note {
                label: label.clone(),
                line: *line,
            }),
            Inline::SoftBreak => push_text(out, fmt, " "),
            Inline::HardBreak => out.push(Piece::Break(fmt.clone())),
        }
    }
}

/// Word's sixteen highlight colours (`w:highlight`); any other colour is
/// written as run shading.
const HIGHLIGHTS: &[(&str, &str)] = &[
    ("FFFF00", "yellow"),
    ("00FF00", "green"),
    ("00FFFF", "cyan"),
    ("FF00FF", "magenta"),
    ("0000FF", "blue"),
    ("FF0000", "red"),
    ("000080", "darkBlue"),
    ("008080", "darkCyan"),
    ("008000", "darkGreen"),
    ("800080", "darkMagenta"),
    ("800000", "darkRed"),
    ("808000", "darkYellow"),
    ("808080", "darkGray"),
    ("C0C0C0", "lightGray"),
    ("000000", "black"),
    ("FFFFFF", "white"),
];

fn jc(align: Align) -> Option<&'static str> {
    match align {
        Align::None => None,
        Align::Left => Some("left"),
        Align::Center => Some("center"),
        Align::Right => Some("right"),
        Align::Justify => Some("both"),
    }
}

/// The block inside an alignment wrapper, and the alignment.
fn unaligned(block: &Block) -> (&Block, Option<Align>) {
    match block {
        Block::Aligned(align, inner) => (unaligned(inner).0, Some(*align)),
        other => (other, None),
    }
}

/// Paragraph properties written in `CT_PPr` order.
#[derive(Debug, Clone, Default)]
struct ParaProps {
    style: Option<String>,
    keep_next: bool,
    numbering: Option<(i64, i64)>,
    indent_left: Option<i64>,
    /// Hanging indent with `indent_left` (list items).
    hanging: Option<i64>,
    align: Option<&'static str>,
}

/// Run properties written in `CT_RPr` order.
#[derive(Debug, Clone, Default)]
struct RunProps {
    style: Option<String>,
    font: Option<String>,
    bold: bool,
    italic: bool,
    strike: bool,
    color: Option<String>,
    /// Font size in half-points.
    size: Option<u32>,
    /// A `w:highlight` colour name.
    highlight: Option<&'static str>,
    underline: bool,
    /// Run shading fill (`RRGGBB`): a highlight in a colour Word's
    /// highlighter lacks.
    shading: Option<String>,
    vert_align: Option<&'static str>,
}

/// Where paragraphs of a block sequence go and how they look.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    /// Role for ordinary paragraphs (quote, footnote, definition).
    paragraph_role: Option<Role>,
    /// Left indent for paragraphs continuing a list item (twips).
    indent: Option<i64>,
    /// Block-quote nesting.
    quote_depth: u8,
    /// Inside a footnote (no nested notes, no heading bookmarks).
    in_note: bool,
    /// Top-level body sequence (first-paragraph styling applies).
    top_level: bool,
    /// Widest picture that fits (EMU).
    max_image_width: i64,
    /// Alignment the editor gave the enclosing block.
    align: Option<Align>,
}

/// Placement anchor in the main body.
enum Anchor {
    /// Before the body's final `w:sectPr` (or at the end).
    End,
    /// In place of this paragraph.
    Replace(NodeId),
}

struct Lists {
    part: String,
    bullet_abstract: i64,
    decimal_abstract: i64,
    bullet_num: Option<i64>,
    /// The reference document's `List Bullet` / `List Number` numbering,
    /// when those styles carry one.
    template_bullet: Option<TemplateList>,
    template_number: Option<TemplateList>,
}

/// A list look the reference document defines through paragraph styles
/// (`List Bullet`, `List Bullet 2`, …) bound to numbering instances.
#[derive(Debug, Clone)]
struct TemplateList {
    /// UI name of the base style (`List Bullet`).
    name: String,
    /// Where each nesting level comes from.
    levels: BTreeMap<i64, LevelSource>,
}

/// Levels a numbering instance defines: `ilvl → (left indent, hanging)`.
type LevelIndents = BTreeMap<i64, (i64, i64)>;

#[derive(Debug, Clone)]
struct LevelSource {
    num_id: i64,
    abstract_id: i64,
    ilvl: i64,
    style_id: Option<String>,
    /// Left indent of the level's text (twips).
    indent: i64,
    /// Hanging indent of the level's marker (twips).
    hanging: i64,
    /// Paragraph indent written on the item when the template defines no
    /// such level (`(left, hanging)`, extrapolated from its deepest level).
    extrapolated: Option<(i64, i64)>,
}

pub(crate) struct Renderer<'t, 'd, 'p> {
    txn: &'t mut Txn<'d>,
    parsed: &'p Parsed,
    options: &'p MarkdownOptions,
    report: &'t mut MarkdownReport,
    styles: Resolver,
    main: String,
    lists: Option<Lists>,
    /// Heading bookmark names in document order.
    heading_bookmarks: VecDeque<String>,
    /// Link target (`slug` or explicit id) → bookmark name.
    anchors: BTreeMap<String, String>,
    next_bookmark_id: i64,
    /// `(level, text, bookmark)` of every body heading, for the contents.
    toc_entries: Vec<(u8, String, String)>,
    /// `wp:docPr` elements created, renumbered once attached.
    pictures: Vec<(String, NodeId)>,
    /// A manual line break was written into a paragraph.
    line_breaks: bool,
    /// The text width (EMU).
    block_width: i64,
    /// Each contents entry's page number text (`w:t`) and the bookmark of
    /// its heading.
    toc_numbers: Vec<(NodeId, String)>,
}

/// Render `parsed` into the transaction's main document.
pub(crate) fn render(
    txn: &mut Txn<'_>,
    parsed: &Parsed,
    metadata: &Metadata,
    options: &MarkdownOptions,
    prefer_own_styles: bool,
    report: &mut MarkdownReport,
) -> Result<()> {
    let main = txn.document().package().main_part_name().to_string();
    let (body_node, anchor, width, blank_first_page) = {
        let tree = txn.tree_mut(&main)?;
        let body_node = body(tree)?;
        let blank_first_page = blank_first_page(tree, body_node);
        let anchor = match &options.placement {
            Placement::ReplaceBody => {
                let inherited = inherited_references(tree)?;
                clear_except(tree, body_node, "sectPr")?;
                adopt_references(tree, body_node, &inherited)?;
                Anchor::End
            }
            Placement::Append => Anchor::End,
            Placement::Placeholder(marker) => {
                Anchor::Replace(find_placeholder(tree, body_node, marker)?)
            }
        };
        let width = block_width(tree).unwrap_or(FALLBACK_BLOCK_WIDTH);
        (body_node, anchor, width, blank_first_page)
    };
    let mut renderer = Renderer {
        txn,
        parsed,
        options,
        report,
        styles: Resolver::new(options.style_map.clone(), options.style_policy),
        main: main.clone(),
        lists: None,
        heading_bookmarks: VecDeque::new(),
        anchors: BTreeMap::new(),
        next_bookmark_id: 0,
        toc_entries: Vec::new(),
        pictures: Vec::new(),
        line_breaks: false,
        block_width: width,
        toc_numbers: Vec::new(),
    };
    if matches!(options.placement, Placement::ReplaceBody) {
        drop_replaced_body_leftovers(renderer.txn, &main)?;
    }
    renderer.styles.prefer_own = prefer_own_styles;
    renderer.plan_bookmarks()?;
    let ctx = Ctx {
        paragraph_role: None,
        indent: None,
        quote_depth: 0,
        in_note: false,
        top_level: true,
        max_image_width: width,
        align: None,
    };
    let mut nodes = Vec::new();
    if matches!(options.placement, Placement::ReplaceBody) {
        nodes.extend(renderer.cover_page(&main, body_node, blank_first_page)?);
    }
    if options.title_block {
        nodes.extend(renderer.title_block(body_node, metadata)?);
    }
    let toc = options.toc.or(metadata.toc).unwrap_or(false);
    if toc {
        let depth = options
            .toc_depth
            .or(metadata.toc_depth)
            .unwrap_or(3)
            .clamp(1, 9);
        let title = options
            .toc_title
            .clone()
            .or_else(|| metadata.toc_title.clone())
            .unwrap_or_else(|| "Contents".to_string());
        nodes.extend(renderer.toc(body_node, depth, &title)?);
    }
    let body_start = nodes.len();
    nodes.extend(renderer.blocks(&main, body_node, &parsed.blocks, ctx)?);
    if toc && nodes.len() > body_start {
        renderer.break_after_toc(&main, body_node, &mut nodes, body_start)?;
    }
    if nodes.is_empty() && matches!(options.placement, Placement::ReplaceBody) {
        let tree = renderer.txn.tree_mut(&main)?;
        nodes.push(w_el(tree, body_node, "p")?);
    }
    {
        let tree = renderer.txn.tree_mut(&main)?;
        match anchor {
            Anchor::End => {
                for node in nodes {
                    insert_before_successors(tree, body_node, node, &["sectPr"])?;
                }
            }
            Anchor::Replace(placeholder) => {
                for node in nodes {
                    add_previous(tree, placeholder, node)?;
                }
                crate::authoring::remove(tree, placeholder)?;
            }
        }
    }
    renderer.number_pictures()?;
    if prefer_own_styles {
        // A starter reference document carries every role's style, used
        // or not, so each can be restyled in Word.
        for role in Role::ALL {
            renderer.style(role)?;
        }
    }
    if matches!(options.placement, Placement::ReplaceBody) {
        let keep_update = toc && options.toc_page_numbers;
        let line_breaks = renderer.line_breaks;
        tidy_settings(renderer.txn, keep_update, line_breaks)?;
    }
    if options.page_numbers {
        renderer.page_number_footer(&main, body_node)?;
    }
    let set = if options.set_properties {
        renderer.core_properties(metadata)?
    } else {
        Vec::new()
    };
    if matches!(options.placement, Placement::ReplaceBody) {
        renderer.clear_template_properties(&main, &set)?;
        drop_unreferenced_parts(renderer.txn, &main)?;
    }
    if !renderer.toc_numbers.is_empty() {
        renderer.toc_page_numbers(&main)?;
    }
    for (line, html) in &parsed.unknown_html {
        renderer.warn(
            "raw_html_dropped",
            format!("HTML tag {html} has no Word equivalent; its text is kept"),
            Some(*line),
        );
    }
    for (line, style) in &parsed.dropped_styles {
        renderer.warn(
            "html_style_dropped",
            format!("CSS {style} has no Word equivalent and was ignored"),
            Some(*line),
        );
    }
    if !renderer.styles.missing.is_empty() {
        return Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "reference_style_missing",
            format!(
                "the reference document lacks styles for: {}. Add them to the reference document, map the roles to existing styles, or let the converter create them; nothing was changed",
                renderer.styles.missing.join("; ")
            ),
        ));
    }
    let resolved = std::mem::take(&mut renderer.styles.resolved);
    for (role, style) in resolved {
        if style.created {
            renderer.report.created_styles.push(style.name.clone());
        }
        renderer.report.styles.insert(role.key().to_string(), style);
    }
    Ok(())
}

/// Relationship types (last path segment) whose targets only body content
/// refers to, by `r:id`/`r:embed`/`r:link`.
const CONTENT_RELATIONSHIPS: &[&str] = &[
    "image",
    "hdphoto",
    "oleObject",
    "package",
    "chart",
    "chartUserShapes",
    "diagramData",
    "diagramLayout",
    "diagramQuickStyle",
    "diagramColors",
    "diagramDrawing",
    "video",
    "audio",
    "media",
    "control",
    "aFChunk",
    "subDocument",
    "hyperlink",
];

/// Comment machinery, dropped when no comment is referenced any more.
const COMMENT_RELATIONSHIPS: &[&str] = &[
    "comments",
    "commentsExtended",
    "commentsIds",
    "commentsExtensible",
    "people",
];

/// After replacing the body, nothing of the template's sample content may
/// remain in the package: for a template made from a real client document,
/// its pictures, embedded files and comments are that client's data. Drop
/// the main part's content relationships the new body does not reference,
/// the comment parts when no comment is referenced, custom XML when no
/// content control is bound to it, the package thumbnail (a rendering of
/// the template's first page), and then every part no relationship chain
/// from the package reaches. Headers, footers, styles, numbering,
/// theme, settings and everything they reference stay.
fn drop_unreferenced_parts(txn: &mut Txn<'_>, main: &str) -> Result<()> {
    let (referenced, comments_used) = {
        let tree = txn.tree(main)?;
        let referenced: HashSet<String> = crate::revision::relationship_values(tree, tree.root())
            .into_iter()
            .collect();
        let comments_used = tree.descendants(tree.root()).any(|node| {
            tree.is(node, W, "commentReference") || tree.is(node, W, "commentRangeStart")
        });
        (referenced, comments_used)
    };
    let rels = txn.relationships(main)?;
    // Content controls in the kept stories bound to custom XML keep it.
    let mut bound = false;
    let main_uri = format!("/{}", main.trim_start_matches('/'));
    for rel in rels.iter().filter(|rel| !rel.external) {
        let kind = rel.rel_type.rsplit('/').next().unwrap_or("");
        if matches!(kind, "header" | "footer" | "footnotes" | "endnotes")
            && let Some(part) =
                txn.resolve(&rel.target_part_name(crate::package::rels::base_uri(&main_uri)))
        {
            let tree = txn.tree(&part)?;
            bound |= tree
                .descendants(tree.root())
                .any(|node| tree.is(node, W, "dataBinding"));
        }
    }
    {
        let tree = txn.tree(main)?;
        bound |= tree
            .descendants(tree.root())
            .any(|node| tree.is(node, W, "dataBinding"));
    }
    let doomed: Vec<String> = rels
        .iter()
        .filter(|rel| !referenced.contains(&rel.id))
        .filter(|rel| {
            let kind = rel.rel_type.rsplit('/').next().unwrap_or("");
            CONTENT_RELATIONSHIPS.contains(&kind)
                || (!comments_used && COMMENT_RELATIONSHIPS.contains(&kind))
                || (!bound && kind == "customXml")
        })
        .map(|rel| rel.id.clone())
        .collect();
    for r_id in doomed {
        crate::revision::drop_rel(txn, main, &r_id)?;
    }
    // The package thumbnail is a picture of the template's first page.
    let thumbnails: Vec<String> = txn
        .relationships("/")?
        .into_iter()
        .filter(|rel| rel.rel_type.ends_with("/metadata/thumbnail"))
        .map(|rel| rel.id)
        .collect();
    for r_id in thumbnails {
        crate::revision::drop_rel(txn, "/", &r_id)?;
    }
    // Every part a relationship chain from the package reaches stays.
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::from(["/".to_string()]);
    while let Some(source) = queue.pop_front() {
        let source_uri = if source == "/" {
            "/".to_string()
        } else {
            format!("/{source}")
        };
        for rel in txn.relationships(&source)? {
            if rel.external {
                continue;
            }
            let target = rel.target_part_name(crate::package::rels::base_uri(&source_uri));
            if let Some(member) = txn.resolve(&target)
                && reachable.insert(member.clone())
            {
                queue.push_back(member);
            }
        }
    }
    let unreachable: Vec<String> = txn
        .document()
        .package()
        .parts()
        .map(|part| part.name().to_string())
        .filter(|name| {
            name != "[Content_Types].xml" && !name.ends_with(".rels") && !reachable.contains(name)
        })
        .collect();
    for name in unreachable {
        txn.remove_part(&name)?;
    }
    Ok(())
}

const EXTENDED_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
const CUSTOM_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";

/// Field instructions (simple and complex fields) in the main part and the
/// headers, footers and notes it relates.
fn kept_field_instructions(txn: &Txn<'_>, main: &str) -> Result<Vec<String>> {
    let main_uri = format!("/{}", main.trim_start_matches('/'));
    let mut parts = vec![main.to_string()];
    for rel in txn.relationships(main)?.iter().filter(|rel| !rel.external) {
        let kind = rel.rel_type.rsplit('/').next().unwrap_or("");
        if matches!(kind, "header" | "footer" | "footnotes" | "endnotes")
            && let Some(part) =
                txn.resolve(&rel.target_part_name(crate::package::rels::base_uri(&main_uri)))
        {
            parts.push(part);
        }
    }
    let mut out = Vec::new();
    for part in parts {
        let tree = txn.tree(&part)?;
        // Complex fields nest; each collects the instruction text between
        // its begin and end.
        let mut open: Vec<String> = Vec::new();
        for node in tree.descendants(tree.root()) {
            if tree.is(node, W, "fldSimple") {
                if let Some(instr) = w_attr(tree, node, "instr") {
                    out.push(instr.to_string());
                }
            } else if tree.is(node, W, "fldChar") {
                match w_attr(tree, node, "fldCharType") {
                    Some("begin") => open.push(String::new()),
                    Some("end") => out.extend(open.pop()),
                    _ => {}
                }
            } else if tree.is(node, W, "instrText")
                && let Some(current) = open.last_mut()
            {
                current.push_str(&crate::wml::leading_text(tree, node));
            }
        }
        out.extend(open);
    }
    Ok(out)
}

/// A field instruction's words, quotes removed.
fn field_words(instruction: &str) -> impl Iterator<Item = String> + '_ {
    let mut chars = instruction.chars().peekable();
    std::iter::from_fn(move || {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let first = chars.next()?;
        let mut word = String::new();
        if first == '"' {
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                word.push(c);
            }
        } else {
            word.push(first);
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        Some(word)
    })
}

/// Clean up after replacing the body: hyperlink relationships and
/// footnotes/endnotes only the old body used go. Image relationships stay:
/// the package keeps every member, and an image part left unreachable
/// would block the next image part name.
fn drop_replaced_body_leftovers(txn: &mut Txn<'_>, main: &str) -> Result<()> {
    let (referenced, notes) = {
        let tree = txn.tree(main)?;
        let referenced: HashSet<String> = crate::revision::relationship_values(tree, tree.root())
            .into_iter()
            .collect();
        let mut notes: HashSet<(&'static str, String)> = HashSet::new();
        for node in tree.descendants(tree.root()) {
            for (reference, kind) in [
                ("footnoteReference", "footnote"),
                ("endnoteReference", "endnote"),
            ] {
                if tree.is(node, W, reference)
                    && let Some(id) = w_attr(tree, node, "id")
                {
                    notes.insert((kind, id.to_string()));
                }
            }
        }
        (referenced, notes)
    };
    let orphaned: Vec<String> = txn
        .relationships(main)?
        .into_iter()
        .filter(|rel| rel.rel_type == rt::HYPERLINK && !referenced.contains(&rel.id))
        .map(|rel| rel.id)
        .collect();
    for r_id in orphaned {
        crate::revision::drop_rel(txn, main, &r_id)?;
    }
    for (rel_type, kind) in [(rt::FOOTNOTES, "footnote"), (rt::ENDNOTES, "endnote")] {
        let Some(part) = txn.part_related_by(main, rel_type)? else {
            continue;
        };
        let doomed: Vec<NodeId> = {
            let tree = txn.tree(&part)?;
            w_children(tree, tree.root(), kind)
                .filter(|note| w_attr(tree, *note, "type").is_none_or(|t| t == "normal"))
                .filter(|note| {
                    w_attr(tree, *note, "id")
                        .is_some_and(|id| !notes.contains(&(kind, id.to_string())))
                })
                .collect()
        };
        if doomed.is_empty() {
            continue;
        }
        let tree = txn.tree_mut(&part)?;
        for note in doomed {
            crate::authoring::remove(tree, note)?;
        }
    }
    Ok(())
}

/// `CT_Compat` child order.
const COMPAT_SEQ: &[&str] = &[
    "useSingleBorderforContiguousCells",
    "wpJustification",
    "noTabHangInd",
    "noLeading",
    "spaceForUL",
    "noColumnBalance",
    "balanceSingleByteDoubleByteWidth",
    "noExtraLineSpacing",
    "doNotLeaveBackslashAlone",
    "ulTrailSpace",
    "doNotExpandShiftReturn",
    "spacingInWholePoints",
    "lineWrapLikeWord6",
    "printBodyTextBeforeHeader",
    "printColBlack",
    "wpSpaceWidth",
    "showBreaksInFrames",
    "subFontBySize",
    "suppressBottomSpacing",
    "suppressTopSpacing",
    "suppressSpacingAtTopOfPage",
    "suppressTopSpacingWP",
    "suppressSpBfAfterPgBrk",
    "swapBordersFacingPages",
    "convMailMergeEsc",
    "truncateFontHeightsLikeWP6",
    "mwSmallCaps",
    "usePrinterMetrics",
    "doNotSuppressParagraphBorders",
    "wrapTrailSpaces",
    "footnoteLayoutLikeWW8",
    "shapeLayoutLikeWW8",
    "alignTablesRowByRow",
    "forgetLastTabAlignment",
    "adjustLineHeightInTable",
    "autoSpaceLikeWord95",
    "noSpaceRaiseLower",
    "doNotUseHTMLParagraphAutoSpacing",
    "layoutRawTableWidth",
    "layoutTableRowsApart",
    "useWord97LineBreakRules",
    "doNotBreakWrappedTables",
    "doNotSnapToGridInCell",
    "selectFldWithFirstOrLastChar",
    "applyBreakingRules",
    "doNotWrapTextWithPunct",
    "doNotUseEastAsianBreakRules",
    "useWord2002TableStyleRules",
    "growAutofit",
    "useFELayout",
    "useNormalStyleForList",
    "doNotUseIndentAsNumberingTabStop",
    "useAltKinsokuLineBreakRules",
    "allowSpaceOfSameStyleInTable",
    "doNotSuppressIndentation",
    "doNotAutofitConstrainedTables",
    "autofitToFirstFixedWidthCell",
    "underlineTabInNumList",
    "displayHangulFixedWidth",
    "splitPgBreakAndParaMark",
    "doNotVertAlignCellWithSp",
    "doNotBreakConstrainedForcedTable",
    "doNotVertAlignInTxbx",
    "useAnsiKerningPairs",
    "cachedColBalance",
    "compatSetting",
];

/// Settings for a document whose body was replaced: an inherited
/// "update fields on open" (left by fields in the old body) would make Word
/// ask about fields the new body may not have, and a template without any
/// compatibility settings is laid out by Word in legacy mode, so it gets
/// Word 2013+ mode as Word itself writes it.
fn tidy_settings(txn: &mut Txn<'_>, keep_update_fields: bool, line_breaks: bool) -> Result<()> {
    let main = txn.document().package().main_part_name().to_string();
    let Some(settings) = txn.part_related_by(&main, rt::SETTINGS)? else {
        return Ok(());
    };
    let (update, has_compat) = {
        let tree = txn.tree(&settings)?;
        let root = tree.root();
        (
            w_child(tree, root, "updateFields"),
            w_child(tree, root, "compat").is_some(),
        )
    };
    if let Some(update) = update.filter(|_| !keep_update_fields) {
        let tree = txn.tree_mut(&settings)?;
        crate::authoring::remove(tree, update)?;
    }
    if !has_compat {
        let tree = txn.tree_mut(&settings)?;
        let root = tree.root();
        let fragment = format!(
            "<w:compat xmlns:w=\"{W}\"><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat>"
        );
        let compat = import_fragment(tree, root, &fragment)?;
        // CT_Settings: compat precedes docVars, rsids, mathPr, … .
        const SUCCESSORS: &[&str] = &[
            "docVars",
            "rsids",
            "mathPr",
            "attachedSchema",
            "themeFontLang",
            "clrSchemeMapping",
            "doNotIncludeSubdocsInStats",
            "doNotAutoCompressPictures",
            "forceUpgrade",
            "captions",
            "readModeInkLockDown",
            "smartTagType",
            "schemaLibrary",
            "shapeDefaults",
            "doNotEmbedSmartTags",
            "decimalSymbol",
            "listSeparator",
        ];
        let position = tree.children(root).iter().position(|child| {
            tree.local_name(*child)
                .is_some_and(|local| SUCCESSORS.contains(&local))
        });
        match position {
            Some(index) => tree.insert(root, index, compat)?,
            None => append(tree, root, compat)?,
        }
    }
    // Markdown line breaks become manual breaks; in a justified body style
    // Word would stretch each such line across the page unless told not to.
    let missing = {
        let tree = txn.tree(&settings)?;
        w_child(tree, tree.root(), "compat")
            .is_some_and(|compat| w_child(tree, compat, "doNotExpandShiftReturn").is_none())
    };
    if line_breaks && missing {
        let tree = txn.tree_mut(&settings)?;
        let root = tree.root();
        if let Some(compat) = w_child(tree, root, "compat") {
            crate::oxml::get_or_add(tree, compat, "doNotExpandShiftReturn", COMPAT_SEQ)?;
        }
    }
    Ok(())
}

/// Whether the reference document's own body leaves its first page blank:
/// it opens with empty paragraphs up to a page break, or with at least
/// eight of them (padding that pushes the sample past a cover).
/// `(space before, space after)` in twips a paragraph of style `style_id`
/// (the default paragraph style when `None`) gets from its style chain and
/// the document defaults.
fn effective_spacing(styles: &XmlTree, style_id: Option<&str>) -> (Option<i64>, Option<i64>) {
    let root = styles.root();
    let find = |id: &str| {
        w_children(styles, root, "style").find(|s| w_attr(styles, *s, "styleId") == Some(id))
    };
    let default = || {
        w_children(styles, root, "style").find(|s| {
            w_attr(styles, *s, "type") == Some("paragraph")
                && matches!(w_attr(styles, *s, "default"), Some("1" | "true" | "on"))
        })
    };
    let mut current = match style_id {
        Some(id) => find(id),
        None => default(),
    };
    let (mut before, mut after) = (None, None);
    let read = |node: NodeId, name: &str| {
        w_child(styles, node, "pPr")
            .and_then(|p| w_child(styles, p, "spacing"))
            .and_then(|s| w_attr(styles, s, name))
            .and_then(|v| v.parse::<i64>().ok())
    };
    for _ in 0..16 {
        let Some(style) = current else {
            break;
        };
        before = before.or_else(|| read(style, "before"));
        after = after.or_else(|| read(style, "after"));
        current = w_child(styles, style, "basedOn")
            .and_then(|b| w_attr(styles, b, "val"))
            .and_then(find);
    }
    let defaults = w_child(styles, root, "docDefaults")
        .and_then(|d| w_child(styles, d, "pPrDefault"))
        .and_then(|d| w_child(styles, d, "pPr"))
        .and_then(|p| w_child(styles, p, "spacing"));
    let default_value = |name: &str| {
        defaults
            .and_then(|s| w_attr(styles, s, name))
            .and_then(|v| v.parse::<i64>().ok())
    };
    (
        before.or_else(|| default_value("before")),
        after.or_else(|| default_value("after")),
    )
}

/// Header and footer references the final section inherits from earlier
/// sections (Word reuses the previous section's header of a type the
/// section does not define): `(element, type, relationship id)`.
pub(crate) fn inherited_references(tree: &XmlTree) -> Result<Vec<(&'static str, String, String)>> {
    let sections = crate::authoring::section_properties(tree)?;
    let Some((last, earlier)) = sections.split_last() else {
        return Ok(Vec::new());
    };
    let mut latest: BTreeMap<(&'static str, String), String> = BTreeMap::new();
    for sect_pr in earlier {
        for local in ["headerReference", "footerReference"] {
            for reference in w_children(tree, *sect_pr, local) {
                let kind = w_attr(tree, reference, "type")
                    .unwrap_or("default")
                    .to_string();
                if let Some(r_id) = tree.attr(reference, Some(R), "id") {
                    latest.insert((local, kind), r_id.to_string());
                }
            }
        }
    }
    Ok(latest
        .into_iter()
        .filter(|((local, kind), _)| {
            !w_children(tree, *last, local)
                .any(|r| w_attr(tree, r, "type").unwrap_or("default") == kind)
        })
        .map(|((local, kind), r_id)| (local, kind, r_id))
        .collect())
}

/// Write `references` into the body's final section properties, ahead of
/// its other children.
fn adopt_references(
    tree: &mut XmlTree,
    body_node: NodeId,
    references: &[(&'static str, String, String)],
) -> Result<()> {
    let Some(sect_pr) = w_child(tree, body_node, "sectPr") else {
        return Ok(());
    };
    for (local, kind, r_id) in references {
        let reference = w_el(tree, sect_pr, local)?;
        set_w(tree, reference, "type", kind)?;
        tree.set_attr(reference, Some(R), "id", r_id)?;
        let position = tree
            .children(sect_pr)
            .iter()
            .position(|c| {
                tree.element(*c).is_some()
                    && !(tree.is(*c, W, "headerReference") || tree.is(*c, W, "footerReference"))
            })
            .unwrap_or(tree.children(sect_pr).len());
        tree.insert(sect_pr, position, reference)?;
    }
    Ok(())
}

fn blank_first_page(tree: &XmlTree, body_node: NodeId) -> bool {
    let mut blank = 0;
    for block in tree.child_elements(body_node) {
        if !tree.is(block, W, "p") {
            break;
        }
        let page_break = tree.descendants(block).any(|node| {
            (tree.is(node, W, "br") && w_attr(tree, node, "type") == Some("page"))
                || tree.is(node, W, "pageBreakBefore")
                || tree.is(node, W, "sectPr")
        });
        let empty = paragraph_text(tree, block).trim().is_empty()
            && !tree
                .descendants(block)
                .any(|node| tree.is(node, W, "drawing") || tree.local_name(node) == Some("pict"));
        if page_break {
            return blank > 0 || empty;
        }
        if !empty {
            break;
        }
        blank += 1;
    }
    blank >= 8
}

/// Number format of level `ilvl` of list instance `num_id`.
fn level_format(numbering: &XmlTree, num_id: i64, ilvl: i64) -> Option<String> {
    let root = numbering.root();
    let int = |node: NodeId, name: &str| {
        w_attr(numbering, node, name).and_then(|v| v.parse::<i64>().ok())
    };
    let abstract_id = w_children(numbering, root, "num")
        .find(|n| int(*n, "numId") == Some(num_id))
        .and_then(|n| w_child(numbering, n, "abstractNumId"))
        .and_then(|a| w_attr(numbering, a, "val"))?;
    w_children(numbering, root, "abstractNum")
        .find(|a| w_attr(numbering, *a, "abstractNumId") == Some(abstract_id))
        .and_then(|a| w_children(numbering, a, "lvl").find(|l| int(*l, "ilvl") == Some(ilvl)))
        .and_then(|l| w_child(numbering, l, "numFmt"))
        .and_then(|f| w_attr(numbering, f, "val"))
        .map(str::to_string)
}

fn find_placeholder(tree: &XmlTree, body_node: NodeId, marker: &str) -> Result<NodeId> {
    let matches: Vec<NodeId> = w_children(tree, body_node, "p")
        .filter(|p| paragraph_text(tree, *p).trim() == marker.trim())
        .collect();
    match matches.as_slice() {
        [] => Err(Error::refusal(
            RefusalKind::TargetNotFound,
            "placeholder_not_found",
            format!("no top-level body paragraph reads exactly {marker:?}"),
        )),
        [one] => {
            let carries_section = w_child(tree, *one, "pPr")
                .and_then(|ppr| w_child(tree, ppr, "sectPr"))
                .is_some();
            if carries_section {
                return Err(Error::refusal(
                    RefusalKind::UnsupportedStructure,
                    "placeholder_ends_section",
                    "the placeholder paragraph carries a section break; replacing it would drop the section",
                ));
            }
            Ok(*one)
        }
        many => Err(Error::refusal(
            RefusalKind::AmbiguousTarget,
            "placeholder_ambiguous",
            format!(
                "{} top-level body paragraphs read exactly {marker:?}",
                many.len()
            ),
        )),
    }
}

/// GitHub's heading slug: lower-case, punctuation dropped, spaces → `-`.
pub(crate) fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// A Word-legal bookmark name for `slug` (letter first, word characters,
/// at most 40), unique against `taken` (case-insensitive).
fn bookmark_name(slug: &str, taken: &HashSet<String>) -> String {
    let mut base: String = slug
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if !base.chars().next().is_some_and(|c| c.is_alphabetic()) {
        base.insert(0, 'h');
    }
    let base: String = base.chars().take(40).collect();
    if !taken.contains(&base.to_lowercase()) {
        return base;
    }
    (2..)
        .map(|n| {
            let suffix = format!("_{n}");
            let keep = 40 - suffix.len();
            format!("{}{suffix}", base.chars().take(keep).collect::<String>())
        })
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .expect("an unused bookmark name")
}

fn collect_headings<'a>(blocks: &'a [Block], out: &mut Vec<(Option<&'a str>, String, u8)>) {
    for block in blocks {
        match block {
            Block::Heading { id, content, level } => {
                out.push((id.as_deref(), inline_text(content), *level))
            }
            Block::Quote { blocks, .. } => collect_headings(blocks, out),
            Block::Aligned(_, block) => collect_headings(std::slice::from_ref(block), out),
            Block::List { items, .. } => {
                for item in items {
                    collect_headings(&item.blocks, out);
                }
            }
            // In rendering order: row by row, cell by cell.
            Block::Table { head, rows, .. } => {
                for cell in head.iter().chain(rows.iter().flatten()) {
                    collect_headings(cell, out);
                }
            }
            Block::DefinitionList(entries) => {
                for (_, definitions) in entries {
                    for definition in definitions {
                        collect_headings(definition, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The bytes `text` decodes to (what [`base64_decode`] returns when it is
/// valid), counted without decoding.
fn base64_len(text: &str) -> u64 {
    let digits = text
        .bytes()
        .take_while(|c| *c != b'=')
        .filter(|c| !c.is_ascii_whitespace())
        .count() as u64;
    digits * 6 / 8
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Encode characters a relationship target cannot hold raw.
fn link_target(url: &str) -> String {
    let mut out = String::new();
    for c in url.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '"' => out.push_str("%22"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    if !out.contains(':') && !out.starts_with('/') && !out.starts_with('.') && out.contains('@') {
        out.insert_str(0, "mailto:");
    }
    out
}

/// Whether a hyperlink may point at `target` (as [`link_target`] wrote
/// it): an `http`, `https`, `mailto` or `tel` URL, or a relative reference.
/// A colon before the first `/`, `?` or `#` starts some other scheme
/// (`javascript:`, `file:`, `C:`), and a leading `//` or `\\` names a
/// network host; neither is linked.
fn safe_link_target(target: &str) -> bool {
    if target.starts_with("//") || target.starts_with("\\\\") {
        return false;
    }
    let head = target.split(['/', '?', '#']).next().unwrap_or("");
    match head.split_once(':') {
        None => true,
        Some((scheme, _)) => ["http", "https", "mailto", "tel"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
    }
}

impl<'p> Renderer<'_, '_, 'p> {
    fn warn(&mut self, code: &'static str, message: String, line: Option<usize>) {
        self.report.warnings.push(MarkdownWarning {
            code,
            message,
            line,
        });
    }

    fn style(&mut self, role: Role) -> Result<Option<String>> {
        self.styles.id(self.txn, role)
    }

    /// Bookmark names for every heading, planned before rendering so links
    /// can point forward.
    fn plan_bookmarks(&mut self) -> Result<()> {
        let mut taken: HashSet<String> = HashSet::new();
        let mut highest = 0i64;
        let mut parts = vec![self.main.clone()];
        if let Some(notes) = self.txn.part_related_by(&self.main, rt::FOOTNOTES)? {
            parts.push(notes);
        }
        for part in parts {
            let tree = self.txn.tree(&part)?;
            for node in tree.descendants(tree.root()) {
                let start = tree.is(node, W, "bookmarkStart");
                if (start || tree.is(node, W, "bookmarkEnd"))
                    && let Some(id) = w_attr(tree, node, "id").and_then(|v| v.parse::<i64>().ok())
                {
                    highest = highest.max(id);
                }
                if start && let Some(name) = w_attr(tree, node, "name") {
                    taken.insert(name.to_lowercase());
                }
            }
        }
        self.next_bookmark_id = highest + 1;
        let mut headings = Vec::new();
        collect_headings(&self.parsed.blocks, &mut headings);
        let mut slugs: HashSet<String> = HashSet::new();
        for (explicit, text, level) in headings {
            let key = match explicit {
                Some(id) => id.to_string(),
                None => {
                    let base = slug(&text);
                    let mut candidate = base.clone();
                    let mut n = 1;
                    while slugs.contains(&candidate) {
                        candidate = format!("{base}-{n}");
                        n += 1;
                    }
                    candidate
                }
            };
            slugs.insert(key.clone());
            let name = bookmark_name(&key, &taken);
            taken.insert(name.to_lowercase());
            self.anchors.entry(key).or_insert_with(|| name.clone());
            let level =
                (i16::from(level) + i16::from(self.options.heading_offset)).clamp(1, 9) as u8;
            self.toc_entries.push((level, text.clone(), name.clone()));
            self.heading_bookmarks.push_back(name);
        }
        Ok(())
    }

    // ------------------------------------------------------------ blocks

    fn blocks(
        &mut self,
        part: &str,
        scope: NodeId,
        blocks: &[Block],
        ctx: Ctx,
    ) -> Result<Vec<NodeId>> {
        let mut out: Vec<NodeId> = Vec::new();
        let mut after_paragraph = false;
        let mut pending_caption: Option<Vec<Inline>> = None;
        let mut last_table: Option<usize> = None;
        // Indices in `out` of the last node of each list.
        let mut list_ends: Vec<usize> = Vec::new();
        let mut index = 0;
        while index < blocks.len() {
            let (block, own_align) = unaligned(&blocks[index]);
            index += 1;
            let ctx = Ctx {
                align: own_align.or(ctx.align),
                ..ctx
            };
            let was_paragraph = after_paragraph;
            after_paragraph = false;
            match block {
                Block::Paragraph(inlines) | Block::Plain(inlines) => {
                    if let Some(caption) = table_caption(inlines) {
                        if matches!(blocks.get(index), Some(Block::Table { .. })) {
                            pending_caption = Some(caption);
                            continue;
                        }
                        if let Some(table_at) = last_table.take() {
                            let paragraph =
                                self.caption(part, scope, Role::TableCaption, &caption)?;
                            out.insert(table_at, paragraph);
                            continue;
                        }
                    }
                    // Editor HTML has no figures: its pictures sit in
                    // ordinary paragraphs.
                    if let Some(image) = lone_image(inlines).filter(|_| !self.parsed.html) {
                        out.extend(self.figure(part, scope, image, ctx)?);
                        continue;
                    }
                    let role = match (ctx.paragraph_role, block) {
                        (Some(role), _) => role,
                        (None, Block::Plain(_)) => Role::Compact,
                        (None, _) if ctx.top_level && !was_paragraph => Role::FirstParagraph,
                        (None, _) => Role::Body,
                    };
                    let props = ParaProps {
                        style: self.style(role)?,
                        indent_left: ctx.indent.or(quote_indent(ctx.quote_depth)),
                        align: ctx.align.and_then(jc),
                        ..ParaProps::default()
                    };
                    out.push(self.paragraph(part, scope, &props, inlines, ctx)?);
                    after_paragraph = true;
                }
                Block::Heading { level, content, .. } => {
                    let level = (i16::from(*level) + i16::from(self.options.heading_offset))
                        .clamp(1, 9) as u8;
                    out.push(self.heading(part, scope, level, content, ctx)?);
                }
                Block::Code { text, .. } => {
                    out.push(self.code_block(part, scope, text, ctx)?);
                }
                Block::Quote {
                    alert,
                    blocks: inner,
                } => {
                    let inner_ctx = Ctx {
                        paragraph_role: Some(Role::BlockQuote),
                        quote_depth: ctx.quote_depth.saturating_add(1),
                        top_level: false,
                        ..ctx
                    };
                    if let Some(alert) = alert {
                        out.push(self.alert_label(part, scope, *alert, inner_ctx)?);
                    }
                    out.extend(self.blocks(part, scope, inner, inner_ctx)?);
                }
                Block::List {
                    start,
                    tight,
                    items,
                } => {
                    out.extend(self.list(part, scope, *start, *tight, items, 0, ctx)?);
                    if let Some(end) = out.len().checked_sub(1) {
                        list_ends.push(end);
                    }
                }
                Block::Table {
                    aligns,
                    head,
                    rows,
                    layout,
                } => {
                    if let Some(caption) = pending_caption.take() {
                        out.push(self.caption(part, scope, Role::TableCaption, &caption)?);
                    }
                    // Word joins tables that touch; keep them apart.
                    let touches_table = {
                        let tree = self.txn.tree(part)?;
                        out.last().is_some_and(|node| tree.is(*node, W, "tbl"))
                    };
                    if touches_table {
                        let props = ParaProps {
                            style: self.style(Role::Body)?,
                            ..ParaProps::default()
                        };
                        out.push(self.new_paragraph(part, scope, &props)?);
                    }
                    last_table = Some(out.len());
                    out.push(self.table(
                        part,
                        scope,
                        aligns,
                        head,
                        rows,
                        layout.as_deref(),
                        ctx,
                    )?);
                    continue;
                }
                Block::DefinitionList(entries) => {
                    for (term, definitions) in entries {
                        let props = ParaProps {
                            style: self.style(Role::DefinitionTerm)?,
                            indent_left: ctx.indent,
                            ..ParaProps::default()
                        };
                        out.push(self.paragraph(part, scope, &props, term, ctx)?);
                        for definition in definitions {
                            let inner = Ctx {
                                paragraph_role: Some(Role::Definition),
                                top_level: false,
                                ..ctx
                            };
                            out.extend(self.blocks(part, scope, definition, inner)?);
                        }
                    }
                }
                Block::DisplayMath(math) => {
                    let props = ParaProps {
                        style: self.style(ctx.paragraph_role.unwrap_or(Role::Body))?,
                        indent_left: ctx.indent,
                        ..ParaProps::default()
                    };
                    let p = self.new_paragraph(part, scope, &props)?;
                    self.math(part, p, math, true)?;
                    out.push(p);
                }
                // Word's own horizontal line (a thin grey standard rule on a
                // line of its own), as pandoc writes it: no style, so no
                // body-text spacing or border.
                Block::Rule => {
                    let props = ParaProps {
                        indent_left: ctx.indent,
                        ..ParaProps::default()
                    };
                    let p = self.new_paragraph(part, scope, &props)?;
                    let tree = self.txn.tree_mut(part)?;
                    let r = w_el(tree, p, "r")?;
                    append(tree, p, r)?;
                    let pict = w_el(tree, r, "pict")?;
                    append(tree, r, pict)?;
                    let rect = tree.create_element(pict, VML_NS, "rect")?;
                    tree.set_attr(rect, None, "style", "width:0;height:1.5pt")?;
                    for (name, value) in [("hralign", "center"), ("hrstd", "t"), ("hr", "t")] {
                        tree.set_attr(rect, Some(OFFICE_NS), name, value)?;
                    }
                    append(tree, pict, rect)?;
                    out.push(p);
                }
                Block::PageBreak => {
                    let props = ParaProps {
                        style: self.style(Role::Body)?,
                        ..ParaProps::default()
                    };
                    let p = self.new_paragraph(part, scope, &props)?;
                    let tree = self.txn.tree_mut(part)?;
                    let r = w_el(tree, p, "r")?;
                    append(tree, p, r)?;
                    let br = w_el(tree, r, "br")?;
                    set_w(tree, br, "type", "page")?;
                    append(tree, r, br)?;
                    out.push(p);
                }
                Block::Toc => {
                    if ctx.in_note || part != self.main {
                        self.warn(
                            "toc_outside_body",
                            "a table of contents marker inside a footnote was dropped".into(),
                            None,
                        );
                    } else {
                        let depth = self.options.toc_depth.unwrap_or(3).clamp(1, 9);
                        let title = self
                            .options
                            .toc_title
                            .clone()
                            .unwrap_or_else(|| "Contents".to_string());
                        out.extend(self.toc(scope, depth, &title)?);
                    }
                }
                // Unwrapped above.
                Block::Aligned(..) => {}
                Block::Html { html, line } => {
                    let shown: String = html.trim().chars().take(60).collect();
                    self.warn(
                        "raw_html_dropped",
                        format!("HTML block {shown:?} has no Word equivalent and was dropped"),
                        Some(*line),
                    );
                }
            }
            last_table = None;
        }
        if let Some(caption) = pending_caption {
            out.push(self.caption(part, scope, Role::TableCaption, &caption)?);
        }
        self.space_after_blocks(part, &out, &list_ends)?;
        Ok(out)
    }

    /// A paragraph right after a table or a list gets the gap the template
    /// leaves between paragraphs (its style's space after, else 6 pt) as
    /// space before, when its style gives it none: Word puts no space under
    /// a table, and list styles usually have none after their last item, so
    /// the text would touch the last row or item. `list_ends` are the
    /// indices in `nodes` of each outermost list's last node.
    fn space_after_blocks(
        &mut self,
        part: &str,
        nodes: &[NodeId],
        list_ends: &[usize],
    ) -> Result<()> {
        let styles_part = crate::parts::ensure_styles_part(self.txn)?;
        let style_of = |tree: &XmlTree, p: NodeId| {
            w_child(tree, p, "pPr")
                .and_then(|p| w_child(tree, p, "pStyle"))
                .and_then(|s| w_attr(tree, s, "val"))
                .map(str::to_string)
        };
        // (paragraph, its style, the space the node above leaves below it)
        let mut targets = Vec::new();
        {
            let tree = self.txn.tree(part)?;
            let styles = self.txn.tree(&styles_part)?;
            for (index, pair) in nodes.windows(2).enumerate() {
                if !tree.is(pair[1], W, "p") {
                    continue;
                }
                let above = if tree.is(pair[0], W, "tbl") {
                    Some(0)
                } else if list_ends.contains(&index) && tree.is(pair[0], W, "p") {
                    Some(
                        effective_spacing(styles, style_of(tree, pair[0]).as_deref())
                            .1
                            .unwrap_or(0),
                    )
                } else {
                    None
                };
                if let Some(above) = above {
                    targets.push((pair[1], style_of(tree, pair[1]), above));
                }
            }
        }
        for (p, style, above) in targets {
            let (before, after) = {
                let styles = self.txn.tree(&styles_part)?;
                effective_spacing(styles, style.as_deref())
            };
            let gap = after.filter(|a| *a > 0).unwrap_or(120);
            // A last item that already leaves the gap below needs no more.
            if before.unwrap_or(0) > 0 || above >= gap {
                continue;
            }
            let tree = self.txn.tree_mut(part)?;
            let ppr = crate::oxml::get_or_add_ppr(tree, p)?;
            let spacing = crate::oxml::get_or_add(tree, ppr, "spacing", crate::oxml::PPR_SEQ)?;
            if w_attr(tree, spacing, "before").is_none() {
                set_w(tree, spacing, "before", &gap.to_string())?;
            }
        }
        Ok(())
    }

    /// A new detached paragraph with `props`.
    fn new_paragraph(&mut self, part: &str, scope: NodeId, props: &ParaProps) -> Result<NodeId> {
        let tree = self.txn.tree_mut(part)?;
        let p = w_el(tree, scope, "p")?;
        write_ppr(tree, p, props)?;
        self.report.counts.paragraphs += 1;
        Ok(p)
    }

    fn paragraph(
        &mut self,
        part: &str,
        scope: NodeId,
        props: &ParaProps,
        inlines: &[Inline],
        ctx: Ctx,
    ) -> Result<NodeId> {
        let p = self.new_paragraph(part, scope, props)?;
        let mut pieces = Vec::new();
        flatten(inlines, &Fmt::default(), &mut pieces);
        self.emit(part, p, &pieces, ctx)?;
        Ok(p)
    }

    fn heading(
        &mut self,
        part: &str,
        scope: NodeId,
        level: u8,
        content: &[Inline],
        ctx: Ctx,
    ) -> Result<NodeId> {
        let props = ParaProps {
            style: self.style(Role::heading(level))?,
            align: ctx.align.and_then(jc),
            ..ParaProps::default()
        };
        let p = self.new_paragraph(part, scope, &props)?;
        self.report.counts.headings += 1;
        let bookmark = if ctx.in_note || part != self.main {
            None
        } else {
            self.heading_bookmarks.pop_front()
        };
        let id = self.next_bookmark_id;
        if let Some(name) = &bookmark {
            self.next_bookmark_id += 1;
            let tree = self.txn.tree_mut(part)?;
            let start = w_el(tree, p, "bookmarkStart")?;
            set_w(tree, start, "id", &id.to_string())?;
            set_w(tree, start, "name", name)?;
            append(tree, p, start)?;
        }
        let mut pieces = Vec::new();
        flatten(content, &Fmt::default(), &mut pieces);
        self.emit(part, p, &pieces, ctx)?;
        if bookmark.is_some() {
            let tree = self.txn.tree_mut(part)?;
            let end = w_el(tree, p, "bookmarkEnd")?;
            set_w(tree, end, "id", &id.to_string())?;
            append(tree, p, end)?;
        }
        Ok(p)
    }

    fn code_block(&mut self, part: &str, scope: NodeId, text: &str, ctx: Ctx) -> Result<NodeId> {
        let props = ParaProps {
            style: self.style(Role::CodeBlock)?,
            indent_left: ctx.indent.or(quote_indent(ctx.quote_depth)),
            ..ParaProps::default()
        };
        let p = self.new_paragraph(part, scope, &props)?;
        self.report.counts.code_blocks += 1;
        let text = text.strip_suffix('\n').unwrap_or(text);
        if !text.is_empty() {
            let tree = self.txn.tree_mut(part)?;
            let r = w_el(tree, p, "r")?;
            append(tree, p, r)?;
            append_run_content(tree, r, text)?;
        }
        Ok(p)
    }

    fn alert_label(
        &mut self,
        part: &str,
        scope: NodeId,
        alert: AlertKind,
        ctx: Ctx,
    ) -> Result<NodeId> {
        let props = ParaProps {
            style: self.style(Role::BlockQuote)?,
            keep_next: true,
            indent_left: quote_indent(ctx.quote_depth),
            ..ParaProps::default()
        };
        let inlines = [Inline::Strong(vec![Inline::Text(
            alert.label().to_string(),
        )])];
        self.paragraph(part, scope, &props, &inlines, ctx)
    }

    fn caption(
        &mut self,
        part: &str,
        scope: NodeId,
        role: Role,
        inlines: &[Inline],
    ) -> Result<NodeId> {
        let props = ParaProps {
            style: self.style(role)?,
            keep_next: role == Role::TableCaption,
            ..ParaProps::default()
        };
        let ctx = Ctx {
            paragraph_role: None,
            indent: None,
            quote_depth: 0,
            in_note: part != self.main,
            top_level: false,
            max_image_width: FALLBACK_BLOCK_WIDTH,
            align: None,
        };
        self.paragraph(part, scope, &props, inlines, ctx)
    }

    fn figure(
        &mut self,
        part: &str,
        scope: NodeId,
        image: &Inline,
        ctx: Ctx,
    ) -> Result<Vec<NodeId>> {
        let Inline::Image { alt, .. } = image else {
            return Ok(Vec::new());
        };
        let props = ParaProps {
            style: self.style(Role::Figure)?,
            indent_left: ctx.indent,
            align: ctx.align.and_then(jc),
            ..ParaProps::default()
        };
        let p = self.paragraph(part, scope, &props, std::slice::from_ref(image), ctx)?;
        let mut out = vec![p];
        if self.options.figure_captions && !inline_text(alt).trim().is_empty() {
            out.push(self.caption(part, scope, Role::ImageCaption, alt)?);
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn list(
        &mut self,
        part: &str,
        scope: NodeId,
        start: Option<u64>,
        tight: bool,
        items: &[ListItem],
        depth: usize,
        ctx: Ctx,
    ) -> Result<Vec<NodeId>> {
        let level = depth.min(8) as i64;
        // Lists inside quotes, footnotes and definitions keep that look.
        let role = match ctx.paragraph_role {
            Some(role @ (Role::BlockQuote | Role::FootnoteText | Role::Definition)) => role,
            _ if tight => Role::Compact,
            _ => Role::Body,
        };
        let keeps_context_look = matches!(
            ctx.paragraph_role,
            Some(Role::BlockQuote | Role::FootnoteText | Role::Definition)
        );
        // The reference document's own list look wins for the levels it
        // defines; deeper levels use the converter's definitions.
        let template = {
            let lists = self.lists()?;
            if start.is_some() {
                lists.template_number.clone()
            } else {
                lists.template_bullet.clone()
            }
        };
        let source = template.as_ref().and_then(|t| {
            t.levels
                .get(&level)
                .cloned()
                .map(|source| (t.name.clone(), source))
        });
        let (num_id, ilvl, item_style, indent, item_indent) = match source {
            Some((name, source)) => {
                let num_id = match start {
                    None => source.num_id,
                    Some(first) => self.add_num(source.abstract_id, Some((source.ilvl, first)))?,
                };
                let style = match &source.style_id {
                    Some(id) if !keeps_context_look => Some(id.clone()),
                    _ => self.style(role)?,
                };
                let key = if start.is_some() {
                    "list_number"
                } else {
                    "list_bullet"
                };
                self.report
                    .styles
                    .entry(key.to_string())
                    .or_insert_with(|| super::RoleStyle {
                        name,
                        style_id: source.style_id.clone(),
                        created: false,
                    });
                (
                    num_id,
                    source.ilvl,
                    style,
                    source.indent,
                    source.extrapolated,
                )
            }
            None => {
                let num_id = match start {
                    None => self.bullet_num()?,
                    Some(first) => self.ordered_num(level, first)?,
                };
                (
                    num_id,
                    level,
                    self.style(role)?,
                    LIST_INDENT * (level + 1),
                    None,
                )
            }
        };
        let continuation = Ctx {
            indent: Some(indent),
            top_level: false,
            paragraph_role: Some(role),
            ..ctx
        };
        let mut out = Vec::new();
        for item in items {
            self.report.counts.list_items += 1;
            let marker = item
                .task
                .map(|done| if done { "\u{2612} " } else { "\u{2610} " });
            let mut rest: &[Block] = &item.blocks;
            let (first, first_align) = match rest.first().map(unaligned) {
                Some((Block::Paragraph(inlines) | Block::Plain(inlines), align)) => {
                    rest = &rest[1..];
                    (inlines.clone(), align)
                }
                _ => (Vec::new(), None),
            };
            let props = ParaProps {
                style: item_style.clone(),
                numbering: Some((num_id, ilvl)),
                indent_left: item_indent.map(|(left, _)| left),
                hanging: item_indent.map(|(_, hanging)| hanging),
                align: first_align.or(ctx.align).and_then(jc),
                ..ParaProps::default()
            };
            let mut inlines = Vec::new();
            if let Some(marker) = marker {
                inlines.push(Inline::Text(marker.to_string()));
            }
            inlines.extend(first);
            out.push(self.paragraph(part, scope, &props, &inlines, continuation)?);
            for block in rest {
                match unaligned(block) {
                    (
                        Block::List {
                            start,
                            tight,
                            items,
                        },
                        align,
                    ) => {
                        let ctx = Ctx {
                            align: align.or(ctx.align),
                            ..ctx
                        };
                        out.extend(self.list(part, scope, *start, *tight, items, depth + 1, ctx)?)
                    }
                    _ => out.extend(self.blocks(
                        part,
                        scope,
                        std::slice::from_ref(block),
                        continuation,
                    )?),
                }
            }
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        part: &str,
        scope: NodeId,
        aligns: &[Align],
        head: &[Vec<Block>],
        rows: &[Vec<Vec<Block>>],
        layout: Option<&TableLayout>,
        ctx: Ctx,
    ) -> Result<NodeId> {
        // An HTML table lists its header rows among the rows; a Markdown
        // table always has exactly one.
        let mut all_rows: Vec<&[Vec<Block>]> = Vec::new();
        if layout.is_none() || !head.is_empty() {
            all_rows.push(head);
        }
        all_rows.extend(rows.iter().map(Vec::as_slice));
        if all_rows.is_empty() {
            all_rows.push(&[]);
        }
        let header_rows = layout.map_or(1, |layout| layout.header_rows);
        let spans: Vec<Vec<super::ast::CellSpan>> = match layout {
            Some(layout) if layout.spans.len() == all_rows.len() => layout.spans.clone(),
            _ => all_rows
                .iter()
                .map(|row| vec![super::ast::CellSpan::default(); row.len()])
                .collect(),
        };
        let (grid, cols) = place(&spans);
        let cols = cols.max(1);
        let width = ctx.max_image_width - ctx.indent.unwrap_or(0) * EMU_PER_TWIP;
        let table_style = self.style(Role::Table)?;
        let cell_style = self.style(Role::TableContents)?;
        self.report.counts.tables += 1;
        let total_twips = width / EMU_PER_TWIP;
        let col_twips = column_widths(&all_rows, &grid, cols, header_rows, layout, total_twips);
        let (tbl, cells) = {
            let tree = self.txn.tree_mut(part)?;
            let tbl = new_tbl(tree, scope, all_rows.len(), cols, width)?;
            set_table_style_id(tree, tbl, table_style.as_deref())?;
            // The template's sample table shows which conditional looks
            // (first column, banding, …) it wants; the header row look
            // follows whether the table has one.
            let look_flags = self.styles.sample_table_look();
            if let Some(look) =
                w_child(tree, tbl, "tblPr").and_then(|p| w_child(tree, p, "tblLook"))
                && (!look_flags.is_empty() || header_rows == 0)
            {
                let mut bits = 0u32;
                for (name, bit) in [
                    ("firstRow", 0x20u32),
                    ("lastRow", 0x40),
                    ("firstColumn", 0x80),
                    ("lastColumn", 0x100),
                    ("noHBand", 0x200),
                    ("noVBand", 0x400),
                ] {
                    let on = if name == "firstRow" {
                        header_rows > 0
                    } else if look_flags.is_empty() {
                        w_attr(tree, look, name) == Some("1")
                    } else {
                        look_flags.iter().any(|(flag, on)| flag == name && *on)
                    };
                    set_w(tree, look, name, if on { "1" } else { "0" })?;
                    if on {
                        bits |= bit;
                    }
                }
                set_w(tree, look, "val", &format!("{bits:04X}"))?;
            }
            // Full text width in twips, with the computed columns kept as
            // drawn. A percentage width (`pct`) would also fill the page in
            // desktop Word, but Word for the web mis-lays such tables.
            if let Some(tbl_pr) = w_child(tree, tbl, "tblPr") {
                if let Some(tbl_w) = w_child(tree, tbl_pr, "tblW") {
                    set_w(tree, tbl_w, "type", "dxa")?;
                    set_w(tree, tbl_w, "w", &total_twips.to_string())?;
                }
                let layout = w_el(tree, tbl_pr, "tblLayout")?;
                set_w(tree, layout, "type", "fixed")?;
                insert_before_successors(
                    tree,
                    tbl_pr,
                    layout,
                    &[
                        "tblCellMar",
                        "tblLook",
                        "tblCaption",
                        "tblDescription",
                        "tblPrChange",
                    ],
                )?;
            }
            if let Some(grid_node) = w_child(tree, tbl, "tblGrid") {
                for (col, twips) in w_children(tree, grid_node, "gridCol")
                    .collect::<Vec<_>>()
                    .into_iter()
                    .zip(&col_twips)
                {
                    set_w(tree, col, "w", &twips.to_string())?;
                }
            }
            let mut cells = Vec::new();
            for (row_index, tr) in w_children(tree, tbl, "tr")
                .collect::<Vec<_>>()
                .into_iter()
                .enumerate()
            {
                if row_index < header_rows {
                    let tr_pr = w_el(tree, tr, "trPr")?;
                    let header = w_el(tree, tr_pr, "tblHeader")?;
                    append(tree, tr_pr, header)?;
                    tree.insert(tr, 0, tr_pr)?;
                }
                let tcs: Vec<NodeId> = w_children(tree, tr, "tc").collect();
                let slots = &grid[row_index];
                // One `w:tc` per slot; a spanning cell covers several
                // columns, so the row needs fewer.
                for extra in tcs.iter().skip(slots.len()) {
                    crate::authoring::remove(tree, *extra)?;
                }
                for (tc, slot) in tcs.into_iter().zip(slots) {
                    let col = slot.col();
                    let span = slot.colspan();
                    let twips: i64 = col_twips[col..(col + span).min(cols)].iter().sum();
                    let tc_pr = w_child(tree, tc, "tcPr").expect("new_tbl gives every cell tcPr");
                    if let Some(tc_w) = w_child(tree, tc_pr, "tcW") {
                        set_w(tree, tc_w, "w", &twips.to_string())?;
                    }
                    let add = |tree: &mut XmlTree, local: &str, value: Option<&str>| {
                        let node = w_el(tree, tc_pr, local)?;
                        if let Some(value) = value {
                            set_w(tree, node, "val", value)?;
                        }
                        append(tree, tc_pr, node)
                    };
                    if span > 1 {
                        add(tree, "gridSpan", Some(&span.to_string()))?;
                    }
                    let valign = match *slot {
                        Slot::Cell {
                            span, merges_down, ..
                        } => {
                            if merges_down {
                                add(tree, "vMerge", Some("restart"))?;
                            }
                            span.valign
                        }
                        Slot::Continue { span, .. } => {
                            add(tree, "vMerge", None)?;
                            span.valign
                        }
                        Slot::Empty { .. } => None,
                    };
                    if let Some(valign) = valign {
                        let value = match valign {
                            VAlign::Top => "top",
                            VAlign::Center => "center",
                            VAlign::Bottom => "bottom",
                        };
                        add(tree, "vAlign", Some(value))?;
                    }
                    let p = w_child(tree, tc, "p").expect("new_tbl gives every cell a paragraph");
                    let index = match *slot {
                        Slot::Cell { index, .. } => Some(index),
                        _ => None,
                    };
                    cells.push((row_index, col, twips, index, tc, p));
                }
            }
            (tbl, cells)
        };
        for (row_index, col, twips, index, tc, p) in cells {
            let column_align = aligns.get(col).copied().unwrap_or(Align::None);
            let cell_ctx = Ctx {
                max_image_width: twips * EMU_PER_TWIP,
                indent: None,
                top_level: false,
                paragraph_role: Some(Role::TableContents),
                align: None,
                ..ctx
            };
            let content: &[Block] = index
                .and_then(|index| all_rows[row_index].get(index))
                .map_or(&[], Vec::as_slice);
            // The cell's own paragraph takes leading inline content; any
            // further blocks (paragraphs, lists) follow it in the cell.
            let (first, own_align, rest): (&[Inline], Option<Align>, &[Block]) =
                match content.split_first() {
                    Some((block, rest)) => match unaligned(block) {
                        (Block::Plain(inlines) | Block::Paragraph(inlines), align) => {
                            (inlines, align, rest)
                        }
                        _ => (&[], None, content),
                    },
                    None => (&[], None, content),
                };
            let props = ParaProps {
                style: cell_style.clone(),
                align: jc(own_align.unwrap_or(column_align)),
                ..ParaProps::default()
            };
            {
                let tree = self.txn.tree_mut(part)?;
                write_ppr(tree, p, &props)?;
            }
            self.report.counts.paragraphs += 1;
            let mut pieces = Vec::new();
            flatten(first, &Fmt::default(), &mut pieces);
            self.emit(part, p, &pieces, cell_ctx)?;
            if rest.is_empty() {
                continue;
            }
            let nodes = self.blocks(part, tc, rest, cell_ctx)?;
            let tree = self.txn.tree_mut(part)?;
            if first.is_empty() {
                crate::authoring::remove(tree, p)?;
            }
            for node in nodes {
                append(tree, tc, node)?;
            }
            // A cell must end with a paragraph.
            let ends_with_paragraph = tree
                .child_elements(tc)
                .last()
                .is_some_and(|last| tree.is(last, W, "p"));
            if !ends_with_paragraph {
                let empty = w_el(tree, tc, "p")?;
                append(tree, tc, empty)?;
            }
        }
        let styles_part = crate::parts::ensure_styles_part(self.txn)?;
        let header_color = super::styles::header_color_override(
            self.txn.tree(&styles_part)?,
            table_style.as_deref(),
            cell_style.as_deref(),
        );
        if let Some(color) = header_color.filter(|_| header_rows > 0) {
            let tree = self.txn.tree_mut(part)?;
            let header_runs: Vec<NodeId> = w_child(tree, tbl, "tr")
                .map(|tr| {
                    tree.descendants(tr)
                        .filter(|node| tree.is(*node, W, "r"))
                        .collect()
                })
                .unwrap_or_default();
            for run in header_runs {
                let rpr = crate::oxml::get_or_add_rpr(tree, run)?;
                // Code and link runs keep their own character style.
                if w_child(tree, rpr, "rStyle").is_some() || w_child(tree, rpr, "color").is_some() {
                    continue;
                }
                let node = crate::oxml::get_or_add(tree, rpr, "color", crate::oxml::RPR_SEQ)?;
                set_w(tree, node, "val", &color)?;
            }
        }
        Ok(tbl)
    }

    // ----------------------------------------------------------- inlines

    fn run_props(&mut self, fmt: &Fmt) -> Result<RunProps> {
        let style = if fmt.code {
            self.style(Role::InlineCode)?
        } else if fmt.link {
            self.style(Role::Hyperlink)?
        } else {
            None
        };
        // An editor highlight in one of Word's highlighter colours is a
        // highlight; any other colour is run shading.
        let (highlight, shading) = match &fmt.style.highlight {
            Some(rgb) => match HIGHLIGHTS.iter().find(|(hex, _)| hex == rgb) {
                Some((_, name)) => (Some(*name), None),
                None => (None, Some(rgb.clone())),
            },
            None => (fmt.highlight.then_some("yellow"), None),
        };
        Ok(RunProps {
            style,
            font: fmt.style.font.clone(),
            bold: fmt.bold,
            italic: fmt.italic,
            strike: fmt.strike,
            color: fmt
                .style
                .color
                .clone()
                .or_else(|| (fmt.code && fmt.link).then(|| "0563C1".to_string())),
            size: fmt.style.size,
            highlight,
            shading,
            underline: fmt.underline || (fmt.code && fmt.link),
            vert_align: if fmt.superscript {
                Some("superscript")
            } else if fmt.subscript {
                Some("subscript")
            } else {
                None
            },
        })
    }

    fn emit(&mut self, part: &str, parent: NodeId, pieces: &[Piece], ctx: Ctx) -> Result<()> {
        for piece in pieces {
            match piece {
                Piece::Math(source) => self.math(part, parent, source, false)?,
                Piece::Text(fmt, text) => {
                    let props = self.run_props(fmt)?;
                    let tree = self.txn.tree_mut(part)?;
                    let r = new_run(tree, parent, &props)?;
                    append_run_content(tree, r, text)?;
                }
                Piece::Break(fmt) => {
                    self.line_breaks = true;
                    let props = self.run_props(fmt)?;
                    let tree = self.txn.tree_mut(part)?;
                    let r = new_run(tree, parent, &props)?;
                    let br = w_el(tree, r, "br")?;
                    append(tree, r, br)?;
                }
                Piece::Image {
                    url,
                    title,
                    alt,
                    line,
                    width,
                    height,
                } => {
                    let size = (*width, *height);
                    if !self.picture(part, parent, url, title, alt, *line, size, ctx)? {
                        let props = self.run_props(&Fmt {
                            italic: true,
                            ..Fmt::default()
                        })?;
                        let tree = self.txn.tree_mut(part)?;
                        let r = new_run(tree, parent, &props)?;
                        let shown = if alt.is_empty() {
                            url.as_str()
                        } else {
                            alt.as_str()
                        };
                        append_run_content(tree, r, &format!("[{shown}]"))?;
                    }
                }
                Piece::Note { label, line } => self.footnote(part, parent, label, *line, ctx)?,
                Piece::Link { url, title, pieces } => {
                    self.link(part, parent, url, title, pieces, ctx)?
                }
            }
        }
        Ok(())
    }

    fn link(
        &mut self,
        part: &str,
        parent: NodeId,
        url: &str,
        title: &str,
        pieces: &[Piece],
        ctx: Ctx,
    ) -> Result<()> {
        if url.is_empty() {
            return self.emit(part, parent, &unlinked(pieces), ctx);
        }
        let (anchor, r_id) = if let Some(fragment) = url.strip_prefix('#') {
            let key = percent_decode(fragment);
            match self
                .anchors
                .get(&key)
                .or_else(|| self.anchors.get(&slug(&key)))
            {
                Some(name) => (Some(name.clone()), None),
                None => {
                    self.warn(
                        "broken_internal_link",
                        format!(
                            "link target {url} matches no heading; the text is kept without a link"
                        ),
                        None,
                    );
                    return self.emit(part, parent, &unlinked(pieces), ctx);
                }
            }
        } else {
            let target = link_target(url);
            if !safe_link_target(&target) {
                self.warn(
                    "link_scheme_skipped",
                    format!(
                        "link target {url} uses a scheme other than http, https, mailto or tel; the text is kept without a link"
                    ),
                    None,
                );
                return self.emit(part, parent, &unlinked(pieces), ctx);
            }
            let r_id = self
                .txn
                .relate_to(part, rt::HYPERLINK, RelTarget::External(&target))?;
            (None, Some(r_id))
        };
        self.report.counts.links += 1;
        let hyperlink = {
            let tree = self.txn.tree_mut(part)?;
            let hyperlink = w_el(tree, parent, "hyperlink")?;
            if let Some(r_id) = &r_id {
                tree.set_attr(hyperlink, Some(R), "id", r_id)?;
            }
            if let Some(anchor) = &anchor {
                set_w(tree, hyperlink, "anchor", anchor)?;
            }
            if !title.is_empty() {
                set_w(tree, hyperlink, "tooltip", title)?;
            }
            set_w(tree, hyperlink, "history", "1")?;
            append(tree, parent, hyperlink)?;
            hyperlink
        };
        self.emit(part, hyperlink, pieces, ctx)
    }

    fn footnote(
        &mut self,
        part: &str,
        parent: NodeId,
        label: &str,
        line: usize,
        ctx: Ctx,
    ) -> Result<()> {
        let parsed = self.parsed;
        let definition = parsed.footnotes.get(label);
        let refuse_reason = if ctx.in_note || part != self.main {
            Some((
                "nested_footnote",
                format!("footnote [^{label}] inside a footnote is kept as text"),
            ))
        } else if definition.is_none() {
            Some((
                "undefined_footnote",
                format!("footnote [^{label}] has no definition and is kept as text"),
            ))
        } else {
            None
        };
        let Some(definition) = definition.filter(|_| refuse_reason.is_none()) else {
            let (code, message) = refuse_reason.expect("a reason");
            self.warn(code, message, Some(line));
            let props = self.run_props(&Fmt::default())?;
            let tree = self.txn.tree_mut(part)?;
            let r = new_run(tree, parent, &props)?;
            add_t(tree, r, &format!("[^{label}]"))?;
            return Ok(());
        };
        let reference_style = self.style(Role::FootnoteReference)?;
        let text_style = self.style(Role::FootnoteText)?;
        let (notes, id) = crate::notes::footnotes_part_and_next_id(self.txn)?;
        self.report.counts.footnotes += 1;
        let note = {
            let tree = self.txn.tree_mut(&notes)?;
            let root = tree.root();
            let note = w_el(tree, root, "footnote")?;
            set_w(tree, note, "id", &id.to_string())?;
            append(tree, root, note)?;
            note
        };
        let note_ctx = Ctx {
            paragraph_role: Some(Role::FootnoteText),
            indent: None,
            quote_depth: 0,
            in_note: true,
            top_level: false,
            max_image_width: ctx.max_image_width,
            align: None,
        };
        let content = self.blocks(&notes, note, definition, note_ctx)?;
        let tree = self.txn.tree_mut(&notes)?;
        let starts_with_paragraph = content.first().is_some_and(|n| tree.is(*n, W, "p"));
        let first = if starts_with_paragraph {
            content[0]
        } else {
            let p = w_el(tree, note, "p")?;
            write_ppr(
                tree,
                p,
                &ParaProps {
                    style: text_style.clone(),
                    ..ParaProps::default()
                },
            )?;
            append(tree, note, p)?;
            p
        };
        for node in content {
            append(tree, note, node)?;
        }
        // The note's own mark, then a space, open its first paragraph.
        let mark_props = RunProps {
            style: reference_style.clone(),
            vert_align: reference_style.is_none().then_some("superscript"),
            ..RunProps::default()
        };
        let mark = w_el(tree, first, "r")?;
        write_rpr(tree, mark, &mark_props)?;
        let reference = w_el(tree, mark, "footnoteRef")?;
        append(tree, mark, reference)?;
        let space = w_el(tree, first, "r")?;
        add_t(tree, space, " ")?;
        let at = usize::from(w_child(tree, first, "pPr").is_some());
        tree.insert(first, at, mark)?;
        tree.insert(first, at + 1, space)?;
        // The reference in the text.
        let tree = self.txn.tree_mut(part)?;
        let r = new_run(tree, parent, &mark_props)?;
        let reference = w_el(tree, r, "footnoteReference")?;
        set_w(tree, reference, "id", &id.to_string())?;
        append(tree, r, reference)?;
        Ok(())
    }

    // ----------------------------------------------------------- pictures

    /// The picture bytes a reference names, and the file name its media
    /// part may take. Bytes supplied in `images` come first, then a `data:`
    /// URI, then a file, read only when a `resource_dir` is set and only
    /// from inside it (see [`resolve_in`]). Remote URLs are never fetched,
    /// and nothing larger than the package's `max_part_bytes` is taken.
    /// Warnings quote the reference as written, never a resolved path or an
    /// operating system error.
    fn load_image(&mut self, url: &str, line: usize) -> Option<(Cow<'p, [u8]>, Option<String>)> {
        let options: &'p MarkdownOptions = self.options;
        let limit = self.txn.document().package().limits().max_part_bytes;
        // Supplied bytes first, keyed by the reference as written (a path,
        // an id or a URL, which is never fetched); `./a.png` and `a%20b.png`
        // find `a.png` and `a b.png`, and the other way round.
        let key = image_key(url);
        let supplied = options.images.get(url).or_else(|| {
            options
                .images
                .iter()
                .find(|(name, _)| image_key(name) == key)
                .map(|(_, bytes)| bytes)
        });
        if let Some(bytes) = supplied {
            if bytes.len() as u64 > limit {
                self.image_too_large(&image_name(url), limit, line);
                return None;
            }
            let name = key
                .split(['?', '#'])
                .next()
                .and_then(|path| path.rsplit('/').next())
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            return Some((Cow::Borrowed(bytes.as_slice()), name));
        }
        if let Some(rest) = url.strip_prefix("data:") {
            let data = rest
                .split_once(',')
                .filter(|(meta, _)| meta.ends_with(";base64"))
                .map(|(_, data)| data);
            if let Some(data) = data
                && base64_len(data) > limit
            {
                self.image_too_large("a data: image", limit, line);
                return None;
            }
            let decoded = data.and_then(base64_decode);
            if decoded.is_none() {
                self.warn(
                    "image_unreadable",
                    "a data: image is not base64-encoded and was replaced by its alt text".into(),
                    Some(line),
                );
            }
            return decoded.map(|bytes| (Cow::Owned(bytes), None));
        }
        if url.contains("://") && !url.starts_with("file://") {
            self.warn(
                "remote_image_skipped",
                format!("remote image {url} is not downloaded; its alt text is shown instead"),
                Some(line),
            );
            return None;
        }
        let dir = match &options.resource_dir {
            // No file system: only images supplied in `images` exist.
            Some(dir) if !cfg!(all(target_family = "wasm", target_os = "unknown")) => dir,
            _ => {
                self.warn(
                    "image_not_found",
                    format!(
                        "image {url} was not supplied and no resource directory is set to read it from; its alt text is shown instead"
                    ),
                    Some(line),
                );
                return None;
            }
        };
        let decoded = percent_decode(url);
        match resolve_in(dir, &decoded).and_then(|path| read_within(&path, limit)) {
            Ok(bytes) => {
                let name = decoded
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|name| !name.is_empty())
                    .map(str::to_string);
                Some((Cow::Owned(bytes), name))
            }
            Err(Unread::Outside) => {
                self.warn(
                    "image_outside_resource_dir",
                    format!(
                        "image {url} is not inside the resource directory and was not read; its alt text is shown instead"
                    ),
                    Some(line),
                );
                None
            }
            Err(Unread::Missing) => {
                self.warn(
                    "image_not_found",
                    format!("image {url} could not be read; its alt text is shown instead"),
                    Some(line),
                );
                None
            }
            Err(Unread::TooLarge) => {
                self.image_too_large(url, limit, line);
                None
            }
        }
    }

    fn image_too_large(&mut self, shown: &str, limit: u64, line: usize) {
        self.warn(
            "image_too_large",
            format!(
                "image {shown} is larger than the {limit}-byte part limit; its alt text is shown instead"
            ),
            Some(line),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn picture(
        &mut self,
        part: &str,
        parent: NodeId,
        url: &str,
        title: &str,
        alt: &str,
        line: usize,
        (width_px, height_px): (Option<f64>, Option<f64>),
        ctx: Ctx,
    ) -> Result<bool> {
        let Some((bytes, filename)) = self.load_image(url, line) else {
            return Ok(false);
        };
        let shown = image_name(url);
        let image = match Image::from_vec(bytes.into_owned()) {
            Ok(image) => image,
            Err(err) if err.reason() == "unrecognized_image" => {
                self.warn(
                    "image_unsupported",
                    format!("image {shown} is not a BMP, GIF, JPEG, PNG or TIFF file; its alt text is shown instead"),
                    Some(line),
                );
                return Ok(false);
            }
            Err(_) => {
                self.image_unreadable(&shown, line);
                return Ok(false);
            }
        };
        // The media part is named after the source only when that name has
        // an image extension (`img-123`, `photo?id=4` do not); otherwise
        // after the detected format, so the part gets a content type.
        let filename = filename.filter(|name| {
            matches!(
                crate::image::py_splitext_ext(name)
                    .to_ascii_lowercase()
                    .as_str(),
                "png" | "jpg" | "jpeg" | "jpe" | "gif" | "bmp" | "tif" | "tiff"
            )
        });
        let max_width =
            (ctx.max_image_width - ctx.indent.unwrap_or(0) * EMU_PER_TWIP).max(EMU_PER_TWIP);
        // The editor's size (CSS pixels, 96 per inch), else the image's
        // own; never wider than the text.
        let px = |value: f64| (value * EMU_PER_PX).round() as i64;
        let Ok((mut width, mut height)) =
            image.layout_dimensions(width_px.map(px), height_px.map(px))
        else {
            self.image_unreadable(&shown, line);
            return Ok(false);
        };
        if width > max_width && width > 0 {
            height = (height as f64 * max_width as f64 / width as f64).round() as i64;
            width = max_width;
        }
        // A size no drawing can hold (a damaged header, or an absurd
        // editor size) is not laid out.
        if !(0..=MAX_EXTENT).contains(&height) || !(0..=MAX_EXTENT).contains(&width) {
            self.image_unreadable(&shown, line);
            return Ok(false);
        }
        let r = {
            let tree = self.txn.tree_mut(part)?;
            let r = w_el(tree, parent, "r")?;
            append(tree, parent, r)?;
            r
        };
        add_picture_named(
            self.txn,
            part,
            r,
            image.blob(),
            filename.as_deref(),
            Some(width.max(1)),
            Some(height.max(1)),
        )?;
        self.report.counts.images += 1;
        let tree = self.txn.tree_mut(part)?;
        let doc_pr = tree
            .descendants(r)
            .find(|node| tree.is(*node, WP, "docPr"))
            .ok_or_else(|| Error::internal("picture_doc_pr", "new picture has no wp:docPr"))?;
        if !alt.is_empty() {
            tree.set_attr(doc_pr, None, "descr", alt)?;
        }
        if !title.is_empty() {
            tree.set_attr(doc_pr, None, "title", title)?;
        }
        self.pictures.push((part.to_string(), doc_pr));
        Ok(true)
    }

    fn image_unreadable(&mut self, shown: &str, line: usize) {
        self.warn(
            "image_unreadable",
            format!(
                "image {shown} has a damaged or truncated header and cannot be sized; its alt text is shown instead"
            ),
            Some(line),
        );
    }

    /// Pictures were built detached, so each took the same next free id;
    /// give them distinct ones now that they are attached.
    fn number_pictures(&mut self) -> Result<()> {
        let mut by_part: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        for (part, node) in std::mem::take(&mut self.pictures) {
            by_part.entry(part).or_default().push(node);
        }
        for (part, ours) in by_part {
            let tree = self.txn.tree_mut(&part)?;
            let mut highest = 0u64;
            for node in tree.descendants(tree.root()) {
                if ours.contains(&node) {
                    continue;
                }
                if (tree.is(node, WP, "docPr") || tree.is(node, A, "cNvPr"))
                    && let Some(id) = tree
                        .attr(node, None, "id")
                        .and_then(|v| v.parse::<u64>().ok())
                {
                    highest = highest.max(id);
                }
            }
            for (offset, node) in ours.into_iter().enumerate() {
                let id = highest + 1 + offset as u64;
                tree.set_attr(node, None, "id", &id.to_string())?;
                tree.set_attr(node, None, "name", &format!("Picture {id}"))?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------- numbering

    fn lists(&mut self) -> Result<&mut Lists> {
        if self.lists.is_none() {
            let part = get_or_create_numbering(self.txn)?;
            let bullet_abstract = self.abstract_definition(&part, BULLET_NAME)?;
            let decimal_abstract = self.abstract_definition(&part, DECIMAL_NAME)?;
            let (template_bullet, template_number) = if self.options.template_lists {
                (
                    self.template_list(&part, "List Bullet")?,
                    self.template_list(&part, "List Number")?,
                )
            } else {
                (None, None)
            };
            self.lists = Some(Lists {
                part,
                bullet_abstract,
                decimal_abstract,
                bullet_num: None,
                template_bullet,
                template_number,
            });
        }
        Ok(self.lists.as_mut().expect("lists"))
    }

    /// The list look the reference document's `name` styles define: level
    /// `n` comes from `"{name} {n+1}"` when that style binds numbering
    /// (Word's built-in list styles are separate lists), else from level
    /// `n` of the base style's numbering.
    fn template_list(&mut self, numbering: &str, name: &str) -> Result<Option<TemplateList>> {
        let styles_part = crate::parts::ensure_styles_part(self.txn)?;
        // (style id, numId, ilvl) for the base style and each level style.
        let mut bound: BTreeMap<i64, (Option<String>, i64, i64)> = BTreeMap::new();
        // Without such styles, the list paragraphs of the template's own
        // sample body (Word's list buttons, or a custom style such as
        // "Bullet Points" that binds numbering): (style, numId, ilvl) in
        // document order, read from the document as opened.
        let mut samples: Vec<(Option<String>, i64, i64)> = Vec::new();
        {
            let tree = self.txn.tree(&styles_part)?;
            let paragraph_styles: Vec<NodeId> = w_children(tree, tree.root(), "style")
                .filter(|s| w_attr(tree, *s, "type").unwrap_or("paragraph") == "paragraph")
                .collect();
            let named = |wanted: &str| {
                paragraph_styles.iter().copied().find(|s| {
                    w_child(tree, *s, "name")
                        .and_then(|n| w_attr(tree, n, "val"))
                        .is_some_and(|v| v.eq_ignore_ascii_case(wanted))
                })
            };
            // numId and ilvl through the basedOn chain (nearest wins).
            let binding = |style: NodeId| {
                let (mut num_id, mut ilvl) = (None, None);
                let mut current = Some(style);
                for _ in 0..16 {
                    let Some(style) = current else {
                        break;
                    };
                    if let Some(num_pr) =
                        w_child(tree, style, "pPr").and_then(|p| w_child(tree, p, "numPr"))
                    {
                        let value = |local: &str| {
                            w_child(tree, num_pr, local)
                                .and_then(|n| w_attr(tree, n, "val"))
                                .and_then(|v| v.parse::<i64>().ok())
                        };
                        num_id = num_id.or_else(|| value("numId"));
                        ilvl = ilvl.or_else(|| value("ilvl"));
                    }
                    if num_id.is_some() && ilvl.is_some() {
                        break;
                    }
                    current = w_child(tree, style, "basedOn")
                        .and_then(|b| w_attr(tree, b, "val"))
                        .and_then(|id| {
                            paragraph_styles
                                .iter()
                                .copied()
                                .find(|s| w_attr(tree, *s, "styleId") == Some(id))
                        });
                }
                num_id.filter(|n| *n != 0).map(|n| (n, ilvl.unwrap_or(0)))
            };
            // Whether a style (through its basedOn chain) is a heading.
            let outlined = |style: NodeId| {
                let mut current = Some(style);
                for _ in 0..16 {
                    let Some(style) = current else {
                        return false;
                    };
                    let level = w_child(tree, style, "pPr")
                        .and_then(|p| w_child(tree, p, "outlineLvl"))
                        .and_then(|o| w_attr(tree, o, "val"))
                        .and_then(|v| v.parse::<u8>().ok());
                    if let Some(level) = level {
                        return level < 9;
                    }
                    current = w_child(tree, style, "basedOn")
                        .and_then(|b| w_attr(tree, b, "val"))
                        .and_then(|id| {
                            paragraph_styles
                                .iter()
                                .copied()
                                .find(|s| w_attr(tree, *s, "styleId") == Some(id))
                        });
                }
                false
            };
            for level in 0..9i64 {
                let wanted = if level == 0 {
                    name.to_string()
                } else {
                    format!("{name} {}", level + 1)
                };
                if let Some(style) = named(&wanted)
                    && let (Some(id), Some((num_id, ilvl))) =
                        (w_attr(tree, style, "styleId"), binding(style))
                {
                    bound.insert(level, (Some(id.to_string()), num_id, ilvl));
                }
            }
            if bound.is_empty() {
                // A sample list paragraph's style is taken only when it is a
                // list style (it binds the numbering itself) or List
                // Paragraph; a general style the sample body used for
                // everything (Normal (Web) in HTML-converted documents) is
                // replaced by List Paragraph, or else by the body role.
                let list_paragraph = named("List Paragraph")
                    .and_then(|s| w_attr(tree, s, "styleId"))
                    .map(str::to_string);
                let package = self.txn.document().package();
                let main = package.xml(package.main_part_name())?;
                let int = |node: Option<NodeId>| {
                    node.and_then(|n| w_attr(main, n, "val"))
                        .and_then(|v| v.parse::<i64>().ok())
                };
                let body = w_child(main, main.root(), "body");
                for p in body
                    .into_iter()
                    .flat_map(|b| w_children(main, b, "p").collect::<Vec<_>>())
                {
                    let Some(ppr) = w_child(main, p, "pPr") else {
                        continue;
                    };
                    let style_id = w_child(main, ppr, "pStyle")
                        .and_then(|s| w_attr(main, s, "val"))
                        .map(str::to_string);
                    let style = style_id.as_deref().and_then(|id| {
                        paragraph_styles
                            .iter()
                            .copied()
                            .find(|s| w_attr(tree, *s, "styleId") == Some(id))
                    });
                    // Numbered headings are not lists.
                    let heading =
                        w_child(main, ppr, "outlineLvl").is_some() || style.is_some_and(outlined);
                    if heading {
                        continue;
                    }
                    let from_style = style.and_then(binding);
                    let num_pr = w_child(main, ppr, "numPr");
                    let num_id = match num_pr.and_then(|n| w_child(main, n, "numId")) {
                        Some(node) => int(Some(node)),
                        None => from_style.map(|(num_id, _)| num_id),
                    };
                    let ilvl = int(num_pr.and_then(|n| w_child(main, n, "ilvl")))
                        .or(from_style.map(|(_, ilvl)| ilvl))
                        .unwrap_or(0);
                    let is_list_paragraph = style.is_some_and(|s| {
                        w_child(tree, s, "name")
                            .and_then(|n| w_attr(tree, n, "val"))
                            .is_some_and(|v| v.eq_ignore_ascii_case("List Paragraph"))
                    });
                    let style_id = if from_style.is_some() || is_list_paragraph {
                        style_id
                    } else {
                        list_paragraph.clone()
                    };
                    if let Some(num_id) = num_id.filter(|n| *n != 0)
                        && !samples.iter().any(|(_, n, l)| (*n, *l) == (num_id, ilvl))
                    {
                        samples.push((style_id, num_id, ilvl));
                    }
                }
            }
        }
        let tree = self.txn.tree(numbering)?;
        let bullets = name == "List Bullet";
        samples.retain(|(_, num_id, ilvl)| {
            level_format(tree, *num_id, *ilvl)
                .is_some_and(|format| format != "none" && (format == "bullet") == bullets)
        });
        let base = bound.get(&0).cloned().or_else(|| samples.first().cloned());
        let Some((base_style, base_num, base_ilvl)) = base else {
            return Ok(None);
        };
        let root = tree.root();
        // abstractNumId and defined levels (ilvl → (indent, hanging)).
        let instance = |num_id: i64| -> Option<(i64, LevelIndents)> {
            let num = w_children(tree, root, "num").find(|n| {
                w_attr(tree, *n, "numId").and_then(|v| v.parse::<i64>().ok()) == Some(num_id)
            })?;
            let abstract_id = w_child(tree, num, "abstractNumId")
                .and_then(|a| w_attr(tree, a, "val"))
                .and_then(|v| v.parse::<i64>().ok())?;
            let definition = w_children(tree, root, "abstractNum").find(|a| {
                w_attr(tree, *a, "abstractNumId").and_then(|v| v.parse::<i64>().ok())
                    == Some(abstract_id)
            });
            let mut levels = BTreeMap::new();
            let defined = definition
                .into_iter()
                .flat_map(|a| w_children(tree, a, "lvl").collect::<Vec<_>>())
                .chain(
                    w_children(tree, num, "lvlOverride")
                        .filter_map(|o| w_child(tree, o, "lvl"))
                        .collect::<Vec<_>>(),
                );
            for lvl in defined {
                if let Some(level) = w_attr(tree, lvl, "ilvl").and_then(|v| v.parse::<i64>().ok()) {
                    let ind = w_child(tree, lvl, "pPr").and_then(|ppr| w_child(tree, ppr, "ind"));
                    let value = |names: [&str; 2]| {
                        ind.and_then(|ind| {
                            w_attr(tree, ind, names[0]).or_else(|| w_attr(tree, ind, names[1]))
                        })
                        .and_then(|v| v.parse::<i64>().ok())
                    };
                    let indent = value(["left", "start"]).unwrap_or(LIST_INDENT * (level + 1));
                    let hanging = value(["hanging", "hanging"]).unwrap_or(360);
                    levels.insert(level, (indent, hanging));
                }
            }
            Some((abstract_id, levels))
        };
        let Some((base_abstract, base_levels)) = instance(base_num) else {
            return Ok(None);
        };
        // Each further sample list indented deeper than the last becomes
        // the next level ("Second Bullet Point").
        if bound.is_empty() {
            let mut last = base_levels.get(&base_ilvl).map(|(indent, _)| *indent);
            for (style, num_id, ilvl) in samples.iter().skip(1) {
                let indent = instance(*num_id)
                    .and_then(|(_, defined)| defined.get(ilvl).map(|(indent, _)| *indent));
                if let (Some(indent), Some(previous)) = (indent, last)
                    && indent > previous
                    && bound.len() < 8
                {
                    bound.insert(bound.len() as i64 + 1, (style.clone(), *num_id, *ilvl));
                    last = Some(indent);
                }
            }
        }
        let mut levels = BTreeMap::new();
        for level in 0..9i64 {
            let source = match bound.get(&level) {
                Some((style_id, num_id, ilvl)) if level > 0 => {
                    instance(*num_id).and_then(|(abstract_id, defined)| {
                        defined.get(ilvl).map(|(indent, hanging)| LevelSource {
                            num_id: *num_id,
                            abstract_id,
                            ilvl: *ilvl,
                            style_id: style_id.clone(),
                            indent: *indent,
                            hanging: *hanging,
                            extrapolated: None,
                        })
                    })
                }
                _ => None,
            }
            .or_else(|| {
                let ilvl = base_ilvl + level;
                base_levels.get(&ilvl).map(|(indent, hanging)| LevelSource {
                    num_id: base_num,
                    abstract_id: base_abstract,
                    ilvl,
                    style_id: base_style.clone(),
                    indent: *indent,
                    hanging: *hanging,
                    extrapolated: None,
                })
            });
            match source {
                Some(source) => {
                    levels.insert(level, source);
                }
                // Deeper than the template goes: keep its deepest look and
                // continue its indent step.
                None => {
                    let deepest: Vec<&LevelSource> = levels.values().collect();
                    if let [.., previous, last] | [previous @ last] = deepest.as_slice() {
                        // Step by the template's own spacing, never less
                        // than the marker's hanging width.
                        let step = (last.indent - previous.indent).max(last.hanging);
                        let hanging = last.hanging;
                        let indent = last.indent + step;
                        let mut source = (*last).clone();
                        source.indent = indent;
                        source.extrapolated = Some((indent, hanging));
                        levels.insert(level, source);
                    }
                }
            }
        }
        Ok(Some(TemplateList {
            name: name.to_string(),
            levels,
        }))
    }

    /// The abstract definition named `name`, added when missing.
    fn abstract_definition(&mut self, part: &str, name: &str) -> Result<i64> {
        let tree = self.txn.tree_mut(part)?;
        let root = tree.root();
        let existing = w_children(tree, root, "abstractNum")
            .find(|a| w_child(tree, *a, "name").and_then(|n| w_attr(tree, n, "val")) == Some(name));
        if let Some(existing) = existing
            && let Some(id) = w_attr(tree, existing, "abstractNumId").and_then(|v| v.parse().ok())
        {
            return Ok(id);
        }
        let (abstract_id, _) = next_free_ids(tree)?;
        let xml = abstract_xml(abstract_id, name);
        let node = import_fragment(tree, root, &xml)?;
        match w_child(tree, root, "num") {
            Some(first_num) => add_previous(tree, first_num, node)?,
            None => match w_child(tree, root, "numIdMacAtCleanup") {
                Some(cleanup) => add_previous(tree, cleanup, node)?,
                None => append(tree, root, node)?,
            },
        }
        Ok(abstract_id)
    }

    fn add_num(&mut self, abstract_id: i64, restart: Option<(i64, u64)>) -> Result<i64> {
        let part = self.lists()?.part.clone();
        let tree = self.txn.tree_mut(&part)?;
        let root = tree.root();
        let (_, num_id) = next_free_ids(tree)?;
        let override_xml = match restart {
            Some((level, start)) => format!(
                "<w:lvlOverride w:ilvl=\"{level}\"><w:startOverride w:val=\"{start}\"/></w:lvlOverride>"
            ),
            None => String::new(),
        };
        let xml = format!(
            "<w:num xmlns:w=\"{W}\" w:numId=\"{num_id}\"><w:abstractNumId w:val=\"{abstract_id}\"/>{override_xml}</w:num>"
        );
        let node = import_fragment(tree, root, &xml)?;
        match w_children(tree, root, "num").last() {
            Some(last) => add_next(tree, last, node)?,
            None => match w_child(tree, root, "numIdMacAtCleanup") {
                Some(cleanup) => add_previous(tree, cleanup, node)?,
                None => append(tree, root, node)?,
            },
        }
        Ok(num_id)
    }

    fn bullet_num(&mut self) -> Result<i64> {
        if let Some(num) = self.lists()?.bullet_num {
            return Ok(num);
        }
        let abstract_id = self.lists()?.bullet_abstract;
        let num = self.add_num(abstract_id, None)?;
        self.lists()?.bullet_num = Some(num);
        Ok(num)
    }

    /// Every ordered list gets its own instance restarted at its start
    /// number (instances of one definition otherwise continue counting).
    fn ordered_num(&mut self, level: i64, start: u64) -> Result<i64> {
        let abstract_id = self.lists()?.decimal_abstract;
        self.add_num(abstract_id, Some((level, start)))
    }

    // ------------------------------------------------- front matter, TOC

    fn title_block(&mut self, scope: NodeId, metadata: &Metadata) -> Result<Vec<NodeId>> {
        let main = self.main.clone();
        let ctx = Ctx {
            paragraph_role: None,
            indent: None,
            quote_depth: 0,
            in_note: false,
            top_level: false,
            max_image_width: FALLBACK_BLOCK_WIDTH,
            align: None,
        };
        let mut lines: Vec<(Role, String)> = Vec::new();
        if let Some(title) = &metadata.title {
            lines.push((Role::Title, title.clone()));
        }
        if let Some(subtitle) = &metadata.subtitle {
            lines.push((Role::Subtitle, subtitle.clone()));
        }
        for author in &metadata.authors {
            lines.push((Role::Author, author.clone()));
        }
        if let Some(date) = &metadata.date {
            lines.push((Role::Date, date.clone()));
        }
        if let Some(abstract_) = &metadata.abstract_ {
            for paragraph in abstract_
                .split("\n\n")
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                lines.push((Role::Abstract, paragraph.replace('\n', " ")));
            }
        }
        let mut out = Vec::new();
        for (role, text) in lines {
            let props = ParaProps {
                style: self.style(role)?,
                ..ParaProps::default()
            };
            out.push(self.paragraph(&main, scope, &props, &[Inline::Text(text)], ctx)?);
        }
        Ok(out)
    }

    fn toc(&mut self, scope: NodeId, depth: i64, title: &str) -> Result<Vec<NodeId>> {
        let main = self.main.clone();
        let heading_props = ParaProps {
            style: self.style(Role::TocHeading)?,
            ..ParaProps::default()
        };
        let ctx = Ctx {
            paragraph_role: None,
            indent: None,
            quote_depth: 0,
            in_note: false,
            top_level: false,
            max_image_width: FALLBACK_BLOCK_WIDTH,
            align: None,
        };
        let heading = self.paragraph(
            &main,
            scope,
            &heading_props,
            &[Inline::Text(title.to_string())],
            ctx,
        )?;
        // The field's result is built here from the headings, so every
        // reader shows the entries (linked to their headings) without
        // updating fields. Page numbers need layout: the entries carry none,
        // and Word's own update of the field (which asks for no `\n`
        // switch) fills them in; with `toc_page_numbers` Word updates on
        // open after asking.
        let numbers = self.options.toc_page_numbers;
        let instruction = format!(" TOC \\o \"1-{depth}\" \\h \\z \\u ");
        let entries: Vec<(u8, String, String)> = self
            .toc_entries
            .iter()
            // A heading with no text (only whitespace or line breaks) gets
            // no entry: Word's own update leaves it out.
            .filter(|(level, text, _)| i64::from(*level) <= depth && !text.trim().is_empty())
            .cloned()
            .collect();
        let mut out = vec![heading];
        let count = entries.len().max(1);
        let width_twips = self.block_width / EMU_PER_TWIP;
        let mut numbers_out = Vec::new();
        for index in 0..count {
            let entry = entries.get(index);
            let role = Role::toc(entry.map_or(1, |(level, _, _)| *level));
            let props = ParaProps {
                style: self.style(role)?,
                ..ParaProps::default()
            };
            let p = self.new_paragraph(&main, scope, &props)?;
            let tree = self.txn.tree_mut(&main)?;
            if index == 0 {
                let begin = fld_char_run(tree, p, "begin", numbers)?;
                let instr_run = w_el(tree, p, "r")?;
                let instr = w_el(tree, instr_run, "instrText")?;
                tree.set_attr(instr, Some(XML_NS), "space", "preserve")?;
                let text = tree.create_text(instruction.clone());
                tree.insert(instr, 0, text)?;
                append(tree, instr_run, instr)?;
                let separate = fld_char_run(tree, p, "separate", false)?;
                for piece in [begin, instr_run, separate] {
                    append(tree, p, piece)?;
                }
            }
            match entry {
                Some((_, text, bookmark)) => {
                    let link = w_el(tree, p, "hyperlink")?;
                    set_w(tree, link, "anchor", bookmark)?;
                    set_w(tree, link, "history", "1")?;
                    append(tree, p, link)?;
                    let r = w_el(tree, link, "r")?;
                    append(tree, link, r)?;
                    append_run_content(tree, r, text)?;
                    if self.options.toc_numbers {
                        // As Word writes an entry: a dot-leader tab to the
                        // right margin and a PAGEREF field whose result is
                        // the page number (filled in after layout).
                        let ppr = crate::oxml::get_or_add_ppr(tree, p)?;
                        let tabs =
                            crate::oxml::get_or_add(tree, ppr, "tabs", crate::oxml::PPR_SEQ)?;
                        let tab = w_el(tree, tabs, "tab")?;
                        set_w(tree, tab, "val", "right")?;
                        set_w(tree, tab, "leader", "dot")?;
                        set_w(tree, tab, "pos", &(width_twips).to_string())?;
                        append(tree, tabs, tab)?;
                        let tab_run = w_el(tree, link, "r")?;
                        let tab_char = w_el(tree, tab_run, "tab")?;
                        append(tree, tab_run, tab_char)?;
                        append(tree, link, tab_run)?;
                        let begin = fld_char_run(tree, link, "begin", false)?;
                        let instr_run = w_el(tree, link, "r")?;
                        let instr = w_el(tree, instr_run, "instrText")?;
                        tree.set_attr(instr, Some(XML_NS), "space", "preserve")?;
                        let code = tree.create_text(format!(" PAGEREF {bookmark} \\h "));
                        tree.insert(instr, 0, code)?;
                        append(tree, instr_run, instr)?;
                        let separate = fld_char_run(tree, link, "separate", false)?;
                        let number_run = w_el(tree, link, "r")?;
                        let number = add_t(tree, number_run, "1")?;
                        let end = fld_char_run(tree, link, "end", false)?;
                        for piece in [begin, instr_run, separate, number_run, end] {
                            append(tree, link, piece)?;
                        }
                        numbers_out.push((number, bookmark.clone()));
                    }
                }
                None => {
                    let r = w_el(tree, p, "r")?;
                    append(tree, p, r)?;
                    add_t(tree, r, "No headings to list.")?;
                }
            }
            if index + 1 == count {
                let end = fld_char_run(tree, p, "end", false)?;
                append(tree, p, end)?;
            }
            out.push(p);
        }
        if numbers {
            set_update_fields_on_open(self.txn)?;
        }
        self.toc_numbers = numbers_out;
        Ok(out)
    }

    /// Write each contents entry's page number: lay the finished document
    /// out (layout::layout, Word for Mac's line and page breaking from font
    /// metrics), and repeat while the numbers change the contents' own
    /// length.
    fn toc_page_numbers(&mut self, main: &str) -> Result<()> {
        let mut layout = None;
        for _ in 0..3 {
            let estimate = crate::layout::layout(self.txn, main, Vec::new())?;
            let mut changed = false;
            let tree = self.txn.tree_mut(main)?;
            for (number, bookmark) in &self.toc_numbers {
                let shown = estimate
                    .bookmark_page(bookmark)
                    .map_or_else(String::new, |page| estimate.page_number(page).to_string());
                let current = tree.text_content(*number);
                if current != shown {
                    crate::wml::set_leading_text(tree, *number, &shown)?;
                    changed = true;
                }
            }
            layout = Some(estimate);
            if !changed {
                break;
            }
        }
        if let Some(layout) = layout {
            for font in &layout.missing_fonts {
                self.warn(
                    "font_metrics_missing",
                    format!(
                        "no metrics for the font {font:?}; contents page numbers are estimated with a substitute"
                    ),
                    None,
                );
            }
            self.report.toc = Some(super::TocReport {
                entries: self.toc_numbers.len(),
                pages: layout.pages,
                missing_fonts: layout.missing_fonts.clone(),
                embedded_fonts: layout.embedded_fonts.clone(),
            });
        }
        Ok(())
    }

    /// The content after the table of contents starts on a new page: the
    /// first body paragraph gets `pageBreakBefore`, a leading table gets a
    /// page-break paragraph in front, and a body that already opens with a
    /// page break is left alone (no empty page).
    fn break_after_toc(
        &mut self,
        main: &str,
        body_node: NodeId,
        nodes: &mut Vec<NodeId>,
        body_start: usize,
    ) -> Result<()> {
        let first = nodes[body_start];
        let tree = self.txn.tree_mut(main)?;
        if tree.is(first, W, "p") {
            let opens_with_break = tree
                .descendants(first)
                .find(|n| tree.is(*n, W, "br") || tree.is(*n, W, "t"))
                .is_some_and(|n| tree.is(n, W, "br") && w_attr(tree, n, "type") == Some("page"));
            if !opens_with_break {
                let ppr = crate::oxml::get_or_add_ppr(tree, first)?;
                crate::oxml::get_or_add(tree, ppr, "pageBreakBefore", crate::oxml::PPR_SEQ)?;
            }
            return Ok(());
        }
        let props = ParaProps {
            style: self.style(Role::Body)?,
            ..ParaProps::default()
        };
        let p = self.new_paragraph(main, body_node, &props)?;
        let tree = self.txn.tree_mut(main)?;
        let r = w_el(tree, p, "r")?;
        append(tree, p, r)?;
        let br = w_el(tree, r, "br")?;
        set_w(tree, br, "type", "page")?;
        append(tree, r, br)?;
        nodes.insert(body_start, p);
        Ok(())
    }

    /// A centred PAGE field in a new default footer, unless the final
    /// section already has a footer (a template's footer is never changed).
    /// Apply [`MarkdownOptions::cover_page`]: a page break before the
    /// content when the first page stays a cover, or the distinct first page
    /// turned off.
    fn cover_page(
        &mut self,
        main: &str,
        body_node: NodeId,
        blank_first_page: bool,
    ) -> Result<Vec<NodeId>> {
        let (title_pg, first_header, page_height) = {
            let tree = self.txn.tree(main)?;
            let Some(sect_pr) = w_child(tree, body_node, "sectPr") else {
                return Ok(Vec::new());
            };
            let title_pg = w_child(tree, sect_pr, "titlePg")
                .filter(|t| !matches!(w_attr(tree, *t, "val"), Some("0" | "false" | "off")));
            let first_header = w_children(tree, sect_pr, "headerReference")
                .find(|h| w_attr(tree, *h, "type") == Some("first"))
                .and_then(|h| tree.attr(h, Some(R), "id").map(str::to_string));
            let page_height = w_child(tree, sect_pr, "pgSz")
                .and_then(|p| w_attr(tree, p, "h"))
                .and_then(|h| h.parse::<i64>().ok())
                .unwrap_or(15840);
            (title_pg, first_header, page_height)
        };
        let Some(title_pg) = title_pg else {
            return Ok(Vec::new());
        };
        // The tallest drawing in the first-page header, against the page.
        let graphic = match first_header {
            Some(r_id) => {
                let target = self
                    .txn
                    .relationships(main)?
                    .into_iter()
                    .find(|rel| rel.id == r_id && !rel.external)
                    .map(|rel| {
                        rel.target_part_name(crate::package::rels::base_uri(&format!(
                            "/{}",
                            main.trim_start_matches('/')
                        )))
                    })
                    .and_then(|name| self.txn.resolve(&name));
                match target {
                    Some(part) => {
                        let tree = self.txn.tree(&part)?;
                        let tallest = tree
                            .descendants(tree.root())
                            .filter(|node| tree.local_name(*node) == Some("extent"))
                            .filter_map(|node| tree.attr(node, None, "cy"))
                            .filter_map(|cy| cy.parse::<i64>().ok())
                            .max()
                            .unwrap_or(0);
                        tallest * 2 >= page_height * EMU_PER_TWIP
                    }
                    None => false,
                }
            }
            None => false,
        };
        let cover = match self.options.cover_page {
            None => graphic && blank_first_page,
            Some(true) => true,
            Some(false) => {
                if graphic {
                    let tree = self.txn.tree_mut(main)?;
                    crate::authoring::remove(tree, title_pg)?;
                    self.report.cover_page = Some("cover_removed".into());
                }
                false
            }
        };
        if !cover {
            return Ok(Vec::new());
        }
        let props = ParaProps {
            style: self.style(Role::Body)?,
            ..ParaProps::default()
        };
        let p = self.new_paragraph(main, body_node, &props)?;
        let tree = self.txn.tree_mut(main)?;
        let r = w_el(tree, p, "r")?;
        append(tree, p, r)?;
        let br = w_el(tree, r, "br")?;
        set_w(tree, br, "type", "page")?;
        append(tree, r, br)?;
        self.report.cover_page = Some("content_after_cover".into());
        Ok(vec![p])
    }

    fn page_number_footer(&mut self, main: &str, body_node: NodeId) -> Result<()> {
        let sect_pr = {
            let tree = self.txn.tree(main)?;
            match w_child(tree, body_node, "sectPr") {
                Some(sect_pr) if w_child(tree, sect_pr, "footerReference").is_some() => {
                    self.report.page_numbers = Some("template_footer_kept".into());
                    return Ok(());
                }
                Some(sect_pr) => sect_pr,
                None => {
                    self.report.page_numbers = Some("no_section".into());
                    return Ok(());
                }
            }
        };
        let footer_style = {
            let styles = crate::parts::ensure_styles_part(self.txn)?;
            let tree = self.txn.tree(&styles)?;
            w_children(tree, tree.root(), "style")
                .find(|s| {
                    w_child(tree, *s, "name")
                        .and_then(|n| w_attr(tree, n, "val"))
                        .is_some_and(|v| v.eq_ignore_ascii_case("footer"))
                })
                .and_then(|s| w_attr(tree, s, "styleId").map(str::to_string))
        };
        let style = footer_style
            .map(|id| format!("<w:pStyle w:val=\"{}\"/>", super::styles::xml_escape(&id)))
            .unwrap_or_default();
        let xml = format!(
            "<w:ftr xmlns:w=\"{W}\" xmlns:r=\"{R}\"><w:p><w:pPr>{style}<w:jc w:val=\"center\"/></w:pPr>\
             <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGE </w:instrText></w:r>\
             <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p></w:ftr>"
        );
        let tree = crate::xml::parse_str(&xml)
            .map_err(|err| Error::internal("footer_template", err.to_string()))?;
        let name = self.txn.next_partname("word/footer{}.xml");
        self.txn
            .add_xml_part(&name, crate::package::ct::WML_FOOTER, tree)?;
        let r_id = self
            .txn
            .relate_to(main, rt::FOOTER, RelTarget::Part(&name))?;
        let tree = self.txn.tree_mut(main)?;
        let reference = w_el(tree, sect_pr, "footerReference")?;
        set_w(tree, reference, "type", "default")?;
        tree.set_attr(reference, Some(R), "id", &r_id)?;
        // Header/footer references lead the section properties.
        let position = tree
            .children(sect_pr)
            .iter()
            .position(|c| {
                !(tree.is(*c, W, "headerReference") || tree.is(*c, W, "footerReference"))
                    && tree.element(*c).is_some()
            })
            .unwrap_or(tree.children(sect_pr).len());
        tree.insert(sect_pr, position, reference)?;
        self.report.page_numbers = Some("footer_added".into());
        Ok(())
    }

    /// Write the front matter's document properties; returns the ones set.
    fn core_properties(&mut self, metadata: &Metadata) -> Result<Vec<CoreProperty>> {
        let mut changes: Vec<(CoreProperty, String)> = Vec::new();
        if let Some(title) = &metadata.title {
            changes.push((CoreProperty::Title, title.clone()));
        }
        if !metadata.authors.is_empty() {
            changes.push((CoreProperty::Author, metadata.authors.join("; ")));
        }
        if let Some(subject) = metadata.subject.as_ref().or(metadata.subtitle.as_ref()) {
            changes.push((CoreProperty::Subject, subject.clone()));
        }
        if let Some(description) = metadata
            .description
            .as_ref()
            .or(metadata.abstract_.as_ref())
        {
            changes.push((CoreProperty::Comments, description.clone()));
        }
        if !metadata.keywords.is_empty() {
            changes.push((CoreProperty::Keywords, metadata.keywords.join(", ")));
        }
        if let Some(category) = &metadata.category {
            changes.push((CoreProperty::Category, category.clone()));
        }
        if let Some(lang) = &metadata.lang {
            changes.push((CoreProperty::Language, lang.clone()));
        }
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        let part = ensure_core_part(self.txn, self.options.now)?;
        let tree = self.txn.tree_mut(&part)?;
        let mut set = Vec::new();
        for (property, value) in changes {
            write_core(tree, property, &value)?;
            set.push(property);
        }
        Ok(set)
    }

    /// The template's document properties describe the template, often a
    /// finished client document: who wrote it, its title, its statistics,
    /// its SharePoint metadata. None of it may ship in every export. Core
    /// properties lose their people, descriptive text and print date (but
    /// what the front matter `set`), and are stamped as created and
    /// modified now at revision 1; extended properties lose their
    /// statistics, manager, part titles and hyperlink list (company,
    /// template and application stay); custom properties stay only when a
    /// `DOCPROPERTY` field in the kept body, headers, footers or notes
    /// reads them, and the part goes when none is left. Each property
    /// cleared is reported as `core:`/`app:`/`custom:` plus its name.
    fn clear_template_properties(&mut self, main: &str, set: &[CoreProperty]) -> Result<()> {
        let fields = kept_field_instructions(self.txn, main)?;
        let read_property: HashSet<String> = fields
            .iter()
            .filter_map(|instruction| {
                let mut words = field_words(instruction);
                let code = words.next()?;
                code.eq_ignore_ascii_case("DOCPROPERTY")
                    .then(|| words.next())
                    .flatten()
                    .map(|name| name.to_lowercase())
            })
            .collect();
        let now = self
            .options
            .now
            .unwrap_or_else(crate::clock::Timestamp::now);
        let mut cleared = Vec::new();
        if let Some(part) = self.txn.part_related_by("/", rt::CORE_PROPERTIES)? {
            const CLEARED: &[CoreProperty] = &[
                CoreProperty::Author,
                CoreProperty::LastModifiedBy,
                CoreProperty::Title,
                CoreProperty::Subject,
                CoreProperty::Keywords,
                CoreProperty::Comments,
                CoreProperty::Category,
                CoreProperty::LastPrinted,
                CoreProperty::ContentStatus,
            ];
            let tree = self.txn.tree_mut(&part)?;
            let root = tree.root();
            for property in CLEARED.iter().filter(|p| !set.contains(p)) {
                let (ns, local) = property.element();
                let doomed: Vec<NodeId> = tree
                    .child_elements(root)
                    .filter(|node| tree.is(*node, ns, local))
                    .collect();
                if !doomed.is_empty() {
                    cleared.push(format!("core:{local}"));
                }
                for node in doomed {
                    tree.detach(node)?;
                }
            }
            write_core(tree, CoreProperty::Created, &now.to_xml())?;
            write_core(tree, CoreProperty::Modified, &now.to_xml())?;
            write_core(tree, CoreProperty::Revision, "1")?;
        }
        if let Some(part) = self.txn.part_related_by("/", EXTENDED_PROPERTIES)? {
            const CLEARED: &[&str] = &[
                "Pages",
                "Words",
                "Characters",
                "CharactersWithSpaces",
                "Lines",
                "Paragraphs",
                "TotalTime",
                "Manager",
                "HeadingPairs",
                "TitlesOfParts",
                "HLinks",
            ];
            let tree = self.txn.tree_mut(&part)?;
            let root = tree.root();
            let doomed: Vec<(NodeId, String)> = tree
                .child_elements(root)
                .filter_map(|node| {
                    let name = tree.element(node)?.name.local.to_string();
                    CLEARED.contains(&name.as_str()).then_some((node, name))
                })
                .collect();
            for (node, name) in doomed {
                tree.detach(node)?;
                cleared.push(format!("app:{name}"));
            }
        }
        if let Some(part) = self.txn.part_related_by("/", CUSTOM_PROPERTIES)? {
            let tree = self.txn.tree_mut(&part)?;
            let root = tree.root();
            let properties: Vec<(NodeId, String)> = tree
                .child_elements(root)
                .map(|node| {
                    let name = tree.attr(node, None, "name").unwrap_or("").to_string();
                    (node, name)
                })
                .collect();
            let mut kept = 0;
            for (node, name) in properties {
                if read_property.contains(&name.to_lowercase()) {
                    kept += 1;
                    continue;
                }
                tree.detach(node)?;
                cleared.push(format!("custom:{name}"));
            }
            if kept == 0 {
                // Unrelated, the part is then left out with the others
                // nothing reaches.
                let ids: Vec<String> = self
                    .txn
                    .relationships("/")?
                    .into_iter()
                    .filter(|rel| rel.rel_type == CUSTOM_PROPERTIES)
                    .map(|rel| rel.id)
                    .collect();
                for id in ids {
                    crate::revision::drop_rel(self.txn, "/", &id)?;
                }
            }
        }
        // A kept field that shows a cleared property would come up empty
        // once Word updates it.
        let mut warned = HashSet::new();
        for instruction in &fields {
            let mut words = field_words(instruction);
            let Some(code) = words.next().map(|c| c.to_uppercase()) else {
                continue;
            };
            let property = match code.as_str() {
                "DOCPROPERTY" => words.next().map(|name| name.to_lowercase()),
                "AUTHOR" | "TITLE" | "SUBJECT" | "KEYWORDS" | "COMMENTS" | "LASTSAVEDBY" => {
                    Some(code.to_lowercase())
                }
                _ => None,
            };
            let Some(property) = property else {
                continue;
            };
            let cleared_core = match property.as_str() {
                "author" => Some(CoreProperty::Author),
                "lastsavedby" => Some(CoreProperty::LastModifiedBy),
                "title" => Some(CoreProperty::Title),
                "subject" => Some(CoreProperty::Subject),
                "keywords" => Some(CoreProperty::Keywords),
                "comments" => Some(CoreProperty::Comments),
                "category" => Some(CoreProperty::Category),
                _ => None,
            };
            let shows_cleared =
                cleared_core.is_some_and(|p| !set.contains(&p)) || property == "manager";
            if shows_cleared && warned.insert(property.clone()) {
                self.warn(
                    "template_property_cleared",
                    format!(
                        "a {code} field in the template shows a document property that was cleared ({property}); set it in the front matter or the document will show it empty"
                    ),
                    None,
                );
            }
        }
        self.report.cleared_properties = cleared;
        Ok(())
    }

    /// LaTeX math as a Word equation (`m:oMath`, or `m:oMathPara` for
    /// display math).
    fn math(&mut self, part: &str, parent: NodeId, source: &str, display: bool) -> Result<()> {
        let parsed = super::math::parse(source);
        if !parsed.unknown.is_empty() {
            self.warn(
                "math_unknown_command",
                format!(
                    "math commands {} are not supported and are shown as text",
                    parsed.unknown.join(", ")
                ),
                None,
            );
        }
        let tree = self.txn.tree_mut(part)?;
        if display {
            super::math::write_display(tree, parent, &parsed.nodes)?;
        } else {
            super::math::write_math(tree, parent, &parsed.nodes)?;
        }
        self.report.counts.equations += 1;
        Ok(())
    }
}

/// The largest extent a drawing may have (`ST_PositiveCoordinate`).
const MAX_EXTENT: i64 = 27_273_042_316_900;

/// An image reference as the `images` map is matched: percent-decoded once,
/// without a leading `./`.
fn image_key(reference: &str) -> String {
    percent_decode(reference)
        .trim_start_matches("./")
        .to_string()
}

/// An image reference as a warning quotes it (a `data:` URI is not quoted).
fn image_name(url: &str) -> String {
    if url.starts_with("data:") {
        "(a data: image)".into()
    } else {
        url.to_string()
    }
}

/// Why a local image was not read.
#[derive(Debug, PartialEq, Eq)]
enum Unread {
    /// Not a relative path that stays inside the resource directory.
    Outside,
    /// No such file, not a file, or unreadable.
    Missing,
    /// Larger than `max_part_bytes`.
    TooLarge,
}

/// The file `reference` (percent-decoded) names inside `dir`. Absolute
/// paths, drive and UNC prefixes (`C:`, `\\server`, `//server`) and
/// `file:` URLs are refused, as is a `..` that climbs above `dir`, all
/// before the file system is touched, so nothing outside answers whether
/// it exists. What is left is joined onto `dir` and both are canonicalised:
/// the file must lie inside `dir` with symbolic links resolved. A missing
/// file under a link that leads out of `dir` counts as outside, not
/// missing, so the link cannot be used to probe for files elsewhere.
fn resolve_in(dir: &Path, reference: &str) -> std::result::Result<PathBuf, Unread> {
    use std::path::Component;
    let lower = reference.get(..5).map(str::to_ascii_lowercase);
    if reference.starts_with("//")
        || reference.starts_with("\\\\")
        || lower.as_deref() == Some("file:")
    {
        return Err(Unread::Outside);
    }
    let relative = Path::new(reference);
    let mut depth = 0usize;
    for component in relative.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return Err(Unread::Outside),
            Component::ParentDir => depth = depth.checked_sub(1).ok_or(Unread::Outside)?,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
        }
    }
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let root = std::fs::canonicalize(dir).map_err(|_| Unread::Missing)?;
    let joined = root.join(relative);
    let path = match std::fs::canonicalize(&joined) {
        Ok(path) => path,
        // Nothing there. Whether the answer is "missing" or "outside" comes
        // from the nearest ancestor that does exist: past a symbolic link
        // leading out of `dir`, every path is "outside", so the two
        // warnings never tell whether a file outside exists.
        Err(_) => {
            let mut ancestor = joined.parent();
            while let Some(candidate) = ancestor {
                if let Ok(resolved) = std::fs::canonicalize(candidate) {
                    return Err(if resolved.starts_with(&root) {
                        Unread::Missing
                    } else {
                        Unread::Outside
                    });
                }
                ancestor = candidate.parent();
            }
            return Err(Unread::Missing);
        }
    };
    if !path.starts_with(&root) {
        return Err(Unread::Outside);
    }
    Ok(path)
}

/// A regular file's bytes, at most `limit` of them.
fn read_within(path: &Path, limit: u64) -> std::result::Result<Vec<u8>, Unread> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| Unread::Missing)?;
    let metadata = file.metadata().map_err(|_| Unread::Missing)?;
    if !metadata.is_file() {
        return Err(Unread::Missing);
    }
    if metadata.len() > limit {
        return Err(Unread::TooLarge);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| Unread::Missing)?;
    if bytes.len() as u64 > limit {
        return Err(Unread::TooLarge);
    }
    Ok(bytes)
}

/// Column widths in twips filling `total`. The editor's widths in
/// proportion when it sized every column; weights from the text when it
/// sized none (the first `header_rows` rows are headers). In between, a sized column keeps its pixels (a percentage
/// is of the text width) and the others share what is left by their text,
/// none narrower than the editor's minimum column; the whole is scaled
/// down only when the sized columns and those minimums do not fit.
fn column_widths(
    rows: &[&[Vec<Block>]],
    grid: &[Vec<Slot>],
    cols: usize,
    header_rows: usize,
    layout: Option<&TableLayout>,
    total: i64,
) -> Vec<i64> {
    let given: Vec<Option<f64>> = (0..cols)
        .map(|c| layout.and_then(|layout| layout.widths.get(c).copied().flatten()))
        .collect();
    let sized = given.iter().flatten().count();
    let weights: Vec<f64> = if sized == cols {
        given.iter().flatten().map(|w| w.abs()).collect()
    } else if sized == 0 {
        text_weights(rows, grid, cols, header_rows)
    } else {
        let twips = |w: f64| {
            if w < 0.0 {
                total as f64 * -w / 100.0
            } else {
                w * TWIPS_PER_PX
            }
        };
        let text = text_weights(rows, grid, cols, header_rows);
        let floor = MIN_COLUMN_PX * TWIPS_PER_PX;
        let mut widths: Vec<f64> = given.iter().map(|w| w.map_or(0.0, twips)).collect();
        let mut open: Vec<usize> = (0..cols).filter(|c| given[*c].is_none()).collect();
        let mut left = total as f64 - widths.iter().sum::<f64>();
        // A column the sharing would put under the floor takes the floor and
        // leaves the sharing to the others.
        loop {
            let weight: f64 = open.iter().map(|c| text[*c]).sum();
            let short: Vec<usize> = open
                .iter()
                .copied()
                .filter(|c| left * text[*c] / weight < floor)
                .collect();
            if short.is_empty() {
                for c in &open {
                    widths[*c] = left * text[*c] / weight;
                }
                break;
            }
            for c in &short {
                widths[*c] = floor;
            }
            open.retain(|c| !short.contains(c));
            left -= short.len() as f64 * floor;
        }
        widths
    };
    let sum: f64 = weights.iter().sum::<f64>().max(f64::MIN_POSITIVE);
    let mut widths: Vec<i64> = weights
        .iter()
        .map(|w| (total as f64 * w / sum).floor() as i64)
        .collect();
    // The last column takes the rounding so the columns fill the width.
    let drawn: i64 = widths.iter().sum();
    if let Some(last) = widths.last_mut() {
        *last += total - drawn;
    }
    widths
}

/// Each column's weight from its text: long body text wants room, within
/// limits, a header wraps between its words, and no column is narrower
/// than its longest word.
fn text_weights(
    rows: &[&[Vec<Block>]],
    grid: &[Vec<Slot>],
    cols: usize,
    header_rows: usize,
) -> Vec<f64> {
    (0..cols)
        .map(|c| {
            grid.iter()
                .zip(rows)
                .enumerate()
                .flat_map(|(r, (slots, row))| {
                    slots.iter().filter_map(move |slot| match *slot {
                        Slot::Cell {
                            index, col, span, ..
                        } if col == c && span.colspan == 1 => {
                            row.get(index).map(|cell| (r < header_rows, cell))
                        }
                        _ => None,
                    })
                })
                .map(|(header, cell)| {
                    let word = longest_word(cell) + 2;
                    if header {
                        word.max(4)
                    } else {
                        (longest_line(cell) as i64).clamp(4, 40).max(word)
                    }
                })
                .max()
                .unwrap_or(4) as f64
        })
        .collect()
}

/// Length of the longest word in `blocks`' paragraphs.
fn longest_word(blocks: &[Block]) -> i64 {
    let mut longest = 0;
    for block in blocks {
        let text = match unaligned(block).0 {
            Block::Paragraph(inlines) | Block::Plain(inlines) => inline_text(inlines),
            Block::Heading { content, .. } => inline_text(content),
            _ => continue,
        };
        for word in text.split_whitespace() {
            longest = longest.max(word.chars().count() as i64);
        }
    }
    longest
}

/// Length of the longest paragraph of `blocks` (column width weight).
fn longest_line(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(inlines) | Block::Plain(inlines) => {
                inline_text(inlines).chars().count()
            }
            Block::Heading { content, .. } => inline_text(content).chars().count(),
            Block::Code { text, .. } => text.lines().map(|l| l.chars().count()).max().unwrap_or(0),
            Block::Quote { blocks, .. } => longest_line(blocks),
            Block::Aligned(_, block) => longest_line(std::slice::from_ref(block)),
            // List items also take the marker's width.
            Block::List { items, .. } => items
                .iter()
                .map(|item| longest_line(&item.blocks) + 3)
                .max()
                .unwrap_or(0),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// Indent for paragraphs nested in quotes beyond the first level.
fn quote_indent(depth: u8) -> Option<i64> {
    (depth >= 2).then(|| LIST_INDENT * i64::from(depth))
}

/// `Table: caption` / `: caption` (pandoc's table caption syntax): the
/// caption without its marker.
fn table_caption(inlines: &[Inline]) -> Option<Vec<Inline>> {
    let Some(Inline::Text(text)) = inlines.first() else {
        return None;
    };
    let rest = text
        .strip_prefix("Table:")
        .or_else(|| text.strip_prefix(": "))?;
    let mut caption = inlines.to_vec();
    caption[0] = Inline::Text(rest.trim_start().to_string());
    Some(caption)
}

/// A paragraph that holds nothing but one image.
fn lone_image(inlines: &[Inline]) -> Option<&Inline> {
    let meaningful: Vec<&Inline> = inlines
        .iter()
        .filter(|i| match i {
            Inline::Text(text) => !text.trim().is_empty(),
            Inline::SoftBreak | Inline::HardBreak => false,
            _ => true,
        })
        .collect();
    match meaningful.as_slice() {
        [image @ Inline::Image { .. }] => Some(image),
        _ => None,
    }
}

/// Link pieces with the link formatting removed.
fn unlinked(pieces: &[Piece]) -> Vec<Piece> {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(fmt, text) => Piece::Text(
                Fmt {
                    link: false,
                    ..fmt.clone()
                },
                text.clone(),
            ),
            Piece::Break(fmt) => Piece::Break(Fmt {
                link: false,
                ..fmt.clone()
            }),
            other => other.clone(),
        })
        .collect()
}

fn abstract_xml(abstract_id: i64, name: &str) -> String {
    let bullet = name == BULLET_NAME;
    let mut xml = format!(
        "<w:abstractNum xmlns:w=\"{W}\" w:abstractNumId=\"{abstract_id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/><w:name w:val=\"{name}\"/>"
    );
    for level in 0..9i64 {
        let left = LIST_INDENT * (level + 1);
        let (format, text, font) = if bullet {
            match level % 3 {
                0 => ("bullet", "\u{F0B7}".to_string(), Some("Symbol")),
                1 => ("bullet", "o".to_string(), Some("Courier New")),
                _ => ("bullet", "\u{F0A7}".to_string(), Some("Wingdings")),
            }
        } else {
            ("decimal", format!("%{}.", level + 1), None)
        };
        xml.push_str(&format!(
            "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{format}\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{left}\" w:hanging=\"360\"/></w:pPr>"
        ));
        if let Some(font) = font {
            xml.push_str(&format!(
                "<w:rPr><w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:hint=\"default\"/></w:rPr>"
            ));
        }
        xml.push_str("</w:lvl>");
    }
    xml.push_str("</w:abstractNum>");
    xml
}

fn write_ppr(tree: &mut XmlTree, p: NodeId, props: &ParaProps) -> Result<()> {
    let has_any = props.style.is_some()
        || props.keep_next
        || props.numbering.is_some()
        || props.indent_left.is_some()
        || props.align.is_some();
    if !has_any {
        return Ok(());
    }
    let ppr = w_el(tree, p, "pPr")?;
    tree.insert(p, 0, ppr)?;
    let child = |tree: &mut XmlTree, local: &str, attrs: &[(&str, &str)]| -> Result<NodeId> {
        let node = w_el(tree, ppr, local)?;
        for (name, value) in attrs {
            set_w(tree, node, name, value)?;
        }
        append(tree, ppr, node)?;
        Ok(node)
    };
    if let Some(style) = &props.style {
        child(tree, "pStyle", &[("val", style)])?;
    }
    if props.keep_next {
        child(tree, "keepNext", &[])?;
    }
    if let Some((num_id, level)) = props.numbering {
        let num_pr = child(tree, "numPr", &[])?;
        let ilvl = w_el(tree, num_pr, "ilvl")?;
        set_w(tree, ilvl, "val", &level.to_string())?;
        append(tree, num_pr, ilvl)?;
        let num = w_el(tree, num_pr, "numId")?;
        set_w(tree, num, "val", &num_id.to_string())?;
        append(tree, num_pr, num)?;
    }
    if let Some(left) = props.indent_left {
        match props.hanging {
            Some(hanging) => child(
                tree,
                "ind",
                &[
                    ("left", &left.to_string()),
                    ("hanging", &hanging.to_string()),
                ],
            )?,
            None => child(tree, "ind", &[("left", &left.to_string())])?,
        };
    }
    if let Some(align) = props.align {
        child(tree, "jc", &[("val", align)])?;
    }
    Ok(())
}

fn write_rpr(tree: &mut XmlTree, r: NodeId, props: &RunProps) -> Result<()> {
    let has_any = props.style.is_some()
        || props.font.is_some()
        || props.bold
        || props.italic
        || props.strike
        || props.color.is_some()
        || props.size.is_some()
        || props.highlight.is_some()
        || props.underline
        || props.shading.is_some()
        || props.vert_align.is_some();
    if !has_any {
        return Ok(());
    }
    let rpr = w_el(tree, r, "rPr")?;
    tree.insert(r, 0, rpr)?;
    let child = |tree: &mut XmlTree, local: &str, attrs: &[(&str, &str)]| -> Result<()> {
        let node = w_el(tree, rpr, local)?;
        for (name, value) in attrs {
            set_w(tree, node, name, value)?;
        }
        append(tree, rpr, node)
    };
    if let Some(style) = &props.style {
        child(tree, "rStyle", &[("val", style)])?;
    }
    if let Some(font) = &props.font {
        child(
            tree,
            "rFonts",
            &[("ascii", font), ("hAnsi", font), ("cs", font)],
        )?;
    }
    if props.bold {
        child(tree, "b", &[])?;
        child(tree, "bCs", &[])?;
    }
    if props.italic {
        child(tree, "i", &[])?;
        child(tree, "iCs", &[])?;
    }
    if props.strike {
        child(tree, "strike", &[])?;
    }
    if let Some(color) = &props.color {
        child(tree, "color", &[("val", color)])?;
    }
    if let Some(size) = props.size {
        let size = size.to_string();
        child(tree, "sz", &[("val", &size)])?;
        child(tree, "szCs", &[("val", &size)])?;
    }
    if let Some(highlight) = props.highlight {
        child(tree, "highlight", &[("val", highlight)])?;
    }
    if props.underline {
        child(tree, "u", &[("val", "single")])?;
    }
    if let Some(fill) = &props.shading {
        child(
            tree,
            "shd",
            &[("val", "clear"), ("color", "auto"), ("fill", fill)],
        )?;
    }
    if let Some(vert) = props.vert_align {
        child(tree, "vertAlign", &[("val", vert)])?;
    }
    Ok(())
}

/// A run appended to `parent` with `props`.
fn new_run(tree: &mut XmlTree, parent: NodeId, props: &RunProps) -> Result<NodeId> {
    let r = w_el(tree, parent, "r")?;
    append(tree, parent, r)?;
    write_rpr(tree, r, props)?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_section_inherits_earlier_headers_it_lacks() {
        let xml = format!(
            "<w:document xmlns:w=\"{W}\" xmlns:r=\"{R}\"><w:body>\
             <w:p><w:pPr><w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId1\"/>\
             <w:footerReference w:type=\"default\" r:id=\"rId2\"/></w:sectPr></w:pPr></w:p>\
             <w:sectPr><w:footerReference w:type=\"default\" r:id=\"rId3\"/><w:pgSz w:w=\"1\"/></w:sectPr>\
             </w:body></w:document>"
        );
        let mut tree = crate::xml::parse_str(&xml).unwrap();
        let inherited = inherited_references(&tree).unwrap();
        assert_eq!(
            inherited,
            vec![("headerReference", "default".to_string(), "rId1".to_string())]
        );
        let body_node = body(&tree).unwrap();
        clear_except(&mut tree, body_node, "sectPr").unwrap();
        adopt_references(&mut tree, body_node, &inherited).unwrap();
        let out = String::from_utf8(crate::xml::serialize(&tree)).unwrap();
        let header = out.find("headerReference").unwrap();
        assert!(header < out.find("pgSz").unwrap(), "{out}");
        assert!(out.contains("rId3") && !out.contains("rId2"), "{out}");
    }

    #[test]
    fn every_warning_code_has_a_redacted_message() {
        let source = include_str!("render.rs");
        let mut codes: Vec<&str> = Vec::new();
        for (at, _) in source.match_indices("warn(") {
            let rest = &source[at..];
            if let Some(start) = rest.find('"') {
                let code = &rest[start + 1..start + 1 + rest[start + 1..].find('"').unwrap()];
                if code.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !code.is_empty() {
                    codes.push(code);
                }
            }
        }
        assert!(codes.len() >= 9, "{codes:?}");
        codes.extend(["nested_footnote", "undefined_footnote"]);
        for code in codes {
            assert!(
                super::super::WARNING_CODES
                    .iter()
                    .any(|(known, _)| *known == code),
                "{code} has no redacted message"
            );
        }
    }

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  Ünïcode — dash_x "), "ünïcode--dash_x");
    }

    #[test]
    fn bookmark_names_are_word_legal_and_unique() {
        let mut taken = HashSet::new();
        assert_eq!(bookmark_name("1-intro", &taken), "h1_intro");
        taken.insert("intro".to_string());
        assert_eq!(bookmark_name("intro", &taken), "intro_2");
        let long = "a".repeat(60);
        assert_eq!(bookmark_name(&long, &HashSet::new()).len(), 40);
    }

    #[test]
    fn decoding_helpers() {
        assert_eq!(percent_decode("a%20b%2"), "a b%2");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(link_target("me@example.com"), "mailto:me@example.com");
        assert_eq!(link_target("https://x.y/a b"), "https://x.y/a%20b");
    }
}
