#!/bin/sh
# Print the release version (Cargo.toml [workspace.package]) after checking
# that the library dependency the CLI publishes with and, when given, a tag
# (v0.1.0) agree with it:  tools/release/version.sh [TAG]
set -e
root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
dependency=$(sed -n 's/^docrs = .*version = "\([^"]*\)".*/\1/p' "$root/Cargo.toml")
[ "$dependency" = "$version" ] || { echo "Cargo.toml: workspace version $version, library dependency $dependency" >&2; exit 1; }
[ -z "$1" ] || [ "$1" = "v$version" ] || { echo "tag $1 is not v$version" >&2; exit 1; }
echo "$version"
