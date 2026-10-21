#!/bin/sh
# List what still keeps a version from publishing, and exit 1 if anything
# does: TODO(release) markers, npm "private", an undated CHANGELOG section.
#   tools/release/ready.sh VERSION
set -u
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
open=0
grep -n "TODO(release)" Cargo.toml crates/*/Cargo.toml && open=1
grep -n '"private": true' crates/docrs-wasm/js/package.json && open=1
grep -q "^## $1 (unreleased)" CHANGELOG.md && { echo "CHANGELOG.md: date the $1 section"; open=1; }
[ $open = 0 ] && echo "nothing; ready to publish (RELEASING.md)"
exit $open
