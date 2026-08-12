//! Paragraph styles that name a block container. Word, Pandoc and
//! LibreOffice all mark quotations and preformatted text with a built-in
//! paragraph style name rather than dedicated markup, so the name is the
//! only signal a frontend has.

use crate::model::{Block, Inline, inlines_are_empty, inlines_to_plain_text};

/// The block container a paragraph style designates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockStyle {
    Quote,
    Code,
}

/// The container a paragraph style name designates. ODF encodes spaces in
/// internal style names as `_20_`.
pub fn from_style_name(name: &str) -> Option<BlockStyle> {
    let name = name.replace("_20_", " ");
    match name.trim().to_ascii_lowercase().as_str() {
        // Word, Pandoc, LibreOffice.
        "quote" | "intense quote" | "block text" | "quotations" => Some(BlockStyle::Quote),
        "html preformatted" | "source code" | "preformatted text" => Some(BlockStyle::Code),
        _ => None,
    }
}

/// Consecutive paragraphs sharing one styled container, folded into a single
/// block: producers write a multi-paragraph quote, and a code block one line
/// per paragraph, as a run of separately styled paragraphs.
#[derive(Default)]
pub enum StyledRun {
    #[default]
    Empty,
    Quote(Vec<Block>),
    Code(Vec<String>),
}

impl StyledRun {
    /// The container currently open.
    fn style(&self) -> Option<BlockStyle> {
        match self {
            StyledRun::Empty => None,
            StyledRun::Quote(_) => Some(BlockStyle::Quote),
            StyledRun::Code(_) => Some(BlockStyle::Code),
        }
    }

    /// Add one styled paragraph, closing the open container first when the
    /// style changes.
    pub fn push(&mut self, style: BlockStyle, inlines: Vec<Inline>, out: &mut Vec<Block>) {
        if self.style() != Some(style) {
            self.flush(out);
            *self = match style {
                BlockStyle::Quote => StyledRun::Quote(Vec::new()),
                BlockStyle::Code => StyledRun::Code(Vec::new()),
            };
        }
        match self {
            // Code is literal text; character styling is presentation the
            // source applied to its syntax, never content.
            StyledRun::Code(lines) => lines.push(inlines_to_plain_text(&inlines)),
            StyledRun::Quote(blocks) => {
                if !inlines_are_empty(&inlines) {
                    blocks.push(Block::Paragraph(inlines));
                }
            }
            StyledRun::Empty => {}
        }
    }

    /// Close the open container, if any.
    pub fn flush(&mut self, out: &mut Vec<Block>) {
        match std::mem::take(self) {
            StyledRun::Quote(inner) => {
                if !inner.is_empty() {
                    out.push(Block::BlockQuote(inner));
                }
            }
            StyledRun::Code(lines) => {
                // Blank lines count only between the code's own lines.
                let first = lines.iter().position(|l| !l.trim().is_empty());
                let last = lines.iter().rposition(|l| !l.trim().is_empty());
                if let (Some(first), Some(last)) = (first, last) {
                    out.push(Block::CodeBlock { lang: None, text: lines[first..=last].join("\n") });
                }
            }
            StyledRun::Empty => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_style_names_map_to_containers() {
        for name in ["Quote", "Intense Quote", "Block Text", "Quotations", "QUOTATIONS"] {
            assert_eq!(from_style_name(name), Some(BlockStyle::Quote), "{name}");
        }
        for name in ["HTML Preformatted", "Source Code", "Preformatted Text"] {
            assert_eq!(from_style_name(name), Some(BlockStyle::Code), "{name}");
        }
        // ODF internal names encode the spaces.
        assert_eq!(from_style_name("Preformatted_20_Text"), Some(BlockStyle::Code));
        for name in ["Normal", "Body Text", "heading 1", ""] {
            assert_eq!(from_style_name(name), None, "{name}");
        }
    }

    #[test]
    fn consecutive_paragraphs_fold_into_one_container() {
        let mut out = Vec::new();
        let mut run = StyledRun::default();
        run.push(BlockStyle::Code, vec![Inline::plain("fn main() {")], &mut out);
        run.push(BlockStyle::Code, vec![Inline::plain("}")], &mut out);
        run.push(BlockStyle::Quote, vec![Inline::plain("first")], &mut out);
        run.push(BlockStyle::Quote, vec![Inline::plain("second")], &mut out);
        run.flush(&mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        let Block::CodeBlock { text, .. } = &out[0] else { panic!("{out:?}") };
        assert_eq!(text, "fn main() {\n}");
        let Block::BlockQuote(inner) = &out[1] else { panic!("{out:?}") };
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn blank_paragraphs_around_a_code_run_are_dropped() {
        let mut out = Vec::new();
        let mut run = StyledRun::default();
        run.push(BlockStyle::Code, vec![Inline::plain("")], &mut out);
        run.push(BlockStyle::Code, vec![Inline::plain("a")], &mut out);
        run.push(BlockStyle::Code, vec![Inline::plain("")], &mut out);
        run.push(BlockStyle::Code, vec![Inline::plain("b")], &mut out);
        run.push(BlockStyle::Code, vec![Inline::plain("")], &mut out);
        run.flush(&mut out);
        let [Block::CodeBlock { text, .. }] = &out[..] else { panic!("{out:?}") };
        assert_eq!(text, "a\n\nb");
    }
}
