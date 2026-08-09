//! Tri-state style deltas used during cascade resolution. A property is
//! either explicitly on, explicitly off, or unset (inherit); only after the
//! full cascade is a delta collapsed into the model's resolved [`Style`].

use crate::model::{Inline, Style, VertAlign};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleDelta {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
    pub code: Option<bool>,
    pub vert_align: Option<VertAlign>,
}

impl StyleDelta {
    /// Overlay `child` on `self`: an explicit child value (on **or** off)
    /// wins; unset inherits.
    pub fn merge(self, child: StyleDelta) -> StyleDelta {
        StyleDelta {
            bold: child.bold.or(self.bold),
            italic: child.italic.or(self.italic),
            strike: child.strike.or(self.strike),
            code: child.code.or(self.code),
            vert_align: child.vert_align.or(self.vert_align),
        }
    }

    pub fn apply(self, base: Style) -> Style {
        Style {
            bold: self.bold.unwrap_or(base.bold),
            italic: self.italic.unwrap_or(base.italic),
            strike: self.strike.unwrap_or(base.strike),
            code: self.code.unwrap_or(base.code),
            vert_align: self.vert_align.unwrap_or(base.vert_align),
        }
    }

    pub fn resolve(self) -> Style {
        self.apply(Style::PLAIN)
    }
}

/// Drop from every run the emphasis `base` already carries. A heading style
/// defines its own typography, so its runs should carry only what they add
/// beyond it - otherwise the bold in `## **Heading**` is the style's, not the
/// author's.
pub fn rebase_emphasis(inlines: &mut [Inline], base: Style) {
    if base == Style::PLAIN {
        return;
    }
    for inline in inlines {
        match inline {
            Inline::Text { style, .. } => {
                style.bold &= !base.bold;
                style.italic &= !base.italic;
                style.strike &= !base.strike;
            }
            Inline::Link { content, .. } => rebase_emphasis(content, base),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_off_beats_inherited_on() {
        let base = StyleDelta { bold: Some(true), ..Default::default() };
        let child = StyleDelta { bold: Some(false), ..Default::default() };
        assert_eq!(base.merge(child).resolve(), Style::PLAIN);
    }

    #[test]
    fn unset_inherits() {
        let base = StyleDelta { bold: Some(true), italic: Some(true), ..Default::default() };
        let child = StyleDelta { italic: Some(false), ..Default::default() };
        let resolved = base.merge(child).resolve();
        assert!(resolved.bold && !resolved.italic);
    }
}
