/// Fully resolved character style. Tri-state deltas exist only during
/// frontend resolution (`shared::delta`); by the time content reaches the
/// model every toggle has a definite value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    /// Bold weight.
    pub bold: bool,
    /// Italic or oblique.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
    /// Monospace, from a code or teletype character style.
    pub code: bool,
}

impl Style {
    /// No toggle set.
    pub const PLAIN: Style = Style { bold: false, italic: false, strike: false, code: false };
}
