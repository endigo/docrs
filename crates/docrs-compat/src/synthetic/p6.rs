//! Synthetic documents for work package P6 (composition): destinations and
//! sources mirroring the documents upstream's composition tests build with
//! python-docx and raw `OxmlElement` edits. Every package starts from the
//! compacted default template (python-docx `remove_blank_text` semantics)
//! or the compact `minimal.docx` fixture; images are generated here.

use crate::hostile::build::{RawMember, build_zip};

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const HEADER_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
const FOOTER_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";
const DRAWING_NS: &str = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// A 1×1 24-bit BMP of the given colour.
fn bmp(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&58u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[b, g, r, 0]);
    out
}

/// A package being assembled from a compact base.
struct Pkg {
    members: Vec<(String, Vec<u8>)>,
}

impl Pkg {
    fn from_bytes(bytes: &[u8]) -> Pkg {
        let contents = docrs::package::zip_read::read_archive(bytes, &docrs::Limits::default())
            .expect("base package reads");
        let members = contents
            .members
            .into_iter()
            .map(|member| {
                if member.name.ends_with(".xml") || member.name.ends_with(".rels") {
                    let tree = docrs::parts::template(&member.data).expect("XML part parses");
                    (member.name, docrs::xml::serialize(&tree))
                } else {
                    (member.name, member.data)
                }
            })
            .collect();
        Pkg { members }
    }

    /// The default template (python-docx `docx.Document()`), compacted.
    fn blank() -> Pkg {
        let (bytes, _) = docrs::Document::new()
            .patch_bytes()
            .expect("blank document serializes");
        Pkg::from_bytes(&bytes)
    }

    /// `generated/minimal-clean/minimal.docx`.
    fn minimal() -> Pkg {
        let path = crate::hostile::fixtures_dir().join("generated/minimal-clean/minimal.docx");
        Pkg::from_bytes(&std::fs::read(path).expect("minimal fixture"))
    }

    fn text(&self, name: &str) -> String {
        let (_, data) = self
            .members
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("member {name}"));
        String::from_utf8(data.clone()).expect("utf-8 part")
    }

    fn put(mut self, name: &str, data: impl Into<Vec<u8>>) -> Pkg {
        let data = data.into();
        match self.members.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = data,
            None => self.members.push((name.to_string(), data)),
        }
        self
    }

    fn remove(mut self, name: &str) -> Pkg {
        self.members.retain(|(n, _)| n != name);
        self
    }

    fn edit(self, name: &str, f: impl FnOnce(String) -> String) -> Pkg {
        let text = f(self.text(name));
        self.put(name, text)
    }

    /// Replace the body content before the final `w:sectPr`.
    fn body(self, content: &str) -> Pkg {
        self.edit("word/document.xml", |doc| {
            let start = doc.find("<w:body>").expect("body") + "<w:body>".len();
            let sect = doc.rfind("<w:sectPr").expect("sectPr");
            format!("{}{content}{}", &doc[..start], &doc[sect..])
        })
    }

    /// Append `content` to the body before the final `w:sectPr`.
    fn body_append(self, content: &str) -> Pkg {
        self.edit("word/document.xml", |doc| {
            let sect = doc.rfind("<w:sectPr").expect("sectPr");
            format!("{}{content}{}", &doc[..sect], &doc[sect..])
        })
    }

    /// Insert `content` at the start of the final `w:sectPr`.
    fn sect_prepend(self, content: &str) -> Pkg {
        self.edit("word/document.xml", |doc| {
            let sect = doc.rfind("<w:sectPr").expect("sectPr");
            let open = sect + doc[sect..].find('>').expect("sectPr open") + 1;
            format!("{}{content}{}", &doc[..open], &doc[open..])
        })
    }

    fn replace(self, name: &str, from: &str, to: &str) -> Pkg {
        self.edit(name, |text| {
            assert!(text.contains(from), "{name} lacks {from}");
            text.replacen(from, to, 1)
        })
    }

    fn style(self, xml: &str) -> Pkg {
        self.edit("word/styles.xml", |text| {
            text.replace("</w:styles>", &format!("{xml}</w:styles>"))
        })
    }

    fn settings(self, xml: &str, before: &str) -> Pkg {
        self.replace("word/settings.xml", before, &format!("{xml}{before}"))
    }

    fn rel(self, source: &str, id: &str, kind: &str, target: &str, external: bool) -> Pkg {
        let (dir, file) = source.rsplit_once('/').unwrap_or(("", source));
        let member = if dir.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{dir}/_rels/{file}.rels")
        };
        let mode = if external {
            " TargetMode=\"External\""
        } else {
            ""
        };
        let record =
            format!("<Relationship Id=\"{id}\" Type=\"{RT}/{kind}\" Target=\"{target}\"{mode}/>");
        if self.members.iter().any(|(n, _)| *n == member) {
            self.edit(&member, |text| {
                text.replace("</Relationships>", &format!("{record}</Relationships>"))
            })
        } else {
            let text = format!(
                "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{record}</Relationships>"
            );
            self.put(&member, text)
        }
    }

    fn override_ct(self, part: &str, content_type: &str) -> Pkg {
        self.edit("[Content_Types].xml", |text| {
            text.replace(
                "</Types>",
                &format!("<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/></Types>"),
            )
        })
    }

    fn default_ct(self, ext: &str, content_type: &str) -> Pkg {
        self.edit("[Content_Types].xml", |text| {
            text.replace(
                "</Types>",
                &format!("<Default Extension=\"{ext}\" ContentType=\"{content_type}\"/></Types>"),
            )
        })
    }

    /// A header (`header`) or footer part related as `id`, referenced from
    /// the final section as `kind` (`default`/`even`/`first`).
    fn hdrftr(self, header: bool, number: u32, id: &str, kind: &str, content: &str) -> Pkg {
        let (tag, file, ct, rel, reference) = if header {
            (
                "hdr",
                format!("header{number}.xml"),
                HEADER_CT,
                "header",
                "headerReference",
            )
        } else {
            (
                "ftr",
                format!("footer{number}.xml"),
                FOOTER_CT,
                "footer",
                "footerReference",
            )
        };
        let xml = format!(
            "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<w:{tag} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\">{content}</w:{tag}>"
        );
        self.put(&format!("word/{file}"), xml)
            .override_ct(&format!("word/{file}"), ct)
            .rel("word/document.xml", id, rel, &file, false)
            .sect_prepend(&format!("<w:{reference} w:type=\"{kind}\" r:id=\"{id}\"/>"))
    }

    fn build(self) -> Vec<u8> {
        let members: Vec<RawMember> = self
            .members
            .iter()
            .map(|(name, data)| RawMember::new(name, data))
            .collect();
        build_zip(&members)
    }
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

fn p_style(style: &str, text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>"
    )
}

fn header_p(text: &str, extra: &str) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>{extra}</w:p>"
    )
}

fn bookmark(id: &str, name: &str, inner: &str) -> String {
    format!(
        "<w:bookmarkStart w:id=\"{id}\" w:name=\"{name}\"/>{inner}<w:bookmarkEnd w:id=\"{id}\"/>"
    )
}

fn run(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn simple_field(instr: &str, result: &str) -> String {
    format!(
        "<w:fldSimple w:instr=\"{instr}\">{}</w:fldSimple>",
        run(result)
    )
}

fn fld_char(kind: &str) -> String {
    format!("<w:r><w:fldChar w:fldCharType=\"{kind}\"/></w:r>")
}

fn instr(text: &str) -> String {
    format!("<w:r><w:instrText xml:space=\"preserve\">{text}</w:instrText></w:r>")
}

fn complex_ref(fragments: &[&str], result: &str) -> String {
    let mut out = fld_char("begin");
    for fragment in fragments {
        out.push_str(&instr(fragment));
    }
    out.push_str(&fld_char("separate"));
    out.push_str(&run(result));
    out.push_str(&fld_char("end"));
    out
}

fn control(id: Option<u32>, text: &str, bound: bool) -> String {
    let mut pr = String::new();
    if let Some(id) = id {
        pr.push_str(&format!("<w:id w:val=\"{id}\"/>"));
    }
    if bound {
        pr.push_str("<w:dataBinding w:xpath=\"/root/value\" w:storeItemID=\"{AUDIT-STORE}\"/>");
    }
    let pr = if id.is_some() || bound {
        format!("<w:sdtPr>{pr}</w:sdtPr>")
    } else {
        String::new()
    };
    format!(
        "<w:sdt>{pr}<w:sdtContent>{}</w:sdtContent></w:sdt>",
        p(text)
    )
}

fn paragraph_style(id: &str, name: &str, extra: &str) -> String {
    format!(
        "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/>{extra}</w:style>"
    )
}

fn inline_picture(r_id: &str, id: u32) -> String {
    format!(
        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"9525\" cy=\"9525\"/><wp:docPr id=\"{id}\" name=\"Picture {id}\"/><a:graphic {DRAWING_NS}><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"0\" name=\"image.png\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{r_id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"9525\" cy=\"9525\"/></a:xfrm><a:prstGeom prst=\"rect\"/></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )
}

/// `ensure_decimal_definition` output appended to the default numbering
/// (abstract 9, num 10), for sources that need it without ops steps.
fn with_decimal(pkg: Pkg, extra_abstract: &str) -> Pkg {
    let abstract_num = format!(
        "<w:abstractNum w:abstractNumId=\"9\"><w:multiLevelType w:val=\"hybridMultilevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl>{extra_abstract}</w:abstractNum>"
    );
    pkg.edit("word/numbering.xml", |text| {
        let first_num = text.find("<w:num ").expect("num");
        let with_abstract = format!("{}{abstract_num}{}", &text[..first_num], &text[first_num..]);
        with_abstract.replace(
            "</w:numbering>",
            "<w:num w:numId=\"10\"><w:abstractNumId w:val=\"9\"/></w:num></w:numbering>",
        )
    })
}

fn numbered(text: &str, num_id: &str) -> String {
    format!(
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>{}</w:p>",
        run(text)
    )
}

fn dest() -> Pkg {
    Pkg::blank().body(&p("Destination anchor"))
}

fn field_destination(kind: &str, closed: bool) -> Vec<u8> {
    let begin = fld_char("begin");
    let end = if closed {
        fld_char("end")
    } else {
        String::new()
    };
    let body = match kind {
        "paragraph" => format!("<w:p>{begin}{}{end}</w:p>", run("Destination anchor")),
        "table" => format!(
            "<w:p>{begin}{}</w:p><w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"8640\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"8640\"/></w:tcPr><w:p>{}{end}</w:p></w:tc></w:tr></w:tbl>",
            run("Field result begins"),
            run("Destination table")
        ),
        "empty_control" => format!("<w:p>{begin}</w:p><w:sdt><w:sdtPr/><w:sdtContent/></w:sdt>"),
        _ => format!(
            "<w:p>{begin}{}</w:p><w:sdt><w:sdtPr><w:id w:val=\"7\"/></w:sdtPr><w:sdtContent><w:p>{}{end}</w:p></w:sdtContent></w:sdt>",
            run("Field result begins"),
            run("Destination control")
        ),
    };
    Pkg::blank().body(&body).build()
}

fn destinations() -> Vec<(String, Vec<u8>)> {
    let protection = "<w:documentProtection w:edit=\"readOnly\" w:enforcement=\"1\"/>";
    let mut out = vec![
        ("p6-blank".to_string(), Pkg::blank().build()),
        ("p6-dest".into(), dest().build()),
        (
            "p6-dest-protected".into(),
            dest().settings(protection, "<w:defaultTabStop").build(),
        ),
        (
            "p6-dest-two".into(),
            Pkg::blank()
                .body(&format!(
                    "{}{}{}",
                    p("Destination before"),
                    p("destination alpha end"),
                    p("destination beta start")
                ))
                .settings(protection, "<w:defaultTabStop")
                .build(),
        ),
        (
            "p6-dest-houseterm".into(),
            Pkg::minimal()
                .style(&paragraph_style("HouseTerm", "HouseTerm", "<w:rPr><w:sz w:val=\"18\"/></w:rPr>"))
                .build(),
        ),
        (
            "p6-dest-houseterm-same".into(),
            Pkg::minimal()
                .style(
                    &paragraph_style("HouseTerm", "HouseTerm", "<w:rPr><w:sz w:val=\"32\"/></w:rPr>")
                        .replace("w:styleId=\"HouseTerm\"", "w:styleId=\"HT\" w:rsid=\"00AB12CD\""),
                )
                .build(),
        ),
        (
            "p6-dest-houseterm-bold".into(),
            Pkg::minimal()
                .style(&paragraph_style("HouseTerm", "HouseTerm", "<w:rPr><w:b/></w:rPr>"))
                .build(),
        ),
        (
            "p6-dest-fresh-taken".into(),
            Pkg::minimal()
                .style(&paragraph_style("Fresh", "Some other style", ""))
                .style(&paragraph_style("FreshImported", "Yet another style", ""))
                .build(),
        ),
        (
            "p6-dest-term".into(),
            Pkg::minimal()
                .replace(
                    "word/document.xml",
                    "perfectly ordinary text.</w:t></w:r>",
                    "perfectly ordinary text.</w:t></w:r><w:bookmarkStart w:id=\"7\" w:name=\"Term\"/><w:bookmarkEnd w:id=\"7\"/>",
                )
                .build(),
        ),
        (
            "p6-dest-target".into(),
            Pkg::minimal()
                .replace(
                    "word/document.xml",
                    "perfectly ordinary text.</w:t></w:r>",
                    "perfectly ordinary text.</w:t></w:r><w:bookmarkStart w:id=\"7\" w:name=\"Target\"/><w:bookmarkEnd w:id=\"7\"/>",
                )
                .build(),
        ),
        (
            "p6-dest-existing".into(),
            Pkg::minimal()
                .replace(
                    "word/document.xml",
                    "perfectly ordinary text.</w:t></w:r>",
                    "perfectly ordinary text.</w:t></w:r><w:bookmarkStart w:id=\"9\" w:name=\"Existing\"/>",
                )
                .replace(
                    "word/document.xml",
                    "equally unremarkable.</w:t></w:r>",
                    "equally unremarkable.</w:t></w:r><w:bookmarkEnd w:id=\"9\"/>",
                )
                .build(),
        ),
        (
            "p6-dest-x".into(),
            Pkg::minimal()
                .style("<w:style w:type=\"paragraph\" w:styleId=\"X\"><w:name w:val=\"DestX\"/><w:rPr><w:i/></w:rPr></w:style>")
                .build(),
        ),
        (
            "p6-dest-sdt1".into(),
            Pkg::blank().body(&control(Some(1), "Existing control", false)).build(),
        ),
        (
            "p6-dest-cleanup".into(),
            dest()
                .replace(
                    "word/numbering.xml",
                    "</w:numbering>",
                    "<w:numIdMacAtCleanup w:val=\"1\"/></w:numbering>",
                )
                .build(),
        ),
        (
            "p6-dest-badnum".into(),
            dest()
                .replace("word/numbering.xml", "<w:num w:numId=\"1\">", "<w:num w:numId=\"not-a-number\">")
                .build(),
        ),
        (
            "p6-dest-no-numbering".into(),
            dest()
                .replace(
                    "word/_rels/document.xml.rels",
                    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
                    "",
                )
                .replace(
                    "[Content_Types].xml",
                    "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>",
                    "",
                )
                .remove("word/numbering.xml")
                .build(),
        ),
        (
            "p6-dest-shared-p".into(),
            dest()
                .style("<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"SharedStyleId\"><w:name w:val=\"Unrelated destination style\"/></w:style>")
                .build(),
        ),
        (
            "p6-dest-shared-r".into(),
            dest()
                .style("<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"SharedStyleId\"><w:name w:val=\"Unrelated destination style\"/></w:style>")
                .build(),
        ),
        (
            "p6-dest-missingdep".into(),
            dest()
                .style("<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"MissingDependency\"><w:name w:val=\"Unrelated chain destination\"/></w:style>")
                .build(),
        ),
        (
            "p6-dest-clauseref".into(),
            Pkg::blank()
                .body(&p("Destination"))
                .style(&paragraph_style("ClauseRef", "ClauseRef", ""))
                .build(),
        ),
        (
            "p6-dest-plain".into(),
            Pkg::blank().body(&p("Destination")).build(),
        ),
        (
            "p6-dest-grossincome".into(),
            Pkg::blank()
                .body(&format!("<w:p>{}</w:p>", bookmark("0", "GrossIncome", &run("existing"))))
                .build(),
        ),
        (
            "p6-dest-capital-term".into(),
            Pkg::blank()
                .body(&format!("<w:p>{}</w:p>", bookmark("0", "Term", &run("destination term"))))
                .build(),
        ),
        (
            "p6-dest-aaaa".into(),
            Pkg::blank()
                .body(&format!(
                    "<w:p>{}{}</w:p>",
                    run("Destination anchor"),
                    bookmark("8", &"A".repeat(40), &run(""))
                ))
                .build(),
        ),
        (
            "p6-dest-sections2".into(),
            Pkg::minimal()
                .body_append("<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr></w:pPr></w:p>")
                .build(),
        ),
        (
            "p6-dest-sharedmark".into(),
            Pkg::minimal()
                .replace(
                    "word/document.xml",
                    "<w:r><w:t>First body paragraph with perfectly ordinary text.</w:t></w:r>",
                    "<w:r><w:t xml:space=\"preserve\">First body paragraph with </w:t></w:r><w:bookmarkStart w:id=\"0\" w:name=\"SharedMark\"/><w:r><w:t>perfectly ordinary</w:t></w:r><w:bookmarkEnd w:id=\"0\"/><w:r><w:t xml:space=\"preserve\"> text.</w:t></w:r>",
                )
                .build(),
        ),
        (
            "p6-dest-header".into(),
            Pkg::minimal()
                .hdrftr(true, 1, "rId20", "default", &header_p("Dest running", ""))
                .build(),
        ),
        (
            "p6-dest-titlepg".into(),
            Pkg::minimal()
                .hdrftr(true, 1, "rId20", "default", &header_p("Dest running", ""))
                .replace("word/document.xml", "<w:docGrid", "<w:titlePg/><w:docGrid")
                .build(),
        ),
        (
            "p6-dest-table-last".into(),
            Pkg::blank()
                .body(&format!("{}{}", p("Destination anchor"), table("Destination table")))
                .build(),
        ),
        (
            "p6-dest-table-first".into(),
            Pkg::blank()
                .body(&format!("{}{}", table("Destination table"), p("Destination anchor")))
                .build(),
        ),
        (
            "p6-dest-image".into(),
            Pkg::minimal()
                .body_append(&format!("<w:p>{}</w:p>", inline_picture("rId30", 1)))
                .put("word/media/image1.png", hex(super::p4::TINY_PNG_HEX))
                .default_ct("png", "image/png")
                .rel("word/document.xml", "rId30", "image", "media/image1.png", false)
                .build(),
        ),
        (
            "p6-dest-cell".into(),
            Pkg::blank()
                .body(&format!("{}{}", table("Cell anchor"), p("Destination anchor")))
                .build(),
        ),
    ];
    out.push((
        "p6-dest-unicode-mark".into(),
        Pkg::blank()
            .body(&format!(
                "<w:p>{}</w:p><w:p>{}</w:p>",
                bookmark("0", "GRÖSSE-1", &run("Destination anchor")),
                bookmark("1", "Cafe\u{301}", &run("second destination"))
            ))
            .build(),
    ));
    out.push((
        "p6-dest-no-styles".into(),
        dest()
            .replace(
                "word/_rels/document.xml.rels",
                "<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
                "",
            )
            .replace(
                "[Content_Types].xml",
                "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>",
                "",
            )
            .remove("word/styles.xml")
            .build(),
    ));
    out.push((
        "p6-dest-dup-abstract".into(),
        dest()
            .replace(
                "word/numbering.xml",
                "<w:abstractNum w:abstractNumId=\"1\"",
                "<w:abstractNum w:abstractNumId=\"0\"",
            )
            .build(),
    ));
    for kind in ["paragraph", "table", "control", "empty_control"] {
        for closed in [false, true] {
            let state = if closed { "closed" } else { "open" };
            out.push((
                format!("p6-dest-field-{}-{state}", kind.replace('_', "-")),
                field_destination(kind, closed),
            ));
        }
    }
    out
}

fn table(text: &str) -> String {
    format!(
        "<w:tbl><w:tblPr><w:tblW w:type=\"auto\" w:w=\"0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"8640\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"8640\"/></w:tcPr>{}</w:tc></w:tr></w:tbl>",
        p(text)
    )
}

fn sources() -> Vec<(String, Vec<u8>)> {
    let blocks = |names: &[&str]| names.iter().map(|n| p(n)).collect::<String>();
    let src = |body: &str| Pkg::blank().body(body);
    let houseterm = |sz: &str| {
        src(&p_style("HouseTerm", "Large-type clause.")).style(&paragraph_style(
            "HouseTerm",
            "HouseTerm",
            &format!("<w:rPr><w:sz w:val=\"{sz}\"/></w:rPr>"),
        ))
    };
    let chart_ct = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
    let mut out = vec![
        ("p6-src-blocks".to_string(), src(&blocks(&["Block A", "Block B", "Block C", "Block D"])).build()),
        ("p6-src-abc".into(), src(&blocks(&["Block A", "Block B", "Block C"])).build()),
        (
            "p6-src-styles".into(),
            src(&format!(
                "{}{}{}{}",
                p_style("Heading1", "Clause Library"),
                p("Indemnity: the supplier shall hold harmless."),
                p_style("Heading2", "Payment Terms"),
                p("Payment falls due after thirty days.")
            ))
            .build(),
        ),
        (
            "p6-src-clauseterm".into(),
            src(&p_style("ClauseTerm", "A specially styled clause."))
                .style(&paragraph_style("ClauseTerm", "ClauseTerm", "<w:rPr><w:i/></w:rPr>"))
                .build(),
        ),
        ("p6-src-houseterm".into(), houseterm("32").build()),
        (
            "p6-src-fresh".into(),
            src(&p_style("Fresh", "Fresh styled clause."))
                .style(&paragraph_style("Fresh", "Fresh", "<w:rPr><w:b/></w:rPr>"))
                .build(),
        ),
        (
            "p6-src-chain".into(),
            src(&format!("{}{}", p_style("ChildStyle", "Chained clause."), "<w:p><w:r><w:rPr><w:rStyle w:val=\"ClauseChar\"/></w:rPr><w:t>Character run.</w:t></w:r></w:p>"))
                .style(
                    "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"ChildStyle\"><w:name w:val=\"ChildStyle\"/><w:basedOn w:val=\"ParentStyle\"/><w:next w:val=\"Normal\"/><w:link w:val=\"ClauseChar\"/><w:rsid w:val=\"00112233\"/></w:style>",
                )
                .style("<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"ParentStyle\"><w:name w:val=\"ParentStyle\"/><w:basedOn w:val=\"\"/><w:rPr><w:caps/></w:rPr></w:style>")
                .style("<w:style w:type=\"character\" w:customStyle=\"1\" w:styleId=\"ClauseChar\"><w:name w:val=\"ClauseChar\"/><w:link w:val=\"ChildStyle\"/><w:rPr><w:u w:val=\"single\"/></w:rPr></w:style>")
                .build(),
        ),
        (
            "p6-src-term-ref".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p>{}</w:p>",
                bookmark("1", "Term", &run("the definition body")),
                simple_field(" REF Term \\h ", "(cross-reference)")
            ))
            .build(),
        ),
        (
            "p6-src-tracked".into(),
            src(&format!(
                "{}<w:p><w:r><w:t xml:space=\"preserve\">Tracked </w:t></w:r><w:ins w:id=\"1\" w:author=\"Alice Editor\" w:date=\"2024-01-02T03:04:05Z\">{}</w:ins></w:p>",
                p("First body paragraph"),
                run("insertion")
            ))
            .build(),
        ),
        (
            "p6-src-object".into(),
            src("<w:p><w:r><w:t xml:space=\"preserve\">Embedded thing: </w:t></w:r><w:r><w:object/></w:r></w:p>").build(),
        ),
        (
            "p6-src-cross".into(),
            src(&blocks(&["Source before", "source alpha end", "source beta start", "Source after"])).build(),
        ),
        ("p6-src-block".into(), src(&p("Source block")).build()),
        ("p6-src-two".into(), src(&blocks(&["First source block", "Second source block"])).build()),
        ("p6-src-sdt".into(), src(&control(Some(7), "Controlled source block", false)).build()),
        ("p6-src-bound".into(), src(&control(Some(7), "Bound value", true)).build()),
        (
            "p6-src-sdts".into(),
            src(&format!(
                "{}{}",
                control(Some(2), "First imported control", false),
                control(Some(1), "Second imported control", false)
            ))
            .build(),
        ),
        (
            "p6-src-customxml-top".into(),
            src(&format!("{}<w:customXml>{}</w:customXml>", p("Safe-looking source block"), p("custom XML payload"))).build(),
        ),
        (
            "p6-src-altchunk-top".into(),
            src(&format!("{}<w:altChunk r:id=\"rIdMissingChunk\"/>", p("Safe-looking source block"))).build(),
        ),
        (
            "p6-src-customxml-between".into(),
            src(&format!(
                "{}<w:customXml>{}</w:customXml>{}",
                p("First selected block"),
                p("custom XML payload"),
                p("Second selected block")
            ))
            .build(),
        ),
        (
            "p6-src-aaaa".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p>{}</w:p>",
                bookmark("1", &"A".repeat(40), &run("Source bookmark target")),
                complex_ref(&[" r", &format!("ef {}", "A".repeat(20)), &format!("{} \\h ", "A".repeat(20))], "(reference)")
            ))
            .build(),
        ),
        (
            "p6-src-malformed-id".into(),
            src(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"AuditImportedStyle\"/></w:pPr>{}</w:p>",
                bookmark("not-a-number", "Malformed", &run("Malformed target"))
            ))
            .style(&paragraph_style("AuditImportedStyle", "AuditImportedStyle", ""))
            .build(),
        ),
        (
            "p6-src-control-end".into(),
            src(&format!("{}<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt>", p("Selected paragraph"), p("Control boundary"))).build(),
        ),
        (
            "p6-src-slice".into(),
            src(&format!(
                "{}{}<w:customXml><w:p/></w:customXml>{}{}",
                p("Start boundary"),
                p("First selected block"),
                p("Second selected block"),
                p("End boundary")
            ))
            .build(),
        ),
        (
            "p6-src-chart".into(),
            src("<w:p><w:r><w:t>Source chart</w:t></w:r><w:r><w:drawing><c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" r:id=\"rId99\"/></w:drawing></w:r></w:p>")
                .put("word/charts/chart1.xml", "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"/>")
                .override_ct("word/charts/chart1.xml", chart_ct)
                .rel("word/document.xml", "rId99", "chart", "charts/chart1.xml", false)
                .build(),
        ),
        (
            "p6-src-missing-abstract".into(),
            with_decimal(src(&numbered("Source numbered paragraph", "10")), "")
                .replace("word/numbering.xml", "<w:abstractNumId w:val=\"9\"/>", "<w:abstractNumId w:val=\"42\"/>")
                .build(),
        ),
        (
            "p6-src-styled-numbered".into(),
            with_decimal(
                src("<w:p><w:pPr><w:pStyle w:val=\"AuditCompositionStyle\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"10\"/></w:numPr></w:pPr><w:r><w:t>Styled numbered source</w:t></w:r></w:p>"),
                "",
            )
            .style(&paragraph_style("AuditCompositionStyle", "AuditCompositionStyle", ""))
            .build(),
        ),
        (
            "p6-src-undefined-pstyle".into(),
            src("<w:p><w:pPr><w:pStyle w:val=\"SharedStyleId\"/></w:pPr><w:r><w:t>Source with an undefined style</w:t></w:r></w:p>").build(),
        ),
        (
            "p6-src-undefined-rstyle".into(),
            src("<w:p><w:r><w:rPr><w:rStyle w:val=\"SharedStyleId\"/></w:rPr><w:t>Source with an undefined style</w:t></w:r></w:p>").build(),
        ),
        (
            "p6-src-page-field".into(),
            src(&format!("<w:p>{}{}</w:p>", run("Field source"), "<w:fldSimple w:instr=\" PAGE \"/>")).build(),
        ),
        (
            "p6-src-numbered".into(),
            with_decimal(src(&format!("{}{}", numbered("First imported item", "10"), numbered("Second imported item", "10"))), "").build(),
        ),
        (
            "p6-src-numbered-two-lists".into(),
            with_decimal(src(&format!("{}{}{}", numbered("Decimal item", "10"), numbered("Default list item", "1"), numbered("Decimal again", "10"))), "").build(),
        ),
        (
            "p6-src-picture-bullet".into(),
            with_decimal(src(&numbered("Picture bullet item", "10")), "<w:lvl w:ilvl=\"1\"><w:lvlPicBulletId w:val=\"0\"/></w:lvl>")
                .replace(
                    "word/numbering.xml",
                    "<w:abstractNum w:abstractNumId=\"0\"",
                    "<w:numPicBullet w:numPicBulletId=\"0\"><w:pict/></w:numPicBullet><w:abstractNum w:abstractNumId=\"0\"",
                )
                .build(),
        ),
        (
            "p6-src-clauseref".into(),
            with_decimal(src(&format!("<w:p>{}{}</w:p>", run("Field source"), "<w:fldSimple w:instr=\" STYLEREF ClauseRef \"/>")), "")
                .style(&paragraph_style("ClauseRef", "ClauseRef", "<w:pPr><w:numPr><w:numId w:val=\"10\"/></w:numPr></w:pPr>"))
                .build(),
        ),
        (
            "p6-src-grossincome".into(),
            src(&format!("<w:p>{}{}</w:p>", bookmark("0", "GrossIncome", &run("100")), "<w:fldSimple w:instr=\" =ROUND(GrossIncome,0) \"/>")).build(),
        ),
        (
            "p6-src-term-lower".into(),
            src(&format!("<w:p>{}{}</w:p>", bookmark("0", "term", &run("source term")), "<w:fldSimple w:instr=\" REF term \"/>")).build(),
        ),
        (
            "p6-src-ref-outside".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p>{}{}</w:p>",
                bookmark("0", "Term", &run("defined term")),
                run("Reference block"),
                "<w:fldSimple w:instr=\" REF Term \"/>"
            ))
            .build(),
        ),
        (
            "p6-src-link-outside".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p><w:hyperlink w:anchor=\"Term\">{}</w:hyperlink></w:p>",
                bookmark("0", "Term", &run("defined term")),
                run("Link block")
            ))
            .build(),
        ),
        (
            "p6-src-ambiguous".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p>{}</w:p>",
                bookmark("0", "Term", &run("first term")),
                bookmark("1", "TERM", &run("second term"))
            ))
            .build(),
        ),
        (
            "p6-src-linked-numbering".into(),
            with_decimal(src(&numbered("Linked numbering", "10")), "<w:numStyleLink w:val=\"NumLinkStyle\"/>").build(),
        ),
        (
            "p6-src-wide".into(),
            src(&format!(
                "<w:p>{}<w:bookmarkStart w:id=\"5\" w:name=\"Wide\"/></w:p><w:p>{}<w:bookmarkEnd w:id=\"5\"/></w:p>",
                run("range starts here"),
                run("copied tail")
            ))
            .build(),
        ),
        (
            "p6-src-gone".into(),
            src(&format!(
                "<w:p><w:bookmarkStart w:id=\"3\" w:name=\"_GoBack\"/>{}<w:bookmarkEnd w:id=\"3\"/><w:bookmarkStart w:id=\"4\" w:name=\"Kept\"/>{}<w:bookmarkEnd w:id=\"4\"/></w:p>",
                run("cursor noise "),
                run("kept mark")
            ))
            .build(),
        ),
        (
            "p6-src-numbered-style-only".into(),
            with_decimal(src(&p_style("NumberedClause", "clause body text")), "")
                .style("<w:style w:type=\"paragraph\" w:styleId=\"NumberedClause\"><w:name w:val=\"NumberedClause\"/><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"10\"/></w:numPr></w:pPr></w:style>")
                .build(),
        ),
        (
            "p6-src-x-ximported".into(),
            src(&format!("{}{}", p_style("X", "styled one"), p_style("XImported", "styled two")))
                .style("<w:style w:type=\"paragraph\" w:styleId=\"X\"><w:name w:val=\"SourceX\"/><w:rPr><w:b/></w:rPr></w:style>")
                .style("<w:style w:type=\"paragraph\" w:styleId=\"XImported\"><w:name w:val=\"Other\"/><w:rPr><w:b/></w:rPr></w:style>")
                .build(),
        ),
        (
            "p6-src-split-ref".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p>{}</w:p>",
                bookmark("1", "Target", &run("the target text")),
                complex_ref(&[" REF ", "Target \\h "], "(ref)")
            ))
            .build(),
        ),
        (
            "p6-src-picture".into(),
            src(&format!("{}<w:p>{}</w:p>", p("Before the picture."), inline_picture("rId30", 1)))
                .put("word/media/image1.png", hex(super::p4::TINY_PNG_HEX))
                .default_ct("png", "image/png")
                .rel("word/document.xml", "rId30", "image", "media/image1.png", false)
                .build(),
        ),
        (
            "p6-src-two-pictures".into(),
            src(&format!(
                "{}<w:p>{}{}</w:p><w:p>{}</w:p>",
                p("Before the pictures."),
                inline_picture("rId30", 1),
                inline_picture("rId31", 2),
                inline_picture("rId30", 3)
            ))
            .put("word/media/image1.png", hex(super::p4::TINY_PNG_HEX))
            .put("word/media/image2.bmp", bmp(0, 128, 0))
            .default_ct("png", "image/png")
            .default_ct("bmp", "image/bmp")
            .rel("word/document.xml", "rId30", "image", "media/image1.png", false)
            .rel("word/document.xml", "rId31", "image", "media/image2.bmp", false)
            .build(),
        ),
        (
            "p6-src-bad-image".into(),
            src(&format!("{}<w:p>{}</w:p>", p("before image"), inline_picture("rId30", 1)))
                .put("word/media/image1.png", b"not an image at all".to_vec())
                .default_ct("png", "image/png")
                .rel("word/document.xml", "rId30", "image", "media/image1.png", false)
                .build(),
        ),
        (
            "p6-src-linked-picture".into(),
            src(&format!("{}<w:p>{}</w:p>", p("before linked"), inline_picture("rId30", 1).replace("r:embed", "r:link")))
                .rel("word/document.xml", "rId30", "image", "http://example.com/x.png", true)
                .build(),
        ),
        (
            "p6-src-hyperlink".into(),
            src("<w:p><w:r><w:t xml:space=\"preserve\">Visit </w:t></w:r><w:hyperlink r:id=\"rId40\"><w:r><w:t>the terms page</w:t></w:r></w:hyperlink></w:p>")
                .rel("word/document.xml", "rId40", "hyperlink", "https://paper.example/terms", true)
                .build(),
        ),
        (
            "p6-src-tables".into(),
            src(&format!("{}{}{}", p("Tables follow"), table("Source table one"), table("Source table two"))).build(),
        ),
        (
            "p6-src-tables-only".into(),
            src(&format!("{}{}", table("Source table one"), table("Source table two"))).build(),
        ),
        (
            "p6-src-table-p".into(),
            src(&format!("{}{}", table("Lead table"), p("After table"))).build(),
        ),
        (
            "p6-src-sections".into(),
            src(&format!(
                "{}<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr></w:pPr>{}</w:p>{}",
                p("Section one"),
                run("Section break"),
                p("Section two")
            ))
            .build(),
        ),
        (
            "p6-src-comment-top".into(),
            src(&format!("{}<!--body comment-->{}", p("Before comment"), p("After comment"))).build(),
        ),
        ("p6-src-empty".into(), src("").build()),
        (
            "p6-src-unicode-mark".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p>{}</w:p><w:p>{}{}</w:p>",
                bookmark("1", "Größe-1", &run("unicode target")),
                bookmark("2", "café", &run("combining target")),
                "<w:fldSimple w:instr=\" REF Größe-1 \\h \"/>",
                "<w:hyperlink w:anchor=\"CAFÉ\"><w:r><w:t>jump</w:t></w:r></w:hyperlink>"
            ))
            .build(),
        ),
        (
            "p6-src-anchor-link".into(),
            src(&format!(
                "<w:p>{}</w:p><w:p><w:hyperlink w:anchor=\"Term\">{}</w:hyperlink></w:p>",
                bookmark("1", "Term", &run("the definition body")),
                run("jump to term")
            ))
            .build(),
        ),
        (
            "p6-src-vml-image".into(),
            src(&format!(
                "{}<w:p><w:r><w:pict><v:shape style=\"width:1pt;height:1pt\"><v:imagedata r:id=\"rId30\" o:title=\"\"/></v:shape></w:pict></w:r></w:p>",
                p("before vml")
            ))
            .put("word/media/image1.png", hex(super::p4::TINY_PNG_HEX))
            .default_ct("png", "image/png")
            .rel("word/document.xml", "rId30", "image", "media/image1.png", false)
            .build(),
        ),
        (
            "p6-src-vml-relid".into(),
            src(&format!(
                "{}<w:p><w:r><w:pict><v:shape o:relid=\"rId30\" style=\"width:1pt;height:1pt\"/></w:pict></w:r></w:p>",
                p("before vml")
            ))
            .put("word/media/image1.png", hex(super::p4::TINY_PNG_HEX))
            .default_ct("png", "image/png")
            .rel("word/document.xml", "rId30", "image", "media/image1.png", false)
            .build(),
        ),
        (
            "p6-src-numbered-no-part".into(),
            Pkg::blank()
                .body(&numbered("Numbered without part", "1"))
                .replace(
                    "word/_rels/document.xml.rels",
                    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
                    "",
                )
                .replace(
                    "[Content_Types].xml",
                    "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>",
                    "",
                )
                .remove("word/numbering.xml")
                .build(),
        ),
        ("p6-src-num-zero".into(), src(&numbered("Numbering removed", "0")).build()),
        ("p6-src-num-malformed".into(), src(&numbered("Malformed numbering", "one")).build()),
        ("p6-src-num-undefined".into(), src(&numbered("Undefined numbering", "77")).build()),
        (
            "p6-src-clauseref-complex".into(),
            with_decimal(
                src(&format!(
                    "<w:p>{}{}</w:p>",
                    run("Complex field source"),
                    complex_ref(&[" STYLE", "REF Clause", "Ref \\* MERGEFORMAT "], "ref")
                )),
                "",
            )
            .style(&paragraph_style("ClauseRef", "ClauseRef", ""))
            .build(),
        ),
    ];
    for tag in ["basedOn", "link", "next"] {
        out.push((
            format!("p6-src-chain-{tag}"),
            Pkg::blank()
                .body(&p_style(
                    "SourceChainLeaf",
                    "Source with a broken style chain",
                ))
                .style(&paragraph_style(
                    "SourceChainLeaf",
                    "Source chain leaf",
                    &format!("<w:{tag} w:val=\"MissingDependency\"/>"),
                ))
                .build(),
        ));
    }
    out.extend(letterhead_sources());
    out
}

fn letterhead_sources() -> Vec<(String, Vec<u8>)> {
    let m = Pkg::minimal;
    vec![
        (
            "p6-src-hdr".to_string(),
            m().hdrftr(true, 1, "rId20", "default", &header_p("Source letterhead", "")).build(),
        ),
        (
            "p6-src-hdr-ftr".into(),
            m().hdrftr(true, 1, "rId20", "default", &header_p("Source letterhead", ""))
                .hdrftr(false, 1, "rId21", "default", &p("Source footer"))
                .build(),
        ),
        (
            "p6-src-hdr-image".into(),
            m().hdrftr(true, 1, "rId20", "default", &header_p("Header picture", &inline_picture("rId1", 9)))
                .put("word/media/image1.bmp", bmp(0, 128, 0))
                .default_ct("bmp", "image/bmp")
                .rel("word/header1.xml", "rId1", "image", "media/image1.bmp", false)
                .build(),
        ),
        (
            "p6-src-hdr-link".into(),
            m().hdrftr(
                true,
                1,
                "rId20",
                "default",
                "<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr><w:hyperlink r:id=\"rId1\"><w:r><w:rPr><w:rStyle w:val=\"Hyperlink\"/></w:rPr><w:t>SourceHeaderLink</w:t></w:r></w:hyperlink></w:p>",
            )
            .rel("word/header1.xml", "rId1", "hyperlink", "https://example.com/letterhead", true)
            .style("<w:style w:type=\"character\" w:styleId=\"Hyperlink\"><w:name w:val=\"Hyperlink\"/><w:basedOn w:val=\"DefaultParagraphFont\"/><w:uiPriority w:val=\"99\"/><w:unhideWhenUsed/><w:rPr><w:color w:val=\"0563C1\" w:themeColor=\"hyperlink\"/><w:u w:val=\"single\"/></w:rPr></w:style>")
            .build(),
        ),
        (
            "p6-src-hdr-even".into(),
            m().hdrftr(true, 1, "rId20", "even", &header_p("Even letterhead", ""))
                .settings("<w:evenAndOddHeaders/>", "<w:characterSpacingControl")
                .build(),
        ),
        (
            "p6-src-hdr-even-inherited".into(),
            m().hdrftr(true, 1, "rId20", "even", &header_p("Inherited even", ""))
                .settings("<w:evenAndOddHeaders/>", "<w:characterSpacingControl")
                .edit("word/document.xml", |doc| {
                    // the referencing sectPr becomes the first section's; the
                    // new final section has no references (add_section)
                    let sect = doc.rfind("<w:sectPr").expect("sectPr");
                    let end = doc.rfind("</w:sectPr>").expect("end") + "</w:sectPr>".len();
                    let first = doc[sect..end].to_string();
                    let last = first.replace("<w:headerReference w:type=\"even\" r:id=\"rId20\"/>", "");
                    format!("{}<w:p><w:pPr>{first}</w:pPr></w:p>{last}{}", &doc[..sect], &doc[end..])
                })
                .build(),
        ),
        (
            "p6-src-hdr-even-unreferenced".into(),
            m().settings("<w:evenAndOddHeaders/>", "<w:characterSpacingControl")
                .body_append("<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr></w:pPr></w:p>")
                .build(),
        ),
        (
            "p6-src-hdr-style".into(),
            m().hdrftr(true, 1, "rId20", "default", &p_style("LetterheadBrand", "Source letterhead"))
                .style(&paragraph_style("LetterheadBrand", "LetterheadBrand", "<w:rPr><w:i/></w:rPr>"))
                .build(),
        ),
        (
            "p6-src-hdr-numbered".into(),
            with_decimal(m(), "")
                .hdrftr(true, 1, "rId20", "default", &numbered("Header item", "10"))
                .build(),
        ),
        (
            "p6-src-hdr-body-numbered".into(),
            with_decimal(m().body_append(&numbered("Body item", "10")), "")
                .hdrftr(true, 1, "rId20", "default", &numbered("Header item", "10"))
                .build(),
        ),
        (
            "p6-src-hdr-footnote".into(),
            m().hdrftr(
                true,
                1,
                "rId20",
                "default",
                &header_p("Source letterhead", "<w:r><w:footnoteReference w:id=\"1\"/></w:r>"),
            )
            .build(),
        ),
        (
            "p6-src-hdr-bookmark".into(),
            m().hdrftr(
                true,
                1,
                "rId20",
                "default",
                &format!(
                    "<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr>{}</w:p>",
                    bookmark("0", "SharedMark", &run("HeaderMark"))
                ),
            )
            .build(),
        ),
        (
            "p6-src-hdr-houseterm".into(),
            m().body_append(&p_style("HouseTerm", "Body branded"))
                .style(&paragraph_style("HouseTerm", "HouseTerm", "<w:rPr><w:i/></w:rPr>"))
                .hdrftr(true, 1, "rId20", "default", &p_style("HouseTerm", "Header branded"))
                .build(),
        ),
        (
            "p6-src-hdr-refs".into(),
            m().body_append(&format!("<w:p>{}</w:p>", bookmark("0", "SharedMark", &run("Source body mark"))))
                .hdrftr(
                    true,
                    1,
                    "rId20",
                    "default",
                    &header_p(
                        "",
                        &format!(
                            "{}<w:hyperlink w:anchor=\"SharedMark\">{}</w:hyperlink>",
                            simple_field(" REF SharedMark \\h ", "see mark"),
                            run("jump")
                        ),
                    ),
                )
                .build(),
        ),
        (
            "p6-src-hdr-firstpage".into(),
            m().hdrftr(true, 1, "rId20", "default", &header_p("Source running", ""))
                .hdrftr(true, 2, "rId21", "first", &header_p("Source first page", ""))
                .replace("word/document.xml", "<w:docGrid", "<w:titlePg/><w:docGrid")
                .build(),
        ),
        (
            "p6-src-hdr-field".into(),
            m().hdrftr(true, 1, "rId20", "default", &header_p("", &simple_field(" PAGE ", "1"))).build(),
        ),
        (
            "p6-src-hdr-sdt".into(),
            m().hdrftr(true, 1, "rId20", "default", &control(Some(1), "Header control", false)).build(),
        ),
        (
            "p6-src-hdr-top-bookmark".into(),
            m().hdrftr(true, 1, "rId20", "default", &format!("{}<w:bookmarkStart w:id=\"0\" w:name=\"Loose\"/>", header_p("Loose", "")))
                .build(),
        ),
        (
            "p6-src-hdr-malformed-id".into(),
            m().hdrftr(true, 1, "rId20", "default", &format!("<w:p>{}</w:p>", bookmark("x", "Bad", &run("bad id"))))
                .build(),
        ),
        (
            "p6-src-no-sect".into(),
            Pkg::blank()
                .edit("word/document.xml", |doc| {
                    let sect = doc.rfind("<w:sectPr").expect("sectPr");
                    let end = doc.rfind("</w:sectPr>").expect("end") + "</w:sectPr>".len();
                    format!("{}{}{}", &doc[..sect], p("No section"), &doc[end..])
                })
                .build(),
        ),
    ]
}

/// `(name, bytes)` documents usable as `synthetic:<name>` fixtures.
pub fn documents() -> Vec<(String, Vec<u8>)> {
    let mut out = destinations();
    out.extend(sources());
    out
}
