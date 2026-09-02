#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};

// Property values, wrapped in a valid message container so mutation reaches
// the string decoders and the body splitter instead of dying at the OLE
// gate. The same bytes fill a Unicode and an ANSI property, and stand in as
// the property store, so one input exercises every decoding path.
fuzz_target!(|data: &[u8]| {
    let Ok(mut ole) = cfb::CompoundFile::create(Cursor::new(Vec::new())) else {
        return;
    };
    for name in [
        "__properties_version1.0",
        // PR_BODY and PR_SUBJECT as PT_UNICODE, PR_DISPLAY_TO as PT_STRING8.
        "__substg1.0_1000001F",
        "__substg1.0_0037001F",
        "__substg1.0_0E04001E",
    ] {
        match ole.create_stream(name) {
            Ok(mut stream) => {
                if stream.write_all(data).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let bytes = ole.into_inner().into_inner();
    // Conversion may fail with a typed error; it must never panic, hang,
    // or exhaust memory.
    let _ = anydoc::to_markdown_bytes(&bytes, anydoc::Format::Msg);
});
