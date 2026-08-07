//! Fixture corpus snapshot harness.
//!
//! One snapshot per fixture of the single conversion result: the Markdown
//! output, or the terminal error. Logs are never snapshotted. Fixtures under
//! `abuse/` are resource-abuse shapes and are exercised by dedicated tests once
//! limits exist, not by the corpus sweep. Malformed fixtures encode their
//! expected outcome as `name--<outcome>.ext` where outcome is one of
//! `recovers`, `skips`, `ignores`, `errors`.

mod common;

use anydoc::model::{Block, Inline, LinkTarget};
use common::{fixture_root, walk};
use std::fmt::Write as _;
use std::path::Path;

/// Convert one file, capturing panics so a bad parser records a baseline
/// instead of aborting the whole harness.
fn convert(path: &Path) -> String {
    let result = std::panic::catch_unwind(|| anydoc::to_markdown(path));
    match result {
        Ok(Ok(md)) => md,
        Ok(Err(e)) => format!("ERROR: {e:#}"),
        Err(_) => "PANIC".to_string(),
    }
}

fn expected_outcome(path: &Path) -> Option<&'static str> {
    let stem = path.file_stem()?.to_str()?;
    let (_, outcome) = stem.rsplit_once("--")?;
    match outcome {
        "recovers" => Some("recovers"),
        "skips" => Some("skips"),
        "ignores" => Some("ignores"),
        "errors" => Some("errors"),
        _ => None,
    }
}

#[test]
fn corpus() {
    let root = fixture_root();
    let mut files = Vec::new();
    walk(&root, &mut files);
    for path in files {
        let rel = path.strip_prefix(&root).unwrap();
        if rel.starts_with("abuse") {
            continue;
        }
        let name = rel.to_string_lossy().replace(['\\', '/'], "__");
        let output = convert(&path);
        // Once the unified recovery policy exists (P1), annotated malformed
        // fixtures must match their single expected outcome.
        if let Some(outcome) = expected_outcome(&path) {
            let is_err = output.starts_with("ERROR: ") || output == "PANIC";
            match outcome {
                "errors" => {
                    assert!(is_err, "{name}: annotated `errors` but converted successfully")
                }
                // recovers/skips/ignores fixtures must produce usable output
                // post-refactor; during the baseline phase current behavior is
                // recorded as-is, so this stays a snapshot-only expectation
                // until P1 flips `STRICT_ANNOTATIONS` on.
                _ if STRICT_ANNOTATIONS => {
                    assert!(!is_err, "{name}: annotated `{outcome}` but failed: {output}")
                }
                _ => {}
            }
        }
        insta::assert_snapshot!(name, output);
    }
}

/// Annotated malformed fixtures must produce their single expected outcome:
/// `recovers`/`skips`/`ignores` convert successfully, `errors` return a typed
/// error. (Baseline recording ran with this off; it is enforced from P1 on.)
const STRICT_ANNOTATIONS: bool = true;

/// Every well-formed corpus fixture must be identified from its bytes
/// alone. Only the signature-less text formats (csv) legitimately need the
/// extension fallback.
#[test]
fn fixtures_detect_from_content() {
    use anydoc::Format;
    let expected = [
        ("doc", Some(Format::Doc)),
        ("docx", Some(Format::Docx)),
        ("epub", Some(Format::Epub)),
        ("odp", Some(Format::Odp)),
        ("ods", Some(Format::Ods)),
        ("odt", Some(Format::Odt)),
        ("pdf", Some(Format::Pdf)),
        ("ppt", Some(Format::Ppt)),
        ("pptx", Some(Format::Pptx)),
        ("rtf", Some(Format::Rtf)),
        ("xls", Some(Format::Excel)),
        ("xlsx", Some(Format::Excel)),
        ("csv", None),
    ];
    let root = fixture_root();
    for (dir, format) in expected {
        let mut files = Vec::new();
        walk(&root.join(dir), &mut files);
        assert!(!files.is_empty(), "no fixtures under {dir}/");
        for path in files {
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(
                Format::from_bytes(&bytes),
                format,
                "detection mismatch for {}",
                path.display()
            );
        }
    }
}

fn slide_anchor_ids(doc: &anydoc::model::Document) -> Vec<String> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(inlines) => match inlines.as_slice() {
                [Inline::Anchor(id)] if id.starts_with("slide-") => Some(id.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn has_slide_link(doc: &anydoc::model::Document, target: &str) -> bool {
    doc.blocks.iter().any(|block| match block {
        Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
            inlines.iter().any(|inline| {
                matches!(
                    inline,
                    Inline::Link { target: LinkTarget::Anchor(id), .. } if id == target
                )
            })
        }
        _ => false,
    })
}

#[test]
fn presentation_slides_are_anchored_and_separated() {
    for (directory, filename, format) in [
        ("pptx", "handmade-slide-identity.pptx", anydoc::Format::Pptx),
        ("ppt", "handmade-slide-identity.ppt", anydoc::Format::Ppt),
        ("odp", "handmade-slide-identity.odp", anydoc::Format::Odp),
    ] {
        let path = fixture_root().join(directory).join(filename);
        let bytes = std::fs::read(&path).unwrap();
        let doc = anydoc::to_document(&bytes, format).unwrap();
        assert_eq!(
            slide_anchor_ids(&doc),
            ["slide-1", "slide-2", "slide-3", "slide-4"],
            "{filename}: slide anchors must follow source order"
        );
        let rules = doc.blocks.iter().filter(|block| matches!(block, Block::Rule)).count();
        assert_eq!(rules, 2, "{filename}: empty slides must not double a rule");
        assert!(!matches!(doc.blocks.first(), Some(Block::Rule)));
        assert!(!matches!(doc.blocks.last(), Some(Block::Rule)));
        let markdown = anydoc::to_markdown(&path).unwrap();
        assert!(!markdown.contains("---\n\n---"), "{filename}: empty slides must not double rules");
    }
}

#[test]
fn pptx_slide_links_still_target_the_sequential_anchor() {
    let path = fixture_root().join("pptx").join("handmade-links.pptx");
    let bytes = std::fs::read(path).unwrap();
    let doc = anydoc::to_document(&bytes, anydoc::Format::Pptx).unwrap();
    assert_eq!(slide_anchor_ids(&doc), ["slide-1", "slide-2"]);
    assert!(has_slide_link(&doc, "slide-2"));
}

/// Embedded object payloads land in `Document::assets` with their identity
/// and media type (the Markdown output shows only the alt text).
#[test]
fn embedded_ole_payload_is_retained() {
    let path = fixture_root().join("pptx").join("handmade-order.pptx");
    let bytes = std::fs::read(&path).unwrap();
    let doc = anydoc::to_document(&bytes, anydoc::Format::Pptx).unwrap();
    let ole = doc
        .assets
        .iter()
        .find(|a| a.media_type == "application/vnd.ms-ole-object")
        .expect("OLE payload retained as an asset");
    assert_eq!(ole.bytes, b"OLE-PAYLOAD-STAND-IN".repeat(4));
}

/// Standard Word OLE markup places a VML preview image next to the
/// `o:OLEObject`; the object payload (not the preview) must be retained.
#[test]
fn docx_ole_payload_wins_over_its_preview_image() {
    let path = fixture_root().join("docx").join("handmade-ole.docx");
    let bytes = std::fs::read(&path).unwrap();
    let doc = anydoc::to_document(&bytes, anydoc::Format::Docx).unwrap();
    let ole = doc
        .assets
        .iter()
        .find(|a| a.media_type == "application/vnd.ms-ole-object")
        .expect("OLE payload retained as an asset");
    assert_eq!(ole.bytes, b"DOCX-OLE-PAYLOAD".repeat(4));
}

/// Repeated references to one part must neither re-decompress against the
/// archive budget nor duplicate the retained asset (S12).
#[test]
fn repeated_part_references_convert_and_dedupe() {
    let path = fixture_root().join("docx").join("handmade-manyrefs.docx");
    let bytes = std::fs::read(&path).unwrap();
    let doc = anydoc::to_document(&bytes, anydoc::Format::Docx).unwrap();
    assert_eq!(doc.assets.len(), 1, "one shared asset for seventy references");
}

/// The binary DOC inline picture (sprmCPicLocation -> PICF + OfficeArt in
/// the Data stream) is retained as an asset.
#[test]
fn doc_inline_picture_is_retained() {
    let bytes = std::fs::read(fixture_root().join("doc").join("text.doc")).unwrap();
    let doc = anydoc::to_document(&bytes, anydoc::Format::Doc).unwrap();
    assert!(
        doc.assets.iter().any(|a| a.media_type.starts_with("image/")),
        "expected an extracted picture asset, got {:?}",
        doc.assets.iter().map(|a| (&a.media_type, a.bytes.len())).collect::<Vec<_>>()
    );
}

/// The RTF `\pict` payload is retained as an asset (the Markdown output
/// shows only the alt text, which is empty for pictures without one).
#[test]
fn rtf_inline_picture_is_retained() {
    let bytes = std::fs::read(fixture_root().join("rtf").join("text.rtf")).unwrap();
    let doc = anydoc::to_document(&bytes, anydoc::Format::Rtf).unwrap();
    let png = doc.assets.iter().find(|a| a.media_type == "image/png");
    let png = png.expect("png pict retained as an asset");
    assert!(png.bytes.starts_with(&[0x89, b'P', b'N', b'G']), "payload decodes from hex");
}

/// Resource-abuse fixtures must hard-fail with `ResourceLimit` - the one
/// class of malformed input that never converts.
#[test]
fn abuse_fixtures_hard_fail() {
    let root = fixture_root().join("abuse");
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(!files.is_empty(), "abuse fixture set is missing");
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let result = anydoc::to_markdown(&path);
        assert!(
            matches!(result, Err(anydoc::ConvertError::ResourceLimit { .. })),
            "{name}: expected a ResourceLimit error, got {result:?}"
        );
    }
}

/// Local-only sweep over the ~100 real-world files in `samples/` (gitignored).
/// Snapshots a compact digest (length + hash + head) per file so drift is
/// detected without committing megabytes of output. Run with:
/// `cargo test --test snapshots -- --ignored`
#[test]
#[ignore = "requires the local gitignored samples/ directory"]
fn samples_sweep() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples");
    if !root.is_dir() {
        eprintln!("samples/ not present; skipping sweep");
        return;
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    for path in files {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("pdf") {
            continue; // pdf is an output target, not an input format
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let output = convert(&path);
        let digest = digest(&output);
        insta::assert_snapshot!(format!("sample__{name}"), digest);
    }
}

fn digest(output: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash: String =
        Sha256::digest(output.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let mut s = String::new();
    writeln!(s, "chars: {}", output.chars().count()).unwrap();
    writeln!(s, "sha256: {hash}").unwrap();
    writeln!(s, "--- head ---").unwrap();
    let head: String = output.chars().take(2000).collect();
    s.push_str(&head);
    s
}
