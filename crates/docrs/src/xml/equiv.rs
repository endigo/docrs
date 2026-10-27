//! Semantic XML equivalence (upstream `xml_equivalent`).
//!
//! Element and attribute names compare by namespace URI plus local name, so
//! prefix choices never matter while URIs always do. Attribute order is
//! insignificant; child order is significant; character data compares
//! verbatim, whitespace included. Comments and processing instructions are
//! nodes (inside elements and in the prolog/epilog). The XML declaration is
//! not compared.
//!
//! **Compatibility deviation (deliberate correction):** upstream compares
//! QName-valued attribute *values* textually. This port resolves the prefixes
//! in the known QName-valued attributes — `xsi:type`, the
//! markup-compatibility attributes (`mc:Ignorable`, `mc:MustUnderstand`,
//! `mc:ProcessContent`, `mc:PreserveElements`, `mc:PreserveAttributes`) and
//! `mc:Choice/@Requires` — and compares the resolved names. A prefix rebound
//! to a different URI is therefore a change, and two prefixes bound to the
//! same URI are equal.

use std::borrow::Cow;

use super::tree::{NodeData, NodeId, XmlTree};
use super::{MC_NS, XSI_NS};

/// True when two parsed documents are semantically identical.
pub fn trees_equivalent(a: &XmlTree, b: &XmlTree) -> bool {
    let (a_prolog, a_epilog) = doc_level(a);
    let (b_prolog, b_epilog) = doc_level(b);
    if a_prolog != b_prolog || a_epilog != b_epilog {
        return false;
    }
    let mut stack = vec![(a.root(), b.root())];
    while let Some((x, y)) = stack.pop() {
        if !shallow_equal(a, x, b, y) {
            return false;
        }
        let xs = normalized_children(a, x);
        let ys = normalized_children(b, y);
        if xs.len() != ys.len() {
            return false;
        }
        for (cx, cy) in xs.into_iter().zip(ys) {
            match (cx, cy) {
                (Child::Text(tx), Child::Text(ty)) => {
                    if tx != ty {
                        return false;
                    }
                }
                (Child::Node(nx), Child::Node(ny)) => match (a.data(nx), b.data(ny)) {
                    (NodeData::Element(_), NodeData::Element(_)) => stack.push((nx, ny)),
                    (NodeData::Comment(cx), NodeData::Comment(cy)) => {
                        if cx != cy {
                            return false;
                        }
                    }
                    (
                        NodeData::Pi {
                            target: tx,
                            data: dx,
                        },
                        NodeData::Pi {
                            target: ty,
                            data: dy,
                        },
                    ) => {
                        if tx != ty || dx != dy {
                            return false;
                        }
                    }
                    _ => return false,
                },
                _ => return false,
            }
        }
    }
    true
}

#[derive(PartialEq, Eq, Debug)]
enum DocNode<'a> {
    Comment(&'a str),
    Pi(&'a str, &'a str),
}

fn doc_level(tree: &XmlTree) -> (Vec<DocNode<'_>>, Vec<DocNode<'_>>) {
    let mut prolog = Vec::new();
    let mut epilog = Vec::new();
    let mut after_root = false;
    for node in &tree.doc_nodes {
        if *node == tree.root() {
            after_root = true;
            continue;
        }
        let item = match tree.data(*node) {
            NodeData::Comment(text) => DocNode::Comment(text),
            NodeData::Pi { target, data } => DocNode::Pi(target, data),
            _ => continue,
        };
        if after_root {
            epilog.push(item)
        } else {
            prolog.push(item)
        }
    }
    (prolog, epilog)
}

enum Child<'a> {
    Text(Cow<'a, str>),
    Node(NodeId),
}

fn normalized_children(tree: &XmlTree, id: NodeId) -> Vec<Child<'_>> {
    let mut out: Vec<Child<'_>> = Vec::new();
    for child in tree.children(id) {
        match tree.data(*child) {
            NodeData::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                if let Some(Child::Text(existing)) = out.last_mut() {
                    existing.to_mut().push_str(text);
                } else {
                    out.push(Child::Text(Cow::Borrowed(text)));
                }
            }
            _ => out.push(Child::Node(*child)),
        }
    }
    out
}

fn shallow_equal(a: &XmlTree, x: NodeId, b: &XmlTree, y: NodeId) -> bool {
    let (Some(ex), Some(ey)) = (a.element(x), b.element(y)) else {
        return false;
    };
    if ex.name.local != ey.name.local || a.ns_uri(x) != b.ns_uri(y) {
        return false;
    }
    if ex.attrs.len() != ey.attrs.len() {
        return false;
    }
    let is_choice = a.ns_uri(x) == Some(MC_NS) && &*ex.name.local == "Choice";
    for attr in &ex.attrs {
        let attr_ns = attr.ns.map(|ns| a.uri(ns));
        let Some(other) = ey.attrs.iter().find(|candidate| {
            candidate.name.local == attr.name.local && candidate.ns.map(|ns| b.uri(ns)) == attr_ns
        }) else {
            return false;
        };
        let kind = qname_value_kind(attr_ns, &attr.name.local, is_choice);
        match kind {
            None => {
                if attr.value != other.value {
                    return false;
                }
            }
            Some(kind) => {
                if resolve_tokens(a, x, &attr.value, kind)
                    != resolve_tokens(b, y, &other.value, kind)
                {
                    return false;
                }
            }
        }
    }
    true
}

#[derive(Clone, Copy)]
enum ValueKind {
    /// One QName (`xsi:type`).
    QName,
    /// Whitespace-separated prefixes.
    Prefixes,
    /// Whitespace-separated `prefix:local` or `prefix:*` names.
    QNames,
}

fn qname_value_kind(ns: Option<&str>, local: &str, on_choice: bool) -> Option<ValueKind> {
    match (ns, local) {
        (Some(XSI_NS), "type") => Some(ValueKind::QName),
        (Some(MC_NS), "Ignorable" | "MustUnderstand") => Some(ValueKind::Prefixes),
        (Some(MC_NS), "ProcessContent" | "PreserveElements" | "PreserveAttributes") => {
            Some(ValueKind::QNames)
        }
        (None, "Requires") if on_choice => Some(ValueKind::Prefixes),
        _ => None,
    }
}

fn resolve_tokens(tree: &XmlTree, at: NodeId, value: &str, kind: ValueKind) -> Vec<String> {
    let resolve = |prefix: Option<&str>| -> String {
        match tree.lookup_prefix(at, prefix) {
            Some(Some(ns)) => tree.uri(ns).to_string(),
            Some(None) => String::new(),
            None => format!("?unbound:{}", prefix.unwrap_or("")),
        }
    };
    match kind {
        ValueKind::QName => {
            let token = value.trim();
            let (prefix, local) = match token.split_once(':') {
                Some((p, l)) => (Some(p), l),
                None => (None, token),
            };
            vec![format!("{{{}}}{local}", resolve(prefix))]
        }
        ValueKind::Prefixes => value
            .split_ascii_whitespace()
            .map(|prefix| resolve(Some(prefix)))
            .collect(),
        ValueKind::QNames => value
            .split_ascii_whitespace()
            .map(|token| match token.split_once(':') {
                Some((p, l)) => format!("{{{}}}{l}", resolve(Some(p))),
                None => format!("?invalid:{token}"),
            })
            .collect(),
    }
}
