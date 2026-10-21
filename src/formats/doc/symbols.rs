//! Legacy DOC symbol placeholders: resolve sprmCSymbol through SttbfFfn.
//! Only known checkbox glyphs are mapped; unknown symbols keep their source text.
use super::sprm::walk_sprms;
use crate::shared::binary::{get_u16, get_u32};

#[derive(Default)]
pub(super) struct Fonts(Vec<String>);

impl Fonts {
    pub(super) fn parse(word: &[u8], table: &[u8]) -> Self {
        let Some(offset) = get_u32(word, 0x112).map(|v| v as usize) else {
            return Self::default();
        };
        let length = get_u32(word, 0x116).unwrap_or(0) as usize;
        let Some(data) = table.get(offset..offset.saturating_add(length)) else {
            return Self::default();
        };
        // MS-DOC SttbfFfn is a non-extended STTB, with no extra data.
        let count = get_u16(data, 0).unwrap_or(0) as usize;
        if count > 0x7ff0 || get_u16(data, 2) != Some(0) {
            return Self::default();
        }
        let mut fonts = Vec::new();
        let mut pos = 4;
        for _ in 0..count {
            let Some(&length) = data.get(pos) else { break };
            let end = pos + 1 + length as usize;
            let Some(ffn) = data.get(pos..end) else { break };
            // Length prefix + 39 fixed bytes, followed by a UTF-16 name.
            let name = ffn
                .get(40..)
                .and_then(|bytes| {
                    let units: Vec<u16> = bytes
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]]))
                        .take_while(|&u| u != 0)
                        .collect();
                    String::from_utf16(&units).ok()
                })
                .unwrap_or_default();
            fonts.push(name);
            pos = end;
        }
        Self(fonts)
    }

    pub(super) fn checkbox(&self, symbol: Symbol) -> Option<char> {
        let font = self.0.get(symbol.font as usize)?;
        let code = match symbol.code {
            0xf000..=0xf0ff => symbol.code - 0xf000,
            code => code,
        };
        if font.eq_ignore_ascii_case("Wingdings 2") {
            match code {
                0xa3 => Some('□'),
                0x52 => Some('☑'),
                _ => None,
            }
        } else if font.eq_ignore_ascii_case("Wingdings") {
            match code {
                0x6f => Some('□'),
                0xfe => Some('☑'),
                _ => None,
            }
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Symbol {
    font: u16,
    code: u16,
}

/// Later property runs override earlier runs even when their glyph is unknown.
pub(super) fn apply(grpprl: &[u8], symbol: &mut Option<Symbol>) {
    walk_sprms(grpprl, |sprm, operand| {
        if sprm == 0x6a09 {
            *symbol = get_u16(operand, 0)
                .zip(get_u16(operand, 2))
                .map(|(font, code)| Symbol { font, code });
        }
    });
}
