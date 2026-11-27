#!/bin/sh
# Convert the corpus (tools/pdf/corpus.tsv) and the PDF fixtures
# (tests/fixtures/pdf/cases.tsv) and lay each out; with --word, export each
# through Microsoft Word (macOS), dump Word's lines and compare the pages
# Word starts each paragraph on with ours.
#   tools/pdf/check.sh OUT_DIR [--word] [--png] [--update-fixtures]
# OUT_DIR gets <case>.docx, <case>.layout.json, baseline.tsv and, with
# --word, <case>.word.lines.json, environment.json, summary.tsv and
# summary.md. Word's PDFs (and with --png their pages as PNG) go to
# target/pdf-oracle/. --update-fixtures copies the fixtures' Word lines to
# tests/fixtures/pdf/<case>/word.lines.json.
set -e
out=${1:?usage: tools/pdf/check.sh OUT_DIR [--word] [--png] [--update-fixtures]}
shift
word=no png=no update=no
for arg; do
    case $arg in
        --word) word=yes ;;
        --png) png=yes ;;
        --update-fixtures) update=yes ;;
        *) echo "unknown option $arg" >&2; exit 2 ;;
    esac
done
root=$(cd "$(dirname "$0")/../.." && pwd)
"$root/tools/pdf/corpus.sh" "$out"
out=$(cd "$out" && pwd)
docrs=$root/target/release/docrs
tab=$(printf '\t')
cases=$root/tests/fixtures/pdf/cases.tsv

tail -n +2 "$cases" | while IFS=$tab read -r case source reference font options; do
    [ "$options" = "-" ] && options=
    # $options is a list of flags.
    # shellcheck disable=SC2086
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
python3 -I "$root/tools/pdf/environment.py" "$out" > "$out/environment.json"

# Word is sandboxed: it reads and writes files in its own container.
staging=$HOME/Library/Containers/com.microsoft.Word/Data/tmp/docrs-pdf
mkdir -p "$staging"
for docx in "$out"/*.docx; do
    base=$(basename "$docx" .docx)
    cp "$docx" "$staging/$base.docx"
    rm -f "$staging/$base.pdf"
    osascript "$root/tools/markdown/word2pdf.applescript" "$staging/$base.docx" "$staging/$base.pdf" > /dev/null
    mv "$staging/$base.pdf" "$oracle/$base.pdf"
    rm -f "$staging/$base.docx"
    "$oracle/pdflines" "$oracle/$base.pdf" > "$out/$base.word.lines.json"
    if [ $png = yes ]; then
        "$oracle/pdfpages" "$oracle/$base.pdf" "$oracle/png/$base" 1.0 > /dev/null
    fi
    echo "$base: $(grep -c '"lines": \[' "$out/$base.word.lines.json") pages in Word"
done
python3 -I "$root/tools/pdf/compare.py" "$out"

if [ $update = yes ]; then
    tail -n +2 "$cases" | while IFS=$tab read -r case _; do
        mkdir -p "$root/tests/fixtures/pdf/$case"
        cp "$out/pdf-$case.word.lines.json" "$root/tests/fixtures/pdf/$case/word.lines.json"
    done
    echo "updated tests/fixtures/pdf/*/word.lines.json"
fi
