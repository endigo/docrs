//! Content controls (structured document tags) — upstream `controls.py`.
//!
//! Enumerate a document's controls by tag/alias, read their values and set
//! them type-correctly: text controls get their runs replaced (placeholder
//! state cleared), checkboxes flip `w14:checked` *and* their glyph,
//! dropdowns validate against their `w:listItem` choices, dates set
//! `w:fullDate` alongside the display text. Data-bound controls
//! (`w:dataBinding`) write the custom XML store Word re-syncs on open, then
//! update the visible surface. Every write is staged in one transaction and
//! committed only after every check passed.
//!
//! Adapted from paper-docx (MIT); see `NOTICE.md`.

use serde::Serialize;

use crate::clock::Timestamp;
use crate::document::DocumentId;
use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::validate_writable_text;
use crate::search::Span;
use crate::txn::Txn;
use crate::wml::{
    W, W14, first_choice_children, is_w, leading_text, node_children, set_leading_text, w_attr,
    w_child, w_children,
};
use crate::xml::{NodeData, NodeId, XSI_NS, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

#[path = "p1_xpath.rs"]
mod xpath;

const BLOCK_CONTROL_PARENTS: &[&str] = &[
    "body",
    "tc",
    "hdr",
    "ftr",
    "footnote",
    "endnote",
    "comment",
    "txbxContent",
];
const CHECKED_GLYPH: &str = "\u{2612}";
const UNCHECKED_GLYPH: &str = "\u{2610}";
const CUSTOM_XML_PROPERTIES: &str =
    "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";
const DS_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/customXml";
const MONTH_NAMES: [&str; 13] = [
    "",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

fn unsupported(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::UnsupportedStructure, reason, message)
}

fn not_found(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::TargetNotFound, reason, message)
}

/// Python `repr` of a string (single quotes unless the text holds one).
pub(crate) fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// A value to set (or a value read back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlValue {
    /// Text for text, rich-text, combo, dropdown and date controls.
    Text(String),
    /// Checkbox state.
    Checked(bool),
    /// A calendar date (upstream `datetime.date`): midnight, no zone.
    Date {
        /// Year (1..=9999).
        year: i32,
        /// Month (1..=12).
        month: u32,
        /// Day of month.
        day: u32,
    },
    /// An instant (upstream aware `datetime`), formatted in UTC.
    Instant(Timestamp),
}

impl ControlValue {
    fn python_type(&self) -> &'static str {
        match self {
            ControlValue::Text(_) => "str",
            ControlValue::Checked(_) => "bool",
            ControlValue::Date { .. } => "date",
            ControlValue::Instant(_) => "datetime",
        }
    }
}

impl From<&str> for ControlValue {
    fn from(value: &str) -> Self {
        ControlValue::Text(value.to_string())
    }
}

impl From<String> for ControlValue {
    fn from(value: String) -> Self {
        ControlValue::Text(value)
    }
}

impl From<bool> for ControlValue {
    fn from(value: bool) -> Self {
        ControlValue::Checked(value)
    }
}

impl From<Timestamp> for ControlValue {
    fn from(value: Timestamp) -> Self {
        ControlValue::Instant(value)
    }
}

/// UTC calendar fields of a date stamp (upstream `_utc_date_stamp`).
#[derive(Debug, Clone, Copy)]
struct Stamp {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    /// Monday = 0 (Python `weekday()`).
    weekday: usize,
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

impl Stamp {
    fn from_days_secs(days: i64, secs: i64) -> Stamp {
        let (year, month, day) = civil_from_days(days);
        Stamp {
            year,
            month,
            day,
            hour: (secs / 3600) as u32,
            minute: ((secs % 3600) / 60) as u32,
            second: (secs % 60) as u32,
            // 1970-01-01 was a Thursday (weekday 3).
            weekday: (days + 3).rem_euclid(7) as usize,
        }
    }

    fn of(value: &ControlValue) -> Result<Option<Stamp>> {
        let stamp = match value {
            ControlValue::Date { year, month, day } => {
                let (year, month, day) = (*year as i64, *month, *day);
                let days = days_from_civil(year, month as i64, day as i64);
                if !(1..=12).contains(&month) || civil_from_days(days) != (year, month, day) {
                    return Err(Error::invalid_argument(
                        "invalid_date",
                        format!("{year:04}-{month:02}-{day:02} is not a calendar date"),
                    ));
                }
                Stamp::from_days_secs(days, 0)
            }
            ControlValue::Instant(ts) => {
                let secs = ts.unix();
                Stamp::from_days_secs(secs.div_euclid(86_400), secs.rem_euclid(86_400))
            }
            _ => return Ok(None),
        };
        if !(1..=9999).contains(&stamp.year) {
            return Err(Error::invalid_argument(
                "invalid_date",
                "year is out of range (Python datetime supports 1..9999)",
            ));
        }
        Ok(Some(stamp))
    }

    fn full_date(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// Identity and current state of one content control (upstream
/// `ControlInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlInfo {
    /// `w:tag/@w:val`.
    pub tag: Option<String>,
    /// `w:alias/@w:val`.
    pub alias: Option<String>,
    /// text | rich_text | checkbox | dropdown | combo | date | picture |
    /// group | building_block.
    pub control_type: String,
    /// Current value as text (`"True"`/`"False"` for checkboxes).
    pub value: String,
    /// Story name.
    pub story: String,
    /// `w:showingPlcHdr` is present.
    pub showing_placeholder: bool,
    /// `w:dataBinding` is present.
    pub is_data_bound: bool,
    /// Dropdown/combo display texts.
    pub choices: Vec<String>,
}

impl ControlInfo {
    /// Upstream `to_dict()`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// Upstream `_control_type`.
pub(crate) fn control_type(tree: &XmlTree, sdt_pr: Option<NodeId>) -> &'static str {
    let Some(pr) = sdt_pr else {
        return "rich_text";
    };
    if tree.first_child_named(pr, W14, "checkbox").is_some() {
        return "checkbox";
    }
    for (local, kind) in [
        ("dropDownList", "dropdown"),
        ("comboBox", "combo"),
        ("date", "date"),
        ("picture", "picture"),
        ("docPartObj", "building_block"),
        ("group", "group"),
        ("text", "text"),
    ] {
        if w_child(tree, pr, local).is_some() {
            return kind;
        }
    }
    "rich_text"
}

fn choices(tree: &XmlTree, sdt_pr: Option<NodeId>) -> Vec<String> {
    let Some(pr) = sdt_pr else {
        return Vec::new();
    };
    for holder in ["dropDownList", "comboBox"] {
        if let Some(holder) = w_child(tree, pr, holder) {
            return w_children(tree, holder, "listItem")
                .map(|item| {
                    w_attr(tree, item, "displayText")
                        .filter(|v| !v.is_empty())
                        .or_else(|| w_attr(tree, item, "value").filter(|v| !v.is_empty()))
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
        }
    }
    Vec::new()
}

fn content_text(tree: &XmlTree, sdt: NodeId) -> String {
    let Some(content) = w_child(tree, sdt, "sdtContent") else {
        return String::new();
    };
    tree.descendants(content)
        .filter(|node| is_w(tree, *node, "t"))
        .map(|node| leading_text(tree, node))
        .collect()
}

fn checkbox_checked(tree: &XmlTree, sdt_pr: Option<NodeId>) -> bool {
    sdt_pr
        .and_then(|pr| tree.first_child_named(pr, W14, "checkbox"))
        .and_then(|checkbox| tree.first_child_named(checkbox, W14, "checked"))
        .and_then(|checked| tree.attr(checked, Some(W14), "val"))
        .is_some_and(|value| matches!(value, "1" | "true"))
}

/// Upstream `_iter_sdts`: pre-order over first-choice children.
fn iter_sdts(tree: &XmlTree, element: NodeId, out: &mut Vec<NodeId>) {
    for child in first_choice_children(tree, element) {
        if is_w(tree, child, "sdt") {
            out.push(child);
        }
        iter_sdts(tree, child, out);
    }
}

/// A live content-control handle (upstream `Control`).
///
/// It names one `w:sdt` element of one document; every read and write checks
/// that the handle belongs to the document passed in. Writes additionally
/// require the control to still be in the document's active control
/// traversal.
#[derive(Debug, Clone)]
pub struct Control {
    document: DocumentId,
    story: String,
    part: String,
    sdt: NodeId,
    incarnation: u32,
}

impl Control {
    fn tree<'d>(&self, document: &'d Document) -> Result<&'d XmlTree> {
        if document.id() != self.document {
            return Err(Error::refusal(
                RefusalKind::BoundaryViolation,
                "foreign_control",
                "control belongs to a different document; reacquire it from the document that now contains it",
            ));
        }
        let tree = document
            .package()
            .part(&self.part)
            .map(|_| document.package().xml(&self.part))
            .transpose()?
            .ok_or_else(|| {
                not_found(
                    "stale_control",
                    "control is stale: it was removed from its document",
                )
            })?;
        if !tree.contains(self.sdt) || tree.incarnation() != self.incarnation {
            return Err(not_found(
                "stale_control",
                "control is stale: it was removed from its document",
            ));
        }
        Ok(tree)
    }

    fn sdt_pr(&self, tree: &XmlTree) -> Option<NodeId> {
        w_child(tree, self.sdt, "sdtPr")
    }

    fn pr_value(&self, document: &Document, local: &str) -> Result<Option<String>> {
        let tree = self.tree(document)?;
        Ok(self
            .sdt_pr(tree)
            .and_then(|pr| w_child(tree, pr, local))
            .and_then(|node| w_attr(tree, node, "val"))
            .map(str::to_string))
    }

    /// Story name.
    pub fn story(&self) -> &str {
        &self.story
    }

    /// `w:tag/@w:val`.
    pub fn tag(&self, document: &Document) -> Result<Option<String>> {
        self.pr_value(document, "tag")
    }

    /// `w:alias/@w:val`.
    pub fn alias(&self, document: &Document) -> Result<Option<String>> {
        self.pr_value(document, "alias")
    }

    /// Control type token.
    pub fn control_type(&self, document: &Document) -> Result<&'static str> {
        let tree = self.tree(document)?;
        Ok(control_type(tree, self.sdt_pr(tree)))
    }

    /// `w:dataBinding` is present.
    pub fn is_data_bound(&self, document: &Document) -> Result<bool> {
        let tree = self.tree(document)?;
        Ok(self
            .sdt_pr(tree)
            .is_some_and(|pr| w_child(tree, pr, "dataBinding").is_some()))
    }

    /// `w:showingPlcHdr` is present.
    pub fn showing_placeholder(&self, document: &Document) -> Result<bool> {
        let tree = self.tree(document)?;
        Ok(self
            .sdt_pr(tree)
            .is_some_and(|pr| w_child(tree, pr, "showingPlcHdr").is_some()))
    }

    /// Current value: [`ControlValue::Checked`] for checkboxes,
    /// [`ControlValue::Text`] otherwise.
    pub fn value(&self, document: &Document) -> Result<ControlValue> {
        let tree = self.tree(document)?;
        let sdt_pr = self.sdt_pr(tree);
        Ok(if control_type(tree, sdt_pr) == "checkbox" {
            ControlValue::Checked(checkbox_checked(tree, sdt_pr))
        } else {
            ControlValue::Text(content_text(tree, self.sdt))
        })
    }

    /// Identity and state.
    pub fn info(&self, document: &Document) -> Result<ControlInfo> {
        let tree = self.tree(document)?;
        let sdt_pr = self.sdt_pr(tree);
        let kind = control_type(tree, sdt_pr);
        let value = if kind == "checkbox" {
            if checkbox_checked(tree, sdt_pr) {
                "True".to_string()
            } else {
                "False".to_string()
            }
        } else {
            content_text(tree, self.sdt)
        };
        let pr_value = |local: &str| {
            sdt_pr
                .and_then(|pr| w_child(tree, pr, local))
                .and_then(|node| w_attr(tree, node, "val"))
                .map(str::to_string)
        };
        Ok(ControlInfo {
            tag: pr_value("tag"),
            alias: pr_value("alias"),
            control_type: kind.to_string(),
            value,
            story: self.story.clone(),
            showing_placeholder: sdt_pr
                .is_some_and(|pr| w_child(tree, pr, "showingPlcHdr").is_some()),
            is_data_bound: sdt_pr.is_some_and(|pr| w_child(tree, pr, "dataBinding").is_some()),
            choices: choices(tree, sdt_pr),
        })
    }

    /// Upstream `_validate_ownership`.
    fn validate_ownership(&self, document: &Document) -> Result<()> {
        let tree = self.tree(document)?;
        for story in crate::story::stories(document) {
            if story.part != self.part {
                continue;
            }
            let story_tree = document.package().xml(&story.part)?;
            let mut sdts = Vec::new();
            iter_sdts(story_tree, story_tree.root(), &mut sdts);
            if sdts.contains(&self.sdt) {
                return Ok(());
            }
        }
        if tree.is_attached(self.sdt) {
            return Err(not_found(
                "stale_control",
                "control is stale: it is no longer in the document's active content-control traversal",
            ));
        }
        Err(not_found(
            "stale_control",
            "control is stale: it was removed from its document",
        ))
    }

    /// Set the control's value, validating fully before anything changes
    /// (upstream `Control.set_value`).
    ///
    /// Text and rich-text controls take text, checkboxes a bool, dropdowns
    /// one of their choices, dates a date/instant (or display text). Refuses
    /// a protected document, a stale or foreign control, a locked or
    /// data-bound surface, picture/group/building-block controls, a dropdown
    /// value outside its choices and an unsupported date format.
    pub fn set_value(&self, document: &mut Document, value: impl Into<ControlValue>) -> Result<()> {
        let value = value.into();
        let staged = {
            let doc: &Document = document;
            self.validate_ownership(doc)?;
            refuse_if_protected(doc, "set a control value", OperationClass::FormFieldValue)?;
            self.plan(doc, &value)?
        };
        document.commit(staged)
    }

    fn plan(&self, doc: &Document, value: &ControlValue) -> Result<crate::txn::Staged> {
        let tree = doc.package().xml(&self.part)?;
        let sdt = self.sdt;
        let sdt_pr = self.sdt_pr(tree);
        let kind = control_type(tree, sdt_pr);
        let mut txn = Txn::new(doc);
        let binding = sdt_pr.and_then(|pr| w_child(tree, pr, "dataBinding"));
        if let Some(binding) = binding {
            if kind != "text" && kind != "rich_text" {
                return Err(unsupported(
                    "data_bound_typed_control",
                    format!("data-bound {kind} controls are not settable; nothing was changed"),
                ));
            }
            let ControlValue::Text(text) = value else {
                return Err(Error::invalid_argument(
                    "control_value_type",
                    "data-bound controls take a string value",
                ));
            };
            validate_writable_text(text, "value")?;
            refuse_locked_control_content(tree, sdt)?;
            write_bound_store(&mut txn, tree, binding, text)?;
            set_text(txn.tree_mut(&self.part)?, sdt, text, None)?;
            return txn.finish();
        }
        refuse_control_write_restrictions(tree, sdt)?;
        if kind == "checkbox" {
            let ControlValue::Checked(checked) = value else {
                return Err(Error::invalid_argument(
                    "control_value_type",
                    "checkbox controls take a bool value",
                ));
            };
            let checkbox = sdt_pr
                .and_then(|pr| tree.first_child_named(pr, W14, "checkbox"))
                .ok_or_else(|| {
                    unsupported(
                        "checkbox_without_properties",
                        "checkbox control has no w14:checkbox properties; nothing was changed",
                    )
                })?;
            let (glyph, font) = declared_checkbox_state(tree, checkbox, *checked)?;
            validate_writable_text(&glyph, "declared checkbox glyph")?;
            let staged = txn.tree_mut(&self.part)?;
            set_text(staged, sdt, &glyph, font.as_deref())?;
            set_checkbox_state(staged, checkbox, *checked)?;
            return txn.finish();
        }
        if matches!(kind, "picture" | "group" | "building_block") {
            return Err(unsupported(
                "unsettable_control",
                format!("{kind} controls are not settable"),
            ));
        }
        if matches!(value, ControlValue::Checked(_)) {
            return Err(Error::invalid_argument(
                "control_value_type",
                format!("{kind} controls take text, not bool"),
            ));
        }
        if kind == "date"
            && let Some(stamp) = Stamp::of(value)?
        {
            let date_pr = sdt_pr
                .and_then(|pr| w_child(tree, pr, "date"))
                .ok_or_else(|| {
                    unsupported(
                        "date_without_properties",
                        "date control has no w:date properties; nothing was changed",
                    )
                })?;
            let display = format_declared_date(tree, date_pr, &stamp)?;
            validate_writable_text(&display, "formatted date value")?;
            let staged = txn.tree_mut(&self.part)?;
            set_text(staged, sdt, &display, None)?;
            staged.set_attr(date_pr, Some(W), "fullDate", &stamp.full_date())?;
            return txn.finish();
        }
        let ControlValue::Text(text) = value else {
            return Err(Error::invalid_argument(
                "control_value_type",
                format!(
                    "{kind} controls take a string value (got {})",
                    value.python_type()
                ),
            ));
        };
        validate_writable_text(text, "value")?;
        if kind == "dropdown" {
            let choices = choices(tree, sdt_pr);
            if !choices.contains(text) {
                let listed: Vec<String> = choices.iter().map(|c| py_repr(c)).collect();
                return Err(not_found(
                    "not_a_choice",
                    format!(
                        "{} is not one of this dropdown's choices [{}]",
                        py_repr(text),
                        listed.join(", ")
                    ),
                ));
            }
        }
        let staged = txn.tree_mut(&self.part)?;
        set_text(staged, sdt, text, None)?;
        if kind == "date"
            && let Some(date_pr) = sdt_pr.and_then(|pr| w_child(staged, pr, "date"))
            && w_attr(staged, date_pr, "fullDate").is_some_and(|v| !v.is_empty())
        {
            staged.remove_attr(date_pr, Some(W), "fullDate");
        }
        txn.finish()
    }
}

/// Every content control in every story, document order (upstream
/// `iter_controls`).
pub fn iter_controls(document: &Document) -> Result<Vec<Control>> {
    let mut out = Vec::new();
    for story in crate::story::stories(document) {
        let tree = document.package().xml(&story.part)?;
        let mut sdts = Vec::new();
        iter_sdts(tree, tree.root(), &mut sdts);
        out.extend(sdts.into_iter().map(|sdt| Control {
            document: document.id(),
            story: story.name.clone(),
            part: story.part.clone(),
            sdt,
            incarnation: tree.incarnation(),
        }));
    }
    Ok(out)
}

/// Identity and state of every control (upstream `list_controls`).
pub fn list_controls(document: &Document) -> Result<Vec<ControlInfo>> {
    iter_controls(document)?
        .iter()
        .map(|control| control.info(document))
        .collect()
}

/// The single control matching `tag` and/or `alias` (upstream
/// `get_control`).
pub fn get_control(document: &Document, tag: Option<&str>, alias: Option<&str>) -> Result<Control> {
    if tag.is_none() && alias.is_none() {
        return Err(Error::invalid_argument(
            "missing_selector",
            "provide tag= and/or alias=",
        ));
    }
    let mut matches = Vec::new();
    for control in iter_controls(document)? {
        let info_tag = control.tag(document)?;
        let info_alias = control.alias(document)?;
        if tag.is_none_or(|t| info_tag.as_deref() == Some(t))
            && alias.is_none_or(|a| info_alias.as_deref() == Some(a))
        {
            matches.push(control);
        }
    }
    let wanted = match (tag, alias) {
        (Some(tag), _) => format!("tag={}", py_repr(tag)),
        (None, Some(alias)) => format!("alias={}", py_repr(alias)),
        (None, None) => String::new(),
    };
    match matches.len() {
        0 => Err(not_found(
            "no_control",
            format!("no content control with {wanted}"),
        )),
        1 => Ok(matches.remove(0)),
        count => Err(Error::refusal(
            RefusalKind::AmbiguousTarget,
            "multiple_controls",
            format!(
                "{count} content controls match {wanted}; give controls unique tags in the template"
            ),
        )),
    }
}

/// Set the value of the control `tag`/`alias` names (upstream
/// `set_control_value`).
pub fn set_control_value(
    document: &mut Document,
    value: impl Into<ControlValue>,
    tag: Option<&str>,
    alias: Option<&str>,
) -> Result<()> {
    let control = get_control(document, tag, alias)?;
    control.set_value(document, value)
}

impl Document {
    /// Every content control (see [`iter_controls`]).
    pub fn controls(&self) -> Result<Vec<Control>> {
        iter_controls(self)
    }

    /// Identity and state of every control (see [`list_controls`]).
    pub fn list_controls(&self) -> Result<Vec<ControlInfo>> {
        list_controls(self)
    }

    /// The single control matching `tag` and/or `alias` (see
    /// [`get_control`]).
    pub fn control(&self, tag: Option<&str>, alias: Option<&str>) -> Result<Control> {
        get_control(self, tag, alias)
    }

    /// Set a control's value by tag/alias (see [`set_control_value`]).
    pub fn set_control_value(
        &mut self,
        value: impl Into<ControlValue>,
        tag: Option<&str>,
        alias: Option<&str>,
    ) -> Result<()> {
        set_control_value(self, value, tag, alias)
    }
}

/// Upstream `_validate_span_surface_edit`.
pub(crate) fn validate_span_surface_edit(tree: &XmlTree, sdt: NodeId) -> Result<()> {
    let sdt_pr = w_child(tree, sdt, "sdtPr");
    if sdt_pr.is_some_and(|pr| w_child(tree, pr, "dataBinding").is_some()) {
        return Err(unsupported(
            "data_bound_control",
            "span lies in a data-bound control whose surface text Word re-syncs from custom XML; use the binding source instead",
        ));
    }
    let kind = control_type(tree, sdt_pr);
    if kind != "text" && kind != "rich_text" {
        return Err(unsupported(
            "typed_control",
            format!(
                "span lies in a {kind} control whose display text is tied to machine-readable control state; use Control.set_value()"
            ),
        ));
    }
    Ok(())
}

/// Upstream `_refuse_locked_control_content`.
pub(crate) fn refuse_locked_control_content(tree: &XmlTree, sdt: NodeId) -> Result<()> {
    let Some(pr) = w_child(tree, sdt, "sdtPr") else {
        return Ok(());
    };
    let locks: Vec<NodeId> = w_children(tree, pr, "lock").collect();
    if locks.len() > 1 {
        return Err(unsupported(
            "multiple_locks",
            "control has multiple w:lock declarations; nothing was changed",
        ));
    }
    if locks.first().is_some_and(|lock| {
        matches!(
            w_attr(tree, *lock, "val"),
            Some("contentLocked" | "sdtContentLocked")
        )
    }) {
        return Err(unsupported(
            "locked_control",
            "control content is locked; nothing was changed",
        ));
    }
    Ok(())
}

/// Upstream `_refuse_control_write_restrictions`.
pub(crate) fn refuse_control_write_restrictions(tree: &XmlTree, sdt: NodeId) -> Result<()> {
    if w_child(tree, sdt, "sdtPr").is_some_and(|pr| w_child(tree, pr, "dataBinding").is_some()) {
        return Err(unsupported(
            "data_bound_control",
            "control is data-bound; edits could be discarded on sync",
        ));
    }
    refuse_locked_control_content(tree, sdt)
}

/// The `_refuse_control_surface` check of `links.py` and `notes.py`: every
/// control around the span must be an unlocked, unbound rich-text surface.
/// `what` names the inserted markup in the plain-text refusal.
pub(crate) fn refuse_control_surface(tree: &XmlTree, span: &Span, what: &str) -> Result<()> {
    let mut controls: Vec<NodeId> = Vec::new();
    for atom in &span.atoms {
        for ancestor in tree.ancestors(atom.element) {
            if is_w(tree, ancestor, "sdt") && !controls.contains(&ancestor) {
                controls.push(ancestor);
            }
        }
    }
    for control in controls {
        validate_span_surface_edit(tree, control)?;
        refuse_control_write_restrictions(tree, control)?;
        if control_type(tree, w_child(tree, control, "sdtPr")) == "text" {
            return Err(unsupported(
                "plain_text_control",
                format!(
                    "span lies in a plain-text content control; {what} would break the control's content model. Nothing was changed"
                ),
            ));
        }
    }
    Ok(())
}

/// Python `int(raw, 16)`: optional sign, optional `0x`, `_` separators,
/// surrounding whitespace.
fn py_int_hex(raw: &str) -> Option<i64> {
    let trimmed = crate::unicode::py_strip(raw);
    let (negative, body) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let body = body
        .strip_prefix("0x")
        .or_else(|| body.strip_prefix("0X"))
        .map(|rest| rest.strip_prefix('_').unwrap_or(rest))
        .unwrap_or(body);
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut value: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let digit = c.to_digit(16)? as i64;
        value = value.checked_mul(16)?.checked_add(digit)?;
    }
    Some(if negative { -value } else { value })
}

/// Upstream `_declared_checkbox_state`: the declared glyph and font.
fn declared_checkbox_state(
    tree: &XmlTree,
    checkbox: NodeId,
    checked: bool,
) -> Result<(String, Option<String>)> {
    let (local, fallback) = if checked {
        ("checkedState", CHECKED_GLYPH)
    } else {
        ("uncheckedState", UNCHECKED_GLYPH)
    };
    let Some(state) = tree.first_child_named(checkbox, W14, local) else {
        return Ok((fallback.to_string(), None));
    };
    let raw = tree.attr(state, Some(W14), "val");
    let glyph = raw
        .and_then(py_int_hex)
        .filter(|cp| !(0xD800..=0xDFFF).contains(cp))
        .and_then(|cp| u32::try_from(cp).ok())
        .and_then(char::from_u32);
    let Some(glyph) = glyph else {
        let shown = raw.map(py_repr).unwrap_or_else(|| "None".into());
        return Err(unsupported(
            "invalid_checkbox_glyph",
            format!("checkbox declares invalid hexadecimal glyph {shown}; nothing was changed"),
        ));
    };
    let font = tree
        .attr(state, Some(W14), "font")
        .filter(|f| !f.is_empty())
        .map(str::to_string);
    Ok((glyph.to_string(), font))
}

fn set_checkbox_state(tree: &mut XmlTree, checkbox: NodeId, checked: bool) -> Result<()> {
    let state = match tree.first_child_named(checkbox, W14, "checked") {
        Some(state) => state,
        None => {
            let state = tree.create_element(checkbox, W14, "checked")?;
            tree.insert(checkbox, 0, state)?;
            state
        }
    };
    tree.set_attr(state, Some(W14), "val", if checked { "1" } else { "0" })
}

/// Upstream `_format_declared_date`.
fn format_declared_date(tree: &XmlTree, date_pr: NodeId, stamp: &Stamp) -> Result<String> {
    let calendar = w_child(tree, date_pr, "calendar").and_then(|c| w_attr(tree, c, "val"));
    if calendar.is_some_and(|c| c != "gregorian") {
        return Err(unsupported(
            "unsupported_calendar",
            format!(
                "date control declares unsupported calendar {}; nothing was changed",
                py_repr(calendar.unwrap_or(""))
            ),
        ));
    }
    let Some(format_elm) = w_child(tree, date_pr, "dateFormat") else {
        return Err(unsupported(
            "missing_date_format",
            "date control has no w:dateFormat; nothing was changed",
        ));
    };
    let pattern = w_attr(tree, format_elm, "val").unwrap_or("");
    if pattern.is_empty() {
        return Err(unsupported(
            "empty_date_format",
            "date control declares an empty w:dateFormat; nothing was changed",
        ));
    }
    let language = w_child(tree, date_pr, "lid").and_then(|l| w_attr(tree, l, "val"));
    let chars: Vec<char> = pattern.chars().collect();
    let mut pieces = String::new();
    let mut uses_names = false;
    let mut index = 0;
    let bad = |detail: String| {
        unsupported(
            "unsupported_date_format",
            format!(
                "unsupported date format {}{detail}; nothing was changed",
                py_repr(pattern)
            ),
        )
    };
    while index < chars.len() {
        let c = chars[index];
        if c == '\'' || c == '"' {
            index += 1;
            let start = index;
            while index < chars.len() && chars[index] != c {
                index += 1;
            }
            if index >= chars.len() {
                return Err(bad(": unterminated literal".into()));
            }
            pieces.extend(&chars[start..index]);
            index += 1;
            continue;
        }
        if c == '\\' {
            if index + 1 >= chars.len() {
                return Err(bad(": trailing escape".into()));
            }
            pieces.push(chars[index + 1]);
            index += 2;
            continue;
        }
        if !crate::bookmarks::pyclass::is_py_alpha(c) {
            pieces.push(c);
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < chars.len() && chars[end] == c {
            end += 1;
        }
        let token: String = chars[index..end].iter().collect();
        let hour_12 = if stamp.hour.is_multiple_of(12) {
            12
        } else {
            stamp.hour % 12
        };
        let replacement = match token.as_str() {
            "d" => stamp.day.to_string(),
            "dd" => format!("{:02}", stamp.day),
            "ddd" => WEEKDAY_NAMES[stamp.weekday][..3].to_string(),
            "dddd" => WEEKDAY_NAMES[stamp.weekday].to_string(),
            "M" => stamp.month.to_string(),
            "MM" => format!("{:02}", stamp.month),
            "MMM" => MONTH_NAMES[stamp.month as usize][..3].to_string(),
            "MMMM" => MONTH_NAMES[stamp.month as usize].to_string(),
            "y" => stamp.year.to_string(),
            "yy" => format!("{:02}", stamp.year % 100),
            "yyyy" => format!("{:04}", stamp.year),
            "H" => stamp.hour.to_string(),
            "HH" => format!("{:02}", stamp.hour),
            "h" => hour_12.to_string(),
            "hh" => format!("{hour_12:02}"),
            "m" => stamp.minute.to_string(),
            "mm" => format!("{:02}", stamp.minute),
            "s" => stamp.second.to_string(),
            "ss" => format!("{:02}", stamp.second),
            "t" => if stamp.hour < 12 { "A" } else { "P" }.to_string(),
            "tt" => if stamp.hour < 12 { "AM" } else { "PM" }.to_string(),
            _ => return Err(bad(format!(" (token {})", py_repr(&token)))),
        };
        uses_names =
            uses_names || matches!(token.as_str(), "ddd" | "dddd" | "MMM" | "MMMM" | "t" | "tt");
        pieces.push_str(&replacement);
        index = end;
    }
    if uses_names {
        let Some(language) = language else {
            return Err(unsupported(
                "date_names_without_locale",
                format!(
                    "date format {} uses locale-sensitive names, but the date control has no w:lid; nothing was changed",
                    py_repr(pattern)
                ),
            ));
        };
        let normalized = language.to_lowercase();
        if normalized != "en" && !normalized.starts_with("en-") {
            return Err(unsupported(
                "unsupported_date_locale",
                format!(
                    "date format {} requires locale {}, but only English date names are supported; nothing was changed",
                    py_repr(pattern),
                    py_repr(language)
                ),
            ));
        }
    }
    Ok(pieces)
}

/// Upstream `_control_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Block,
    Inline,
}

fn control_level(tree: &XmlTree, sdt: NodeId, content: Option<NodeId>) -> Result<Level> {
    let parent_level = tree.parent(sdt).and_then(|parent| {
        if is_w(tree, parent, "p") {
            Some(Level::Inline)
        } else if tree.ns_uri(parent) == Some(W)
            && tree
                .local_name(parent)
                .is_some_and(|local| BLOCK_CONTROL_PARENTS.contains(&local))
        {
            Some(Level::Block)
        } else {
            None
        }
    });
    if let Some(content) = content
        && node_children(tree, content).next().is_some()
    {
        let has_paragraph = tree.child_elements(content).any(|c| is_w(tree, c, "p"));
        let has_run = tree.child_elements(content).any(|c| is_w(tree, c, "r"));
        if has_paragraph == has_run {
            return Err(unsupported(
                "ambiguous_control_shape",
                "control content has an ambiguous block/inline shape; nothing was changed",
            ));
        }
        let content_level = if has_paragraph {
            Level::Block
        } else {
            Level::Inline
        };
        if parent_level.is_some_and(|level| level != content_level) {
            return Err(unsupported(
                "control_shape_mismatch",
                "control content does not match its block/inline parent; nothing was changed",
            ));
        }
        return Ok(content_level);
    }
    parent_level.ok_or_else(|| {
        unsupported(
            "ambiguous_control_parent",
            "empty control has an ambiguous block/inline parent; nothing was changed",
        )
    })
}

/// lxml `parent.remove(node)`: the node leaves together with its tail (the
/// character data that follows it).
pub(crate) fn remove_with_tail(tree: &mut XmlTree, node: NodeId) -> Result<()> {
    let tail = tail_nodes(tree, node);
    tree.detach(node)?;
    for text in tail {
        tree.detach(text)?;
    }
    Ok(())
}

/// The text nodes directly following `node` (its lxml tail).
pub(crate) fn tail_nodes(tree: &XmlTree, node: NodeId) -> Vec<NodeId> {
    let Some(parent) = tree.parent(node) else {
        return Vec::new();
    };
    let siblings = tree.children(parent);
    let index = siblings
        .iter()
        .position(|s| *s == node)
        .unwrap_or(siblings.len());
    siblings[index + 1..]
        .iter()
        .copied()
        .take_while(|s| matches!(tree.data(*s), NodeData::Text(_)))
        .collect()
}

fn clear_placeholder_state(tree: &mut XmlTree, sdt: NodeId) -> Result<()> {
    if let Some(pr) = w_child(tree, sdt, "sdtPr")
        && let Some(showing) = w_child(tree, pr, "showingPlcHdr")
    {
        tree.detach(showing)?;
    }
    let styles: Vec<NodeId> = tree
        .descendants(sdt)
        .filter(|node| {
            is_w(tree, *node, "rStyle") && w_attr(tree, *node, "val") == Some("PlaceholderText")
        })
        .filter(|node| {
            !tree
                .ancestors(*node)
                .take_while(|a| *a != sdt)
                .any(|a| is_w(tree, a, "sdt"))
        })
        .collect();
    for style in styles {
        tree.detach(style)?;
    }
    Ok(())
}

/// Upstream `Control._set_text`: replace the control's content with one run
/// of `text`, keeping the first run's formatting (block controls keep one
/// paragraph with its `w:pPr`).
fn set_text(tree: &mut XmlTree, sdt: NodeId, text: &str, font: Option<&str>) -> Result<()> {
    let content = w_child(tree, sdt, "sdtContent");
    if let Some(content) = content {
        if tree
            .descendants(content)
            .skip(1)
            .any(|node| is_w(tree, node, "sdt"))
        {
            return Err(unsupported(
                "nested_controls",
                "control content holds nested content controls; set the inner controls individually instead of overwriting the group",
            ));
        }
        if tree
            .descendants(content)
            .skip(1)
            .any(|node| is_w(tree, node, "tbl"))
        {
            return Err(unsupported(
                "control_holds_table",
                "control content holds a table; refusing to replace structure with plain text",
            ));
        }
        let has_block = w_child(tree, content, "p").is_some();
        let has_inline = tree.child_elements(content).any(|c| is_w(tree, c, "r"))
            || node_children(tree, content).next().is_none();
        if !has_block && !has_inline {
            return Err(unsupported(
                "control_not_simple_text",
                "control content is not simple text (no paragraph or runs to replace); refusing to destroy its structure",
            ));
        }
    }
    let level = control_level(tree, sdt, content)?;

    let first_run = content.and_then(|c| tree.descendants(c).find(|n| is_w(tree, *n, "r")));
    let mut template_rpr = first_run
        .and_then(|run| w_child(tree, run, "rPr"))
        .map(|rpr| tree.deep_clone(rpr));
    let content = match content {
        Some(content) => content,
        None => {
            let content = tree.create_element(sdt, W, "sdtContent")?;
            let end = tree.children(sdt).len();
            tree.insert(sdt, end, content)?;
            content
        }
    };
    let run = tree.create_element(content, W, "r")?;
    let target_paragraph = if level == Level::Block {
        let paragraph = match w_child(tree, content, "p") {
            Some(paragraph) => paragraph,
            None => {
                let paragraph = tree.create_element(content, W, "p")?;
                let end = tree.children(content).len();
                tree.insert(content, end, paragraph)?;
                paragraph
            }
        };
        let doomed: Vec<NodeId> = node_children(tree, paragraph)
            .filter(|child| !is_w(tree, *child, "pPr"))
            .collect();
        for child in doomed {
            remove_with_tail(tree, child)?;
        }
        let end = tree.children(paragraph).len();
        tree.insert(paragraph, end, run)?;
        let extras: Vec<NodeId> = node_children(tree, content)
            .filter(|child| *child != paragraph)
            .collect();
        for extra in extras {
            remove_with_tail(tree, extra)?;
        }
        Some(paragraph)
    } else {
        let doomed: Vec<NodeId> = node_children(tree, content).collect();
        for child in doomed {
            remove_with_tail(tree, child)?;
        }
        let end = tree.children(content).len();
        tree.insert(content, end, run)?;
        None
    };
    let _ = target_paragraph;
    if let Some(font) = font {
        let rpr = match template_rpr {
            Some(rpr) => rpr,
            None => tree.create_element(run, W, "rPr")?,
        };
        tree.insert(run, 0, rpr)?;
        let fonts = match w_child(tree, rpr, "rFonts") {
            Some(fonts) => fonts,
            None => {
                let fonts = tree.create_element(rpr, W, "rFonts")?;
                tree.insert(rpr, 0, fonts)?;
                fonts
            }
        };
        for attribute in ["ascii", "hAnsi", "eastAsia", "cs"] {
            tree.set_attr(fonts, Some(W), attribute, font)?;
        }
        template_rpr = None;
    }
    if let Some(rpr) = template_rpr {
        tree.insert(run, 0, rpr)?;
    }
    crate::oxml::add_t(tree, run, text)?;
    clear_placeholder_state(tree, sdt)
}

/// `[\w.-]` of a Python `str` regex.
fn is_name_tail(c: char) -> bool {
    crate::bookmarks::pyclass::is_py_word(c) || c == '.' || c == '-'
}

fn is_name_head(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Length of `[A-Za-z_][\w.-]*` at `chars[at..]`.
fn match_name(chars: &[char], at: usize) -> Option<usize> {
    if !chars.get(at).is_some_and(|c| is_name_head(*c)) {
        return None;
    }
    let mut end = at + 1;
    while end < chars.len() && is_name_tail(chars[end]) {
        end += 1;
    }
    Some(end - at)
}

/// Upstream `_prefix_map` (the `xmlns(?::prefix)?\s*=\s*['"]uri['"]` scan).
fn prefix_map(raw: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    let space = crate::unicode::is_py_space;
    while i < chars.len() {
        let attempt = (|| {
            if chars.get(i..i + 5)? != ['x', 'm', 'l', 'n', 's'] {
                return None;
            }
            let mut j = i + 5;
            let mut prefix = None;
            if chars.get(j) == Some(&':') {
                let length = match_name(&chars, j + 1)?;
                prefix = Some(chars[j + 1..j + 1 + length].iter().collect::<String>());
                j += 1 + length;
            }
            while j < chars.len() && space(chars[j]) {
                j += 1;
            }
            if chars.get(j) != Some(&'=') {
                return None;
            }
            j += 1;
            while j < chars.len() && space(chars[j]) {
                j += 1;
            }
            if !matches!(chars.get(j), Some('\'' | '"')) {
                return None;
            }
            j += 1;
            let start = j;
            while j < chars.len() && !matches!(chars[j], '\'' | '"') {
                j += 1;
            }
            if j == start || j >= chars.len() {
                return None;
            }
            let uri: String = chars[start..j].iter().collect();
            Some((prefix.unwrap_or_else(|| "_".into()), uri, j + 1))
        })();
        match attempt {
            Some((prefix, uri, end)) => {
                match out.iter_mut().find(|(p, _)| *p == prefix) {
                    Some(existing) => existing.1 = uri,
                    None => out.push((prefix, uri)),
                }
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

/// Upstream `_binding_namespaces`.
fn binding_namespaces(item: &XmlTree, binding_mappings: Option<&str>) -> Vec<(String, String)> {
    let mapped = prefix_map(binding_mappings.unwrap_or(""));
    if !mapped.is_empty() {
        return mapped;
    }
    let mut out = Vec::new();
    let mut default = None;
    for (prefix, uri) in item.in_scope(item.root()) {
        let Some(uri) = uri else { continue };
        match prefix {
            Some(prefix) => out.push((prefix.to_string(), item.uri(uri).to_string())),
            None => default = Some(item.uri(uri).to_string()),
        }
    }
    if let Some(default) = default {
        out.push(("_".into(), default));
    }
    out
}

/// Upstream `_evaluate_store_xpath`.
fn evaluate_store_xpath(
    item: &XmlTree,
    expr: &str,
    namespaces: &[(String, String)],
) -> Result<Vec<xpath::XNode>> {
    match xpath::evaluate(item, expr, namespaces) {
        Ok(xpath::XValue::Nodes(nodes)) => Ok(nodes),
        Ok(other) => Err(unsupported(
            "xpath_not_node_set",
            format!(
                "the data-bound control's xpath {} evaluates to {}, not to a node, so there is nothing in the custom \
                 XML store to write to. Point the binding at an element or attribute; nothing was changed",
                py_repr(expr),
                other.python_type()
            ),
        )),
        Err(xpath::XError::Unsupported(detail)) => Err(unsupported(
            "xpath_unsupported",
            format!(
                "the data-bound control's xpath {} uses {detail}, which this library does not evaluate, so the bound \
                 value cannot be located. The document itself is intact; nothing was changed",
                py_repr(expr)
            ),
        )),
        Err(xpath::XError::Syntax(detail) | xpath::XError::Eval(detail)) => Err(unsupported(
            "xpath_invalid",
            format!(
                "the data-bound control's xpath {} is not a valid XPath expression, so the bound value cannot be \
                 located. The document itself is intact — Word treats such a binding as unbound. Repair the \
                 w:dataBinding expression, or set the control's value in a document whose binding resolves; nothing \
                 was changed ({detail})",
                py_repr(expr)
            ),
        )),
    }
}

/// `re.search(r"(^|/)[A-Za-z_][\w.-]*:", xpath)`.
fn has_prefixed_step(chars: &[char]) -> bool {
    std::iter::once(0)
        .chain(
            chars
                .iter()
                .enumerate()
                .filter(|(_, c)| **c == '/')
                .map(|(i, _)| i + 1),
        )
        .any(|at| match_name(chars, at).is_some_and(|length| chars.get(at + length) == Some(&':')))
}

/// `re.sub(r"(^|/)([A-Za-z_][\w.-]*)", r"\1_:\2", xpath)`.
fn qualify_steps(chars: &[char]) -> String {
    let mut out = String::new();
    let mut pos = 0;
    while pos < chars.len() {
        if pos == 0
            && let Some(length) = match_name(chars, 0)
        {
            out.push_str("_:");
            out.extend(&chars[..length]);
            pos = length;
            continue;
        }
        if chars[pos] == '/'
            && let Some(length) = match_name(chars, pos + 1)
        {
            out.push_str("/_:");
            out.extend(&chars[pos + 1..pos + 1 + length]);
            pos += 1 + length;
            continue;
        }
        out.push(chars[pos]);
        pos += 1;
    }
    out
}

/// Upstream `_eval_store_xpath` (with the default-namespace retry).
fn eval_store_xpath(
    item: &XmlTree,
    expr: &str,
    namespaces: &[(String, String)],
) -> Result<Vec<xpath::XNode>> {
    let nodes = evaluate_store_xpath(item, expr, namespaces)?;
    if !nodes.is_empty() {
        return Ok(nodes);
    }
    let chars: Vec<char> = expr.chars().collect();
    if !namespaces.iter().any(|(p, _)| p == "_") || has_prefixed_step(&chars) {
        return Ok(nodes);
    }
    evaluate_store_xpath(item, &qualify_steps(&chars), namespaces)
}

/// Upstream `_write_bound_store`: write `value` into the custom XML item the
/// binding names (staged in `txn`).
fn write_bound_store(
    txn: &mut Txn<'_>,
    tree: &XmlTree,
    binding: NodeId,
    value: &str,
) -> Result<()> {
    let store_id = w_attr(tree, binding, "storeItemID").filter(|v| !v.is_empty());
    let expr = w_attr(tree, binding, "xpath").filter(|v| !v.is_empty());
    let (Some(store_id), Some(expr)) = (store_id, expr) else {
        return Err(unsupported(
            "incomplete_binding",
            "data-bound control is missing storeItemID or xpath; nothing was changed",
        ));
    };
    let doc = txn.document();
    let package = doc.package();
    let mut props_part = None;
    for part in package.parts().filter(|p| p.is_reachable()) {
        if part.content_type() != Some(CUSTOM_XML_PROPERTIES) {
            continue;
        }
        let Ok(root_tree) = package.xml(part.name()) else {
            continue;
        };
        if root_tree.attr(root_tree.root(), Some(DS_NS), "itemID") == Some(store_id) {
            props_part = Some(part.name().to_string());
            break;
        }
    }
    let Some(props_part) = props_part else {
        return Err(not_found(
            "no_custom_xml_store",
            format!(
                "no custom XML store with itemID {}; nothing was changed",
                py_repr(store_id)
            ),
        ));
    };
    let mut item_part = None;
    'owners: for owner in package.parts().filter(|p| p.is_reachable()) {
        let rels = package.relationships_at(&format!("/{}", owner.name()))?;
        let base = crate::package::rels::base_uri(&format!("/{}", owner.name())).to_string();
        for rel in rels.iter().filter(|rel| !rel.external) {
            let target = rel.target_part_name(&base);
            if package.resolve_member(&target) == Some(props_part.as_str()) {
                item_part = Some(owner.name().to_string());
                break 'owners;
            }
        }
    }
    let item_part = match item_part {
        Some(item) if item != props_part => item,
        _ => {
            return Err(not_found(
                "no_custom_xml_item",
                "custom XML properties have no item payload; nothing was changed",
            ));
        }
    };
    let is_xml = package
        .part(&item_part)
        .and_then(|p| p.content_type())
        .is_some_and(|ct| ct.to_lowercase().contains("xml"));
    if !is_xml
        || package
            .part(&item_part)
            .is_some_and(|p| p.current_bytes().is_empty())
    {
        return Err(unsupported(
            "custom_xml_not_xml",
            "custom XML item is not writable XML; nothing was changed",
        ));
    }
    let item_tree = txn.tree(&item_part)?;
    let namespaces = binding_namespaces(item_tree, w_attr(tree, binding, "prefixMappings"));
    let nodes = eval_store_xpath(item_tree, expr, &namespaces)?;
    let Some(first) = nodes.first().copied() else {
        return Err(not_found(
            "xpath_matched_nothing",
            format!(
                "xpath {} matched no node in the custom XML store",
                py_repr(expr)
            ),
        ));
    };
    let staged = txn.tree_mut(&item_part)?;
    match first {
        xpath::XNode::Node(id) if matches!(staged.data(id), NodeData::Element(_)) => {
            staged.remove_attr(id, Some(XSI_NS), "nil");
            set_leading_text(staged, id, value)
        }
        xpath::XNode::Attr(element, index) => {
            let (ns, local) = {
                let attr = staged
                    .element(element)
                    .and_then(|e| e.attrs.get(index))
                    .ok_or_else(|| Error::internal("xpath_attr", "attribute vanished"))?;
                (
                    attr.ns.map(|n| staged.uri(n).to_string()),
                    attr.name.local.to_string(),
                )
            };
            staged.set_attr(element, ns.as_deref(), &local, value)
        }
        _ => Err(unsupported(
            "non_element_store_target",
            "xpath matched a non-element store target; nothing was changed",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_maps_and_step_qualification() {
        assert_eq!(
            prefix_map("xmlns:ns0='http://example.com/form' xmlns='urn:d'"),
            vec![
                ("ns0".to_string(), "http://example.com/form".to_string()),
                ("_".to_string(), "urn:d".to_string())
            ]
        );
        let chars: Vec<char> = "/root/name".chars().collect();
        assert_eq!(qualify_steps(&chars), "/_:root/_:name");
        assert!(!has_prefixed_step(&chars));
        let chars: Vec<char> = "/ns0:root[1]/ns0:name".chars().collect();
        assert!(has_prefixed_step(&chars));
        let chars: Vec<char> = "a[b/c]".chars().collect();
        assert_eq!(qualify_steps(&chars), "_:a[b/_:c]");
        assert_eq!(py_int_hex("0x2612"), Some(0x2612));
        assert_eq!(py_int_hex(" 26_12 "), Some(0x2612));
        assert_eq!(py_int_hex("zz"), None);
    }

    #[test]
    fn weekday_matches_python() {
        // 2026-07-08 is a Wednesday (weekday 2).
        let stamp = Stamp::of(&ControlValue::Date {
            year: 2026,
            month: 7,
            day: 8,
        })
        .unwrap()
        .unwrap();
        assert_eq!(stamp.weekday, 2);
    }
}
