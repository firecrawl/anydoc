//! LaTeX emission shared by the OMML and MathML converters: a buffer that
//! keeps control words separated from following letters, and the Unicode
//! to LaTeX mappings for symbols, delimiters, accents and function names.

/// Accumulates LaTeX source.
#[derive(Default)]
pub(crate) struct Tex {
    out: String,
    /// The output ends in a letter-named control word, which would swallow
    /// a letter appended directly after it.
    after_macro: bool,
}

impl Tex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    pub fn contains(&self, c: char) -> bool {
        self.out.contains(c)
    }

    pub fn push_str(&mut self, s: &str) {
        let Some(first) = s.chars().next() else { return };
        if self.after_macro && first.is_ascii_alphabetic() {
            self.out.push(' ');
        }
        self.out.push_str(s);
        self.after_macro = false;
    }

    pub fn push_char(&mut self, c: char) {
        if self.after_macro && c.is_ascii_alphabetic() {
            self.out.push(' ');
        }
        self.out.push(c);
        self.after_macro = false;
    }

    /// A control word, with its backslash (`\alpha`, `\{`). The backslash
    /// ends any control word before it, so no separator is needed.
    pub fn push_macro(&mut self, name: &str) {
        self.out.push_str(name);
        self.after_macro = name.ends_with(|c: char| c.is_ascii_alphabetic());
    }

    pub fn push_tex(&mut self, other: &Tex) {
        if other.out.is_empty() {
            return;
        }
        self.push_str(&other.out);
        self.after_macro = other.after_macro;
    }

    /// `{inner}`.
    pub fn push_group(&mut self, inner: &Tex) {
        self.push_char('{');
        self.push_tex(inner);
        self.push_char('}');
    }

    /// A script base: `inner` as it stands when it is one atom (a single
    /// character or control word), `{inner}` otherwise.
    pub fn push_base(&mut self, inner: &Tex) {
        let atom = inner.out.chars().count() == 1
            || inner.out.strip_prefix('\\').is_some_and(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic())
            });
        if atom { self.push_tex(inner) } else { self.push_group(inner) }
    }

    /// `\name{inner}`.
    pub fn push_command(&mut self, name: &str, inner: &Tex) {
        self.push_macro(name);
        self.push_group(inner);
    }

    /// Text in math mode: symbols become control words, styled
    /// mathematical alphanumerics fold back to their base letters inside
    /// the matching font command, and TeX specials are escaped.
    pub fn push_math_text(&mut self, text: &str) {
        let mut pending: Option<(Variant, String)> = None;
        for c in text.chars() {
            match fold_alnum(c) {
                Some((base, variant)) if variant != Variant::Plain => match &mut pending {
                    Some((v, run)) if *v == variant => run.push(base),
                    _ => {
                        self.flush_variant(pending.take());
                        pending = Some((variant, base.to_string()));
                    }
                },
                Some((base, _)) => {
                    self.flush_variant(pending.take());
                    self.push_math_char(base);
                }
                None => {
                    self.flush_variant(pending.take());
                    self.push_math_char(c);
                }
            }
        }
        self.flush_variant(pending.take());
    }

    fn flush_variant(&mut self, pending: Option<(Variant, String)>) {
        let Some((variant, run)) = pending else { return };
        let mut inner = Tex::new();
        inner.push_math_text(&run);
        self.push_command(variant.command(), &inner);
    }

    /// One character in math mode.
    pub fn push_math_char(&mut self, c: char) {
        if let Some(mapped) = symbol(c) {
            if mapped.starts_with('\\') {
                self.push_macro(mapped);
            } else {
                self.push_str(mapped);
            }
            return;
        }
        match c {
            '{' | '}' | '$' | '%' | '&' | '#' | '_' => {
                self.push_char('\\');
                self.push_char(c);
            }
            '\\' => self.push_macro("\\backslash"),
            '^' => self.push_str("\\char`^"),
            '~' => self.push_macro("\\sim"),
            '\u{a0}' => self.push_str("\\ "),
            c if c.is_control() => {}
            c => self.push_char(c),
        }
    }

    /// `\text{...}`: literal text inside a formula.
    pub fn push_text_mode(&mut self, text: &str) {
        let mut inner = Tex::new();
        for c in text.chars() {
            match c {
                '{' | '}' | '$' | '%' | '&' | '#' | '_' => {
                    inner.push_char('\\');
                    inner.push_char(c);
                }
                '\\' => inner.push_str("\\textbackslash{}"),
                '^' => inner.push_str("\\textasciicircum{}"),
                '~' => inner.push_str("\\textasciitilde{}"),
                c if c.is_control() => {}
                c => inner.push_char(c),
            }
        }
        self.push_command("\\text", &inner);
    }

    pub fn finish(self) -> String {
        let mut out = String::with_capacity(self.out.len());
        let mut prev_space = true;
        for c in self.out.trim().chars() {
            let space = c == ' ';
            if !(space && prev_space) {
                out.push(c);
            }
            prev_space = space;
        }
        out
    }
}

/// Font variant of a mathematical alphanumeric symbol.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    Plain,
    Bold,
    BoldItalic,
    Script,
    Fraktur,
    DoubleStruck,
    SansSerif,
    Monospace,
}

impl Variant {
    pub fn command(self) -> &'static str {
        match self {
            Variant::Plain => "\\mathit",
            Variant::Bold => "\\mathbf",
            Variant::BoldItalic => "\\boldsymbol",
            Variant::Script => "\\mathcal",
            Variant::Fraktur => "\\mathfrak",
            Variant::DoubleStruck => "\\mathbb",
            Variant::SansSerif => "\\mathsf",
            Variant::Monospace => "\\mathtt",
        }
    }

    /// The variant a MathML `mathvariant` names; `None` for the default
    /// and for values without a LaTeX font command.
    pub fn from_mathvariant(value: &str) -> Option<Variant> {
        Some(match value {
            "bold" => Variant::Bold,
            "bold-italic" => Variant::BoldItalic,
            "script" | "bold-script" => Variant::Script,
            "fraktur" | "bold-fraktur" => Variant::Fraktur,
            "double-struck" => Variant::DoubleStruck,
            "sans-serif" | "bold-sans-serif" | "sans-serif-italic" | "sans-serif-bold-italic" => {
                Variant::SansSerif
            }
            "monospace" => Variant::Monospace,
            _ => return None,
        })
    }
}

const GREEK_UPPER: &str = "ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡϴΣΤΥΦΧΨΩ";
const GREEK_LOWER: &str = "αβγδεζηθικλμνξοπρςστυφχψω";
const GREEK_EXTRA: &str = "∂ϵϑϰϕϱϖ";

/// Fold a Mathematical Alphanumeric Symbol (or a letterlike symbol in the
/// same role) to its base character and font variant. Italic is the
/// default math style, so it folds to `Plain`.
pub(crate) fn fold_alnum(c: char) -> Option<(char, Variant)> {
    use Variant::*;
    let cp = c as u32;
    let letterlike = match c {
        'ℎ' => ('h', Plain),
        'ℬ' => ('B', Script),
        'ℰ' => ('E', Script),
        'ℱ' => ('F', Script),
        'ℋ' => ('H', Script),
        'ℐ' => ('I', Script),
        'ℒ' => ('L', Script),
        'ℳ' => ('M', Script),
        'ℛ' => ('R', Script),
        'ℯ' => ('e', Script),
        'ℊ' => ('g', Script),
        'ℴ' => ('o', Script),
        'ℭ' => ('C', Fraktur),
        'ℌ' => ('H', Fraktur),
        'ℑ' => ('I', Fraktur),
        'ℜ' => ('R', Fraktur),
        'ℨ' => ('Z', Fraktur),
        'ℂ' => ('C', DoubleStruck),
        'ℍ' => ('H', DoubleStruck),
        'ℕ' => ('N', DoubleStruck),
        'ℙ' => ('P', DoubleStruck),
        'ℚ' => ('Q', DoubleStruck),
        'ℝ' => ('R', DoubleStruck),
        'ℤ' => ('Z', DoubleStruck),
        _ => {
            if !(0x1D400..0x1D800).contains(&cp) {
                return None;
            }
            let offset = cp - 0x1D400;
            if offset < 13 * 52 {
                const LATIN: [Variant; 13] = [
                    Bold,
                    Plain,
                    BoldItalic,
                    Script,
                    Script,
                    Fraktur,
                    DoubleStruck,
                    Fraktur,
                    SansSerif,
                    SansSerif,
                    SansSerif,
                    SansSerif,
                    Monospace,
                ];
                let (style, i) = ((offset / 52) as usize, offset % 52);
                let base = if i < 26 { b'A' + i as u8 } else { b'a' + (i - 26) as u8 };
                return Some((base as char, LATIN[style]));
            }
            let greek = cp.checked_sub(0x1D6A8)?;
            if greek < 5 * 58 {
                const GREEK: [Variant; 5] = [Bold, Plain, BoldItalic, SansSerif, SansSerif];
                let (style, i) = ((greek / 58) as usize, (greek % 58) as usize);
                let base = match i {
                    0..=24 => GREEK_UPPER.chars().nth(i)?,
                    25 => '∇',
                    26..=50 => GREEK_LOWER.chars().nth(i - 26)?,
                    _ => GREEK_EXTRA.chars().nth(i - 51)?,
                };
                return Some((base, GREEK[style]));
            }
            let digit = cp.checked_sub(0x1D7CE)?;
            if digit < 5 * 10 {
                const DIGITS: [Variant; 5] = [Bold, DoubleStruck, SansSerif, SansSerif, Monospace];
                let base = (b'0' + (digit % 10) as u8) as char;
                return Some((base, DIGITS[(digit / 10) as usize]));
            }
            return None;
        }
    };
    Some(letterlike)
}

/// LaTeX for a Unicode math character: a control word, a literal
/// replacement, or an empty string for characters with no visible form.
pub(crate) fn symbol(c: char) -> Option<&'static str> {
    Some(match c {
        // Greek
        'α' => "\\alpha",
        'β' => "\\beta",
        'γ' => "\\gamma",
        'δ' => "\\delta",
        'ε' => "\\varepsilon",
        'ϵ' => "\\epsilon",
        'ζ' => "\\zeta",
        'η' => "\\eta",
        'θ' => "\\theta",
        'ϑ' => "\\vartheta",
        'ι' => "\\iota",
        'κ' => "\\kappa",
        'ϰ' => "\\varkappa",
        'λ' => "\\lambda",
        'μ' => "\\mu",
        'ν' => "\\nu",
        'ξ' => "\\xi",
        'ο' => "o",
        'π' => "\\pi",
        'ϖ' => "\\varpi",
        'ρ' => "\\rho",
        'ϱ' => "\\varrho",
        'σ' => "\\sigma",
        'ς' => "\\varsigma",
        'τ' => "\\tau",
        'υ' => "\\upsilon",
        'φ' => "\\varphi",
        'ϕ' => "\\phi",
        'χ' => "\\chi",
        'ψ' => "\\psi",
        'ω' => "\\omega",
        'Α' => "A",
        'Β' => "B",
        'Γ' => "\\Gamma",
        'Δ' => "\\Delta",
        'Ε' => "E",
        'Ζ' => "Z",
        'Η' => "H",
        'Θ' | 'ϴ' => "\\Theta",
        'Ι' => "I",
        'Κ' => "K",
        'Λ' => "\\Lambda",
        'Μ' => "M",
        'Ν' => "N",
        'Ξ' => "\\Xi",
        'Ο' => "O",
        'Π' => "\\Pi",
        'Ρ' => "P",
        'Σ' => "\\Sigma",
        'Τ' => "T",
        'Υ' => "\\Upsilon",
        'Φ' => "\\Phi",
        'Χ' => "X",
        'Ψ' => "\\Psi",
        'Ω' => "\\Omega",
        // Big operators
        '∑' => "\\sum",
        '∏' => "\\prod",
        '∐' => "\\coprod",
        '∫' => "\\int",
        '∬' => "\\iint",
        '∭' => "\\iiint",
        '⨌' => "\\iiiint",
        '∮' => "\\oint",
        '∯' => "\\oiint",
        '∰' => "\\oiiint",
        '⋃' => "\\bigcup",
        '⋂' => "\\bigcap",
        '⋁' => "\\bigvee",
        '⋀' => "\\bigwedge",
        '⨁' => "\\bigoplus",
        '⨂' => "\\bigotimes",
        '⨀' => "\\bigodot",
        '⨄' => "\\biguplus",
        '⨆' => "\\bigsqcup",
        // Binary operators
        '×' => "\\times",
        '÷' => "\\div",
        '±' => "\\pm",
        '∓' => "\\mp",
        '⋅' | '·' | '∙' => "\\cdot",
        '∗' => "\\ast",
        '∘' => "\\circ",
        '∖' => "\\setminus",
        '⊕' => "\\oplus",
        '⊖' => "\\ominus",
        '⊗' => "\\otimes",
        '⊘' => "\\oslash",
        '⊙' => "\\odot",
        '∪' => "\\cup",
        '∩' => "\\cap",
        '⊎' => "\\uplus",
        '⊓' => "\\sqcap",
        '⊔' => "\\sqcup",
        '∧' => "\\wedge",
        '∨' => "\\vee",
        '†' => "\\dagger",
        '‡' => "\\ddagger",
        '⋆' => "\\star",
        // Relations
        '≤' | '⩽' => "\\le",
        '≥' | '⩾' => "\\ge",
        '≠' => "\\ne",
        '≈' => "\\approx",
        '≡' => "\\equiv",
        '≢' => "\\not\\equiv",
        '≅' => "\\cong",
        '≃' => "\\simeq",
        '∼' => "\\sim",
        '≁' => "\\nsim",
        '∝' => "\\propto",
        '≪' => "\\ll",
        '≫' => "\\gg",
        '≺' => "\\prec",
        '≻' => "\\succ",
        '⪯' | '≼' => "\\preceq",
        '⪰' | '≽' => "\\succeq",
        '⊂' => "\\subset",
        '⊃' => "\\supset",
        '⊆' => "\\subseteq",
        '⊇' => "\\supseteq",
        '⊄' => "\\not\\subset",
        '⊈' => "\\nsubseteq",
        '⊊' => "\\subsetneq",
        '⊋' => "\\supsetneq",
        '⊏' => "\\sqsubset",
        '⊐' => "\\sqsupset",
        '⊑' => "\\sqsubseteq",
        '⊒' => "\\sqsupseteq",
        '∈' => "\\in",
        '∉' => "\\notin",
        '∋' => "\\ni",
        '∌' => "\\not\\ni",
        '≐' => "\\doteq",
        '≜' => "\\triangleq",
        '≝' => "\\overset{\\mathrm{def}}{=}",
        '≔' => "\\coloneqq",
        '⊥' => "\\perp",
        '∥' => "\\parallel",
        '∦' => "\\nparallel",
        '⊢' => "\\vdash",
        '⊣' => "\\dashv",
        '⊨' => "\\models",
        '⊤' => "\\top",
        '≍' => "\\asymp",
        '≀' => "\\wr",
        '⋈' => "\\bowtie",
        '∣' => "\\mid",
        '∤' => "\\nmid",
        // Arrows
        '→' => "\\to",
        '←' => "\\leftarrow",
        '↔' => "\\leftrightarrow",
        '⇒' => "\\Rightarrow",
        '⇐' => "\\Leftarrow",
        '⇔' => "\\Leftrightarrow",
        '↑' => "\\uparrow",
        '↓' => "\\downarrow",
        '↕' => "\\updownarrow",
        '⇑' => "\\Uparrow",
        '⇓' => "\\Downarrow",
        '⇕' => "\\Updownarrow",
        '↦' => "\\mapsto",
        '⟶' => "\\longrightarrow",
        '⟵' => "\\longleftarrow",
        '⟷' => "\\longleftrightarrow",
        '⟹' => "\\Longrightarrow",
        '⟸' => "\\Longleftarrow",
        '⟺' => "\\Longleftrightarrow",
        '⟼' => "\\longmapsto",
        '↗' => "\\nearrow",
        '↘' => "\\searrow",
        '↙' => "\\swarrow",
        '↖' => "\\nwarrow",
        '↩' => "\\hookleftarrow",
        '↪' => "\\hookrightarrow",
        '⇀' => "\\rightharpoonup",
        '↼' => "\\leftharpoonup",
        '⇌' => "\\rightleftharpoons",
        '↶' => "\\curvearrowleft",
        '↷' => "\\curvearrowright",
        // Logic and sets
        '∀' => "\\forall",
        '∃' => "\\exists",
        '∄' => "\\nexists",
        '¬' => "\\neg",
        '∅' | '⌀' => "\\emptyset",
        '∞' => "\\infty",
        '∂' => "\\partial",
        '∇' => "\\nabla",
        '∴' => "\\therefore",
        '∵' => "\\because",
        'ℵ' => "\\aleph",
        'ℶ' => "\\beth",
        'ℏ' => "\\hbar",
        'ℓ' => "\\ell",
        '℘' => "\\wp",
        '℧' => "\\mho",
        '√' => "\\surd",
        '∠' => "\\angle",
        '∡' => "\\measuredangle",
        '△' => "\\triangle",
        '□' | '◻' => "\\square",
        '◊' => "\\lozenge",
        '°' => "^{\\circ}",
        '′' => "'",
        '″' => "''",
        '‴' => "'''",
        '⁗' => "''''",
        '…' => "\\ldots",
        '⋯' => "\\cdots",
        '⋮' => "\\vdots",
        '⋱' => "\\ddots",
        '⋰' => "\\iddots",
        // Delimiters
        '⟨' | '〈' => "\\langle",
        '⟩' | '〉' => "\\rangle",
        '⌈' => "\\lceil",
        '⌉' => "\\rceil",
        '⌊' => "\\lfloor",
        '⌋' => "\\rfloor",
        '‖' => "\\|",
        // Spacing and invisible characters
        '−' | '‐' | '‑' | '‒' | '–' => "-",
        '\u{2009}' | '\u{200a}' | '\u{2006}' => "\\,",
        '\u{2005}' | '\u{2004}' => "\\:",
        '\u{2003}' | '\u{2002}' => "\\;",
        '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{2064}' | '\u{200b}' | '\u{feff}' => "",
        _ => return None,
    })
}

/// LaTeX for a character in a `\left` / `\right` position; characters
/// that cannot stretch are written as themselves.
pub(crate) fn delimiter(c: char) -> &'static str {
    match c {
        '(' | '⟮' => "(",
        ')' | '⟯' => ")",
        '[' => "[",
        ']' => "]",
        '{' => "\\{",
        '}' => "\\}",
        '|' | '∣' => "|",
        '‖' | '∥' => "\\|",
        '⟨' | '〈' => "\\langle",
        '⟩' | '〉' => "\\rangle",
        '⌈' => "\\lceil",
        '⌉' => "\\rceil",
        '⌊' => "\\lfloor",
        '⌋' => "\\rfloor",
        '/' => "/",
        '\\' => "\\backslash",
        '↑' => "\\uparrow",
        '↓' => "\\downarrow",
        '↕' => "\\updownarrow",
        '⇑' => "\\Uparrow",
        '⇓' => "\\Downarrow",
        '⇕' => "\\Updownarrow",
        _ => ".",
    }
}

/// Accent command for a combining mark or its spacing equivalent, as used
/// by OMML `acc` and MathML `mover`/`munder`.
pub(crate) fn accent(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{300}' | '`' => "\\grave",
        '\u{301}' | '´' => "\\acute",
        '\u{302}' | '^' | 'ˆ' => "\\hat",
        '\u{303}' | '~' | '˜' => "\\tilde",
        '\u{304}' | '¯' | 'ˉ' => "\\bar",
        '\u{305}' | '‾' | '⎴' => "\\overline",
        '\u{306}' | '˘' => "\\breve",
        '\u{307}' | '˙' => "\\dot",
        '\u{308}' | '¨' => "\\ddot",
        '\u{30a}' | '˚' => "\\mathring",
        '\u{30c}' | 'ˇ' => "\\check",
        '\u{332}' | '_' | '⎵' => "\\underline",
        '\u{20d6}' | '←' => "\\overleftarrow",
        '\u{20d7}' | '→' => "\\vec",
        '\u{20e1}' | '↔' => "\\overleftrightarrow",
        '\u{20db}' => "\\dddot",
        '\u{20dc}' => "\\ddddot",
        '\u{20ee}' => "\\underleftarrow",
        '\u{20ef}' => "\\underrightarrow",
        '⏞' | '︷' => "\\overbrace",
        '⏟' | '︸' => "\\underbrace",
        _ => return None,
    })
}

/// Control word for a function name LaTeX predefines (`\sin`), or an
/// `\operatorname` for any other alphabetic name.
pub(crate) fn function_name(name: &str) -> Option<String> {
    const KNOWN: &[&str] = &[
        "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det",
        "dim", "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log",
        "max", "min", "Pr", "sec", "sin", "sinh", "sup", "tan", "tanh",
    ];
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(if KNOWN.contains(&name) {
        format!("\\{name}")
    } else {
        format!("\\operatorname{{{name}}}")
    })
}
