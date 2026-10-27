# Changelog

## 0.3.10 (2026-10-27)

### Library

- Page 1 of the output shows the header and footer the template's page 1 shows. A template with a first section for a cover or letter and a last section for the body pages (19 of 272 production templates in a Word-render review, 13 of them one licensee's) lost its page 1 logo or letterhead: the output took its headers from the last section alone. The output's single section now shows, on page 1, the first section's first-page header and footer when that section has a distinct first page and its default ones otherwise, and on the other pages the last section's default ones, inherited from earlier sections where the last lacks them, as Word resolves them. When page 1 differs from the other pages the section gets a distinct first page (`w:titlePg`) carrying page 1's header and footer; when they are the same it has none, and a first-page reference the last section defined is dropped. Page size, margins and orientation still come from the last section. `inspectReference` reports the resulting `section.title_page` and `headers_footers` (a story the last section does not define itself is `inherited`), and its `multiple_sections` problem says so.

- A Body Text style counts as used by the template only when it holds prose, as a First Paragraph does (below): short lines and list items in it do not keep it the body. A template whose Body Text held two short lines and four numbered steps of a closing section while sixteen paragraphs of prose were Normal (Arial 11 pt) exported the body in Body Text at 10 pt; the body now follows the prose to Normal, and `inspectReference` reports it under `roles.body`.

- A `List Bullet` or `List Number` style the template never writes in takes the font and size of the template's own bulleted or numbered sample paragraphs, which sit in another style (List Paragraph, or whatever Word's list buttons used), as Word resolves them through the style chain and the defaults: a theme font stays a theme font. A template whose List Bullet was bound to the bullet numbering in the theme's minor font while its bullets were List Paragraph in Arial exported the bullets in the theme font; they are Arial 11 pt now. A list style the template writes in keeps its own font.

- A heading level the template never uses, whose font still comes from the theme, takes the font the template named on the nearest higher level it does use: a Heading 4 in the theme's major font under Headings 1 to 3 set to Arial follows them (its italic and colour stay). A heading level the template uses, or one with a named font anywhere in its style chain, is left as it is.

- A First Paragraph style counts as used by the template only when it holds prose, a paragraph of 40 characters or more, as the body look does since 0.3.9; a short sample line such as "First Paragraph.", which templates built from Pandoc's sample reference carry for every style, no longer keeps the style in use. Such a template, whose prose is Body Text, gave the paragraph after each heading the First Paragraph style and so Normal's look; it now takes the body's style, as a template with no First Paragraph text does, and `inspectReference` reports the body's style under `roles.first_paragraph`. A First Paragraph holding prose, and a template with no prose at all, are unchanged.

## 0.3.9 (2026-10-27)

### Library

- A colour lifted from the template's sample text onto an existing style keeps its value beside its theme colour. A heading coloured by hand with `<w:color w:val="000000" w:themeColor="text1"/>` was lifted onto Heading 1 as `<w:color w:themeColor="text1"/>`, without the `w:val` the schema requires, and the Open XML SDK validator rejected the styles part. The sample's attributes still replace the style's one by one, and a font or colour set by name alone still drops the theme attribute Word would otherwise prefer (and a theme font the named font), but an attribute the sample sets itself is never dropped, and `w:color` always keeps a `w:val`.

- Every properties element a conversion copies from a template part is written in schema order, whatever order the template used: the paragraph and run properties lifted from a hand-made heading or from body text, a sample table's cell properties (`tcBorders`, then `shd`, then `tcMar`) and table properties, and the sides of borders and cell margins. A table whose header cells listed `w:shd` before `w:tcBorders`, as some editors save it, gave a table style the validator rejected; Word opens either.

- A First Paragraph style the template defines but never writes text in is passed over: the paragraph after each heading takes the same style as the rest of the body (Body Text, or the default style when that carries the prose), and the template's First Paragraph is left as it is. A template whose prose is Body Text (Arial 10 pt) beside an unused First Paragraph based on Normal exported the first paragraph of every section in the theme font at 12 pt. A First Paragraph the template does use is kept as the template defines it, following the body's look and spacing when it has none of its own (0.3.6, 0.3.7). `inspectReference` and `markdown-inspect` report the body's style under `roles.first_paragraph` for such a template.

- A body look or spacing is lifted from the template's sample text, and a Body Text or First Paragraph the template writes no text in is passed over, only when the template has prose to go by: paragraphs of 40 characters or more, not letterhead or label lines. A template whose Body Text is defined at 15 pt and whose only text is two short Normal lines in Times 12 pt exported the body in a "Template Body Text" at 12 pt; it now keeps Body Text as the body style, creates nothing, and `inspectReference` reports `lifted.body` as null. A letterhead-only template with three short lines in brown Trebuchet over an Arial 10 pt Body Text keeps the Body Text the same way. Prose formatted by hand still lifts its look and spacing (0.3.5 to 0.3.7), and a 6 pt spacer Body Text beside real prose in Normal is still passed over for Normal (0.3.6).

## 0.3.8 (2026-10-26)

### Library

- `inspectReference` and `markdown-inspect` report, and a conversion warns (`picture_larger_than_page`), when a header or footer the output keeps holds a picture it cannot show whole: an inline picture wider than the page or taller than half of it, an anchored picture wider or taller than the page. The message names the header or footer (the one every page shows, the first-page one or the even-page one) and gives the picture's size and the page's in centimetres, in plain language a practice can act on: resize or remove it in the template. A first-page header holds the cover of templates that have one, so its inline picture is reported only when it does not fit on the page; a page-sized background anchored behind the text, and Word's own watermarks (VML shapes, not measured), are not reported. The output is unchanged: headers and footers are kept as they are. A footer holding a 37.3 by 16.7 cm picture on an A4 page, which Word clips at the page edge while other renderers lay out in full, left both reports silent.

## 0.3.7 (2026-10-26)

### Library

- The body style takes the paragraph spacing the template's prose shows, as it takes the run look: the spacing (before, after, line and its rule, as Word resolves them through the style chain and the document defaults) carrying the most characters of the unstyled paragraphs and of the body style's own paragraphs, when it differs from what the style gives. A Body Text defined as 6 pt after at 1.5 lines whose every paragraph is set by hand to none at single lines now exports at none and single; a template whose prose is Normal at 10 pt after and 1.15 lines while Body Text holds one sample line now spaces the body like the prose. The spacing is written on the style in full (`w:spacing` before, after, line, lineRule), so nothing inherited shows through. A First Paragraph or Compact style with no spacing of its own follows the body, as it follows the body's look. When the unstyled paragraphs alone show a spacing other than the default style's, the Body Text added for them carries it. The gap a conversion puts above a paragraph after a table or list is unchanged. `inspectReference` and `markdown-inspect` report the spacing in `lifted.spacing` (points before and after; `line` a multiple of single spacing under the `auto` rule, else points).

## 0.3.6 (2026-10-26)

### Library

- The look the body style takes from a template's sample paragraphs is the one carrying the most characters, a plurality, no longer one carrying at least 60% of them. A template whose prose is formatted by hand as Arial 10 pt grey (52% of its text) beside an address block in 8 pt and a bold sub-heading now exports body text in Arial 10 pt grey instead of the theme font. A tie, or plain text (the style's own look) in the lead, still lifts nothing.

- A template's First Paragraph and Compact styles follow the body. Each takes the look its own sample paragraphs show by hand; one that wears only the default style's look (based on Normal with no font, size or colour of its own or of a parent) takes the look the body style took, so the paragraph after each heading matches the rest of the body. One with a font, size or colour of its own, or based on the body style, is left alone. A First Paragraph style the template sets no run look on came out in the theme font while the body was lifted to Calibri 11 pt grey.

- A Body Text (or Paragraph, First Paragraph, Compact) style the template writes no text in while its default style carries the prose, as a PDF-to-Word converter leaves a 6 pt Body Text on empty spacer paragraphs, is not the template's body: body text, first paragraphs and compact text use the default style as the prose does, and the template's style is left as it is. When that prose shows a look by hand, the style added for it is named "Template Body Text", apart from the template's own. Body text came out in the spacer's 6 pt.

## 0.3.5 (2026-10-26)

### Library

- A look lifted from the template's sample paragraphs onto an existing style merges into the style's run properties attribute by attribute instead of replacing each element. A heading run carrying only `<w:rFonts w:cs="Arial"/>` (what Word leaves after complex-script edits) no longer drops the style's `w:ascii` and `w:hAnsi`, so the headings keep the style's Arial instead of falling back to the theme font. A font or colour the sample sets by name drops the theme attribute Word would otherwise prefer (`asciiTheme`, `cstheme`, `themeColor`, …), and the other way round.

- The body style the conversion uses (Body Text, Paragraph, or the template's own) takes the look its sample paragraphs show by hand, as headings do: the run look carrying at least 60% of the characters of the template's top-level paragraphs in that style, table cells and list items left out, emphasis left out, a bold or over-14 pt first run read as a title. A template whose Body Text style says Bookman Old Style 10 pt while every body paragraph is formatted directly as Calibri 11 pt grey now exports body text as Calibri 11 pt grey. The default style is never restyled: when the template's unstyled paragraphs show a look, a Body Text style is added with it as before, the look now chosen by characters rather than by paragraph count. `inspectReference` and `markdown-inspect` report the look the body style takes in `lifted.body`.

## 0.3.4 (2026-10-23)

### Library

- A horizontal rule (`---` in Markdown, `<hr>` in HTML, `horizontalRule` in TipTap JSON) is one flat line across the text: an empty paragraph with a thin single bottom border in the text colour (`w:pBdr`, ¾ pt), 6 pt of space above and below, and a 1 pt paragraph mark so the line takes no more room than that. Every source writes the same paragraph. It was Word's VML "standard" rule (`v:rect` with `o:hr`), which Word draws as a shaded bar, two grey lines with a lighter fill between them, and which reads as a double line. No VML is written any more. A reference document cannot restyle the rule: its look is on the paragraph, not in a style.

- A table of contents (`toc: true`, front matter `toc`, `[TOC]`, TipTap's `tableOfContents`) is Word's table-of-contents block: the heading and every entry sit inside one content control (`w:sdt` with the `Table of Contents` document-part gallery), as Word and LibreOffice write one, so Word shows the TOC frame with its Update Table tab when the contents is clicked, treats it as the document's contents under References, and replaces heading and entries together. The field, its cached page numbers and the PAGEREF links are unchanged. The heading and entries are no longer direct body paragraphs: `Document::paragraphs` (python-docx's `document.paragraphs`) lists what follows them, as it does for a contents Word wrote.

- The title above the table of contents is, in order: `toc_title` (CLI `--toc-title`, Node `tocTitle`), the front matter's `toc-title`, the title of the reference document's own table of contents, else "Table of Contents", what Microsoft 365's References > Table of Contents inserts (it was "Contents"). The reference's title is the first text before the TOC field in its table-of-contents block (Word's and LibreOffice's content control), or, for a bare TOC field, the TOC Heading paragraph just before it; trimmed, and only when it is 1 to 80 characters. `inspectReference` and `markdown-inspect` report it as `toc_title` (null when the reference has no contents); the conversion report says where the title came from in `toc_title_source` (`option`, `front_matter`, `reference`, `default`; absent without a contents). A `[TOC]` marker or `tableOfContents` node now takes the front matter's `toc-title` like the `toc` option does.

## 0.3.3 (2026-10-23)

### Library

- A partly resized table keeps the widths the editor set. In an HTML or TipTap JSON table where some columns carry a `colwidth` and others do not (Tiptap's column resizing writes one onto the dragged column only), a sized column is now as wide as the editor shows it (its pixels at 96 dpi; a percentage is of the text width) and the columns without one share the rest of the text width by their text, the measure a table with no widths already uses. When the sized columns alone are wider than the text, every column is scaled down in proportion, the unsized ones from the editor's 25 px minimum. A spanning cell with no `colwidth` sizes no column; one with a `colwidth` list sizes each column it spans, as before. A table with every column sized (proportions over the text width) renders as before. Such a table came out as equal columns, the one width the editor set dropped along with the sizing by content. In the measure by text (every source, Markdown included), a header cell now counts its longest word rather than its whole text, so a long header wraps between its words, as the editor shows it, instead of widening a column of short values; body cells count their longest line as before, and no column is narrower than its longest word.

## 0.3.2 (2026-10-22)

### Node.js

- The package recovers from a panic by itself. A panic (a bug in docrs-wasm) still throws a `WebAssembly.RuntimeError` with no `code` from the call that hit it; the next call now runs on a fresh instance of the module, so a poisoned instance never serves another call. The new entry point, `index.js`, evaluates the wasm-bindgen glue again around a new `WebAssembly.Instance` of the module compiled when the package loaded and rethrows the error as it is; any other exception that unwinds through the module (a stack overflow, say) is handled the same way. Errors docrs reports (an `Error` with a `code`) leave the instance in place, and nothing is retried: the same input would panic again. The README's reload-and-retry recipe is gone with the need for it.

## 0.3.1 (2026-10-22)

### Library

- A picture's `textAlign` (Tiptap's TextAlign extension configured for `image`) aligns the paragraph the picture stands alone in, with the values and defaults of a paragraph's `textAlign`. The TipTap JSON source ignored it silently before; now a centred picture renders the same from `getJSON()` as from the editor's HTML, whether that is Tiptap's own (`text-align` on the `img`, which the HTML source now reads instead of reporting it as a dropped style) or an app's `<div data-image-align>` around the picture. A `textAlign` value that cannot be read, or an alignment a picture cannot follow because it shares its paragraph with other content or with another alignment, is reported (`tiptap_attribute_dropped`; `html_style_dropped` from HTML).

### Node.js

- `fromJson(document, options?)` is the name of the TipTap entry point, next to `fromMarkdown` and `fromHtml`. `fromTiptap`, its name in 0.3.0, stays as a deprecated alias of the same function.

## 0.3.0 (2026-10-22)

### Library

- **TipTap JSON to Word** (`markdown` feature): `markdown::insert_tiptap` and `InputFormat::Tiptap` render a TipTap document (`editor.getJSON()`) directly, without `getHTML()` in between. The nodes and marks of Tiptap 3's own extensions convert to the same document the editor's HTML gives: headings and paragraphs with `textAlign` (the editor's default alignment is left to the template's styles), bold, italic, underline, strike, code, subscript, superscript, links, highlights, text styles (colour, font, size, background), bullet, ordered and task lists, quotes, code blocks, rules, hard breaks, images, tables with `colspan`, `rowspan`, `colwidth` and cell alignment; and from the JSON what the HTML only hints at: inline and block math as equations, details (summary in bold), mentions, YouTube embeds as links, a table of contents and page breaks. New warnings `tiptap_node_dropped`, `tiptap_mark_dropped` and `tiptap_attribute_dropped` report what has no Word equivalent (the text is kept). A source that is not JSON or not a `doc` node is refused (`invalid_argument`, `tiptap_json_invalid`) and arrays and objects nested past `MAX_NESTING_DEPTH` are refused before parsing (`markdown_nesting_too_deep`), with nothing changed either way.

- Characters XML cannot carry (control characters other than tab and line breaks, U+FFFE, U+FFFF) in a Markdown, HTML or TipTap source are dropped with a warning, `control_characters_dropped`. Before, the conversion failed with the internal error `staged_reparse`; Word refuses a file that holds one.

### Command line

- `from-markdown` takes TipTap JSON: `--from tiptap` (or `json`), the default for `.json` files.

### Node.js

- `fromTiptap(document, options?)`: the editor's `getJSON()` as an object or as JSON text, with the options and result of `fromMarkdown`; the same size budgets apply.
- The README describes what a panic leaves behind (measured against the published 0.2.0): the instance keeps answering with correct output, but each panic leaks the call's memory and a later panic prints nothing, so a `WebAssembly.RuntimeError` of any message means reload and retry once.

## 0.2.0 (2026-10-22)

### Library

- **Package and XML:**
  - A removed part's name can be used again, in the same edit or a later one. Before, adding a part under a removed name failed with the internal error `duplicate_part`. A part re-added with identical bytes saves as the original. Handles into the removed member (spans, blocks, paragraphs, runs, tables, controls) are stale against the new one (`story_replaced`, `stale_handle`, …) even when its content lines up (`XmlTree::incarnation`).
  - Opening a package is linear in its part count: 64k parts open in about 0.3 s instead of 48 s.
  - Copying deeply nested markup (deep copies, importing from another document as `append` and `insert-from` do) and comparing run properties during replace no longer recurse. With `--trusted`, 40k levels of nested content controls no longer abort `append` with a stack overflow.
- **Protection:** every inherited writer now honours Restrict Editing: authoring (paragraphs, runs, tables, cells, pictures, tab stops, `clear_body_content`), sections, headers and footers, styles and latent styles, and core properties. A forbidden edit refuses `document_protected` and changes nothing; `Document::acknowledge_protection` lets it through. `Run::mark_comment_range` counts as a comment edit. `Document::ensure_styles_part` refuses only when it would create the part.
- **Compare (breaking for struct literals of `CompareOptions`):** an original under Restrict Editing that forbids body edits is refused unless the new `CompareOptions::acknowledge_protection` is set (CLI: `compare --acknowledge-protection`). The redline no longer comes back with protection acknowledged on the caller's behalf. `author` is validated like other writers' text (XML 1.0, no control characters, at most `MAX_AUTHOR_CHARS` = 255 characters), and the redline is committed through the transaction path. Code that builds `CompareOptions` with a struct literal needs the new field (`acknowledge_protection: false` keeps the safe default).
- **Comments:** a span captured under `view=original` or `view=all` no longer anchors a comment (`Span::comment`, `comments::add_comment`); it refuses `historical_view_target` like replace does, so a comment cannot anchor on deleted text.
- **Comment ids:** allocating a comment `w:id` after an id at the 64-bit limit no longer overflows; past the signed 32-bit range the lowest free id is reused as before. A new `w14:paraId` that would reach `0x80000000` refuses `invalid_argument(para_id_exhausted)` instead of writing an id Word rejects.
- **Revisions:** `Revisions::accept_all` / `reject_all` refresh the caller's snapshot from the staged trees before committing, so they can no longer report an error after the document has changed.
- **Markdown and HTML to Word, hostile input:**
  - Inline HTML closed across Markdown emphasis (`<b>*x</b>*`) no longer panics; the Markdown spans close with the HTML span and reopen after it, so the text and its formatting are kept.
  - An unclosed inline HTML tag ends with its block instead of swallowing what follows (a nested list, display math).
  - Front matter indented with a non-ASCII space (U+00A0) no longer panics: YAML indentation is ASCII spaces only, anything else is content.
  - Structure nested deeper than 128 levels (`markdown::MAX_NESTING_DEPTH`: block quotes, lists, emphasis, links, images, HTML elements, math groups) is refused with `ResourceLimit` (`markdown_nesting_too_deep`) before anything changes, instead of overflowing the stack.
  - Tables are narrowed to Word's 63 columns: spans past the last column are cut back and cells past it merge into it, with a `table_columns_clamped` warning. 300 cells with `colspan="1000"` took tens of seconds and wrote 300,000 columns.
  - HTML browsers do not display (`head`, `title`, `style`, `script`, `template`, `noscript`, …) is dropped with its content and an `html_element_skipped` warning per element kind, instead of becoming body paragraphs. Script, style and title text is no longer read as markup.
- **Markdown and HTML to Word, untrusted input:**
  - Local images are read only when `MarkdownOptions::resource_dir` is set, and only from inside it: absolute paths, drive and UNC prefixes, `file:` URLs and paths leading out of it (`..`, symbolic links) are refused with an `image_outside_resource_dir` warning. **Behaviour change:** with `resource_dir: None` (the default) nothing is read from disk; pass images in `images` or set the directory.
  - Image warnings quote the reference as written, never the resolved path or the operating system's error.
  - Pictures larger than the document's `Limits::max_part_bytes` (`data:` URIs measured before decoding) are left out with an `image_too_large` warning.
  - A damaged or truncated picture is left out with an `image_unreadable` warning instead of failing the conversion; a resolution of zero is taken as 96 dpi (`Image::layout_dimensions`).
  - Links keep their target only for `http`, `https`, `mailto`, `tel`, `#fragment` and relative references; other schemes (`javascript:`, `file:`, ...) keep their text without the link (`link_scheme_skipped`).
  - The template's package thumbnail (a picture of its first page) is left out of replace-body conversions and listed in `removed_parts`.
  - `images` keys and references match alike: `./a.png` finds `a.png`, `a%20b.png` finds `a b.png`, and the other way round.
  - Very deeply nested block quotes no longer overflow the quote depth.
- **Hyperlinks:** `add_hyperlink` refuses `javascript:` and `vbscript:` addresses and addresses with control characters (`invalid_argument`).
- **Images:** `Image::from_vec` characterises owned bytes without copying them.

### Command line

- `compare --acknowledge-protection` redlines an original under Restrict Editing; without it such an original is refused (exit 3).
- `from-markdown` reads images from the current directory for standard input, as before, and only from inside the Markdown file's directory or `--resource-path`.
- **Panics** are reported as an internal error (exit 70, code `internal`, reason `internal_panic`, the JSON envelope under `--json`) instead of Rust's panic message and exit 101.
- **Argument errors under `--json`** are the JSON error envelope (`invalid_argument`, exit 2) instead of plain text. `--help` and `--version` still print text and exit 0.
- **`replace --all`** exits 3 and writes nothing when it replaces nothing (no match: `target_not_found`; every match refused: the first refusal), as `replace` without `--all` does. Reports give `replaced_count` and `refused_count`.
- **`layout --font`** with an unreadable file is an I/O error (exit 5), not an invalid argument.

### Node.js

- **Budgets before copies:** the source, the reference document and images are checked against the `limits` budgets by their size before they are copied into WebAssembly memory. The reference is also held to the package budget `max_archive_bytes` up front.
- **`markdownStyles(reference?, styles?, options?)`** takes options; `limits`, `strictPackage` and `redact` apply as in the other functions.
- **`now`** must be a finite number of milliseconds from 0 to the end of the year 9999; anything else is refused (`invalid_argument`, `invalid_options`).
- **Arguments:** a source that is not a string is refused (`invalid_source`); the `reference` argument of `inspectReference` and `markdownStyles` must be a `Uint8Array` (a `Buffer` is one).
- **Builds** of the npm package use `Cargo.lock` (`--locked`), and CI builds wasm-bindgen-cli from crates.io instead of downloading an unverified binary.
- **Panics** (bugs) print their message on `console.error`. The call still throws `RuntimeError: unreachable`; load the module again before the next call (README).

## 0.1.0 (2026-10-21)

The first release. Three packages:
- `docrs`, the library;
- `docrs-cli`, the `docrs` command;
- `docrs-wasm` on npm, WebAssembly for Node.js.

### Library

- **Behaviour:** checked operation by operation against the Python implementation it adapts, used as a test oracle; the differences are documented in `docs/compatibility.md`.
- **Package and XML:**
  - Guarded package reading: ZIP central-directory validation and resource limits (`Limits`). Irregularities Word tolerates (Office `[trash]` members, data in ZIP directory entries) are refused by default; an opt-in `Tolerance` sets them aside and reports what it set aside.
  - A lossless, namespace-preserving XML arena.
  - Patch saves that rewrite only changed parts. A no-op save is byte-identical.
- **Editing:**
  - Search with normalisation, outlines and stories.
  - Replace one or all occurrences.
  - Tracked changes (make, list, accept, reject) and comments.
  - Comparison into a verified redline.
  - Appending or inserting documents with styles, numbering, media, links and bookmarks reconciled.
  - Numbering, tables, sections, headers and footers, fields, bookmarks, links, footnotes, content controls, document properties and protection.
- **Handles and refusals:** handles carry freshness evidence, and refusals are typed and atomic.
- **Markdown and HTML to Word** (`markdown` feature): Markdown or editor HTML into a reference template.
  - It takes the template's styles, numbering, page setup, headers, footers and the branding its sample content shows.
  - It leaves out the template's sample body, comments and private document properties.
  - Template inspection (`markdown::inspect`), serverless resource presets and redacted warnings.
- **Table of contents page numbers:** computed by estimating Word for Mac's layout from bundled font metrics (numbers only, no font software). Fonts embedded in the template are used when present. On 84 test documents, 95.8% of entries match Word's own update exactly and none is more than one page off.

### Command line

- `docrs` with:
  - `diagnose`, `outline`, `find`, `replace`, `diff`, `text-diff`, `pending`;
  - `revisions`, `resolve`, `compare`, `append`, `insert-from`;
  - `from-markdown`, `markdown-reference`, `markdown-styles`, `markdown-inspect`;
  - `layout`, `roundtrip`.
- `--json` reports (`schema_version: 1`), stable exit codes and atomic outputs.

### Node.js

- `fromMarkdown`, `fromHtml`, `inspectReference`, `markdownStyles`, `referenceDocument` and `version`, compiled to WebAssembly with TypeScript definitions. No native code; conversions use no file system or network (the module reads only its own `.wasm` when loaded).
