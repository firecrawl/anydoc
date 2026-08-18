#!/usr/bin/env bash
# Build the host cdylib and generate Kotlin bindings from it.
# Run from the repository root: sh kotlin/scripts/generate-bindings.sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

profile=${ANYDOC_PROFILE:-debug}
if [ "$profile" = release ]; then
  cargo build -p anydoc-kotlin --lib --release
else
  cargo build -p anydoc-kotlin --lib
fi

lib=""
for candidate in \
  "target/$profile/libanydoc_kotlin.so" \
  "target/$profile/libanydoc_kotlin.dylib" \
  "target/$profile/anydoc_kotlin.dll"
do
  if [ -f "$candidate" ]; then
    lib=$candidate
    break
  fi
done

if [ -z "$lib" ]; then
  echo "error: anydoc_kotlin native library not found under target/$profile" >&2
  exit 1
fi

out="$root/kotlin/android/generated"
mkdir -p "$out"
cargo run -p anydoc-kotlin --features cli --bin uniffi-bindgen -- \
  generate --library "$lib" --language kotlin --out-dir "$out"

echo "generated Kotlin bindings in $out from $lib"
echo "native dir: $root/target/$profile"
