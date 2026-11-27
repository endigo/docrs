"""Write the single-font reference documents the PDF fixtures convert into.

Usage: python3 -I tools/pdf/make_references.py STARTER.docx OUT_DIR

STARTER is `docrs markdown-reference --out STARTER.docx`. Each output sets
the theme's major and minor Latin typefaces to one family, so headings and
body text share it. Zip entries keep a fixed timestamp, so the output only
changes when the starter does.
"""

import os
import re
import sys
import zipfile

FAMILIES = {
    "aptos": "Aptos",
    "arial": "Arial",
    "times-new-roman": "Times New Roman",
    "inter": "Inter",
}


def main():
    starter, out_dir = sys.argv[1], sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    with zipfile.ZipFile(starter) as source:
        entries = [(info, source.read(info.filename)) for info in source.infolist()]
    for slug, family in FAMILIES.items():
        path = os.path.join(out_dir, slug + ".docx")
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as out:
            for info, data in entries:
                if info.filename.startswith("word/theme/"):
                    text = data.decode("utf-8")
                    text = re.sub(r'<a:latin typeface="[^"]*"', f'<a:latin typeface="{family}"', text)
                    data = text.encode("utf-8")
                fixed = zipfile.ZipInfo(info.filename, date_time=(2026, 1, 1, 0, 0, 0))
                fixed.compress_type = zipfile.ZIP_DEFLATED
                out.writestr(fixed, data)
        print(path)


if __name__ == "__main__":
    main()
