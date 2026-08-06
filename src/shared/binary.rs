//! Shared primitives for the legacy OLE2 binary formats (DOC, PPT):
//! checked little-endian integer readers and bounded compound-file stream
//! reading.

use crate::error::ConvertError;
use crate::package::limits;
use std::io::Read;

/// Little-endian `u16` at `off`; `None` when out of bounds.
pub fn get_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

/// Little-endian `u32` at `off`; `None` when out of bounds.
pub fn get_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// Read a named stream from an OLE2 compound file. A missing stream is
/// `MissingPart`; the read is hard-capped at `MAX_ENTRY_BYTES` so a corrupt
/// sector chain cannot expand without bound.
pub fn read_ole_stream<R: Read + std::io::Seek>(
    ole: &mut cfb::CompoundFile<R>,
    name: &str,
) -> Result<Vec<u8>, ConvertError> {
    let stream =
        ole.open_stream(name).map_err(|_| ConvertError::MissingPart { part: name.to_string() })?;
    let mut bytes = Vec::new();
    let read = stream.take(limits::MAX_ENTRY_BYTES + 1).read_to_end(&mut bytes).map_err(|e| {
        ConvertError::Malformed {
            part: Some(name.to_string()),
            detail: format!("unreadable stream: {e}"),
        }
    })? as u64;
    if read > limits::MAX_ENTRY_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!("{name} stream exceeds the read cap"),
        });
    }
    Ok(bytes)
}

/// Open an OLE2 compound file, repairing one known writer defect first.
///
/// Apple's Cocoa exporter (TextEdit "Save as Word 97", `textutil -convert
/// doc`) zero-fills the MiniFAT instead of terminating chains with
/// `FREESECT`/`ENDOFCHAIN`, so every entry reads as "next is mini sector 0"
/// and `cfb` rejects the file ("mini sector 0 pointed to twice"). An
/// all-zero MiniFAT cannot occur in a valid file — entry 0 would point at
/// itself — so when that exact shape is detected the chains are rebuilt
/// from the directory entries (each mini stream's sectors are contiguous
/// from its start, which is how sequential writers allocate them) and the
/// open is retried. Anything else fails exactly as before.
pub fn open_ole(bytes: &[u8]) -> Result<cfb::CompoundFile<std::io::Cursor<Vec<u8>>>, ConvertError> {
    match cfb::CompoundFile::open(std::io::Cursor::new(bytes.to_vec())) {
        Ok(ole) => Ok(ole),
        Err(original) => match repair_zeroed_minifat(bytes) {
            Some(repaired) => cfb::CompoundFile::open(std::io::Cursor::new(repaired))
                .map_err(|_| ole_open_error(&original)),
            None => Err(ole_open_error(&original)),
        },
    }
}

fn ole_open_error(e: &std::io::Error) -> ConvertError {
    ConvertError::malformed(format!("not an OLE2 compound file: {e}"))
}

/// Rebuild an all-zero MiniFAT from the directory entries; `None` when the
/// file does not have that exact defect or the rebuild would be ambiguous.
fn repair_zeroed_minifat(bytes: &[u8]) -> Option<Vec<u8>> {
    const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
    const FREESECT: u32 = 0xFFFF_FFFF;
    const MAX_SECTORS: u32 = 0xFFFF_FFFA;

    if !bytes.starts_with(&OLE_MAGIC) {
        return None;
    }
    let sector_shift = get_u16(bytes, 30)?;
    if !(7..=15).contains(&sector_shift) {
        return None;
    }
    let sector_size = 1usize << sector_shift;
    let mini_shift = get_u16(bytes, 32)?;
    let mini_size = 1u64 << mini_shift.min(15);
    let mini_cutoff = get_u32(bytes, 56)? as u64;
    let dir_start = get_u32(bytes, 48)?;
    let minifat_start = get_u32(bytes, 60)?;
    let num_minifat = get_u32(bytes, 64)?;
    if num_minifat == 0 || minifat_start >= MAX_SECTORS {
        return None;
    }

    // Sector N starts at (N + 1) << sector_shift: the 512-byte header pads
    // to a full sector in version-4 (4096-byte-sector) files.
    let sector_off = |sector: u32| -> Option<usize> {
        let off = (sector as usize).checked_add(1)?.checked_mul(sector_size)?;
        (off + sector_size <= bytes.len()).then_some(off)
    };

    // FAT sectors from the header DIFAT (first 109 entries). Documents small
    // enough to come from the writers with this defect never need an
    // external DIFAT chain; bail rather than guess if one is present.
    if get_u32(bytes, 72)? != 0 {
        return None;
    }
    let mut fat: Vec<u32> = Vec::new();
    for i in 0..109 {
        let s = get_u32(bytes, 76 + i * 4)?;
        if s >= MAX_SECTORS {
            break;
        }
        let off = sector_off(s)?;
        for j in 0..sector_size / 4 {
            fat.push(get_u32(bytes, off + j * 4)?);
        }
    }
    let next_in_fat = |sector: u32| -> Option<u32> { fat.get(sector as usize).copied() };

    // Walk the MiniFAT's own sector chain and require every entry to be
    // zero — the defect's signature, impossible in a valid file.
    let mut minifat_sectors: Vec<u32> = Vec::new();
    let mut cursor = minifat_start;
    while cursor < MAX_SECTORS && minifat_sectors.len() < num_minifat as usize {
        if minifat_sectors.contains(&cursor) {
            return None;
        }
        minifat_sectors.push(cursor);
        cursor = next_in_fat(cursor)?;
    }
    let entries_per_sector = sector_size / 4;
    for &s in &minifat_sectors {
        let off = sector_off(s)?;
        for j in 0..entries_per_sector {
            if get_u32(bytes, off + j * 4)? != 0 {
                return None;
            }
        }
    }

    // Mini-stream chains from the directory: every stream entry below the
    // mini cutoff owns `ceil(size / mini_size)` contiguous mini sectors.
    let total_entries = minifat_sectors.len() * entries_per_sector;
    let mut minifat: Vec<u32> = vec![FREESECT; total_entries];
    let mut dir_sectors: Vec<u32> = Vec::new();
    let mut cursor = dir_start;
    while cursor < MAX_SECTORS {
        if dir_sectors.contains(&cursor) || dir_sectors.len() > 4096 {
            return None;
        }
        dir_sectors.push(cursor);
        cursor = next_in_fat(cursor)?;
    }
    for &s in &dir_sectors {
        let off = sector_off(s)?;
        for e in 0..sector_size / 128 {
            let entry = off + e * 128;
            let object_type = *bytes.get(entry + 66)?;
            if object_type != 2 {
                continue; // streams only; the root's size is the container
            }
            let size = get_u32(bytes, entry + 120)? as u64;
            if size == 0 || size >= mini_cutoff {
                continue;
            }
            let start = get_u32(bytes, entry + 116)?;
            let count = size.div_ceil(mini_size) as usize;
            let (first, end) = (start as usize, start as usize + count);
            if end > total_entries {
                return None;
            }
            for (i, slot) in minifat.iter_mut().enumerate().take(end).skip(first) {
                if *slot != FREESECT {
                    return None; // overlapping chains: not confidently repairable
                }
                *slot = if i + 1 == end { ENDOFCHAIN } else { (i + 1) as u32 };
            }
        }
    }

    // Write the rebuilt table over the zeroed sectors.
    let mut repaired = bytes.to_vec();
    for (n, &s) in minifat_sectors.iter().enumerate() {
        let off = sector_off(s)?;
        for j in 0..entries_per_sector {
            let value = minifat.get(n * entries_per_sector + j).copied().unwrap_or(FREESECT);
            repaired[off + j * 4..off + j * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    Some(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Write};

    /// A compound file with small streams (mini-stream users) and one large
    /// stream, built by the `cfb` crate itself so its geometry is valid.
    fn valid_ole() -> Vec<u8> {
        let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        ole.create_stream("Small").unwrap().write_all(b"tiny mini stream payload").unwrap();
        ole.create_stream("Other").unwrap().write_all(&[0x42u8; 200]).unwrap();
        ole.create_stream("Big").unwrap().write_all(&vec![0x7Au8; 5000]).unwrap();
        ole.into_inner().into_inner()
    }

    fn sector_range(bytes: &[u8], sector: u32) -> std::ops::Range<usize> {
        let size = 1usize << get_u16(bytes, 30).unwrap();
        (sector as usize + 1) * size..(sector as usize + 2) * size
    }

    /// Overwrite every MiniFAT sector with the given fill byte. Zero
    /// replicates the Apple Cocoa writer defect; other fills make chains
    /// that are corrupt without matching the repair's signature.
    fn fill_minifat(bytes: &mut [u8], fill: u8) {
        let size = 1usize << get_u16(bytes, 30).unwrap();
        let minifat_start = get_u32(bytes, 60).unwrap();
        let num_minifat = get_u32(bytes, 64).unwrap();
        let mut fat: Vec<u32> = Vec::new();
        for i in 0..109 {
            let s = get_u32(bytes, 76 + i * 4).unwrap();
            if s >= 0xFFFF_FFFA {
                break;
            }
            let range = sector_range(bytes, s);
            for j in 0..size / 4 {
                fat.push(get_u32(bytes, range.start + j * 4).unwrap());
            }
        }
        let mut cursor = minifat_start;
        for _ in 0..num_minifat {
            let range = sector_range(bytes, cursor);
            bytes[range].fill(fill);
            cursor = fat[cursor as usize];
        }
    }

    fn read_stream(bytes: &[u8], name: &str) -> Vec<u8> {
        let mut ole = open_ole(bytes).unwrap();
        let mut out = Vec::new();
        ole.open_stream(name).unwrap().read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn zeroed_minifat_is_repaired() {
        // The Apple Cocoa writer defect: a zero-filled MiniFAT. The rebuilt
        // chains must make every stream readable with its original bytes.
        let mut bytes = valid_ole();
        fill_minifat(&mut bytes, 0);
        assert!(
            cfb::CompoundFile::open(Cursor::new(bytes.clone())).is_err(),
            "harness sanity: cfb must reject the zeroed MiniFAT on its own"
        );
        assert_eq!(read_stream(&bytes, "Small"), b"tiny mini stream payload");
        assert_eq!(read_stream(&bytes, "Other"), [0x42u8; 200]);
        assert_eq!(read_stream(&bytes, "Big"), vec![0x7Au8; 5000]);
    }

    #[test]
    fn valid_files_bypass_the_repair() {
        // A well-formed file opens directly, and the repair recognizes that
        // it carries no defect.
        let bytes = valid_ole();
        assert_eq!(read_stream(&bytes, "Small"), b"tiny mini stream payload");
        assert!(repair_zeroed_minifat(&bytes).is_none(), "no defect, no repair");
    }

    #[test]
    fn other_minifat_corruption_still_fails() {
        // A MiniFAT that is corrupt but NOT all-zero is outside the repair's
        // signature: the original error must surface unchanged.
        let mut bytes = valid_ole();
        fill_minifat(&mut bytes, 0x01); // every entry: "next is mini sector 0x01010101"
        assert!(repair_zeroed_minifat(&bytes).is_none(), "signature must not match");
        assert!(open_ole(&bytes).is_err());
    }

    #[test]
    fn non_ole_bytes_fail_cleanly() {
        assert!(open_ole(b"not a compound file at all").is_err());
        assert!(repair_zeroed_minifat(b"%PDF-1.4 whatever").is_none());
    }
}
