# Spreadsheet provenance implementation plan

This plan implements `docs/spreadsheet-provenance-spec.md` on the
`feat/sheet-provenance` branch.

## Allowed scope

Modify only the shared document model, spreadsheet parsers, ODS parser,
bindings, generated binding declarations, relevant tests, and the two
spreadsheet provenance documents.

Do not change Markdown rendering, format detection, hidden-content policy,
resource limits, unrelated parsers, the CSR project, or dependencies unless a
test proves a narrowly scoped change is required.

## Commit plan

### Commit 1 — public model and source-coordinate primitives

Files:

- `src/model/source.rs` (new);
- `src/model/mod.rs`;
- `src/model/table.rs`;
- model and renderer construction tests that use explicit struct literals.

Deliverables:

- zero-based inclusive `SpreadsheetCoordinate` and `SpreadsheetRange`;
- table-level `SpreadsheetSource` containing original sheet index, name, and
  bounding range;
- optional `Table.source` and `Cell.source` fields;
- constructors/defaults preserve `None` for non-spreadsheet tables;
- no renderer behavior change.

### Commit 2 — Excel provenance through the shared grid path

Files:

- `src/formats/sheet/xlsx.rs`;
- `src/formats/sheet/xlsb.rs`;
- `src/formats/sheet/xls.rs`;
- shared spreadsheet tests as needed.

Deliverables:

- carry source sheet identity before hidden-sheet filtering;
- attach original one-cell ranges to ordinary and generated empty cells;
- attach original merge rectangles to merged origins;
- compute table source ranges from original coordinates;
- preserve current normalized grid and Markdown output.

### Commit 3 — ODS provenance with repeat-aware cursors

Files:

- `src/formats/odf/table.rs`;
- `src/formats/odf/mod.rs` only if sheet-order context must be threaded there;
- ODS parser tests.

Deliverables:

- source row/column cursors independent from normalized `GridBuilder` cursors;
- correct ranges for repeated rows, repeated columns, covered cells, and
  row/column spans;
- source range for each returned ODS table;
- existing repeat and expansion limits unchanged.

### Commit 4 — bindings, declarations, fixtures, and regression coverage

Files:

- `node/src/document.rs`, `node/index.d.ts`;
- `python/src/document.rs`, `python/anydoc/_anydoc.pyi`;
- `wasm/src/document.rs`, `wasm/src/typescript.rs`;
- binding tests and any generated declaration updates;
- README/API notes if the existing project convention requires them.

Deliverables:

- equivalent Rust/Node/Python/WASM shapes;
- optional provenance is absent/null according to each binding's existing
  convention for non-spreadsheet documents;
- binding smoke tests for spreadsheet and DOCX/CSV cases;
- complete diff and compatibility review.

## Fixture design

The core parser tests already use compact in-module builders instead of
checking binary fixtures into the repository. Extend that pattern so each
format tests its real reader and the shared model contract.

### XLSX fixture

Build a workbook with:

- visible sheet `Data Sheet` containing `C3 = "value"`;
- another visible sheet with a different name;
- a hidden sheet between them to verify original sheet indices;
- a visible sheet region starting at `D11:E12`;
- a merge such as `F1:G2` and a hidden row or column intersecting a merge.

Assertions:

- table source has the original sheet name/index and inclusive bounding range;
- the normalized first cell still has its original source coordinate;
- merged origin retains `F1:G2` even if normalized span changes;
- hidden sheets remain omitted and do not change source indices.

### XLSB fixture

Reuse the existing synthetic OPC workbook builder and binary record helpers.
Create the same logical workbook cases as XLSX, using `BrtBundleSh`,
`BrtRowHdr`, cell records, and `BrtMergeCell`.

Assertions must compare the provenance shape to the XLSX expectations without
assuming the container's internal part names.

### XLS fixture

Reuse the existing OLE/BIFF builder and records. Create visible and hidden
`BOUNDSHEET` entries, BIFF cell records at non-zero row/column positions, and a
`MERGEDCELLS` rectangle.

Assertions must verify BIFF row/column values are retained after the shared
grid builder crops the table.

### ODS fixture

Build a minimal `content.xml` with:

- a table named `Data Sheet`;
- leading empty cells before a populated cell;
- `number-columns-repeated` for a repeated value;
- `number-rows-repeated` for repeated rows;
- a row/column span and explicit covered cells;
- a second table to verify table order and identity.

Assertions must verify that repeat expansion receives distinct source
coordinates, spans retain their full source ranges, and normalized covered
slots still point to the correct origin.

### Binding fixture policy

Bindings should consume the core model fixtures or construct the smallest
in-memory documents possible. They must assert field names and value shapes,
not duplicate parser logic.

## Verification commands

Run, when the required toolchains are available:

```text
cargo fmt --all -- --check
cargo test --workspace
npm test --prefix node
python -m pytest python/tests
```

If a binding requires a generated/native artifact or a toolchain is missing,
record the exact command and environment limitation rather than treating the
unrun test as passing.

## Stop conditions

Pause implementation and revisit the Spec if any of these occur:

- source coordinates cannot be defined consistently across a format;
- preserving provenance requires changing Markdown output;
- hidden-sheet behavior would expose previously omitted content;
- a new dependency appears necessary;
- a binding cannot represent the core model without a format-specific shape;
- a public API compatibility issue requires a different model design.
