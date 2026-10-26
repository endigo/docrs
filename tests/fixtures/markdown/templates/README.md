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
| `unused-body-text-style.docx` | As a PDF-to-Word converter leaves it: a 6 pt Body Text style used on empty spacer paragraphs only, the prose in Normal (Arial 11 pt by style). Same origin. |
| `body-spacing-by-hand.docx` | A Body Text style defined as 6 pt after at 1.5 lines while every body paragraph is spaced by hand to none at single lines. Same origin. |
| `body-spacing-in-normal.docx` | The prose in Normal (10 pt after, 1.15 lines by style) while Body Text (6 pt after, single lines) holds one short sample line. Same origin. |
| `list-shapes.md` | Nested lists deeper than the template defines, continuations, mixed list kinds, an HTML table with a list in a cell. |
| `record-of-advice.md` | A full Record of Advice as users edit it: repeated tables, `<u>`, `<br>` in cells, empty `<p></p>`, links. Messy on purpose. |
