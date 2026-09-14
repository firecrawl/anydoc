//! Synthetic Word 97 documents: no private or application-generated fixtures.
use std::io::{Cursor, Read, Write};

const DELETE: [u8; 3] = [0x00, 0x08, 1];
const INVERT_DELETE: [u8; 3] = [0x00, 0x08, 0x81];

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
fn document(runs: &[Run<'_>], prcs: &[&[u8]]) -> Vec<u8> {
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
    let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    ole.create_stream("WordDocument").unwrap().write_all(&word).unwrap();
    ole.create_stream("0Table").unwrap().write_all(&table).unwrap();
    ole.into_inner().into_inner()
}

fn markdown(runs: &[Run<'_>], prcs: &[&[u8]]) -> String {
    anydoc::to_markdown_bytes(&document(runs, prcs), anydoc::Format::Doc).unwrap()
}

fn with_streams(bytes: Vec<u8>, edit: impl FnOnce(&mut Vec<u8>, &mut Vec<u8>)) -> Vec<u8> {
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

/// PAPX entries use the same segment boundaries as the character runs.
fn table_document(runs: &[Run<'_>], properties: &[&[u8]]) -> Vec<u8> {
    with_streams(document(runs, &[]), |word, table| {
        let n = runs.len();
        let mut page = vec![0; 512];
        let mut blob = ((n + 1) * 4 + n * 13 + 1) & !1;
        let mut fc = 1024;
        for (i, (run, props)) in runs.iter().zip(properties).enumerate() {
            u32_at(&mut page, i * 4, fc);
            fc += run.text.encode_utf16().count() * 2;
            page[(n + 1) * 4 + i * 13] = (blob / 2) as u8;
            // cb == 0 form, padded UPX containing istd followed by grpprl.
            let len = (2 + props.len() + 1) & !1;
            page[blob + 1] = (len / 2) as u8;
            page[blob + 4..blob + 4 + props.len()].copy_from_slice(props);
            blob += 2 + len;
        }
        assert!(blob < 511);
        u32_at(&mut page, n * 4, fc);
        page[511] = n as u8;
        u32_at(word, 0x102, table.len());
        u32_at(word, 0x106, 12);
        for value in [1024u32, fc as u32, (word.len() / 512) as u32] {
            table.extend_from_slice(&value.to_le_bytes());
        }
        word.extend_from_slice(&page);
    })
}

#[test]
fn deleted_text_is_omitted_but_insertions_and_strikethrough_remain() {
    let output = markdown(
        &[
            run("old", &DELETE),
            run("also old", &INVERT_DELETE),
            run("new", &[1, 8, 1]),
            run("discard", &[1, 8, 1, 0, 8, 1]),
            run(" ", &[]),
            run("strike", &[0x37, 8, 1]),
            run("\r", &[]),
        ],
        &[],
    );
    assert_eq!(output.trim(), "new ~~strike~~");
}

#[test]
fn deletion_toggles_use_the_default_revision_state() {
    let runs = [
        run("off", &[0, 8, 0]),
        run("same", &[0, 8, 0x80]),
        run("inverse", &INVERT_DELETE),
        run("on", &DELETE),
        run("default", &[]),
        run("\r", &[0, 8, 0]),
    ];
    assert_eq!(markdown(&runs, &[]).trim(), "offsamedefault");
}

#[test]
fn piece_modifiers_override_character_runs() {
    let runs = [
        Run { text: "old", chpx: &[], prm: (1 << 8) | (0x41 << 1) },
        Run { text: "keep", chpx: &DELETE, prm: 1 },
        Run { text: "discard", chpx: &[], prm: 3 },
        run("\r", &[]),
    ];
    assert_eq!(markdown(&runs, &[&[0, 8, 0], &DELETE]).trim(), "keep");
}

#[test]
fn piece_toggles_do_not_invert_direct_formatting() {
    let runs = [
        Run { text: "keep", chpx: &DELETE, prm: (0x80 << 8) | (0x41 << 1) },
        Run { text: "old", chpx: &DELETE, prm: 1 },
        run("\r", &[]),
    ];
    assert_eq!(markdown(&runs, &[&INVERT_DELETE]).trim(), "keep");
}

#[test]
fn unicode_positions_and_deleted_paragraph_marks() {
    let runs = [run("当前😀", &[]), run("旧内容𠀀\r", &DELETE), run("正文𠀀\r", &[])];
    assert_eq!(markdown(&runs, &[]).trim(), "当前😀正文𠀀");
}

#[test]
fn deleted_field_delimiters_do_not_leave_a_field_open() {
    let runs = [
        run("before ", &[]),
        run("\u{13} DATE \u{14}obsolete\u{15}", &DELETE),
        run("after\r", &[]),
    ];
    assert_eq!(markdown(&runs, &[]).trim(), "before after");
    let runs = [run("\u{13} DATE \u{14}", &[]), run("old\u{15}", &DELETE), run("after\r", &[])];
    assert_eq!(markdown(&runs, &[]).trim(), "after");
    let runs = [
        run("\u{13} IF 1 = 1 \u{14}kept ", &[]),
        run("\u{13} DATE \u{14}old\u{15}", &DELETE),
        run("result\u{15} after\r", &[]),
    ];
    assert_eq!(markdown(&runs, &[]).trim(), "kept result after");
}

#[test]
fn deleted_cell_content_does_not_merge_neighboring_cells() {
    let runs = [run("left\u{7}", &[]), run("old\u{7}", &DELETE), run("right\u{7}", &[])];
    let output = markdown(&runs, &[]);
    assert!(!output.contains("old"));
    assert!(output.contains("| left |  | right |"), "{output}");
}

#[test]
fn deleted_cells_preserve_columns_across_rows() {
    let runs = [
        run("A\u{7}", &[]),
        run("B\u{7}", &[]),
        run("\u{7}", &[]),
        run("old\u{7}", &DELETE),
        run("C\u{7}", &[]),
        run("\u{7}", &DELETE),
    ];
    let cell: &[u8] = &[0x16, 0x24, 1];
    let row: &[u8] = &[0x16, 0x24, 1, 0x17, 0x24, 1];
    let bytes = table_document(&runs, &[cell, cell, row, cell, cell, row]);
    let output = anydoc::to_markdown_bytes(&bytes, anydoc::Format::Doc).unwrap();
    assert!(output.contains("| A | B |"), "{output}");
    assert!(output.contains("|  | C |"), "{output}");
    assert!(!output.contains("old"));
}

#[test]
fn deleted_inner_cell_marks_still_separate_paragraphs() {
    let runs = [run("first", &[]), run("\r", &DELETE), run("second\u{7}", &[])];
    let cell: &[u8] = &[0x16, 0x24, 1];
    let inner: &[u8] = &[0x16, 0x24, 1, 0x4B, 0x24, 1];
    let bytes = table_document(&runs, &[cell, inner, cell]);
    let output = anydoc::to_markdown_bytes(&bytes, anydoc::Format::Doc).unwrap();
    assert!(output.contains("first<br>second"), "{output}");
}

#[test]
fn deleted_note_reference_also_omits_its_body() {
    for deleted in [false, true] {
        let runs = [
            run("text", &[]),
            run("\u{2}", if deleted { &DELETE } else { &[] }),
            run("\r", &[]),
            run("note body\r", &[]),
        ];
        let bytes = with_streams(document(&runs, &[]), |word, table| {
            u32_at(word, 0x4C, 6);
            u32_at(word, 0x50, 10);
            u32_at(word, 0xAA, table.len());
            u32_at(word, 0xAE, 10);
            table.extend_from_slice(&4u32.to_le_bytes());
            table.extend_from_slice(&6u32.to_le_bytes());
            table.extend_from_slice(&1u16.to_le_bytes());
            u32_at(word, 0xB2, table.len());
            u32_at(word, 0xB6, 8);
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&10u32.to_le_bytes());
        });
        let doc = anydoc::to_document(&bytes, anydoc::Format::Doc).unwrap();
        assert_eq!(doc.notes.len(), usize::from(!deleted));
        let output = anydoc::to_markdown_bytes(&bytes, anydoc::Format::Doc).unwrap();
        assert_eq!(output.contains("note body"), !deleted, "{output}");
        assert_eq!(output.contains("[^"), !deleted, "{output}");
    }
}

#[test]
fn partial_and_nested_field_deletions_preserve_outer_hyperlinks() {
    let runs = [
        run("\u{13} HYPERLINK \"https://example.com\" \u{14}current ", &[]),
        run("old ", &DELETE),
        run("\u{13} DATE \u{14}obsolete\u{15}", &DELETE),
        run("label\u{15}\r", &[]),
    ];
    assert_eq!(markdown(&runs, &[]).trim(), "[current label](https://example.com)");
}
