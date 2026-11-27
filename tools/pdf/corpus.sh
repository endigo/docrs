#!/bin/sh
# Lay out tools/pdf/corpus.tsv and write OUT_DIR/baseline.tsv (tools/pdf/README.md).
set -e
out=${1:?usage: tools/pdf/corpus.sh OUT_DIR}
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
cargo build -q --release --manifest-path "$root/Cargo.toml" -p docrs-cli
docrs=$root/target/release/docrs
tab=$(printf '\t')

check_hash() {
    now=$(shasum -a 256 "$root/$2" | cut -d' ' -f1)
    if [ "$now" != "$3" ]; then
        echo "$1: $2 hashes $now, tools/pdf/corpus.tsv says $3" >&2
        exit 1
    fi
}

baseline=$out/baseline.tsv
printf 'case\titem\tpage\ttext\n' > "$baseline"
tail -n +2 "$root/tools/pdf/corpus.tsv" | while IFS=$tab read -r case template template_sha input input_sha; do
    check_hash "$case" "$template" "$template_sha"
    check_hash "$case" "$input" "$input_sha"
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
