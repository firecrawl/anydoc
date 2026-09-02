# Fuzzing

    cargo +nightly fuzz run xlsx fuzz/corpus/xlsx fuzz/seeds/xlsx

`fuzz/seeds/` holds checked-in starting inputs; `fuzz/corpus/` is the working
directory libfuzzer writes to and is not checked in.

`xls`, `xlsb`, `numfmt` and `msg` wrap their input in a valid container (an
OLE compound file, an OPC package, a styles part, a message property store)
so mutation reaches the record, format-code and property parsers rather than
dying at the container gate. `xlsx` takes
a whole workbook, and its seeds cover all three containers because the
frontend picks the reader from the bytes.
