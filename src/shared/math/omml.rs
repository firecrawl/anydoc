//! Office Math (OMML) to LaTeX. The same element tree serves docx and pptx
//! directly, and rtf through its math destinations, which mirror OMML
//! element for element: there, an attribute arrives as the element's text
//! (`{\mtype lin}`) and a run's text sits directly in the run.

use super::tex::{Tex, accent, delimiter, function_name, symbol};
use crate::package::xml::{Element, Node, ns};

/// One `m:oMath` as LaTeX source without delimiters.
pub fn omath_to_tex(omath: &Element) -> String {
    let mut tex = Tex::new();
    walk_children(omath, &mut tex, Mode::Math);
    tex.finish()
}

/// The equations of an `m:oMathPara`, one per `m:oMath` line.
pub fn omath_para_to_tex(para: &Element) -> Vec<String> {
    let lines: Vec<String> =
        para.find_all(ns::M, "oMath").map(omath_to_tex).filter(|t| !t.is_empty()).collect();
    if lines.is_empty() {
        let whole = omath_to_tex(para);
        return if whole.is_empty() { Vec::new() } else { vec![whole] };
    }
    lines
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Math,
    /// Inside `m:fName`: run text names a function (`\sin`, `\lim`).
    FuncName,
    /// Inside an equation-array row: `&` marks an alignment point.
    Array,
}

fn walk_children(elem: &Element, tex: &mut Tex, mode: Mode) {
    for child in elem.child_elems() {
        walk_elem(child, tex, mode);
    }
}

fn walk_elem(e: &Element, tex: &mut Tex, mode: Mode) {
    if e.ns.as_deref() != Some(ns::M) {
        // Revision and bookmark markup from the host document.
        match e.local.as_str() {
            "del" | "moveFrom" | "rPr" | "pPr" => {}
            _ => walk_children(e, tex, mode),
        }
        return;
    }
    match e.local.as_str() {
        "r" => run(e, tex, mode),
        "t" => tex.push_math_text(&e.text()),
        "f" => fraction(e, tex),
        "sSup" => {
            tex.push_base(&arg(e, "e", Mode::Math));
            tex.push_char('^');
            tex.push_group(&arg(e, "sup", Mode::Math));
        }
        "sSub" => {
            tex.push_base(&arg(e, "e", Mode::Math));
            tex.push_char('_');
            tex.push_group(&arg(e, "sub", Mode::Math));
        }
        "sSubSup" => {
            tex.push_base(&arg(e, "e", Mode::Math));
            tex.push_char('_');
            tex.push_group(&arg(e, "sub", Mode::Math));
            tex.push_char('^');
            tex.push_group(&arg(e, "sup", Mode::Math));
        }
        "sPre" => {
            tex.push_str("{}_");
            tex.push_group(&arg(e, "sub", Mode::Math));
            tex.push_char('^');
            tex.push_group(&arg(e, "sup", Mode::Math));
            tex.push_group(&arg(e, "e", Mode::Math));
        }
        "rad" => {
            let deg = arg(e, "deg", Mode::Math);
            tex.push_macro("\\sqrt");
            if flag(e, "radPr", "degHide") != Some(true) && !deg.is_empty() {
                tex.push_char('[');
                tex.push_tex(&deg);
                tex.push_char(']');
            }
            tex.push_group(&arg(e, "e", Mode::Math));
        }
        "d" => delimited(e, tex),
        "nary" => nary(e, tex),
        "func" => {
            if let Some(name) = e.find(ns::M, "fName") {
                walk_children(name, tex, Mode::FuncName);
            }
            tex.push_group(&arg(e, "e", Mode::Math));
        }
        "acc" => {
            let chr = prop(e, "accPr", "chr").and_then(|v| v.chars().next()).unwrap_or('\u{302}');
            tex.push_command(accent(chr).unwrap_or("\\hat"), &arg(e, "e", Mode::Math));
        }
        "bar" => {
            let top = prop(e, "barPr", "pos").is_some_and(|p| p == "top");
            let cmd = if top { "\\overline" } else { "\\underline" };
            tex.push_command(cmd, &arg(e, "e", Mode::Math));
        }
        "borderBox" => tex.push_command("\\boxed", &arg(e, "e", Mode::Math)),
        "phant" if flag(e, "phantPr", "show") == Some(false) => {
            tex.push_command("\\phantom", &arg(e, "e", Mode::Math));
        }
        "box" | "phant" => tex.push_group(&arg(e, "e", mode)),
        "groupChr" => group_char(e, tex),
        "limLow" | "limUpp" => limit(e, tex, mode),
        "m" => matrix(e, tex),
        "eqArr" => equation_array(e, tex),
        local if local.ends_with("Pr") => {}
        _ => walk_children(e, tex, mode),
    }
}

/// Child argument `name` of `parent`, converted on its own.
fn arg(parent: &Element, name: &str, mode: Mode) -> Tex {
    let mut tex = Tex::new();
    if let Some(child) = parent.find(ns::M, name) {
        walk_children(child, &mut tex, mode);
    }
    tex
}

/// A property value: `parent/pr/name/@m:val`, or the element's own text
/// where the tree came from rtf.
fn prop(parent: &Element, pr: &str, name: &str) -> Option<String> {
    let elem = parent.find(ns::M, pr)?.find(ns::M, name)?;
    Some(match elem.attr(ns::M, "val") {
        Some(v) => v.to_string(),
        None => direct_text(elem).trim().to_string(),
    })
}

/// An on/off property: absent is `None`; present with no value is on.
fn flag(parent: &Element, pr: &str, name: &str) -> Option<bool> {
    let value = prop(parent, pr, name)?;
    Some(!matches!(value.as_str(), "0" | "off" | "false"))
}

fn direct_text(elem: &Element) -> String {
    let mut out = String::new();
    for node in &elem.children {
        if let Node::Text(t) = node {
            out.push_str(t);
        }
    }
    out
}

/// A run's text: its `m:t` children, or its own text where the tree came
/// from rtf.
fn run_text(r: &Element) -> String {
    let mut from_t = String::new();
    let mut has_t = false;
    for t in r.find_all(ns::M, "t") {
        has_t = true;
        from_t.push_str(&t.text());
    }
    if has_t {
        return from_t;
    }
    let direct = direct_text(r);
    if direct.trim().is_empty() { String::new() } else { direct }
}

fn run(r: &Element, tex: &mut Tex, mode: Mode) {
    let text = run_text(r);
    if text.is_empty() {
        return;
    }
    // Run properties sit under `m:rPr`; rtf writes them on the run itself,
    // with `sty` and `scr` as numbers in their enumeration order.
    let value = |name: &str| -> Option<String> {
        let elem = r.find(ns::M, "rPr").and_then(|p| p.find(ns::M, name)).or(r.find(ns::M, name))?;
        Some(match elem.attr(ns::M, "val") {
            Some(v) => v.to_string(),
            None => direct_text(elem).trim().to_string(),
        })
    };
    if value("nor").is_some_and(|v| !matches!(v.as_str(), "0" | "off" | "false")) {
        tex.push_text_mode(&text);
        return;
    }
    if mode == Mode::FuncName
        && let Some(name) = function_name(&text)
    {
        tex.push_macro(&name);
        return;
    }
    let font = match value("scr").as_deref() {
        Some("script" | "1") => Some("\\mathcal"),
        Some("fraktur" | "2") => Some("\\mathfrak"),
        Some("double-struck" | "3") => Some("\\mathbb"),
        Some("sans-serif" | "4") => Some("\\mathsf"),
        Some("monospace" | "5") => Some("\\mathtt"),
        _ => match value("sty").as_deref() {
            Some("p" | "0") => Some("\\mathrm"),
            Some("b" | "1") => Some("\\mathbf"),
            Some("bi" | "3") => Some("\\boldsymbol"),
            _ => None,
        },
    };
    let mut inner = Tex::new();
    match mode {
        Mode::Array => {
            for (i, piece) in text.split('&').enumerate() {
                if i > 0 {
                    inner.push_str(" & ");
                }
                inner.push_math_text(piece);
            }
        }
        _ => inner.push_math_text(&text),
    }
    match font {
        Some(cmd) => tex.push_command(cmd, &inner),
        None => tex.push_tex(&inner),
    }
}

fn fraction(e: &Element, tex: &mut Tex) {
    let num = arg(e, "num", Mode::Math);
    let den = arg(e, "den", Mode::Math);
    match prop(e, "fPr", "type").as_deref() {
        Some("lin") => {
            tex.push_group(&num);
            tex.push_char('/');
            tex.push_group(&den);
        }
        Some("skw") => {
            tex.push_str("{}^");
            tex.push_group(&num);
            tex.push_str("/_");
            tex.push_group(&den);
        }
        Some("noBar") => {
            tex.push_char('{');
            tex.push_tex(&num);
            tex.push_macro("\\atop");
            tex.push_tex(&den);
            tex.push_char('}');
        }
        _ => {
            tex.push_macro("\\frac");
            tex.push_group(&num);
            tex.push_group(&den);
        }
    }
}

fn delimited(e: &Element, tex: &mut Tex) {
    // An explicitly empty character means no delimiter on that side.
    let chr = |name: &str, default: char| -> Option<char> {
        match prop(e, "dPr", name) {
            Some(v) => v.chars().next(),
            None => Some(default),
        }
    };
    let beg = chr("begChr", '(');
    let end = chr("endChr", ')');
    let sep = chr("sepChr", '|');
    tex.push_macro("\\left");
    tex.push_str(beg.map_or(".", delimiter));
    for (i, part) in e.find_all(ns::M, "e").enumerate() {
        if i > 0 {
            if let Some(sep) = sep {
                tex.push_macro("\\middle");
                tex.push_str(delimiter(sep));
            } else {
                tex.push_char(',');
            }
        }
        let mut inner = Tex::new();
        walk_children(part, &mut inner, Mode::Math);
        tex.push_tex(&inner);
    }
    tex.push_macro("\\right");
    tex.push_str(end.map_or(".", delimiter));
}

fn nary(e: &Element, tex: &mut Tex) {
    let chr = prop(e, "naryPr", "chr").and_then(|v| v.chars().next()).unwrap_or('∫');
    match symbol(chr) {
        Some(op) if op.starts_with('\\') => tex.push_macro(op),
        _ => {
            tex.push_macro("\\operatorname*");
            let mut inner = Tex::new();
            inner.push_math_char(chr);
            tex.push_group(&inner);
        }
    }
    let integral = matches!(chr, '∫' | '∬' | '∭' | '⨌' | '∮' | '∯' | '∰');
    match prop(e, "naryPr", "limLoc").as_deref() {
        Some("undOvr") if integral => tex.push_macro("\\limits"),
        Some("subSup") if !integral => tex.push_macro("\\nolimits"),
        _ => {}
    }
    if flag(e, "naryPr", "subHide") != Some(true) {
        let sub = arg(e, "sub", Mode::Math);
        if !sub.is_empty() {
            tex.push_char('_');
            tex.push_group(&sub);
        }
    }
    if flag(e, "naryPr", "supHide") != Some(true) {
        let sup = arg(e, "sup", Mode::Math);
        if !sup.is_empty() {
            tex.push_char('^');
            tex.push_group(&sup);
        }
    }
    tex.push_group(&arg(e, "e", Mode::Math));
}

fn group_char(e: &Element, tex: &mut Tex) {
    let chr = prop(e, "groupChrPr", "chr").and_then(|v| v.chars().next()).unwrap_or('\u{23df}');
    let top = prop(e, "groupChrPr", "pos").is_some_and(|p| p == "top");
    let body = arg(e, "e", Mode::Math);
    match accent(chr) {
        Some(cmd) => tex.push_command(cmd, &body),
        None => {
            let mut mark = Tex::new();
            mark.push_math_char(chr);
            tex.push_macro(if top { "\\overset" } else { "\\underset" });
            tex.push_group(&mark);
            tex.push_group(&body);
        }
    }
}

fn limit(e: &Element, tex: &mut Tex, mode: Mode) {
    let upper = e.local == "limUpp";
    let base = arg(e, "e", mode);
    let lim = arg(e, "lim", Mode::Math);
    if mode == Mode::FuncName {
        // `\lim_{x \to 0}`: the operator takes its own limits.
        tex.push_tex(&base);
        tex.push_char(if upper { '^' } else { '_' });
        tex.push_group(&lim);
        return;
    }
    tex.push_macro(if upper { "\\overset" } else { "\\underset" });
    tex.push_group(&lim);
    tex.push_group(&base);
}

fn matrix(e: &Element, tex: &mut Tex) {
    tex.push_str("\\begin{matrix}");
    for (i, row) in e.find_all(ns::M, "mr").enumerate() {
        if i > 0 {
            tex.push_str(" \\\\");
        }
        for (j, cell) in row.find_all(ns::M, "e").enumerate() {
            tex.push_str(if j > 0 { " & " } else { " " });
            let mut inner = Tex::new();
            walk_children(cell, &mut inner, Mode::Math);
            tex.push_tex(&inner);
        }
    }
    tex.push_str(" \\end{matrix}");
}

fn equation_array(e: &Element, tex: &mut Tex) {
    let rows: Vec<Tex> = e
        .find_all(ns::M, "e")
        .map(|row| {
            let mut inner = Tex::new();
            walk_children(row, &mut inner, Mode::Array);
            inner
        })
        .collect();
    let env = if rows.iter().any(|r| r.contains('&')) { "aligned" } else { "gathered" };
    tex.push_str(&format!("\\begin{{{env}}}"));
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            tex.push_str(" \\\\");
        }
        tex.push_char(' ');
        tex.push_tex(row);
    }
    tex.push_str(&format!(" \\end{{{env}}}"));
}
