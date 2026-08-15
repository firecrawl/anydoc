use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub mod libreoffice;
pub mod office;
pub mod pdf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderBackend {
    MicrosoftOffice,
    LibreOffice,
    PdfDirect,
    TextOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedPage {
    pub page_number: u32,
    pub image_path: PathBuf,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderManifest {
    pub backend: RenderBackend,
    pub pages: Vec<RenderedPage>,
}

pub trait OfficeConverter: Send + Sync {
    fn backend(&self) -> RenderBackend;
    fn convert(&self, source: &Path, output_pdf: &Path) -> Result<()>;
}

pub trait PdfPageRenderer: Send + Sync {
    fn render_pages(&self, pdf: &Path, output_dir: &Path) -> Result<Vec<RenderedPage>>;
}

pub struct Renderer {
    office_converters: Vec<Arc<dyn OfficeConverter>>,
    pdf_renderer: Arc<dyn PdfPageRenderer>,
}

impl Renderer {
    pub fn new(
        office_converters: Vec<Arc<dyn OfficeConverter>>,
        pdf_renderer: Arc<dyn PdfPageRenderer>,
    ) -> Self {
        Self { office_converters, pdf_renderer }
    }

    pub fn render(&self, source: &Path, output_dir: &Path) -> Result<RenderManifest> {
        std::fs::create_dir_all(output_dir)
            .with_context(|| format!("create render output {}", output_dir.display()))?;

        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if extension == "pdf" {
            return Ok(match self.pdf_renderer.render_pages(source, output_dir) {
                Ok(pages) => RenderManifest { backend: RenderBackend::PdfDirect, pages },
                Err(_) => text_only_manifest(),
            });
        }

        if !matches!(extension.as_str(), "doc" | "docx" | "ppt" | "pptx") {
            return Ok(text_only_manifest());
        }

        let output_pdf = output_dir.join("rendered.pdf");
        ensure_output_path(output_dir, &output_pdf)?;

        for converter in &self.office_converters {
            let _ = std::fs::remove_file(&output_pdf);
            if converter.convert(source, &output_pdf).is_err() || !output_pdf.is_file() {
                continue;
            }

            return Ok(match self.pdf_renderer.render_pages(&output_pdf, output_dir) {
                Ok(pages) => RenderManifest { backend: converter.backend(), pages },
                Err(_) => text_only_manifest(),
            });
        }

        Ok(text_only_manifest())
    }
}

pub(crate) struct ProcessOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) fn run_with_timeout(command: &mut Command, timeout: Duration) -> Result<ProcessOutput> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("start renderer process")?;
    let stdout = child.stdout.take().context("capture renderer stdout")?;
    let stderr = child.stderr.take().context("capture renderer stderr")?;

    let stdout_reader = thread::spawn(move || read_stream(stdout));
    let stderr_reader = thread::spawn(move || read_stream(stderr));
    let started = Instant::now();

    let status = loop {
        if let Some(status) = child.try_wait().context("poll renderer process")? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("renderer process timed out after {} seconds", timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    };

    let stdout =
        stdout_reader.join().map_err(|_| anyhow::anyhow!("renderer stdout reader panicked"))??;
    let stderr =
        stderr_reader.join().map_err(|_| anyhow::anyhow!("renderer stderr reader panicked"))??;

    Ok(ProcessOutput { success: status.success(), stdout, stderr })
}

fn read_stream(mut stream: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn ensure_output_path(root: &Path, candidate: &Path) -> Result<()> {
    let canonical_root =
        root.canonicalize().with_context(|| format!("resolve output root {}", root.display()))?;
    let parent = candidate.parent().context("output path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("resolve output parent {}", parent.display()))?;
    if !canonical_parent.starts_with(&canonical_root) {
        bail!("render output escaped the document cache");
    }
    Ok(())
}

fn text_only_manifest() -> RenderManifest {
    RenderManifest { backend: RenderBackend::TextOnly, pages: Vec::new() }
}
