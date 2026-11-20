#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Conversion may fail with a typed error; it must never panic, hang,
    // or exhaust memory. Nested multipart, transfer-decoding and
    // encoded-words all run over attacker-shaped bytes here.
    let _ = anydoc::to_markdown_bytes(data, anydoc::Format::Eml);
});
