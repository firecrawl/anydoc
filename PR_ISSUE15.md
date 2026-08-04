<!-- Title: fix(csv): promote a label-like first row to the table header -->

Fixes #15.

This takes the heuristic direction from the issue, with one correction to
its framing that makes a simpler rule safe.

## What

CSV built its table with zero header rows, and GFM requires a header, so the
renderer emitted a blank header row with the column names sitting in the
body as data:

```
|  |  |  |
| --- | --- | --- |
| name | role | team |
| Alice | Engineer | Platform |
```

Now the first record is promoted to the header when it looks like labels:

```
| name | role | team |
| --- | --- | --- |
| Alice | Engineer | Platform |
```

## Why the heuristic can be simpler than the issue proposed

Two observations changed the design:

1. **A false positive only restyles a row.** Promotion sets `header_rows = 1`
   — the row still renders, in the header position. Nothing is deleted, so
   the issue's "guessing wrong would silently delete a data row" cost doesn't
   exist. The real asymmetry is: false positive = cosmetic; false negative =
   blank header + column names read as data.
2. **The issue's own suggested signal ("at least one later row parses as
   numeric/date") fails the issue's own repro** — `Alice,Engineer,Platform`
   is all text. Header detection can't rely on body types.

So the rule asks only that the first record *look like labels*, with no body
evidence required: **at least two columns, as wide as every record, and each
field non-empty, non-numeric (decimal commas like `1,5` count as numeric),
and distinct** (case-insensitive). Anything else — numeric first rows,
ragged or repeated fields, empty fields, a single record, one column —
fails safe to today's headerless output.

## Tests

- Unit: the issue's all-text repro promotes; numeric first rows (dot and
  decimal-comma) don't; duplicate labels, empty fields, and a first row
  narrower than the body don't; single-record and single-column files don't;
  promotion keeps all rows (nothing dropped).
- New fixture `handmade-numeric.csv` + snapshot pinning the fail-safe
  (numeric data keeps the blank header).
- All four existing CSV snapshots update to exactly the issue's "Expected"
  shape — header names in the header row, body unchanged. No non-CSV
  snapshot moved.
- `cargo fmt --check`, `clippy --workspace --all-targets --all-features
  -D warnings`, `cargo test --locked` all green.

## On the alternative

The issue also floated an explicit `has_headers` option. That's compatible
with this change (an option could override the heuristic later), but the
public API today takes bytes in and gives Markdown out with no per-format
options, so the heuristic route avoids inventing new API surface for the
common case.
