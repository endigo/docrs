# Changelog

## Unreleased

### Library

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

- `from-markdown` reads images from the current directory for standard input, as before, and only from inside the Markdown file's directory or `--resource-path`.

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
