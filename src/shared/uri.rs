//! URI classification shared across frontends (EPUB hrefs, ODF hrefs and
//! image references, Word field targets).

/// True when `s` starts with an RFC 3986 scheme followed by `:`
/// (`scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`). One-character
/// schemes are valid.
fn has_scheme(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let mut chars = s[..colon].chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// True for Windows drive-letter paths (`C:\...`, `C:/...`). They parse as
/// a one-letter scheme under RFC 3986, but in documents they are local file
/// paths (legacy Word HYPERLINK fields), never URIs.
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
}

/// True when `s` should be treated as an absolute URI with a scheme rather
/// than a relative/package reference.
pub fn is_absolute_uri(s: &str) -> bool {
    has_scheme(s) && !is_drive_path(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes_follow_rfc_3986() {
        assert!(has_scheme("https://e.com"));
        assert!(has_scheme("mailto:x@y"));
        assert!(has_scheme("a:relative-scheme-uri"), "one-character schemes are valid");
        assert!(has_scheme("view-source:x"));
        assert!(!has_scheme("1http:x"), "schemes must start with a letter");
        assert!(!has_scheme("no scheme here"));
        assert!(!has_scheme(":empty"));
        assert!(!has_scheme("path/with:colon"), "slash before the colon is not a scheme");
    }

    #[test]
    fn drive_paths_are_not_uris() {
        assert!(is_drive_path(r"C:\docs\a.doc"));
        assert!(is_drive_path("c:/docs/a.doc"));
        assert!(!is_drive_path("cs:whatever"));
        assert!(!is_absolute_uri(r"C:\docs\a.doc"));
        assert!(is_absolute_uri("https://e.com"));
    }
}
