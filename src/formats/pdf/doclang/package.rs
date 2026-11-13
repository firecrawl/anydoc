//! OPC packaging: a `.dclx` archive is a ZIP carrying `document.xml`, the
//! OPC plumbing (`[Content_Types].xml`, `_rels/.rels`), extracted assets,
//! and the original source bytes (Annex B of the DocLang spec).

use crate::error::ConvertError;
use std::collections::BTreeSet;
use std::io::{Cursor, Write};

/// A file to store in the archive; `path` is relative to the package root
/// without a leading slash (`assets/img1.png`).
pub(crate) struct Part {
    pub path: String,
    pub bytes: Vec<u8>,
}

const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const DOCREL: &str = "http://doclang.ai/ns/package/2026/relationships/document";

/// Build the `.dclx` ZIP. `source` carries the original document bytes for
/// the optional `/source` part.
pub(crate) fn pack(
    document_xml: &[u8],
    parts: &[Part],
    source: Option<&[u8]>,
) -> Result<Vec<u8>, ConvertError> {
    let opts = zip::write::SimpleFileOptions::default();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));

    w.start_file("[Content_Types].xml", opts).map_err(zip_err)?;
    w.write_all(content_types(parts, source.is_some()).as_bytes()).map_err(ConvertError::Io)?;
    w.start_file("_rels/.rels", opts).map_err(zip_err)?;
    w.write_all(
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <Relationships xmlns=\"{RELS_NS}\">\
             <Relationship Id=\"rId1\" Type=\"{DOCREL}\" Target=\"document.xml\"/>\
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
    if let Some(bytes) = source {
        w.start_file("source", opts).map_err(zip_err)?;
        w.write_all(bytes).map_err(ConvertError::Io)?;
    }
    Ok(w.finish().map_err(zip_err)?.into_inner())
}

fn zip_err(e: zip::result::ZipError) -> ConvertError {
    ConvertError::malformed(format!("cannot build DocLang archive: {e}"))
}

/// `[Content_Types].xml`: a `<Default>` for every extension present plus
/// `<Override>`s for `/document.xml` and, when present, `/source`.
fn content_types(parts: &[Part], has_source: bool) -> String {
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
    if has_source {
        out.push_str("<Override PartName=\"/source\" ContentType=\"application/pdf\"/>");
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
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_has_required_parts() {
        let parts = vec![Part { path: "assets/a.png".into(), bytes: vec![1, 2, 3] }];
        let zip_bytes = pack(b"<doclang/>", &parts, Some(b"%PDF")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(&zip_bytes[..])).unwrap();
        let names: Vec<String> =
            (0..zip.len()).filter_map(|i| zip.name_for_index(i)).map(String::from).collect();
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
    }
}
