use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use super::{OfficeConverter, RenderBackend, ensure_output_path, run_with_timeout};

const OFFICE_SCRIPT: &str = include_str!("render_office.ps1");

pub struct MicrosoftOfficeConverter {
    powershell: PathBuf,
    timeout: Duration,
}

impl Default for MicrosoftOfficeConverter {
    fn default() -> Self {
        Self { powershell: PathBuf::from("powershell.exe"), timeout: Duration::from_secs(120) }
    }
}

impl MicrosoftOfficeConverter {
    pub fn new(powershell: PathBuf, timeout: Duration) -> Self {
        Self { powershell, timeout }
    }
}

impl OfficeConverter for MicrosoftOfficeConverter {
    fn backend(&self) -> RenderBackend {
        RenderBackend::MicrosoftOffice
    }

    fn convert(&self, source: &Path, output_pdf: &Path) -> Result<()> {
        let output_dir = output_pdf.parent().context("PDF output has no parent")?;
        std::fs::create_dir_all(output_dir)?;
        ensure_output_path(output_dir, output_pdf)?;

        let script_path = output_dir.join("render-office.ps1");
        ensure_output_path(output_dir, &script_path)?;
        std::fs::write(&script_path, OFFICE_SCRIPT)
            .with_context(|| format!("write Office renderer script {}", script_path.display()))?;

        let output = run_with_timeout(
            Command::new(&self.powershell)
                .arg("-NoLogo")
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(&script_path)
                .arg("-SourcePath")
                .arg(source)
                .arg("-OutputPath")
                .arg(output_pdf),
            self.timeout,
        );
        let _ = std::fs::remove_file(script_path);
        let output = output?;

        if !output.success {
            bail!(
                "Microsoft Office export failed: {}{}",
                output.stderr.trim(),
                output.stdout.trim()
            );
        }
        if !output_pdf.is_file() || output_pdf.metadata()?.len() == 0 {
            bail!("Microsoft Office did not create a non-empty PDF");
        }
        Ok(())
    }
}
