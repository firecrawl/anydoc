//! In-memory access to CHM archive entries.

use crate::error::ConvertError;
use crate::package::limits;
use lzxd::{Lzxd, WindowSize};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const CONTROL: &str = "::DataSpace/Storage/MSCompressed/ControlData";
const CONTENT: &str = "::DataSpace/Storage/MSCompressed/Content";
const RESET_TABLE: &str = "::DataSpace/Storage/MSCompressed/Transform/{7FC28940-9D31-11D0-9B27-00A0C91E9C7C}/InstanceData/ResetTable";

struct Entry {
    name: String,
    space: u64,
    start: u64,
    length: u64,
}

pub(super) struct Archive<'a> {
    bytes: &'a [u8],
    data_offset: u64,
    lcid: u32,
    entries: Vec<Entry>,
    exact: HashMap<String, usize>,
    folded: HashMap<String, usize>,
    section: Option<Vec<u8>>,
    section_failed: bool,
    cache: HashMap<usize, Rc<[u8]>>,
    total_read: u64,
}

impl<'a> Archive<'a> {
    pub(super) fn open(bytes: &'a [u8]) -> Result<Self, ConvertError> {
        if bytes.get(..4) != Some(b"ITSF") {
            return Err(bad("missing ITSF signature"));
        }
        let version = u32_at(bytes, 4)?;
        let header_len = match version {
            2 => 0x58,
            3 => 0x60,
            _ => return Err(bad("unsupported ITSF version")),
        };
        if u32_at(bytes, 8)? < header_len || bytes.len() < header_len as usize {
            return Err(bad("truncated ITSF header"));
        }
        let lcid = u32_at(bytes, 0x14)?;
        let dir_offset = u64_at(bytes, 0x48)?;
        let dir_length = u64_at(bytes, 0x50)?;
        let data_offset = if version == 3 {
            u64_at(bytes, 0x58)?
        } else {
            dir_offset.checked_add(dir_length).ok_or_else(|| bad("directory offset overflows"))?
        };
        let directory = range(bytes, dir_offset, dir_length)?;
        if directory.get(..4) != Some(b"ITSP")
            || u32_at(directory, 4)? != 1
            || u32_at(directory, 8)? != 0x54
        {
            return Err(bad("invalid ITSP directory header"));
        }
        let block_len = u32_at(directory, 0x10)? as usize;
        let block_count = u32_at(directory, 0x2c)? as usize;
        if !(0x14..=1024 * 1024).contains(&block_len)
            || block_count > directory.len().saturating_sub(0x54) / block_len
        {
            return Err(bad("invalid directory block bounds"));
        }
        if block_count > limits::MAX_ENTRY_COUNT {
            return Err(limit("max_entry_count", "too many CHM directory blocks"));
        }
        let mut block = u32_at(directory, 0x20)? as i32;
        if block < -1 {
            return Err(bad("invalid PMGL directory head"));
        }
        let mut visited = HashSet::new();
        let mut entries = Vec::new();
        let mut entry_count = 0;
        while block >= 0 {
            let index = block as usize;
            if index >= block_count || !visited.insert(index) {
                return Err(bad("invalid or cyclic PMGL directory chain"));
            }
            let offset = 0x54 + index * block_len;
            let leaf = &directory[offset..offset + block_len];
            if leaf.get(..4) != Some(b"PMGL") {
                return Err(bad("invalid PMGL directory block"));
            }
            let end = block_len
                .checked_sub(u32_at(leaf, 4)? as usize)
                .filter(|&end| end >= 0x14)
                .ok_or_else(|| bad("invalid PMGL free-space length"))?;
            let mut pos = 0x14;
            while pos < end {
                entry_count += 1;
                if entry_count > limits::MAX_ENTRY_COUNT {
                    return Err(limit("max_entry_count", "too many CHM directory entries"));
                }
                let used = &leaf[..end];
                let name_len = usize::try_from(cword(used, &mut pos)?)
                    .map_err(|_| bad("entry path length overflows"))?;
                let name_end =
                    pos.checked_add(name_len).ok_or_else(|| bad("entry path overflows"))?;
                let name = std::str::from_utf8(
                    used.get(pos..name_end).ok_or_else(|| bad("truncated entry path"))?,
                )
                .map_err(|_| bad("entry path is not UTF-8"))?;
                pos = name_end;
                let space = cword(used, &mut pos)?;
                let start = cword(used, &mut pos)?;
                let length = cword(used, &mut pos)?;
                if !name.ends_with('/') {
                    entries.push(Entry {
                        name: name.trim_start_matches('/').to_string(),
                        space,
                        start,
                        length,
                    });
                }
            }
            block = u32_at(leaf, 0x10)? as i32;
            if block < -1 {
                return Err(bad("invalid PMGL directory successor"));
            }
        }
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            exact.entry(entry.name.clone()).or_insert(index);
            folded.entry(entry.name.to_ascii_lowercase()).or_insert(index);
        }
        Ok(Self {
            bytes,
            data_offset,
            lcid,
            entries,
            exact,
            folded,
            section: None,
            section_failed: false,
            cache: HashMap::new(),
            total_read: 0,
        })
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    pub(super) fn part(&mut self, name: &str) -> Result<Option<Rc<[u8]>>, ConvertError> {
        let Some(index) = self.index(name) else { return Ok(None) };
        if let Some(bytes) = self.cache.get(&index) {
            return Ok(Some(Rc::clone(bytes)));
        }
        let entry = &self.entries[index];
        if entry.length > limits::MAX_ENTRY_BYTES {
            return Err(limit(
                "max_entry_bytes",
                format!("{} declares {} bytes", entry.name, entry.length),
            ));
        }
        let (space, start, length) = (entry.space, entry.start, entry.length);
        if space == 1 && self.section.is_none() {
            if self.section_failed {
                return Err(bad("unreadable LZX data space"));
            }
            if let Err(error) = self.decode_section() {
                // Every topic shares the stream. Do not replay a corrupt
                // stream for each remaining topic in a large directory.
                self.section_failed = !error.is_fatal();
                return Err(error);
            }
        }
        self.check_budget(length)?;
        let bytes = match space {
            0 => self.stored(&self.entries[index])?,
            1 => range(self.section.as_deref().unwrap(), start, length)?,
            _ => return Err(bad("unsupported CHM data space")),
        };
        let bytes: Rc<[u8]> = Rc::from(bytes);
        self.total_read += length;
        self.cache.insert(index, Rc::clone(&bytes));
        Ok(Some(bytes))
    }

    pub(super) fn lcid(&self) -> u32 {
        self.lcid
    }

    fn index(&self, name: &str) -> Option<usize> {
        let name = name.trim_start_matches('/');
        self.exact.get(name).or_else(|| self.folded.get(&name.to_ascii_lowercase())).copied()
    }

    fn stored(&self, entry: &Entry) -> Result<&'a [u8], ConvertError> {
        let start = self
            .data_offset
            .checked_add(entry.start)
            .ok_or_else(|| bad("entry offset overflows"))?;
        range(self.bytes, start, entry.length)
    }

    fn metadata(&self, name: &str) -> Result<&'a [u8], ConvertError> {
        let index =
            self.index(name).ok_or_else(|| ConvertError::MissingPart { part: name.to_string() })?;
        let entry = &self.entries[index];
        if entry.space != 0 {
            return Err(bad("compressed LZX control metadata"));
        }
        if entry.length > limits::MAX_ENTRY_BYTES {
            return Err(limit("max_entry_bytes", format!("{name} is too large")));
        }
        self.stored(entry)
    }

    fn check_budget(&self, length: u64) -> Result<(), ConvertError> {
        if length > limits::MAX_TOTAL_BYTES.saturating_sub(self.total_read) {
            return Err(limit(
                "max_total_bytes",
                "CHM decompression and retained entries exceed the archive budget",
            ));
        }
        Ok(())
    }

    /// Decode the solid stream once, in physical order. TOC order can jump
    /// arbitrarily between topics; replaying LZX windows per topic would let
    /// a small archive amplify CPU work. Both the section and retained part
    /// copies count against the same fixed memory budget.
    fn decode_section(&mut self) -> Result<(), ConvertError> {
        let control = self.metadata(CONTROL)?;
        if control.get(4..8) != Some(b"LZXC") {
            return Err(bad("invalid LZXC control signature"));
        }
        let unit = match u32_at(control, 8)? {
            1 => 1u64,
            2 => 32768,
            _ => return Err(bad("unsupported LZXC control version")),
        };
        let reset_interval = u64::from(u32_at(control, 12)?) * unit;
        let window_bytes = u64::from(u32_at(control, 16)?) * unit;
        let windows_per_reset = u64::from(u32_at(control, 20)?);
        let window = match window_bytes {
            32768 => WindowSize::KB32,
            65536 => WindowSize::KB64,
            131072 => WindowSize::KB128,
            262144 => WindowSize::KB256,
            524288 => WindowSize::KB512,
            1048576 => WindowSize::MB1,
            2097152 => WindowSize::MB2,
            _ => return Err(bad("invalid CHM LZX window size")),
        };
        if reset_interval == 0
            || windows_per_reset == 0
            || !reset_interval.is_multiple_of(window_bytes / 2)
        {
            return Err(bad("invalid LZX reset interval"));
        }
        let reset_frames = (reset_interval / (window_bytes / 2))
            .checked_mul(windows_per_reset)
            .ok_or_else(|| bad("LZX reset count overflows"))?;
        let table = self.metadata(RESET_TABLE)?;
        if u32_at(table, 0)? != 2 || u64_at(table, 32)? != 32768 {
            return Err(bad("invalid LZX reset table"));
        }
        let count = u64::from(u32_at(table, 4)?);
        let table_offset = u64::from(u32_at(table, 12)?);
        let length = u64_at(table, 16)?;
        let compressed_len = u64_at(table, 24)?;
        self.check_budget(length)?;
        if count != length.div_ceil(32768) || table_offset < 40 {
            return Err(bad("inconsistent LZX reset table length"));
        }
        let offsets = range(
            table,
            table_offset,
            count.checked_mul(8).ok_or_else(|| bad("reset table overflows"))?,
        )?;
        let content = range(self.metadata(CONTENT)?, 0, compressed_len)?;
        if count > 0 && u64_at(offsets, 0)? != 0 {
            return Err(bad("LZX stream does not start at offset zero"));
        }
        let mut decoder = Lzxd::new(window);
        let mut section = Vec::with_capacity(
            usize::try_from(length).map_err(|_| bad("LZX section length overflows"))?,
        );
        for frame in 0..count {
            if frame.is_multiple_of(reset_frames) {
                decoder.reset();
            }
            let start = u64_at(offsets, (frame * 8) as usize)?;
            let end = if frame + 1 == count {
                compressed_len
            } else {
                u64_at(offsets, ((frame + 1) * 8) as usize)?
            };
            let chunk = range(
                content,
                start,
                end.checked_sub(start).ok_or_else(|| bad("LZX offsets are not increasing"))?,
            )?;
            let output_len = (length - frame * 32768).min(32768) as usize;
            // The decoder has assertions for malformed bitstreams. Keep an
            // invalid third-party compression stream a typed conversion error.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                decoder
                    .decompress_next(chunk, output_len)
                    .map(|bytes| section.extend_from_slice(bytes))
            }))
            .map_err(|_| bad("invalid LZX compressed block"))?
            .map_err(|error| bad(format!("corrupt LZX compressed block: {error}")))?;
        }
        self.total_read += length;
        self.section = Some(section);
        Ok(())
    }
}

fn bad(detail: impl Into<String>) -> ConvertError {
    ConvertError::malformed(format!("CHM: {}", detail.into()))
}

fn limit(name: &'static str, detail: impl Into<String>) -> ConvertError {
    ConvertError::ResourceLimit { limit: name, detail: detail.into() }
}

fn range(bytes: &[u8], start: u64, length: u64) -> Result<&[u8], ConvertError> {
    let end = start.checked_add(length).ok_or_else(|| bad("byte range overflows"))?;
    let start = usize::try_from(start).map_err(|_| bad("byte offset overflows"))?;
    let end = usize::try_from(end).map_err(|_| bad("byte offset overflows"))?;
    bytes.get(start..end).ok_or_else(|| bad("byte range runs past the archive"))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ConvertError> {
    let value = bytes
        .get(offset..offset.checked_add(4).ok_or_else(|| bad("offset overflows"))?)
        .ok_or_else(|| bad("truncated binary field"))?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, ConvertError> {
    let value = bytes
        .get(offset..offset.checked_add(8).ok_or_else(|| bad("offset overflows"))?)
        .ok_or_else(|| bad("truncated binary field"))?;
    Ok(u64::from_le_bytes(value.try_into().unwrap()))
}

fn cword(bytes: &[u8], pos: &mut usize) -> Result<u64, ConvertError> {
    let mut value = 0u64;
    loop {
        let byte = *bytes.get(*pos).ok_or_else(|| bad("truncated directory integer"))?;
        *pos += 1;
        if value > u64::MAX >> 7 {
            return Err(bad("directory integer overflows"));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte < 0x80 {
            return Ok(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::limits;

    type Entry<'a> = (&'a str, u64, u64, u64);

    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn cword(value: u64, bytes: &mut Vec<u8>) {
        let mut digits = vec![(value & 0x7f) as u8];
        let mut value = value >> 7;
        while value != 0 {
            digits.push((value & 0x7f) as u8 | 0x80);
            value >>= 7;
        }
        bytes.extend(digits.into_iter().rev());
    }

    /// A self-contained ITSF/ITSP/PMGL container, independent of an external compiler.
    fn container(entries: &[Entry<'_>], data: &[u8]) -> Vec<u8> {
        let mut directory = Vec::new();
        for &(name, space, start, length) in entries {
            cword(name.len() as u64, &mut directory);
            directory.extend_from_slice(name.as_bytes());
            for value in [space, start, length] {
                cword(value, &mut directory);
            }
        }
        let block_len = (directory.len() + 0x16).max(4096).next_power_of_two();
        let dir_offset = 0x60;
        let block_offset = dir_offset + 0x54;
        let data_offset = block_offset + block_len;
        let mut bytes = vec![0u8; data_offset];
        bytes[..4].copy_from_slice(b"ITSF");
        put32(&mut bytes, 4, 3);
        put32(&mut bytes, 8, 0x60);
        put32(&mut bytes, 0x14, 0x0804);
        put64(&mut bytes, 0x48, dir_offset as u64);
        put64(&mut bytes, 0x50, (0x54 + block_len) as u64);
        put64(&mut bytes, 0x58, data_offset as u64);
        bytes[dir_offset..dir_offset + 4].copy_from_slice(b"ITSP");
        put32(&mut bytes, dir_offset + 4, 1);
        put32(&mut bytes, dir_offset + 8, 0x54);
        put32(&mut bytes, dir_offset + 0x10, block_len as u32);
        put32(&mut bytes, dir_offset + 0x1c, u32::MAX);
        put32(&mut bytes, dir_offset + 0x20, 0);
        put32(&mut bytes, dir_offset + 0x24, 0);
        put32(&mut bytes, dir_offset + 0x2c, 1);
        bytes[block_offset..block_offset + 4].copy_from_slice(b"PMGL");
        put32(&mut bytes, block_offset + 4, (block_len - 0x14 - directory.len()) as u32);
        put32(&mut bytes, block_offset + 0x0c, u32::MAX);
        put32(&mut bytes, block_offset + 0x10, u32::MAX);
        bytes[block_offset + 0x14..block_offset + 0x14 + directory.len()]
            .copy_from_slice(&directory);
        bytes.extend_from_slice(data);
        bytes
    }

    fn stored(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut entries = Vec::new();
        for &(name, bytes) in parts {
            entries.push((name, 0, data.len() as u64, bytes.len() as u64));
            data.extend_from_slice(bytes);
        }
        container(&entries, &data)
    }

    fn compressed(payload: &[u8]) -> Vec<u8> {
        // One LZX uncompressed block: optional E8 flag, block kind 3, 24-bit
        // length, alignment, then the three repeated offsets and the raw bytes.
        let bits = (3u32 << 28) | ((payload.len() as u32) << 4);
        let mut stream = Vec::new();
        stream.extend_from_slice(&((bits >> 16) as u16).to_le_bytes());
        stream.extend_from_slice(&(bits as u16).to_le_bytes());
        for _ in 0..3 {
            stream.extend_from_slice(&1u32.to_le_bytes());
        }
        stream.extend_from_slice(payload);
        if !payload.len().is_multiple_of(2) {
            stream.push(0);
        }
        let mut control = vec![0u8; 28];
        put32(&mut control, 0, 6);
        control[4..8].copy_from_slice(b"LZXC");
        for (offset, value) in [(8, 2), (12, 2), (16, 2), (20, 1)] {
            put32(&mut control, offset, value);
        }
        let mut table = vec![0u8; 48];
        put32(&mut table, 0, 2);
        put32(&mut table, 4, 1);
        put32(&mut table, 8, 8);
        put32(&mut table, 12, 40);
        put64(&mut table, 16, payload.len() as u64);
        put64(&mut table, 24, stream.len() as u64);
        put64(&mut table, 32, 32768);
        let content_offset = control.len() + table.len();
        let mut data = control;
        data.extend_from_slice(&table);
        data.extend_from_slice(&stream);
        container(
            &[
                ("/page.html", 1, 0, payload.len() as u64),
                ("::DataSpace/Storage/MSCompressed/ControlData", 0, 0, 28),
                (
                    "::DataSpace/Storage/MSCompressed/Transform/{7FC28940-9D31-11D0-9B27-00A0C91E9C7C}/InstanceData/ResetTable",
                    0,
                    28,
                    48,
                ),
                (
                    "::DataSpace/Storage/MSCompressed/Content",
                    0,
                    content_offset as u64,
                    stream.len() as u64,
                ),
            ],
            &data,
        )
    }

    #[test]
    fn stored_entries_are_read_from_memory_and_repeated_reads_share_bytes() {
        let bytes = stored(&[("/", b""), ("/Page.HTML", b"<h1>Page</h1>"), ("/#SYSTEM", b"meta")]);
        let mut archive = Archive::open(&bytes).unwrap();
        assert_eq!(archive.lcid(), 0x0804);
        assert_eq!(archive.entries().collect::<Vec<_>>(), ["Page.HTML", "#SYSTEM"]);
        let first = archive.part("page.html").unwrap().unwrap();
        let second = archive.part("/Page.HTML").unwrap().unwrap();
        assert_eq!(&*first, b"<h1>Page</h1>");
        assert!(Rc::ptr_eq(&first, &second));
        assert!(archive.part("missing.html").unwrap().is_none());
    }

    #[test]
    fn exact_case_wins_and_case_insensitive_fallback_is_deterministic() {
        let bytes = stored(&[("/Page.html", b"first"), ("/page.html", b"second")]);
        let mut archive = Archive::open(&bytes).unwrap();
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), b"second");
        assert_eq!(&*archive.part("PAGE.HTML").unwrap().unwrap(), b"first");
    }

    #[test]
    fn lzx_section_is_decoded_and_cached() {
        let bytes = compressed(b"<h1>LZX topic</h1>");
        let mut archive = Archive::open(&bytes).unwrap();
        let first = archive.part("page.html").unwrap().unwrap();
        assert_eq!(&*first, b"<h1>LZX topic</h1>");
        assert!(Rc::ptr_eq(&first, &archive.part("page.html").unwrap().unwrap()));
    }

    #[test]
    fn invalid_headers_and_directory_cycles_are_rejected() {
        assert!(Archive::open(b"ITSF").is_err());
        let mut bytes = stored(&[("/page.html", b"page")]);
        put32(&mut bytes, 0x60 + 0x54 + 0x10, 0);
        assert!(Archive::open(&bytes).is_err());
    }

    #[test]
    fn entry_count_is_limited_before_collecting_the_archive() {
        let bytes = container(&vec![("/x", 0, 0, 0); limits::MAX_ENTRY_COUNT + 1], &[]);
        assert!(matches!(
            Archive::open(&bytes),
            Err(ConvertError::ResourceLimit { limit: "max_entry_count", .. })
        ));
    }

    #[test]
    fn oversized_and_overflowing_entries_fail_before_reading() {
        let bytes = container(
            &[
                ("/huge.html", 0, 0, limits::MAX_ENTRY_BYTES + 1),
                ("/overflow.html", 0, u64::MAX, 8),
            ],
            &[],
        );
        let mut archive = Archive::open(&bytes).unwrap();
        assert!(matches!(
            archive.part("huge.html"),
            Err(ConvertError::ResourceLimit { limit: "max_entry_bytes", .. })
        ));
        assert!(archive.part("overflow.html").is_err());
    }

    #[test]
    fn oversized_solid_section_is_rejected_before_allocating() {
        let mut bytes = compressed(b"topic");
        let data_offset = u64_at(&bytes, 0x58).unwrap() as usize;
        put64(&mut bytes, data_offset + 28 + 16, limits::MAX_TOTAL_BYTES + 1);
        let mut archive = Archive::open(&bytes).unwrap();
        assert!(matches!(
            archive.part("page.html"),
            Err(ConvertError::ResourceLimit { limit: "max_total_bytes", .. })
        ));
    }

    #[test]
    fn corrupt_solid_stream_is_not_replayed_for_later_topics() {
        let mut bytes = compressed(b"topic");
        bytes.pop();
        let mut archive = Archive::open(&bytes).unwrap();
        assert!(archive.part("page.html").is_err());
        assert!(archive.section_failed);
        assert!(archive.part("page.html").is_err());
        assert!(archive.section.is_none());
    }
}
