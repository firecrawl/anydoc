"""Tests for CHM benchmark accounting; no timing thresholds."""

import sys
import unittest

import chm
import convert


class ChmBenchmarkTests(unittest.TestCase):
    def test_chm_is_only_advertised_for_anydoc(self):
        self.assertIn("chm", convert.ALL_FORMATS)
        self.assertEqual(
            [tool for tool, (formats, _) in convert.TOOLS.items() if "chm" in formats],
            ["anydoc"],
        )

    def test_peak_rss_platform_units(self):
        self.assertEqual(chm.peak_rss_bytes(1234, "darwin"), 1234)
        self.assertEqual(chm.peak_rss_bytes(1234, "linux"), 1234 * 1024)
        with self.assertRaises(ValueError):
            chm.peak_rss_bytes(1234, "win32")

    def test_cli_conversion_time(self):
        self.assertEqual(chm.conversion_ms("converted a book.chm in 1.25ms\n"), 1.25)
        self.assertEqual(chm.conversion_ms("converted a book.chm in 2001ms\n"), 2001)
        with self.assertRaises(ValueError):
            chm.conversion_ms("error: invalid archive\n")

    @unittest.skipUnless(sys.platform in {"darwin", "linux"}, "requires wait4 RSS")
    def test_measure_waits_for_its_own_child(self):
        script = "import sys; print('discarded'); print('converted test.chm in 1.25ms', file=sys.stderr)"
        result = chm.measure([sys.executable, "-c", script])
        self.assertEqual(result["conversion_ms"], 1.25)
        self.assertGreater(result["peak_rss_bytes"], 0)
        self.assertGreater(result["wall_ms"], 0)

    @unittest.skipUnless(sys.platform in {"darwin", "linux"}, "requires wait4 RSS")
    def test_measure_reports_failed_conversion(self):
        script = "import sys; print('error: invalid archive', file=sys.stderr); sys.exit(2)"
        with self.assertRaisesRegex(RuntimeError, "invalid archive"):
            chm.measure([sys.executable, "-c", script])


if __name__ == "__main__":
    unittest.main()
