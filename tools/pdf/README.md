# PDF engine baselines

What the layout code decides today, recorded before the PDF engine changes `crates/docrs/src/layout`, and tools to compare our lines and pages with Word for Mac's.

## Layout snapshots (CI)

`crates/docrs-cli/tests/layout_snapshots.rs` runs `docrs --json layout` and compares the output with `tests/fixtures/layout/`:

| Snapshot | Document |
| -- | -- |
| `templates/*.json`, `templates/v2/*.json` | each reference template in `tests/fixtures/markdown/templates` (45) |
| `examples/*.json` | each DOCX in `examples/markdown` |
| `corpus/<case>.json` | each conversion in `corpus.tsv` (84) |
| `pdf/<case>.json` | each PDF fixture in `tests/fixtures/pdf/cases.tsv` |

A snapshot is the layout JSON without `schema_version`, with one paragraph per line: page, `y` (points from the page top, rounded to 0.01), whether it starts a bookmark (`heading`), a table row end, and its first 40 characters as Word shows them (field results, not codes). It also holds the page count and the missing and embedded fonts. Layout reads only the bundled metrics table and the fonts named on the command line, so the output is the same on every machine.

A PR that should not move pagination leaves every snapshot unchanged. When a change is intended:

```
DOCRS_UPDATE_SNAPSHOTS=1 cargo test -p docrs-cli --test layout_snapshots
```

rewrites the snapshots, deletes those no document makes any more and rewrites the hashes in `corpus.tsv`. Review the diff.

## Corpus

`corpus.tsv` lists 84 conversions with the SHA-256 of each template and input: 21 templates (the 20 in `tests/fixtures/markdown/templates/v2` and `examples/markdown/reference.docx`) × 4 inputs. This is the shape of the October run in `tools/layout/README.md` (95.8% of 1,344 TOC entries), but that run's inputs were not kept. Its 54-entry demo is not in the repository, so the two corpora's figures are not comparable. The four inputs are the samples the tests convert:

- `examples/markdown/showcase.md` (Markdown, the CLI and determinism tests);
- `examples/markdown/memo.md` (Markdown);
- `tests/fixtures/markdown/tiptap/core.html` (editor HTML, `tests/tiptap.rs`);
- `tests/fixtures/markdown/tiptap/nested.json` (TipTap JSON, `tests/tiptap.rs`).

Each is converted with `--toc`.

```
tools/pdf/corpus.sh OUT_DIR
```

checks the hashes, converts and lays out each document, and writes `OUT_DIR/baseline.tsv`: each case's page count, then each bookmarked paragraph's page and text. It needs jq. The corpus snapshots under `tests/fixtures/layout/corpus` hold the same pages, so later PRs compare against those.

## Word oracle (macOS with Word; not in CI)

```
tools/pdf/check.sh OUT_DIR [--fixtures-only] [--word] [--png] [--update-fixtures]
```

OUT_DIR must be new or empty. With `--word`, Word must have no document open: the export closes the front document without saving.

1. Runs `corpus.sh OUT_DIR` (not with `--fixtures-only`), then converts the PDF fixtures (`pdf-<case>.docx`) and lays them out (`.layout.json`). Inter cases pass `tests/fixtures/fonts/inter` to layout with `--font`.
2. With `--word`:
   - exports each DOCX to PDF through Word, one at a time (`tools/markdown/word2pdf.applescript`, staged in Word's sandbox container);
   - dumps Word's lines with `pdflines.swift` (`OUT_DIR/<case>.word.lines.json`);
   - records the docrs commit (and whether the tree was dirty), the Word version and build, the macOS version and build, and each installed font file (path, faces, size, SHA-256) whose family the documents name, in `OUT_DIR/environment.json` (`environment.py`). Paths under the home directory are written `~/…`, and no user or host name is recorded;
   - compares paragraph start pages (`compare.py`) and writes `OUT_DIR/summary.tsv` and `summary.md`.
3. `--png` renders Word's pages (`tools/markdown/pdfpages.swift`).
4. `--update-fixtures` copies the PDF fixtures' Word lines to `tests/fixtures/pdf/<case>/word.lines.json`, and `environment.json` to `tests/fixtures/pdf/environment.json`.

`python3 -I tools/pdf/compare.py OUT_DIR` reruns the comparison on an existing directory.

Word's PDFs and PNGs go to `target/pdf-oracle/` (ignored by git). They embed subsets of Microsoft fonts, so they are never committed. Only the fixtures' line geometry is committed.

Before a run, install Inter from `tests/fixtures/fonts/inter` (Font Book). Word downloads Aptos when a document uses it; `environment.json` shows whether it was there.

**Comparison.** Paragraphs are matched to Word's text in document order: each match starts after the previous one. Letters and digits only are compared (lower-cased), so line breaks, list labels on their own line, table cells Word splits or joins, and quote styles don't matter. A paragraph's first 24 of them (12 in a table cell) are looked for; the page of the first one found is Word's start page. Word's contents lines (dot leaders ending in a number) are left out of its text. Not compared, and counted by reason:

- `contents`: our contents entries (text ending in a tab and a page number);
- `empty`, `short`: fewer than 4 letters and digits;
- `table cell`: a short paragraph in a table cell (before a row end, from the row's top down);
- `not found`: not in Word's text after the previous match.

The share per document is agreeing paragraphs over compared ones, `n/a` when none was compared. The summary warns about documents where over 20% of the paragraphs looked for were not found.

**pdflines.** `swiftc -O tools/pdf/pdflines.swift -o pdflines && ./pdflines FILE.pdf` prints, per page, its size and its lines. Each line has its text, `x`, `top`, `width` and `height` (PDFKit's line selection bounds), `baseline` and `page`. All values are points from the page's top-left corner, rounded to 0.01. The baseline comes from the content stream's text matrix: the commonest text origin in the lower half of the line's box, the lowest on a tie. It is null when the line's text is drawn from a form XObject. When several origins fall in the box, `baselines` lists them all, top down: PDFKit sometimes merges lines into one (a `cupsfilter` page comes back as one 723 pt line).

`tools/pdf/checks/run.sh` checks `compare.py` (unit tests) and `pdflines` (Core Text lines 20 pt apart, a paragraph at 0.8 line height, a `cupsfilter` page at a 12 pt pitch). It needs macOS, swiftc and jq.
