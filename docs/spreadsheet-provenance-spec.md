# Spreadsheet provenance in the document model

Status: proposed implementation spec

Related issue: [Expose worksheet identity and source coordinates in `to_document()` #10](https://github.com/firecrawl/anydoc/issues/10)

## Problem

The spreadsheet readers already know the source worksheet and the original row
and column of each populated cell. During canonical grid construction, however,
hidden rows and columns are removed, merged regions are remapped, and the
resulting `Table` only exposes normalized grid coordinates. A caller therefore
cannot map `table.grid[0][0]` back to a worksheet cell such as `Data Sheet!C3`.

The worksheet name is also currently represented only by a Markdown heading
when a workbook has more than one visible sheet. This makes worksheet identity
dependent on the rendering path and loses it for single-sheet workbooks.

## Scope

This change applies to the structured document model returned by
`to_document()`. It covers all spreadsheet containers currently parsed by
anydoc:

- `.xlsx` and `.xlsm` (SpreadsheetML);
- `.xlsb` (binary SpreadsheetML);
- `.xls` (BIFF/OLE);
- `.ods` (OpenDocument Spreadsheet).

CSV is intentionally out of scope: it has no worksheet identity or workbook
coordinate system. Markdown rendering, format detection, hidden-sheet policy,
and existing resource limits are otherwise unchanged.

## Goals

1. Preserve worksheet identity independently of Markdown rendering.
2. Give every returned spreadsheet origin cell an exact source range.
3. Keep normalized `Table.grid` behavior unchanged for existing consumers.
4. Expose the same provenance semantics through Rust, Node.js, Python, and
   WASM bindings.
5. Preserve provenance through cropping, hidden row/column filtering, merged
   cells, and ODS repeat expansion.

## Non-goals

- Preserve formulas, comments, styles, drawings, charts, or other spreadsheet
  features that are not currently represented by the document model.
- Change the Markdown output to include sheet names or coordinates.
- Add file paths or file hashes to the model. `to_document()` accepts bytes and
  does not have a reliable source path.
- Add provenance for non-spreadsheet tables in this change.

## Coordinate contract

Coordinates are zero-based and ranges are inclusive at both ends.

```rust
SpreadsheetCoordinate { row: 2, column: 2 } // C3
SpreadsheetRange {
    start: SpreadsheetCoordinate { row: 2, column: 2 },
    end: SpreadsheetCoordinate { row: 2, column: 2 },
} // C3:C3
```

`sheet_index` is the zero-based position in the source workbook/table order,
including hidden Excel sheets. Hidden sheets remain omitted from the returned
document exactly as they are today; their presence must not renumber the
visible sheets that follow them.

The table range is the smallest source-coordinate bounding range containing
all returned origin cells and the complete source ranges of merged origins.
It is not derived from the normalized grid dimensions. A range can contain
holes when hidden rows or columns were omitted.

For a normal spreadsheet cell, `Cell.source` is a one-cell range. For a merged
origin, it is the complete source merge range, even if hidden rows or columns
cause the normalized span to be smaller. Covered grid slots continue to point
to their normalized origin; the origin cell carries the source range.

Materialized empty padding cells also carry their original one-cell range. A
cell is allowed to have no source only when it was not produced by a
spreadsheet parser.

## Proposed Rust model

The following names are the proposed public API; exact naming can be adjusted
during upstream review without changing the semantics.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpreadsheetCoordinate {
    pub row: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpreadsheetRange {
    pub start: SpreadsheetCoordinate,
    pub end: SpreadsheetCoordinate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadsheetSource {
    pub sheet_index: u32,
    pub sheet_name: String,
    pub range: SpreadsheetRange,
}

pub struct Table {
    pub grid: Vec<Vec<CellSlot>>,
    pub header_rows: usize,
    pub kind: TableKind,
    pub source: Option<SpreadsheetSource>,
}

pub struct Cell {
    pub blocks: Vec<Block>,
    pub col_span: u32,
    pub row_span: u32,
    pub source: Option<SpreadsheetRange>,
}
```

`Table.source` is `Some` for tables produced by spreadsheet parsers and
`None` for DOC/DOCX, RTF, ODT, presentation, HTML-derived, and CSV tables.
The worksheet identity is stored once at table level; each cell stores only its
range to avoid duplicating the sheet name for every cell.

## Parser changes

### XLSX, XLSB, and XLS

`xlsx`, `xlsb`, and `xls` already share `SheetContent` and `build_table()`.
The implementation should:

1. carry a `SpreadsheetSource` sheet identity into `build_table()`;
2. retain the original `(row, column)` key while building each origin cell;
3. retain the original merge rectangle separately from its normalized span;
4. attach a one-cell range to generated empty cells;
5. compute the table bounding range from source coordinates before returning;
6. enumerate sheet identity before filtering hidden sheets.

The normalized grid, merge behavior, number formatting, and hidden-content
policy must remain unchanged.

### ODS

`parse_spreadsheet()` and `parse_table()` need explicit logical source cursors
because ODS can encode a single cell or row with repeat attributes.

- `number-rows-repeated` increments the source row for every emitted row;
- `number-columns-repeated` increments the source column for every emitted
  cell;
- `number-columns-spanned` and `number-rows-spanned` produce an inclusive
  source range covering the full origin span;
- `covered-table-cell` remains a covered slot when it belongs to an origin;
- a stray covered cell, if recovered as an empty origin, receives its source
  coordinate;
- repeat expansion continues to use the existing safety budgets.

## Binding contract

All bindings expose the same data with their established naming conventions.

| Binding | Public shape |
|---|---|
| Rust | `SpreadsheetCoordinate`, `SpreadsheetRange`, `SpreadsheetSource`; optional `Table.source` and `Cell.source` |
| Node.js | `sheetIndex`, `sheetName`, `range`, `start`, `end`, `row`, `column`; optional `source` |
| Python | `sheet_index`, `sheet_name`, `range`, `start`, `end`, `row`, `column`; optional `source` |
| WASM | camelCase JSON/TypeScript shape matching Node.js |

The generated Node declaration, Python stub, and WASM TypeScript section must
be updated together with their conversion code. Non-spreadsheet output should
continue to expose `None`/`undefined`/omitted optional provenance according to
each binding's existing optional-field convention.

## Test strategy

### Core parser tests

Add focused cases for each supported container:

1. a single-sheet workbook with one value at `C3`;
2. multiple sheets with distinct names and a hidden sheet before a visible
   sheet;
3. a non-`A1` used range such as `D11:E12`;
4. hidden rows and columns that change normalized grid positions;
5. merged cells whose source range differs from normalized span;
6. ODS repeated rows, repeated columns, and covered cells;
7. empty materialized padding cells;
8. non-spreadsheet and CSV tables with no provenance.

The XLSX, XLSB, and XLS tests should exercise the shared grid path while still
asserting each format's reader. ODS tests should assert repeat and span cursor
behavior independently.

### Regression tests

- Existing Markdown snapshots and renderer tests remain unchanged.
- Existing table/grid invariants continue to pass.
- Binding tests assert the new fields and their absence for non-spreadsheet
  tables.
- Existing malformed-input and resource-limit tests remain green.

## Acceptance criteria

The change is complete when:

- `to_document()` returns `Data Sheet!C3:C3` and the cell range for a
  single-value `C3` workbook;
- every returned spreadsheet origin cell is traceable to its original sheet
  and inclusive source range;
- `.xlsx`, `.xlsm`, `.xlsb`, `.xls`, and `.ods` follow the same coordinate
  contract;
- normalized grid shape and Markdown output do not regress;
- all four public surfaces expose the new metadata consistently;
- tests cover hidden coordinates, merges, repeats, and empty padding;
- no CSR project files are modified by this upstream change.

## Compatibility and review risks

`Table` and `Cell` currently expose public fields, so adding public fields can
break downstream code that constructs them with struct literals. This is an
intentional public-model change required by the issue and must be called out
in the PR description and release notes. The optional values preserve runtime
behavior for non-spreadsheet documents.

## Implementation order

1. Add and document the shared provenance types and optional model fields.
2. Thread sheet identity and source ranges through the shared Excel grid path.
3. Add ODS logical source cursors and range tracking.
4. Update Rust, Node.js, Python, and WASM bindings.
5. Add parser, binding, and regression tests.
6. Review the complete diff, run the relevant test matrix, and prepare the
   upstream PR description.
