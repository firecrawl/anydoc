use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use super::{OfficeConverter, RenderBackend, ensure_output_path, run_with_timeout};

pub struct LibreOfficeConverter {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for LibreOfficeConverter {
    fn default() -> Self {
        let executable = [
            PathBuf::from(r"C:\Program Files\LibreOffice\program\soffice.exe"),
            PathBuf::from(r"C:\Program Files (x86)\LibreOffice\program\soffice.exe"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("soffice"));
        Self { executable, timeout: Duration::from_secs(120) }
    }
}

impl LibreOfficeConverter {
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self { executable, timeout }
    }
}

impl OfficeConverter for LibreOfficeConverter {
    fn backend(&self) -> RenderBackend {
        RenderBackend::LibreOffice
    }

    fn convert(&self, source: &Path, output_pdf: &Path) -> Result<()> {
        let output_dir = output_pdf.parent().context("PDF output has no parent")?;
        std::fs::create_dir_all(output_dir)?;
        ensure_output_path(output_dir, output_pdf)?;

        let output = run_with_timeout(
            Command::new(&self.executable)
                .arg("--headless")
                .arg("--convert-to")
                .arg("pdf")
                .arg("--outdir")
                .arg(output_dir)
                .arg(source),
            self.timeout,
        )?;
        if !output.success {
            bail!("LibreOffice export failed: {}{}", output.stderr.trim(), output.stdout.trim());
        }

        let generated_pdf = output_dir.join(format!(
            "{}.pdf",
            source
                .file_stem()
                .and_then(|value| value.to_str())
                .context("Office source has no UTF-8 file stem")?
        ));
        ensure_output_path(output_dir, &generated_pdf)?;
        if !generated_pdf.is_file() || generated_pdf.metadata()?.len() == 0 {
            bail!("LibreOffice did not create a non-empty PDF");
        }
        if generated_pdf != output_pdf {
            let _ = std::fs::remove_file(output_pdf);
            std::fs::rename(&generated_pdf, output_pdf).with_context(|| {
                format!(
                    "move LibreOffice PDF {} to {}",
                    generated_pdf.display(),
                    output_pdf.display()
                )
            })?;
        }
        Ok(())
    }
}
