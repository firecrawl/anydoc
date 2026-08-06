//! End-to-end tests for the `anydoc` CLI binary: argument handling, exit
//! codes, and the conversion paths (file, stdin, explicit format, --output).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(rel)
}

fn anydoc(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anydoc")).args(args).output().unwrap()
}

fn anydoc_stdin(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anydoc"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> &str {
    str::from_utf8(&output.stdout).unwrap()
}

fn stderr(output: &Output) -> &str {
    str::from_utf8(&output.stderr).unwrap()
}

#[test]
fn help_prints_usage() {
    for flag in ["-h", "--help"] {
        let out = anydoc(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert!(stdout(&out).contains("Usage:"), "help text missing from {flag}");
    }
}

#[test]
fn version_prints_crate_version() {
    for flag in ["-V", "--version"] {
        let out = anydoc(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(stdout(&out).trim(), env!("CARGO_PKG_VERSION"));
    }
}

#[test]
fn converts_a_file_to_stdout() {
    let path = fixture("docx/handmade-numbering.docx");
    let out = anydoc(&[path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(!out.stdout.is_empty());
}

#[test]
fn converts_stdin_with_named_format() {
    let bytes = std::fs::read(fixture("csv/handmade-quoted.csv")).unwrap();
    let out = anydoc_stdin(&["-", "--format", "csv"], &bytes);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains('|'), "expected a Markdown table, got: {}", stdout(&out));
}

#[test]
fn inline_format_value_is_accepted() {
    let bytes = std::fs::read(fixture("csv/handmade-quoted.csv")).unwrap();
    let out = anydoc_stdin(&["-", "--format=csv"], &bytes);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
}

#[test]
fn format_alias_resolves() {
    // xls resolves to the Excel parser; the fixture is a real xlsx.
    let path = fixture("xlsx");
    let file = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "xlsx"));
    let Some(file) = file else { return };
    let out = anydoc(&[file.to_str().unwrap(), "-f", "xls"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
}

#[test]
fn output_flag_writes_the_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    let dest = dir.join("cli-output.md");
    let path = fixture("docx/handmade-numbering.docx");
    let out = anydoc(&[path.to_str().unwrap(), "-o", dest.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert!(!std::fs::read_to_string(&dest).unwrap().is_empty());
    std::fs::remove_file(&dest).unwrap();
}

#[test]
fn stdin_without_format_is_a_conversion_error_for_csv() {
    let bytes = std::fs::read(fixture("csv/handmade-quoted.csv")).unwrap();
    let out = anydoc_stdin(&["-"], &bytes);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("anydoc: "), "stderr: {}", stderr(&out));
}

#[test]
fn missing_file_is_a_conversion_error() {
    let out = anydoc(&["no-such-document.docx"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.starts_with("anydoc: "), "stderr: {err}");
    assert!(err.contains("no-such-document.docx"), "path missing from: {err}");
}

#[test]
fn missing_input_is_a_usage_error() {
    let out = anydoc(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("missing input"), "stderr: {}", stderr(&out));
}

#[test]
fn unknown_option_is_a_usage_error() {
    let out = anydoc(&["--frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("unknown option"), "stderr: {}", stderr(&out));
}

#[test]
fn invalid_format_is_a_usage_error() {
    let out = anydoc(&["-", "--format", "wat"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("invalid format"), "stderr: {}", stderr(&out));
}

#[test]
fn second_input_is_a_usage_error() {
    let out = anydoc(&["a.docx", "b.docx"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("second input"), "stderr: {}", stderr(&out));
}

#[test]
fn missing_option_value_is_a_usage_error() {
    let out = anydoc(&["a.docx", "--output"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("requires a value"), "stderr: {}", stderr(&out));
}

#[test]
fn double_dash_ends_option_parsing() {
    // After --, an option-looking argument is taken as the input path; the
    // file does not exist, so this is a conversion error, not a usage error.
    let out = anydoc(&["--", "--help"]);
    assert_eq!(out.status.code(), Some(1));
}
