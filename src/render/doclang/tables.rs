//! Table detection for DocLang export, two sources of truth:
//!
//! - **Tagged**: the structure tree's `Table`/`TR`/`TH`/`TD` elements
//!   assign page items to cells by marked-content id — authoritative when
//!   the document carries a structure tree.
//! - **Ruled**: axis-aligned rules drawn by the content stream (`re`
//!   rects and `m`/`l`/`h` path segments under stroke ops) are clustered
//!   into grids; boundaries missing inside a cell band merge cells into
//!   horizontal/vertical spans. Runs only on the pages
//!   `DoclangHints::pages_with_tables` scopes it to (`None` = every page).
//!
//! Items claimed by either path are consumed out of the page's text flow
//! and emitted as `<table>` pieces; what remains falls through to the
//! normal line/list layout.

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use pdf_inspector::TextItem;
use pdf_inspector::structure_tree::StructTable;
use std::collections::{HashMap, HashSet};

use super::FormCache;
use super::layout::{BBox, CellToken, TableBlock, TableCell};

/// Per-page input for [`detect`].
pub(crate) struct TableCtx<'a> {
    /// Tables parsed from the structure tree (all pages; cells carry
    /// `(mcid, page)` so filtering happens per cell).
    pub tagged: &'a [StructTable],
    /// Axis-aligned rules extracted from this page's content stream by
    /// [`page_rules`]. Only populated when `ruled` is on.
    pub rules: &'a [Rule],
    /// Whether the ruled engine runs on this page (`tables` hint on and
    /// the page is inside `pages_with_tables`, or that list is `None`).
    pub ruled: bool,
    /// 1-indexed page number.
    pub page: u32,
}

/// Pull table regions out of `items` (draining consumed items) and return
/// them as blocks in reading order. Tagged tables win by identity; the
/// ruled engine only sees leftovers.
pub(crate) fn detect(items: &mut Vec<TextItem>, ctx: &TableCtx) -> Vec<TableBlock> {
    let mut blocks = Vec::new();
    if !ctx.tagged.is_empty() {
        blocks.extend(tagged_blocks(items, ctx));
    }
    if ctx.ruled && !ctx.rules.is_empty() {
        blocks.extend(ruled_blocks(items, ctx));
    }
    blocks.sort_by(|a, b| b.bbox.y1.partial_cmp(&a.bbox.y1).unwrap_or(std::cmp::Ordering::Equal));
    blocks
}

/// Sort items top-down then left-right (PDF y grows upward).
fn reading_order(a: &TextItem, b: &TextItem) -> std::cmp::Ordering {
    b.y.partial_cmp(&a.y)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
}

/// Drain the flagged items once, preserving order.
fn drop_consumed(items: &mut Vec<TextItem>, consumed: &HashSet<usize>) {
    let mut idx = 0;
    items.retain(|_| {
        let keep = !consumed.contains(&idx);
        idx += 1;
        keep
    });
}

// ---- Tagged tables ------------------------------------------------------

/// Match structure-tree tables against this page's items by
/// `(mcid, page)`; consumed items are drained from `items`.
fn tagged_blocks(items: &mut Vec<TextItem>, ctx: &TableCtx) -> Vec<TableBlock> {
    let mut consumed: HashSet<usize> = HashSet::new();
    let mut blocks = Vec::new();
    for table in ctx.tagged {
        // A row belongs to this page when any cell references it; a table
        // spanning pages yields one block per page.
        let mut rows: Vec<Vec<TableCell>> = Vec::new();
        let mut header_flags: Vec<Vec<bool>> = Vec::new();
        let mut bbox: Option<BBox> = None;
        for srow in &table.rows {
            let mut cells: Vec<TableCell> = Vec::new();
            let mut flags: Vec<bool> = Vec::new();
            let mut on_page = false;
            for scell in &srow.cells {
                let wanted: HashSet<i64> =
                    scell.mcids.iter().filter(|(_, p)| *p == ctx.page).map(|(m, _)| *m).collect();
                let mut cell_items = Vec::new();
                let mut cell_bbox: Option<BBox> = None;
                for (idx, item) in items.iter().enumerate() {
                    if consumed.contains(&idx) || !item.mcid.is_some_and(|m| wanted.contains(&m)) {
                        continue;
                    }
                    consumed.insert(idx);
                    cell_bbox = Some(match cell_bbox {
                        Some(mut b) => {
                            b.union(BBox::of(item));
                            b
                        }
                        None => BBox::of(item),
                    });
                    cell_items.push(item.clone());
                }
                // The tag places the cell on this page even when text
                // extraction yielded nothing inside it.
                on_page |= !wanted.is_empty() || !cell_items.is_empty();
                cell_items.sort_by(reading_order);
                if let Some(b) = cell_bbox {
                    bbox = Some(match bbox {
                        Some(mut u) => {
                            u.union(b);
                            u
                        }
                        None => b,
                    });
                }
                cells.push(TableCell {
                    token: if cell_items.is_empty() { CellToken::Ecel } else { CellToken::Fcel },
                    items: cell_items,
                    bbox: cell_bbox.unwrap_or(BBox { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 }),
                });
                flags.push(scell.is_header);
            }
            if on_page && !cells.is_empty() {
                rows.push(cells);
                header_flags.push(flags);
            }
        }
        if rows.len() < 2 {
            // Rows consumed from a too-small table still got claimed —
            // that is fine, structure-tree membership is authoritative.
            continue;
        }
        let Some(bbox) = bbox else { continue };
        // `TH` tokens: a fully-header first row marks column headers, a
        // fully-header first column row headers; their shared corner
        // cell gets `corn`.
        let row0_headers = header_flags[0].iter().all(|h| *h);
        let col0_headers = header_flags.iter().all(|r| r.first().is_some_and(|h| *h));
        for (r, row) in rows.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                if !header_flags[r][c] || cell.token == CellToken::Ecel {
                    continue;
                }
                cell.token = match (r == 0, c == 0) {
                    (true, true) if row0_headers && col0_headers => CellToken::Corn,
                    (true, _) => CellToken::Ched,
                    _ => CellToken::Rhed,
                };
            }
        }
        blocks.push(TableBlock { cells: rows, bbox });
    }
    drop_consumed(items, &consumed);
    blocks
}

// ---- Rules from the content stream --------------------------------------

/// An axis-aligned drawn rule in page space: verticals span `lo..hi` in
/// `y` at `x == pos`; horizontals span `lo..hi` in `x` at `y == pos`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rule {
    pub vertical: bool,
    /// The rule's fixed coordinate (x for verticals, y for horizontals).
    pub pos: f32,
    /// Span along the varying axis, `lo <= hi`.
    pub lo: f32,
    pub hi: f32,
}

/// Minimum segment length that reads as a rule (points).
const MIN_RULE: f32 = 6.0;
/// A `re` rect counts as a painted rule bar when its thin side is this
/// small (typical border weights are 0.5–2pt).
const THIN_RECT: f32 = 2.5;
/// Axis-aligned tolerance for transformed segments.
const AXIS_TOL: f32 = 0.4;
/// Two rules count as touching (same grid component) within this gap.
const JOIN_TOL: f32 = 3.0;
/// Coordinates this close merge into one grid boundary.
const CLUSTER_TOL: f32 = 1.5;
/// A cell boundary exists when drawn rules cover this share of the band.
const EDGE_COVER: f32 = 0.55;
/// Hard cap on rules per page — a pathological content stream degrades
/// to plain text rather than an O(n²) blowup.
const MAX_RULES: usize = 4_000;

/// The page's resource dictionaries: its own dict plus every dict
/// reachable through `/Parent`.
pub(crate) fn page_resources(doc: &Document, page_id: ObjectId) -> Vec<Dictionary> {
    let (own, inherited_ids) = doc.get_page_resources(page_id).unwrap_or_default();
    let mut resources: Vec<Dictionary> = Vec::new();
    if let Some(dict) = own {
        resources.push(dict.clone());
    }
    for id in inherited_ids {
        if let Ok(dict) = doc.get_dictionary(id) {
            resources.push(dict.clone());
        }
    }
    resources
}

/// Accumulator for the rule walk: the collected rules plus the per-path
/// visited set that stops Form-XObject cycles.
#[derive(Default)]
struct RuleAcc {
    out: Vec<Rule>,
    seen: HashSet<ObjectId>,
}

/// Extract every axis-aligned rule a page's content stream draws.
/// Recurses into Form XObjects (with their `/Matrix`) like the image
/// walk, tracking `q`/`Q`/`cm` so coordinates land in page space. The
/// caller decodes the page content once and shares it with the image
/// walk; `cache` carries decoded Form contents so both walks expand a
/// form only once.
pub(crate) fn page_rules(
    doc: &Document,
    resources: &[Dictionary],
    content: &Content,
    cache: &mut FormCache,
) -> Vec<Rule> {
    let mut acc = RuleAcc::default();
    collect_rules(doc, resources, content, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut acc, 0, cache);
    acc.out
}

/// How a paint operator disposes of the pending path.
enum Paint {
    /// `S`/`s`: stroked — segments become rules, rects do not count as
    /// filled bars.
    Stroke,
    /// `f`/`F`/`f*`: filled — thin `re` rects are painted bars.
    Fill,
    /// `B`/`b`/`B*`/`b*`: both.
    Both,
    /// `n`: path discarded, nothing painted.
    Discard,
}

/// Graphics state relevant to rule collection: the CTM stack plus the
/// path under construction (segments and `re` rects, already
/// CTM-transformed).
struct Gfx {
    ctm: [f32; 6],
    stack: Vec<[f32; 6]>,
    segs: Vec<(f32, f32, f32, f32)>,
    rects: Vec<BBox>,
    cur: Option<(f32, f32)>,
    start: Option<(f32, f32)>,
}

impl Gfx {
    fn new(ctm: [f32; 6]) -> Gfx {
        Gfx { ctm, stack: Vec::new(), segs: Vec::new(), rects: Vec::new(), cur: None, start: None }
    }

    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.ctm;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// Add a path segment in user space; `paint` applies the CTM at
    /// paint time, so a path spanning a `q`/`Q` boundary keeps the PDF
    /// semantics (paths are not part of the saved graphics state).
    fn seg(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.segs.push((x0, y0, x1, y1));
    }

    /// Dispose of the pending path per `mode`, committing drawn rules.
    /// Segments and rects are stored in user space and transformed here
    /// with the CTM in effect at the paint operator.
    fn paint(&mut self, mode: Paint, out: &mut Vec<Rule>) {
        if matches!(mode, Paint::Stroke | Paint::Both) {
            for &(x0, y0, x1, y1) in &self.segs {
                let (ax, ay) = self.point(x0, y0);
                let (bx, by) = self.point(x1, y1);
                push_rule(ax, ay, bx, by, out);
            }
        }
        if matches!(mode, Paint::Fill | Paint::Both) {
            for r in &self.rects {
                let (ax, ay) = self.point(r.x0, r.y0);
                let (bx, by) = self.point(r.x1, r.y0);
                let (cx, cy) = self.point(r.x1, r.y1);
                let (dx, dy) = self.point(r.x0, r.y1);
                let (px0, px1) = (ax.min(bx).min(cx).min(dx), ax.max(bx).max(cx).max(dx));
                let (py0, py1) = (ay.min(by).min(cy).min(dy), ay.max(by).max(cy).max(dy));
                let (w, h) = (px1 - px0, py1 - py0);
                if w.min(h) <= THIN_RECT && w.max(h) >= MIN_RULE {
                    if h < w {
                        let y = (py0 + py1) / 2.0;
                        push_rule(px0, y, px1, y, out);
                    } else {
                        let x = (px0 + px1) / 2.0;
                        push_rule(x, py0, x, py1, out);
                    }
                }
            }
        }
        self.segs.clear();
        self.rects.clear();
        self.cur = None;
        self.start = None;
    }
}

/// Register one transformed segment as a rule when it is axis-aligned and
/// long enough.
fn push_rule(x0: f32, y0: f32, x1: f32, y1: f32, out: &mut Vec<Rule>) {
    let (dx, dy) = ((x1 - x0).abs(), (y1 - y0).abs());
    if dx <= AXIS_TOL && dy >= MIN_RULE {
        out.push(Rule { vertical: true, pos: (x0 + x1) / 2.0, lo: y0.min(y1), hi: y0.max(y1) });
    } else if dy <= AXIS_TOL && dx >= MIN_RULE {
        out.push(Rule { vertical: false, pos: (y0 + y1) / 2.0, lo: x0.min(x1), hi: x0.max(x1) });
    }
}

/// Concatenate `m` (applied first) into `ctm` — row-vector PDF
/// convention; `cm` and Form `/Matrix` compose this way.
fn mul(m: [f32; 6], ctm: [f32; 6]) -> [f32; 6] {
    let [a, b, c, d, e, f] = m;
    let [g, h, i, j, k, l] = ctm;
    [
        a * g + b * i,
        a * h + b * j,
        c * g + d * i,
        c * h + d * j,
        e * g + f * i + k,
        e * h + f * j + l,
    ]
}

/// A numeric operand; `None` for malformed objects instead of silently
/// treating them as 0 and corrupting the CTM.
fn num(obj: &Object) -> Option<f32> {
    match obj {
        Object::Real(v) => Some(*v),
        Object::Integer(v) => Some(*v as f32),
        _ => None,
    }
}

/// Six numeric operands (`cm` matrix, Form `/Matrix`), or `None` when
/// any entry is malformed.
fn nums(ops: &[Object]) -> Option<[f32; 6]> {
    let mut out = [0.0; 6];
    for (v, o) in out.iter_mut().zip(ops.iter()) {
        *v = num(o)?;
    }
    Some(out)
}

fn num2(ops: &[Object]) -> Option<(f32, f32)> {
    Some((num(ops.first()?)?, num(ops.get(1)?)?))
}

/// Resolve an XObject resource name to a Form XObject's `(id, stream)`.
/// `None` for images and other subtypes.
fn resolve_form<'a>(
    doc: &'a Document,
    resources: &[Dictionary],
    name: &[u8],
) -> Option<(ObjectId, &'a Stream)> {
    for dict in resources {
        let Ok(xobjects) = dict.get(b"XObject") else { continue };
        let xobjects = match xobjects {
            Object::Reference(id) => doc.get_dictionary(*id).ok()?,
            Object::Dictionary(d) => d,
            _ => continue,
        };
        let Ok(entry) = xobjects.get(name) else { continue };
        let id = entry.as_reference().ok()?;
        let stream = doc.get_object(id).and_then(Object::as_stream).ok()?;
        return (stream.dict.get(b"Subtype").and_then(Object::as_name).ok()
            == Some(b"Form".as_slice()))
        .then_some((id, stream));
    }
    None
}

fn collect_rules(
    doc: &Document,
    resources: &[Dictionary],
    content: &Content,
    ctm: [f32; 6],
    acc: &mut RuleAcc,
    depth: usize,
    cache: &mut FormCache,
) {
    if depth > 8 || acc.out.len() >= MAX_RULES {
        return;
    }
    let mut gfx = Gfx::new(ctm);
    for op in &content.operations {
        match op.operator.as_str() {
            "q" => gfx.stack.push(gfx.ctm),
            "Q" => {
                if let Some(ctm) = gfx.stack.pop() {
                    gfx.ctm = ctm;
                }
            }
            "cm" => {
                if op.operands.len() >= 6
                    && let Some(m) = nums(&op.operands[..6])
                {
                    gfx.ctm = mul(m, gfx.ctm);
                }
            }
            "m" => {
                if let Some(p) = num2(&op.operands) {
                    gfx.cur = Some(p);
                    gfx.start = Some(p);
                }
            }
            "l" => {
                if let Some((x0, y0)) = gfx.cur
                    && let Some((x1, y1)) = num2(&op.operands)
                {
                    gfx.seg(x0, y0, x1, y1);
                    gfx.cur = Some((x1, y1));
                }
            }
            "h" => {
                if let (Some(cur), Some(start)) = (gfx.cur, gfx.start) {
                    gfx.seg(cur.0, cur.1, start.0, start.1);
                    gfx.cur = gfx.start;
                }
            }
            "c" | "v" | "y" => {
                // Curve segments never produce rules; the current point
                // still advances to the endpoint.
                if op.operands.len() >= 2 {
                    let n = op.operands.len();
                    gfx.cur = num2(&op.operands[n - 2..]).or(gfx.cur);
                }
            }
            "re" => {
                if op.operands.len() >= 4
                    && let (Some(x), Some(y), Some(w), Some(h)) = (
                        num(&op.operands[0]),
                        num(&op.operands[1]),
                        num(&op.operands[2]),
                        num(&op.operands[3]),
                    )
                {
                    let (x1, y1) = (x + w, y + h);
                    gfx.rects.push(BBox {
                        x0: x.min(x1),
                        y0: y.min(y1),
                        x1: x.max(x1),
                        y1: y.max(y1),
                    });
                    gfx.segs.push((x, y, x1, y));
                    gfx.segs.push((x1, y, x1, y1));
                    gfx.segs.push((x1, y1, x, y1));
                    gfx.segs.push((x, y1, x, y));
                    gfx.cur = Some((x, y));
                    gfx.start = Some((x, y));
                }
            }
            "S" | "s" => gfx.paint(Paint::Stroke, &mut acc.out),
            "f" | "F" | "f*" => gfx.paint(Paint::Fill, &mut acc.out),
            "B" | "b" | "B*" | "b*" => gfx.paint(Paint::Both, &mut acc.out),
            "n" => gfx.paint(Paint::Discard, &mut acc.out),
            "Do" => {
                let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else {
                    continue;
                };
                let Some((id, form)) = resolve_form(doc, resources, name) else { continue };
                if !acc.seen.insert(id) {
                    continue;
                }
                let matrix = match form.dict.get(b"Matrix") {
                    Ok(Object::Array(a)) if a.len() >= 6 => {
                        nums(&a[..6]).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
                    }
                    _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                };
                let mut form_resources = resources.to_vec();
                match form.dict.get(b"Resources") {
                    Ok(Object::Dictionary(res)) => form_resources.insert(0, res.clone()),
                    Ok(Object::Reference(id)) => {
                        if let Ok(res) = doc.get_dictionary(*id) {
                            form_resources.insert(0, res.clone());
                        }
                    }
                    _ => {}
                }
                if let Some(inner) = cache.content(id, form) {
                    collect_rules(
                        doc,
                        &form_resources,
                        &inner,
                        mul(matrix, gfx.ctm),
                        acc,
                        depth + 1,
                        cache,
                    );
                }
                acc.seen.remove(&id);
            }
            _ => {}
        }
    }
}

// ---- Ruled tables --------------------------------------------------------

/// Detect ruled tables among the remaining items: cluster rules into
/// connected grids, then keep each grid that resolves to a plausible
/// cell matrix with text inside.
fn ruled_blocks(items: &mut Vec<TextItem>, ctx: &TableCtx) -> Vec<TableBlock> {
    let mut consumed: HashSet<usize> = HashSet::new();
    let mut blocks = Vec::new();
    for comp in components(ctx.rules) {
        let Some(grid) = Grid::new(&comp) else { continue };
        let mut claims: HashSet<usize> = HashSet::new();
        for (idx, item) in items.iter().enumerate() {
            if consumed.contains(&idx) {
                continue;
            }
            let b = BBox::of(item);
            let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            if grid.bbox.contains(cx, cy) {
                claims.insert(idx);
            }
        }
        if let Some((block, used)) = grid.build(items, &claims) {
            // Only consume items that actually landed in a cell — claims
            // that fell outside the grid return to normal layout.
            consumed.extend(used);
            blocks.push(block);
        }
    }
    drop_consumed(items, &consumed);
    blocks
}

/// Group rules into connected components: a vertical and a horizontal
/// rule touch when each one's position lands inside the other's span
/// (within `JOIN_TOL`) — the crossings and corners of one grid.
fn components(rules: &[Rule]) -> Vec<Vec<Rule>> {
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let touches = |a: &Rule, b: &Rule| {
        if a.vertical == b.vertical {
            // Same-orientation rules connect only transitively; direct
            // overlap must not merge two parallel grid lines.
            return false;
        }
        let (v, h) = if a.vertical { (a, b) } else { (b, a) };
        v.pos >= h.lo - JOIN_TOL
            && v.pos <= h.hi + JOIN_TOL
            && h.pos >= v.lo - JOIN_TOL
            && h.pos <= v.hi + JOIN_TOL
    };
    let mut parent: Vec<usize> = (0..rules.len()).collect();
    for i in 0..rules.len() {
        for j in (i + 1)..rules.len() {
            if touches(&rules[i], &rules[j]) {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }
    let mut comps: HashMap<usize, Vec<Rule>> = HashMap::new();
    for (i, rule) in rules.iter().enumerate() {
        comps.entry(root(&mut parent, i)).or_default().push(*rule);
    }
    comps.into_values().collect()
}

/// A ruled grid: clustered column and row boundaries over one connected
/// component of rules.
struct Grid {
    /// Ascending column boundaries (`x`), `len` = columns + 1.
    xs: Vec<f32>,
    /// Descending row boundaries (`y`, top edge first).
    ys: Vec<f32>,
    /// The component's vertical rules, for edge-coverage checks.
    verts: Vec<Rule>,
    /// The component's horizontal rules.
    hors: Vec<Rule>,
    /// Region spanned by the grid.
    bbox: BBox,
}

impl Grid {
    /// Cluster the component's rule positions into boundaries. Needs at
    /// least a 2×2 cell matrix (three boundaries per axis) to qualify.
    fn new(comp: &[Rule]) -> Option<Grid> {
        // Anchor clustering: a position joins the open cluster only when
        // it is within CLUSTER_TOL of the cluster's first member, so the
        // boundary diameter stays under CLUSTER_TOL and a staircase of
        // close positions cannot chain distinct lines into one.
        let cluster = |rules: &[Rule]| -> Vec<f32> {
            let mut positions: Vec<f32> = rules.iter().map(|r| r.pos).collect();
            positions.sort_by(f32::total_cmp);
            let mut sums: Vec<f32> = Vec::new();
            let mut counts: Vec<usize> = Vec::new();
            let mut anchor = f32::NEG_INFINITY;
            for p in positions {
                if p - anchor <= CLUSTER_TOL {
                    *sums.last_mut().unwrap() += p;
                    *counts.last_mut().unwrap() += 1;
                } else {
                    sums.push(p);
                    counts.push(1);
                    anchor = p;
                }
            }
            sums.iter().zip(counts.iter()).map(|(s, n)| s / *n as f32).collect()
        };
        let verts: Vec<Rule> = comp.iter().filter(|r| r.vertical).copied().collect();
        let hors: Vec<Rule> = comp.iter().filter(|r| !r.vertical).copied().collect();
        let xs = cluster(&verts);
        let mut ys = cluster(&hors);
        ys.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        if xs.len() < 3 || ys.len() < 3 {
            return None;
        }
        Some(Grid {
            bbox: BBox { x0: xs[0], y0: ys[ys.len() - 1], x1: xs[xs.len() - 1], y1: ys[0] },
            xs,
            ys,
            verts,
            hors,
        })
    }

    /// Whether a real rule runs along boundary `k` across the band
    /// `lo..hi`: drawn rules near the clustered position must cover at
    /// least `EDGE_COVER` of the band.
    fn boundary(&self, vertical: bool, k: usize, lo: f32, hi: f32) -> bool {
        let (pos, rules) =
            if vertical { (self.xs[k], &self.verts) } else { (self.ys[k], &self.hors) };
        let span = (hi - lo).max(0.001);
        let covered: f32 = rules
            .iter()
            .filter(|r| (r.pos - pos).abs() <= CLUSTER_TOL)
            .map(|r| (r.hi.min(hi) - r.lo.max(lo)).max(0.0))
            .sum::<f32>()
            .min(span);
        covered / span >= EDGE_COVER
    }

    /// Resolve the grid into a cell matrix: missing interior boundaries
    /// merge cells into spans (`lcel`/`ucel`/`xcel` continuations), then
    /// the claimed items drop into their cells. Returns the block plus
    /// the subset of `claims` that actually landed in a cell, or `None`
    /// when the matrix is implausible (single row/column of content, or
    /// too little text) — the claims are then left for normal layout.
    fn build(
        &self,
        items: &[TextItem],
        claims: &HashSet<usize>,
    ) -> Option<(TableBlock, HashSet<usize>)> {
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let (nrows, ncols) = (self.ys.len() - 1, self.xs.len() - 1);
        // Union-find over cells: adjacent cells merge where the boundary
        // between them has no drawn rule covering this band.
        let mut parent: Vec<usize> = (0..nrows * ncols).collect();
        for r in 0..nrows {
            for c in 0..ncols {
                if c + 1 < ncols && !self.boundary(true, c + 1, self.ys[r + 1], self.ys[r]) {
                    let (a, b) =
                        (find(&mut parent, r * ncols + c), find(&mut parent, r * ncols + c + 1));
                    if a != b {
                        parent[b] = a;
                    }
                }
                if r + 1 < nrows && !self.boundary(false, r + 1, self.xs[c], self.xs[c + 1]) {
                    let (a, b) =
                        (find(&mut parent, r * ncols + c), find(&mut parent, (r + 1) * ncols + c));
                    if a != b {
                        parent[b] = a;
                    }
                }
            }
        }
        // Leader cell = top-leftmost of each merged region; continuations
        // are lcel (same row as leader), ucel (same column), or xcel.
        let mut leader_of: HashMap<usize, (usize, usize)> = HashMap::new();
        for r in 0..nrows {
            for c in 0..ncols {
                let root = find(&mut parent, r * ncols + c);
                leader_of
                    .entry(root)
                    .and_modify(|l| {
                        if (r, c) < *l {
                            *l = (r, c);
                        }
                    })
                    .or_insert((r, c));
            }
        }
        let mut cells: Vec<Vec<TableCell>> = (0..nrows)
            .map(|r| {
                (0..ncols)
                    .map(|c| {
                        let root = find(&mut parent, r * ncols + c);
                        let (lr, lc) = leader_of[&root];
                        let token = if (lr, lc) == (r, c) {
                            CellToken::Fcel
                        } else if lr == r {
                            CellToken::Lcel
                        } else if lc == c {
                            CellToken::Ucel
                        } else {
                            CellToken::Xcel
                        };
                        TableCell {
                            token,
                            items: Vec::new(),
                            bbox: BBox {
                                x0: self.xs[c],
                                y0: self.ys[r + 1],
                                x1: self.xs[c + 1],
                                y1: self.ys[r],
                            },
                        }
                    })
                    .collect()
            })
            .collect();
        // Drop claimed items into their leader cell.
        let mut used: HashSet<usize> = HashSet::new();
        for &idx in claims {
            let b = BBox::of(&items[idx]);
            let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            let col =
                self.xs.iter().position(|&x| cx < x).unwrap_or(self.xs.len()).saturating_sub(1);
            let row =
                self.ys.iter().position(|&y| cy > y).unwrap_or(self.ys.len()).saturating_sub(1);
            if row >= nrows || col >= ncols {
                continue;
            }
            let root = find(&mut parent, row * ncols + col);
            let (lr, lc) = leader_of[&root];
            let cell = &mut cells[lr][lc];
            cell.bbox.union(b);
            cell.items.push(items[idx].clone());
            used.insert(idx);
        }
        // Plausibility: content in at least two rows and two columns —
        // otherwise this is a ruled box or ruled lines, not a table.
        let mut filled: HashSet<(usize, usize)> = HashSet::new();
        for (r, row) in cells.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                if cell.token == CellToken::Fcel {
                    cell.items.sort_by(reading_order);
                    if cell.items.is_empty() {
                        cell.token = CellToken::Ecel;
                    } else {
                        filled.insert((r, c));
                    }
                }
            }
        }
        let rows_filled: HashSet<usize> = filled.iter().map(|(r, _)| *r).collect();
        let cols_filled: HashSet<usize> = filled.iter().map(|(_, c)| *c).collect();
        if rows_filled.len() < 2 || cols_filled.len() < 2 || filled.len() < 3 {
            return None;
        }
        // A mostly-bold first row marks column headers.
        let mut first_row_items = 0usize;
        let mut first_row_bold = 0usize;
        for cell in &cells[0] {
            for item in &cell.items {
                first_row_items += 1;
                if item.is_bold {
                    first_row_bold += 1;
                }
            }
        }
        if first_row_items > 0 && first_row_bold * 2 >= first_row_items {
            for cell in &mut cells[0] {
                if cell.token == CellToken::Fcel {
                    cell.token = CellToken::Ched;
                }
            }
        }
        Some((TableBlock { cells, bbox: self.bbox }, used))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_inspector::structure_tree::{StructTableCell, StructTableRow};
    use pdf_inspector::types::ItemType;

    fn item(text: &str, x: f32, y: f32, mcid: Option<i64>) -> TextItem {
        TextItem {
            text: text.to_string(),
            x,
            y,
            width: text.len() as f32 * 5.0,
            height: 10.0,
            font: "F1".into(),
            font_size: 10.0,
            page: 1,
            is_bold: false,
            is_italic: false,
            is_underline: false,
            is_strikeout: false,
            item_type: ItemType::Text,
            mcid,
        }
    }

    /// Vertical rule `x = pos`, spanning `y0..y1`.
    fn v(x: f32, y0: f32, y1: f32) -> Rule {
        Rule { vertical: true, pos: x, lo: y0, hi: y1 }
    }

    /// Horizontal rule `y = pos`, spanning `x0..x1`.
    fn h(y: f32, x0: f32, x1: f32) -> Rule {
        Rule { vertical: false, pos: y, lo: x0, hi: x1 }
    }

    /// A 2-column × 2-row ruled grid plus `items`; runs `detect` and
    /// returns `(blocks, remaining_items)`.
    fn detect_with(
        rules: Vec<Rule>,
        items: Vec<TextItem>,
        tagged: &[StructTable],
    ) -> (Vec<TableBlock>, Vec<TextItem>) {
        let mut items = items;
        let ctx = TableCtx { tagged, rules: &rules, ruled: true, page: 1 };
        let blocks = detect(&mut items, &ctx);
        (blocks, items)
    }

    /// Full 2×2 grid: x-boundaries {60,180,300}, y {740,710,680}.
    fn grid_rules() -> Vec<Rule> {
        vec![
            v(60.0, 680.0, 740.0),
            v(180.0, 680.0, 740.0),
            v(300.0, 680.0, 740.0),
            h(740.0, 60.0, 300.0),
            h(710.0, 60.0, 300.0),
            h(680.0, 60.0, 300.0),
        ]
    }

    fn cell_items(b: &TableBlock, r: usize, c: usize) -> String {
        b.cells[r][c].items.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn ruled_grid_assigns_items_to_cells() {
        let items = vec![
            item("A1", 70.0, 715.0, None),
            item("B1", 200.0, 715.0, None),
            item("A2", 70.0, 685.0, None),
            item("B2", 200.0, 685.0, None),
            item("outside", 400.0, 300.0, None),
        ];
        let (blocks, rest) = detect_with(grid_rules(), items, &[]);
        assert_eq!(blocks.len(), 1);
        let t = &blocks[0];
        assert_eq!(t.cells.len(), 2);
        assert_eq!(t.cells[0].len(), 2);
        assert_eq!(cell_items(t, 0, 0), "A1");
        assert_eq!(cell_items(t, 0, 1), "B1");
        assert_eq!(cell_items(t, 1, 0), "A2");
        assert_eq!(cell_items(t, 1, 1), "B2");
        assert!(rest.iter().any(|i| i.text == "outside"));
    }

    #[test]
    fn missing_interior_rule_spans_cells() {
        // Drop the mid vertical of the top row only → (0,0)+(0,1) merge.
        let rules = vec![
            v(60.0, 680.0, 740.0),
            v(180.0, 680.0, 709.0), // only covers the bottom row band
            v(300.0, 680.0, 740.0),
            h(740.0, 60.0, 300.0),
            h(710.0, 60.0, 300.0),
            h(680.0, 60.0, 300.0),
        ];
        let items = vec![
            item("wide", 120.0, 715.0, None),
            item("A2", 70.0, 685.0, None),
            item("B2", 200.0, 685.0, None),
        ];
        let (blocks, _) = detect_with(rules, items, &[]);
        let t = &blocks[0];
        assert_eq!(t.cells[0][0].token, CellToken::Fcel);
        assert_eq!(t.cells[0][1].token, CellToken::Lcel);
        assert_eq!(cell_items(t, 0, 0), "wide");
    }

    #[test]
    fn ruled_box_without_text_columns_is_rejected() {
        // Same grid, but all text lands in one column → not a table.
        let items = vec![item("line one", 70.0, 715.0, None), item("line two", 70.0, 685.0, None)];
        let (blocks, rest) = detect_with(grid_rules(), items, &[]);
        assert!(blocks.is_empty());
        assert_eq!(rest.len(), 2);
    }

    #[test]
    fn tagged_table_matches_mcids() {
        let tagged = vec![StructTable {
            rows: vec![
                StructTableRow {
                    cells: vec![
                        StructTableCell { is_header: true, mcids: vec![(10, 1)] },
                        StructTableCell { is_header: true, mcids: vec![(11, 1)] },
                    ],
                },
                StructTableRow {
                    cells: vec![
                        StructTableCell { is_header: false, mcids: vec![(12, 1)] },
                        StructTableCell { is_header: false, mcids: vec![(13, 1)] },
                    ],
                },
            ],
        }];
        let items = vec![
            item("H1", 70.0, 715.0, Some(10)),
            item("H2", 200.0, 715.0, Some(11)),
            item("V1", 70.0, 685.0, Some(12)),
            item("V2", 200.0, 685.0, Some(13)),
            item("other", 100.0, 500.0, Some(99)),
        ];
        let (blocks, rest) = detect_with(Vec::new(), items, &tagged);
        assert_eq!(blocks.len(), 1);
        let t = &blocks[0];
        assert_eq!(t.cells[0][0].token, CellToken::Ched);
        assert_eq!(t.cells[0][1].token, CellToken::Ched);
        assert_eq!(t.cells[1][0].token, CellToken::Fcel);
        assert_eq!(cell_items(t, 1, 1), "V2");
        assert!(rest.iter().any(|i| i.text == "other"));
    }

    #[test]
    fn content_ops_yield_rules() {
        // `re` borders + stroked m/l segments; the curve and the `n` path
        // produce nothing.
        let ops = b"\
            q 1 0 0 1 10 20 cm \
            50 660 m 50 720 l S \
            50 720 m 290 720 l S \
            50 660 240 60 re S \
            200 500 m 400 500 l n \
            Q";
        let content = Content::decode(ops).unwrap();
        let doc = Document::new();
        let mut acc = RuleAcc::default();
        collect_rules(
            &doc,
            &[],
            &content,
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            &mut acc,
            0,
            &mut FormCache::default(),
        );
        let out = acc.out;
        // cm shifts everything by (10,20): the vertical lands at x=60.
        assert!(out.iter().any(|r| r.vertical
            && (r.pos - 60.0).abs() < 0.5
            && (r.lo - 680.0).abs() < 0.5
            && (r.hi - 740.0).abs() < 0.5));
        // re contributes its four edges too.
        assert!(out.iter().any(|r| !r.vertical && (r.pos - 680.0).abs() < 0.5));
        // The `n` path produced no rule at y=520.
        assert!(!out.iter().any(|r| !r.vertical && (r.pos - 520.0).abs() < 0.5));
    }

    #[test]
    fn stroke_after_q_uses_restored_ctm() {
        // Path built under a scale-2 CTM but stroked after `Q`: the
        // restored identity CTM applies at paint time, so the rule
        // lands at 50–100, not 100–200.
        let ops = b"q 2 0 0 2 0 0 cm 50 50 m 100 50 l Q S";
        let content = Content::decode(ops).unwrap();
        let mut acc = RuleAcc::default();
        collect_rules(
            &Document::new(),
            &[],
            &content,
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            &mut acc,
            0,
            &mut FormCache::default(),
        );
        let out = acc.out;
        assert!(out.iter().any(|r| !r.vertical
            && (r.pos - 50.0).abs() < 0.5
            && (r.lo - 50.0).abs() < 0.5
            && (r.hi - 100.0).abs() < 0.5));
    }

    #[test]
    fn item_on_grid_edge_is_not_consumed() {
        // Center exactly on the rightmost boundary: claimed by the grid
        // bbox but placed in no cell — it must survive in normal flow.
        let items = vec![
            item("A1", 70.0, 715.0, None),
            item("B1", 200.0, 715.0, None),
            item("A2", 70.0, 685.0, None),
            item("B2", 200.0, 685.0, None),
            // width 4×5=20 → center x = 290+10 = 300, the right boundary.
            item("edge", 290.0, 715.0, None),
        ];
        let (blocks, rest) = detect_with(grid_rules(), items, &[]);
        assert_eq!(blocks.len(), 1);
        assert!(rest.iter().any(|i| i.text == "edge"));
    }

    #[test]
    fn tagged_table_spanning_pages_skips_foreign_rows() {
        let tagged = vec![StructTable {
            rows: vec![
                StructTableRow {
                    cells: vec![
                        StructTableCell { is_header: false, mcids: vec![(10, 1)] },
                        StructTableCell { is_header: false, mcids: vec![(11, 1)] },
                    ],
                },
                StructTableRow {
                    cells: vec![
                        StructTableCell { is_header: false, mcids: vec![(12, 1)] },
                        // This cell's content lives on page 2.
                        StructTableCell { is_header: false, mcids: vec![(13, 2)] },
                    ],
                },
                StructTableRow {
                    cells: vec![
                        StructTableCell { is_header: false, mcids: vec![(14, 2)] },
                        StructTableCell { is_header: false, mcids: vec![(15, 2)] },
                    ],
                },
            ],
        }];
        let items = vec![
            item("A1", 70.0, 715.0, Some(10)),
            item("B1", 200.0, 715.0, Some(11)),
            item("A2", 70.0, 685.0, Some(12)),
        ];
        let (blocks, _) = detect_with(Vec::new(), items, &tagged);
        let t = &blocks[0];
        // The all-page-2 row produced no row; the mixed row keeps its
        // foreign cell as an empty placeholder so the grid stays
        // rectangular — with a degenerate box, not a bogus location.
        assert_eq!(t.cells.len(), 2);
        assert_eq!(t.cells[1][0].token, CellToken::Fcel);
        assert_eq!(t.cells[1][1].token, CellToken::Ecel);
        let b = t.cells[1][1].bbox;
        assert!(b.x1 <= b.x0 && b.y1 <= b.y0);
    }

    #[test]
    fn close_boundaries_do_not_chain_merge() {
        // Verticals ~1.4pt apart: comparing each position to the previous
        // one would chain all three into one boundary; anchored clustering
        // keeps {60,61.4} separate from 62.8, so a 3-boundary grid forms.
        let comp = vec![
            v(60.0, 680.0, 740.0),
            v(61.4, 680.0, 740.0),
            v(62.8, 680.0, 740.0),
            v(300.0, 680.0, 740.0),
            h(740.0, 60.0, 300.0),
            h(710.0, 60.0, 300.0),
            h(680.0, 60.0, 300.0),
        ];
        let grid = Grid::new(&comp).expect("boundaries must not chain-merge");
        assert_eq!(grid.xs.len(), 3);
    }

    #[test]
    fn malformed_cm_does_not_corrupt_ctm() {
        // `/bad 20 cm` has non-numeric operands: the CTM must stay
        // identity rather than silently inserting zeros.
        let ops = b"/bad 20 cm 50 660 m 50 720 l S";
        let content = Content::decode(ops).unwrap();
        let mut acc = RuleAcc::default();
        collect_rules(
            &Document::new(),
            &[],
            &content,
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            &mut acc,
            0,
            &mut FormCache::default(),
        );
        let out = acc.out;
        assert!(out.iter().any(|r| r.vertical
            && (r.pos - 50.0).abs() < 0.5
            && (r.lo - 660.0).abs() < 0.5
            && (r.hi - 720.0).abs() < 0.5));
    }

    #[test]
    fn form_cache_decodes_once() {
        let mut cache = FormCache::default();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        let stream = Stream::new(dict, b"0 0 m 10 10 l S".to_vec());
        let a = cache.content((7, 0), &stream).unwrap();
        let b = cache.content((7, 0), &stream).unwrap();
        // Same object id → the same decoded content, decoded once.
        assert!(std::rc::Rc::ptr_eq(&a, &b));
    }
}
