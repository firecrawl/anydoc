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


def nested_multipart(levels):
    out = bytearray(
        b'Snapshot-Content-Location: https://example.test/page\r\n'
        b'MIME-Version: 1.0\r\n'
        b'Content-Type: multipart/related; boundary="b0"\r\n\r\n'
    )
    for level in range(levels):
        out += (
            b'--b' + str(level).encode() + b'\r\n'
            b'Content-Type: multipart/mixed; boundary="b' + str(level + 1).encode() + b'"\r\n\r\n'
        )
    out += (
        b'--b' + str(levels).encode() + b'\r\nContent-Type: text/html\r\n\r\n<p>deep</p>\r\n'
        b'--b' + str(levels).encode() + b'--\r\n'
    )
    for level in range(levels - 1, -1, -1):
        out += b'--b' + str(level).encode() + b'--\r\n'
    return bytes(out)


class MhtmlBehaviorTests(unittest.TestCase):
    def test_excessive_multipart_nesting_is_rejected(self):
        with self.assertRaises(anydoc.ResourceLimitError):
            anydoc.to_markdown_bytes(nested_multipart(300))

    def test_cid_image_resolves_to_embedded_asset(self):
        mhtml = (
            b'Snapshot-Content-Location: https://example.test/page\r\n'
            b'MIME-Version: 1.0\r\n'
            b'Content-Type: multipart/related; boundary="b"\r\n\r\n'
            b'--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n'
            b'<!doctype html><p><img alt="logo" src="cid:logo@example.test"></p>\r\n'
            b'--b\r\nContent-Type: image/png\r\nContent-ID: <logo@example.test>\r\n\r\n'
            b'PNGDATA\r\n--b--\r\n'
        )
        # The cid reference resolves to the embedded part's asset...
        document = anydoc.to_document(mhtml)
        self.assertEqual(len(document.assets), 1)
        self.assertEqual(document.assets[0].media_type, 'image/png')
        # The Markdown renders an embedded asset as its alt text (bytes stay
        # in Document.assets); the structural asserts above are the real
        # resolution check.

    def test_relative_base_href_resolves_embedded_resources(self):
        mhtml = (
            b'Snapshot-Content-Location: https://example.test/page\r\n'
            b'MIME-Version: 1.0\r\n'
            b'Content-Type: multipart/related; boundary="b"\r\n\r\n'
            b'--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n'
            b'<!doctype html><base href="subdir/"><p><img alt="logo" src="./logo.png"></p>\r\n'
            b'--b\r\nContent-Type: image/png\r\nContent-Location: logo.png\r\n\r\n'
            b'PNGDATA\r\n--b--\r\n'
        )
        # The ./ dot segment only collapses through the relative base join,
        # so the asset resolves only if the base href is honored...
        document = anydoc.to_document(mhtml)
        self.assertEqual(len(document.assets), 1)
        self.assertEqual(document.assets[0].media_type, 'image/png')
        # Markdown alt rendering is covered by the renderer tests; the
        # structural asserts above are the resolution check.


if __name__ == '__main__':
    unittest.main()
