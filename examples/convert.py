# Convert a document to Markdown:
# `python examples/convert.py <file> [-f csv] [-o out.md] [--assets dir]`
#
# Run `pip install maturin && maturin develop` in `python/` first, or
# `pip install firecrawl-anydoc` instead of the local build.
import argparse
import re
import sys
import time
from pathlib import Path

import anydoc


def millis(ms: float) -> str:
    return f"{ms:.2f}ms" if ms < 10 else f"{ms:.0f}ms"


parser = argparse.ArgumentParser(prog="convert", add_help=False)
parser.add_argument("file")
parser.add_argument("-f", "--format")
parser.add_argument("-o", "--output")
parser.add_argument("--assets")
args = parser.parse_args()

try:
    data = Path(args.file).read_bytes()
    # Without -f the format comes from the file content, with the extension
    # as the fallback.
    format = args.format or anydoc.format_from_bytes(data) or anydoc.format_from_path(args.file)
    if not format:
        raise ValueError(f"unrecognized file content and extension: {args.file}")

    start = time.perf_counter()
    markdown = anydoc.to_markdown_bytes(data, format)
    elapsed = (time.perf_counter() - start) * 1000
    print(f"converted {args.file} in {millis(elapsed)}", file=sys.stderr)

    if args.output:
        Path(args.output).write_text(markdown, encoding="utf-8")
    else:
        sys.stdout.write(markdown)

    # Images and embedded objects live on the document model, not in the
    # Markdown, so they need a second pass to write out.
    if args.assets:
        document = anydoc.to_document(data, format)
        assets = Path(args.assets)
        assets.mkdir(parents=True, exist_ok=True)
        stem = Path(args.file).stem
        for asset in document.assets:
            kind, _, subtype = asset.media_type.partition("/")
            extension = re.sub(r"[^a-z0-9]", "", subtype, flags=re.I) if kind == "image" else "bin"
            (assets / f"{stem}-{asset.id}.{extension}").write_bytes(asset.data)
        print(f"wrote {len(document.assets)} assets to {args.assets}", file=sys.stderr)
except Exception as error:
    print(f"error: {error}", file=sys.stderr)
    sys.exit(1)
