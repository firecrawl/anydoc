//! MathML presentation markup to LaTeX.
//!
//! Only part of the MathML in the wild carries a TeX annotation. Without one
//! the markup used to reach the writer as its bare characters, which keeps the
//! glyphs and drops every relation the layout expressed: a `<mfrac>` over 1 and
//! 3 read as `13`.
//!
//! Structure is translated; glyphs are not. KaTeX accepts Unicode operators and
//! Greek directly and has metrics for them, so `α` and `∑` are left as the
//! document wrote them and only the invisible operators, which would otherwise
//! reach the reader as nothing at all, are dropped.

use crate::model::Inline;
use crate::package::xml::Element;
use crate::shared::latex::{
    KATEX_FUNCTIONS, MAX_LATEX_BYTES, accent_command, delim_glyph, delim_sep, push_text,
};

const MAX_DEPTH: usize = 64;
const MAX_TABLE_CELLS: usize = 10_000;

/// Operators that take their scripts as limits rather than as corner scripts.
const BIG_OPERATORS: &str = "∑∏∐∫∬∭∮∯∰⋀⋁⋂⋃⨀⨁⨂⨄⨆";

pub(crate) fn to_inline(math: &Element, display: bool) -> Option<Inline> {
    if let Some(latex) = tex_annotation(math) {
        return Some(Inline::Math { latex, display });
    }
    let mut out = String::new();
    emit_children(math, &mut out, 0);
    let latex = out.trim().to_string();
    if latex.is_empty() {
        return None;
    }
    Some(Inline::Math { latex, display })
}

/// The LaTeX a `<semantics>` may carry alongside the presentation tree. It is
/// what the author wrote, so it wins over anything derived from the layout.
fn tex_annotation(math: &Element) -> Option<String> {
    let text = math
        .descendant_elems()
        .filter(|e| e.local == "annotation")
        .find(|e| {
            matches!(e.attr_any("encoding"), Some("application/x-tex" | "application/x-latex"))
        })?
        .text();
    let text = text.trim();
    if text.is_empty() { None } else { Some(text.replace(['\n', '\r'], " ")) }
}

fn emit_children(parent: &Element, out: &mut String, depth: usize) {
    for child in parent.child_elems() {
        emit(child, out, depth);
    }
}

fn arg(parent: &Element, index: usize) -> Option<&Element> {
    parent.child_elems().nth(index)
}

/// Emit `elem` as a braced group. An absent argument still needs its braces:
/// `\frac{a}` is a syntax error where `\frac{a}{}` is not.
fn group(elem: Option<&Element>, out: &mut String, depth: usize) {
    out.push('{');
    if let Some(elem) = elem {
        emit(elem, out, depth);
    }
    out.push('}');
}

/// A base needs no braces when it is one character or one control word;
/// anything longer would let the script bind to its last atom only. Braces
/// around a large operator would also cost it its limits, so the two cases
/// that may go bare are exactly the two that are already single atoms.
fn emit_base(elem: Option<&Element>, out: &mut String, depth: usize) {
    let mut base = String::new();
    if let Some(elem) = elem {
        emit(elem, &mut base, depth);
    }
    let bare = base.chars().count() == 1
        || (base.starts_with('\\') && base[1..].chars().all(|c| c.is_ascii_alphabetic()));
    if bare {
        out.push_str(&base);
    } else {
        out.push('{');
        out.push_str(&base);
        out.push('}');
    }
}

fn emit(elem: &Element, out: &mut String, depth: usize) {
    if out.len() >= MAX_LATEX_BYTES {
        return;
    }
    if depth > MAX_DEPTH {
        log::debug!("MathML nested past {MAX_DEPTH} levels; the subtree degrades to its text");
        push_text(elem.text().trim(), out);
        return;
    }
    let depth = depth + 1;
    match elem.local.as_str() {
        "mi" => identifier(elem, out),
        "mn" => push_text(elem.text().trim(), out),
        "mo" => operator(elem, out),
        "mtext" => text_run(&collapsed(&elem.text()), out),
        "ms" => quoted(elem, out),
        "mspace" => space(elem, out),
        "mglyph" => push_text(elem.attr_any("alt").unwrap_or_default(), out),
        "mphantom" => {
            out.push_str("\\phantom");
            out.push('{');
            emit_children(elem, out, depth);
            out.push('}');
        }
        "msqrt" => {
            out.push_str("\\sqrt{");
            emit_children(elem, out, depth);
            out.push('}');
        }
        "mroot" => {
            out.push_str("\\sqrt[");
            if let Some(index) = arg(elem, 1) {
                emit(index, out, depth);
            }
            out.push_str("]{");
            if let Some(base) = arg(elem, 0) {
                emit(base, out, depth);
            }
            out.push('}');
        }
        "mfrac" => fraction(elem, out, depth),
        "msub" => {
            emit_base(arg(elem, 0), out, depth);
            out.push('_');
            group(arg(elem, 1), out, depth);
        }
        "msup" => {
            emit_base(arg(elem, 0), out, depth);
            out.push('^');
            group(arg(elem, 1), out, depth);
        }
        "msubsup" => {
            emit_base(arg(elem, 0), out, depth);
            out.push('_');
            group(arg(elem, 1), out, depth);
            out.push('^');
            group(arg(elem, 2), out, depth);
        }
        "munder" => under_over(elem, out, depth, true, false),
        "mover" => under_over(elem, out, depth, false, true),
        "munderover" => under_over(elem, out, depth, true, true),
        "mmultiscripts" => multiscripts(elem, out, depth),
        "mtable" => table(elem, out, depth),
        "mfenced" => fenced(elem, out, depth),
        "semantics" => {
            let presentation = elem
                .child_elems()
                .find(|c| !matches!(c.local.as_str(), "annotation" | "annotation-xml"));
            if let Some(presentation) = presentation {
                emit(presentation, out, depth);
            }
        }
        "maction" => {
            if let Some(shown) = arg(elem, 0) {
                emit(shown, out, depth);
            }
        }
        "annotation" | "annotation-xml" | "none" | "mprescripts" => {}
        _ => emit_children(elem, out, depth),
    }
}

/// A single character is a variable and italic already; anything longer is a
/// name, which MathML sets upright and LaTeX would otherwise set as a product
/// of its letters.
fn identifier(elem: &Element, out: &mut String) {
    let text = elem.text();
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if KATEX_FUNCTIONS.contains(&text) {
        out.push('\\');
        out.push_str(text);
        return;
    }
    if text.chars().count() == 1 && elem.attr_any("mathvariant") != Some("normal") {
        push_text(text, out);
        return;
    }
    out.push_str("\\mathrm{");
    push_text(text, out);
    out.push('}');
}

/// The invisible operators carry grouping that the surrounding markup already
/// states, and no glyph. Passing them through would put unreadable codepoints
/// in front of a reader.
fn operator(elem: &Element, out: &mut String) {
    let text = elem.text();
    let text = text.trim();
    if text.is_empty() || text.chars().all(|c| ('\u{2061}'..='\u{2064}').contains(&c)) {
        return;
    }
    for chr in text.chars() {
        out.push_str(&delim_sep(chr));
    }
}

fn text_run(text: &str, out: &mut String) {
    if text.is_empty() {
        return;
    }
    out.push_str("\\text{");
    push_text(text, out);
    out.push('}');
}

fn quoted(elem: &Element, out: &mut String) {
    let open = elem.attr_any("lquote").unwrap_or("\"");
    let close = elem.attr_any("rquote").unwrap_or("\"");
    text_run(&format!("{open}{}{close}", collapsed(&elem.text())), out);
}

/// Symbols are trimmed, prose is not: a space between `<mtext>if </mtext>` and
/// what follows is a word boundary, and dropping it reads as one word.
fn collapsed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for chr in text.chars() {
        if chr.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space {
            out.push(' ');
        }
        in_space = false;
        out.push(chr);
    }
    if in_space && !out.is_empty() {
        out.push(' ');
    }
    out
}

/// Only a CSS length can be handed to `\hspace`; the named widths
/// (`thickmathspace`) have no LaTeX spelling and are dropped rather than
/// guessed at.
fn space(elem: &Element, out: &mut String) {
    let Some(width) = elem.attr_any("width") else {
        return;
    };
    let unit_at = width.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(0);
    let (amount, unit) = width.split_at(unit_at);
    if amount.parse::<f32>().is_ok()
        && matches!(unit, "em" | "ex" | "px" | "pt" | "cm" | "mm" | "in" | "pc")
    {
        out.push_str("\\hspace{");
        out.push_str(width);
        out.push('}');
    }
}

fn fraction(elem: &Element, out: &mut String, depth: usize) {
    // A zero rule is a choose-style stack, which `\frac` would draw a bar under.
    if elem.attr_any("linethickness").is_some_and(is_zero_length) {
        out.push('{');
        group(arg(elem, 0), out, depth);
        out.push_str("\\atop ");
        group(arg(elem, 1), out, depth);
        out.push('}');
        return;
    }
    out.push_str("\\frac");
    group(arg(elem, 0), out, depth);
    group(arg(elem, 1), out, depth);
}

fn is_zero_length(value: &str) -> bool {
    let digits = value.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    digits.trim().parse::<f32>() == Ok(0.0)
}

/// A script under or over a large operator is a limit and belongs in `_`/`^`;
/// over anything else it is either an accent or a genuine overset.
fn under_over(elem: &Element, out: &mut String, depth: usize, under: bool, over: bool) {
    let base = arg(elem, 0);
    let first = arg(elem, 1);
    let second = if under && over { arg(elem, 2) } else { None };

    if let (false, Some(mark)) = (under, first.and_then(single_char))
        && let Some(command) = accent_command(mark)
    {
        out.push_str(command);
        group(base, out, depth);
        return;
    }

    if base.is_some_and(takes_limits) {
        emit_base(base, out, depth);
        if under {
            out.push('_');
            group(first, out, depth);
        }
        if over {
            out.push('^');
            group(if under { second } else { first }, out, depth);
        }
        return;
    }

    // `\overset` and `\underset` take the script first, so carrying both means
    // nesting: the outer command's base is the whole inner one.
    let mut stacked = String::new();
    if over {
        stacked.push_str("\\overset");
        group(if under { second } else { first }, &mut stacked, depth);
    }
    if under {
        let inner = std::mem::take(&mut stacked);
        stacked.push_str("\\underset");
        group(first, &mut stacked, depth);
        stacked.push('{');
        stacked.push_str(&inner);
        group(base, &mut stacked, depth);
        stacked.push('}');
        out.push_str(&stacked);
        return;
    }
    out.push_str(&stacked);
    group(base, out, depth);
}

fn single_char(elem: &Element) -> Option<char> {
    let text = elem.text();
    let mut chars = text.trim().chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

fn takes_limits(elem: &Element) -> bool {
    let text = elem.text();
    let text = text.trim();
    text.chars().all(|c| BIG_OPERATORS.contains(c)) && !text.is_empty()
        || matches!(text, "lim" | "max" | "min" | "sup" | "inf" | "limsup" | "liminf")
}

/// `mmultiscripts` lists post-scripts first, then `mprescripts` and the
/// pre-scripts, each as a sub/sup pair. An empty slot is spelled `<none/>`.
fn multiscripts(elem: &Element, out: &mut String, depth: usize) {
    let children: Vec<&Element> = elem.child_elems().collect();
    let Some((base, scripts)) = children.split_first() else {
        return;
    };
    let split = scripts.iter().position(|c| c.local == "mprescripts");
    let (post, pre) = match split {
        Some(at) => (&scripts[..at], &scripts[at + 1..]),
        None => (scripts, &[][..]),
    };

    for pair in pre.chunks(2) {
        out.push_str("{}");
        emit_script('_', pair.first().copied(), out, depth);
        emit_script('^', pair.get(1).copied(), out, depth);
    }
    emit_base(Some(base), out, depth);
    for pair in post.chunks(2) {
        emit_script('_', pair.first().copied(), out, depth);
        emit_script('^', pair.get(1).copied(), out, depth);
    }
}

fn emit_script(marker: char, elem: Option<&Element>, out: &mut String, depth: usize) {
    let Some(elem) = elem else {
        return;
    };
    if elem.local == "none" {
        return;
    }
    out.push(marker);
    group(Some(elem), out, depth);
}

fn table(elem: &Element, out: &mut String, depth: usize) {
    out.push_str("\\begin{matrix}");
    let mut cells = 0usize;
    for (row_index, row) in elem.child_elems().filter(|c| c.local == "mtr").enumerate() {
        if row_index > 0 {
            out.push_str("\\\\");
        }
        for (cell_index, cell) in row.child_elems().filter(|c| c.local == "mtd").enumerate() {
            if cells >= MAX_TABLE_CELLS {
                log::warn!("MathML table past {MAX_TABLE_CELLS} cells; the rest is dropped");
                out.push_str("\\end{matrix}");
                return;
            }
            cells += 1;
            if cell_index > 0 {
                out.push('&');
            }
            emit_children(cell, out, depth);
        }
    }
    out.push_str("\\end{matrix}");
}

/// `mfenced` was dropped in MathML 4 but is still what many producers emit.
/// Its delimiters are attributes, so the pair is balanced by construction and
/// can safely take `\left`/`\right`.
fn fenced(elem: &Element, out: &mut String, depth: usize) {
    let open = elem.attr_any("open").unwrap_or("(");
    let close = elem.attr_any("close").unwrap_or(")");
    let separators: Vec<char> =
        elem.attr_any("separators").unwrap_or(",").chars().filter(|c| !c.is_whitespace()).collect();

    out.push_str("\\left");
    out.push_str(&delim_glyph(first_char(open)));
    for (index, child) in elem.child_elems().enumerate() {
        if index > 0 {
            let separator = separators.get(index - 1).or_else(|| separators.last()).copied();
            if let Some(separator) = separator {
                out.push_str(&delim_sep(separator));
            }
        }
        emit(child, out, depth);
    }
    out.push_str("\\right");
    out.push_str(&delim_glyph(first_char(close)));
}

/// An explicitly empty delimiter draws no glyph, which `delim_glyph` spells as
/// the LaTeX null delimiter.
fn first_char(value: &str) -> Option<char> {
    value.chars().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::xml::parse_xml;

    fn latex(body: &str) -> String {
        let doc = format!(r#"<math xmlns="http://www.w3.org/1998/Math/MathML">{body}</math>"#);
        let root = parse_xml(doc.as_bytes()).unwrap();
        let math = root.descendants_any("math").next().unwrap();
        match to_inline(math, false) {
            Some(Inline::Math { latex, .. }) => latex,
            _ => String::new(),
        }
    }

    #[test]
    fn a_fraction_keeps_the_relation_its_glyphs_lost() {
        assert_eq!(latex("<mfrac><mn>1</mn><mn>3</mn></mfrac>"), r"\frac{1}{3}");
    }

    #[test]
    fn a_zero_rule_is_a_stack_not_a_fraction() {
        assert_eq!(
            latex(r#"<mfrac linethickness="0"><mi>n</mi><mi>k</mi></mfrac>"#),
            r"{{n}\atop {k}}"
        );
    }

    #[test]
    fn invisible_operators_carry_no_glyph_and_leave_none() {
        let xml = "<mi>x</mi><mo>\u{2062}</mo><mi>y</mi><mo>\u{2061}</mo><mi>z</mi>";
        assert_eq!(latex(xml), "xyz");
    }

    #[test]
    fn a_multi_character_identifier_is_a_name_not_a_product() {
        assert_eq!(latex("<mi>Rank</mi><mi>A</mi>"), r"\mathrm{Rank}A");
        assert_eq!(latex("<mi>det</mi><mi>A</mi>"), r"\det A");
        assert_eq!(latex(r#"<mi mathvariant="normal">d</mi>"#), r"\mathrm{d}");
    }

    #[test]
    fn a_script_under_a_large_operator_is_a_limit() {
        let xml = "<munderover><mo>\u{2211}</mo><mi>i</mi><mi>n</mi></munderover>";
        assert_eq!(latex(xml), "\u{2211}_{i}^{n}");
    }

    #[test]
    fn a_script_over_an_ordinary_base_is_an_overset() {
        assert_eq!(
            latex("<munderover><mi>A</mi><mn>1</mn><mn>2</mn></munderover>"),
            r"\underset{1}{\overset{2}{A}}"
        );
    }

    #[test]
    fn a_combining_mark_over_a_base_is_an_accent() {
        assert_eq!(latex("<mover><mi>x</mi><mo>\u{302}</mo></mover>"), r"\widehat{x}");
    }

    #[test]
    fn a_sequence_base_is_braced_so_the_script_binds_all_of_it() {
        let xml = "<msup><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mn>2</mn></msup>";
        assert_eq!(latex(xml), "{a+b}^{2}");
    }

    #[test]
    fn prescripts_precede_the_base_on_an_empty_atom() {
        let xml = "<mmultiscripts><mi>X</mi><mn>1</mn><mn>2</mn>\
                   <mprescripts/><mn>3</mn><mn>4</mn></mmultiscripts>";
        assert_eq!(latex(xml), "{}_{3}^{4}X_{1}^{2}");
    }

    #[test]
    fn an_empty_multiscript_slot_emits_no_script() {
        let xml = "<mmultiscripts><mi>X</mi><none/><mn>2</mn></mmultiscripts>";
        assert_eq!(latex(xml), "X^{2}");
    }

    #[test]
    fn prose_keeps_the_space_that_separates_it_from_the_next_symbol() {
        assert_eq!(latex("<mtext>if  </mtext><mi>x</mi>"), r"\text{if }x");
    }

    #[test]
    fn the_authors_own_latex_wins_over_the_presentation_tree() {
        let xml = r#"<semantics><mfrac><mn>1</mn><mn>2</mn></mfrac>
                     <annotation encoding="application/x-tex">\tfrac12</annotation></semantics>"#;
        assert_eq!(latex(xml), r"\tfrac12");
    }

    #[test]
    fn an_annotation_in_another_encoding_is_not_latex() {
        // ODF writes StarMath here, which would be nonsense as LaTeX.
        let xml = r#"<semantics><mfrac><mn>1</mn><mn>2</mn></mfrac>
                     <annotation encoding="StarMath 5.0">1 over 2</annotation></semantics>"#;
        assert_eq!(latex(xml), r"\frac{1}{2}");
    }

    #[test]
    fn a_named_width_has_no_latex_spelling_and_is_dropped() {
        assert_eq!(latex(r#"<mi>a</mi><mspace width="thickmathspace"/><mi>b</mi>"#), "ab");
        assert_eq!(latex(r#"<mi>a</mi><mspace width="1em"/><mi>b</mi>"#), r"a\hspace{1em}b");
    }

    #[test]
    fn a_bare_pipe_would_split_a_markdown_row() {
        assert_eq!(latex("<mo>|</mo><mi>x</mi><mo>|</mo>"), r"\mid x\mid");
    }

    #[test]
    fn text_cannot_escape_the_math_span() {
        assert_eq!(latex("<mtext>$x$</mtext>"), r"\text{\$x\$}");
    }

    #[test]
    fn an_unmodelled_element_keeps_its_content() {
        assert_eq!(latex("<mstyle><mi>q</mi></mstyle>"), "q");
    }

    #[test]
    fn nesting_past_the_bound_degrades_to_text() {
        let mut xml = "<mi>x</mi>".to_string();
        for _ in 0..MAX_DEPTH + 2 {
            xml = format!("<mrow>{xml}</mrow>");
        }
        assert_eq!(latex(&xml), "x");
    }

    #[test]
    fn an_empty_equation_produces_nothing() {
        assert_eq!(latex(""), "");
        assert_eq!(latex("<mrow/>"), "");
    }
}
