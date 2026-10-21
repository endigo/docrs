//! Sections, headers and footers (upstream python-docx `section.py`,
//! `oxml/section.py`, `parts/hdrftr.py`, `parts/document.py`,
//! `settings.py` / `oxml/settings.py`; MIT, see `NOTICE.md`).
//!
//! [`Section`] is a live handle on a `w:sectPr` of the main document
//! (`Document::sections`, upstream `Sections`). Page setup is read with
//! [`Section::setup`] and written with [`Section::set`], a batch of
//! [`SectionChange`]s applied in order exactly like the upstream setters.
//!
//! [`HeaderFooter`] mirrors upstream `_Header` / `_Footer`: it is a
//! (section, header-or-footer, type) triple, not an element handle.
//! `is_linked_to_previous` reports whether the section has its own
//! definition; assigning `false` adds a definition part (from upstream's
//! `default-header.xml` / `default-footer.xml` at the next free
//! `word/header%d.xml` / `word/footer%d.xml`), assigning `true` removes the
//! reference and drops the relationship. Content access goes through
//! [`HeaderFooter::definition`] (upstream `_get_or_add_definition`, which
//! follows prior sections and creates a definition on the first section
//! when none exists) and [`crate::authoring::BlockContainer`].
//!
//! Every write validates first, stages in a [`Txn`] and commits last.

use crate::authoring::{
    BlockContainer, BlockItem, ContainerKind, Handle, SECT_PR_SEQ, SectionStart, on_off_val,
    parse_signed_twips, parse_twips_measure, remove, remove_all, section_properties, set_on_off,
    set_opt, signed_twips_measure, twips_measure,
};
use crate::oxml::{append, get_or_add};
use crate::package::ct;
use crate::package::rels::{base_uri, rels_member_for, rt};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::{RelTarget, Txn};
use crate::wml::{W, w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

const DEFAULT_HEADER: &[u8] = include_bytes!("../assets/default-header.xml");
const DEFAULT_FOOTER: &[u8] = include_bytes!("../assets/default-footer.xml");

/// Office relationships (`r:`).
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Page orientation (`WD_ORIENTATION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Orientation {
    /// `portrait` (the default; written as no `w:orient`).
    Portrait,
    /// `landscape`.
    Landscape,
}

impl Orientation {
    /// XML token.
    pub fn as_str(self) -> &'static str {
        match self {
            Orientation::Portrait => "portrait",
            Orientation::Landscape => "landscape",
        }
    }

    /// From an XML token (`ValueError` upstream for unknown tokens).
    pub fn parse(token: &str) -> Result<Orientation> {
        match token {
            "portrait" => Ok(Orientation::Portrait),
            "landscape" => Ok(Orientation::Landscape),
            other => Err(Error::invalid_argument(
                "invalid_orientation",
                format!("WD_ORIENTATION has no XML mapping for '{other}'"),
            )),
        }
    }
}

/// Which header or footer of a section (`WD_HEADER_FOOTER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum HeaderFooterType {
    /// `default` (upstream `PRIMARY`).
    Primary,
    /// `first` (`FIRST_PAGE`).
    FirstPage,
    /// `even` (`EVEN_PAGE`).
    EvenPage,
}

impl HeaderFooterType {
    /// XML token of `w:headerReference/@w:type`.
    pub fn as_str(self) -> &'static str {
        match self {
            HeaderFooterType::Primary => "default",
            HeaderFooterType::FirstPage => "first",
            HeaderFooterType::EvenPage => "even",
        }
    }
}

/// Header or footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum HeaderFooterRole {
    /// `w:headerReference` / header part.
    Header,
    /// `w:footerReference` / footer part.
    Footer,
}

impl HeaderFooterRole {
    fn reference(self) -> &'static str {
        match self {
            HeaderFooterRole::Header => "headerReference",
            HeaderFooterRole::Footer => "footerReference",
        }
    }

    fn rel_type(self) -> &'static str {
        match self {
            HeaderFooterRole::Header => rt::HEADER,
            HeaderFooterRole::Footer => rt::FOOTER,
        }
    }

    fn container(self) -> ContainerKind {
        match self {
            HeaderFooterRole::Header => ContainerKind::Header,
            HeaderFooterRole::Footer => ContainerKind::Footer,
        }
    }

    fn root(self) -> &'static str {
        match self {
            HeaderFooterRole::Header => "hdr",
            HeaderFooterRole::Footer => "ftr",
        }
    }
}

/// A live `w:sectPr` handle of the main document (upstream `Section`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub(crate) h: Handle,
}

/// Upstream `Section` page-setup getters (`None` when not set).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SectionSetup {
    /// `start_type`.
    pub start_type: SectionStart,
    /// `page_width` (EMU).
    pub page_width: Option<i64>,
    /// `page_height` (EMU).
    pub page_height: Option<i64>,
    /// `orientation`.
    pub orientation: Orientation,
    /// `left_margin` (EMU).
    pub left_margin: Option<i64>,
    /// `right_margin` (EMU).
    pub right_margin: Option<i64>,
    /// `top_margin` (EMU).
    pub top_margin: Option<i64>,
    /// `bottom_margin` (EMU).
    pub bottom_margin: Option<i64>,
    /// `gutter` (EMU).
    pub gutter: Option<i64>,
    /// `header_distance` (EMU).
    pub header_distance: Option<i64>,
    /// `footer_distance` (EMU).
    pub footer_distance: Option<i64>,
    /// `different_first_page_header_footer`.
    pub different_first_page_header_footer: bool,
}

/// One `Section` property assignment (applied in order by
/// [`Section::set`]). Lengths are EMU; `None` removes the setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionChange {
    /// `start_type` (`None` and `NewPage` remove `w:type`).
    StartType(Option<SectionStart>),
    /// `page_width` (`w:pgSz/@w:w`).
    PageWidth(Option<i64>),
    /// `page_height` (`w:pgSz/@w:h`).
    PageHeight(Option<i64>),
    /// `orientation` (`None` means portrait).
    Orientation(Option<Orientation>),
    /// `left_margin`.
    LeftMargin(Option<i64>),
    /// `right_margin`.
    RightMargin(Option<i64>),
    /// `top_margin` (signed).
    TopMargin(Option<i64>),
    /// `bottom_margin` (signed).
    BottomMargin(Option<i64>),
    /// `gutter`.
    Gutter(Option<i64>),
    /// `header_distance` (`w:pgMar/@w:header`).
    HeaderDistance(Option<i64>),
    /// `footer_distance` (`w:pgMar/@w:footer`).
    FooterDistance(Option<i64>),
    /// `different_first_page_header_footer` (`w:titlePg`).
    DifferentFirstPageHeaderFooter(bool),
}

/// Upstream `_Header` / `_Footer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderFooter {
    section: Section,
    role: HeaderFooterRole,
    kind: HeaderFooterType,
}

// ----------------------------------------------------------- reading

fn main_part(doc: &Document) -> String {
    doc.package().main_part_name().to_string()
}

fn pg_attr(
    tree: &XmlTree,
    sect: NodeId,
    child: &str,
    local: &str,
    signed: bool,
) -> Result<Option<i64>> {
    match w_child(tree, sect, child).and_then(|e| w_attr(tree, e, local)) {
        None => Ok(None),
        Some(raw) if signed => parse_signed_twips(raw).map(Some),
        Some(raw) => parse_twips_measure(raw).map(Some),
    }
}

fn read_setup(tree: &XmlTree, sect: NodeId) -> Result<SectionSetup> {
    let start_type = match w_child(tree, sect, "type").and_then(|t| w_attr(tree, t, "val")) {
        None => SectionStart::NewPage,
        Some(token) => SectionStart::parse(token)?,
    };
    let orientation = match w_child(tree, sect, "pgSz").and_then(|p| w_attr(tree, p, "orient")) {
        None => Orientation::Portrait,
        Some(token) => Orientation::parse(token)?,
    };
    let different_first_page_header_footer = match w_child(tree, sect, "titlePg") {
        None => false,
        Some(title) => on_off_val(tree, title)?,
    };
    Ok(SectionSetup {
        start_type,
        page_width: pg_attr(tree, sect, "pgSz", "w", false)?,
        page_height: pg_attr(tree, sect, "pgSz", "h", false)?,
        orientation,
        left_margin: pg_attr(tree, sect, "pgMar", "left", false)?,
        right_margin: pg_attr(tree, sect, "pgMar", "right", false)?,
        top_margin: pg_attr(tree, sect, "pgMar", "top", true)?,
        bottom_margin: pg_attr(tree, sect, "pgMar", "bottom", true)?,
        gutter: pg_attr(tree, sect, "pgMar", "gutter", false)?,
        header_distance: pg_attr(tree, sect, "pgMar", "header", false)?,
        footer_distance: pg_attr(tree, sect, "pgMar", "footer", false)?,
        different_first_page_header_footer,
    })
}

/// `./w:{role}Reference[@w:type='{kind}']` (first match).
fn reference(
    tree: &XmlTree,
    sect: NodeId,
    role: HeaderFooterRole,
    kind: HeaderFooterType,
) -> Option<NodeId> {
    w_children(tree, sect, role.reference())
        .find(|r| w_attr(tree, *r, "type") == Some(kind.as_str()))
}

/// `./preceding::w:sectPr[1]`: the nearest `w:sectPr` before `sect` in
/// document order that is not an ancestor.
fn preceding_sect_pr(tree: &XmlTree, sect: NodeId) -> Option<NodeId> {
    let mut last = None;
    for node in tree.descendants(tree.root()) {
        if node == sect {
            break;
        }
        if tree.is(node, W, "sectPr") && !tree.ancestors(sect).any(|a| a == node) {
            last = Some(node);
        }
    }
    last
}

// ----------------------------------------------------------- writing

fn validate_change(change: &SectionChange) -> Result<()> {
    match *change {
        SectionChange::PageWidth(Some(v))
        | SectionChange::PageHeight(Some(v))
        | SectionChange::LeftMargin(Some(v))
        | SectionChange::RightMargin(Some(v))
        | SectionChange::Gutter(Some(v))
        | SectionChange::HeaderDistance(Some(v))
        | SectionChange::FooterDistance(Some(v)) => twips_measure(v, "w:pgMar").map(|_| ()),
        SectionChange::TopMargin(Some(v)) | SectionChange::BottomMargin(Some(v)) => {
            signed_twips_measure(v, "w:pgMar").map(|_| ())
        }
        _ => Ok(()),
    }
}

fn apply_change(tree: &mut XmlTree, sect: NodeId, change: &SectionChange) -> Result<()> {
    let measure = |value: Option<i64>, signed: bool| -> Result<Option<String>> {
        value
            .map(|emu| {
                if signed {
                    signed_twips_measure(emu, "w:pgMar")
                } else {
                    twips_measure(emu, "w:pgMar")
                }
            })
            .transpose()
    };
    let pg_mar = |tree: &mut XmlTree, local: &str, value: Option<String>| -> Result<()> {
        let mar = get_or_add(tree, sect, "pgMar", SECT_PR_SEQ)?;
        set_opt(tree, mar, local, value)
    };
    match *change {
        SectionChange::StartType(value) => match value {
            None | Some(SectionStart::NewPage) => remove_all(tree, sect, "type"),
            Some(start) => {
                let kind = get_or_add(tree, sect, "type", SECT_PR_SEQ)?;
                set_opt(tree, kind, "val", Some(start.as_str().to_string()))
            }
        },
        SectionChange::PageWidth(value) | SectionChange::PageHeight(value) => {
            let local = if matches!(change, SectionChange::PageWidth(_)) {
                "w"
            } else {
                "h"
            };
            let size = get_or_add(tree, sect, "pgSz", SECT_PR_SEQ)?;
            set_opt(tree, size, local, measure(value, false)?)
        }
        SectionChange::Orientation(value) => {
            let size = get_or_add(tree, sect, "pgSz", SECT_PR_SEQ)?;
            let token = match value {
                Some(Orientation::Landscape) => Some("landscape".to_string()),
                _ => None,
            };
            set_opt(tree, size, "orient", token)
        }
        SectionChange::LeftMargin(value) => pg_mar(tree, "left", measure(value, false)?),
        SectionChange::RightMargin(value) => pg_mar(tree, "right", measure(value, false)?),
        SectionChange::TopMargin(value) => pg_mar(tree, "top", measure(value, true)?),
        SectionChange::BottomMargin(value) => pg_mar(tree, "bottom", measure(value, true)?),
        SectionChange::Gutter(value) => pg_mar(tree, "gutter", measure(value, false)?),
        SectionChange::HeaderDistance(value) => pg_mar(tree, "header", measure(value, false)?),
        SectionChange::FooterDistance(value) => pg_mar(tree, "footer", measure(value, false)?),
        SectionChange::DifferentFirstPageHeaderFooter(value) => {
            if value {
                let title = get_or_add(tree, sect, "titlePg", SECT_PR_SEQ)?;
                set_on_off(tree, title, true)
            } else {
                remove_all(tree, sect, "titlePg")
            }
        }
    }
}

/// `w:{role}Reference` with `w:type` and `r:id`, inserted before the first
/// `CT_SectPr._tag_seq` child (after existing references).
fn add_reference(
    tree: &mut XmlTree,
    sect: NodeId,
    role: HeaderFooterRole,
    kind: HeaderFooterType,
    r_id: &str,
) -> Result<()> {
    let element = tree.create_element(sect, W, role.reference())?;
    crate::oxml::insert_before_successors(tree, sect, element, SECT_PR_SEQ)?;
    tree.set_attr(element, Some(W), "type", kind.as_str())?;
    tree.set_attr(element, Some(R), "id", r_id)?;
    Ok(())
}

/// `DocumentPart.add_header_part()` / `add_footer_part()` + reference.
fn add_definition(
    txn: &mut Txn<'_>,
    sect: NodeId,
    role: HeaderFooterRole,
    kind: HeaderFooterType,
) -> Result<String> {
    let main = main_part(txn.document());
    let (template, content_type, name_template) = match role {
        HeaderFooterRole::Header => (DEFAULT_HEADER, ct::WML_HEADER, "word/header{}.xml"),
        HeaderFooterRole::Footer => (DEFAULT_FOOTER, ct::WML_FOOTER, "word/footer{}.xml"),
    };
    let name = txn.next_partname(name_template);
    txn.add_xml_part(&name, content_type, crate::parts::template(template)?)?;
    let r_id = txn.relate_to(&main, role.rel_type(), RelTarget::Part(&name))?;
    add_reference(txn.tree_mut(&main)?, sect, role, kind, &r_id)?;
    Ok(name)
}

/// `Part.drop_rel(rId)`: remove the relationship when fewer than two
/// `r:id` references to it remain in the part.
///
/// Upstream drops it even when one reference remains (another section
/// sharing the definition), which leaves that reference dangling; Rust
/// keeps a relationship that is still referenced (deviation, see the P7
/// progress note).
fn drop_rel(txn: &mut Txn<'_>, source: &str, r_id: &str) -> Result<()> {
    let references = {
        let tree = txn.tree(source)?;
        tree.descendants(tree.root())
            .filter(|node| tree.attr(*node, Some(R), "id") == Some(r_id))
            .count()
    };
    if references >= 1 {
        return Ok(());
    }
    let source_name = format!("/{source}");
    let Some(rels) = txn.resolve(&rels_member_for(&source_name)) else {
        return Ok(());
    };
    let tree = txn.tree_mut(&rels)?;
    let root = tree.root();
    let doomed: Vec<NodeId> = tree
        .child_elements(root)
        .filter(|rel| tree.attr(*rel, None, "Id") == Some(r_id))
        .collect();
    if let Some(rel) = doomed.first() {
        remove(tree, *rel)?;
    }
    Ok(())
}

/// The part a reference's `r:id` targets (`related_parts[rId]`).
fn referenced_part(txn: &Txn<'_>, reference_el: NodeId) -> Result<String> {
    let main = main_part(txn.document());
    let tree = txn.tree(&main)?;
    let r_id = tree.attr(reference_el, Some(R), "id").ok_or_else(|| {
        Error::invalid_argument(
            "missing_required_attribute",
            "required 'r:id' attribute not present",
        )
    })?;
    let source_name = format!("/{main}");
    let rels = txn.relationships(&main)?;
    let rel = rels
        .iter()
        .find(|rel| rel.id == r_id && !rel.external)
        .ok_or_else(|| {
            Error::refusal(
                RefusalKind::TargetNotFound,
                "missing_header_footer_relationship",
                format!("no relationship {r_id} for the header/footer reference"),
            )
        })?;
    txn.resolve(&rel.target_part_name(base_uri(&source_name)))
        .ok_or_else(|| {
            Error::refusal(
                RefusalKind::TargetNotFound,
                "missing_header_footer_part",
                format!("relationship {r_id} targets a missing part"),
            )
        })
}

// ----------------------------------------------------------- public API

impl Document {
    /// Upstream `Document.sections`: `w:body/w:p/w:pPr/w:sectPr` and
    /// `w:body/w:sectPr` in document order.
    pub fn sections(&self) -> Result<Vec<Section>> {
        let part = main_part(self);
        let tree = self.package().xml(&part)?;
        Ok(section_properties(tree)?
            .into_iter()
            .map(|sect| Section {
                h: Handle::capture(self, &part, tree, sect),
            })
            .collect())
    }

    /// Upstream `Settings.odd_and_even_pages_header_footer`
    /// (`w:evenAndOddHeaders`). Reading never creates the settings part
    /// (upstream adds a default one); without it the value is `false`.
    pub fn odd_and_even_pages_header_footer(&self) -> Result<bool> {
        let package = self.package();
        let Some(part) = package.part_related_by(package.main_part_name(), rt::SETTINGS)? else {
            return Ok(false);
        };
        let tree = package.xml(&part)?;
        match w_child(tree, tree.root(), "evenAndOddHeaders") {
            None => Ok(false),
            Some(element) => on_off_val(tree, element),
        }
    }

    /// Upstream `Settings.odd_and_even_pages_header_footer = value`
    /// (creating the settings part from the default template if missing).
    pub fn set_odd_and_even_pages_header_footer(&mut self, value: bool) -> Result<()> {
        refuse_if_protected(
            self,
            "change header and footer settings",
            OperationClass::BodyContent,
        )?;
        let mut txn = Txn::new(self);
        let part = crate::parts::ensure_settings_part(&mut txn)?;
        let tree = txn.tree_mut(&part)?;
        let root = tree.root();
        if value {
            let element = match w_child(tree, root, "evenAndOddHeaders") {
                Some(existing) => existing,
                None => {
                    let element = tree.create_element(root, W, "evenAndOddHeaders")?;
                    insert_settings_child(tree, root, element)?;
                    element
                }
            };
            set_on_off(tree, element, true)?;
        } else {
            remove_all(tree, root, "evenAndOddHeaders")?;
        }
        let staged = txn.finish()?;
        self.commit(staged)
    }
}

/// `CT_Settings` successors of `w:evenAndOddHeaders` (with their
/// namespaces; `m:mathPr` and `sl:schemaLibrary` are not `w:`).
const SETTINGS_SUCCESSORS: &[(&str, &str)] = &[
    (W, "bookFoldRevPrinting"),
    (W, "bookFoldPrinting"),
    (W, "bookFoldPrintingSheets"),
    (W, "drawingGridHorizontalSpacing"),
    (W, "drawingGridVerticalSpacing"),
    (W, "displayHorizontalDrawingGridEvery"),
    (W, "displayVerticalDrawingGridEvery"),
    (W, "doNotUseMarginsForDrawingGridOrigin"),
    (W, "drawingGridHorizontalOrigin"),
    (W, "drawingGridVerticalOrigin"),
    (W, "doNotShadeFormData"),
    (W, "noPunctuationKerning"),
    (W, "characterSpacingControl"),
    (W, "printTwoOnOne"),
    (W, "strictFirstAndLastChars"),
    (W, "noLineBreaksAfter"),
    (W, "noLineBreaksBefore"),
    (W, "savePreviewPicture"),
    (W, "doNotValidateAgainstSchema"),
    (W, "saveInvalidXml"),
    (W, "ignoreMixedContent"),
    (W, "alwaysShowPlaceholderText"),
    (W, "doNotDemarcateInvalidXml"),
    (W, "saveXmlDataOnly"),
    (W, "useXSLTWhenSaving"),
    (W, "saveThroughXslt"),
    (W, "showXMLTags"),
    (W, "alwaysMergeEmptyNamespace"),
    (W, "updateFields"),
    (W, "hdrShapeDefaults"),
    (W, "footnotePr"),
    (W, "endnotePr"),
    (W, "compat"),
    (W, "docVars"),
    (W, "rsids"),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/math",
        "mathPr",
    ),
    (W, "attachedSchema"),
    (W, "themeFontLang"),
    (W, "clrSchemeMapping"),
    (W, "doNotIncludeSubdocsInStats"),
    (W, "doNotAutoCompressPictures"),
    (W, "forceUpgrade"),
    (W, "captions"),
    (W, "readModeInkLockDown"),
    (W, "smartTagType"),
    (
        "http://schemas.openxmlformats.org/schemaLibrary/2006/main",
        "schemaLibrary",
    ),
    (W, "shapeDefaults"),
    (W, "doNotEmbedSmartTags"),
    (W, "decimalSymbol"),
    (W, "listSeparator"),
];

fn insert_settings_child(tree: &mut XmlTree, root: NodeId, element: NodeId) -> Result<()> {
    let index = tree.children(root).iter().position(|node| {
        SETTINGS_SUCCESSORS
            .iter()
            .any(|(ns, local)| tree.is(*node, ns, local))
    });
    match index {
        Some(index) => tree.insert(root, index, element),
        None => append(tree, root, element),
    }
}

impl Section {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "sectPr", "section")
    }

    /// Upstream `Section` page-setup getters.
    pub fn setup(&self, doc: &Document) -> Result<SectionSetup> {
        read_setup(self.live(doc)?, self.h.element)
    }

    /// Upstream `Section` setters, validated first and applied in order.
    /// Like upstream, assigning `None` to a margin or page dimension still
    /// adds an empty `w:pgMar` / `w:pgSz`.
    pub fn set(&self, doc: &mut Document, changes: &[SectionChange]) -> Result<()> {
        self.live(doc)?;
        for change in changes {
            validate_change(change)?;
        }
        refuse_if_protected(doc, "change a section's setup", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        let tree = txn.tree_mut(&self.h.part)?;
        for change in changes {
            apply_change(tree, self.h.element, change)?;
        }
        let staged = txn.finish()?;
        doc.commit(staged)
    }

    /// Upstream `Section.iter_inner_content()`: the paragraphs and tables
    /// of this section, in document order.
    pub fn inner_content(&self, doc: &Document) -> Result<Vec<BlockItem>> {
        let tree = self.live(doc)?;
        let sects = section_properties(tree)?;
        let index = sects
            .iter()
            .position(|s| *s == self.h.element)
            .ok_or_else(|| {
                Error::refusal(
                    RefusalKind::UnsupportedStructure,
                    "section_not_in_body",
                    "the section properties are not a body or body-paragraph w:sectPr",
                )
            })?;
        let in_and_above = |sect: NodeId| -> Vec<NodeId> {
            let parent = tree.parent(sect);
            // p-based section: the paragraph holding the sectPr and the p/tbl
            // before it; body-based: the p/tbl before the sectPr.
            let anchor = match parent {
                Some(ppr) if tree.is(ppr, W, "pPr") => {
                    tree.parent(ppr).filter(|p| tree.is(*p, W, "p"))
                }
                _ => None,
            };
            let (terminal, include) = match anchor {
                Some(p) => (p, true),
                None if parent.is_some_and(|b| tree.is(b, W, "body")) => (sect, false),
                None => return Vec::new(),
            };
            let Some(container) = tree.parent(terminal) else {
                return Vec::new();
            };
            let mut out: Vec<NodeId> = tree
                .child_elements(container)
                .take_while(|n| *n != terminal)
                .filter(|n| tree.is(*n, W, "p") || tree.is(*n, W, "tbl"))
                .collect();
            if include {
                out.push(terminal);
            }
            out
        };
        let skip = if index == 0 {
            0
        } else {
            in_and_above(sects[index - 1]).len()
        };
        let blocks: Vec<NodeId> = in_and_above(self.h.element)
            .into_iter()
            .skip(skip)
            .collect();
        Ok(blocks
            .into_iter()
            .map(|node| {
                let h = Handle::capture(doc, &self.h.part, tree, node);
                if tree.is(node, W, "p") {
                    BlockItem::Paragraph(crate::authoring::Paragraph { h })
                } else {
                    BlockItem::Table(crate::authoring::Table { h })
                }
            })
            .collect())
    }

    fn header_footer(&self, role: HeaderFooterRole, kind: HeaderFooterType) -> HeaderFooter {
        HeaderFooter {
            section: self.clone(),
            role,
            kind,
        }
    }

    /// Upstream `Section.header`.
    pub fn header(&self) -> HeaderFooter {
        self.header_footer(HeaderFooterRole::Header, HeaderFooterType::Primary)
    }

    /// Upstream `Section.footer`.
    pub fn footer(&self) -> HeaderFooter {
        self.header_footer(HeaderFooterRole::Footer, HeaderFooterType::Primary)
    }

    /// Upstream `Section.first_page_header`.
    pub fn first_page_header(&self) -> HeaderFooter {
        self.header_footer(HeaderFooterRole::Header, HeaderFooterType::FirstPage)
    }

    /// Upstream `Section.first_page_footer`.
    pub fn first_page_footer(&self) -> HeaderFooter {
        self.header_footer(HeaderFooterRole::Footer, HeaderFooterType::FirstPage)
    }

    /// Upstream `Section.even_page_header`.
    pub fn even_page_header(&self) -> HeaderFooter {
        self.header_footer(HeaderFooterRole::Header, HeaderFooterType::EvenPage)
    }

    /// Upstream `Section.even_page_footer`.
    pub fn even_page_footer(&self) -> HeaderFooter {
        self.header_footer(HeaderFooterRole::Footer, HeaderFooterType::EvenPage)
    }

    /// Any header or footer of this section.
    pub fn header_footer_of(&self, role: HeaderFooterRole, kind: HeaderFooterType) -> HeaderFooter {
        self.header_footer(role, kind)
    }
}

impl HeaderFooter {
    /// Header or footer.
    pub fn role(&self) -> HeaderFooterRole {
        self.role
    }

    /// Default, first-page or even-page.
    pub fn header_footer_type(&self) -> HeaderFooterType {
        self.kind
    }

    /// The section.
    pub fn section(&self) -> &Section {
        &self.section
    }

    /// Upstream `is_linked_to_previous`: `true` when this section has no
    /// reference of this role and type.
    pub fn is_linked_to_previous(&self, doc: &Document) -> Result<bool> {
        let tree = self.section.live(doc)?;
        Ok(reference(tree, self.section.h.element, self.role, self.kind).is_none())
    }

    /// Upstream `is_linked_to_previous = value`: `false` adds a new, empty
    /// definition (part + reference) when none exists; `true` removes the
    /// reference and drops its relationship. Unchanged values do nothing.
    pub fn set_linked_to_previous(&self, doc: &mut Document, value: bool) -> Result<()> {
        if value == self.is_linked_to_previous(doc)? {
            return Ok(());
        }
        let sect = self.section.h.element;
        let main = main_part(doc);
        refuse_if_protected(
            doc,
            "link or unlink a header or footer",
            OperationClass::BodyContent,
        )?;
        let mut txn = Txn::new(doc);
        if value {
            let tree = txn.tree_mut(&main)?;
            let reference_el = reference(tree, sect, self.role, self.kind)
                .ok_or_else(|| Error::internal("hdrftr", "reference vanished"))?;
            let r_id = tree
                .attr(reference_el, Some(R), "id")
                .map(str::to_string)
                .ok_or_else(|| {
                    Error::invalid_argument(
                        "missing_required_attribute",
                        "required 'r:id' attribute not present",
                    )
                })?;
            remove(tree, reference_el)?;
            drop_rel(&mut txn, &main, &r_id)?;
        } else {
            add_definition(&mut txn, sect, self.role, self.kind)?;
        }
        let staged = txn.finish()?;
        doc.commit(staged)
    }

    /// The container of this header's or footer's own definition, or of
    /// the definition it inherits from a prior section; `None` when no
    /// section up to this one defines it. Never creates anything.
    pub fn existing_definition(&self, doc: &Document) -> Result<Option<BlockContainer>> {
        let tree = self.section.live(doc)?;
        let mut sect = Some(self.section.h.element);
        let txn = Txn::new(doc);
        while let Some(current) = sect {
            if let Some(reference_el) = reference(tree, current, self.role, self.kind) {
                let part = referenced_part(&txn, reference_el)?;
                return self.container_of(doc, &part).map(Some);
            }
            sect = preceding_sect_pr(tree, current);
        }
        Ok(None)
    }

    fn container_of(&self, doc: &Document, part: &str) -> Result<BlockContainer> {
        let tree = doc.package().xml(part)?;
        let root = tree.root();
        if !tree.is(root, W, self.role.root()) {
            return Err(Error::refusal(
                RefusalKind::UnsupportedStructure,
                "unexpected_header_footer_root",
                format!("{part} is not a w:{} part", self.role.root()),
            ));
        }
        Ok(BlockContainer::new(
            Handle::capture(doc, part, tree, root),
            self.role.container(),
        ))
    }

    /// Upstream `_get_or_add_definition()` (what `paragraphs`, `tables`,
    /// `add_paragraph`, `add_table` and pictures use): this section's
    /// definition, else the nearest prior section's, else a new definition
    /// added to the first section of the chain.
    pub fn definition(&self, doc: &mut Document) -> Result<BlockContainer> {
        if let Some(existing) = self.existing_definition(doc)? {
            return Ok(existing);
        }
        let tree = self.section.live(doc)?;
        let mut first = self.section.h.element;
        while let Some(prior) = preceding_sect_pr(tree, first) {
            first = prior;
        }
        refuse_if_protected(doc, "add a header or footer", OperationClass::BodyContent)?;
        let mut txn = Txn::new(doc);
        let part = add_definition(&mut txn, first, self.role, self.kind)?;
        let staged = txn.finish()?;
        doc.commit(staged)?;
        self.container_of(doc, &part)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_tokens_round_trip() {
        assert_eq!(
            Orientation::parse("landscape").unwrap(),
            Orientation::Landscape
        );
        assert!(Orientation::parse("sideways").is_err());
        assert_eq!(HeaderFooterType::EvenPage.as_str(), "even");
    }
}
