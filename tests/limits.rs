//! Each structural limit fires on its own shape and names itself, including
//! on inputs far past the cap (generated here rather than committed).

use std::io::{Cursor, Write};
use std::path::Path;

fn limit_of(result: Result<String, anydoc::ConvertError>) -> &'static str {
    match result {
        Err(anydoc::ConvertError::ResourceLimit { limit, .. }) => limit,
        other => panic!("expected a ResourceLimit error, got {other:?}"),
    }
}

#[test]
fn fixtures_name_their_limit() {
    for (name, limit) in [
        ("deepnest--errors.ppt", "max_record_depth"),
        ("deepnotes--errors.ppt", "max_record_depth"),
        ("manyattrs--errors.docx", "max_xml_attrs"),
        ("deepgroups--errors.rtf", "max_rtf_group_depth"),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/abuse").join(name);
        assert_eq!(limit_of(anydoc::to_markdown(&path)), limit, "{name}");
    }
}

fn ppt(document: &[u8]) -> Vec<u8> {
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    file.create_stream("Current User").unwrap();
    file.create_stream("PowerPoint Document").unwrap().write_all(document).unwrap();
    file.flush().unwrap();
    file.into_inner().into_inner()
}

/// Recovery-mode notes nesting far past the cap fails fast, even on a small
/// thread stack like the ones host runtimes give worker threads.
#[test]
fn deep_notes_nesting_in_recovery() {
    let mut stream = Vec::new();
    let depth = 100_000u32;
    for level in 0..depth {
        let len = (depth - level - 1) * 8 + 12;
        stream.extend_from_slice(&0x000Fu16.to_le_bytes());
        stream.extend_from_slice(&0x03F0u16.to_le_bytes());
        stream.extend_from_slice(&len.to_le_bytes());
    }
    stream.extend_from_slice(&0x0000u16.to_le_bytes());
    stream.extend_from_slice(&0x0FA8u16.to_le_bytes());
    stream.extend_from_slice(&4u32.to_le_bytes());
    stream.extend_from_slice(b"deep");
    let bytes = ppt(&stream);
    let limit = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || limit_of(anydoc::to_markdown_bytes(&bytes, anydoc::Format::Ppt)))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(limit, "max_record_depth");
}

#[test]
fn deep_rtf_groups() {
    let mut rtf = b"{\\rtf1 ".to_vec();
    rtf.extend(std::iter::repeat_n(b'{', 1_000_000));
    assert_eq!(
        limit_of(anydoc::to_markdown_bytes(&rtf, anydoc::Format::Rtf)),
        "max_rtf_group_depth"
    );
}

#[test]
fn rtf_nesting_under_the_cap_converts() {
    let mut rtf = b"{\\rtf1 ".to_vec();
    rtf.extend(std::iter::repeat_n(b'{', 200));
    rtf.extend_from_slice(b"inner");
    rtf.extend(std::iter::repeat_n(b'}', 201));
    let md = anydoc::to_markdown_bytes(&rtf, anydoc::Format::Rtf).unwrap();
    assert!(md.contains("inner"), "{md}");
}
