use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::ImageFormat;
use pdfium_render::prelude::*;

use super::{PdfPageRenderer, RenderedPage, ensure_output_path};

pub struct PdfiumPageRenderer {
    library_path: Option<PathBuf>,
    target_width: i32,
    maximum_height: i32,
}

impl Default for PdfiumPageRenderer {
    fn default() -> Self {
        Self { library_path: None, target_width: 1800, maximum_height: 3000 }
    }
}

impl PdfiumPageRenderer {
    pub fn new(library_path: Option<PathBuf>) -> Self {
        Self { library_path, ..Self::default() }
    }

    fn pdfium(&self) -> Result<Pdfium> {
        let mut candidates = Vec::new();
        if let Some(path) = &self.library_path {
            candidates.push(path.clone());
        }
        if let Ok(path) = std::env::var("ANYDOC_PDFIUM_PATH") {
            candidates.push(PathBuf::from(path));
        }
        if let Ok(executable) = std::env::current_exe()
            && let Some(directory) = executable.parent()
        {
            candidates.push(directory.join("pdfium.dll"));
            candidates.push(directory.join("resources").join("pdfium.dll"));
        }
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium.dll"));

        for candidate in candidates {
            if candidate.is_file() {
                return match Pdfium::bind_to_library(&candidate) {
                    Ok(bindings) => Ok(Pdfium::new(bindings)),
                    Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => {
                        Ok(Pdfium::default())
                    }
                    Err(error) => Err(error)
                        .with_context(|| format!("bind PDFium library {}", candidate.display())),
                };
            }
        }

        match Pdfium::bind_to_system_library() {
            Ok(bindings) => Ok(Pdfium::new(bindings)),
            Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Ok(Pdfium::default()),
            Err(error) => Err(error).context("no packaged or system PDFium library is available"),
        }
    }
}

impl PdfPageRenderer for PdfiumPageRenderer {
    fn render_pages(&self, pdf: &Path, output_dir: &Path) -> Result<Vec<RenderedPage>> {
        std::fs::create_dir_all(output_dir)?;
        let pdfium = self.pdfium()?;
        let document = pdfium
            .load_pdf_from_file(pdf, None)
            .with_context(|| format!("open PDF {}", pdf.display()))?;
        let config = PdfRenderConfig::new()
            .set_target_width(self.target_width)
            .set_maximum_height(self.maximum_height)
            .render_annotations(true)
            .render_form_data(true);
        let mut pages = Vec::with_capacity(document.pages().len() as usize);

        for (index, page) in document.pages().iter().enumerate() {
            let image = page
                .render_with_config(&config)
                .with_context(|| format!("render PDF page {}", index + 1))?
                .as_image()
                .with_context(|| format!("convert PDF page {} to an image", index + 1))?;
            let (width, height) = (image.width(), image.height());
            if width == 0 || height == 0 {
                bail!("PDF page {} rendered with empty dimensions", index + 1);
            }

            let image_path = output_dir.join(format!("page-{:04}.png", index + 1));
            ensure_output_path(output_dir, &image_path)?;
            let temporary_path = output_dir.join(format!(".page-{:04}.png.tmp", index + 1));
            ensure_output_path(output_dir, &temporary_path)?;
            image
                .save_with_format(&temporary_path, ImageFormat::Png)
                .with_context(|| format!("write rendered page {}", temporary_path.display()))?;
            if image_path.exists() {
                std::fs::remove_file(&image_path)?;
            }
            std::fs::rename(&temporary_path, &image_path)?;
            pages.push(RenderedPage { page_number: index as u32 + 1, image_path, width, height });
        }

        Ok(pages)
    }
}
