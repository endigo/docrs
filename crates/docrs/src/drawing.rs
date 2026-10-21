//! DrawingML drawings (upstream `docx/drawing/__init__.py`, MIT; see
//! `NOTICE.md`): picture detection, the embedded image, and picture
//! replacement.
//!
//! A [`Drawing`] is a live handle on one `w:drawing` element (document
//! identity, member, node id, freshness evidence) like the authoring
//! handles. [`Drawing::replace_picture`] keeps upstream's checks and their
//! order — no picture (`invalid_argument`), linked picture
//! (`unsupported_structure`), then the body-content protection gate — and
//! always writes a new image part, so other drawings sharing the old part
//! keep it; the old relationship stays behind.

use crate::authoring::{A, Handle, PIC, R, Run, WP, add_image_part};
use crate::image::Image;
use crate::package::rels::{base_uri, rt};
use crate::protection::{OperationClass, refuse_if_protected};
use crate::txn::{RelTarget, Txn};
use crate::wml::W;
use crate::xml::{NodeId, XmlTree};
use crate::{Document, Error, RefusalKind, Result};

/// A live `w:drawing` handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawing {
    pub(crate) h: Handle,
}

/// Outcome of [`Drawing::replace_picture`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PictureReplacement {
    /// The new relationship id every blip now embeds.
    pub r_id: String,
    /// The new image part (member name).
    pub image_part: String,
}

fn children_named<'a>(
    tree: &'a XmlTree,
    id: NodeId,
    ns: &'a str,
    local: &'a str,
) -> impl Iterator<Item = NodeId> + 'a {
    tree.child_elements(id)
        .filter(move |c| tree.is(*c, ns, local))
}

/// `./wp:inline|./wp:anchor` `/a:graphic/a:graphicData/pic:pic` exists.
fn has_picture_xml(tree: &XmlTree, drawing: NodeId) -> bool {
    tree.child_elements(drawing)
        .filter(|c| tree.is(*c, WP, "inline") || tree.is(*c, WP, "anchor"))
        .flat_map(|frame| children_named(tree, frame, A, "graphic").collect::<Vec<_>>())
        .flat_map(|graphic| children_named(tree, graphic, A, "graphicData").collect::<Vec<_>>())
        .any(|data| children_named(tree, data, PIC, "pic").next().is_some())
}

/// `.//pic:blipFill/a:blip` in document order.
fn blips(tree: &XmlTree, drawing: NodeId) -> Vec<NodeId> {
    tree.descendants(drawing)
        .filter(|node| *node != drawing && tree.is(*node, PIC, "blipFill"))
        .collect::<Vec<_>>()
        .into_iter()
        .flat_map(|fill| children_named(tree, fill, A, "blip").collect::<Vec<_>>())
        .collect()
}

fn drawings_of(doc: &Document, part: &str, tree: &XmlTree, scope: NodeId) -> Vec<Drawing> {
    tree.descendants(scope)
        .filter(|node| tree.is(*node, W, "drawing"))
        .map(|node| Drawing {
            h: Handle::capture(doc, part, tree, node),
        })
        .collect()
}

impl Document {
    /// Every `w:drawing` in the main document part, in document order.
    pub fn drawings(&self) -> Result<Vec<Drawing>> {
        let part = self.package().main_part_name().to_string();
        self.drawings_in(&part)
    }

    /// Every `w:drawing` in story part `part` (e.g. `word/header1.xml`), in
    /// document order.
    pub fn drawings_in(&self, part: &str) -> Result<Vec<Drawing>> {
        let member = self.package().resolve_member(part).ok_or_else(|| {
            Error::refusal(
                RefusalKind::TargetNotFound,
                "unknown_part",
                format!("no part named {part:?}"),
            )
        })?;
        let member = member.to_string();
        let tree = self.package().xml(&member)?;
        Ok(drawings_of(self, &member, tree, tree.root()))
    }
}

impl Run {
    /// The run's direct `w:drawing` children (upstream
    /// `Run.iter_inner_content` drawings).
    pub fn drawings(&self, doc: &Document) -> Result<Vec<Drawing>> {
        let tree = self.h.check(doc, "r", "run")?;
        Ok(children_named(tree, self.h.element, W, "drawing")
            .map(|node| Drawing {
                h: Handle::capture(doc, &self.h.part, tree, node),
            })
            .collect())
    }
}

impl Drawing {
    fn live<'d>(&self, doc: &'d Document) -> Result<&'d XmlTree> {
        self.h.check(doc, "drawing", "drawing")
    }

    /// Story part holding the drawing.
    pub fn part(&self) -> &str {
        &self.h.part
    }

    /// Upstream `Drawing.has_picture`: an inline or floating embedded
    /// picture (`pic:pic`) is present.
    pub fn has_picture(&self, doc: &Document) -> Result<bool> {
        Ok(has_picture_xml(self.live(doc)?, self.h.element))
    }

    /// Upstream `Drawing.image`: the image the first
    /// `pic:blipFill/a:blip/@r:embed` relates to, characterized.
    pub fn image(&self, doc: &Document) -> Result<Image> {
        let tree = self.live(doc)?;
        let r_id = blips(tree, self.h.element)
            .into_iter()
            .find_map(|blip| tree.attr(blip, Some(R), "embed").map(str::to_string))
            .ok_or_else(|| {
                Error::invalid_argument("no_picture", "drawing does not contain a picture")
            })?;
        let source = format!("/{}", self.h.part);
        let rel = doc
            .package()
            .relationships_at(&source)?
            .into_iter()
            .find(|rel| rel.id == r_id && !rel.external)
            .ok_or_else(|| {
                Error::refusal(
                    RefusalKind::TargetNotFound,
                    "missing_image_relationship",
                    format!(
                        "the picture embeds {r_id:?}, which is not an internal relationship of {}",
                        self.h.part
                    ),
                )
            })?;
        let target = rel.target_part_name(base_uri(&source));
        let member = doc.package().resolve_member(&target).ok_or_else(|| {
            Error::refusal(
                RefusalKind::TargetNotFound,
                "missing_image_part",
                format!("relationship {r_id:?} targets missing part {target}"),
            )
        })?;
        let part = doc
            .package()
            .part(member)
            .ok_or_else(|| Error::internal("image_part", "resolved image part vanished"))?;
        Image::from_blob(&part.current_bytes())
    }

    /// Upstream `Drawing.replace_picture(image)`: swap the picture's bytes,
    /// keeping its size and position. A new image part is always written
    /// and every `pic:blipFill/a:blip` of this drawing re-embedded to it.
    pub fn replace_picture(&self, doc: &mut Document, image: &[u8]) -> Result<PictureReplacement> {
        let tree = self.live(doc)?;
        if !has_picture_xml(tree, self.h.element) {
            return Err(Error::invalid_argument(
                "no_picture",
                "drawing does not contain a picture",
            ));
        }
        let all_blips = blips(tree, self.h.element);
        if all_blips
            .iter()
            .any(|blip| tree.attr(*blip, Some(R), "link").is_some())
        {
            return Err(Error::refusal(
                RefusalKind::UnsupportedStructure,
                "linked_picture",
                "drawing is a linked picture; nothing was changed",
            ));
        }
        refuse_if_protected(doc, "replace a picture", OperationClass::BodyContent)?;
        let image = Image::from_blob(image)?;
        let mut txn = Txn::new(doc);
        let image_part = add_image_part(&mut txn, &image)?;
        let r_id = txn.relate_to(&self.h.part, rt::IMAGE, RelTarget::Part(&image_part))?;
        let tree = txn.tree_mut(&self.h.part)?;
        for blip in all_blips {
            tree.set_attr(blip, Some(R), "embed", &r_id)?;
        }
        let staged = txn.finish()?;
        doc.commit(staged)?;
        Ok(PictureReplacement { r_id, image_part })
    }
}
