# /// script
# requires-python = ">=3.10"
# dependencies = ["docling-core>=2.106.0,<3.0.0"]
# ///
"""Regenerate the Docling groundtruth exports under tests/fixtures/doclang.

Reads serialized ``DoclingDocument`` JSONs from a Docling checkout
(``tests/data/pdf/groundtruth/*.json``) and writes the DocLang export of
each into ``tests/fixtures/doclang/<name>.doclang``. The test in
``tests/doclang_docling.rs`` pairs these with the source PDFs from the same
checkout.

Run with ``uv run scripts/export_docling_doclang.py`` from the repo root.
Set ``DOCLING_PDF_TESTDATA`` to point at a non-default Docling checkout.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from docling_core.types.doc.document import DoclingDocument

REPO_ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--groundtruth",
        type=Path,
        default=None,
        help="Docling groundtruth dir (default: $DOCLING_PDF_TESTDATA/"
        "groundtruth or ../docling/tests/data/pdf/groundtruth).",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=REPO_ROOT / "tests" / "fixtures" / "doclang",
        help="Output directory for the .doclang files.",
    )
    args = parser.parse_args()

    groundtruth = args.groundtruth
    if groundtruth is None:
        env = os.environ.get("DOCLING_PDF_TESTDATA")
        groundtruth = (
            Path(env) / "groundtruth"
            if env
            else REPO_ROOT.parent / "docling" / "tests" / "data" / "pdf" / "groundtruth"
        )
    if not groundtruth.is_dir():
        print(f"error: no groundtruth dir at {groundtruth}", file=sys.stderr)
        return 1

    args.out.mkdir(parents=True, exist_ok=True)
    failures: list[str] = []
    for json_path in sorted(groundtruth.glob("*.json")):
        if json_path.name.endswith(".pages.meta.json"):
            continue
        name = json_path.stem
        try:
            doc = DoclingDocument.model_validate_json(json_path.read_text())
            xml = doc.export_to_doclang()
            (args.out / f"{name}.doclang").write_text(xml, encoding="utf-8")
            print(f"OK   {name}: {len(xml)} chars")
        except Exception as exc:  # noqa: BLE001
            failures.append(name)
            print(f"FAIL {name}: {exc}")

    print(f"\n{len(failures)} failures" if failures else "\nall exported")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
