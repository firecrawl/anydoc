//! OLE previews must be usable images while the original object stays retained.
//! Markup-compatibility alternatives must not leak content from rejected branches.

use anydoc::model::{Block, Document, ImageSource, Inline};
use anydoc::{Format, to_document};
use std::io::{Cursor, Write};

const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const CHART: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const PAYLOAD: &str = "OLE payload, not an image";

// Complete 1 x 1 RGB PNGs: red preview, blue control and green rejected branch.
const PREVIEW_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];
const CONTROL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x60, 0xf8, 0x0f,
    0x00, 0x01, 0x03, 0x01, 0x00, 0x08, 0x89, 0xc2, 0xec, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];
const REJECTED_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0xf8, 0xcf, 0x00,
    0x00, 0x02, 0x02, 0x01, 0x00, 0x7b, 0x09, 0x81, 0x78, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn presentation(shapes: &str) -> Document {
    let pres = format!(
        r#"<p:presentation xmlns:p="{P}" xmlns:r="{R}"><p:sldIdLst><p:sldId id="256" r:id="slide"/></p:sldIdLst></p:presentation>"#
    );
    let pres_rels = format!(
        r#"<Relationships xmlns="{RELS}"><Relationship Id="slide" Type="{R}/slide" Target="slides/slide1.xml"/></Relationships>"#
    );
    let slide = format!(
        r#"<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}" xmlns:mc="{MC}" xmlns:c="{CHART}" xmlns:dgm="{DGM}" xmlns:v="urn:schemas-microsoft-com:vml"><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#
    );
    let slide_rels = format!(
        r#"<Relationships xmlns="{RELS}">
        <Relationship Id="payload" Type="{R}/oleObject" Target="../embeddings/object.bin"/>
        <Relationship Id="preview" Type="{R}/image" Target="../media/preview.png"/>
        <Relationship Id="control" Type="{R}/image" Target="../media/control.png"/>
        <Relationship Id="rejected" Type="{R}/image" Target="../media/rejected.png"/>
        <Relationship Id="external" Type="{R}/image" Target="https://example.com/preview.png" TargetMode="External"/>
        <Relationship Id="chart" Type="{R}/chart" Target="../charts/chart1.xml"/>
        <Relationship Id="diagram" Type="{R}/diagramData" Target="../diagrams/data1.xml"/>
        </Relationships>"#
    );
    let chart = format!(
        r#"<c:chartSpace xmlns:c="{CHART}" xmlns:a="{A}"><c:chart><c:title><c:tx><c:rich><a:p><a:r><a:t>Chart title</a:t></a:r></a:p></c:rich></c:tx></c:title></c:chart></c:chartSpace>"#
    );
    let diagram = format!(
        r#"<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{A}"><dgm:ptLst><dgm:pt modelId="1"><dgm:t><a:p><a:r><a:t>Diagram point</a:t></a:r></a:p></dgm:t></dgm:pt></dgm:ptLst></dgm:dataModel>"#
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in [
        ("ppt/presentation.xml", pres.as_bytes()),
        ("ppt/_rels/presentation.xml.rels", pres_rels.as_bytes()),
        ("ppt/slides/slide1.xml", slide.as_bytes()),
        ("ppt/slides/_rels/slide1.xml.rels", slide_rels.as_bytes()),
        ("ppt/embeddings/object.bin", PAYLOAD.as_bytes()),
        ("ppt/media/preview.png", PREVIEW_PNG),
        ("ppt/media/control.png", CONTROL_PNG),
        ("ppt/media/rejected.png", REJECTED_PNG),
        ("ppt/charts/chart1.xml", chart.as_bytes()),
        ("ppt/diagrams/data1.xml", diagram.as_bytes()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(text).unwrap();
    }
    to_document(&zip.finish().unwrap().into_inner(), Format::Pptx).unwrap()
}

fn picture(rid: &str) -> String {
    format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="1" name="{rid}" descr="{rid}"/></p:nvPicPr><p:blipFill><a:blip r:embed="{rid}"/></p:blipFill></p:pic>"#
    )
}

fn ole(preview: &str) -> String {
    format!(
        r#"<p:oleObj r:id="payload" progId="Excel.Sheet.12" name="Sheet1"><p:embed/>{preview}</p:oleObj>"#
    )
}

fn frame(contents: &str) -> String {
    format!(
        r#"<p:graphicFrame><a:graphic><a:graphicData uri="{P}/ole">{contents}</a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

fn alternate(requires: &str, choice: &str, fallback: &str) -> String {
    format!(
        r#"<mc:AlternateContent><mc:Choice Requires="{requires}">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"#
    )
}

fn images(doc: &Document) -> Vec<(&str, &ImageSource)> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(inlines) => Some(inlines),
            _ => None,
        })
        .flatten()
        .filter_map(|inline| match inline {
            Inline::Image { alt, source } => Some((alt.as_str(), source)),
            _ => None,
        })
        .collect()
}

fn image_parts(doc: &Document) -> Vec<&str> {
    images(doc)
        .iter()
        .map(|(_, source)| match source {
            ImageSource::Asset(id) => doc.assets[id.0].origin_part.as_str(),
            _ => panic!("expected an embedded image, got {source:?}"),
        })
        .collect()
}

fn assert_payload_retained(doc: &Document) {
    let payload = doc.assets.iter().find(|a| a.origin_part == "ppt/embeddings/object.bin");
    let payload = payload.expect("original OLE object must stay available to document consumers");
    assert_eq!(payload.media_type, "application/vnd.ms-ole-object");
    assert_eq!(payload.bytes, PAYLOAD.as_bytes());
}

#[test]
fn plain_ole_uses_its_preview_and_preserves_payload_and_adjacent_pictures() {
    let doc = presentation(&format!(
        "{}{}{}",
        picture("control"),
        frame(&ole(&picture("preview"))),
        picture("control")
    ));
    assert_eq!(
        image_parts(&doc),
        ["ppt/media/control.png", "ppt/media/preview.png", "ppt/media/control.png"]
    );
    assert_eq!(images(&doc)[1].0, "Sheet1");
    assert_payload_retained(&doc);
    assert_eq!(doc.assets.len(), 3, "shared control image is retained only once");
    let preview = doc.assets.iter().find(|a| a.origin_part == "ppt/media/preview.png").unwrap();
    assert_eq!(preview.media_type, "image/png");
    assert_eq!(preview.bytes, PREVIEW_PNG);
}

#[test]
fn graphic_frame_uses_fallback_instead_of_unsupported_choice() {
    let doc = presentation(&frame(&alternate(
        "v",
        &ole(&picture("rejected")),
        &ole(&picture("preview")),
    )));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert_payload_retained(&doc);
    assert_eq!(doc.assets.len(), 2, "unselected branch must not add an asset");
}

#[test]
fn graphic_frame_selects_first_fully_supported_choice() {
    let contents = format!(
        r#"<mc:AlternateContent>
        <mc:Choice Requires="p v">{}</mc:Choice>
        <mc:Choice Requires="p a">{}</mc:Choice>
        <mc:Choice Requires="p">{}</mc:Choice>
        <mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        ole(&picture("rejected")),
        ole(&picture("preview")),
        ole(&picture("rejected")),
        ole(&picture("rejected"))
    );
    let doc = presentation(&frame(&contents));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert_eq!(doc.assets.len(), 2);
}

#[test]
fn unsupported_choice_without_fallback_is_skipped() {
    let doc = presentation(&format!(
        "{}{}",
        frame(&format!(
            r#"<mc:AlternateContent><mc:Choice Requires="v">{}</mc:Choice></mc:AlternateContent>"#,
            ole(&picture("rejected"))
        )),
        picture("control")
    ));
    assert_eq!(image_parts(&doc), ["ppt/media/control.png"]);
    assert_eq!(doc.assets.len(), 1);
}

#[test]
fn nested_alternatives_and_later_siblings_are_not_lost() {
    let skipped = alternate("v", &ole(&picture("rejected")), "");
    let nested = alternate("p", &ole(&picture("preview")), &ole(&picture("rejected")));
    let selected = alternate("v", &ole(&picture("rejected")), &nested);
    let doc = presentation(&frame(&format!("{skipped}{selected}")));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert_eq!(doc.assets.len(), 2);
}

#[test]
fn preview_selection_respects_alternate_content_inside_ole_object() {
    let preview = alternate("v", &picture("rejected"), &picture("preview"));
    let doc = presentation(&frame(&ole(&preview)));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert_payload_retained(&doc);
}

#[test]
fn ole_wrapper_in_a_shape_alternative_keeps_its_preview() {
    let doc = presentation(&alternate("v", &ole(""), &ole(&picture("preview"))));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert_payload_retained(&doc);
}

#[test]
fn preview_remains_usable_when_the_payload_relationship_is_missing() {
    let contents = ole(&picture("preview")).replace("r:id=\"payload\"", "r:id=\"missing\"");
    let doc = presentation(&frame(&contents));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert_eq!(doc.assets.len(), 1);
}

#[test]
fn ole_without_usable_preview_keeps_existing_payload_source() {
    for preview in [String::new(), picture("missing")] {
        let doc = presentation(&frame(&ole(&preview)));
        assert_eq!(image_parts(&doc), ["ppt/embeddings/object.bin"]);
        assert_payload_retained(&doc);
        assert_eq!(doc.assets.len(), 1);
    }
}

#[test]
fn external_preview_stays_external_and_payload_stays_embedded() {
    let preview = picture("external").replace("r:embed", "r:link");
    let doc = presentation(&frame(&ole(&preview)));
    assert_eq!(
        images(&doc),
        [("Sheet1", &ImageSource::External("https://example.com/preview.png".into()))]
    );
    assert_payload_retained(&doc);
    assert_eq!(doc.assets.len(), 1);
}

#[test]
fn unselected_table_cannot_hide_the_selected_ole_object() {
    let table = r#"<a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>rejected table</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl>"#;
    let doc = presentation(&frame(&alternate("v", table, &ole(&picture("preview")))));
    assert_eq!(image_parts(&doc), ["ppt/media/preview.png"]);
    assert!(!doc.blocks.iter().any(|b| matches!(b, Block::Table(_))));
}

#[test]
fn chart_namespace_is_supported_in_graphic_frame_choices() {
    let chart = r#"<c:chart r:id="chart"/>"#;
    for shapes in [
        frame(&alternate("c", chart, &ole(&picture("rejected")))),
        alternate("c", &frame(chart), &picture("rejected")),
    ] {
        let doc = presentation(&shapes);
        assert!(images(&doc).is_empty(), "a supported chart must win over its fallback");
        assert!(doc.blocks.iter().any(|block| matches!(
            block,
            Block::Paragraph(inlines) if inlines.iter().any(|inline| matches!(
                inline, Inline::Text { text, .. } if text == "Chart title"
            ))
        )));
    }
}

#[test]
fn diagram_namespace_is_supported_in_graphic_frame_choices() {
    let diagram = r#"<dgm:relIds r:dm="diagram"/>"#;
    for shapes in [
        frame(&alternate("dgm", diagram, &ole(&picture("rejected")))),
        alternate("dgm", &frame(diagram), &picture("rejected")),
    ] {
        let doc = presentation(&shapes);
        assert!(images(&doc).is_empty(), "a supported diagram must win over its fallback");
        let list = doc
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::List(list) => Some(list),
                _ => None,
            })
            .expect("diagram text retained as a list");
        assert!(matches!(
            &list.items[0].blocks[0],
            Block::Paragraph(inlines) if inlines.iter().any(|inline| matches!(
                inline, Inline::Text { text, .. } if text == "Diagram point"
            ))
        ));
    }
}
