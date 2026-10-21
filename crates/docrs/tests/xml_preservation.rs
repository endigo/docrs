//! Namespace-scope, QName and unknown-content preservation (no upstream test
//! covers XML namespace collisions; these are port-owned regressions).

mod support;

use docrs::xml::{self, NodeData, XmlError, parse_str, serialize, trees_equivalent};
use docrs::{ErrorKind, Limits, RefusalKind};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

const SCOPED: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<!-- prolog comment --><?mso-application progid="Word.Document"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" mc:Ignorable="w14 x"><w:body><w:p w14:paraId="1A2B" xmlns:x="urn:unknown:x"><x:unknown x:attr="v" plain="p">opaque <![CDATA[<raw>]]> &amp; text<!-- inner --><?pi data?></x:unknown><w:r><w:t xml:space="preserve"> Hello </w:t></w:r><mc:AlternateContent><mc:Choice Requires="w14"><w14:thing/></mc:Choice><mc:Fallback><w:t>fb</w:t></mc:Fallback></mc:AlternateContent><q:redeclared xmlns:q="urn:q:1"><q:inner xmlns:q="urn:q:2" xsi:type="q:T"/></q:redeclared><w:r xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:t>rebound-same</w:t></w:r></w:p></w:body></w:document><!-- epilog -->"#;

fn find_text(tree: &xml::XmlTree, needle: &str) -> xml::NodeId {
    tree.descendants(tree.root())
        .find(|node| matches!(tree.data(*node), NodeData::Text(t) if t == needle))
        .unwrap_or_else(|| panic!("text {needle:?}"))
}

#[test]
fn edit_preserves_scopes_prefixes_and_unknown_markup() {
    let tree = parse_str(SCOPED).unwrap();
    let mut edited = tree.clone();
    let node = find_text(&edited, " Hello ");
    edited.set_text(node, " Goodbye ").unwrap();
    let out = String::from_utf8(serialize(&edited)).unwrap();

    // Declarations stay on the elements that carried them, in order.
    assert!(
        out.contains(r#"<w:p w14:paraId="1A2B" xmlns:x="urn:unknown:x">"#)
            || out.contains(r#"<w:p xmlns:x="urn:unknown:x" w14:paraId="1A2B">"#),
        "{out}"
    );
    assert!(
        out.contains(
            r#"<q:redeclared xmlns:q="urn:q:1"><q:inner xmlns:q="urn:q:2" xsi:type="q:T"/>"#
        ),
        "{out}"
    );
    assert!(out.contains(r#"mc:Ignorable="w14 x""#));
    assert!(out.contains(r#"<mc:Choice Requires="w14"><w14:thing/></mc:Choice>"#));
    assert!(out.contains(r#"<x:unknown x:attr="v" plain="p">opaque &lt;raw&gt; &amp; text<!-- inner --><?pi data?></x:unknown>"#), "{out}");
    assert!(out.contains("<!-- prolog comment --><?mso-application progid=\"Word.Document\"?>"));
    assert!(out.ends_with("<!-- epilog -->"));
    assert!(out.contains(r#"<w:t xml:space="preserve"> Goodbye </w:t>"#));
    assert!(out.contains(r#"<w:r xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:t>rebound-same</w:t>"#));

    // Reparse: exactly one semantic change.
    let reparsed = parse_str(&out).unwrap();
    assert!(!trees_equivalent(&tree, &reparsed));
    let mut reverted = reparsed.clone();
    let node = find_text(&reverted, " Goodbye ");
    reverted.set_text(node, " Hello ").unwrap();
    assert!(trees_equivalent(&tree, &reverted));
}

#[test]
fn equivalence_is_prefix_insensitive_and_uri_sensitive() {
    let a = parse_str(r#"<a:x xmlns:a="urn:1" a:k="v"/>"#).unwrap();
    let b = parse_str(r#"<b:x xmlns:b="urn:1" b:k="v"/>"#).unwrap();
    let c = parse_str(r#"<a:x xmlns:a="urn:2" a:k="v"/>"#).unwrap();
    assert!(trees_equivalent(&a, &b));
    assert!(!trees_equivalent(&a, &c));
    // Attribute order does not matter; child order and whitespace do.
    assert!(trees_equivalent(
        &parse_str("<r x='1' y='2'/>").unwrap(),
        &parse_str("<r y='2' x='1'/>").unwrap()
    ));
    assert!(!trees_equivalent(
        &parse_str("<r><a/><b/></r>").unwrap(),
        &parse_str("<r><b/><a/></r>").unwrap()
    ));
    assert!(!trees_equivalent(
        &parse_str("<r><t>a </t></r>").unwrap(),
        &parse_str("<r><t>a</t></r>").unwrap()
    ));
    // CDATA and references are character data.
    assert!(trees_equivalent(
        &parse_str("<r>&lt;x&gt;</r>").unwrap(),
        &parse_str("<r><![CDATA[<x>]]></r>").unwrap()
    ));
    // Prolog PIs and comments are compared.
    assert!(!trees_equivalent(
        &parse_str("<?mso-application x?><r/>").unwrap(),
        &parse_str("<r/>").unwrap()
    ));
}

/// Deliberate correction over upstream: QName-valued attribute values are
/// compared by resolved namespace.
#[test]
fn qname_valued_attributes_compare_by_resolved_namespace() {
    let base = r#"<r xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:p="urn:A"><e xsi:type="p:T"/></r>"#;
    let rebound = r#"<r xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:p="urn:B"><e xsi:type="p:T"/></r>"#;
    let renamed = r#"<r xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:z="urn:A"><e xsi:type="z:T"/></r>"#;
    assert!(!trees_equivalent(
        &parse_str(base).unwrap(),
        &parse_str(rebound).unwrap()
    ));
    assert!(trees_equivalent(
        &parse_str(base).unwrap(),
        &parse_str(renamed).unwrap()
    ));

    let ign = |uri: &str, prefix: &str| {
        format!(
            r#"<r xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:{prefix}="{uri}" mc:Ignorable="{prefix}"/>"#
        )
    };
    assert!(trees_equivalent(
        &parse_str(&ign("urn:w14", "w14")).unwrap(),
        &parse_str(&ign("urn:w14", "v")).unwrap()
    ));
    assert!(!trees_equivalent(
        &parse_str(&ign("urn:w14", "w14")).unwrap(),
        &parse_str(&ign("urn:w15", "w14")).unwrap()
    ));
}

#[test]
fn relocation_that_would_change_prefix_meaning_refuses() {
    let mut tree = parse_str(
        r#"<r xmlns:p="urn:1"><a><x xsi:type="p:T" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"/></a><b xmlns:p="urn:2"/><c/></r>"#,
    )
    .unwrap();
    let root = tree.root();
    let children: Vec<_> = tree.child_elements(root).collect();
    let (a, b, c) = (children[0], children[1], children[2]);
    let x = tree.child_elements(a).next().unwrap();
    let before = serialize(&tree);
    tree.detach(x).unwrap();
    let err = tree.insert(b, 0, x).unwrap_err();
    assert_eq!(
        err.kind(),
        ErrorKind::Refusal(RefusalKind::UnsupportedStructure)
    );
    assert_eq!(err.reason(), "namespace_scope_change");
    // Same-scope relocation is fine.
    tree.insert(c, 0, x).unwrap();
    assert_ne!(serialize(&tree), before);
}

#[test]
fn new_elements_take_in_scope_prefixes() {
    let mut tree = parse_str(&format!(r#"<w:p xmlns:w="{W}"><w:r/></w:p>"#)).unwrap();
    let root = tree.root();
    let run = tree.create_element(root, W, "r").unwrap();
    tree.insert(root, 1, run).unwrap();
    assert_eq!(
        String::from_utf8(serialize(&tree)).unwrap(),
        format!(r#"<w:p xmlns:w="{W}"><w:r/><w:r/></w:p>"#)
    );
    // A namespace not in scope is declared on the new element itself.
    let foreign = tree.create_element(root, "urn:not-declared", "x").unwrap();
    tree.set_attr(foreign, Some("urn:not-declared"), "k", "v")
        .unwrap();
    tree.insert(root, 2, foreign).unwrap();
    let out = String::from_utf8(serialize(&tree)).unwrap();
    assert!(
        out.contains(r#"<ns0:x xmlns:ns0="urn:not-declared" ns0:k="v"/>"#),
        "{out}"
    );
    assert!(trees_equivalent(&tree, &parse_str(&out).unwrap()));
    // With w as the default namespace, attributes get a declared w prefix.
    let mut tree = parse_str(&format!(r#"<p xmlns="{W}"/>"#)).unwrap();
    let root = tree.root();
    tree.set_attr(root, Some(W), "val", "1").unwrap();
    let out = String::from_utf8(serialize(&tree)).unwrap();
    assert_eq!(out, format!(r#"<p xmlns="{W}" xmlns:w="{W}" w:val="1"/>"#));
}

#[test]
fn utf16_part_is_rewritten_as_utf8_with_matching_declaration() {
    let text = "<?xml version=\"1.0\" encoding=\"UTF-16\" standalone=\"yes\"?><w:document xmlns:w=\"u\"><w:t>Ünïcødé ᠮᠣᠩᠭᠣᠯ 😀</w:t></w:document>";
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
    let tree = xml::parse(&bytes, &Limits::default()).unwrap();
    let out = serialize(&tree);
    let out_text = String::from_utf8(out.clone()).unwrap();
    assert!(out_text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>"));
    assert!(out_text.contains("Ünïcødé ᠮᠣᠩᠭᠣᠯ 😀"));
    assert!(trees_equivalent(
        &tree,
        &xml::parse(&out, &Limits::default()).unwrap()
    ));
}

#[test]
fn whitespace_and_line_endings_follow_xml_normalization() {
    let tree = parse_str("<r a='x\ty\nz' b='&#9;&#10;&#13;'>l1\r\nl2\rl3&#13;</r>").unwrap();
    let root = tree.root();
    assert_eq!(tree.attr(root, None, "a"), Some("x y z"));
    assert_eq!(tree.attr(root, None, "b"), Some("\t\n\r"));
    assert_eq!(tree.text_content(root), "l1\nl2\nl3\r");
    let again = parse_str(std::str::from_utf8(&serialize(&tree)).unwrap()).unwrap();
    assert_eq!(again.attr(again.root(), None, "b"), Some("\t\n\r"));
    assert_eq!(again.text_content(again.root()), "l1\nl2\nl3\r");
}

#[test]
fn malformed_and_namespace_invalid_xml_is_refused() {
    let bad = [
        "<a:b/>",
        "<b a:x='1'/>",
        "<b x='1' x='2'/>",
        "<b xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>",
        "<b xmlns:p=''/>",
        "<b xmlns:xml='http://other'/>",
        "<b xmlns:xmlns='u'/>",
        "<b xmlns:p='http://www.w3.org/XML/1998/namespace'/>",
        "<b>&foo;</b>",
        "<b>&#0;</b>",
        "<b>\u{1}</b>",
        "<b/>x",
        "<b/><c/>",
        "<b x='<'/>",
        "<b>]]></b>",
        "<a:b:c xmlns:a='u'/>",
        " <?xml version='1.0'?><b/>",
        "<b><?xml foo?></b>",
        "<b><!-- a -- b --></b>",
        "<b></c>",
        "<a:b xmlns:a='u' xmlns:c='u'></c:b>",
        "<b x=1/>",
        "<b>&#xD800;</b>",
        "<b>\u{FFFE}</b>",
        "",
        "<b",
    ];
    for case in bad {
        match parse_str(case) {
            Err(XmlError::Malformed { .. }) => {}
            other => panic!("{case:?} should be malformed, got {other:?}"),
        }
    }
    let good = [
        "<b xmlns:xml='http://www.w3.org/XML/1998/namespace'/>",
        "<b xmlns=''/>",
        "<?xml version='1.1'?><b/>",
        "\u{FEFF}<b/>",
        "<b xmlns='rel/uri' x='1'/>",
        "<b><?xml-stylesheet x?></b>",
    ];
    for case in good {
        assert!(parse_str(case).is_ok(), "{case:?}");
    }
}

#[test]
fn text_node_budget_is_a_resource_limit() {
    let limits = Limits {
        max_text_node_bytes: 10,
        ..Limits::default()
    };
    match xml::parse(b"<r>01234567890</r>", &limits) {
        Err(XmlError::Limit(err)) => {
            assert_eq!(err.limit_exceeded().unwrap().limit, "max_text_node_bytes")
        }
        other => panic!("{other:?}"),
    }
    assert!(xml::parse(b"<r>0123456789</r>", &limits).is_ok());
}
