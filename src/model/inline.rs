use crate::model::{AnchorId, ImageSource, LinkTarget, Style};

/// One span of inline content.
#[derive(Debug, Clone)]
pub enum Inline {
    /// Styled text. Runs are split wherever the style changes.
    Text {
        /// The text itself.
        text: String,
        /// The character style covering all of it.
        style: Style,
    },
    /// A hyperlink wrapping its own inline content.
    Link {
        /// The link text, which may carry its own styling.
        content: Vec<Inline>,
        /// Where it points.
        target: LinkTarget,
    },
    /// An image.
    Image {
        /// Alt text, empty when the source gives none. Markdown cannot embed
        /// bytes, so this is what an embedded image renders as.
        alt: String,
        /// Where the bytes are, or that they are gone.
        source: ImageSource,
    },
    /// Zero-width anchor marking an internal link target at this position
    /// (bookmarks on paragraphs, spans, list items, table cells, ...).
    Anchor(AnchorId),
    /// A reference to the [`Note`](crate::model::Note) with this id.
    NoteRef(String),
    /// A line break inside a block, not a new block.
    LineBreak,
}

impl Inline {
    /// Unstyled text.
    pub fn plain(text: impl Into<String>) -> Self {
        Inline::Text { text: text.into(), style: Style::PLAIN }
    }
}

/// Flatten inlines to their text, dropping styling and links but keeping link
/// text and image alt text. Line breaks become newlines; anchors and note
/// references contribute nothing.
pub fn inlines_to_plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    collect_plain_text(inlines, &mut out);
    out
}

fn collect_plain_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::Link { content, .. } => collect_plain_text(content, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::Anchor(_) | Inline::NoteRef(_) => {}
            Inline::LineBreak => out.push('\n'),
        }
    }
}

/// True when nothing here would render as visible content: only whitespace,
/// empty-target links, anchors, and line breaks. An image or a note reference
/// always counts as content.
pub fn inlines_are_empty(inlines: &[Inline]) -> bool {
    inlines.iter().all(|i| match i {
        Inline::Text { text, .. } => text.trim().is_empty(),
        Inline::Link { content, target } => target.is_empty() && inlines_are_empty(content),
        Inline::Image { .. } | Inline::NoteRef(_) => false,
        Inline::Anchor(_) | Inline::LineBreak => true,
    })
}
