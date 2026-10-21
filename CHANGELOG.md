# Changelog

## Unreleased

### Library

- **Markdown and HTML to Word, hostile input:**
  - Inline HTML closed across Markdown emphasis (`<b>*x</b>*`) no longer panics; the Markdown spans close with the HTML span and reopen after it, so the text and its formatting are kept.
  - An unclosed inline HTML tag ends with its block instead of swallowing what follows (a nested list, display math).
  - Front matter indented with a non-ASCII space (U+00A0) no longer panics: YAML indentation is ASCII spaces only, anything else is content.
  - Structure nested deeper than 128 levels (`markdown::MAX_NESTING_DEPTH`: block quotes, lists, emphasis, links, images, HTML elements, math groups) is refused with `ResourceLimit` (`markdown_nesting_too_deep`) before anything changes, instead of overflowing the stack.
  - Tables are narrowed to Word's 63 columns: spans past the last column are cut back and cells past it merge into it, with a `table_columns_clamped` warning. 300 cells with `colspan="1000"` took tens of seconds and wrote 300,000 columns.
  - HTML browsers do not display (`head`, `title`, `style`, `script`, `template`, `noscript`, …) is dropped with its content and an `html_element_skipped` warning per element kind, instead of becoming body paragraphs. Script, style and title text is no longer read as markup.

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
