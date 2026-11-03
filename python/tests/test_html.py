import unittest

import anydoc


class HtmlBindingTests(unittest.TestCase):
    def test_standalone_html_is_exposed(self):
        data = b'<!doctype html><h1>Hello</h1><p><b>world</b></p>'
        self.assertEqual(anydoc.format_from_extension('html'), 'html')
        self.assertEqual(anydoc.format_from_bytes(data), 'html')
        self.assertEqual(anydoc.to_markdown_bytes(data), '# Hello\n\n**world**\n')


if __name__ == '__main__':
    unittest.main()
