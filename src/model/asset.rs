/// Index into `Document::assets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetId(pub usize);

/// An embedded binary asset (image, object payload). Bytes are always
/// retained so the document stays self-contained; total retained bytes are
/// capped by the fixed `max_asset_total_bytes` limit at parse time.
#[derive(Debug, Clone)]
pub struct Asset {
    /// This asset's own index, so a detached `Asset` still identifies itself.
    pub id: AssetId,
    /// MIME type, e.g. `image/png`.
    pub media_type: String,
    /// Package part or stream the asset came from, for provenance.
    pub origin_part: String,
    /// The payload, exactly as stored in the source.
    pub bytes: Vec<u8>,
}
