#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd -P)"
input_path="${ANYDOC_INPUT:-/Users/henryx/orca/private-fixtures/vibe-mvp/anydoc/source.pdf}"

# This host was bootstrapped without changing the user's shell profile.
if ! command -v cargo >/dev/null 2>&1 && [[ -x "$repo_root/.local-rust/cargo/bin/cargo" ]]; then
  export CARGO_HOME="$repo_root/.local-rust/cargo"
  export RUSTUP_HOME="$repo_root/.local-rust/rustup"
  export PATH="$CARGO_HOME/bin:$PATH"
fi
export RUSTUP_TOOLCHAIN=1.88.0

mkdir -p "$repo_root/henry-mvp/out/private"
cargo run --release --manifest-path "$repo_root/henry-mvp/Cargo.toml" -- \
  --input "$input_path" \
  --output "$repo_root/henry-mvp/out/private/blacklake.md" \
  --report "$repo_root/henry-mvp/out/private/report.json" \
  --label blacklake-preipo-bp
