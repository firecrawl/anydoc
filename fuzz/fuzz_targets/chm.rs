#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = anydoc::to_markdown_bytes(data, anydoc::Format::Chm);
});
