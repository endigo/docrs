//! Synthetic documents for M4 work package p1 (controls, bookmarks, links,
//! notes). Each mirrors the inline XML upstream tests build on top of a
//! blank document, gathered so one fixture serves many cases.

use crate::hostile::build::{DECLS, RawMember, build_zip};

const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT_PROPS: &str = "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";
const CT_SETTINGS: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
const CT_FOOTNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";

struct Extra<'a> {
    name: &'a str,
    content_type: Option<&'a str>,
    data: String,
}

fn rels(entries: &[(&str, &str, &str)]) -> String {
    let body: String = entries
        .iter()
        .map(|(id, kind, target)| {
            format!("<Relationship Id=\"{id}\" Type=\"{RT}/{kind}\" Target=\"{target}\"/>")
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"{REL_NS}\">{body}</Relationships>"
    )
}

fn package(body: &str, doc_rels: &[(&str, &str, &str)], extras: Vec<Extra<'_>>) -> Vec<u8> {
    let overrides: String = extras
        .iter()
        .filter_map(|extra| {
            extra.content_type.map(|ct| {
                format!(
                    "<Override PartName=\"/{}\" ContentType=\"{ct}\"/>",
                    extra.name
                )
            })
        })
        .collect();
    let content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>{overrides}</Types>"
    );
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {DECLS}><w:body>{body}<w:sectPr/></w:body></w:document>"
    );
    let mut members = vec![
        RawMember::new("[Content_Types].xml", content_types.as_bytes()),
        RawMember::new(
            "_rels/.rels",
            rels(&[("rId1", "officeDocument", "word/document.xml")]).as_bytes(),
        ),
        RawMember::new("word/document.xml", document.as_bytes()),
    ];
    if !doc_rels.is_empty() {
        members.push(RawMember::new(
            "word/_rels/document.xml.rels",
            rels(doc_rels).as_bytes(),
        ));
    }
    for extra in &extras {
        members.push(RawMember::new(extra.name, extra.data.as_bytes()));
    }
    build_zip(&members)
}

fn p(inner: &str) -> String {
    format!("<w:p>{inner}</w:p>")
}

fn r(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn sdt(pr: &str, content: &str) -> String {
    format!("<w:sdt><w:sdtPr>{pr}</w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>")
}

fn tag(value: &str) -> String {
    format!("<w:tag w:val=\"{value}\"/>")
}

fn inline(pr: &str, text: &str) -> String {
    p(&sdt(pr, &format!("<w:r><w:t>{text}</w:t></w:r>")))
}

fn controls_body() -> String {
    let date = |t: &str, inner: &str| inline(&format!("{}<w:date>{inner}</w:date>", tag(t)), "old");
    [
        p(&r("First body paragraph with perfectly ordinary text.")),
        // test_verbs placeholder control (PLACEHOLDER fixture shape)
        p(&format!(
            "{}{}",
            r("Client name: "),
            sdt(
                "<w:alias w:val=\"ClientName\"/><w:tag w:val=\"client-name\"/><w:id w:val=\"2001\"/><w:showingPlcHdr/>",
                "<w:r><w:rPr><w:rStyle w:val=\"PlaceholderText\"/></w:rPr><w:t>Click or tap here to enter text.</w:t></w:r>"
            )
        )),
        inline(
            &format!("{}<w14:checkbox><w14:checked w14:val=\"0\"/></w14:checkbox>", tag("agree")),
            "☐",
        ),
        inline(
            &format!(
                "{}<w14:checkbox><w14:checked w14:val=\"0\"/><w14:checkedState w14:val=\"2611\" w14:font=\"Arial Unicode MS\"/><w14:uncheckedState w14:val=\"2610\" w14:font=\"Arial Unicode MS\"/></w14:checkbox>",
                tag("choice")
            ),
            "☐",
        ),
        inline(&format!("{}<w14:checkbox/>", tag("nochecked")), "☐"),
        inline(
            &format!(
                "{}<w14:checkbox><w14:checked w14:val=\"0\"/><w14:checkedState w14:val=\"zz\"/></w14:checkbox>",
                tag("badglyph")
            ),
            "☐",
        ),
        inline(
            &format!(
                "{}<w14:checkbox><w14:checked w14:val=\"1\"/><w14:uncheckedState w14:val=\"000A\"/></w14:checkbox>",
                tag("ctrlglyph")
            ),
            "☒",
        ),
        inline(
            &format!(
                "{}<w14:checkbox><w14:checked w14:val=\"true\"/><w14:uncheckedState w14:val=\"0x2717\"/></w14:checkbox>",
                tag("hexglyph")
            ),
            "☒",
        ),
        p(&sdt(
            &format!("{}<w14:checkbox><w14:checked w14:val=\"0\"/></w14:checkbox>", tag("complex")),
            "<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>",
        )),
        inline(
            &format!(
                "{}<w:dropDownList><w:listItem w:displayText=\"California\" w:value=\"CA\"/><w:listItem w:displayText=\"New York\" w:value=\"NY\"/></w:dropDownList>",
                tag("state")
            ),
            "Choose.",
        ),
        inline(
            &format!("{}<w:dropDownList><w:listItem w:value=\"OnlyValue\"/><w:listItem w:displayText=\"\" w:value=\"\"/></w:dropDownList>", tag("vo")),
            "x",
        ),
        inline(
            &format!("{}<w:comboBox><w:listItem w:displayText=\"Alpha\" w:value=\"a\"/></w:comboBox>", tag("combo")),
            "pick",
        ),
        date("signed", "<w:dateFormat w:val=\"yyyy-MM-dd\"/>"),
        date(
            "signed-long",
            "<w:dateFormat w:val=\"MMMM d, yyyy\"/><w:lid w:val=\"en-US\"/>",
        ),
        inline(
            &format!("{}<w:date w:fullDate=\"2020-01-01T00:00:00Z\"/>", tag("unformatted")),
            "old",
        ),
        inline(
            &format!(
                "{}<w:date w:fullDate=\"2020-01-01T00:00:00Z\"><w:dateFormat w:val=\"MMMM d, yyyy\"/></w:date>",
                tag("unlocalized")
            ),
            "ancien",
        ),
        date("quarter", "<w:dateFormat w:val=\"Q yyyy\"/>"),
        date("hijri", "<w:calendar w:val=\"hijri\"/><w:dateFormat w:val=\"yyyy\"/>"),
        date("gregorian", "<w:calendar w:val=\"gregorian\"/><w:dateFormat w:val=\"d/M/yy\"/>"),
        date(
            "times",
            "<w:dateFormat w:val=\"dddd, ddd dd MMM MM yy HH:mm:ss h:m:s hh H tt t\"/><w:lid w:val=\"en-GB\"/>",
        ),
        date(
            "literal",
            "<w:dateFormat w:val=\"'Day' d \\o\\f &quot;month&quot; M\"/><w:lid w:val=\"EN\"/>",
        ),
        date("frlocale", "<w:dateFormat w:val=\"d MMMM yyyy\"/><w:lid w:val=\"fr-FR\"/>"),
        date("unterminated", "<w:dateFormat w:val=\"'abc\"/>"),
        date("trailing", "<w:dateFormat w:val=\"yyyy\\\"/>"),
        date("emptyformat", "<w:dateFormat w:val=\"\"/>"),
        p(&sdt(
            &tag("outer"),
            &sdt(&tag("inner"), "<w:r><w:t>inner text</w:t></w:r>"),
        )),
        inline(&format!("{}<w:picture/>", tag("pic")), "picture"),
        inline(&format!("{}<w:group/>", tag("grp")), "group"),
        inline(&format!("{}<w:docPartObj/>", tag("bb")), "building"),
        inline(
            &format!("{}<w:text/><w:lock w:val=\"contentLocked\"/>", tag("locked")),
            "locked",
        ),
        inline(
            &format!("{}<w:lock w:val=\"sdtContentLocked\"/>", tag("sdtlocked")),
            "sdtlocked",
        ),
        inline(
            &format!("{}<w:lock w:val=\"sdtLocked\"/>", tag("sdtonly")),
            "sdtonly",
        ),
        inline(
            &format!(
                "{}<w:lock w:val=\"sdtLocked\"/><w:lock w:val=\"sdtLocked\"/>",
                tag("multilock")
            ),
            "multi",
        ),
        inline(
            &format!("{}<w:dataBinding w:xpath=\"/x\"/>", tag("bound")),
            "synced",
        ),
        inline(&tag("dup"), "one"),
        inline(&tag("dup"), "two"),
        inline("<w:alias w:val=\"OnlyAlias\"/>", "aliased"),
        inline(
            "<w:alias w:val=\"Both\"/><w:tag w:val=\"both\"/><w:text/>",
            "both",
        ),
        sdt(&format!("{}<w:text/>", tag("block")), ""),
        format!("<w:sdt><w:sdtPr>{}<w:text/></w:sdtPr></w:sdt>", tag("block-nocontent")),
        p(&sdt(&format!("{}<w:text/>", tag("inline-empty")), "")),
        p(&format!(
            "<w:sdt><w:sdtPr>{}<w:text/></w:sdtPr></w:sdt>",
            tag("inline-nocontent")
        )),
        format!(
            "<w:customXml w:element=\"wrapper\">{}</w:customXml>",
            sdt(&format!("{}<w:text/>", tag("ambiguous")), "")
        ),
        sdt(
            &tag("multi-para"),
            &format!(
                "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr>{}{}</w:p>{}",
                "<w:r><w:rPr><w:b/></w:rPr><w:t>bold first</w:t></w:r>",
                r(" second run"),
                p(&r("second paragraph"))
            ),
        ),
        p(&sdt(&tag("mixed"), &format!("{}{}", r("run"), p(&r("para"))))),
        p(&sdt(
            &tag("markers-only"),
            "<w:bookmarkStart w:id=\"90\" w:name=\"Inside\"/><w:bookmarkEnd w:id=\"90\"/>",
        )),
        p(&sdt(&tag("shape-mismatch"), &p(&r("block in inline")))),
        p(&sdt(
            &tag("rich-bold"),
            "<w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>bold</w:t></w:r><w:r><w:t> plain</w:t></w:r>",
        )),
        "<w:tbl><w:tr><w:tc>".to_string()
            + &sdt(&tag("in-cell"), &p(&r("cell control")))
            + "</w:tc></w:tr></w:tbl>",
        p(&format!(
            "<mc:AlternateContent><mc:Choice Requires=\"w14\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>",
            sdt(&tag("in-choice"), &r("chosen")),
            sdt(&tag("in-fallback"), &r("fallback"))
        )),
        p(&sdt(
            &format!("{}<w14:checkbox><w14:checked w14:val=\"0\"/></w14:checkbox>", tag("box-formatted")),
            "<w:r><w:rPr><w:rFonts w:ascii=\"Old\"/><w:sz w:val=\"28\"/></w:rPr><w:t>☐</w:t></w:r>",
        )),
        p(&sdt(
            &format!(
                "{}<w14:checkbox><w14:checked w14:val=\"0\"/><w14:checkedState w14:val=\"2612\" w14:font=\"Segoe UI Symbol\"/></w14:checkbox>",
                tag("box-font")
            ),
            "<w:r><w:rPr><w:rFonts w:ascii=\"Old\" w:hint=\"eastAsia\"/><w:sz w:val=\"28\"/></w:rPr><w:t>☐</w:t></w:r>",
        )),
    ]
    .concat()
}

/// A package whose document root declares only the `w` namespace (new
/// `r:`/`w14:` markup must declare its own prefixes).
fn w_only(body: &str) -> Vec<u8> {
    crate::hostile::build::minimal_package_with(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr/></w:body></w:document>"
    ))
}

fn bound_body() -> String {
    let store1 = "{11111111-1111-1111-1111-111111111111}";
    let store2 = "{22222222-2222-2222-2222-222222222222}";
    let ns0 = "xmlns:ns0='http://example.com/form'";
    let bound = |name: &str, extra: &str, mappings: Option<&str>, xpath: &str, store: &str| {
        let mapping = mappings
            .map(|m| format!(" w:prefixMappings=\"{m}\""))
            .unwrap_or_default();
        inline(
            &format!(
                "{}{extra}<w:dataBinding{mapping} w:xpath=\"{xpath}\" w:storeItemID=\"{store}\"/>",
                tag(name)
            ),
            "old",
        )
    };
    [
        p(&r("First body paragraph with perfectly ordinary text.")),
        bound("bound", "", Some(ns0), "/ns0:root/ns0:name", store1),
        bound(
            "bound-locked",
            "<w:lock w:val=\"contentLocked\"/>",
            Some(ns0),
            "/ns0:root/ns0:name",
            store1,
        ),
        bound(
            "bound-box",
            "<w14:checkbox><w14:checked w14:val=\"0\"/></w14:checkbox>",
            Some(ns0),
            "/ns0:root/ns0:name",
            store1,
        ),
        bound("bound-pic", "<w:picture/>", Some(ns0), "/ns0:root/ns0:name", store1),
        bound("bound-text", "<w:text/>", Some(ns0), "/ns0:root/ns0:name", store1),
        bound("bound-attr", "", Some(ns0), "/ns0:root/ns0:name/@status", store1),
        bound("bound-nomap", "", None, "/ns0:root/ns0:name", store1),
        bound("bound-nil", "", Some(ns0), "/ns0:root/ns0:nilled", store1),
        bound(
            "bound-predicate",
            "",
            Some(ns0),
            "/ns0:root/ns0:item[@key='b']",
            store1,
        ),
        bound(
            "bound-positional",
            "",
            Some(ns0),
            "/ns0:root[1]/ns0:item[last()]",
            store1,
        ),
        bound("bound-descendant", "", Some(ns0), "//ns0:item[1]", store1),
        bound("bad-syntax", "", Some(ns0), "/ns0:root/[[[", store1),
        bound("bad-function", "", Some(ns0), "bogus-fn(/ns0:root)", store1),
        bound("bad-count", "", Some(ns0), "count(/ns0:root/ns0:name)", store1),
        bound("bad-string", "", Some(ns0), "string(/ns0:root/ns0:name)", store1),
        bound("bad-boolean", "", Some(ns0), "boolean(/ns0:root)", store1),
        bound("bad-prefix", "", Some(ns0), "/zz:root", store1),
        bound("no-match", "", Some(ns0), "/ns0:root/ns0:missing", store1),
        bound("text-target", "", Some(ns0), "/ns0:root/ns0:name/text()", store1),
        bound("wrong-store", "", Some(ns0), "/ns0:root/ns0:name", "{99999999-0000-0000-0000-000000000000}"),
        inline(
            &format!("{}<w:dataBinding w:prefixMappings=\"{ns0}\" w:xpath=\"\" w:storeItemID=\"{store1}\"/>", tag("no-xpath")),
            "old",
        ),
        bound("bound-default", "", None, "/root/name", store2),
        bound(
            "bound-default-map",
            "",
            Some("xmlns='http://example.com/form'"),
            "/root/name",
            store2,
        ),
        bound("bound-default-text", "", None, "/root/name/text()", store2),
        p(&sdt(
            &format!(
                "{}<w:dataBinding w:prefixMappings=\"{ns0}\" w:xpath=\"/ns0:root/ns0:name\" w:storeItemID=\"{store1}\"/>",
                tag("bound-nested")
            ),
            &sdt(&tag("bound-inner"), &r("inner")),
        )),
    ]
    .concat()
}

fn props(store: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?><ds:datastoreItem xmlns:ds=\"http://schemas.openxmlformats.org/officeDocument/2006/customXml\" ds:itemID=\"{store}\"/>"
    )
}

fn bound_document() -> Vec<u8> {
    let item2 = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ns0:root xmlns:ns0=\"http://example.com/form\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><ns0:name status=\"old\">old</ns0:name><ns0:nilled xsi:nil=\"true\"/><ns0:item key=\"a\">A</ns0:item><ns0:item key=\"b\">B</ns0:item></ns0:root>";
    let item3 = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><root xmlns=\"http://example.com/form\"><name>old</name></root>";
    package(
        &bound_body(),
        &[
            ("rId1", "customXml", "../customXml/item2.xml"),
            ("rId2", "customXml", "../customXml/item3.xml"),
        ],
        vec![
            Extra {
                name: "customXml/item2.xml",
                content_type: None,
                data: item2.to_string(),
            },
            Extra {
                name: "customXml/itemProps2.xml",
                content_type: Some(CT_PROPS),
                data: props("{11111111-1111-1111-1111-111111111111}"),
            },
            Extra {
                name: "customXml/_rels/item2.xml.rels",
                content_type: None,
                data: rels(&[("rId1", "customXmlProps", "itemProps2.xml")]),
            },
            Extra {
                name: "customXml/item3.xml",
                content_type: None,
                data: item3.to_string(),
            },
            Extra {
                name: "customXml/itemProps3.xml",
                content_type: Some(CT_PROPS),
                data: props("{22222222-2222-2222-2222-222222222222}"),
            },
            Extra {
                name: "customXml/_rels/item3.xml.rels",
                content_type: None,
                data: rels(&[("rId1", "customXmlProps", "itemProps3.xml")]),
            },
        ],
    )
}

fn protected(edit: Option<&str>, formatting: bool, body: &str) -> Vec<u8> {
    let mut attrs = String::new();
    if let Some(edit) = edit {
        attrs.push_str(&format!(" w:edit=\"{edit}\""));
    }
    if formatting {
        attrs.push_str(" w:formatting=\"1\"");
    }
    let settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:documentProtection{attrs} w:enforcement=\"1\"/></w:settings>"
    );
    package(
        body,
        &[("rId1", "settings", "settings.xml")],
        vec![Extra {
            name: "word/settings.xml",
            content_type: Some(CT_SETTINGS),
            data: settings,
        }],
    )
}

fn protection_body() -> String {
    [
        p(&r("an ordinary sentence of body text")),
        p(&sdt(
            "<w:alias w:val=\"Field\"/><w:tag w:val=\"field-1\"/><w:id w:val=\"4242\"/>",
            "<w:r><w:t>controlled text</w:t></w:r>",
        )),
    ]
    .concat()
}

fn links_body() -> String {
    [
        p(&r("First body paragraph with perfectly ordinary text.")),
        p(&format!(
            "{}<w:bookmarkStart w:id=\"1\" w:name=\"Term\"/>{}<w:bookmarkEnd w:id=\"1\"/>",
            r("hello "),
            r("world")
        )),
        p(&format!("{}{}", r("frag"), "<w:r><w:rPr><w:i/></w:rPr><w:t>mented text</w:t></w:r>")),
        p("<w:r><w:rPr><w:rStyle w:val=\"Strong\"/><w:b/></w:rPr><w:t xml:space=\"preserve\">styled phrase here</w:t></w:r>"),
        p(&format!("<w:hyperlink w:anchor=\"Term\">{}</w:hyperlink>{}", r("already linked"), r(" tail"))),
        p(&format!("{}<w:proofErr w:type=\"spellStart\"/>{}", r("proof "), r("checked"))),
        p(&r("tab<w:tab/>bed")),
        inline(
            "<w:tag w:val=\"bound\"/><w:dataBinding w:xpath=\"/x\" w:storeItemID=\"{11111111-1111-1111-1111-111111111111}\"/>",
            "BoundLink",
        ),
        inline(
            "<w:tag w:val=\"locked\"/><w:lock w:val=\"contentLocked\"/>",
            "LockedLink",
        ),
        inline("<w:tag w:val=\"plain\"/>", "PlainLink"),
        inline("<w:tag w:val=\"plain-text\"/><w:text/>", "PlainTextLink"),
        inline(
            "<w:tag w:val=\"drop\"/><w:dropDownList><w:listItem w:value=\"DropLink\"/></w:dropDownList>",
            "DropLink",
        ),
        p(&format!(
            "<w:ins w:id=\"7\" w:author=\"A\">{}</w:ins>{}",
            r("inserted "),
            r("words")
        )),
        p(&format!(
            "{}<w:del w:id=\"8\" w:author=\"A\"><w:r><w:delText>gone</w:delText></w:r></w:del>{}",
            r("keep"),
            r("ing")
        )),
        p(&format!(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> DATE </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>",
            r("June 1, 2026")
        )),
        p(&r("Second body paragraph, equally unremarkable.")),
    ]
    .concat()
}

fn bookmarks_body() -> String {
    let field = |instr: &str| p(&format!("<w:fldSimple w:instr=\"{instr}\"/>"));
    let mark = |id: u32, name: &str, text: &str| {
        p(&format!(
            "<w:bookmarkStart w:id=\"{id}\" w:name=\"{name}\"/>{}<w:bookmarkEnd w:id=\"{id}\"/>",
            r(text)
        ))
    };
    [
        p(&format!(
            "{}<w:bookmarkStart w:id=\"1\" w:name=\"DefinedTerm\"/>{}<w:bookmarkEnd w:id=\"1\"/>{}<w:bookmarkStart w:id=\"2\" w:name=\"_GoBack\"/><w:bookmarkEnd w:id=\"2\"/>",
            r("See "),
            "<w:r><w:t>the Master Agreement</w:t></w:r>",
            r(" for definitions.")
        )),
        p("<w:bookmarkStart w:id=\"3\" w:name=\"CurrentText\"/><w:r><w:t>A</w:t></w:r><w:moveFrom w:id=\"20\" w:author=\"A\"><w:r><w:t>B</w:t></w:r></w:moveFrom><w:r><w:t>C</w:t></w:r><w:bookmarkEnd w:id=\"3\"/>"),
        p("<w:r><w:t>X</w:t></w:r><w:del w:id=\"21\" w:author=\"A\"><w:r><w:delText>Y</w:delText></w:r></w:del><w:r><w:t>Z</w:t></w:r>"),
        mark(4, "Referenced", "referenced target"),
        mark(5, "Implicit", "implicit target"),
        mark(6, "Quoted", "quoted target"),
        mark(7, "EMBED", "embedded"),
        mark(8, "Split", "split target"),
        mark(9, "Anchor", "anchor target"),
        mark(10, "KeyPhrase", "Referenced target"),
        mark(11, "Plain", "plain target"),
        mark(12, "Formula", "100"),
        mark(13, "TocMark", "toc target"),
        p(&format!(
            "{}<w:bookmarkStart w:id=\"21\" w:name=\"Dup\"/><w:bookmarkEnd w:id=\"21\"/>",
            r("dup one")
        )),
        p(&format!(
            "{}<w:bookmarkStart w:id=\"22\" w:name=\"Dup\"/><w:bookmarkEnd w:id=\"22\"/>",
            r("dup two")
        )),
        mark(23, "Case", "case one"),
        mark(24, "CASE", "case two"),
        p(&format!(
            "<w:bookmarkStart w:id=\"25\" w:name=\"Across\"/>{}",
            r("across start")
        )),
        p(&format!("{}<w:bookmarkEnd w:id=\"25\"/>", r("across end"))),
        p("<w:bookmarkStart w:id=\"26\" w:name=\"RichAnchor\"/><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t><w:br/><w:t>C</w:t><w:noBreakHyphen/><w:t>D</w:t></w:r><w:bookmarkEnd w:id=\"26\"/>"),
        field(" REF Referenced \\h "),
        field(" Implicit \\* MERGEFORMAT "),
        field(" QUOTE &quot;literal REF Quoted text&quot; "),
        field(" REF OtherBookmark "),
        field(" EMBED Word.Document.12 "),
        field(" =ROUND(Formula,0) "),
        field(" TOC \\o &quot;1-3&quot; \\b TocMark "),
        p("<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> REF </w:instrText></w:r><w:r><w:instrText xml:space=\"preserve\">Split \\h </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>"),
        p(&format!("<w:hyperlink w:anchor=\"Anchor\">{}</w:hyperlink>", r("jump"))),
        p(&format!(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> pa</w:instrText></w:r><w:r><w:instrText xml:space=\"preserve\">geref &quot;key</w:instrText></w:r><w:r><w:instrText xml:space=\"preserve\">phrase&quot; \\h </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>",
            r("(reference)")
        )),
        p(&format!(
            "<w:fldSimple w:instr=\" DATE \">{}</w:fldSimple>",
            r("June 1, 2026")
        )),
        p(&format!(
            "<w:ins w:id=\"30\" w:author=\"A\">{}</w:ins>{}",
            r("pending "),
            r("settled")
        )),
        p(&r("first second third")),
        p("<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">bold edges here</w:t></w:r>"),
    ]
    .concat()
}

fn notes_body() -> String {
    [
        p(&r("First body paragraph with perfectly ordinary text.")),
        p("<w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\">styled note anchor here</w:t></w:r>"),
        inline(
            "<w:tag w:val=\"bound\"/><w:dataBinding w:xpath=\"/x\" w:storeItemID=\"{11111111-1111-1111-1111-111111111111}\"/>",
            "BoundNote",
        ),
        inline(
            "<w:tag w:val=\"locked\"/><w:lock w:val=\"contentLocked\"/>",
            "LockedNote",
        ),
        inline("<w:tag w:val=\"rich\"/>", "RichNote"),
        inline("<w:tag w:val=\"plain-text\"/><w:text/>", "PlainTextNote"),
        p(&format!(
            "{}<w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteReference w:id=\"1\"/></w:r>{}",
            r("marked"),
            r(" after")
        )),
        p(&format!(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> DATE </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>",
            r("June 1, 2026")
        )),
        p(&r("A &amp; B &lt;C&gt; target")),
    ]
    .concat()
}

fn footnotes(extra: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:footnotes xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:footnote w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>{extra}</w:footnotes>"
    )
}

fn with_footnotes(body: &str, extra: &str) -> Vec<u8> {
    package(
        body,
        &[("rId1", "footnotes", "footnotes.xml")],
        vec![Extra {
            name: "word/footnotes.xml",
            content_type: Some(CT_FOOTNOTES),
            data: footnotes(extra),
        }],
    )
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    let simple = |body: String| package(&body, &[], Vec::new());
    vec![
        ("p1-controls".into(), simple(controls_body())),
        ("p1-bound".into(), bound_document()),
        ("p1-readonly".into(), protected(Some("readOnly"), false, &protection_body())),
        ("p1-comments".into(), protected(Some("comments"), false, &protection_body())),
        ("p1-tracked".into(), protected(Some("trackedChanges"), false, &protection_body())),
        ("p1-forms".into(), protected(Some("forms"), false, &protection_body())),
        ("p1-formatting".into(), protected(None, true, &protection_body())),
        ("p1-unknown-edit".into(), protected(Some("lockedTighterThanThis"), true, &protection_body())),
        ("p1-links".into(), simple(links_body())),
        (
            "p1-w-only".into(),
            w_only(&[
                p(&r("plain words to link")),
                p(&sdt(
                    "<w:tag w:val=\"box\"/><w14:checkbox xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"/>",
                    &r("☐"),
                )),
            ]
            .concat()),
        ),
        ("p1-bookmarks".into(), simple(bookmarks_body())),
        (
            "p1-bm-malformed".into(),
            simple(
                [
                    p(&format!(
                        "<w:bookmarkStart w:id=\"1\" w:name=\"Target\"/>{}<w:bookmarkEnd w:id=\"1\"/><w:bookmarkStart w:id=\"bad\" w:name=\"Other\"/>",
                        r("target")
                    )),
                    p(&r("another paragraph")),
                ]
                .concat(),
            ),
        ),
        (
            "p1-bm-dupid".into(),
            simple(p(&format!(
                "{}<w:bookmarkStart w:id=\"5\" w:name=\"target\"/><w:bookmarkEnd w:id=\"5\"/><w:bookmarkStart w:id=\"05\" w:name=\"unrelated\"/><w:bookmarkEnd w:id=\"05\"/>",
                r("bookmark targets")
            ))),
        ),
        (
            "p1-bm-unpaired".into(),
            simple(p(&format!(
                "<w:bookmarkStart w:id=\"1\" w:name=\"Open\"/>{}<w:bookmarkStart w:id=\"2\" w:name=\"Closed\"/><w:bookmarkEnd w:id=\"2\"/>",
                r("unpaired")
            ))),
        ),
        (
            "p1-bm-crossrev".into(),
            simple(p(&format!(
                "<w:ins w:id=\"9\" w:author=\"A\"><w:bookmarkStart w:id=\"1\" w:name=\"Straddle\"/>{}</w:ins>{}<w:bookmarkEnd w:id=\"1\"/>",
                r("inserted "),
                r("plain")
            ))),
        ),
        (
            "p1-bm-badfield".into(),
            simple(
                [
                    p(&format!(
                        "<w:bookmarkStart w:id=\"1\" w:name=\"Target\"/>{}<w:bookmarkEnd w:id=\"1\"/>",
                        r("target")
                    )),
                    p("<w:fldSimple w:instr=\" REF &quot;Target \"/>"),
                ]
                .concat(),
            ),
        ),
        (
            "p1-bm-badid-create".into(),
            simple(p(&format!(
                "<w:bookmarkStart w:id=\"x1\" w:name=\"Weird\"/>{}<w:bookmarkEnd w:id=\"x1\"/>",
                r("weird id text")
            ))),
        ),
        ("p1-notes".into(), simple(notes_body())),
        (
            "p1-notes-existing".into(),
            with_footnotes(
                &notes_body(),
                "<w:footnote w:id=\"3\"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space=\"preserve\"> three</w:t></w:r></w:p></w:footnote><w:footnote w:id=\"1\"><w:p/></w:footnote>",
            ),
        ),
        (
            "p1-notes-badid".into(),
            with_footnotes(&notes_body(), "<w:footnote w:id=\"one\"><w:p/></w:footnote>"),
        ),
    ]
}
