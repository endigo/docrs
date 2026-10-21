//! Block-operation anchors (upstream `blocks._locate_anchor_paragraph`).
//!
//! A [`BlockTarget`] is an exact string (found with `find_one`; ambiguity
//! refuses), a live [`Span`] wholly inside one paragraph, or a live [`Block`].
//! Live targets must belong to the document, be fresh, and come from the
//! `current` view; legacy anchors cannot authorize mutations.

use crate::protection::{OperationClass, refuse_if_protected};
use crate::replace::validate_fresh;
use crate::story::{self, Block, BlockKind, View, container_elements, iter_block_elements};
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, FindOptions, RefusalKind, Result, Span};

/// What a block operation anchors on.
#[derive(Debug, Clone, Copy)]
pub enum BlockTarget<'a> {
    /// Exact text, located with `find_one` (exact, current view).
    Text(&'a str),
    /// A live block.
    Block(&'a Block),
    /// A live span within one paragraph.
    Span(&'a Span),
}

impl<'a> From<&'a str> for BlockTarget<'a> {
    fn from(value: &'a str) -> Self {
        BlockTarget::Text(value)
    }
}

impl<'a> From<&'a Block> for BlockTarget<'a> {
    fn from(value: &'a Block) -> Self {
        BlockTarget::Block(value)
    }
}

impl<'a> From<&'a Span> for BlockTarget<'a> {
    fn from(value: &'a Span) -> Self {
        BlockTarget::Span(value)
    }
}

/// A resolved paragraph anchor.
#[derive(Debug, Clone)]
pub struct Located {
    /// Story name.
    pub story: String,
    /// Package member.
    pub part: String,
    /// The element (a paragraph unless `kind` says otherwise).
    pub element: NodeId,
    /// Kind of the located block.
    pub kind: BlockKind,
}

fn stale(reason: &'static str, message: impl Into<String>) -> Error {
    Error::refusal(RefusalKind::TargetNotFound, reason, message)
}

/// Upstream `require_anchor_owner`.
pub fn require_owner(document: &Document, target: BlockTarget<'_>, argument: &str) -> Result<()> {
    match target {
        BlockTarget::Text(_) => Ok(()),
        BlockTarget::Span(span) => {
            if span.document_id() != document.id() {
                return Err(Error::refusal(
                    RefusalKind::BoundaryViolation,
                    "foreign_span",
                    format!(
                        "{argument} belongs to a different document; re-find the text in the document passed to this operation"
                    ),
                ));
            }
            Ok(())
        }
        BlockTarget::Block(block) => match block.handle.document {
            None => Err(stale(
                "not_live_block",
                format!("{argument} is not a live block; reacquire it with blocks() or outline()"),
            )),
            Some(id) if id != document.id() => Err(Error::refusal(
                RefusalKind::BoundaryViolation,
                "foreign_block",
                format!(
                    "{argument} belongs to a different document; reacquire the block from the document passed to this operation"
                ),
            )),
            Some(_) => Ok(()),
        },
    }
}

/// Upstream `_require_current_mutation_view`.
pub fn require_current_view(target: BlockTarget<'_>) -> Result<()> {
    let (view, label) = match target {
        BlockTarget::Text(_) => return Ok(()),
        BlockTarget::Span(span) => (span.view(), "Span"),
        BlockTarget::Block(block) => (block.view(), "Block"),
    };
    if view != View::Current {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "historical_view_target",
            format!(
                "live {label} was captured from view={:?}; historical projections are inspection-only as mutation \
                 anchors. Reacquire the intended target with view=\"current\" and retry",
                view.as_str()
            ),
        ));
    }
    Ok(())
}

/// Locate without the protection gate (upstream `_locate_anchor_paragraph`),
/// returning blocks of either kind.
pub fn locate_block(document: &Document, target: BlockTarget<'_>) -> Result<Located> {
    require_owner(document, target, "anchor")?;
    match target {
        BlockTarget::Text(text) => {
            let span = document.find_one(text, &FindOptions::default())?;
            locate_span(document, &span)
        }
        BlockTarget::Span(span) => locate_span(document, span),
        BlockTarget::Block(block) => locate_live_block(document, block),
    }
}

fn locate_span(document: &Document, span: &Span) -> Result<Located> {
    let tree = document.package().xml(&span.part)?;
    validate_fresh(document, tree, span, None)?;
    let Some(paragraph) = span.atoms.first().and_then(|atom| atom.paragraph) else {
        return Err(stale(
            "not_in_paragraph",
            "span anchor is not inside a paragraph",
        ));
    };
    if span.crosses_paragraphs()
        || span
            .atoms
            .iter()
            .any(|atom| atom.paragraph != Some(paragraph))
    {
        return Err(Error::refusal(
            RefusalKind::BoundaryViolation,
            "multi_paragraph_target",
            "this operation requires a target wholly within one paragraph and cannot infer first-versus-last intent \
             from a cross-paragraph selection; select text wholly within one paragraph or supply an explicit live \
             Block endpoint",
        ));
    }
    Ok(Located {
        story: span.story().to_string(),
        part: span.part.clone(),
        element: paragraph,
        kind: BlockKind::Paragraph,
    })
}

fn locate_live_block(document: &Document, block: &Block) -> Result<Located> {
    let Some(story) = story::stories(document)
        .into_iter()
        .find(|s| s.name == block.story)
    else {
        return Err(stale(
            "story_removed",
            format!(
                "live block is stale: story part {:?} was removed or replaced",
                block.story
            ),
        ));
    };
    let tree: &XmlTree = document.package().xml(&story.part)?;
    if tree.root() != block.handle.root || story.part != block.handle.part {
        return Err(stale(
            "story_replaced",
            format!(
                "live block is stale: story part {:?} was removed or replaced",
                block.story
            ),
        ));
    }
    let element = block.element;
    if !tree.contains(element)
        || tree.parent(element) != block.handle.parent
        || tree.struct_gen(element) != block.handle.struct_gen
    {
        return Err(stale(
            "block_moved",
            "live block is stale: its exact document element was detached or reparented",
        ));
    }
    let candidates: Vec<BlockKind> = iter_block_elements(tree, &block.story)?
        .into_iter()
        .filter(|candidate| candidate.element == element)
        .map(|candidate| candidate.kind)
        .collect();
    if candidates.len() != 1 {
        return Err(stale(
            "block_unreachable",
            "live block is stale: its exact element is no longer reachable once through supported story traversal",
        ));
    }
    if candidates[0] != block.kind || container_elements(tree, element) != block.handle.containers {
        return Err(stale(
            "block_context_changed",
            "live block is stale: its kind or containing story structure changed",
        ));
    }
    Ok(Located {
        story: block.story.clone(),
        part: story.part,
        element,
        kind: candidates[0],
    })
}

/// Upstream `_resolve_anchor_paragraph`: owner, freshness, current view,
/// paragraph kind and the paragraph-mutation protection gate.
pub fn resolve_anchor_paragraph(document: &Document, target: BlockTarget<'_>) -> Result<Located> {
    require_owner(document, target, "anchor")?;
    let located = locate_paragraph(document, target)?;
    require_current_view(target)?;
    refuse_if_protected(
        document,
        "insert or remove paragraphs",
        OperationClass::BodyContent,
    )?;
    Ok(located)
}

/// [`locate_block`] restricted to paragraphs (upstream `_paragraph_block`).
pub fn locate_paragraph(document: &Document, target: BlockTarget<'_>) -> Result<Located> {
    let located = locate_block(document, target)?;
    if located.kind != BlockKind::Paragraph {
        return Err(Error::refusal(
            RefusalKind::UnsupportedStructure,
            "table_anchor",
            "target addresses a table block; block operations anchor on paragraphs",
        ));
    }
    Ok(located)
}
