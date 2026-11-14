//! Shared text normalization.

/// Drop control characters and layout-only invisibles, convert NBSP to a
/// regular space, strip soft hyphens. Line breaks become single spaces; a
/// CRLF pair is one break. U+200C (ZWNJ) and U+200D (ZWJ) are **preserved**:
/// they carry meaning in Arabic/Indic shaping and emoji sequences.
/// Typographic ligatures (ff, fi, fl, ffi, ffl, st) are expanded to their
/// constituent letters so words are not silently corrupted.
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        // Expand typographic ligatures to ASCII equivalents.
        if let Some(expansion) = ligature_expansion(c) {
            out.push_str(expansion);
            continue;
        }
        match c {
            '\u{a0}' => out.push(' '),
            '\u{ad}' | '\u{200b}' | '\u{feff}' => {}
            '\t' => out.push('\t'),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            '\n' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The ASCII expansion for a typographic ligature (U+FB00–U+FB06:
/// ﬀ ﬁ ﬂ ﬃ ﬄ ﬅ ﬆ), or `None` for any other character.
fn ligature_expansion(c: char) -> Option<&'static str> {
    match c {
        '\u{fb00}' => Some("ff"),
        '\u{fb01}' => Some("fi"),
        '\u{fb02}' => Some("fl"),
        '\u{fb03}' => Some("ffi"),
        '\u{fb04}' => Some("ffl"),
        '\u{fb05}' | '\u{fb06}' => Some("st"),
        _ => None,
    }
}

/// Expand typographic ligatures to their constituent ASCII letters.
///
/// The PDF extraction path uses this directly instead of [`clean_text`]:
/// PDF Markdown must keep its line breaks, which `clean_text` collapses.
pub(crate) fn expand_ligatures(text: &str) -> String {
    // `str::contains` takes a `Pattern`, and `RangeInclusive<char>` only
    // implements `Pattern` on nightly, so test chars explicitly. Membership
    // comes from `ligature_expansion` so the fast path cannot drift from
    // the expansion table.
    if !text.chars().any(|c| ligature_expansion(c).is_some()) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match ligature_expansion(c) {
            Some(expansion) => out.push_str(expansion),
            None => out.push(c),
        }
    }
    out
}

/// Collapse whitespace runs to single spaces.
pub fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{clean_text, expand_ligatures};

    #[test]
    fn join_controls_preserved() {
        assert_eq!(clean_text("می\u{200c}خواهم"), "می\u{200c}خواهم");
        assert_eq!(clean_text("👨\u{200d}👩\u{200d}👧"), "👨\u{200d}👩\u{200d}👧");
    }

    #[test]
    fn layout_invisibles_stripped() {
        assert_eq!(clean_text("a\u{ad}b\u{200b}c\u{feff}d\u{a0}e"), "abcd e");
    }

    #[test]
    fn ligatures_expanded() {
        assert_eq!(clean_text("classi\u{fb01}es"), "classifies");
        assert_eq!(clean_text("a\u{fb00}ected"), "affected");
        assert_eq!(clean_text("de\u{fb02}ect"), "deflect");
        assert_eq!(clean_text("di\u{fb03}cult"), "difficult");
        assert_eq!(clean_text("wa\u{fb04}e"), "waffle");
        assert_eq!(clean_text("fa\u{fb05}"), "fast");
        assert_eq!(clean_text("ju\u{fb06}"), "just");
    }

    #[test]
    fn expand_ligatures_pdf_path() {
        assert_eq!(
            expand_ligatures("\u{fb00}\u{fb01}\u{fb02}\u{fb03}\u{fb04}\u{fb05}\u{fb06}"),
            "fffiflffifflstst"
        );
        // Unlike clean_text, the PDF path must keep line breaks.
        assert_eq!(expand_ligatures("a\nb\rc"), "a\nb\rc");
        // Fast path: text without ligatures is returned unchanged.
        assert_eq!(expand_ligatures("plain text"), "plain text");
    }
}
