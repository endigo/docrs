"""Write the DOCX inputs for checking table looks against Word.

    python3 tools/layout/make_table_fixtures.py

conflicts.docx: two-cell tables whose shared edge has conflicting borders,
one per rule of Word's ranking (MS-OI29500 2.1.169). spans.docx: merged
cells across columns and rows, a centred and an indented table.
seams.docx: a bordered table whose row splits across a page break, a
vertical merge whose fill continues on the next page, and centred and
bottom-aligned cells. cascade.docx: a table style with every conditional
format, a cnfStyle that claims the wrong ones, and a row exception. All are
Arial 11 on US Letter with 1-inch margins, compatibility mode 15.
"""

import pathlib
import zipfile

OUT = pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures/layout/tables"
W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def p(text, ppr="", rpr=""):
    run = f'<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>' if text else ""
    return f"<w:p><w:pPr>{ppr}</w:pPr>{run}</w:p>"


def side(name, val, sz=4, color="000000"):
    return f'<w:{name} w:val="{val}" w:sz="{sz}" w:space="0" w:color="{color}"/>'


def cell(width, content, tcpr=""):
    paras = "".join(p(t) for t in content.split("\n"))
    return f'<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/>{tcpr}</w:tcPr>{paras}</w:tc>'


def row(*cells, trpr="", ex=""):
    return f"<w:tr>{ex}<w:trPr>{trpr}</w:trPr>{''.join(cells)}</w:tr>"


def table(rows, widths, tblpr="", borders=""):
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    borders = f"<w:tblBorders>{borders}</w:tblBorders>" if borders else ""
    return (
        f'<w:tbl><w:tblPr><w:tblW w:w="{sum(widths)}" w:type="dxa"/>{tblpr}{borders}'
        f'<w:tblLayout w:type="fixed"/></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{rows}</w:tbl>'
    )


def lines(label, count):
    return "\n".join(f"{label} {i}" for i in range(1, count + 1))


def fill(count):
    return "".join(p(f"Filler paragraph {i}.") for i in range(1, count + 1))


THIN = "".join(side(s, "single", 4, "BFBFBF") for s in ["top", "left", "bottom", "right"])

# Left cell's right edge against the right cell's left edge; the expected
# winner is in the label.
CONFLICTS = [
    ("Width: single 1.5 pt beats single 0.5 pt", side("right", "single", 4, "C00000"), side("left", "single", 12, "0070C0")),
    ("Style weight: double 0.5 pt (12) beats single 1 pt (8)", side("right", "double", 4, "C00000"), side("left", "single", 8, "0070C0")),
    ("Equal weight 12: single 1.5 pt beats double 0.5 pt", side("right", "double", 4, "C00000"), side("left", "single", 12, "0070C0")),
    ("Dotted weighs 1: single 0.25 pt beats dotted 3 pt", side("right", "dotted", 24, "C00000"), side("left", "single", 2, "0070C0")),
    ("Colour: red beats blue (darker on B + 2G)", side("right", "single", 24, "0000FF"), side("left", "single", 24, "FF0000")),
    ("Colour: magenta beats green (green counts twice)", side("right", "single", 24, "00FF00"), side("left", "single", 24, "FF00FF")),
    ("none yields: the right cell's thick line shows", side("right", "none", 0), side("left", "thick", 24, "0070C0")),
    ("nil hides: no line, though the right cell has one", side("right", "nil", 0), side("left", "thick", 24, "0070C0")),
    ("Cell beats table: a cell's own none defers to the table's insideV", side("right", "none", 0), ""),
]


def conflict(label, left, right):
    borders = THIN + side("insideV", "single", 24, "00B050")
    a = cell(3000, "Left", f"<w:tcBorders>{left}</w:tcBorders>")
    b = cell(3000, "Right", f"<w:tcBorders>{right}</w:tcBorders>" if right else "")
    return p(label, '<w:spacing w:before="240"/>', "<w:b/>") + table(row(a, b), [3000, 3000], borders=borders)


CONFLICTS_BODY = "".join(conflict(*case) for case in CONFLICTS) + p("")

GRID = "".join(side(s, "single", 8, "404040") for s in ["top", "left", "bottom", "right", "insideH", "insideV"])
ORANGE = '<w:shd w:val="clear" w:color="auto" w:fill="FFC000"/>'
SPANS_BODY = (
    p("Spans: row 1 spans columns 1-2, column 1 merges rows 2-4 (orange)", "", "<w:b/>")
    + table(
        row(cell(4000, "Columns 1-2", '<w:gridSpan w:val="2"/>'), cell(2000, "Column 3"))
        + row(cell(2000, "Rows 2-4", f'<w:vMerge w:val="restart"/>{ORANGE}'), cell(2000, "B2"), cell(2000, "C2"))
        + row(cell(2000, "", "<w:vMerge/>"), cell(2000, "B3"), cell(2000, "C3"))
        + row(cell(2000, "", "<w:vMerge/>"), cell(4000, "Columns 2-3", '<w:gridSpan w:val="2"/>'))
        + row(cell(2000, "A5"), cell(2000, "B5"), cell(2000, "C5")),
        [2000, 2000, 2000],
        borders=GRID,
    )
    + p("A centred table", '<w:spacing w:before="240"/>', "<w:b/>")
    + table(row(cell(2000, "Centred"), cell(2000, "Table")), [2000, 2000], '<w:jc w:val="center"/>', GRID)
    + p("A table indented 1 inch", '<w:spacing w:before="240"/>', "<w:b/>")
    + table(row(cell(2000, "Indented"), cell(2000, "Table")), [2000, 2000], '<w:tblInd w:w="1440" w:type="dxa"/>', GRID)
    + p("")
)

SEAM_BORDERS = (
    side("top", "single", 24, "C00000")
    + side("bottom", "single", 24, "00B050")
    + side("insideH", "single", 8, "0070C0")
    + side("left", "single", 8, "404040")
    + side("right", "single", 8, "404040")
    + side("insideV", "single", 8, "404040")
)
SEAMS_BODY = (
    fill(22)
    + p("A row split by the page break; the left column merges rows 2-3 (orange)", "", "<w:b/>")
    + table(
        row(cell(3000, "First row"), cell(3000, "Short"))
        + row(cell(3000, lines("Line", 16), f'<w:vMerge w:val="restart"/>{ORANGE}'), cell(3000, lines("Note", 12)))
        + row(cell(3000, "", "<w:vMerge/>"), cell(3000, "Third row, after the merge"))
        + row(cell(3000, "Last row"), cell(3000, "Short")),
        [3000, 3000],
        borders=SEAM_BORDERS,
    )
    + p("Vertical alignment in a 1-inch row", '<w:spacing w:before="240"/>', "<w:b/>")
    + table(
        row(
            cell(2000, "Top"),
            cell(2000, "Centre", '<w:vAlign w:val="center"/>'),
            cell(2000, "Bottom", '<w:vAlign w:val="bottom"/>'),
            trpr='<w:trHeight w:val="1440" w:hRule="exact"/>',
        ),
        [2000, 2000, 2000],
        borders=GRID,
    )
    + p("")
)

PARTS = {
    "wholeTable": None,
    "band1Horz": "DEEAF6",
    "band2Horz": "FFF2CC",
    "band1Vert": "E2EFD9",
    "band2Vert": "FBE4D5",
    "firstCol": "A9D08E",
    "lastCol": "F4B084",
    "firstRow": "2F5496",
    "lastRow": "8EA9DB",
    "nwCell": "7030A0",
    "neCell": "C00000",
    "swCell": "00B0F0",
    "seCell": "FFC000",
}


def part(kind, colour):
    rpr = '<w:rPr><w:b/><w:color w:val="FFFFFF"/></w:rPr>' if kind in ("firstRow", "nwCell", "neCell") else ""
    borders = f"<w:tcBorders>{side('top', 'double', 4, '2F5496')}</w:tcBorders>" if kind == "lastRow" else ""
    return f'<w:tblStylePr w:type="{kind}">{rpr}<w:tcPr>{borders}<w:shd w:val="clear" w:color="auto" w:fill="{colour}"/></w:tcPr></w:tblStylePr>'


ALL_PARTS = "".join(part(kind, colour) for kind, colour in PARTS.items() if colour)
TABLE_STYLES = (
    '<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/>'
    '<w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>'
    '<w:style w:type="table" w:styleId="Everything"><w:name w:val="Everything"/><w:basedOn w:val="TableNormal"/>'
    f"<w:pPr><w:spacing w:after=\"0\"/></w:pPr><w:tblPr><w:tblStyleRowBandSize w:val=\"1\"/><w:tblStyleColBandSize w:val=\"1\"/><w:tblBorders>{GRID}</w:tblBorders></w:tblPr>"
    f"{ALL_PARTS}</w:style>"
)


def cascade_table(look, rows=6, cnf_row=None, exception_row=None):
    out = ""
    for r in range(rows):
        trpr = '<w:cnfStyle w:val="100000000000" w:firstRow="1"/>' if r == cnf_row else ""
        ex = ""
        if r == exception_row:
            ex = f'<w:tblPrEx><w:tblBorders>{side("insideH", "single", 24, "00B050")}{side("insideV", "single", 24, "00B050")}</w:tblBorders><w:shd w:val="clear" w:color="auto" w:fill="D9D9D9"/></w:tblPrEx>'
        out += row(*(cell(1560, f"r{r + 1}c{c + 1}") for c in range(6)), trpr=trpr, ex=ex)
    return table(out, [1560] * 6, f'<w:tblStyle w:val="Everything"/>{look}')


CASCADE_BODY = (
    p("Every conditional format on (tblLook attributes)", "", "<w:b/>")
    + cascade_table('<w:tblLook w:firstRow="1" w:lastRow="1" w:firstColumn="1" w:lastColumn="1" w:noHBand="0" w:noVBand="0"/>')
    + p("The same by the w:val bitmask 01E0", '<w:spacing w:before="240"/>', "<w:b/>")
    + cascade_table('<w:tblLook w:val="01E0"/>')
    + p("Header row and row bands only (04A0, no first column shown: firstColumn=0)", '<w:spacing w:before="240"/>', "<w:b/>")
    + cascade_table('<w:tblLook w:val="04A0" w:firstColumn="0"/>')
    + p("Row 3 claims the first row by cnfStyle; row 5 has a row exception (grey, green inside lines)", '<w:spacing w:before="240"/>', "<w:b/>")
    + cascade_table('<w:tblLook w:val="04A0" w:firstColumn="0"/>', cnf_row=2, exception_row=4)
    + p("")
)

STYLES = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr>'
    '<w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault>'
    '<w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>'
    f'<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>{TABLE_STYLES}</w:styles>'
)
SETTINGS = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="{W}"><w:compat>'
    '<w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/>'
    '<w:compatSetting w:name="overrideTableStyleFontSizeAndJustification" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/>'
    "</w:compat></w:settings>"
)
CT = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
    '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
    '<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>'
)
ROOT = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>'
RELS = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    f'<Relationship Id="rId1" Type="{R}/styles" Target="styles.xml"/><Relationship Id="rId2" Type="{R}/settings" Target="settings.xml"/></Relationships>'
)
SECT = '<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>'


def write(name, body):
    document = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}"><w:body>{body}{SECT}</w:body></w:document>'
    with zipfile.ZipFile(OUT / name, "w", zipfile.ZIP_DEFLATED) as z:
        for member, data in [
            ("[Content_Types].xml", CT),
            ("_rels/.rels", ROOT),
            ("word/document.xml", document),
            ("word/_rels/document.xml.rels", RELS),
            ("word/styles.xml", STYLES),
            ("word/settings.xml", SETTINGS),
        ]:
            z.writestr(zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0)), data)


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    write("conflicts.docx", CONFLICTS_BODY)
    write("spans.docx", SPANS_BODY)
    write("seams.docx", SEAMS_BODY)
    write("cascade.docx", CASCADE_BODY)
