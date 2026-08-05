use crate::error::ConvertError;
use crate::model::{Block, Document, Inline, inlines_to_plain_text};
use crate::package::relationships::{Relationships, read_rels, rel_type};
use crate::package::xml::ns;
use crate::package::{Package, path};
use std::cell::RefCell;

/// Read `dc:title` from the OPC core-properties part selected by the package
/// relationship, without assuming the conventional `docProps/core.xml` path.
pub(crate) fn opc_title(
    pkg: &RefCell<Package<'_>>,
    root_rels: &Relationships,
) -> Result<Option<String>, ConvertError> {
    let Some(rel) = root_rels.first_of_type(rel_type::CORE_PROPERTIES) else {
        return Ok(None);
    };
    let Some(target) = path::resolve("", &rel.target).ok() else {
        return Ok(None);
    };
    let Some(tree) = pkg.borrow_mut().optional_xml_part(&target.path)? else {
        return Ok(None);
    };
    Ok(tree.descendants(ns::DC, "title").next().map(|title| title.text()))
}

/// Read an OPC title when the caller only has the source bytes, as with the
/// calamine-backed spreadsheet frontend.
pub(crate) fn opc_title_from_bytes(bytes: &[u8]) -> Result<Option<String>, ConvertError> {
    let pkg = RefCell::new(Package::open(bytes)?);
    let root_rels = read_rels(&mut pkg.borrow_mut(), "_rels/.rels")?;
    opc_title(&pkg, &root_rels)
}

/// Read the Dublin Core title from an ODF package's optional metadata part.
pub(crate) fn odf_title(pkg: &RefCell<Package<'_>>) -> Result<Option<String>, ConvertError> {
    let Some(tree) = pkg.borrow_mut().optional_xml_part("meta.xml")? else {
        return Ok(None);
    };
    Ok(tree.descendants(ns::DC, "title").next().map(|title| title.text()))
}

/// Add a document title as the top-level H1 unless it is empty or already
/// equals the first heading after whitespace and case normalization.
pub(crate) fn prepend_title(document: &mut Document, title: Option<&str>) {
    let Some(title) = title else { return };
    let collapsed = collapse_title(title);
    let normalized = collapsed.to_lowercase();
    if normalized.is_empty() {
        return;
    }
    let duplicate = document.blocks.iter().find_map(|block| match block {
        Block::Heading { content, .. } => Some(normalize_title(&inlines_to_plain_text(content))),
        _ => None,
    });
    if duplicate.as_deref() == Some(normalized.as_str()) {
        return;
    }
    document.blocks.insert(0, Block::heading(1, vec![Inline::plain(collapsed)]));
}

fn normalize_title(value: &str) -> String {
    collapse_title(value).to_lowercase()
}

fn collapse_title(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(text: &str) -> Block {
        Block::heading(2, vec![Inline::plain(text)])
    }

    #[test]
    fn title_insertion_ignores_empty_and_normalized_duplicates() {
        let mut empty = Document { blocks: vec![heading("Body")], ..Document::default() };
        prepend_title(&mut empty, Some(" \t\n"));
        assert_eq!(empty.blocks.len(), 1);

        let mut duplicate =
            Document { blocks: vec![heading("  Fixture   BOOK ")], ..Document::default() };
        prepend_title(&mut duplicate, Some("fixture book"));
        assert_eq!(duplicate.blocks.len(), 1);
        assert!(matches!(duplicate.blocks[0], Block::Heading { level: 2, .. }));
    }

    #[test]
    fn genuine_title_precedes_the_existing_body() {
        let mut document =
            Document { blocks: vec![heading("First heading")], ..Document::default() };
        prepend_title(&mut document, Some("Document title"));
        assert!(matches!(document.blocks[0], Block::Heading { level: 1, .. }));
    }
}
