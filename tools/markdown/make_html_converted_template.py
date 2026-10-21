#!/usr/bin/env python3
"""Write tests/fixtures/markdown/html-converted-template.docx.

A synthetic export style shaped like a finished advice document that another
tool converted from HTML (the shape behind an integrator's template-comparison
feedback): a thin style shell (Heading 1-3 and Normal (Web) in Times New
Roman, no Body Text or TOC Heading), the visible look in direct formatting
(Arial body runs, Heading 1 black Arial regular, Heading 2 orange Arial
12 pt), lists made of Normal (Web) paragraphs with numbering, data tables
with a cyan header row and thin grey row lines, one orange bold label cell,
a layout table, sample pictures, a comment and custom XML in the sample
body, two sections and a footer with a logo, "page | total" and a
DOCPROPERTY field. Only TOC 1 is defined (Arial 10). The document
properties carry what a practice's finished document would: people, a
title, statistics and SharePoint metadata.

Every name and number is invented. Standard library only:

    python3 tools/markdown/make_html_converted_template.py
"""

import struct
import zipfile
import zlib
from pathlib import Path

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
RT = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
OUT = Path(__file__).resolve().parents[2] / "tests/fixtures/markdown/html-converted-template.docx"


def png(width, height, rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


ARIAL = '<w:rFonts w:ascii="Arial" w:eastAsia="Times New Roman" w:hAnsi="Arial" w:cs="Arial"/>'


def run(text, rpr=""):
    return f'<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>'


def para(text, style="NormalWeb", rpr=ARIAL + '<w:sz w:val="20"/><w:szCs w:val="20"/>', extra=""):
    pstyle = "" if style == "Normal" else f'<w:pStyle w:val="{style}"/>'
    return f'<w:p><w:pPr>{pstyle}{extra}</w:pPr>{run(text, rpr)}</w:p>'


def heading1(text):
    return para(text, "Heading1", ARIAL + '<w:b w:val="0"/><w:bCs w:val="0"/><w:color w:val="000000"/>')


def heading2(text):
    return para(text, "Heading2", ARIAL + '<w:color w:val="FAA436"/><w:sz w:val="24"/><w:szCs w:val="24"/>')


def bullet(text):
    return para(text, extra='<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>')


def picture(r_id, n):
    emu = 952500
    return (
        f'<w:p><w:pPr><w:pStyle w:val="NormalWeb"/></w:pPr><w:r><w:drawing>'
        f'<wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">'
        f'<wp:extent cx="{emu}" cy="{emu}"/><wp:docPr id="{n}" name="Picture {n}"/>'
        f'<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        f'<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:nvPicPr><pic:cNvPr id="0" name="sample{n}.png"/><pic:cNvPicPr/></pic:nvPicPr>'
        f'<pic:blipFill><a:blip r:embed="{r_id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{emu}" cy="{emu}"/></a:xfrm>'
        f'<a:prstGeom prst="rect"/></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline>'
        f"</w:drawing></w:r></w:p>"
    )


def cell(text, tcpr, rpr):
    return f"<w:tc><w:tcPr>{tcpr}</w:tcPr><w:p><w:pPr><w:pStyle w:val=\"NormalWeb\"/></w:pPr>{run(text, rpr)}</w:p></w:tc>"


HEADER_TCPR = '<w:tcBorders><w:bottom w:val="single" w:sz="6" w:space="0" w:color="40C7F2"/></w:tcBorders><w:shd w:val="clear" w:color="auto" w:fill="40C7F2"/><w:tcMar><w:top w:w="100" w:type="dxa"/><w:left w:w="100" w:type="dxa"/><w:bottom w:w="100" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tcMar>'
HEADER_RPR = ARIAL + '<w:b/><w:bCs/><w:color w:val="FFFFFF"/><w:sz w:val="20"/><w:szCs w:val="20"/>'
BODY_TCPR = '<w:tcBorders><w:bottom w:val="single" w:sz="6" w:space="0" w:color="D0D0D0"/></w:tcBorders><w:shd w:val="clear" w:color="auto" w:fill="FFFFFF"/><w:tcMar><w:top w:w="100" w:type="dxa"/><w:left w:w="100" w:type="dxa"/><w:bottom w:w="100" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tcMar>'
BODY_RPR = ARIAL + '<w:color w:val="000000"/><w:sz w:val="20"/><w:szCs w:val="20"/>'
LABEL_RPR = ARIAL + '<w:b/><w:bCs/><w:color w:val="FAA436"/><w:sz w:val="20"/><w:szCs w:val="20"/>'


def data_table(rows, label_first=False):
    xml = '<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid><w:gridCol w:w="4500"/><w:gridCol w:w="4500"/></w:tblGrid>'
    xml += "<w:tr>" + cell("Item", HEADER_TCPR, HEADER_RPR) + cell("Details", HEADER_TCPR, HEADER_RPR) + "</w:tr>"
    for index, (left, right) in enumerate(rows):
        first = LABEL_RPR if label_first and index == 0 else BODY_RPR
        xml += "<w:tr>" + cell(left, BODY_TCPR, first) + cell(right, BODY_TCPR, BODY_RPR) + "</w:tr>"
    return xml + "</w:tbl>"


LAYOUT_TABLE = (
    '<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:tblGrid><w:gridCol w:w="9000"/></w:tblGrid>'
    "<w:tr>" + cell("Prepared for Example Client", "", ARIAL + '<w:b/><w:sz w:val="36"/>') + "</w:tr></w:tbl>"
)

SECTION_1 = (
    '<w:p><w:pPr><w:sectPr><w:pgSz w:w="11906" w:h="16838"/>'
    '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/>'
    "</w:sectPr></w:pPr></w:p>"
)

BODY = "".join(
    [
        LAYOUT_TABLE,
        heading1("Your situation"),
        para("This sample advice describes an invented client."),
        # Most sample text sits in unstyled paragraphs with Arial runs.
        para("Unstyled sample text one.", "Normal"),
        para("Unstyled sample text two.", "Normal"),
        para("Unstyled sample text three.", "Normal"),
        heading2("Personal details"),
        data_table([("Priority:", "Keep a cash reserve"), ("Residence", "Example City"), ("Lifestyle", "Sailing")], label_first=True),
        para("Text right after a table."),
        picture("rId20", 1),
        SECTION_1,
        heading1("Recommendations"),
        heading2("Superannuation"),
        para("Consolidate the example accounts."),
        '<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> DOCPROPERTY ClientRef </w:instrText></w:r>'
        '<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>EX-1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>',
        bullet("First sample point"),
        bullet("Second sample point"),
        bullet("Third sample point"),
        data_table([("Fund", "Example Super"), ("Balance", "$100,000"), ("Fees", "$500")]),
        heading2("Insurance"),
        '<w:p><w:pPr><w:pStyle w:val="NormalWeb"/></w:pPr><w:commentRangeStart w:id="0"/>'
        + run("Reviewed sample text.", ARIAL)
        + '<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r></w:p>',
        picture("rId21", 2),
        picture("rId22", 3),
        '<w:sectPr><w:footerReference w:type="default" r:id="rId30"/><w:pgSz w:w="11906" w:h="16838"/>'
        '<w:pgMar w:top="998" w:right="998" w:bottom="998" w:left="998" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>',
    ]
)

DOCUMENT = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{BODY}</w:body></w:document>'

STYLES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:cstheme="minorBidi"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:rPrDefault><w:pPrDefault/></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:before="100" w:beforeAutospacing="1" w:after="100" w:afterAutospacing="1"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:kern w:val="36"/><w:sz w:val="48"/><w:szCs w:val="48"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:before="100" w:beforeAutospacing="1" w:after="100" w:afterAutospacing="1"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="36"/><w:szCs w:val="36"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="27"/><w:szCs w:val="27"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="NormalWeb"><w:name w:val="Normal (Web)"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="100" w:beforeAutospacing="1" w:after="100" w:afterAutospacing="1"/></w:pPr><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="TOC1"><w:name w:val="toc 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="100"/></w:pPr><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/><w:basedOn w:val="Normal"/></w:style>
<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/></w:style>
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
</w:styles>"""

NUMBERING = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val=""/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol" w:hint="default"/></w:rPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"""

FOOTER = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr xmlns:w="{W}" xmlns:r="{R}"><w:p><w:pPr><w:pStyle w:val="Footer"/></w:pPr>{run("Advice prepared for an example client ", ARIAL)}<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>{run(" | ")}<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> NUMPAGES </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>{run(" ")}<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> DOCPROPERTY </w:instrText></w:r><w:r><w:instrText xml:space="preserve">"PracticeName" \\* MERGEFORMAT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Example Practice</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>{picture("rId1", 9).replace('<w:pStyle w:val="NormalWeb"/>', '<w:pStyle w:val="Footer"/>')}</w:ftr>"""

COMMENTS = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="{W}"><w:comment w:id="0" w:author="Example Reviewer" w:initials="ER"><w:p>{run("Sample review note.")}</w:p></w:comment></w:comments>"""

SETTINGS = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="{W}"><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"""

THEME = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Sample"><a:themeElements><a:clrScheme name="Sample"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Sample"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Sample"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"""

CUSTOM_ITEM = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<sampleProperties xmlns="urn:example:sample"><clientName>Example Client</clientName></sampleProperties>'
CUSTOM_PROPS = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<ds:datastoreItem xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml" ds:itemID="{11111111-2222-3333-4444-555555555555}"><ds:schemaRefs/></ds:datastoreItem>'


CORE = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Sample advice for Example Client</dc:title><dc:subject>Example subject</dc:subject><dc:creator>Example Adviser</dc:creator><cp:keywords>example keywords</cp:keywords><dc:description>Example description</dc:description><cp:lastModifiedBy>Example Editor</cp:lastModifiedBy><cp:revision>42</cp:revision><cp:lastPrinted>2020-01-02T03:04:05Z</cp:lastPrinted><dcterms:created xsi:type="dcterms:W3CDTF">2020-01-01T00:00:00Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2020-01-03T00:00:00Z</dcterms:modified><cp:category>Example category</cp:category><cp:contentStatus>Final</cp:contentStatus><dc:language>en-AU</dc:language></cp:coreProperties>"""

APP = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Template>Normal.dotm</Template><TotalTime>321</TotalTime><Pages>74</Pages><Words>17194</Words><Characters>98000</Characters><Application>Microsoft Office Word</Application><DocSecurity>0</DocSecurity><Lines>800</Lines><Paragraphs>230</Paragraphs><ScaleCrop>false</ScaleCrop><HeadingPairs><vt:vector size="2" baseType="variant"><vt:variant><vt:lpstr>Title</vt:lpstr></vt:variant><vt:variant><vt:i4>1</vt:i4></vt:variant></vt:vector></HeadingPairs><TitlesOfParts><vt:vector size="1" baseType="lpstr"><vt:lpstr>Sample advice for Example Client</vt:lpstr></vt:vector></TitlesOfParts><Manager>Example Manager</Manager><Company>Example Practice</Company><LinksUpToDate>false</LinksUpToDate><CharactersWithSpaces>115000</CharactersWithSpaces><SharedDoc>false</SharedDoc><HyperlinksChanged>false</HyperlinksChanged><AppVersion>16.0000</AppVersion></Properties>"""

CUSTOM_DOC_PROPS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="ContentTypeId"><vt:lpwstr>0x0101000000000000000000000000000000000000</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="3" name="PracticeName"><vt:lpwstr>Example Practice</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="4" name="ClientRef"><vt:lpwstr>EX-1</vt:lpwstr></property></Properties>"""


def rels(entries):
    body = "".join(
        f'<Relationship Id="{rid}" Type="{kind if kind.startswith("http") else RT + "/" + kind}" Target="{target}"/>'
        for rid, kind, target in entries
    )
    return f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>'


CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/><Override PartName="/customXml/itemProps1.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/><Override PartName="/docProps/custom.xml" ContentType="application/vnd.openxmlformats-officedocument.custom-properties+xml"/></Types>"""


def main():
    members = {
        "[Content_Types].xml": CONTENT_TYPES,
        "_rels/.rels": rels(
            [
                ("rId1", "officeDocument", "word/document.xml"),
                ("rId2", "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties", "docProps/core.xml"),
                ("rId3", "extended-properties", "docProps/app.xml"),
                ("rId4", "custom-properties", "docProps/custom.xml"),
            ]
        ),
        "docProps/core.xml": CORE,
        "docProps/app.xml": APP,
        "docProps/custom.xml": CUSTOM_DOC_PROPS,
        "word/document.xml": DOCUMENT,
        "word/_rels/document.xml.rels": rels(
            [
                ("rId1", "styles", "styles.xml"),
                ("rId2", "numbering", "numbering.xml"),
                ("rId3", "settings", "settings.xml"),
                ("rId4", "theme", "theme/theme1.xml"),
                ("rId5", "comments", "comments.xml"),
                ("rId6", "customXml", "../customXml/item1.xml"),
                ("rId20", "image", "media/image1.png"),
                ("rId21", "image", "media/image2.png"),
                ("rId22", "image", "media/image3.png"),
                ("rId30", "footer", "footer1.xml"),
            ]
        ),
        "word/styles.xml": STYLES,
        "word/numbering.xml": NUMBERING,
        "word/settings.xml": SETTINGS,
        "word/theme/theme1.xml": THEME,
        "word/footer1.xml": FOOTER,
        "word/_rels/footer1.xml.rels": rels([("rId1", "image", "media/logo.png")]),
        "word/comments.xml": COMMENTS,
        "customXml/item1.xml": CUSTOM_ITEM,
        "customXml/itemProps1.xml": CUSTOM_PROPS,
        "customXml/_rels/item1.xml.rels": rels([("rId1", "customXmlProps", "itemProps1.xml")]),
    }
    images = {
        "word/media/image1.png": png(4, 4, (200, 30, 30)),
        "word/media/image2.png": png(4, 4, (30, 200, 30)),
        "word/media/image3.png": png(4, 4, (30, 30, 200)),
        "word/media/logo.png": png(4, 4, (64, 199, 242)),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, text in members.items():
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            archive.writestr(info, text.encode("utf-8"), zipfile.ZIP_DEFLATED)
        for name, data in images.items():
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            archive.writestr(info, data, zipfile.ZIP_DEFLATED)
    print(OUT)


if __name__ == "__main__":
    main()
