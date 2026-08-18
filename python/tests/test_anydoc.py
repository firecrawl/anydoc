"""Smoke test: the bindings load and every entry point round-trips a fixture."""

import ast
import io
import unittest
import zipfile
from pathlib import Path

import anydoc

FIXTURES = Path(__file__).resolve().parents[2] / "tests" / "fixtures"
OUTLINE = FIXTURES / "docx" / "handmade-outline.docx"
RICH = FIXTURES / "docx" / "handmade-rich.docx"
CSV = FIXTURES / "csv" / "sheet.csv"
PDF = FIXTURES / "pdf" / "text.pdf"
ENCRYPTED = FIXTURES / "malformed" / "encrypted--errors.odt"
ZIPBOMB = FIXTURES / "abuse" / "zipbomb--errors.docx"


class AnydocTest(unittest.TestCase):
    def test_to_markdown_detects_the_format_from_the_file_content(self):
        markdown = anydoc.to_markdown(OUTLINE)
        self.assertRegex(markdown, r"(?m)^# ")

    def test_to_markdown_bytes_converts_in_memory(self):
        markdown = anydoc.to_markdown_bytes(RICH.read_bytes(), "docx")
        self.assertIn("| Quarter | Widgets |", markdown)

    def test_to_markdown_bytes_detects_the_format_when_none_is_named(self):
        markdown = anydoc.to_markdown_bytes(RICH.read_bytes())
        self.assertIn("| Quarter | Widgets |", markdown)
        # CSV carries no signature, so it has to be named.
        with self.assertRaisesRegex(anydoc.ConvertError, "unrecognized file content"):
            anydoc.to_markdown_bytes(CSV.read_bytes())
        self.assertIn("| --- |", anydoc.to_markdown_bytes(CSV.read_bytes(), "csv"))

    def test_to_document_exposes_the_document_model(self):
        document = anydoc.to_document(OUTLINE.read_bytes(), "docx")
        heading = next(block for block in document.blocks if block.kind == "heading")
        self.assertTrue(1 <= heading.level <= 6)
        self.assertIsInstance(heading.content[0].text, str)
        self.assertEqual(heading.content[0].kind, "text")
        self.assertIsInstance(heading.content[0].style.bold, bool)

    def test_to_document_carries_embedded_assets_as_bytes(self):
        document = anydoc.to_document(RICH.read_bytes(), "docx")
        image = next(asset for asset in document.assets if asset.media_type == "image/png")
        self.assertIsInstance(image.data, bytes)
        self.assertGreater(len(image.data), 0)
        self.assertEqual(image.id, document.assets.index(image))

    def test_pdf_pages_returns_a_page_at_a_time_with_its_ocr_verdict(self):
        pages = anydoc.pdf_pages(PDF.read_bytes())
        self.assertEqual([page.index for page in pages], [0, 1])
        self.assertIn("Fixture Document", pages[0].markdown)
        # Every page of this fixture has an extractable text layer, including
        # the thin second one that the document-level flag reports otherwise.
        self.assertEqual([page.index for page in pages if page.needs_ocr], [])
        self.assertIsNone(pages[0].ocr_reason)

    def test_pdf_pages_refuses_anything_that_is_not_a_pdf(self):
        with self.assertRaises(anydoc.MalformedError):
            anydoc.pdf_pages(RICH.read_bytes())

    def test_format_detection_reads_content_extension_and_path(self):
        self.assertEqual(anydoc.format_from_bytes(RICH.read_bytes()), "docx")
        # CSV carries no signature: only the extension names it.
        self.assertIsNone(anydoc.format_from_bytes(CSV.read_bytes()))
        self.assertEqual(anydoc.format_from_extension(".pptm"), "pptx")
        self.assertEqual(anydoc.format_from_extension("xls"), "xlsx")
        self.assertEqual(anydoc.format_from_path("report.odt"), "odt")
        self.assertIsNone(anydoc.format_from_path("report.unknown"))

    def test_conversion_errors_raise_the_subclass_that_names_the_failure(self):
        with self.assertRaises(anydoc.MalformedError) as caught:
            anydoc.to_markdown_bytes(b"not a document", "docx")
        # The base class still catches every one of them.
        self.assertIsInstance(caught.exception, anydoc.ConvertError)
        # Nothing about these bytes is a package part.
        self.assertIsNone(caught.exception.part)

        with self.assertRaises(anydoc.UnsupportedError):
            anydoc.to_markdown_bytes(CSV.read_bytes())

        with self.assertRaises(anydoc.EncryptedError):
            anydoc.to_markdown_bytes(ENCRYPTED.read_bytes(), "odt")

        with self.assertRaises(anydoc.ResourceLimitError) as caught:
            anydoc.to_markdown_bytes(ZIPBOMB.read_bytes(), "docx")
        self.assertEqual(caught.exception.limit, "max_entry_bytes")

        # A readable package carrying none of the parts a docx is made of.
        package = io.BytesIO()
        with zipfile.ZipFile(package, "w") as archive:
            archive.writestr("[Content_Types].xml", "<Types/>")
        with self.assertRaises(anydoc.MissingPartError) as caught:
            anydoc.to_markdown_bytes(package.getvalue(), "docx")
        self.assertEqual(caught.exception.part, "word/document.xml")

    def test_unreadable_files_and_bad_arguments_raise_the_python_exception(self):
        with self.assertRaises(FileNotFoundError):
            anydoc.to_markdown("no-such-file.docx")
        with self.assertRaisesRegex(ValueError, "unknown format"):
            anydoc.to_markdown_bytes(b"", "wat")

    def test_the_stubs_cover_the_module(self):
        stub = Path(anydoc.__file__).with_name("_anydoc.pyi")
        stubbed = {
            node.name
            for node in ast.parse(stub.read_text()).body
            if isinstance(node, (ast.FunctionDef, ast.ClassDef))
        }
        exported = {name for name in dir(anydoc._anydoc) if not name.startswith("_")}
        self.assertEqual(stubbed, exported)
        # __init__.py re-exports the whole module, plus the Format alias.
        self.assertEqual(set(anydoc.__all__), exported | {"Format"})


if __name__ == "__main__":
    unittest.main()
