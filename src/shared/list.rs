//! Nested-list assembly with numbering identity.
//!
//! Frontends resolve each list paragraph to a [`ListEntry`] carrying its
//! indent level, its list identity ([`ListKey`]), and its computed effective
//! number. Assembly splits runs whenever the identity or marker kind changes
//! at a level, or an ordered sequence is non-contiguous (a restart), so the
//! renderer's `start + index` numbering reproduces the source exactly.

pub use crate::model::MarkerKind;
use crate::model::{Block, List, ListItem};

/// Identity of a resolved list at one level: which list instance the entry
/// belongs to and what marker its level uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListKey {
    /// Stable per-instance id (DOCX `numId`, RTF `\lsN`, DOC list identity
    /// `lsid`, a counter for HTML/ODF lists).
    pub instance: u64,
    pub marker: MarkerKind,
}

/// One flat, fully resolved list paragraph (plus any blocks attached to the
/// same item, like text-box content anchored in it).
#[derive(Debug)]
pub struct ListEntry {
    pub level: usize,
    pub key: ListKey,
    /// Effective item number at this entry (ignored for bullets).
    pub number: u64,
    /// Literal marker text when the source number text is not reproducible
    /// from the marker kind and number alone (composite number text).
    pub label: Option<String>,
    pub blocks: Vec<Block>,
}

/// Pop the accumulated run of list paragraphs into list blocks; one block
/// per identity segment.
pub fn flush_list(blocks: &mut Vec<Block>, run: &mut Vec<ListEntry>) {
    let entries = std::mem::take(run);
    if entries.is_empty() {
        return;
    }
    blocks.extend(build_lists(entries));
}

/// Fold a flat run into nested lists, splitting at identity/marker changes
/// and ordered-sequence discontinuities.
fn build_lists(entries: Vec<ListEntry>) -> Vec<Block> {
    let Some(min_lvl) = entries.iter().map(|e| e.level).min() else {
        return Vec::new();
    };
    let mut out: Vec<Block> = Vec::new();
    let mut current: Option<(List, ListKey, u64)> = None; // list, key, last number

    let flush_current = |current: &mut Option<(List, ListKey, u64)>, out: &mut Vec<Block>| {
        if let Some((list, _, _)) = current.take()
            && !list.items.is_empty()
        {
            out.push(Block::List(list));
        }
    };

    let mut iter = entries.into_iter().peekable();
    while let Some(&ListEntry { level, key, number, .. }) = iter.peek() {
        if level <= min_lvl {
            let entry = iter.next().unwrap();
            let split = match &current {
                Some((_, cur_key, last_number)) => {
                    *cur_key != key
                        || (key.marker.ordered() && last_number.checked_add(1) != Some(number))
                }
                None => true,
            };
            if split {
                flush_current(&mut current, &mut out);
                let list = List {
                    marker: key.marker,
                    start: if key.marker.ordered() { number } else { 1 },
                    items: Vec::new(),
                };
                current = Some((list, key, number));
            }
            let (list, _, last) = current.as_mut().unwrap();
            list.items.push(ListItem { blocks: entry.blocks, marker_label: entry.label });
            *last = number;
        } else {
            let mut sub = Vec::new();
            while iter.peek().is_some_and(|e| e.level > min_lvl) {
                sub.push(iter.next().unwrap());
            }
            let sublists = build_lists(sub);
            if sublists.is_empty() {
                continue;
            }
            if current.is_none() {
                // Sub-level content with no parent item yet: host it in an
                // anonymous item so nesting is preserved.
                let key = ListKey { instance: u64::MAX, marker: MarkerKind::Bullet };
                current = Some((
                    List { marker: MarkerKind::Bullet, start: 1, items: Vec::new() },
                    key,
                    0,
                ));
            }
            let (list, _, _) = current.as_mut().unwrap();
            if list.items.is_empty() {
                list.items.push(ListItem::default());
            }
            list.items.last_mut().unwrap().blocks.extend(sublists);
        }
    }
    flush_current(&mut current, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Inline;

    fn entry(
        level: usize,
        instance: u64,
        marker: MarkerKind,
        number: u64,
        text: &str,
    ) -> ListEntry {
        ListEntry {
            level,
            key: ListKey { instance, marker },
            number,
            label: None,
            blocks: vec![Block::Paragraph(vec![Inline::plain(text)])],
        }
    }

    fn lists(entries: Vec<ListEntry>) -> Vec<Block> {
        build_lists(entries)
    }

    #[test]
    fn contiguous_numbers_stay_one_list() {
        let out = lists(vec![
            entry(0, 1, MarkerKind::Decimal, 1, "a"),
            entry(0, 1, MarkerKind::Decimal, 2, "b"),
        ]);
        assert_eq!(out.len(), 1);
        let Block::List(l) = &out[0] else { panic!() };
        assert!(l.ordered());
        assert_eq!((l.start, l.items.len()), (1, 2));
    }

    #[test]
    fn restart_splits_with_new_start() {
        let out = lists(vec![
            entry(0, 1, MarkerKind::Decimal, 1, "a"),
            entry(0, 1, MarkerKind::Decimal, 10, "restarted"),
        ]);
        assert_eq!(out.len(), 2);
        let Block::List(l) = &out[1] else { panic!() };
        assert_eq!(l.start, 10);
    }

    #[test]
    fn distinct_instances_split() {
        let out = lists(vec![
            entry(0, 1, MarkerKind::Decimal, 1, "a"),
            entry(0, 2, MarkerKind::Decimal, 1, "b"),
        ]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn marker_change_splits() {
        let out = lists(vec![
            entry(0, 1, MarkerKind::Decimal, 1, "a"),
            entry(0, 1, MarkerKind::Bullet, 0, "b"),
        ]);
        assert_eq!(out.len(), 2);
        let Block::List(l) = &out[1] else { panic!() };
        assert!(!l.ordered());
    }

    #[test]
    fn nesting_preserved() {
        let out = lists(vec![
            entry(0, 1, MarkerKind::Decimal, 1, "outer"),
            entry(1, 1, MarkerKind::LowerRoman, 1, "inner"),
            entry(0, 1, MarkerKind::Decimal, 2, "outer2"),
        ]);
        assert_eq!(out.len(), 1);
        let Block::List(l) = &out[0] else { panic!() };
        assert_eq!(l.items.len(), 2);
        assert!(matches!(l.items[0].blocks.last(), Some(Block::List(sub)) if sub.ordered()));
    }
}
