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
            // A reader treats `\` before CR or LF as a paragraph mark; the
            // trailing LF of a CRLF pair is skipped as plain-text whitespace.
            if b == b'\r' || b == b'\n' {
                return Some(Token::Word { name: "par", param: None });
            }
            if b == b'\'' {
                let pair = self.bytes.get(self.pos..)?.get(..2)?;
                let hi = (pair[0] as char).to_digit(16);
                let lo = (pair[1] as char).to_digit(16);
                return match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        self.pos += 2;
                        Some(Token::Hex((hi * 16 + lo) as u8))
                    }
                    // Truncated escape: recover by treating it as literal.
                    _ => Some(Token::Byte(b'\'')),
                };
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
            let end = self.pos.saturating_add(n).min(self.bytes.len());
            let payload = &self.bytes[self.pos..end];
            self.pos = end;
            return Some(Token::Bin(payload));
        }
        Some(Token::Word { name, param })
    }
}

/// The prelude tables and codepage, extracted in one lexer pass.
pub struct PreludeScan<'a> {
    pub fonttbl: Vec<&'a [u8]>,
    pub stylesheet: Vec<&'a [u8]>,
    pub listtable: Vec<&'a [u8]>,
    pub listoverridetable: Vec<&'a [u8]>,
    pub codepage: Option<i32>,
}

/// Which prelude destination an open group captures into.
#[derive(Clone, Copy)]
enum Dest {
    Fonts,
    Styles,
    Lists,
    Overrides,
}

/// Capture the prelude destination groups and the header `\ansicpg` value.
pub fn scan_prelude(bytes: &[u8]) -> PreludeScan<'_> {
    let mut scan = PreludeScan {
        fonttbl: Vec::new(),
        stylesheet: Vec::new(),
        listtable: Vec::new(),
        listoverridetable: Vec::new(),
        codepage: None,
    };
    let mut lexer = Lexer::new(bytes);
    // A group whose first control word (skipping `\*`) names a prelude
    // table is captured from after that word until its group closes.
    let mut depth = 0usize;
    let mut expecting_word_at: Option<usize> = None;
    let mut capture: Option<(usize, usize, Dest)> = None; // (depth, start, dest)
    loop {
        let before = lexer.pos;
        let Some(token) = lexer.next_token() else { break };
        match token {
            Token::Open => {
                depth += 1;
                expecting_word_at = Some(depth);
            }
            Token::Close => {
                if let Some((d, start, dest)) = capture
                    && depth == d
                {
                    let range = &bytes[start..before];
                    match dest {
                        Dest::Fonts => scan.fonttbl.push(range),
                        Dest::Styles => scan.stylesheet.push(range),
                        Dest::Lists => scan.listtable.push(range),
                        Dest::Overrides => scan.listoverridetable.push(range),
                    }
                    capture = None;
                }
                depth = depth.saturating_sub(1);
                expecting_word_at = None;
            }
            Token::Symbol(b'*') if expecting_word_at == Some(depth) => {}
            Token::Word { name, param } => {
                if name == "ansicpg" && scan.codepage.is_none() {
                    scan.codepage = param;
                }
                if expecting_word_at == Some(depth) && capture.is_none() {
                    match name {
                        "fonttbl" => capture = Some((depth, lexer.pos, Dest::Fonts)),
                        "stylesheet" => capture = Some((depth, lexer.pos, Dest::Styles)),
                        "listtable" => capture = Some((depth, lexer.pos, Dest::Lists)),
                        "listoverridetable" => capture = Some((depth, lexer.pos, Dest::Overrides)),
                        _ => {}
                    }
                }
                expecting_word_at = None;
            }
            _ => expecting_word_at = None,
        }
    }
    scan
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
    fn backslash_before_a_line_break_is_one_paragraph_mark() {
        for src in [b"{\\rtf1 a\\\nb}".as_slice(), b"{\\rtf1 a\\\r\nb}", b"{\\rtf1 a\\\rb}"] {
            let mut lexer = Lexer::new(src);
            let mut pars = 0;
            let mut symbols = 0;
            while let Some(t) = lexer.next_token() {
                match t {
                    Token::Word { name: "par", param: None } => pars += 1,
                    Token::Symbol(_) => symbols += 1,
                    _ => {}
                }
            }
            assert_eq!((pars, symbols), (1, 0), "source: {:?}", String::from_utf8_lossy(src));
        }
    }

    #[test]
    fn prelude_scan_finds_destinations_and_codepage() {
        let src = br"{\rtf1\ansicpg1252{\*\listtable{\list\listid5}}{\fonttbl{\f0 Arial;}} body}";
        let scan = scan_prelude(src);
        assert_eq!(scan.codepage, Some(1252));
        assert_eq!(scan.listtable.len(), 1);
        assert!(scan.listtable[0].starts_with(br"{\list"));
        let fonts = scan.fonttbl;
        assert_eq!(fonts.len(), 1);
        assert!(fonts[0].starts_with(br"{\f0"));
    }

    #[test]
    fn prelude_scan_finds_styles_and_overrides() {
        let src =
            br"{\rtf1{\stylesheet{\s0 Normal;}}{\*\listoverridetable{\listoverride\listid1\ls1}}}";
        let scan = scan_prelude(src);
        let styles = scan.stylesheet;
        assert_eq!(styles.len(), 1);
        assert!(styles[0].starts_with(br"{\s0"));
        let overrides = scan.listoverridetable;
        assert_eq!(overrides.len(), 1);
        assert!(overrides[0].starts_with(br"{\listoverride"));
    }
}
