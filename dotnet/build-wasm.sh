#!/bin/sh
# Build the anydoc WASM engine from the repo's wasm/ crate and lay the
# wasm-bindgen output into Firecrawl.Anydoc.Wasm/wwwroot so the NuGet package
# ships them as static web assets.
#
# Produces:
#   Firecrawl.Anydoc.Wasm/wwwroot/anydoc_wasm.js      wasm-bindgen glue
#   Firecrawl.Anydoc.Wasm/wwwroot/anydoc_wasm_bg.wasm engine binary
#
# The static wwwroot/anydoc-wasm.js interop shim is checked in (not generated).
#
# Prerequisites:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-pack   (or use the wasm-bindgen CLI directly)
set -eu

cd "$(dirname "$0")"
root=$(pwd)
wasm_dir="$root/../wasm"
out="$root/Firecrawl.Anydoc.Wasm/wwwroot"

build_with_wasm_pack() {
  (cd "$wasm_dir" && wasm-pack build --release --target web --out-dir pkg)
  cp "$wasm_dir/pkg/anydoc_wasm.js"   "$out/anydoc_wasm.js"
  cp "$wasm_dir/pkg/anydoc_wasm_bg.wasm" "$out/anydoc_wasm_bg.wasm"
}

build_with_wasm_bindgen() {
  rustup target add wasm32-unknown-unknown >/dev/null
  (cd "$wasm_dir" && cargo build --release --target wasm32-unknown-unknown --package anydoc-wasm)
  wasm-bindgen "$root/../target/wasm32-unknown-unknown/release/anydoc_wasm.wasm" \
    --target web --out-dir "$out" --out-name anydoc_wasm
}

rm -f "$out/anydoc_wasm.js" "$out/anydoc_wasm_bg.wasm"
if command -v wasm-pack >/dev/null 2>&1; then
  build_with_wasm_pack
elif command -v wasm-bindgen >/dev/null 2>&1; then
  build_with_wasm_bindgen
else
  echo "no wasm-pack or wasm-bindgen toolchain found." >&2
  echo "install one: cargo install wasm-pack  (or cargo install wasm-bindgen-cli)" >&2
  exit 1
fi

echo "-> $out/anydoc_wasm.js"
echo "-> $out/anydoc_wasm_bg.wasm"