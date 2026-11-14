"""Smoke test: the bindings load and every entry point round-trips a fixture."""

import ast
import io
import json
import os
import tempfile
import threading
import unittest
import zipfile
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import anydoc

FIXTURES = Path(__file__).resolve().parents[2] / "tests" / "fixtures"
OUTLINE = FIXTURES / "docx" / "handmade-outline.docx"
RICH = FIXTURES / "docx" / "handmade-rich.docx"
CSV = FIXTURES / "csv" / "sheet.csv"
ENCRYPTED = FIXTURES / "malformed" / "encrypted--errors.odt"
ZIPBOMB = FIXTURES / "abuse" / "zipbomb--errors.docx"
MIXED = FIXTURES / "pdf" / "handmade-mixed.pdf"
SCANNED_OFD = FIXTURES / "ofd" / "image-only.ofd"

HOSTED_MARKDOWN = "# Read by the hosted parser\n"


@contextmanager
def hosted_stub(status, body, sent=None):
    """A stand-in for api.firecrawl.dev that answers every request with one
    reply and records each hit as (path, whether a PDF came with it), and the
    Parse options each one sent into `sent`. The block runs keyless, whatever
    the environment."""
    hits = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            payload = self.rfile.read(int(self.headers.get("content-length", 0)))
            hits.append((self.path, b"%PDF-" in payload))
            if sent is not None:
                sent.append(parse_options(payload))
            reply = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    saved = {name: os.environ.pop(name, None) for name in ("FIRECRAWL_API_URL", "FIRECRAWL_API_KEY")}
    os.environ["FIRECRAWL_API_URL"] = f"http://127.0.0.1:{server.server_port}"
    try:
        yield hits
    finally:
        server.shutdown()
        server.server_close()
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def parse_options(payload):
    options = payload.split(b'name="options"\r\n\r\n', 1)[1]
    return json.loads(options.split(b"\r\n--", 1)[0])


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

    def test_format_detection_reads_content_extension_and_path(self):
        self.assertEqual(anydoc.format_from_bytes(RICH.read_bytes()), "docx")
        # CSV carries no signature: only the extension names it.
        self.assertIsNone(anydoc.format_from_bytes(CSV.read_bytes()))
        self.assertEqual(anydoc.format_from_extension(".pptm"), "pptx")
        self.assertEqual(anydoc.format_from_extension("xls"), "xlsx")
        self.assertEqual(anydoc.format_from_extension("ofd"), "ofd")
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

        # A scanned page is reported, not dropped from the output.
        with self.assertRaises(anydoc.NeedsOcrError) as caught:
            anydoc.to_markdown(MIXED)
        self.assertEqual((caught.exception.pages, caught.exception.page_count), ([2], 2))

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

    def test_ocr_hosted_sends_a_pdf_with_scanned_pages_to_firecrawl_parse_and_nothing_else(self):
        reply = {"success": True, "data": {"markdown": HOSTED_MARKDOWN}}
        with hosted_stub(200, reply) as hits:
            self.assertEqual(anydoc.to_markdown(MIXED, ocr="hosted"), HOSTED_MARKDOWN)
            self.assertEqual(hits, [("/v2/parse", True)])
            self.assertRegex(anydoc.to_markdown(OUTLINE, ocr="hosted"), r"(?m)^# ")
            self.assertEqual(hits, [("/v2/parse", True)])

    def test_ocr_hosted_keeps_image_only_ofd_as_needs_ocr(self):
        reply = {"success": True, "data": {"markdown": HOSTED_MARKDOWN}}
        with hosted_stub(200, reply) as hits:
            for convert in (
                lambda: anydoc.to_markdown(SCANNED_OFD, ocr="hosted"),
                lambda: anydoc.to_markdown_bytes(SCANNED_OFD.read_bytes(), "ofd", ocr="hosted"),
                lambda: anydoc.to_markdown_bytes(SCANNED_OFD.read_bytes(), ocr="hosted"),
            ):
                with self.assertRaises(anydoc.NeedsOcrError):
                    convert()
            self.assertEqual(hits, [])

    def test_hosted_fallback_uses_content_before_a_misleading_extension(self):
        reply = {"success": True, "data": {"markdown": HOSTED_MARKDOWN}}
        with tempfile.TemporaryDirectory() as directory:
            ofd_named_pdf = Path(directory) / "scan.pdf"
            pdf_named_ofd = Path(directory) / "scan.ofd"
            ofd_named_pdf.write_bytes(SCANNED_OFD.read_bytes())
            pdf_named_ofd.write_bytes(MIXED.read_bytes())
            with hosted_stub(200, reply) as hits:
                with self.assertRaises(anydoc.NeedsOcrError):
                    anydoc.to_markdown(ofd_named_pdf, ocr="hosted")
                self.assertEqual(hits, [])
                self.assertEqual(anydoc.to_markdown(pdf_named_ofd, ocr="hosted"), HOSTED_MARKDOWN)
                self.assertEqual(hits, [("/v2/parse", True)])

    def test_parse_mode_picks_the_parse_pdf_mode_auto_by_default(self):
        reply = {"success": True, "data": {"markdown": HOSTED_MARKDOWN}}
        sent = []
        with hosted_stub(200, reply, sent):
            anydoc.to_markdown(MIXED, ocr="hosted")
            anydoc.to_markdown_bytes(MIXED.read_bytes(), "pdf", ocr="hosted", parse_mode="ocr")
            anydoc.to_markdown(MIXED, ocr="hosted", parse_mode="fast")
        self.assertEqual(
            [options["parsers"] for options in sent],
            [
                [{"type": "pdf", "mode": "auto"}],
                [{"type": "pdf", "mode": "ocr"}],
                [{"type": "pdf", "mode": "fast"}],
            ],
        )

    def test_an_invalid_parse_mode_raises_before_anything_is_sent(self):
        reply = {"success": True, "data": {"markdown": HOSTED_MARKDOWN}}
        expected = "invalid parse_mode .*expected one of: auto, fast, ocr"
        with hosted_stub(200, reply) as hits:
            with self.assertRaisesRegex(anydoc.HostedError, expected):
                anydoc.to_markdown(OUTLINE, ocr="hosted", parse_mode="OCR")
            with self.assertRaisesRegex(anydoc.HostedError, expected):
                anydoc.to_markdown_bytes(MIXED.read_bytes(), ocr="hosted", parse_mode="accurate")
            self.assertEqual(hits, [])

    def test_the_keyless_limit_says_to_set_an_api_key(self):
        with hosted_stub(429, {"success": False, "error": "Rate limit exceeded"}):
            with self.assertRaisesRegex(anydoc.HostedError, "set FIRECRAWL_API_KEY"):
                anydoc.to_markdown_bytes(MIXED.read_bytes(), ocr="hosted")

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
        # __init__.py re-exports the whole module, plus what it adds itself.
        added = {"Format", "HostedError", "Ocr", "ParseMode"}
        self.assertEqual(set(anydoc.__all__), exported | added)


if __name__ == "__main__":
    unittest.main()
