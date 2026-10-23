/// > An LZXD bitstream is encoded as a sequence of aligned 16-bit integers stored in the
/// > least-significant- byte to most-significant-byte order, also known as byte-swapped,
/// > or little-endian, words. Given an input stream of bits named a, b, c,..., x, y, z,
/// > A, B, C, D, E, F, the output byte stream MUST be as [ 0| 1| 2| 3|...|30|31].
///
/// It is worth mentioning that older revisions of the document explain this better:
///
/// > Given an input stream of bits named a, b, c, ..., x, y, z, A, B, C, D, E, F, the output
/// > byte stream (with byte boundaries highlighted) would be as follows:
/// > [i|j|k|l|m|n|o#p|a|b|c|d|e|f|g|h#y|z|A|B|C|D|E|F#q|r|s|t|u|v|w|x]
use super::DecodeFailed;

pub struct Bitstream<'a> {
    buffer: &'a [u8],
    // Next number in the bitstream.
    n: u16,
    // How many bits left in the current `n`.
    remaining: u8,
}

impl<'a> Bitstream<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer, n: 0, remaining: 0 }
    }

    // Advance the buffer to the next 16-bit integer.
    fn advance_buffer(&mut self) -> Result<(), DecodeFailed> {
        if self.buffer.len() < 2 {
            return Err(DecodeFailed::UnexpectedEof);
        }

        self.remaining = 16;
        self.n = u16::from_le_bytes([self.buffer[0], self.buffer[1]]);
        self.buffer = &self.buffer[2..];
        Ok(())
    }

    pub fn read_bit(&mut self) -> Result<u16, DecodeFailed> {
        if self.remaining == 0 {
            self.advance_buffer()?;
        }

        self.remaining -= 1;
        self.n = self.n.rotate_left(1);
        Ok(self.n & 1)
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        if self.buffer.is_empty() {
            return None;
        }
        let byte = self.buffer[0];
        self.buffer = &self.buffer[1..];
        Some(byte)
    }

    /// Read from the bistream, no more than 16 bits (one word).
    fn read_bits_oneword(&mut self, bits: u8) -> Result<u16, DecodeFailed> {
        assert!(bits <= 16);
        debug_assert!(self.remaining <= 16);

        Ok(if bits <= self.remaining {
            self.remaining -= bits;
            self.n = self.n.rotate_left(bits as u32);
            self.n & ((1 << bits) - 1)
        } else {
            // No need to store `rol` result in `n` as we're about to overwrite it.
            let hi = self.n.rotate_left(self.remaining as u32) & ((1 << self.remaining) - 1);
            let bits = bits - self.remaining;
            self.advance_buffer()?;

            self.remaining -= bits;
            self.n = self.n.rotate_left(bits as u32);
            // `bits` may be 16 which would overflow the left shift, operate on `u32` and trunc.
            let lo = self.n & ((1u32 << bits) as u16).wrapping_sub(1);

            ((hi as u32) << bits) as u16 | lo
        })
    }

    pub fn read_bits(&mut self, bits: u8) -> Result<u32, DecodeFailed> {
        if bits <= 16 {
            self.read_bits_oneword(bits).map(|w| w as u32)
        } else {
            assert!(bits <= 32);

            // Read the two words.
            let w0 = self.read_bits_oneword(16)? as u32;
            let w1 = self.read_bits_oneword(bits - 16)? as u32;

            Ok((w0 << (bits - 16)) | w1)
        }
    }

    /// Peek from the bitstream, no more than 16 bits (one word).
    fn peek_bits_oneword(&self, bits: u8) -> u16 {
        // Copy paste of `read_bits`, but without advancing the buffer.
        assert!(bits <= 16);

        if bits <= self.remaining {
            self.n.rotate_left(bits as u32) & ((1 << bits) - 1)
        } else {
            let hi = self.n.rotate_left(self.remaining as u32) & ((1 << self.remaining) - 1);
            let bits = bits - self.remaining;

            // We may peek more than we need (i.e. at the end of a chunk), due to the way
            // our decoder is implemented. This is a bit ugly but luckily we can pretend
            // there are just zeros after.
            let n = if self.buffer.len() < 2 {
                0
            } else {
                u16::from_le_bytes([self.buffer[0], self.buffer[1]])
            };
            let lo = n.rotate_left(bits as u32) & ((1u32 << bits) as u16).wrapping_sub(1);

            ((hi as u32) << bits) as u16 | lo
        }
    }

    pub fn peek_bits(&self, bits: u8) -> u32 {
        if bits <= 16 {
            self.peek_bits_oneword(bits) as u32
        } else {
            assert!(bits <= 32);

            // Read the two words.
            let mut advanced_stream =
                Self { buffer: self.buffer, n: self.n, remaining: self.remaining };
            let w0 = advanced_stream.read_bits_oneword(16).unwrap() as u32;
            let w1 = advanced_stream.peek_bits_oneword(bits - 16) as u32;

            (w0 << (bits - 16)) | w1
        }
    }

    pub fn read_u32_le(&mut self) -> Result<u32, DecodeFailed> {
        let lo = self.read_bits_oneword(16)?.to_le_bytes();
        let hi = self.read_bits_oneword(16)?.to_le_bytes();
        Ok(u32::from_le_bytes([lo[0], lo[1], hi[0], hi[1]]))
    }

    pub fn read_u24_be(&mut self) -> Result<u32, DecodeFailed> {
        let hi = self.read_bits(16)?;
        let lo = self.read_bits(8)?;
        Ok(hi << 8 | lo)
    }

    pub fn align(&mut self) -> Result<(), DecodeFailed> {
        if self.remaining == 0 {
            self.read_bits(16)?;
        } else {
            self.remaining = 0;
        }
        Ok(())
    }

    /// Copies from the current buffer to the destination output ignoring the representation.
    pub fn read_raw(&mut self, output: &mut [u8]) -> Result<(), DecodeFailed> {
        if self.buffer.len() < output.len() {
            return Err(DecodeFailed::UnexpectedEof);
        }
        output.copy_from_slice(&self.buffer[..output.len()]);
        self.buffer = &self.buffer[output.len()..];
        Ok(())
    }

    pub fn remaining_bytes(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_sequential() {
        // 0..=10 and padding using the least amount of bits possible, read LTR
        let ns = [0b0110_1110_0101_1101_u16, 0b1110_0010_0110_1000_u16];
        let bit_lengths = [1u8, 1, 2, 2, 3, 3, 3, 3, 4, 4, 4];

        // Convert input sequence of 16-bit integers to byte-stream
        let mut bytes = Vec::with_capacity(ns.len() * 2);
        ns.iter().for_each(|n| bytes.extend(&n.to_le_bytes()));

        let mut bitstream = Bitstream::new(&bytes);
        bit_lengths.iter().copied().enumerate().for_each(|(value, bit_length)| {
            assert_eq!(bitstream.read_bits(bit_length), Ok(value as u32));
        });
    }

    #[test]
    fn read_32le() {
        let bytes = [0x56, 0x78, 0x12, 0x34];
        let mut bitstream = Bitstream::new(&bytes);

        assert_eq!(bitstream.read_u32_le(), Ok(873625686));
    }

    #[test]
    fn read_24be() {
        let ns = [0b0000_1100_0001_1000_u16, 0b0001_1000_0011_0000_u16];
        let mut bytes = Vec::with_capacity(ns.len() * 2);
        ns.iter().for_each(|n| bytes.extend(&n.to_le_bytes()));

        let mut bitstream = Bitstream::new(&bytes);

        assert_eq!(bitstream.read_bits(4), Ok(0));
        assert_eq!(bitstream.read_u24_be(), Ok(0b1100_0001_1000_0001_1000_0011));
        assert_eq!(bitstream.read_bits(4), Ok(0));
    }

    #[test]
    fn align() {
        let bytes = [0b0100_0000, 0b0010_0000, 0b1000_0000, 0b0110_0000];
        let mut bitstream = Bitstream::new(&bytes);

        assert_eq!(bitstream.read_bits(3), Ok(1));
        bitstream.align().unwrap();
        assert_eq!(bitstream.read_bits(3), Ok(3));
    }

    #[test]
    fn no_remain_after_aligned() {
        let bytes = [0b0100_0000, 0b0010_0000, 0b1000_0000, 0b0110_0000];
        let mut bitstream = Bitstream::new(&bytes);

        bitstream.read_bits(3).unwrap();
        assert_ne!(bitstream.remaining, 0);

        bitstream.align().unwrap();
        assert_eq!(bitstream.remaining, 0);

        bitstream.read_bits(16).unwrap();
        assert_eq!(bitstream.remaining, 0);
    }

    #[test]
    fn check_read_bit() {
        let bytes = [0b0110_1001, 0b1001_0110];
        let mut bitstream_1 = Bitstream::new(&bytes);
        let mut bitstream_n = Bitstream::new(&bytes);

        (0..16).for_each(|_| {
            assert_eq!(bitstream_1.read_bit().map(|b| b as u32), bitstream_n.read_bits(1))
        });
    }

    #[test]
    fn read_bit_positions_match_description() {
        // bits _abcdefgh_ijklmnop_qrstuvwx_yzABCDEF become:
        let bit_indices: [u32; 32] = [
            8,  // i
            9,  // j
            10, // k
            11, // l
            12, // m
            13, // n
            14, // o
            15, // p
            0,  // a
            1,  // b
            2,  // c
            3,  // d
            4,  // e
            5,  // f
            6,  // g
            7,  // h
            24, // y
            25, // z
            26, // A
            27, // B
            28, // C
            29, // D
            30, // E
            31, // F
            16, // q
            17, // r
            18, // s
            19, // t
            20, // u
            21, // v
            22, // w
            23, // x
        ];
        for (index, bit_index) in bit_indices.iter().copied().enumerate() {
            let n = 1u32.rotate_right(1).rotate_right(bit_index);
            let bytes = n.to_be_bytes();
            eprintln!("index={index}, bit_index={bit_index}, bytes={n:032b}");

            let mut bitstream = Bitstream::new(&bytes);

            if index != 0 {
                assert_eq!(bitstream.read_bits(index as u8), Ok(0));
            }

            assert_eq!(bitstream.read_bit(), Ok(1));

            if let Some(remaining) = 31usize.checked_sub(index) {
                assert_eq!(bitstream.read_bits(remaining as u8), Ok(0));
            }
        }
    }

    #[test]
    fn read_equals_peek() {
        for index in 0..20 {
            let n = (0b11_0111_0111_1101_1111_1101_1111_1111_1101_1111_1111_1111_u64)
                .rotate_left(index);

            let bytes = n.to_be_bytes();
            for offset in 0..20 {
                for size in 0..20 {
                    let mut bitstream = Bitstream::new(&bytes);
                    bitstream.read_bits(offset).unwrap();

                    let peeked = bitstream.peek_bits(size);
                    assert_eq!(
                        bitstream.read_bits(size),
                        Ok(peeked),
                        "offset={offset}, size={size}, bytes={n:032b}",
                    );
                }
            }
        }
    }
}
