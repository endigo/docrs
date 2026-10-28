//! Writing the style collection (python-docx `styles/styles.py`
//! `add_style`, `styles/style.py` setters and `delete`, `styles/latent.py`
//! and `oxml/styles.py`; MIT, see `NOTICE.md`).
//!
//! [`StyleRef`] and [`LatentStyleRef`] are live handles on a `w:style` /
//! `w:lsdException` element of the styles part (document identity, member,
//! node id and freshness evidence, like the authoring handles). Every
//! mutation re-validates its handle, validates its arguments, stages in a
//! [`Txn`] and commits last, so a refusal leaves the document unchanged.
//!
//! Like upstream `document.styles`, the operations that add something
//! ([`Document::add_style`], [`Document::add_latent_style`],
//! [`Document::set_latent_defaults`]) create the default styles part when
//! the document has none, and the latent-style operations add
//! `w:latentStyles` when missing (upstream `get_or_add_latentStyles`).
//! Lookups ([`Document::style`], [`Document::latent_style`]) never create
//! anything; [`Document::ensure_styles_part`] creates the part explicitly.

use crate::authoring::{
    FontChange, FontValues, Handle, Host, ParagraphFormat, ParagraphFormatValues, STYLE_SEQ,
    apply_font, apply_format, read_font, read_format, remove, remove_all, set_on_off,
};
use crate::oxml::{append, get_or_add, insert_before_successors, set_w, successors};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::styles::{
    LatentStyle, Style, StyleType, read_styles, style_element_by_id, style_element_by_name,
    style_elements, style_id_from_name, styles_part, ui_to_internal,
};
use crate::txn::Txn;
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// xmlchemy `_add_<tag>`: a new `w:local` child in `CT_Style` order.
fn add_style_child(tree: &mut XmlTree, style: NodeId, local: &str) -> Result<NodeId> {
    let child = tree.create_element(style, W, local)?;
    insert_before_successors(tree, style, child, successors(STYLE_SEQ, local))?;
    Ok(child)
}

/// `XsdInt` validation (`ST_DecimalNumber`).
fn decimal_number(value: i64, what: &str) -> Result<String> {
    if value < i64::from(i32::MIN) || value > i64::from(i32::MAX) {
        return Err(Error::invalid_argument(
            "value_out_of_range",
            format!(
                "{what} value must be in range {} to {} inclusive, got {value}",
                i32::MIN,
                i32::MAX
            ),
        ));
    }
    Ok(value.to_string())
}

/// `ST_OnOff.convert_to_xml` (`XsdBoolean`): `"1"` / `"0"`.
fn on_off_xml(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn missing_key(what: &str, key: &str) -> Error {
    Error::invalid_argument("missing_key", format!("no {what} with name '{key}'"))
}

fn no_styles_part() -> Error {
    Error::refusal(
        RefusalKind::TargetNotFound,
        "no_styles_part",
        "the document has no styles part; Document::ensure_styles_part creates the default one",
    )
}

/// The styles part inside `txn` (created from the default template, as
/// upstream `DocumentPart._styles_part`).
fn txn_styles_part(txn: &mut Txn<'_>) -> Result<String> {
    let main = txn.document().package().main_part_name().to_string();
    if txn
        .part_related_by(&main, crate::package::rels::rt::STYLES)?
        .is_none()
        && txn.resolve("word/styles.xml").is_none()
    {
        // A stale `Override` for the missing part (Word leaves them behind)
        // would become ambiguous next to the new one; python-docx rewrites
        // the content types from its parts and keeps exactly one.
        txn.drop_content_type_override("word/styles.xml")?;
    }
    crate::parts::ensure_styles_part(txn)
}

/// `CT_Styles.get_or_add_latentStyles` (before the first `w:style`).
fn get_or_add_latent(tree: &mut XmlTree) -> Result<NodeId> {
    let root = tree.root();
    if let Some(existing) = w_child(tree, root, "latentStyles") {
        return Ok(existing);
    }
    let latent = tree.create_element(root, W, "latentStyles")?;
    insert_before_successors(tree, root, latent, &["style"])?;
    Ok(latent)
}

/// Upstream `Styles.__getitem__` element lookup: by internal name, else (the
/// deprecated path) by style id.
fn find_style(tree: &XmlTree, key: &str) -> Option<NodeId> {
    style_element_by_name(tree, ui_to_internal(key)).or_else(|| style_element_by_id(tree, key))
}

/// `w:type` of a style element (`BaseStyle.type`: paragraph when absent).
fn style_type_of(tree: &XmlTree, style: NodeId) -> Result<StyleType> {
    match w_attr(tree, style, "type") {
        None => Ok(StyleType::Paragraph),
        Some(token) => StyleType::from_xml(token),
    }
}

/// Finish `txn` and commit it to `doc`.
macro_rules! commit {
    ($doc:expr, $txn:expr) => {{
        let staged = $txn.finish()?;
        $doc.commit(staged)
    }};
}

/// A live `w:style` handle (upstream `BaseStyle` and subclasses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRef {
    pub(crate) h: Handle,
}

/// One style property assignment (upstream `BaseStyle` / `CharacterStyle`
/// / `ParagraphStyle` setters), applied in order by [`StyleRef::set`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleChange {
    /// `name` (raw `w:name/@w:val`; `None` removes `w:name`).
    Name(Option<String>),
    /// `style_id` (`w:styleId`; `None` removes it).
    StyleId(Option<String>),
    /// `hidden` (`w:semiHidden`).
    Hidden(bool),
    /// `locked` (`w:locked`).
    Locked(bool),
    /// `priority` (`w:uiPriority`; `None` removes it).
    Priority(Option<i64>),
    /// `quick_style` (`w:qFormat`).
    QuickStyle(bool),
    /// `unhide_when_used` (`w:unhideWhenUsed`).
    UnhideWhenUsed(bool),
    /// `base_style`: the style id of the new base style (`style.style_id`
    /// upstream), or `None` to remove `w:basedOn`. Character, paragraph and
    /// table styles only.
    BaseStyle(Option<String>),
    /// `next_paragraph_style`: the style id of the next style, or `None`.
    /// Assigning `None` or the style's own id removes `w:next`. Paragraph
    /// and table styles only.
    NextParagraphStyle(Option<String>),
}

impl StyleRef {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "style", "style")
    }

    /// Package member holding the style.
    pub fn part(&self) -> &str {
        &self.h.part
    }

    /// Snapshot of the style's properties (upstream getters).
    pub fn info(&self, doc: &Document) -> Result<Style> {
        let tree = self.live(doc)?;
        let index = style_elements(tree)
            .position(|node| node == self.h.element)
            .ok_or_else(|| Error::internal("style_index", "live style is not a w:style child"))?;
        let styles = read_styles(tree, Some(self.h.part.clone()))?;
        styles
            .iter()
            .nth(index)
            .cloned()
            .ok_or_else(|| Error::internal("style_index", "style snapshot out of range"))
    }

    /// `style_id` (`w:styleId`).
    pub fn style_id(&self, doc: &Document) -> Result<Option<String>> {
        let tree = self.live(doc)?;
        Ok(w_attr(tree, self.h.element, "styleId").map(str::to_string))
    }

    /// `type` (paragraph when `w:type` is absent).
    pub fn style_type(&self, doc: &Document) -> Result<StyleType> {
        style_type_of(self.live(doc)?, self.h.element)
    }

    /// `base_style` (character, paragraph and table styles): the style
    /// named by `w:basedOn`, if it exists.
    pub fn base_style(&self, doc: &Document) -> Result<Option<StyleRef>> {
        let tree = self.live(doc)?;
        let Some(id) =
            w_child(tree, self.h.element, "basedOn").and_then(|b| w_attr(tree, b, "val"))
        else {
            return Ok(None);
        };
        Ok(style_element_by_id(tree, id).map(|element| StyleRef {
            h: Handle::capture(doc, &self.h.part, tree, element),
        }))
    }

    /// `next_paragraph_style` (paragraph and table styles): the `w:next`
    /// style when it exists and is declared a paragraph style, else this
    /// style.
    pub fn next_paragraph_style(&self, doc: &Document) -> Result<StyleRef> {
        let tree = self.live(doc)?;
        let next = w_child(tree, self.h.element, "next")
            .and_then(|n| w_attr(tree, n, "val"))
            .and_then(|id| style_element_by_id(tree, id))
            .filter(|element| w_attr(tree, *element, "type") == Some("paragraph"));
        Ok(match next {
            Some(element) => StyleRef {
                h: Handle::capture(doc, &self.h.part, tree, element),
            },
            None => self.clone(),
        })
    }

    /// Upstream `BaseStyle.delete()`: the definition is removed (content
    /// using it is left alone). The handle is stale afterwards.
    pub fn delete(&self, doc: &mut Document) -> Result<()> {
        self.live(doc)?;
        refuse_if_protected(doc, "delete a style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        remove(txn.tree_mut(&self.h.part)?, self.h.element)?;
        commit!(doc, txn)
    }

    /// Apply style property assignments in order, atomically.
    pub fn set(&self, doc: &mut Document, changes: &[StyleChange]) -> Result<()> {
        let tree = self.live(doc)?;
        let kind = style_type_of(tree, self.h.element)?;
        for change in changes {
            match change {
                StyleChange::Priority(Some(value)) => {
                    decimal_number(*value, "w:uiPriority")?;
                }
                StyleChange::BaseStyle(_) if kind == StyleType::Numbering => {
                    return Err(unsupported(kind, "base_style"));
                }
                StyleChange::NextParagraphStyle(_)
                    if !matches!(kind, StyleType::Paragraph | StyleType::Table) =>
                {
                    return Err(unsupported(kind, "next_paragraph_style"));
                }
                _ => {}
            }
        }
        refuse_if_protected(doc, "modify a style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        let style = self.h.element;
        for change in changes {
            apply_style_change(tree, style, change)?;
        }
        commit!(doc, txn)
    }

    fn require(&self, doc: &Document, allowed: &[StyleType], what: &str) -> Result<()> {
        let kind = style_type_of(self.live(doc)?, self.h.element)?;
        if allowed.contains(&kind) {
            Ok(())
        } else {
            Err(unsupported(kind, what))
        }
    }

    /// `font` getters (character, paragraph and table styles).
    pub fn font(&self, doc: &Document) -> Result<FontValues> {
        self.require(doc, FONT_TYPES, "font")?;
        read_font(self.live(doc)?, self.h.element)
    }

    /// `font` setters, applied in order and atomically.
    pub fn set_font(&self, doc: &mut Document, changes: &[FontChange]) -> Result<()> {
        self.require(doc, FONT_TYPES, "font")?;
        refuse_if_protected(doc, "modify a style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        for change in changes {
            apply_font(tree, self.h.element, Host::Style, change)?;
        }
        commit!(doc, txn)
    }

    /// `paragraph_format` getters (paragraph and table styles).
    pub fn paragraph_format(&self, doc: &Document) -> Result<ParagraphFormatValues> {
        self.require(doc, PARFMT_TYPES, "paragraph_format")?;
        read_format(self.live(doc)?, self.h.element)
    }

    /// `paragraph_format` setters, applied atomically.
    pub fn set_paragraph_format(&self, doc: &mut Document, format: &ParagraphFormat) -> Result<()> {
        self.require(doc, PARFMT_TYPES, "paragraph_format")?;
        refuse_if_protected(doc, "modify a style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        apply_format(
            txn.tree_mut(&self.h.part)?,
            self.h.element,
            Host::Style,
            format,
        )?;
        commit!(doc, txn)
    }
}

const FONT_TYPES: &[StyleType] = &[StyleType::Character, StyleType::Paragraph, StyleType::Table];
const PARFMT_TYPES: &[StyleType] = &[StyleType::Paragraph, StyleType::Table];

fn unsupported(kind: StyleType, what: &str) -> Error {
    Error::invalid_argument(
        "style_attribute_unsupported",
        format!(
            "a {} style has no {what} (AttributeError upstream)",
            kind.as_xml()
        ),
    )
}

/// One upstream setter on `style`.
fn apply_style_change(tree: &mut XmlTree, style: NodeId, change: &StyleChange) -> Result<()> {
    match change {
        StyleChange::Name(value) => {
            // `name_val`: remove, then add a new `w:name` when not None.
            remove_all(tree, style, "name")?;
            if let Some(name) = value {
                let element = add_style_child(tree, style, "name")?;
                set_w(tree, element, "val", name)?;
            }
            Ok(())
        }
        StyleChange::StyleId(value) => {
            match value {
                None => {
                    tree.remove_attr(style, Some(W), "styleId");
                }
                Some(id) => set_w(tree, style, "styleId", id)?,
            }
            Ok(())
        }
        StyleChange::Hidden(value) => toggle(tree, style, "semiHidden", *value, true),
        StyleChange::Locked(value) => toggle(tree, style, "locked", *value, true),
        StyleChange::UnhideWhenUsed(value) => toggle(tree, style, "unhideWhenUsed", *value, true),
        StyleChange::QuickStyle(value) => toggle(tree, style, "qFormat", *value, false),
        StyleChange::Priority(value) => {
            remove_all(tree, style, "uiPriority")?;
            if let Some(priority) = value {
                let text = decimal_number(*priority, "w:uiPriority")?;
                let element = add_style_child(tree, style, "uiPriority")?;
                set_w(tree, element, "val", &text)?;
            }
            Ok(())
        }
        StyleChange::BaseStyle(value) => match value {
            None => remove_all(tree, style, "basedOn"),
            Some(id) => {
                let element = get_or_add(tree, style, "basedOn", STYLE_SEQ)?;
                set_w(tree, element, "val", id)
            }
        },
        StyleChange::NextParagraphStyle(value) => {
            let own = w_attr(tree, style, "styleId").map(str::to_string);
            match value {
                Some(id) if Some(id) != own.as_ref() => {
                    let element = get_or_add(tree, style, "next", STYLE_SEQ)?;
                    set_w(tree, element, "val", id)
                }
                _ => remove_all(tree, style, "next"),
            }
        }
    }
}

/// `semiHidden_val` / `locked_val` / `unhideWhenUsed_val` / `qFormat_val`
/// setters: remove the child, then add a new one when `value` is true
/// (with `val` set, i.e. no attribute, when `set_val`).
fn toggle(
    tree: &mut XmlTree,
    style: NodeId,
    local: &str,
    value: bool,
    set_val: bool,
) -> Result<()> {
    remove_all(tree, style, local)?;
    if value {
        let element = add_style_child(tree, style, local)?;
        if set_val {
            set_on_off(tree, element, true)?;
        }
    }
    Ok(())
}

impl Document {
    /// The styles part, created from the default template when missing
    /// (what upstream does on any access to `document.styles`). Returns its
    /// member name.
    pub fn ensure_styles_part(&mut self) -> Result<String> {
        if let Some(part) = styles_part(self)? {
            return Ok(part);
        }
        refuse_if_protected(self, "add a styles part", OperationClass::BodyContent)?;
        let mut txn = Txn::new(self);
        let part = txn_styles_part(&mut txn)?;
        commit!(self, txn)?;
        Ok(part)
    }

    /// Upstream `document.styles[key]`: the style whose name is the internal
    /// form of `key` (UI name), else (deprecated upstream) the style with
    /// style id `key`. Refuses `missing_key` (upstream `KeyError`).
    pub fn style(&self, key: &str) -> Result<StyleRef> {
        let part = styles_part(self)?.ok_or_else(no_styles_part)?;
        let tree = self.package().xml(&part)?;
        let element = find_style(tree, key).ok_or_else(|| missing_key("style", key))?;
        Ok(StyleRef {
            h: Handle::capture(self, &part, tree, element),
        })
    }

    /// Every style in document order (`iter(document.styles)`).
    pub fn style_refs(&self) -> Result<Vec<StyleRef>> {
        let Some(part) = styles_part(self)? else {
            return Ok(Vec::new());
        };
        let tree = self.package().xml(&part)?;
        Ok(style_elements(tree)
            .map(|element| StyleRef {
                h: Handle::capture(self, &part, tree, element),
            })
            .collect())
    }

    /// Upstream `Styles.add_style(name, style_type, builtin)`: a new
    /// `w:style` appended with `w:type`, `w:customStyle="1"` unless
    /// `builtin`, the style id python-docx derives from the name, and
    /// `w:name`. Refuses (`style_name_in_use`, upstream `ValueError`) when a
    /// style with that name exists.
    pub fn add_style(
        &mut self,
        name: &str,
        style_type: StyleType,
        builtin: bool,
    ) -> Result<StyleRef> {
        refuse_if_protected(self, "add a style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(self);
        let part = txn_styles_part(&mut txn)?;
        let internal = ui_to_internal(name).to_string();
        {
            let tree = txn.tree(&part)?;
            // `style_name in self` (BabelFish applied again by __contains__).
            let lookup = ui_to_internal(&internal);
            let taken = style_elements(tree).any(|style| {
                w_child(tree, style, "name").and_then(|n| w_attr(tree, n, "val")) == Some(lookup)
            });
            if taken {
                return Err(Error::invalid_argument(
                    "style_name_in_use",
                    format!("document already contains style '{name}'"),
                ));
            }
        }
        let tree = txn.tree_mut(&part)?;
        let root = tree.root();
        let style = tree.create_element(root, W, "style")?;
        append(tree, root, style)?;
        set_w(tree, style, "type", style_type.as_xml())?;
        if !builtin {
            set_w(tree, style, "customStyle", "1")?;
        }
        set_w(tree, style, "styleId", &style_id_from_name(&internal))?;
        let name_element = add_style_child(tree, style, "name")?;
        set_w(tree, name_element, "val", &internal)?;
        let handle = Handle::capture(txn.document(), &part, txn.tree(&part)?, style);
        commit!(self, txn)?;
        Ok(StyleRef { h: handle })
    }

    /// Upstream `latent_styles[key]` (UI name; BabelFish applied).
    pub fn latent_style(&self, key: &str) -> Result<LatentStyleRef> {
        let part = styles_part(self)?.ok_or_else(no_styles_part)?;
        let tree = self.package().xml(&part)?;
        let internal = ui_to_internal(key);
        let element = w_child(tree, tree.root(), "latentStyles")
            .and_then(|latent| {
                w_children(tree, latent, "lsdException")
                    .find(|e| w_attr(tree, *e, "name") == Some(internal))
            })
            .ok_or_else(|| missing_key("latent style", key))?;
        Ok(LatentStyleRef {
            h: Handle::capture(self, &part, tree, element),
        })
    }

    /// Upstream `LatentStyles.add_latent_style(name)`: a new
    /// `w:lsdException` appended with the internal form of `name`.
    pub fn add_latent_style(&mut self, name: &str) -> Result<LatentStyleRef> {
        refuse_if_protected(self, "add a latent style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(self);
        let part = txn_styles_part(&mut txn)?;
        let tree = txn.tree_mut(&part)?;
        let latent = get_or_add_latent(tree)?;
        let exception = tree.create_element(latent, W, "lsdException")?;
        append(tree, latent, exception)?;
        set_w(tree, exception, "name", ui_to_internal(name))?;
        let handle = Handle::capture(txn.document(), &part, txn.tree(&part)?, exception);
        commit!(self, txn)?;
        Ok(LatentStyleRef { h: handle })
    }

    /// Upstream `LatentStyles` default setters, applied in order and
    /// atomically (`w:latentStyles` is added when missing).
    pub fn set_latent_defaults(&mut self, changes: &[LatentDefaultsChange]) -> Result<()> {
        for change in changes {
            if let LatentDefaultsChange::DefaultPriority(Some(value))
            | LatentDefaultsChange::LoadCount(Some(value)) = change
            {
                decimal_number(*value, "latent styles")?;
            }
        }
        refuse_if_protected(self, "modify latent styles", OperationClass::BodyContent)?;
        let mut txn = Txn::new(self);
        let part = txn_styles_part(&mut txn)?;
        let tree = txn.tree_mut(&part)?;
        let latent = get_or_add_latent(tree)?;
        for change in changes {
            let (local, value) = match change {
                LatentDefaultsChange::DefaultPriority(v) => {
                    ("defUIPriority", v.map(|n| n.to_string()))
                }
                LatentDefaultsChange::LoadCount(v) => ("count", v.map(|n| n.to_string())),
                LatentDefaultsChange::DefaultToHidden(v) => {
                    ("defSemiHidden", Some(on_off_xml(*v).to_string()))
                }
                LatentDefaultsChange::DefaultToLocked(v) => {
                    ("defLockedState", Some(on_off_xml(*v).to_string()))
                }
                LatentDefaultsChange::DefaultToQuickStyle(v) => {
                    ("defQFormat", Some(on_off_xml(*v).to_string()))
                }
                LatentDefaultsChange::DefaultToUnhideWhenUsed(v) => {
                    ("defUnhideWhenUsed", Some(on_off_xml(*v).to_string()))
                }
            };
            match value {
                None => {
                    tree.remove_attr(latent, Some(W), local);
                }
                Some(value) => set_w(tree, latent, local, &value)?,
            }
        }
        commit!(self, txn)
    }
}

/// One `LatentStyles` default assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatentDefaultsChange {
    /// `default_priority` (`w:defUIPriority`; `None` removes it).
    DefaultPriority(Option<i64>),
    /// `load_count` (`w:count`; `None` removes it).
    LoadCount(Option<i64>),
    /// `default_to_hidden` (`w:defSemiHidden`, written `1`/`0`).
    DefaultToHidden(bool),
    /// `default_to_locked` (`w:defLockedState`).
    DefaultToLocked(bool),
    /// `default_to_quick_style` (`w:defQFormat`).
    DefaultToQuickStyle(bool),
    /// `default_to_unhide_when_used` (`w:defUnhideWhenUsed`).
    DefaultToUnhideWhenUsed(bool),
}

/// One `_LatentStyle` assignment (tri-state: `None` removes the attribute
/// so the `LatentStyles` default applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatentStyleChange {
    /// `hidden` (`w:semiHidden`).
    Hidden(Option<bool>),
    /// `locked` (`w:locked`).
    Locked(Option<bool>),
    /// `priority` (`w:uiPriority`).
    Priority(Option<i64>),
    /// `quick_style` (`w:qFormat`).
    QuickStyle(Option<bool>),
    /// `unhide_when_used` (`w:unhideWhenUsed`).
    UnhideWhenUsed(Option<bool>),
}

/// A live `w:lsdException` handle (upstream `_LatentStyle`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatentStyleRef {
    pub(crate) h: Handle,
}

impl LatentStyleRef {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "lsdException", "latent style")
    }

    /// Snapshot of the exception (upstream getters).
    pub fn info(&self, doc: &Document) -> Result<LatentStyle> {
        let tree = self.live(doc)?;
        let parent = tree
            .parent(self.h.element)
            .ok_or_else(|| Error::internal("latent_index", "latent style has no parent"))?;
        let index = w_children(tree, parent, "lsdException")
            .position(|node| node == self.h.element)
            .ok_or_else(|| Error::internal("latent_index", "latent style not found"))?;
        let styles = read_styles(tree, Some(self.h.part.clone()))?;
        styles
            .latent_styles()
            .and_then(|latent| latent.exceptions.get(index))
            .cloned()
            .ok_or_else(|| {
                Error::refusal(
                    RefusalKind::UnsupportedStructure,
                    "latent_style_outside_latent_styles",
                    "the w:lsdException is not a child of the styles part's w:latentStyles",
                )
            })
    }

    /// Upstream `_LatentStyle.delete()`. The handle is stale afterwards.
    pub fn delete(&self, doc: &mut Document) -> Result<()> {
        self.live(doc)?;
        refuse_if_protected(doc, "delete a latent style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        remove(txn.tree_mut(&self.h.part)?, self.h.element)?;
        commit!(doc, txn)
    }

    /// Apply assignments in order, atomically.
    pub fn set(&self, doc: &mut Document, changes: &[LatentStyleChange]) -> Result<()> {
        self.live(doc)?;
        let mut rendered = Vec::new();
        for change in changes {
            let (local, value) = match change {
                LatentStyleChange::Hidden(v) => ("semiHidden", v.map(|b| on_off_xml(b).into())),
                LatentStyleChange::Locked(v) => ("locked", v.map(|b| on_off_xml(b).into())),
                LatentStyleChange::QuickStyle(v) => ("qFormat", v.map(|b| on_off_xml(b).into())),
                LatentStyleChange::UnhideWhenUsed(v) => {
                    ("unhideWhenUsed", v.map(|b| on_off_xml(b).into()))
                }
                LatentStyleChange::Priority(v) => (
                    "uiPriority",
                    v.map(|n| decimal_number(n, "w:uiPriority")).transpose()?,
                ),
            };
            rendered.push((local, value));
        }
        refuse_if_protected(doc, "modify a latent style", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        for (local, value) in rendered {
            match value {
                None => {
                    tree.remove_attr(self.h.element, Some(W), local);
                }
                Some(value) => set_w(tree, self.h.element, local, &value)?,
            }
        }
        commit!(doc, txn)
    }
}

/// Every latent-style exception in document order (`iter(latent_styles)`).
pub fn latent_style_refs(doc: &Document) -> Result<Vec<LatentStyleRef>> {
    let Some(part) = styles_part(doc)? else {
        return Ok(Vec::new());
    };
    let tree = doc.package().xml(&part)?;
    let Some(latent) = w_child(tree, tree.root(), "latentStyles") else {
        return Ok(Vec::new());
    };
    Ok(w_children(tree, latent, "lsdException")
        .map(|element| LatentStyleRef {
            h: Handle::capture(doc, &part, tree, element),
        })
        .collect())
}
