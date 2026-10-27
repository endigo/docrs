# Template samples (synthetic)

Reference templates and sample Markdown in the shapes a document-export
integration meets in production, prepared on 2026-10-20. All content is
synthetic test data; no customer templates or client documents.

| File | Represents |
| -- | -- |
| `docx-template-missing-pandoc-styles.docx` | Branded firm template as uploaded (Georgia, lower-case `heading N`, Body Text, bordered `Table` style; no First Paragraph/Compact). Its ZIP has `word/` and `word/_rels/` directory entries with 2-byte deflate bodies, which the guarded reader refuses by default (as upstream does). |
| `docx-template-missing-pandoc-styles.normalised.docx` | The same template after an upload normaliser added the styles Pandoc expects. |
| `list-style-template.docx` | Branded List Bullet / List Number styles that only hold `w:numPr`; glyphs, colours and indents in `numbering.xml`, with a `w:lvlOverride`. Modelled on a real customer template shape. |
| `existing-template.docx` | A Word file with no `styles.xml` ("typed in Word, never styled"). |
| `heading-cs-font.docx` | Heading styles in Arial whose sample headings carry only `<w:rFonts w:cs="Arial"/>`, as Word leaves after complex-script edits; the theme fonts are Aptos. From the fidelity comparison of 2026-10-26 (synthetic). |
| `body-direct-formatting.docx` | A Body Text style saying Bookman Old Style 10 pt while every body paragraph is formatted by hand as Calibri 11 pt, colour 404040. Same origin. |
| `normal-direct-formatting.docx` | No Body Text style; the prose is Normal formatted by hand as Arial 10 pt, colour 333333 (52% of the text), beside ten address lines in 8 pt grey and a bold blue sub-heading, all Normal, as a document reused as a template. Same origin. |
| `first-paragraph-style.docx` | A First Paragraph style based on Normal (theme font, 12 pt) and a Body Text style (Bookman Old Style 10 pt); the prose in both is formatted by hand as Calibri 11 pt, colour 404040. Same origin. |
| `unused-body-text-style.docx` | As a PDF-to-Word converter leaves it: a 6 pt Body Text style used on empty spacer paragraphs only, the prose in Normal (Arial 11 pt by style). Same origin; the prose lengthened past 40 characters on 2026-10-27, when a body look started to need prose to go by. |
| `body-spacing-by-hand.docx` | A Body Text style defined as 6 pt after at 1.5 lines while every body paragraph is spaced by hand to none at single lines. Same origin. |
| `body-spacing-in-normal.docx` | The prose in Normal (10 pt after, 1.15 lines by style) while Body Text (6 pt after, single lines) holds one short sample line. Same origin. |
| `oversized-footer-picture.docx` | The default footer holds a small logo and a page number beside an inline picture of 37.3 × 16.7 cm, wider than the A4 page; Word clips it at the page edge, LibreOffice lays it out in full. From the fidelity comparison of 2026-10-26 (synthetic). |
| `heading-theme-colour.docx` | Every heading coloured by hand with both a value and a theme colour (`<w:color w:val="000000" w:themeColor="text1"/>`); the prose is Body Text. From the Open XML SDK validator and re-scoring pass of 2026-10-27 (synthetic, built from docrs's own reference document). |
| `table-cell-shading-order.docx` | A sample table whose header cells put `w:shd` before `w:tcBorders` inside `w:tcPr`, as some editors save it (Word opens it; the schema order is borders, then shading). Same origin. |
| `first-paragraph-unused.docx` | A First Paragraph style based on Normal with no look of its own that no paragraph uses; the prose is Body Text (Arial 10 pt, 1 pt before, 5 pt after). Same origin. |
| `first-paragraph-used.docx` | The control: a First Paragraph based on Body Text with a look of its own (italic, 12 pt before), used on the paragraph after each heading. Same origin. |
| `body-text-no-prose.docx` | A Body Text style defined at 15 pt whose template holds no prose: its only text is two short Normal lines (a 24 pt bold title and a 12 pt Times line), as a letterhead-only template. Same origin (from the Claras e2e suite, synthetic). Its ZIP has `word/` and `word/_rels/` directory entries with data, so it opens with `Tolerance::word()`, as `from-markdown --reference` does. |
| `list-shapes.md` | Nested lists deeper than the template defines, continuations, mixed list kinds, an HTML table with a list in a cell. |
| `record-of-advice.md` | A full Record of Advice as users edit it: repeated tables, `<u>`, `<br>` in cells, empty `<p></p>`, links. Messy on purpose. |
