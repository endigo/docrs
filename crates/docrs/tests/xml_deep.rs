//! Tree copies and canonical forms walk deep markup without recursion: 50k
//! levels under `Limits::trusted()` must not overflow a 2 MB test-thread
//! stack.

mod support;

use docrs::composition::AppendOptions;
use docrs::xml::{self, NodeId, XmlTree, trees_equivalent};
use docrs::{Document, FindOptions, Limits};
use support::*;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
/// `w:sdt/w:sdtContent` pairs: 50k nested elements.
const PAIRS: usize = 25_000;

/// A document whose body holds `PAIRS` nested content controls around one
/// paragraph that uses `w14` (declared on the root only).
fn deep_document(root_decls: &str) -> String {
    let mut xml = format!("<w:document {root_decls}><w:body>");
    for _ in 0..PAIRS {
        xml.push_str("<w:sdt><w:sdtContent>");
    }
    xml.push_str("<w:p w14:paraId=\"1A2B3C4D\"><w:r><w:t>deep</w:t></w:r></w:p>");
    for _ in 0..PAIRS {
        xml.push_str("</w:sdtContent></w:sdt>");
    }
    xml.push_str("</w:body></w:document>");
    xml
}

fn both_decls() -> String {
    format!("xmlns:w=\"{W}\" xmlns:w14=\"{W14}\"")
}

fn parse(xml: &str) -> XmlTree {
    xml::parse(xml.as_bytes(), &Limits::trusted()).unwrap()
}

fn body(tree: &XmlTree) -> NodeId {
    tree.first_child_named(tree.root(), W, "body").unwrap()
}

fn deepest_paragraph(tree: &XmlTree, top: NodeId) -> NodeId {
    tree.descendants(top)
        .find(|node| tree.is(*node, W, "p"))
        .unwrap()
}

fn assert_preorder_ids(tree: &XmlTree, top: NodeId) {
    let ids: Vec<u32> = tree.descendants(top).map(NodeId::index).collect();
    assert_eq!(ids.len(), 2 * PAIRS + 4);
    assert!(ids.windows(2).all(|pair| pair[0] + 1 == pair[1]));
}

#[test]
fn deep_clone_of_fifty_thousand_levels() {
    let mut tree = parse(&deep_document(&both_decls()));
    let original = tree.clone();
    let body = body(&tree);
    let top = tree.children(body)[0];
    let copy = tree.deep_clone(top);
    assert_ne!(copy, top);
    assert!(tree.parent(copy).is_none());
    assert_preorder_ids(&tree, copy);

    tree.detach(top).unwrap();
    tree.insert(body, 0, copy).unwrap();
    assert!(trees_equivalent(&original, &tree));
}

#[test]
fn import_subtree_of_fifty_thousand_levels() {
    let source = parse(&deep_document(&both_decls()));
    let top = source.children(body(&source))[0];
    let mut destination = parse(&format!(
        "<w:document {}><w:body/></w:document>",
        both_decls()
    ));
    let scope = body(&destination);
    let copy = destination.import_subtree(&source, top, scope).unwrap();
    assert_preorder_ids(&destination, copy);
    // Every prefix is already bound at the destination.
    assert!(destination.element(copy).unwrap().ns_decls.is_empty());
    destination.insert(scope, 0, copy).unwrap();
    assert!(trees_equivalent(&source, &destination));
}

#[test]
fn import_subtree_declares_free_prefixes_once_on_the_copy() {
    let source = parse(&deep_document(&both_decls()));
    let top = source.children(body(&source))[0];
    let mut destination = parse(&format!(
        "<w:document xmlns:w=\"{W}\"><w:body/></w:document>"
    ));
    let scope = body(&destination);
    let copy = destination.import_subtree(&source, top, scope).unwrap();
    let decls = &destination.element(copy).unwrap().ns_decls;
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].prefix.as_deref(), Some("w14"));
    assert_eq!(decls[0].uri.map(|ns| destination.uri(ns)), Some(W14));
    destination.insert(scope, 0, copy).unwrap();

    let reparsed = parse(&String::from_utf8(xml::serialize(&destination)).unwrap());
    let paragraph = deepest_paragraph(&reparsed, reparsed.root());
    assert_eq!(
        reparsed.attr(paragraph, Some(W14), "paraId"),
        Some("1A2B3C4D")
    );
}

#[test]
fn import_subtree_skips_prefixes_declared_inside_the_subtree() {
    let source = parse(&format!(
        "<w:document xmlns:w=\"{W}\" xmlns:a=\"urn:outer\" xmlns:b=\"urn:b\"><w:body><w:p><w:r a:x=\"1\"><w:t xmlns:a=\"urn:inner\" a:y=\"2\" b:z=\"3\"/></w:r></w:p></w:body></w:document>"
    ));
    let top = source.children(body(&source))[0];
    let mut destination = parse(&format!(
        "<w:document xmlns:w=\"{W}\"><w:body/></w:document>"
    ));
    let scope = body(&destination);
    let copy = destination.import_subtree(&source, top, scope).unwrap();
    let decls: Vec<(Option<&str>, Option<&str>)> = destination
        .element(copy)
        .unwrap()
        .ns_decls
        .iter()
        .map(|decl| {
            (
                decl.prefix.as_deref(),
                decl.uri.map(|ns| destination.uri(ns)),
            )
        })
        .collect();
    // `a` is free on w:r (bound outside) even though w:t redeclares it.
    assert_eq!(
        decls,
        [(Some("a"), Some("urn:outer")), (Some("b"), Some("urn:b"))]
    );
}

#[test]
fn replace_canonicalizes_deep_run_properties() {
    let mut rpr = String::from("<w:rPr><w:b/>");
    for _ in 0..2 * PAIRS {
        rpr.push_str("<x:e>");
    }
    for _ in 0..2 * PAIRS {
        rpr.push_str("</x:e>");
    }
    rpr.push_str("</w:rPr>");
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\" xmlns:x=\"urn:x\"><w:body><w:p><w:r>{rpr}<w:t>Hello</w:t></w:r></w:p></w:body></w:document>"
    );
    let mut doc =
        Document::open_bytes(&minimal_package_with(&document), &Limits::trusted()).unwrap();
    let span = doc.find_one("ell", &FindOptions::default()).unwrap();
    let report = doc.replace(&span, "ipp", &Default::default()).unwrap();
    assert!(report.changed);

    let main = doc.package().main_part_name().to_string();
    let tree = doc.package().xml(&main).unwrap();
    assert_eq!(tree.text_content(tree.root()), "Hippo");
    let kept = tree
        .descendants(tree.root())
        .filter(|node| tree.is(*node, "urn:x", "e"))
        .count();
    assert_eq!(kept, 2 * PAIRS, "the run keeps its properties");
}

/// `append` imports the source body through the same copies: the whole
/// path, open to commit, at depth under the trusted profile.
#[test]
fn append_of_fifty_thousand_levels() {
    let source = Document::open_bytes(
        &minimal_package_with(&deep_document(&both_decls())),
        &Limits::trusted(),
    )
    .unwrap();
    let mut dest =
        Document::open_bytes(&minimal_package_with(DOCUMENT_XML), &Limits::trusted()).unwrap();
    dest.append_document(&source, &AppendOptions::default())
        .unwrap();
    let tree = dest.package().xml("word/document.xml").unwrap();
    let sdts = tree
        .descendants(tree.root())
        .filter(|node| tree.is(*node, W, "sdt"))
        .count();
    assert_eq!(sdts, PAIRS);
    let text = dest.find_text("deep", &FindOptions::default()).unwrap();
    assert_eq!(text.len(), 1);
}
