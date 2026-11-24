//! OPC packaging: a `.dclx` archive is a ZIP carrying `document.xml`, the
//! OPC plumbing (`[Content_Types].xml`, `_rels/.rels`), extracted assets,
//! and the original source bytes (Annex B of the DocLang spec).

use crate::error::ConvertError;
use crate::package::limits;
use std::collections::BTreeSet;
use std::io::{Cursor, Write};

/// A file to store in the archive; `path` is relative to the package root
/// without a leading slash (`assets/img1.png`).
pub(crate) struct Part {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// The original document an archive was produced from, stored unmodified
/// as the `/source` part (Annex B of the DocLang spec).
pub(crate) struct SourcePart<'a> {
    /// The source bytes, exactly as they went in.
    pub bytes: &'a [u8],
    /// MIME type declared by the `/source` `<Override>`.
    pub content_type: &'static str,
}

const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const DOCREL: &str = "http://doclang.ai/ns/package/2026/relationships/document";
const SOURCEREL: &str = "http://doclang.ai/ns/package/2026/relationships/source";

/// Build the `.dclx` ZIP. `source` carries the original document bytes for
/// the optional `/source` part, identified by a `source` relationship.
pub(crate) fn pack(
    document_xml: &[u8],
    parts: &[Part],
    source: Option<SourcePart<'_>>,
) -> Result<Vec<u8>, ConvertError> {
    check_limits(document_xml, parts, source.as_ref().map(|s| s.bytes))?;
    let opts = zip::write::SimpleFileOptions::default();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));

    w.start_file("[Content_Types].xml", opts).map_err(zip_err)?;
    w.write_all(content_types(parts, source.as_ref().map(|s| s.content_type)).as_bytes())
        .map_err(ConvertError::Io)?;
    let source_rel = if source.is_some() {
        format!("<Relationship Id=\"rId2\" Type=\"{SOURCEREL}\" Target=\"source\"/>")
    } else {
        String::new()
    };
    w.start_file("_rels/.rels", opts).map_err(zip_err)?;
    w.write_all(
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <Relationships xmlns=\"{RELS_NS}\">\
             <Relationship Id=\"rId1\" Type=\"{DOCREL}\" Target=\"document.xml\"/>\
             {source_rel}\
             </Relationships>"
        )
        .as_bytes(),
    )
    .map_err(ConvertError::Io)?;
    w.start_file("document.xml", opts).map_err(zip_err)?;
    w.write_all(document_xml).map_err(ConvertError::Io)?;
    for part in parts {
        w.start_file(&part.path, opts).map_err(zip_err)?;
        w.write_all(&part.bytes).map_err(ConvertError::Io)?;
    }
    if let Some(source) = source {
        w.start_file("source", opts).map_err(zip_err)?;
        w.write_all(source.bytes).map_err(ConvertError::Io)?;
    }
    Ok(w.finish().map_err(zip_err)?.into_inner())
}

/// Bound the archive like an archive read: the same per-part, total-bytes,
/// and entry-count caps apply to what we write out.
fn check_limits(
    document_xml: &[u8],
    parts: &[Part],
    source: Option<&[u8]>,
) -> Result<(), ConvertError> {
    let over = |limit: &'static str, detail: String| ConvertError::ResourceLimit { limit, detail };
    if document_xml.len() as u64 > limits::MAX_ENTRY_BYTES {
        return Err(over(
            "max_entry_bytes",
            format!("document.xml is {} bytes", document_xml.len()),
        ));
    }
    // The reader counts every ZIP entry: the three mandatory OPC parts
    // and `/source` ride along with the caller's parts.
    let fixed = 3 + usize::from(source.is_some());
    if parts.len() + fixed > limits::MAX_ENTRY_COUNT {
        return Err(over("max_entry_count", format!("{} entries", parts.len() + fixed)));
    }
    if let Some(source) = source
        && source.len() as u64 > limits::MAX_ENTRY_BYTES
    {
        return Err(over("max_entry_bytes", format!("source is {} bytes", source.len())));
    }
    // 4 KiB covers the OPC plumbing ([Content_Types].xml, _rels/.rels)
    // plus ZIP local/central headers, so the emitted archive stays
    // within the logical content-byte budget.
    let mut total = document_xml.len() as u64 + source.map_or(0, |s| s.len() as u64) + 4_096;
    for part in parts {
        if part.bytes.len() as u64 > limits::MAX_ENTRY_BYTES {
            return Err(over(
                "max_entry_bytes",
                format!("{} is {} bytes", part.path, part.bytes.len()),
            ));
        }
        total += part.bytes.len() as u64;
    }
    if total > limits::MAX_TOTAL_BYTES {
        return Err(over(
            "max_total_bytes",
            format!("archive exceeds {} bytes", limits::MAX_TOTAL_BYTES),
        ));
    }
    Ok(())
}

fn zip_err(e: zip::result::ZipError) -> ConvertError {
    ConvertError::malformed(format!("cannot build DocLang archive: {e}"))
}

/// `[Content_Types].xml`: a `<Default>` for every extension present plus
/// `<Override>`s for `/document.xml` and, when present, `/source`.
fn content_types(parts: &[Part], source_content_type: Option<&'static str>) -> String {
    let mut defaults = BTreeSet::from(["rels".to_string()]);
    for part in parts {
        if let Some(ext) = part.path.rsplit('.').next().filter(|e| *e != part.path) {
            defaults.insert(ext.to_ascii_lowercase());
        }
    }
    let mut out =
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"{CONTENT_TYPES}\">");
    for ext in defaults {
        out.push_str(&format!("<Default Extension=\"{ext}\" ContentType=\"{}\"/>", mime(&ext)));
    }
    out.push_str(
        "<Override PartName=\"/document.xml\" \
         ContentType=\"application/vnd.doclang.document+xml\"/>",
    );
    if let Some(content_type) = source_content_type {
        out.push_str(&format!("<Override PartName=\"/source\" ContentType=\"{content_type}\"/>"));
    }
    out.push_str("</Types>");
    out
}

/// MIME type for an archive part extension.
fn mime(ext: &str) -> &'static str {
    match ext {
        "rels" => "application/vnd.openxmlformats-package.relationships+xml",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "jp2" | "jpx" => "image/jp2",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_has_required_parts() {
        let parts = vec![Part { path: "assets/a.png".into(), bytes: vec![1, 2, 3] }];
        let zip_bytes = pack(
            b"<doclang/>",
            &parts,
            Some(SourcePart { bytes: b"%PDF", content_type: "application/pdf" }),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(&zip_bytes[..])).unwrap();
        let names: Vec<String> = (0..zip.len())
            .filter_map(|i| zip.name_for_index(i))
            .filter_map(|n| n.ok())
            .map(|n| n.into_owned())
            .collect();
        for required in
            ["[Content_Types].xml", "_rels/.rels", "document.xml", "assets/a.png", "source"]
        {
            assert!(names.iter().any(|n| n == required), "missing {required}");
        }
        let mut ct = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("[Content_Types].xml").unwrap(), &mut ct)
            .unwrap();
        assert!(ct.contains("application/vnd.doclang.document+xml"));
        assert!(ct.contains("Extension=\"png\" ContentType=\"image/png\""));
        assert!(ct.contains("PartName=\"/source\" ContentType=\"application/pdf\""));
        let mut rels = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("_rels/.rels").unwrap(), &mut rels).unwrap();
        assert!(rels.contains("relationships/source\" Target=\"source\""));
    }
}
