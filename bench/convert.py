"""Run anydoc and competitor converters over the samples corpus.

Writes markdown to out/<tool>/<stem>.md and appends timing rows to out/timings.jsonl.
Python tools are timed in-process (warm); CLI tools include process spawn.
"""

import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent
ROOT = BENCH.parent
OUT = BENCH / "out"

ALL_FORMATS = {"doc", "docx", "docm", "odt", "rtf", "epub", "xls", "xlsx", "xlsm", "xlsb", "ods", "csv", "ppt", "pptx", "odp"}


def ext(path: Path) -> str:
    return path.suffix.lstrip(".").lower()


def decode(b: bytes) -> str:
    return b.decode("utf-8", "replace")


def timed_loop(fn, iters):
    times, result = [], None
    for _ in range(iters):
        t0 = time.perf_counter()
        result = fn()
        times.append((time.perf_counter() - t0) * 1000)
    return result, min(times), statistics.fmean(times)


# --- anydoc ---

def anydoc_bin() -> Path:
    exe = ROOT / "target" / "release" / "examples" / ("convert.exe" if os.name == "nt" else "convert")
    if not exe.exists():
        sys.exit("anydoc bench binary missing; run: cargo build --release --example convert")
    return exe


def convert_anydoc(path: Path, iters: int):
    # The CLI reports its own conversion time, so process spawn stays out of it.
    times, text = [], ""
    for _ in range(iters):
        r = subprocess.run([str(anydoc_bin()), str(path)], capture_output=True)
        if r.returncode != 0:
            raise RuntimeError(decode(r.stderr).strip())
        text = decode(r.stdout)
        reported = re.search(r" in ([\d.]+)ms", decode(r.stderr))
        if not reported:
            raise RuntimeError("anydoc did not report a conversion time")
        times.append(float(reported.group(1)))
    return text, min(times), statistics.fmean(times)


# --- markitdown ---

_markitdown = None

def convert_markitdown(path: Path, iters: int):
    global _markitdown
    from markitdown import MarkItDown
    if _markitdown is None:
        _markitdown = MarkItDown()
    return timed_loop(lambda: _markitdown.convert(str(path)).text_content, iters)


# --- docling ---

_docling = None

def convert_docling(path: Path, iters: int):
    global _docling
    from docling.document_converter import DocumentConverter
    if _docling is None:
        _docling = DocumentConverter()
    return timed_loop(lambda: _docling.convert(str(path)).document.export_to_markdown(), iters)


# --- unstructured ---
# partition.auto segfaults on Windows; dispatch to per-format partitioners.

UNSTRUCTURED_PARTITIONERS = {
    "doc": ("doc", "partition_doc"), "docx": ("docx", "partition_docx"),
    "odt": ("odt", "partition_odt"), "rtf": ("rtf", "partition_rtf"),
    "epub": ("epub", "partition_epub"), "xls": ("xlsx", "partition_xlsx"),
    "xlsx": ("xlsx", "partition_xlsx"), "csv": ("csv", "partition_csv"),
}

_unstructured_path_done = False

def _unstructured_env():
    # partition_doc shells out to soffice, partition_rtf/epub/odt to pandoc
    global _unstructured_path_done
    if _unstructured_path_done:
        return
    _unstructured_path_done = True
    pandoc = find_pandoc()
    if pandoc:
        os.environ.setdefault("PYPANDOC_PANDOC", pandoc)
        os.environ["PATH"] += os.pathsep + str(Path(pandoc).parent)
    try:
        os.environ["PATH"] += os.pathsep + str(Path(find_soffice()).parent)
    except RuntimeError:
        pass


def convert_unstructured(path: Path, iters: int):
    import importlib
    from markdownify import markdownify
    _unstructured_env()
    mod_name, fn_name = UNSTRUCTURED_PARTITIONERS[ext(path)]
    partition = getattr(importlib.import_module(f"unstructured.partition.{mod_name}"), fn_name)

    def one():
        parts = []
        for el in partition(filename=str(path)):
            cat = type(el).__name__
            text = (el.text or "").strip()
            if cat == "Title" and text:
                parts.append("## " + text)
            elif cat == "ListItem" and text:
                parts.append("- " + text)
            elif cat == "Table":
                html = getattr(el.metadata, "text_as_html", None)
                parts.append(markdownify(html).strip() if html else text)
            elif text:
                parts.append(text)
        return "\n\n".join(p for p in parts if p)

    return timed_loop(one, iters)


# --- pandoc ---

PANDOC_FROM = {"docx": "docx", "odt": "odt", "rtf": "rtf", "epub": "epub", "csv": "csv"}

def find_pandoc():
    cands = [os.environ.get("PANDOC"), "pandoc",
             os.path.expandvars(r"%LOCALAPPDATA%\Pandoc\pandoc.exe"),
             r"C:\Program Files\Pandoc\pandoc.exe"]
    for c in cands:
        if not c:
            continue
        p = shutil.which(c) or (c if Path(c).exists() else None)
        if p:
            return p
    return None


def convert_pandoc(path: Path, iters: int):
    cmd = [find_pandoc(), "-f", PANDOC_FROM[ext(path)], "-t", "gfm", "--wrap=none", str(path)]

    def one():
        r = subprocess.run(cmd, capture_output=True)
        if r.returncode != 0:
            raise RuntimeError(decode(r.stderr).strip()[:500])
        return decode(r.stdout)

    return timed_loop(one, iters)


# --- mammoth (node) ---

def convert_mammoth(path: Path, iters: int):
    tmp = BENCH / "out" / ".mammoth_tmp.md"
    r = subprocess.run(
        ["node", str(BENCH / "js" / "mammoth_convert.js"), str(path), str(tmp), str(iters)],
        capture_output=True, cwd=BENCH,
    )
    if r.returncode != 0:
        raise RuntimeError(decode(r.stderr).strip()[:500])
    min_ms, mean_ms = decode(r.stdout).strip().split("\t")
    md = tmp.read_text(encoding="utf-8")
    tmp.unlink(missing_ok=True)
    return md, float(min_ms), float(mean_ms)


# --- libreoffice (soffice -> html -> pandoc gfm) ---

def find_soffice() -> str:
    cands = [os.environ.get("SOFFICE"), "soffice",
             r"C:\Program Files\LibreOffice\program\soffice.com",
             r"C:\Program Files\LibreOffice\program\soffice.exe"]
    for c in cands:
        if not c:
            continue
        p = shutil.which(c) or (c if Path(c).exists() else None)
        if p:
            return p
    raise RuntimeError("LibreOffice not found; set SOFFICE env var")


def convert_libreoffice(path: Path, iters: int):
    soffice = find_soffice()

    def one():
        with tempfile.TemporaryDirectory() as td:
            r = subprocess.run(
                [soffice, "--headless", "--convert-to", "html", "--outdir", td, str(path)],
                capture_output=True,
            )
            html = Path(td) / (path.stem + ".html")
            if r.returncode != 0 or not html.exists():
                raise RuntimeError("soffice conversion failed: " + decode(r.stderr).strip()[:300])
            r = subprocess.run([find_pandoc(), "-f", "html", "-t", "gfm", "--wrap=none", str(html)],
                               capture_output=True)
            if r.returncode != 0:
                raise RuntimeError(decode(r.stderr).strip()[:300])
            return decode(r.stdout)

    return timed_loop(one, iters)


TOOLS = {
    "anydoc": (ALL_FORMATS, convert_anydoc),
    "markitdown": ({"docx", "xlsx", "xls", "csv", "epub", "pptx"}, convert_markitdown),
    "docling": ({"docx", "xlsx", "csv", "pptx"}, convert_docling),
    "pandoc": (set(PANDOC_FROM), convert_pandoc),
    "unstructured": ({"doc", "docx", "odt", "rtf", "epub", "xls", "xlsx", "csv", "ppt", "pptx"},
                     convert_unstructured),
    "mammoth": ({"docx"}, convert_mammoth),
    "libreoffice": (ALL_FORMATS - {"epub", "csv"}, convert_libreoffice),
}


def preflight(tools):
    missing = {}
    if "pandoc" in tools or "libreoffice" in tools:
        if not find_pandoc():
            missing.update({t: "pandoc not found" for t in ("pandoc", "libreoffice") if t in tools})
    if "mammoth" in tools:
        if not shutil.which("node"):
            missing["mammoth"] = "node not on PATH"
        elif not (BENCH / "node_modules" / "mammoth").is_dir():
            missing["mammoth"] = "run npm install in bench/"
    if "libreoffice" in tools and "libreoffice" not in missing:
        try:
            find_soffice()
        except RuntimeError as e:
            missing["libreoffice"] = str(e)
    return missing


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--samples", default=str(ROOT / "samples"))
    ap.add_argument("--tools", default=",".join(TOOLS))
    ap.add_argument("--formats", default=",".join(sorted(ALL_FORMATS)))
    ap.add_argument("--iters", type=int, default=3)
    ap.add_argument("--only", help="substring filter on file names")
    args = ap.parse_args()

    tools = [t.strip() for t in args.tools.split(",") if t.strip()]
    unknown = [t for t in tools if t not in TOOLS]
    if unknown:
        sys.exit(f"unknown tools: {unknown}; available: {list(TOOLS)}")

    for tool, missing in list(preflight(tools).items()):
        print(f"{tool:14} disabled ({missing})", file=sys.stderr)
        tools.remove(tool)
    formats = {f.strip().lower() for f in args.formats.split(",")}

    files = sorted(p for p in Path(args.samples).iterdir()
                   if p.is_file() and ext(p) in ALL_FORMATS and ext(p) in formats
                   and (not args.only or args.only in p.name))
    if not files:
        sys.exit("no matching sample files")

    OUT.mkdir(exist_ok=True)
    timings = (OUT / "timings.jsonl").open("a", encoding="utf-8")
    disabled = set()

    for tool in tools:
        (OUT / tool).mkdir(exist_ok=True)
    for path in files:
        for tool in tools:
            fmts, fn = TOOLS[tool]
            if tool in disabled or ext(path) not in fmts:
                continue
            row = {"tool": tool, "file": path.name, "stem": path.stem, "format": ext(path),
                   "bytes": path.stat().st_size, "iters": args.iters, "ts": time.time()}
            try:
                md, min_ms, mean_ms = fn(path, args.iters)
                (OUT / tool / (path.stem + ".md")).write_text(md, encoding="utf-8")
                row.update(ok=True, out_chars=len(md), min_ms=round(min_ms, 2), mean_ms=round(mean_ms, 2))
                print(f"{tool:14} {path.name:60} {min_ms:9.1f} ms")
            except ImportError as e:
                disabled.add(tool)
                print(f"{tool:14} disabled (not installed: {e})", file=sys.stderr)
                continue
            except Exception as e:
                row.update(ok=False, error=str(e)[:500])
                print(f"{tool:14} {path.name:60} ERROR {str(e)[:80]}", file=sys.stderr)
            timings.write(json.dumps(row) + "\n")
            timings.flush()
    timings.close()


if __name__ == "__main__":
    main()
