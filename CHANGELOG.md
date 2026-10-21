# Changelog

## Unreleased

### Command line

- **Panics** are reported as an internal error (exit 70, code `internal`, reason `internal_panic`, the JSON envelope under `--json`) instead of Rust's panic message and exit 101.
- **Argument errors under `--json`** are the JSON error envelope (`invalid_argument`, exit 2) instead of plain text. `--help` and `--version` still print text and exit 0.
- **`replace --all`** exits 3 and writes nothing when it replaces nothing (no match: `target_not_found`; every match refused: the first refusal), as `replace` without `--all` does. Reports give `replaced_count` and `refused_count`.
- **`layout --font`** with an unreadable file is an I/O error (exit 5), not an invalid argument.

### Node.js

- **Budgets before copies:** the source, the reference document and images are checked against the `limits` budgets by their size before they are copied into WebAssembly memory. The reference is also held to the package budget `max_archive_bytes` up front.
- **`markdownStyles(reference?, styles?, options?)`** takes options; `limits`, `strictPackage` and `redact` apply as in the other functions.
- **`now`** must be a finite number of milliseconds from 0 to the end of the year 9999; anything else is refused (`invalid_argument`, `invalid_options`).
- **Arguments:** a source that is not a string is refused (`invalid_source`); the `reference` argument of `inspectReference` and `markdownStyles` must be a `Uint8Array` (a `Buffer` is one).
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
