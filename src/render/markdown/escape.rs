//! Context-sensitive minimal escaping: document text is escaped only where a
//! character could actually parse as Markdown syntax in its context.

/// Where an inline run is being rendered; controls which characters can be
/// syntax there.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum InlineContext {
    Block,
    Heading,
    TableCell,
}

/// Fine-grained escaping context beyond [`InlineContext`]: where the run
/// sits relative to its surroundings.
#[derive(Clone, Copy, Default)]
pub(crate) struct EscapeOpts {
    /// The run begins at the start of an output line, where block syntax
    /// (headings, list markers, setext underlines) could form.
    pub at_line_start: bool,
    /// The run is wrapped in emphasis delimiters, so delimiter characters
    /// inside it always need escaping.
    pub styled: bool,
    /// The character following the run is unknown or active markup; pairable
    /// delimiters must assume the worst.
    pub trailing_active: bool,
    /// More of the same paragraph follows this run. A delimiter is left
    /// unescaped when nothing in the run pairs with it, but Markdown pairs
    /// across the whole paragraph, not one run: the partner can arrive in a
    /// later run, with a line break or an anchor in between.
    pub pairs_ahead: bool,
    /// Inside a link label / image alt, where an unmatched `]` (or `[`)
    /// would terminate the label early.
    pub in_label: bool,
}

/// Escape Markdown syntax in document text.
pub(crate) fn escape_text(text: &str, ctx: InlineContext, opts: EscapeOpts) -> String {
    let EscapeOpts { at_line_start, styled, trailing_active, pairs_ahead, in_label } = opts;
    let chars: Vec<char> = text.chars().collect();
    // Last position of each pairable delimiter; a lone one is inert.
    let mut last: [Option<usize>; 5] = [None; 5]; // * _ ~ ` ]
    for (j, &c) in chars.iter().enumerate() {
        match c {
            '*' => last[0] = Some(j),
            '_' => last[1] = Some(j),
            '~' => last[2] = Some(j),
            '`' => last[3] = Some(j),
            ']' => last[4] = Some(j),
            _ => {}
        }
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut line_has_content = !(at_line_start && ctx == InlineContext::Block);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            out.push('\n');
            if ctx == InlineContext::Block {
                line_has_content = false;
            }
            i += 1;
            continue;
        }
        let start_of_line = !line_has_content;
        if !c.is_whitespace() {
            line_has_content = true;
        }
        let next = chars.get(i + 1).copied();
        // At the run's end the next character is unknown; trailing_active assumes the worst.
        let next_nonspace = next.map_or(trailing_active, |n| !n.is_whitespace());
        let paired =
            |slot: usize| trailing_active || pairs_ahead || last[slot].is_some_and(|j| j > i);
        let escape = match c {
            '\\' => true,
            ']' if in_label => true,
            '`' => styled || paired(3),
            '*' => styled || start_of_line || (next_nonspace && paired(0)),
            '_' => {
                let prev_alnum = i > 0 && chars[i - 1].is_alphanumeric();
                let next_alnum = next.is_some_and(char::is_alphanumeric);
                styled || (next_nonspace && !(prev_alnum && next_alnum) && paired(1))
            }
            '~' => styled || (next_nonspace && paired(2)),
            '[' => in_label || paired(4),
            '<' => next.is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')),
            '!' => next.is_none() && trailing_active,
            '|' if ctx == InlineContext::TableCell => true,
            '&' if entity_ahead(&chars[i..]) => {
                out.push_str("&amp;");
                i += 1;
                continue;
            }
            '#' if start_of_line => {
                let j = (i..chars.len()).find(|&j| chars[j] != '#').unwrap_or(chars.len());
                chars.get(j).is_none_or(|n| n.is_whitespace())
            }
            '-' if start_of_line => !next_nonspace || line_is_only(&chars[i..], '-'),
            '+' if start_of_line => !next_nonspace,
            '>' if start_of_line => true,
            '=' if start_of_line => line_is_only(&chars[i..], '='),
            '0'..='9' if start_of_line => {
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j < chars.len()
                    && (chars[j] == '.' || chars[j] == ')')
                    && chars.get(j + 1).is_none_or(|n| n.is_whitespace())
                {
                    out.extend(&chars[i..j]);
                    out.push('\\');
                    out.push(chars[j]);
                    i = j + 1;
                    continue;
                }
                false
            }
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        i += 1;
    }
    out
}

/// True when the rest of the current line is just `c`, spaces, and tabs
/// (a setext underline or thematic break).
fn line_is_only(chars: &[char], c: char) -> bool {
    chars.iter().take_while(|&&ch| ch != '\n').all(|&ch| ch == c || ch == ' ' || ch == '\t')
}

fn entity_ahead(chars: &[char]) -> bool {
    let mut i = 1;
    if i < chars.len() && chars[i] == '#' {
        return true;
    }
    let mut seen = 0;
    while i < chars.len() && chars[i].is_ascii_alphanumeric() {
        i += 1;
        seen += 1;
    }
    seen > 0 && i < chars.len() && chars[i] == ';'
}

/// Format a link destination, angle-bracketing when needed.
pub(crate) fn format_url(url: &str) -> String {
    if url.chars().any(|c| c.is_whitespace() || c == '(' || c == ')' || c == '<' || c == '>') {
        let escaped: String = url
            .chars()
            .map(|c| match c {
                '<' => "%3C".to_string(),
                '>' => "%3E".to_string(),
                '\n' | '\r' => String::new(),
                c => c.to_string(),
            })
            .collect();
        format!("<{escaped}>")
    } else {
        url.to_string()
    }
}

pub(crate) fn escape_url_as_text(url: &str) -> String {
    url.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]")
}

/// Shortest backtick fence longer than any backtick run in `text`.
pub(crate) fn backtick_fence(text: &str, min: usize) -> String {
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat((longest_run + 1).max(min))
}
