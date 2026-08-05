use crate::error::ConvertError;
use crate::model::{Block, Inline, inlines_are_empty};
use crate::package::limits;
use std::collections::HashMap;

/// Canonical table grid. **Invariant:** every logical grid position appears
/// exactly once - content and spans exist only on the origin slot, and each
/// position covered by a span holds a [`CellSlot::Covered`] marker pointing
/// back at its origin. Frontends construct grids through one internal builder
/// that enforces this, so a `Table` handed to a consumer always holds.
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// Rows of slots. Rows may differ in length when the source is ragged.
    pub grid: Vec<Vec<CellSlot>>,
    /// Number of leading rows that are header rows (0 = no header).
    pub header_rows: usize,
    /// Whether the source used this table for data or for layout.
    pub kind: TableKind,
}

/// What a table is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableKind {
    /// A real data table.
    #[default]
    Data,
    /// Layout scaffolding (text boxes, positioning tables); renderers may
    /// unwrap trivial layout tables.
    Layout,
}

/// One position in a [`Table::grid`]: either a cell or the shadow of one.
#[derive(Debug, Clone)]
pub enum CellSlot {
    /// The cell itself, holding the content and the span extents.
    Origin(Cell),
    /// A position swallowed by a span, pointing back at the origin that
    /// covers it.
    Covered {
        /// Row of the covering origin.
        origin_row: usize,
        /// Column of the covering origin.
        origin_col: usize,
    },
}

/// A table cell and the extent it spans.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    /// The cell's content.
    pub blocks: Vec<Block>,
    /// Columns covered, at least 1.
    pub col_span: u32,
    /// Rows covered, at least 1.
    pub row_span: u32,
}

impl Cell {
    /// A cell spanning one position.
    pub fn new(blocks: Vec<Block>) -> Self {
        Cell { blocks, col_span: 1, row_span: 1 }
    }

    /// A one-paragraph cell spanning one position.
    pub fn from_inlines(inlines: Vec<Inline>) -> Self {
        Cell::new(vec![Block::Paragraph(inlines)])
    }

    /// A cell covering `col_span` by `row_span` positions; either span given
    /// as 0 is raised to 1.
    pub fn spanning(blocks: Vec<Block>, col_span: u32, row_span: u32) -> Self {
        Cell { blocks, col_span: col_span.max(1), row_span: row_span.max(1) }
    }

    /// True when the cell holds nothing that would render: only paragraphs
    /// count toward emptiness, so a cell with a table or list in it is not
    /// empty even if that content is blank.
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|b| match b {
            Block::Paragraph(inlines) => inlines_are_empty(inlines),
            _ => false,
        })
    }
}

impl Table {
    /// Build a plain span-less table from rows of cells (spreadsheets, CSV).
    pub fn from_rows(rows: Vec<Vec<Cell>>, header_rows: usize, kind: TableKind) -> Table {
        let mut b = GridBuilder::new();
        for row in rows {
            b.next_row();
            for cell in row {
                // Span-less placement never charges the expansion budget.
                b.place(Cell { col_span: 1, row_span: 1, ..cell })
                    .expect("span-less placement cannot exceed the expansion budget");
            }
        }
        let mut table = b.finish(kind);
        table.header_rows = header_rows;
        table
    }

    /// True when the table is a single origin cell (any covered padding aside).
    pub fn is_single_cell(&self) -> bool {
        self.grid.len() == 1
            && self.grid[0].len() == 1
            && matches!(self.grid[0][0], CellSlot::Origin(_))
    }
}

/// Sole constructor for [`Table`] grids. Enforces the exactly-once invariant:
/// spans register their covered positions, later placements skip over them,
/// and overlapping spans are clamped rather than double-counted.
#[derive(Debug, Default)]
pub struct GridBuilder {
    grid: Vec<Vec<CellSlot>>,
    /// Positions in future rows covered by an earlier row-spanning origin.
    pending: HashMap<(usize, usize), (usize, usize)>,
    /// Covered positions claimed by span expansion, charged against
    /// [`limits::MAX_EXPANSION`] *before* any per-position work so a tiny
    /// document carrying a huge span cannot force unbounded insertions.
    expansion: u64,
    /// When set, [`GridBuilder::finish`] keeps trailing rows that consist
    /// only of covered positions instead of trimming them as filler.
    keep_covered_extent: bool,
}

impl GridBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_row(&mut self) {
        self.grid.push(Vec::new());
    }

    fn row_index(&mut self) -> usize {
        if self.grid.is_empty() {
            self.grid.push(Vec::new());
        }
        self.grid.len() - 1
    }

    /// Materialize pending covered positions at the cursor, so the next
    /// placement lands on the first genuinely free column.
    fn skip_pending(&mut self, row: usize) {
        while let Some((origin_row, origin_col)) = self.pending.remove(&(row, self.grid[row].len()))
        {
            self.grid[row].push(CellSlot::Covered { origin_row, origin_col });
        }
    }

    /// Place a cell at the next free position of the current row. The cell's
    /// spans register covered positions as *pending*: an explicitly written
    /// covered cell (ODF) consumes one via [`GridBuilder::covered`], while
    /// producers that omit them (HTML, OOXML) have the positions
    /// materialized automatically when the next cell is placed.
    ///
    /// The complete span area is charged against the expansion budget before
    /// any expansion happens; `ResourceLimit` when the budget is exceeded.
    pub fn place(&mut self, cell: Cell) -> Result<(), ConvertError> {
        let area = (cell.col_span.max(1) as u64) * (cell.row_span.max(1) as u64);
        self.expansion = self.expansion.saturating_add(area - 1);
        if self.expansion > limits::MAX_EXPANSION {
            return Err(ConvertError::ResourceLimit {
                limit: "max_expansion",
                detail: "table span expansion exceeds the content budget".into(),
            });
        }
        let row = self.row_index();
        self.skip_pending(row);
        let col = self.grid[row].len();
        let mut col_span = cell.col_span.max(1) as usize;
        let mut row_span = cell.row_span.max(1) as usize;
        // Clamp the spans to positions this origin can actually own - an
        // overlap with an earlier span would break the exactly-once
        // invariant (the stored span must match the covered markers).
        for dc in 1..col_span {
            if self.pending.contains_key(&(row, col + dc)) {
                col_span = dc;
                break;
            }
        }
        'rows: for dr in 1..row_span {
            for dc in 0..col_span {
                if self.pending.contains_key(&(row + dr, col + dc)) {
                    row_span = dr;
                    break 'rows;
                }
            }
        }
        self.grid[row].push(CellSlot::Origin(Cell {
            col_span: col_span as u32,
            row_span: row_span as u32,
            ..cell
        }));
        for dr in 0..row_span {
            for dc in 0..col_span {
                if (dr, dc) == (0, 0) {
                    continue;
                }
                self.pending.insert((row + dr, col + dc), (row, col));
            }
        }
        Ok(())
    }

    /// Consume one explicitly-written covered position (ODF
    /// `covered-table-cell`). Returns `false` when no span accounts for the
    /// position - the stray marker then becomes an empty cell.
    pub fn covered(&mut self) -> bool {
        let row = self.row_index();
        let col = self.grid[row].len();
        match self.pending.remove(&(row, col)) {
            Some((origin_row, origin_col)) => {
                self.grid[row].push(CellSlot::Covered { origin_row, origin_col });
                true
            }
            None => {
                self.grid[row].push(CellSlot::Origin(Cell::default()));
                false
            }
        }
    }

    /// Preserve trailing rows that consist only of covered positions, rather
    /// than trimming them as filler. Callers whose covered positions are a
    /// source-declared extent - spreadsheet merges cover real cells even
    /// when those cells are empty - opt in so `finish` does not collapse the
    /// region down to its origin row.
    pub fn keep_covered_extent(&mut self) {
        self.keep_covered_extent = true;
    }

    pub fn finish(mut self, kind: TableKind) -> Table {
        // Materialize every pending covered position in surviving rows,
        // including tails behind a gap (a short row under a span at a later
        // column): intervening slots fill with empty cells so each covered
        // marker lands on its true column.
        let mut by_row: HashMap<usize, Vec<usize>> = HashMap::new();
        for &(row, col) in self.pending.keys() {
            if row < self.grid.len() {
                by_row.entry(row).or_default().push(col);
            }
        }
        for (row, mut cols) in by_row {
            cols.sort_unstable();
            for col in cols {
                while self.grid[row].len() < col {
                    self.grid[row].push(CellSlot::Origin(Cell::default()));
                }
                let (origin_row, origin_col) = self.pending.remove(&(row, col)).unwrap();
                // Placement always consumes pending slots at the cursor, so a
                // still-pending position can only sit at or past the row end.
                debug_assert_eq!(self.grid[row].len(), col);
                self.grid[row].push(CellSlot::Covered { origin_row, origin_col });
            }
        }
        // Drop trailing all-empty rows. Covered slots count as filler, unless
        // the caller opted to preserve a source-declared covered extent.
        while self.grid.last().is_some_and(|r| {
            r.iter().all(|s| match s {
                CellSlot::Origin(c) => c.is_empty(),
                CellSlot::Covered { .. } => !self.keep_covered_extent,
            })
        }) {
            self.grid.pop();
        }
        // Clamp stored spans to the surviving grid so every claimed
        // position is backed by a covered marker (exactly-once invariant).
        let rows = self.grid.len();
        for r in 0..rows {
            let width = self.grid[r].len();
            for c in 0..width {
                if let CellSlot::Origin(cell) = &mut self.grid[r][c] {
                    cell.row_span = cell.row_span.min((rows - r) as u32);
                    cell.col_span = cell.col_span.min((width - c) as u32);
                }
            }
        }
        Table { grid: self.grid, header_rows: 0, kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_cell(t: &str) -> Cell {
        Cell::from_inlines(vec![Inline::plain(t)])
    }

    fn widths(table: &Table) -> Vec<usize> {
        table.grid.iter().map(|r| r.len()).collect()
    }

    #[test]
    fn col_span_covers_positions() {
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(Cell::spanning(vec![], 2, 1)).unwrap();
        b.place(text_cell("end")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![3]);
        assert!(matches!(t.grid[0][1], CellSlot::Covered { origin_row: 0, origin_col: 0 }));
    }

    #[test]
    fn row_span_skips_next_row_position() {
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(Cell::spanning(vec![], 1, 2)).unwrap();
        b.place(text_cell("b1")).unwrap();
        b.next_row();
        b.place(text_cell("b2")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![2, 2]);
        assert!(matches!(t.grid[1][0], CellSlot::Covered { origin_row: 0, origin_col: 0 }));
        assert!(matches!(&t.grid[1][1], CellSlot::Origin(c) if !c.is_empty()));
    }

    #[test]
    fn explicit_covered_consumes_exactly_one() {
        // ODF shape: spanning cell followed by its explicit covered marker.
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(Cell::spanning(vec![], 2, 1)).unwrap();
        assert!(b.covered());
        b.place(text_cell("end")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![3]);
    }

    #[test]
    fn omitted_covered_positions_materialize() {
        // HTML/OOXML shape: no explicit covered cells at all.
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(Cell::spanning(vec![], 2, 1)).unwrap();
        b.place(text_cell("end")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![3]);
        assert!(matches!(t.grid[0][1], CellSlot::Covered { .. }));
        assert!(matches!(&t.grid[0][2], CellSlot::Origin(c) if !c.is_empty()));
    }

    #[test]
    fn covered_row_consumption() {
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(Cell::spanning(vec![], 1, 2)).unwrap();
        b.place(text_cell("b1")).unwrap();
        b.next_row();
        assert!(b.covered());
        b.place(text_cell("b2")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![2, 2]);
    }

    #[test]
    fn stray_covered_becomes_empty_cell() {
        let mut b = GridBuilder::new();
        b.next_row();
        assert!(!b.covered());
        b.place(text_cell("x")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![2]);
    }

    fn spanning_text(t: &str, cols: u32, rows: u32) -> Cell {
        Cell::spanning(vec![Block::Paragraph(vec![Inline::plain(t)])], cols, rows)
    }

    /// Every position a stored span claims must hold a covered marker
    /// pointing back at that origin.
    fn assert_spans_backed(t: &Table) {
        for (r, row) in t.grid.iter().enumerate() {
            for (c, slot) in row.iter().enumerate() {
                if let CellSlot::Origin(cell) = slot {
                    for dr in 0..cell.row_span as usize {
                        for dc in 0..cell.col_span as usize {
                            if (dr, dc) == (0, 0) {
                                continue;
                            }
                            let covered = t.grid.get(r + dr).and_then(|row| row.get(c + dc));
                            assert!(
                                matches!(covered,
                                    Some(CellSlot::Covered { origin_row, origin_col })
                                        if *origin_row == r && *origin_col == c),
                                "span of origin ({r},{c}) not backed at ({},{})",
                                r + dr,
                                c + dc
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn overlapping_spans_clamp_the_late_origin() {
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(spanning_text("tall", 1, 3)).unwrap();
        b.place(text_cell("a")).unwrap();
        b.next_row();
        assert!(b.covered());
        b.place(spanning_text("wide", 2, 2)).unwrap();
        b.next_row();
        assert!(b.covered());
        b.place(text_cell("tail")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_spans_backed(&t);
    }

    #[test]
    fn conflicting_span_rectangles_stay_consistent() {
        // An origin placed where covered positions belong lands after them
        // and keeps a span consistent with what it owns.
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(spanning_text("block", 2, 2)).unwrap(); // owns (1,0),(1,1)
        b.next_row();
        b.place(spanning_text("late", 2, 1)).unwrap();
        let t = b.finish(TableKind::Data);
        assert!(matches!(t.grid[1][0], CellSlot::Covered { .. }));
        assert!(matches!(t.grid[1][1], CellSlot::Covered { .. }));
        assert!(matches!(&t.grid[1][2], CellSlot::Origin(c) if c.col_span == 2));
        assert_spans_backed(&t);
    }

    #[test]
    fn trimmed_rows_clamp_surviving_spans() {
        // A row-span whose covered rows are trimmed as trailing filler must
        // not keep claiming them.
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(Cell::spanning(vec![Block::Paragraph(vec![Inline::plain("x")])], 1, 3)).unwrap();
        b.next_row();
        b.next_row();
        let t = b.finish(TableKind::Data);
        assert_eq!(t.grid.len(), 1);
        assert!(matches!(&t.grid[0][0], CellSlot::Origin(c) if c.row_span == 1));
        assert_spans_backed(&t);
    }

    #[test]
    fn huge_span_hits_the_expansion_budget_before_expanding() {
        let mut b = GridBuilder::new();
        b.next_row();
        let err = b.place(Cell::spanning(vec![], u32::MAX, u32::MAX)).unwrap_err();
        assert!(matches!(err, ConvertError::ResourceLimit { limit: "max_expansion", .. }));
    }

    #[test]
    fn accumulated_spans_hit_the_expansion_budget() {
        // Preload the counter instead of allocating millions of real pending
        // entries (the global allocation counter feeds the R22 memory test).
        let mut b = GridBuilder::new();
        b.expansion = limits::MAX_EXPANSION - 10;
        b.next_row();
        b.place(Cell::spanning(vec![], 3, 3)).unwrap(); // 8 more: still within
        let err = b.place(Cell::spanning(vec![], 2, 2)).unwrap_err(); // 3 more: over
        assert!(matches!(err, ConvertError::ResourceLimit { limit: "max_expansion", .. }));
    }

    #[test]
    fn covered_tail_behind_short_row_gap_materializes() {
        // M5: a row-span at a late column must stay backed even when the
        // next row stops short of it — the gap fills with empty cells.
        let mut b = GridBuilder::new();
        b.next_row();
        for _ in 0..5 {
            b.place(text_cell("h")).unwrap();
        }
        b.place(spanning_text("tall", 1, 2)).unwrap();
        b.next_row();
        b.place(text_cell("only")).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(t.grid[1].len(), 6);
        assert!(matches!(t.grid[1][5], CellSlot::Covered { origin_row: 0, origin_col: 5 }));
        assert!(matches!(&t.grid[0][5], CellSlot::Origin(c) if c.row_span == 2));
        assert_spans_backed(&t);
    }

    #[test]
    fn trailing_empty_rows_trimmed() {
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(text_cell("x")).unwrap();
        b.next_row();
        b.place(Cell::default()).unwrap();
        let t = b.finish(TableKind::Data);
        assert_eq!(widths(&t), vec![1]);
    }

    #[test]
    fn covered_extent_rows_survive_when_preserved() {
        // A merge's covered rows are real structure: with the flag set they
        // are kept, and the origin keeps its full span.
        let mut b = GridBuilder::new();
        b.keep_covered_extent();
        b.next_row();
        b.place(spanning_text("title", 2, 3)).unwrap();
        b.next_row();
        assert!(b.covered());
        assert!(b.covered());
        b.next_row();
        assert!(b.covered());
        assert!(b.covered());
        let t = b.finish(TableKind::Data);
        assert_eq!(t.grid.len(), 3);
        assert!(matches!(&t.grid[0][0], CellSlot::Origin(c) if c.row_span == 3 && c.col_span == 2));
        assert_spans_backed(&t);
    }

    #[test]
    fn covered_extent_rows_trim_without_the_flag() {
        // The default stays conservative: phantom covered rows are filler and
        // get trimmed, clamping the span to what actually remains.
        let mut b = GridBuilder::new();
        b.next_row();
        b.place(spanning_text("title", 2, 3)).unwrap();
        b.next_row();
        assert!(b.covered());
        assert!(b.covered());
        b.next_row();
        assert!(b.covered());
        assert!(b.covered());
        let t = b.finish(TableKind::Data);
        assert_eq!(t.grid.len(), 1);
        assert!(matches!(&t.grid[0][0], CellSlot::Origin(c) if c.row_span == 1));
    }
}
