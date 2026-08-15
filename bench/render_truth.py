"""Render ground-truth page images for the LLM judge.

soffice converts each sample to PDF, PyMuPDF rasterizes the first pages to
truth/<stem>/page-NN.png. EPUB and CSV are handled textually by judge.py.
"""

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

import fitz  # pymupdf

from convert import BENCH, ROOT, decode, ext, find_soffice

RENDERABLE = {"doc", "docx", "docm", "odt", "rtf", "xls", "xlsx", "xlsm", "xlsb", "ods",
              "ppt", "pptx", "odp"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--samples", default=str(ROOT / "samples"))
    ap.add_argument("--max-pages", type=int, default=6)
    ap.add_argument("--dpi", type=int, default=100)
    ap.add_argument("--only", help="substring filter on file names")
    args = ap.parse_args()

    soffice = find_soffice()
    files = sorted(p for p in Path(args.samples).iterdir()
                   if p.is_file() and ext(p) in RENDERABLE
                   and (not args.only or args.only in p.name))

    for path in files:
        outdir = BENCH / "truth" / path.stem
        if (outdir / "page-01.png").exists():
            continue
        with tempfile.TemporaryDirectory() as td:
            r = subprocess.run([soffice, "--headless", "--convert-to", "pdf", "--outdir", td, str(path)],
                               capture_output=True)
            pdf = Path(td) / (path.stem + ".pdf")
            if r.returncode != 0 or not pdf.exists():
                print(f"SKIP {path.name}: soffice failed: {decode(r.stderr).strip()[:200]}", file=sys.stderr)
                continue
            outdir.mkdir(parents=True, exist_ok=True)
            doc = fitz.open(pdf)
            n = min(len(doc), args.max_pages)
            for i in range(n):
                pix = doc[i].get_pixmap(dpi=args.dpi)
                pix.save(outdir / f"page-{i + 1:02}.png")
            doc.close()
            print(f"{path.name}: {n} page(s)")


if __name__ == "__main__":
    main()
