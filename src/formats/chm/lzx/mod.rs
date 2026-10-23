//! This library implements the LZX compression format as described in
//! [LZX DELTA Compression and Decompression], revision 9.0.
//!
//! Lempel-Ziv Extended (LZX) is an LZ77-based compression engine, as described in [UASDC],
//! that is a universal lossless data compression algorithm. It performs no analysis on the
//! data.
//!
//! Lempel-Ziv Extended Delta (LZXD) is a derivative of the Lempel-Ziv Extended (LZX) format with
//! some modifications to facilitate efficient delta compression.
//!
//! In order to use this module, refer to the main [`Lzxd`] type and its methods.
//!
//! [LZX DELTA Compression and Decompression]: https://docs.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-patch/cc78752a-b4af-4eee-88cb-01f4d8a4c2bf
//! [UASDC]: https://ieeexplore.ieee.org/document/1055714
//! [`Lzxd`]: struct.Lzxd.html
use std::{fmt, mem};

pub(crate) use bitstream::Bitstream;
pub(crate) use block::{Block, Decoded, Kind as BlockKind};
pub(crate) use tree::{CanonicalTree, Tree};
use window::Window;
pub use window::WindowSize;

mod bitstream;
mod block;
mod tree;
mod window;

/// A chunk represents exactly 32 KB of uncompressed data until the last chunk in the stream,
/// which can represent less than 32 KB.
pub const MAX_CHUNK_SIZE: usize = 32 * 1024;

/// Decoder state needed for new blocks.
// TODO not sure how much we want to keep in DecoderState and Lzxd respectively
pub(crate) struct DecoderState {
    /// The window size we're working with.
    window_size: WindowSize,

    /// This tree cannot be used directly, it exists only to apply the delta of upcoming trees
    /// to its path lengths.
    main_tree: CanonicalTree,

    /// This tree cannot be used directly, it exists only to apply the delta of upcoming trees
    /// to its path lengths.
    length_tree: CanonicalTree,
}

struct PostProcessState {
    /// The pointer in the file at which to stop performing E8 translation.
    e8_translation_size: i32,

    /// A buffer that can be used to hold postprocessed chunks.
    data_chunk: Box<[u8]>,
}

/// The main interface to perform LZXD decompression.
///
/// This structure stores the required state to process the compressed chunks of data in a
/// sequential order.
///
pub struct Lzxd {
    /// Sliding window into which data is decompressed.
    window: Window,

    /// Current decoder state.
    state: DecoderState,

    /// > The three most recent real match offsets are kept in a list.
    r: [u32; 3],

    /// The current offset into the decompressed data.
    chunk_offset: usize,

    /// Match history available since the most recent dictionary reset.
    history_len: usize,

    /// Has the very first chunk been read yet? Unlike the rest, it has additional data.
    first_chunk_read: bool,

    /// Current block.
    current_block: Block,

    /// Information and data related to E8 postprocessing. This is populated after
    /// the first chunk is read.
    postprocess: Option<PostProcessState>,
}

/// Specific cause for decompression failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeFailed {
    /// The chunk data caused a read of more items than the current block had in a single step.
    OverreadBlock,

    /// There was not enough data in the chunk to fully decode, and a premature end was found.
    UnexpectedEof,

    /// An invalid block type was found.
    InvalidBlock(u8),

    /// An invalid block size was found.
    InvalidBlockSize(u32),

    /// An invalid pretree element was found.
    InvalidPretreeElement(u16),

    /// Invalid pretree run-length encoding.
    InvalidPretreeRle,

    /// When attempting to construct a decode tree, we encountered an invalid path length tree.
    InvalidPathLengths,

    /// A required decode tree was empty (all path lengths were 0).
    EmptyTree,

    /// A match references zero, unavailable history, or bytes outside the window.
    InvalidMatchOffset,

    /// The given window size was too small.
    WindowTooSmall,

    /// Tried to read a chunk longer than [`MAX_CHUNK_SIZE`].
    ///
    /// [`MAX_CHUNK_SIZE`]: constant.MAX_CHUNK_SIZE.html
    ChunkTooLong,
}

impl fmt::Display for DecodeFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DecodeFailed::*;

        match self {
            OverreadBlock => {
                write!(f, "read more items than available in the block in a single step")
            }
            UnexpectedEof => write!(f, "reached end of chunk without fully decoding it"),
            InvalidBlock(kind) => write!(f, "block type {} is invalid", kind),
            InvalidBlockSize(size) => write!(f, "block size {} is invalid", size),
            InvalidPretreeElement(elem) => write!(f, "found invalid pretree element {}", elem),
            InvalidPretreeRle => write!(f, "found invalid pretree rle element"),
            InvalidPathLengths => write!(f, "encountered invalid path lengths"),
            EmptyTree => write!(f, "encountered empty decode tree"),
            InvalidMatchOffset => write!(f, "match offset is outside the available history"),
            WindowTooSmall => write!(f, "decode window was too small"),
            ChunkTooLong => write!(f, "tried reading a chunk longer than {} bytes", MAX_CHUNK_SIZE),
        }
    }
}

impl std::error::Error for DecodeFailed {}

/// The error type used when decompression fails.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DecompressError(DecodeFailed);

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for DecompressError {}

impl From<DecodeFailed> for DecompressError {
    fn from(value: DecodeFailed) -> Self {
        Self(value)
    }
}

impl Lzxd {
    /// Creates a new instance of the LZXD decoder state. The [`WindowSize`] must be obtained
    /// from elsewhere (e.g. it may be predetermined to a certain value), and if it's wrong,
    /// the decompressed values won't be those expected.
    ///
    /// [`WindowSize`]: enum.WindowSize.html
    pub fn new(window_size: WindowSize) -> Self {
        // > The main tree comprises 256 elements that correspond to all possible 8-bit
        // > characters, plus 8 * NUM_POSITION_SLOTS elements that correspond to matches.
        let main_tree = CanonicalTree::new(256 + 8 * window_size.position_slots());

        // > The length tree comprises 249 elements.
        let length_tree = CanonicalTree::new(249);

        Self {
            window: window_size.create_buffer(),
            // > Because trees are output several times during compression of large amounts of
            // > data (multiple blocks), LZXD optimizes compression by encoding only the delta
            // > path lengths lengths between the current and previous trees.
            //
            // Because it uses deltas, we need to store the previous value across blocks.
            state: DecoderState { window_size, main_tree, length_tree },
            // > The initial state of R0, R1, R2 is (1, 1, 1).
            r: [1, 1, 1],
            first_chunk_read: false,
            chunk_offset: 0,
            history_len: 0,
            postprocess: None,
            // Start with some dummy value.
            current_block: Block {
                remaining: 0,
                size: 0,
                kind: BlockKind::Uncompressed { r: [1, 1, 1] },
            },
        }
    }

    /// Try reading the header for the first chunk.
    fn try_read_first_chunk(&mut self, bitstream: &mut Bitstream) -> Result<(), DecodeFailed> {
        // > The first bit in the first chunk in the LZXD bitstream (following the 2-byte,
        // > chunk-size prefix described in section 2.2.1) indicates the presence or absence of
        // > two 16-bit fields immediately following the single bit. If the bit is set, E8
        // > translation is enabled.
        if !self.first_chunk_read {
            self.first_chunk_read = true;

            let e8_translation = bitstream.read_bit()? != 0;
            self.postprocess = if e8_translation {
                Some(PostProcessState {
                    data_chunk: vec![0; MAX_CHUNK_SIZE].into_boxed_slice(),
                    e8_translation_size: bitstream.read_bits(32)? as i32,
                })
            } else {
                None
            };
        }

        Ok(())
    }

    /// Attempts to perform post-decompression E8 fixups on an output data buffer.
    fn postprocess(
        translation_size: i32,
        chunk_offset: usize,
        idata: &mut [u8],
    ) -> Result<&[u8], DecodeFailed> {
        let mut processed = 0usize;

        // Find the next E8 match, or finish once there are no more E8 matches.
        while let Some(pos) =
            idata[processed..].iter().position(|&e| e == 0xE8).map(|pos| processed + pos)
        {
            // N.B: E8 fixups are only performed for up to 10 bytes before the end of a chunk.
            if idata.len() - pos <= 10 {
                break;
            }

            // This is the current file output pointer.
            let current_pointer = chunk_offset + pos;

            // Match. Fix up the following bytes.
            let abs_val = i32::from_le_bytes([
                idata[pos + 1],
                idata[pos + 2],
                idata[pos + 3],
                idata[pos + 4],
            ]);
            if (abs_val >= -(current_pointer as i32)) && abs_val < translation_size {
                let rel_val = if abs_val >= 0 {
                    abs_val.wrapping_sub(current_pointer as i32)
                } else {
                    abs_val.wrapping_add(translation_size)
                };

                idata[pos + 1..pos + 5].copy_from_slice(&rel_val.to_le_bytes());
            }

            processed = pos + 5;
        }

        Ok(idata)
    }

    /// Decompresses the next compressed `chunk` from the LZXD data stream.
    pub fn decompress_next_stream(
        &mut self,
        chunk: &[u8],
        output_len: usize,
    ) -> Result<(&[u8], usize), DecompressError> {
        if output_len > MAX_CHUNK_SIZE {
            return Err(DecodeFailed::ChunkTooLong.into());
        }
        // > A chunk represents exactly 32 KB of uncompressed data until the last chunk in the
        // > stream, which can represent less than 32 KB.
        //
        // > The LZXD engine encodes a compressed, chunk-size prefix field preceding each
        // > compressed chunk in the compressed byte stream. The compressed, chunk-size prefix
        // > field is a byte aligned, little-endian, 16-bit field.
        //
        // However, this doesn't seem to be part of LZXD itself? At least when testing with
        // `.xnb` files, every chunk comes with a compressed chunk size unless it has the flag
        // set to 0xff where it also includes the uncompressed chunk size.
        //
        // TODO maybe the docs could clarify whether this length is compressed or not

        let mut bitstream = Bitstream::new(chunk);

        self.try_read_first_chunk(&mut bitstream)?;

        let mut decoded_len = 0;
        while decoded_len != output_len {
            if self.current_block.remaining == 0 {
                // Re-align the bitstream to word
                // Related: https://github.com/GNOME/gcab/blob/master/libgcab/decomp.c#L883.
                // Related: https://github.com/kyz/libmspack/blob/master/libmspack/mspack/lzxd.c#L469
                if matches!(self.current_block.kind, BlockKind::Uncompressed { .. })
                    && !self.current_block.size.is_multiple_of(2)
                {
                    bitstream.read_byte();
                }
                self.current_block = Block::read(&mut bitstream, &mut self.state)?;
                assert_ne!(self.current_block.remaining, 0);
            }

            let decoded = self.current_block.decode_element(&mut bitstream, &mut self.r)?;

            let advance = match decoded {
                Decoded::Single(value) => {
                    self.window.push(value);
                    1
                }
                Decoded::Match { offset, length } => {
                    if offset == 0
                        || offset > self.state.window_size as usize
                        || offset > self.history_len + decoded_len
                    {
                        return Err(DecodeFailed::InvalidMatchOffset.into());
                    }
                    self.window.copy_from_self(offset, length);
                    length
                }
                Decoded::Read(length) => {
                    // Read up to end of chunk, to allow for larger blocks.
                    let length =
                        length.min(bitstream.remaining_bytes()).min(output_len - decoded_len);
                    // Will re-align if needed, just as decompressed reads mandate.
                    self.window.copy_from_bitstream(&mut bitstream, length)?;
                    length
                }
            };

            if advance == 0 {
                return Err(DecodeFailed::UnexpectedEof.into());
            }

            if advance > output_len - decoded_len {
                return Err(DecodeFailed::OverreadBlock.into());
            }
            decoded_len += advance;
            if let Some(value) = self.current_block.remaining.checked_sub(advance as u32) {
                self.current_block.remaining = value;
            } else {
                return Err(DecodeFailed::OverreadBlock.into());
            }
        }

        // Each compressed frame ends on a 16-bit boundary; the bit reader has
        // already fetched its last padded word. Odd uncompressed blocks carry
        // one additional padding byte, including when they end at the boundary.
        if self.current_block.remaining == 0
            && matches!(self.current_block.kind, BlockKind::Uncompressed { .. })
            && !self.current_block.size.is_multiple_of(2)
        {
            bitstream.read_byte().ok_or(DecodeFailed::UnexpectedEof)?;
            self.current_block.size = 0;
        }
        let consumed = chunk.len() - bitstream.remaining_bytes();
        let chunk_offset = self.chunk_offset;
        self.chunk_offset += decoded_len;
        self.history_len = (self.history_len + decoded_len).min(self.state.window_size as usize);

        let view = self.window.past_view(decoded_len)?;
        if let Some(postprocess) = self.postprocess.as_mut() {
            // E8 fixups are disabled after 1GB of input data,
            // or if the chunk size is too small.
            if chunk_offset >= 0x4000_0000 || decoded_len <= 10 {
                Ok((view, consumed))
            } else {
                let postprocess_buf = &mut postprocess.data_chunk[..decoded_len];
                postprocess_buf.copy_from_slice(view);

                // E8 fixups are enabled. Postprocess the output buffer.
                let view = Self::postprocess(
                    postprocess.e8_translation_size,
                    chunk_offset,
                    postprocess_buf,
                )?;
                Ok((view, consumed))
            }
        } else {
            Ok((view, consumed))
        }
    }

    #[cfg(test)]
    fn decompress_next(
        &mut self,
        chunk: &[u8],
        output_len: usize,
    ) -> Result<&[u8], DecompressError> {
        self.decompress_next_stream(chunk, output_len).map(|(bytes, _)| bytes)
    }

    /// Reset compression state while preserving the absolute E8 output position.
    pub fn reset(&mut self) {
        let mut this = Self::new(self.state.window_size);
        this.chunk_offset = self.chunk_offset;
        let _ = mem::replace(self, this);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_uncompressed() {
        let data = [
            0x00, 0x30, 0x30, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00, b'a', b'b', b'c', 0x00,
        ];

        let mut lzxd = Lzxd::new(WindowSize::KB32); // size does not matter
        let res = lzxd.decompress_next(&data, 3);
        assert_eq!(res.unwrap(), [b'a', b'b', b'c']);
    }

    #[test]
    fn reset() {
        let data = [
            0x00, 0x30, 0x30, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00, b'a', b'b', b'c', 0x00,
        ];

        let mut lzxd = Lzxd::new(WindowSize::KB32); // size does not matter
        let res = lzxd.decompress_next(&data, 3);
        assert_eq!(res.unwrap(), [b'a', b'b', b'c']);

        lzxd.reset();
        let res = lzxd.decompress_next(&data, 3);
        assert_eq!(res.unwrap(), [b'a', b'b', b'c']);
    }

    #[test]
    fn check_e8() {
        let data = [
            0x5B, 0x80, 0x80, 0x8D, 0x00, 0x30, 0x80, 0x0A, 0x18, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x54, 0x68, 0x69, 0x73, 0x20, 0x66, 0x69, 0x6C,
            0x65, 0x20, 0x68, 0x61, 0x73, 0x20, 0x61, 0x6E, 0x20, 0x45, 0x38, 0x20, 0x62, 0x79,
            0x74, 0x65, 0x20, 0x74, 0x6F, 0x20, 0x74, 0x65, 0x73, 0x74, 0x20, 0x45, 0x38, 0x20,
            0x74, 0x72, 0x61, 0x6E, 0x73, 0x6C, 0x61, 0x74, 0x69, 0x6F, 0x6E, 0x2C, 0x20, 0x58,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0xE8, 0x7B,
            0x00, 0x00, 0x00, 0xE8, 0x7B, 0x00, 0x00, 0x00, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
            0x64, 0x64, 0x64, 0x64, 0x64, 0x64,
        ];

        let mut lzxd = Lzxd::new(WindowSize::KB32);
        let res = lzxd.decompress_next(&data, 168);
        assert_eq!(
            res.unwrap(),
            b"This file has an E8 byte to test E8 translation, Xdddddddddddddddd\
              dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\
              dddddddddddddd\xE8\xE9\xFF\xFF\xFF\xE8\xE4\xFF\xFF\xFFdddddddddddd"
        );
    }

    #[test]
    fn uncompressed_single_byte_data() {
        let mut lzxd = Lzxd::new(WindowSize::KB32);
        let res = lzxd.decompress_next(&[0x00], 32);
        assert_eq!(res.unwrap_err().0, DecodeFailed::UnexpectedEof);
    }

    #[test]
    fn uncompressed_advance_zero() {
        // Regression test for "assertion `left != right` failed: left: 0, right: 0" panic
        // This occurs when the decoder cannot advance (advance == 0)
        // Data from fuzzer crash
        let data = vec![
            0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32,
            0x32, 0x32,
        ];

        let mut lzxd = Lzxd::new(WindowSize::KB32);
        let res = lzxd.decompress_next(&data, 32);
        assert_eq!(res.unwrap_err().0, DecodeFailed::UnexpectedEof);
    }

    #[test]
    fn peek_bits_oneword_bounds_check() {
        // Regression test for peek_bits_oneword index out of bounds panic
        // This exercises the buffer.len() < 2 check in peek_bits_oneword
        // Data from fuzzer crash
        let data = vec![
            0x12, 0x12, 0x12, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12,
            0x12, 0x12, 0x12, 0x12, 0x12, 0x12, 0x12, 0x12, 0x00, 0x00, 0x00, 0x12, 0x8a,
        ];

        let mut lzxd = Lzxd::new(WindowSize::KB32);
        let res = lzxd.decompress_next(&data, 32);
        assert_eq!(res.unwrap_err().0, DecodeFailed::UnexpectedEof);
    }
}
