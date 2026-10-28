//! Tracked-change primitives shared by the editing operations: revision ids,
//! `w:ins`/`w:del` elements and the runs inside them, text pieces of a span.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::search::{Atom, slice_chars};
use crate::styles::py_int;
use crate::unicode::is_py_space;
use crate::wml::{W, is_w, iter_nodes, leading_text, set_leading_text, w_attr, w_child};
use crate::xml::{NodeId, XML_NS, XmlTree};
use crate::{Document, Error, Result};

/// Upstream `_next_revision_id`: one above the highest revision id in any
/// story (staged trees take precedence over live ones).
pub(crate) fn next_revision_id(
    document: &Document,
    overrides: &BTreeMap<String, XmlTree>,
) -> Result<i64> {
    next_revision_id_staged(document, |part| overrides.get(part))
}

/// [`next_revision_id`] with the staged trees looked up by part name.
pub(crate) fn next_revision_id_staged<'t>(
    document: &Document,
    staged: impl Fn(&str) -> Option<&'t XmlTree>,
) -> Result<i64> {
    const TAGS: &[&str] = &[
        "ins",
        "del",
        "moveFrom",
        "moveTo",
        "moveFromRangeStart",
        "moveToRangeStart",
        "rPrChange",
        "pPrChange",
        "tblPrChange",
        "sectPrChange",
        "cellIns",
        "cellDel",
        "cellMerge",
    ];
    let mut highest = 0i64;
    for story in crate::story::stories(document) {
        let tree: &XmlTree = match staged(&story.part) {
            Some(tree) => tree,
            None => document.package().xml(&story.part)?,
        };
        for node in tree.descendants(tree.root()) {
            if tree.ns_uri(node) == Some(W)
                && tree.local_name(node).is_some_and(|l| TAGS.contains(&l))
                && let Some(value) = w_attr(tree, node, "id")
                    .filter(|v| !v.is_empty())
                    .and_then(py_int)
            {
                highest = highest.max(value);
            }
        }
    }
    Ok(highest + 1)
}

/// Upstream `_pieces_in_range`.
pub(crate) fn pieces_in_range(
    pieces: &[(Option<NodeId>, String)],
    start: usize,
    end: usize,
) -> Vec<(Option<NodeId>, String)> {
    let mut out: Vec<(Option<NodeId>, String)> = Vec::new();
    let mut position = 0;
    for (run, text) in pieces {
        let length = text.chars().count();
        let lo = start.max(position);
        let hi = end.min(position + length);
        if hi > lo {
            let fragment = slice_chars(text, lo - position, hi - position);
            match out.last_mut() {
                Some((last_run, last_text)) if last_run == run => last_text.push_str(&fragment),
                _ => out.push((*run, fragment)),
            }
        }
        position += length;
    }
    out
}

/// Upstream `_hollowed_bookmarks(atoms, end_offset)`: names of the
/// bookmarks whose every text node the edit empties.
pub(crate) fn hollowed_bookmarks(tree: &XmlTree, atoms: &[Atom], end_offset: usize) -> Vec<String> {
    let Some(paragraph) = atoms.first().and_then(|a| a.paragraph) else {
        return Vec::new();
    };
    let mut emptied: HashSet<NodeId> = HashSet::new();
    if atoms.len() > 2 {
        emptied.extend(atoms[1..atoms.len() - 1].iter().map(|a| a.element));
    }
    if atoms.len() > 1 && end_offset >= atoms[atoms.len() - 1].text(tree).chars().count() {
        emptied.insert(atoms[atoms.len() - 1].element);
    }
    if emptied.is_empty() {
        return Vec::new();
    }
    let stream: Vec<NodeId> = iter_nodes(tree, paragraph).collect();
    let mut starts: HashMap<Option<String>, (usize, String)> = HashMap::new();
    let mut hollowed = Vec::new();
    for (position, node) in stream.iter().enumerate() {
        if is_w(tree, *node, "bookmarkStart") {
            starts.insert(
                w_attr(tree, *node, "id").map(str::to_string),
                (
                    position,
                    w_attr(tree, *node, "name").unwrap_or("").to_string(),
                ),
            );
        } else if is_w(tree, *node, "bookmarkEnd") {
            let Some((start, name)) = starts.get(&w_attr(tree, *node, "id").map(str::to_string))
            else {
                continue;
            };
            if name == "_GoBack" {
                continue;
            }
            let texts: Vec<NodeId> = stream[start + 1..position]
                .iter()
                .copied()
                .filter(|inner| {
                    (is_w(tree, *inner, "t") || is_w(tree, *inner, "delText"))
                        && !leading_text(tree, *inner).is_empty()
                })
                .collect();
            if !texts.is_empty() && texts.iter().all(|t| emptied.contains(t)) {
                hollowed.push(name.clone());
            }
        }
    }
    hollowed
}

/// Upstream `_set_preserved_text`: the element's text, with `xml:space`
/// kept only when an edge is whitespace.
pub(crate) fn set_preserved_text(tree: &mut XmlTree, element: NodeId, text: &str) -> Result<()> {
    set_leading_text(tree, element, text)?;
    let edge = text.chars().next().is_some_and(is_py_space)
        || text.chars().last().is_some_and(is_py_space);
    if edge {
        tree.set_attr(element, Some(XML_NS), "space", "preserve")?;
    } else {
        tree.remove_attr(element, Some(XML_NS), "space");
    }
    Ok(())
}

/// Insert detached `node` right after `anchor`.
pub(crate) fn insert_after(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "insertion anchor is detached"))?;
    let index = tree.index_in_parent(anchor).unwrap_or(0) + 1;
    tree.insert(parent, index, node)
}

/// Insert detached `node` right before `anchor`.
pub(crate) fn insert_before(tree: &mut XmlTree, anchor: NodeId, node: NodeId) -> Result<()> {
    let parent = tree
        .parent(anchor)
        .ok_or_else(|| Error::internal("detached_anchor", "insertion anchor is detached"))?;
    let index = tree.index_in_parent(anchor).unwrap_or(0);
    tree.insert(parent, index, node)
}

/// A detached `w:local` revision element (`ins`, `del`, ...) stamped with
/// its id, author and date.
pub(crate) fn new_revision(
    tree: &mut XmlTree,
    scope: NodeId,
    local: &str,
    id: i64,
    author: &str,
    date: &str,
) -> Result<NodeId> {
    let element = crate::oxml::w_el(tree, scope, local)?;
    crate::oxml::set_w(tree, element, "id", &id.to_string())?;
    crate::oxml::set_w(tree, element, "author", author)?;
    crate::oxml::set_w(tree, element, "date", date)?;
    Ok(element)
}

/// Upstream `blocks._stamp_paragraph_mark`: a `w:local` revision on the
/// paragraph mark (`w:pPr/w:rPr`).
pub(crate) fn stamp_paragraph_mark(
    tree: &mut XmlTree,
    paragraph: NodeId,
    local: &str,
    id: i64,
    author: &str,
    date: &str,
) -> Result<()> {
    let ppr = crate::oxml::get_or_add_ppr(tree, paragraph)?;
    let rpr = match w_child(tree, ppr, "rPr") {
        Some(existing) => existing,
        None => {
            let rpr = crate::oxml::w_el(tree, ppr, "rPr")?;
            crate::oxml::insert_before_successors(tree, ppr, rpr, &["sectPr", "pPrChange"])?;
            rpr
        }
    };
    let mark = new_revision(tree, rpr, local, id, author, date)?;
    tree.insert(rpr, 0, mark)
}

/// Upstream `blocks._named_bookmarks_in`: names of non-point, non-`_GoBack`
/// bookmarks with a marker in `paragraph`.
pub(crate) fn named_bookmarks_in(tree: &XmlTree, paragraph: NodeId) -> Vec<String> {
    let stream: Vec<NodeId> = iter_nodes(tree, paragraph).collect();
    let mut starts: HashMap<Option<String>, (usize, String)> = HashMap::new();
    let mut named: Vec<(Option<String>, String)> = Vec::new();
    for (position, node) in stream.iter().enumerate() {
        if is_w(tree, *node, "bookmarkStart") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let name = w_attr(tree, *node, "name").unwrap_or("").to_string();
            starts.insert(id.clone(), (position, name.clone()));
            if name != "_GoBack" {
                named.push((id, name));
            }
        } else if is_w(tree, *node, "bookmarkEnd") {
            let id = w_attr(tree, *node, "id").map(str::to_string);
            let Some((start, name)) = starts.remove(&id) else {
                continue;
            };
            if name == "_GoBack" {
                continue;
            }
            let has_text = stream[start + 1..position]
                .iter()
                .any(|inner| is_w(tree, *inner, "t") && !leading_text(tree, *inner).is_empty());
            if !has_text {
                named.retain(|(i, _)| *i != id);
            }
        }
    }
    named.into_iter().map(|(_, name)| name).collect()
}

/// Upstream `CT_RunTrackChange.add_tracked_run`.
pub(crate) fn add_tracked_run(
    tree: &mut XmlTree,
    revision: NodeId,
    text: &str,
    rpr: Option<NodeId>,
    deleted: bool,
) -> Result<()> {
    let run = crate::oxml::w_el(tree, revision, "r")?;
    if let Some(rpr) = rpr {
        let copy = tree.deep_clone(rpr);
        tree.insert(run, 0, copy)?;
    }
    if deleted {
        let del_text = crate::oxml::w_el(tree, run, "delText")?;
        if !text.is_empty() {
            let node = tree.create_text(text);
            tree.insert(del_text, 0, node)?;
        }
        if text.chars().next().is_some_and(is_py_space)
            || text.chars().last().is_some_and(is_py_space)
        {
            tree.set_attr(del_text, Some(XML_NS), "space", "preserve")?;
        }
        crate::oxml::append(tree, run, del_text)?;
    } else {
        crate::oxml::add_t(tree, run, text)?;
    }
    crate::oxml::append(tree, revision, run)
}
