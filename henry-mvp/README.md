# anydoc local MVP

This isolated Rust binary converts one local document with the repository's `anydoc` crate, writes Markdown to a Git-ignored private directory, and emits a content-free JSON quality report. It never calls Firecrawl Parse, cloud OCR, or another network conversion service.

## Toolchain

Rust 1.88.0 is pinned in `rust-toolchain.toml`. On the current host, rustup and the toolchain were installed under the repository's ignored `.local-rust/` directory so no shell profile or system package was changed.

To use that repository-local installation in a new shell:

```bash
export CARGO_HOME="$PWD/.local-rust/cargo"
export RUSTUP_HOME="$PWD/.local-rust/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
rustc --version
cargo --version
```

If `.local-rust/` is absent, install Rust 1.88.0 with rustup using a location appropriate for the host, then run the version checks above. The crate also declares `rust-version = "1.88"`.

## Run

The launcher expects the staged private fixture at `/Users/henryx/orca/private-fixtures/vibe-mvp/anydoc/source.pdf` unless `ANYDOC_INPUT` overrides it:

```bash
bash henry-mvp/scripts/demo-local.sh
test -s henry-mvp/out/private/blacklake.md
test -s henry-mvp/out/private/report.json
jq '{label,sha256,input_bytes,elapsed_ms,markdown_chars,headings,list_items,table_rows,warnings}' \
  henry-mvp/out/private/report.json
```

The CLI requires exactly `--input`, `--output`, `--report`, and `--label`. Labels containing a slash, unknown or duplicate flags, and output/report paths outside `henry-mvp/out/private/` are rejected with exit code 2. Conversion failures return exit code 1; success returns 0 and writes non-empty Markdown plus a pretty JSON report.

## Privacy and limitations

- `henry-mvp/out/private/` and the crate's `target/` directory are Git-ignored.
- The JSON report contains a caller-supplied safe label, SHA-256, byte and timing measurements, structural counts, and warnings. It contains neither the input path nor extracted Markdown.
- Writes use temporary sibling files followed by atomic renames.
- Unknown formats and image-only PDFs fail with typed diagnostics and do not create output files.
- `anydoc` does not OCR image-only pages. A scanned document therefore requires a separate, explicitly approved local OCR workflow before conversion.

## Verify

From the repository root:

```bash
cargo test
cargo test --manifest-path henry-mvp/Cargo.toml
cargo fmt --manifest-path henry-mvp/Cargo.toml -- --check
cargo clippy --manifest-path henry-mvp/Cargo.toml --all-targets -- -D warnings
git ls-files | rg 'source\.pdf|blacklake\.md|report\.json|out/private' && exit 1 || true
```

Measured evidence from the real local conversion is recorded in `docs/acceptance.md` without document content.
