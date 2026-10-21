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
| `list-shapes.md` | Nested lists deeper than the template defines, continuations, mixed list kinds, an HTML table with a list in a cell. |
| `record-of-advice.md` | A full Record of Advice as users edit it: repeated tables, `<u>`, `<br>` in cells, empty `<p></p>`, links. Messy on purpose. |
