//! Run appearance in the layout model, read through `layout::line_records`:
//! every property through the style chain and direct formatting. The
//! fixtures `tests/fixtures/pdf/appearance.docx` and `superscript.docx` are
//! `fixture()` and `script_fixture()`, written with `DOCRS_UPDATE_SNAPSHOTS=1`.

mod support;

use docrs::layout::{Appearance, Caps, Colour, FontSet, Link, Record, Rgb, Strike, records_with};
use docrs::txn::Txn;
use docrs::{Document, Limits};
use support::*;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const URI: &str = "https://example.com/docrs";

const UNDERLINES: [&str; 17] = [
    "single",
    "words",
    "double",
    "thick",
    "dotted",
    "dottedHeavy",
    "dash",
    "dashedHeavy",
    "dashLong",
    "dashLongHeavy",
    "dotDash",
    "dashDotHeavy",
    "dotDotDash",
    "dashDotDotHeavy",
    "wave",
    "wavyHeavy",
    "wavyDouble",
];

const HIGHLIGHTS: [(&str, &str); 16] = [
    ("black", "000000"),
    ("blue", "0000FF"),
    ("cyan", "00FFFF"),
    ("green", "00FF00"),
    ("magenta", "FF00FF"),
    ("red", "FF0000"),
    ("yellow", "FFFF00"),
    ("white", "FFFFFF"),
    ("darkBlue", "000080"),
    ("darkCyan", "008080"),
    ("darkGreen", "008000"),
    ("darkMagenta", "800080"),
    ("darkRed", "800000"),
    ("darkYellow", "808000"),
    ("darkGray", "808080"),
    ("lightGray", "C0C0C0"),
];

/// The Office theme's colours (Word 2013–2022).
fn theme() -> String {
    let srgb = |slot: &str, hex: &str| format!("<a:{slot}><a:srgbClr val=\"{hex}\"/></a:{slot}>");
    let scheme = [
        "<a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1>".to_string(),
        "<a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>".to_string(),
        srgb("dk2", "44546A"),
        srgb("lt2", "E7E6E6"),
        srgb("accent1", "4472C4"),
        srgb("accent2", "ED7D31"),
        srgb("accent3", "A5A5A5"),
        srgb("accent4", "FFC000"),
        srgb("accent5", "5B9BD5"),
        srgb("accent6", "70AD47"),
        srgb("hlink", "0563C1"),
        srgb("folHlink", "954F72"),
    ]
    .concat();
    let font = "<a:latin typeface=\"Arial\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/>";
    let format = format_scheme();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><a:theme xmlns:a=\"{A}\" name=\"Office Theme\"><a:themeElements><a:clrScheme name=\"Office\">{scheme}</a:clrScheme><a:fontScheme name=\"Office\"><a:majorFont>{font}</a:majorFont><a:minorFont>{font}</a:minorFont></a:fontScheme>{format}</a:themeElements></a:theme>"
    )
}

/// Word refuses a theme whose four style lists have fewer than three entries.
fn format_scheme() -> String {
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let fills = fill.repeat(3);
    let lines = ["9525", "25400", "38100"]
        .map(|w| format!("<a:ln w=\"{w}\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\">{fill}<a:prstDash val=\"solid\"/></a:ln>"))
        .concat();
    let effects = "<a:effectStyle><a:effectLst/></a:effectStyle>".repeat(3);
    format!(
        "<a:fmtScheme name=\"Office\"><a:fillStyleLst>{fills}</a:fillStyleLst><a:lnStyleLst>{lines}</a:lnStyleLst><a:effectStyleLst>{effects}</a:effectStyleLst><a:bgFillStyleLst>{fills}</a:bgFillStyleLst></a:fmtScheme>"
    )
}

fn package(body: &str, styles: &str) -> Vec<u8> {
    package_in("Arial", body, styles)
}

fn package_in(font: &str, body: &str, styles: &str) -> Vec<u8> {
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\" xmlns:r=\"{R}\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"{W}\"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:cs=\"{font}\"/><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>{styles}</w:styles>"
    );
    let ct = CT_XML.replace(
        "</Types>",
        "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/word/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/></Types>",
    );
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{R}/styles\" Target=\"styles.xml\"/><Relationship Id=\"rId2\" Type=\"{R}/theme\" Target=\"theme/theme1.xml\"/><Relationship Id=\"rId3\" Type=\"{R}/hyperlink\" Target=\"{URI}\" TargetMode=\"External\"/></Relationships>"
    );
    let theme = theme();
    build_zip(&[
        RawMember::new("[Content_Types].xml", ct.as_bytes()),
        RawMember::new("_rels/.rels", ROOT_RELS.as_bytes()),
        RawMember::new("word/document.xml", document.as_bytes()),
        RawMember::new("word/_rels/document.xml.rels", rels.as_bytes()),
        RawMember::new("word/styles.xml", styles.as_bytes()),
        RawMember::new("word/theme/theme1.xml", theme.as_bytes()),
    ])
}

fn run(rpr: &str, text: &str) -> String {
    format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

fn para(ppr: &str, runs: &str) -> String {
    format!("<w:p><w:pPr>{ppr}</w:pPr>{runs}</w:p>")
}

fn style(kind: &str, id: &str, based_on: &str, rpr: &str) -> String {
    let based_on = if based_on.is_empty() {
        String::new()
    } else {
        format!("<w:basedOn w:val=\"{based_on}\"/>")
    };
    format!(
        "<w:style w:type=\"{kind}\" w:styleId=\"{id}\"><w:name w:val=\"{id}\"/>{based_on}<w:rPr>{rpr}</w:rPr></w:style>"
    )
}

fn pstyle(id: &str) -> String {
    format!("<w:pStyle w:val=\"{id}\"/>")
}

fn rstyle(id: &str) -> String {
    format!("<w:rStyle w:val=\"{id}\"/>")
}

fn styles() -> String {
    [
        style("paragraph", "BoldPara", "Normal", "<w:b/>"),
        style("paragraph", "ItalicPara", "Normal", "<w:i/>"),
        style("paragraph", "StrikePara", "Normal", "<w:strike/>"),
        style(
            "paragraph",
            "StrikeOff",
            "StrikePara",
            "<w:strike w:val=\"0\"/>",
        ),
        style("character", "ItalicChar", "", "<w:i/>"),
        style(
            "character",
            "AccentChar",
            "",
            "<w:color w:val=\"ED7D31\" w:themeColor=\"accent2\"/><w:u w:val=\"double\"/>",
        ),
        style(
            "character",
            "Hyperlink",
            "",
            "<w:color w:val=\"0563C1\" w:themeColor=\"hyperlink\"/><w:u w:val=\"single\"/>",
        ),
    ]
    .concat()
}

fn field(code: &str, result: &str) -> String {
    let link = rstyle("Hyperlink");
    format!(
        "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\">{code}</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>{}<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>",
        run(&link, result)
    )
}

/// A paragraph for each property, each run named for what it shows.
fn body() -> String {
    let colour = |attrs: &str| format!("<w:color {attrs}/>");
    let link = rstyle("Hyperlink");
    let underlines: String = UNDERLINES
        .iter()
        .map(|u| run(&format!("<w:u w:val=\"{u}\"/>"), &format!("{u} ")))
        .collect();
    let highlights: String = HIGHLIGHTS
        .iter()
        .map(|(name, _)| {
            run(
                &format!("<w:highlight w:val=\"{name}\"/>"),
                &format!("{name} "),
            )
        })
        .collect();
    [
        para(
            "",
            &[
                run(&colour("w:val=\"FF0000\""), "rgb-red "),
                run(&colour("w:val=\"4472C4\" w:themeColor=\"accent1\""), "accent1 "),
                run(
                    &colour("w:val=\"8EAADB\" w:themeColor=\"accent1\" w:themeTint=\"99\""),
                    "accent1-lighter-40 ",
                ),
                run(
                    &colour("w:val=\"2F5496\" w:themeColor=\"accent1\" w:themeShade=\"BF\""),
                    "accent1-darker-25 ",
                ),
                run(&colour("w:val=\"000000\" w:themeColor=\"text1\""), "text1 "),
                run(&colour("w:val=\"auto\""), "auto "),
                run(&rstyle("AccentChar"), "style-accent2"),
            ]
            .concat(),
        ),
        para(
            &pstyle("BoldPara"),
            &[
                run("", "style-bold "),
                run("<w:b w:val=\"0\"/>", "direct-bold-off"),
            ]
            .concat(),
        ),
        para(
            &pstyle("ItalicPara"),
            &[
                run("", "style-italic "),
                run(&rstyle("ItalicChar"), "italic-xor"),
            ]
            .concat(),
        ),
        para(
            &pstyle("StrikeOff"),
            &[
                run("", "strike-off "),
                run("<w:strike/>", "strike "),
                run("<w:dstrike/>", "dstrike"),
            ]
            .concat(),
        ),
        para("", &underlines),
        para(
            "",
            &[
                run("<w:u w:val=\"single\" w:color=\"FF0000\"/>", "red-underline "),
                run(
                    "<w:u w:val=\"wave\" w:color=\"ED7D31\" w:themeColor=\"accent2\"/>",
                    "accent2-wave ",
                ),
                run(
                    &format!("{}<w:u w:val=\"none\"/>", rstyle("AccentChar")),
                    "underline-none",
                ),
            ]
            .concat(),
        ),
        para("", &highlights),
        para(
            "",
            &[
                run("<w:caps/>", "caps "),
                run("<w:smallCaps/>", "small-caps "),
                run("<w:spacing w:val=\"40\"/>", "spaced "),
                run("<w:w w:val=\"150\"/>", "scaled"),
            ]
            .concat(),
        ),
        para(
            "",
            &[
                run("", "E=mc"),
                run("<w:vertAlign w:val=\"superscript\"/>", "2"),
                run("", " H"),
                run("<w:vertAlign w:val=\"subscript\"/>", "2"),
                run("", "O "),
                run("<w:position w:val=\"6\"/>", "raised "),
                run("<w:position w:val=\"-6\"/>", "lowered"),
            ]
            .concat(),
        ),
        para(
            "",
            &[
                run("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FFFF00\"/>", "fill "),
                run("<w:shd w:val=\"pct50\" w:color=\"000000\" w:fill=\"FFFFFF\"/>", "pct50 "),
                ["pct10", "pct62", "pct75", "pct87"]
                    .map(|pct| run(&format!("<w:shd w:val=\"{pct}\" w:color=\"000000\" w:fill=\"FFFFFF\"/>"), &format!("{pct} ")))
                    .concat(),
                run(
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"4472C4\" w:themeFill=\"accent1\"/>",
                    "theme-fill",
                ),
            ]
            .concat(),
        ),
        para(
            "",
            &[
                format!("<w:hyperlink r:id=\"rId3\">{}</w:hyperlink>", run(&link, "uri ")),
                format!(
                    "<w:hyperlink w:anchor=\"target\">{}</w:hyperlink>",
                    run(&link, "anchor ")
                ),
                format!(
                    "<w:hyperlink r:id=\"rId3\" w:anchor=\"top\">{}</w:hyperlink>",
                    run(&link, "uri-anchor ")
                ),
                field(&format!(" HYPERLINK \"{URI}\" \\l \"part\" "), "field-uri "),
                field(" HYPERLINK \\l \"target\" ", "field-anchor "),
                format!(
                    "<w:fldSimple w:instr=\" HYPERLINK &quot;{URI}&quot; \">{}</w:fldSimple>",
                    run(&link, "simple ")
                ),
                run("", "plain"),
            ]
            .concat(),
        ),
        "<w:p><w:bookmarkStart w:id=\"0\" w:name=\"target\"/><w:r><w:t>target</w:t></w:r><w:bookmarkEnd w:id=\"0\"/></w:p>".to_string(),
    ]
    .concat()
}

fn fixture() -> Vec<u8> {
    package(&body(), &styles())
}

/// A line naming what its middle run shows: `label: ` and ` end` are plain.
fn sample(label: &str, rpr: &str, text: &str) -> String {
    para(
        "",
        &[
            run("", &format!("{label}: ")),
            run(rpr, text),
            run("", " end"),
        ]
        .concat(),
    )
}

/// The decoration sheet, in Inter: every underline, strikethrough,
/// highlight and shading, theme colours, and `auto` on dark fills.
fn decoration_body() -> String {
    let fox = "quick brown fox";
    let mut out: Vec<String> = UNDERLINES
        .iter()
        .map(|u| sample(u, &format!("<w:u w:val=\"{u}\"/>"), fox))
        .collect();
    out.extend([
        sample(
            "red-underline",
            "<w:color w:val=\"1F4E79\"/><w:u w:val=\"single\" w:color=\"FF0000\"/>",
            fox,
        ),
        sample(
            "accent2-wave",
            "<w:u w:val=\"wave\" w:color=\"ED7D31\" w:themeColor=\"accent2\"/>",
            fox,
        ),
        para(
            "",
            &[
                run("", "sizes: "),
                run("<w:u w:val=\"single\"/>", "small "),
                run("<w:u w:val=\"single\"/><w:sz w:val=\"44\"/>", "BIG"),
                run("<w:u w:val=\"single\"/>", " small"),
                run("", " end"),
            ]
            .concat(),
        ),
        para(
            "",
            &[
                run("", "script: "),
                run("<w:u w:val=\"single\"/>", "E=mc"),
                run(
                    "<w:u w:val=\"single\"/><w:vertAlign w:val=\"superscript\"/>",
                    "2",
                ),
                run("", " end"),
            ]
            .concat(),
        ),
        sample("strike", "<w:strike/>", fox),
        sample("dstrike", "<w:dstrike/>", fox),
        sample("raised-strike", "<w:strike/><w:position w:val=\"6\"/>", fox),
        sample("red-strike", "<w:strike/><w:color w:val=\"C00000\"/>", fox),
    ]);
    for half in HIGHLIGHTS.chunks(8) {
        let runs: String = half
            .iter()
            .map(|(name, _)| run(&format!("<w:highlight w:val=\"{name}\"/>"), name) + &run("", " "))
            .collect();
        out.push(para("", &runs));
    }
    let shd = |val: &str, extra: &str| format!("<w:shd w:val=\"{val}\" w:color=\"auto\" {extra}/>");
    let shades: String = ["pct10", "pct25", "pct50", "pct62", "pct75", "pct87"]
        .map(|pct| run(&shd(pct, "w:fill=\"FFFFFF\""), pct) + &run("", " "))
        .concat();
    out.extend([
        para("", &shades),
        para(
            "",
            &[
                run(&shd("clear", "w:fill=\"FFFF00\""), "fill"),
                run("", " "),
                run(
                    &shd("clear", "w:fill=\"4472C4\" w:themeFill=\"accent1\""),
                    "accent1-fill",
                ),
                run("", " "),
                run(
                    &shd(
                        "clear",
                        "w:fill=\"B4C6E7\" w:themeFill=\"accent1\" w:themeFillTint=\"66\"",
                    ),
                    "accent1-fill-lighter-60",
                ),
                run("", " "),
                run(
                    &format!(
                        "<w:highlight w:val=\"yellow\"/>{}",
                        shd("clear", "w:fill=\"00B0F0\"")
                    ),
                    "highlight-over-fill",
                ),
            ]
            .concat(),
        ),
    ]);
    let theme = |name: &str, extra: &str, label: &str| {
        run(
            &format!("<w:color w:val=\"000000\" w:themeColor=\"{name}\" {extra}/>"),
            label,
        ) + &run("", " ")
    };
    out.push(para(
        "",
        &[
            "accent1",
            "accent2",
            "accent3",
            "accent4",
            "accent5",
            "accent6",
            "text2",
            "hyperlink",
        ]
        .map(|name| theme(name, "", name))
        .concat(),
    ));
    out.push(para(
        "",
        &[
            theme("accent1", "w:themeTint=\"33\"", "lighter-80"),
            theme("accent1", "w:themeTint=\"66\"", "lighter-60"),
            theme("accent1", "w:themeTint=\"99\"", "lighter-40"),
            theme("accent1", "w:themeShade=\"BF\"", "darker-25"),
            theme("accent1", "w:themeShade=\"80\"", "darker-50"),
        ]
        .concat(),
    ));
    let auto = |extra: &str, label: &str| {
        run(&format!("<w:color w:val=\"auto\"/>{extra}"), label) + &run("", " ")
    };
    out.push(para(
        "",
        &[
            auto(&shd("clear", "w:fill=\"1F3864\""), "auto-navy"),
            auto(&shd("clear", "w:fill=\"7F7F7F\""), "auto-7F7F7F"),
            auto(&shd("clear", "w:fill=\"808080\""), "auto-808080"),
            auto("<w:highlight w:val=\"darkBlue\"/>", "auto-darkBlue"),
            auto(
                &format!(
                    "<w:u w:val=\"single\"/>{}",
                    shd("clear", "w:fill=\"000000\"")
                ),
                "auto-underline-black",
            ),
        ]
        .concat(),
    ));
    out.push(para(
        "",
        &[
            run("", "shown "),
            run("<w:vanish/>", "hidden "),
            run("", "shown"),
        ]
        .concat(),
    ));
    out.concat()
}

fn decoration_fixture() -> Vec<u8> {
    package_in("Inter", &decoration_body(), "")
}

/// Fill-in lines, in Inter: an underlined and a struck tab to a stop at
/// 300 pt, and underlined text ending in no-break spaces, in spaces, or
/// in spaces before a line break.
fn fill_in_fixture() -> Vec<u8> {
    let stop = "<w:tabs><w:tab w:val=\"left\" w:pos=\"6000\"/></w:tabs>";
    let tab = |rpr: &str| format!("<w:r><w:rPr>{rpr}</w:rPr><w:tab/></w:r>");
    let body = [
        para(stop, &(run("", "Name: ") + &tab("<w:u w:val=\"single\"/>"))),
        para(stop, &(run("", "Struck: ") + &tab("<w:strike/>"))),
        para(
            "",
            &(run("", "Date: ") + &run("<w:u w:val=\"single\"/>", "text\u{a0}\u{a0}\u{a0}\u{a0}")),
        ),
        para(
            "",
            &(run("", "Spaces: ") + &run("<w:u w:val=\"single\"/>", "text    ")),
        ),
        para(
            "",
            &(run("", "Break: ")
                + &run("<w:u w:val=\"single\"/>", "text    ")
                + "<w:r><w:rPr><w:u w:val=\"single\"/></w:rPr><w:br/></w:r>"
                + &run("", "after")),
        ),
    ]
    .concat();
    package_in("Inter", &body, "")
}

const SCRIPT_FONTS: [&str; 4] = ["Arial", "Times New Roman", "Georgia", "Courier New"];
const SCRIPT_SIZES: [u32; 6] = [8, 10, 11, 12, 24, 72];
/// The marker paragraph's runs, with their properties.
const SCRIPT_RUNS: [(&str, &str); 8] = [
    ("", "x"),
    ("<w:vertAlign w:val=\"superscript\"/>", "sup"),
    ("", " x"),
    ("<w:vertAlign w:val=\"subscript\"/>", "sub"),
    ("", " x"),
    ("<w:position w:val=\"6\"/>", "up3"),
    ("", " x"),
    ("<w:position w:val=\"-6\"/>", "down3"),
];

/// The letters of the long runs at `size`: 60, fewer above 12 pt so that
/// each fits on one line.
fn script_letters(size: u32) -> String {
    let count = 60 * 12 / size.max(12);
    ('a'..='z').cycle().take(count as usize).collect()
}

/// `superscript.docx`: every `SCRIPT_FONTS` at every `SCRIPT_SIZES`.
fn script_fixture() -> Vec<u8> {
    let groups: Vec<_> = SCRIPT_FONTS
        .iter()
        .flat_map(|font| SCRIPT_SIZES.map(|size| (*font, size)))
        .collect();
    script_groups(&groups)
}

/// For each font and size: a heading in the default font, the
/// `SCRIPT_RUNS`, then the same letters plain, as superscript and as
/// subscript, each in a paragraph of its own.
fn script_groups(groups: &[(&str, u32)]) -> Vec<u8> {
    let group = |font: &str, size: u32| {
        let rpr = |extra: &str| {
            format!(
                "<w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:cs=\"{font}\"/><w:sz w:val=\"{}\"/>{extra}",
                size * 2
            )
        };
        let heading = para("", &run("", &format!("{font} {size} pt")));
        let markers: String = SCRIPT_RUNS
            .iter()
            .map(|(extra, text)| run(&rpr(extra), text))
            .collect();
        let letters = script_letters(size);
        let long = ["", SCRIPT_RUNS[1].0, SCRIPT_RUNS[3].0]
            .map(|extra| para("", &run(&rpr(extra), &letters)))
            .concat();
        heading + &para("", &markers) + &long
    };
    let body: String = groups
        .iter()
        .map(|(font, size)| group(font, *size))
        .collect();
    package(&body, "")
}

/// Each run's text with its look and link.
fn runs(bytes: &[u8]) -> Vec<(String, Appearance, Option<Link>)> {
    runs_with(bytes, &FontSet::default())
}

fn runs_with(bytes: &[u8], fonts: &FontSet) -> Vec<(String, Appearance, Option<Link>)> {
    let doc = Document::open_bytes(bytes, &Limits::default()).unwrap();
    let txn = Txn::new(&doc);
    let main = doc.package().main_part_name().to_string();
    let mut table = (Vec::new(), Vec::new());
    let mut out = Vec::new();
    records_with(
        &txn,
        &main,
        fonts,
        |looks, links| {
            table = (looks.to_vec(), links.to_vec());
            Ok(())
        },
        |record| {
            if let Record::Line(line) = record {
                out.extend(line.runs.into_iter().map(|run| (run.text, run.look)));
            }
            Ok(())
        },
    )
    .unwrap();
    let (looks, links) = table;
    out.into_iter()
        .map(|(text, look)| {
            let look = looks[look as usize];
            let link = look.link.map(|l| links[l as usize].clone());
            (text.trim_end().to_string(), look, link)
        })
        .collect()
}

fn look<'r>(runs: &'r [(String, Appearance, Option<Link>)], text: &str) -> &'r Appearance {
    &find(runs, text).1
}

fn find<'r>(
    runs: &'r [(String, Appearance, Option<Link>)],
    text: &str,
) -> &'r (String, Appearance, Option<Link>) {
    runs.iter().find(|(t, ..)| t == text).unwrap_or_else(|| {
        panic!(
            "no run {text:?} in {:?}",
            runs.iter().map(|r| &r.0).collect::<Vec<_>>()
        )
    })
}

fn rgb(hex: &str) -> Rgb {
    let v = u32::from_str_radix(hex, 16).unwrap();
    Rgb {
        r: (v >> 16) as u8,
        g: (v >> 8) as u8,
        b: v as u8,
    }
}

fn colour(hex: &str) -> Colour {
    Colour::Rgb(rgb(hex))
}

#[test]
fn the_committed_fixtures_are_the_ones_these_tests_build() {
    for (name, built) in [
        ("appearance.docx", fixture()),
        ("decorations.docx", decoration_fixture()),
        ("fill-in.docx", fill_in_fixture()),
    ] {
        let path = repo_root().join("tests/fixtures/pdf").join(name);
        if std::env::var_os("DOCRS_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, &built).unwrap();
        }
        assert!(
            std::fs::read(&path).unwrap() == built,
            "{name}: rerun with DOCRS_UPDATE_SNAPSHOTS=1"
        );
    }
}

/// The number of elements directly inside the first `<a:{list}>`.
fn entries(xml: &str, list: &str) -> usize {
    let open = format!("<a:{list}>");
    let start = xml.find(&open).unwrap() + open.len();
    let inner = &xml[start..xml[start..].find(&format!("</a:{list}>")).unwrap() + start];
    let mut depth = 0;
    let mut count = 0;
    for tag in inner.split('<').skip(1) {
        if tag.starts_with('/') {
            depth -= 1;
        } else {
            count += usize::from(depth == 0);
            if !tag.split('>').next().unwrap().ends_with('/') {
                depth += 1;
            }
        }
    }
    count
}

#[test]
fn the_committed_script_fixture_is_the_one_these_tests_build() {
    let path = repo_root().join("tests/fixtures/pdf/superscript.docx");
    if std::env::var_os("DOCRS_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, script_fixture()).unwrap();
    }
    assert!(
        std::fs::read(&path).unwrap() == script_fixture(),
        "rerun with DOCRS_UPDATE_SNAPSHOTS=1"
    );
}

#[test]
fn the_theme_has_three_entries_in_each_format_list() {
    let theme = part_xml(&open_bytes(&fixture()), "word/theme/theme1.xml");
    for list in [
        "fillStyleLst",
        "lnStyleLst",
        "effectStyleLst",
        "bgFillStyleLst",
    ] {
        assert_eq!(entries(&theme, list), 3, "{list}");
    }
}

#[test]
fn colours_resolve_through_the_theme_with_tint_and_shade() {
    let runs = runs(&fixture());
    assert_eq!(look(&runs, "rgb-red").colour, colour("FF0000"));
    assert_eq!(look(&runs, "accent1").colour, colour("4472C4"));
    assert_eq!(look(&runs, "accent1-lighter-40").colour, colour("8EAADB"));
    assert_eq!(look(&runs, "accent1-darker-25").colour, colour("2F5496"));
    assert_eq!(look(&runs, "text1").colour, colour("000000"));
    assert_eq!(look(&runs, "auto").colour, Colour::Auto);
    assert_eq!(look(&runs, "style-accent2").colour, colour("ED7D31"));
}

#[test]
fn toggles_xor_through_styles_and_direct_formatting_is_absolute() {
    let runs = runs(&fixture());
    assert!(look(&runs, "style-bold").bold);
    assert!(!look(&runs, "direct-bold-off").bold);
    assert!(look(&runs, "style-italic").italic);
    assert!(!look(&runs, "italic-xor").italic);
    assert_eq!(look(&runs, "strike-off").strike, Strike::None);
    assert_eq!(look(&runs, "strike").strike, Strike::Single);
    assert_eq!(look(&runs, "dstrike").strike, Strike::Double);
}

#[test]
fn every_underline_value_is_read_with_its_colour() {
    let runs = runs(&fixture());
    for name in UNDERLINES {
        let underline = look(&runs, name).underline.unwrap();
        assert_eq!(underline.style.name(), name);
        assert_eq!(underline.colour, Colour::Auto);
    }
    assert_eq!(
        look(&runs, "red-underline").underline.unwrap().colour,
        colour("FF0000")
    );
    assert_eq!(
        look(&runs, "accent2-wave").underline.unwrap().colour,
        colour("ED7D31")
    );
    assert_eq!(
        look(&runs, "style-accent2").underline.unwrap().style.name(),
        "double"
    );
    assert_eq!(look(&runs, "underline-none").underline, None);
}

#[test]
fn every_highlight_name_has_word_s_colour() {
    let runs = runs(&fixture());
    for (name, hex) in HIGHLIGHTS {
        assert_eq!(look(&runs, name).highlight, Some(rgb(hex)), "{name}");
    }
    assert_eq!(look(&runs, "rgb-red").highlight, None);
}

#[test]
fn caps_spacing_scale_and_shading_are_kept() {
    let runs = runs(&fixture());
    assert_eq!(look(&runs, "caps").caps, Caps::All);
    assert_eq!(look(&runs, "small-caps").caps, Caps::Small);
    assert_eq!(look(&runs, "spaced").spacing, 2.0);
    assert_eq!(look(&runs, "scaled").scale, 1.5);
    assert_eq!(look(&runs, "fill").shade, Some(rgb("FFFF00")));
    for (pct, want) in [
        ("pct10", "E5E5E5"),
        ("pct50", "7F7F7F"),
        ("pct62", "606060"),
        ("pct75", "404040"),
        ("pct87", "202020"),
    ] {
        assert_eq!(look(&runs, pct).shade, Some(rgb(want)), "{pct}");
    }
    assert_eq!(look(&runs, "theme-fill").shade, Some(rgb("4472C4")));
    assert_eq!(look(&runs, "rgb-red").shade, None);
}

/// Word 16.115.2's script size: 60 raised letters' width over the plain
/// ones', times the size (Inter's 11 pt from its `Tf`).
fn word_script_size(font: &str, size: u32) -> f64 {
    match (font, size) {
        ("Inter", 11) => 6.5,
        ("Inter", 24) => 14.5,
        (_, 5) => 3.0,
        (_, 8) => 5.0,
        (_, 10) => 6.5,
        (_, 11) => 7.0,
        (_, 12) => 8.0,
        (_, 15) => 9.5,
        (_, 24) => 15.5,
        (_, 25) => 16.0,
        (_, 72) => 47.0,
        _ => unreachable!("{font} {size} pt was not measured"),
    }
}

/// The runs after the heading `{font} {size} pt`, up to the next heading.
fn script_group<'r>(
    runs: &'r [(String, Appearance, Option<Link>)],
    font: &str,
    size: u32,
) -> &'r [(String, Appearance, Option<Link>)] {
    let heading = format!("{font} {size} pt");
    let at = runs.iter().position(|r| r.0 == heading).unwrap() + 1;
    let end = runs[at..]
        .iter()
        .position(|r| r.0.ends_with(" pt"))
        .map_or(runs.len(), |n| at + n);
    &runs[at..end]
}

/// The plain, superscript and subscript letter runs' sizes.
fn script_sizes(group: &[(String, Appearance, Option<Link>)], size: u32) -> Vec<f64> {
    let letters = script_letters(size);
    group
        .iter()
        .filter(|r| r.0 == letters)
        .map(|r| r.1.size)
        .collect()
}

#[test]
fn super_and_subscript_shift_by_a_share_of_the_size_in_every_font() {
    let runs = runs(&script_fixture());
    for font in SCRIPT_FONTS {
        for size in SCRIPT_SIZES {
            let group = script_group(&runs, font, size);
            let marker = |text: &str| look(group, text);
            let (heading, plain) = (format!("{font} {size} pt"), f64::from(size));
            assert_eq!(marker("sup").rise, plain * 0.41, "{heading} sup");
            assert_eq!(marker("sub").rise, -plain * 0.087, "{heading} sub");
            assert_eq!(marker("up3").rise, 3.0, "{heading} up3");
            assert_eq!(marker("down3").rise, -3.0, "{heading} down3");
            let small = word_script_size(font, size);
            assert_eq!(
                script_sizes(group, size),
                [plain, small, small],
                "{heading}"
            );
        }
    }
}

#[test]
fn a_script_is_the_font_s_os2_size_to_the_nearest_half_point() {
    let groups = [
        ("Inter", 11),
        ("Inter", 24),
        ("Arial", 5),
        ("Arial", 15),
        ("Arial", 25),
    ];
    let path = repo_root().join("tests/fixtures/fonts/inter/Inter[opsz,wght].ttf");
    let mut inter = FontSet::builder();
    inter.add(std::fs::read(path).unwrap()).unwrap();
    let runs = runs_with(&script_groups(&groups), &inter.build());
    for (font, size) in groups {
        let small = word_script_size(font, size);
        let sizes = script_sizes(script_group(&runs, font, size), size);
        assert_eq!(sizes, [f64::from(size), small, small], "{font} {size} pt");
    }
}

#[test]
fn super_and_subscript_are_word_s_script_size_and_shifted() {
    let runs = runs(&fixture());
    let plain = look(&runs, "E=mc");
    let sup = &runs[runs.iter().position(|r| r.0 == "E=mc").unwrap() + 1].1;
    let sub = &runs[runs.iter().position(|r| r.0 == " H").unwrap() + 1].1;
    assert_eq!((plain.size, plain.rise), (11.0, 0.0));
    assert_eq!((sup.size, sub.size), (7.0, 7.0));
    assert!(sup.rise > 0.0 && sub.rise < 0.0, "{sup:?} {sub:?}");
    assert_eq!(sup.metric, plain.metric, "line height is the full size's");
    assert_eq!(look(&runs, "raised").rise, 3.0);
    assert_eq!(look(&runs, "lowered").rise, -3.0);
}

#[test]
fn hyperlinks_and_hyperlink_fields_name_their_targets() {
    let runs = runs(&fixture());
    let link = |text: &str| find(&runs, text).2.clone();
    let uri = |u: &str| Some(Link::Uri(u.to_string()));
    assert_eq!(link("uri"), uri(URI));
    assert_eq!(link("anchor"), Some(Link::Anchor("target".to_string())));
    assert_eq!(link("uri-anchor"), uri(&format!("{URI}#top")));
    assert_eq!(link("field-uri"), uri(&format!("{URI}#part")));
    assert_eq!(
        link("field-anchor"),
        Some(Link::Anchor("target".to_string()))
    );
    assert_eq!(link("simple"), uri(URI));
    assert_eq!(link("plain"), None);
    assert_eq!(
        look(&runs, "uri").link,
        look(&runs, "simple").link,
        "one target, one link"
    );
    assert_eq!(look(&runs, "uri").colour, colour("0563C1"));
}
