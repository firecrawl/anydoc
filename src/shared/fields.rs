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

/// Fork (sznb): complex-field nesting across paragraphs, to find the
/// paragraphs a `TOC` field's result covers (the service's `content=false`
/// drops them). Feed field events in document order, then call
/// [`TocTracker::finish_paragraph`] at each paragraph end.
#[derive(Default)]
pub struct TocTracker {
    /// Open fields (begin without its end yet).
    depth: usize,
    /// Nesting depth the open TOC field began at.
    toc_at: Option<usize>,
    /// Instruction text of the innermost field still before its separator.
    instr: Option<(usize, String)>,
    /// The current paragraph lies (partly) inside a TOC field.
    touched: bool,
    /// A TOC field began in the current paragraph.
    started: bool,
}

impl TocTracker {
    pub fn begin(&mut self) {
        self.depth += 1;
        self.instr = Some((self.depth, String::new()));
    }

    pub fn instr_text(&mut self, text: &str) {
        if let Some((_, buf)) = self.instr.as_mut() {
            buf.push_str(text);
        }
    }

    pub fn separate(&mut self) {
        self.close_instr();
    }

    pub fn end(&mut self) {
        self.close_instr();
        if self.toc_at == Some(self.depth) {
            self.toc_at = None;
            self.touched = true;
        }
        self.depth = self.depth.saturating_sub(1);
    }

    /// A self-contained field (`w:fldSimple`) with this instruction.
    pub fn simple(&mut self, instr: &str) {
        if is_toc_instr(instr) {
            self.touched = true;
            self.started = true;
        }
    }

    /// True while the instruction of the innermost field is being read.
    pub fn reading_instr(&self) -> bool {
        self.instr.is_some()
    }

    fn close_instr(&mut self) {
        if let Some((depth, instr)) = self.instr.take()
            && self.toc_at.is_none()
            && is_toc_instr(&instr)
        {
            self.toc_at = Some(depth);
            self.touched = true;
            self.started = true;
        }
    }

    /// Paragraph end: `(in_toc, toc_started_here)`; resets for the next one.
    pub fn finish_paragraph(&mut self) -> (bool, bool) {
        let out = (self.touched || self.toc_at.is_some(), self.started);
        self.touched = self.toc_at.is_some();
        self.started = false;
        out
    }
}

/// A `TOC` field instruction (`TOC \o "1-3" \h \z \u`), any case.
pub fn is_toc_instr(instr: &str) -> bool {
    let t = instr.trim_start();
    t.get(..3).is_some_and(|head| head.eq_ignore_ascii_case("TOC"))
        && t[3..].chars().next().is_none_or(|c| c.is_whitespace() || c == '\\')
}

/// The title paragraph Word / WPS put right before a TOC (目录 / 目 录 /
/// Contents / Table of Contents, or a short custom title ending in 目录 such
/// as 「公共应用组件调用目录」). Only ever asked of the block right before a
/// TOC, so the suffix rule cannot reach body text.
pub fn is_toc_title(block: &crate::model::Block) -> bool {
    use crate::model::Block;
    let (Block::Paragraph(inlines) | Block::Heading { content: inlines, .. }) = block else {
        return false;
    };
    let text: String = crate::model::inlines_to_plain_text(inlines)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    matches!(text.as_str(), "目录" | "目錄" | "contents" | "tableofcontents")
        || ((text.ends_with("目录") || text.ends_with("目錄")) && text.chars().count() <= 30)
}

/// Fork (sznb): drop the TOC title — the last non-blank block, when it is
/// one ([`is_toc_title`]). Blank paragraphs between title and TOC (vertical
/// spacing) are looked past and left in place.
pub fn drop_toc_title(blocks: &mut Vec<crate::model::Block>) {
    use crate::model::Block;
    let blank = |b: &Block| matches!(b, Block::Paragraph(inlines) if crate::model::inlines_are_empty(inlines));
    if let Some(i) = blocks.iter().rposition(|b| !blank(b))
        && is_toc_title(&blocks[i])
    {
        blocks.remove(i);
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
