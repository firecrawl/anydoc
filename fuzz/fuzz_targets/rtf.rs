#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Conversion may fail with a typed error; it must never panic, hang,
    // or exhaust memory.
    let _ = anydoc::to_markdown_bytes(data, anydoc::Format::Rtf);
});
