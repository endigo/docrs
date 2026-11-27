# PDF engine baselines

What the layout code decides today, recorded before the PDF engine changes `crates/docrs/src/layout`, and tools to compare our lines and pages with Word for Mac's.

## Layout snapshots (CI)

`crates/docrs-cli/tests/layout_snapshots.rs` runs `docrs --json layout` and compares the output with `tests/fixtures/layout/`:

| Snapshot | Document |
| -- | -- |
| `templates/*.json`, `templates/v2/*.json` | each reference template in `tests/fixtures/markdown/templates` (47) |
| `examples/*.json` | each DOCX in `examples/markdown` |
| `corpus/<case>.json` | each conversion in `corpus.tsv` (84) |
| `pdf/<case>.json` | each PDF fixture in `tests/fixtures/pdf/cases.tsv` |

A snapshot is the layout JSON without `schema_version`, with one paragraph per line: page, `y` (points from the page top, rounded to 0.01), whether it starts a bookmark (`heading`), a table row end, and its first 40 characters. It also holds the page count and the missing and embedded fonts. Layout reads only the bundled metrics table and the fonts named on the command line, so the output is the same on every machine.

A PR that should not move pagination leaves every snapshot unchanged. When a change is intended:

```
DOCRS_UPDATE_SNAPSHOTS=1 cargo test -p docrs-cli --test layout_snapshots
```

rewrites the snapshots, deletes those no document makes any more and rewrites the hashes in `corpus.tsv`. Review the diff.

## Corpus and baseline

`corpus.tsv` lists 84 conversions with the SHA-256 of each template and input: 21 templates (the 20 in `tests/fixtures/markdown/templates/v2` and `examples/markdown/reference.docx`) × 4 inputs. This is the shape of the October run in `tools/layout/README.md` (95.8% of 1,344 TOC entries), but that run's inputs were not kept. Its 54-entry demo is not in the repository, so the two corpora's figures are not comparable. The four inputs are the samples the tests convert:

- `examples/markdown/showcase.md` (Markdown, the CLI and determinism tests);
- `examples/markdown/memo.md` (Markdown);
- `tests/fixtures/markdown/tiptap/core.html` (editor HTML, `tests/tiptap.rs`);
- `tests/fixtures/markdown/tiptap/nested.json` (TipTap JSON, `tests/tiptap.rs`).

Each is converted with `--toc`.

```
tools/pdf/corpus.sh OUT_DIR
```

checks the hashes, converts and lays out each document, and writes `OUT_DIR/baseline.tsv`. For each case it has a `pages` row with the page count, then one `heading N` row per bookmarked paragraph, with its page and first 40 characters. `baseline.tsv` here is that output for the commit that added it; later PRs diff against it. It needs jq.

## Word oracle (macOS with Word; not in CI)

```
tools/pdf/check.sh OUT_DIR [--word] [--png] [--update-fixtures]
```

1. Runs `corpus.sh OUT_DIR`, then converts the PDF fixtures (`pdf-<case>.docx`) and lays them out (`.layout.json`). Inter cases pass `tests/fixtures/fonts/inter` to layout with `--font`.
2. With `--word`:
   - records the Word version and build, the macOS version and build, and each installed font file (path, faces, size, SHA-256) whose family the documents name, in `OUT_DIR/environment.json` (`environment.py`);
   - exports each DOCX to PDF through Word, one at a time (`tools/markdown/word2pdf.applescript`, staged in Word's sandbox container);
   - dumps Word's lines with `pdflines.swift` (`OUT_DIR/<case>.word.lines.json`);
   - compares paragraph start pages (`compare.py`) and writes `OUT_DIR/summary.tsv` and `summary.md`.
3. `--png` renders Word's pages (`tools/markdown/pdfpages.swift`).
4. `--update-fixtures` copies the PDF fixtures' Word lines to `tests/fixtures/pdf/<case>/word.lines.json`.

Word's PDFs and PNGs go to `target/pdf-oracle/` (ignored by git). They embed subsets of Microsoft fonts, so they are never committed. Only the fixtures' line geometry is committed.

Before a run, install Inter from `tests/fixtures/fonts/inter` (Font Book). Word downloads Aptos when a document uses it; `environment.json` shows whether it was there.

**Comparison.** Letters and digits only are compared (lower-cased), so line breaks, list labels and quote styles don't matter. Each paragraph's first 24 of them are looked for in Word's text, after the previous paragraph's match; the page of the first character found is Word's start page. Not counted: table row ends, paragraphs with fewer than 4 letters and digits, and paragraphs not found (`unmatched`, for example a TOC entry whose field code is in our text). The share per document is agreeing paragraphs over counted ones.

**pdflines.** `swiftc -O tools/pdf/pdflines.swift -o pdflines && ./pdflines FILE.pdf` prints, per page, its size and its lines. Each line has its text, `x`, `top`, `width` and `height` (PDFKit's line selection bounds), `baseline` and `page`. All values are points from the page's top-left corner, rounded to 0.01. The baseline comes from the content stream's text matrix. It is null when the line's text is drawn from a form XObject.
