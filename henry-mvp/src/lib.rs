use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureStats {
    pub markdown_chars: u64,
    pub headings: u64,
    pub list_items: u64,
    pub paragraphs: u64,
    pub table_rows: u64,
    pub tables: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionReport {
    pub label: String,
    pub sha256: String,
    pub input_bytes: u64,
    pub elapsed_ms: u64,
    pub markdown_chars: u64,
    pub headings: u64,
    pub list_items: u64,
    pub paragraphs: u64,
    pub table_rows: u64,
    pub tables: u64,
    pub warnings: Vec<String>,
}

impl ConversionReport {
    pub fn fixture_for_test() -> Self {
        Self {
            label: "privacy-contract-fixture".to_string(),
            sha256: "0".repeat(64),
            input_bytes: 1,
            elapsed_ms: 1,
            markdown_chars: 1,
            headings: 1,
            list_items: 0,
            paragraphs: 0,
            table_rows: 0,
            tables: 0,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum ConversionError {
    InvalidLabel,
    FormatDetection,
    AnyDoc(anydoc::ConvertError),
    EmptyMarkdown,
    OutputCollision,
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl ConversionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidLabel => "invalid_label",
            Self::FormatDetection => "format_detection",
            Self::AnyDoc(error) => error.code(),
            Self::EmptyMarkdown => "empty_markdown",
            Self::OutputCollision => "output_collision",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
        }
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: ", self.code())?;
        match self {
            Self::InvalidLabel => write!(formatter, "label must be non-empty and contain no slash"),
            Self::FormatDetection => {
                write!(formatter, "input format is unsupported or unrecognized")
            }
            Self::AnyDoc(error) => write!(formatter, "{error}"),
            Self::EmptyMarkdown => write!(formatter, "conversion produced no extractable text"),
            Self::OutputCollision => write!(formatter, "Markdown and report paths must differ"),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for ConversionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::AnyDoc(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ConversionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ConversionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn summarize_markdown(markdown: &str) -> StructureStats {
    let mut stats = StructureStats {
        markdown_chars: markdown.chars().count() as u64,
        headings: 0,
        list_items: 0,
        paragraphs: 0,
        table_rows: 0,
        tables: 0,
    };

    let mut in_paragraph = false;
    let mut in_table = false;

    for line in markdown.lines() {
        let trimmed = line.trim();
        let is_heading = trimmed.starts_with('#');
        let is_list_item = is_list_item(trimmed);
        let is_table_row = trimmed.starts_with('|') && trimmed.ends_with('|');

        if is_heading {
            stats.headings += 1;
        }
        if is_list_item {
            stats.list_items += 1;
        }
        if is_table_row {
            stats.table_rows += 1;
            if !in_table {
                stats.tables += 1;
            }
        }
        if trimmed.is_empty() || is_heading || is_list_item || is_table_row {
            in_paragraph = false;
        } else if !in_paragraph {
            stats.paragraphs += 1;
            in_paragraph = true;
        }
        in_table = is_table_row;
    }

    stats
}

fn is_list_item(line: &str) -> bool {
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return true;
    }
    let digit_count = line.bytes().take_while(u8::is_ascii_digit).count();
    digit_count > 0
        && matches!(line.as_bytes().get(digit_count..digit_count + 2), Some(b". ") | Some(b") "))
}

pub fn convert_file(
    input: &Path,
    output: &Path,
    report: &Path,
    label: &str,
) -> Result<ConversionReport, Box<dyn Error>> {
    if label.is_empty() || label.contains('/') || label.contains('\\') {
        return Err(Box::new(ConversionError::InvalidLabel));
    }
    if output == report {
        return Err(Box::new(ConversionError::OutputCollision));
    }

    let bytes = fs::read(input).map_err(ConversionError::from)?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let input_bytes = bytes.len() as u64;
    let format = anydoc::Format::from_bytes(&bytes)
        .or_else(|| anydoc::Format::from_path(input))
        .ok_or(ConversionError::FormatDetection)?;

    let started = Instant::now();
    let markdown = anydoc::to_markdown_bytes(&bytes, format).map_err(ConversionError::AnyDoc)?;
    if markdown.trim().is_empty() {
        return Err(Box::new(ConversionError::EmptyMarkdown));
    }
    let warnings = if format == anydoc::Format::Pdf {
        pdf_warnings(&bytes).map_err(ConversionError::AnyDoc)?
    } else {
        Vec::new()
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let stats = summarize_markdown(&markdown);
    let conversion_report = ConversionReport {
        label: label.to_string(),
        sha256,
        input_bytes,
        elapsed_ms,
        markdown_chars: stats.markdown_chars,
        headings: stats.headings,
        list_items: stats.list_items,
        paragraphs: stats.paragraphs,
        table_rows: stats.table_rows,
        tables: stats.tables,
        warnings,
    };

    let mut report_json =
        serde_json::to_vec_pretty(&conversion_report).map_err(ConversionError::from)?;
    report_json.push(b'\n');
    publish_pair(output, markdown.as_bytes(), report, &report_json)
        .map_err(ConversionError::from)?;

    Ok(conversion_report)
}

fn pdf_warnings(bytes: &[u8]) -> Result<Vec<String>, anydoc::ConvertError> {
    let analysis = pdf_inspector::process_pdf_mem(bytes).map_err(|error| match error {
        pdf_inspector::PdfError::Encrypted => anydoc::ConvertError::Encrypted,
        pdf_inspector::PdfError::Io(error) => anydoc::ConvertError::Io(error),
        other => anydoc::ConvertError::Unsupported(format!("PDF warning analysis failed: {other}")),
    })?;
    let mut warnings = Vec::new();
    if !analysis.pages_needing_ocr.is_empty() {
        warnings.push(format!(
            "pdf_ocr_required: {} of {} pages need OCR and were not extracted",
            analysis.pages_needing_ocr.len(),
            analysis.page_count
        ));
    }
    if analysis.has_encoding_issues {
        warnings.push(
            "pdf_encoding_issues: broken font encodings detected; extracted text may be garbled"
                .to_string(),
        );
    }
    Ok(warnings)
}

pub fn ensure_private_directory(path: &Path) -> Result<(), std::io::Error> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn publish_pair(
    output: &Path,
    markdown: &[u8],
    report: &Path,
    report_json: &[u8],
) -> Result<(), std::io::Error> {
    let output_temporary = stage_private_file(output, markdown)?;
    let report_temporary = match stage_private_file(report, report_json) {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_file(&output_temporary);
            return Err(error);
        }
    };

    let output_backup = if output.exists() {
        if !output.is_file() {
            let _ = fs::remove_file(&output_temporary);
            let _ = fs::remove_file(&report_temporary);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Markdown output path is not a regular file",
            ));
        }
        let backup = temporary_sibling_path(output, "backup")?;
        if let Err(error) = fs::rename(output, &backup) {
            let _ = fs::remove_file(&output_temporary);
            let _ = fs::remove_file(&report_temporary);
            return Err(error);
        }
        if let Err(error) = set_private_file_permissions(&backup) {
            let _ = fs::rename(&backup, output);
            let _ = fs::remove_file(&output_temporary);
            let _ = fs::remove_file(&report_temporary);
            return Err(error);
        }
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(&output_temporary, output) {
        let rollback = restore_output(output, output_backup.as_deref());
        let _ = fs::remove_file(&output_temporary);
        let _ = fs::remove_file(&report_temporary);
        return Err(rollback_error(error, rollback));
    }

    if let Err(error) = fs::rename(&report_temporary, report) {
        let rollback = restore_output(output, output_backup.as_deref());
        let _ = fs::remove_file(&report_temporary);
        return Err(rollback_error(error, rollback));
    }

    set_private_file_permissions(output)?;
    set_private_file_permissions(report)?;
    if let Some(backup) = output_backup {
        fs::remove_file(backup)?;
    }
    Ok(())
}

fn stage_private_file(path: &Path, bytes: &[u8]) -> Result<std::path::PathBuf, std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent directory")
    })?;
    if !parent.exists() {
        ensure_private_directory(parent)?;
    }
    let temporary_path = temporary_sibling_path(path, "stage")?;

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary_path)?;
        set_private_file_permissions(&temporary_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(temporary_path.clone())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn temporary_sibling_path(path: &Path, role: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent directory")
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no file name")
    })?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(
        ".{}.{}-{}-{timestamp}-{sequence}",
        file_name.to_string_lossy(),
        role,
        std::process::id()
    );
    Ok(parent.join(temporary_name))
}

fn restore_output(output: &Path, backup: Option<&Path>) -> Result<(), std::io::Error> {
    match backup {
        Some(backup) => {
            if fs::rename(backup, output).is_err() {
                if output.exists() {
                    fs::remove_file(output)?;
                }
                fs::rename(backup, output)?;
            }
            set_private_file_permissions(output)
        }
        None => {
            if output.exists() {
                fs::remove_file(output)?;
            }
            Ok(())
        }
    }
}

fn rollback_error(
    publication_error: std::io::Error,
    rollback: Result<(), std::io::Error>,
) -> std::io::Error {
    match rollback {
        Ok(()) => publication_error,
        Err(rollback_error) => std::io::Error::other(format!(
            "publication failed ({publication_error}); rollback also failed ({rollback_error})"
        )),
    }
}

fn set_private_file_permissions(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
