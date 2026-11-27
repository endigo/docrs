#!/bin/sh
# Checks for the Word oracle tools (macOS; not in CI): tools/pdf/checks/run.sh
set -e
root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$root/target/pdf-oracle/checks
mkdir -p "$work"
python3 -I "$root/tools/pdf/checks/test_compare.py"
python3 -I "$root/tools/pdf/checks/test_environment.py"

swiftc -O "$root/tools/pdf/pdflines.swift" -o "$work/pdflines"
swiftc -O "$root/tools/pdf/checks/tight.swift" -o "$work/tight"
"$work/tight" "$work/tight.pdf"
"$work/pdflines" "$work/tight.pdf" > "$work/tight.json"
jq -e '[.pages[0].lines[].baseline] == [100, 120, 140, 160, 180]' "$work/tight.json" > /dev/null
jq -e '[.pages[1].lines[] | .baseline] as $b
    | ($b | length) > 4
    and ([range(1; $b | length) | $b[.] - $b[. - 1] | (. - 11.2) | fabs < 0.02] | all)
    and ([.pages[].lines[] | has("baselines")] | any | not)' "$work/tight.json" > /dev/null

seq 1 60 | sed 's/.*/Line number & of the test document, with some words./' > "$work/pitch.txt"
cupsfilter "$work/pitch.txt" > "$work/pitch.pdf" 2> /dev/null
"$work/pdflines" "$work/pitch.pdf" > "$work/pitch.json"
jq -e '[.pages[0].lines[] | .baselines // [.baseline]] | add as $b
    | ($b | length) == 60 and $b[0] == 28.72
    and ([range(1; 60) | $b[.] - $b[. - 1] | (. - 12) | fabs < 0.02] | all)' "$work/pitch.json" > /dev/null
echo "pdflines: Core Text lines, 0.8 line height and a 12 pt merged page: ok"
