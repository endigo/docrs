//! Lossless, namespace-aware XML arena.
//!
//! The arena is the preservation authority for a parsed part. It keeps:
//!
//! * every node in document order — elements, merged character data
//!   (text, CDATA and references, like lxml's `.text`/`.tail`), comments and
//!   processing instructions, plus top-level prolog/epilog comments and PIs;
//! * each element's lexical prefix and namespace declarations exactly where
//!   they appeared, so serialization reproduces the original scopes and
//!   QName-valued attribute values (`xsi:type`, `mc:Ignorable`,
//!   `mc:Choice/@Requires`, unknown vocabularies) keep their meaning;
//! * attributes in original order with their resolved namespace.
//!
//! [`NodeId`]s index an append-only arena and are never reused, so a detached
//! node can never be confused with later content. Cloning a tree (used for
//! transaction staging) retains the IDs of the same logical nodes. Every
//! mutation bumps the touched node's revision and the tree's mutation counter;
//! handles use these as local freshness evidence.

use std::collections::HashMap;

use super::{XML_NS, XMLNS_NS, XmlError, decode::SourceEncoding};
use crate::{Error, RefusalKind, Result};

/// Arena index of a node. Never reused within one tree lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    /// The raw index (session-local; never a durable anchor).
    pub fn index(self) -> u32 {
        self.0
    }
}

/// Interned namespace URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NsId(pub(crate) u32);

/// A lexical qualified name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QName {
    /// Lexical prefix (`None` for unprefixed names).
    pub prefix: Option<Box<str>>,
    /// Local part.
    pub local: Box<str>,
}

impl QName {
    /// Lexical form (`prefix:local` or `local`).
    pub fn lexical(&self) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}:{}", self.local),
            None => self.local.to_string(),
        }
    }
}

/// An attribute with its resolved namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    /// Lexical name.
    pub name: QName,
    /// Resolved namespace (unprefixed attributes have none).
    pub ns: Option<NsId>,
    /// Normalized value (entity and character references resolved).
    pub value: String,
}

/// A namespace declaration (`xmlns` or `xmlns:prefix`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NsDecl {
    /// Declared prefix (`None` for the default namespace).
    pub prefix: Option<Box<str>>,
    /// Bound URI (`None` for `xmlns=""`, which undeclares the default).
    pub uri: Option<NsId>,
}

/// Element payload.
#[derive(Debug, Clone)]
pub struct Element {
    /// Lexical name.
    pub name: QName,
    /// Resolved namespace.
    pub ns: Option<NsId>,
    /// Namespace declarations in original order.
    pub ns_decls: Vec<NsDecl>,
    /// Attributes in original order.
    pub attrs: Vec<Attr>,
    pub(crate) children: Vec<NodeId>,
}

/// Node payload.
#[derive(Debug, Clone)]
pub enum NodeData {
    /// An element.
    Element(Element),
    /// Character data.
    Text(String),
    /// A comment.
    Comment(String),
    /// A processing instruction.
    Pi {
        /// PI target.
        target: String,
        /// PI data (leading whitespace removed).
        data: String,
    },
}

#[derive(Debug, Clone)]
struct Slot {
    data: NodeData,
    parent: Option<NodeId>,
    rev: u32,
    /// Bumped whenever the node is detached or (re)inserted.
    struct_gen: u32,
}

/// XML declaration fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlDecl {
    /// `version`.
    pub version: String,
    /// `encoding` as written.
    pub encoding: Option<String>,
    /// `standalone`.
    pub standalone: Option<bool>,
}

/// A parsed XML part.
#[derive(Debug, Clone)]
pub struct XmlTree {
    slots: Vec<Slot>,
    uris: Vec<Box<str>>,
    uri_lookup: HashMap<Box<str>, NsId>,
    pub(crate) decl: Option<XmlDecl>,
    pub(crate) doc_nodes: Vec<NodeId>,
    pub(crate) root: NodeId,
    pub(crate) source_encoding: SourceEncoding,
    mutations: u64,
    /// Scope a detached or cloned node was taken from, for relocation checks.
    origin_scope: HashMap<NodeId, NodeId>,
}

impl XmlTree {
    pub(crate) fn empty(source_encoding: SourceEncoding) -> Self {
        let mut tree = XmlTree {
            slots: Vec::new(),
            uris: Vec::new(),
            uri_lookup: HashMap::new(),
            decl: None,
            doc_nodes: Vec::new(),
            root: NodeId(u32::MAX),
            source_encoding,
            mutations: 0,
            origin_scope: HashMap::new(),
        };
        tree.intern(XML_NS);
        tree
    }

    pub(crate) fn intern(&mut self, uri: &str) -> NsId {
        if let Some(id) = self.uri_lookup.get(uri) {
            return *id;
        }
        let id = NsId(self.uris.len() as u32);
        self.uris.push(uri.into());
        self.uri_lookup.insert(uri.into(), id);
        id
    }

    /// Look up an interned URI without interning it.
    pub fn ns_id(&self, uri: &str) -> Option<NsId> {
        self.uri_lookup.get(uri).copied()
    }

    /// The URI text of `ns`.
    pub fn uri(&self, ns: NsId) -> &str {
        &self.uris[ns.0 as usize]
    }

    pub(crate) fn push(&mut self, data: NodeData, parent: Option<NodeId>) -> NodeId {
        let id = NodeId(self.slots.len() as u32);
        self.slots.push(Slot {
            data,
            parent,
            rev: 0,
            struct_gen: 0,
        });
        if let Some(parent) = parent
            && let NodeData::Element(element) = &mut self.slots[parent.0 as usize].data
        {
            element.children.push(id);
        }
        id
    }

    pub(crate) fn link_child(&mut self, parent: NodeId, child: NodeId) {
        self.slots[child.0 as usize].parent = Some(parent);
        if let NodeData::Element(element) = &mut self.slots[parent.0 as usize].data {
            element.children.push(child);
        }
    }

    /// Number of arena slots ever allocated (attached or not).
    pub fn arena_len(&self) -> usize {
        self.slots.len()
    }

    /// The document element.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The XML declaration, if the source had one.
    pub fn decl(&self) -> Option<&XmlDecl> {
        self.decl.as_ref()
    }

    /// The encoding the part was read in.
    pub fn source_encoding(&self) -> SourceEncoding {
        self.source_encoding
    }

    /// Total mutations applied since parse (local freshness evidence).
    pub fn mutation_count(&self) -> u64 {
        self.mutations
    }

    /// Revision of one node; bumped whenever that node itself changes.
    pub fn rev(&self, id: NodeId) -> u32 {
        self.slots[id.0 as usize].rev
    }

    /// Structural generation of one node: bumped each time it is detached or
    /// inserted, so a node moved away and back is distinguishable from one
    /// that never moved.
    pub fn struct_gen(&self, id: NodeId) -> u32 {
        self.slots[id.0 as usize].struct_gen
    }

    /// Whether `id` names a slot in this arena.
    pub fn contains(&self, id: NodeId) -> bool {
        (id.0 as usize) < self.slots.len()
    }

    /// Node payload.
    pub fn data(&self, id: NodeId) -> &NodeData {
        &self.slots[id.0 as usize].data
    }

    /// Element payload, if `id` is an element.
    pub fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.slots[id.0 as usize].data {
            NodeData::Element(element) => Some(element),
            _ => None,
        }
    }

    /// Character data, if `id` is a text node.
    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.slots[id.0 as usize].data {
            NodeData::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Parent element (`None` for the root and for detached nodes).
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.slots[id.0 as usize].parent
    }

    /// Child nodes of an element (empty for other node kinds).
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        match &self.slots[id.0 as usize].data {
            NodeData::Element(element) => &element.children,
            _ => &[],
        }
    }

    /// Element children only.
    pub fn child_elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id)
            .iter()
            .copied()
            .filter(|child| self.element(*child).is_some())
    }

    /// True when `id` is reachable from the document element.
    pub fn is_attached(&self, id: NodeId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let mut current = id;
        loop {
            if current == self.root {
                return true;
            }
            match self.parent(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Position of `id` among its parent's children.
    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let parent = self.parent(id)?;
        self.children(parent).iter().position(|child| *child == id)
    }

    /// Ancestors from parent to root.
    pub fn ancestors(&self, id: NodeId) -> Ancestors<'_> {
        Ancestors {
            tree: self,
            next: self.parent(id),
        }
    }

    /// Pre-order traversal of `id` and its descendants.
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            tree: self,
            stack: vec![id],
        }
    }

    /// Resolved namespace URI of an element.
    pub fn ns_uri(&self, id: NodeId) -> Option<&str> {
        self.element(id)
            .and_then(|element| element.ns.map(|ns| self.uri(ns)))
    }

    /// Local name of an element.
    pub fn local_name(&self, id: NodeId) -> Option<&str> {
        self.element(id).map(|element| &*element.name.local)
    }

    /// True when `id` is an element named `{ns}local`.
    pub fn is(&self, id: NodeId, ns: &str, local: &str) -> bool {
        match self.element(id) {
            Some(element) => {
                &*element.name.local == local && element.ns.map(|n| self.uri(n)) == Some(ns)
            }
            None => false,
        }
    }

    /// Clark-notation name of an element (`{uri}local` or `local`).
    pub fn clark(&self, id: NodeId) -> Option<String> {
        let element = self.element(id)?;
        Some(match element.ns {
            Some(ns) => format!("{{{}}}{}", self.uri(ns), element.name.local),
            None => element.name.local.to_string(),
        })
    }

    /// Attribute `{ns}local` (`ns = None` for unprefixed attributes).
    pub fn attr(&self, id: NodeId, ns: Option<&str>, local: &str) -> Option<&str> {
        let element = self.element(id)?;
        element
            .attrs
            .iter()
            .find(|attr| &*attr.name.local == local && attr.ns.map(|n| self.uri(n)) == ns)
            .map(|attr| attr.value.as_str())
    }

    /// First element child named `{ns}local`.
    pub fn first_child_named(&self, id: NodeId, ns: &str, local: &str) -> Option<NodeId> {
        self.child_elements(id)
            .find(|child| self.is(*child, ns, local))
    }

    /// The URI bound to `prefix` in scope at element `id`.
    ///
    /// Returns `Some(None)` when the prefix is explicitly unbound (only the
    /// default namespace can be) and `None` when it is not declared at all.
    pub fn lookup_prefix(&self, id: NodeId, prefix: Option<&str>) -> Option<Option<NsId>> {
        let mut current = Some(id);
        while let Some(node) = current {
            if let Some(element) = self.element(node) {
                for decl in &element.ns_decls {
                    if decl.prefix.as_deref() == prefix {
                        return Some(decl.uri);
                    }
                }
            }
            current = self.scope_parent(node);
        }
        match prefix {
            Some("xml") => Some(self.ns_id(XML_NS)),
            None => Some(None),
            _ => None,
        }
    }

    /// Parent for namespace scoping: the real parent, or for a detached node
    /// the scope it was created for or taken from.
    fn scope_parent(&self, id: NodeId) -> Option<NodeId> {
        self.parent(id)
            .or_else(|| self.origin_scope.get(&id).copied())
    }

    /// Prefix → URI bindings in scope at `id` (innermost wins).
    pub fn in_scope(&self, id: NodeId) -> Vec<(Option<Box<str>>, Option<NsId>)> {
        let mut seen: Vec<(Option<Box<str>>, Option<NsId>)> = Vec::new();
        let mut current = Some(id);
        while let Some(node) = current {
            if let Some(element) = self.element(node) {
                for decl in &element.ns_decls {
                    if !seen.iter().any(|(prefix, _)| *prefix == decl.prefix) {
                        seen.push((decl.prefix.clone(), decl.uri));
                    }
                }
            }
            current = self.scope_parent(node);
        }
        seen
    }

    /// A lexical prefix bound to `uri` in scope at `id`, unshadowed.
    pub fn prefix_for(&self, id: NodeId, uri: &str) -> Option<Option<Box<str>>> {
        let ns = self.ns_id(uri)?;
        self.in_scope(id)
            .into_iter()
            .find(|(_, bound)| *bound == Some(ns))
            .map(|(prefix, _)| prefix)
    }

    /// Concatenated character data of `id`'s descendants.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        for node in self.descendants(id) {
            if let NodeData::Text(text) = self.data(node) {
                out.push_str(text);
            }
        }
        out
    }

    // ----------------------------------------------------------------------
    // Mutation
    // ----------------------------------------------------------------------

    fn touch(&mut self, id: NodeId) {
        let slot = &mut self.slots[id.0 as usize];
        slot.rev = slot.rev.wrapping_add(1);
        self.mutations += 1;
    }

    /// Replace a text node's character data.
    pub fn set_text(&mut self, id: NodeId, text: impl Into<String>) -> Result<()> {
        match &mut self.slots[id.0 as usize].data {
            NodeData::Text(existing) => {
                *existing = text.into();
            }
            _ => return Err(Error::internal("not_text", "set_text on a non-text node")),
        }
        self.touch(id);
        Ok(())
    }

    /// Set attribute `{ns}local`, reusing an existing attribute when present.
    ///
    /// A new namespaced attribute takes a prefix already bound to `ns` at the
    /// element; if none is in scope the call refuses rather than inventing a
    /// declaration.
    pub fn set_attr(
        &mut self,
        id: NodeId,
        ns: Option<&str>,
        local: &str,
        value: &str,
    ) -> Result<()> {
        let existing = {
            let element = self
                .element(id)
                .ok_or_else(|| Error::internal("not_element", "set_attr on a non-element"))?;
            element
                .attrs
                .iter()
                .position(|attr| &*attr.name.local == local && attr.ns.map(|n| self.uri(n)) == ns)
        };
        if let Some(index) = existing {
            if let NodeData::Element(element) = &mut self.slots[id.0 as usize].data {
                element.attrs[index].value = value.to_string();
            }
            self.touch(id);
            return Ok(());
        }
        let (prefix, ns_id) = match ns {
            None => (None, None),
            Some(XML_NS) => (Some("xml".into()), self.ns_id(XML_NS)),
            Some(uri) => match self.prefix_for(id, uri).flatten() {
                Some(prefix) => (Some(prefix), self.ns_id(uri)),
                // Attributes cannot use the default namespace: declare a
                // prefix on the element itself (lxml generates one too).
                None => {
                    let prefix = self.declare_prefix(id, uri);
                    (Some(prefix), self.ns_id(uri))
                }
            },
        };
        if let NodeData::Element(element) = &mut self.slots[id.0 as usize].data {
            element.attrs.push(Attr {
                name: QName {
                    prefix,
                    local: local.into(),
                },
                ns: ns_id,
                value: value.to_string(),
            });
        }
        self.touch(id);
        Ok(())
    }

    /// Remove attribute `{ns}local`; returns whether it existed.
    pub fn remove_attr(&mut self, id: NodeId, ns: Option<&str>, local: &str) -> bool {
        let position = match self.element(id) {
            Some(element) => element
                .attrs
                .iter()
                .position(|attr| &*attr.name.local == local && attr.ns.map(|n| self.uri(n)) == ns),
            None => None,
        };
        if let Some(index) = position {
            if let NodeData::Element(element) = &mut self.slots[id.0 as usize].data {
                element.attrs.remove(index);
            }
            self.touch(id);
            true
        } else {
            false
        }
    }

    /// A new detached element `{ns}local` whose prefix is chosen from the
    /// scope of `scope` (an attached element).
    pub fn create_element(&mut self, scope: NodeId, ns: &str, local: &str) -> Result<NodeId> {
        let bound = self.prefix_for(scope, ns);
        let ns_id = self.intern(ns);
        let id = self.push(
            NodeData::Element(Element {
                name: QName {
                    prefix: bound.clone().flatten(),
                    local: local.into(),
                },
                ns: Some(ns_id),
                ns_decls: Vec::new(),
                attrs: Vec::new(),
                children: Vec::new(),
            }),
            None,
        );
        self.origin_scope.insert(id, scope);
        if bound.is_none() {
            // Not in scope: declare it on the new element.
            let prefix = self.fresh_prefix(scope, ns);
            if let NodeData::Element(element) = &mut self.slots[id.0 as usize].data {
                element.name.prefix = Some(prefix.clone());
                element.ns_decls.push(NsDecl {
                    prefix: Some(prefix),
                    uri: Some(ns_id),
                });
            }
        }
        self.mutations += 1;
        Ok(id)
    }

    /// A prefix unbound at `scope`, preferring the conventional one for `uri`.
    fn fresh_prefix(&self, scope: NodeId, uri: &str) -> Box<str> {
        let preferred = conventional_prefix(uri);
        let unbound = |candidate: &str| self.lookup_prefix(scope, Some(candidate)).is_none();
        if let Some(preferred) = preferred.filter(|p| unbound(p)) {
            return preferred.into();
        }
        (0..)
            .map(|n| format!("ns{n}"))
            .find(|candidate| unbound(candidate))
            .unwrap_or_default()
            .into()
    }

    /// Declare a prefix for `uri` on `element` and return it.
    fn declare_prefix(&mut self, element: NodeId, uri: &str) -> Box<str> {
        let prefix = self.fresh_prefix(element, uri);
        let ns_id = self.intern(uri);
        if let NodeData::Element(data) = &mut self.slots[element.0 as usize].data {
            data.ns_decls.push(NsDecl {
                prefix: Some(prefix.clone()),
                uri: Some(ns_id),
            });
        }
        prefix
    }

    /// A new detached text node.
    pub fn create_text(&mut self, text: impl Into<String>) -> NodeId {
        self.mutations += 1;
        self.push(NodeData::Text(text.into()), None)
    }

    /// Detach `id` from its parent, remembering its original scope.
    pub fn detach(&mut self, id: NodeId) -> Result<()> {
        if id == self.root {
            return Err(Error::internal(
                "detach_root",
                "cannot detach the document element",
            ));
        }
        let parent = self
            .parent(id)
            .ok_or_else(|| Error::internal("already_detached", "node is already detached"))?;
        if let NodeData::Element(element) = &mut self.slots[parent.0 as usize].data {
            element.children.retain(|child| *child != id);
        }
        self.slots[id.0 as usize].parent = None;
        self.slots[id.0 as usize].struct_gen = self.slots[id.0 as usize].struct_gen.wrapping_add(1);
        self.origin_scope.insert(id, parent);
        self.touch(parent);
        self.touch(id);
        Ok(())
    }

    /// Deep-copy `id` into new detached nodes with fresh IDs.
    pub fn deep_clone(&mut self, id: NodeId) -> NodeId {
        let origin = self.parent(id);
        let copy = self.clone_rec(id, None);
        if let Some(origin) = origin {
            self.origin_scope.insert(copy, origin);
        }
        self.mutations += 1;
        copy
    }

    fn clone_rec(&mut self, id: NodeId, parent: Option<NodeId>) -> NodeId {
        let mut data = self.slots[id.0 as usize].data.clone();
        let children = match &mut data {
            NodeData::Element(element) => std::mem::take(&mut element.children),
            _ => Vec::new(),
        };
        let copy = self.push(data, parent);
        for child in children {
            self.clone_rec(child, Some(copy));
        }
        copy
    }

    /// Insert detached `child` into `parent` at `index`.
    ///
    /// Refuses when the move would change what any prefix inside the moved
    /// subtree means: every binding visible at the subtree's origin (minus
    /// those the subtree redeclares at its top) must be identical at the
    /// destination. This covers QName-valued attribute *values* in unknown
    /// vocabularies, which cannot be enumerated.
    pub fn insert(&mut self, parent: NodeId, index: usize, child: NodeId) -> Result<()> {
        if self.parent(child).is_some() || child == self.root {
            return Err(Error::internal(
                "not_detached",
                "insert of an attached node",
            ));
        }
        if self.element(parent).is_none() {
            return Err(Error::internal("not_element", "insert into a non-element"));
        }
        if self.descendants(child).any(|node| node == parent) {
            return Err(Error::internal("cycle", "insert would create a cycle"));
        }
        if self.element(child).is_some() {
            self.check_relocation(child, parent)?;
        }
        let len = self.children(parent).len();
        if index > len {
            return Err(Error::internal("index", "insert index out of range"));
        }
        if let NodeData::Element(element) = &mut self.slots[parent.0 as usize].data {
            element.children.insert(index, child);
        }
        self.slots[child.0 as usize].parent = Some(parent);
        self.slots[child.0 as usize].struct_gen =
            self.slots[child.0 as usize].struct_gen.wrapping_add(1);
        self.origin_scope.remove(&child);
        self.touch(parent);
        Ok(())
    }

    fn check_relocation(&self, child: NodeId, destination: NodeId) -> Result<()> {
        let own: Vec<Option<&str>> = self
            .element(child)
            .map(|element| {
                element
                    .ns_decls
                    .iter()
                    .map(|decl| decl.prefix.as_deref())
                    .collect()
            })
            .unwrap_or_default();
        let refuse = |detail: String| {
            Err(Error::refusal(
                RefusalKind::UnsupportedStructure,
                "namespace_scope_change",
                format!(
                    "relocating markup would change namespace meaning ({detail}); nothing was changed"
                ),
            ))
        };
        match self.origin_scope.get(&child) {
            Some(origin) => {
                let before = self.in_scope(*origin);
                let after = self.in_scope(destination);
                for (prefix, _) in before.iter().chain(after.iter()) {
                    if own.contains(&prefix.as_deref()) {
                        continue;
                    }
                    let at_origin = self.lookup_prefix(*origin, prefix.as_deref());
                    let at_destination = self.lookup_prefix(destination, prefix.as_deref());
                    if at_origin != at_destination {
                        return refuse(format!(
                            "prefix {:?} differs between source and destination scope",
                            prefix.as_deref().unwrap_or("")
                        ));
                    }
                }
                Ok(())
            }
            None => {
                // Brand-new markup: every name must resolve to its stored namespace.
                for node in self.descendants(child) {
                    let Some(element) = self.element(node) else {
                        continue;
                    };
                    let resolved = self.resolve_detached(
                        node,
                        child,
                        destination,
                        element.name.prefix.as_deref(),
                    );
                    if resolved != Some(element.ns) {
                        return refuse(format!("element {}", element.name.lexical()));
                    }
                    for attr in &element.attrs {
                        if attr.name.prefix.is_none() {
                            continue;
                        }
                        let resolved = self.resolve_detached(
                            node,
                            child,
                            destination,
                            attr.name.prefix.as_deref(),
                        );
                        if resolved != Some(attr.ns) {
                            return refuse(format!("attribute {}", attr.name.lexical()));
                        }
                    }
                }
                Ok(())
            }
        }
    }

    /// Resolve `prefix` at `node` inside detached subtree `top` as if `top`
    /// were inserted under `destination`.
    fn resolve_detached(
        &self,
        node: NodeId,
        top: NodeId,
        destination: NodeId,
        prefix: Option<&str>,
    ) -> Option<Option<NsId>> {
        let mut current = Some(node);
        while let Some(id) = current {
            if let Some(element) = self.element(id) {
                for decl in &element.ns_decls {
                    if decl.prefix.as_deref() == prefix {
                        return Some(decl.uri);
                    }
                }
            }
            if id == top {
                break;
            }
            current = self.parent(id);
        }
        self.lookup_prefix(destination, prefix)
    }

    /// Rename an element's local name in place (prefix and namespace kept),
    /// e.g. retagging `w:t` as `w:delText`.
    pub fn set_local_name(&mut self, id: NodeId, local: &str) -> Result<()> {
        match &mut self.slots[id.0 as usize].data {
            NodeData::Element(element) => element.name.local = local.into(),
            _ => return Err(Error::internal("not_element", "rename of a non-element")),
        }
        self.touch(id);
        Ok(())
    }

    /// Copy `node` (and its subtree) from another arena as a new detached
    /// subtree scoped for insertion under `scope` (lxml cross-document
    /// `deepcopy`): lexical prefixes, attributes and the subtree's own
    /// declarations are kept, and any prefix the subtree uses without
    /// declaring is declared on the copy unless `scope` already binds it to
    /// the same namespace.
    pub fn import_subtree(
        &mut self,
        source: &XmlTree,
        node: NodeId,
        scope: NodeId,
    ) -> Result<NodeId> {
        let copy = self.import_rec(source, node, None);
        // Free prefixes: used in the subtree but bound outside it.
        let mut needed: Vec<(Option<Box<str>>, Option<NsId>)> = Vec::new();
        for item in source.descendants(node) {
            let Some(element) = source.element(item) else {
                continue;
            };
            let mut uses: Vec<Option<&str>> = vec![element.name.prefix.as_deref()];
            uses.extend(
                element
                    .attrs
                    .iter()
                    .filter(|a| a.name.prefix.is_some())
                    .map(|a| a.name.prefix.as_deref()),
            );
            for prefix in uses {
                if prefix == Some("xml") {
                    continue;
                }
                let mut declared_inside = false;
                let mut current = Some(item);
                while let Some(ancestor) = current {
                    if source
                        .element(ancestor)
                        .is_some_and(|e| e.ns_decls.iter().any(|d| d.prefix.as_deref() == prefix))
                    {
                        declared_inside = true;
                        break;
                    }
                    if ancestor == node {
                        break;
                    }
                    current = source.parent(ancestor);
                }
                if declared_inside {
                    continue;
                }
                let bound = source.lookup_prefix(node, prefix).flatten();
                if needed.iter().any(|(p, _)| p.as_deref() == prefix) {
                    continue;
                }
                needed.push((prefix.map(Into::into), bound));
            }
        }
        for (prefix, bound) in needed {
            let uri = bound.map(|ns| source.uri(ns).to_string());
            let here = self.lookup_prefix(scope, prefix.as_deref());
            let same = match (&here, &uri) {
                (Some(Some(id)), Some(u)) => self.uri(*id) == u,
                (Some(None), None) => true,
                (None, None) => prefix.is_none(),
                _ => false,
            };
            if !same {
                let ns = uri.as_deref().map(|u| self.intern(u));
                if let NodeData::Element(element) = &mut self.slots[copy.0 as usize].data {
                    element.ns_decls.push(NsDecl { prefix, uri: ns });
                }
            }
        }
        self.origin_scope.insert(copy, scope);
        self.mutations += 1;
        Ok(copy)
    }

    fn import_rec(&mut self, source: &XmlTree, node: NodeId, parent: Option<NodeId>) -> NodeId {
        let data = match source.data(node) {
            NodeData::Element(element) => {
                let ns = element.ns.map(|ns| self.intern(source.uri(ns)));
                let ns_decls = element
                    .ns_decls
                    .iter()
                    .map(|d| NsDecl {
                        prefix: d.prefix.clone(),
                        uri: d.uri.map(|u| self.intern(source.uri(u))),
                    })
                    .collect();
                let attrs = element
                    .attrs
                    .iter()
                    .map(|a| Attr {
                        name: a.name.clone(),
                        ns: a.ns.map(|ns| self.intern(source.uri(ns))),
                        value: a.value.clone(),
                    })
                    .collect();
                NodeData::Element(Element {
                    name: element.name.clone(),
                    ns,
                    ns_decls,
                    attrs,
                    children: Vec::new(),
                })
            }
            other => other.clone(),
        };
        let copy = self.push(data, parent);
        for child in source.children(node).to_vec() {
            self.import_rec(source, child, Some(copy));
        }
        copy
    }

    /// Mark the tree changed without structural edits (used by callers that
    /// need a fresh revision on `id`).
    pub fn bump(&mut self, id: NodeId) {
        self.touch(id);
    }
}

/// Iterator over ancestors.
pub struct Ancestors<'a> {
    tree: &'a XmlTree,
    next: Option<NodeId>,
}

impl Iterator for Ancestors<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.next?;
        self.next = self.tree.parent(current);
        Some(current)
    }
}

/// Pre-order iterator.
pub struct Descendants<'a> {
    tree: &'a XmlTree,
    stack: Vec<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.stack.pop()?;
        let children = self.tree.children(current);
        self.stack.extend(children.iter().rev().copied());
        Some(current)
    }
}

/// The python-docx `nsmap` prefix for well-known OOXML namespaces.
pub fn conventional_prefix(uri: &str) -> Option<&'static str> {
    Some(match uri {
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main" => "w",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships" => "r",
        "http://schemas.openxmlformats.org/drawingml/2006/main" => "a",
        "http://schemas.openxmlformats.org/drawingml/2006/picture" => "pic",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" => "wp",
        "http://schemas.openxmlformats.org/officeDocument/2006/math" => "m",
        "http://schemas.microsoft.com/office/word/2010/wordml" => "w14",
        "http://schemas.microsoft.com/office/word/2012/wordml" => "w15",
        "http://schemas.openxmlformats.org/markup-compatibility/2006" => "mc",
        "http://www.w3.org/2001/XMLSchema-instance" => "xsi",
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties" => "cp",
        "http://purl.org/dc/elements/1.1/" => "dc",
        "http://purl.org/dc/terms/" => "dcterms",
        "urn:schemas-microsoft-com:vml" => "v",
        "urn:schemas-microsoft-com:office:office" => "o",
        _ => return None,
    })
}

pub(crate) fn check_reserved_binding(prefix: Option<&str>, uri: &str) -> Result<(), XmlError> {
    match prefix {
        Some("xmlns") => Err(XmlError::malformed(
            "reserved_namespace",
            "redefinition of the xmlns prefix is forbidden",
        )),
        Some("xml") if uri != XML_NS => Err(XmlError::malformed(
            "reserved_namespace",
            "xml namespace prefix mapped to wrong URI",
        )),
        Some("xml") => Ok(()),
        _ if uri == XML_NS => Err(XmlError::malformed(
            "reserved_namespace",
            "xml namespace URI mapped to wrong prefix",
        )),
        _ if uri == XMLNS_NS => Err(XmlError::malformed(
            "reserved_namespace",
            "reuse of the xmlns namespace name is forbidden",
        )),
        _ => Ok(()),
    }
}
