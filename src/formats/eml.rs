//! RFC 5322 / MIME email (`.eml`) -> model blocks.
//!
//! Emits the Subject as an H1 (matching the EPUB title convention), a metadata
//! paragraph of From/To/Cc/Date, then the body. Prefers the `text/plain`
//! alternative; a message carrying only `text/html` errors as unsupported
//! rather than emitting markup as prose.
//!
//! Malformed individual parts are skipped with a log, per the crate-wide
//! recovery policy; only a message with no usable body errors.

use crate::error::ConvertError;
use crate::model::{Block, Document, Inline, Style};
use crate::shared::text::clean_text;
use std::borrow::Cow;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let (headers, body) = split_headers(bytes);
    let headers = Headers::parse(&headers);

    let mut doc = Document::default();

    if let Some(subject) = headers.get("subject") {
        let subject = decode_words(&subject);
        if !subject.trim().is_empty() {
            doc.blocks.push(Block::Heading {
                level: 1,
                anchor: None,
                content: vec![Inline::plain(clean_text(&subject))],
            });
        }
    }

    let meta = metadata_block(&headers);
    if !meta.is_empty() {
        doc.blocks.push(Block::Paragraph(meta));
    }

    let text = extract_text(&headers, body, 0)?;
    for para in text.split("\n\n") {
        let para = para.trim_matches('\n');
        if para.trim().is_empty() {
            continue;
        }
        let mut inlines = Vec::new();
        for (i, line) in para.lines().enumerate() {
            if i > 0 {
                inlines.push(Inline::LineBreak);
            }
            inlines.push(Inline::plain(clean_text(line)));
        }
        if !inlines.is_empty() {
            doc.blocks.push(Block::Paragraph(inlines));
        }
    }

    Ok(doc)
}

/// Bold label + value, one line per header present.
fn metadata_block(headers: &Headers) -> Vec<Inline> {
    let mut out = Vec::new();
    for (label, key) in [("From", "from"), ("To", "to"), ("Cc", "cc"), ("Date", "date")] {
        let Some(raw) = headers.get(key) else { continue };
        let value = decode_words(&raw);
        if value.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Inline::LineBreak);
        }
        out.push(Inline::Text {
            text: format!("{label}: "),
            style: Style { bold: true, ..Style::default() },
        });
        out.push(Inline::plain(clean_text(&value)));
    }
    out
}

/// Split at the first blank line: everything before is the header block.
fn split_headers(bytes: &[u8]) -> (Vec<u8>, &[u8]) {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let rest = &bytes[i + 1..];
            if rest.starts_with(b"\n") {
                return (bytes[..i].to_vec(), &rest[1..]);
            }
            if rest.starts_with(b"\r\n") {
                return (bytes[..i].to_vec(), &rest[2..]);
            }
        }
        i += 1;
    }
    (bytes.to_vec(), &[])
}

struct Headers(Vec<(String, String)>);

impl Headers {
    /// Unfold continuation lines (leading whitespace) into their parent field.
    fn parse(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes);
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some(last) = fields.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(line.trim());
                }
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        Headers(fields)
    }

    fn get(&self, name: &str) -> Option<String> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    /// `Content-Type` parameter, e.g. `boundary` or `charset`.
    fn ct_param(&self, name: &str) -> Option<String> {
        let ct = self.get("content-type")?;
        for part in ct.split(';').skip(1) {
            let (k, v) = part.split_once('=')?;
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().trim_matches('"').to_string());
            }
        }
        None
    }

    fn content_type(&self) -> String {
        self.get("content-type")
            .map(|c| c.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .unwrap_or_else(|| "text/plain".into())
    }
}

const MAX_MIME_DEPTH: usize = 32;

/// Walk the MIME tree, preferring `text/plain`.
fn extract_text(headers: &Headers, body: &[u8], depth: usize) -> Result<String, ConvertError> {
    if depth > MAX_MIME_DEPTH {
        return Err(ConvertError::malformed("MIME nesting exceeds the depth limit"));
    }
    let ct = headers.content_type();

    if ct.starts_with("multipart/") {
        let Some(boundary) = headers.ct_param("boundary") else {
            log::warn!("multipart part has no boundary parameter; treating as flat text");
            return Ok(decode_body(headers, body).into_owned());
        };
        let parts = split_parts(body, &boundary);
        let mut html_seen = false;

        // Prefer text/plain anywhere in this level, then recurse into nested
        // multiparts, matching the alternative-selection rule.
        for pass in 0..2 {
            for part in &parts {
                let (ph, pb) = split_headers(part);
                let ph = Headers::parse(&ph);
                let pct = ph.content_type();
                match pass {
                    0 if pct == "text/plain" => {
                        let s = decode_body(&ph, pb);
                        if !s.trim().is_empty() {
                            return Ok(s.into_owned());
                        }
                    }
                    0 if pct == "text/html" => html_seen = true,
                    1 if pct.starts_with("multipart/") => {
                        if let Ok(s) = extract_text(&ph, pb, depth + 1)
                            && !s.trim().is_empty()
                        {
                            return Ok(s);
                        }
                    }
                    _ => {}
                }
            }
        }
        if html_seen {
            return Err(ConvertError::Unsupported(
                "email body is text/html only; no text/plain alternative".into(),
            ));
        }
        return Err(ConvertError::Unsupported("email has no text body part".into()));
    }

    if ct == "text/html" {
        return Err(ConvertError::Unsupported(
            "email body is text/html only; no text/plain alternative".into(),
        ));
    }

    let s = decode_body(headers, body);
    if s.trim().is_empty() {
        return Err(ConvertError::Unsupported("email has no text body".into()));
    }
    Ok(s.into_owned())
}

/// Split a multipart body on `--boundary` delimiters (anchored at line start).
fn split_parts<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut start: Option<usize> = None;
    let mut pos = 0;

    for line in body.split_inclusive(|&b| b == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
        if trimmed.starts_with(delim.as_bytes()) {
            if let Some(s) = start {
                parts.push(&body[s..pos]);
            }
            // Closing delimiter is `--boundary--`.
            if trimmed.ends_with(b"--") && trimmed.len() > delim.len() {
                return parts;
            }
            start = Some(pos + line.len());
        }
        pos += line.len();
    }
    if let Some(s) = start
        && s < body.len()
    {
        parts.push(&body[s..]);
    }
    parts
}

/// Apply Content-Transfer-Encoding, then the declared charset.
fn decode_body<'a>(headers: &Headers, body: &'a [u8]) -> Cow<'a, str> {
    let cte = headers
        .get("content-transfer-encoding")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();

    let decoded: Cow<'a, [u8]> = match cte.as_str() {
        "quoted-printable" => Cow::Owned(decode_qp(body)),
        "base64" => match decode_base64(body) {
            Some(b) => Cow::Owned(b),
            None => {
                log::warn!("undecodable base64 body part; using raw bytes");
                Cow::Borrowed(body)
            }
        },
        _ => Cow::Borrowed(body),
    };

    let charset = headers.ct_param("charset").unwrap_or_else(|| "utf-8".into());
    let enc = encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    Cow::Owned(enc.decode(&decoded).0.into_owned())
}

/// RFC 2045 quoted-printable: `=XX` hex escapes and `=`-terminated soft breaks.
fn decode_qp(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != b'=' {
            out.push(input[i]);
            i += 1;
            continue;
        }
        match input.get(i + 1) {
            Some(b'\n') => i += 2,
            Some(b'\r') if input.get(i + 2) == Some(&b'\n') => i += 3,
            Some(&h) => match (hex(h), input.get(i + 2).copied().and_then(hex)) {
                (Some(a), Some(b)) => {
                    out.push(a * 16 + b);
                    i += 3;
                }
                _ => {
                    out.push(b'=');
                    i += 1;
                }
            },
            None => {
                out.push(b'=');
                i += 1;
            }
        }
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Standard base64, ignoring whitespace. `None` if the payload is malformed.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in input {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// RFC 2047 encoded-words: `=?charset?B|Q?text?=`.
fn decode_words(input: &str) -> String {
    if !input.contains("=?") {
        return input.to_string();
    }
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("=?") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(decoded) = decode_one_word(after) else {
            out.push_str("=?");
            rest = after;
            continue;
        };
        let (text, consumed) = decoded;
        out.push_str(&text);
        rest = &after[consumed..];
    }
    out.push_str(rest);
    out
}

/// Decode a single `charset?enc?payload?=`, returning the text and bytes consumed.
///
/// The terminator is located *after* the two `?` separators: a payload's own
/// `=XX` escapes make `?=` appear inside the word (`=?UTF-8?Q?=24=33?=`), so
/// searching from the start finds the wrong end.
fn decode_one_word(after: &str) -> Option<(String, usize)> {
    let sep1 = after.find('?')?;
    let sep2 = sep1 + 1 + after[sep1 + 1..].find('?')?;
    let end = sep2 + 1 + after[sep2 + 1..].find("?=")?;
    let word = &after[..end];
    let mut it = word.splitn(3, '?');
    let charset = it.next()?;
    let enc = it.next()?;
    let payload = it.next()?;

    let raw = match enc.to_ascii_uppercase().as_str() {
        "B" => decode_base64(payload.as_bytes())?,
        // In encoded-words `_` stands for a space.
        "Q" => decode_qp(&payload.replace('_', " ").into_bytes()),
        _ => return None,
    };
    let e = encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    Some((e.decode(&raw).0.into_owned(), end + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_text(doc: &Document) -> String {
        doc.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(inlines) => Some(
                    inlines
                        .iter()
                        .map(|i| match i {
                            Inline::Text { text, .. } => text.as_str(),
                            Inline::LineBreak => "\n",
                            _ => "",
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    #[test]
    fn qp_decodes_escapes_and_soft_breaks() {
        assert_eq!(decode_qp(b"a=3Db"), b"a=b");
        assert_eq!(decode_qp(b"long=\nline"), b"longline");
        assert_eq!(decode_qp(b"trailing="), b"trailing=");
    }

    #[test]
    fn base64_rejects_invalid_and_decodes_valid() {
        assert_eq!(decode_base64(b"aGVsbG8=").unwrap(), b"hello");
        assert!(decode_base64(b"not!valid").is_none());
    }

    #[test]
    fn encoded_words_decode_both_schemes() {
        assert_eq!(decode_words("=?UTF-8?B?aGVsbG8=?="), "hello");
        assert_eq!(decode_words("=?UTF-8?Q?caf=C3=A9?="), "café");
        assert_eq!(decode_words("=?UTF-8?Q?a_b?="), "a b");
        assert_eq!(decode_words("plain text"), "plain text");
    }

    #[test]
    fn encoded_word_payload_containing_question_equals() {
        // `=24` etc. put a literal `?=` inside the payload; the terminator
        // search must start past both separators.
        assert_eq!(decode_words("=?UTF-8?Q?=24=33=32?="), "$32");
        assert_eq!(decode_words("a =?UTF-8?Q?=24=33?= b"), "a $3 b");
    }

    #[test]
    fn multipart_alternative_prefers_plain() {
        let eml = b"Subject: Test\r\nFrom: a@example.com\r\n\
Content-Type: multipart/alternative; boundary=\"BB\"\r\n\r\n\
--BB\r\nContent-Type: text/plain\r\n\r\nplain body\r\n\
--BB\r\nContent-Type: text/html\r\n\r\n<p>html body</p>\r\n--BB--\r\n";
        let doc = parse(eml).unwrap();
        let text = body_text(&doc);
        assert!(text.contains("plain body"), "got: {text}");
        assert!(!text.contains("html body"), "html leaked: {text}");
    }

    #[test]
    fn subject_becomes_heading() {
        let doc = parse(b"Subject: Hello\r\n\r\nbody").unwrap();
        assert!(matches!(&doc.blocks[0], Block::Heading { level: 1, .. }));
    }

    #[test]
    fn html_only_errors_rather_than_emitting_markup() {
        let eml = b"Subject: T\r\nContent-Type: multipart/alternative; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: text/html\r\n\r\n<p>only html</p>\r\n--B--\r\n";
        let err = parse(eml).unwrap_err();
        assert!(matches!(err, ConvertError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn folded_headers_unfold() {
        let h = Headers::parse(b"Subject: one\r\n  two\r\nFrom: x@y.z");
        assert_eq!(h.get("subject").unwrap(), "one two");
    }
}
