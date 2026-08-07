use anydoc_henry_mvp::{convert_file, ensure_private_directory};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug)]
struct CliArgs {
    input: PathBuf,
    output: PathBuf,
    report: PathBuf,
    label: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err((exit_code, diagnostic)) => {
            eprintln!("{diagnostic}");
            ExitCode::from(exit_code)
        }
    }
}

fn run() -> Result<(), (u8, String)> {
    let args = parse_args(std::env::args_os().skip(1))
        .map_err(|error| (2, format!("cli_error: {error}")))?;
    let private_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("out/private");
    ensure_private_directory(&private_root)
        .map_err(|error| (2, format!("cli_error: cannot prepare private output: {error}")))?;
    let output = validate_private_path(&args.output, &private_root)
        .map_err(|error| (2, format!("cli_error: invalid --output: {error}")))?;
    let report = validate_private_path(&args.report, &private_root)
        .map_err(|error| (2, format!("cli_error: invalid --report: {error}")))?;
    if output == report {
        return Err((2, "cli_error: --output and --report must differ".to_string()));
    }
    for parent in [output.parent(), report.parent()].into_iter().flatten() {
        ensure_private_directory(parent).map_err(|error| {
            (2, format!("cli_error: cannot secure private output directory: {error}"))
        })?;
    }

    convert_file(&args.input, &output, &report, &args.label)
        .map_err(|error| (1, format!("conversion_error: {error}")))?;
    Ok(())
}

fn parse_args(arguments: impl Iterator<Item = OsString>) -> Result<CliArgs, String> {
    let mut arguments = arguments.peekable();
    let mut input = None;
    let mut output = None;
    let mut report = None;
    let mut label = None;

    while let Some(flag) = arguments.next() {
        let flag = flag.to_str().ok_or_else(|| "flags must be valid UTF-8".to_string())?;
        let destination = match flag {
            "--input" => &mut input,
            "--output" => &mut output,
            "--report" => &mut report,
            "--label" => &mut label,
            _ => return Err(format!("unknown flag: {flag}")),
        };
        if destination.is_some() {
            return Err(format!("duplicate flag: {flag}"));
        }
        let value = arguments.next().ok_or_else(|| format!("missing value for {flag}"))?;
        if value.to_string_lossy().starts_with("--") {
            return Err(format!("missing value for {flag}"));
        }
        *destination = Some(value);
    }

    let input = input.ok_or_else(|| "missing required flag: --input".to_string())?;
    let output = output.ok_or_else(|| "missing required flag: --output".to_string())?;
    let report = report.ok_or_else(|| "missing required flag: --report".to_string())?;
    let label = label.ok_or_else(|| "missing required flag: --label".to_string())?;
    let label = label.into_string().map_err(|_| "--label must be valid UTF-8".to_string())?;
    if label.is_empty() || label.contains('/') || label.contains('\\') {
        return Err("--label must be non-empty and contain no slash".to_string());
    }

    Ok(CliArgs {
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        report: PathBuf::from(report),
        label,
    })
}

fn validate_private_path(path: &Path, private_root: &Path) -> Result<PathBuf, String> {
    let private_root = fs::canonicalize(private_root)
        .map_err(|error| format!("cannot resolve private output root: {error}"))?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?
            .join(path)
    };
    let normalized = normalize_path(&absolute);
    if !normalized.starts_with(&private_root) || normalized == private_root {
        return Err("path must be inside henry-mvp/out/private".to_string());
    }
    let parent = normalized.parent().ok_or_else(|| "path has no parent directory".to_string())?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("parent directory must already exist: {error}"))?;
    if !canonical_parent.starts_with(&private_root) {
        return Err("resolved parent escapes henry-mvp/out/private".to_string());
    }
    let file_name = normalized.file_name().ok_or_else(|| "path has no file name".to_string())?;
    Ok(canonical_parent.join(file_name))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
