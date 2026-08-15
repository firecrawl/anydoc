use std::{path::Path, sync::Arc};

use anyhow::{Result, bail};

use crate::rendering::{
    OfficeConverter, PdfPageRenderer, RenderBackend, RenderedPage, Renderer,
    libreoffice::LibreOfficeConverter, office::MicrosoftOfficeConverter, pdf::PdfiumPageRenderer,
};

struct FakeConverter {
    backend: RenderBackend,
    succeeds: bool,
}

impl OfficeConverter for FakeConverter {
    fn backend(&self) -> RenderBackend {
        self.backend
    }

    fn convert(&self, _source: &Path, output_pdf: &Path) -> Result<()> {
        if !self.succeeds {
            bail!("converter unavailable");
        }
        std::fs::write(output_pdf, b"fake pdf")?;
        Ok(())
    }
}

struct FakePdfRenderer;

impl PdfPageRenderer for FakePdfRenderer {
    fn render_pages(&self, _pdf: &Path, output_dir: &Path) -> Result<Vec<RenderedPage>> {
        let image_path = output_dir.join("page-0001.png");
        std::fs::write(&image_path, b"fake png")?;
        Ok(vec![RenderedPage { page_number: 1, image_path, width: 1600, height: 900 }])
    }
}

#[test]
fn falls_back_to_libreoffice_when_office_fails() {
    let renderer = Renderer::new(
        vec![
            Arc::new(FakeConverter { backend: RenderBackend::MicrosoftOffice, succeeds: false }),
            Arc::new(FakeConverter { backend: RenderBackend::LibreOffice, succeeds: true }),
        ],
        Arc::new(FakePdfRenderer),
    );
    let output = tempfile::tempdir().expect("temporary output directory");

    let result =
        renderer.render(Path::new("slides.pptx"), output.path()).expect("fallback succeeds");

    assert_eq!(result.backend, RenderBackend::LibreOffice);
    assert_eq!(result.pages.len(), 1);
}

#[test]
fn returns_text_only_when_all_office_converters_fail() {
    let renderer = Renderer::new(
        vec![
            Arc::new(FakeConverter { backend: RenderBackend::MicrosoftOffice, succeeds: false }),
            Arc::new(FakeConverter { backend: RenderBackend::LibreOffice, succeeds: false }),
        ],
        Arc::new(FakePdfRenderer),
    );
    let output = tempfile::tempdir().expect("temporary output directory");

    let result = renderer
        .render(Path::new("document.docx"), output.path())
        .expect("text fallback is a valid result");

    assert_eq!(result.backend, RenderBackend::TextOnly);
    assert!(result.pages.is_empty());
}

#[test]
fn renders_pdf_directly_without_office_conversion() {
    let renderer = Renderer::new(Vec::new(), Arc::new(FakePdfRenderer));
    let output = tempfile::tempdir().expect("temporary output directory");

    let result =
        renderer.render(Path::new("report.pdf"), output.path()).expect("PDF renders directly");

    assert_eq!(result.backend, RenderBackend::PdfDirect);
    assert_eq!(result.pages[0].width, 1600);
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires locally installed Microsoft Word and PowerPoint"]
fn renders_real_docx_and_pptx_into_non_empty_page_images() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_root = manifest_dir.join("../../..");
    let pdfium = manifest_dir.join("resources/pdfium.dll");
    let renderer = Renderer::new(
        vec![
            Arc::new(MicrosoftOfficeConverter::default()),
            Arc::new(LibreOfficeConverter::default()),
        ],
        Arc::new(PdfiumPageRenderer::new(Some(pdfium))),
    );
    let fixtures = [
        repository_root.join("tests/fixtures/docx/text.docx"),
        repository_root.join("tests/fixtures/pptx/pres.pptx"),
    ];
    let output_root = tempfile::tempdir().expect("temporary render output");

    for (index, fixture) in fixtures.iter().enumerate() {
        let output_dir = output_root.path().join(index.to_string());
        let result = renderer
            .render(fixture, &output_dir)
            .unwrap_or_else(|error| panic!("render {}: {error:#}", fixture.display()));

        assert_eq!(result.backend, RenderBackend::MicrosoftOffice);
        assert!(!result.pages.is_empty());
        for page in result.pages {
            assert!(page.image_path.is_file());
            assert!(page.width > 0);
            assert!(page.height > 0);
            let decoded = image::open(&page.image_path).expect("rendered PNG decodes");
            assert_eq!(decoded.width(), page.width);
            assert_eq!(decoded.height(), page.height);
        }
    }
}
