# TipTap JSON to Word

docrs converts a TipTap document straight from `editor.getJSON()` (the
ProseMirror document: `{"type": "doc", "content": [...]}`), without
`getHTML()` in between. Library: `docrs::markdown::insert_tiptap`. CLI:
`docrs from-markdown page.json` (`--from tiptap`). Node: `fromJson` (also
under its 0.3.0 name, `fromTiptap`). The
options, report and warnings are those of the Markdown and HTML sources;
pictures get no caption unless asked (`captions`).

The nodes and marks below are those of Tiptap 3's own extensions, under
their default names. The same document renders identically from its JSON
and from its HTML, except where the HTML is only a placeholder for the
editor's rendering (math, details, mentions, emoji, embeds): there the JSON
carries what the HTML does not. Nothing is dropped silently; the last
column names the warning a loss raises.

## Nodes

| Node | Attributes read | Word | Reported |
|---|---|---|---|
| `doc` | | the document; the root must be a `doc` | otherwise refused, `tiptap_json_invalid` |
| `paragraph` | `textAlign` | a paragraph in the template's body styles; `center`, `right`, `justify`/`end` set its alignment, `null`, `left` and `start` leave the style's own | an unknown `textAlign` value: `tiptap_attribute_dropped` |
| `heading` | `level`, `id`, `textAlign` | Heading 1 to 9 (`level` clamped) with a bookmark for `id`, so links to `#id` work; alignment as for paragraphs | |
| `blockquote` | | the block-quote style, nested quotes indented further | |
| `codeBlock` | `language` | the code-block style, one line per `hardBreak` or newline, whitespace kept; `language` has no Word equivalent and is not reported (as in Markdown) | |
| `horizontalRule` | | a rule | |
| `bulletList`, `orderedList` | `start`, `type` | the template's list numbering, nested through levels; `start` restarts an ordered list | `type` other than `1` (letters, roman): `tiptap_attribute_dropped` |
| `taskList` / `taskItem` | `checked` | a bullet list whose items begin with ☒ or ☐ | |
| `listItem`, `taskItem` outside a list | | drawn as bullets; consecutive ones form one list | |
| `table`, `tableRow`, `tableHeader`, `tableCell` | `colspan`, `rowspan`, `colwidth`, `align` | a table over the text width; `colwidth` sets the column proportions, spans merge cells (1 to 1000 each), rows made only of `tableHeader` repeat as header rows, `align` sets a cell's alignment | more than 63 columns: narrowed, `table_columns_clamped` |
| `image` (block or inline) | `src`, `alt`, `title`, `width`, `height`, `textAlign` | a picture from the bytes given for `src` (`images`, a `data:` URI, or `resource_dir`), at the editor's size in pixels (numbers or CSS lengths such as `200px`), capped at the text width; `alt` becomes the description; `textAlign` (the TextAlign extension configured for `image`) aligns the paragraph a picture stands alone in, with the values of a paragraph's `textAlign`, the same as the editor's HTML gives (`text-align` on the `img`, or an app's `<div data-image-align>` around it) | no bytes for a remote `src`: `remote_image_skipped`; an unreadable size or `textAlign`, or an alignment the picture cannot follow because it shares its paragraph with other content or with another alignment: `tiptap_attribute_dropped` |
| `hardBreak` | | a line break | |
| `inlineMath`, `blockMath` | `latex` | an inline or displayed Word equation | LaTeX it cannot read: `math_unknown_command` |
| `tableOfContents` | | a table of contents at that place, with computed page numbers | inside a cell or note: `toc_outside_body` |
| `pageBreak` (also `page-break`, `page_break`, any case) | | a page break | |
| `details`, `detailsSummary`, `detailsContent` | | the summary as a bold paragraph, then the content, always expanded | |
| `youtube` | `src` | a paragraph holding a link to the video | |
| `mention` | `label`, `id`, `mentionSuggestionChar` | text, as the editor shows it: `@label` (the id when there is no label) | |
| `emoji` | `name` | the text `:name:`; the glyph is the editor's to look up | `tiptap_node_dropped` |
| any other node | | its content is converted in place; its text is kept | `tiptap_node_dropped`, once per type |

A node without a `type`, or one that is not an object, refuses the whole
conversion (`tiptap_json_invalid`) with the document unchanged.

## Marks

| Mark | Attributes read | Word | Reported |
|---|---|---|---|
| `bold`, `italic`, `underline`, `strike` | | run formatting | |
| `code` | | the inline-code style | |
| `subscript`, `superscript` | | vertical alignment | |
| `link` | `href`, `title` | a hyperlink with its tooltip, for `http`, `https`, `mailto`, `tel`, `#fragment` and relative targets; adjacent text nodes with the same link form one hyperlink; `target`, `rel` and `class` are the page's concern and ignored | other schemes (`javascript:`, `data:`, …) keep the text without a link: `link_scheme_skipped` |
| `highlight` | `color` | Word's yellow highlight without a colour, else shading in that colour | a colour it cannot read: `tiptap_attribute_dropped` |
| `textStyle` | `color`, `fontFamily`, `fontSize`, `backgroundColor`, `lineHeight` | text colour, the first family of `fontFamily`, the size (px, pt, em, rem, %), shading for `backgroundColor`; empty strings, as Tiptap writes for unset styles, mean unset | `lineHeight` and values it cannot read: `tiptap_attribute_dropped` |
| any other mark | | the text is kept without it | `tiptap_mark_dropped`, once per type |

Attributes that carry no formatting (ids, classes, an app's own data) are
ignored without a report, as `class` is in HTML. Strict conversions
(`strict`) refuse any reported loss and change nothing.

## Refused

- Not JSON, a root that is not a `doc` node, a node without a `type`:
  `invalid_argument`, reason `tiptap_json_invalid`.
- Arrays and objects nested deeper than `MAX_NESTING_DEPTH` (128): a bracket
  scan refuses the source before it is parsed, `markdown_nesting_too_deep`.
  A node and its `content` are two levels, so about sixty nested quotes or
  lists is the limit.
- Over the size budgets, when `limits: "serverless"` is set: 384 KiB of
  source, refused before it is copied into WebAssembly memory.

All three leave the document unchanged.

## Getting the JSON

- In the browser, `editor.getJSON()`; post it to your server as the request
  body.
- From stored HTML, on the server: `generateJSON(html, extensions)` from
  `@tiptap/html/server` gives the same document, or pass the HTML to
  `fromHtml` / `insert_html` directly.
- The fixtures under `tests/fixtures/markdown/tiptap/` were generated this
  way from Tiptap 3.32 and show the exact shape docrs expects (`textAlign:
  null` by default, `""` for unset text styles, `colwidth` arrays, cell
  `align`).
