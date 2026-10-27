//! Serialization of an [`XmlTree`] back to UTF-8 bytes.
//!
//! Namespace declarations are written exactly where the arena holds them, in
//! their original order, and every name keeps its lexical prefix; the output
//! therefore reproduces the source's namespace scopes. Character data and
//! attribute values are escaped so that a re-parse yields identical decoded
//! values (tabs, newlines and carriage returns in attributes are written as
//! character references). The XML declaration is kept when the source had
//! one, with its encoding rewritten to UTF-8.

use super::tree::{NodeData, NodeId, XmlTree};

/// Serialize the whole document.
pub fn serialize(tree: &XmlTree) -> Vec<u8> {
    let mut out = String::new();
    if let Some(decl) = tree.decl() {
        out.push_str("<?xml version=\"");
        out.push_str(&decl.version);
        out.push_str("\" encoding=\"UTF-8\"");
        if let Some(standalone) = decl.standalone {
            out.push_str(if standalone {
                " standalone=\"yes\""
            } else {
                " standalone=\"no\""
            });
        }
        out.push_str("?>\r\n");
    }
    for node in &tree.doc_nodes {
        write_node(tree, *node, &mut out);
    }
    out.into_bytes()
}

/// `text` escaped for an attribute value or character data.
pub(crate) fn escape_attr_to_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_attr(text, &mut out);
    out
}

/// Serialize one subtree (no declaration).
pub fn serialize_node(tree: &XmlTree, id: NodeId) -> String {
    let mut out = String::new();
    write_node(tree, id, &mut out);
    out
}

fn write_node(tree: &XmlTree, id: NodeId, out: &mut String) {
    // Iterative to keep deep trees off the call stack.
    enum Step {
        Open(NodeId),
        Close(NodeId),
    }
    let mut stack = vec![Step::Open(id)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Open(node) => match tree.data(node) {
                NodeData::Text(text) => escape_text(text, out),
                NodeData::Comment(text) => {
                    out.push_str("<!--");
                    out.push_str(text);
                    out.push_str("-->");
                }
                NodeData::Pi { target, data } => {
                    out.push_str("<?");
                    out.push_str(target);
                    if !data.is_empty() {
                        out.push(' ');
                        out.push_str(data);
                    }
                    out.push_str("?>");
                }
                NodeData::Element(element) => {
                    out.push('<');
                    push_qname(element.name.prefix.as_deref(), &element.name.local, out);
                    for decl in &element.ns_decls {
                        out.push_str(" xmlns");
                        if let Some(prefix) = &decl.prefix {
                            out.push(':');
                            out.push_str(prefix);
                        }
                        out.push_str("=\"");
                        if let Some(uri) = decl.uri {
                            escape_attr(tree.uri(uri), out);
                        }
                        out.push('"');
                    }
                    for attr in &element.attrs {
                        out.push(' ');
                        push_qname(attr.name.prefix.as_deref(), &attr.name.local, out);
                        out.push_str("=\"");
                        escape_attr(&attr.value, out);
                        out.push('"');
                    }
                    let children = tree.children(node);
                    if children.is_empty() {
                        out.push_str("/>");
                    } else {
                        out.push('>');
                        stack.push(Step::Close(node));
                        for child in children.iter().rev() {
                            stack.push(Step::Open(*child));
                        }
                    }
                }
            },
            Step::Close(node) => {
                if let Some(element) = tree.element(node) {
                    out.push_str("</");
                    push_qname(element.name.prefix.as_deref(), &element.name.local, out);
                    out.push('>');
                }
            }
        }
    }
}

fn push_qname(prefix: Option<&str>, local: &str, out: &mut String) {
    if let Some(prefix) = prefix {
        out.push_str(prefix);
        out.push(':');
    }
    out.push_str(local);
}

fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}

fn escape_attr(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}
