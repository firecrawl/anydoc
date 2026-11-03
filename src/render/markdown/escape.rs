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
#[allow(dead_code)] // fork: positional fields survive for upstream callers
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
    /// The rest of the line starts with a non-whitespace character that is
    /// not active markup (a hard break's backslash, an anchor tag), so a
    /// run-final delimiter can still be left-flanking.
    pub trailing_nonspace: bool,
    /// Delimiters that later runs on the same rendered line will emit; a
    /// delimiter in this run can pair with one of them across the run seam.
    pub trailing_delims: Delims,
    /// Inside a link label / image alt, where an unmatched `]` (or `[`)
    /// would terminate the label early.
    pub in_label: bool,
}

/// Set of pairable delimiter characters (`*` `_` `~` `` ` `` `]` `$`).
#[derive(Clone, Copy, Default)]
pub(crate) struct Delims([bool; 6]);

impl Delims {
    fn slot(c: char) -> Option<usize> {
        match c {
            '*' => Some(0),
            '_' => Some(1),
            '~' => Some(2),
            '`' => Some(3),
            ']' => Some(4),
            '$' => Some(5),
            _ => None,
        }
    }

    pub(crate) fn insert(&mut self, c: char) {
        if let Some(slot) = Self::slot(c) {
            self.0[slot] = true;
        }
    }

    /// Record the partners `text` contributes when emitted as document
    /// text: every backtick and `]`, plus the emphasis delimiters that
    /// can close.
    pub(crate) fn insert_closers(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut j = 0;
        while j < chars.len() {
            let end = run_end(&chars, j);
            if let Some(slot) = partner_slot(&chars, j, end) {
                self.0[slot] = true;
            }
            j = end;
        }
    }

    pub(crate) fn union(&mut self, other: Delims) {
        for (slot, set) in self.0.iter_mut().zip(other.0) {
            *slot |= set;
        }
    }

    fn contains(self, slot: usize) -> bool {
        self.0[slot]
    }
}

/// End of the run of identical characters starting at `j`.
fn run_end(chars: &[char], j: usize) -> usize {
    let mut end = j + 1;
    while end < chars.len() && chars[end] == chars[j] {
        end += 1;
    }
    end
}

/// Slot for the delimiter run `j..end` when it can act as a pairing partner.
/// Backticks and `]` always can: code spans pair by backtick-string length
/// (even a backslash-escaped backtick still closes one) and brackets pair
/// as link structure. `*`, `_` and `~` pair by flanking, and `$` closes
/// math only after a non-space and before a non-digit, so those count only
/// where they can close.
fn partner_slot(chars: &[char], j: usize, end: usize) -> Option<usize> {
    let slot = Delims::slot(chars[j])?;
    let closes = match chars[j] {
        '`' | ']' => true,
        '$' => can_close_math(chars, j, end),
        _ => can_close(chars, j, end),
    };
    closes.then_some(slot)
}

/// Whether the `$` run `j..end` could close a math span: not preceded by
/// whitespace, not followed by a digit. Unknown neighbours at the edges
/// assume the worst.
fn can_close_math(chars: &[char], j: usize, end: usize) -> bool {
    let prev = j.checked_sub(1).map(|p| chars[p]);
    let next = chars.get(end).copied();
    !prev.is_some_and(char::is_whitespace) && !next.is_some_and(|n| n.is_ascii_digit())
}

/// Whether the emphasis or strikethrough run `j..end` could close a pair:
/// approximate right-flanking (not preceded by whitespace, nor preceded by
/// punctuation with a word character after), plus the intraword exclusion
/// for `_`. Unknown neighbours at the edges assume the worst; the
/// punctuation test stays ASCII so an unclassified character never
/// suppresses a genuine closer.
fn can_close(chars: &[char], j: usize, end: usize) -> bool {
    let prev = j.checked_sub(1).map(|p| chars[p]);
    let next = chars.get(end).copied();
    if prev.is_some_and(char::is_whitespace) {
        return false;
    }
    if prev.is_some_and(|p| p.is_ascii_punctuation()) && next.is_some_and(char::is_alphanumeric) {
        return false;
    }
    chars[j] != '_'
        || !(prev.is_some_and(char::is_alphanumeric) && next.is_some_and(char::is_alphanumeric))
}

/// Escape Markdown syntax in document text.
pub(crate) fn escape_text(text: &str, ctx: InlineContext, opts: EscapeOpts) -> String {
    // Fork (sznb): document text renders verbatim — no Markdown escaping —
    // matching the original pipeline's raw-text output (`1.` list-looking
    // lines, `12*8GB` formulas, `C:\path` backslashes, `#`-starting lines all
    // stay exactly as written). Escaping survives only where a bare
    // character would break structure the renderer itself emits:
    // - a `|` inside a GFM table cell would split the cell;
    // - a `]` inside a link label would terminate it;
    // - pairing delimiters inside a styled span would close it early.
    let styled = opts.styled;
    let in_label = opts.in_label;
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        let escape = match c {
            '|' if ctx == InlineContext::TableCell => true,
            ']' if in_label => true,
            '*' | '~' | '`' if styled => true,
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// True when the rest of the current line is just `c`, spaces, and tabs
/// (a setext underline or thematic break).
#[allow(dead_code)] // fork: kept for upstream parity
fn line_is_only(chars: &[char], c: char) -> bool {
    chars.iter().take_while(|&&ch| ch != '\n').all(|&ch| ch == c || ch == ' ' || ch == '\t')
}

#[allow(dead_code)] // fork: kept for upstream parity
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
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut escaped = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            '<' => escaped.push_str("%3C"),
            '>' => escaped.push_str("%3E"),
            // Raw pipes split GFM table cells.
            '|' => escaped.push_str("%7C"),
            // Encode controls so they cannot split the Markdown output.
            c if c.is_control() => {
                let mut bytes = [0; 4];
                for byte in c.encode_utf8(&mut bytes).bytes() {
                    escaped.push('%');
                    escaped.push(HEX[(byte >> 4) as usize] as char);
                    escaped.push(HEX[(byte & 0x0F) as usize] as char);
                }
            }
            c => escaped.push(c),
        }
    }
    if escaped.chars().any(|c| c.is_whitespace() || c == '(' || c == ')') {
        format!("<{escaped}>")
    } else {
        escaped
    }
}

#[allow(dead_code)]
pub(crate) fn escape_url_as_text(url: &str, ctx: InlineContext) -> String {
    let cleaned: String = url.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    escape_text(
        &cleaned,
        ctx,
        EscapeOpts { trailing_active: true, in_label: true, ..Default::default() },
    )
}

/// Prepare a code span's text for a table cell, where a pipe is the only
/// character between the fences that is still syntax.
///
/// A backslash run already sitting in front of a pipe would pair off with the
/// escape and leave the pipe bare, so it is doubled to keep the escape intact.
/// That doubling survives into the rendered code span: GFM has no encoding for
/// a code span that contains a backslash immediately before a pipe, and an
/// intact row is worth more than the exact backslash count.
pub(crate) fn escape_cell_code_span(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut backslashes = 0;
    for c in text.chars() {
        match c {
            '|' => {
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                backslashes = 0;
            }
            '\\' => backslashes += 1,
            _ => backslashes = 0,
        }
        out.push(c);
    }
    out
}

/// Shortest backtick fence longer than any backtick run in `text`.
pub(crate) fn backtick_fence(text: &str, min: usize) -> String {
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat((longest_run + 1).max(min))
}
