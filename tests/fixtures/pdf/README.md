# PDF fixtures

Small English documents for the PDF engine. `cases.tsv` gives each case's source, reference document, the font file layout measures with (`-`: the bundled table) and the converter's flags.

| Source | What it exercises |
| -- | -- |
| `sources/memo.md` | a prose memo over two pages, with headings, bold and hard line breaks |
| `sources/lists.md` | nested numbered and bullet lists; two separate ordered lists, the second restarting at 1 |
| `sources/table.md` | a table with a header row and right-aligned numbers |
| `sources/cover.md` | the `v2/cover-page-sections.docx` cover page (`--cover-page`) with a title block |
| `sources/toc.md` | a table of contents with 15 headings over five pages |
| `sources/stories.md` | prose over three pages in `references/stories.docx`: a first-page, an even and a default header, and a "Page X of Y" footer |

`appearance.docx` has a run for each appearance property (theme colours with tint and shade, toggles through styles, every underline, every highlight, caps, spacing, scale, super- and subscript, raised and lowered text, shading and hyperlinks), each named for what it shows. `crates/docrs/tests/appearance.rs` builds it and checks it against the committed file.

The references set one family for headings and body:

- `references/arial.docx`, `times-new-roman.docx`, `aptos.docx` and `inter.docx` are written by `tools/pdf/make_references.py` from `docrs markdown-reference`. They set the theme's major and minor Latin fonts.
- `references/stories.docx` is `arial.docx` with headers and a footer, a distinct first page and distinct even pages, written by `tools/pdf/make_stories_reference.py`.
- Inter is in `../fonts/inter` (see `../fonts/PROVENANCE.md`). It is a variable font, as Google Fonts ships it. Layout measures its default (Regular) instance for every weight, while the PDF draws the instance a run's weight selects; wave 3 (the painter) closes this with one source of advances: layout's `FontBook` gives a face per (file, face, `Style`) whose advances are that instance's, chosen by the same Style-to-axis rule the PDF uses (`layout::variation::Fvar::variation`, which `FaceInfo::instance` calls), and a check that each CID's `/W` equals layout's advance for its glyph. The Inter snapshots will move then.

`page-top/` holds 14 small documents for the space a paragraph keeps at the top of a page that a page break starts (`crates/docrs/tests/layout.rs`, `the_space_kept_at_the_top_of_page_2_is_word_s`). Each is Letter with a 1 in top margin, in Arial 11 pt with single line spacing: three filler lines, a paragraph with some space after, then the case on page 2.

| Files | Case |
| -- | -- |
| `0-control-pbb-0-before` | `pageBreakBefore` with no space either side |
| `1-pbb-after-exceeds-before` | `pageBreakBefore`, 6 pt before after 18 pt after |
| `5-pbb-before-exceeds-after` | `pageBreakBefore`, 24 pt before after 5 pt after |
| `2-pbb-contextual` | `pageBreakBefore` between two paragraphs of one style with contextual spacing |
| `3-br-page-then-24-before` | a page break run (`w:br w:type="page"`), then 24 pt before |
| `4-section-next-page` | a next-page section break, then 24 pt before |
| `pbb-m14-*`, `pbb-m15-*` | case 5 in compatibility mode 14 or 15 |
| `section-m14-*`, `section-m15-*` | case 4 in compatibility mode 14 or 15 |

The numbered files have no `settings.xml`; `-fe` adds `w:useFELayout`, `-nofe` leaves it out. The test holds page 2's first baseline to Word's.

`<case>/word.lines.json` is Word's line geometry for the case, and `environment.json` the Word, macOS and font files it was made with (`tools/pdf/check.sh OUT_DIR --fixtures-only --word --update-fixtures`). Word's PDFs themselves are not committed.

`one-page.pdf`, `font-line.pdf` and `image-logo.pdf` are the PDF writer's golden files (`crates/docrs/tests/pdf.rs`, `pdf_fonts.rs` and `pdf_images.rs`): an empty page, a line of Liberation Serif embedded as a subset, and `images/logo.png` with its soft mask.

`images/` holds images from real encoders (`images/PROVENANCE.md`).
