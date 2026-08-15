//! Package target resolution per OPC/EPUB URI semantics, applied before any
//! archive lookup.
//!
//! The reference is treated as a URI: fragment and query split off first,
//! then each path segment is percent-decoded *after* segmentation. A segment
//! whose decoded form would introduce structure (`/`, `\`, `.`, `..`) is
//! rejected — encoded traversal never becomes path structure.

use crate::error::ConvertError;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// Normalized archive path (no leading slash).
    pub(crate) path: String,
    pub(crate) fragment: Option<String>,
}

/// Resolve a relative or package-absolute reference against the part it
/// appears in.
pub(crate) fn resolve(base_part: &str, reference: &str) -> Result<Target, ConvertError> {
    let (reference, fragment) = match reference.split_once('#') {
        Some((r, f)) => (r, Some(decode_component(f))),
        None => (reference, None),
    };
    let reference = reference.split_once('?').map_or(reference, |(r, _)| r);
    if reference.is_empty() {
        // Fragment-only reference: the target is the base part itself.
        return Ok(Target { path: base_part.to_string(), fragment });
    }

    let mut segments: Vec<String> = Vec::new();
    if !reference.starts_with('/') {
        // Start from the base part's directory.
        if let Some((dir, _)) = base_part.rsplit_once('/') {
            segments.extend(dir.split('/').filter(|s| !s.is_empty()).map(str::to_string));
        }
    }
    for raw in reference.split('/') {
        match raw {
            "" | "." => {}
            ".." => {
                // Dot segments resolve against the base, clamped at the
                // package root (a traversal above root is producer sloppiness
                // in the wild; clamping preserves OPC behavior).
                segments.pop();
            }
            raw => {
                let decoded = decode_component(raw);
                if decoded.contains('/') || decoded.contains('\\') {
                    return Err(ConvertError::malformed(format!(
                        "percent-encoded separator in package reference segment {raw:?}"
                    )));
                }
                if decoded == "." || decoded == ".." {
                    return Err(ConvertError::malformed(format!(
                        "percent-encoded traversal in package reference segment {raw:?}"
                    )));
                }
                segments.push(decoded);
            }
        }
    }
    Ok(Target { path: segments.join("/"), fragment })
}

/// Percent-decode one URI component. Infallible by design: a `%` not
/// followed by two hex digits passes through literally (producers emit such
/// names), and non-UTF-8 decoded bytes degrade lossily - the result is only
/// ever matched against archive entry names, where a near-miss simply fails
/// the lookup.
fn decode_component(component: &str) -> String {
    let bytes = component.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = component.get(i..).and_then(|rest| rest.get(1..3))
            // from_str_radix alone also accepts `+5`; only true hex digit
            // pairs decode.
            && hex.bytes().all(|b| b.is_ascii_hexdigit())
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn decode_fragment(fragment: &str) -> String {
    decode_component(fragment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(base: &str, r: &str) -> String {
        resolve(base, r).unwrap().path
    }

    #[test]
    fn relative_against_base_directory() {
        assert_eq!(path("word/document.xml", "media/image1.png"), "word/media/image1.png");
        assert_eq!(path("word/document.xml", "styles.xml"), "word/styles.xml");
        assert_eq!(path("content.opf", "ch1.xhtml"), "ch1.xhtml");
    }

    #[test]
    fn absolute_targets_resolve_from_root() {
        assert_eq!(path("word/document.xml", "/docProps/core.xml"), "docProps/core.xml");
    }

    #[test]
    fn dot_segments_resolve_and_clamp_at_root() {
        assert_eq!(path("OEBPS/text/ch1.xhtml", "../images/i.png"), "OEBPS/images/i.png");
        assert_eq!(path("a/b.xml", "./c.xml"), "a/c.xml");
        assert_eq!(path("a/b.xml", "../../../x.xml"), "x.xml");
    }

    #[test]
    fn fragments_and_queries_split_off() {
        let t = resolve("OEBPS/ch1.xhtml", "ch2.xhtml#sec-2").unwrap();
        assert_eq!(t.path, "OEBPS/ch2.xhtml");
        assert_eq!(t.fragment.as_deref(), Some("sec-2"));
        let t = resolve("OEBPS/ch1.xhtml", "ch2.xhtml?x=1#f").unwrap();
        assert_eq!(t.path, "OEBPS/ch2.xhtml");
        assert_eq!(t.fragment.as_deref(), Some("f"));
    }

    #[test]
    fn fragments_are_percent_decoded() {
        let t = resolve("OEBPS/ch1.xhtml", "ch2.xhtml#caf%C3%A9%20menu").unwrap();
        assert_eq!(t.fragment.as_deref(), Some("café menu"));
        assert_eq!(decode_fragment("caf%C3%A9"), "café");
    }

    #[test]
    fn percent_decoding_within_segments() {
        assert_eq!(path("OEBPS/x.opf", "my%20file.xhtml"), "OEBPS/my file.xhtml");
    }

    #[test]
    fn encoded_separators_rejected() {
        assert!(resolve("a/b.xml", "x%2Fy.xml").is_err());
        assert!(resolve("a/b.xml", "x%5Cy.xml").is_err());
    }

    #[test]
    fn encoded_traversal_rejected() {
        assert!(resolve("a/b.xml", "%2E%2E/secret.xml").is_err());
        assert!(resolve("a/b.xml", "%2e%2e/secret.xml").is_err());
    }

    #[test]
    fn plain_fragment_only_reference() {
        let t = resolve("OEBPS/ch1.xhtml", "#local").unwrap();
        assert_eq!(t.path, "OEBPS/ch1.xhtml");
        assert_eq!(t.fragment.as_deref(), Some("local"));
    }
}
