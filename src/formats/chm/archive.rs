//! In-memory access to CHM archive entries.

use super::lzx::{Lzxd, WindowSize};
use crate::error::ConvertError;
use crate::package::limits;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const CONTROL: &str = "::DataSpace/Storage/MSCompressed/ControlData";
const CONTENT: &str = "::DataSpace/Storage/MSCompressed/Content";
const SPAN_INFO: &str = "::DataSpace/Storage/MSCompressed/SpanInfo";
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
        if reset_interval == 0 || !reset_interval.is_multiple_of(32768) {
            return Err(bad("invalid LZX reset interval"));
        }
        // ControlData's reset interval is expressed in bytes (version 1) or
        // 32 KiB units (version 2), independently of the dictionary size. The
        // following unknown field does not alter the decoder's reset timing.
        let reset_frames = reset_interval / 32768;
        let content = self.metadata(CONTENT)?;
        let (length, content, offsets) = if self.index(RESET_TABLE).is_some() {
            let table = self.metadata(RESET_TABLE)?;
            if u32_at(table, 0)? != 2 || u64_at(table, 32)? != 32768 {
                return Err(bad("invalid LZX reset table"));
            }
            let entry_width = match u32_at(table, 8)? {
                width @ (4 | 8) => u64::from(width),
                _ => return Err(bad("invalid LZX reset table entry size")),
            };
            let count = u64::from(u32_at(table, 4)?);
            let table_offset = u64::from(u32_at(table, 12)?);
            let length = u64_at(table, 16)?;
            self.check_budget(length)?;
            if count != length.div_ceil(32768) || table_offset < 40 {
                return Err(bad("inconsistent LZX reset table length"));
            }
            let offsets = range(table, table_offset, count * entry_width)?;
            let content = range(content, 0, u64_at(table, 24)?)?;
            (length, content, Some((offsets, entry_width)))
        } else {
            let span = self.metadata(SPAN_INFO)?;
            if span.len() != 8 {
                return Err(bad("invalid LZX SpanInfo length"));
            }
            let length = u64_at(span, 0)?;
            if length == 0 {
                return Err(bad("empty LZX SpanInfo section"));
            }
            self.check_budget(length)?;
            (length, content, None)
        };
        let count = length.div_ceil(32768);
        let offset_at = |frame| {
            let (offsets, width) = offsets.unwrap();
            let offset = (frame * width) as usize;
            if width == 4 {
                u32_at(offsets, offset).map(u64::from)
            } else {
                u64_at(offsets, offset)
            }
        };
        if offsets.is_some() && count > 0 && offset_at(0)? != 0 {
            return Err(bad("LZX stream does not start at offset zero"));
        }
        let mut decoder = Lzxd::new(window);
        let mut section = Vec::with_capacity(
            usize::try_from(length).map_err(|_| bad("LZX section length overflows"))?,
        );
        let mut next_offset = 0;
        for frame in 0..count {
            if frame.is_multiple_of(reset_frames) {
                decoder.reset();
            }
            let (start, end) = if offsets.is_some() {
                (
                    offset_at(frame)?,
                    if frame + 1 == count { content.len() as u64 } else { offset_at(frame + 1)? },
                )
            } else {
                (next_offset, content.len() as u64)
            };
            let chunk = range(
                content,
                start,
                end.checked_sub(start).ok_or_else(|| bad("LZX offsets are not increasing"))?,
            )?;
            let output_len = (length - frame * 32768).min(32768) as usize;
            // The decoder has assertions for malformed bitstreams. Keep an
            // invalid third-party compression stream a typed conversion error.
            let consumed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                decoder.decompress_next_stream(chunk, output_len).map(|(bytes, consumed)| {
                    section.extend_from_slice(bytes);
                    consumed
                })
            }))
            .map_err(|_| bad("invalid LZX compressed block"))?
            .map_err(|error| bad(format!("corrupt LZX compressed block: {error}")))?;
            if consumed == 0 {
                return Err(bad("LZX frame did not consume input"));
            }
            next_offset = start + consumed as u64;
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

    /// Write MSB-first bits as the little-endian 16-bit words used by LZX.
    #[derive(Default)]
    struct Bits(Vec<bool>);

    impl Bits {
        fn push(&mut self, value: u32, count: u8) {
            self.0.extend((0..count).rev().map(|shift| value & (1 << shift) != 0));
        }

        fn finish(mut self) -> Vec<u8> {
            self.0.resize(self.0.len().next_multiple_of(16), false);
            self.0
                .chunks(16)
                .flat_map(|bits| {
                    bits.iter().fold(0u16, |word, &bit| (word << 1) | u16::from(bit)).to_le_bytes()
                })
                .collect()
        }

        fn tree_lengths(&mut self, lengths: &[u8]) {
            // Complete canonical pretree: 12 codes of length 4, 8 of length 5.
            for symbol in 0..20 {
                self.push(if symbol < 12 { 4 } else { 5 }, 4);
            }
            for &length in lengths {
                let delta = (17 - u32::from(length)) % 17;
                if delta < 12 {
                    self.push(delta, 4);
                } else {
                    self.push(24 + delta - 12, 5);
                }
            }
        }
    }

    /// Original verbatim-Huffman LZX block spanning two frames. The second
    /// frame contains only matches, so decoding it depends on the first frame's
    /// dictionary, main tree, and unfinished block. A second group must reset.
    fn huffman_group(literal: u8) -> Vec<Vec<u8>> {
        huffman_group_with_header(literal, true)
    }

    fn huffman_group_with_header(literal: u8, first_chunk: bool) -> Vec<Vec<u8>> {
        let mut bits = Bits::default();
        if first_chunk {
            bits.push(0, 1); // No E8 translation.
        }
        bits.push(1, 3); // Verbatim Huffman block, not an uncompressed LZX block.
        bits.push(65536, 24);
        let mut main = [0u8; 512]; // 64 KiB window: 32 position slots.
        main[usize::from(b'A')] = 2;
        main[usize::from(b'B')] = 2;
        main[261] = 2; // R0 match of length 7.
        main[262] = 2; // R0 match of length 8.
        bits.tree_lengths(&main[..256]);
        bits.tree_lengths(&main[256..]);
        bits.tree_lengths(&[0; 249]);
        bits.push(u32::from(literal - b'A'), 2);
        for _ in 0..4095 {
            bits.push(3, 2);
        }
        bits.push(2, 2);
        let first = bits.finish();
        let mut bits = Bits::default();
        for _ in 0..4096 {
            bits.push(3, 2);
        }
        vec![first, bits.finish()]
    }

    fn framed(chunks: &[Vec<u8>], length: u64, width: Option<u32>, window_units: u32) -> Vec<u8> {
        let mut control = vec![0; 28];
        put32(&mut control, 0, 6);
        control[4..8].copy_from_slice(b"LZXC");
        for (offset, value) in [(8, 2), (12, 2), (16, window_units), (20, 1)] {
            put32(&mut control, offset, value);
        }
        let stream = chunks.concat();
        let metadata = if let Some(width) = width {
            let mut table = vec![0; 40 + chunks.len() * width as usize];
            put32(&mut table, 0, 2);
            put32(&mut table, 4, chunks.len() as u32);
            put32(&mut table, 8, width);
            put32(&mut table, 12, 40);
            put64(&mut table, 16, length);
            put64(&mut table, 24, stream.len() as u64);
            put64(&mut table, 32, 32768);
            let mut offset = 0;
            for (i, chunk) in chunks.iter().enumerate() {
                if width == 4 {
                    put32(&mut table, 40 + i * 4, offset as u32);
                } else {
                    put64(&mut table, 40 + i * 8, offset);
                }
                offset += chunk.len() as u64;
            }
            table
        } else {
            length.to_le_bytes().to_vec()
        };
        let mut data = control;
        data.extend_from_slice(&metadata);
        data.extend_from_slice(&stream);
        container(
            &[
                ("/page.html", 1, 0, length),
                (CONTROL, 0, 0, 28),
                (
                    if width.is_some() { RESET_TABLE } else { SPAN_INFO },
                    0,
                    28,
                    metadata.len() as u64,
                ),
                (CONTENT, 0, 28 + metadata.len() as u64, stream.len() as u64),
            ],
            &data,
        )
    }

    #[test]
    fn four_byte_reset_offsets_decode_huffman_frames_with_shared_dictionary() {
        let chunks = huffman_group(b'A');
        let bytes = framed(&chunks, 65536, Some(4), 2);
        let mut archive = Archive::open(&bytes).unwrap();
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), vec![b'A'; 65536]);
    }

    #[test]
    fn huffman_frames_reset_at_control_interval_independently_of_unknown_field() {
        // A 64 KiB reset interval. The unknown
        // ControlData field at offset 20 must not change reset timing.
        let mut chunks = huffman_group(b'A');
        chunks.extend(huffman_group(b'B'));
        let mut bytes = framed(&chunks, 131072, Some(8), 2);
        let data_offset = u64_at(&bytes, 0x58).unwrap() as usize;
        put32(&mut bytes, data_offset + 20, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        let mut expected = vec![b'A'; 65536];
        expected.extend(vec![b'B'; 65536]);
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), expected);
    }

    #[test]
    fn spaninfo_decodes_huffman_frames_with_dictionary_and_resets() {
        let mut chunks = huffman_group(b'A');
        chunks.extend(huffman_group(b'B'));
        let bytes = framed(&chunks, 131072, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        let mut expected = vec![b'A'; 65536];
        expected.extend(vec![b'B'; 65536]);
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), expected);
    }

    fn raw_stream(payload: &[u8], e8_size: Option<u32>) -> Vec<u8> {
        let mut bits = Bits::default();
        bits.push(u32::from(e8_size.is_some()), 1);
        if let Some(size) = e8_size {
            bits.push(size, 32);
        }
        bits.push(3, 3);
        bits.push(payload.len() as u32, 24);
        let mut stream = bits.finish();
        for _ in 0..3 {
            stream.extend_from_slice(&1u32.to_le_bytes());
        }
        stream.extend_from_slice(payload);
        if !payload.len().is_multiple_of(2) {
            stream.push(0);
        }
        stream
    }

    #[test]
    fn spaninfo_streams_unfinished_raw_blocks_and_final_odd_padding() {
        let expected = (0..65537).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let stream = raw_stream(&expected, None);
        let mut bytes = framed(&[stream], expected.len() as u64, None, 2);
        let data_offset = u64_at(&bytes, 0x58).unwrap() as usize;
        put32(&mut bytes, data_offset + 12, 4);
        let mut archive = Archive::open(&bytes).unwrap();
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), expected);
    }

    #[test]
    fn spaninfo_consumes_odd_block_padding_at_a_frame_boundary() {
        let mut stream = raw_stream(&vec![b'A'; 32767], None);
        let continuation = |payload: &[u8]| {
            let mut bits = Bits::default();
            bits.push(3, 3);
            bits.push(payload.len() as u32, 24);
            let mut block = bits.finish();
            for _ in 0..3 {
                block.extend_from_slice(&1u32.to_le_bytes());
            }
            block.extend_from_slice(payload);
            if !payload.len().is_multiple_of(2) {
                block.push(0);
            }
            block
        };
        stream.extend(continuation(b"B"));
        stream.extend(continuation(&vec![b'C'; 32768]));
        let bytes = framed(&[stream], 65536, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        let mut expected = vec![b'A'; 32767];
        expected.push(b'B');
        expected.extend(vec![b'C'; 32768]);
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), expected);
    }

    #[test]
    fn spaninfo_preserves_e8_positions_across_frames() {
        let mut payload = vec![b'x'; 65536];
        let mut expected = payload.clone();
        for position in [20, 32768 + 20] {
            payload[position] = 0xe8;
            payload[position + 1..position + 5].copy_from_slice(&50000i32.to_le_bytes());
            expected[position] = 0xe8;
            expected[position + 1..position + 5]
                .copy_from_slice(&(50000i32 - position as i32).to_le_bytes());
        }
        let stream = raw_stream(&payload, Some(65536));
        let bytes = framed(&[stream], payload.len() as u64, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), expected);
    }

    #[test]
    fn spaninfo_keeps_absolute_e8_positions_across_dictionary_resets() {
        let mut payload = vec![b'x'; 65536];
        payload[20] = 0xe8;
        payload[21..25].copy_from_slice(&50000i32.to_le_bytes());
        let stream = raw_stream(&payload, Some(131072));
        let bytes = framed(&[stream.clone(), stream], 131072, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        let topic = archive.part("page.html").unwrap().unwrap();
        assert_eq!(&topic[21..25], &49980i32.to_le_bytes());
        assert_eq!(&topic[65536 + 21..65536 + 25], &(-15556i32).to_le_bytes());
    }

    #[test]
    fn spaninfo_translates_an_e8_operand_targeting_position_zero() {
        let mut payload = vec![b'x'; 32];
        payload[20] = 0xe8;
        payload[21..25].copy_from_slice(&0i32.to_le_bytes());
        let stream = raw_stream(&payload, Some(65536));
        let bytes = framed(&[stream], payload.len() as u64, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        let topic = archive.part("page.html").unwrap().unwrap();
        assert_eq!(&topic[21..25], &(-20i32).to_le_bytes());
    }

    #[test]
    fn invalid_repeated_match_offsets_return_errors_without_unwinding() {
        for offset in [0, 65536, u32::MAX] {
            let mut raw = raw_stream(&vec![b'A'; 32768], None);
            put32(&mut raw, 4, offset);
            let following = huffman_group_with_header(b'A', false);
            let result = std::panic::catch_unwind(|| {
                let mut decoder = Lzxd::new(WindowSize::KB64);
                decoder.decompress_next_stream(&raw, 32768).unwrap();
                decoder.decompress_next_stream(&following[0], 32768).is_err()
            });
            assert!(
                matches!(result, Ok(true)),
                "invalid R0 {offset} must return an error without panicking"
            );
        }
    }

    #[test]
    fn spaninfo_size_budget_and_truncation_are_enforced() {
        let bytes = framed(&[], limits::MAX_TOTAL_BYTES + 1, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        assert!(matches!(
            archive.decode_section(),
            Err(ConvertError::ResourceLimit { limit: "max_total_bytes", .. })
        ));
        let stream = raw_stream(b"truncated topic", None);
        let bytes = framed(&[stream[..stream.len() - 2].to_vec()], 15, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        assert!(matches!(archive.part("page.html"), Err(ConvertError::Malformed { .. })));
        assert!(archive.section_failed);
        assert!(archive.section.is_none());
    }

    #[test]
    fn spaninfo_decodes_a_single_frame_without_reset_table() {
        let source = compressed(b"span topic");
        let archive = Archive::open(&source).unwrap();
        let stream = archive.metadata(CONTENT).unwrap().to_vec();
        let bytes = framed(&[stream], 10, None, 2);
        let mut archive = Archive::open(&bytes).unwrap();
        assert_eq!(&*archive.part("page.html").unwrap().unwrap(), b"span topic");
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
