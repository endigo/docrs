#!/bin/sh
# Build the Node.js package into crates/docrs-wasm/pkg:
#   crates/docrs-wasm/build.sh [--pack]
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli at the
# version Cargo.lock pins for the wasm-bindgen crate. The npm version is the
# crate's. --pack also runs `npm pack` and prints the tarball, which installs
# exactly what npm would serve.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
bindgen=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"/\1/p;}' "$root/Cargo.lock")
case "$(wasm-bindgen --version 2>/dev/null)" in
  "wasm-bindgen $bindgen"*) ;;
  *) echo "needs wasm-bindgen-cli $bindgen: cargo install wasm-bindgen-cli --version $bindgen --locked" >&2; exit 1 ;;
esac
cargo build --manifest-path "$root/Cargo.toml" --release -p docrs-wasm --target wasm32-unknown-unknown
rm -rf "$here/pkg"
wasm-bindgen --target nodejs --out-dir "$here/pkg" --out-name docrs \
  "$root/target/wasm32-unknown-unknown/release/docrs_wasm.wasm"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
node -e 'const [src, out, version] = process.argv.slice(1); const p = require(src);
  require("fs").writeFileSync(out, JSON.stringify({ name: p.name, version, ...p }, null, 2) + "\n");' \
  "$here/js/package.json" "$here/pkg/package.json" "$version"
cp "$here/js/README.md" "$root/LICENSE" "$root/NOTICE.md" "$here/pkg/"
if [ "$1" = "--pack" ]; then
  (cd "$here/pkg" && npm pack --silent)
fi
