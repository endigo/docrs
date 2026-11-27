use std::collections::HashSet;

use docrs::wml::{W, is_w, w_attr};
use docrs::xml::{NodeId, XmlTree};

/// A paragraph's text as Word shows it: field results, not field codes.
pub fn shown_text(tree: &XmlTree, paragraph: NodeId) -> String {
    let in_run = |node| {
        tree.parent(node)
            .is_some_and(|parent| tree.is(parent, W, "r"))
    };
    let mut out = String::new();
    for node in tree.descendants(paragraph) {
        match w_local(tree, node) {
            Some("t") => out.push_str(&tree.text_content(node)),
            Some("tab") if in_run(node) => out.push('\t'),
            Some("br" | "cr") if in_run(node) => out.push('\n'),
            Some("noBreakHyphen") => out.push('-'),
            Some("sym") => out.extend(symbol(tree, node)),
            _ => {}
        }
    }
    out
}

fn w_local(tree: &XmlTree, node: NodeId) -> Option<&str> {
    tree.local_name(node)
        .filter(|local| is_w(tree, node, local))
}

/// `w:sym`'s character, unless it is in the Private Use Area (a symbol
/// font's own encoding).
fn symbol(tree: &XmlTree, node: NodeId) -> Option<char> {
    let code = u32::from_str_radix(w_attr(tree, node, "char")?, 16).ok()?;
    char::from_u32(code).filter(|c| !('\u{E000}'..='\u{F8FF}').contains(c))
}

/// Paragraphs of a table of contents: inside a Table of Contents content
/// control or a TOC field.
pub fn contents_paragraphs(tree: &XmlTree, body: NodeId) -> HashSet<NodeId> {
    let mut out = HashSet::new();
    let mut fields: Vec<(String, bool)> = Vec::new();
    for paragraph in tree.descendants(body).filter(|&n| tree.is(n, W, "p")) {
        let mut contents = in_contents_control(tree, paragraph) || fields.iter().any(|f| f.1);
        for node in tree.descendants(paragraph) {
            match w_local(tree, node) {
                Some("fldChar") => match w_attr(tree, node, "fldCharType") {
                    Some("begin") => fields.push((String::new(), false)),
                    Some("end") => {
                        fields.pop();
                    }
                    _ => {}
                },
                Some("instrText") => {
                    if let Some(field) = fields.last_mut() {
                        field.0.push_str(&tree.text_content(node));
                        field.1 = field.0.trim_start().starts_with("TOC ");
                        contents |= field.1;
                    }
                }
                _ => {}
            }
        }
        if contents {
            out.insert(paragraph);
        }
    }
    out
}

fn in_contents_control(tree: &XmlTree, paragraph: NodeId) -> bool {
    let mut node = tree.parent(paragraph);
    while let Some(id) = node {
        if tree.is(id, W, "sdt") {
            let gallery = tree
                .descendants(id)
                .find(|&n| tree.is(n, W, "docPartGallery"))
                .and_then(|n| w_attr(tree, n, "val"));
            if gallery == Some("Table of Contents") {
                return true;
            }
        }
        node = tree.parent(id);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

    fn body(xml: &str) -> XmlTree {
        let doc = format!("<w:document {NS}><w:body>{xml}</w:body></w:document>");
        docrs::xml::parse(doc.as_bytes(), &docrs::Limits::default()).unwrap()
    }

    fn paragraphs(tree: &XmlTree) -> Vec<NodeId> {
        tree.descendants(tree.root())
            .filter(|&n| tree.is(n, W, "p"))
            .collect()
    }

    #[test]
    fn text_shows_what_word_draws() {
        let tree = body(concat!(
            "<w:p><w:pPr><w:tabs><w:tab w:val=\"right\" w:pos=\"9000\"/></w:tabs></w:pPr>",
            "<w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t><w:softHyphen/><w:t>er</w:t></w:r>",
            "<w:r><w:tab/><w:sym w:font=\"Symbol\" w:char=\"03A9\"/><w:sym w:font=\"Wingdings\" w:char=\"F0FC\"/></w:r>",
            "<w:r><w:br/><w:t>2</w:t><w:cr/></w:r>",
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r>",
            "<w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>7</w:t></w:r>",
            "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>",
        ));
        let p = paragraphs(&tree)[0];
        assert_eq!(shown_text(&tree, p), "co-oper\t\u{3A9}\n2\n7");
    }

    #[test]
    fn contents_are_the_toc_field_and_content_control_paragraphs() {
        let begin = "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>";
        let end = "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>";
        let tree = body(&format!(
            "<w:p><w:r><w:t>Before</w:t></w:r></w:p>\
             <w:p>{begin}<w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-3\" </w:instrText></w:r>\
             <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>A long entry</w:t></w:r></w:p>\
             <w:p><w:r><w:t>Another entry</w:t></w:r>{end}</w:p>\
             <w:p>{begin}<w:r><w:instrText> PAGE </w:instrText></w:r>{end}</w:p>\
             <w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/></w:docPartObj></w:sdtPr>\
             <w:sdtContent><w:p><w:r><w:t>Contents</w:t></w:r></w:p></w:sdtContent></w:sdt>\
             <w:p><w:r><w:t>After</w:t></w:r></w:p>"
        ));
        let all = paragraphs(&tree);
        let contents = contents_paragraphs(&tree, tree.root());
        let flags: Vec<bool> = all.iter().map(|p| contents.contains(p)).collect();
        assert_eq!(flags, [false, true, true, false, true, false]);
    }
}
