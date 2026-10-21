# docrs

A preservation-first DOCX editing library and CLI in Rust: guarded OPC
reads, lossless XML arenas, narrow patch saves, typed refusals. Markdown and
editor HTML convert to Word through a reference template.

* Compatibility: [docs/compatibility.md](docs/compatibility.md)
* Capability ledger: [docs/capabilities.md](docs/capabilities.md) · Case format: [docs/cases.md](docs/cases.md)
* Notices: [NOTICE.md](NOTICE.md) (MIT, upstream attribution) · Changes: [CHANGELOG.md](CHANGELOG.md)

## Install

| Use | Package | Install |
| -- | -- | -- |
| Rust library | [`docrs`](crates/docrs/README.md) | `cargo add docrs` (add `--features markdown` for Markdown/HTML to Word) |
| Command line (`docrs`) | [`docrs-cli`](crates/docrs-cli/README.md) | `cargo install docrs-cli --locked`, or a prebuilt binary from the release page |
| Node.js / Next.js | [`docrs-wasm`](crates/docrs-wasm/js/README.md) on npm (WebAssembly) | `npm install docrs-wasm` |

Rust 1.88 or newer. Node.js 18 or newer.

## Guarantees

* **Preservation.** Parts are edited in place in a namespace-preserving XML
  arena; unknown elements, attributes, comments, processing instructions and
  namespace declarations survive. A no-op save is the original archive byte
  for byte, and a save rewrites only the parts an operation changed.
* **Refusal atomicity.** Every operation validates first and commits last. A
  refusal or error leaves the document, its live handles and any existing
  output file unchanged.
* **Local freshness.** Spans and block handles carry evidence of what they
  addressed; using one after the text moved, or in another document, is
  refused instead of silently editing something else.
* **Unicode.** Offsets are Unicode scalar positions; normalized search
  (case-folding, typographic folds, whitespace) maps every match back to whole
  source characters.

## CLI

```bash
docrs diagnose input.docx
docrs outline input.docx --view current
docrs find input.docx "payment terms" --normalized
docrs replace input.docx --find 'payment terms' --with 'payment schedule' --out output.docx
docrs replace input.docx --find 'Acme' --with 'Acme Ltd' --all --out output.docx
docrs revisions input.docx
docrs resolve input.docx --accept [--author NAME] --out output.docx
docrs compare original.docx revised.docx --author Reviewer --out redline.docx
docrs append base.docx addition.docx [--styles import_renamed] --out combined.docx
docrs insert-from base.docx source.docx --start 'Clause 4' --end 'Clause 6' --anchor 'Clause 3' --out out.docx
docrs text-diff a.docx b.docx
docrs pending input.docx
docrs from-markdown notes.md --reference template.docx --out notes.docx
docrs from-markdown page.html --reference template.docx --out page.docx  # editor HTML (--from html)
docrs markdown-reference --out reference.docx   # starter template to restyle
docrs markdown-styles template.docx             # which style each element takes
docrs markdown-inspect template.docx            # what a conversion takes from it
docrs layout input.docx                         # estimated page of each paragraph
docrs diff a.docx b.docx
docrs roundtrip input.docx --out copy.docx
```

`docrs --help` lists every command and `docrs <command> --help` its options.
`--json` prints machine-readable reports (`schema_version: 1`). Exit codes:
0 success, 2 invalid argument, 3 refusal (ambiguous, not found, protected,
unsupported structure, boundary, relationship policy, malformed package),
4 resource limit, 5 I/O, 70 internal error. `--out` may equal the input;
outputs are written atomically. `--acknowledge-protection` (on `replace`,
`resolve`, `compare`, `append`, `insert-from` and `from-markdown`) proceeds through
Restrict Editing without removing it. `--trusted` uses the trusted-input
resource profile instead of the default budgets.

## Markdown to DOCX

`from-markdown` renders Markdown into a reference document the way pandoc's
`--reference-doc` does: the reference supplies every style, the page setup,
headers, footers and theme; the Markdown supplies the body. Each element
takes a style by role (pandoc's style names first, then Word's built-ins), so
both pandoc reference documents and ordinary Word templates work.

```bash
docrs from-markdown examples/markdown/showcase.md \
  --reference examples/markdown/reference.docx --out showcase.docx
docrs from-markdown examples/markdown/memo.md --reference examples/markdown/letterhead.docx \
  --placeholder '{{content}}' --out memo.docx
```

* Supported: CommonMark + GitHub tables, task lists, strikethrough,
  footnotes and alerts; definition lists; heading ids and internal links;
  sub/superscript; images (local or `data:`, scaled to the page); front
  matter (title block and document properties); `[TOC]`/`toc: true`;
  page breaks; LaTeX math as native Word equations.
* Missing styles are added and reported; `--strict-styles` refuses instead,
  `--style code_block="My Code"` maps a role, `--strict` refuses on any
  loss (missing image, unsupported HTML or LaTeX). Refusals change nothing.
* Lists take the reference's own List Bullet / List Number numbering
  (glyphs, colours, indents) where it defines them; `--converter-lists`
  uses the converter's definitions instead. Simple HTML blocks (tables
  with lists in cells, `<p></p>` blank lines) are converted too.
* Tables span the full text width; cells use a compact Table Contents
  style. The table of contents is written out with links to each heading
  and page numbers computed by estimating Word for Mac's layout, so it needs
  no field update (`--no-toc-numbers` leaves the numbers out;
  `--toc-page-numbers` asks Word to recompute them on open).
  `--page-numbers` adds a page-number footer when the template has none.
* Branding a template only shows in its sample content (hand-formatted
  headings, body text or tables, Word list buttons, custom bullet styles)
  is lifted into styles. Tables follow the style most sample tables use;
  a header fill or banding added by hand becomes a table style (`Template
  Table` when the template already has a `Table` style).
* A template whose first-page header draws a cover (and whose sample body
  leaves page 1 blank) keeps page 1 as the cover; `--no-cover-page` turns
  the cover off, `--cover-page` keeps page 1 as a cover whenever the first
  page is distinct.
* Reference documents with irregularities Word tolerates (Office `[trash]`
  members, data-free ZIP directory entries) are read with those set aside
  and reported; `--strict-package` refuses them as the library does by
  default.
* `--append` adds after the existing body; `--placeholder TEXT` replaces
  one marker paragraph; the default replaces the body.
* Example templates: `examples/markdown/reference.docx` and
  `letterhead.docx` (built by `cargo run -p docrs --features markdown
  --example markdown_reference -- examples/markdown/reference.docx`, which
  writes `letterhead.docx` next to it). `tools/markdown/check.sh DIR --word`
  re-exports every feature test and reopens the files with the pinned
  python-docx oracle, pandoc (when installed) and Microsoft Word for Mac.

## Node.js and Next.js (WebAssembly)

`crates/docrs-wasm` compiles the Markdown conversion to WebAssembly
for Node.js: no native binaries, no pandoc, no LibreOffice. Bytes go in and
out as `Uint8Array`; nothing touches the file system.

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked   # the version Cargo.lock pins; build.sh checks
crates/docrs-wasm/build.sh --pack     # pkg/ and an npm tarball
node --test crates/docrs-wasm/js/test.mjs
```

```ts
import { fromMarkdown } from "docrs-wasm";

const { docx, report, referenceSetAside } = fromMarkdown(markdown, {
  reference: templateBytes,            // optional .docx template
  toc: true,
  pageNumbers: true,
  images: { "chart.png": chartBytes }, // images the Markdown refers to
});
```

Options follow the CLI flags in camelCase, with `--no-…` flags as booleans
(`tocNumbers: false`, `templateLists: false`, `coverPage: false`), `styles`
as an object and images passed in `images`; unknown names are refused.
Errors are `Error`s with `code`, `reason`, `refusal` and `limit`. Also
exported: `fromHtml`, `inspectReference`, `markdownStyles`,
`referenceDocument`, `version`. In Next.js, call
it from a Route Handler on the Node.js runtime and add
`serverExternalPackages: ["docrs-wasm"]` to `next.config.js`; the
package README has a complete route.

## Library

```rust
use docrs::{Document, FindOptions, Limits, ReplaceOptions};

let mut document = Document::open_path("in.docx", &Limits::default())?;
let span = document.find_one("payment terms", &FindOptions::default())?;
document.replace(&span, "payment schedule", &ReplaceOptions::default())?;
document.save("out.docx")?;
```

Runnable examples in `crates/docrs/examples/`: `safe_replace`,
`review` (tracked change + comment), `compose` (append with style
reconciliation) and `redline` (verified compare), e.g.
`cargo run -p docrs --example redline -- a.docx b.docx out.docx`.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Every oracle parity suite plus the loader gate (requires .reference/ and .venv/;
# without LibreOffice the loader gate reports UNVERIFIED unless --require-libreoffice):
cargo run --release -p docrs-compat -- check --upstream-rev 3c0f9a5
```

The pinned Python project is a development oracle only; the crates are
Python-free at runtime.

## License

MIT. Portions are adapted from paper-docx; see [NOTICE.md](NOTICE.md).
