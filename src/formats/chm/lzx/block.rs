use super::{Bitstream, DecodeFailed, DecoderState, Tree};

// if position_slot < 4 {
//     0
// } else if position_slot >= 36 {
//     17
// } else {
//     (position_slot - 2) / 2
// }
const FOOTER_BITS: [u8; 289] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13, 14, 14, 15, 15, 16, 16, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17,
];

// if position_slot == 0 {
//     0
// } else {
//     BASE_POSITION[position_slot - 1] + (1 << FOOTER_BITS[position_slot - 1])
// }
const BASE_POSITION: [u32; 290] = [
    0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536,
    2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 49152, 65536, 98304, 131072, 196608,
    262144, 393216, 524288, 655360, 786432, 917504, 1048576, 1179648, 1310720, 1441792, 1572864,
    1703936, 1835008, 1966080, 2097152, 2228224, 2359296, 2490368, 2621440, 2752512, 2883584,
    3014656, 3145728, 3276800, 3407872, 3538944, 3670016, 3801088, 3932160, 4063232, 4194304,
    4325376, 4456448, 4587520, 4718592, 4849664, 4980736, 5111808, 5242880, 5373952, 5505024,
    5636096, 5767168, 5898240, 6029312, 6160384, 6291456, 6422528, 6553600, 6684672, 6815744,
    6946816, 7077888, 7208960, 7340032, 7471104, 7602176, 7733248, 7864320, 7995392, 8126464,
    8257536, 8388608, 8519680, 8650752, 8781824, 8912896, 9043968, 9175040, 9306112, 9437184,
    9568256, 9699328, 9830400, 9961472, 10092544, 10223616, 10354688, 10485760, 10616832, 10747904,
    10878976, 11010048, 11141120, 11272192, 11403264, 11534336, 11665408, 11796480, 11927552,
    12058624, 12189696, 12320768, 12451840, 12582912, 12713984, 12845056, 12976128, 13107200,
    13238272, 13369344, 13500416, 13631488, 13762560, 13893632, 14024704, 14155776, 14286848,
    14417920, 14548992, 14680064, 14811136, 14942208, 15073280, 15204352, 15335424, 15466496,
    15597568, 15728640, 15859712, 15990784, 16121856, 16252928, 16384000, 16515072, 16646144,
    16777216, 16908288, 17039360, 17170432, 17301504, 17432576, 17563648, 17694720, 17825792,
    17956864, 18087936, 18219008, 18350080, 18481152, 18612224, 18743296, 18874368, 19005440,
    19136512, 19267584, 19398656, 19529728, 19660800, 19791872, 19922944, 20054016, 20185088,
    20316160, 20447232, 20578304, 20709376, 20840448, 20971520, 21102592, 21233664, 21364736,
    21495808, 21626880, 21757952, 21889024, 22020096, 22151168, 22282240, 22413312, 22544384,
    22675456, 22806528, 22937600, 23068672, 23199744, 23330816, 23461888, 23592960, 23724032,
    23855104, 23986176, 24117248, 24248320, 24379392, 24510464, 24641536, 24772608, 24903680,
    25034752, 25165824, 25296896, 25427968, 25559040, 25690112, 25821184, 25952256, 26083328,
    26214400, 26345472, 26476544, 26607616, 26738688, 26869760, 27000832, 27131904, 27262976,
    27394048, 27525120, 27656192, 27787264, 27918336, 28049408, 28180480, 28311552, 28442624,
    28573696, 28704768, 28835840, 28966912, 29097984, 29229056, 29360128, 29491200, 29622272,
    29753344, 29884416, 30015488, 30146560, 30277632, 30408704, 30539776, 30670848, 30801920,
    30932992, 31064064, 31195136, 31326208, 31457280, 31588352, 31719424, 31850496, 31981568,
    32112640, 32243712, 32374784, 32505856, 32636928, 32768000, 32899072, 33030144, 33161216,
    33292288, 33423360,
];

struct DecodeInfo<'a> {
    aligned_offset_tree: Option<&'a Tree>,
    main_tree: &'a Tree,
    length_tree: Option<&'a Tree>,
}

#[derive(Debug)]
pub enum Decoded {
    Single(u8),
    Match { offset: usize, length: usize },
    Read(usize),
}

#[derive(Debug)]
pub enum Kind {
    Verbatim { main_tree: Tree, length_tree: Option<Tree> },
    AlignedOffset { aligned_offset_tree: Tree, main_tree: Tree, length_tree: Option<Tree> },
    Uncompressed { r: [u32; 3] },
}

/// Note that this is not the block header, but the head of the block's body, which includes
/// everything except the tail of the block data (either uncompressed data or token sequence).
pub struct Block {
    /// Only 24 bits may be used.
    pub remaining: u32,
    pub size: u32,
    pub kind: Kind,
}

/// Read the pretrees for the main and length tree, and with those also read the trees
/// themselves, using the path lengths from a previous tree if any.
///
/// This is used when reading a verbatim or aligned block.
fn read_main_and_length_trees(
    bitstream: &mut Bitstream,
    state: &mut DecoderState,
) -> Result<(), DecodeFailed> {
    // Verbatim block
    // Entry                                             Comments
    // Pretree for first 256 elements of main tree       20 elements, 4 bits each
    // Path lengths of first 256 elements of main tree   Encoded using pretree
    // Pretree for remainder of main tree                20 elements, 4 bits each
    // Path lengths of remaining elements of main tree   Encoded using pretree
    // Pretree for length tree                           20 elements, 4 bits each
    // Path lengths of elements in length tree           Encoded using pretree
    // Token sequence (matches and literals)             Specified in section 2.6

    state.main_tree.update_range_with_pretree(bitstream, 0..256)?;

    state
        .main_tree
        .update_range_with_pretree(bitstream, 256..256 + 8 * state.window_size.position_slots())?;

    state.length_tree.update_range_with_pretree(bitstream, 0..249)?;

    Ok(())
}

fn decode_element(
    bitstream: &mut Bitstream,
    r: &mut [u32; 3],
    DecodeInfo { aligned_offset_tree, main_tree, length_tree }: DecodeInfo,
) -> Result<Decoded, DecodeFailed> {
    // Decoding Matches and Literals (Aligned and Verbatim Blocks)
    let main_element = main_tree.decode_element(bitstream)?;

    // Check if it is a literal character.
    Ok(if main_element < 256 {
        // It is a literal, so copy the literal to output.
        Decoded::Single(main_element as u8)
    } else {
        // Decode the match. For a match, there are two components, offset and length.
        let length_header = (main_element - 256) & 7;

        let match_length = if length_header == 7 {
            // Length of the footer.
            length_tree.ok_or(DecodeFailed::EmptyTree)?.decode_element(bitstream)? + 7 + 2
        } else {
            length_header + 2 // no length footer
            // Decoding a match length (if a match length < 257).
        };
        assert_ne!(match_length, 0);

        let position_slot = (main_element - 256) >> 3;

        // Check for repeated offsets (positions 0, 1, 2).
        let match_offset;
        if position_slot == 0 {
            match_offset = r[0];
        } else if position_slot == 1 {
            match_offset = r[1];
            r.swap(0, 1);
        } else if position_slot == 2 {
            match_offset = r[2];
            r.swap(0, 2);
        } else {
            // Not a repeated offset.
            let offset_bits = FOOTER_BITS[position_slot as usize];

            let formatted_offset = if let Some(aligned_offset_tree) = aligned_offset_tree.as_ref() {
                let verbatim_bits;
                let aligned_bits;

                // This means there are some aligned bits.
                if offset_bits >= 3 {
                    verbatim_bits = bitstream.read_bits(offset_bits - 3)? << 3;
                    aligned_bits = aligned_offset_tree.decode_element(bitstream)?;
                } else {
                    // 0, 1, or 2 verbatim bits
                    verbatim_bits = bitstream.read_bits(offset_bits)?;
                    aligned_bits = 0;
                }

                BASE_POSITION[position_slot as usize] + verbatim_bits + aligned_bits as u32
            } else {
                // Block_type is a verbatim_block.
                let verbatim_bits = bitstream.read_bits(offset_bits)?;
                BASE_POSITION[position_slot as usize] + verbatim_bits
            };

            // Decoding a match offset.
            match_offset = formatted_offset - 2;

            // Update repeated offset least recently used queue.
            r[2] = r[1];
            r[1] = r[0];
            r[0] = match_offset;
        }

        // Check for extra length.
        // > If the match length is 257 or larger, the encoded match length token
        // > (or match length, as specified in section 2.6) value is 257, and an
        // > encoded Extra Length field follows the other match encoding components,
        // > as specified in section 2.6.7, in the bitstream.

        // TODO for some reason, if we do this, parsing .xnb files with window size
        //      64KB, it breaks and stops decompressing correctly, but no idea why.
        /*
        let match_length = if match_length == 257 {
            // Decode the extra length.
            let extra_len = if bitstream.read_bit() != 0 {
                if bitstream.read_bit() != 0 {
                    if bitstream.read_bit() != 0 {
                        // > Prefix 0b111; Number of bits to decode 15;
                        bitstream.read_bits(15)
                    } else {
                        // > Prefix 0b110; Number of bits to decode 12;
                        bitstream.read_bits(12) + 1024 + 256
                    }
                } else {
                    // > Prefix 0b10; Number of bits to decode 10;
                    bitstream.read_bits(10) + 256
                }
            } else {
                // > Prefix 0b0; Number of bits to decode 8;
                bitstream.read_bits(8)
            };

            // Get the match length (if match length >= 257).
            // In all cases,
            // > Base value to add to decoded value 257 + …
            257 + extra_len
        } else {
            match_length as u16
        };
        */

        // Get match length and offset. Perform copy and paste work.
        Decoded::Match { offset: match_offset as usize, length: match_length as usize }
    })
}

impl Block {
    pub(crate) fn read(
        bitstream: &mut Bitstream,
        state: &mut DecoderState,
    ) -> Result<Self, DecodeFailed> {
        // > Each block of compressed data begins with a 3-bit Block Type field.
        // > Of the eight possible values, only three are valid values for the Block Type
        // > field.
        let kind = bitstream.read_bits(3)? as u8;
        let size = bitstream.read_u24_be()?;
        if size == 0 {
            return Err(DecodeFailed::InvalidBlockSize(size));
        }

        let kind = match kind {
            0b001 => {
                read_main_and_length_trees(bitstream, state)?;

                Kind::Verbatim {
                    main_tree: state.main_tree.create_instance()?,
                    length_tree: state.length_tree.create_instance_allow_empty()?,
                }
            }
            0b010 => {
                // > encoding only the delta path lengths between the current and previous trees
                //
                // This means we don't need to worry about deltas on this tree.
                let aligned_offset_tree = {
                    let mut path_lengths = Vec::with_capacity(8);
                    for _ in 0..8 {
                        path_lengths.push(bitstream.read_bits(3)? as u8);
                    }

                    Tree::from_path_lengths(path_lengths)?
                };

                // > An aligned offset block is identical to the verbatim block except for the
                // > presence of the aligned offset tree preceding the other trees.
                read_main_and_length_trees(bitstream, state)?;

                Kind::AlignedOffset {
                    aligned_offset_tree,
                    main_tree: state.main_tree.create_instance()?,
                    length_tree: state.length_tree.create_instance_allow_empty()?,
                }
            }
            0b011 => {
                bitstream.align()?;
                Kind::Uncompressed {
                    r: [
                        bitstream.read_u32_le()?,
                        bitstream.read_u32_le()?,
                        bitstream.read_u32_le()?,
                    ],
                }
            }
            _ => return Err(DecodeFailed::InvalidBlock(kind)),
        };

        Ok(Block { remaining: size, size, kind })
    }

    pub(crate) fn decode_element(
        &self,
        bitstream: &mut Bitstream,
        r: &mut [u32; 3],
    ) -> Result<Decoded, DecodeFailed> {
        match &self.kind {
            Kind::Verbatim { main_tree, length_tree } => decode_element(
                bitstream,
                r,
                DecodeInfo {
                    aligned_offset_tree: None,
                    main_tree,
                    length_tree: length_tree.as_ref(),
                },
            ),
            Kind::AlignedOffset { aligned_offset_tree, main_tree, length_tree } => decode_element(
                bitstream,
                r,
                DecodeInfo {
                    aligned_offset_tree: Some(aligned_offset_tree),
                    main_tree,
                    length_tree: length_tree.as_ref(),
                },
            ),
            Kind::Uncompressed { r: new_r } => {
                r.copy_from_slice(new_r);
                Ok(Decoded::Read(self.remaining as usize))
            }
        }
    }
}
