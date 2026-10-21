#!/bin/sh
# Everything a release needs, short of publishing:
#   tools/release/check.sh [--allow-dirty]
# Ends with what is still open (TODO(release) markers, npm "private");
# publishing itself is in RELEASING.md.
set -e
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
dirty=""
[ "$1" = "--allow-dirty" ] && dirty="--allow-dirty"

step() { printf '\n== %s\n' "$1"; }

step "version"
version=$(tools/release/version.sh)
echo "$version"
grep -q "^## $version" CHANGELOG.md || { echo "CHANGELOG.md has no $version section"; exit 1; }

step "format, lint, tests"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --quiet

step "minimum Rust version"
msrv=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)
RUSTUP_AUTO_INSTALL=0 cargo "+$msrv" check --locked --quiet -p docrs -p docrs-cli --all-features \
  || { echo "needs Rust $msrv: rustup toolchain install $msrv --profile minimal"; exit 1; }
echo "checks with $msrv"

step "crates.io (dry run)"
cargo publish --dry-run $dirty -p docrs -p docrs-cli

step "command line"
shown=$(./target/debug/docrs --version)
echo "$shown"
[ "$shown" = "docrs $version" ]

step "npm package"
crates/docrs-wasm/build.sh --pack > /dev/null
node --test crates/docrs-wasm/js/test.mjs
tarball="crates/docrs-wasm/pkg/docrs-$version.tgz"
ls -l "$tarball"
shasum -a 256 "$tarball"

step "open before publishing"
open=0
grep -n "TODO(release)" Cargo.toml crates/*/Cargo.toml && open=1
grep -n '"private": true' crates/docrs-wasm/js/package.json && open=1
grep -q "^## $version (unreleased)" CHANGELOG.md && { echo "CHANGELOG.md: date the $version section"; open=1; }
[ $open = 0 ] && echo "nothing; ready to publish (RELEASING.md)"
exit 0
