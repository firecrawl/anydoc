# anydoc local MVP

This isolated Rust binary converts one local document with the repository's `anydoc` crate, writes Markdown to a Git-ignored private directory, and emits a content-free JSON quality report. It never calls Firecrawl Parse, cloud OCR, or another network conversion service.

## Toolchain

Rust 1.88.0, rustfmt, and Clippy are pinned in `rust-toolchain.toml`. On the current host, rustup and the toolchain were installed under the repository's tracked-ignored `.local-rust/` directory so no shell profile or system package was changed. The launcher always uses that repository-local `CARGO_HOME`, `RUSTUP_HOME`, and toolchain; it does not fall back to a global Cargo binary.

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
jq '{label,sha256,input_bytes,elapsed_ms,markdown_chars,headings,list_items,paragraphs,table_rows,tables,warnings}' \
  henry-mvp/out/private/report.json
```

The CLI requires exactly `--input`, `--output`, `--report`, and `--label`. Labels containing a slash, unknown or duplicate flags, and output/report paths outside `henry-mvp/out/private/` are rejected with exit code 2. Conversion failures return exit code 1; success returns 0 and writes non-empty Markdown plus a pretty JSON report.

## Privacy and limitations

- `.local-rust/`, `henry-mvp/out/private/`, and the crate's `target/` directory are tracked in Git ignore rules.
- Private output directories are mode `0700`; Markdown, JSON, staging, and rollback files are mode `0600` on Unix.
- The JSON report contains a caller-supplied safe label, SHA-256, byte and timing measurements, structural counts, and warnings. It contains neither the input path nor extracted Markdown. `paragraphs` counts contiguous plain-text blocks, and `tables` counts contiguous runs of table rows; `table_rows` remains available.
- PDF conversion reuses the same pinned `pdf-inspector` version as `anydoc` to surface safe OCR-page counts and encoding-quality warnings. A mixed PDF cannot appear false-clean when pages need OCR.
- Markdown and JSON are staged before publication. If the second publication fails, the first file is restored to its previous version or removed when no previous file existed.
- Unknown formats and image-only PDFs fail with typed diagnostics and do not create output files.
- `anydoc` does not OCR image-only pages. A scanned document therefore requires a separate, explicitly approved local OCR workflow before conversion.

## Verify

From the repository root:

```bash
cargo test --locked
cargo test --locked --manifest-path henry-mvp/Cargo.toml
cargo fmt --manifest-path henry-mvp/Cargo.toml -- --check
cargo clippy --locked --manifest-path henry-mvp/Cargo.toml --all-targets -- -D warnings
bash henry-mvp/scripts/privacy-audit.sh
git ls-files | rg 'source\.pdf|blacklake\.md|report\.json|out/private' && exit 1 || true
```

Measured evidence from the real local conversion is recorded in `docs/acceptance.md` without document content.
