//! Office Math (OMML) to LaTeX.
//!
//! Parsing builds a tree before emitting: bracing depends on a node's shape --
//! `y^{2}` as another script's base needs braces, `\frac{a}{b}` does not -- and
//! the emitted string cannot answer that. Property elements are read through
//! named lookups only, never enumerated, so an absent bag and an empty one
//! resolve alike. Emitted commands stay inside the subset KaTeX implements.

use crate::model::Inline;
use crate::package::xml::{Element, ns};

/// Deepest OMML nesting translated. Real mathematics stays under a dozen
/// levels; past this the subtree degrades to its text, which keeps a hostile
/// document from driving recursion on a bounded stack.
const MAX_DEPTH: usize = 64;

/// Largest LaTeX one equation may produce. Wrappers multiply through nesting
/// (`\left(\right)` is twelve characters per level), so a small document can
/// otherwise amplify without bound. A dense page of mathematics is under 4 KiB.
const MAX_LATEX_BYTES: usize = 64 * 1024;

/// Most cells one matrix may contribute.
const MAX_MATRIX_CELLS: usize = 10_000;

/// The default n-ary operator when `m:naryPr/m:chr` is absent (ECMA-376).
const DEFAULT_NARY: char = '∫';
/// The default accent when `m:accPr/m:chr` is absent: combining circumflex.
const DEFAULT_ACCENT: char = '\u{0302}';
/// The default group character when `m:groupChrPr/m:chr` is absent.
const DEFAULT_GROUP: char = '\u{23DF}';

/// A parsed equation, shaped for LaTeX emission.
enum Node {
    /// Literal text from `m:t`, still unescaped.
    Run(String),
    Seq(Vec<Node>),
    Frac {
        num: Box<Node>,
        den: Box<Node>,
        kind: FracKind,
    },
    Rad {
        deg: Option<Box<Node>>,
        base: Box<Node>,
    },
    Script {
        base: Box<Node>,
        sub: Option<Box<Node>>,
        sup: Option<Box<Node>>,
        pre: bool,
    },
    Nary {
        op: char,
        sub: Option<Box<Node>>,
        sup: Option<Box<Node>>,
        base: Box<Node>,
        limits: bool,
    },
    Delim {
        open: Option<char>,
        close: Option<char>,
        sep: char,
        parts: Vec<Node>,
    },
    Accent {
        chr: char,
        base: Box<Node>,
    },
    Bar {
        top: bool,
        base: Box<Node>,
    },
    Group {
        chr: Option<char>,
        top: bool,
        base: Box<Node>,
    },
    Func {
        name: Box<Node>,
        arg: Box<Node>,
    },
    Limit {
        upper: bool,
        base: Box<Node>,
        lim: Box<Node>,
    },
    Matrix(Vec<Vec<Node>>),
    EqArr(Vec<Node>),
    Boxed {
        strike: Strike,
        base: Box<Node>,
    },
    Phantom {
        base: Box<Node>,
    },
}

enum FracKind {
    Bar,
    Skewed,
    NoBar,
    Linear,
}

enum Strike {
    None,
    Forward,
    Back,
    Cross,
}

/// Translate an `m:oMath` or `m:oMathPara` element.
///
/// Returns `None` only when the element carries nothing renderable, so a
/// caller can drop an empty equation rather than emit empty delimiters.
pub fn to_inline(elem: &Element, display: bool) -> Option<Inline> {
    let node = parse_seq(elem, 0);
    let mut latex = String::new();
    emit(&node, &mut latex, false);
    let latex = latex.trim().to_string();
    if latex.is_empty() {
        return None;
    }
    Some(Inline::Math { latex, display })
}

/// The `m:val` of a named property inside a named property bag, if every
/// level is present. Property bags are never enumerated: a `m:ctrlPr` holds
/// the formatting Word uses to draw its own glyphs and has no content of ours.
fn pr_val<'a>(parent: &'a Element, bag: &str, prop: &str) -> Option<&'a str> {
    parent.find(ns::M, bag)?.find(ns::M, prop)?.attr(ns::M, "val")
}

/// True when a property is present and explicitly on.
fn pr_on(parent: &Element, bag: &str, prop: &str) -> bool {
    matches!(pr_val(parent, bag, prop), Some("1" | "true" | "on"))
}

fn first_char(value: &str) -> Option<char> {
    value.chars().next()
}

/// Children of an argument container (`m:e`, `m:num`, `m:den`, ...) as one node.
fn parse_arg(parent: &Element, name: &str, depth: usize) -> Option<Node> {
    Some(parse_seq(parent.find(ns::M, name)?, depth))
}

fn boxed_arg(parent: &Element, name: &str, depth: usize) -> Box<Node> {
    Box::new(parse_arg(parent, name, depth).unwrap_or(Node::Seq(Vec::new())))
}

/// Every child of `elem`, flattened into one sequence.
fn parse_seq(elem: &Element, depth: usize) -> Node {
    if depth >= MAX_DEPTH {
        return Node::Run(elem.text());
    }
    let mut parts: Vec<Node> = Vec::new();
    for child in elem.child_elems() {
        if child.ns.as_deref() != Some(ns::M) {
            // Revision marks and bookmarks wrap runs that are part of the maths.
            if child.ns.as_deref() == Some(ns::W)
                && let Node::Seq(inner) = parse_seq(child, depth + 1)
            {
                parts.extend(inner);
            }
            continue;
        }
        if let Some(node) = parse_elem(child, depth) {
            parts.push(node);
        }
    }
    if parts.len() == 1 { parts.pop().unwrap() } else { Node::Seq(parts) }
}

fn parse_elem(elem: &Element, depth: usize) -> Option<Node> {
    let d = depth + 1;
    match elem.local.as_str() {
        name if name.ends_with("Pr") => None,
        "t" => {
            let text = elem.text();
            if text.is_empty() { None } else { Some(Node::Run(text)) }
        }
        "r" => Some(parse_seq(elem, d)),
        "oMath" | "oMathPara" | "box" | "e" | "num" | "den" | "lim" | "deg" | "fName" | "sub"
        | "sup" => Some(parse_seq(elem, d)),
        "f" => Some(Node::Frac {
            num: boxed_arg(elem, "num", d),
            den: boxed_arg(elem, "den", d),
            kind: match pr_val(elem, "fPr", "type") {
                Some("skw") => FracKind::Skewed,
                Some("noBar") => FracKind::NoBar,
                Some("lin") => FracKind::Linear,
                _ => FracKind::Bar,
            },
        }),
        "rad" => {
            // An empty `m:deg` is a square root; `m:degHide` says the same.
            let deg = elem
                .find(ns::M, "deg")
                .filter(|d| !d.text().trim().is_empty())
                .filter(|_| !pr_on(elem, "radPr", "degHide"))
                .map(|deg| Box::new(parse_seq(deg, d)));
            Some(Node::Rad { deg, base: boxed_arg(elem, "e", d) })
        }
        "sSup" => Some(Node::Script {
            base: boxed_arg(elem, "e", d),
            sub: None,
            sup: parse_arg(elem, "sup", d).map(Box::new),
            pre: false,
        }),
        "sSub" => Some(Node::Script {
            base: boxed_arg(elem, "e", d),
            sub: parse_arg(elem, "sub", d).map(Box::new),
            sup: None,
            pre: false,
        }),
        "sSubSup" => Some(Node::Script {
            base: boxed_arg(elem, "e", d),
            sub: parse_arg(elem, "sub", d).map(Box::new),
            sup: parse_arg(elem, "sup", d).map(Box::new),
            pre: false,
        }),
        "sPre" => Some(Node::Script {
            base: boxed_arg(elem, "e", d),
            sub: parse_arg(elem, "sub", d).map(Box::new),
            sup: parse_arg(elem, "sup", d).map(Box::new),
            pre: true,
        }),
        "nary" => Some(Node::Nary {
            op: pr_val(elem, "naryPr", "chr").and_then(first_char).unwrap_or(DEFAULT_NARY),
            sub: parse_arg(elem, "sub", d)
                .filter(|_| !pr_on(elem, "naryPr", "subHide"))
                .map(Box::new),
            sup: parse_arg(elem, "sup", d)
                .filter(|_| !pr_on(elem, "naryPr", "supHide"))
                .map(Box::new),
            base: boxed_arg(elem, "e", d),
            limits: pr_val(elem, "naryPr", "limLoc") == Some("undOvr"),
        }),
        "d" => {
            // An explicitly empty delimiter means "no glyph", which is not the
            // same as an absent property taking its default.
            let bag = elem.find(ns::M, "dPr");
            let chr = |name: &str, default: char| match bag.and_then(|b| b.find(ns::M, name)) {
                None => Some(default),
                Some(e) => e.attr(ns::M, "val").and_then(first_char),
            };
            Some(Node::Delim {
                open: chr("begChr", '('),
                close: chr("endChr", ')'),
                sep: pr_val(elem, "dPr", "sepChr").and_then(first_char).unwrap_or('|'),
                parts: elem.find_all(ns::M, "e").map(|e| parse_seq(e, d)).collect(),
            })
        }
        "func" => {
            Some(Node::Func { name: boxed_arg(elem, "fName", d), arg: boxed_arg(elem, "e", d) })
        }
        "limLow" => Some(Node::Limit {
            upper: false,
            base: boxed_arg(elem, "e", d),
            lim: boxed_arg(elem, "lim", d),
        }),
        "limUpp" => Some(Node::Limit {
            upper: true,
            base: boxed_arg(elem, "e", d),
            lim: boxed_arg(elem, "lim", d),
        }),
        "m" => {
            let mut rows = Vec::new();
            let mut cells = 0usize;
            for mr in elem.find_all(ns::M, "mr") {
                let row: Vec<Node> = mr.find_all(ns::M, "e").map(|e| parse_seq(e, d)).collect();
                cells += row.len();
                if cells > MAX_MATRIX_CELLS {
                    break;
                }
                rows.push(row);
            }
            Some(Node::Matrix(rows))
        }
        "eqArr" => Some(Node::EqArr(elem.find_all(ns::M, "e").map(|e| parse_seq(e, d)).collect())),
        "acc" => Some(Node::Accent {
            chr: pr_val(elem, "accPr", "chr").and_then(first_char).unwrap_or(DEFAULT_ACCENT),
            base: boxed_arg(elem, "e", d),
        }),
        "bar" => Some(Node::Bar {
            top: pr_val(elem, "barPr", "pos") == Some("top"),
            base: boxed_arg(elem, "e", d),
        }),
        "groupChr" => {
            let bag = elem.find(ns::M, "groupChrPr");
            let chr = match bag.and_then(|b| b.find(ns::M, "chr")) {
                None => Some(DEFAULT_GROUP),
                Some(e) => e.attr(ns::M, "val").and_then(first_char),
            };
            Some(Node::Group {
                chr,
                top: pr_val(elem, "groupChrPr", "pos") == Some("top"),
                base: boxed_arg(elem, "e", d),
            })
        }
        "borderBox" => Some(Node::Boxed {
            strike: match (
                pr_on(elem, "borderBoxPr", "strikeBLTR"),
                pr_on(elem, "borderBoxPr", "strikeTLBR"),
            ) {
                (true, true) => Strike::Cross,
                (true, false) => Strike::Forward,
                (false, true) => Strike::Back,
                (false, false) => Strike::None,
            },
            base: boxed_arg(elem, "e", d),
        }),
        "phant" => Some(Node::Phantom { base: boxed_arg(elem, "e", d) }),
        "br" => Some(Node::Run(" ".into())),
        // Unmodelled elements still hold content; descend rather than drop it.
        _ => Some(parse_seq(elem, d)),
    }
}

/// LaTeX-escape literal text. A `$` would close the surrounding math span and
/// hand the rest of the document to the Markdown parser, so it is escaped
/// here rather than trusted; newlines cannot appear inside inline math.
fn push_text(text: &str, out: &mut String) {
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
fn ends_with_control_word(out: &str) -> bool {
    let trailing_letters =
        out.len() - out.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len();
    trailing_letters > 0
        && out[..out.len() - trailing_letters].ends_with('\\')
        && !out[..out.len() - trailing_letters].ends_with("\\\\")
}

/// A script's base needs braces when it already ends in a script (`y^{2}^{3}`
/// is an error) or when it is a sequence the script would otherwise bind only
/// the last atom of. A fraction, radical or delimiter is a single atom already.
fn base_needs_braces(node: &Node) -> bool {
    match node {
        Node::Seq(parts) => parts.len() != 1,
        Node::Script { .. } | Node::Nary { .. } | Node::Limit { .. } => true,
        _ => false,
    }
}

/// Emit `node` as a braced group unless it is a single atom.
fn emit_group(node: &Node, out: &mut String) {
    out.push('{');
    emit(node, out, false);
    out.push('}');
}

fn emit(node: &Node, out: &mut String, _in_group: bool) {
    if out.len() >= MAX_LATEX_BYTES {
        return;
    }
    match node {
        Node::Run(text) => push_text(text, out),
        Node::Seq(parts) => {
            for part in parts {
                emit(part, out, false);
            }
        }
        Node::Frac { num, den, kind } => match kind {
            FracKind::Bar => {
                out.push_str("\\frac");
                emit_group(num, out);
                emit_group(den, out);
            }
            FracKind::NoBar => {
                out.push_str("\\binom");
                emit_group(num, out);
                emit_group(den, out);
            }
            FracKind::Skewed | FracKind::Linear => {
                emit_group(num, out);
                out.push('/');
                emit_group(den, out);
            }
        },
        Node::Rad { deg, base } => {
            out.push_str("\\sqrt");
            if let Some(deg) = deg {
                out.push('[');
                emit(deg, out, false);
                out.push(']');
            }
            emit_group(base, out);
        }
        Node::Script { base, sub, sup, pre } => {
            if *pre {
                // Scripts hang off an empty base; the real base follows.
                out.push_str("{}");
                if let Some(sub) = sub {
                    out.push('_');
                    emit_group(sub, out);
                }
                if let Some(sup) = sup {
                    out.push('^');
                    emit_group(sup, out);
                }
                emit(base, out, false);
                return;
            }
            if base_needs_braces(base) {
                emit_group(base, out);
            } else {
                emit(base, out, false);
            }
            if let Some(sub) = sub {
                out.push('_');
                emit_group(sub, out);
            }
            if let Some(sup) = sup {
                out.push('^');
                emit_group(sup, out);
            }
        }
        Node::Nary { op, sub, sup, base, limits } => {
            out.push_str(nary_command(*op));
            if *limits {
                out.push_str("\\limits");
            }
            if let Some(sub) = sub {
                out.push('_');
                emit_group(sub, out);
            }
            if let Some(sup) = sup {
                out.push('^');
                emit_group(sup, out);
            }
            emit(base, out, false);
        }
        Node::Delim { open, close, sep, parts } => {
            out.push_str("\\left");
            out.push_str(&delim_glyph(*open));
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    out.push_str(&delim_sep(*sep));
                }
                emit(part, out, false);
            }
            out.push_str("\\right");
            out.push_str(&delim_glyph(*close));
        }
        Node::Accent { chr, base } => {
            out.push_str(accent_command(*chr));
            emit_group(base, out);
        }
        Node::Bar { top, base } => {
            out.push_str(if *top { "\\overline" } else { "\\underline" });
            emit_group(base, out);
        }
        Node::Group { chr, top, base } => match chr.map(group_command) {
            Some(cmd) => {
                out.push_str(cmd);
                emit_group(base, out);
            }
            // An explicitly blank group character draws nothing.
            None => {
                let _ = top;
                emit(base, out, false);
            }
        },
        Node::Func { name, arg } => {
            let mut rendered = String::new();
            emit(name, &mut rendered, false);
            let trimmed = rendered.trim();
            if KATEX_FUNCTIONS.contains(&trimmed) {
                out.push('\\');
                out.push_str(trimmed);
            } else {
                out.push_str("\\operatorname");
                out.push('{');
                out.push_str(trimmed);
                out.push('}');
            }
            emit_group(arg, out);
        }
        Node::Limit { upper, base, lim } => {
            let mut rendered = String::new();
            emit(base, &mut rendered, false);
            let trimmed = rendered.trim();
            // An operator name takes a real script; anything else stacks.
            if KATEX_FUNCTIONS.contains(&trimmed) {
                out.push('\\');
                out.push_str(trimmed);
                out.push(if *upper { '^' } else { '_' });
                emit_group(lim, out);
                return;
            }
            out.push_str(if *upper { "\\overset" } else { "\\underset" });
            emit_group(lim, out);
            out.push('{');
            out.push_str(trimmed);
            out.push('}');
        }
        Node::Matrix(rows) => {
            out.push_str("\\begin{matrix}");
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\\\ ");
                }
                for (j, cell) in row.iter().enumerate() {
                    if j > 0 {
                        out.push_str(" & ");
                    }
                    emit(cell, out, false);
                }
            }
            out.push_str("\\end{matrix}");
        }
        Node::EqArr(rows) => {
            out.push_str("\\begin{aligned}");
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\\\ ");
                }
                emit(row, out, false);
            }
            out.push_str("\\end{aligned}");
        }
        Node::Boxed { strike, base } => {
            out.push_str(match strike {
                Strike::None => "\\boxed",
                Strike::Forward => "\\cancel",
                Strike::Back => "\\bcancel",
                Strike::Cross => "\\xcancel",
            });
            emit_group(base, out);
        }
        Node::Phantom { base } => {
            out.push_str("\\phantom");
            emit_group(base, out);
        }
    }
}

/// Operator names KaTeX spells with a leading backslash.
const KATEX_FUNCTIONS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det", "dim",
    "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min",
    "sec", "sin", "sinh", "sup", "tan", "tanh",
];

fn nary_command(chr: char) -> &'static str {
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
        _ => "\\int",
    }
}

/// OMML gives a combining codepoint; KaTeX wants the accent command.
fn accent_command(chr: char) -> &'static str {
    match chr {
        '\u{0300}' => "\\grave",
        '\u{0301}' => "\\acute",
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
        _ => "\\widehat",
    }
}

fn group_command(chr: char) -> &'static str {
    match chr {
        '\u{23DE}' | '\u{FE37}' => "\\overbrace",
        '\u{23B4}' => "\\overbracket",
        '\u{23B5}' => "\\underbracket",
        '\u{23DC}' => "\\overgroup",
        '\u{23DD}' => "\\undergroup",
        '←' => "\\overleftarrow",
        '→' => "\\overrightarrow",
        _ => "\\underbrace",
    }
}

/// A delimiter glyph in `\left`/`\right` position. An absent delimiter is a
/// bare `.`, which is how LaTeX spells "no glyph but keep the pair balanced".
fn delim_glyph(chr: Option<char>) -> String {
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
        Some(c) => c.to_string(),
    }
}

/// A separator between delimiter parts. A bare `|` would split a Markdown
/// table row, so it is always spelled as a command.
fn delim_sep(chr: char) -> String {
    match chr {
        '|' => "\\mid ".into(),
        '‖' => "\\Vert ".into(),
        c => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::xml::parse_xml;

    /// Convert one `m:oMath` written as a source fragment.
    fn latex(body: &str) -> String {
        let doc = format!(
            r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"
               xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{body}</m:oMath>"#
        );
        let root = parse_xml(doc.as_bytes()).unwrap();
        let math = root.find(ns::M, "oMath").unwrap();
        match to_inline(math, false) {
            Some(Inline::Math { latex, .. }) => latex,
            _ => String::new(),
        }
    }

    fn run(text: &str) -> String {
        format!("<m:r><m:t>{text}</m:t></m:r>")
    }

    #[test]
    fn absent_property_bags_take_their_spec_defaults() {
        assert_eq!(latex(&format!("<m:d><m:e>{}</m:e></m:d>", run("a"))), r"\left(a\right)");
        assert_eq!(latex(&format!("<m:acc><m:e>{}</m:e></m:acc>", run("y"))), r"\widehat{y}");
        assert_eq!(latex(&format!("<m:bar><m:e>{}</m:e></m:bar>", run("x"))), r"\underline{x}");
        assert_eq!(
            latex(&format!("<m:groupChr><m:e>{}</m:e></m:groupChr>", run("x"))),
            r"\underbrace{x}"
        );
        assert_eq!(
            latex(&format!("<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>", run("a"), run("b"))),
            r"\frac{a}{b}"
        );
    }

    #[test]
    fn absent_nary_operator_is_the_integral_not_the_sum() {
        let xml = format!("<m:nary><m:e>{}</m:e></m:nary>", run("f"));
        assert_eq!(latex(&xml), r"\int f");
    }

    #[test]
    fn an_explicitly_empty_delimiter_draws_no_glyph() {
        let xml = format!(
            r#"<m:d><m:dPr><m:begChr m:val=""/><m:endChr m:val=""/></m:dPr><m:e>{}</m:e></m:d>"#,
            run("v")
        );
        assert_eq!(latex(&xml), r"\left.v\right.");
    }

    #[test]
    fn a_script_base_that_is_itself_a_script_is_braced() {
        // `y^{2}^{3}` is a double superscript, which is an error.
        let inner = format!("<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>", run("y"), run("2"));
        let outer = format!("<m:sSup><m:e>{inner}</m:e><m:sup>{}</m:sup></m:sSup>", run("3"));
        assert_eq!(latex(&outer), "{y^{2}}^{3}");
    }

    #[test]
    fn a_fraction_base_needs_no_braces() {
        let frac = format!("<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>", run("a"), run("b"));
        let xml = format!("<m:sSup><m:e>{frac}</m:e><m:sup>{}</m:sup></m:sSup>", run("2"));
        assert_eq!(latex(&xml), r"\frac{a}{b}^{2}");
    }

    #[test]
    fn text_cannot_escape_the_math_span() {
        let out = latex(&run(r"a$b \href{x}{y} 50%"));
        assert!(!out.contains('$') || out.contains(r"\$"), "{out}");
        assert!(!out.contains(r"\href"), "{out}");
        assert_eq!(out, r"a\$b \backslash href\{x\}\{y\} 50\%");
    }

    #[test]
    fn a_delimiter_separator_never_emits_a_bare_pipe() {
        // A raw pipe would split the row the equation sits in.
        let xml = format!("<m:d><m:e>{}</m:e><m:e>{}</m:e></m:d>", run("x"), run("y"));
        assert!(!latex(&xml).contains('|'));
    }

    #[test]
    fn an_unmodelled_element_keeps_its_content() {
        let xml = format!("<m:futureThing><m:e>{}</m:e></m:futureThing>", run("keep"));
        assert_eq!(latex(&xml), "keep");
    }

    #[test]
    fn nesting_past_the_bound_degrades_to_text() {
        let mut xml = run("deep");
        for _ in 0..MAX_DEPTH + 8 {
            xml = format!("<m:d><m:e>{xml}</m:e></m:d>");
        }
        assert!(latex(&xml).contains("deep"));
    }

    #[test]
    fn an_empty_equation_produces_nothing() {
        assert_eq!(latex(""), "");
    }
}
