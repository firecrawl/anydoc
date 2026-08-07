use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureStats {
    pub markdown_chars: u64,
    pub headings: u64,
    pub list_items: u64,
    pub table_rows: u64,
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
    pub table_rows: u64,
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
            table_rows: 0,
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
        table_rows: 0,
    };

    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            stats.headings += 1;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("1. ") {
            stats.list_items += 1;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            stats.table_rows += 1;
        }
    }

    stats
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
        table_rows: stats.table_rows,
        warnings: Vec::new(),
    };

    let mut report_json =
        serde_json::to_vec_pretty(&conversion_report).map_err(ConversionError::from)?;
    report_json.push(b'\n');
    atomic_write(output, markdown.as_bytes()).map_err(ConversionError::from)?;
    atomic_write(report, &report_json).map_err(ConversionError::from)?;

    Ok(conversion_report)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent directory")
    })?;
    fs::create_dir_all(parent)?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no file name")
    })?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(
        ".{}.tmp-{}-{timestamp}-{sequence}",
        file_name.to_string_lossy(),
        std::process::id()
    );
    let temporary_path = parent.join(temporary_name);

    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}
