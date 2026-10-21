# Template samples v2

Twenty synthetic reference templates that reproduce shapes practices upload in production. Every name, address, ABN and AFSL number is a placeholder; no customer file was opened or copied.

Checks run when they were made: every part is well-formed XML (xmllint), property children are in schema order, python-docx opens each one, and Pandoc 3.9 reads each file and uses each as a reference doc.

| File | Production shape it mimics |
| -- | -- |
| `normal-only.docx` | Practice restyled only Normal (Arial 10.5 pt, dark grey); no heading, Body Text, Title or Table styles. |
| `direct-run-formatting.docx` | Body formatted by hand as direct run formatting (Arial 11) with Normal left at Word defaults. |
| `paragraph-style.docx` | Body copy in a custom style named "Paragraph" (justified Garamond), headings in a custom "Section Heading" style; no Body Text. |
| `outline-level-headings.docx` | Headings made with direct formatting plus `w:outlineLvl` on body paragraphs, no heading styles. |
| `numbered-headings.docx` | Legal-style numbered headings (1, 1.1, 1.1.1): a multilevel list linked to Heading 1-3 by `w:pStyle` back-links and `w:numPr` on the styles. |
| `serif-navy.docx` | Traditional serif brand: Georgia, navy headings, italic First Paragraph, Block Text with a gold left rule. |
| `sans-teal.docx` | Modern sans brand: Helvetica, teal all-caps letter-spaced Heading 1 with a bottom border, grey body. |
| `direct-formatted-table.docx` | One sample table formatted by hand (borders, cell margins, navy header fill, white bold header text) and no Table style. |
| `banded-table-style.docx` | A real `Table` style: header fill, banded rows, last-row double rule, tighter cell paragraph spacing. |
| `builtin-table-style-only.docx` | Sample table uses a Word gallery style ("Grid Table 4 Accent 1") and nothing is named `Table`, so Pandoc tables lose this look. |
| `letterhead-header-footer.docx` | Letterhead: logo and address table in the header, a larger floating logo on a different first page (`w:titlePg`), disclaimer plus "Page X of Y" (`PAGE`, `NUMPAGES` fields) in the footer. |
| `cover-page-sections.docx` | Designed cover page in its own section (centred vertically), then a content section with header, footer `PAGE` field and `w:pgNumType w:start="1"`. Pandoc discards the template body and keeps the **first** `w:sectPr`, which is the cover's: in Pandoc 3.9 the export gets `w:vAlign="center"` on every page and no header or footer references. |
| `landscape-a4.docx` | Landscape A4 with 2 cm margins for wide cash-flow tables. |
| `multi-section-portrait-then-landscape.docx` | Portrait first section and a landscape appendix as the last section. Pandoc keeps only the first `w:sectPr` in document order, so in Pandoc 3.9 the whole export is portrait and the landscape appendix is lost. |
| `us-letter.docx` | Built on a US Word install: Letter paper, 1 inch margins, en-US, Cambria body (A4 everywhere else in this set). |
| `compat-word2007.docx` | Older template still in Word 2007 compatibility mode (`compatibilityMode` 12 plus `w:useFELayout`). |
| `compat-word2003-legacy.docx` | Converted from a .doc file: legacy compat flags and no `compatibilityMode` setting, which Word lays out as Word 2003. |
| `list-paragraph-bullets.docx` | Branded orange bullets made with Word's bullet button (List Paragraph plus numbering), no List Bullet style. |
| `localised-style-ids-de.docx` | Saved by a German Word install: style ids are localised (`Standard`, `berschrift1`, `Textkrper`, `Titel`) while names stay English. Pandoc matches styles on ids. |
| `rtl-arabic.docx` | **Not a production shape.** Right-to-left Arabic edge case (`w:bidi` paragraphs and section, `w:rtl` runs, `w:bidiVisual` table) for the converter only. |

## Notes from generating these

- **Section properties come from the first `w:sectPr` in the template**, not the body's final one. I checked this with Pandoc 3.9 against `cover-page-sections` and `multi-section-...` (the output carries the first section's `w:type`, `w:pgSz` and `w:vAlign`, and no header references). Production runs Pandoc 2.9.2.1, which I did not run. For single-section templates the two are the same.
