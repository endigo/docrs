# PDF fixtures

Small English documents for the PDF engine. `cases.tsv` gives each case's source, reference document, the font file layout measures with (`-`: the bundled table) and the converter's flags.

| Source | What it exercises |
| -- | -- |
| `sources/memo.md` | a prose memo over two pages, with headings, bold and hard line breaks |
| `sources/lists.md` | nested numbered and bullet lists; two separate ordered lists, the second restarting at 1 |
| `sources/table.md` | a table with a header row and right-aligned numbers |
| `sources/cover.md` | the `v2/cover-page-sections.docx` cover page (`--cover-page`) with a title block |
| `sources/toc.md` | a table of contents with 15 headings over five pages |

`appearance.docx` has a run for each appearance property (theme colours with tint and shade, toggles through styles, every underline, every highlight, caps, spacing, scale, super- and subscript, raised and lowered text, shading and hyperlinks), each named for what it shows. `crates/docrs/tests/appearance.rs` builds it and checks it against the committed file.

The references set one family for headings and body:

- `references/arial.docx`, `times-new-roman.docx`, `aptos.docx` and `inter.docx` are written by `tools/pdf/make_references.py` from `docrs markdown-reference`. They set the theme's major and minor Latin fonts.
- Inter is in `../fonts/inter` (see `../fonts/PROVENANCE.md`). It is a variable font, as Google Fonts ships it. Until layout instances variable fonts, it measures the default (Regular) instance for every weight, so the Inter snapshots will move when instancing lands.

`<case>/word.lines.json` is Word's line geometry for the case, and `environment.json` the Word, macOS and font files it was made with (`tools/pdf/check.sh OUT_DIR --fixtures-only --word --update-fixtures`). Word's PDFs themselves are not committed.

`one-page.pdf` and `font-line.pdf` are the PDF writer's golden files (`crates/docrs/tests/pdf.rs` and `pdf_fonts.rs`): an empty page, and a line of Liberation Serif embedded as a subset.
