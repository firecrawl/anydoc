//! Embedded-asset accumulation with the fixed retained-bytes cap.

use crate::error::ConvertError;
use crate::model::{Asset, AssetId, ImageSource};
use crate::package::Package;
use crate::package::limits;
use crate::package::relationships::{Relationships, TargetMode, rel_target_bytes};
use std::cell::RefCell;

#[derive(Default)]
pub struct AssetSink {
    pub assets: Vec<Asset>,
    /// Origin part -> retained asset, so repeated references to the same
    /// part share one asset instead of duplicating its bytes (and falsely
    /// accumulating against the cap).
    by_part: std::collections::HashMap<String, AssetId>,
    total: usize,
}

impl AssetSink {
    pub fn new() -> Self {
        AssetSink::default()
    }

    /// Retain an asset's bytes (copied only when the part is not already
    /// retained). Crossing the retained-bytes cap is a hard error.
    pub fn add(
        &mut self,
        media_type: String,
        origin_part: String,
        bytes: &[u8],
    ) -> Result<AssetId, ConvertError> {
        if let Some(&id) = self.by_part.get(&origin_part) {
            return Ok(id);
        }
        self.total += bytes.len();
        if self.total > limits::MAX_ASSET_TOTAL_BYTES {
            return Err(ConvertError::ResourceLimit {
                limit: "max_asset_total_bytes",
                detail: "embedded assets exceed the retained-bytes cap".into(),
            });
        }
        let id = AssetId(self.assets.len());
        self.by_part.insert(origin_part.clone(), id);
        self.assets.push(Asset { id, media_type, origin_part, bytes: bytes.to_vec() });
        Ok(id)
    }
}

/// Resolve an image relationship to its source. External-mode relationships
/// carry the image's URL directly; internal-mode targets load from the
/// package and are retained as assets. Failures degrade to `None` per the
/// unified policy; fatal errors propagate.
pub fn rel_image_source(
    pkg: &RefCell<Package>,
    rels: &Relationships,
    base_part: &str,
    assets: &RefCell<AssetSink>,
    rel_id: &str,
) -> Result<Option<ImageSource>, ConvertError> {
    let Some(rel) = rels.get(rel_id) else {
        return Ok(None);
    };
    if rel.mode == TargetMode::External {
        return Ok((!rel.target.is_empty()).then(|| ImageSource::External(rel.target.clone())));
    }
    match rel_target_bytes(pkg, rels, base_part, rel_id)? {
        Some((part, bytes)) => {
            let media = media_type_for(&part);
            let id = assets.borrow_mut().add(media, part, &bytes)?;
            Ok(Some(ImageSource::Asset(id)))
        }
        None => Ok(None),
    }
}

/// MIME type from a part path's extension.
pub fn media_type_for(part: &str) -> String {
    let ext = part.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/emf",
        "wmf" => "image/wmf",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_origin_parts_share_one_asset() {
        let mut sink = AssetSink::new();
        let a = sink.add("image/png".into(), "media/one.png".into(), &[1; 64]).unwrap();
        let b = sink.add("image/png".into(), "media/one.png".into(), &[1; 64]).unwrap();
        assert_eq!(a, b);
        assert_eq!(sink.assets.len(), 1);
        assert_eq!(sink.total, 64, "repeated references must not re-count bytes");
    }

    #[test]
    fn repeated_references_cannot_trip_the_cap() {
        // Preload the counter to just under the cap (avoids large real
        // allocations, which would feed the R22 memory test's counter).
        let mut sink = AssetSink::new();
        sink.total = limits::MAX_ASSET_TOTAL_BYTES - 10;
        for _ in 0..8 {
            assert!(sink.add("image/png".into(), "media/big.png".into(), &[0; 8]).is_ok());
        }
    }

    #[test]
    fn crossing_the_cap_is_a_hard_error() {
        let mut sink = AssetSink::new();
        sink.total = limits::MAX_ASSET_TOTAL_BYTES - 10;
        let err = sink.add("image/png".into(), "media/big.png".into(), &[0; 11]).unwrap_err();
        assert!(matches!(err, ConvertError::ResourceLimit { limit: "max_asset_total_bytes", .. }));
    }
}
