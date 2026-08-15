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
}

/// Assemble logical rows into the canonical grid.
pub fn build_edge_table(rows: Vec<GridRow>) -> Result<Option<Block>, ConvertError> {
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
    edges.sort_unstable();
    let mut clusters: Vec<i64> = Vec::new();
    for e in edges {
        if clusters.last().is_none_or(|&c| e - c > EDGE_TOLERANCE) {
            clusters.push(e);
        }
    }
    let col_of = |x: i64| -> usize { clusters.partition_point(|&c| c < x - EDGE_TOLERANCE) };

    let mut rows: Vec<Vec<Origin>> = Vec::new();
    for row in rows_edged {
        let mut out: Vec<Origin> = Vec::new();
        let mut col = 0usize;
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
    for row in rows {
        builder.next_row();
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
