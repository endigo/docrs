PR #57 round 2 (head 4d49096): default optical size, Word's synthesized bold and italic. Same commands and fonts as round 1 (../README.txt).

Made with docrs at commit 4d49096 (branch pdf/pdf-text), release build:
  cargo build --release -p docrs-cli
  D=target/release/docrs
Run from the repository root. fonts/ is a directory holding exactly the three
font files listed below (copies of the repo files):
  mkdir fonts
  cp 'tests/fixtures/fonts/inter/Inter[opsz,wght].ttf' \
     tests/fixtures/fonts/liberation/LiberationSerif-Regular.ttf \
     tests/fixtures/fonts/source-sans/SourceSans3-Regular.otf fonts/

  REF=tests/fixtures/pdf/references/inter.docx
  $D from-markdown converted.md --reference $REF --out converted.docx --font-dir fonts
  $D from-markdown tests/fixtures/pdf/sources/memo.md --reference $REF --out memo.docx --font-dir fonts
  $D from-html fontmix.html --reference $REF --out fontmix.docx --font-dir fonts
  $D pdf converted.docx --out converted.pdf --font-dir fonts
  $D pdf memo.docx --out memo.pdf --font-dir fonts
  $D pdf fontmix.docx --out fontmix.pdf --font-dir fonts

converted.md and fontmix.html are the sources (memo.md is in the repo at
tests/fixtures/pdf/sources/memo.md). The .docx files are the ones Word should
open; File > Save As > PDF, then compare with the docrs .pdf next to them.
Install the three fonts (Font Book) before opening them in Word.

Fonts:
- Inter[opsz,wght].ttf
    repo path: tests/fixtures/fonts/inter/Inter[opsz,wght].ttf
    name id 4 (full name): Inter Regular
    name id 5 (version):   Version 4.001;git-66647c0bb
    876576 bytes, SHA-256 29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031
- LiberationSerif-Regular.ttf
    repo path: tests/fixtures/fonts/liberation/LiberationSerif-Regular.ttf
    name id 4 (full name): Liberation Serif
    name id 5 (version):   Version 2.1.5
    393576 bytes, SHA-256 058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74
- SourceSans3-Regular.otf
    repo path: tests/fixtures/fonts/source-sans/SourceSans3-Regular.otf
    name id 4 (full name): Source Sans 3
    name id 5 (version):   Version 3.052;hotconv 1.1.0;makeotfexe 2.6.0
    334924 bytes, SHA-256 08df266400933d3178d081a45f94a08814c3e55b4b7dd2e0ff69cb1329f13ab6

What to compare (also in the PR body):
1. converted: line breaks of the two paragraphs, list label and text x, the
   second list starting at 1.
2. memo: every line break and the page break; page 1 ends after "Personal
   items...", and the "The fit-out starts..." paragraph takes 3 lines.
3. fontmix: Inter 6.5 pt regular vs bold widths, synthesized italic (Inter)
   and synthesized italic/bold (Liberation Serif), the Source Sans line width.
