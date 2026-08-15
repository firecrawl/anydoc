//! ODF style resolution: text/paragraph style chains as tri-state deltas
//! (so `font-weight: normal` can clear inherited bold), plus list styles
//! with per-level marker kinds and start values.
//!
//! Chains are keyed by owned `family\0name` strings because definitions come
//! from two separately parsed trees (`styles.xml` and `content.xml`).

use crate::error::ConvertError;
use crate::package::xml::{Element, ns};
use crate::shared::blockstyle::{self, BlockStyle};
use crate::shared::delta::StyleDelta;
use crate::shared::list::MarkerKind;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub const LIST_LEVELS: usize = 10;

#[derive(Debug, Clone)]
pub struct ListLevel {
    pub marker: MarkerKind,
    pub start: u64,
    /// `style:num-prefix` literal text before the number.
    pub prefix: String,
    /// `style:num-suffix` literal text after the number; absent means the
    /// bare number is displayed.
    pub suffix: Option<String>,
    /// `text:display-levels`: how many levels (ending at this one) display,
    /// joined by `.`.
    pub display_levels: usize,
}

impl Default for ListLevel {
    fn default() -> Self {
        ListLevel {
            marker: MarkerKind::Bullet,
            start: 1,
            prefix: String::new(),
            suffix: None,
            display_levels: 1,
        }
    }
}

impl ListLevel {
    /// The level's number text as the shared pattern IR, referencing the
    /// trailing `display_levels` levels.
    pub fn pattern(&self, depth: usize) -> crate::shared::numbering::NumberPattern {
        use crate::shared::numbering::{NumberPattern, NumberText};
        let mut text = Vec::new();
        if !self.prefix.is_empty() {
            text.push(NumberText::Literal(self.prefix.clone()));
        }
        let shown = self.display_levels.clamp(1, depth + 1);
        for (i, level) in (depth + 1 - shown..=depth).enumerate() {
            if i > 0 {
                text.push(NumberText::Literal(".".into()));
            }
            text.push(NumberText::Level(level as u8));
        }
        if let Some(suffix) = &self.suffix
            && !suffix.is_empty()
        {
            text.push(NumberText::Literal(suffix.clone()));
        }
        NumberPattern { text, legal: false }
    }
}

#[derive(Default)]
pub struct OdfStyles<'a> {
    raw: HashMap<String, (&'a Element, Option<String>)>,
    memo: RefCell<HashMap<String, StyleDelta>>,
    list_styles: HashMap<String, [ListLevel; LIST_LEVELS]>,
    /// `text:outline-style`: heading numbering per outline level.
    outline: Option<[ListLevel; LIST_LEVELS]>,
    /// `style:default-style` per family: the base beneath every named chain
    /// (and the delta for unstyled content of that family).
    defaults: HashMap<String, StyleDelta>,
}

fn key(family: &str, name: &str) -> String {
    format!("{family}\u{0}{name}")
}

impl<'a> OdfStyles<'a> {
    /// Collect styles from one document tree (`styles.xml` or `content.xml`).
    /// Call for styles.xml first so content.xml definitions chain onto it.
    pub fn collect(&mut self, tree: &'a Element) {
        for root in tree.child_elems() {
            for section in root.child_elems() {
                if !(section.is(ns::OFFICE, "automatic-styles") || section.is(ns::OFFICE, "styles"))
                {
                    continue;
                }
                for style in section.child_elems() {
                    if style.is(ns::STYLE, "default-style")
                        && let Some(family) = style.attr(ns::STYLE, "family")
                    {
                        self.defaults.insert(family.to_string(), text_properties_delta(style));
                    }
                    if style.is(ns::STYLE, "style")
                        && let Some(name) = style.attr(ns::STYLE, "name")
                    {
                        let family = style.attr(ns::STYLE, "family").unwrap_or("");
                        let parent =
                            style.attr(ns::STYLE, "parent-style-name").map(|p| key(family, p));
                        self.raw.insert(key(family, name), (style, parent));
                    }
                    if style.is(ns::TEXT, "list-style")
                        && let Some(name) = style.attr(ns::STYLE, "name")
                    {
                        self.list_styles.insert(name.to_string(), parse_list_style(style));
                    }
                    if style.is(ns::TEXT, "outline-style") {
                        self.outline = Some(parse_list_style(style));
                    }
                }
            }
        }
    }

    /// Cumulative delta of a style through its `parent-style-name` chain,
    /// over the family's `style:default-style` base.
    pub fn delta(&self, family: &str, name: &str) -> Result<StyleDelta, ConvertError> {
        let id = key(family, name);
        if let Some(hit) = self.memo.borrow().get(&id) {
            return Ok(*hit);
        }
        let mut chain: Vec<&str> = Vec::new();
        let mut visited: HashSet<&str> = HashSet::new();
        let mut cursor = self.raw.get_key_value(&id).map(|(k, _)| k.as_str());
        while let Some(current) = cursor {
            if !visited.insert(current) {
                return Err(ConvertError::malformed(format!(
                    "style inheritance cycle at {current:?}"
                )));
            }
            chain.push(current);
            cursor = match self.raw.get(current) {
                Some((_, Some(parent))) => {
                    self.raw.get_key_value(parent.as_str()).map(|(k, _)| k.as_str())
                }
                _ => None,
            };
        }
        let mut delta = self.defaults.get(family).copied().unwrap_or_default();
        for current in chain.into_iter().rev() {
            let (def, _) = self.raw[current];
            delta = delta.merge(text_properties_delta(def));
            self.memo.borrow_mut().insert(current.to_string(), delta);
        }
        Ok(delta)
    }

    /// The block container a paragraph style names, walking
    /// `parent-style-name`: an automatic style carries the name on the
    /// named style it derives from.
    pub fn block_style(&self, name: &str) -> Option<BlockStyle> {
        let mut id = key("paragraph", name);
        let mut visited: HashSet<String> = HashSet::new();
        while visited.insert(id.clone()) {
            let (def, parent) = self.raw.get(&id)?;
            if let Some(hit) = def.attr(ns::STYLE, "name").and_then(blockstyle::from_style_name) {
                return Some(hit);
            }
            id = parent.clone()?;
        }
        None
    }

    pub fn list_level(&self, style_name: &str, depth: usize) -> ListLevel {
        self.list_styles
            .get(style_name)
            .and_then(|levels| levels.get(depth).cloned())
            .unwrap_or_default()
    }

    /// The full level array of a list style, for composite-label rendering.
    pub fn list_levels(&self, style_name: &str) -> Option<&[ListLevel; LIST_LEVELS]> {
        self.list_styles.get(style_name)
    }

    /// The document's heading numbering (`text:outline-style`) levels.
    pub fn outline_levels(&self) -> Option<&[ListLevel; LIST_LEVELS]> {
        self.outline.as_ref()
    }
}

/// Clamp an untrusted start value so counters can never overflow.
pub fn parse_start(v: &str) -> Option<u64> {
    v.parse::<u64>().ok().map(|n| n.min(u32::MAX as u64))
}

/// Levels of a `text:list-style` or `text:outline-style` (the level
/// elements differ in name, not in shape).
fn parse_list_style(style: &Element) -> [ListLevel; LIST_LEVELS] {
    let mut levels: [ListLevel; LIST_LEVELS] = std::array::from_fn(|_| ListLevel::default());
    for lvl in style.child_elems() {
        let Some(n) = lvl.attr(ns::TEXT, "level").and_then(|v| v.parse::<usize>().ok()) else {
            continue;
        };
        if !(1..=LIST_LEVELS).contains(&n) {
            continue;
        }
        let slot = &mut levels[n - 1];
        if lvl.is(ns::TEXT, "list-level-style-number") || lvl.is(ns::TEXT, "outline-level-style") {
            slot.marker = match lvl.attr(ns::STYLE, "num-format") {
                Some("a") => MarkerKind::LowerAlpha,
                Some("A") => MarkerKind::UpperAlpha,
                Some("i") => MarkerKind::LowerRoman,
                Some("I") => MarkerKind::UpperRoman,
                Some("") => MarkerKind::Bullet,
                _ => MarkerKind::Decimal,
            };
            slot.start = lvl.attr(ns::TEXT, "start-value").and_then(parse_start).unwrap_or(1);
            slot.prefix = lvl.attr(ns::STYLE, "num-prefix").unwrap_or_default().to_string();
            slot.suffix = lvl.attr(ns::STYLE, "num-suffix").map(str::to_string);
            slot.display_levels =
                lvl.attr(ns::TEXT, "display-levels").and_then(|v| v.parse().ok()).unwrap_or(1);
        }
    }
    levels
}

/// Delta carried by a style's `style:text-properties`.
pub fn text_properties_delta(elem: &Element) -> StyleDelta {
    let Some(props) = elem.find(ns::STYLE, "text-properties") else {
        return StyleDelta::default();
    };
    StyleDelta {
        bold: props
            .attr(ns::FO, "font-weight")
            .map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)),
        italic: props.attr(ns::FO, "font-style").map(|s| s == "italic" || s == "oblique"),
        strike: props.attr(ns::STYLE, "text-line-through-style").map(|lt| lt != "none"),
        code: None,
    }
}
