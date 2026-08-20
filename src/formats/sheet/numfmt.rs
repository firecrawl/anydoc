//! SpreadsheetML number format codes (ISO/IEC 29500-1 §18.8.30/31).
//!
//! A code parses into up to four `;`-separated sections. Numeric sections
//! render the value here; date/time sections are only classified, because
//! the sheet reader deliberately keeps its ISO-like date output (`mm/dd`
//! versus `dd/mm` is ambiguous for a downstream reader, ISO is not). Any
//! construct outside the implemented grammar makes [`NumberFormat::parse`]
//! return `None` and the caller falls back to General: approximating a
//! format silently would be worse than not applying it.

/// Implied format codes for built-in numFmtIds. Ids 5-8 are absent
/// deliberately: the standard leaves them to the file's own formatCode, so
/// an unresolved reference falls back to General rather than a guessed
/// currency format. Ids 27-36 and 50-81 are locale-specific (zh, ja, ko, th)
/// and unresolvable without a locale.
pub(super) fn builtin_code(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// What a resolved format asks the caller to do with a numeric value.
#[derive(Debug, PartialEq)]
pub(super) enum Rendered {
    /// Render this value the way an unformatted cell renders.
    General { value: f64, prefix: String, suffix: String },
    /// The section is a date/time format: render the serial as a date, time
    /// of day, or (when `elapsed`) a duration.
    DateTime { elapsed: bool },
    /// The formatted text, ready to emit.
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Region {
    Int,
    Frac,
    Exp,
    Num,
    Den,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Digit {
        place: char,
        region: Region,
    },
    Decimal,
    Percent,
    Literal(String),
    /// Unquoted digits, valid only as a fixed fraction denominator.
    BareDigits(String),
    /// A `,` pending resolution into grouping, scaling, or a literal.
    Comma,
    /// `E+` / `E-`; `plus` keeps the sign on non-negative exponents.
    Exp {
        plus: bool,
    },
    /// Fraction bar between numerator and denominator placeholders.
    Slash,
    /// `@`, the text placeholder.
    At,
    /// `_x`: skip the width of one character (one space here).
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy)]
struct Cond {
    op: Op,
    operand: f64,
}

impl Cond {
    fn matches(&self, v: f64) -> bool {
        match self.op {
            Op::Lt => v < self.operand,
            Op::Le => v <= self.operand,
            Op::Gt => v > self.operand,
            Op::Ge => v >= self.operand,
            Op::Eq => v == self.operand,
            Op::Ne => v != self.operand,
        }
    }
}

#[derive(Debug)]
struct NumSpec {
    toks: Vec<Tok>,
    grouping: bool,
    /// Trailing commas: each divides the value by 1000.
    scale: u32,
    /// Each `%` multiplies the value by 100.
    percents: u32,
    int_places: usize,
    frac_places: usize,
    exp: bool,
    num_places: usize,
    den_places: usize,
    /// Fraction denominator written as literal digits.
    fixed_den: Option<u64>,
}

#[derive(Debug)]
enum Body {
    /// `General` with the literals that decorate it, which render around the
    /// value the way an unformatted cell would show it.
    General {
        prefix: String,
        suffix: String,
    },
    DateTime {
        elapsed: bool,
    },
    Number(NumSpec),
    Text(Vec<Tok>),
}

#[derive(Debug)]
struct Section {
    condition: Option<Cond>,
    body: Body,
}

/// A parsed format code.
#[derive(Debug)]
pub(super) struct NumberFormat {
    sections: Vec<Section>,
}

impl NumberFormat {
    pub(super) fn parse(code: &str) -> Option<NumberFormat> {
        // An empty code is not the `;;;` idiom for hiding a value, it is a
        // format that says nothing, so the caller falls back to General.
        if code.is_empty() {
            return None;
        }
        let parts = split_sections(code)?;
        if parts.is_empty() || parts.len() > 4 {
            return None;
        }
        let sections: Vec<Section> =
            parts.iter().map(|p| parse_section(p)).collect::<Option<_>>()?;
        if sections.iter().filter(|s| s.condition.is_some()).count() > 2 {
            return None;
        }
        // A text section is only valid in last position; the fourth section
        // is the text position, so nothing else may sit there.
        let last = sections.len() - 1;
        for (i, s) in sections.iter().enumerate() {
            let is_text = matches!(s.body, Body::Text(_));
            if is_text && (i != last || s.condition.is_some()) {
                return None;
            }
            if sections.len() == 4 && i == 3 && !is_text && !is_empty_body(&s.body) {
                return None;
            }
        }
        Some(NumberFormat { sections })
    }

    fn numeric_sections(&self) -> &[Section] {
        match self.sections.last() {
            Some(s) if matches!(s.body, Body::Text(_)) => &self.sections[..self.sections.len() - 1],
            _ if self.sections.len() == 4 => &self.sections[..3],
            _ => &self.sections,
        }
    }

    pub(super) fn format_number(&self, v: f64) -> Rendered {
        if !v.is_finite() {
            return Rendered::General { value: v, prefix: String::new(), suffix: String::new() };
        }
        let Some((section, value, auto_minus)) = select(self.numeric_sections(), v) else {
            return Rendered::General { value: v, prefix: String::new(), suffix: String::new() };
        };
        match &section.body {
            Body::General { prefix, suffix } => {
                Rendered::General { value, prefix: prefix.clone(), suffix: suffix.clone() }
            }
            Body::DateTime { elapsed } => Rendered::DateTime { elapsed: *elapsed },
            Body::Number(spec) => {
                match render_number(spec, value.abs(), auto_minus && value < 0.0) {
                    Some(s) => Rendered::Text(s),
                    None => {
                        Rendered::General { value: v, prefix: String::new(), suffix: String::new() }
                    }
                }
            }
            Body::Text(_) => {
                Rendered::General { value: v, prefix: String::new(), suffix: String::new() }
            }
        }
    }

    /// Apply the text section to a text value; `None` means the format has
    /// no section for text, so the value stays as it is.
    pub(super) fn format_text(&self, text: &str) -> Option<String> {
        let section = match self.sections.last() {
            Some(s) if matches!(s.body, Body::Text(_)) => s,
            _ if self.sections.len() == 4 => &self.sections[3],
            _ => return None,
        };
        match &section.body {
            Body::Text(toks) => {
                let mut out = String::new();
                for tok in toks {
                    match tok {
                        Tok::Literal(s) => out.push_str(s),
                        Tok::Skip => out.push(' '),
                        Tok::At => out.push_str(text),
                        _ => {}
                    }
                }
                Some(out)
            }
            // An empty fourth section hides text.
            _ => Some(String::new()),
        }
    }
}

fn is_empty_body(body: &Body) -> bool {
    matches!(body, Body::Number(spec) if spec.toks.is_empty())
}

/// Pick the section for a value. Returns the section, the value to render
/// (magnitude only for the positional negative section), and whether a
/// leading minus must be emitted for negative values.
fn select(sections: &[Section], v: f64) -> Option<(&Section, f64, bool)> {
    if sections.is_empty() {
        return None;
    }
    if sections.iter().any(|s| s.condition.is_some()) {
        for s in sections {
            match s.condition {
                Some(c) if c.matches(v) => return Some((s, v, true)),
                Some(_) => {}
                None => return Some((s, v, true)),
            }
        }
        return None;
    }
    let idx = match sections.len() {
        1 => 0,
        2 if v >= 0.0 => 0,
        2 => 1,
        _ if v > 0.0 => 0,
        _ if v < 0.0 => 1,
        _ => 2,
    };
    // The positional negative section renders the magnitude; its code
    // supplies the sign (parens, a literal minus).
    let neg_positional = idx == 1;
    Some((&sections[idx], if neg_positional { v.abs() } else { v }, !neg_positional))
}

/// Split a code on `;` outside quotes, brackets, and escapes.
fn split_sections(code: &str) -> Option<Vec<String>> {
    let mut parts = vec![String::new()];
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            ';' => parts.push(String::new()),
            '"' | '[' => {
                let close = if c == '"' { '"' } else { ']' };
                let part = parts.last_mut().unwrap();
                part.push(c);
                loop {
                    let c = chars.next()?;
                    part.push(c);
                    if c == close {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                let next = chars.next()?;
                let part = parts.last_mut().unwrap();
                part.push(c);
                part.push(next);
            }
            c => parts.last_mut().unwrap().push(c),
        }
    }
    Some(parts)
}

const COLORS: &[&str] = &["black", "blue", "cyan", "green", "magenta", "red", "white", "yellow"];

/// The text literal tokens render as, used to carry the decoration around a
/// `General` body.
fn decoration(toks: &[Tok]) -> String {
    toks.iter()
        .map(|t| match t {
            Tok::Literal(s) => s.as_str(),
            _ => " ",
        })
        .collect()
}

fn push_literal(raw: &mut Vec<Tok>, c: char) {
    match raw.last_mut() {
        Some(Tok::Literal(s)) => s.push(c),
        _ => raw.push(Tok::Literal(c.to_string())),
    }
}

fn parse_section(s: &str) -> Option<Section> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut raw: Vec<Tok> = Vec::new();
    let mut general_at = 0usize;
    let mut condition: Option<Cond> = None;
    let mut has_date = false;
    let mut elapsed = false;
    let mut has_general = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '[' => {
                let end = chars[i..].iter().position(|&c| c == ']')? + i;
                let inner: String = chars[i + 1..end].iter().collect();
                i = end + 1;
                bracket(&inner, &mut raw, &mut condition, &mut has_date, &mut elapsed)?;
            }
            '"' => {
                let end = chars[i + 1..].iter().position(|&c| c == '"')? + i + 1;
                for &c in &chars[i + 1..end] {
                    push_literal(&mut raw, c);
                }
                i = end + 1;
            }
            '\\' => {
                push_literal(&mut raw, *chars.get(i + 1)?);
                i += 2;
            }
            '_' => {
                chars.get(i + 1)?;
                raw.push(Tok::Skip);
                i += 2;
            }
            '*' => {
                // Repeat-to-fill: no column width exists here, emit nothing.
                chars.get(i + 1)?;
                i += 2;
            }
            '0' | '#' | '?' => {
                raw.push(Tok::Digit { place: c, region: Region::Int });
                i += 1;
            }
            '.' => {
                raw.push(Tok::Decimal);
                i += 1;
            }
            ',' => {
                raw.push(Tok::Comma);
                i += 1;
            }
            '%' => {
                raw.push(Tok::Percent);
                i += 1;
            }
            '@' => {
                raw.push(Tok::At);
                i += 1;
            }
            'E' | 'e' if matches!(chars.get(i + 1), Some('+') | Some('-')) => {
                raw.push(Tok::Exp { plus: chars[i + 1] == '+' });
                i += 2;
            }
            'y' | 'Y' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' | 'm' | 'M' => {
                has_date = true;
                while i < chars.len() && chars[i].eq_ignore_ascii_case(&c) {
                    i += 1;
                }
            }
            'g' | 'G' => {
                let word: String = chars[i..chars.len().min(i + 7)].iter().collect();
                if !word.eq_ignore_ascii_case("general") {
                    return None;
                }
                has_general = true;
                // Adjacent literals merge, so the split has to be recorded
                // against the rendered text rather than a token index.
                general_at = decoration(&raw).chars().count();
                i += 7;
            }
            'a' | 'A' => {
                // Matched over chars: the tokens are ASCII, but a multibyte
                // char can follow them, and byte slicing would split it.
                let len = ["AM/PM", "A/P"]
                    .iter()
                    .find(|t| {
                        chars.len() - i >= t.len()
                            && chars[i..i + t.len()]
                                .iter()
                                .zip(t.chars())
                                .all(|(c, want)| c.eq_ignore_ascii_case(&want))
                    })
                    .map(|t| t.len())?;
                has_date = true;
                i += len;
            }
            '1'..='9' => {
                let end = chars[i..]
                    .iter()
                    .position(|c| !c.is_ascii_digit())
                    .map_or(chars.len(), |p| p + i);
                raw.push(Tok::BareDigits(chars[i..end].iter().collect()));
                i = end;
            }
            '$' | '-' | '+' | '(' | ')' | ':' | ' ' => {
                push_literal(&mut raw, c);
                i += 1;
            }
            '/' => {
                raw.push(Tok::Slash);
                i += 1;
            }
            _ => return None,
        }
    }
    let body = if has_general {
        if raw.iter().any(|t| !matches!(t, Tok::Literal(_) | Tok::Skip)) {
            return None;
        }
        let text = decoration(&raw);
        let split = text.char_indices().nth(general_at).map_or(text.len(), |(b, _)| b);
        Body::General { prefix: text[..split].to_string(), suffix: text[split..].to_string() }
    } else if has_date {
        if raw.iter().any(|t| matches!(t, Tok::At | Tok::Exp { .. } | Tok::BareDigits(_))) {
            return None;
        }
        Body::DateTime { elapsed }
    } else if raw.iter().any(|t| matches!(t, Tok::At)) {
        if raw.iter().any(|t| {
            matches!(
                t,
                Tok::Digit { .. }
                    | Tok::Decimal
                    | Tok::Exp { .. }
                    | Tok::Slash
                    | Tok::BareDigits(_)
            )
        }) {
            return None;
        }
        let toks = raw
            .into_iter()
            .map(|t| match t {
                Tok::Percent => Tok::Literal("%".to_string()),
                Tok::Comma => Tok::Literal(",".to_string()),
                t => t,
            })
            .collect();
        Body::Text(toks)
    } else {
        Body::Number(resolve_number(raw)?)
    };
    Some(Section { condition, body })
}

fn bracket(
    inner: &str,
    raw: &mut Vec<Tok>,
    condition: &mut Option<Cond>,
    has_date: &mut bool,
    elapsed: &mut bool,
) -> Option<()> {
    match inner.chars().next()? {
        '<' | '>' | '=' => {
            if condition.is_some() {
                return None;
            }
            let (op, rest) = if let Some(r) = inner.strip_prefix(">=") {
                (Op::Ge, r)
            } else if let Some(r) = inner.strip_prefix("<=") {
                (Op::Le, r)
            } else if let Some(r) = inner.strip_prefix("<>") {
                (Op::Ne, r)
            } else if let Some(r) = inner.strip_prefix('>') {
                (Op::Gt, r)
            } else if let Some(r) = inner.strip_prefix('<') {
                (Op::Lt, r)
            } else {
                (Op::Eq, inner.strip_prefix('=')?)
            };
            *condition = Some(Cond { op, operand: rest.trim().parse().ok()? });
        }
        '$' => {
            // `[$sym-lcid]`: the currency string emits literally, the
            // locale id affects nothing rendered here.
            let sym = inner[1..].split('-').next().unwrap_or("");
            if !sym.is_empty() {
                raw.push(Tok::Literal(sym.to_string()));
            }
        }
        c @ ('h' | 'H' | 'm' | 'M' | 's' | 'S')
            if inner.chars().all(|x| x.eq_ignore_ascii_case(&c)) =>
        {
            *has_date = true;
            *elapsed = true;
        }
        _ => {
            let lower = inner.to_ascii_lowercase();
            let is_color = COLORS.contains(&lower.as_str())
                || lower
                    .strip_prefix("color")
                    .and_then(|n| n.trim().parse::<u32>().ok())
                    .is_some_and(|n| (1..=56).contains(&n));
            if !is_color {
                return None;
            }
        }
    }
    Some(())
}

/// Second pass over a numeric section: assign digit regions, resolve commas
/// into grouping or scaling, and recognize the fraction form.
fn resolve_number(raw: Vec<Tok>) -> Option<NumSpec> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut region = Region::Int;
    let mut spec = NumSpec {
        toks: Vec::new(),
        grouping: false,
        scale: 0,
        percents: 0,
        int_places: 0,
        frac_places: 0,
        exp: false,
        num_places: 0,
        den_places: 0,
        fixed_den: None,
    };
    let mut iter = raw.into_iter().peekable();
    while let Some(tok) = iter.next() {
        match tok {
            Tok::Digit { place, .. } => {
                if spec.fixed_den.is_some() && region == Region::Den {
                    return None;
                }
                toks.push(Tok::Digit { place, region });
            }
            Tok::Decimal => {
                if region != Region::Int {
                    return None;
                }
                region = Region::Frac;
                toks.push(Tok::Decimal);
            }
            Tok::Exp { plus } => {
                if spec.exp || matches!(region, Region::Num | Region::Den) {
                    return None;
                }
                spec.exp = true;
                region = Region::Exp;
                toks.push(Tok::Exp { plus });
            }
            Tok::Slash => {
                // A fraction bar needs a numerator run directly before it;
                // otherwise the slash is a literal.
                let run = toks
                    .iter()
                    .rev()
                    .take_while(|t| matches!(t, Tok::Digit { region: Region::Int, .. }))
                    .count();
                if run == 0 || region != Region::Int || spec.fixed_den.is_some() {
                    push_literal(&mut toks, '/');
                    continue;
                }
                let at = toks.len() - run;
                for t in &mut toks[at..] {
                    if let Tok::Digit { region, .. } = t {
                        *region = Region::Num;
                    }
                }
                toks.push(Tok::Slash);
                region = Region::Den;
                if let Some(Tok::BareDigits(_)) = iter.peek() {
                    let Some(Tok::BareDigits(d)) = iter.next() else { unreachable!() };
                    spec.fixed_den = Some(d.parse().ok()?);
                    toks.push(Tok::Literal(d));
                }
            }
            Tok::Percent => {
                spec.percents += 1;
                toks.push(Tok::Percent);
            }
            // Unquoted digits anywhere but a fixed denominator are outside
            // the grammar.
            Tok::BareDigits(_) => return None,
            other => toks.push(other),
        }
    }
    // Commas between digit placeholders group; commas after the last digit
    // placeholder scale by 1000 each; the rest are literal.
    let digit_at: Vec<bool> = toks.iter().map(|t| matches!(t, Tok::Digit { .. })).collect();
    let first_digit = digit_at.iter().position(|&d| d);
    let last_digit = digit_at.iter().rposition(|&d| d);
    let mut out: Vec<Tok> = Vec::new();
    for (i, tok) in toks.into_iter().enumerate() {
        if tok != Tok::Comma {
            out.push(tok);
            continue;
        }
        match (first_digit, last_digit) {
            (Some(_), Some(l)) if i > l => spec.scale += 1,
            (Some(f), Some(l)) if i > f && i < l => spec.grouping = true,
            _ => push_literal(&mut out, ','),
        }
    }
    for t in &out {
        if let Tok::Digit { region, .. } = t {
            match region {
                Region::Int => spec.int_places += 1,
                Region::Frac => spec.frac_places += 1,
                Region::Exp => {}
                Region::Num => spec.num_places += 1,
                Region::Den => spec.den_places += 1,
            }
        }
    }
    if spec.exp && !out.iter().any(|t| matches!(t, Tok::Digit { region: Region::Exp, .. })) {
        return None;
    }
    if spec.num_places > 0 && spec.den_places == 0 && spec.fixed_den.is_none() {
        return None;
    }
    spec.toks = out;
    Some(spec)
}

fn render_number(spec: &NumSpec, v_abs: f64, minus: bool) -> Option<String> {
    let mut v = v_abs;
    for _ in 0..spec.percents {
        v *= 100.0;
    }
    for _ in 0..spec.scale {
        v /= 1000.0;
    }
    if !v.is_finite() {
        return None;
    }
    let body = if spec.exp {
        render_scientific(spec, v)?
    } else if spec.num_places > 0 {
        render_fraction(spec, v)?
    } else {
        let (int_digits, frac_digits) = split_digits(v, spec.frac_places)?;
        emit(spec, &int_digits, &frac_digits, "", 0)
    };
    Some(if minus { format!("-{body}") } else { body })
}

/// The rounded value's digits: the integer part (empty when zero, so `#`
/// can drop it) and exactly `dp` fractional digits. Rounding happens on the
/// 15-significant-digit decimal form, half away from zero, the way a
/// spreadsheet displays - binary arithmetic would round 5.255 at two
/// decimals to 5.25.
fn split_digits(v: f64, dp: usize) -> Option<(String, String)> {
    if !v.is_finite() || v < 0.0 || dp > 512 {
        return None;
    }
    if v == 0.0 {
        return Some((String::new(), "0".repeat(dp)));
    }
    // `{:.14e}` is the value at 15 significant decimal digits: an integer D
    // of up to 15 digits and an exponent, v = D * 10^(e-14).
    let repr = format!("{v:.14e}");
    let (mantissa, e) = repr.split_once('e')?;
    let e: i64 = e.parse().ok()?;
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // Digits of round(v * 10^dp), by shifting or by decimal rounding.
    let shift = e - 14 + dp as i64;
    let scaled = if shift >= 0 {
        format!("{digits}{}", "0".repeat(usize::try_from(shift).ok()?))
    } else {
        let drop = usize::try_from(-shift).ok()?;
        if drop > digits.len() {
            String::new()
        } else {
            let (kept, rest) = digits.split_at(digits.len() - drop);
            let mut kept: Vec<u8> = kept.bytes().collect();
            if rest.as_bytes().first().is_some_and(|&b| b >= b'5') {
                let mut i = kept.len();
                loop {
                    if i == 0 {
                        kept.insert(0, b'1');
                        break;
                    }
                    i -= 1;
                    if kept[i] == b'9' {
                        kept[i] = b'0';
                    } else {
                        kept[i] += 1;
                        break;
                    }
                }
            }
            String::from_utf8(kept).ok()?
        }
    };
    let scaled = scaled.trim_start_matches('0');
    let mut s = scaled.to_string();
    if s.len() < dp {
        s = format!("{}{}", "0".repeat(dp - s.len()), s);
    }
    let (int, frac) = s.split_at(s.len() - dp);
    Some((int.to_string(), frac.to_string()))
}

fn places(toks: &[Tok], r: Region) -> Vec<char> {
    toks.iter()
        .filter_map(|t| match t {
            Tok::Digit { place, region } if *region == r => Some(*place),
            _ => None,
        })
        .collect()
}

/// Distribute a digit string over placeholders: the first takes any excess
/// digits, and placeholders past the available digits pad per their kind
/// (`0` a zero, `?` a space, `#` nothing).
fn assign(digits: &str, places: &[char]) -> Vec<String> {
    let k = places.len();
    if k == 0 {
        return Vec::new();
    }
    let n = digits.len();
    let mut out = Vec::with_capacity(k);
    if n > k {
        out.push(digits[..n - k + 1].to_string());
        for c in digits[n - k + 1..].chars() {
            out.push(c.to_string());
        }
    } else {
        for &p in &places[..k - n] {
            out.push(match p {
                '0' => "0".to_string(),
                '?' => " ".to_string(),
                _ => String::new(),
            });
        }
        for c in digits.chars() {
            out.push(c.to_string());
        }
    }
    out
}

/// Walk the token list emitting digits into placeholders.
fn emit(
    spec: &NumSpec,
    int_digits: &str,
    frac_digits: &str,
    exp_digits: &str,
    exp_sign: i8,
) -> String {
    let int_assigned = assign(int_digits, &places(&spec.toks, Region::Int));
    let exp_assigned = assign(exp_digits, &places(&spec.toks, Region::Exp));
    let is_digit = |c: &char| c.is_ascii_digit();
    let total_int: usize = int_assigned.iter().map(|s| s.chars().filter(is_digit).count()).sum();
    // Fractional digits kept: everything up to the rightmost `0` placeholder
    // always shows; beyond that, trailing zeros drop from `#` and pad `?`.
    let frac_places = places(&spec.toks, Region::Frac);
    let min_frac = frac_places.iter().rposition(|&p| p == '0').map_or(0, |i| i + 1);
    let keep_frac = frac_digits.trim_end_matches('0').len().max(min_frac);

    // The point shows only when something follows it: kept digits, or the
    // alignment spaces a `?` place emits.
    let show_decimal = keep_frac > 0 || frac_places.contains(&'?');

    let mut out = String::new();
    let (mut int_i, mut frac_i, mut exp_i) = (0usize, 0usize, 0usize);
    let mut int_remaining = total_int;
    for tok in &spec.toks {
        match tok {
            Tok::Digit { region: Region::Int, .. } => {
                for c in int_assigned[int_i].chars() {
                    out.push(c);
                    if c.is_ascii_digit() {
                        int_remaining -= 1;
                        if spec.grouping && int_remaining > 0 && int_remaining.is_multiple_of(3) {
                            out.push(',');
                        }
                    }
                }
                int_i += 1;
            }
            Tok::Digit { place, region: Region::Frac } => {
                if frac_i < keep_frac {
                    out.push(frac_digits.as_bytes()[frac_i] as char);
                } else if *place == '?' {
                    out.push(' ');
                }
                frac_i += 1;
            }
            Tok::Digit { region: Region::Exp, .. } => {
                out.push_str(&exp_assigned[exp_i]);
                exp_i += 1;
            }
            Tok::Digit { .. } => {}
            Tok::Decimal => {
                if show_decimal {
                    out.push('.');
                }
            }
            Tok::Percent => out.push('%'),
            Tok::Literal(s) => out.push_str(s),
            Tok::Exp { plus } => {
                out.push('E');
                if exp_sign < 0 {
                    out.push('-');
                } else if *plus {
                    out.push('+');
                }
            }
            Tok::Skip => out.push(' '),
            Tok::Slash | Tok::At | Tok::Comma | Tok::BareDigits(_) => {}
        }
    }
    out
}

fn render_scientific(spec: &NumSpec, v: f64) -> Option<String> {
    let n_int = spec.int_places.max(1) as i64;
    let (int_digits, frac_digits, exp10) = if v == 0.0 {
        let (i, f) = split_digits(0.0, spec.frac_places)?;
        (i, f, 0i64)
    } else {
        // The decimal exponent comes from the shortest round-trip
        // formatting; the float log10 misplaces boundaries like 1000.
        let repr = format!("{v:e}");
        let mut e: i64 = repr.split('e').nth(1)?.parse().ok()?;
        // The exponent stays a multiple of the integer placeholder count
        // (engineering notation for `##0.0E+0`).
        e = e.div_euclid(n_int) * n_int;
        let m = v / 10f64.powi(i32::try_from(e).ok()?);
        let (mut i, mut f) = split_digits(m, spec.frac_places)?;
        // Rounding at the display precision can carry into a new digit
        // (9.99 -> 10.0): renormalize.
        if i.len() > usize::try_from(n_int).ok()? {
            e += n_int;
            let m = v / 10f64.powi(i32::try_from(e).ok()?);
            (i, f) = split_digits(m, spec.frac_places)?;
        }
        (i, f, e)
    };
    let bare = exp10.unsigned_abs().to_string();
    let pad = places(&spec.toks, Region::Exp).len().saturating_sub(bare.len());
    let exp_digits = format!("{}{}", "0".repeat(pad), bare);
    Some(emit(spec, &int_digits, &frac_digits, &exp_digits, if exp10 < 0 { -1 } else { 1 }))
}

fn render_fraction(spec: &NumSpec, v: f64) -> Option<String> {
    if v >= 1e15 {
        return None;
    }
    let has_int = spec.int_places > 0;
    let (mut whole, target) = if has_int { (v.trunc(), v.fract()) } else { (0.0, v) };
    let (mut num, den) = best_fraction(target, spec.fixed_den, spec.den_places)?;
    if has_int && num == den && den > 0 {
        whole += 1.0;
        num = 0;
    }
    let int_digits = if whole == 0.0 {
        // A zero integer part still shows when the whole value is zero.
        if num == 0 { "0".to_string() } else { String::new() }
    } else {
        split_digits(whole, 0)?.0
    };
    // A zero numerator blanks the fraction: "5", not "5 0/1".
    let hide = has_int && num == 0;
    let mut int_assigned = assign(&int_digits, &places(&spec.toks, Region::Int)).into_iter();
    let mut num_assigned = assign(&num.to_string(), &places(&spec.toks, Region::Num)).into_iter();
    let mut den_assigned = assign(&den.to_string(), &places(&spec.toks, Region::Den)).into_iter();
    let mut out = String::new();
    for tok in &spec.toks {
        match tok {
            Tok::Digit { region: Region::Int, .. } => {
                out.push_str(&int_assigned.next().unwrap_or_default());
            }
            Tok::Digit { region: Region::Num, .. } => {
                if !hide {
                    out.push_str(&num_assigned.next().unwrap_or_default());
                }
            }
            Tok::Digit { region: Region::Den, .. } => {
                if !hide {
                    out.push_str(&den_assigned.next().unwrap_or_default());
                }
            }
            Tok::Slash => {
                if !hide {
                    out.push('/');
                }
            }
            Tok::Literal(s) => {
                // The fixed denominator is stored as a literal; it hides
                // with the rest of the fraction.
                if !(hide && spec.fixed_den.is_some_and(|d| d.to_string() == *s)) {
                    out.push_str(s);
                }
            }
            Tok::Percent => out.push('%'),
            Tok::Skip => out.push(' '),
            _ => {}
        }
    }
    Some(out.trim_end().to_string())
}

fn best_fraction(x: f64, fixed: Option<u64>, den_places: usize) -> Option<(u64, u64)> {
    if x < 0.0 || !x.is_finite() {
        return None;
    }
    if let Some(d) = fixed {
        let n = (x * d as f64).round();
        return (n < 1e18).then_some((n as u64, d));
    }
    // Capped at the widest denominator Excel's own formats reach, so a
    // pile of placeholders cannot turn the search into a long loop.
    let max_den =
        10u64.saturating_pow(u32::try_from(den_places).ok()?).saturating_sub(1).min(9_999);
    let mut best = (0u64, 1u64);
    let mut best_err = f64::INFINITY;
    for d in 1..=max_den.max(1) {
        let n = (x * d as f64).round();
        if n >= 1e18 {
            return None;
        }
        let err = (x - n / d as f64).abs();
        if err < best_err {
            best_err = err;
            best = (n as u64, d);
            if err == 0.0 {
                break;
            }
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(code: &str, v: f64) -> String {
        match NumberFormat::parse(code).expect("code must parse").format_number(v) {
            Rendered::Text(s) => s,
            other => panic!("expected text for {code:?} on {v}, got {other:?}"),
        }
    }

    #[test]
    fn percent_scales_by_hundred() {
        assert_eq!(fmt("0.0%", 0.075), "7.5%");
        assert_eq!(fmt("0%", 0.155), "16%");
        assert_eq!(fmt("0.00%", -0.5), "-50.00%");
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(fmt("#,##0", 9876543.0), "9,876,543");
        assert_eq!(fmt("#,##0.00", 1234.5), "1,234.50");
        assert_eq!(fmt("#,##0", 0.0), "0");
        assert_eq!(fmt("#,##0", 999.0), "999");
    }

    #[test]
    fn quoted_and_bracketed_currency_pass_through() {
        assert_eq!(fmt("\"$\"#,##0.00", 1234.5), "$1,234.50");
        assert_eq!(fmt("[$$-409]#,##0.00", 1234.5), "$1,234.50");
        assert_eq!(fmt("#,##0.00\\ \"kr\"", 1234.5), "1,234.50 kr");
    }

    #[test]
    fn digit_placeholders_pad_per_kind() {
        assert_eq!(fmt("00000", 42.0), "00042");
        assert_eq!(fmt("#", 0.0), "");
        assert_eq!(fmt("0.##", 5.0), "5");
        assert_eq!(fmt("0.??", 5.0), "5.  ");
        assert_eq!(fmt("0.0#", 5.25), "5.25");
        assert_eq!(fmt("0.00", 5.255), "5.26");
        assert_eq!(fmt("???", 42.0), " 42");
    }

    #[test]
    fn sections_map_by_sign() {
        assert_eq!(fmt("0.00;(0.00)", -3.5), "(3.50)");
        assert_eq!(fmt("0.00;(0.00)", 3.5), "3.50");
        assert_eq!(fmt("0;-0;\"zero\"", 0.0), "zero");
        assert_eq!(fmt("0", -3.0), "-3");
    }

    #[test]
    fn colors_are_discarded() {
        assert_eq!(fmt("#,##0;[Red](#,##0)", -1234.0), "(1,234)");
    }

    #[test]
    fn conditions_select_sections() {
        assert_eq!(fmt("[>=100]0.0;0.00", 250.0), "250.0");
        assert_eq!(fmt("[>=100]0.0;0.00", 3.0), "3.00");
    }

    #[test]
    fn scaling_commas_divide_by_thousand() {
        assert_eq!(fmt("0.0,,", 12_345_678.0), "12.3");
        assert_eq!(fmt("#,##0,", 12_345_678.0), "12,346");
    }

    #[test]
    fn scientific_notation() {
        assert_eq!(fmt("0.00E+00", 12345.0), "1.23E+04");
        assert_eq!(fmt("0.00E+00", 0.0001234), "1.23E-04");
        assert_eq!(fmt("##0.0E+0", 0.0000123), "12.3E-6");
        assert_eq!(fmt("0.00E+00", 0.0), "0.00E+00");
    }

    #[test]
    fn fractions_approximate() {
        assert_eq!(fmt("# ?/?", 5.25), "5 1/4");
        assert_eq!(fmt("# ??/??", 2.675), "2 27/40");
        assert_eq!(fmt("# ?/?", 5.0), "5");
        assert_eq!(fmt("?/?", 0.5), "1/2");
        assert_eq!(fmt("# ?/8", 5.25), "5 2/8");
    }

    #[test]
    fn skip_emits_space_and_fill_emits_nothing() {
        assert_eq!(fmt("0.00_);(0.00)", 3.5), "3.50 ");
        assert_eq!(fmt("$* 0.00", 3.5), "$3.50");
    }

    #[test]
    fn escaped_and_quoted_text_is_literal() {
        assert_eq!(fmt("0.0\\ \"m/s\"", 3.51), "3.5 m/s");
        // Quoted date letters must not turn the section into a date.
        assert_eq!(fmt("0\"d\"", 3.0), "3d");
    }

    #[test]
    fn literal_only_sections_hide_the_value() {
        assert_eq!(fmt("\"yes\";\"yes\";\"no\"", 1.0), "yes");
        assert_eq!(fmt("\"yes\";\"yes\";\"no\"", 0.0), "no");
    }

    #[test]
    fn date_sections_classify_without_rendering() {
        let f = NumberFormat::parse("yyyy\\-mm\\-dd").unwrap();
        assert_eq!(f.format_number(45000.0), Rendered::DateTime { elapsed: false });
        let f = NumberFormat::parse("[hh]:mm:ss").unwrap();
        assert_eq!(f.format_number(1.5), Rendered::DateTime { elapsed: true });
        let f = NumberFormat::parse("h:mm AM/PM").unwrap();
        assert_eq!(f.format_number(0.5), Rendered::DateTime { elapsed: false });
    }

    #[test]
    fn general_keeps_the_literals_around_it() {
        // `General"kg"` shows the unformatted value with its unit, so the
        // literals cannot be dropped on the way through.
        let f = NumberFormat::parse(r#""~"General" kg""#).unwrap();
        assert_eq!(
            f.format_number(1234.5),
            Rendered::General { value: 1234.5, prefix: "~".into(), suffix: " kg".into() }
        );
    }

    #[test]
    fn general_renders_generally() {
        let f = NumberFormat::parse("General").unwrap();
        assert_eq!(
            f.format_number(3.5),
            Rendered::General { value: 3.5, prefix: String::new(), suffix: String::new() }
        );
        // The positional negative section receives the magnitude.
        let f = NumberFormat::parse("General;General").unwrap();
        assert_eq!(
            f.format_number(-3.5),
            Rendered::General { value: 3.5, prefix: String::new(), suffix: String::new() }
        );
    }

    #[test]
    fn unsupported_constructs_refuse_to_parse() {
        assert!(NumberFormat::parse("[DBNum1]0").is_none());
        assert!(NumberFormat::parse("0.0.0").is_none());
        assert!(NumberFormat::parse("abc0").is_none());
        assert!(NumberFormat::parse("0;0;0;0;0").is_none());
        // Unquoted currency letters are outside the implemented grammar.
        assert!(NumberFormat::parse("€0.00").is_none());
    }

    #[test]
    fn text_section_applies_to_text_only() {
        let f = NumberFormat::parse("0.00;(0.00);\"-\";\"* \"@\" *\"").unwrap();
        assert_eq!(f.format_text("hi"), Some("* hi *".to_string()));
        let f = NumberFormat::parse("@").unwrap();
        assert_eq!(f.format_text("hi"), Some("hi".to_string()));
        assert_eq!(
            f.format_number(3.5),
            Rendered::General { value: 3.5, prefix: String::new(), suffix: String::new() }
        );
        let f = NumberFormat::parse("0.00").unwrap();
        assert_eq!(f.format_text("hi"), None);
    }

    #[test]
    fn fifteen_digit_rounding_applies_before_formatting() {
        // 0.075 is stored just under 0.075; the percent path must still
        // show 7.5, not 7.4.
        assert_eq!(fmt("0.0%", 0.075), "7.5%");
        assert_eq!(fmt("0", 2.5), "3");
    }
}
