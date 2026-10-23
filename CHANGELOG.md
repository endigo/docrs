# Changelog

## 0.3.4 (2026-10-23)

### Library

- A horizontal rule (`---` in Markdown, `<hr>` in HTML, `horizontalRule` in TipTap JSON) is one flat line across the text: an empty paragraph with a thin single bottom border in the text colour (`w:pBdr`, ¾ pt), 6 pt of space above and below, and a 1 pt paragraph mark so the line takes no more room than that. Every source writes the same paragraph. It was Word's VML "standard" rule (`v:rect` with `o:hr`), which Word draws as a shaded bar, two grey lines with a lighter fill between them, and which reads as a double line. No VML is written any more. A reference document cannot restyle the rule: its look is on the paragraph, not in a style.

- A table of contents (`toc: true`, front matter `toc`, `[TOC]`, TipTap's `tableOfContents`) is Word's table-of-contents block: the heading and every entry sit inside one content control (`w:sdt` with the `Table of Contents` document-part gallery), as Word and LibreOffice write one, so Word shows the TOC frame with its Update Table tab when the contents is clicked, treats it as the document's contents under References, and replaces heading and entries together. The field, its cached page numbers and the PAGEREF links are unchanged. The heading and entries are no longer direct body paragraphs: `Document::paragraphs` (python-docx's `document.paragraphs`) lists what follows them, as it does for a contents Word wrote.

- The default title above the table of contents is "Table of Contents", what Microsoft 365's References > Table of Contents inserts; it was "Contents". `toc_title` (CLI `--toc-title`, Node `tocTitle`, front matter `toc-title`) still overrides it.

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
