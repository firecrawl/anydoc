#!/bin/sh
# Verify that the release version agrees across the seven declared locations.
#
# Usage (from the repo root):
#   sh scripts/check-versions.sh          # check agreement only
#   sh scripts/check-versions.sh v1.2.3   # also check that the tag matches
set -u

err=0

report() { printf '%s\n' "$*" >&2; }

for f in Cargo.toml python/Cargo.toml wasm/Cargo.toml node/package.json node/index.js go/Cargo.toml go/version.go; do
  if [ ! -f "$f" ]; then
    report "error: $f not found - run this script from the repository root."
    exit 1
  fi
done

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
go_version=$(toml_version go/Cargo.toml)
package_json_version=$(sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' node/package.json | head -n 1)
guard_version=$(grep -o "!== '[^']*'" node/index.js | head -n 1 | sed "s/^!== '//; s/'\$//")
go_const_version=$(sed -n 's/^const Version = "\([^"]*\)".*/\1/p' go/version.go | head -n 1)

[ -n "$cargo_version" ]        || { report "error: could not read [package].version from Cargo.toml"; err=1; }
[ -n "$python_version" ]       || { report "error: could not read [package].version from python/Cargo.toml"; err=1; }
[ -n "$wasm_version" ]         || { report "error: could not read [package].version from wasm/Cargo.toml"; err=1; }
[ -n "$package_json_version" ] || { report "error: could not read .version from node/package.json"; err=1; }
[ -n "$guard_version" ]        || { report "error: could not find the version guard in node/index.js"; err=1; }
[ -n "$go_version" ]           || { report "error: could not read [package].version from go/Cargo.toml"; err=1; }
[ -n "$go_const_version" ]     || { report "error: could not read const Version from go/version.go"; err=1; }

[ "$err" -eq 0 ] || exit 1

if [ "$python_version" != "$cargo_version" ] || \
   [ "$wasm_version" != "$cargo_version" ] || \
   [ "$package_json_version" != "$cargo_version" ] || \
   [ "$guard_version" != "$cargo_version" ] || \
   [ "$go_version" != "$cargo_version" ] || \
   [ "$go_const_version" != "$cargo_version" ]; then
  report "error: release version locations disagree:"
  report "  Cargo.toml [package].version        = $cargo_version"
  report "  python/Cargo.toml [package].version = $python_version"
  report "  wasm/Cargo.toml [package].version   = $wasm_version"
  report "  node/package.json .version          = $package_json_version"
  report "  node/index.js version guard         = $guard_version"
  report "  go/Cargo.toml [package].version     = $go_version"
  report "  go/version.go const Version         = $go_const_version"
  exit 1
fi

if [ "$#" -ge 1 ] && [ -n "$1" ] && [ "$1" != "v$cargo_version" ]; then
  report "error: tag '$1' does not match the declared version '$cargo_version' (expected tag 'v$cargo_version')."
  exit 1
fi

printf '%s\n' "$cargo_version"
