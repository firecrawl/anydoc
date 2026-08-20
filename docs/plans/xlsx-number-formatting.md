---
title: XLSX Number Formatting - Plan
type: fix
date: 2026-08-08
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# XLSX Number Formatting - Plan

## Goal Capsule

- **Objective:** Render XLSX numeric cells using their Excel display format (`65%`, `$56,701.03`, `1,234.50`) instead of raw floats (`0.653035934239184`, `56701.0309278351`), for issue firecrawl/anydoc#64 and its duplicate #27.
- **Authority:** Product behavior is owned by the Requirements (R-IDs). Implementation mechanism is owned by the Key Technical Decisions (KTD-IDs). Implementation units (U-IDs) execute within both.
- **Stop condition:** Conversion output for formatted numeric cells matches the Excel-displayed value for the supported format-code families, all existing snapshots pass (except justified, reviewed changes), and the verification contract passes.
- **Execution profile:** Code change in the Rust crate root package; no binding or node/python changes needed (they consume the same `to_markdown` pipeline).
- **Tail ownership:** The implementing workflow owns branch, tests, commit, and PR. This plan is a `fix:`-typed change to the `anydoc` crate.

## Product Contract

### Summary

XLSX cells that carry a display number format (percent, currency, fixed decimals, thousands separators) currently render as raw floats in Markdown output. A cell formatted `65%` in Excel renders as `0.653035934239184`; a cell formatted `$56,701.03` renders as `56701.0309278351`. The issue reporter processes CRE financial documents (cap rates, LTV, DSCR, currency values) where the formatted representation is what downstream AI consumption expects. This change renders display-formatted values per Excel display semantics, which is a fidelity-for-semantics trade: a `0.00%` cell rounds its value to the format's precision by design, so downstream math on formatted output is exact only to that precision. That matches what the issue requests (Excel-displayed values) and what Excel itself shows.

### Problem Frame

- `src/formats/sheet/mod.rs::format_data()` converts `Data::Float` via `format_float()` at 15 significant digits and `Data::Int` via `to_string()`, never consulting the cell's Excel number format code.
- The library is promoted for LLM ingestion; the raw-float output is plausible-looking but loses the display semantics (percentage vs ratio, currency, precision) of formatted cells.
- Date and time formats are mostly handled: calamine (with the `dates` feature) classifies date/time formats and converts those cells to `Data::DateTime`, which `format_data()` renders as ISO text. The gap is numeric display formats on numeric cells, plus builtin date ids 27-36 which calamine does not classify (those stay out of scope, R10).

### Requirements

**Rendering of formatted numbers**

- R1. A numeric cell whose format code renders a percentage (`0%`, `0.00%`, `#,##0.0%`) must emit the multiplied, formatted value (`0.653...` renders as `65%` or `65.30%` per the code).
- R2. A numeric cell whose format code carries currency or literal symbols (`$#,##0.00`, `"€"#,##0.00`, `[$$-409]#,##0.00`) must emit those literals around the formatted number (`$56,701.03`, `$1,234.50`).
- R3. A numeric cell whose format code specifies decimals, thousands separators, or scaling (`0.00`, `#,##0`, `#,##0.00`, `#,##0,`) must emit the value with that precision, grouping, and scaling (`1234.5` renders `1,234.50`; `#,##0,` renders thousands as `1,235`).
- R4. Negative values must follow the format's negative section when present (`(0.00)` → `(1.50)`), or a leading `-` when absent. Empty sections render as Excel does (`0.00;` shows blank for negatives; `0.00;;` shows blank for zeros).
- R5. Scientific format codes (`0.00E+00`, `##0.0E+0`) must emit the value in that notation (`1.23E+05`).
- R6. Cells with `General` format (or no format) must render exactly as today: `format_float` at 15 significant digits for floats, `to_string` for ints.
- R7. Cells whose format code is date/time-like must keep today's behavior (calamine's `Data::DateTime` path; never double-format). Builtin date ids 27-36 (locale East-Asian dates) and 45-47 fall back to today's rendering.
- R8. Unsupported or malformed format codes must fall back to today's raw rendering; the fallback must never fail the conversion or emit a partial render. Fraction formats (`# ?/?`) render through the engine (supported by the renderer); only renderer-parse failures fall back.
- R9. A malformed, unreadable, or missing `styles.xml`/worksheet part must degrade to today's output with a log line, except resource-limit errors which propagate per the crate's limits policy (R9a); it must never fail the conversion otherwise (recovery policy parity with the rest of the crate).

**Scope of formats**

- R10. The format pipeline applies to OOXML workbooks (xlsx, xlsm, xlam — calamine loads all three as `Sheets::Xlsx`). xls (BIFF), xlsb, and ods keep today's behavior; calamine exposes no per-cell format codes for them.
- R11. Format rendering applies only when the cell value is numeric (`Data::Float` or `Data::Int`). String, boolean, and error cells keep today's rendering.

**Output stability**

- R12. Format-rendered cell text must remain plain inline text in the Markdown table cell. The existing context-sensitive escape pipeline (`src/render/markdown/escape.rs` `escape_text`) escapes any potentially active markup characters (`|`, `*`, `_`, `~`, backticks, `[`, `]`) that appear in rendered cell text; format-code literals that survive rendering flow through that pipeline like any other cell text, and no new escaping logic is added in this change.

### Acceptance Examples

- AE1. A cell with value `0.653035934239184` and format code `0.00%` renders `65.30%` (issue #64 example `65%` uses `0%`).
- AE2. A cell with value `56701.0309278351` and format code `$#,##0.00` renders `$56,701.03`.
- AE3. A cell with value `0.075` and format code `0.0%` renders `7.5%` (issue #27 example).
- AE4. A cell with value `1234.5` and format code `#,##0.00` renders `1,234.50`.
- AE5. A cell with value `-1.5` and format code `0.00;(0.00)` renders `(1.50)`.
- AE6. A cell with value `0.5` and format code `General` renders `0.5`.
- AE7. A string cell containing the text `5.75%` renders unchanged as `5.75%` (issue #64's third example case; string cells never enter the numeric format path).

### Scope Boundaries

**Deferred for later**

- Number formats on xls (BIFF), xlsb, and ods workbooks (no calamine format API; separate binary/xml mechanisms).
- Rendering of builtin locale date ids 27-36 (calamine does not classify them; rendering them needs locale and 1904-system plumbing).
- Excel colors in format codes (rendered without color) and conditional-section selection: the renderer's default behavior is used; colors are dropped, conditions select sections per Excel defaults.
- Non-US locale currency substitution for builtin ids 41-44 (rendered with the renderer's default `$` symbol).

**Outside this product's identity**

- No changes to the markdown renderer, the document model, or the public API (no opt-out flag; the raw-value trade is documented in R1-R5 and the DoD).
- No new opt-in/opt-out configuration surface.

### Sources

- Issue firecrawl/anydoc#64 (this work's origin) and #27 (duplicate report with `0.075` example).
- `ssfmt` crate (v0.1.2, MIT OR Apache-2.0, zero dependencies with `default-features = false`): Excel-compatible ECMA-376 number-format renderer, 99.9999% compatibility with SheetJS SSF over 19.5M test cases; `format(value, code, opts)` one-shot API plus `builtin_formats::format_code_from_id` for builtin numFmtIds.
- calamine 0.36.1 source at `~/.cargo/registry/src/.../calamine-0.36.1/src/`: `formats.rs` (`detect_custom_number_format`, `builtin_format_by_id` classify only ids 14-22, 45-47 as dates) and `auto.rs:42` (xlsx/xlsm/xlam load as `Sheets::Xlsx`); no public per-cell format code API exists.
- ECMA-376 Part 1 §18.8.30 builtin format table (ids 0-49) for the id mapping in KTD5.

## Planning Contract

### Key Technical Decisions

- KTD1. Adopt the `ssfmt` crate (v0.1.2, `default-features = false` → zero runtime dependencies) as the number-format renderer instead of hand-rolling an engine. The original hand-roll plan was rejected after review: a maintained, pure-Rust, SSF-verified renderer exists (review evidence: crates.io `ssfmt`, MIT OR Apache-2.0, no deps, `format_code_from_id` builtin support), and a hand-rolled engine re-solves known-unsolved fidelity problems (half-away-from-zero rounding ties, trailing-comma scaling, empty sections, currency-locale `[$$-409]` brackets, fraction formats) with test risk. `ssfmt` is pure Rust and wasm-compatible; the crate is young (published 2026-01, v0.1.2) and will be pinned exactly. Rejected alternative: `rust_xlsxwriter`'s engine — writer-side, no public format-code renderer API, larger dependency. Rejected alternative: hand-rolled engine — justified above.
- KTD2. Read format metadata from the OOXML parts directly (`xl/styles.xml`, `xl/workbook.xml`, `xl/_rels/workbook.xml.rels`, `xl/worksheets/sheetN.xml`) through the existing `crate::package::{Package, xml::parse_xml}` infrastructure, alongside calamine for cell values. Calamine (latest, 0.36.1) exposes no public per-cell format API. Cell coordinate alignment between our XML read and calamine's used range is by absolute cell reference (`r="A1"`) minus the range start, which is exact; cells without an `r` attribute resolve positionally within their `<row>` (OOXML allows omitted `r`). Cells without an `s` attribute resolve to style 0. Rejected alternative: reimplement xlsx value parsing — duplicate of calamine with no benefit.
- KTD3. Recovery policy: every format-metadata read failure that is not a resource-limit error (missing part, malformed XML, style index out of range) degrades to today's rendering with a `log::warn!`, matching the crate's unified recovery policy. Resource-limit errors (`ConvertError::ResourceLimit` from `src/package/limits.rs`) propagate — the crate's security caps must never be silently swallowed. A format bug must never fail a conversion (R9).
- KTD4. Date/time-like format codes are never rendered by the numeric engine; they are left to calamine's `Data::DateTime` conversion. The engine skips any code containing date/time tokens (`d`, `m`, `h`, `y`, `s` outside literal contexts, `[h]`/`[m]`/`[s]` duration brackets) — a defensive guard, since calamine converts those cells before anydoc sees them (R7).
- KTD5. Builtin numFmtId resolution: ids 0-13, 48 render via `ssfmt::builtin_formats::format_code_from_id` (numbers, percentages, scientific, fractions). Ids 14-22, 45-47 (date/time — calamine converts) and 27-36 (locale dates — calamine does not classify; out of scope) map to `None` (fallback). Ids 23-26 (undefined in ECMA-376 §18.8.30) map to `None`. Ids 37-44 (accounting/currency) render via `format_code_from_id` with the renderer's default locale (`$` symbol); negative sections with `[Red]` render without color. Id 49 (`@`, text) maps to `None`. Custom numFmtIds resolve from `styles.xml` `numFmts`; if a style's numFmtId is absent from both maps, it maps to `None` (R6).
- KTD6. Section selection and value semantics (percent scaling, comma grouping/scaling, rounding half-away-from-zero, empty sections, currency brackets) are delegated to `ssfmt`, which matches Excel's display behavior (R1-R5, R4); the plan pins representative outputs in unit tests including tie values (`0.5` with `0` → `1`).

### High-Level Technical Design

Data flow for an xlsx/xlsm/xlam workbook:

1. `parse(bytes)` opens the workbook via calamine as today.
2. In parallel, the new styles reader opens the same bytes with `Package::open` and builds `HashMap<sheet_name, HashMap<(u32, u32), String>>` (absolute coordinates) mapping cells to their format code string:
   - `xl/styles.xml`: `numFmts` entries build `numFmtId -> code`; `cellXfs` entries build `style_index -> (numFmtId, applyNumberFormat)`; when `applyNumberFormat` is present and `"0"`, the style's effective numFmtId is 0 (General) regardless of the declared id; builtin ids resolve through KTD5's mapping.
   - `xl/_rels/workbook.xml.rels` + `xl/workbook.xml`: sheet name -> sheet part path.
   - Each `xl/worksheets/sheetN.xml`: `<c r="A1" s="3">` yields `(row, col) -> style_index` (`r` parsed from the cell reference, positional fallback when absent); combined with the style table, `(row, col) -> code`.
   - Any non-fatal parse error aborts the whole styles build with a warn log (KTD3); a resource-limit error propagates.
3. In the cell loop, the relative position `(r, c)` plus the used-range `start` gives the absolute coordinate; a code lookup hits `format_data(data, code)`:
   - `Data::Float` / `Data::Int` with a renderable numeric code -> `ssfmt` output (R1-R5, R4, R5).
   - Everything else -> today's `format_float`/`to_string` paths unchanged (R6, R7, R8).

The renderer adapter is a pure function `render(value: f64, code: &str) -> Option<String>`; `None` means fallback to raw. It delegates to `ssfmt::format(value, code, FormatOptions::default())` (or the compile-once `NumberFormat::parse` + `format` pair), returning `None` on any `ParseError` or `FormatError`, and `None` for `General` and the KTD4/KTD5 skip-list codes. `Data::Int` values convert through `f64`; integer values above 2^53 lose sub-integer digits, which matches Excel's own 15-significant-digit display precision (documented, not a regression relative to Excel display).

### Assumptions

- The committed fixture `tests/fixtures/xlsx/sheet.xlsx` (LibreOffice-generated) contains formatted numeric cells (verified: numFmtId 166 = `[$$-409]#,##0.00` currency cell at B3, plus percent and thousands cells); its snapshot changes are certain, and each diff is reviewed and accepted only when it matches Excel-displayed values.
- The Python binding and Node binding need no changes: they call the same core pipeline.
- openpyxl (pip-installed ad hoc for the dev-only manual cross-check) writes `applyNumberFormat` and `numFmtId` in the standard way; handmade fixtures are assembled directly and need no office tooling.

## Implementation Units

### U1. Number-format renderer adapter

- **Goal:** Implement the `ssfmt`-based renderer with fallback.
- **Requirements:** R1-R8.
- **KTDs:** KTD1, KTD4, KTD5, KTD6.
- **Files:**
  - `Cargo.toml`: add `ssfmt = { version = "0.1.2", default-features = false }`.
  - `src/formats/sheet/numfmt.rs` (new): `render(value: f64, code: &str) -> Option<String>`; `code_for_style(num_fmt_id: u32, custom: &HashMap<u32, String>) -> Option<String>` implementing KTD5; `General` and skip-list handling; module-level unit tests.
  - `src/formats/sheet/mod.rs`: `mod numfmt;` declaration only (no wiring in this unit).
- **Approach:**
  - `render` returns `None` for `General`, empty codes, and KTD4/KTD5 skip-list codes; otherwise `ssfmt::format(value, code, &FormatOptions::default())`, mapping `Err` to `None`.
  - `code_for_style` resolves custom codes from `numFmts` first, then builtin ids per KTD5 (renderable ids -> `format_code_from_id`, date/undefined/text ids -> `None`).
  - The adapter must not panic on any input; `ssfmt` errors map to `None`.
- **Test scenarios (unit tests in `src/formats/sheet/numfmt.rs`):**
  - T1.1 `render(0.653035934239184, "0%") == Some("65%")`; `render(0.653035934239184, "0.00%") == Some("65.30%")` (AE1).
  - T1.2 `render(56701.0309278351, "$#,##0.00") == Some("$56,701.03")` (AE2).
  - T1.3 `render(0.075, "0.0%") == Some("7.5%")` (AE3).
  - T1.4 `render(1234.5, "#,##0.00") == Some("1,234.50")` (AE4).
  - T1.5 `render(-1.5, "0.00;(0.00)") == Some("(1.50)")`; `render(-1.5, "0.00") == Some("-1.50")` (R4).
  - T1.6 `render(123456.0, "0.00E+00") == Some("1.23E+05")` (R5).
  - T1.7 `render(1234567.0, "#,##0,") == Some("1,235")` (R3 trailing-comma scaling).
  - T1.8 `render(1234.5, "[$$-409]#,##0.00") == Some("$1,234.50")` (R2 currency-locale bracket).
  - T1.9 `render(0.5, "0") == Some("1")`; `render(2.5, "0") == Some("3")` (KTD6 tie rounding, pins ssfmt behavior).
  - T1.10 `render(-1.5, "0.00;") == Some("")` (R4 empty negative section).
  - T1.11 `render(1.5, "General") == None`; `render(1.5, "dd/mm/yyyy") == None` (R6, R7 guards).
  - T1.12 `render(1.5, "0.00E+00_XYZ") == Some(...)` or `None` per ssfmt — pin whatever ssfmt produces for a malformed code (R8; adjust to actual behavior, must not panic).
  - T1.13 `code_for_style(9, &{}) == Some("0%")`; `code_for_style(4, &{}) == Some("#,##0.00")`; `code_for_style(164, &{164: "0.00%"}) == Some("0.00%")`; `code_for_style(14, &{}) == None`; `code_for_style(27, &{}) == None`; `code_for_style(49, &{}) == None` (KTD5).
- **Verification:** `cargo test numfmt` passes; `cargo clippy -D warnings` clean on the new module.

### U2. OOXML styles and sheet-format reader

- **Goal:** Read per-cell format codes from xlsx/xlsm parts and expose a lookup keyed by absolute `(row, col)` per sheet.
- **Requirements:** R9, R9a, R10, R11 (enforcement of R11 lives in U3).
- **KTDs:** KTD2, KTD3, KTD5.
- **Files:**
  - `src/formats/sheet/styles.rs` (new): `pub struct CellFormats { by_sheet: HashMap<String, HashMap<(u32, u32), String>> }` with `pub fn from_ooxml(bytes: &[u8]) -> Result<Option<Self>, ConvertError>` and `pub fn code(&self, sheet: &str, row: u32, col: u32) -> Option<&str>`.
  - `src/formats/sheet/mod.rs`: `mod styles;` declaration only.
- **Approach:**
  - Open with `Package::open(bytes)`; if no `xl/styles.xml` part exists, return `Ok(None)` (no formats).
  - Parse `xl/styles.xml` with `parse_xml`: `numFmts/numFmt[@numFmtId][@formatCode]` map; `cellXfs/xf[@numFmtId][@applyNumberFormat]` list indexed by style index; resolve each style to a code via KTD5's mapping, honoring `applyNumberFormat="0"` as General.
  - Parse `xl/workbook.xml` + `xl/_rels/workbook.xml.rels` to map sheet names to worksheet part paths (normalize via `crate::package::xml::normalize_ooxml_uri`).
  - For each sheet part: parse `<c r="A1" s="3">` cells; convert `r` to `(row, col)` (0-based absolute); cells with no `r` fall back to positional coordinates within their `<row>` (column index = position in the row); cells with no `s` attribute map to style 0. Store the resolved code string.
  - Non-fatal errors (part read failure, malformed XML, out-of-range style index): `log::warn!` and return `Ok(None)` (KTD3). `ConvertError::ResourceLimit` propagates (R9a).
- **Test scenarios (unit tests in `src/formats/sheet/styles.rs`, using the in-repo zip-writing helper pattern from `xlsx_with_merge`):**
  - T2.1 A zip with `styles.xml` (custom `0.00%` at numFmtId 164, cellXfs referencing it), workbook/rels, and a sheet with `<c r="A1" s="1">` yields `code("S", 0, 0) == Some("0.00%")`.
  - T2.2 A cell with no `s` attribute resolves to style 0 (builtin `General` -> `None`).
  - T2.3 A zip with no `styles.xml` returns `Ok(None)`.
  - T2.4 A zip with malformed `styles.xml` returns `Ok(None)` (logged), not an error (R9).
  - T2.5 A style index pointing past `cellXfs` length returns `Ok(None)` (KTD3).
  - T2.6 Multi-sheet workbook maps each sheet name to its own cell map.
  - T2.7 An `xf` with `numFmtId="164"` and `applyNumberFormat="0"` yields `None` for its cells (General).
  - T2.8 A sheet whose cells omit `r` (positional only) still maps to the correct coordinates.
  - T2.9 An oversized `styles.xml` part tripping `limits::MAX_ENTRY_BYTES` propagates `ConvertError::ResourceLimit` (R9a).
- **Verification:** `cargo test styles` passes; clippy clean.

### U3. Wiring into the parse loop

- **Goal:** Apply the format lookup in `parse()` so formatted numeric cells render through the engine.
- **Requirements:** R1-R12.
- **KTDs:** KTD2, KTD3, KTD6.
- **Files:**
  - `src/formats/sheet/mod.rs`: in `parse()`, build `CellFormats` from the bytes when the workbook is `Sheets::Xlsx` (covers xlsx, xlsm, xlam per calamine auto.rs:42); pass the absolute cell coordinate (start + relative) and code into the cell loop; extend `format_data(data, code: Option<&str>)` to consult the renderer for `Data::Float`/`Data::Int` when `code` is `Some` and non-`General`; everything else unchanged.
- **Approach:**
  - Keep the signature change local: `format_data(data, code)`; the date/time and string branches of `format_data` ignore `code` (R7, R11).
  - `Data::Float(f)` with a renderable code: `numfmt::render(f, code)` -> text, `None` -> `format_float(f)`.
  - `Data::Int(i)` with a renderable code: `numfmt::render(i as f64, code)` -> text, `None` -> `i.to_string()` (documented 2^53/Excel 15-digit parity).
  - Cells whose absolute coordinate is absent from the lookup map render as today (R6).
  - Coordinate conversion: absolute `(start.0 + r as u32, start.1 + c as u32)`.
- **Test scenarios (unit tests in `src/formats/sheet/mod.rs`):**
  - T3.1 Extend the `xlsx_with_merge` helper family with a styles-bearing xlsx builder; a sheet with `A1=0.65` (numFmtId 164, `0.00%`) converts to markdown containing `65.00%`.
  - T3.2 Same builder without styles.xml: `A1` renders `0.65` (R9, R6).
  - T3.3 A `General`-formatted float renders via `format_float` exactly as today (R6).
  - T3.4 A formatted int cell (`#,##0`, value 1234567) renders `1,234,567`.
  - T3.5 A date cell in the same workbook still renders via the `Data::DateTime` path (R7) — regression guard.
  - T3.6 A percent cell next to a plain cell both render correctly in the same table.
  - T3.7 A string cell containing `5.75%` renders unchanged (AE7).
- **Verification:** `cargo test --locked` passes with the new unit tests; `cargo fmt --all --check` clean.

### U4. Fixtures and snapshot coverage

- **Goal:** Add a committed corpus fixture and snapshot proving the end-to-end behavior, and review the existing-snapshot deltas.
- **Requirements:** R1-R5, R7, R8, R9, R12.
- **Files:**
  - `tests/gen_fixtures.py`: new `numberformats_xlsx()` builder (mirrors `merged_xlsx()` at `tests/gen_fixtures.py:872`) writing `tests/fixtures/xlsx/handmade-numberformats.xlsx` with: a custom `0.00%` numFmt (id 164), builtin `$#,##0.00` (id 4), builtin `#,##0` (id 3), `0.00` (id 2), `0.00E+00` (id 11), a negative-section custom `0.00;(0.00)` (id 165), a currency-locale `[$$-409]#,##0.00` (id 166), a fraction format (id 12) cell, a `General` float, a string cell `5.75%`, a date cell, and a format-code literal cell with a pipe character; register the builder in `main()`.
  - `tests/fixtures/xlsx/handmade-numberformats.xlsx`: generated artifact (regenerate via `python3 tests/gen_fixtures.py --skip-office` if the rest of the corpus allows, otherwise emit the fixture by hand from the same XML).
  - `tests/snapshots/snapshots__xlsx__handmade-numberformats.xlsx.snap`: new snapshot via `INSTA_UPDATE` run, then reviewed.
- **Approach:**
  - The handmade xlsx encodes: header row of labels, then rows of value/format pairs covering AE1-AE7 and the fallback cases.
  - Run the snapshot suite; review every diff, including the certain `sheet.xlsx` diff (Assumptions); accept only diffs that match Excel-displayed values.
  - Include one cell whose format code contains a pipe character inside a quoted literal to pin R12 escaping through the existing pipeline.
- **Test scenarios:**
  - T4.1 Snapshot `handmade-numberformats.xlsx` contains `65.00%`, `$56,701.03`, `1,234.50`, `1,234,567`, `1.23E+05`, `(1.50)`, `$1,234.50` (currency-locale cell), the fraction-formatted value, raw `0.653035934239184`-style fallback only where the renderer rejects the code, `5.75%` for the string cell, and the ISO date.
  - T4.2 The pre-existing `sheet.xlsx` snapshot diff is justified cell-by-cell against Excel display semantics.
- **Verification:** `cargo test --locked` (snapshot suite) green; `cargo fmt --all --check` and clippy clean; `python3 tests/gen_fixtures.py --skip-office` regenerates the fixture byte-identically (deterministic ZIP timestamps).

## Verification Contract

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked`
- Manual cross-check (dev only, not committed): openpyxl writes `0.00%` / `$#,##0.00` / `#,##0` / `[$$-409]#,##0.00` cells, `anydoc::to_markdown` output matches Excel display; the issue #64 example values render per AE1, AE2, AE7; a tie value (1.005 with `0.00`) matches Excel's displayed output.
- The full snapshot corpus must remain green; every snapshot delta must be traced to the format change and reviewed against Excel display semantics.

## Definition of Done

**Global**

- Issue #64's three example cells render their formatted values (AE1, AE2, AE7).
- All R1-R12 hold; renderer-rejected format families fall back to raw (R8) with a logged, non-fatal recovery path (R9), and resource-limit errors propagate (R9a).
- Verification Contract passes in a clean tree.
- No dead or experimental code remains: any approach variant abandoned during implementation is removed, not left behind commented or feature-gated.
- One pinned dependency added (`ssfmt 0.1.2`, `default-features = false`); no other runtime dependencies; no public API changes.
- Known limitation recorded in the PR description: formatted output is display-rounded to the format's precision (values differing below that precision become indistinguishable); General cells keep exact raw rendering.

**Per unit**

- U1: renderer adapter unit tests T1.1-T1.13 green; dependency pinned.
- U2: styles reader unit tests T2.1-T2.9 green.
- U3: parse-loop unit tests T3.1-T3.7 green; `format_data` signature change complete with all call sites updated.
- U4: fixture committed, snapshot committed and reviewed, generator regenerates byte-identically.
