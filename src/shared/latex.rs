//! Shared LaTeX emission: escaping, and the Unicode-to-command tables the
//! math frontends resolve against.
//!
//! An unmapped glyph is passed through rather than guessed at. A guess emits a
//! different operator, well-formed and wrong, which is worse than a character
//! the reader can still see.

/// Largest LaTeX one equation may produce. Wrappers multiply through nesting
/// (`\left(\right)` is twelve characters per level), so a small document can
/// otherwise amplify without bound. A dense page of mathematics is under 4 KiB.
pub(crate) const MAX_LATEX_BYTES: usize = 64 * 1024;

/// LaTeX-escape literal text. A `$` would close the surrounding math span and
/// hand the rest of the document to the Markdown parser, so it is escaped
/// here rather than trusted; newlines cannot appear inside inline math.
pub(crate) fn push_text(text: &str, out: &mut String) {
    // A command runs until a non-letter, so `\int` + `f` would lex as `\intf`.
    if text.starts_with(|c: char| c.is_ascii_alphabetic()) && ends_with_control_word(out) {
        out.push(' ');
    }
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\backslash "),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '$' => out.push_str("\\$"),
            '&' => out.push_str("\\&"),
            '#' => out.push_str("\\#"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            '^' => out.push_str("\\^{}"),
            '~' => out.push_str("\\~{}"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
}

/// True when `out` ends in a LaTeX control word, whose name would absorb a
/// following letter.
pub(crate) fn ends_with_control_word(out: &str) -> bool {
    let trailing_letters =
        out.len() - out.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len();
    trailing_letters > 0
        && out[..out.len() - trailing_letters].ends_with('\\')
        && !out[..out.len() - trailing_letters].ends_with("\\\\")
}

/// Operator names KaTeX spells with a leading backslash.
pub(crate) const KATEX_FUNCTIONS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det", "dim",
    "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min",
    "sec", "sin", "sinh", "sup", "tan", "tanh",
];

pub(crate) fn nary_command(chr: char) -> String {
    match chr {
        '∑' => "\\sum",
        '∏' => "\\prod",
        '∐' => "\\coprod",
        '∫' => "\\int",
        '∬' => "\\iint",
        '∭' => "\\iiint",
        '∮' => "\\oint",
        '∯' => "\\oiint",
        '∰' => "\\oiiint",
        '⋀' => "\\bigwedge",
        '⋁' => "\\bigvee",
        '⋂' => "\\bigcap",
        '⋃' => "\\bigcup",
        '⨀' => "\\bigodot",
        '⨁' => "\\bigoplus",
        '⨂' => "\\bigotimes",
        '⨄' => "\\biguplus",
        '⨆' => "\\bigsqcup",
        // Passing the glyph through says what the document said; guessing a
        // command would emit a different operator, well-formed and wrong.
        _ => return escaped(chr),
    }
    .into()
}

/// OMML gives a combining codepoint; KaTeX wants the accent command. An
/// unmapped mark is stacked over the base rather than replaced by a guess.
pub(crate) fn accent_command(chr: char) -> Option<&'static str> {
    match chr {
        '\u{0300}' => "\\grave",
        '\u{0301}' => "\\acute",
        '\u{0302}' | '^' => "\\widehat",
        '\u{0303}' | '~' => "\\widetilde",
        '\u{0304}' => "\\bar",
        '\u{0305}' => "\\overline",
        '\u{0306}' => "\\breve",
        '\u{0307}' => "\\dot",
        '\u{0308}' => "\\ddot",
        '\u{030A}' => "\\mathring",
        '\u{030C}' => "\\check",
        '\u{0332}' => "\\underline",
        '\u{20D6}' => "\\overleftarrow",
        '\u{20D7}' | '→' => "\\vec",
        '\u{20DB}' => "\\dddot",
        '\u{20E1}' => "\\overleftrightarrow",
        _ => return None,
    }
    .into()
}

pub(crate) fn group_command(chr: char) -> Option<&'static str> {
    match chr {
        '\u{23DE}' | '\u{FE37}' => "\\overbrace",
        '\u{23B4}' => "\\overbracket",
        '\u{23B5}' => "\\underbracket",
        '\u{23DC}' => "\\overgroup",
        '\u{23DD}' => "\\undergroup",
        '←' => "\\overleftarrow",
        '→' => "\\overrightarrow",
        '\u{23DF}' | '\u{FE38}' => "\\underbrace",
        _ => return None,
    }
    .into()
}

/// A delimiter glyph in `\left`/`\right` position. An absent delimiter is a
/// bare `.`, which is how LaTeX spells "no glyph but keep the pair balanced".
pub(crate) fn delim_glyph(chr: Option<char>) -> String {
    match chr {
        None => ".".into(),
        Some('{') => "\\{".into(),
        Some('}') => "\\}".into(),
        Some('|') => "\\vert".into(),
        Some('‖') => "\\Vert".into(),
        Some('⌈') => "\\lceil".into(),
        Some('⌉') => "\\rceil".into(),
        Some('⌊') => "\\lfloor".into(),
        Some('⌋') => "\\rfloor".into(),
        Some('⟨') => "\\langle".into(),
        Some('⟩') => "\\rangle".into(),
        Some(c) => escaped(c),
    }
}

/// One author-supplied character, LaTeX-escaped. Delimiters and operators come
/// from the document and reach the body outside `push_text`.
pub(crate) fn escaped(chr: char) -> String {
    let mut out = String::new();
    push_text(&chr.to_string(), &mut out);
    out
}

/// A separator between delimiter parts. A bare `|` would split a Markdown
/// table row, so it is always spelled as a command.
pub(crate) fn delim_sep(chr: char) -> String {
    match chr {
        '|' => "\\mid ".into(),
        '‖' => "\\Vert ".into(),
        c => escaped(c),
    }
}
