# PDF fixtures

Small English documents for the PDF engine. `cases.tsv` gives each case's source, reference document, the font file layout measures with (`-`: the bundled table) and the converter's flags.

| Source | What it exercises |
| -- | -- |
| `sources/memo.md` | a prose memo over two pages, with headings and bold |
| `sources/lists.md` | nested numbered and bullet lists; two separate ordered lists, the second restarting at 1 |
| `sources/table.md` | a table with a header row and right-aligned numbers |
| `sources/cover.md` | the `v2/cover-page-sections.docx` cover page (`--cover-page`) with a title block |
| `sources/toc.md` | a table of contents with 15 headings over five pages |

The references set one family for headings and body:

- `references/arial.docx`, `times-new-roman.docx`, `aptos.docx` and `inter.docx` are written by `tools/pdf/make_references.py` from `docrs markdown-reference`. They set the theme's major and minor Latin fonts.
- Inter is in `../fonts/inter` (see `../fonts/PROVENANCE.md`).

`<case>/word.lines.json` is Word's line geometry for the case (`tools/pdf/check.sh OUT_DIR --word --update-fixtures`). Word's PDFs themselves are not committed.
