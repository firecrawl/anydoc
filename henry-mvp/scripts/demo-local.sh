#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd -P)"
input_path="${ANYDOC_INPUT:-/Users/henryx/orca/private-fixtures/vibe-mvp/anydoc/source.pdf}"

export CARGO_HOME="$repo_root/.local-rust/cargo"
export RUSTUP_HOME="$repo_root/.local-rust/rustup"
local_cargo="$CARGO_HOME/bin/cargo"
if [[ ! -x "$local_cargo" ]]; then
  echo "local Rust toolchain is missing; bootstrap Rust 1.88.0 under $repo_root/.local-rust" >&2
  exit 1
fi
export PATH="$CARGO_HOME/bin:$PATH"
export RUSTUP_TOOLCHAIN=1.88.0

mkdir -p "$repo_root/henry-mvp/out/private"
chmod 700 "$repo_root/henry-mvp/out/private"
"$local_cargo" +1.88.0 run --locked --release --manifest-path "$repo_root/henry-mvp/Cargo.toml" -- \
  --input "$input_path" \
  --output "$repo_root/henry-mvp/out/private/blacklake.md" \
  --report "$repo_root/henry-mvp/out/private/report.json" \
  --label blacklake-preipo-bp
