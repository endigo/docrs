//! Strict, namespace-aware XML 1.0 parser producing an [`XmlTree`].
//!
//! The parser enforces well-formedness and Namespaces-in-XML constraints at
//! least as strictly as libxml2 (the upstream oracle's parser), with the
//! security posture upstream relies on:
//!
//! * DOCTYPE declarations are refused outright (no DTD, no entity expansion,
//!   no external fetches);
//! * undeclared prefixes, reserved-namespace misuse, empty prefixed
//!   declarations and duplicate attributes (lexical or expanded) are errors;
//! * line endings are normalized and attribute values are whitespace-normalized
//!   exactly as the XML specification requires, so decoded values match lxml.
//!
//! Resource budgets ([`Limits`]) are checked as content is consumed and
//! surface as [`XmlError::Limit`], never as malformed input.

use std::collections::HashSet;

use super::XmlError;
use super::chars::{is_name_char, is_name_start_char, is_ncname, is_xml_char, is_xml_space};
use super::decode::decode;
use super::tree::{
    Attr, Element, NodeData, NodeId, NsDecl, QName, XmlDecl, XmlTree, check_reserved_binding,
};
use crate::{Error, Limits};

/// Parse `bytes` (any supported encoding) into a tree.
pub fn parse(bytes: &[u8], limits: &Limits) -> Result<XmlTree, XmlError> {
    let (text, encoding) = decode(bytes)?;
    let text = normalize_line_endings(text);
    let mut parser = Parser {
        s: &text,
        pos: 0,
        limits,
        tree: XmlTree::empty(encoding),
        nodes: 0,
    };
    parser.document()?;
    Ok(parser.tree)
}

fn normalize_line_endings(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

struct RawAttr<'a> {
    name: &'a str,
    value: String,
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
    limits: &'a Limits,
    tree: XmlTree,
    nodes: u64,
}

impl<'a> Parser<'a> {
    fn err(&self, reason: &'static str, message: impl AsRef<str>) -> XmlError {
        let consumed = &self.s[..self.pos.min(self.s.len())];
        let line = consumed.matches('\n').count() + 1;
        let column = consumed
            .rsplit('\n')
            .next()
            .map(|s| s.chars().count())
            .unwrap_or(0)
            + 1;
        XmlError::malformed(
            reason,
            format!("{}, line {line}, column {column}", message.as_ref()),
        )
    }

    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn starts(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self, c: char) {
        self.pos += c.len_utf8();
    }

    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_xml_space(c) {
                self.bump(c);
            } else {
                break;
            }
        }
        self.pos != start
    }

    fn expect(&mut self, token: &str, what: &str) -> Result<(), XmlError> {
        if self.starts(token) {
            self.pos += token.len();
            Ok(())
        } else {
            Err(self.err("syntax", format!("expected {what}")))
        }
    }

    fn count_node(&mut self) -> Result<(), XmlError> {
        self.nodes += 1;
        if self.nodes > self.limits.max_xml_nodes {
            return Err(XmlError::Limit(Error::limit(
                "max_xml_nodes",
                self.limits.max_xml_nodes,
                self.nodes,
            )));
        }
        Ok(())
    }

    fn name(&mut self) -> Result<&'a str, XmlError> {
        let start = self.pos;
        match self.peek() {
            Some(c) if is_name_start_char(c) => self.bump(c),
            Some(c) if !is_xml_char(c) => {
                return Err(self.err(
                    "invalid_char",
                    format!("Char 0x{:X} out of allowed range", c as u32),
                ));
            }
            _ => return Err(self.err("syntax", "name expected")),
        }
        while let Some(c) = self.peek() {
            if is_name_char(c) {
                self.bump(c);
            } else {
                break;
            }
        }
        Ok(&self.s[start..self.pos])
    }

    fn check_chars(&self, text: &str, what: &str) -> Result<(), XmlError> {
        if let Some(c) = text.chars().find(|c| !is_xml_char(*c)) {
            return Err(self.err(
                "invalid_char",
                format!("{what} invalid Char value {}", c as u32),
            ));
        }
        Ok(())
    }

    fn document(&mut self) -> Result<(), XmlError> {
        if self.starts("<?xml") && self.s[5..].chars().next().is_some_and(is_xml_space) {
            self.xml_decl()?;
        }
        loop {
            self.skip_ws();
            if self.pos >= self.s.len() {
                return Err(self.err("no_root", "Document is empty"));
            }
            if self.starts("<!--") {
                let node = self.comment()?;
                self.tree.doc_nodes.push(node);
            } else if self.starts("<!DOCTYPE") {
                return Err(self.err(
                    "dtd_prohibited",
                    "DOCTYPE declarations are prohibited in package parts",
                ));
            } else if self.starts("<?") {
                let node = self.pi()?;
                self.tree.doc_nodes.push(node);
            } else if self.starts("<") {
                break;
            } else {
                return Err(self.err("syntax", "Start tag expected, '<' not found"));
            }
        }
        let root = self.element_tree()?;
        self.tree.root = root;
        self.tree.doc_nodes.push(root);
        loop {
            self.skip_ws();
            if self.pos >= self.s.len() {
                return Ok(());
            }
            if self.starts("<!--") {
                let node = self.comment()?;
                self.tree.doc_nodes.push(node);
            } else if self.starts("<?") {
                let node = self.pi()?;
                self.tree.doc_nodes.push(node);
            } else {
                return Err(self.err("extra_content", "Extra content at the end of the document"));
            }
        }
    }

    fn xml_decl(&mut self) -> Result<(), XmlError> {
        self.pos += "<?xml".len();
        self.skip_ws();
        self.expect("version", "version in XML declaration")?;
        let version = self.pseudo_attr()?;
        let valid_version = version
            .strip_prefix("1.")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
        if !valid_version {
            return Err(self.err("syntax", format!("unsupported XML version {version:?}")));
        }
        let mut encoding = None;
        let mut standalone = None;
        let had_space = self.skip_ws();
        if had_space && self.starts("encoding") {
            self.pos += "encoding".len();
            let value = self.pseudo_attr()?;
            let valid = value
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
            if !valid {
                return Err(self.err("syntax", "invalid encoding name"));
            }
            encoding = Some(value);
            self.skip_ws();
        }
        if self.starts("standalone") {
            self.pos += "standalone".len();
            let value = self.pseudo_attr()?;
            standalone = Some(match value.as_str() {
                "yes" => true,
                "no" => false,
                _ => return Err(self.err("syntax", "standalone accepts only 'yes' or 'no'")),
            });
            self.skip_ws();
        }
        self.expect("?>", "'?>' closing the XML declaration")?;
        self.tree.decl = Some(XmlDecl {
            version,
            encoding,
            standalone,
        });
        Ok(())
    }

    fn pseudo_attr(&mut self) -> Result<String, XmlError> {
        self.skip_ws();
        self.expect("=", "'='")?;
        self.skip_ws();
        let quote = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(self.err("syntax", "quoted value expected")),
        };
        self.bump(quote);
        let rest = self.rest();
        let end = rest
            .find(quote)
            .ok_or_else(|| self.err("syntax", "unterminated value"))?;
        let value = rest[..end].to_string();
        self.pos += end + 1;
        Ok(value)
    }

    fn comment(&mut self) -> Result<NodeId, XmlError> {
        self.pos += "<!--".len();
        let rest = self.rest();
        let end = rest
            .find("--")
            .ok_or_else(|| self.err("syntax", "Comment not terminated"))?;
        if !rest[end..].starts_with("-->") {
            return Err(self.err("syntax", "Double hyphen within comment"));
        }
        let content = &rest[..end];
        self.check_chars(content, "Comment")?;
        self.pos += end + 3;
        self.count_node()?;
        Ok(self.tree.push(NodeData::Comment(content.to_string()), None))
    }

    fn pi(&mut self) -> Result<NodeId, XmlError> {
        self.pos += 2;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(self.err(
                "syntax",
                "XML declaration allowed only at the start of the document",
            ));
        }
        if target.contains(':') {
            return Err(self.err("namespace", "colons are forbidden from PI names"));
        }
        let data = if self.starts("?>") {
            self.pos += 2;
            String::new()
        } else {
            if !self.skip_ws() {
                return Err(self.err("syntax", "ParsePI: PI space expected"));
            }
            let rest = self.rest();
            let end = rest
                .find("?>")
                .ok_or_else(|| self.err("syntax", "PI not terminated"))?;
            let data = &rest[..end];
            self.check_chars(data, "PI")?;
            self.pos += end + 2;
            data.to_string()
        };
        self.count_node()?;
        Ok(self.tree.push(
            NodeData::Pi {
                target: target.to_string(),
                data,
            },
            None,
        ))
    }

    fn reference(&mut self, out: &mut String) -> Result<(), XmlError> {
        self.pos += 1;
        let rest = self.rest();
        let end = rest
            .find(';')
            .filter(|end| *end <= 64)
            .ok_or_else(|| self.err("syntax", "EntityRef: expecting ';'"))?;
        let body = &rest[..end];
        let c = if let Some(hex) = body.strip_prefix("#x") {
            u32::from_str_radix(hex, 16)
                .ok()
                .filter(|_| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        } else if let Some(dec) = body.strip_prefix('#') {
            dec.parse::<u32>()
                .ok()
                .filter(|_| !dec.is_empty() && dec.chars().all(|c| c.is_ascii_digit()))
        } else {
            let c = match body {
                "lt" => '<',
                "gt" => '>',
                "amp" => '&',
                "apos" => '\'',
                "quot" => '"',
                _ => {
                    if is_ncname(body) || body.contains(':') {
                        return Err(
                            self.err("undefined_entity", format!("Entity '{body}' not defined"))
                        );
                    }
                    return Err(self.err("syntax", "EntityRef: expecting ';'"));
                }
            };
            out.push(c);
            self.pos += end + 1;
            return Ok(());
        };
        let value = c.ok_or_else(|| self.err("syntax", "malformed character reference"))?;
        match char::from_u32(value).filter(|c| is_xml_char(*c)) {
            Some(c) => out.push(c),
            None => {
                return Err(self.err(
                    "invalid_char",
                    format!("xmlParseCharRef: invalid xmlChar value {value}"),
                ));
            }
        }
        self.pos += end + 1;
        Ok(())
    }

    fn att_value(&mut self) -> Result<String, XmlError> {
        let quote = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(self.err("syntax", "AttValue: \" or ' expected")),
        };
        self.bump(quote);
        let mut value = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err("syntax", "AttValue: ' expected"));
            };
            match c {
                c if c == quote => {
                    self.bump(c);
                    break;
                }
                '<' => {
                    return Err(
                        self.err("syntax", "Unescaped '<' not allowed in attributes values")
                    );
                }
                '&' => self.reference(&mut value)?,
                '\t' | '\n' => {
                    value.push(' ');
                    self.bump(c);
                }
                c if !is_xml_char(c) => {
                    return Err(self.err(
                        "invalid_char",
                        format!("invalid Char value {} in attribute", c as u32),
                    ));
                }
                c => {
                    value.push(c);
                    self.bump(c);
                }
            }
            if value.len() as u64 > self.limits.max_text_node_bytes {
                return Err(XmlError::Limit(Error::limit(
                    "max_text_node_bytes",
                    self.limits.max_text_node_bytes,
                    value.len() as u64,
                )));
            }
        }
        Ok(value)
    }

    fn split_qname(&self, name: &'a str) -> Result<(Option<&'a str>, &'a str), XmlError> {
        match name.split_once(':') {
            None => Ok((None, name)),
            Some((prefix, local)) => {
                if is_ncname(prefix) && is_ncname(local) {
                    Ok((Some(prefix), local))
                } else {
                    Err(self.err("namespace", format!("Failed to parse QName '{name}'")))
                }
            }
        }
    }

    /// Parse a start tag; returns the new element and whether it was empty.
    fn start_tag(
        &mut self,
        parent: Option<NodeId>,
        depth: usize,
    ) -> Result<(NodeId, &'a str, bool), XmlError> {
        if depth as u64 > self.limits.max_xml_depth as u64 {
            return Err(XmlError::Limit(Error::limit(
                "max_xml_depth",
                self.limits.max_xml_depth as u64,
                depth as u64,
            )));
        }
        self.pos += 1;
        let raw_name = self.name()?;
        let mut raw_attrs: Vec<RawAttr<'a>> = Vec::new();
        let empty;
        loop {
            let had_space = self.skip_ws();
            if self.starts("/>") {
                self.pos += 2;
                empty = true;
                break;
            }
            if self.starts(">") {
                self.pos += 1;
                empty = false;
                break;
            }
            if self.pos >= self.s.len() {
                return Err(self.err("syntax", "Couldn't find end of Start Tag"));
            }
            if !had_space {
                return Err(self.err("syntax", "attributes construct error"));
            }
            let name = self.name()?;
            self.skip_ws();
            self.expect("=", "'=' after attribute name")?;
            self.skip_ws();
            let value = self.att_value()?;
            raw_attrs.push(RawAttr { name, value });
            if raw_attrs.len() as u64 > self.limits.max_xml_attributes as u64 {
                return Err(XmlError::Limit(Error::limit(
                    "max_xml_attributes",
                    self.limits.max_xml_attributes as u64,
                    raw_attrs.len() as u64,
                )));
            }
        }

        let mut seen: HashSet<&str> = HashSet::with_capacity(raw_attrs.len());
        for attr in &raw_attrs {
            if !seen.insert(attr.name) {
                return Err(self.err(
                    "duplicate_attribute",
                    format!("Attribute {} redefined", attr.name),
                ));
            }
        }

        let mut ns_decls: Vec<NsDecl> = Vec::new();
        let mut attrs_raw: Vec<(&'a str, String)> = Vec::new();
        for attr in raw_attrs {
            if attr.name == "xmlns" {
                check_reserved_binding(None, &attr.value)?;
                let uri = if attr.value.is_empty() {
                    None
                } else {
                    Some(self.tree.intern(&attr.value))
                };
                ns_decls.push(NsDecl { prefix: None, uri });
            } else if let Some(prefix) = attr.name.strip_prefix("xmlns:") {
                if !is_ncname(prefix) {
                    return Err(self.err(
                        "namespace",
                        format!("Failed to parse QName '{}'", attr.name),
                    ));
                }
                if attr.value.is_empty() {
                    return Err(self.err(
                        "namespace",
                        format!("xmlns:{prefix}: Empty XML namespace is not allowed"),
                    ));
                }
                check_reserved_binding(Some(prefix), &attr.value)?;
                let uri = self.tree.intern(&attr.value);
                ns_decls.push(NsDecl {
                    prefix: Some(prefix.into()),
                    uri: Some(uri),
                });
            } else {
                attrs_raw.push((attr.name, attr.value));
            }
        }
        if ns_decls.len() as u64 > self.limits.max_namespace_declarations as u64 {
            return Err(XmlError::Limit(Error::limit(
                "max_namespace_declarations",
                self.limits.max_namespace_declarations as u64,
                ns_decls.len() as u64,
            )));
        }

        let resolve = |tree: &XmlTree, decls: &[NsDecl], prefix: Option<&str>| {
            for decl in decls {
                if decl.prefix.as_deref() == prefix {
                    return Some(decl.uri);
                }
            }
            match parent {
                Some(parent) => tree.lookup_prefix(parent, prefix),
                None => match prefix {
                    None => Some(None),
                    Some("xml") => Some(tree.ns_id(super::XML_NS)),
                    Some(_) => None,
                },
            }
        };

        let (prefix, local) = self.split_qname(raw_name)?;
        if prefix == Some("xmlns") {
            return Err(self.err("namespace", "xmlns prefix used on an element"));
        }
        let ns = resolve(&self.tree, &ns_decls, prefix).ok_or_else(|| {
            self.err(
                "undeclared_prefix",
                format!(
                    "Namespace prefix {} on {local} is not defined",
                    prefix.unwrap_or("")
                ),
            )
        })?;

        let mut attrs = Vec::with_capacity(attrs_raw.len());
        let mut expanded: HashSet<(u32, &str)> = HashSet::new();
        for (name, value) in attrs_raw {
            let (attr_prefix, attr_local) = self.split_qname(name)?;
            let attr_ns = match attr_prefix {
                None => None,
                Some(p) => {
                    let bound = resolve(&self.tree, &ns_decls, Some(p)).flatten().ok_or_else(|| {
                        self.err("undeclared_prefix", format!("Namespace prefix {p} for {attr_local} on {local} is not defined"))
                    })?;
                    if !expanded.insert((bound.0, attr_local)) {
                        return Err(self.err(
                            "duplicate_attribute",
                            format!(
                                "Namespaced Attribute {attr_local} in '{}' redefined",
                                self.tree.uri(bound)
                            ),
                        ));
                    }
                    Some(bound)
                }
            };
            attrs.push(Attr {
                name: QName {
                    prefix: attr_prefix.map(Into::into),
                    local: attr_local.into(),
                },
                ns: attr_ns,
                value,
            });
        }

        self.count_node()?;
        let id = self.tree.push(
            NodeData::Element(Element {
                name: QName {
                    prefix: prefix.map(Into::into),
                    local: local.into(),
                },
                ns,
                ns_decls,
                attrs,
                children: Vec::new(),
            }),
            parent,
        );
        Ok((id, raw_name, empty))
    }

    fn flush_text(&mut self, parent: NodeId, text: &mut String) -> Result<(), XmlError> {
        if !text.is_empty() {
            self.count_node()?;
            self.tree
                .push(NodeData::Text(std::mem::take(text)), Some(parent));
        }
        Ok(())
    }

    fn push_text(&self, text: &mut String, piece: &str) -> Result<(), XmlError> {
        text.push_str(piece);
        if text.len() as u64 > self.limits.max_text_node_bytes {
            return Err(XmlError::Limit(Error::limit(
                "max_text_node_bytes",
                self.limits.max_text_node_bytes,
                text.len() as u64,
            )));
        }
        Ok(())
    }

    fn element_tree(&mut self) -> Result<NodeId, XmlError> {
        let (root, root_name, empty) = self.start_tag(None, 1)?;
        if empty {
            return Ok(root);
        }
        let mut stack: Vec<(NodeId, &'a str)> = vec![(root, root_name)];
        let mut text = String::new();
        while let Some(&(current, current_name)) = stack.last() {
            if self.pos >= self.s.len() {
                return Err(self.err(
                    "syntax",
                    format!("Premature end of data in tag {current_name}"),
                ));
            }
            if self.starts("</") {
                self.pos += 2;
                let name = self.name()?;
                self.skip_ws();
                self.expect(">", "'>' closing the end tag")?;
                if name != current_name {
                    return Err(self.err(
                        "tag_mismatch",
                        format!("Opening and ending tag mismatch: {current_name} and {name}"),
                    ));
                }
                self.flush_text(current, &mut text)?;
                stack.pop();
            } else if self.starts("<!--") {
                self.flush_text(current, &mut text)?;
                let node = self.comment()?;
                self.attach(current, node);
            } else if self.starts("<![CDATA[") {
                self.pos += "<![CDATA[".len();
                let rest = self.rest();
                let end = rest
                    .find("]]>")
                    .ok_or_else(|| self.err("syntax", "CData section not finished"))?;
                let content = &rest[..end];
                self.check_chars(content, "CData")?;
                self.pos += end + 3;
                self.push_text(&mut text, content)?;
            } else if self.starts("<?") {
                self.flush_text(current, &mut text)?;
                let node = self.pi()?;
                self.attach(current, node);
            } else if self.starts("<!") {
                return Err(self.err("syntax", "markup declarations are not allowed in content"));
            } else if self.starts("<") {
                self.flush_text(current, &mut text)?;
                let (child, child_name, empty) = self.start_tag(Some(current), stack.len() + 1)?;
                if !empty {
                    stack.push((child, child_name));
                }
            } else if self.starts("&") {
                let mut piece = String::new();
                self.reference(&mut piece)?;
                self.push_text(&mut text, &piece)?;
            } else {
                let rest = self.rest();
                let end = rest.find(['<', '&']).unwrap_or(rest.len());
                let segment = &rest[..end];
                if let Some(offset) = segment.find("]]>") {
                    self.pos += offset;
                    return Err(self.err("syntax", "Sequence ']]>' not allowed in content"));
                }
                self.check_chars(segment, "PCDATA")?;
                self.pos += end;
                self.push_text(&mut text, segment)?;
            }
        }
        Ok(root)
    }

    fn attach(&mut self, parent: NodeId, node: NodeId) {
        // Comments/PIs are created detached by `comment`/`pi`; link them here.
        self.tree.link_child(parent, node);
    }
}
