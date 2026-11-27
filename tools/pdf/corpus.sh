#!/bin/sh
# Convert every document of tools/pdf/corpus.tsv (with --toc), lay each out,
# and write OUT_DIR/baseline.tsv: each document's page count and the page of
# each heading (a paragraph that starts a bookmark).
#   tools/pdf/corpus.sh OUT_DIR
# Writes OUT_DIR/<case>.docx and OUT_DIR/<case>.layout.json too. Needs jq.
set -e
out=${1:?usage: tools/pdf/corpus.sh OUT_DIR}
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
cargo build -q --release --manifest-path "$root/Cargo.toml" -p docrs-cli
docrs=$root/target/release/docrs
tab=$(printf '\t')

baseline=$out/baseline.tsv
printf 'case\titem\tpage\ttext\n' > "$baseline"
tail -n +2 "$root/tools/pdf/corpus.tsv" | while IFS=$tab read -r case template template_sha input input_sha; do
    for pair in "$template $template_sha" "$input $input_sha"; do
        set -- $pair
        now=$(shasum -a 256 "$root/$1" | cut -d' ' -f1)
        if [ "$now" != "$2" ]; then
            echo "$case: $1 hashes $now, tools/pdf/corpus.tsv says $2" >&2
            exit 1
        fi
    done
    "$docrs" --json from-markdown "$root/$input" --reference "$root/$template" \
        --out "$out/$case.docx" --toc > /dev/null
    "$docrs" --json layout "$out/$case.docx" > "$out/$case.layout.json"
    jq -r --arg case "$case" '
        [$case, "pages", .pages, ""],
        ([.paragraphs[] | select(.heading)] | to_entries[]
            | [$case, "heading \(.key + 1)", .value.page, .value.text])
        | @tsv' "$out/$case.layout.json" >> "$baseline"
done
echo "$(tail -n +2 "$root/tools/pdf/corpus.tsv" | wc -l | tr -d ' ') documents; $baseline"
