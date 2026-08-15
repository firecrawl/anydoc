use crate::model::AssetId;

/// Normalized, document-scoped anchor id. Frontends that concatenate multiple
/// source parts (EPUB chapters) scope ids during normalization so they stay
/// unique across the whole document.
pub type AnchorId = String;

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Absolute URL with a scheme.
    External(String),
    /// Scheme-less relative reference, preserved as written.
    Relative(String),
    /// Internal target: a heading anchor or an `Inline::Anchor` node.
    Anchor(AnchorId),
}

impl LinkTarget {
    /// True when the target string is empty, whichever kind it is.
    pub fn is_empty(&self) -> bool {
        match self {
            LinkTarget::External(s) | LinkTarget::Relative(s) | LinkTarget::Anchor(s) => {
                s.is_empty()
            }
        }
    }
}

/// Where an image's bytes live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// Absolute URL with a scheme.
    External(String),
    /// Embedded image stored in `Document::assets`.
    Asset(AssetId),
    /// No usable source: the image's part is missing or unreadable and it
    /// has no URL. Only the alt text remains.
    Unavailable,
}
