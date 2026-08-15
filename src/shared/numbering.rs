//! Shared list-number pattern IR.
//!
//! A level's number text is a sequence of literal pieces and level
//! references, rendered against the live counter values. Every frontend
//! resolves its native syntax onto this one form: WordprocessingML
//! `w:lvlText` (`%1.%2)`), the DOC `xst` placeholder string, RTF
//! `\leveltext`/`\levelnumbers`, and ODF prefix/suffix/display-levels.
//! Rendering returns `None` when the pattern reproduces the default label
//! the renderer builds from marker + value alone, so native Markdown list
//! numbering is used whenever it is faithful.

use crate::shared::list::MarkerKind;

/// One piece of a level's number text: literal characters, or the current
/// number of a (zero-based) level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberText {
    Literal(String),
    Level(u8),
}

/// A level's resolved number pattern.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NumberPattern {
    /// Empty = no pattern known; the marker kind's default label applies.
    pub text: Vec<NumberText>,
    /// Legal numbering (`w:isLgl`, LVLF `fLegal`): referenced levels render
    /// as decimal regardless of their own format.
    pub legal: bool,
}

/// Parse WordprocessingML-style percent patterns (`%1`–`%9` level
/// references, everything else literal) into pattern tokens.
pub fn parse_percent_pattern(text: &str) -> Vec<NumberText> {
    let mut out: Vec<NumberText> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(d) = chars.peek().and_then(|c| c.to_digit(10))
            && (1..=9).contains(&d)
        {
            chars.next();
            out.push(NumberText::Level((d - 1) as u8));
            continue;
        }
        match out.last_mut() {
            Some(NumberText::Literal(s)) => s.push(c),
            _ => out.push(NumberText::Literal(c.to_string())),
        }
    }
    out
}

/// Render a pattern against the current sequence values. `level_marker` and
/// `level_value` supply each referenced level's marker kind and current
/// ordinal (the level's start value when it has not begun counting).
/// `None` when the result matches the default label produced from the own
/// level's marker and value.
pub fn composite_label(
    pattern: &NumberPattern,
    own_marker: MarkerKind,
    own_value: u64,
    level_marker: impl Fn(usize) -> MarkerKind,
    level_value: impl Fn(usize) -> u64,
) -> Option<String> {
    if pattern.text.is_empty() {
        return None;
    }
    let mut out = String::new();
    for piece in &pattern.text {
        match piece {
            NumberText::Literal(s) => out.push_str(s),
            NumberText::Level(l) => {
                let l = *l as usize;
                let kind = if pattern.legal { MarkerKind::Decimal } else { level_marker(l) };
                out.push_str(&kind.ordinal(level_value(l)));
            }
        }
    }
    if out == own_marker.label(own_value) { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_patterns_tokenize() {
        assert_eq!(
            parse_percent_pattern("%1.%2)"),
            vec![
                NumberText::Level(0),
                NumberText::Literal(".".into()),
                NumberText::Level(1),
                NumberText::Literal(")".into()),
            ]
        );
        assert_eq!(
            parse_percent_pattern("Ch. %3:"),
            vec![
                NumberText::Literal("Ch. ".into()),
                NumberText::Level(2),
                NumberText::Literal(":".into()),
            ]
        );
        // `%0` and a trailing `%` are literal.
        assert_eq!(parse_percent_pattern("%0%"), vec![NumberText::Literal("%0%".into())]);
    }

    #[test]
    fn default_pattern_yields_no_label() {
        let pattern = NumberPattern { text: parse_percent_pattern("%1."), legal: false };
        let label =
            composite_label(&pattern, MarkerKind::Decimal, 3, |_| MarkerKind::Decimal, |_| 3);
        assert_eq!(label, None);
    }

    #[test]
    fn composite_pattern_renders_parent_values() {
        let pattern = NumberPattern { text: parse_percent_pattern("%1-%2)"), legal: false };
        let values = [2u64, 5];
        let label = composite_label(
            &pattern,
            MarkerKind::LowerAlpha,
            5,
            |l| if l == 0 { MarkerKind::Decimal } else { MarkerKind::LowerAlpha },
            |l| values[l.min(1)],
        );
        assert_eq!(label.as_deref(), Some("2-e)"));
    }

    #[test]
    fn legal_numbering_forces_decimal_references() {
        let pattern = NumberPattern { text: parse_percent_pattern("%1.%2"), legal: true };
        let label = composite_label(
            &pattern,
            MarkerKind::LowerRoman,
            4,
            |_| MarkerKind::LowerRoman,
            |l| if l == 0 { 2 } else { 4 },
        );
        assert_eq!(label.as_deref(), Some("2.4"));
    }
}
