use crate::model::{AnchorId, Inline, List, Table};

/// One block-level piece of a document body.
#[derive(Debug, Clone)]
pub enum Block {
    /// A section heading.
    Heading {
        /// Outline depth as the source assigns it, 1-based. Word outline
        /// levels reach past 6, so renderers clamp to what their target
        /// supports rather than the model doing it here.
        level: u8,
        /// Stable anchor id when the source document targets this heading
        /// (bookmark, chapter fragment, ...). Renderers map it to the
        /// heading's own anchor rather than emitting a separate one.
        anchor: Option<AnchorId>,
        /// The heading text.
        content: Vec<Inline>,
    },
    /// A run of body text.
    Paragraph(Vec<Inline>),
    /// A list, with its numbering already resolved.
    List(List),
    /// A table grid.
    Table(Table),
    /// Quoted content, which nests.
    BlockQuote(Vec<Block>),
    /// Preformatted text.
    CodeBlock {
        /// Language hint when the source names one.
        lang: Option<String>,
        /// The literal text, newlines intact.
        text: String,
    },
    /// A horizontal rule.
    Rule,
    /// A displayed formula, as LaTeX math without delimiters.
    Math(String),
}

impl Block {
    /// A heading with no anchor, for the common case where nothing in the
    /// document links to it.
    pub fn heading(level: u8, content: Vec<Inline>) -> Block {
        Block::Heading { level, anchor: None, content }
    }
}
