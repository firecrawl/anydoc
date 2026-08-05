//! PresentationML text-property cascade: run properties resolve through
//! paragraph `pPr` -> shape `lstStyle` -> layout placeholder -> master
//! placeholder / `txStyles` -> presentation `defaultTextStyle`, with
//! explicit-off states honored at every layer.

use crate::package::xml::{Element, ns};
use crate::shared::delta::StyleDelta;
use crate::shared::list::MarkerKind;

pub const LEVELS: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bullet {
    /// Not specified at this layer; inherit from the next one.
    #[default]
    Inherit,
    /// Explicitly no bullet (`a:buNone`).
    None,
    /// Character bullet (`a:buChar`).
    Char,
    /// Auto-numbered (`a:buAutoNum`).
    AutoNum { marker: MarkerKind, start: u64, wrap: NumWrap },
}

/// The punctuation an `ST_TextAutoNumberScheme` wraps around the ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumWrap {
    /// `1.` — matches the renderer's default label.
    #[default]
    Period,
    /// `1)`
    ParenR,
    /// `(1)`
    ParenBoth,
    /// Bare `1`.
    Plain,
}

impl NumWrap {
    /// The literal marker label for ordinal `n`; `None` when the default
    /// label (`n.`) is already faithful.
    pub fn label(self, marker: MarkerKind, n: u64) -> Option<String> {
        match self {
            NumWrap::Period => None,
            NumWrap::ParenR => Some(format!("{})", marker.ordinal(n))),
            NumWrap::ParenBoth => Some(format!("({})", marker.ordinal(n))),
            NumWrap::Plain => Some(marker.ordinal(n)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TextProps {
    pub delta: StyleDelta,
    pub bullet: Bullet,
}

impl TextProps {
    /// Overlay `over` on `self`: explicit values win, `Inherit` falls through.
    pub fn merge(self, over: TextProps) -> TextProps {
        TextProps {
            delta: self.delta.merge(over.delta),
            bullet: if over.bullet == Bullet::Inherit { self.bullet } else { over.bullet },
        }
    }
}

/// Per-level text properties from a `lstStyle`-shaped element
/// (`a:lvl1pPr`..`a:lvl9pPr` children).
#[derive(Debug, Clone, Copy, Default)]
pub struct LevelStyle {
    levels: [TextProps; LEVELS],
}

impl LevelStyle {
    pub fn level(&self, lvl: usize) -> TextProps {
        self.levels[lvl.min(LEVELS - 1)]
    }
}

pub fn parse_level_styles(elem: Option<&Element>) -> LevelStyle {
    let mut out = LevelStyle::default();
    let Some(elem) = elem else {
        return out;
    };
    for (i, slot) in out.levels.iter_mut().enumerate() {
        let name = format!("lvl{}pPr", i + 1);
        if let Some(ppr) = elem.child_elems().find(|e| e.is(ns::A, &name)) {
            *slot = paragraph_props(ppr);
        }
    }
    out
}

/// Properties carried by one `pPr`-shaped element (direct paragraph
/// properties or one `lvlNpPr` level).
pub fn paragraph_props(ppr: &Element) -> TextProps {
    let bullet = if ppr.find(ns::A, "buNone").is_some() {
        Bullet::None
    } else if let Some(auto) = ppr.find(ns::A, "buAutoNum") {
        let scheme = auto.attr(ns::A, "type").unwrap_or("");
        let marker = match scheme {
            t if t.starts_with("alphaLc") => MarkerKind::LowerAlpha,
            t if t.starts_with("alphaUc") => MarkerKind::UpperAlpha,
            t if t.starts_with("romanLc") => MarkerKind::LowerRoman,
            t if t.starts_with("romanUc") => MarkerKind::UpperRoman,
            _ => MarkerKind::Decimal,
        };
        let wrap = match scheme {
            t if t.ends_with("ParenBoth") => NumWrap::ParenBoth,
            t if t.ends_with("ParenR") => NumWrap::ParenR,
            t if t.ends_with("Plain") => NumWrap::Plain,
            _ => NumWrap::Period,
        };
        // ST_TextBulletStartAtNum: 1..=32767; untrusted values are clamped
        // so counters can never overflow.
        let start = auto
            .attr(ns::A, "startAt")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|v| v.clamp(1, 32767) as u64)
            .unwrap_or(1);
        Bullet::AutoNum { marker, start, wrap }
    } else if ppr.find(ns::A, "buChar").is_some() {
        Bullet::Char
    } else {
        Bullet::Inherit
    };
    let delta = ppr.find(ns::A, "defRPr").map(rpr_delta).unwrap_or_default();
    TextProps { delta, bullet }
}

/// Delta from an `a:rPr`/`a:defRPr` element's attributes.
pub fn rpr_delta(rpr: &Element) -> StyleDelta {
    let on_off = |name: &str| rpr.attr(ns::A, name).map(|v| matches!(v, "1" | "true" | "on"));
    StyleDelta {
        bold: on_off("b"),
        italic: on_off("i"),
        strike: rpr.attr(ns::A, "strike").map(|v| matches!(v, "sngStrike" | "dblStrike")),
        code: None,
    }
}

/// A placeholder shape's identity and level styles inside a layout/master.
#[derive(Debug)]
pub struct Placeholder {
    pub ph_type: String,
    pub idx: Option<String>,
    pub styles: LevelStyle,
}

pub fn collect_placeholders(sp_tree: &Element) -> Vec<Placeholder> {
    let mut out = Vec::new();
    for sp in sp_tree.descendants(ns::P, "sp") {
        let Some(ph) = sp.first_descendant(ns::P, "ph") else {
            continue;
        };
        let styles =
            parse_level_styles(sp.find(ns::P, "txBody").and_then(|tx| tx.find(ns::A, "lstStyle")));
        out.push(Placeholder {
            ph_type: ph.attr(ns::P, "type").unwrap_or("body").to_string(),
            idx: ph.attr(ns::P, "idx").map(str::to_string),
            styles,
        });
    }
    out
}

/// Find the placeholder a shape inherits from: match by `idx` first, then by
/// type (title variants unify).
pub fn match_placeholder<'p>(
    placeholders: &'p [Placeholder],
    ph_type: &str,
    idx: Option<&str>,
) -> Option<&'p Placeholder> {
    if let Some(idx) = idx
        && let Some(hit) = placeholders.iter().find(|p| p.idx.as_deref() == Some(idx))
    {
        return Some(hit);
    }
    let class = title_class(ph_type);
    placeholders
        .iter()
        .find(|p| {
            title_class(&p.ph_type) == class && (class == TitleClass::Title || p.ph_type == ph_type)
        })
        .or_else(|| placeholders.iter().find(|p| title_class(&p.ph_type) == class))
}

#[derive(PartialEq, Clone, Copy)]
pub enum TitleClass {
    Title,
    Subtitle,
    Body,
}

pub fn title_class(ph_type: &str) -> TitleClass {
    match ph_type {
        "title" | "ctrTitle" => TitleClass::Title,
        "subTitle" => TitleClass::Subtitle,
        _ => TitleClass::Body,
    }
}
