//! OOXML WordprocessingML (.docx / .docm).
//!
//! Resolution pipeline: package parts -> style/numbering models ->
//! spec-order property resolution -> document model.

mod content;
mod numbering;
mod ole;
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

#[cfg(test)]
pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    parse_with(bytes, true)
}

/// Fork (sznb): `include_toc = false` drops the table of contents — a
/// `Table of Contents` SDT and the result paragraphs of a bare `TOC` field
/// (plus the 目录 / Contents title right before it).
pub fn parse_with(bytes: &[u8], include_toc: bool) -> Result<Document, ConvertError> {
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
        skip_toc: !include_toc,
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
    fn unmarked_run_edge_whitespace_is_kept() {
        // Converters that never write xml:space carry inter-word spacing on
        // run edges; dropping it glues the words together.
        let document = format!(
            r#"<w:document {W}><w:body><w:p>
            <w:r><w:t>This</w:t></w:r>
            <w:r><w:t> by-law</w:t></w:r>
            <w:r><w:t> grants</w:t></w:r>
            </w:p></w:body></w:document>"#
        );
        let doc = parse(&docx_parts(&[("word/document.xml", &document)])).unwrap();
        let Some(Block::Paragraph(inlines)) = doc.blocks.first() else { panic!() };
        assert_eq!(crate::model::inlines_to_plain_text(inlines), "This by-law grants");
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

    /// Fork (sznb): `(numId, text)` paragraphs over a single-level abstract
    /// with label `第%1项`; `nums` is the `<w:num>` list. Returns the text
    /// output lines.
    fn shared_abstract_lines(paras: &[(u32, &str)], nums: &str) -> Vec<String> {
        let body: String = paras
            .iter()
            .map(|(num_id, text)| {
                format!(
                    r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num_id}"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
                )
            })
            .collect();
        let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
        let numbering = format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0">
                <w:numFmt w:val="decimal"/><w:start w:val="1"/><w:lvlText w:val="第%1项"/>
            </w:lvl></w:abstractNum>
            {nums}
            </w:numbering>"#
        );
        let bytes = docx_parts(&[("word/document.xml", &document), ("word/numbering.xml", &numbering)]);
        let text = crate::to_text_bytes(&bytes, crate::Format::Docx).unwrap();
        text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    #[test]
    fn num_ids_sharing_an_abstract_share_one_counter() {
        // Word/LibreOffice: numIds over the same abstractNum continue one
        // sequence (a numId is only an instance handle, not a new list).
        let lines = shared_abstract_lines(
            &[(1, "丁一"), (2, "丁二"), (1, "丁三")],
            r#"<w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num>
               <w:num w:numId="2"><w:abstractNumId w:val="7"/></w:num>"#,
        );
        let joined = lines.join("\n");
        for want in ["第1项", "第2项", "第3项"] {
            assert!(joined.contains(want), "missing {want} in:\n{joined}");
        }
        assert!(joined.find("第1项") < joined.find("丁一"));
        assert!(joined.find("丁一") < joined.find("第2项") && joined.find("第2项") < joined.find("丁二"));
        assert!(joined.find("丁二") < joined.find("第3项") && joined.find("第3项") < joined.find("丁三"));
    }

    #[test]
    fn start_override_restarts_the_shared_counter_once() {
        // startOverride restarts the shared counter at its value on that
        // numId's first use; later paragraphs (either numId) continue: 1, 5, 6.
        let lines = shared_abstract_lines(
            &[(1, "甲"), (2, "乙"), (1, "丙"), (2, "丁")],
            r#"<w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num>
               <w:num w:numId="2"><w:abstractNumId w:val="7"/>
                 <w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num>"#,
        );
        let joined = lines.join("\n");
        let order: Vec<usize> = ["第1项", "甲", "第5项", "乙", "第6项", "丙", "第7项", "丁"]
            .iter()
            .map(|w| joined.find(w).unwrap_or_else(|| panic!("missing {w} in:\n{joined}")))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "wrong order in:\n{joined}");
    }

    #[test]
    fn orphan_sub_level_consumes_the_upper_start() {
        // Fork (sznb): Word/LibreOffice — sub-level items ahead of any
        // top-level item use up the top level's start, so the first real
        // top-level item is 2 (技术使用手册: 1.1–1.4 then 「2. 网关接口调用」).
        let p = |ilvl: u8, text: &str| {
            format!(
                r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
            )
        };
        let body = [p(1, "甲"), p(1, "乙"), p(0, "丙"), p(1, "丁")].concat();
        let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
        let numbering = format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="7">
              <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:start w:val="1"/><w:lvlText w:val="第%1项"/></w:lvl>
              <w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/><w:start w:val="1"/><w:lvlText w:val="%1.%2"/></w:lvl>
            </w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num>
            </w:numbering>"#
        );
        let bytes = docx_parts(&[("word/document.xml", &document), ("word/numbering.xml", &numbering)]);
        let text = crate::to_text_bytes(&bytes, crate::Format::Docx).unwrap();
        let order: Vec<usize> = ["1.1", "甲", "1.2", "乙", "第2项", "丙", "2.1", "丁"]
            .iter()
            .map(|w| text.find(w).unwrap_or_else(|| panic!("missing {w} in:\n{text}")))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "wrong order in:\n{text}");
        assert!(!text.contains("第1项"), "top level must not restart at 1:\n{text}");
    }

    #[test]
    fn empty_section_break_carrier_takes_no_number() {
        // Fork (sznb): an empty numbered paragraph that only holds a section
        // break (144pages: Heading 1 + sectPr before 第1章) shows no label
        // and does not advance the list; a plain empty numbered paragraph
        // still counts, as in Word/LibreOffice.
        let num_p = |extra: &str, run: &str| {
            format!(
                r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr><w:tabs><w:tab w:val="clear" w:pos="420"/></w:tabs>{extra}</w:pPr>{run}</w:p>"#
            )
        };
        let body = [
            num_p("<w:sectPr/>", ""),
            num_p("", "<w:r><w:t>甲</w:t></w:r>"),
            num_p("", ""),
            num_p("", "<w:r><w:t>乙</w:t></w:r>"),
        ]
        .concat();
        let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
        let numbering = format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0">
                <w:numFmt w:val="decimal"/><w:start w:val="1"/><w:lvlText w:val="第%1章"/>
            </w:lvl></w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num>
            </w:numbering>"#
        );
        let bytes = docx_parts(&[("word/document.xml", &document), ("word/numbering.xml", &numbering)]);
        let text = crate::to_text_bytes(&bytes, crate::Format::Docx).unwrap();
        assert!(text.contains("第1章 甲") || text.contains("第1章甲"), "got:\n{text}");
        assert!(text.contains("第3章 乙") || text.contains("第3章乙"), "got:\n{text}");
    }

    // ---------- Fork (sznb): include_toc / dedup_cells ----------

    fn run_p(text: &str) -> String {
        format!(r#"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"#)
    }

    fn opts(dedup_cells: bool, include_toc: bool, html_tables: bool) -> crate::ConvertOptions {
        crate::ConvertOptions { html_tables, dedup_cells, include_toc }
    }

    #[test]
    fn toc_sdt_is_dropped_only_without_include_toc() {
        let sdt = format!(
            r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/></w:docPartObj></w:sdtPr>
            <w:sdtContent>{}{}</w:sdtContent></w:sdt>"#,
            run_p("目录"),
            run_p("第一章 总则 1")
        );
        // Custom title outside the SDT (技术使用手册: 「公共应用组件调用目录」).
        let body = [run_p("封面"), run_p("公共应用组件调用目录"), sdt, run_p("正文开始")].concat();
        let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
        let bytes = docx_parts(&[("word/document.xml", &document)]);
        let keep = crate::to_text_bytes_with(&bytes, crate::Format::Docx, opts(true, true, true)).unwrap();
        assert!(keep.contains("第一章 总则 1") && keep.contains("目录"), "got:\n{keep}");
        let drop = crate::to_text_bytes_with(&bytes, crate::Format::Docx, opts(true, false, true)).unwrap();
        assert!(!drop.contains("总则") && !drop.contains("目录"), "got:\n{drop}");
        assert!(keep.contains("公共应用组件调用目录"), "got:\n{keep}");
        assert!(drop.contains("封面") && drop.contains("正文开始"), "got:\n{drop}");
    }

    #[test]
    fn bare_toc_field_paragraphs_and_title_are_dropped() {
        // TOC field begins in the first entry paragraph and ends in the last
        // one (Word's bare-field form, not wrapped in an SDT).
        let fld = |kind: &str| format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#);
        let first = format!(
            r#"<w:p>{}<w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r>{}<w:r><w:t>第1章 概况 3</w:t></w:r></w:p>"#,
            fld("begin"),
            fld("separate")
        );
        let middle = run_p("第2章 需求 9");
        let last = format!(r#"<w:p><w:r><w:t>第3章 方案 20</w:t></w:r>{}</w:p>"#, fld("end"));
        // A blank paragraph between title and field (bullet_problem).
        let blank = "<w:p/>".to_string();
        let body = [run_p("目 录"), blank, first, middle, last, run_p("第1章 概况正文")].concat();
        let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
        let bytes = docx_parts(&[("word/document.xml", &document)]);
        let keep = crate::to_markdown_bytes_with(&bytes, crate::Format::Docx, opts(true, true, true)).unwrap();
        assert!(keep.contains("第2章 需求 9") && keep.contains("目 录"), "got:\n{keep}");
        let drop = crate::to_markdown_bytes_with(&bytes, crate::Format::Docx, opts(true, false, true)).unwrap();
        for gone in ["目 录", "概况 3", "需求 9", "方案 20"] {
            assert!(!drop.contains(gone), "{gone} survived:\n{drop}");
        }
        assert!(drop.contains("第1章 概况正文"), "body lost:\n{drop}");
    }

    fn merged_table_docx() -> Vec<u8> {
        // 3×3: row 0 「甲」 spans two columns; column 0 「乙」 spans rows 1–2.
        let tc = |pr: &str, text: &str| format!(r#"<w:tc><w:tcPr>{pr}</w:tcPr>{}</w:tc>"#, run_p(text));
        let rows = [
            [tc(r#"<w:gridSpan w:val="2"/>"#, "甲"), tc("", "丙")].concat(),
            [tc(r#"<w:vMerge w:val="restart"/>"#, "乙"), tc("", "a"), tc("", "b")].concat(),
            [tc("<w:vMerge/>", ""), tc("", "c"), tc("", "d")].concat(),
        ]
        .map(|r| format!("<w:tr>{r}</w:tr>"))
        .concat();
        let grid = r#"<w:tblGrid><w:gridCol/><w:gridCol/><w:gridCol/></w:tblGrid>"#;
        let document = format!(r#"<w:document {W}><w:body><w:tbl>{grid}{rows}</w:tbl></w:body></w:document>"#);
        docx_parts(&[("word/document.xml", &document)])
    }

    #[test]
    fn dedup_cells_false_repeats_merged_content() {
        let bytes = merged_table_docx();
        let count = |s: &str, w: &str| s.matches(w).count();
        // Markdown, HTML form: spans by default, repeated cells without dedup.
        let html = crate::to_markdown_bytes_with(&bytes, crate::Format::Docx, opts(true, true, true)).unwrap();
        assert!(html.contains("colspan") && html.contains("rowspan"), "got:\n{html}");
        assert_eq!((count(&html, "甲"), count(&html, "乙")), (1, 1), "got:\n{html}");
        let html = crate::to_markdown_bytes_with(&bytes, crate::Format::Docx, opts(false, true, true)).unwrap();
        assert!(!html.contains("colspan") && !html.contains("rowspan"), "got:\n{html}");
        assert_eq!((count(&html, "甲"), count(&html, "乙")), (2, 2), "got:\n{html}");
        // GFM keeps three columns either way.
        let gfm = crate::to_markdown_bytes_with(&bytes, crate::Format::Docx, opts(false, true, false)).unwrap();
        assert_eq!((count(&gfm, "甲"), count(&gfm, "乙")), (2, 2), "got:\n{gfm}");
        assert!(gfm.lines().all(|l| !l.starts_with('|') || l.matches('|').count() == 4), "got:\n{gfm}");
        // Plain text.
        let txt = crate::to_text_bytes_with(&bytes, crate::Format::Docx, opts(true, true, true)).unwrap();
        assert_eq!((count(&txt, "甲"), count(&txt, "乙")), (1, 1), "got:\n{txt}");
        let txt = crate::to_text_bytes_with(&bytes, crate::Format::Docx, opts(false, true, true)).unwrap();
        assert_eq!((count(&txt, "甲"), count(&txt, "乙")), (2, 2), "got:\n{txt}");
        assert!(txt.contains("甲\t甲\t丙") && txt.contains("乙\tc\td"), "got:\n{txt}");
    }
}
