//! Synthetic Word 97 documents: no private or application-generated fixtures.
use std::io::{Cursor, Read, Write};

struct Run<'a> {
    text: &'a str,
    chpx: &'a [u8],
    prm: u16,
}

fn run<'a>(text: &'a str, chpx: &'a [u8]) -> Run<'a> {
    Run { text, chpx, prm: 0 }
}

fn u16_at(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn u32_at(bytes: &mut [u8], offset: usize, value: usize) {
    bytes[offset..offset + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

/// One UTF-16 piece and CHPX run per segment. All offsets in FKPs are file
/// positions; the piece-table boundaries count UTF-16 code units instead.
fn document(runs: &[Run<'_>], prcs: &[&[u8]], fonts: &[&str]) -> Vec<u8> {
    let mut word = vec![0; 2048];
    u16_at(&mut word, 0, 0xA5EC);
    u16_at(&mut word, 2, 0xC1);
    u16_at(&mut word, 6, 0x0409);
    u16_at(&mut word, 0x0A, 4); // fComplex
    u16_at(&mut word, 0x20, 14); // csw
    u16_at(&mut word, 0x3E, 22); // cslw
    u16_at(&mut word, 0x98, 93); // cbRgFcLcb
    let n = runs.len();
    let mut plc = vec![0; 4 + n * 12];
    let mut fkp = vec![0; 512];
    let mut blob = ((n + 1) * 4 + n + 1) & !1;
    let mut cp = 0;
    let mut fc = 1024;
    for (i, run) in runs.iter().enumerate() {
        u32_at(&mut plc, i * 4, cp);
        u32_at(&mut plc, (n + 1) * 4 + i * 8 + 2, fc);
        u16_at(&mut plc, (n + 1) * 4 + i * 8 + 6, run.prm);
        u32_at(&mut fkp, i * 4, fc);
        if !run.chpx.is_empty() {
            fkp[(n + 1) * 4 + i] = (blob / 2) as u8;
            fkp[blob] = run.chpx.len() as u8;
            fkp[blob + 1..blob + 1 + run.chpx.len()].copy_from_slice(run.chpx);
            blob = (blob + run.chpx.len() + 2) & !1;
        }
        for unit in run.text.encode_utf16() {
            u16_at(&mut word, fc, unit);
            fc += 2;
            cp += 1;
        }
    }
    assert!(fc <= 1536 && blob < 511);
    u32_at(&mut plc, n * 4, cp);
    u32_at(&mut fkp, n * 4, fc);
    fkp[511] = n as u8;
    word[1536..2048].copy_from_slice(&fkp);
    u32_at(&mut word, 0x18, 1024);
    u32_at(&mut word, 0x1C, fc);
    u32_at(&mut word, 0x4C, cp);
    let mut table = Vec::new();
    for prc in prcs {
        table.push(1);
        table.extend_from_slice(&(prc.len() as u16).to_le_bytes());
        table.extend_from_slice(prc);
    }
    table.push(2);
    table.extend_from_slice(&(plc.len() as u32).to_le_bytes());
    table.extend_from_slice(&plc);
    u32_at(&mut word, 0x1A2, 0);
    u32_at(&mut word, 0x1A6, table.len());
    u32_at(&mut word, 0xFA, table.len());
    u32_at(&mut word, 0xFE, 12);
    for value in [1024u32, fc as u32, 3] {
        table.extend_from_slice(&value.to_le_bytes());
    }
    let font_start = table.len();
    table.extend_from_slice(&(fonts.len() as u16).to_le_bytes());
    table.extend_from_slice(&0u16.to_le_bytes()); // cbExtra
    for name in fonts {
        let mut ffn = vec![0; 40];
        ffn[4] = 2; // SYMBOL_CHARSET
        for unit in name.encode_utf16().chain(std::iter::once(0)) {
            ffn.extend_from_slice(&unit.to_le_bytes());
        }
        ffn[0] = (ffn.len() - 1) as u8;
        table.extend_from_slice(&ffn);
    }
    u32_at(&mut word, 0x112, font_start);
    u32_at(&mut word, 0x116, table.len() - font_start);
    let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    ole.create_stream("WordDocument").unwrap().write_all(&word).unwrap();
    ole.create_stream("0Table").unwrap().write_all(&table).unwrap();
    ole.into_inner().into_inner()
}

fn markdown(runs: &[Run<'_>], prcs: &[&[u8]], fonts: &[&str]) -> String {
    anydoc::to_markdown_bytes(&document(runs, prcs, fonts), anydoc::Format::Doc).unwrap()
}

fn symbol(font: u16, code: u16) -> [u8; 6] {
    let [font_lo, font_hi] = font.to_le_bytes();
    let [code_lo, code_hi] = code.to_le_bytes();
    [0x09, 0x6A, font_lo, font_hi, code_lo, code_hi]
}

#[test]
fn wingdings_checkbox_states_are_preserved() {
    for (font, unchecked, checked) in [("Wingdings 2", 0xA3, 0x52), ("Wingdings", 0x6F, 0xFE)] {
        let empty = symbol(1, unchecked);
        let tick = symbol(1, checked);
        let runs = [run("(", &empty), run(" yes ", &[]), run("(", &tick), run(" no\r", &[])];
        assert_eq!(markdown(&runs, &[], &["Arial", font]).trim(), "□ yes ☑ no", "{font}");
    }
}

#[test]
fn private_use_symbol_codes_are_equivalent() {
    for (font, unchecked, checked) in
        [("Wingdings 2", 0xF0A3, 0xF052), ("Wingdings", 0xF06F, 0xF0FE)]
    {
        let empty = symbol(0, unchecked);
        let tick = symbol(0, checked);
        let runs = [run("(", &empty), run("(", &tick), run("\r", &[])];
        assert_eq!(markdown(&runs, &[], &[font]).trim(), "□☑", "{font}");
    }
}

#[test]
fn unsupported_font_code_and_missing_font_keep_original_text() {
    for (font, font_index, code) in
        [("Arial", 0, 0x52), ("Wingdings 2", 0, 0x01), ("Wingdings 2", 7, 0x52)]
    {
        let props = symbol(font_index, code);
        let runs = [run("(", &props), run("\r", &[])];
        assert_eq!(markdown(&runs, &[], &[font]).trim(), "(");
    }
    let props = symbol(0, 0x52);
    assert_eq!(markdown(&[run("(", &props), run("\r", &[])], &[], &[]).trim(), "(");
}

#[test]
fn ordinary_parentheses_unicode_and_utf16_positions_remain_intact() {
    let tick = symbol(0, 0x52);
    let runs = [run("😀 (plain) □☑ ", &[]), run("(", &tick), run(" end\r", &[])];
    assert_eq!(markdown(&runs, &[], &["Wingdings 2"]).trim(), "😀 (plain) □☑ ☑ end");
    let runs = [run("□☑ text\r", &tick)];
    assert_eq!(markdown(&runs, &[], &["Wingdings 2"]).trim(), "□☑ text");
}

#[test]
fn piece_symbol_overrides_character_formatting() {
    let empty = symbol(0, 0xA3);
    let tick = symbol(0, 0x52);
    let runs = [
        Run { text: "(", chpx: &empty, prm: 1 },
        Run { text: "(", chpx: &[], prm: 3 },
        run("\r", &[]),
    ];
    assert_eq!(markdown(&runs, &[&tick, &empty], &["Wingdings 2"]).trim(), "☑□");
}

fn edit_streams(bytes: Vec<u8>, edit: impl FnOnce(&mut Vec<u8>, &mut Vec<u8>)) -> Vec<u8> {
    let mut ole = cfb::CompoundFile::open(Cursor::new(bytes)).unwrap();
    let mut word = Vec::new();
    let mut table = Vec::new();
    ole.open_stream("WordDocument").unwrap().read_to_end(&mut word).unwrap();
    ole.open_stream("0Table").unwrap().read_to_end(&mut table).unwrap();
    edit(&mut word, &mut table);
    ole.create_stream("WordDocument").unwrap().write_all(&word).unwrap();
    ole.create_stream("0Table").unwrap().write_all(&table).unwrap();
    ole.into_inner().into_inner()
}

#[test]
fn malformed_font_tables_preserve_the_placeholder() {
    let tick = symbol(0, 0x52);
    let runs = [run("(", &tick), run("\r", &[])];
    for case in 0..5 {
        let bytes = edit_streams(document(&runs, &[], &["Wingdings 2"]), |word, table| {
            let offset = u32::from_le_bytes(word[0x112..0x116].try_into().unwrap()) as usize;
            match case {
                0 => u32_at(word, 0x112, usize::MAX),
                1 => u32_at(word, 0x116, 3),  // Incomplete STTB header.
                2 => table[offset + 4] = 255, // FFN extends beyond the table.
                3 => u16_at(table, offset + 2, 1), // Unsupported cbExtra.
                4 => u16_at(table, offset, 0xffff), // Extended STTB is invalid here.
                _ => unreachable!(),
            }
        });
        assert_eq!(
            anydoc::to_markdown_bytes(&bytes, anydoc::Format::Doc).unwrap().trim(),
            "(",
            "case {case}"
        );
    }
}

#[test]
fn short_font_entries_do_not_shift_later_font_indexes() {
    let tick = symbol(1, 0x52);
    let bytes = edit_streams(
        document(&[run("(", &tick), run("\r", &[])], &[], &["Wingdings 2"]),
        |word, table| {
            let offset = u32::from_le_bytes(word[0x112..0x116].try_into().unwrap()) as usize;
            u16_at(table, offset, 2);
            table.insert(offset + 4, 0); // Empty malformed FFN occupies index zero.
            u32_at(word, 0x116, table.len() - offset);
        },
    );
    assert_eq!(anydoc::to_markdown_bytes(&bytes, anydoc::Format::Doc).unwrap().trim(), "☑");
}

#[test]
fn unknown_piece_symbol_overrides_known_character_symbol() {
    let tick = symbol(0, 0x52);
    let unknown = symbol(0, 0x01);
    let runs = [Run { text: "(", chpx: &tick, prm: 1 }, run("\r", &[])];
    assert_eq!(markdown(&runs, &[&unknown], &["Wingdings 2"]).trim(), "(");
}
