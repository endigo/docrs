# Layout metrics

`crates/docrs/src/layout` estimates where Word for Mac breaks lines and pages, so a converted document's table of contents can show page numbers without updating fields. Per font face it needs the advance width of each character, the vertical metrics (ascent, descent, line gap) and the pair kerning.

## Font metrics table

`make_font_metrics.py` writes `crates/docrs/src/layout/font_metrics.bin`. It contains:

- **units per em** for each face;
- **hhea ascender, descender and line gap**;
- **average character width**;
- **advance widths** for 367 code points: Basic Latin, Latin-1, Latin Extended-A, general punctuation and a few symbols. Symbol fonts (Symbol, Wingdings 1–3, Webdings) get U+F020–F0FF instead, where bullet characters land;
- **kerning pairs** from the font's `kern` table, between printable ASCII, the no-break space, the typographic quotes and dashes, and the ellipsis (text faces only).

It contains numbers only. No font file, outline or glyph data is shipped. This follows the maintainer's decision of 2026-10-20: tables measured from the fonts references actually name, Microsoft fonts included, may be shipped; the fonts themselves may not. The table holds 410 faces of 205 families: 350 KB compressed, 235 KB of it kerning.

Faces are read in Word for Mac's order of precedence:

1. system fonts (`/System/Library/Fonts/Supplemental`, `/System/Library/Fonts`, `/Library/Fonts`);
2. Word's own (`Microsoft Word.app/Contents/Resources/DFonts`);
3. Office cloud fonts already downloaded (`~/Library/Group Containers/UBF8T346G9.Office/FontCache/4/CloudFonts`);
4. any directories given on the command line. Liberation 2.1.5, under the SIL Open Font License, is passed this way, because reference documents made in LibreOffice name it.

Families Word measures at exactly Cambria's line height are ones it substitutes Cambria for (Liberation Mono, for example); they are left out of the table, so layout substitutes them too.

```
python3 tools/layout/make_font_metrics.py <liberation-fonts-ttf-2.1.5>
```

Word for Mac uses the system copy when both exist. Its Times New Roman, for example, has a line gap that Word's bundled copy lacks. Where Word's measured line height shows it uses another copy, that copy is taken instead. Symbol is the case in point: Word uses its bundled copy, not Apple's.

## Line heights

Word for Mac's single line height is the font's hhea `ascender + descender + lineGap`, times the font size. This was measured over 40 lines for Arial, Times New Roman, Courier New, Calibri, Cambria, Georgia, Aptos, Verdana, Segoe UI, Consolas, Tahoma and others, and matched to within 0.005 pt.

Word for Windows uses the OS/2 win metrics instead. So for fonts where the two differ (Aptos, Times New Roman), Windows paginates differently.

Some fonts don't follow the hhea rule. Helvetica measures 1.2007 em against an hhea total of 1.0, and several other Apple fonts deviate too. `line_heights.tsv` holds the line height Word was measured to use for every family in the table: one page (up to 14 lines) of 40 pt text per family, read back through AppleScript, about 0.001 em precise. Where that differs from the formula by more than 0.004 em, and no other copy of the font matches, the generator adjusts the face's line gap so the table gives the measured height.

## Rules measured in Word for Mac

Each rule below was measured with synthetic documents (the measuring scripts are not included). It is implemented in `flow.rs`, `model.rs` or `fonts.rs`; most have a test in `flow.rs` or `tests/layout.rs` (kerning and the unknown-font substitution do not yet).

- **Lines mixing fonts:** the line is as tall as the largest ascent plus line gap, plus the largest descent. For example, Calibri with a Symbol bullet is 1.274 em, where each font alone is at most 1.225.
- **Multiple line spacing** (for example 259/240): each font's extra height, (multiple − 1) × its single height, adds to its own ascent.
- **List labels:** a label counts only its ascent, plus the extra height of the line's text font.
- **Bottom of the page:** a line fits when its single-spaced height fits; the extra from multiple spacing may overflow the bottom margin. The same holds for the last line a keep-with-next chain needs.
- **Widow control:** applies unless a paragraph turns it off. When the setting is absent, Word acts as if it were on. Inside a table row that splits across pages too.
- **Justified text:** each space may shrink by up to 0.2375 of its width for the line to fit (measured between 0.235 and 0.240). Left-aligned text gets no shrink.
- **Table rows:** each row is taller by its top border's width (the table top border, or the inside horizontal one); the last row also by the bottom border.
- **Contextual spacing in cells:** a cell's last paragraph with contextual spacing has no space after.
- **Table style order:** a table style with table properties of its own that is based on a style defined later in `styles.xml` inherits nothing from it, and its unset cell margins are 0.
- **Kerning:** where `w:kern` turns it on at the run's size, kerning applies between neighbouring characters of a run in the same face.
- **Unknown fonts:** a font the font table declares roman, or doesn't list, is laid out in Cambria; any other declared font in Calibri.

## Ground truth

Word for Mac is driven through AppleScript in three ways:

- **Per paragraph:** `information type active end page number` and `vertical position relative to page`, compared with `docrs layout <file> --json`. This is slow, about 15 minutes for 2,500 paragraphs, so it is used on ranges.
- **Start pages:** the document is exported to PDF and each paragraph's start page is found from the page texts (PDFKit).
- **The table of contents:** Word updates the TOC (`update table of contents`), and its page numbers are compared entry by entry with the numbers the converter wrote. This is the accuracy measure.

Accuracy on 84 documents (21 reference templates × 4 inputs, 1,344 TOC entries), measured 2026-10-20:
- 95.8% of entries match Word exactly;
- no entry is more than one page off;
- every document with up to 6 entries matches exactly.

The remaining misses are in the 54-entry demo, where a row or line overflows the page bottom by less than a point after rounding drift.
