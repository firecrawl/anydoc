# anydoc local MVP acceptance

Date: 2026-08-07
Result: **PASS**

## Accepted artifact and provenance

The private fixture was copied byte-for-byte from the designated source without modifying, moving, renaming, or overwriting the source. Only the safe label and non-content measurements are recorded here.

| Check | Measured result |
| --- | ---: |
| Safe label | `blacklake-preipo-bp` |
| Pages | 23 |
| Encrypted | no |
| Source/fixture bytes | 20,039,805 |
| First three pages extractable bytes | 2,466 |
| Source/fixture SHA-256 | `84d251f7729f87194741cf74bf23d2856a020da248b68de89e31321a666be7a2` |
| Release conversion time | 51 ms |
| Markdown characters | 9,132 |
| Headings | 137 |
| List items | 15 |
| Table rows | 14 |
| Warnings | `[]` |

The generated Markdown was 19,306 bytes and non-empty; the JSON report was 267 bytes and non-empty. Neither generated file is tracked by Git.

## Acceptance commands

These commands were run from the repository root with the pinned Rust 1.88.0 toolchain available on `PATH`:

```bash
rustc --version
cargo --version
shasum -a 256 /Users/henryx/orca/private-fixtures/vibe-mvp/anydoc/source.pdf
cargo test
cargo test --manifest-path henry-mvp/Cargo.toml
cargo fmt --manifest-path henry-mvp/Cargo.toml -- --check
cargo clippy --manifest-path henry-mvp/Cargo.toml --all-targets -- -D warnings
bash henry-mvp/scripts/demo-local.sh
test -s henry-mvp/out/private/blacklake.md
test -s henry-mvp/out/private/report.json
jq '{label,sha256,input_bytes,elapsed_ms,markdown_chars,headings,list_items,table_rows,warnings}' henry-mvp/out/private/report.json
git status --short
git ls-files | rg 'source\.pdf|blacklake\.md|report\.json|out/private' && exit 1 || true
```

The original source path is intentionally not reproduced in this tracked report. Its integrity was verified separately: its current SHA-256 matched both the staged private fixture and the private manifest.

## Test results

- Upstream `cargo test`: 208 library tests passed, 1 robustness test passed, and 8 snapshot tests passed; 1 local-samples test was ignored by design.
- MVP `cargo test`: 8 integration tests passed. They cover structural counting, privacy-safe serialization, real public-fixture conversion, unsupported-format failure, image-only PDF/OCR failure, strict flags and labels, and output-boundary enforcement.
- Formatting and Clippy checks passed with warnings denied.
- The real release conversion completed successfully and produced the measured structural report above.

## Privacy checks

- The source PDF remains outside the repository and the staged fixture remains under the explicitly approved private fixture directory with mode `0600`.
- No source PDF, generated Markdown, generated JSON report, or `out/private` path is tracked.
- The tracked MVP files contain neither the original source absolute path nor extracted BP text.
- The report schema has no field for an input path or Markdown body; the serialization test enforces both omissions.
- No cloud parsing or OCR service was called.

## Known limitation

`anydoc` does not OCR image-only pages. Image-only or scanned PDFs return a typed `unsupported` error mentioning that OCR is required, exit non-zero, and leave both output paths unwritten. Mixed PDFs may convert their extractable text while upstream logging warns about pages that need OCR; this wrapper's report currently records an empty warnings array because `anydoc` exposes those notices through logging rather than its return value.
