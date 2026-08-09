//! (X)HTML element tree -> model blocks. Used by the EPUB frontend.
//!
//! Applies a deliberately small CSS subset - the semantic properties only:
//! `font-weight`, `font-style`, `text-decoration: line-through`, and
//! `display: none` - from inline `style` attributes and element/class rules.
//! Tables build the canonical grid (`rowspan`/`colspan`); ordered lists honor
//! `start`, `reversed`, `type`, and per-item `value`.

use crate::error::ConvertError;
use crate::model::{
    AnchorId, Block, Cell, GridBuilder, ImageSource, Inline, LinkTarget, List, ListItem,
    MarkerKind, TableKind, VertAlign, inlines_are_empty, inlines_to_plain_text,
};
use crate::package::xml::{Element, Node};
use crate::shared::delta::{StyleDelta, rebase_emphasis};
use crate::shared::header::resolve_header_rows;
use crate::shared::text::{clean_text, collapse_ws};
use std::collections::HashMap;

/// Frontend hooks: how hrefs, image sources, and anchor ids resolve in the
/// containing document (EPUB scopes them per chapter).
pub trait HtmlCtx {
    fn link_target(&self, href: &str) -> Option<LinkTarget>;
    /// A failed load degrades to `Ok(None)`; resource-limit errors propagate.
    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError>;
    fn anchor_id(&self, raw: &str) -> AnchorId;
}

pub fn to_blocks(
    body: &Element,
    css: &Stylesheet,
    ctx: &dyn HtmlCtx,
) -> Result<Vec<Block>, ConvertError> {
    let mut builder =
        Builder { blocks: Vec::new(), inlines: Vec::new(), css, ctx, start_boundary: true };
    builder.walk_children(body, StyleDelta::default())?;
    Ok(builder.finish())
}

// ---------------------------------------------------------------------------
// Minimal CSS

#[derive(Debug, Clone, Copy, Default)]
pub struct StyleProps {
    pub delta: StyleDelta,
    /// `display` visibility as a tri-state: a higher-priority
    /// `display: block` (or any non-none value) can restore content a
    /// lower-priority `display: none` hid.
    pub hidden: Option<bool>,
}

impl StyleProps {
    fn merge(self, over: StyleProps) -> StyleProps {
        StyleProps { delta: self.delta.merge(over.delta), hidden: over.hidden.or(self.hidden) }
    }

    fn is_default(&self) -> bool {
        self.delta == StyleDelta::default() && self.hidden.is_none()
    }
}

/// Cascade priority tiers: rules < inline style < `!important` rules <
/// `!important` inline style, with selector specificity ordering within a
/// tier and source order breaking ties.
const INLINE_PRIORITY: u32 = 100_000;
const IMPORTANT_PRIORITY: u32 = 1_000_000;

#[derive(Debug)]
struct Rule {
    tag: Option<String>,
    class: Option<String>,
    /// Cascade priority: the tier base plus selector specificity within the
    /// supported subset (a class outweighs any number of tags: `.c` = 10,
    /// `t.c` = 11, `t` = 1). Source order breaks ties (rules are stored in
    /// document order).
    priority: u32,
    props: StyleProps,
}

#[derive(Debug, Default)]
pub struct Stylesheet {
    rules: Vec<Rule>,
}

impl Stylesheet {
    /// Add rules from one stylesheet's text. Only simple `tag`, `.class`,
    /// and `tag.class` selectors participate; `!important` declarations
    /// enter the higher cascade tier.
    pub fn add(&mut self, css: &str) {
        let css = strip_css_comments(css);
        for chunk in css.split('}') {
            let Some((selectors, body)) = chunk.split_once('{') else {
                continue;
            };
            let decls = parse_declarations(body);
            if decls.normal.is_default() && decls.important.is_default() {
                continue;
            }
            for selector in selectors.split(',') {
                let s = selector.trim();
                if s.is_empty() || s.contains(' ') || s.contains(':') || s.contains('[') {
                    continue; // combinators/pseudo/attribute selectors: out of subset
                }
                let (tag, class) = match s.split_once('.') {
                    Some((t, c)) => (
                        if t.is_empty() { None } else { Some(t.to_ascii_lowercase()) },
                        Some(c.to_string()),
                    ),
                    None => (Some(s.to_ascii_lowercase()), None),
                };
                let specificity = u32::from(class.is_some()) * 10 + u32::from(tag.is_some());
                for (props, base) in [(decls.normal, 0), (decls.important, IMPORTANT_PRIORITY)] {
                    if !props.is_default() {
                        self.rules.push(Rule {
                            tag: tag.clone(),
                            class: class.clone(),
                            priority: base + specificity,
                            props,
                        });
                    }
                }
            }
        }
    }

    /// Matching rules for one element as (priority, props) pairs.
    fn matching_rules(&self, tag: &str, classes: &[&str]) -> Vec<(u32, StyleProps)> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.tag.as_deref().is_none_or(|t| t == tag)
                    && rule.class.as_deref().is_none_or(|c| classes.contains(&c))
            })
            .map(|rule| (rule.priority, rule.props))
            .collect()
    }
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// A declaration block's properties, split by cascade tier.
#[derive(Debug, Clone, Copy, Default)]
struct DeclProps {
    normal: StyleProps,
    important: StyleProps,
}

/// The semantic subset of a declaration block.
fn parse_declarations(body: &str) -> DeclProps {
    let mut out = DeclProps::default();
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let mut value = value.trim().to_ascii_lowercase();
        // `!important` moves the declaration into the higher cascade tier.
        let mut important = false;
        if let Some(pos) = value.find('!') {
            if value[pos + 1..].trim() == "important" {
                important = true;
            }
            value.truncate(pos);
            value.truncate(value.trim_end().len());
        }
        let props = if important { &mut out.important } else { &mut out.normal };
        match name.as_str() {
            "font-weight" => {
                props.delta.bold = Some(
                    value == "bold"
                        || value == "bolder"
                        || value.parse::<u32>().is_ok_and(|n| n >= 600),
                );
            }
            "font-style" => {
                props.delta.italic = Some(value == "italic" || value == "oblique");
            }
            "text-decoration" | "text-decoration-line" => {
                if value.contains("line-through") {
                    props.delta.strike = Some(true);
                } else if value == "none" {
                    props.delta.strike = Some(false);
                }
            }
            "display" => {
                props.hidden = Some(value == "none");
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Walking

struct Builder<'e> {
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
    css: &'e Stylesheet,
    ctx: &'e dyn HtmlCtx,
    /// Whether text appended while `inlines` is empty sits at a whitespace
    /// boundary (block start: leading whitespace collapses away; inline
    /// sub-builders inherit the surrounding run's state instead).
    start_boundary: bool,
}

/// Content worth keeping: visible text, or an anchor node some link may
/// target (the renderer drops unreferenced anchors itself).
fn keeps_paragraph(inlines: &[Inline]) -> bool {
    !inlines_are_empty(inlines) || inlines.iter().any(|i| matches!(i, Inline::Anchor(_)))
}

/// Whether text appended after `inlines` sits at a whitespace boundary
/// (its leading spaces collapse away): at a block start, after emitted
/// whitespace or a line break, looking through zero-width anchors.
fn at_space_boundary(inlines: &[Inline], start: bool) -> bool {
    for inline in inlines.iter().rev() {
        match inline {
            Inline::Anchor(_) => continue,
            Inline::Text { text, .. } => {
                if text.is_empty() {
                    continue;
                }
                return text.ends_with(char::is_whitespace);
            }
            Inline::LineBreak => return true,
            Inline::Link { content, .. } => {
                if inlines_are_empty(content) {
                    continue;
                }
                return at_space_boundary(content, false);
            }
            Inline::Image { .. } | Inline::NoteRef(_) | Inline::Math { .. } => return false,
        }
    }
    start
}

impl Builder<'_> {
    fn flush_paragraph(&mut self) {
        if !self.inlines.is_empty() {
            let inlines = std::mem::take(&mut self.inlines);
            if keeps_paragraph(&inlines) {
                self.blocks.push(Block::Paragraph(inlines));
            }
        }
        self.start_boundary = true;
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush_paragraph();
        self.blocks
    }

    /// Sub-walk in a fresh block context (list items, quotes, cells).
    fn sub_blocks(
        &mut self,
        elem: &Element,
        delta: StyleDelta,
    ) -> Result<Vec<Block>, ConvertError> {
        self.sub_blocks_at(elem, delta, true)
    }

    /// Sub-walk starting at the given whitespace-boundary state (`false`
    /// when the sub-content continues an inline run, so its leading space
    /// stays significant relative to the surrounding text).
    fn sub_blocks_at(
        &mut self,
        elem: &Element,
        delta: StyleDelta,
        start_boundary: bool,
    ) -> Result<Vec<Block>, ConvertError> {
        let mut b = Builder {
            blocks: Vec::new(),
            inlines: Vec::new(),
            css: self.css,
            ctx: self.ctx,
            start_boundary,
        };
        b.walk_children(elem, delta)?;
        Ok(b.finish())
    }

    /// Element-level props: matching stylesheet rules and the inline `style`
    /// attribute merged in one cascade order — normal rules, inline style,
    /// `!important` rules, `!important` inline style.
    fn element_props(&self, elem: &Element) -> StyleProps {
        let classes: Vec<&str> =
            elem.attr_any("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
        let mut entries = self.css.matching_rules(&elem.local, &classes);
        if let Some(style) = elem.attr_any("style") {
            let decls = parse_declarations(style);
            entries.push((INLINE_PRIORITY, decls.normal));
            entries.push((IMPORTANT_PRIORITY + INLINE_PRIORITY, decls.important));
        }
        entries.sort_by_key(|&(priority, _)| priority); // stable: keeps source order
        let mut props = StyleProps::default();
        for (_, entry) in entries {
            props = props.merge(entry);
        }
        props
    }

    fn push_anchor(&mut self, elem: &Element) {
        if let Some(id) = elem.attr_any("id").filter(|i| !i.is_empty()) {
            self.inlines.push(Inline::Anchor(self.ctx.anchor_id(id)));
        }
        if elem.local == "a"
            && let Some(name) = elem.attr_any("name").filter(|n| !n.is_empty())
        {
            self.inlines.push(Inline::Anchor(self.ctx.anchor_id(name)));
        }
    }

    fn walk_children(&mut self, elem: &Element, delta: StyleDelta) -> Result<(), ConvertError> {
        for node in &elem.children {
            match node {
                Node::Text(t) => self.push_text(t, delta),
                Node::Elem(e) => self.walk_elem(e, delta)?,
            }
        }
        Ok(())
    }

    fn push_text(&mut self, text: &str, delta: StyleDelta) {
        let collapsed = collapse_ws(&clean_text(text));
        if collapsed.is_empty() {
            return;
        }
        // Collapse across node boundaries: leading whitespace vanishes at a
        // block start and after already-emitted whitespace, no matter how
        // the source split its text nodes and inline elements.
        let text = if at_space_boundary(&self.inlines, self.start_boundary) {
            collapsed.trim_start_matches(' ').to_string()
        } else {
            collapsed
        };
        if text.is_empty() {
            return;
        }
        self.inlines.push(Inline::Text { text, style: delta.resolve() });
    }

    fn walk_elem(&mut self, elem: &Element, delta: StyleDelta) -> Result<(), ConvertError> {
        let props = self.element_props(elem);
        if props.hidden == Some(true) {
            return Ok(());
        }
        // Cascade order: inherited delta, then the tag's presentational
        // default (`<b>`, `<i>`, …), then CSS — so `font-weight: normal`
        // can undo a `<b>`.
        let delta = merge_inline_tag(elem, delta).merge(props.delta);
        match elem.local.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush_paragraph();
                let level = elem.local[1..].parse::<u8>().unwrap_or(1);
                let mut content = self.inline_children(elem, delta)?;
                // The heading element's own styling is how it looks, not
                // markup applied to the words; a `<b>` inside it still is.
                rebase_emphasis(&mut content, delta.resolve());
                let anchor = elem.attr_any("id").map(|id| self.ctx.anchor_id(id));
                if !inlines_are_empty(&content) {
                    self.blocks.push(Block::Heading { level, anchor, content });
                } else {
                    // An empty heading still carries link targets: its own
                    // id and any anchors inside.
                    let kept: Vec<Inline> = anchor
                        .map(Inline::Anchor)
                        .into_iter()
                        .chain(content.into_iter().filter(|i| matches!(i, Inline::Anchor(_))))
                        .collect();
                    if !kept.is_empty() {
                        self.blocks.push(Block::Paragraph(kept));
                    }
                }
            }
            "p" => {
                self.flush_paragraph();
                self.push_anchor(elem);
                let mut content = std::mem::take(&mut self.inlines);
                content.extend(self.inline_children(elem, delta)?);
                if keeps_paragraph(&content) {
                    self.blocks.push(Block::Paragraph(content));
                }
            }
            "ul" | "ol" => {
                self.flush_paragraph();
                let lists = self.parse_list(elem, delta)?;
                self.blocks.extend(lists);
            }
            "table" => {
                self.flush_paragraph();
                if let Some(caption) = elem.child_elems().find(|e| e.local == "caption") {
                    let content = self.inline_children(caption, delta)?;
                    if keeps_paragraph(&content) {
                        self.blocks.push(Block::Paragraph(content));
                    }
                }
                if let Some(t) = self.parse_table(elem, delta)? {
                    self.blocks.push(t);
                }
            }
            "blockquote" => {
                self.flush_paragraph();
                let inner = self.sub_blocks(elem, delta)?;
                if !inner.is_empty() {
                    self.blocks.push(Block::BlockQuote(inner));
                }
            }
            "pre" => {
                self.flush_paragraph();
                let text = elem.text();
                if !text.trim().is_empty() {
                    self.blocks.push(Block::CodeBlock { lang: None, text });
                }
            }
            "hr" => {
                self.flush_paragraph();
                self.blocks.push(Block::Rule);
            }
            name if is_container_tag(name) => {
                self.push_anchor(elem);
                if has_block_children(elem) {
                    self.flush_paragraph();
                    self.walk_children(elem, delta)?;
                    self.flush_paragraph();
                } else {
                    self.walk_children(elem, delta)?;
                }
            }
            "script" | "style" | "head" | "template" | "noscript" => {}
            // MathML metadata, not content: an annotation holds a second
            // encoding of the same expression.
            "annotation" | "annotation-xml" => {}
            _ => self.walk_inline(elem, delta)?,
        }
        Ok(())
    }

    /// MathML. A `&lt;semantics&gt;` wrapper usually carries the source LaTeX in an
    /// `annotation`, which is exact where re-deriving it from the presentation
    /// tree is not; without one the characters are kept but the shape is lost.
    fn walk_math(&mut self, elem: &Element, delta: StyleDelta) -> Result<(), ConvertError> {
        let display = elem.attr_any("display") == Some("block");
        if let Some(latex) = tex_annotation(elem) {
            self.inlines.push(Inline::Math { latex, display });
            return Ok(());
        }
        log::debug!("MathML without a TeX annotation; keeping its characters only");
        self.walk_children(elem, delta)
    }

    fn walk_inline(&mut self, elem: &Element, delta: StyleDelta) -> Result<(), ConvertError> {
        self.push_anchor(elem);
        match elem.local.as_str() {
            "br" => self.inlines.push(Inline::LineBreak),
            "math" => self.walk_math(elem, delta)?,
            "img" | "image" => {
                let alt = clean_text(elem.attr_any("alt").unwrap_or(""));
                let src = elem.attr_any("src").or_else(|| elem.attr_any("href")).unwrap_or("");
                let source = self.ctx.image_source(src)?;
                if source.is_some() || !alt.trim().is_empty() {
                    self.inlines.push(Inline::Image {
                        alt,
                        source: source.unwrap_or(ImageSource::Unavailable),
                    });
                }
            }
            "a" => {
                let target = elem.attr_any("href").and_then(|href| self.ctx.link_target(href));
                let content = self.inline_children_at(
                    elem,
                    delta,
                    at_space_boundary(&self.inlines, self.start_boundary),
                )?;
                // An empty label still keeps a resolved target: the renderer
                // shows the URL as the link text.
                match target {
                    Some(target) => self.inlines.push(Inline::Link { content, target }),
                    None => self.inlines.extend(content),
                }
            }
            _ => self.walk_children(elem, delta)?,
        }
        Ok(())
    }

    fn inline_children(
        &mut self,
        elem: &Element,
        delta: StyleDelta,
    ) -> Result<Vec<Inline>, ConvertError> {
        self.inline_children_at(elem, delta, true)
    }

    /// Inline children starting at the given whitespace-boundary state.
    fn inline_children_at(
        &mut self,
        elem: &Element,
        delta: StyleDelta,
        start_boundary: bool,
    ) -> Result<Vec<Inline>, ConvertError> {
        let mut blocks = self.sub_blocks_at(elem, delta, start_boundary)?;
        if blocks.len() == 1
            && let Block::Paragraph(inlines) = &mut blocks[0]
        {
            return Ok(std::mem::take(inlines));
        }
        let mut out = Vec::new();
        for (i, block) in blocks.into_iter().enumerate() {
            if i > 0 {
                out.push(Inline::LineBreak);
            }
            match block {
                Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
                    out.extend(inlines)
                }
                other => out.push(Inline::plain(collapse_ws(&block_text(&other)))),
            }
        }
        Ok(out)
    }

    /// Ordered/unordered list with `start`, `reversed`, `type`, and per-item
    /// `value`. Non-contiguous numbering splits into consecutive list blocks
    /// so the rendered numbers match the source.
    fn parse_list(
        &mut self,
        elem: &Element,
        delta: StyleDelta,
    ) -> Result<Vec<Block>, ConvertError> {
        let ordered = elem.local == "ol";
        let items: Vec<&Element> = elem.child_elems().filter(|e| e.local == "li").collect();
        if items.is_empty() {
            return Ok(Vec::new());
        }
        if !ordered {
            let mut list_items = Vec::with_capacity(items.len());
            for li in &items {
                list_items.push(ListItem {
                    blocks: self.sub_blocks(li, delta)?,
                    checked: None,
                    marker_label: None,
                });
            }
            let list = List { marker: MarkerKind::Bullet, start: 1, items: list_items };
            return Ok(vec![Block::List(list)]);
        }
        let marker = match elem.attr_any("type") {
            Some("a") => MarkerKind::LowerAlpha,
            Some("A") => MarkerKind::UpperAlpha,
            Some("i") => MarkerKind::LowerRoman,
            Some("I") => MarkerKind::UpperRoman,
            _ => MarkerKind::Decimal,
        };
        let reversed = elem.attr_any("reversed").is_some();
        let start: i64 = elem
            .attr_any("start")
            .and_then(|v| v.parse().ok())
            .unwrap_or(if reversed { items.len() as i64 } else { 1 });
        let mut numbers: Vec<i64> = Vec::with_capacity(items.len());
        let mut next = start;
        for li in &items {
            if let Some(v) = li.attr_any("value").and_then(|v| v.parse::<i64>().ok()) {
                next = v;
            }
            numbers.push(next);
            // Source-controlled values sit anywhere in the i64 range; the
            // step must not overflow.
            next = if reversed { next.saturating_sub(1) } else { next.saturating_add(1) };
        }
        // Zero/negative numbers are valid ordered-list values but cannot be
        // a `start` for the renderer's start+index numbering; such lists
        // carry every number as an explicit literal marker instead.
        if numbers.iter().any(|&n| n < 1) {
            let mut list_items = Vec::with_capacity(items.len());
            for (li, &n) in items.iter().zip(&numbers) {
                list_items.push(ListItem {
                    blocks: self.sub_blocks(li, delta)?,
                    checked: None,
                    marker_label: Some(format!("{n}.")),
                });
            }
            return Ok(vec![Block::List(List { marker, start: 1, items: list_items })]);
        }
        let mut out: Vec<Block> = Vec::new();
        let mut current: Option<List> = None;
        let mut last_number = 0i64;
        for (li, &number) in items.iter().zip(&numbers) {
            let item =
                ListItem { blocks: self.sub_blocks(li, delta)?, checked: None, marker_label: None };
            let contiguous = current.is_some() && last_number.checked_add(1) == Some(number);
            if !contiguous {
                if let Some(list) = current.take() {
                    out.push(Block::List(list));
                }
                current = Some(List { marker, start: number as u64, items: Vec::new() });
            }
            current.as_mut().unwrap().items.push(item);
            last_number = number;
        }
        if let Some(list) = current {
            out.push(Block::List(list));
        }
        Ok(out)
    }

    fn parse_table(
        &mut self,
        elem: &Element,
        delta: StyleDelta,
    ) -> Result<Option<Block>, ConvertError> {
        // (row, is thead row, row-group index): each thead/tbody/tfoot is
        // one row group; consecutive direct `tr` children form an implicit
        // one. `rowspan="0"` spans to the end of its group.
        let mut row_elems: Vec<(&Element, bool, usize)> = Vec::new();
        let mut group = 0usize;
        let mut in_implicit_group = false;
        for child in elem.child_elems() {
            match child.local.as_str() {
                "thead" | "tbody" | "tfoot" => {
                    if in_implicit_group {
                        in_implicit_group = false;
                        group += 1;
                    }
                    let in_head = child.local == "thead";
                    for tr in child.child_elems().filter(|e| e.local == "tr") {
                        row_elems.push((tr, in_head, group));
                    }
                    group += 1;
                }
                "tr" => {
                    in_implicit_group = true;
                    row_elems.push((child, false, group));
                }
                _ => {}
            }
        }
        if row_elems.is_empty() {
            return Ok(None);
        }
        // Last row index of each group, for rowspan=0 expansion.
        let mut group_end: HashMap<usize, usize> = HashMap::new();
        for (i, &(_, _, g)) in row_elems.iter().enumerate() {
            group_end.insert(g, i);
        }
        let mut builder = GridBuilder::new();
        let mut header_rows = 0usize;
        for (i, (tr, in_head, grp)) in row_elems.iter().enumerate() {
            builder.next_row();
            let mut all_th = true;
            let mut any_cell = false;
            for cell in tr.child_elems() {
                if !matches!(cell.local.as_str(), "td" | "th") {
                    continue;
                }
                any_cell = true;
                if cell.local != "th" {
                    all_th = false;
                }
                // HTML clamps colspan to 1000 and rowspan to 65534.
                let col_span: u32 = cell
                    .attr_any("colspan")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let row_span: u32 = match cell.attr_any("rowspan").and_then(|v| v.parse().ok()) {
                    // rowspan=0: span all remaining rows of the row group.
                    Some(0) => (group_end[grp] - i + 1) as u32,
                    Some(n) => n.clamp(1, 65534),
                    None => 1,
                };
                let blocks = self.sub_blocks(cell, delta)?;
                builder.place(Cell::spanning(blocks, col_span, row_span))?;
            }
            if i == header_rows && (*in_head || (all_th && any_cell)) {
                header_rows += 1;
            }
        }
        let mut table = builder.finish(TableKind::Data);
        if table.grid.is_empty() {
            return Ok(None);
        }
        table.header_rows = resolve_header_rows(&table, header_rows);
        Ok(Some(Block::Table(table)))
    }
}

/// The LaTeX an `annotation` carries, if the MathML supplies one.
fn tex_annotation(math: &Element) -> Option<String> {
    let text = math
        .descendant_elems()
        .filter(|e| e.local == "annotation")
        .find(|e| {
            matches!(e.attr_any("encoding"), Some("application/x-tex" | "application/x-latex"))
        })?
        .text();
    let text = text.trim();
    if text.is_empty() { None } else { Some(text.replace(['\n', '\r'], " ")) }
}

fn merge_inline_tag(elem: &Element, mut delta: StyleDelta) -> StyleDelta {
    match elem.local.as_str() {
        "b" | "strong" => delta.bold = Some(true),
        "i" | "em" | "cite" | "dfn" | "var" => delta.italic = Some(true),
        "s" | "del" | "strike" => delta.strike = Some(true),
        "code" | "kbd" | "samp" | "tt" => delta.code = Some(true),
        "sup" => delta.vert_align = Some(VertAlign::Superscript),
        "sub" => delta.vert_align = Some(VertAlign::Subscript),
        _ => {}
    }
    delta
}

fn block_text(block: &Block) -> String {
    match block {
        Block::Paragraph(i) | Block::Heading { content: i, .. } => inlines_to_plain_text(i),
        Block::List(list) => list
            .items
            .iter()
            .flat_map(|it| it.blocks.iter().map(block_text))
            .collect::<Vec<_>>()
            .join(" "),
        Block::BlockQuote(blocks) => blocks.iter().map(block_text).collect::<Vec<_>>().join(" "),
        Block::CodeBlock { text, .. } => text.clone(),
        Block::Table(table) => table
            .grid
            .iter()
            .flat_map(|row| {
                row.iter().filter_map(|slot| match slot {
                    crate::model::CellSlot::Origin(cell) => {
                        Some(cell.blocks.iter().map(block_text).collect::<Vec<_>>().join(" "))
                    }
                    crate::model::CellSlot::Covered { .. } => None,
                })
            })
            .collect::<Vec<_>>()
            .join(" "),
        Block::Rule => String::new(),
    }
}

/// Elements that only group other content, walked through transparently.
fn is_container_tag(name: &str) -> bool {
    matches!(
        name,
        "div"
            | "section"
            | "article"
            | "aside"
            | "main"
            | "nav"
            | "header"
            | "footer"
            | "figure"
            | "figcaption"
            | "center"
            | "details"
            | "summary"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "body"
    )
}

fn is_block_tag(name: &str) -> bool {
    is_container_tag(name)
        || matches!(
            name,
            "p" | "ul"
                | "ol"
                | "table"
                | "blockquote"
                | "pre"
                | "hr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
        )
}

fn has_block_children(elem: &Element) -> bool {
    elem.child_elems().any(|e| is_block_tag(&e.local))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellSlot, ImageSource, Style};
    use crate::package::xml::parse_xml;

    struct NullCtx;

    impl HtmlCtx for NullCtx {
        fn link_target(&self, href: &str) -> Option<LinkTarget> {
            (!href.is_empty()).then(|| LinkTarget::External(href.to_string()))
        }

        fn image_source(&self, _src: &str) -> Result<Option<ImageSource>, ConvertError> {
            Ok(None)
        }

        fn anchor_id(&self, raw: &str) -> AnchorId {
            raw.to_string()
        }
    }

    fn blocks_with_css(html: &str, css: &str) -> Vec<Block> {
        let tree = parse_xml(html.as_bytes()).unwrap();
        let body = tree.child_elems().next().unwrap();
        let mut sheet = Stylesheet::default();
        sheet.add(css);
        to_blocks(body, &sheet, &NullCtx).unwrap()
    }

    fn blocks(html: &str) -> Vec<Block> {
        blocks_with_css(html, "")
    }

    fn para_text(block: &Block) -> String {
        let Block::Paragraph(inlines) = block else { panic!("expected paragraph: {block:?}") };
        crate::model::inlines_to_plain_text(inlines)
    }

    fn first_text_style(inlines: &[Inline]) -> Style {
        for inline in inlines {
            match inline {
                Inline::Text { style, .. } => return *style,
                Inline::Link { content, .. } => return first_text_style(content),
                _ => {}
            }
        }
        panic!("no text run in {inlines:?}");
    }

    #[test]
    fn whitespace_collapses_across_inline_boundaries() {
        // M7: a lone space inside a span must survive between words, and
        // formatting splits must not double or drop spaces.
        let out = blocks("<body><p>foo<span> </span>bar</p></body>");
        assert_eq!(para_text(&out[0]), "foo bar");
        let out = blocks("<body><p>foo <span> bar</span></p></body>");
        assert_eq!(para_text(&out[0]), "foo bar");
        let out = blocks("<body><p>  foo\n  bar  </p></body>");
        assert_eq!(para_text(&out[0]), "foo bar ");
    }

    #[test]
    fn link_label_whitespace_joins_the_surrounding_run() {
        // `foo <a> bar</a>`: the label's leading space collapses with the
        // space already emitted before the link.
        let out = blocks(r#"<body><p>foo <a href="u"> bar</a></p></body>"#);
        let Block::Paragraph(inlines) = &out[0] else { panic!() };
        let Some(Inline::Link { content, .. }) =
            inlines.iter().find(|i| matches!(i, Inline::Link { .. }))
        else {
            panic!("no link in {inlines:?}");
        };
        assert_eq!(crate::model::inlines_to_plain_text(content), "bar");
        // `foo<a> bar</a>`: no space yet, so the label keeps its lead.
        let out = blocks(r#"<body><p>foo<a href="u"> bar</a></p></body>"#);
        assert_eq!(para_text(&out[0]), "foo bar");
    }

    #[test]
    fn inline_style_overrides_the_tag_default() {
        // M7: `<b style="font-weight: normal">` renders plain.
        let out = blocks(r#"<body><p><b style="font-weight: normal">x</b></p></body>"#);
        let Block::Paragraph(inlines) = &out[0] else { panic!() };
        assert!(!first_text_style(inlines).bold);
    }

    #[test]
    fn important_declarations_win_the_cascade() {
        let css = "span.x { font-weight: bold !important } span.x { font-weight: normal }";
        let out = blocks_with_css(r#"<body><p><span class="x">x</span></p></body>"#, css);
        let Block::Paragraph(inlines) = &out[0] else { panic!() };
        assert!(first_text_style(inlines).bold);
    }

    #[test]
    fn heading_styling_stays_out_of_its_content() {
        // A heading's own CSS is how the heading looks, so it does not reach
        // the runs; emphasis a child adds beyond it still does.
        let css = "h2 { font-style: italic; font-weight: bold }";
        let out = blocks_with_css("<body><h2>title <b>b</b></h2></body>", css);
        let Block::Heading { content, .. } = &out[0] else { panic!("{out:?}") };
        assert_eq!(first_text_style(content), Style::PLAIN);

        let out = blocks_with_css("<body><h2>title <b>b</b></h2></body>", "");
        let Block::Heading { content, .. } = &out[0] else { panic!("{out:?}") };
        assert_eq!(first_text_style(content), Style::PLAIN);
        let Some(Inline::Text { style, .. }) =
            content.iter().rfind(|i| matches!(i, Inline::Text { .. }))
        else {
            panic!()
        };
        assert!(style.bold);
    }

    #[test]
    fn extreme_ordered_list_values_do_not_overflow() {
        // H2: source-controlled `start`/`value` sit anywhere in i64.
        let html = format!(
            "<body><ol start=\"{}\"><li>a</li><li>b</li><li value=\"{}\">c</li><li>d</li></ol></body>",
            i64::MAX,
            i64::MIN,
        );
        let out = blocks(&html);
        assert!(!out.is_empty());
    }

    #[test]
    fn rowspan_zero_spans_to_the_end_of_the_row_group() {
        // M8: rowspan="0" covers the remaining rows of its row group only.
        let html = r#"<body><table>
            <tbody>
                <tr><td rowspan="0">tall</td><td>a</td></tr>
                <tr><td>b</td></tr>
            </tbody>
            <tbody><tr><td>c</td><td>d</td></tr></tbody>
        </table></body>"#;
        let out = blocks(html);
        let Block::Table(t) = &out[0] else { panic!("{out:?}") };
        assert!(matches!(&t.grid[0][0], CellSlot::Origin(c) if c.row_span == 2));
        assert!(matches!(t.grid[1][0], CellSlot::Covered { origin_row: 0, origin_col: 0 }));
        assert!(matches!(&t.grid[2][0], CellSlot::Origin(_)), "next group must not be covered");
    }
}
