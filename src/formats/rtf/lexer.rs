//! Position-explicit RTF lexer. `\binN` consumes exactly N raw bytes, so
//! binary payloads can never corrupt group state; unbalanced groups are the
//! caller's to recover (deliberately, with a log).

pub enum Token<'a> {
    Open,
    Close,
    /// Control word with optional numeric parameter.
    Word {
        name: &'a str,
        param: Option<i32>,
    },
    /// Control symbol (`\~`, `\*`, `\{`, ...).
    Symbol(u8),
    /// `\'xx` hex-escaped byte in the current code page.
    Hex(u8),
    /// One plain text byte.
    Byte(u8),
    /// The raw payload of a `\binN` control.
    Bin(&'a [u8]),
}

pub struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Lexer { bytes, pos: 0 }
    }

    pub fn next_token(&mut self) -> Option<Token<'a>> {
        loop {
            let b = *self.bytes.get(self.pos)?;
            self.pos += 1;
            return Some(match b {
                b'{' => Token::Open,
                b'}' => Token::Close,
                b'\\' => self.control()?,
                b'\r' | b'\n' | b'\0' => continue,
                b => Token::Byte(b),
            });
        }
    }

    fn control(&mut self) -> Option<Token<'a>> {
        let b = *self.bytes.get(self.pos)?;
        if !b.is_ascii_alphabetic() {
            self.pos += 1;
            if b == b'\'' {
                let hi = (*self.bytes.get(self.pos)? as char).to_digit(16);
                let lo = (*self.bytes.get(self.pos + 1)? as char).to_digit(16);
                return match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        self.pos += 2;
                        Some(Token::Hex((hi * 16 + lo) as u8))
                    }
                    // Truncated escape: recover by treating it as literal.
                    _ => Some(Token::Byte(b'\'')),
                };
            }
            // An escaped CR or LF is a paragraph mark: the spec makes `\`
            // followed by a newline equivalent to `\par` (Cocoa's writer
            // ends every body paragraph this way). Lexing it as the control
            // word keeps every consumer's `par` handling in one place; the
            // LF of a `\<CR><LF>` pair is then plain-text whitespace, which
            // `next_token` already skips.
            if b == b'\r' || b == b'\n' {
                return Some(Token::Word { name: "par", param: None });
            }
            return Some(Token::Symbol(b));
        }
        let start = self.pos;
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_alphabetic) {
            self.pos += 1;
        }
        let name = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        let mut param: Option<i32> = None;
        let mut negative = false;
        if self.bytes.get(self.pos) == Some(&b'-') {
            negative = true;
            self.pos += 1;
        }
        let num_start = self.pos;
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        if self.pos > num_start {
            if let Ok(n) =
                std::str::from_utf8(&self.bytes[num_start..self.pos]).unwrap_or("0").parse::<i64>()
            {
                let n = if negative { -n } else { n };
                param = Some(n.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
            }
        } else if negative {
            self.pos -= 1;
        }
        // One space after a control word is part of the control.
        if self.bytes.get(self.pos) == Some(&b' ') {
            self.pos += 1;
        }
        if name == "bin" {
            let n = param.unwrap_or(0).max(0) as usize;
            let end = (self.pos + n).min(self.bytes.len());
            let payload = &self.bytes[self.pos..end];
            self.pos = end;
            return Some(Token::Bin(payload));
        }
        Some(Token::Word { name, param })
    }
}

/// Extract the balanced content of destination groups named `name`
/// (`{\name ...}` or `{\*\name ...}`), bin-aware. Returns the byte ranges of
/// the group bodies including the destination word.
pub fn destination_groups<'a>(bytes: &'a [u8], name: &str) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut lexer = Lexer::new(bytes);
    // Track group starts; when a group's first control word (skipping `\*`)
    // matches, remember its start depth and capture until it closes.
    let mut depth = 0usize;
    let mut expecting_word_at: Option<usize> = None;
    let mut capture: Option<(usize, usize)> = None; // (depth, start_pos)
    loop {
        let before = lexer.pos;
        let Some(token) = lexer.next_token() else { break };
        match token {
            Token::Open => {
                depth += 1;
                expecting_word_at = Some(depth);
            }
            Token::Close => {
                if let Some((d, start)) = capture
                    && depth == d
                {
                    out.push(&bytes[start..before]);
                    capture = None;
                }
                depth = depth.saturating_sub(1);
                expecting_word_at = None;
            }
            Token::Symbol(b'*') if expecting_word_at == Some(depth) => {}
            Token::Word { name: w, .. } => {
                if expecting_word_at == Some(depth) && w == name && capture.is_none() {
                    capture = Some((depth, lexer.pos));
                }
                expecting_word_at = None;
            }
            _ => expecting_word_at = None,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_payload_consumed_raw() {
        // Payload bytes are braces and backslashes; they must not lex.
        let src = br"{\rtf1 a\bin5 }}{\\x b}";
        let mut lexer = Lexer::new(src);
        let mut bin: Vec<u8> = Vec::new();
        let mut opens = 0;
        let mut closes = 0;
        while let Some(t) = lexer.next_token() {
            match t {
                Token::Bin(p) => bin.extend_from_slice(p),
                Token::Open => opens += 1,
                Token::Close => closes += 1,
                _ => {}
            }
        }
        assert_eq!(bin, br"}}{\\");
        assert_eq!((opens, closes), (1, 1));
    }

    #[test]
    fn escaped_newline_lexes_as_par() {
        // `\<LF>`, `\<CR><LF>` and `\<CR>` are each one paragraph mark.
        let src = b"{\\rtf1 Alpha\\\nBeta\\\r\nGamma\\\rDelta}";
        let mut lexer = Lexer::new(src);
        let (mut pars, mut text) = (0, Vec::new());
        while let Some(t) = lexer.next_token() {
            match t {
                Token::Word { name: "par", .. } => pars += 1,
                Token::Byte(b) => text.push(b),
                _ => {}
            }
        }
        assert_eq!(pars, 3);
        assert_eq!(text, b"AlphaBetaGammaDelta");
    }

    #[test]
    fn destination_extraction() {
        let src = br"{\rtf1{\*\listtable{\list\listid5}}{\fonttbl{\f0 Arial;}} body}";
        let lists = destination_groups(src, "listtable");
        assert_eq!(lists.len(), 1);
        assert!(lists[0].starts_with(br"{\list"));
        let fonts = destination_groups(src, "fonttbl");
        assert_eq!(fonts.len(), 1);
    }
}
