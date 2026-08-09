/// Vertical position of a run relative to the baseline.
///
/// Unlike the emphasis fields this is a value, not a toggle: OOXML models it
/// as `ST_VerticalAlignRun` and ODF as a percentage-plus-size pair, and in
/// both the nearest specification along the style chain wins outright rather
/// than flipping an inherited state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VertAlign {
    /// On the baseline.
    #[default]
    Baseline,
    /// Raised, as in `10⁻³` or an ordinal suffix.
    Superscript,
    /// Lowered, as in the 2 of `H₂O`.
    Subscript,
}

impl VertAlign {
    /// The OOXML spelling, and the value the language bindings publish.
    pub fn as_str(self) -> &'static str {
        match self {
            VertAlign::Baseline => "baseline",
            VertAlign::Superscript => "superscript",
            VertAlign::Subscript => "subscript",
        }
    }
}

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
    /// Position relative to the baseline.
    pub vert_align: VertAlign,
}

impl Style {
    /// No toggle set, on the baseline.
    pub const PLAIN: Style = Style {
        bold: false,
        italic: false,
        strike: false,
        code: false,
        vert_align: VertAlign::Baseline,
    };
}
