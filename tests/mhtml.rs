use anydoc::model::{AssetId, Block, ImageSource, Inline};
use anydoc::{ConvertError, Format, to_document, to_markdown_bytes};

fn mhtml_fixture(source: &str) -> Vec<u8> {
    source.replace('\n', "\r\n").into_bytes()
}

#[test]
fn mhtml_extensions_are_named() {
    assert_eq!(Format::from_extension("mhtml"), Some(Format::Mhtml));
    assert_eq!(Format::from_extension("MHT"), Some(Format::Mhtml));
}

#[test]
fn multipart_related_html_is_detected_as_mhtml() {
    let mhtml = mhtml_fixture(
        r#"From: <Saved by Blink>
Snapshot-Content-Location: https://example.test/page
MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><p>Hello</p>
--b--
"#,
    );
    assert_eq!(Format::from_bytes(&mhtml), Some(Format::Mhtml));
}

#[test]
fn generic_multipart_alternative_email_is_not_mhtml() {
    let email = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="b"

--b
Content-Type: text/plain

Hello
--b
Content-Type: text/html

<p>Hello</p>
--b--
"#,
    );
    assert_eq!(Format::from_bytes(&email), None);
}

#[test]
fn ordinary_related_html_email_is_not_mhtml() {
    let email = mhtml_fixture(
        r#"From: sender@example.test
To: recipient@example.test
Subject: Inline image
MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8

<p>Hello <img src="cid:image@id"></p>
--b
Content-Type: image/png
Content-ID: <image@id>
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    assert_eq!(Format::from_bytes(&email), None);
}

#[test]
fn quoted_printable_html_root_converts() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=windows-1252
Content-Transfer-Encoding: quoted-printable
Content-Location: https://example.test/page

<!doctype html><p>caf=E9</p>
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "café\n");
}

#[test]
fn mime_charset_overrides_html_fallback() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=iso-8859-2
Content-Transfer-Encoding: quoted-printable
Content-Location: https://example.test/page

<!doctype html><meta charset=3D"utf-8"><p>=A3</p>
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "Ł\n");
}

#[test]
fn start_parameter_selects_the_related_html_root() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; start="<root@id>"; boundary="b"

--b
Content-Type: text/html
Content-ID: <other@id>

<p>wrong root</p>
--b
Content-Type: text/html
Content-ID: <root@id>

<p>right root</p>
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "right root\n");
}

#[test]
fn uppercase_cid_start_parameter_selects_the_related_html_root() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; start="CID:root@id"; boundary="b"

--b
Content-Type: text/html
Content-ID: <other@id>

<p>wrong root</p>
--b
Content-Type: text/html
Content-ID: <root@id>

<p>right root</p>
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "right root\n");
}

#[test]
fn related_mhtml_without_html_root_is_malformed() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/css

p { font-weight: bold }
--b--
"#,
    );
    let error = to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap_err();
    assert!(matches!(error, ConvertError::Malformed { .. }));
}

#[test]
fn linked_css_is_resolved_from_cid() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><link rel="stylesheet" href="cid:style@id"><p class="hidden">drop me</p><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-ID: <style@id>

.hidden { display: none }
.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "**keep me**\n");
}

#[test]
fn linked_css_and_inline_style_preserve_document_order() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><link rel="stylesheet" href="cid:style@id"><style>.strong { font-weight: normal }</style><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-ID: <style@id>

.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "keep me\n");
}

#[test]
fn relative_linked_css_resolves_against_root_content_location() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/docs/page.html

<!doctype html><link rel="stylesheet" href="styles/site.css"><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-Location: https://example.test/docs/styles/site.css

.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "**keep me**\n");
}

#[test]
fn html_base_href_resolves_relative_embedded_image() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/docs/page.html

<!doctype html><base href="https://cdn.example.test/assets/"><p><img alt="pixel" src="pixel.png"></p>
--b
Content-Type: image/png
Content-Location: https://cdn.example.test/assets/pixel.png
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn uppercase_cid_image_becomes_an_embedded_document_asset() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><p><img alt="pixel" src="CID:image@id"></p>
--b
Content-Type: image/png
Content-ID: <image@id>
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn cid_image_becomes_an_embedded_document_asset() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><p><img alt="pixel" src="cid:image@id"></p>
--b
Content-Type: image/png
Content-ID: <image@id>
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    assert_eq!(document.assets.len(), 1);
    assert_eq!(document.assets[0].media_type, "image/png");
    assert_eq!(document.assets[0].bytes, [0, 1, 2, 3]);
    match &document.blocks[0] {
        Block::Paragraph(inlines) => match &inlines[0] {
            Inline::Image { alt, source: ImageSource::Asset(AssetId(0)) } => {
                assert_eq!(alt, "pixel");
            }
            other => panic!("expected embedded image, got {other:?}"),
        },
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn content_location_image_becomes_an_embedded_document_asset() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><p><img alt="pixel" src="https://example.test/pixel.png"></p>
--b
Content-Type: image/png
Content-Location: https://example.test/pixel.png
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    assert_eq!(document.assets.len(), 1);
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn mhtml_detection_requires_exact_related_media_type() {
    let mhtml = mhtml_fixture(
        r#"Snapshot-Content-Location: https://example.test/page
MIME-Version: 1.0
Content-Type: multipart/relatedness; boundary="b"

--b
Content-Type: text/html

<!doctype html><p>Hello</p>
--b--
"#,
    );
    assert_eq!(Format::from_bytes(&mhtml), None);
}

#[test]
fn relative_css_part_content_location_resolves_against_root() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/docs/page.html

<!doctype html><link rel="stylesheet" href="styles/site.css"><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-Location: styles/site.css

.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "**keep me**\n");
}

#[test]
fn relative_image_part_content_location_resolves_against_root() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/docs/page.html

<!doctype html><p><img alt="pixel" src="images/pixel.png"></p>
--b
Content-Type: image/png
Content-Location: images/pixel.png
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn cid_identifier_matching_is_case_insensitive() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html

<!doctype html><p><img alt="pixel" src="cid:IMAGE@ID"></p>
--b
Content-Type: image/png
Content-ID: <image@id>
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn start_parameter_content_id_matching_is_case_insensitive() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; start="<ROOT@ID>"; boundary="b"

--b
Content-Type: text/html
Content-ID: <root@id>

<p>right root</p>
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "right root\n");
}

#[test]
fn relative_part_content_location_resolves_against_html_base() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/docs/page.html

<!doctype html><base href="https://cdn.example.test/assets/"><link rel="stylesheet" href="styles/site.css"><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-Location: styles/site.css

.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "**keep me**\n");
}

#[test]
fn empty_html_base_falls_back_to_root_content_location() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/docs/page.html

<!doctype html><base href=""><p><img alt="pixel" src="images/pixel.png"></p>
--b
Content-Type: image/png
Content-Location: https://example.test/docs/images/pixel.png
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    assert!(
        matches!(&document.blocks[0], Block::Paragraph(inlines) if matches!(&inlines[0], Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }))
    );
}

#[test]
fn image_fragment_is_ignored_for_embedded_resource_lookup() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/page.html

<!doctype html><p><img alt="pixel" src="https://example.test/pixel.png#view"></p>
--b
Content-Type: image/png
Content-Location: https://example.test/pixel.png
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    assert!(
        matches!(&document.blocks[0], Block::Paragraph(inlines) if matches!(&inlines[0], Inline::Image { source: ImageSource::Asset(AssetId(0)), .. }))
    );
}

#[test]
fn stylesheet_fragment_is_ignored_for_embedded_resource_lookup() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/page.html

<!doctype html><link rel="stylesheet" href="https://example.test/site.css#theme"><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-Location: https://example.test/site.css

.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "**keep me**\n");
}

#[test]
fn protocol_relative_mhtml_image_without_base_is_external() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8

<!doctype html><p><img alt="pixel" src="//cdn.example.test/image.png"></p>
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    assert!(
        matches!(&document.blocks[0], Block::Paragraph(inlines) if matches!(&inlines[0], Inline::Image { source: ImageSource::External(url), .. } if url == "//cdn.example.test/image.png"))
    );
}
#[test]
fn mhtml_root_enforces_html_depth_limit() {
    let html = format!("<!doctype html>{}deep{}", "<div>".repeat(300), "</div>".repeat(300));
    let mhtml = mhtml_fixture(&format!(
        "MIME-Version: 1.0\nContent-Type: multipart/related; type=\"text/html\"; boundary=\"b\"\n\n--b\nContent-Type: text/html; charset=utf-8\n\n{html}\n--b--\n"
    ));
    let error = to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap_err();
    assert!(matches!(error, ConvertError::ResourceLimit { limit: "max_xml_depth", .. }));
}

#[test]
fn bare_relative_image_does_not_match_content_id() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8

<!doctype html><p><img alt="pixel" src="image@id"></p>
--b
Content-Type: image/png
Content-ID: <image@id>
Content-Transfer-Encoding: base64

AAECAw==
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => {
            assert!(matches!(&inlines[0], Inline::Image { source: ImageSource::Unavailable, .. }))
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn content_location_fragment_is_ignored_for_resource_lookup() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Content-Type: multipart/related; type="text/html"; boundary="b"

--b
Content-Type: text/html; charset=utf-8
Content-Location: https://example.test/page.html

<!doctype html><link rel="stylesheet" href="https://example.test/site.css"><p class="strong">keep me</p>
--b
Content-Type: text/css; charset=utf-8
Content-Location: https://example.test/site.css#saved

.strong { font-weight: bold }
--b--
"#,
    );
    assert_eq!(to_markdown_bytes(&mhtml, Some(Format::Mhtml)).unwrap(), "**keep me**\n");
}

#[test]
fn frameset_root_preserves_collected_assets() {
    let mhtml = mhtml_fixture(
        r#"MIME-Version: 1.0
Snapshot-Content-Location: https://example.test/page
Content-Type: multipart/related; boundary="b"

--b
Content-Type: text/html; charset=utf-8

<!doctype html>
<frameset>
<frame src="https://example.test/frame.html">
</frameset>
--b
Content-Type: image/png
Content-Location: https://example.test/logo.png

PNGDATA
--b--
"#,
    );
    let document = to_document(&mhtml, Some(Format::Mhtml)).unwrap();
    assert!(document.blocks.is_empty());
    assert_eq!(document.assets.len(), 1);
    assert_eq!(document.assets[0].media_type, "image/png");
}
