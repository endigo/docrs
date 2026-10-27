//! WordprocessingML vocabulary and small tree helpers shared by the story,
//! search and editing layers.

use crate::xml::{NodeData, NodeId, XmlTree};

/// `w:` namespace.
pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
/// Office math.
pub const M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
/// DrawingML main.
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// DrawingML wordprocessingDrawing (`wp:`).
pub const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
/// DrawingML picture (`pic:`).
pub const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
/// Office relationships (`r:`).
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// Markup compatibility.
pub const MC: &str = crate::xml::MC_NS;
/// Word 2010 (`w14`).
pub const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";

/// Namespaces whose `mc:Choice` branches story traversal understands
/// (upstream `_SUPPORTED_MC_NAMESPACES`).
pub const SUPPORTED_MC_NAMESPACES: &[&str] = &[
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://schemas.openxmlformats.org/drawingml/2006/picture",
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
    "http://schemas.microsoft.com/office/word/2010/wordml",
    "http://schemas.microsoft.com/office/word/2012/wordml",
    "urn:schemas-microsoft-com:office:office",
    "urn:schemas-microsoft-com:office:word",
    "urn:schemas-microsoft-com:vml",
];

/// True when `id` is the element `w:local`.
pub fn is_w(tree: &XmlTree, id: NodeId, local: &str) -> bool {
    tree.is(id, W, local)
}

/// `w:local` attribute.
pub fn w_attr<'a>(tree: &'a XmlTree, id: NodeId, local: &str) -> Option<&'a str> {
    tree.attr(id, Some(W), local)
}

/// First direct `w:local` child element.
pub fn w_child(tree: &XmlTree, id: NodeId, local: &str) -> Option<NodeId> {
    tree.first_child_named(id, W, local)
}

/// All direct `w:local` children.
pub fn w_children<'a>(
    tree: &'a XmlTree,
    id: NodeId,
    local: &'a str,
) -> impl Iterator<Item = NodeId> + 'a {
    tree.child_elements(id)
        .filter(move |child| tree.is(*child, W, local))
}

/// Child nodes lxml iterates for an element: elements, comments and
/// processing instructions (character data is `.text`/`.tail`).
pub fn node_children(tree: &XmlTree, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    tree.children(id)
        .iter()
        .copied()
        .filter(|child| !matches!(tree.data(*child), NodeData::Text(_)))
}

/// lxml `element.text`: character data before the first child node.
pub fn leading_text(tree: &XmlTree, id: NodeId) -> String {
    let mut out = String::new();
    for child in tree.children(id) {
        match tree.data(*child) {
            NodeData::Text(text) => out.push_str(text),
            _ => break,
        }
    }
    out
}

/// python-docx `CT_R.text`: the run's text with tabs, breaks and hyphens as
/// characters.
pub fn run_text(tree: &XmlTree, run: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(run) {
        if tree.ns_uri(child) != Some(W) {
            continue;
        }
        match tree.local_name(child).unwrap_or("") {
            "t" => out.push_str(&leading_text(tree, child)),
            "tab" | "ptab" => out.push('\t'),
            "cr" => out.push('\n'),
            "br" => {
                if w_attr(tree, child, "type").unwrap_or("textWrapping") == "textWrapping" {
                    out.push('\n');
                }
            }
            "noBreakHyphen" => out.push('-'),
            _ => {}
        }
    }
    out
}

/// python-docx `CT_P.text`: the direct runs' and hyperlink runs' text.
pub fn paragraph_text(tree: &XmlTree, paragraph: NodeId) -> String {
    let mut out = String::new();
    for child in tree.child_elements(paragraph) {
        if is_w(tree, child, "r") {
            out.push_str(&run_text(tree, child));
        } else if is_w(tree, child, "hyperlink") {
            for run in w_children(tree, child, "r") {
                out.push_str(&run_text(tree, run));
            }
        }
    }
    out
}

/// Descendant-or-self elements in document order (lxml `iter()` over
/// elements, comments and PIs).
pub fn iter_nodes(tree: &XmlTree, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    tree.descendants(id)
        .filter(|node| !matches!(tree.data(*node), NodeData::Text(_)))
}

/// Upstream `_first_choice_children`: children with every
/// `mc:AlternateContent` collapsed to its first supported `mc:Choice`, else
/// its first `mc:Fallback`, else nothing.
pub fn first_choice_children(tree: &XmlTree, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    collect_choice_children(tree, id, &mut out);
    out
}

fn collect_choice_children(tree: &XmlTree, id: NodeId, out: &mut Vec<NodeId>) {
    for child in node_children(tree, id) {
        if tree.is(child, MC, "AlternateContent") {
            let mut selected = None;
            let mut fallback = None;
            for alternative in node_children(tree, child) {
                if tree.is(alternative, MC, "Choice") && choice_is_supported(tree, alternative) {
                    selected = Some(alternative);
                    break;
                }
                if tree.is(alternative, MC, "Fallback") && fallback.is_none() {
                    fallback = Some(alternative);
                }
            }
            if let Some(branch) = selected.or(fallback) {
                collect_choice_children(tree, branch, out);
            }
        } else {
            out.push(child);
        }
    }
}

fn choice_is_supported(tree: &XmlTree, choice: NodeId) -> bool {
    let requires = tree.attr(choice, None, "Requires").unwrap_or("");
    let prefixes: Vec<&str> = requires.split_whitespace().collect();
    !prefixes.is_empty()
        && prefixes.iter().all(|prefix| {
            // lxml's nsmap never contains the implicit `xml` binding.
            if *prefix == "xml" {
                return false;
            }
            match tree.lookup_prefix(choice, Some(prefix)) {
                Some(Some(ns)) => SUPPORTED_MC_NAMESPACES.contains(&tree.uri(ns)),
                _ => false,
            }
        })
}

/// lxml `element.text = value`: replace the character data before the first
/// child node (creating or removing the text node as needed).
pub(crate) fn set_leading_text(tree: &mut XmlTree, id: NodeId, value: &str) -> crate::Result<()> {
    let leading: Vec<NodeId> = tree
        .children(id)
        .iter()
        .copied()
        .take_while(|child| matches!(tree.data(*child), NodeData::Text(_)))
        .collect();
    match leading.split_first() {
        Some((first, rest)) => {
            tree.set_text(*first, value)?;
            for extra in rest {
                tree.detach(*extra)?;
            }
        }
        None => {
            if !value.is_empty() {
                let node = tree.create_text(value);
                tree.insert(id, 0, node)?;
            }
        }
    }
    Ok(())
}
