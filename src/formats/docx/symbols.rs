//! Font-specific checkbox glyphs stored as WordprocessingML w:sym elements.
use crate::package::xml::{Element, ns};

pub(super) fn checkbox(symbol: &Element) -> Option<char> {
    let font = symbol.attr(ns::W, "font")?;
    let value = symbol.attr(ns::W, "char")?;
    // Reject signs and overlong values even when integer parsing accepts them.
    if value.is_empty() || value.len() > 4 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let code = u16::from_str_radix(value, 16).ok()?;
    // Word stores legacy symbol-font codes both as bytes and in U+F000..F0FF.
    let code = match code {
        0xf000..=0xf0ff => code - 0xf000,
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
        // Unknown font/code pairs are not Unicode checkbox states. Keep the
        // existing unsupported-symbol behavior instead of inventing a value.
        None
    }
}
