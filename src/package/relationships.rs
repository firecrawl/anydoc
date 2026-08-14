//! OPC relationship parts, typed.

use crate::error::ConvertError;
use crate::package::archive::Package;
use crate::package::xml::{normalize_ooxml_uri, ns};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Well-known OPC relationship types (Transitional form; Strict types are
/// normalized onto these when the rels part is read).
pub mod rel_type {
    pub const OFFICE_DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const STYLES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    pub const NUMBERING: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
    pub const FOOTNOTES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
    pub const ENDNOTES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
    pub const CORE_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMode {
    Internal,
    External,
}

#[derive(Debug, Clone)]
pub struct Relationship {
    pub target: String,
    pub rel_type: String,
    pub mode: TargetMode,
}

#[derive(Debug, Default)]
pub struct Relationships(HashMap<String, Relationship>);

impl Relationships {
    pub fn get(&self, id: &str) -> Option<&Relationship> {
        self.0.get(id)
    }

    /// Internal-mode target for an id, the common case for parts.
    pub fn internal_target(&self, id: &str) -> Option<&str> {
        self.0.get(id).filter(|r| r.mode == TargetMode::Internal).map(|r| r.target.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Relationship)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The internal-mode relationship of a given type, lowest id first so
    /// the pick is deterministic when a producer emits duplicates.
    pub fn first_of_type(&self, rel_type: &str) -> Option<&Relationship> {
        self.0
            .iter()
            .filter(|(_, r)| r.rel_type == rel_type && r.mode == TargetMode::Internal)
            .min_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, r)| r)
    }
}

/// Read a relationships part. An absent, unreadable, or corrupt part yields
/// an empty map (absent is valid - many parts simply have no relationships;
/// the rest degrade per the unified policy with a log). Resource-limit
/// errors always propagate.
pub fn read_rels(pkg: &mut Package, part: &str) -> Result<Relationships, ConvertError> {
    let Some(root) = pkg.optional_xml_part(part)? else {
        return Ok(Relationships::default());
    };
    let mut map = HashMap::new();
    for rel in root.descendants(ns::PKG_RELS, "Relationship") {
        let (Some(id), Some(target)) = (rel.attr_any("Id"), rel.attr_any("Target")) else {
            continue;
        };
        let mode = match rel.attr_any("TargetMode") {
            Some(m) if m.eq_ignore_ascii_case("external") => TargetMode::External,
            _ => TargetMode::Internal,
        };
        let rel_type = rel.attr_any("Type").unwrap_or("");
        let rel_type = normalize_ooxml_uri(rel_type).unwrap_or_else(|| rel_type.to_string());
        map.insert(id.to_string(), Relationship { target: target.to_string(), rel_type, mode });
    }
    Ok(Relationships(map))
}

/// A loaded internal relationship target: its resolved part path and bytes.
pub type RelTarget = (String, Rc<[u8]>);

/// Load an internal relationship target's bytes, resolved against the part
/// the relationship appears in. Unknown ids, external targets, and
/// unresolvable or unreadable targets degrade (log + `None`) per the unified
/// policy; fatal resource-limit errors always propagate.
pub fn rel_target_bytes(
    pkg: &RefCell<Package>,
    rels: &Relationships,
    base_part: &str,
    rel_id: &str,
) -> Result<Option<RelTarget>, ConvertError> {
    let Some(rel) = rels.get(rel_id) else {
        return Ok(None);
    };
    if rel.mode != TargetMode::Internal {
        return Ok(None);
    }
    let target = match crate::package::path::resolve(base_part, &rel.target) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("skipping unresolvable relationship target {:?}: {e}", rel.target);
            return Ok(None);
        }
    };
    match pkg.borrow_mut().optional_part(&target.path)? {
        Some(bytes) => Ok(Some((target.path, bytes))),
        None => {
            log::warn!("relationship target {} is missing", target.path);
            Ok(None)
        }
    }
}

/// Conventional rels part name for a part (`word/document.xml` ->
/// `word/_rels/document.xml.rels`).
pub fn rels_part_for(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}
