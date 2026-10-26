//! Convert a document to Markdown:
//! `cargo run --example convert -- <file> [-f csv] [-o out.md] [--assets dir]`

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anydoc::{ConvertError, Format};

const USAGE: &str = "usage: convert <file> [-f csv] [-o out.md] [--assets dir] [-p PASSWORD]";
/// Env fallback for `-p/--password`: argv leaks into shell history and `ps`.
const PASSWORD_ENV: &str = "ANYDOC_PASSWORD";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut format: Option<Format> = None;
    let mut assets: Option<PathBuf> = None;
    let mut password: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                output = args.get(i).map(PathBuf::from);
            }
            "-f" | "--format" => {
                i += 1;
                let Some(name) = args.get(i) else { break };
                let Some(named) = Format::from_extension(name) else {
                    eprintln!("error: unknown format: {name}");
                    return ExitCode::FAILURE;
                };
                format = Some(named);
            }
            "--assets" => {
                i += 1;
                assets = args.get(i).map(PathBuf::from);
            }
            "-p" | "--password" => {
                i += 1;
                match args.get(i) {
                    Some(pw) => password = Some(pw.clone()),
                    None => {
                        eprintln!("error: --password requires a value");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other => input = Some(PathBuf::from(other)),
        }
        i += 1;
    }
    let Some(input) = input else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    match run(&input, output.as_deref(), format, assets.as_deref(), password) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    input: &Path,
    output: Option<&Path>,
    format: Option<Format>,
    assets: Option<&Path>,
    password: Option<String>,
) -> Result<(), ConvertError> {
    let password = password.or_else(|| std::env::var(PASSWORD_ENV).ok());
    let raw = std::fs::read(input)?;

    // Decrypt before detection: an encrypted OOXML container has no
    // recognizable signature until after decryption, mirroring the library's
    // to_markdown_with_password semantics (#130 review).
    let decrypted;
    let bytes: &[u8] = match password.as_deref() {
        Some(pw) if !pw.is_empty() && anydoc::is_encrypted_ooxml(&raw) => {
            decrypted = anydoc::decrypt_ooxml(raw.clone(), pw)?;
            &decrypted
        }
        _ => &raw,
    };

    // Without -f the format comes from the (now plaintext) content, with the
    // extension as the fallback.
    let format =
        match format.or_else(|| Format::from_bytes(bytes)).or_else(|| Format::from_path(input)) {
            Some(format) => format,
            None => {
                return Err(ConvertError::Unsupported(format!(
                    "unrecognized file content and extension: {}",
                    input.display()
                )));
            }
        };

    let start = std::time::Instant::now();
    let markdown = anydoc::to_markdown_bytes(bytes, format)?;
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("converted {} in {}", input.display(), millis(elapsed));

    match output {
        Some(out) => std::fs::write(out, markdown)?,
        None => {
            use std::io::Write;
            let _ = std::io::stdout().write_all(markdown.as_bytes());
        }
    }

    // Images and embedded objects live on the document model, not in the
    // Markdown, so they need a second pass to write out.
    if let Some(dir) = assets {
        let document = anydoc::to_document(bytes, format)?;
        std::fs::create_dir_all(dir)?;
        let stem = input.file_stem().unwrap_or_default().to_string_lossy();
        for asset in &document.assets {
            let (kind, subtype) = asset.media_type.split_once('/').unwrap_or(("", ""));
            let extension = match kind {
                "image" => subtype.chars().filter(char::is_ascii_alphanumeric).collect(),
                _ => "bin".to_string(),
            };
            std::fs::write(dir.join(format!("{stem}-{}.{extension}", asset.id.0)), &asset.bytes)?;
        }
        eprintln!("wrote {} assets to {}", document.assets.len(), dir.display());
    }
    Ok(())
}

fn millis(ms: f64) -> String {
    if ms < 10.0 { format!("{ms:.2}ms") } else { format!("{ms:.0}ms") }
}
