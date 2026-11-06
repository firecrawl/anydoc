//! Presentation MathML to LaTeX, for EPUB chapters and OpenDocument
//! formula objects. A TeX annotation, when the producer kept one, is taken
//! verbatim over a reconversion of the presentation tree.

use super::tex::{Tex, Variant, accent, delimiter, function_name, symbol};
use crate::package::xml::{Element, Node};

/// A `math` element as LaTeX source without delimiters.
pub fn mathml_to_tex(math: &Element) -> String {
    let mut tex = Tex::new();
    walk_children(math, &mut tex);
    tex.finish()
}

/// Whether a `math` element asks for display (block) layout.
pub fn mathml_is_display(math: &Element) -> bool {
    math.attr_any("display").is_some_and(|d| d == "block")
}

fn walk_children(elem: &Element, tex: &mut Tex) {
    for node in &elem.children {
        match node {
            Node::Elem(child) => walk_elem(child, tex),
            // Text outside token elements is layout whitespace.
            Node::Text(_) => {}
        }
    }
}

/// Child elements, skipping nothing: MathML layout schemata count their
/// arguments by position.
fn args(elem: &Element) -> Vec<&Element> {
    elem.child_elems().collect()
}

fn sub(elem: &Element) -> Tex {
    let mut tex = Tex::new();
    walk_elem(elem, &mut tex);
    tex
}

fn sub_children(elem: &Element) -> Tex {
    let mut tex = Tex::new();
    walk_children(elem, &mut tex);
    tex
}

fn walk_elem(e: &Element, tex: &mut Tex) {
    match e.local.as_str() {
        "semantics" => {
            if let Some(annotation) = tex_annotation(e) {
                tex.push_str(&annotation);
            } else if let Some(first) = e
                .child_elems()
                .find(|c| !matches!(c.local.as_str(), "annotation" | "annotation-xml"))
            {
                walk_elem(first, tex);
            }
        }
        "annotation" | "annotation-xml" => {}
        "mi" => identifier(e, tex),
        "mn" => tex.push_math_text(&e.text()),
        "mo" => operator(e, tex),
        "mtext" => {
            let text = e.text();
            if !text.trim().is_empty() {
                tex.push_text_mode(text.trim_matches(|c: char| c == '\n' || c == '\r'));
            }
        }
        "ms" => {
            let text = e.text();
            let quoted = format!(
                "{}{}{}",
                e.attr_any("lquote").unwrap_or("\""),
                text.trim_matches(|c: char| c == '\n' || c == '\r'),
                e.attr_any("rquote").unwrap_or("\"")
            );
            tex.push_text_mode(&quoted);
        }
        "mspace" => {
            // Only the sign of the width survives: zero (the default) is
            // nothing, negative a thin backspace, anything else a space.
            let width = e.attr_any("width").unwrap_or("0").trim();
            let negative = width.starts_with('-') || width.starts_with("negative");
            let magnitude: f64 = width
                .trim_start_matches('-')
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>()
                .parse()
                .unwrap_or(1.0);
            if magnitude == 0.0 {
            } else if negative {
                tex.push_str("\\!");
            } else {
                tex.push_str("\\ ");
            }
        }
        "mfrac" => {
            let parts = args(e);
            let (Some(num), Some(den)) = (parts.first(), parts.get(1)) else {
                return walk_children(e, tex);
            };
            let (num, den) = (sub(num), sub(den));
            if e.attr_any("bevelled").is_some_and(|b| b == "true") {
                tex.push_group(&num);
                tex.push_char('/');
                tex.push_group(&den);
            } else if e.attr_any("linethickness").is_some_and(|t| matches!(t.trim(), "0" | "0px")) {
                tex.push_char('{');
                tex.push_tex(&num);
                tex.push_macro("\\atop");
                tex.push_tex(&den);
                tex.push_char('}');
            } else {
                tex.push_macro("\\frac");
                tex.push_group(&num);
                tex.push_group(&den);
            }
        }
        "msqrt" => tex.push_command("\\sqrt", &sub_children(e)),
        "mroot" => {
            let parts = args(e);
            tex.push_macro("\\sqrt");
            if let Some(index) = parts.get(1) {
                tex.push_char('[');
                tex.push_tex(&sub(index));
                tex.push_char(']');
            }
            if let Some(base) = parts.first() {
                tex.push_group(&sub(base));
            }
        }
        "msup" | "msub" | "msubsup" => scripts(e, tex),
        "munder" | "mover" | "munderover" => under_over(e, tex),
        "mmultiscripts" => multiscripts(e, tex),
        "mfenced" => {
            let open = e.attr_any("open").unwrap_or("(");
            let close = e.attr_any("close").unwrap_or(")");
            let separators: Vec<char> = e
                .attr_any("separators")
                .unwrap_or(",")
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            tex.push_macro("\\left");
            tex.push_str(open.chars().next().map_or(".", delimiter));
            for (i, part) in e.child_elems().enumerate() {
                if i > 0 {
                    // The last separator repeats for any further arguments.
                    let sep = separators.get(i - 1).or(separators.last());
                    if let Some(sep) = sep {
                        tex.push_math_char(*sep);
                    }
                }
                tex.push_tex(&sub(part));
            }
            tex.push_macro("\\right");
            tex.push_str(close.chars().next().map_or(".", delimiter));
        }
        "mtable" => table(e, tex),
        "mphantom" => tex.push_command("\\phantom", &sub_children(e)),
        "menclose" => {
            let notation = e.attr_any("notation").unwrap_or("longdiv");
            let inner = sub_children(e);
            match notation.split_whitespace().next() {
                Some("box") | Some("roundedbox") => tex.push_command("\\boxed", &inner),
                Some("top") => tex.push_command("\\overline", &inner),
                Some("bottom") | Some("underline") => tex.push_command("\\underline", &inner),
                Some("horizontalstrike")
                | Some("updiagonalstrike")
                | Some("downdiagonalstrike") => tex.push_command("\\cancel", &inner),
                _ => tex.push_group(&inner),
            }
        }
        "mglyph" | "maligngroup" | "malignmark" | "none" | "mprescripts" => {}
        // mrow, mstyle, mpadded, merror, maction, math itself, and anything
        // unknown: the children carry the content.
        _ => walk_children(e, tex),
    }
}

fn tex_annotation(semantics: &Element) -> Option<String> {
    semantics
        .child_elems()
        .filter(|a| a.local == "annotation")
        .find(|a| {
            a.attr_any("encoding").is_some_and(|enc| {
                matches!(
                    enc.trim().to_ascii_lowercase().as_str(),
                    "application/x-tex" | "tex" | "latex" | "application/x-latex"
                )
            })
        })
        .map(|a| a.text().trim().to_string())
        .filter(|t| !t.is_empty())
}

fn identifier(e: &Element, tex: &mut Tex) {
    let text = e.text();
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let variant = e.attr_any("mathvariant");
    let single = text.chars().count() == 1;
    // A multi-letter identifier renders upright: a function name where
    // LaTeX has one, otherwise roman text.
    if !single && variant.is_none_or(|v| v == "normal") && text.chars().all(|c| c.is_alphabetic()) {
        match function_name(text) {
            Some(name) => tex.push_macro(&name),
            None => {
                let mut inner = Tex::new();
                inner.push_math_text(text);
                tex.push_command("\\mathrm", &inner);
            }
        }
        return;
    }
    let mut inner = Tex::new();
    inner.push_math_text(text);
    match variant.and_then(Variant::from_mathvariant) {
        Some(v) => tex.push_command(v.command(), &inner),
        None if variant == Some("normal") && single => tex.push_command("\\mathrm", &inner),
        None => tex.push_tex(&inner),
    }
}

fn operator(e: &Element, tex: &mut Tex) {
    let text = e.text();
    let text = text.trim();
    if text.chars().count() > 1 && text.chars().all(|c| c.is_ascii_alphabetic()) {
        // `<mo>lim</mo>`, `<mo>max</mo>`: a named operator.
        if let Some(name) = function_name(text) {
            tex.push_macro(&name);
            return;
        }
    }
    tex.push_math_text(text);
}

fn scripts(e: &Element, tex: &mut Tex) {
    let parts = args(e);
    let Some(base) = parts.first() else { return };
    let base_tex = sub(base);
    if is_big_operator(base) {
        tex.push_tex(&base_tex);
    } else {
        tex.push_base(&base_tex);
    }
    match e.local.as_str() {
        "msup" => script(tex, '^', parts.get(1)),
        "msub" => script(tex, '_', parts.get(1)),
        _ => {
            script(tex, '_', parts.get(1));
            script(tex, '^', parts.get(2));
        }
    }
}

fn script(tex: &mut Tex, mark: char, arg: Option<&&Element>) {
    let Some(arg) = arg else { return };
    tex.push_char(mark);
    tex.push_group(&sub(arg));
}

fn under_over(e: &Element, tex: &mut Tex) {
    let parts = args(e);
    let Some(base) = parts.first() else { return };
    let (under, over) = match e.local.as_str() {
        "munder" => (parts.get(1), None),
        "mover" => (None, parts.get(1)),
        _ => (parts.get(1), parts.get(2)),
    };
    // Limits on a big operator or a named function take script form,
    // which LaTeX places under and over by itself.
    if is_big_operator(base) || is_named_function(base) {
        tex.push_tex(&sub(base));
        script(tex, '_', under);
        script(tex, '^', over);
        return;
    }
    // An explicit accent="false" / accentunder="false" asks for a plain
    // over- or under-script even on an accent character.
    let plain_over = e.attr_any("accent").is_some_and(|v| v == "false");
    let plain_under = e.attr_any("accentunder").is_some_and(|v| v == "false");
    let mut body = sub(base);
    if let Some(over) = over {
        body = decorate(over, body, true, plain_over);
    }
    if let Some(under) = under {
        body = decorate(under, body, false, plain_under);
    }
    tex.push_tex(&body);
}

/// `body` with `mark` placed over or under it: an accent command where
/// the mark is an accent character (unless `plain`), `\overset` /
/// `\underset` otherwise.
fn decorate(mark: &Element, body: Tex, over: bool, plain: bool) -> Tex {
    let mut out = Tex::new();
    let text = mark.text();
    let text = text.trim();
    if !plain
        && let Some(c) = text.chars().next()
        && text.chars().count() == 1
        && let Some(cmd) = accent(c)
    {
        let cmd = match (cmd, over) {
            ("\\underline", true) => "\\overline",
            ("\\overline", false) => "\\underline",
            ("\\underbrace", true) => "\\overbrace",
            ("\\overbrace", false) => "\\underbrace",
            (cmd, _) => cmd,
        };
        out.push_command(cmd, &body);
        return out;
    }
    out.push_macro(if over { "\\overset" } else { "\\underset" });
    out.push_group(&sub(mark));
    out.push_group(&body);
    out
}

fn multiscripts(e: &Element, tex: &mut Tex) {
    let parts = args(e);
    let Some((base, rest)) = parts.split_first() else { return };
    let split = rest.iter().position(|p| p.local == "mprescripts").unwrap_or(rest.len());
    let (post, pre) = rest.split_at(split);
    let pre = pre.get(1..).unwrap_or(&[]);
    for pair in pre.chunks(2) {
        tex.push_str("{}");
        script_pair(tex, pair);
    }
    tex.push_group(&sub(base));
    for pair in post.chunks(2) {
        script_pair(tex, pair);
    }
}

fn script_pair(tex: &mut Tex, pair: &[&Element]) {
    if let Some(sub_) = pair.first().filter(|p| p.local != "none") {
        tex.push_char('_');
        tex.push_group(&sub(sub_));
    }
    if let Some(sup) = pair.get(1).filter(|p| p.local != "none") {
        tex.push_char('^');
        tex.push_group(&sub(sup));
    }
}

fn table(e: &Element, tex: &mut Tex) {
    tex.push_str("\\begin{matrix}");
    let rows = e.child_elems().filter(|r| matches!(r.local.as_str(), "mtr" | "mlabeledtr"));
    for (i, row) in rows.enumerate() {
        if i > 0 {
            tex.push_str(" \\\\");
        }
        let cells = row.child_elems().filter(|c| c.local == "mtd");
        for (j, cell) in cells.enumerate() {
            tex.push_str(if j > 0 { " & " } else { " " });
            tex.push_tex(&sub_children(cell));
        }
    }
    tex.push_str(" \\end{matrix}");
}

/// An `mo` holding an n-ary operator (∑, ∫, ...).
fn is_big_operator(e: &Element) -> bool {
    if e.local != "mo" {
        return false;
    }
    let text = e.text();
    let mut chars = text.trim().chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => symbol(c).is_some_and(|s| {
            matches!(
                s,
                "\\sum"
                    | "\\prod"
                    | "\\coprod"
                    | "\\int"
                    | "\\iint"
                    | "\\iiint"
                    | "\\iiiint"
                    | "\\oint"
                    | "\\oiint"
                    | "\\oiiint"
                    | "\\bigcup"
                    | "\\bigcap"
                    | "\\bigvee"
                    | "\\bigwedge"
                    | "\\bigoplus"
                    | "\\bigotimes"
                    | "\\bigodot"
                    | "\\biguplus"
                    | "\\bigsqcup"
            )
        }),
        _ => false,
    }
}

/// An `mi` or `mo` naming a function that takes limits (`lim`, `max`).
fn is_named_function(e: &Element) -> bool {
    matches!(e.local.as_str(), "mi" | "mo") && {
        let text = e.text();
        let text = text.trim();
        text.chars().count() > 1 && function_name(text).is_some()
    }
}
