# Word oracle for the corpus pass

Word's rendering of the 84 conversions in `tools/pdf/corpus.tsv` (the 20 templates in `tests/fixtures/markdown/templates/v2` plus `examples/markdown/reference.docx`, × `showcase.md`, `memo.md`, `tiptap/core.html` and `tiptap/nested.json`), plus the 9 PDF fixtures (`pdf-*`).

Each case directory holds:
- `input.docx`: the DOCX docrs wrote, the one Word opened;
- `word.pdf`: Word's **File → Save As → PDF** of it, through `tools/markdown/word2pdf.applescript`;
- `word.lines.json`: `tools/pdf/pdflines.swift` run on `word.pdf`.

How it was made:
- **docrs:** commit `933266e` (`pdf/engine` after #50), with `tools/pdf/check.sh OUT --word --png`. `environment.json` says `dirty: true` only because of an untracked `target` symlink in that worktree; no source differed.
- **Word:** Word for Mac 16.115.2 (16.115.26112215), on macOS 27.2.1.
- **Fonts:** `environment.json` lists every font family installed, with file hashes. That includes Aptos, Arial, Calibri, Cambria, Cambria Math, Consolas, Courier New, Garamond, Georgia, Inter, MS Gothic, Symbol, Times New Roman and Wingdings.

Result at 933266e (`compare.py`):
- 93 documents;
- 2,967 paragraphs compared, 2,835 on Word's page (95.6%);
- 70 documents with every compared paragraph on Word's page;
- 85 documents with Word's page count.

The DOCX only needs rebuilding (`tools/pdf/corpus.sh`, the hashes in `corpus.tsv`) if conversion changes. Layout changes don't affect it.
