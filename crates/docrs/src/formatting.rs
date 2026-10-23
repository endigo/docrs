//! Effective-format resolver — read-only, provenance-bearing (upstream
//! `formatting.py`).
//!
//! "What formatting does this text actually carry": document defaults →
//! paragraph-style inheritance chain → character-style chain → direct
//! formatting. Every [`ResolvedValue`] names the layer it came from; what
//! the resolver does not compute is declared in
//! [`EffectiveFormat::unresolved`] — "unresolved" is a legal answer, a wrong
//! guess is not.
//!
//! Toggle properties (ISO 29500-1 §17.7.3) XOR through the style layers:
//! direct formatting is absolute; otherwise the effective value is the XOR
//! of every layer that specifies *true*. Ordinary properties take the
//! nearest specification. Table-style conditional formatting, numbering-mark
//! properties and theme resolution are declared unresolved, exactly as
//! upstream.
//!
//! Targets map upstream's python-docx proxies onto live Rust handles:
//! `Span` → [`FormatTarget::Span`], `Paragraph` → [`FormatTarget::Paragraph`]
//! (any [`BlockTarget`] addressing one paragraph), `Run` →
//! [`FormatTarget::RunAt`] (the run holding a span's first character).
//! Resolution never mutates the document: without a styles part the bundled
//! default styles are read (upstream would add that part as a side effect).

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::anchor::{self, BlockTarget};
use crate::replace::validate_fresh;
use crate::styles::{default_styles_tree, internal_to_ui, py_int, styles_index, styles_part};
use crate::wml::{W, w_attr, w_child};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result, Span};

/// The twelve OOXML run toggle properties → property keys.
const TOGGLES: &[(&str, &str)] = &[
    ("b", "bold"),
    ("bCs", "bold_cs"),
    ("i", "italic"),
    ("iCs", "italic_cs"),
    ("caps", "caps"),
    ("smallCaps", "small_caps"),
    ("strike", "strike"),
    ("emboss", "emboss"),
    ("imprint", "imprint"),
    ("outline", "outline"),
    ("shadow", "shadow"),
    ("vanish", "vanish"),
];

/// Declared-unresolvable areas (upstream `_UNRESOLVED`, in order).
pub const UNRESOLVED: &[&str] = &[
    "table_style_conditional_formatting",
    "numbering_mark_properties",
    "east_asian_and_complex_script_variants",
    "theme_font_resolution",
    "theme_color_resolution",
    "run_border",
    "run_shading",
    "run_character_spacing",
    "run_character_scaling",
    "run_character_position",
    "run_kerning",
    "run_language",
    "run_proofing",
    "run_additional_text_effects",
    "paragraph_indentation",
    "paragraph_spacing",
    "paragraph_tabs",
    "paragraph_borders",
    "paragraph_shading",
    "paragraph_pagination",
    "paragraph_text_flow",
    "paragraph_frame_properties",
    "section_properties",
];

/// A resolved property value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum FormatValue {
    /// Not specified anywhere (or disagreeing across a span).
    Null,
    /// A toggle.
    Bool(bool),
    /// A token or name (`"single"`, `"0000ff"`, `"theme:minorHAnsi"`, …).
    Text(String),
    /// A number (`size_pt`).
    Number(f64),
}

impl FormatValue {
    /// The boolean, if this is a toggle.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FormatValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The text, if this is a token or name.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FormatValue::Text(value) => Some(value),
            _ => None,
        }
    }

    /// The number, if this is numeric.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            FormatValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// `None` in upstream terms.
    pub fn is_null(&self) -> bool {
        matches!(self, FormatValue::Null)
    }
}

/// One property's effective value and where it came from (upstream
/// `ResolvedValue`). `source` is `direct`, `character_style:<id>`,
/// `paragraph_style:<id>`, `doc_defaults`, `toggle_xor`, `agreeing_layers`,
/// `mixed` or `none`; `chain` holds the contributing layers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedValue {
    /// The value.
    pub value: FormatValue,
    /// Provenance.
    pub source: String,
    /// Contributing layers.
    pub chain: Vec<String>,
}

impl ResolvedValue {
    fn new(value: FormatValue, source: impl Into<String>, chain: Vec<String>) -> Self {
        ResolvedValue {
            value,
            source: source.into(),
            chain,
        }
    }

    fn none() -> Self {
        Self::new(FormatValue::Null, "none", Vec::new())
    }

    /// Upstream `ResolvedValue.to_dict()`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"value": self.value, "source": self.source, "chain": self.chain})
    }
}

/// The resolved property map plus the declared-unresolvable areas
/// (upstream `EffectiveFormat`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveFormat {
    /// Property key → resolution.
    pub properties: BTreeMap<String, ResolvedValue>,
    /// [`UNRESOLVED`].
    pub unresolved: Vec<&'static str>,
}

impl EffectiveFormat {
    /// `format[key]`.
    pub fn get(&self, key: &str) -> Option<&ResolvedValue> {
        self.properties.get(key)
    }

    /// Upstream `value_of(key)`.
    pub fn value_of(&self, key: &str) -> Option<&FormatValue> {
        self.properties.get(key).map(|resolved| &resolved.value)
    }

    /// Upstream `to_dict()` (`schema: paper_effective_format`, `version: 1`).
    pub fn to_json(&self) -> serde_json::Value {
        let properties: serde_json::Map<String, serde_json::Value> = self
            .properties
            .iter()
            .map(|(key, value)| (key.clone(), value.to_json()))
            .collect();
        serde_json::json!({
            "schema": "paper_effective_format",
            "version": 1,
            "properties": properties,
            "unresolved": self.unresolved,
        })
    }
}

impl std::ops::Index<&str> for EffectiveFormat {
    type Output = ResolvedValue;

    fn index(&self, key: &str) -> &ResolvedValue {
        &self.properties[key]
    }
}

/// What [`format_of`] resolves.
#[derive(Debug, Clone, Copy)]
pub enum FormatTarget<'a> {
    /// Every run a live span touches ("mixed" where they disagree).
    Span(&'a Span),
    /// One paragraph: its alignment and style plus the run defaults its
    /// style chain implies.
    Paragraph(BlockTarget<'a>),
    /// The run holding a live span's first character.
    RunAt(&'a Span),
}

struct StyleSource<'t> {
    tree: &'t XmlTree,
    index: HashMap<String, NodeId>,
}

impl StyleSource<'_> {
    fn doc_defaults(&self, default: &str, props: &str) -> Option<NodeId> {
        let root = self.tree.root();
        let defaults = w_child(self.tree, root, "docDefaults")?;
        let holder = w_child(self.tree, defaults, default)?;
        w_child(self.tree, holder, props)
    }

    /// `(styleId, element)` root-first along `w:basedOn`, cycle-guarded.
    fn chain(&self, style_id: Option<&str>) -> Vec<(String, NodeId)> {
        let mut chain: Vec<(String, NodeId)> = Vec::new();
        let mut current = style_id.map(str::to_string);
        while let Some(id) = current.take().filter(|id| !id.is_empty()) {
            let Some(element) = self.index.get(&id).copied() else {
                break;
            };
            if chain.iter().any(|(seen, _)| *seen == id) {
                break;
            }
            current = w_child(self.tree, element, "basedOn")
                .and_then(|based_on| w_attr(self.tree, based_on, "val"))
                .map(str::to_string);
            chain.push((id, element));
        }
        chain.reverse();
        chain
    }

    /// Last default paragraph style (`_default_style_id`): raw `w:type` and
    /// `w:default` in `("1", "true", "on")`.
    fn default_paragraph_style(&self) -> Option<String> {
        let mut last = None;
        for style in crate::styles::style_elements(self.tree) {
            if w_attr(self.tree, style, "type") == Some("paragraph")
                && matches!(
                    w_attr(self.tree, style, "default"),
                    Some("1" | "true" | "on")
                )
            {
                last = Some(w_attr(self.tree, style, "styleId").map(str::to_string));
            }
        }
        last.flatten()
    }
}

type Layers = Vec<(String, Option<NodeId>)>;

/// `w:local` attribute `attr` of the first `w:tag` child of `container`.
fn attr_of<'a>(
    tree: &'a XmlTree,
    container: Option<NodeId>,
    tag: &str,
    attr: &str,
) -> Option<&'a str> {
    let node = w_child(tree, container?, tag)?;
    w_attr(tree, node, attr)
}

fn tri_state(tree: &XmlTree, container: Option<NodeId>, tag: &str) -> Option<bool> {
    let node = w_child(tree, container?, tag)?;
    Some(match w_attr(tree, node, "val") {
        None => true,
        Some(raw) => !matches!(raw.to_lowercase().as_str(), "0" | "false" | "off"),
    })
}

#[derive(Clone, Copy)]
struct Trees<'a> {
    styles: &'a XmlTree,
    story: &'a XmlTree,
}

/// Layer names starting `direct` read from the story tree, others from the
/// styles tree.
fn tree_of<'a>(trees: Trees<'a>, layer: &str) -> &'a XmlTree {
    if layer == "direct" {
        trees.story
    } else {
        trees.styles
    }
}

fn run_layers(
    source: &StyleSource<'_>,
    story: &XmlTree,
    direct_rpr: Option<NodeId>,
    paragraph: Option<NodeId>,
) -> Layers {
    let styles = source.tree;
    let mut layers: Layers = vec![(
        "doc_defaults".to_string(),
        source.doc_defaults("rPrDefault", "rPr"),
    )];
    let mut paragraph_style: Option<String> = None;
    if let Some(paragraph) = paragraph {
        let ppr = w_child(story, paragraph, "pPr");
        paragraph_style = attr_of(story, ppr, "pStyle", "val").map(str::to_string);
    }
    if paragraph_style.is_none() {
        paragraph_style = source.default_paragraph_style();
    }
    for (style_id, style) in source.chain(paragraph_style.as_deref()) {
        layers.push((
            format!("paragraph_style:{style_id}"),
            w_child(styles, style, "rPr"),
        ));
    }
    let character_style = attr_of(story, direct_rpr, "rStyle", "val");
    for (style_id, style) in source.chain(character_style) {
        layers.push((
            format!("character_style:{style_id}"),
            w_child(styles, style, "rPr"),
        ));
    }
    layers.push(("direct".to_string(), direct_rpr));
    layers
}

fn resolve_toggle(trees: Trees<'_>, layers: &Layers, tag: &str) -> ResolvedValue {
    let (direct_name, direct_rpr) = &layers[layers.len() - 1];
    if let Some(direct) = tri_state(tree_of(trees, direct_name), *direct_rpr, tag) {
        return ResolvedValue::new(FormatValue::Bool(direct), "direct", vec!["direct".into()]);
    }
    let mut contributing: Vec<String> = Vec::new();
    let mut value = false;
    for (name, rpr) in &layers[..layers.len() - 1] {
        if tri_state(tree_of(trees, name), *rpr, tag) == Some(true) {
            value = !value;
            contributing.push(name.clone());
        }
    }
    match contributing.len() {
        0 => ResolvedValue::new(FormatValue::Bool(false), "none", Vec::new()),
        1 => ResolvedValue::new(
            FormatValue::Bool(value),
            contributing[0].clone(),
            contributing,
        ),
        _ => ResolvedValue::new(FormatValue::Bool(value), "toggle_xor", contributing),
    }
}

fn resolve_nearest(
    trees: Trees<'_>,
    layers: &Layers,
    reader: impl Fn(&XmlTree, Option<NodeId>) -> Result<Option<FormatValue>>,
) -> Result<ResolvedValue> {
    for (name, props) in layers.iter().rev() {
        if let Some(value) = reader(tree_of(trees, name), *props)? {
            return Ok(ResolvedValue::new(value, name.clone(), vec![name.clone()]));
        }
    }
    Ok(ResolvedValue::none())
}

fn text_reader(
    tag: &'static str,
    attr: &'static str,
) -> impl Fn(&XmlTree, Option<NodeId>) -> Result<Option<FormatValue>> {
    move |tree, props| {
        Ok(attr_of(tree, props, tag, attr).map(|value| FormatValue::Text(value.to_string())))
    }
}

fn size_reader(tree: &XmlTree, props: Option<NodeId>) -> Result<Option<FormatValue>> {
    match attr_of(tree, props, "sz", "val") {
        Some(raw) if !raw.is_empty() => {
            let half_points = py_int(raw).ok_or_else(|| {
                Error::invalid_argument(
                    "invalid_font_size",
                    format!("invalid literal for int() with base 10: {raw:?}"),
                )
            })?;
            Ok(Some(FormatValue::Number(half_points as f64 / 2.0)))
        }
        _ => Ok(None),
    }
}

/// The ascii font; theme references report as `theme:<token>`.
fn font_reader(tree: &XmlTree, props: Option<NodeId>) -> Result<Option<FormatValue>> {
    if let Some(literal) = attr_of(tree, props, "rFonts", "ascii") {
        return Ok(Some(FormatValue::Text(literal.to_string())));
    }
    Ok(attr_of(tree, props, "rFonts", "asciiTheme")
        .map(|theme| FormatValue::Text(format!("theme:{theme}"))))
}

fn resolve_run(
    source: &StyleSource<'_>,
    story: &XmlTree,
    direct_rpr: Option<NodeId>,
    paragraph: Option<NodeId>,
) -> Result<BTreeMap<String, ResolvedValue>> {
    let trees = Trees {
        styles: source.tree,
        story,
    };
    let layers = run_layers(source, story, direct_rpr, paragraph);
    let mut properties = BTreeMap::new();
    for (tag, key) in TOGGLES {
        properties.insert(key.to_string(), resolve_toggle(trees, &layers, tag));
    }
    properties.insert(
        "underline".into(),
        resolve_nearest(trees, &layers, text_reader("u", "val"))?,
    );
    properties.insert(
        "size_pt".into(),
        resolve_nearest(trees, &layers, size_reader)?,
    );
    properties.insert(
        "font_name".into(),
        resolve_nearest(trees, &layers, font_reader)?,
    );
    properties.insert(
        "color_rgb".into(),
        resolve_nearest(trees, &layers, text_reader("color", "val"))?,
    );
    properties.insert(
        "highlight".into(),
        resolve_nearest(trees, &layers, text_reader("highlight", "val"))?,
    );
    properties.insert(
        "vertical_align".into(),
        resolve_nearest(trees, &layers, text_reader("vertAlign", "val"))?,
    );
    if let Some(paragraph) = paragraph {
        properties.extend(paragraph_properties(source, story, paragraph)?);
    }
    Ok(properties)
}

fn paragraph_properties(
    source: &StyleSource<'_>,
    story: &XmlTree,
    paragraph: NodeId,
) -> Result<BTreeMap<String, ResolvedValue>> {
    let styles = source.tree;
    let trees = Trees { styles, story };
    let ppr = w_child(story, paragraph, "pPr");
    let style_id = attr_of(story, ppr, "pStyle", "val")
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| source.default_paragraph_style());
    let mut layers: Layers = vec![(
        "doc_defaults".to_string(),
        source.doc_defaults("pPrDefault", "pPr"),
    )];
    for (chain_id, style) in source.chain(style_id.as_deref()) {
        layers.push((
            format!("paragraph_style:{chain_id}"),
            w_child(styles, style, "pPr"),
        ));
    }
    layers.push(("direct".to_string(), ppr));
    let alignment = resolve_nearest(trees, &layers, text_reader("jc", "val"))?;
    let mut style_name = FormatValue::Null;
    if let Some(element) = style_id.as_deref().and_then(|id| source.index.get(id)) {
        let raw = match w_child(styles, *element, "name") {
            Some(name) => w_attr(styles, name, "val").map(str::to_string),
            None => style_id.clone(),
        };
        if let Some(raw) = raw {
            style_name = FormatValue::Text(internal_to_ui(&raw).to_string());
        }
    }
    let style_source = match style_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!("paragraph_style:{id}"),
        None => "none".to_string(),
    };
    let mut out = BTreeMap::new();
    out.insert("alignment".to_string(), alignment);
    out.insert(
        "style_name".to_string(),
        ResolvedValue::new(style_name, style_source, Vec::new()),
    );
    Ok(out)
}

fn finish(properties: BTreeMap<String, ResolvedValue>) -> EffectiveFormat {
    EffectiveFormat {
        properties,
        unresolved: UNRESOLVED.to_vec(),
    }
}

fn enclosing_paragraph(tree: &XmlTree, node: NodeId) -> Option<NodeId> {
    tree.ancestors(node)
        .find(|ancestor| tree.is(*ancestor, W, "p"))
}

fn require_span_owner(document: &Document, span: &Span) -> Result<()> {
    if span.document_id() != document.id() {
        return Err(Error::refusal(
            RefusalKind::BoundaryViolation,
            "foreign_span",
            "span belongs to a different document; re-find the text in the document passed to this operation",
        ));
    }
    Ok(())
}

fn with_styles<T>(
    document: &Document,
    run: impl FnOnce(&StyleSource<'_>) -> Result<T>,
) -> Result<T> {
    let owned;
    let tree = match styles_part(document)? {
        Some(part) => document.package().xml(&part)?,
        None => {
            owned = default_styles_tree()?;
            &owned
        }
    };
    let source = StyleSource {
        tree,
        index: styles_index(tree),
    };
    run(&source)
}

/// Effective formatting of `target` (upstream `format_of`).
pub fn format_of(document: &Document, target: FormatTarget<'_>) -> Result<EffectiveFormat> {
    match target {
        FormatTarget::Span(span) => resolve_span(document, span),
        FormatTarget::RunAt(span) => {
            require_span_owner(document, span)?;
            let story = document.package().xml(&span.part)?;
            validate_fresh(document, story, span, None)?;
            let Some(run) = span.atoms.first().and_then(|atom| atom.run) else {
                return Err(not_a_run());
            };
            let paragraph = enclosing_paragraph(story, run);
            with_styles(document, |source| {
                resolve_run(source, story, w_child(story, run, "rPr"), paragraph).map(finish)
            })
        }
        FormatTarget::Paragraph(target) => {
            if let BlockTarget::Block(block) = target
                && block.kind == crate::BlockKind::Table
            {
                return Err(Error::invalid_argument(
                    "unsupported_format_target",
                    "format_of resolves Run, Paragraph or Span targets, not Table",
                ));
            }
            let located = anchor::locate_paragraph(document, target)?;
            let story = document.package().xml(&located.part)?;
            with_styles(document, |source| {
                let mut properties = paragraph_properties(source, story, located.element)?;
                // The run defaults implied by the paragraph's own style chain
                // (a probe run without direct properties).
                for (key, value) in resolve_run(source, story, None, Some(located.element))? {
                    properties.entry(key).or_insert(value);
                }
                Ok(finish(properties))
            })
        }
    }
}

fn not_a_run() -> Error {
    Error::refusal(
        RefusalKind::UnsupportedStructure,
        "outside_ordinary_runs",
        "the span includes content outside ordinary runs; resolve a plain text range",
    )
}

fn resolve_span(document: &Document, span: &Span) -> Result<EffectiveFormat> {
    require_span_owner(document, span)?;
    let story = document.package().xml(&span.part)?;
    validate_fresh(document, story, span, None)?;
    let mut runs: Vec<(NodeId, Option<NodeId>)> = Vec::new();
    for atom in &span.atoms {
        let Some(run) = atom.run else {
            return Err(not_a_run());
        };
        if !runs.iter().any(|(existing, _)| *existing == run) {
            runs.push((run, atom.paragraph));
        }
    }
    with_styles(document, |source| {
        let resolved = runs
            .iter()
            .map(|(run, paragraph)| {
                resolve_run(source, story, w_child(story, *run, "rPr"), *paragraph)
            })
            .collect::<Result<Vec<_>>>()?;
        let Some((first, rest)) = resolved.split_first() else {
            return Err(not_a_run());
        };
        if rest.is_empty() {
            return Ok(finish(first.clone()));
        }
        let mut properties = BTreeMap::new();
        for (key, value) in first {
            let agree = rest
                .iter()
                .all(|other| other.get(key).is_some_and(|o| o.value == value.value));
            if agree {
                let mut sources: Vec<String> = Vec::new();
                for item in &resolved {
                    let source = &item[key].source;
                    if !sources.contains(source) {
                        sources.push(source.clone());
                    }
                }
                if sources.len() == 1 {
                    properties.insert(key.clone(), value.clone());
                } else {
                    properties.insert(
                        key.clone(),
                        ResolvedValue::new(value.value.clone(), "agreeing_layers", sources),
                    );
                }
            } else {
                properties.insert(
                    key.clone(),
                    ResolvedValue::new(FormatValue::Null, "mixed", Vec::new()),
                );
            }
        }
        Ok(finish(properties))
    })
}

impl Document {
    /// Effective formatting with provenance (see [`format_of`]).
    pub fn format_of(&self, target: FormatTarget<'_>) -> Result<EffectiveFormat> {
        format_of(self, target)
    }
}
