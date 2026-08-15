#!/bin/sh
# Verify that the release version agrees across the five places it is declared:
#   1. Cargo.toml             [package].version
#   2. python/Cargo.toml      [package].version
#   3. wasm/Cargo.toml        [package].version (wasm-pack stamps it on @firecrawl/anydoc-wasm)
#   4. node/package.json      .version
#   5. node/index.js          generated version guard (contains `!== '<version>'`)
#
# Usage (from the repo root):
#   sh scripts/check-versions.sh          # check agreement only
#   sh scripts/check-versions.sh v1.2.3   # also check that the tag matches
#
# On success prints the agreed version on stdout and exits 0.
# On failure prints actionable errors on stderr and exits 1.
set -u

err=0

report() { printf '%s\n' "$*" >&2; }

for f in Cargo.toml python/Cargo.toml wasm/Cargo.toml node/package.json node/index.js; do
  if [ ! -f "$f" ]; then
    report "error: $f not found - run this script from the repository root."
    exit 1
  fi
done

# Read `version = "..."` only from the [package] section, not [dependencies] etc.
toml_version() {
  awk '
    /^\[package\]/ { in_package = 1; next }
    /^\[/          { in_package = 0 }
    in_package && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
  ' "$1"
}

cargo_version=$(toml_version Cargo.toml)
python_version=$(toml_version python/Cargo.toml)
wasm_version=$(toml_version wasm/Cargo.toml)

# npm keeps top-level "version" as the first version key in package.json.
package_json_version=$(sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' node/package.json | head -n 1)

# The generated Node loader embeds the version it was generated for in a guard
# containing the literal substring `!== '<version>'`.
guard_version=$(grep -o "!== '[^']*'" node/index.js | head -n 1 | sed "s/^!== '//; s/'\$//")

[ -n "$cargo_version" ]        || { report "error: could not read [package].version from Cargo.toml"; err=1; }
[ -n "$python_version" ]       || { report "error: could not read [package].version from python/Cargo.toml"; err=1; }
[ -n "$wasm_version" ]         || { report "error: could not read [package].version from wasm/Cargo.toml"; err=1; }
[ -n "$package_json_version" ] || { report "error: could not read .version from node/package.json"; err=1; }
[ -n "$guard_version" ]        || { report "error: could not find the version guard (!== '<version>') in node/index.js - regenerate it with 'npm run build' in node/"; err=1; }

[ "$err" -eq 0 ] || exit 1

if [ "$python_version" != "$cargo_version" ] || \
   [ "$wasm_version" != "$cargo_version" ] || \
   [ "$package_json_version" != "$cargo_version" ] || \
   [ "$guard_version" != "$cargo_version" ]; then
  report "error: the release version locations disagree:"
  report "  Cargo.toml [package].version        = $cargo_version"
  report "  python/Cargo.toml [package].version = $python_version"
  report "  wasm/Cargo.toml [package].version   = $wasm_version"
  report "  node/package.json .version          = $package_json_version"
  report "  node/index.js version guard         = $guard_version"
  report "fix: set all five to the same version. Bump the three Cargo.toml files"
  report "     and node/package.json, then run 'npm run build' in node/ to"
  report "     regenerate node/index.js, and commit the result."
  exit 1
fi

if [ "$#" -ge 1 ] && [ -n "$1" ]; then
  tag=$1
  if [ "$tag" != "v$cargo_version" ]; then
    report "error: tag '$tag' does not match the declared version '$cargo_version' (expected tag 'v$cargo_version')."
    report "fix: either delete the tag and re-tag the commit that declares ${tag#v},"
    report "     or bump all four version locations to ${tag#v} and tag that commit."
    exit 1
  fi
fi

printf '%s\n' "$cargo_version"
