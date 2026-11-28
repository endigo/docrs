"""Usage: python3 -I tools/pdf/make_stories_reference.py REFERENCE.docx OUT.docx

Copies REFERENCE (tests/fixtures/pdf/references/arial.docx) with a
first-page, an even-page and a default header and a "Page X of Y" footer
on every page, a distinct first page and distinct even pages.
"""

import re
import sys
import zipfile

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
WML = "application/vnd.openxmlformats-officedocument.wordprocessingml"


def paragraph(content, jc):
    return f'<w:p><w:pPr><w:jc w:val="{jc}"/></w:pPr>{content}</w:p>'


def run(text):
    return f'<w:r><w:t xml:space="preserve">{text}</w:t></w:r>'


def field(code):
    char = lambda kind: f'<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>'
    instr = f'<w:r><w:instrText xml:space="preserve"> {code} </w:instrText></w:r>'
    return char("begin") + instr + char("separate") + run("1") + char("end")


def story(root, paragraphs):
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        f'<w:{root} xmlns:w="{W}" xmlns:r="{R}">{paragraphs}</w:{root}>'
    )


PARTS = {
    "header1.xml": ("hdr", "header", paragraph(run("First page header"), "left")),
    "header2.xml": ("hdr", "header", paragraph(run("Even page header"), "left")),
    "header3.xml": ("hdr", "header", paragraph(run("Default header, odd pages"), "right")),
    "footer1.xml": (
        "ftr",
        "footer",
        paragraph(run("Page ") + field("PAGE") + run(" of ") + field("NUMPAGES"), "center"),
    ),
}

REFERENCES = [
    ("header", "first", "header1.xml"),
    ("header", "even", "header2.xml"),
    ("header", "default", "header3.xml"),
    ("footer", "first", "footer1.xml"),
    ("footer", "even", "footer1.xml"),
    ("footer", "default", "footer1.xml"),
]


def main():
    reference, out = sys.argv[1], sys.argv[2]
    with zipfile.ZipFile(reference) as source:
        entries = [(info.filename, source.read(info.filename)) for info in source.infolist()]
    ids = {name: f"rIdStory{n}" for n, name in enumerate(PARTS, 1)}
    edits = {
        "[Content_Types].xml": lambda text: text.replace(
            "</Types>",
            "".join(
                f'<Override PartName="/word/{name}" ContentType="{WML}.{kind}+xml"/>'
                for name, (_, kind, _) in PARTS.items()
            )
            + "</Types>",
        ),
        "word/_rels/document.xml.rels": lambda text: text.replace(
            "</Relationships>",
            "".join(
                f'<Relationship Id="{ids[name]}" Type="{R}/{kind}" Target="{name}"/>'
                for name, (_, kind, _) in PARTS.items()
            )
            + "</Relationships>",
        ),
        "word/document.xml": lambda text: re.sub(
            r"<w:sectPr( [^>]*)?>",
            lambda m: m.group(0)
            + "".join(
                f'<w:{kind}Reference w:type="{which}" r:id="{ids[name]}"/>'
                for kind, which, name in REFERENCES
            ),
            text,
            count=1,
        ).replace("<w:docGrid ", "<w:titlePg/><w:docGrid ", 1),
        # CT_Settings puts evenAndOddHeaders after defaultTabStop and before
        # characterSpacingControl.
        "word/settings.xml": lambda text: text.replace(
            "<w:characterSpacingControl ",
            "<w:evenAndOddHeaders/><w:characterSpacingControl ",
            1,
        ),
    }
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for name, data in entries:
            if name in edits:
                data = edits[name](data.decode("utf-8")).encode("utf-8")
            fixed = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            fixed.compress_type = zipfile.ZIP_DEFLATED
            target.writestr(fixed, data)
        for name, (root, _, paragraphs) in PARTS.items():
            fixed = zipfile.ZipInfo(f"word/{name}", date_time=(2026, 1, 1, 0, 0, 0))
            fixed.compress_type = zipfile.ZIP_DEFLATED
            target.writestr(fixed, story(root, paragraphs))
    print(out)


if __name__ == "__main__":
    main()
