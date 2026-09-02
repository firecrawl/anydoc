//! Outlook message files (`.msg`): MAPI properties in an OLE compound file.
//!
//! A `.msg` is not MIME. Each property sits in its own stream named
//! `__substg1.0_<tag><type>`, where `<tag>` is the MAPI property id and
//! `<type>` its PT_ code, both four uppercase hex digits ([MS-OXMSG]).
//! Strings arrive either as UTF-16LE (`001F`) or in the message code page
//! (`001E`), which the code page properties name.
//!
//! Only the message itself is read: the envelope becomes a heading and a
//! metadata paragraph, and the plain-text body becomes the prose. Recipient
//! and attachment storages are not traversed, because `PR_DISPLAY_TO`
//! already carries the addressees as the sending client rendered them.

use crate::error::ConvertError;
use crate::model::{Block, Document, Inline, Style};
use crate::shared::binary::read_ole_stream;
use crate::shared::text::clean_text;
use std::io::Cursor;

/// MAPI property ids, as the substorage stream names spell them.
mod tag {
    /// `PR_SUBJECT`.
    pub const SUBJECT: u16 = 0x0037;
    /// `PR_BODY`, the plain-text body.
    pub const BODY: u16 = 0x1000;
    /// `PR_SENDER_NAME`.
    pub const SENDER_NAME: u16 = 0x0C1A;
    /// `PR_SENDER_EMAIL_ADDRESS`.
    pub const SENDER_EMAIL: u16 = 0x0C1F;
    /// `PR_DISPLAY_TO`, the addressees as the sending client rendered them.
    pub const DISPLAY_TO: u16 = 0x0E04;
    /// `PR_DISPLAY_CC`.
    pub const DISPLAY_CC: u16 = 0x0E03;
    /// `PR_MESSAGE_CODEPAGE`, the code page `001E` strings are written in.
    pub const MESSAGE_CODEPAGE: u16 = 0x3FFD;
    /// `PR_INTERNET_CPID`, the same thing by another name; producers set one
    /// or the other.
    pub const INTERNET_CPID: u16 = 0x3FDE;
}

/// PT_UNICODE: UTF-16LE, and what any current producer writes.
const PT_UNICODE: u16 = 0x001F;
/// PT_STRING8: bytes in the message code page.
const PT_STRING8: u16 = 0x001E;
/// PT_LONG: a 32-bit value, stored in the property store rather than a
/// stream of its own.
const PT_LONG: u16 = 0x0003;

/// The property store, which carries every fixed-width value.
const PROPERTIES_STREAM: &str = "__properties_version1.0";
/// Bytes of `__properties_version1.0` header before the entries begin, for a
/// top-level message ([MS-OXMSG] 2.4.1.1): eight reserved, the next
/// recipient and attachment ids, their two counts, then eight more reserved.
const PROPERTIES_HEADER: usize = 32;
/// Each fixed-width entry: tag, flags, then eight bytes of value.
const PROPERTY_ENTRY: usize = 16;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let mut ole = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| ConvertError::malformed(format!("not an OLE2 compound file: {e}")))?;

    // Which property streams exist is known only by listing: a message
    // writes a stream per property it carries and omits the rest entirely.
    let mut streams: Vec<String> = Vec::new();
    for entry in ole.read_root_storage() {
        if entry.is_stream() {
            streams.push(entry.name().to_string());
        }
    }

    // The code page is a PT_LONG, so it is not a stream of its own: fixed
    // width values live packed in the property store, and reading them is
    // the only way to decode a `PT_STRING8` as anything but a guess.
    let properties = match read_ole_stream(&mut ole, PROPERTIES_STREAM) {
        Ok(bytes) => bytes,
        Err(e) if e.is_fatal() => return Err(e),
        Err(_) => Vec::new(),
    };
    let codepage = fixed_u32(&properties, tag::MESSAGE_CODEPAGE)
        .or_else(|| fixed_u32(&properties, tag::INTERNET_CPID))
        .and_then(|cp| u16::try_from(cp).ok());

    let subject = read_property(&mut ole, &streams, tag::SUBJECT, codepage)?;
    let body = read_property(&mut ole, &streams, tag::BODY, codepage)?;
    let sender_name = read_property(&mut ole, &streams, tag::SENDER_NAME, codepage)?;
    let sender_email = read_property(&mut ole, &streams, tag::SENDER_EMAIL, codepage)?;
    let to = read_property(&mut ole, &streams, tag::DISPLAY_TO, codepage)?;
    let cc = read_property(&mut ole, &streams, tag::DISPLAY_CC, codepage)?;

    // An OLE file carrying no message property at all is some other compound
    // document, not a message with nothing in it.
    if subject.is_none() && body.is_none() && sender_name.is_none() && to.is_none() {
        return Err(ConvertError::malformed("no MAPI message properties in the compound file"));
    }

    let mut doc = Document::default();
    if let Some(subject) = pick(&subject) {
        doc.blocks.push(Block::heading(1, vec![Inline::plain(subject)]));
    }

    // The envelope reads as labelled lines rather than a table: these are
    // fields of one message, not rows of tabular data.
    let mut envelope: Vec<Inline> = Vec::new();
    for (label, value) in
        [("From", sender(&sender_name, &sender_email)), ("To", pick(&to)), ("Cc", pick(&cc))]
    {
        let Some(value) = value else { continue };
        if !envelope.is_empty() {
            envelope.push(Inline::LineBreak);
        }
        envelope.push(Inline::Text {
            text: format!("{label}: "),
            style: Style { bold: true, ..Style::PLAIN },
        });
        envelope.push(Inline::plain(value));
    }
    if !envelope.is_empty() {
        doc.blocks.push(Block::Paragraph(envelope));
    }

    // A message whose only body is HTML or compressed RTF has no plain text
    // to read; saying so beats emitting markup as prose.
    let Some(body) = body else {
        return Err(ConvertError::Unsupported(
            "the message carries no plain-text body (PR_BODY)".to_string(),
        ));
    };
    doc.blocks.extend(body_blocks(&body));
    Ok(doc)
}

/// A property value with content, cleaned and trimmed. Absent and blank are
/// the same thing in a header field, and a producer writing an explicitly
/// empty property means the second: a string stream holding only its NUL
/// terminator must not become an empty heading or a labelled blank line.
/// Cleaning has to happen before that test, since the characters it drops
/// are exactly the ones that make such a value look non-empty.
fn pick(value: &Option<String>) -> Option<String> {
    let cleaned = clean_text(value.as_deref()?);
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The sender as one field. Name and address together when both are known
/// and differ; whichever exists otherwise. Producers routinely set the
/// address into the name when the sender resolved to no display name.
fn sender(name: &Option<String>, email: &Option<String>) -> Option<String> {
    match (pick(name), pick(email)) {
        (Some(n), Some(e)) if n != e => Some(format!("{n} <{e}>")),
        (Some(n), _) => Some(n),
        (None, Some(e)) => Some(e),
        (None, None) => None,
    }
}

/// Body text as paragraphs. A blank line separates paragraphs; a single
/// newline inside one is a hard break, which is how mail is written and how
/// a client displays it.
fn body_blocks(body: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<Inline> = Vec::new();
    // Split before cleaning: `clean_text` turns a line ending into a space,
    // which is right inside a line and wrong across one.
    for line in body.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let line = clean_text(line);
        // A line of spaces is blank: producers pad rather than leave it
        // empty, and taking it for content ends the paragraph with a hard
        // break onto nothing.
        if line.trim().is_empty() {
            if !para.is_empty() {
                blocks.push(Block::Paragraph(std::mem::take(&mut para)));
            }
            continue;
        }
        if !para.is_empty() {
            para.push(Inline::LineBreak);
        }
        para.push(Inline::plain(line.trim_end()));
    }
    if !para.is_empty() {
        blocks.push(Block::Paragraph(para));
    }
    blocks
}

/// Read one string property, preferring the Unicode stream. `codepage`
/// decodes a `PT_STRING8` value; without one, Windows-1252 stands in, being
/// what the default code pages agree with over ASCII.
fn read_property(
    ole: &mut cfb::CompoundFile<Cursor<&[u8]>>,
    streams: &[String],
    tag: u16,
    codepage: Option<u16>,
) -> Result<Option<String>, ConvertError> {
    for ty in [PT_UNICODE, PT_STRING8] {
        let wanted = format!("__substg1.0_{tag:04X}{ty:04X}");
        // Producers vary the hex case, so compare names the way CFB does.
        // A missing stream means this type is not the one written, not that
        // the property is absent: the other type still has to be tried.
        let Some(name) = streams.iter().find(|s| s.eq_ignore_ascii_case(&wanted)).cloned() else {
            continue;
        };
        let bytes = match read_ole_stream(ole, &name) {
            Ok(b) => b,
            // A safety limit is not an unreadable property: it says the file
            // is past what this conversion will materialize, and continuing
            // would report partial content as if it were the message.
            Err(e) if e.is_fatal() => return Err(e),
            Err(e) => {
                log::warn!("skipping unreadable property stream {name}: {e}");
                continue;
            }
        };
        if bytes.is_empty() {
            continue;
        }
        return Ok(Some(match ty {
            PT_UNICODE => decode_utf16le(&bytes),
            _ => decode_ansi(&bytes, codepage),
        }));
    }
    Ok(None)
}

/// A fixed-width `PT_LONG` from the property store. Entries are a flat array
/// after the header, each naming its own tag, so the store is scanned rather
/// than indexed.
fn fixed_u32(properties: &[u8], tag: u16) -> Option<u32> {
    let entries = properties.get(PROPERTIES_HEADER..)?;
    for entry in entries.chunks_exact(PROPERTY_ENTRY) {
        // The tag packs the property id above its type.
        let packed = u32::from_le_bytes(entry[0..4].try_into().ok()?);
        if (packed >> 16) as u16 == tag && (packed & 0xFFFF) as u16 == PT_LONG {
            return Some(u32::from_le_bytes(entry[8..12].try_into().ok()?));
        }
    }
    None
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|p| u16::from_le_bytes(*p)).collect();
    String::from_utf16_lossy(&units)
}

fn decode_ansi(bytes: &[u8], codepage: Option<u16>) -> String {
    let encoding = codepage
        .and_then(|cp| encoding_rs::Encoding::for_label(format!("windows-{cp}").as_bytes()))
        .unwrap_or(encoding_rs::WINDOWS_1252);
    encoding.decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::inlines_to_plain_text;
    use std::io::Write;

    /// A message built from property streams, the way a producer writes one.
    #[derive(Default)]
    struct Msg {
        props: Vec<(u16, u16, Vec<u8>)>,
        fixed: Vec<(u16, u32)>,
    }

    impl Msg {
        fn unicode(mut self, tag: u16, text: &str) -> Self {
            let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            self.props.push((tag, PT_UNICODE, bytes));
            self
        }

        fn ansi(mut self, tag: u16, bytes: &[u8]) -> Self {
            self.props.push((tag, PT_STRING8, bytes.to_vec()));
            self
        }

        /// A `PT_LONG`, which belongs in the property store rather than a
        /// stream of its own - which is where a real producer puts it.
        fn long(mut self, tag: u16, value: u32) -> Self {
            self.fixed.push((tag, value));
            self
        }

        /// `__properties_version1.0` as [MS-OXMSG] lays it out: the
        /// top-level header, then one 16-byte entry per fixed-width value.
        fn properties(&self) -> Vec<u8> {
            let mut out = vec![0u8; PROPERTIES_HEADER];
            for (tag, value) in &self.fixed {
                let packed = (u32::from(*tag) << 16) | u32::from(PT_LONG);
                out.extend_from_slice(&packed.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
            }
            out
        }

        fn build(&self) -> Vec<u8> {
            let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
            ole.create_stream(PROPERTIES_STREAM).unwrap().write_all(&self.properties()).unwrap();
            for (tag, ty, bytes) in &self.props {
                let name = format!("__substg1.0_{tag:04X}{ty:04X}");
                ole.create_stream(&name).unwrap().write_all(bytes).unwrap();
            }
            ole.into_inner().into_inner()
        }
    }

    fn blocks_text(doc: &Document) -> Vec<String> {
        doc.blocks
            .iter()
            .map(|b| match b {
                Block::Heading { level, content, .. } => {
                    format!("h{level}: {}", inlines_to_plain_text(content))
                }
                Block::Paragraph(i) => format!("p: {}", inlines_to_plain_text(i)),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn subject_envelope_and_body_become_the_document() {
        let msg = Msg::default()
            .unicode(tag::SUBJECT, "Quarterly review")
            .unicode(tag::SENDER_NAME, "Ada Lovelace")
            .unicode(tag::SENDER_EMAIL, "ada@example.com")
            .unicode(tag::DISPLAY_TO, "Grace Hopper")
            .unicode(tag::DISPLAY_CC, "Alan Turing")
            .unicode(tag::BODY, "First paragraph.\r\n\r\nSecond paragraph.");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(
            blocks_text(&doc),
            vec![
                "h1: Quarterly review",
                "p: From: Ada Lovelace <ada@example.com>\nTo: Grace Hopper\nCc: Alan Turing",
                "p: First paragraph.",
                "p: Second paragraph.",
            ]
        );
    }

    #[test]
    fn the_envelope_labels_are_bold() {
        // The labels are anydoc's own framing, not text the message carries,
        // so they are marked as such rather than left to read as content.
        let msg = Msg::default().unicode(tag::SENDER_NAME, "Ada").unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        let Block::Paragraph(inlines) = &doc.blocks[0] else {
            panic!("expected the envelope paragraph first");
        };
        assert!(
            matches!(&inlines[0], Inline::Text { text, style } if text == "From: " && style.bold)
        );
        assert!(
            matches!(&inlines[1], Inline::Text { text, style } if text == "Ada" && !style.bold)
        );
    }

    #[test]
    fn an_absent_envelope_field_is_skipped_not_left_blank() {
        // Producers omit the stream entirely rather than writing an empty
        // one, and a message with no Cc must not grow an empty Cc line.
        let msg = Msg::default()
            .unicode(tag::SENDER_NAME, "Ada")
            .unicode(tag::DISPLAY_TO, "   ")
            .unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc), vec!["p: From: Ada", "p: hi"]);
    }

    #[test]
    fn the_sender_address_is_not_repeated_when_it_is_the_display_name() {
        // Outlook sets the address into the name when the sender resolved to
        // no display name, which would otherwise render "a@b <a@b>".
        let msg = Msg::default()
            .unicode(tag::SENDER_NAME, "ada@example.com")
            .unicode(tag::SENDER_EMAIL, "ada@example.com")
            .unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "p: From: ada@example.com");
    }

    #[test]
    fn a_single_newline_is_a_break_and_a_blank_line_ends_the_paragraph() {
        let msg = Msg::default().unicode(tag::BODY, "one\r\ntwo\r\n\r\nthree");
        let doc = parse(&msg.build()).unwrap();
        let Block::Paragraph(first) = &doc.blocks[0] else { panic!("expected a paragraph") };
        assert!(matches!(first[1], Inline::LineBreak), "a single newline is a hard break");
        assert_eq!(blocks_text(&doc), vec!["p: one\ntwo", "p: three"]);
    }

    #[test]
    fn a_whitespace_only_line_separates_paragraphs() {
        // Producers pad a "blank" line with a space. Taking it for content
        // ends the paragraph with a hard break onto nothing.
        let msg = Msg::default().unicode(tag::BODY, "one\r\n \r\ntwo");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc), vec!["p: one", "p: two"]);
    }

    #[test]
    fn an_ansi_property_decodes_through_the_message_code_page() {
        // PT_STRING8 carries bytes in the code page the message names, so
        // the same byte is a different character under a different one.
        let msg = Msg::default()
            .long(tag::MESSAGE_CODEPAGE, 1251)
            .ansi(tag::SUBJECT, &[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2])
            .unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "h1: Привет");

        // `PR_INTERNET_CPID` names the same thing, and producers set one or
        // the other.
        let msg = Msg::default()
            .long(tag::INTERNET_CPID, 1251)
            .ansi(tag::SUBJECT, &[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2])
            .unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "h1: Привет");

        // With no code page named, Windows-1252 stands in.
        let msg =
            Msg::default().ansi(tag::SUBJECT, &[0x63, 0x61, 0x66, 0xE9]).unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "h1: café");
    }

    #[test]
    fn the_unicode_property_wins_over_the_ansi_one() {
        // Producers write both for compatibility; the Unicode stream is the
        // one that cannot have lost characters.
        let mut msg = Msg::default().unicode(tag::BODY, "hi");
        msg.props.push((tag::SUBJECT, PT_STRING8, b"ascii only".to_vec()));
        let msg = msg.unicode(tag::SUBJECT, "café ☕");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "h1: café ☕");
    }

    #[test]
    fn an_explicitly_empty_property_reads_as_absent() {
        // A producer writing an empty string writes its NUL terminator, so
        // the stream is not empty. Treating that as content produces an
        // empty heading and a labelled blank line.
        let msg = Msg::default()
            .unicode(tag::SUBJECT, "\u{0}")
            .unicode(tag::DISPLAY_TO, "\u{0}")
            .unicode(tag::SENDER_NAME, "Ada")
            .unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc), vec!["p: From: Ada", "p: hi"]);
    }

    #[test]
    fn the_code_page_is_read_from_the_property_store_not_a_stream() {
        // `PR_MESSAGE_CODEPAGE` is a PT_LONG, so no producer writes it as a
        // string stream. Looking for one leaves every ANSI property decoded
        // as Windows-1252 whatever the message says.
        let mut msg = Msg::default().ansi(tag::SUBJECT, &[0xC0, 0xE1]).unicode(tag::BODY, "hi");
        msg.props.push((
            tag::MESSAGE_CODEPAGE,
            PT_UNICODE,
            "1251".encode_utf16().flat_map(|u| u.to_le_bytes()).collect(),
        ));
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "h1: Àá", "a string stream is not where it lives");

        let msg = Msg::default()
            .long(tag::MESSAGE_CODEPAGE, 1251)
            .ansi(tag::SUBJECT, &[0xC0, 0xE1])
            .unicode(tag::BODY, "hi");
        let doc = parse(&msg.build()).unwrap();
        assert_eq!(blocks_text(&doc)[0], "h1: Аб", "the store is");
    }

    #[test]
    fn a_truncated_property_store_is_not_a_panic() {
        // Every length here comes from the file, so a store cut mid-entry
        // has to fall out as a missing code page rather than an index.
        for len in 0..PROPERTIES_HEADER + PROPERTY_ENTRY {
            assert_eq!(fixed_u32(&vec![0u8; len], tag::MESSAGE_CODEPAGE), None, "{len} bytes");
        }
    }

    #[test]
    fn a_message_with_no_plain_body_is_unsupported() {
        // An HTML-only or RTF-only body has no prose to read, and emitting
        // the markup as text would be worse than saying so.
        let msg = Msg::default().unicode(tag::SUBJECT, "html only");
        let err = parse(&msg.build()).unwrap_err();
        assert!(matches!(err, ConvertError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn a_compound_file_that_is_not_a_message_is_rejected() {
        // Detection routes on stream names, so a stray OLE file can still
        // arrive here by extension.
        let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        ole.create_stream("WordDocument").unwrap().write_all(b"x").unwrap();
        let err = parse(&ole.into_inner().into_inner()).unwrap_err();
        assert!(matches!(err, ConvertError::Malformed { .. }), "got {err:?}");
    }
}
