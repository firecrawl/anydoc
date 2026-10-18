//! The `anydoc` command line, shared by every first-party front end.
//!
//! [`run`] is the whole command: parse arguments, convert one document, and
//! write the Markdown to stdout or `--output`. The `cargo install anydoc`
//! binary (`src/bin/anydoc.rs`) and the `firecrawl-anydoc` Python package's
//! console script both call it, so the two behave identically: same options,
//! same help, same exit codes. Its options, help, and exit codes match the
//! npm CLI (`node/cli.js`), apart from the hosted-OCR flags (`--ocr`,
//! `--api-key`, `--api-url`), which only the Node front end carries so far.
//!
//! This is an implementation detail of those front ends, not a stable API.

use crate::{ConvertError, Format};
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;

const FORMATS: &str = "doc, docx, odt, pdf, ppt, pptx, rtf, epub, xlsx, ods, odp, csv";

const CONVERSION_ERROR: i32 = 1;
const USAGE_ERROR: i32 = 2;
const NEEDS_OCR: i32 = 3;

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
input from stdin needs --format csv. Scanned or image-only pages need OCR,
which anydoc does not do: the document exits 3.

Exit codes:
  0  success
  1  the document could not be read or converted
  2  usage error: unknown option, missing input, or invalid --format
  3  a page needs OCR, which anydoc does not do

Examples:
  anydoc report.docx
  anydoc slides.pptx -o slides.md
  anydoc - --format csv < data.csv
  curl -s https://example.com/paper.pdf | anydoc -
"
    )
}

/// A diagnostic bound for stderr and the exit code that goes with it.
struct Fail {
    code: i32,
    message: String,
}

fn fail(code: i32, message: impl Into<String>) -> Fail {
    Fail { code, message: message.into() }
}

/// Anything starting with `-` is an option, except the lone `-` that names
/// stdin. Works on the raw bytes so a non-UTF-8 argument still classifies.
fn is_option(arg: &OsStr) -> bool {
    let bytes = arg.as_encoded_bytes();
    bytes.first() == Some(&b'-') && bytes.len() > 1
}

#[derive(Default)]
struct Args {
    input: Option<OsString>,
    output: Option<OsString>,
    format: Option<OsString>,
}

/// `--help`/`--version` have already printed and only carry an exit code.
enum Parsed {
    Run(Args),
    Done(i32),
}

fn parse_args(argv: &[OsString]) -> Result<Parsed, Fail> {
    let mut args = Args::default();
    let mut positional_only = false;
    let mut iter = argv.iter().peekable();
    while let Some(raw) = iter.next() {
        if positional_only || !is_option(raw) {
            if args.input.is_some() {
                return Err(fail(
                    USAGE_ERROR,
                    format!(
                        "one document per invocation: unexpected second input '{}'",
                        raw.to_string_lossy()
                    ),
                ));
            }
            args.input = Some(raw.clone());
            continue;
        }
        if raw == "--" {
            positional_only = true;
            continue;
        }
        // An option name is always ASCII; a non-UTF-8 argument that starts
        // with `-` cannot match one.
        let Some(text) = raw.to_str() else {
            return Err(fail(
                USAGE_ERROR,
                format!("unknown option '{}' (see anydoc --help)", raw.to_string_lossy()),
            ));
        };
        // --opt=value carries its value inline; short options take the next
        // argument.
        let (name, inline) = match text.strip_prefix("--").and_then(|_| text.split_once('=')) {
            Some((name, value)) => (name, Some(OsString::from(value))),
            None => (text, None),
        };
        let mut value = |name: &str| -> Result<OsString, Fail> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            // A following option (or `--`) means the value was omitted, not
            // that the next argument is it.
            match iter.peek() {
                Some(next) if *next != "--" && !is_option(next) => Ok(iter.next().unwrap().clone()),
                _ => Err(fail(USAGE_ERROR, format!("{name} requires a value"))),
            }
        };
        match name {
            "-h" | "--help" => return Ok(Parsed::Done(write_stdout(help().as_bytes()))),
            "-V" | "--version" => {
                return Ok(Parsed::Done(write_stdout(
                    concat!(env!("CARGO_PKG_VERSION"), "\n").as_bytes(),
                )));
            }
            "-o" | "--output" => args.output = Some(value(name)?),
            "-f" | "--format" => args.format = Some(value(name)?),
            _ => {
                return Err(fail(
                    USAGE_ERROR,
                    format!("unknown option '{name}' (see anydoc --help)"),
                ));
            }
        }
    }
    Ok(Parsed::Run(args))
}

fn read_stdin() -> Result<Vec<u8>, Fail> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Err(fail(
            USAGE_ERROR,
            "stdin is a terminal; pipe or redirect a document into anydoc -",
        ));
    }
    let mut bytes = Vec::new();
    stdin.read_to_end(&mut bytes).map_err(|e| fail(CONVERSION_ERROR, e.to_string()))?;
    Ok(bytes)
}

/// Write to stdout, treating a reader that closed the pipe early
/// (`anydoc big.xlsx | head`) as success rather than a broken-pipe panic.
fn write_stdout(bytes: &[u8]) -> i32 {
    match io::stdout().write_all(bytes) {
        Ok(()) => 0,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(_) => CONVERSION_ERROR,
    }
}

fn convert(argv: &[OsString]) -> Result<i32, Fail> {
    let args = match parse_args(argv)? {
        Parsed::Done(code) => return Ok(code),
        Parsed::Run(args) => args,
    };
    let Some(input) = args.input else {
        return Err(fail(
            USAGE_ERROR,
            "missing input: pass a document path, or - for stdin (see anydoc --help)",
        ));
    };

    let format = match &args.format {
        Some(name) => Some(name.to_str().and_then(Format::from_extension).ok_or_else(|| {
            fail(
                USAGE_ERROR,
                format!("invalid format '{}'; expected one of: {FORMATS}", name.to_string_lossy()),
            )
        })?),
        None => None,
    };

    let from_stdin = input == "-";
    let result = if from_stdin {
        crate::to_markdown_bytes(&read_stdin()?, format)
    } else if let Some(format) = format {
        match std::fs::read(&input) {
            Ok(bytes) => crate::to_markdown_bytes(&bytes, format),
            Err(e) => {
                return Err(fail(
                    CONVERSION_ERROR,
                    format!("{}: {e}", Path::new(&input).display()),
                ));
            }
        }
    } else {
        crate::to_markdown(&input)
    };
    let markdown = result.map_err(|e| {
        // A page that needs OCR gets its own exit code so a script can tell
        // it apart from a document that simply could not be read.
        let code =
            if matches!(e, ConvertError::NeedsOcr { .. }) { NEEDS_OCR } else { CONVERSION_ERROR };
        // An unreadable file surfaces as ConvertError::Io; name the path,
        // which the library-level message does not carry.
        let message = match &e {
            ConvertError::Io(io) if !from_stdin => format!("{}: {io}", Path::new(&input).display()),
            _ => e.to_string(),
        };
        Fail { code, message }
    })?;

    match args.output {
        Some(path) => {
            std::fs::write(&path, markdown).map_err(|e| {
                fail(CONVERSION_ERROR, format!("{}: {e}", Path::new(&path).display()))
            })?;
            Ok(0)
        }
        None => Ok(write_stdout(markdown.as_bytes())),
    }
}

/// Run the `anydoc` command line and return its process exit code. `args` is
/// the argument list without the executable name, e.g.
/// `std::env::args_os().skip(1)`. Converted Markdown goes to stdout or the
/// `--output` path; every diagnostic goes to stderr, prefixed `anydoc:`. This
/// never reads a prompt and never panics on ordinary input, including
/// non-UTF-8 filenames.
pub fn run<I>(args: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let argv: Vec<OsString> = args.into_iter().collect();
    match convert(&argv) {
        Ok(code) => code,
        Err(Fail { code, message }) => {
            eprintln!("anydoc: {message}");
            code
        }
    }
}
