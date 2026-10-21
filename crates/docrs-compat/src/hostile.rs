//! Named hostile/edge archives shared with the Rust test suite's builder.

#[path = "../../docrs/tests/support/mod.rs"]
mod support;

use support::*;
pub use support::{all_fixtures, fixtures_dir};

/// `(name, bytes)` for every generated case.
pub fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, bytes: Vec<u8>| out.push((name.to_string(), bytes));

    add("minimal", build_zip(&minimal_members()));
    let good = build_zip(&minimal_members());
    let mut appended = good.clone();
    appended.extend_from_slice(b"trailing bytes");
    add("appended-bytes", appended);
    add("truncated", good[..good.len() - 5].to_vec());
    add(
        "two-end-records",
        build_zip_with(
            &minimal_members(),
            &ArchiveOptions {
                duplicate_end_record: true,
                ..Default::default()
            },
        ),
    );
    add(
        "count-too-small",
        build_zip_with(
            &minimal_members(),
            &ArchiveOptions {
                count_override: Some(2),
                ..Default::default()
            },
        ),
    );
    add(
        "central-size-off-by-one",
        build_zip_with(
            &minimal_members(),
            &ArchiveOptions {
                central_size_delta: -1,
                ..Default::default()
            },
        ),
    );
    add(
        "prefixed-unshifted",
        build_zip_with(
            &minimal_members(),
            &ArchiveOptions {
                prefix: b"MZ junk".to_vec(),
                ..Default::default()
            },
        ),
    );
    add(
        "archive-comment",
        build_zip_with(
            &minimal_members(),
            &ArchiveOptions {
                comment: b"a comment".to_vec(),
                ..Default::default()
            },
        ),
    );

    let extra = |name: &str| {
        let mut members = minimal_members();
        members.push(RawMember::new(name, b"x"));
        build_zip(&members)
    };
    add("duplicate-member", extra("word/document.xml"));
    add("case-ambiguous-member", extra("WORD/Document.xml"));
    add("file-dir-prefix-collision", extra("word"));
    add("absolute-name", extra("/abs.xml"));
    add("backslash-name", extra("a\\b.xml"));
    add("dotdot-name", extra("a/../b.xml"));
    add("space-name", extra("a b.xml"));
    add("escaped-slash-name", extra("media/a%2fb.png"));
    add("escaped-space-name", extra("media/my%20image.png"));
    add("lowercase-escape-name", extra("media/my%2achar.png"));
    add("unreserved-escape-name", extra("media/%41.png"));
    add("drive-name", extra("C:/x.xml"));
    add("query-name", extra("q?.xml"));
    add("fragment-name", extra("q#1.xml"));
    add("orphan-member", extra("customXml/orphan.bin"));
    {
        let mut members = minimal_members();
        let mut nul = RawMember::new("extra.xml", b"x");
        nul.name = b"extra.xml\0hidden".to_vec();
        members.push(nul);
        add("nul-suffix-name", build_zip(&members));
    }
    {
        let mut members = minimal_members();
        let mut utf8 = RawMember::new("x", b"x");
        utf8.name = "word/résumé.xml".as_bytes().to_vec();
        utf8.flags = 0x0800;
        members.push(utf8);
        add("non-ascii-name", build_zip(&members));
    }
    {
        let mut members = minimal_members();
        members.insert(0, RawMember::new("word/", b"").stored());
        add("directory-record", build_zip(&members));
        let mut members = minimal_members();
        members.insert(0, RawMember::new("word/", b"payload").stored());
        add("directory-record-with-data", build_zip(&members));
    }

    let variant = |edit: &dyn Fn(&mut RawMember)| {
        let mut members = minimal_members();
        edit(&mut members[2]);
        build_zip(&members)
    };
    add("encrypted-flag", variant(&|m| m.flags = 0x1));
    add("patched-data-flag", variant(&|m| m.flags = 0x20));
    add("bzip2-method", variant(&|m| m.method_override = Some(12)));
    add(
        "symlink-entry",
        variant(&|m| m.external_attr = 0o120777 << 16),
    );
    add("dos-directory-attr", variant(&|m| m.external_attr |= 0x10));
    add("bad-crc", variant(&|m| m.crc_override = Some(0xDEADBEEF)));
    add(
        "local-crc-mismatch",
        variant(&|m| m.local_crc_override = Some(1)),
    );
    add(
        "local-flags-mismatch",
        variant(&|m| m.local_flags = Some(0x0800)),
    );
    add(
        "local-name-mismatch",
        variant(&|m| m.local_name = Some(b"word/other.xml".to_vec())),
    );
    add(
        "trailing-member-data",
        variant(&|m| m.trailing_garbage = b"xx".to_vec()),
    );
    add("extract-version-64", variant(&|m| m.extract_version = 64));
    add(
        "corrupt-extra-field",
        variant(&|m| m.central_extra = vec![0x01, 0x00, 0x10, 0x00]),
    );
    add(
        "declared-size-too-small",
        variant(&|m| m.declared_size_override = Some(m.data.len() as u32 - 1)),
    );
    add(
        "declared-size-too-large",
        variant(&|m| m.declared_size_override = Some(m.data.len() as u32 + 1)),
    );
    add(
        "truncated-deflate",
        variant(&|m| {
            let full = deflate(&m.data);
            m.compressed_override = Some(full[..full.len() / 2].to_vec());
        }),
    );
    add(
        "trailing-deflate-bytes",
        variant(&|m| {
            let mut full = deflate(&m.data);
            full.extend_from_slice(b"\x03\x00");
            m.compressed_override = Some(full);
        }),
    );
    add(
        "invalid-deflate",
        variant(&|m| m.compressed_override = Some(vec![0xFF; 16])),
    );
    add(
        "shared-header-offset",
        variant(&|m| m.header_offset_override = Some(0)),
    );
    {
        let mut members = minimal_members();
        let mut member = RawMember::new("word/document.xml", DOCUMENT_XML.as_bytes()).stored();
        member.declared_size_override = Some(3);
        members[2] = member;
        add("stored-size-mismatch", build_zip(&members));
    }
    {
        let data = DOCUMENT_XML.as_bytes();
        let compressed = deflate(data);
        let crc = crc32fast::hash(data);
        let mut good_descriptor = b"PK\x07\x08".to_vec();
        good_descriptor.extend_from_slice(&crc.to_le_bytes());
        good_descriptor.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        good_descriptor.extend_from_slice(&(data.len() as u32).to_le_bytes());
        let with_descriptor = |descriptor: Vec<u8>| {
            let mut members = minimal_members();
            members[2].flags = 0x8;
            members[2].data_descriptor = Some(descriptor);
            build_zip(&members)
        };
        add(
            "data-descriptor-signed",
            with_descriptor(good_descriptor.clone()),
        );
        add(
            "data-descriptor-unsigned",
            with_descriptor(good_descriptor[4..].to_vec()),
        );
        let mut bad = good_descriptor.clone();
        bad[4] ^= 0xFF;
        add("data-descriptor-bad-crc", with_descriptor(bad));
        add("data-descriptor-wrong-size", with_descriptor(vec![0; 7]));
    }
    {
        let mut members = minimal_members();
        members[0] = RawMember::new("[Content_Types].xml", b"<Types xmlns='wrong'/>");
        add("content-types-wrong-root", build_zip(&members));
        let mut members = minimal_members();
        members[0] = RawMember::new("[Content_Types].xml", b"<!DOCTYPE Types><Types xmlns='http://schemas.openxmlformats.org/package/2006/content-types'/>");
        add("content-types-doctype", build_zip(&members));
        let mut members = minimal_members();
        members.remove(0);
        add("content-types-missing", build_zip(&members));
    }
    let ns = "http://schemas.openxmlformats.org/package/2006/relationships";
    let office =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    let styles = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    let with_rels = |rels: String| {
        let mut members = minimal_members();
        members[1] = RawMember::new("_rels/.rels", rels.as_bytes());
        build_zip(&members)
    };
    add(
        "rel-missing-target",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/missing.xml'/></Relationships>"
        )),
    );
    add(
        "rel-two-office-documents",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/document.xml'/><Relationship Id='rId2' Type='{office}' Target='word/document.xml'/></Relationships>"
        )),
    );
    add(
        "rel-external-office-document",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='http://x' TargetMode='External'/></Relationships>"
        )),
    );
    add(
        "rel-no-office-document",
        with_rels(format!("<Relationships xmlns='{ns}'/>")),
    );
    add(
        "rel-wrong-content-type",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/document.xml'/><Relationship Id='rId2' Type='{styles}' Target='word/document.xml'/></Relationships>"
        )),
    );
    add(
        "rel-case-mismatch-target",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='WORD/Document.XML'/></Relationships>"
        )),
    );
    add(
        "rel-duplicate-id",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/document.xml'/><Relationship Id='rId1' Type='x' Target='word/document.xml'/></Relationships>"
        )),
    );
    add(
        "rel-invalid-id",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='1rId' Type='{office}' Target='word/document.xml'/></Relationships>"
        )),
    );
    add(
        "rel-bad-target-mode",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Relationship Id='rId1' Type='{office}' Target='word/document.xml' TargetMode='Sideways'/></Relationships>"
        )),
    );
    add(
        "rel-unexpected-element",
        with_rels(format!(
            "<Relationships xmlns='{ns}'><Other/></Relationships>"
        )),
    );
    add(
        "rel-malformed",
        with_rels(format!("<Relationships xmlns='{ns}'>")),
    );
    {
        let mut members = minimal_members();
        members[0] = RawMember::new(
            "[Content_Types].xml",
            CT_XML
                .replace(
                    "wordprocessingml.document.main",
                    "wordprocessingml.template.main",
                )
                .as_bytes(),
        );
        add("template-main-part", build_zip(&members));
    }
    add(
        "document-truncated-xml",
        minimal_package_with("<w:document"),
    );
    add(
        "document-undeclared-prefix",
        minimal_package_with("<x:document/>"),
    );
    add(
        "document-internal-dtd",
        minimal_package_with(
            "<!DOCTYPE x [<!ENTITY a 'b'>]><w:document xmlns:w='http://schemas.openxmlformats.org/wordprocessingml/2006/main'/>",
        ),
    );
    let deep = |levels: usize| {
        format!(
            "<w:document xmlns:w='http://schemas.openxmlformats.org/wordprocessingml/2006/main'>{}{}</w:document>",
            "<w:x>".repeat(levels),
            "</w:x>".repeat(levels)
        )
    };
    add("xml-depth-256", minimal_package_with(&deep(255)));
    add("xml-depth-257", minimal_package_with(&deep(256)));
    {
        let text = "<?xml version=\"1.0\" encoding=\"UTF-16\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>Ünï 😀</w:t></w:r></w:p></w:body></w:document>";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let mut members = minimal_members();
        members[2] = RawMember::new("word/document.xml", &bytes);
        add("utf16-document", build_zip(&members));
    }
    out
}

const W_NS_DECLS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" \
xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" \
xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" \
xmlns:x=\"urn:unknown\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

/// Synthetic documents exercising Unicode and structural search edges.
pub fn synthetic_documents() -> Vec<(String, Vec<u8>)> {
    let body = |inner: &str| {
        minimal_package_with(&format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {W_NS_DECLS}><w:body>{inner}</w:body></w:document>"
        ))
    };
    let p = |runs: &str| format!("<w:p>{runs}</w:p>");
    let r = |text: &str| format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>");
    let mut out = Vec::new();
    out.push((
        "synthetic-unicode".to_string(),
        body(
            &[
                p(&format!(
                    "{}{}{}",
                    r("Stra"),
                    r("ße und STRASSE"),
                    r(" ſtill")
                )),
                p(&format!("{}{}", r("İstanbul ﬁle "), r("Ǆemal"))),
                p(&format!(
                    "{}{}",
                    r("emoji 😀👍🏽 end"),
                    r(" e\u{301}t\u{e9} café")
                )),
                p(&r("ᠮᠣᠩᠭᠣᠯ ᠪᠢᠴᠢᠭ\u{180e}text")),
                p(&format!(
                    "{}{}",
                    r("non\u{a0}break\u{2007}fig\u{202f}narrow\u{2009}thin"),
                    r("\u{3000}ideo\u{85}sep\u{2028}line")
                )),
                p(&r("“Smart” ‘quotes’ – dash — minus − soft\u{ad}hyphen")),
                p(""),
                p(&r("ΣΑΣ σας ς")),
            ]
            .concat(),
        ),
    ));
    out.push((
        "synthetic-structure".to_string(),
        body(&[
            p(&format!(
                "{}<w:r><w:tab/><w:t>after tab</w:t><w:br/><w:t>after br</w:t><w:br w:type=\"page\"/><w:t>after page</w:t></w:r>",
                r("before")
            )),
            p(&format!("{}<w:r><w:sym w:font=\"Wingdings\" w:char=\"F0FC\"/></w:r>{}", r("sym"), r("bol"))),
            p(&format!("{}<w:r><w:noBreakHyphen/><w:t>hyphen</w:t></w:r>", r("no"))),
            p(&format!(
                "<w:ins w:id=\"1\" w:author=\"A\"><w:r><w:t>inserted </w:t></w:r></w:ins><w:del w:id=\"2\" w:author=\"A\"><w:r><w:delText>deleted </w:delText></w:r></w:del>{}",
                r("plain")
            )),
            p(&format!(
                "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>{}",
                r("7"),
                r(" page")
            )),
            p(&format!("<w:hyperlink r:id=\"rId9\">{}</w:hyperlink>{}", r("linked text"), r(" tail"))),
            format!("<w:sdt><w:sdtPr><w:tag w:val=\"t\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>", p(&r("in control"))),
            p(&format!(
                "<w:r><mc:AlternateContent><mc:Choice Requires=\"wps\"><w:drawing><wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx></w:drawing></mc:Choice><mc:Fallback><w:pict><w:t>fallback dup</w:t></w:pict></mc:Fallback></mc:AlternateContent></w:r>{}",
                p(&r("boxed text")),
                r("anchor para")
            )),
            p(&format!("<mc:AlternateContent><mc:Choice Requires=\"x\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>", r("unsupported choice"), r("fallback chosen"))),
            p(&format!("{}<x:unknown>{}</x:unknown>{}<w:r><!-- c --><w:t>comment run</w:t></w:r>", r("pre"), r("inside unknown"), r("post"))),
            format!(
                "<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr>{}</w:tc></w:tr></w:tbl>",
                p(&r("cell one")),
                p(&r("cell two")),
                p(&r("merged cell"))
            ),
            p(&format!("<w:fldSimple w:instr=\" DATE \">{}</w:fldSimple>", r("simple field"))),
            p(&format!("<w:moveFrom w:id=\"5\" w:author=\"A\">{}</w:moveFrom><w:moveTo w:id=\"6\" w:author=\"A\">{}</w:moveTo>", r("moved"), r("moved"))),
        ]
        .concat()),
    ));
    out
}

/// Synthetic documents targeting M3 replacement rules.
pub fn m3_documents() -> Vec<(String, Vec<u8>)> {
    let doc = |inner: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {W_NS_DECLS}><w:body>{inner}</w:body></w:document>"
        )
    };
    let r = |text: &str| format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>");
    let rb = |props: &str, text: &str| {
        format!("<w:r><w:rPr>{props}</w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
    };
    let body = [
        format!("<w:p>{}{}</w:p>", rb("<w:b/>", "Omeg"), rb("<w:i/>", "a point")),
        format!("<w:p>{}{}{}</w:p>", rb("<w:b/>", "Pay"), rb("<w:b/>", "ment due"), r(" soon")),
        format!(
            "<w:p>{}<w:bookmarkStart w:id=\"1\" w:name=\"Clause\"/>{}<w:bookmarkEnd w:id=\"1\"/>{}</w:p>",
            r("See "),
            r("clause"),
            r(" here")
        ),
        format!("<w:p>{}<w:proofErr w:type=\"spellStart\"/>{}<w:proofErr w:type=\"spellEnd\"/>{}</w:p>", r("Teh "), r("speling"), r(" eror")),
        format!("<w:p>{}<w:hyperlink r:id=\"rId7\">{}</w:hyperlink>{}</w:p>", r("Visit "), r("our site"), r(" today")),
        format!("<w:p><w:ins w:id=\"3\" w:author=\"A\" w:date=\"2024-01-01T00:00:00Z\">{}</w:ins>{}</w:p>", r("pending words"), r(" base words")),
        "<w:p><w:r><w:t>Section 3.</w:t><w:tab/><w:t>Termination</w:t></w:r></w:p>".to_string(),
        format!("<w:p>{}{}</w:p>", rb("<w:b/>", "bold"), rb("<w:i/>", "italic")),
        "<w:sdt><w:sdtPr><w:showingPlcHdr/><w:rPr><w:rStyle w:val=\"PlaceholderText\"/></w:rPr></w:sdtPr><w:sdtContent><w:p><w:r><w:rPr><w:rStyle w:val=\"PlaceholderText\"/></w:rPr><w:t>Click here to enter text.</w:t></w:r></w:p></w:sdtContent></w:sdt>".to_string(),
        format!("<w:p><w:sdt><w:sdtPr><w:lock w:val=\"contentLocked\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>", r("locked value")),
        format!("<w:p><w:sdt><w:sdtPr><w:dataBinding w:xpath=\"/a\" w:storeItemID=\"{{1}}\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>", r("bound value")),
        format!("<w:p><w:sdt><w:sdtPr><w:dropDownList/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>", r("choice value")),
        format!("<w:p><w:sdt><w:sdtPr><w:text/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>", r("plain value")),
        format!("<w:p>{}</w:p>", r("trailing spaces here")),
        "<w:p><w:r><w:t>one</w:t><w:t>two</w:t></w:r></w:p>".to_string(),
    ]
    .concat();
    let protected = |edit: &str| {
        package_with_settings(
            &doc(&format!("<w:p>{}</w:p>", r("Protected body text"))),
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:documentProtection w:edit=\"{edit}\" w:enforcement=\"1\"/></w:settings>"
            ),
        )
    };
    vec![
        ("m3-rules".to_string(), minimal_package_with(&doc(&body))),
        ("m3-protected-readonly".to_string(), protected("readOnly")),
        ("m3-protected-comments".to_string(), protected("comments")),
        (
            "m3-protected-tracked".to_string(),
            protected("trackedChanges"),
        ),
        (
            "m3-protected-unknown".to_string(),
            protected("somethingNew"),
        ),
    ]
}

/// Hand-written cases for the M3 documents.
pub fn m3_extra_cases(name: &str) -> Vec<serde_json::Value> {
    use serde_json::json;
    match name {
        "m3-rules" => vec![
            json!({"needle": "Omega point", "with": "Alpha point"}),
            json!({"needle": "Omega point", "with": "Omega points"}),
            json!({"needle": "Omega point", "with": "Omegb point"}),
            json!({"needle": "Payment due", "with": "Payment overdue"}),
            json!({"needle": "Payment due", "with": "Invoice due"}),
            json!({"needle": "See clause here", "with": "See here"}),
            json!({"needle": "clause", "with": "section"}),
            json!({"needle": "See clause", "with": "Saw clause"}),
            json!({"needle": "e clause h", "with": "e article h"}),
            json!({"needle": "Teh speling", "with": "The speling"}),
            json!({"needle": "Teh speling eror", "with": "The spelling error"}),
            json!({"needle": "speling", "with": "spelling"}),
            json!({"needle": "Visit our site", "with": "Visit their site"}),
            json!({"needle": "Visit our", "with": "Visiting our"}),
            json!({"needle": "e our s", "with": "e my s"}),
            json!({"needle": "our site", "with": "the site"}),
            json!({"needle": "pending words", "with": "pending text"}),
            json!({"needle": "base words", "with": "base text"}),
            json!({"needle": "Section 3.\tTermination", "with": "Section 4. Termination"}),
            json!({"needle": "Section 3.\tTermination", "with": "Section 3. Ending"}),
            json!({"needle": "Section 3.\tTermination", "with": "Section 3 Termination"}),
            json!({"needle": "SECTION 3. TERMINATION", "with": "Section 5. Termination", "match": "normalized"}),
            json!({"needle": "bolditalic", "with": "boldXitalic"}),
            json!({"needle": "bolditalic", "with": "boldital"}),
            json!({"needle": "Click here to enter text.", "with": "Acme Corp"}),
            json!({"needle": "Click here", "with": "Tap here"}),
            json!({"needle": "locked value", "with": "new value"}),
            json!({"needle": "bound value", "with": "new value"}),
            json!({"needle": "choice value", "with": "new value"}),
            json!({"needle": "plain value", "with": "new value"}),
            json!({"needle": "trailing spaces here", "with": " padded "}),
            json!({"needle": "here", "with": "there", "all": true}),
            json!({"needle": "value", "with": "VALUE", "all": true}),
            json!({"needle": "onetwo", "with": "one2"}),
            json!({"needle": "onetwo", "with": "1two"}),
            json!({"needle": "onetwo", "with": "onethree"}),
            json!({"needle": "words", "with": "terms", "all": true}),
            json!({"needle": "point", "with": "point", "again": "pointe"}),
            json!({"needle": "point", "with": "dot", "view": "original", "deviation": "port plan: targets captured under original/all views are inspection-only; upstream edits them"}),
            json!({"needle": "point", "with": "dot\u{1}"}),
        ],
        "m3-protected-readonly"
        | "m3-protected-comments"
        | "m3-protected-tracked"
        | "m3-protected-unknown" => vec![
            json!({"needle": "body", "with": "BODY"}),
            json!({"needle": "body", "with": "BODY", "acknowledge": true}),
            json!({"needle": "body", "with": "BODY", "all": true}),
        ],
        _ => Vec::new(),
    }
}

/// Synthetic documents for M4 authoring cases (see `synthetic/`).
pub fn m4_documents() -> Vec<(String, Vec<u8>)> {
    crate::synthetic::documents()
}

/// A `synthetic:<name>` document's bytes. The builders are deterministic,
/// so the whole catalogue is built once and shared (the first of a
/// repeated name wins, as a linear search would find it).
pub fn synthetic_document(name: &str) -> Option<&'static [u8]> {
    static ALL: std::sync::OnceLock<std::collections::HashMap<String, Vec<u8>>> =
        std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = std::collections::HashMap::new();
        for (name, bytes) in synthetic_documents()
            .into_iter()
            .chain(m3_documents())
            .chain(m4_documents())
        {
            all.entry(name).or_insert(bytes);
        }
        all
    })
    .get(name)
    .map(Vec::as_slice)
}

/// Shared builders for synthetic documents.
#[allow(unused_imports, dead_code)]
pub mod build {
    pub use super::support::{
        CT_XML, DOCUMENT_XML, ROOT_RELS, RawMember, build_zip, minimal_members,
        minimal_package_with, package_with_settings,
    };
    /// Namespace declarations used by the synthetic documents.
    pub const DECLS: &str = super::W_NS_DECLS;
}
