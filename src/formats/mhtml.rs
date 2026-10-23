//! MHTML/MHT MIME container frontend.

use crate::error::ConvertError;
use crate::model::{Asset, AssetId, Document, ImageSource};
use crate::package::limits;
use crate::shared::html::HtmlCtx;
use crate::shared::uri::is_absolute_uri;
use mail_parser::decoders::{base64::base64_decode, quoted_printable::quoted_printable_decode};
use mail_parser::{Encoding, Message, MessageParser, MessagePart, MimeHeaders};
use scraper::Html;
use std::collections::HashMap;

const HEADER_SCAN_LIMIT: usize = 64 * 1024;

pub(crate) fn looks_like_mhtml(bytes: &[u8]) -> bool {
    let header = mime_header_block(bytes);
    let unfolded = unfold_headers(header);
    let lower = unfolded.to_ascii_lowercase();
    let snapshot = lower.lines().any(|line| line.starts_with("snapshot-content-location:"));

    let Some(content_type) = lower.lines().find_map(|line| line.strip_prefix("content-type:"))
    else {
        return false;
    };
    let compact: String = content_type.chars().filter(|c| !c.is_ascii_whitespace()).collect();

    (compact == "multipart/related" || compact.starts_with("multipart/related;")) && snapshot
}

fn mime_header_block(bytes: &[u8]) -> &[u8] {
    let bytes = &bytes[..bytes.len().min(HEADER_SCAN_LIMIT)];
    let crlf = bytes.windows(4).position(|w| w == b"\r\n\r\n");
    let lf = bytes.windows(2).position(|w| w == b"\n\n");
    let end = match (crlf, lf) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => bytes.len(),
    };
    &bytes[..end]
}

fn unfold_headers(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            out.push(' ');
            out.push_str(line.trim());
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    out
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    if bytes.len() as u64 > limits::MAX_TOTAL_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_total_bytes",
            detail: format!(
                "MHTML input is {} bytes; maximum is {}",
                bytes.len(),
                limits::MAX_TOTAL_BYTES
            ),
        });
    }

    preflight_base64_decoder_allocations(bytes)?;

    let message = MessageParser::new()
        .with_mime_headers()
        .parse(bytes)
        .ok_or_else(|| ConvertError::malformed("MHTML MIME structure could not be parsed"))?;

    if !message.is_content_type("multipart", "related") {
        return Err(ConvertError::malformed("MHTML root is not multipart/related"));
    }

    let start = message
        .content_type()
        .and_then(|content_type| content_type.attribute("start"))
        .map(normalize_content_id);

    let (_, html_part) = if let Some(start) = start {
        message.parts.iter().enumerate().find(|(_, part)| {
            part.is_content_type("text", "html")
                && part
                    .content_id()
                    .map(normalize_content_id)
                    .is_some_and(|content_id| content_id == start)
        })
    } else {
        message.parts.iter().enumerate().find(|(_, part)| part.is_content_type("text", "html"))
    }
    .ok_or_else(|| ConvertError::malformed("MHTML contains no HTML root part"))?;

    let html_bytes = transfer_decoded_part_bytes(&message, html_part, "HTML root")?;

    let declared_charset =
        html_part.content_type().and_then(|content_type| content_type.attribute("charset"));
    let html = super::html::decode_html_with_charset(&html_bytes, declared_charset);
    super::html::preflight_html_complexity(&html)?;
    let parsed_html = Html::parse_document(&html);
    let root_location = html_part.content_location();
    let resource_base = html_resource_base(&parsed_html, root_location);

    let resource_index = build_resource_index(&message.parts, resource_base.as_deref());
    let stylesheets = collect_stylesheets_in_order(
        &parsed_html,
        &message.parts,
        &resource_index,
        resource_base.as_deref(),
    )?;
    let (assets, image_assets) = collect_image_assets(&message, resource_base.as_deref())?;
    let ctx = MhtmlCtx { image_assets, resource_base };

    super::html::document_from_parsed_html(&parsed_html, Some(&stylesheets), &ctx, assets)
}

fn preflight_base64_decoder_allocations(bytes: &[u8]) -> Result<(), ConvertError> {
    preflight_base64_decoder_allocations_with_limit(bytes, limits::MAX_ENTRY_BYTES)
}

fn preflight_base64_decoder_allocations_with_limit(
    bytes: &[u8],
    max_entry_bytes: u64,
) -> Result<(), ConvertError> {
    let Some((_, body_start)) = mime_header_body_start(bytes, 0, bytes.len()) else {
        return Ok(());
    };
    let headers = &bytes[..body_start];
    if headers.len() > MAX_PART_HEADER_BYTES {
        return Err(ConvertError::malformed(format!(
            "MHTML root headers are {} bytes; maximum is {MAX_PART_HEADER_BYTES}",
            headers.len()
        )));
    }

    if headers_use_base64(headers) {
        enforce_base64_parser_reserve(bytes.len().saturating_sub(body_start), max_entry_bytes)?;
    }

    let Some(boundary) = mime_boundary_from_headers(headers)? else {
        return Ok(());
    };
    preflight_multipart_base64(bytes, body_start, bytes.len(), &boundary, max_entry_bytes, 1)
}

fn preflight_multipart_base64(
    bytes: &[u8],
    mut offset: usize,
    end: usize,
    boundary: &[u8],
    max_entry_bytes: u64,
    depth: usize,
) -> Result<(), ConvertError> {
    let mut marker = Vec::with_capacity(boundary.len() + 2);
    marker.extend_from_slice(b"--");
    marker.extend_from_slice(boundary);

    while offset < end {
        let Some(marker_start) = find_marker(bytes, offset, end, &marker, false) else {
            break;
        };
        let marker_end = marker_start + marker.len();
        if bytes.get(marker_end..marker_end + 2) == Some(b"--") {
            break;
        }

        let headers_start = skip_boundary_line(bytes, marker_end, end);
        let Some((_, body_start)) = mime_header_body_start(bytes, headers_start, end) else {
            break;
        };
        let headers = &bytes[headers_start..body_start];
        if headers.len() > MAX_PART_HEADER_BYTES {
            return Err(ConvertError::malformed(format!(
                "MHTML part headers are {} bytes; maximum is {MAX_PART_HEADER_BYTES}",
                headers.len()
            )));
        }

        let next_parent = find_marker(bytes, body_start, end, &marker, true).unwrap_or(end);

        if headers_use_base64(headers) {
            // mail-parser 0.11.x reserves from MessageStream::remaining() before it scans
            // for the active MIME boundary, so the safe upper bound is deliberately the
            // entire remaining message rather than only this part's encoded body.
            enforce_base64_parser_reserve(bytes.len().saturating_sub(body_start), max_entry_bytes)?;
        } else if headers_use_quoted_printable(headers) {
            // mail-parser eagerly decodes part bodies while parsing. Quoted-printable
            // never expands its input, so the encoded region size bounds the decoded
            // allocation; reject oversized parts before the parser materializes a
            // body the limits would reject afterwards. The region ends at the line
            // ending that precedes the next boundary delimiter.
            let mut encoded_end = next_parent;
            if bytes.get(encoded_end.saturating_sub(2)..encoded_end) == Some(b"\r\n") {
                encoded_end -= 2;
            } else if bytes.get(encoded_end.saturating_sub(1)..encoded_end) == Some(b"\n") {
                encoded_end -= 1;
            }
            let encoded_len = encoded_end.saturating_sub(body_start);
            if encoded_len as u64 > max_entry_bytes {
                return Err(ConvertError::ResourceLimit {
                    limit: "max_entry_bytes",
                    detail: format!(
                        "MHTML quoted-printable part can decode to at most {encoded_len} bytes; maximum is {max_entry_bytes}"
                    ),
                });
            }
        }

        if headers_are_message(headers) {
            // An encapsulated message carries its own MIME tree; mail-parser
            // parses (and base64-reserves) it during the outer parse, so the
            // preflight must walk it too. Count it against the same depth
            // budget.
            let nested_depth = depth.saturating_add(1);
            if nested_depth > limits::MAX_MIME_DEPTH {
                return Err(ConvertError::ResourceLimit {
                    limit: "max_mime_depth",
                    detail: format!(
                        "MHTML MIME nesting depth {nested_depth} exceeds maximum of {}",
                        limits::MAX_MIME_DEPTH
                    ),
                });
            }
            let mut encoded_end = next_parent;
            if bytes.get(encoded_end.saturating_sub(2)..encoded_end) == Some(b"\r\n") {
                encoded_end -= 2;
            } else if bytes.get(encoded_end.saturating_sub(1)..encoded_end) == Some(b"\n") {
                encoded_end -= 1;
            }
            let body_region = &bytes[body_start..encoded_end];
            if headers_use_base64(headers) {
                // The encapsulated message is only visible after decoding;
                // bound the decode and walk the decoded bytes.
                let upper_bound = base64_decoded_upper_bound(body_region);
                if upper_bound > max_entry_bytes {
                    return Err(ConvertError::ResourceLimit {
                        limit: "max_entry_bytes",
                        detail: format!(
                            "MHTML encapsulated message base64 body can decode to at most {upper_bound} bytes; maximum is {max_entry_bytes}"
                        ),
                    });
                }
                if let Some(decoded) = base64_decode(body_region) {
                    preflight_encapsulated_message(
                        &decoded,
                        0,
                        decoded.len(),
                        max_entry_bytes,
                        nested_depth,
                    )?;
                }
            } else if headers_use_quoted_printable(headers) {
                if body_region.len() as u64 > max_entry_bytes {
                    return Err(ConvertError::ResourceLimit {
                        limit: "max_entry_bytes",
                        detail: format!(
                            "MHTML encapsulated message quoted-printable body is {} bytes; maximum is {max_entry_bytes}",
                            body_region.len()
                        ),
                    });
                }
                if let Some(decoded) = quoted_printable_decode(body_region) {
                    preflight_encapsulated_message(
                        &decoded,
                        0,
                        decoded.len(),
                        max_entry_bytes,
                        nested_depth,
                    )?;
                }
            } else {
                preflight_encapsulated_message(
                    bytes,
                    body_start,
                    encoded_end,
                    max_entry_bytes,
                    nested_depth,
                )?;
            }
        }

        if let Some(nested_boundary) = mime_boundary_from_headers(headers)? {
            let nested_depth = depth.saturating_add(1);
            if nested_depth > limits::MAX_MIME_DEPTH {
                return Err(ConvertError::ResourceLimit {
                    limit: "max_mime_depth",
                    detail: format!(
                        "MHTML MIME nesting depth {nested_depth} exceeds maximum of {}",
                        limits::MAX_MIME_DEPTH
                    ),
                });
            }
            preflight_multipart_base64(
                bytes,
                body_start,
                next_parent,
                &nested_boundary,
                max_entry_bytes,
                nested_depth,
            )?;
        }
        offset = next_parent;
    }

    Ok(())
}

fn headers_are_message(headers: &[u8]) -> bool {
    let Some(message) = MessageParser::new().with_mime_headers().parse_headers(headers) else {
        return false;
    };
    message
        .content_type()
        .is_some_and(|content_type| content_type.ctype().eq_ignore_ascii_case("message"))
}

/// Walk an encapsulated message body the way the multipart walk does: bound
/// the base64 reserve, bound quoted-printable parts, and recurse into any
/// inner multipart tree.
fn preflight_encapsulated_message(
    bytes: &[u8],
    body_start: usize,
    end: usize,
    max_entry_bytes: u64,
    depth: usize,
) -> Result<(), ConvertError> {
    let Some((_, inner_body_start)) = mime_header_body_start(bytes, body_start, end) else {
        return Ok(());
    };
    let headers = &bytes[body_start..inner_body_start];
    if headers.len() > MAX_PART_HEADER_BYTES {
        return Err(ConvertError::malformed(format!(
            "MHTML encapsulated message headers are {} bytes; maximum is {MAX_PART_HEADER_BYTES}",
            headers.len()
        )));
    }

    if headers_use_base64(headers) {
        enforce_base64_parser_reserve(
            bytes.len().saturating_sub(inner_body_start),
            max_entry_bytes,
        )?;
    } else if headers_use_quoted_printable(headers) {
        let mut encoded_end = end;
        if bytes.get(encoded_end.saturating_sub(2)..encoded_end) == Some(b"\r\n") {
            encoded_end -= 2;
        } else if bytes.get(encoded_end.saturating_sub(1)..encoded_end) == Some(b"\n") {
            encoded_end -= 1;
        }
        let encoded_len = encoded_end.saturating_sub(inner_body_start);
        if encoded_len as u64 > max_entry_bytes {
            return Err(ConvertError::ResourceLimit {
                limit: "max_entry_bytes",
                detail: format!(
                    "MHTML quoted-printable part can decode to at most {encoded_len} bytes; maximum is {max_entry_bytes}"
                ),
            });
        }
    }

    if let Some(boundary) = mime_boundary_from_headers(headers)? {
        // Multipart bodies are scanned raw on purpose: RFC 2046 forbids
        // transfer encodings on multipart, and mail-parser looks for the
        // boundary in the undecoded bytes, so the preflight must too.
        preflight_multipart_base64(
            bytes,
            inner_body_start,
            end,
            &boundary,
            max_entry_bytes,
            depth,
        )?;
    } else if headers_are_message(headers) {
        // Chains of message/rfc822 without multipart wrappers must keep
        // being walked too, against the same depth budget. A
        // transfer-encoded encapsulated body only reveals the next chain
        // link after decoding (mail-parser decodes recursively), so the
        // walk continues on the decoded bytes; the encoded size is already
        // bounded above and the decoded upper bound is enforced here.
        let nested_depth = depth.saturating_add(1);
        if nested_depth > limits::MAX_MIME_DEPTH {
            return Err(ConvertError::ResourceLimit {
                limit: "max_mime_depth",
                detail: format!(
                    "MHTML MIME nesting depth {nested_depth} exceeds maximum of {}",
                    limits::MAX_MIME_DEPTH
                ),
            });
        }
        let mut encoded_end = end;
        if bytes.get(encoded_end.saturating_sub(2)..encoded_end) == Some(b"\r\n") {
            encoded_end -= 2;
        } else if bytes.get(encoded_end.saturating_sub(1)..encoded_end) == Some(b"\n") {
            encoded_end -= 1;
        }
        let body_region = &bytes[inner_body_start..encoded_end];
        if headers_use_base64(headers) {
            let upper_bound = base64_decoded_upper_bound(body_region);
            if upper_bound > max_entry_bytes {
                return Err(ConvertError::ResourceLimit {
                    limit: "max_entry_bytes",
                    detail: format!(
                        "MHTML encapsulated message base64 body can decode to at most {upper_bound} bytes; maximum is {max_entry_bytes}"
                    ),
                });
            }
            if let Some(decoded) = base64_decode(body_region) {
                preflight_encapsulated_message(
                    &decoded,
                    0,
                    decoded.len(),
                    max_entry_bytes,
                    nested_depth,
                )?;
            }
        } else if headers_use_quoted_printable(headers) {
            if let Some(decoded) = quoted_printable_decode(body_region) {
                preflight_encapsulated_message(
                    &decoded,
                    0,
                    decoded.len(),
                    max_entry_bytes,
                    nested_depth,
                )?;
            }
        } else {
            preflight_encapsulated_message(
                bytes,
                inner_body_start,
                end,
                max_entry_bytes,
                nested_depth,
            )?;
        }
    }
    Ok(())
}

fn enforce_base64_parser_reserve(
    remaining: usize,
    max_entry_bytes: u64,
) -> Result<(), ConvertError> {
    let reserve = (remaining as u64 / 4).saturating_mul(3);
    if reserve > max_entry_bytes {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!(
                "MHTML base64 decoder may reserve {reserve} bytes before locating the MIME boundary; maximum is {max_entry_bytes}"
            ),
        });
    }
    Ok(())
}

fn mime_header_body_start(bytes: &[u8], start: usize, end: usize) -> Option<(usize, usize)> {
    let slice = bytes.get(start..end)?;
    let crlf = slice.windows(4).position(|window| window == b"\r\n\r\n");
    let lf = slice.windows(2).position(|window| window == b"\n\n");
    match (crlf, lf) {
        (Some(a), Some(b)) if a <= b => Some((start + a, start + a + 4)),
        (Some(_a), Some(b)) => Some((start + b, start + b + 2)),
        (Some(a), None) => Some((start + a, start + a + 4)),
        (None, Some(b)) => Some((start + b, start + b + 2)),
        (None, None) => None,
    }
}

/// Maximum MIME boundary length accepted by the preflight. RFC 2046 caps
/// boundaries at 70 characters; the generous headroom still blocks
/// pathological headers from driving large copies before the marker is
/// built.
const MAX_MIME_BOUNDARY_BYTES: usize = 256;

/// Maximum size of a MIME part header block the preflight will parse.
/// Real headers are far smaller; the cap bounds every allocation driven by
/// header contents (unfolding, mail-parser parsing, boundary copies).
const MAX_PART_HEADER_BYTES: usize = 64 * 1024;

fn mime_boundary_from_headers(headers: &[u8]) -> Result<Option<Vec<u8>>, ConvertError> {
    let Some(message) = MessageParser::new().with_mime_headers().parse_headers(headers) else {
        return Ok(None);
    };
    let Some(content_type) = message.content_type() else {
        return Ok(None);
    };
    // mail-parser only nests multipart media types. A `boundary` parameter on
    // any other media type does not create nested MIME parts, so the preflight
    // must not treat boundary-looking body text there as nested MIME.
    if !content_type.ctype().eq_ignore_ascii_case("multipart") {
        return Ok(None);
    }
    let Some(boundary) = content_type.attribute("boundary") else {
        return Ok(None);
    };
    if boundary.len() > MAX_MIME_BOUNDARY_BYTES {
        return Err(ConvertError::malformed(format!(
            "MHTML MIME boundary is {} bytes; maximum is {MAX_MIME_BOUNDARY_BYTES}",
            boundary.len()
        )));
    }
    Ok(Some(boundary.as_bytes().to_vec()))
}

fn headers_use_base64(headers: &[u8]) -> bool {
    unfold_headers(headers).lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("content-transfer-encoding")
                && value.trim().eq_ignore_ascii_case("base64")
        })
    })
}

fn headers_use_quoted_printable(headers: &[u8]) -> bool {
    unfold_headers(headers).lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("content-transfer-encoding")
                && value.trim().eq_ignore_ascii_case("quoted-printable")
        })
    })
}

fn skip_boundary_line(bytes: &[u8], mut offset: usize, end: usize) -> usize {
    while offset < end {
        match bytes[offset] {
            b'\r' | b' ' | b'\t' => offset += 1,
            b'\n' => {
                offset += 1;
                break;
            }
            _ => break,
        }
    }
    offset
}

/// True when the bytes right after a marker occurrence finish a proper MIME
/// delimiter: the closing dashes, or the CRLF/LF that ends the boundary line.
/// Without this, a boundary that is a prefix of another (b1 vs b10) or random
/// body bytes cross-match.
fn is_delimiter_end(bytes: &[u8], marker_end: usize) -> bool {
    // RFC 2046 allows transport padding (spaces/tabs) between the boundary
    // and the line ending, and skip_boundary_line accepts the same padding,
    // so delimiter matching must accept it too. For the closing delimiter the
    // padding may follow the trailing dashes, which must still end the line
    // or the input; otherwise body text like "--b--junk" would end the walk
    // early and leave later parts unbounded.
    let mut index = marker_end;
    while matches!(bytes.get(index), Some(b' ') | Some(b'\t')) {
        index += 1;
    }
    if matches!(bytes.get(index), Some(b'\r') | Some(b'\n')) {
        return true;
    }
    if bytes.get(index..index + 2) == Some(b"--") {
        index += 2;
        while matches!(bytes.get(index), Some(b' ') | Some(b'\t')) {
            index += 1;
        }
        return index == bytes.len() || matches!(bytes.get(index), Some(b'\r') | Some(b'\n'));
    }
    false
}

/// Find the next proper delimiter for the boundary marker in bytes[start..end].
/// line_start_only additionally requires the RFC 2046 line-start position;
/// detection keeps mail-parser's lenient position matching (see the
/// embedded-boundary unit test) while part-region lookup is strict, so
/// boundary-looking bytes inside a body cannot truncate a region early and
/// under-bound the size checks.
fn find_marker(
    bytes: &[u8],
    start: usize,
    end: usize,
    marker: &[u8],
    line_start_only: bool,
) -> Option<usize> {
    let mut search = start;
    while let Some(offset) = find_bytes(&bytes[search..end], marker) {
        let absolute = search + offset;
        let anchored = !line_start_only || absolute == 0 || bytes[absolute - 1] == b'\n';
        if anchored && is_delimiter_end(bytes, absolute + marker.len()) {
            return Some(absolute);
        }
        search = absolute + 1;
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty() && needle.len() <= haystack.len())
        .then(|| haystack.windows(needle.len()).position(|window| window == needle))
        .flatten()
}

fn transfer_decoded_part_bytes(
    message: &Message<'_>,
    part: &MessagePart<'_>,
    label: &str,
) -> Result<Vec<u8>, ConvertError> {
    let raw = message
        .raw_message
        .get(part.offset_body as usize..part.offset_end as usize)
        .ok_or_else(|| ConvertError::malformed(format!("MHTML {label} byte range is invalid")))?;

    if part.is_encoding_problem {
        return Err(ConvertError::malformed(format!("MHTML {label} transfer encoding is invalid")));
    }

    let decoded = match part.encoding {
        Encoding::None => {
            enforce_part_size(raw.len(), label, "body")?;
            raw.to_vec()
        }
        Encoding::QuotedPrintable => {
            enforce_part_size(raw.len(), label, "quoted-printable encoded body")?;
            quoted_printable_decode(raw).ok_or_else(|| {
                ConvertError::malformed(format!("MHTML {label} quoted-printable body is invalid"))
            })?
        }
        Encoding::Base64 => {
            // base64 expands data by 4/3, so a valid part whose decoded size
            // fits max_entry_bytes can still have a larger encoded body; the
            // decoded upper bound below is the meaningful limit here.
            let upper_bound = base64_decoded_upper_bound(raw);
            if upper_bound > limits::MAX_ENTRY_BYTES {
                return Err(ConvertError::ResourceLimit {
                    limit: "max_entry_bytes",
                    detail: format!(
                        "MHTML {label} base64 body can decode to at most {upper_bound} bytes; maximum is {}",
                        limits::MAX_ENTRY_BYTES
                    ),
                });
            }
            base64_decode(raw).ok_or_else(|| {
                ConvertError::malformed(format!("MHTML {label} base64 body is invalid"))
            })?
        }
    };

    enforce_part_size(decoded.len(), label, "decoded body")?;
    Ok(decoded)
}

fn enforce_part_size(size: usize, label: &str, kind: &str) -> Result<(), ConvertError> {
    if size as u64 > limits::MAX_ENTRY_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!(
                "MHTML {label} {kind} is {size} bytes; maximum is {}",
                limits::MAX_ENTRY_BYTES
            ),
        });
    }
    Ok(())
}

fn base64_decoded_upper_bound(raw: &[u8]) -> u64 {
    let symbols = raw.iter().filter(|byte| !byte.is_ascii_whitespace()).count() as u64;
    symbols.saturating_add(3).saturating_div(4).saturating_mul(3)
}

fn html_resource_base(parsed: &Html, root_location: Option<&str>) -> Option<String> {
    let root = parsed.root_element();
    root.descendent_elements()
        .find(|element| element.value().name() == "base")
        .and_then(|element| element.value().attr("href"))
        .map(|href| resolve_resource_reference(root_location, href))
        .filter(|base| !base.is_empty())
        .or_else(|| root_location.map(canonical_reference))
}

fn build_resource_index(
    parts: &[mail_parser::MessagePart<'_>],
    resource_base: Option<&str>,
) -> HashMap<String, usize> {
    let mut index = HashMap::new();
    for (part_index, part) in parts.iter().enumerate() {
        for key in resource_keys(part, resource_base) {
            index.entry(key).or_insert(part_index);
        }
    }
    index
}

fn collect_stylesheets_in_order(
    parsed: &Html,
    parts: &[mail_parser::MessagePart<'_>],
    resource_index: &HashMap<String, usize>,
    resource_base: Option<&str>,
) -> Result<Vec<String>, ConvertError> {
    let root = parsed.root_element();
    let mut stylesheets = Vec::new();

    for element in root.descendent_elements() {
        match element.value().name() {
            "style" => stylesheets.push(element.text().collect::<String>()),
            "link" => {
                let rel = element.value().attr("rel").unwrap_or("");
                if !rel
                    .split_ascii_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                {
                    continue;
                }
                let Some(href) = element.value().attr("href") else {
                    continue;
                };
                let reference = resolve_resource_reference(resource_base, href);
                let lookup = resource_lookup_key(&reference);
                let Some(&part_index) = resource_index.get(lookup) else {
                    continue;
                };
                let part = &parts[part_index];
                if !part.is_content_type("text", "css") {
                    continue;
                }
                if part.is_encoding_problem {
                    return Err(ConvertError::malformed(
                        "MHTML stylesheet transfer encoding is invalid",
                    ));
                }
                let Some(css) = part.text_contents() else {
                    continue;
                };
                if css.len() as u64 > limits::MAX_ENTRY_BYTES {
                    return Err(ConvertError::ResourceLimit {
                        limit: "max_entry_bytes",
                        detail: format!(
                            "MHTML stylesheet is {} bytes; maximum is {}",
                            css.len(),
                            limits::MAX_ENTRY_BYTES
                        ),
                    });
                }
                stylesheets.push(css.to_owned());
            }
            _ => {}
        }
    }

    Ok(stylesheets)
}

fn collect_image_assets(
    message: &Message<'_>,
    resource_base: Option<&str>,
) -> Result<(Vec<Asset>, HashMap<String, AssetId>), ConvertError> {
    let mut assets = Vec::new();
    let mut image_assets = HashMap::new();
    let mut total_bytes = 0u64;

    for (part_index, part) in message.parts.iter().enumerate() {
        let Some(content_type) = part.content_type() else {
            continue;
        };
        if !content_type.ctype().eq_ignore_ascii_case("image") {
            continue;
        }

        // Decode through the same guarded path as the HTML root so the size
        // limits are enforced before the decoded bytes are used, independent
        // of mail-parser's eager decoding behavior.
        let bytes = transfer_decoded_part_bytes(message, part, "image part")?;
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > limits::MAX_ASSET_TOTAL_BYTES as u64 {
            return Err(ConvertError::ResourceLimit {
                limit: "max_asset_total_bytes",
                detail: format!(
                    "MHTML embedded images retain {total_bytes} bytes; maximum is {}",
                    limits::MAX_ASSET_TOTAL_BYTES
                ),
            });
        }

        let id = AssetId(assets.len());
        let subtype = content_type.subtype().unwrap_or("octet-stream");
        let origin_part = part
            .content_location()
            .or_else(|| part.content_id())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("mhtml-part-{part_index}"));
        assets.push(Asset {
            id,
            media_type: format!("{}/{subtype}", content_type.ctype()),
            origin_part,
            bytes,
        });
        for key in resource_keys(part, resource_base) {
            image_assets.entry(key).or_insert(id);
        }
    }

    Ok((assets, image_assets))
}

fn resource_keys(part: &mail_parser::MessagePart<'_>, resource_base: Option<&str>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(content_id) = part.content_id() {
        let id = normalize_content_id(content_id);
        if !id.is_empty() {
            keys.push(format!("cid:{id}"));
        }
    }
    if let Some(location) = part.content_location() {
        let location = resolve_resource_reference(resource_base, location);
        let location = resource_lookup_key(&location).to_owned();
        if !location.is_empty() && !keys.contains(&location) {
            keys.push(location);
        }
    }
    keys
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &value[prefix.len()..])
}

fn canonical_reference(value: &str) -> String {
    let value = value.trim();
    if let Some(content_id) = strip_prefix_ignore_ascii_case(value, "cid:") {
        format!("cid:{}", normalize_content_id(content_id))
    } else {
        value.to_owned()
    }
}

fn normalize_content_id(value: &str) -> String {
    let value = value.trim();
    let value = strip_prefix_ignore_ascii_case(value, "cid:").unwrap_or(value);
    value.trim_matches(|c| matches!(c, '<' | '>')).to_ascii_lowercase()
}

fn resolve_resource_reference(base: Option<&str>, value: &str) -> String {
    let value = canonical_reference(value);
    if value.is_empty() || value.starts_with("cid:") || is_absolute_uri(&value) {
        return value;
    }
    let Some(base) = base else {
        return value;
    };
    if value.starts_with("//") {
        return base
            .split_once(':')
            .map(|(scheme, _)| format!("{scheme}:{value}"))
            .unwrap_or(value);
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        // The base carries no scheme (no absolute Content-Location and a
        // relative <base href>). Join base and reference as archive paths
        // so embedded resources still resolve against the base, mirroring
        // the resolution applied when the resource index is built.
        let (base_path, _) = split_path_suffix(base);
        let (reference_path, suffix) = split_path_suffix(&value);
        if reference_path.is_empty() {
            return format!("{base_path}{suffix}");
        }
        let joined = if reference_path.starts_with('/') {
            reference_path.to_owned()
        } else {
            let directory_end = base_path.rfind('/').map_or(0, |index| index + 1);
            format!("{}{}", &base_path[..directory_end], reference_path)
        };
        return format!("{}{}", normalize_url_path(&joined), suffix);
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let base_with_suffix = if authority_end < rest.len() { &rest[authority_end..] } else { "/" };
    let (base_path, _) = split_path_suffix(base_with_suffix);
    let (reference_path, suffix) = split_path_suffix(&value);
    if reference_path.is_empty() {
        return format!("{scheme}://{authority}{base_path}{suffix}");
    }
    let joined = if reference_path.starts_with('/') {
        reference_path.to_owned()
    } else {
        let directory_end = base_path.rfind('/').map_or(0, |index| index + 1);
        format!("{}{}", &base_path[..directory_end], reference_path)
    };
    format!("{scheme}://{authority}{}{suffix}", normalize_url_path(&joined))
}

fn split_path_suffix(value: &str) -> (&str, &str) {
    let query = value.find('?');
    let fragment = value.find('#');
    let index = match (query, fragment) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    index.map_or((value, ""), |index| value.split_at(index))
}

fn resource_lookup_key(reference: &str) -> &str {
    // Strip both query and fragment: archive parts are keyed by the resource
    // path, and references carrying either component must still resolve.
    split_path_suffix(reference).0
}

fn normalize_url_path(path: &str) -> String {
    let trailing_slash = path.ends_with('/');
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    let mut normalized = String::from("/");
    normalized.push_str(&segments.join("/"));
    if trailing_slash && !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

struct MhtmlCtx {
    image_assets: HashMap<String, AssetId>,
    resource_base: Option<String>,
}

impl HtmlCtx for MhtmlCtx {
    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError> {
        let src = src.trim();
        if src.is_empty() {
            return Ok(None);
        }
        let reference = resolve_resource_reference(self.resource_base.as_deref(), src);
        let lookup = resource_lookup_key(&reference);
        if let Some(&asset_id) = self.image_assets.get(lookup) {
            return Ok(Some(ImageSource::Asset(asset_id)));
        }
        if reference.starts_with("cid:") {
            return Ok(Some(ImageSource::Unavailable));
        }
        Ok((is_absolute_uri(&reference) || reference.starts_with("//"))
            .then_some(ImageSource::External(reference)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decoder_preflight_rejects_large_reserve_before_mime_parse() {
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\nQQ==\r\n--b\r\nContent-Type: text/plain\r\n\r\n0123456789abcdef\r\n--b--\r\n";
        assert!(matches!(
            preflight_base64_decoder_allocations_with_limit(input, 8),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
    }

    #[test]
    fn base64_decoder_preflight_allows_small_remaining_input() {
        let input = b"Content-Transfer-Encoding: base64\r\n\r\nQQ==\r\n";
        assert!(preflight_base64_decoder_allocations_with_limit(input, 64).is_ok());
    }

    #[test]
    fn base64_decoder_preflight_matches_embedded_boundary_semantics() {
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\npreamblexx--b\r\nContent-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\nQQ==\r\n0123456789abcdef\r\n--b--\r\n";
        assert!(matches!(
            preflight_base64_decoder_allocations_with_limit(input, 8),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
    }

    #[test]
    fn base64_decoder_preflight_ignores_non_boundary_dashes_in_body_text() {
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"real\"\r\n\r\n--real\r\nContent-Type: text/plain\r\n\r\nbody --not-real Content-Transfer-Encoding: base64\r\n\r\n0123456789abcdef\r\n--real--\r\n";
        assert!(preflight_base64_decoder_allocations_with_limit(input, 8).is_ok());
    }

    #[test]
    fn quoted_printable_part_is_bounded_before_mime_parse() {
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>ok</p>\r\n--b\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n0123456789abcdef\r\n--b--\r\n";
        assert!(matches!(
            preflight_base64_decoder_allocations_with_limit(input, 8),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
    }

    #[test]
    fn quoted_printable_part_within_limit_passes_preflight() {
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>ok</p>\r\n--b\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n0123456789abcdef\r\n--b--\r\n";
        assert!(preflight_base64_decoder_allocations_with_limit(input, 64).is_ok());
    }
    fn nested_multipart_chain(levels: usize) -> Vec<u8> {
        let mut out = Vec::from(
            b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b0\"\r\n\r\n"
                .as_slice(),
        );
        for level in 0..levels {
            out.extend_from_slice(
                format!(
                    "--b{level}\r\nContent-Type: multipart/mixed; boundary=\"b{}\"\r\n\r\n",
                    level + 1
                )
                .as_bytes(),
            );
        }
        out.extend_from_slice(
            format!(
                "--b{levels}\r\nContent-Type: text/html\r\n\r\n<p>leaf</p>\r\n--b{levels}--\r\n"
            )
            .as_bytes(),
        );
        for level in (0..levels).rev() {
            out.extend_from_slice(format!("--b{level}--\r\n").as_bytes());
        }
        out
    }

    fn base64_for_test(data: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
            let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((n >> 18) & 63) as usize] as char);
            out.push(TABLE[((n >> 12) & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[((n >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(n & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    #[test]
    fn oversized_root_header_block_is_rejected() {
        let mut input = Vec::from(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\nX-Pad: "
                .as_bytes(),
        );
        input.extend(std::iter::repeat_n(b'a', 70_000));
        input.extend_from_slice(
            b"\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>ok</p>\r\n--b--\r\n",
        );
        assert!(matches!(
            preflight_base64_decoder_allocations(&input),
            Err(ConvertError::Malformed { .. })
        ));
    }

    #[test]
    fn base64_encapsulated_message_is_walked_after_decode() {
        // A 70-level multipart chain (beyond MAX_MIME_DEPTH) hidden inside a
        // base64-encoded message/rfc822 part: the preflight must decode it
        // (within limits) and walk the inner tree, not scan the base64 text as
        // if it were MIME headers.
        let chain = nested_multipart_chain(70);
        let encoded = base64_for_test(&chain);
        let mut input = Vec::from(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: base64\r\n\r\n".as_bytes(),
        );
        input.extend_from_slice(encoded.as_bytes());
        input.extend_from_slice(b"\r\n--b--\r\n");
        assert!(matches!(
            preflight_base64_decoder_allocations(&input),
            Err(ConvertError::ResourceLimit { limit: "max_mime_depth", .. })
        ));
    }

    #[test]
    fn boundary_with_transport_padding_is_still_a_delimiter() {
        // RFC 2046 allows LWSP between the boundary and the CRLF; the padded
        // part must still be found so its QP body stays bounded.
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b  \r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n0123456789abcdefghijklmnop\r\n--b--\r\n";
        assert!(matches!(
            preflight_base64_decoder_allocations_with_limit(input, 8),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
        assert!(preflight_base64_decoder_allocations_with_limit(input, 64).is_ok());
    }

    #[test]
    fn encapsulated_message_chains_are_depth_checked() {
        // 70 nested message/rfc822 levels without multipart wrappers must
        // still hit the MIME depth budget.
        let mut inner = Vec::from(
            b"MIME-Version: 1.0\r\nContent-Type: text/html\r\n\r\n<p>leaf</p>\r\n".as_slice(),
        );
        for _ in 0..70 {
            let mut next =
                Vec::from(b"MIME-Version: 1.0\r\nContent-Type: message/rfc822\r\n\r\n".as_slice());
            next.extend_from_slice(&inner);
            inner = next;
        }
        let mut input = Vec::from(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: message/rfc822\r\n\r\n".as_bytes(),
        );
        input.extend_from_slice(&inner);
        input.extend_from_slice(b"\r\n--b--\r\n");
        assert!(matches!(
            preflight_base64_decoder_allocations(&input),
            Err(ConvertError::ResourceLimit { limit: "max_mime_depth", .. })
        ));
    }

    #[test]
    fn encoded_rfc822_chains_below_the_first_level_are_depth_checked() {
        // Every other chain level hides behind base64: an encapsulated
        // message only reveals the next link after decoding, so the
        // preflight must decode before recursing. Otherwise the walk stops
        // at the first encoded level (no MIME headers are visible in the
        // base64 text) while mail-parser keeps nesting.
        let mut inner = Vec::from(
            b"MIME-Version: 1.0\r\nContent-Type: text/html\r\n\r\n<p>leaf</p>\r\n".as_slice(),
        );
        for level in 0..70 {
            let mut next =
                Vec::from(b"MIME-Version: 1.0\r\nContent-Type: message/rfc822\r\n".as_slice());
            if level % 2 == 0 {
                next.extend_from_slice(b"Content-Transfer-Encoding: base64\r\n\r\n");
                next.extend_from_slice(base64_for_test(&inner).as_bytes());
            } else {
                next.extend_from_slice(b"\r\n");
                next.extend_from_slice(&inner);
            }
            inner = next;
        }
        let mut input = Vec::from(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: message/rfc822\r\n\r\n".as_bytes(),
        );
        input.extend_from_slice(&inner);
        input.extend_from_slice(b"\r\n--b--\r\n");
        assert!(matches!(
            preflight_base64_decoder_allocations(&input),
            Err(ConvertError::ResourceLimit { limit: "max_mime_depth", .. })
        ));
    }

    /// Quote every colon as =3A so the wrapped bytes no longer expose MIME
    /// headers to a raw scan; valid quoted-printable that decodes back to
    /// the original message.
    fn qp_escape_colons(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for byte in data {
            if *byte == b':' {
                out.extend_from_slice(b"=3A");
            } else {
                out.push(*byte);
            }
        }
        out
    }

    #[test]
    fn quoted_printable_rfc822_chains_are_depth_checked() {
        // Each level's payload has its colons quoted-printable-escaped, so
        // the next chain link is only visible after decoding; the preflight
        // must decode quoted-printable encapsulated bodies before recursing
        // or the depth budget stops at the first encoded level.
        let mut inner = Vec::from(
            b"MIME-Version: 1.0\r\nContent-Type: text/html\r\n\r\n<p>leaf</p>\r\n".as_slice(),
        );
        for _ in 0..70 {
            let mut next = Vec::from(
                b"MIME-Version: 1.0\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n"
                    .as_slice(),
            );
            next.extend_from_slice(&qp_escape_colons(&inner));
            inner = next;
        }
        let mut input = Vec::from(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: message/rfc822\r\n\r\n".as_bytes(),
        );
        input.extend_from_slice(&inner);
        input.extend_from_slice(b"\r\n--b--\r\n");
        assert!(matches!(
            preflight_base64_decoder_allocations(&input),
            Err(ConvertError::ResourceLimit { limit: "max_mime_depth", .. })
        ));
    }

    #[test]
    fn fake_close_delimiter_in_preamble_does_not_end_the_walk() {
        // "--b--junk" is not a closing delimiter (no line end after the
        // dashes), so the walk must continue to the real parts and bound the
        // quoted-printable body (26 bytes > 16).
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\npreamble --b--junk\r\n--b\r\nContent-Type: text/html\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n0123456789abcdefghijklmnop\r\n--b--\r\n";
        assert!(matches!(
            preflight_base64_decoder_allocations_with_limit(input, 16),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
    }

    #[test]
    fn embedded_boundary_bytes_do_not_truncate_quoted_printable_regions() {
        let input = b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>ok</p>\r\n--b\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nAAAA--bBBBBBBBBBBBBBBBBBBBB\r\n--b--\r\n";
        // The region ends at the real line-start boundary (27 encoded bytes),
        // not at the embedded mid-line --b (4 bytes), so 16 rejects and 64 passes.
        assert!(matches!(
            preflight_base64_decoder_allocations_with_limit(input, 16),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
        assert!(preflight_base64_decoder_allocations_with_limit(input, 64).is_ok());
    }
}
