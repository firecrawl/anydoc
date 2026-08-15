"""Deterministic structure metrics over converted outputs.

For each out/<tool>/<stem>.md: counts of headings, table rows, list items,
links, footnotes, plus word-trigram containment against anydoc's output.
Writes out/metrics.json.
"""

import json
import re
from pathlib import Path

from convert import OUT, TOOLS

RE_HEADING = re.compile(r"^#{1,6}\s", re.M)
RE_TABLE_ROW = re.compile(r"^\s*\|.*\|\s*$", re.M)
RE_TABLE_SEP = re.compile(r"^\s*\|[\s:|-]+\|\s*$", re.M)
RE_LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s", re.M)
RE_LINK = re.compile(r"\[[^\]]*\]\([^)]+\)")
RE_FOOTNOTE = re.compile(r"^\[\^[^\]]+\]:", re.M)


def trigrams(text: str) -> set:
    words = re.findall(r"\w+", text.lower())
    return {tuple(words[i:i + 3]) for i in range(len(words) - 2)}


def containment(a: set, b: set) -> float:
    return len(a & b) / len(a) if a else 1.0


def counts(md: str) -> dict:
    return {
        "chars": len(md),
        "headings": len(RE_HEADING.findall(md)),
        "table_rows": len(RE_TABLE_ROW.findall(md)) - len(RE_TABLE_SEP.findall(md)),
        "list_items": len(RE_LIST_ITEM.findall(md)),
        "links": len(RE_LINK.findall(md)),
        "footnotes": len(RE_FOOTNOTE.findall(md)),
    }


def main():
    result = {}
    anydoc_grams = {}
    anydoc_dir = OUT / "anydoc"
    if anydoc_dir.is_dir():
        for f in anydoc_dir.glob("*.md"):
            anydoc_grams[f.stem] = trigrams(f.read_text(encoding="utf-8"))

    for tool in TOOLS:
        tdir = OUT / tool
        if not tdir.is_dir():
            continue
        result[tool] = {}
        for f in sorted(tdir.glob("*.md")):
            md = f.read_text(encoding="utf-8")
            row = counts(md)
            if tool != "anydoc" and f.stem in anydoc_grams:
                other = trigrams(md)
                # share of anydoc's content found here, and vice versa
                row["anydoc_recall_here"] = round(containment(anydoc_grams[f.stem], other), 3)
                row["here_recall_anydoc"] = round(containment(other, anydoc_grams[f.stem]), 3)
            result[tool][f.stem] = row

    out = OUT / "metrics.json"
    out.write_text(json.dumps(result, indent=1), encoding="utf-8")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
