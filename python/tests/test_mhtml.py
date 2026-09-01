import unittest
import anydoc

INPUT = (
    b'Snapshot-Content-Location: https://example.test/page\r\n'
    b'MIME-Version: 1.0\r\n'
    b'Content-Type: multipart/related; type="text/html"; boundary="b"\r\n\r\n'
    b'--b\r\nContent-Type: text/html\r\n\r\n'
    b'<!doctype html><h1>Hello MHTML</h1>\r\n--b--\r\n'
)


class MhtmlTests(unittest.TestCase):
    def test_mhtml_binding(self):
        self.assertEqual(anydoc.format_from_extension('mhtml'), 'mhtml')
        self.assertEqual(anydoc.format_from_extension('mht'), 'mhtml')
        self.assertEqual(anydoc.format_from_bytes(INPUT), 'mhtml')
        self.assertEqual(anydoc.to_markdown_bytes(INPUT), '# Hello MHTML\n')


if __name__ == '__main__':
    unittest.main()
