#!/bin/sh
# Export every Markdown feature test as a .docx and reopen each one with
# python-docx, pandoc and (with --word) Microsoft Word.
#   tools/markdown/check.sh OUT_DIR [--word] [--png]
set -e
out=${1:?usage: tools/markdown/check.sh OUT_DIR [--word] [--png]}
shift
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
rm -f "$out"/*.docx
DOCRS_MARKDOWN_EXPORT="$out" cargo test --manifest-path "$root/Cargo.toml" -q -p docrs --features markdown --test markdown
"$root/.venv/bin/python" -I "$root/tools/markdown/check_exports.py" "$out" "$@"
