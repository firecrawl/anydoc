//! Anchor resolution: maps every internal anchor id to the fragment it will
//! have in the rendered Markdown. Heading-coincident anchors reuse the
//! heading's GFM auto-generated slug; an anchor a link targets gets a
//! sanitized, stable HTML id rendered as `<a id="..."></a>` at its position.
//!
//! Anchors nothing links to render nothing, except structural slide boundary
//! anchors: producers mark up far more positions than they reference (a
//! bookmark per paragraph, an id per EPUB element), and an unreachable target
//! is usually only noise in the output.

use crate::model::{Block, Document, Inline, LinkTarget, inlines_to_plain_text};
use std::collections::{HashMap, HashSet};

pub(crate) struct AnchorMap {
    resolved: HashMap<String, Resolved>,
}

struct Resolved {
    fragment: String,
    /// True when the anchor needs an explicit `<a id>` emitted at its
    /// position (false for heading-coincident anchors - the heading's own
    /// slug carries them).
    emit_html: bool,
}

impl AnchorMap {
    /// The `#fragment` a link to `id` should use, if the target exists.
    pub(crate) fn fragment(&self, id: &str) -> Option<&str> {
        self.resolved.get(id).map(|r| r.fragment.as_str())
    }

    /// The HTML id to emit for an `Inline::Anchor` node, when one is needed.
    pub(crate) fn html_id(&self, id: &str) -> Option<&str> {
        self.resolved.get(id).filter(|r| r.emit_html).map(|r| r.fragment.as_str())
    }
}

pub(crate) fn resolve_anchors(doc: &Document) -> AnchorMap {
    let mut ids = UniqueIds::default();
    let mut resolved: HashMap<String, Resolved> = HashMap::new();

    // Anchor ids some link in the document targets, notes included.
    let mut linked: HashSet<&str> = HashSet::new();
    walk_blocks(&doc.blocks, &mut |block| collect_link_targets(block, &mut linked));
    for note in &doc.notes {
        walk_blocks(&note.blocks, &mut |block| collect_link_targets(block, &mut linked));
    }

    // Pass 1: headings claim their GFM slugs in render order, binding any
    // heading-coincident anchor ids (the `anchor` field and anchor nodes
    // inside the heading content) to those slugs.
    walk_blocks(&doc.blocks, &mut |block| {
        if let Block::Heading { anchor, content, .. } = block {
            let Some(slug) = ids.claim(gfm_slug(&inlines_to_plain_text(content))) else {
                return;
            };
            let mut bind = |id: &str| {
                resolved
                    .entry(id.to_string())
                    .or_insert(Resolved { fragment: slug.clone(), emit_html: false });
            };
            if let Some(id) = anchor {
                bind(id);
            }
            for_each_anchor(content, &mut bind);
        }
    });

    // Pass 2: every remaining anchor a link targets gets a sanitized HTML id.
    // Slide boundary anchors are structural, so they render even when no
    // internal link targets them.
    let mut assign = |id: &str| {
        if (linked.contains(id) || is_slide_boundary_anchor(id))
            && !resolved.contains_key(id)
            && let Some(html) = ids.claim(sanitize_id(id))
        {
            resolved.insert(id.to_string(), Resolved { fragment: html, emit_html: true });
        }
    };
    walk_blocks(&doc.blocks, &mut |block| bind_block_anchors(block, &mut assign));
    for note in &doc.notes {
        walk_blocks(&note.blocks, &mut |block| bind_block_anchors(block, &mut assign));
    }

    AnchorMap { resolved }
}

fn collect_link_targets<'a>(block: &'a Block, out: &mut HashSet<&'a str>) {
    match block {
        Block::Heading { content, .. } | Block::Paragraph(content) => {
            for_each_link_target(content, out)
        }
        _ => {}
    }
}

fn for_each_link_target<'a>(inlines: &'a [Inline], out: &mut HashSet<&'a str>) {
    for inline in inlines {
        if let Inline::Link { content, target } = inline {
            if let LinkTarget::Anchor(id) = target {
                out.insert(id.as_str());
            }
            for_each_link_target(content, out);
        }
    }
}

fn bind_block_anchors(block: &Block, assign: &mut impl FnMut(&str)) {
    match block {
        Block::Heading { content, .. } | Block::Paragraph(content) => {
            for_each_anchor(content, assign)
        }
        _ => {}
    }
}

/// Depth-first walk over all blocks, using an explicit stack.
fn walk_blocks<'a>(blocks: &'a [Block], f: &mut impl FnMut(&'a Block)) {
    let mut stack: Vec<&Block> = blocks.iter().rev().collect();
    while let Some(block) = stack.pop() {
        f(block);
        match block {
            Block::List(list) => {
                for item in list.items.iter().rev() {
                    stack.extend(item.blocks.iter().rev());
                }
            }
            Block::Table(table) => {
                for row in table.grid.iter().rev() {
                    for slot in row.iter().rev() {
                        if let crate::model::CellSlot::Origin(cell) = slot {
                            stack.extend(cell.blocks.iter().rev());
                        }
                    }
                }
            }
            Block::BlockQuote(inner) => stack.extend(inner.iter().rev()),
            _ => {}
        }
    }
}

fn for_each_anchor(inlines: &[Inline], f: &mut impl FnMut(&str)) {
    for inline in inlines {
        match inline {
            Inline::Anchor(id) => f(id),
            Inline::Link { content, .. } => for_each_anchor(content, f),
            _ => {}
        }
    }
}

fn is_slide_boundary_anchor(id: &str) -> bool {
    let Some(n) = id.strip_prefix("slide-") else {
        return false;
    };

    matches!(n.parse::<usize>(), Ok(value) if value > 0)
}

/// Allocates ids without repeatedly probing used numeric suffixes.
#[derive(Default)]
struct UniqueIds {
    used: HashSet<String>,
    next_suffix: HashMap<String, usize>,
}

impl UniqueIds {
    fn claim(&mut self, base: String) -> Option<String> {
        if self.used.insert(base.clone()) {
            self.next_suffix.entry(base.clone()).or_insert(1);
            return Some(base);
        }
        let mut n = self.next_suffix.get(&base).copied().unwrap_or(1);
        loop {
            let candidate = format!("{base}-{n}");
            n = n.checked_add(1)?;
            if self.used.insert(candidate.clone()) {
                self.next_suffix.insert(base, n);
                self.next_suffix.entry(candidate.clone()).or_insert(1);
                return Some(candidate);
            }
        }
    }
}

/// GFM-style heading slugs: full-Unicode lowercase, spaces become hyphens,
/// and everything except word-forming characters (letters, numbers, marks,
/// connector punctuation) and hyphens drops. An empty result becomes
/// `section` so the anchor stays linkable.
fn gfm_slug(text: &str) -> String {
    let slug: String = text
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' => Some(c),
            c if c.is_alphanumeric() || is_combining_mark(c) || is_connector_punctuation(c) => {
                Some(c)
            }
            _ => None,
        })
        .collect();
    if slug.is_empty() { "section".to_string() } else { slug }
}

/// Combining-mark blocks `is_alphanumeric` misses (marks outside
/// `Other_Alphabetic`, such as viramas and cantillation/stress signs).
fn is_combining_mark(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0900..=0x0903
            | 0x093A..=0x094F
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

/// Connector punctuation (category Pc): joins words, kept like `_`.
fn is_connector_punctuation(c: char) -> bool {
    matches!(
        u32::from(c),
        0x005F | 0x203F | 0x2040 | 0x2054 | 0xFE33 | 0xFE34 | 0xFE4D..=0xFE4F | 0xFF3F
    )
}

/// Sanitize a source anchor id into a stable `[a-z0-9-_]` HTML id.
fn sanitize_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    let mut prev_dash = false;
    for c in id.chars() {
        let mapped = match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '_' | '-') => c,
            _ => '-',
        };
        if mapped == '-' && prev_dash {
            continue;
        }
        prev_dash = mapped == '-';
        out.push(mapped);
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() { "anchor".to_string() } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_keep_word_forming_characters() {
        assert_eq!(gfm_slug("Hello World!"), "hello-world");
        assert_eq!(gfm_slug("  leading and trailing  "), "leading-and-trailing");
        assert_eq!(gfm_slug("under_score and hy-phen"), "under_score-and-hy-phen");
        assert_eq!(gfm_slug("C++ & Rust"), "c--rust");
        assert_eq!(gfm_slug("étude précomposée"), "étude-précomposée");
        assert_eq!(gfm_slug("combining n\u{0303} tilde"), "combining-n\u{0303}-tilde");
        assert_eq!(gfm_slug("देवनागरी क्षि"), "देवनागरी-क्षि");
        assert_eq!(gfm_slug("日本語の見出し"), "日本語の見出し");
    }

    #[test]
    fn connector_punctuation_is_kept() {
        // S15: category Pc joins words, like the underscore.
        assert_eq!(gfm_slug("a‿b undertie"), "a‿b-undertie");
        assert_eq!(gfm_slug("a⁀b tie"), "a⁀b-tie");
        assert_eq!(gfm_slug("a＿b fullwidth"), "a＿b-fullwidth");
    }

    #[test]
    fn punctuation_and_symbols_drop() {
        assert_eq!(gfm_slug("quotes 'single' \"double\""), "quotes-single-double");
        assert_eq!(gfm_slug("copyright © and € and ∑"), "copyright--and--and-");
        assert_eq!(gfm_slug("parens (and) [brackets]"), "parens-and-brackets");
    }

    #[test]
    fn empty_slugs_become_section() {
        assert_eq!(gfm_slug("!!!"), "section");
        assert_eq!(gfm_slug(""), "section");
    }

    #[test]
    fn duplicate_slugs_get_numeric_suffixes() {
        let mut ids = UniqueIds::default();
        assert_eq!(ids.claim(gfm_slug("dup")).as_deref(), Some("dup"));
        assert_eq!(ids.claim(gfm_slug("dup")).as_deref(), Some("dup-1"));
        assert_eq!(ids.claim(gfm_slug("dup")).as_deref(), Some("dup-2"));
    }

    #[test]
    fn duplicate_slug_suffixes_advance_without_reprobing() {
        let mut ids = UniqueIds::default();
        for n in 0..10_000 {
            let expected = if n == 0 { "same".to_string() } else { format!("same-{n}") };
            assert_eq!(ids.claim("same".to_string()), Some(expected));
        }
        assert_eq!(ids.next_suffix.get("same"), Some(&10_000));
    }
}
