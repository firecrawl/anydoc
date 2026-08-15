//! Word field-instruction parsing (DOC, DOCX, RTF), via a small
//! case-insensitive lexer with known switch arities.

use crate::model::{Inline, LinkTarget, inlines_are_empty};

/// Field accumulator: instruction text before the separator, result after.
#[derive(Default)]
pub struct FieldFrame {
    pub instr: String,
    pub in_result: bool,
    pub inlines: Vec<Inline>,
}

/// Finish a field: wrap the result in a link when the instruction is a
/// hyperlink, otherwise pass the result content through.
pub fn field_result(instr: &str, content: Vec<Inline>) -> Vec<Inline> {
    match hyperlink_target(instr) {
        Some(target) if !inlines_are_empty(&content) => {
            vec![Inline::Link { content, target }]
        }
        _ => content,
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Switch(char),
}

/// Tokenize a field instruction: quoted strings (with `\"` and `\\`
/// escapes), backslash switches, bare words.
fn tokenize(instr: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = instr.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some(esc @ ('"' | '\\')) => word.push(esc),
                        Some(other) => {
                            word.push('\\');
                            word.push(other);
                        }
                        None => break,
                    },
                    c => word.push(c),
                }
            }
            out.push(Token::Word(word));
        } else if c == '\\' {
            chars.next();
            if let Some(&switch) = chars.peek() {
                chars.next();
                out.push(Token::Switch(switch.to_ascii_lowercase()));
            }
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
            out.push(Token::Word(word));
        }
    }
    out
}

/// Switches of the HYPERLINK field that take an argument.
fn hyperlink_switch_takes_arg(switch: char) -> bool {
    matches!(switch, 'l' | 'o' | 't')
}

/// Interpret a HYPERLINK field instruction as a link target.
pub fn hyperlink_target(instr: &str) -> Option<LinkTarget> {
    let mut tokens = tokenize(instr).into_iter().peekable();
    match tokens.next() {
        Some(Token::Word(w)) if w.eq_ignore_ascii_case("HYPERLINK") => {}
        _ => return None,
    }
    let mut url: Option<String> = None;
    let mut anchor: Option<String> = None;
    while let Some(token) = tokens.next() {
        match token {
            Token::Word(w) => {
                if url.is_none() && !w.trim().is_empty() {
                    url = Some(w.trim().to_string());
                }
            }
            Token::Switch(s) => {
                // A switch's argument is the next token only when it is a
                // word; a following switch means the argument was omitted.
                let arg = if hyperlink_switch_takes_arg(s)
                    && matches!(tokens.peek(), Some(Token::Word(_)))
                {
                    match tokens.next() {
                        Some(Token::Word(w)) => Some(w),
                        _ => None,
                    }
                } else {
                    None
                };
                if s == 'l'
                    && let Some(a) = arg
                    && !a.trim().is_empty()
                {
                    anchor = Some(a.trim().to_string());
                }
            }
        }
    }
    match (url, anchor) {
        (Some(url), Some(frag)) => Some(classify(format!("{url}#{frag}"))),
        (Some(url), None) => Some(classify(url)),
        (None, Some(frag)) => Some(LinkTarget::Anchor(frag)),
        (None, None) => None,
    }
}

/// Classify an OPC relationship target as a link target. External-mode
/// targets are parsed like hyperlink field targets (scheme detection);
/// internal-mode targets stay package-relative.
pub fn classify_rel_target(external: bool, target: &str) -> LinkTarget {
    if external { classify(target.to_string()) } else { LinkTarget::Relative(target.to_string()) }
}

fn classify(url: String) -> LinkTarget {
    if let Some(anchor) = url.strip_prefix('#') {
        return LinkTarget::Anchor(anchor.to_string());
    }
    if crate::shared::uri::is_absolute_uri(&url) {
        LinkTarget::External(url)
    } else {
        LinkTarget::Relative(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_url() {
        assert_eq!(
            hyperlink_target(r#" HYPERLINK "https://e.com/a b" "#),
            Some(LinkTarget::External("https://e.com/a b".into()))
        );
    }

    #[test]
    fn case_insensitive_keyword() {
        assert_eq!(
            hyperlink_target(r#"hyperlink "https://e.com""#),
            Some(LinkTarget::External("https://e.com".into()))
        );
    }

    #[test]
    fn anchor_only() {
        assert_eq!(
            hyperlink_target(r#"HYPERLINK \l "sec2""#),
            Some(LinkTarget::Anchor("sec2".into()))
        );
    }

    #[test]
    fn url_plus_anchor() {
        assert_eq!(
            hyperlink_target(r#"HYPERLINK "https://e.com/p" \l "frag""#),
            Some(LinkTarget::External("https://e.com/p#frag".into()))
        );
    }

    #[test]
    fn switch_with_argument_not_mistaken_for_url() {
        assert_eq!(
            hyperlink_target(r#"HYPERLINK \o "tooltip text" "https://e.com""#),
            Some(LinkTarget::External("https://e.com".into()))
        );
    }

    #[test]
    fn escaped_quotes_in_target() {
        assert_eq!(
            hyperlink_target(r#"HYPERLINK "https://e.com/\"q\"""#),
            Some(LinkTarget::External(r#"https://e.com/"q""#.into()))
        );
    }

    #[test]
    fn relative_target() {
        assert_eq!(
            hyperlink_target(r#"HYPERLINK "docs/readme.docx""#),
            Some(LinkTarget::Relative("docs/readme.docx".into()))
        );
    }

    #[test]
    fn argless_switch_does_not_swallow_following_switch() {
        assert_eq!(
            hyperlink_target(r#"HYPERLINK "https://e.com/p" \o \l "frag""#),
            Some(LinkTarget::External("https://e.com/p#frag".into()))
        );
    }

    #[test]
    fn non_hyperlink_fields_ignored() {
        assert_eq!(hyperlink_target("PAGEREF _Toc123 \\h"), None);
    }
}
