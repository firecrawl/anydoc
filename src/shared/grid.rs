//! Edge-based table assembly shared by RTF (`\cellx` boundaries) and binary
//! DOC (TAP `rgdxaCenter` boundaries): grid columns come from the clustered
//! union of every row's cell edges, horizontal merges collapse into column
//! spans, and vertical chains start at an explicit restart flag and are
//! continued by continuation cells whose boundaries match the chain's.

use crate::error::ConvertError;
use crate::model::{Block, Cell, GridBuilder, TableKind};
use crate::shared::header::resolve_header_rows;
use std::collections::HashMap;

/// Merge/boundary properties of one cell.
#[derive(Debug, Clone, Copy, Default)]
pub struct CellProp {
    /// First cell of a horizontally merged set.
    pub merge_first: bool,
    /// Horizontal continuation: folds into the preceding first cell.
    pub merge_cont: bool,
    /// First cell of a vertically merged chain.
    pub vmerge_first: bool,
    /// Vertical continuation: covered by the chain's origin above.
    pub vmerge_cont: bool,
    /// The cell's right boundary in twips - vertical merge chains match on
    /// actual boundaries, not cell ordinals.
    pub right: i64,
}

/// One logical row: its cells with their properties, and whether the row is
/// a header row.
#[derive(Debug)]
pub struct GridRow {
    pub cells: Vec<(Vec<Block>, CellProp)>,
    pub header: bool,
    /// Fork (sznb): the row's left boundary (first cell's left edge) in
    /// twips, when the producer declares one (DOC `rgdxaCenter[0]`). A row
    /// indented past the table's leftmost edge starts at the matching grid
    /// column, with empty cells filling the gap (Word `gridBefore`
    /// semantics); `None` keeps the row anchored at column 0.
    pub left: Option<i64>,
}

/// Assemble logical rows into the canonical grid.
pub fn build_edge_table(rows: Vec<GridRow>, keep_empty_rows: bool) -> Result<Option<Block>, ConvertError> {
    struct Origin {
        blocks: Vec<Block>,
        /// Half-open global column range.
        col_l: usize,
        col_r: usize,
        row_span: u32,
        vmerge_first: bool,
        covered: bool,
    }
    // Producer boundary jitter under this threshold clusters into one edge.
    const EDGE_TOLERANCE: i64 = 10;
    let header_rows = rows.iter().take_while(|r| r.header).count();

    // Normalize each row's right edges to be strictly increasing (cells past
    // the declared boundaries get synthetic edges), then cluster the union
    // into the global column-edge list.
    // Fork (sznb): rows' left edges. The leftmost one is the table origin;
    // any row starting further right contributes its left edge as a column
    // boundary so its cells land in the columns they actually cover.
    let lefts: Vec<Option<i64>> = rows.iter().map(|r| r.left).collect();
    let origin_left = lefts.iter().flatten().copied().min();
    let rows_edged: Vec<Vec<(Vec<Block>, CellProp)>> = rows
        .into_iter()
        .map(|row| {
            let mut last = i64::MIN;
            row.cells
                .into_iter()
                .map(|(blocks, mut prop)| {
                    if prop.right <= last {
                        prop.right = last + 1;
                    }
                    last = prop.right;
                    (blocks, prop)
                })
                .collect()
        })
        .collect();
    let mut edges: Vec<i64> = rows_edged.iter().flatten().map(|(_, prop)| prop.right).collect();
    if let Some(o) = origin_left {
        edges.extend(lefts.iter().flatten().copied().filter(|&l| l - o > EDGE_TOLERANCE));
    }
    edges.sort_unstable();
    let mut clusters: Vec<i64> = Vec::new();
    for e in edges {
        if clusters.last().is_none_or(|&c| e - c > EDGE_TOLERANCE) {
            clusters.push(e);
        }
    }
    let col_of = |x: i64| -> usize { clusters.partition_point(|&c| c < x - EDGE_TOLERANCE) };

    let mut rows: Vec<Vec<Origin>> = Vec::new();
    let mut leads: Vec<usize> = Vec::new();
    for (row, left) in rows_edged.into_iter().zip(lefts) {
        let mut out: Vec<Origin> = Vec::new();
        // Indented row: column k spans (clusters[k-1], clusters[k]], so a row
        // whose left edge sits on clusters[j] starts at column j + 1.
        let mut col = match (left, origin_left) {
            (Some(l), Some(o)) if l - o > EDGE_TOLERANCE => col_of(l) + 1,
            _ => 0,
        };
        let lead = col;
        let mut cells = row.into_iter().peekable();
        while let Some((blocks, prop)) = cells.next() {
            let mut merged_blocks = blocks;
            let mut right = prop.right;
            if prop.merge_first {
                while cells.peek().is_some_and(|(_, p)| p.merge_cont) {
                    let (b, p) = cells.next().unwrap();
                    merged_blocks.extend(b);
                    right = p.right;
                }
            }
            let col_r = (col_of(right) + 1).max(col + 1);
            out.push(Origin {
                blocks: merged_blocks,
                col_l: col,
                col_r,
                row_span: 1,
                vmerge_first: prop.vmerge_first,
                covered: prop.vmerge_cont,
            });
            col = col_r;
        }
        leads.push(lead);
        rows.push(out);
    }

    // Vertical chains keyed by the origin's exact column range.
    let mut active: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for r in 0..rows.len() {
        let mut next_active: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
        for i in 0..rows[r].len() {
            let range = (rows[r][i].col_l, rows[r][i].col_r);
            if rows[r][i].covered {
                if let Some(&(orow, oidx)) = active.get(&range) {
                    rows[orow][oidx].row_span += 1;
                    next_active.insert(range, (orow, oidx));
                    continue;
                }
                // Continuation without a matching chain: keep it visible.
                rows[r][i].covered = false;
            }
            if rows[r][i].vmerge_first {
                next_active.insert(range, (r, i));
            }
        }
        active = next_active;
    }

    let mut builder = GridBuilder::new();
    if keep_empty_rows {
        builder.keep_empty_rows();
    }
    for (row, lead) in rows.into_iter().zip(leads) {
        builder.next_row();
        for _ in 0..lead {
            builder.place(Cell::default())?;
        }
        for origin in row {
            let span = (origin.col_r - origin.col_l) as u32;
            if origin.covered {
                for _ in 0..span {
                    builder.covered();
                }
            } else {
                builder.place(Cell::spanning(origin.blocks, span, origin.row_span))?;
            }
        }
    }
    let mut table = builder.finish(TableKind::Data);
    if table.grid.is_empty() {
        return Ok(None);
    }
    table.header_rows = resolve_header_rows(&table, header_rows);
    Ok(Some(Block::Table(table)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellSlot, Inline};

    fn cell(text: &str, right: i64) -> (Vec<Block>, CellProp) {
        let blocks = if text.is_empty() {
            Vec::new()
        } else {
            vec![Block::Paragraph(vec![Inline::plain(text)])]
        };
        (blocks, CellProp { right, ..Default::default() })
    }

    fn texts(block: &Block) -> Vec<Vec<String>> {
        let Block::Table(t) = block else { panic!("not a table") };
        t.grid
            .iter()
            .map(|r| {
                r.iter()
                    .map(|s| match s {
                        CellSlot::Origin(c) => c
                            .blocks
                            .iter()
                            .flat_map(|b| match b {
                                Block::Paragraph(inl) => inl.clone(),
                                _ => Vec::new(),
                            })
                            .map(|i| match i {
                                Inline::Text { text, .. } => text,
                                _ => String::new(),
                            })
                            .collect(),
                        CellSlot::Covered { .. } => "<cov>".into(),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn indented_row_starts_at_its_left_column() {
        let rows = vec![
            GridRow { cells: vec![cell("title", 5000)], header: false, left: Some(0) },
            GridRow {
                cells: vec![cell("a", 2000), cell("b", 3500), cell("c", 5000)],
                header: false,
                left: Some(1000),
            },
        ];
        let t = build_edge_table(rows, true).unwrap().unwrap();
        let g = texts(&t);
        assert_eq!(g[1], vec!["", "a", "b", "c"]);
        let Block::Table(tbl) = &t else { unreachable!() };
        let CellSlot::Origin(c) = &tbl.grid[0][0] else { panic!() };
        assert_eq!(c.col_span, 4);
    }

    #[test]
    fn missing_left_keeps_rows_at_column_zero() {
        let rows = vec![
            GridRow { cells: vec![cell("x", 1000), cell("y", 2000)], header: false, left: None },
            GridRow { cells: vec![cell("z", 2000)], header: false, left: None },
        ];
        let g = texts(&build_edge_table(rows, false).unwrap().unwrap());
        assert_eq!(g[1][0], "z");
    }
}
