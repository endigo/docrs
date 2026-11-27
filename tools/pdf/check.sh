#!/bin/sh
# Lay out the corpus and the PDF fixtures; with --word, compare with Word (tools/pdf/README.md).
set -e
usage="usage: tools/pdf/check.sh OUT_DIR [--fixtures-only] [--word] [--png] [--update-fixtures]"
out=${1:?$usage}
shift
corpus=yes word=no png=no update=no
for arg; do
    case $arg in
        --fixtures-only) corpus=no ;;
        --word) word=yes ;;
        --png) png=yes ;;
        --update-fixtures) update=yes ;;
        *) echo "$usage" >&2; exit 2 ;;
    esac
done
if [ -d "$out" ] && [ -n "$(ls -A "$out")" ]; then
    echo "$out is not empty; give a new directory" >&2
    exit 2
fi
if [ $word = yes ] && pgrep -xq "Microsoft Word" \
    && [ "$(osascript -e 'tell application "Microsoft Word" to count documents')" -gt 0 ]; then
    echo "Word has documents open; close them first (the export closes the front document unsaved)" >&2
    exit 2
fi
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
cargo build -q --release --manifest-path "$root/Cargo.toml" -p docrs-cli
if [ $corpus = yes ]; then
    "$root/tools/pdf/corpus.sh" "$out"
fi
docrs=$root/target/release/docrs
tab=$(printf '\t')
cases=$root/tests/fixtures/pdf/cases.tsv

tail -n +2 "$cases" | while IFS=$tab read -r case source reference font options; do
    [ "$options" = "-" ] && options=
    "$docrs" --json from-markdown "$root/$source" --reference "$root/$reference" \
        --out "$out/pdf-$case.docx" $options > /dev/null
    if [ "$font" = "-" ]; then
        "$docrs" --json layout "$out/pdf-$case.docx" > "$out/pdf-$case.layout.json"
    else
        "$docrs" --json layout "$out/pdf-$case.docx" --font "$root/$font" > "$out/pdf-$case.layout.json"
    fi
done
echo "$(tail -n +2 "$cases" | wc -l | tr -d ' ') fixtures converted into $out"
if [ $word = no ]; then
    exit 0
fi

oracle=$root/target/pdf-oracle
mkdir -p "$oracle"
swiftc -O "$root/tools/pdf/pdflines.swift" -o "$oracle/pdflines"
if [ $png = yes ]; then
    swiftc -O "$root/tools/markdown/pdfpages.swift" -o "$oracle/pdfpages"
    mkdir -p "$oracle/png"
fi

# Word is sandboxed: it reads and writes files in its own container.
staging=$HOME/Library/Containers/com.microsoft.Word/Data/tmp/docrs-pdf
mkdir -p "$staging"
staged=
trap 'if [ -n "$staged" ]; then rm -f "$staged" "${staged%.docx}.pdf"; fi' EXIT INT TERM
for docx in "$out"/*.docx; do
    base=$(basename "$docx" .docx)
    staged=$staging/$base.docx
    cp "$docx" "$staged"
    rm -f "$staging/$base.pdf"
    osascript "$root/tools/markdown/word2pdf.applescript" "$staged" "$staging/$base.pdf" > /dev/null
    mv "$staging/$base.pdf" "$oracle/$base.pdf"
    rm -f "$staged"
    "$oracle/pdflines" "$oracle/$base.pdf" > "$out/$base.word.lines.json"
    if [ $png = yes ]; then
        "$oracle/pdfpages" "$oracle/$base.pdf" "$oracle/png/$base" 1.0 > /dev/null
    fi
    echo "$base: $(grep -c '"lines": \[' "$out/$base.word.lines.json") pages in Word"
done
python3 -I "$root/tools/pdf/environment.py" "$out" > "$out/environment.json"
python3 -I "$root/tools/pdf/compare.py" "$out"

if [ $update = yes ]; then
    tail -n +2 "$cases" | while IFS=$tab read -r case _; do
        mkdir -p "$root/tests/fixtures/pdf/$case"
        cp "$out/pdf-$case.word.lines.json" "$root/tests/fixtures/pdf/$case/word.lines.json"
    done
    cp "$out/environment.json" "$root/tests/fixtures/pdf/environment.json"
    echo "updated tests/fixtures/pdf/*/word.lines.json and environment.json"
fi
