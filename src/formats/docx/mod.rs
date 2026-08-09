//! OOXML WordprocessingML (.docx / .docm).
//!
//! Resolution pipeline: package parts -> style/numbering models ->
//! spec-order property resolution -> document model.

mod content;
mod numbering;
mod styles;

use crate::error::ConvertError;
use crate::model::{Document, Note, NoteKind};
use crate::package::Package;
use crate::package::relationships::{Relationships, read_rels, rel_type, rels_part_for};
use crate::package::xml::ns;
use crate::shared::assets::AssetSink;
use content::Ctx;
use numbering::Counters;
use std::cell::RefCell;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let pkg = match Package::open(bytes) {
        Ok(p) => p,
        Err(e) => return Err(crate::package::archive::probe_ole(bytes).unwrap_or(e)),
    };
    let pkg = RefCell::new(pkg);

    // OPC part discovery: the main part comes from the package-level
    // officeDocument relationship; its own parts (styles, numbering, notes)
    // from the main part's typed relationships. Conventional paths are the
    // fallback for packages with missing or unusable rels.
    let root_rels = read_rels(&mut pkg.borrow_mut(), "_rels/.rels")?;
    let main_part = root_rels
        .first_of_type(rel_type::OFFICE_DOCUMENT)
        .and_then(|rel| crate::package::path::resolve("", &rel.target).ok())
        .map(|t| t.path)
        .unwrap_or_else(|| "word/document.xml".to_string());
    let doc_rels = read_rels(&mut pkg.borrow_mut(), &rels_part_for(&main_part))?;

    let styles_part = typed_part_path(&doc_rels, &main_part, rel_type::STYLES, "styles.xml");
    let styles_tree = pkg.borrow_mut().optional_xml_part(&styles_part)?;
    let styles =
        styles::Styles::parse_opt(styles_tree.as_ref().and_then(|t| t.find(ns::W, "styles")));

    let numbering_part =
        typed_part_path(&doc_rels, &main_part, rel_type::NUMBERING, "numbering.xml");
    let numbering_tree = pkg.borrow_mut().optional_xml_part(&numbering_part)?;
    let numbering = match numbering_tree.as_ref().and_then(|t| t.find(ns::W, "numbering")) {
        Some(root) => numbering::parse(root, &|style_id| styles.direct_num_id(style_id))?,
        None => Default::default(),
    };

    let doc_tree = pkg.borrow_mut().required_xml_part(&main_part)?;
    let body = doc_tree
        .find(ns::W, "document")
        .and_then(|d| d.find(ns::W, "body"))
        .ok_or_else(|| ConvertError::malformed_part(main_part.clone(), "no document body"))?;

    let counters = RefCell::new(Counters::default());
    let assets = RefCell::new(AssetSink::new());

    let footnotes_part =
        typed_part_path(&doc_rels, &main_part, rel_type::FOOTNOTES, "footnotes.xml");
    let endnotes_part = typed_part_path(&doc_rels, &main_part, rel_type::ENDNOTES, "endnotes.xml");

    let ctx = Ctx {
        pkg: &pkg,
        rels: doc_rels,
        base_part: main_part,
        styles: &styles,
        numbering: &numbering,
        counters: &counters,
        assets: &assets,
    };
    let blocks = content::parse_blocks(body, &ctx)?;

    let mut notes = Vec::new();
    for (part, root_name, elem_name, prefix, kind) in [
        (footnotes_part, "footnotes", "footnote", "fn", NoteKind::Footnote),
        (endnotes_part, "endnotes", "endnote", "en", NoteKind::Endnote),
    ] {
        let Some(tree) = pkg.borrow_mut().optional_xml_part(&part)? else {
            continue;
        };
        let Some(root) = tree.find(ns::W, root_name) else {
            continue;
        };
        let note_rels = read_rels(&mut pkg.borrow_mut(), &rels_part_for(&part))?;
        let note_ctx = ctx.for_part(note_rels, part);
        for note in root.find_all(ns::W, elem_name) {
            if matches!(
                note.attr(ns::W, "type"),
                Some("separator") | Some("continuationSeparator") | Some("continuationNotice")
            ) {
                continue;
            }
            let Some(id) = note.attr(ns::W, "id") else { continue };
            notes.push(Note {
                id: format!("{prefix}{id}"),
                kind,
                blocks: content::parse_blocks(note, &note_ctx)?,
            });
        }
    }

    let assets = std::mem::take(&mut assets.borrow_mut().assets);
    Ok(Document { blocks, notes, assets })
}

/// Path of a typed related part, resolved against the main part; falls back
/// to the conventional sibling name when the relationship is absent.
fn typed_part_path(rels: &Relationships, base: &str, rel_type: &str, fallback: &str) -> String {
    let reference = rels.first_of_type(rel_type).map(|rel| rel.target.as_str()).unwrap_or(fallback);
    match crate::package::path::resolve(base, reference) {
        Ok(target) => target.path,
        Err(e) => {
            log::warn!("skipping unresolvable related-part target {reference:?}: {e}");
            crate::package::path::resolve(base, fallback)
                .map(|t| t.path)
                .unwrap_or_else(|_| fallback.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, ImageSource, Inline};
    use std::io::{Cursor, Write};

    fn docx_parts(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in parts {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn docx(document: &str, rels: &str) -> Vec<u8> {
        docx_parts(&[("word/document.xml", document), ("word/_rels/document.xml.rels", rels)])
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn find_image(blocks: &[Block]) -> Option<&Inline> {
        blocks.iter().find_map(|b| match b {
            Block::Paragraph(inlines) => inlines.iter().find(|i| matches!(i, Inline::Image { .. })),
            _ => None,
        })
    }

    #[test]
    fn linked_image_relationship_becomes_an_external_source() {
        // M9: `r:link` with an external-mode relationship must carry the URL
        // instead of failing the internal-part loader.
        let document = r#"<w:document
            xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <w:body><w:p><w:r><w:drawing>
                <a:blip r:link="rId9"/>
            </w:drawing></w:r></w:p></w:body></w:document>"#;
        let rels = r#"<Relationships
            xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId9"
                Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
                Target="https://e.com/pic.png" TargetMode="External"/>
            </Relationships>"#;
        let doc = parse(&docx(document, rels)).unwrap();
        let image = find_image(&doc.blocks).expect("image inline");
        let Inline::Image { source, .. } = image else { unreachable!() };
        assert_eq!(*source, ImageSource::External("https://e.com/pic.png".into()));
        assert!(doc.assets.is_empty(), "external images are not retained as assets");
    }

    fn numbering_with_start(start: &str) -> String {
        format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0">
                <w:numFmt w:val="decimal"/><w:start w:val="{start}"/>
            </w:lvl></w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
            </w:numbering>"#
        )
    }

    fn numbered_paragraphs() -> String {
        let para = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>
            <w:r><w:t>item</w:t></w:r></w:p>"#;
        format!(r#"<w:document {W}><w:body>{para}{para}</w:body></w:document>"#)
    }

    #[test]
    fn huge_numbering_start_values_cannot_overflow() {
        // H2: w:start is ST_DecimalNumber (xsd:int); out-of-range values are
        // clamped so document-order increments can never overflow.
        for start in ["18446744073709551615", "-5", "2147483647"] {
            let bytes = docx_parts(&[
                ("word/document.xml", &numbered_paragraphs()),
                ("word/numbering.xml", &numbering_with_start(start)),
            ]);
            let doc = parse(&bytes).expect(start);
            assert!(!doc.blocks.is_empty());
        }
    }

    #[test]
    fn partial_num_pr_inherits_property_by_property() {
        // M1: a direct numPr carrying only ilvl merges with the style's
        // numId instead of suppressing numbering.
        let document = format!(
            r#"<w:document {W}><w:body>
            <w:p><w:pPr><w:pStyle w:val="Listy"/>
                <w:numPr><w:ilvl w:val="1"/></w:numPr></w:pPr>
                <w:r><w:t>second level</w:t></w:r></w:p>
            </w:body></w:document>"#
        );
        let styles = format!(
            r#"<w:styles {W}>
            <w:style w:type="paragraph" w:styleId="Listy">
                <w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr>
            </w:style></w:styles>"#
        );
        let numbering = format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="0">
                <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:start w:val="1"/></w:lvl>
                <w:lvl w:ilvl="1"><w:numFmt w:val="lowerLetter"/><w:start w:val="1"/></w:lvl>
            </w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
            </w:numbering>"#
        );
        let bytes = docx_parts(&[
            ("word/document.xml", &document),
            ("word/styles.xml", &styles),
            ("word/numbering.xml", &numbering),
        ]);
        let doc = parse(&bytes).unwrap();
        let Some(Block::List(list)) = doc.blocks.first() else {
            panic!("expected a list, got {:?}", doc.blocks.first());
        };
        assert_eq!(
            list.marker,
            crate::model::MarkerKind::LowerAlpha,
            "level 1 of the style's numbering"
        );
    }

    #[test]
    fn unpreserved_edge_whitespace_is_discarded() {
        // M3: w:t edge whitespace is significant only under
        // xml:space="preserve".
        let document = format!(
            r#"<w:document {W}><w:body><w:p>
            <w:r><w:t>lead</w:t></w:r>
            <w:r><w:t>   discarded   </w:t></w:r>
            <w:r><w:t xml:space="preserve"> kept </w:t></w:r>
            <w:r><w:t>tail</w:t></w:r>
            </w:p></w:body></w:document>"#
        );
        let doc = parse(&docx_parts(&[("word/document.xml", &document)])).unwrap();
        let Some(Block::Paragraph(inlines)) = doc.blocks.first() else { panic!() };
        assert_eq!(crate::model::inlines_to_plain_text(inlines), "leaddiscarded kept tail");
    }

    #[test]
    fn unpreserved_no_break_space_is_kept() {
        // The xml:space contract governs XML whitespace; a no-break space is
        // character data, so it survives an unmarked edge.
        let nbsp = '\u{a0}';
        let document = format!(
            r#"<w:document {W}><w:body><w:p>
            <w:r><w:t>before{nbsp}</w:t></w:r>
            <w:r><w:t>after</w:t></w:r>
            </w:p></w:body></w:document>"#
        );
        let doc = parse(&docx_parts(&[("word/document.xml", &document)])).unwrap();
        let Some(Block::Paragraph(inlines)) = doc.blocks.first() else { panic!() };
        assert_eq!(crate::model::inlines_to_plain_text(inlines), "before after");
    }

    #[test]
    fn page_break_between_runs_keeps_the_word_boundary() {
        // A w:br is unrepresentable in Markdown whatever its type, but the
        // runs it separates must not merge into a word the document never
        // had. A break ending a paragraph still leaves no stray marker.
        let cases = [
            (
                r#"<w:p><w:r><w:t>Alfa</w:t><w:br w:type="page"/><w:t>Beta</w:t></w:r></w:p>"#,
                "Alfa\\\nBeta\n",
            ),
            (
                r#"<w:p><w:r><w:t>Alfa</w:t><w:br w:type="page"/></w:r></w:p>
                <w:p><w:r><w:t>Beta</w:t></w:r></w:p>"#,
                "Alfa\n\nBeta\n",
            ),
        ];
        for (body, expected) in cases {
            let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
            let bytes = docx_parts(&[("word/document.xml", &document)]);
            let markdown = crate::to_markdown_bytes(&bytes, crate::Format::Docx).unwrap();
            assert_eq!(markdown, expected, "body: {body}");
        }
    }

    #[test]
    fn numbered_heading_keeps_its_number() {
        // H1: a heading style with numbering shows its label and advances
        // the sequence.
        let document = format!(
            r#"<w:document {W}><w:body>
            <w:p><w:pPr><w:pStyle w:val="H1"/></w:pPr><w:r><w:t>Intro</w:t></w:r></w:p>
            <w:p><w:pPr><w:pStyle w:val="H1"/></w:pPr><w:r><w:t>Details</w:t></w:r></w:p>
            </w:body></w:document>"#
        );
        let styles = format!(
            r#"<w:styles {W}>
            <w:style w:type="paragraph" w:styleId="H1"><w:name w:val="heading 1"/>
                <w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr>
            </w:style></w:styles>"#
        );
        let numbering = format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="0">
                <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:start w:val="1"/>
                    <w:lvlText w:val="%1."/><w:pStyle w:val="H1"/></w:lvl>
            </w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
            </w:numbering>"#
        );
        let bytes = docx_parts(&[
            ("word/document.xml", &document),
            ("word/styles.xml", &styles),
            ("word/numbering.xml", &numbering),
        ]);
        let doc = parse(&bytes).unwrap();
        let headings: Vec<String> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { content, .. } => {
                    Some(crate::model::inlines_to_plain_text(content))
                }
                _ => None,
            })
            .collect();
        assert_eq!(headings, vec!["1. Intro", "2. Details"]);
    }
}
