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

/// Remove a character style when one non-empty heading run carries it across
/// the complete visible text. Mixed styling is left intact so an emphasized
/// word in an otherwise plain heading remains meaningful.
pub(crate) fn strip_uniform_style(inlines: &mut [Inline]) {
    if uniform_style(inlines).is_some_and(|style| style != Style::PLAIN) {
        strip_style(inlines);
    }
}

/// Return the one style shared by all visible text, or `None` when visible
/// content is mixed or includes content without a character style.
fn uniform_style(inlines: &[Inline]) -> Option<Style> {
    let mut style = None;
    if !collect_uniform_style(inlines, &mut style) {
        return None;
    }
    style
}

fn collect_uniform_style(inlines: &[Inline], style: &mut Option<Style>) -> bool {
    for inline in inlines {
        match inline {
            Inline::Text { text, style: text_style } => {
                if text.is_empty() {
                    continue;
                }
                match style {
                    Some(existing) if *existing != *text_style => return false,
                    Some(_) => {}
                    None => *style = Some(*text_style),
                }
            }
            Inline::Link { content, .. } => {
                if !collect_uniform_style(content, style) {
                    return false;
                }
            }
            Inline::Anchor(_) => {}
            Inline::Image { .. } | Inline::NoteRef(_) | Inline::LineBreak => return false,
        }
    }
    true
}

fn strip_style(inlines: &mut [Inline]) {
    for inline in inlines {
        match inline {
            Inline::Text { style, .. } => *style = Style::PLAIN,
            Inline::Link { content, .. } => strip_style(content),
            Inline::Image { .. } | Inline::Anchor(_) | Inline::NoteRef(_) | Inline::LineBreak => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str, style: Style) -> Inline {
        Inline::Text { text: text.into(), style }
    }

    #[test]
    fn fully_styled_content_is_stripped() {
        let mut inlines = vec![styled("All", Style { bold: true, ..Style::PLAIN })];
        strip_uniform_style(&mut inlines);
        assert!(matches!(inlines[0], Inline::Text { style: Style::PLAIN, .. }));
    }

    #[test]
    fn half_styled_content_is_preserved() {
        let mut inlines =
            vec![Inline::plain("Half "), styled("bold", Style { bold: true, ..Style::PLAIN })];
        strip_uniform_style(&mut inlines);
        assert!(matches!(inlines[0], Inline::Text { style: Style::PLAIN, .. }));
        assert!(matches!(inlines[1], Inline::Text { style: Style { bold: true, .. }, .. }));
    }

    #[test]
    fn uniform_combined_style_is_stripped() {
        let mut inlines = vec![styled("All", Style { bold: true, italic: true, ..Style::PLAIN })];
        strip_uniform_style(&mut inlines);
        assert!(matches!(inlines[0], Inline::Text { style: Style::PLAIN, .. }));
    }

    #[test]
    fn plain_content_is_unchanged() {
        let mut inlines = vec![Inline::plain("Plain")];
        strip_uniform_style(&mut inlines);
        assert!(matches!(inlines[0], Inline::Text { style: Style::PLAIN, .. }));
    }
}
