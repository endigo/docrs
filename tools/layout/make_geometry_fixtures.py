"""Write the DOCX inputs for checking `docrs layout --lines` against Word.

    python3 tools/layout/make_geometry_fixtures.py

alignment.docx: centred, right-aligned and justified paragraphs, a right tab
and a column of amounts on a decimal tab. table.docx: a table whose header
row repeats on the next page and whose long row splits across the break.
row-three-pages.docx: a row over three pages, its two cells of different
lengths. header-rows.docx: 200 rows under two repeated header rows.
cant-split.docx: can't-split rows at a page bottom, one taller than the page.
nested-split.docx: a nested table that splits with the cell holding it.
All are Arial 11 on US Letter with 1-inch margins.
"""

import pathlib
import zipfile

OUT = pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures/layout/geometry"
W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def runs(text, rpr=""):
    out = []
    for i, part in enumerate(text.split("\t")):
        if i:
            out.append(f"<w:r><w:rPr>{rpr}</w:rPr><w:tab/></w:r>")
        if part:
            out.append(f'<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space="preserve">{part}</w:t></w:r>')
    return "".join(out)


def p(text, ppr="", rpr=""):
    return f"<w:p><w:pPr>{ppr}</w:pPr>{runs(text, rpr)}</w:p>"


def cell(width, content, ppr=""):
    paras = "".join(p(t, ppr) for t in content.split("\n"))
    return f'<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr>{paras}</w:tc>'


PROSE = (
    "The layout engine decides every line and page break once, and the painter draws "
    "exactly what it decided. Each line carries where it starts, how wide its text is, "
    "how much room is left over and which spaces a justified line may stretch or shrink. "
)

ALIGNMENT = "".join(
    [
        p("Quarterly operations report", '<w:jc w:val="center"/>', '<w:b/><w:sz w:val="32"/>'),
        p("Operations", '<w:jc w:val="center"/>'),
        p("27 November 2026", '<w:jc w:val="right"/>'),
        p(PROSE * 2, '<w:jc w:val="both"/><w:spacing w:before="240"/>'),
        p(PROSE, '<w:jc w:val="center"/><w:spacing w:before="240"/>'),
        p(PROSE, '<w:jc w:val="right"/><w:spacing w:before="240"/>'),
        p("Region\tRevenue", '<w:tabs><w:tab w:val="right" w:pos="5040"/></w:tabs><w:spacing w:before="240"/>', "<w:b/>"),
    ]
    + [
        p(f"{name}\t{amount}", '<w:tabs><w:tab w:val="decimal" w:pos="4680"/></w:tabs>')
        for name, amount in [
            ("North America", "1,610.25"),
            ("Europe", "1,120.5"),
            ("Asia", "905"),
            ("Oceania", "252.125"),
            ("Total", "3,887.875"),
        ]
    ]
    + [p("Prepared by\tPlatform team", '<w:tabs><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:before="240"/>')]
)

HEADER = '<w:tr><w:trPr><w:tblHeader/></w:trPr>' + cell(2400, "Item") + cell(1800, "Amount", '<w:jc w:val="right"/>') + cell(5160, "Notes") + "</w:tr>"
ROWS = "".join(
    "<w:tr>"
    + cell(2400, f"Item {i}")
    + cell(1800, f"{i * 137 % 1000}.{i % 10}0", '<w:jc w:val="right"/>')
    + cell(5160, "Routine." if i != 21 else "\n".join([PROSE] * 4))
    + "</w:tr>"
    for i in range(1, 31)
)
BORDERS = "".join(f'<w:{side} w:val="single" w:sz="4" w:space="0" w:color="808080"/>' for side in ["top", "left", "bottom", "right", "insideH", "insideV"])
TABLE = (
    p("Expenses by item", "", "<w:b/>")
    + f'<w:tbl><w:tblPr><w:tblW w:w="9360" w:type="dxa"/><w:tblBorders>{BORDERS}</w:tblBorders><w:tblLayout w:type="fixed"/></w:tblPr>'
    + '<w:tblGrid><w:gridCol w:w="2400"/><w:gridCol w:w="1800"/><w:gridCol w:w="5160"/></w:tblGrid>'
    + HEADER
    + ROWS
    + "</w:tbl>"
    + p("End of table.")
)

GRID = '<w:tblGrid><w:gridCol w:w="4680"/><w:gridCol w:w="4680"/></w:tblGrid>'


def table(rows, grid=GRID):
    return f'<w:tbl><w:tblPr><w:tblW w:w="9360" w:type="dxa"/><w:tblBorders>{BORDERS}</w:tblBorders><w:tblLayout w:type="fixed"/></w:tblPr>{grid}{rows}</w:tbl>'


def row(*cells, trpr=""):
    return f"<w:tr><w:trPr>{trpr}</w:trPr>{''.join(cells)}</w:tr>"


def lines(label, count):
    return "\n".join(f"{label} {i}" for i in range(1, count + 1))


def fill(count):
    return "".join(p(f"Filler paragraph {i}.") for i in range(1, count + 1))


ROW_THREE_PAGES = (
    p("A row over three pages", "", "<w:b/>")
    + table(
        row(cell(4680, "First row"), cell(4680, "Short"))
        + row(cell(4680, lines("Line", 80)), cell(4680, lines("Note", 40)))
        + row(cell(4680, "Last row"), cell(4680, "Short"))
    )
    + p("End of table.")
)

HEADER_ROWS = (
    p("Two header rows over 200 rows", "", "<w:b/>")
    + table(
        row(cell(4680, "Region"), cell(4680, "Amount"), trpr="<w:tblHeader/>")
        + row(cell(4680, "Name"), cell(4680, "Thousands"), trpr="<w:tblHeader/>")
        + "".join(row(cell(4680, f"Row {i}"), cell(4680, f"{i * 37 % 1000}")) for i in range(1, 201))
    )
    + p("End of table.")
)

CANT_SPLIT = (
    fill(26)
    + table(
        row(cell(4680, lines("Split", 8)), cell(4680, "May split"))
        + row(cell(4680, lines("Middle", 25)), cell(4680, "Fills the page"))
        + row(cell(4680, lines("Kept", 8)), cell(4680, "Can't split"), trpr="<w:cantSplit/>")
        + row(cell(4680, lines("Tall", 60)), cell(4680, "Can't split, taller than the page"), trpr="<w:cantSplit/>")
        + row(cell(4680, "After the tall row"), cell(4680, "Short"))
    )
    + p("End of table.")
)

NESTED_GRID = '<w:tblGrid><w:gridCol w:w="2200"/><w:gridCol w:w="2200"/></w:tblGrid>'
NESTED = (
    f'<w:tbl><w:tblPr><w:tblW w:w="4400" w:type="dxa"/><w:tblBorders>{BORDERS}</w:tblBorders><w:tblLayout w:type="fixed"/></w:tblPr>{NESTED_GRID}'
    + row(cell(2200, lines("Inner", 16)), cell(2200, lines("Side", 4)))
    + "</w:tbl>"
)
NESTED_SPLIT = (
    fill(24)
    + table(
        row(
            f'<w:tc><w:tcPr><w:tcW w:w="4680" w:type="dxa"/></w:tcPr>{p("Before the nested table")}{NESTED}{p("After the nested table")}</w:tc>',
            cell(4680, "Outer cell"),
        )
    )
    + p("End of table.")
)

STYLES = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>'
CT = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>'
ROOT = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>'
RELS = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/styles" Target="styles.xml"/></Relationships>'
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
        ]:
            z.writestr(zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0)), data)


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    write("alignment.docx", ALIGNMENT)
    write("table.docx", TABLE)
    write("row-three-pages.docx", ROW_THREE_PAGES)
    write("header-rows.docx", HEADER_ROWS)
    write("cant-split.docx", CANT_SPLIT)
    write("nested-split.docx", NESTED_SPLIT)
