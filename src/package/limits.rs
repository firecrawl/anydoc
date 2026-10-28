//! Fixed safety limits, applied identically to every conversion.
//!
//! These are hard caps against attack/abuse input shapes (decompression
//! bombs, pathological nesting, runaway expansion) - crossing one returns
//! [`ConvertError::ResourceLimit`](crate::ConvertError::ResourceLimit),
//! always. They are deliberately not configurable: real-world documents sit
//! orders of magnitude below every value here.

/// Maximum decompressed size of a single archive entry: 128 MiB.
pub const MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;

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

/// Maximum grid positions one spreadsheet table may materialize: 4 million.
pub const MAX_GRID_SLOTS: u64 = 4_000_000;

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

/// Maximum formatting runs retained from one legacy `.doc` FKP stream. The
/// page numbers in the PLC are attacker-controlled and may all alias a single
/// page, so the entry count on its own does not bound how many runs they
/// yield. Past the cap formatting degrades; text extraction is unaffected.
pub const MAX_DOC_RUNS: usize = 1_000_000;

/// Maximum RTF group nesting depth. Real documents nest a few dozen groups;
/// every open group retains a character-state snapshot, and inside a math
/// zone it also opens an element, so unbounded nesting amplifies memory and
/// deepens a tree that is walked and dropped recursively.
pub const MAX_RTF_GROUP_DEPTH: usize = 1024;

/// Maximum bytes of a spreadsheet number-format code. Real format codes run
/// to a few dozen characters; the parser expands each one into several live
/// vectors, so an unbounded `formatCode` attribute amplifies a small archive
/// entry into gigabytes.
pub const MAX_NUMBER_FORMAT_BYTES: usize = 4096;

/// Maximum total binary records visited in one legacy record stream.
pub const MAX_RECORDS: u64 = 16_000_000;
