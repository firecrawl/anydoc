//! Fixed safety limits, applied identically to every conversion.
//!
//! These are hard caps against attack/abuse input shapes (decompression
//! bombs, pathological nesting, runaway expansion) - crossing one returns
//! [`ConvertError::ResourceLimit`](crate::ConvertError::ResourceLimit),
//! always. They are deliberately not configurable: real-world documents sit
//! orders of magnitude below every value here.

/// Maximum decompressed size of a single archive entry: 128 MiB.
pub const MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;

/// Maximum size of a standalone, non-archive input: 128 MiB.
///
/// CSV and RTF do not pass through the archive layer, so this prevents
/// them from duplicating an already-unbounded input before their model
/// materialization budgets can take effect.
pub const MAX_STANDALONE_INPUT_BYTES: usize = 128 * 1024 * 1024;

/// Maximum text bytes copied into the document model: 64 MiB.
pub const MAX_MATERIALIZED_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// Maximum table cells materialized by a standalone frontend.
pub const MAX_MATERIALIZED_CELLS: usize = 4_000_000;

/// Maximum separately allocated text runs in the document model.
pub const MAX_MATERIALIZED_TEXT_RUNS: usize = 2_000_000;

/// Maximum total decompressed bytes read from one archive: 512 MiB.
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// Maximum number of entries in one archive.
pub const MAX_ENTRY_COUNT: usize = 100_000;

/// Maximum XML element nesting depth.
pub const MAX_XML_DEPTH: usize = 256;

/// Maximum number of XML nodes (elements + text runs) in one part. Sized
/// from the measured worst-case DOM cost (~400 bytes/node, see the node-cap
/// memory test) so a saturating part stays around the archive budget.
pub const MAX_XML_NODES: usize = 2_000_000;

/// Maximum content-bearing cells a repeat expansion may produce per table.
pub const MAX_EXPANSION: u64 = 4_000_000;

/// Maximum total text bytes *duplicated* by repeat expansion per document:
/// 64 MiB. The slot budget above bounds positions; this bounds the memory a
/// small document can amplify by repeating content-bearing cells.
pub const MAX_EXPANSION_TEXT_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum total bytes of embedded assets retained in a `Document`: 128 MiB.
pub const MAX_ASSET_TOTAL_BYTES: usize = 128 * 1024 * 1024;

/// Maximum nesting depth of binary record containers (legacy PPT stream).
pub const MAX_RECORD_DEPTH: usize = 64;

/// Maximum total binary records visited in one legacy record stream.
pub const MAX_RECORDS: u64 = 16_000_000;
