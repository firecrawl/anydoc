use std::fmt;
use std::num::NonZeroU8;
use std::ops::Range;

use super::{Bitstream, DecodeFailed};

/// The canonical tree cannot be used to decode elements. Instead, it behaves like a builder for
/// instances of the actual tree that can decode elements efficiently.
#[derive(Debug)]
pub struct CanonicalTree {
    // > Each tree element can have a path length of [0, 16], where a zero path length indicates
    // > that the element has a zero frequency and is not present in the tree.
    //
    // We represent them as `u8` due to their very short range.
    path_lengths: Vec<u8>,
}

pub struct Tree {
    path_lengths: Vec<u8>,
    largest_length: NonZeroU8,
    huffman_tree: Vec<u16>,
}

impl CanonicalTree {
    pub fn new(count: usize) -> Self {
        Self {
            // > In the case of the very first such tree, the delta is calculated against a tree
            // > in which all elements have a zero path length.
            path_lengths: vec![0; count],
        }
    }

    /// Create a new `Tree` instance from this cast that can be used to decode elements. If the
    /// resulting tree is empty (all path lengths are 0), then `Ok(None)` is returned.
    ///
    /// This method transforms the canonical Huffman tree into a different structure that can
    /// be used to better decode elements.
    // > an LZXD decoder uses only the path lengths of the Huffman tree to reconstruct the
    // > identical tree,
    pub fn create_instance_allow_empty(&self) -> Result<Option<Tree>, DecodeFailed> {
        // The ideas implemented by this method are heavily inspired from LeonBlade's xnbcli
        // on GitHub.
        //
        // The path lengths contains the bit indices or zero if its not present, so find the
        // highest path length to determine how big our tree needs to be.
        let largest_length =
            match NonZeroU8::new(*self.path_lengths.iter().max().expect("empty path lengths")) {
                Some(x) => x,
                // N.B: If all the path lengths are zero, then the tree is empty (which is allowed).
                None => return Ok(None),
            };
        let mut huffman_tree = vec![0; 1 << largest_length.get()];

        // > a zero path length indicates that the element has a zero frequency and is not
        // > present in the tree. Tree elements are output in sequential order starting with the
        // > first element
        //
        // We start at the MSB, 1, and write the tree elements in sequential order from index 0.
        let mut pos = 0;
        for bit in 1..=largest_length.get() {
            let amount = 1 << (largest_length.get() - bit);

            // The codes correspond with the indices of the path length (because
            // `path_lengths[code]` is its path length).
            for code in 0..self.path_lengths.len() {
                // As soon as a code's path length matches with our bit index write the code as
                // many times as the bit index itself represents.
                if self.path_lengths[code] == bit {
                    huffman_tree
                        .get_mut(pos..pos + amount)
                        .ok_or(DecodeFailed::InvalidPathLengths)?
                        .iter_mut()
                        .for_each(|x| *x = code as u16);

                    pos += amount;
                }
            }
        }

        // If we didn't fill the entire table, the path lengths were wrong.
        if pos != huffman_tree.len() {
            Err(DecodeFailed::InvalidPathLengths)?;
        }

        Ok(Some(Tree { path_lengths: self.path_lengths.clone(), largest_length, huffman_tree }))
    }

    /// Create a new `Tree` instance from this cast that can be used to decode elements.
    ///
    /// This method transforms the canonical Huffman tree into a different structure that can
    /// be used to better decode elements.
    // > an LZXD decoder uses only the path lengths of the Huffman tree to reconstruct the
    // > identical tree,
    pub fn create_instance(&self) -> Result<Tree, DecodeFailed> {
        self.create_instance_allow_empty()?.ok_or(DecodeFailed::EmptyTree)
    }

    // Note: the tree already exists and is used to apply the deltas.
    pub fn update_range_with_pretree(
        &mut self,
        bitstream: &mut Bitstream,
        range: Range<usize>,
    ) -> Result<(), DecodeFailed> {
        // > Each of the 17 possible values of (len[x] - prev_len[x]) mod 17, plus three
        // > additional codes used for run-length encoding, are not output directly as 5-bit
        // > numbers but are instead encoded via a Huffman tree called the pretree. The pretree
        // > is generated dynamically according to the frequencies of the 20 allowable tree
        // > codes. The structure of the pretree is encoded in a total of 80 bits by using 4 bits
        // > to output the path length of each of the 20 pretree elements. Once again, a zero
        // > path length indicates a zero-frequency element.
        let pretree = {
            let mut path_lengths = Vec::with_capacity(20);
            for _ in 0..20 {
                path_lengths.push(bitstream.read_bits(4)? as u8)
            }

            Tree::from_path_lengths(path_lengths)?
        };

        // > Tree elements are output in sequential order starting with the first element.
        let mut i = range.start;
        while i < range.end {
            // > The "real" tree is then encoded using the pretree Huffman codes.
            let code = pretree.decode_element(bitstream)?;

            // > Elements can be encoded in one of two ways: if several consecutive elements have
            // > the same path length, run-length encoding is employed; otherwise, the element is
            // > output by encoding the difference between the current path length and the
            // > previous path length of the tree, mod 17.
            match code {
                0..=16 => {
                    self.path_lengths[i] = (17 + self.path_lengths[i] - code as u8) % 17;
                    i += 1;
                }
                // > Codes 17, 18, and 19 are used to represent consecutive elements that have the
                // > same path length.
                17 => {
                    let zeros = bitstream.read_bits(4)?;
                    self.path_lengths
                        .get_mut(i..i + zeros as usize + 4)
                        .ok_or(DecodeFailed::InvalidPretreeRle)?
                        .iter_mut()
                        .for_each(|x| *x = 0);
                    i += zeros as usize + 4;
                }
                18 => {
                    let zeros = bitstream.read_bits(5)?;
                    self.path_lengths
                        .get_mut(i..i + zeros as usize + 20)
                        .ok_or(DecodeFailed::InvalidPretreeRle)?
                        .iter_mut()
                        .for_each(|x| *x = 0);
                    i += zeros as usize + 20;
                }
                19 => {
                    let same = bitstream.read_bits(1)?;
                    // "Decode new code" is used to parse the next code from the bitstream, which
                    // has a value range of [0, 16].
                    let code = pretree.decode_element(bitstream)?;
                    if code > 16 {
                        return Err(DecodeFailed::InvalidPretreeElement(code))?;
                    }

                    let value = (17 + self.path_lengths[i] - code as u8) % 17;
                    self.path_lengths
                        .get_mut(i..i + same as usize + 4)
                        .ok_or(DecodeFailed::InvalidPretreeRle)?
                        .iter_mut()
                        .for_each(|x| *x = value);
                    i += same as usize + 4;
                }
                _ => return Err(DecodeFailed::InvalidPretreeElement(code)),
            };
        }

        Ok(())
    }
}

impl Tree {
    /// Create a new usable tree instance directly from known path lengths.
    pub fn from_path_lengths(path_lengths: Vec<u8>) -> Result<Self, DecodeFailed> {
        CanonicalTree { path_lengths }.create_instance()
    }

    pub fn decode_element(&self, bitstream: &mut Bitstream) -> Result<u16, DecodeFailed> {
        // Perform the inverse translation, peeking as many bits as our tree is…
        let code = self.huffman_tree[bitstream.peek_bits(self.largest_length.get()) as usize];

        // …and advancing the stream for as many bits this code actually takes (read to seek).
        bitstream.read_bits(self.path_lengths[code as usize])?;

        Ok(code)
    }
}

impl fmt::Debug for Tree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tree")
            .field("path_lengths", &self.path_lengths.len())
            .field("largest_length", &self.largest_length)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_simple_table() {
        // Based on some aligned offset tree
        let tree = Tree::from_path_lengths(vec![6, 5, 1, 3, 4, 6, 2, 0]).unwrap();
        let value_count = vec![(2, 32), (6, 16), (3, 8), (4, 4), (1, 2), (0, 1), (5, 1)];

        let mut i = 0;
        for (value, count) in value_count.into_iter() {
            (0..count).for_each(|_| {
                assert_eq!(tree.huffman_tree[i], value);
                i += 1;
            })
        }
    }

    #[test]
    fn decode_complex_table() {
        // Based on the pretree of some length tree
        let tree = Tree::from_path_lengths(vec![
            1, 0, 0, 0, 0, 7, 3, 3, 4, 4, 5, 5, 5, 7, 8, 8, 0, 7, 0, 0,
        ])
        .unwrap();
        let value_count = vec![
            (0, 128),
            (6, 32),
            (7, 32),
            (8, 16),
            (9, 16),
            (10, 8),
            (11, 8),
            (12, 8),
            (5, 2),
            (13, 2),
            (17, 2),
            (14, 1),
            (15, 1),
        ];

        let mut i = 0;
        for (value, count) in value_count.into_iter() {
            (0..count).for_each(|_| {
                assert_eq!(tree.huffman_tree[i], value);
                i += 1;
            })
        }
    }

    #[test]
    fn decode_elements() {
        let tree = Tree::from_path_lengths(vec![6, 5, 1, 3, 4, 6, 2, 0]).unwrap();

        let buffer = [0x5b, 0xda, 0x3f, 0xf8];
        let mut bitstream = Bitstream::new(&buffer);
        bitstream.read_bits(11).unwrap();
        assert_eq!(tree.decode_element(&mut bitstream), Ok(3));
        assert_eq!(tree.decode_element(&mut bitstream), Ok(5));
        assert_eq!(tree.decode_element(&mut bitstream), Ok(6));
        assert_eq!(tree.decode_element(&mut bitstream), Ok(2));
    }
}
