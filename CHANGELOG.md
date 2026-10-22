# Changelog

## Unreleased

### Library

- **Package and XML:**
  - A removed part's name can be used again, in the same edit or a later one. Before, adding a part under a removed name failed with the internal error `duplicate_part`. A part re-added with identical bytes saves as the original.
  - Opening a package is linear in its part count: 64k parts open in about 0.3 s instead of 48 s.
  - Copying deeply nested markup (deep copies, importing from another document as `append` and `insert-from` do) and comparing run properties during replace no longer recurse. With `--trusted`, 40k levels of nested content controls no longer abort `append` with a stack overflow.
- **Protection:** every inherited writer now honours Restrict Editing: authoring (paragraphs, runs, tables, cells, pictures, tab stops, `clear_body_content`), sections, headers and footers, styles and latent styles, and core properties. A forbidden edit refuses `document_protected` and changes nothing; `Document::acknowledge_protection` lets it through. `Run::mark_comment_range` counts as a comment edit. `Document::ensure_styles_part` refuses only when it would create the part.
- **Compare:** an original under Restrict Editing that forbids body edits is refused unless the new `CompareOptions::acknowledge_protection` is set (CLI: `compare --acknowledge-protection`). The redline no longer comes back with protection acknowledged on the caller's behalf. `author` is validated like other writers' text (XML 1.0, no control characters, at most `MAX_AUTHOR_CHARS` = 255 characters), and the redline is committed through the transaction path. Code that builds `CompareOptions` with a struct literal needs the new field (`acknowledge_protection: false` keeps the safe default).
- **Comments:** a span captured under `view=original` or `view=all` no longer anchors a comment (`Span::comment`, `comments::add_comment`); it refuses `historical_view_target` like replace does, so a comment cannot anchor on deleted text.
- **Comment ids:** allocating a comment `w:id` after an id at the 64-bit limit no longer overflows; past the signed 32-bit range the lowest free id is reused as before. A new `w14:paraId` that would reach `0x80000000` refuses `invalid_argument(para_id_exhausted)` instead of writing an id Word rejects.
- **Revisions:** `Revisions::accept_all` / `reject_all` refresh the caller's snapshot from the staged trees before committing, so they can no longer report an error after the document has changed.

### Command line

- `compare --acknowledge-protection` redlines an original under Restrict Editing; without it such an original is refused (exit 3).
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
