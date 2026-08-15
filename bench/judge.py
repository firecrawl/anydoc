"""Pairwise LLM judging of anydoc vs each competitor via the Anthropic
Message Batches API.

Each (doc, opponent) pair is judged twice with A/B positions swapped.
Ground truth: rendered page images from truth/ (see render_truth.py);
for EPUB, plain text extracted from the source XHTML. CSV is skipped.

Pending pairs are submitted as batches (chunked by payload size), then polled
until they end. Submitted batches journal to out/batches.jsonl and verdicts to
out/judge.jsonl, so reruns resume: open batches are harvested first and only
never-judged pairs are resubmitted.

Requires ANTHROPIC_API_KEY (read from bench/.env). Model defaults to
claude-sonnet-5 (override with JUDGE_MODEL).
"""

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import time
import zipfile
from pathlib import Path

import requests

from convert import BENCH, OUT, ROOT, TOOLS, ext


def load_dotenv():
    for envfile in (BENCH / ".env", ROOT / ".env"):
        if envfile.exists():
            for line in envfile.read_text(encoding="utf-8").splitlines():
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    k, v = line.split("=", 1)
                    os.environ.setdefault(k.strip(), v.strip().strip('"'))


load_dotenv()

API = "https://api.anthropic.com/v1/messages/batches"
MODEL = os.environ.get("JUDGE_MODEL", "claude-sonnet-5")
MAX_TOKENS = 8000
MD_LIMIT = 40_000
EPUB_TRUTH_LIMIT = 30_000

PROMPT = """You are judging two markdown conversions (A and B) of the same source document.

{truth_note}

The deliverable is GitHub-Flavored Markdown. Judge each output as Markdown:
raw HTML is not Markdown structure and counts against cleanliness. Where the
source uses something Markdown has no syntax for, there is no single right
answer: weigh what each output does on its merits.

Score each output 1-5 on:
- completeness: all source content present as markdown, nothing invented.
  Content carried only inside raw HTML does not count as present.
- structure: headings, lists, and tables match the document's actual structure
- formatting: bold/italic/links/footnotes fidelity
- cleanliness: no artifacts, garbage text, raw HTML, or broken markdown/escaping

Then pick the overall winner. The outputs and ground truth may be truncated;
judge only the content covered by the ground truth shown.

Reply with ONLY this JSON, no other text:
{{"a": {{"completeness": n, "structure": n, "formatting": n, "cleanliness": n}},
 "b": {{"completeness": n, "structure": n, "formatting": n, "cleanliness": n}},
 "winner": "A" | "B" | "tie", "reason": "one sentence"}}"""

IMAGE_TRUTH_NOTE = "Ground truth: the document's rendered pages are attached as images (first {n} pages)."
EPUB_TRUTH_NOTE = ("Ground truth: plain text extracted from the EPUB source (formatting stripped), "
                   "shown below. Judge structure leniently since the ground truth has none.\n\n"
                   "<ground-truth>\n{text}\n</ground-truth>")


def api_headers():
    key = os.environ.get("ANTHROPIC_API_KEY")
    if not key:
        sys.exit("set ANTHROPIC_API_KEY (bench/.env)")
    return {"x-api-key": key, "anthropic-version": "2023-06-01"}


def epub_truth(path: Path) -> str:
    parts, total = [], 0
    with zipfile.ZipFile(path) as z:
        names = [n for n in z.namelist() if n.lower().endswith((".xhtml", ".html", ".htm"))]
        for n in sorted(names):
            html = z.read(n).decode("utf-8", "replace")
            html = re.sub(r"(?s)<(script|style)[^>]*>.*?</\1>", " ", html)
            text = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", html)).strip()
            if text:
                parts.append(text)
                total += len(text)
            if total > EPUB_TRUTH_LIMIT:
                break
    return " ".join(parts)[:EPUB_TRUTH_LIMIT]


def clip(md: str) -> str:
    if len(md) <= MD_LIMIT:
        return md
    return md[:MD_LIMIT] + "\n\n[...truncated...]"


def build_content(task):
    stem, fmt, opponent, order = task["stem"], task["format"], task["opponent"], task["order"]
    a_tool, b_tool = ("anydoc", opponent) if order == "anydoc_a" else (opponent, "anydoc")
    md_a = (OUT / a_tool / f"{stem}.md").read_text(encoding="utf-8")
    md_b = (OUT / b_tool / f"{stem}.md").read_text(encoding="utf-8")
    content = []
    if fmt == "epub":
        truth_note = EPUB_TRUTH_NOTE.format(text=epub_truth(Path(task["sample"])))
        content.append({"type": "text", "text": PROMPT.format(truth_note=truth_note)})
    else:
        pages = sorted((BENCH / "truth" / stem).glob("page-*.png"))
        if not pages:
            return None
        content.append({"type": "text", "text": PROMPT.format(
            truth_note=IMAGE_TRUTH_NOTE.format(n=len(pages)))})
        for p in pages:
            content.append({"type": "image", "source": {
                "type": "base64", "media_type": "image/png",
                "data": base64.b64encode(p.read_bytes()).decode()}})
    content.append({"type": "text", "text": f"<output-A>\n{clip(md_a)}\n</output-A>\n\n"
                                            f"<output-B>\n{clip(md_b)}\n</output-B>"})
    return content


def parse_verdict(text) -> dict:
    if not text:
        raise ValueError("empty judge response")
    try:
        m = re.search(r"\{.*\}", text, re.S)
        v = json.loads(m.group(0))
        assert v["winner"] in ("A", "B", "tie")
        return v
    except Exception:
        # fallback for malformed JSON (usually unescaped quotes in "reason")
        w = re.search(r'"winner"\s*:\s*"(A|B|tie)"', text)
        if not w:
            raise
        v = {"winner": w.group(1), "reason": "(recovered from malformed JSON)"}
        for side in ("a", "b"):
            block = re.search(side + r'"\s*:\s*\{([^}]*)\}', text)
            v[side] = {k: int(n) for k, n in re.findall(r'"(\w+)"\s*:\s*(\d)', block.group(1))} if block else {}
        return v


def task_key(t):
    return (t["stem"], t["opponent"], t["order"])


def custom_id(t):
    return hashlib.md5("|".join(task_key(t)).encode()).hexdigest()


def verdict_record(task, v):
    a_tool, b_tool = ("anydoc", task["opponent"]) if task["order"] == "anydoc_a" else (task["opponent"], "anydoc")
    winner = "tie" if v["winner"] == "tie" else (a_tool if v["winner"] == "A" else b_tool)
    return {"stem": task["stem"], "format": task["format"], "opponent": task["opponent"],
            "order": task["order"], "winner": winner,
            "scores": {a_tool: v["a"], b_tool: v["b"]},
            "reason": v.get("reason", ""), "model": MODEL, "ts": time.time()}


class Journal:
    def __init__(self):
        self.judge_path = OUT / "judge.jsonl"
        self.batch_path = OUT / "batches.jsonl"
        self.done = set()
        if self.judge_path.exists():
            for line in self.judge_path.read_text(encoding="utf-8").splitlines():
                r = json.loads(line)
                self.done.add((r["stem"], r["opponent"], r["order"]))
        self.batches = {}
        self.harvested = set()
        if self.batch_path.exists():
            for line in self.batch_path.read_text(encoding="utf-8").splitlines():
                r = json.loads(line)
                if r.get("harvested"):
                    self.harvested.add(r["batch_id"])
                else:
                    self.batches[r["batch_id"]] = r["tasks"]

    def open_batches(self):
        return {bid: tasks for bid, tasks in self.batches.items() if bid not in self.harvested}

    def submitted_keys(self):
        keys = set()
        for tasks in self.open_batches().values():
            keys.update(task_key(t) for t in tasks.values())
        return keys

    def record_batch(self, batch_id, tasks):
        with self.batch_path.open("a", encoding="utf-8") as f:
            f.write(json.dumps({"batch_id": batch_id, "tasks": tasks, "ts": time.time()}) + "\n")
        self.batches[batch_id] = tasks

    def mark_harvested(self, batch_id):
        with self.batch_path.open("a", encoding="utf-8") as f:
            f.write(json.dumps({"batch_id": batch_id, "harvested": True, "ts": time.time()}) + "\n")
        self.harvested.add(batch_id)

    def record_verdict(self, rec):
        with self.judge_path.open("a", encoding="utf-8") as f:
            f.write(json.dumps(rec) + "\n")
        self.done.add((rec["stem"], rec["opponent"], rec["order"]))


def harvest(journal: Journal) -> int:
    """Collect results from ended batches; returns number of still-open batches."""
    still_open = 0
    for batch_id, tasks in journal.open_batches().items():
        r = requests.get(f"{API}/{batch_id}", headers=api_headers(), timeout=120)
        r.raise_for_status()
        batch = r.json()
        if batch.get("processing_status") != "ended":
            counts = batch.get("request_counts") or {}
            print(f"  {batch_id}: {batch.get('processing_status')} "
                  f"({counts.get('succeeded', 0)} ok / {counts.get('processing', 0)} pending)")
            still_open += 1
            continue
        results = requests.get(f"{API}/{batch_id}/results", headers=api_headers(), timeout=600)
        results.raise_for_status()
        ok = failed = 0
        for line in results.text.splitlines():
            item = json.loads(line)
            task = tasks.get(item["custom_id"])
            if task is None or task_key(task) in journal.done:
                continue
            result = item.get("result") or {}
            if result.get("type") != "succeeded":
                failed += 1
                print(f"  FAILED {task['stem']} vs {task['opponent']}: {result.get('type')}",
                      file=sys.stderr)
                continue
            try:
                text = "".join(b["text"] for b in result["message"]["content"]
                               if b["type"] == "text")
                journal.record_verdict(verdict_record(task, parse_verdict(text)))
                ok += 1
            except Exception as e:
                failed += 1
                print(f"  UNPARSEABLE {task['stem']} vs {task['opponent']}: {e}", file=sys.stderr)
        print(f"  {batch_id}: ended, {ok} verdicts, {failed} failed")
        journal.mark_harvested(batch_id)
    return still_open


def submit(journal: Journal, tasks, chunk_mb: float):
    budget = chunk_mb * 1e6
    chunk, chunk_bytes = {}, 0
    chunks = [chunk]
    for task in tasks:
        content = build_content(task)
        if content is None:
            continue
        req = {"custom_id": custom_id(task),
               "params": {"model": MODEL, "max_tokens": MAX_TOKENS,
                          "messages": [{"role": "user", "content": content}]}}
        size = len(json.dumps(req))
        if chunk and chunk_bytes + size > budget:
            chunk, chunk_bytes = {}, 0
            chunks.append(chunk)
        chunk[req["custom_id"]] = (req, task)
        chunk_bytes += size
    for chunk in chunks:
        if not chunk:
            continue
        r = requests.post(API, headers=api_headers(),
                          json={"requests": [req for req, _ in chunk.values()]}, timeout=600)
        if r.status_code >= 400:
            sys.exit(f"batch submit failed ({r.status_code}): {r.text[:300]}")
        batch_id = r.json()["id"]
        journal.record_batch(batch_id, {cid: task for cid, (_, task) in chunk.items()})
        print(f"submitted {batch_id}: {len(chunk)} requests")


def pending_tasks(args, journal):
    samples = {p.stem: p for p in Path(args.samples).iterdir() if p.is_file()}
    fmt_filter = {f.strip() for f in args.formats.split(",") if f.strip()}
    opponents = [o.strip() for o in args.opponents.split(",") if o.strip() in TOOLS]
    skip = journal.done | journal.submitted_keys()

    tasks, per_bucket = [], {}
    for f in sorted((OUT / "anydoc").glob("*.md")):
        stem = f.stem
        sample = samples.get(stem)
        if sample is None:
            continue
        fmt = ext(sample)
        if fmt == "csv" or (fmt_filter and fmt not in fmt_filter):
            continue
        if fmt != "epub" and not (BENCH / "truth" / stem / "page-01.png").exists():
            continue
        for opp in opponents:
            if not (OUT / opp / f"{stem}.md").exists():
                continue
            key = (fmt, opp)
            per_bucket[key] = per_bucket.get(key, 0) + 1
            if args.limit and per_bucket[key] > args.limit:
                continue
            for order in ("anydoc_a", "anydoc_b"):
                if (stem, opp, order) not in skip:
                    tasks.append({"stem": stem, "format": fmt, "opponent": opp,
                                  "order": order, "sample": str(sample)})
    return tasks


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--samples", default=str(ROOT / "samples"))
    ap.add_argument("--opponents", default="markitdown,docling,pandoc,unstructured,mammoth,libreoffice")
    ap.add_argument("--formats", default="")
    ap.add_argument("--limit", type=int, default=0, help="max docs per (format, opponent)")
    ap.add_argument("--chunk-mb", type=float, default=150.0)
    ap.add_argument("--poll-interval", type=int, default=30)
    ap.add_argument("--no-wait", action="store_true", help="submit and exit; rerun later to harvest")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    OUT.mkdir(exist_ok=True)
    journal = Journal()

    if journal.open_batches():
        print(f"checking {len(journal.open_batches())} open batch(es)...")
        harvest(journal)

    tasks = pending_tasks(args, journal)
    print(f"{len(tasks)} judge calls pending ({len(journal.done)} done, "
          f"{len(journal.submitted_keys())} in flight), model {MODEL}")
    if args.dry_run:
        return
    if tasks:
        submit(journal, tasks, args.chunk_mb)
    if args.no_wait:
        return

    while True:
        still_open = harvest(journal)
        if not still_open:
            break
        time.sleep(args.poll_interval)
    print(f"done: {len(journal.done)} verdicts in {journal.judge_path}")


if __name__ == "__main__":
    main()
