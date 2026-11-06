#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};

// The BIFF record stream, wrapped in a valid OLE container so mutation
// reaches the reader instead of dying at the container gate.
fuzz_target!(|data: &[u8]| {
    let Ok(mut ole) = cfb::CompoundFile::create(Cursor::new(Vec::new())) else {
        return;
    };
    match ole.create_stream("Workbook") {
        Ok(mut stream) => {
            if stream.write_all(data).is_err() {
                return;
            }
        }
        Err(_) => return,
    }
    let bytes = ole.into_inner().into_inner();
    let _ = anydoc::to_markdown_bytes(&bytes, anydoc::Format::Excel);
});
