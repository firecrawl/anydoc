//! Formula conversion to LaTeX, the form [`Inline::Math`](crate::model::Inline::Math)
//! and [`Block::Math`](crate::model::Block::Math) carry.

mod mathml;
mod omml;
mod tex;

pub use mathml::{mathml_is_display, mathml_to_tex};
pub use omml::{omath_para_to_tex, omath_to_tex};

use crate::model::Inline;

/// The equations of a paragraph that holds nothing else, for formats whose
/// math paragraphs arrive as inline content: such a paragraph is displayed
/// math, one block per equation.
pub fn math_lines(inlines: &[Inline]) -> Option<Vec<String>> {
    let lines: Vec<String> = inlines
        .iter()
        .filter_map(|i| match i {
            Inline::Math(tex) => Some(Some(tex.clone())),
            Inline::LineBreak => None,
            Inline::Text { text, .. } if text.trim().is_empty() => None,
            _ => Some(None),
        })
        .collect::<Option<Vec<String>>>()?;
    (!lines.is_empty()).then_some(lines)
}

#[cfg(test)]
mod tests;
