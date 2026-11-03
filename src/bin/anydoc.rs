//! The `anydoc` CLI: convert one document to GitHub-Flavored Markdown.
//!
//! Mirrors the npm CLI (`node/cli.js`) flag for flag, so the two front ends
//! stay interchangeable: same options, same help, same exit codes.

use anydoc::Format;
use std::io::{IsTerminal, Read, Write};
use std::process::exit;

const FORMATS: &str = "doc, docx, odt, pdf, ppt, pptx, rtf, epub, xlsx, ods, odp, csv";

const USAGE_ERROR: i32 = 2;
const CONVERSION_ERROR: i32 = 1;

fn help() -> String {
    format!(
        "anydoc: convert documents to GitHub-Flavored Markdown

Usage:
  anydoc <file> [options]
  anydoc - [options] < file

Converts one document per invocation and writes the Markdown to stdout.
Pass - as the input to read the document from stdin. Never prompts; all
diagnostics go to stderr.

Options:
  -o, --output <path>    Write the Markdown to <path> instead of stdout
  -f, --format <format>  Name the input format instead of detecting it:
                         {FORMATS}
                         (extension aliases like xls, docm, ppsx resolve
                         to these)
  -h, --help             Print this help and exit
  -V, --version          Print the version and exit

The format is detected from the file content; the file extension is the
fallback for signature-less formats (CSV). stdin has no extension, so CSV
input from stdin needs --format csv. Scanned or image-only PDFs need OCR,
which anydoc does not do, and error as unsupported.

Exit codes:
  0  success
  1  the document could not be read or converted
  2  usage error: unknown option, missing input, or invalid --format

Examples:
  anydoc report.docx
  anydoc slides.pptx -o slides.md
  anydoc - --format csv < data.csv
  curl -s https://example.com/paper.pdf | anydoc -
"
    )
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("anydoc: {message}");
    exit(code);
}

#[derive(Default)]
struct Args {
    input: Option<String>,
    output: Option<String>,
    format: Option<String>,
}

fn parse_args(argv: &[String]) -> Args {
    let mut args = Args::default();
    let mut positional_only = false;
    let mut iter = argv.iter().peekable();
    while let Some(raw) = iter.next() {
        if positional_only || raw == "-" || !raw.starts_with('-') {
            if args.input.is_some() {
                fail(
                    USAGE_ERROR,
                    &format!("one document per invocation: unexpected second input '{raw}'"),
                );
            }
            args.input = Some(raw.clone());
            continue;
        }
        if raw == "--" {
            positional_only = true;
            continue;
        }
        // --opt=value carries its value inline; short options take the next
        // argument.
        let (arg, inline) = match raw.starts_with("--").then(|| raw.split_once('=')).flatten() {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (raw.as_str(), None),
        };
        let mut value = |arg: &str| {
            inline.clone().unwrap_or_else(|| {
                iter.next()
                    .unwrap_or_else(|| fail(USAGE_ERROR, &format!("{arg} requires a value")))
                    .clone()
            })
        };
        match arg {
            "-h" | "--help" => {
                print!("{}", help());
                exit(0);
            }
            "-V" | "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                exit(0);
            }
            "-o" | "--output" => args.output = Some(value(arg)),
            "-f" | "--format" => args.format = Some(value(arg)),
            _ => fail(USAGE_ERROR, &format!("unknown option '{arg}' (see anydoc --help)")),
        }
    }
    args
}

fn read_stdin() -> Vec<u8> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        fail(USAGE_ERROR, "stdin is a terminal; pipe or redirect a document into anydoc -");
    }
    let mut bytes = Vec::new();
    if let Err(e) = stdin.read_to_end(&mut bytes) {
        fail(CONVERSION_ERROR, &e.to_string());
    }
    bytes
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&argv);
    let Some(input) = args.input else {
        fail(
            USAGE_ERROR,
            "missing input: pass a document path, or - for stdin (see anydoc --help)",
        );
    };

    let format = args.format.map(|name| {
        Format::from_extension(&name).unwrap_or_else(|| {
            fail(USAGE_ERROR, &format!("invalid format '{name}'; expected one of: {FORMATS}"))
        })
    });

    let result = if input == "-" {
        anydoc::to_markdown_bytes(&read_stdin(), format)
    } else if let Some(format) = format {
        match std::fs::read(&input) {
            Ok(bytes) => anydoc::to_markdown_bytes(&bytes, format),
            Err(e) => fail(CONVERSION_ERROR, &format!("{input}: {e}")),
        }
    } else {
        anydoc::to_markdown(&input)
    };
    let markdown = result.unwrap_or_else(|e| {
        // An unreadable file surfaces as ConvertError::Io; name the path,
        // which the library-level message does not carry.
        let message = match (&e, input.as_str()) {
            (anydoc::ConvertError::Io(io), path) if path != "-" => format!("{path}: {io}"),
            _ => e.to_string(),
        };
        fail(CONVERSION_ERROR, &message)
    });

    match args.output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, markdown) {
                fail(CONVERSION_ERROR, &format!("{path}: {e}"));
            }
        }
        None => {
            // Downstream closing the pipe early (e.g. `anydoc big.xlsx | head`)
            // is not a conversion failure.
            if let Err(e) = std::io::stdout().write_all(markdown.as_bytes()) {
                exit(if e.kind() == std::io::ErrorKind::BrokenPipe { 0 } else { CONVERSION_ERROR });
            }
        }
    }
}
