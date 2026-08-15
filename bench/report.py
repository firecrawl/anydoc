"""Aggregate timings, metrics, and judge verdicts into out/report.md."""

import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

from convert import OUT


def load_timings():
    latest = {}
    path = OUT / "timings.jsonl"
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            r = json.loads(line)
            latest[(r["tool"], r["stem"])] = r
    return list(latest.values())


def speed_section(rows):
    lines = ["## Speed", "",
             "Warm timings; Python tools in-process, CLI tools include process spawn.", ""]
    by_fmt = defaultdict(lambda: defaultdict(list))
    for r in rows:
        if r.get("ok"):
            by_fmt[r["format"]][r["tool"]].append(r)
    for fmt in sorted(by_fmt):
        lines += [f"### {fmt}", "", "| tool | files | total MB | median ms | MB/s |", "| --- | --- | --- | --- | --- |"]
        for tool, rs in sorted(by_fmt[fmt].items(), key=lambda kv: statistics.median(x["min_ms"] for x in kv[1])):
            total_mb = sum(x["bytes"] for x in rs) / 1e6
            total_s = sum(x["min_ms"] for x in rs) / 1000
            med = statistics.median(x["min_ms"] for x in rs)
            mbs = total_mb / total_s if total_s else 0
            lines.append(f"| {tool} | {len(rs)} | {total_mb:.1f} | {med:.1f} | {mbs:.1f} |")
        lines.append("")
    fails = [r for r in rows if not r.get("ok")]
    if fails:
        lines += ["### Failures", ""]
        by_tool = defaultdict(int)
        for r in fails:
            by_tool[r["tool"]] += 1
        for tool, n in sorted(by_tool.items()):
            lines.append(f"- {tool}: {n} file(s) failed")
        lines.append("")
    return lines


def judge_section():
    path = OUT / "judge.jsonl"
    if not path.exists():
        return []
    verdicts = defaultdict(dict)
    for line in path.read_text(encoding="utf-8").splitlines():
        r = json.loads(line)
        verdicts[(r["stem"], r["format"], r["opponent"])][r["order"]] = r["winner"]

    tallies = defaultdict(lambda: [0, 0, 0])  # win, tie, loss for anydoc
    for (stem, fmt, opp), orders in verdicts.items():
        winners = set(orders.values())
        if len(orders) < 2 or len(winners) > 1:
            outcome = 1  # inconsistent across positions, or single verdict: tie
            if len(orders) < 2:
                w = next(iter(winners))
                outcome = 0 if w == "anydoc" else (1 if w == "tie" else 2)
        else:
            w = winners.pop()
            outcome = 0 if w == "anydoc" else (1 if w == "tie" else 2)
        tallies[(fmt, opp)][outcome] += 1

    lines = ["## LLM judge (anydoc vs opponent)", "",
             "Each doc judged twice with positions swapped; disagreement counts as a tie.", "",
             "| format | opponent | anydoc wins | ties | losses | win rate (excl. ties) |",
             "| --- | --- | --- | --- | --- | --- |"]
    for (fmt, opp), (w, t, l) in sorted(tallies.items()):
        rate = f"{w / (w + l):.0%}" if (w + l) else "-"
        lines.append(f"| {fmt} | {opp} | {w} | {t} | {l} | {rate} |")
    lines.append("")
    return lines


DIMENSIONS = ["completeness", "structure", "formatting", "cleanliness"]


def load_scores():
    """Per-tool rubric scores from every verdict the tool appears in."""
    path = OUT / "judge.jsonl"
    if not path.exists():
        return {}
    scores = defaultdict(lambda: defaultdict(list))  # tool -> dimension -> [1-5]
    docs = defaultdict(set)
    by_format = defaultdict(list)  # (format, tool) -> [mean 1-5]
    for line in path.read_text(encoding="utf-8").splitlines():
        r = json.loads(line)
        pair = r.get("scores") or {}
        if not all(all(d in dims for d in DIMENSIONS) for dims in pair.values()):
            continue
        for tool, dims in pair.items():
            for d in DIMENSIONS:
                scores[tool][d].append(dims[d])
            docs[tool].add(r["stem"])
            by_format[(r["format"], tool)].append(statistics.fmean(dims[d] for d in DIMENSIONS))
    return {"scores": scores, "docs": docs, "by_format": by_format}


def pct(values):
    """Mean of 1-5 rubric scores as a percentage of the maximum."""
    return statistics.fmean(values) / 5 * 100


def mark(text, winning):
    return f"**{text}**" if winning else text


def median_ms(rows):
    by_tool = defaultdict(list)
    for r in rows:
        if r.get("ok"):
            by_tool[r["tool"]].append(r["min_ms"])
    return {tool: statistics.median(v) for tool, v in by_tool.items()}


def coverage(rows):
    """Formats each tool converted at least one file of, and the corpus total."""
    covered = defaultdict(set)
    corpus = set()
    for r in rows:
        corpus.add(r["format"])
        if r.get("ok"):
            covered[r["tool"]].add(r["format"])
    return covered, corpus


def score_section(rows):
    data = load_scores()
    if not data:
        return []
    scores, docs, by_format = data["scores"], data["docs"], data["by_format"]
    speed = median_ms(rows)
    covered, corpus = coverage(rows)

    # Mean of a tool's per-format scores, so a corpus with many easy documents
    # in one format cannot skew the number.
    def macro(tool):
        per_format = [pct(v) for (f, t), v in by_format.items() if t == tool]
        return statistics.fmean(per_format) if per_format else 0.0

    ranked = sorted(scores, key=lambda t: (len(covered[t]), macro(t)), reverse=True)
    columns = ["score"] + DIMENSIONS
    values = {tool: {"score": macro(tool), **{d: pct(scores[tool][d]) for d in DIMENSIONS}}
              for tool in ranked}
    best = {c: max(values[t][c] for t in ranked) for c in columns}
    most = max(len(covered[t]) for t in ranked)
    fastest = min(speed.get(t, float("inf")) for t in ranked)

    lines = ["## Coverage, speed, and judge scores", "",
             "Scores are 0-100, the mean of the judge's 1-5 rubric; both outputs of a",
             "pair are scored blind against the same ground truth. `score` and the",
             "dimensions cover only the formats a tool supports, so a tool that reads",
             "one format is not comparable to one that reads them all.", "",
             "| tool | formats | median ms | docs judged | " + " | ".join(columns) + " |",
             "| --- | --- | --- | --- |" + " --- |" * len(columns)]
    for tool in ranked:
        ms = speed.get(tool)
        ms_cell = "-" if ms is None else mark(f"{ms:.1f}", ms == fastest)
        n = len(covered[tool])
        cells = [mark(f"{values[tool][c]:.0f}", values[tool][c] == best[c]) for c in columns]
        lines.append(f"| {tool} | {mark(f'{n}/{len(corpus)}', n == most)} | {ms_cell} | "
                     f"{len(docs[tool])} | " + " | ".join(cells) + " |")
    lines.append("")

    formats = sorted({f for f, _ in by_format})
    lines += ["### Overall score by format", "",
              "| format | " + " | ".join(ranked) + " |",
              "| --- |" + " --- |" * len(ranked)]
    for fmt in formats:
        row = [(fmt, tool, by_format.get((fmt, tool))) for tool in ranked]
        top = max((pct(v) for _, _, v in row if v), default=None)
        cells = []
        for _, _, vals in row:
            if not vals:
                cells.append("-")
                continue
            score = pct(vals)
            cells.append(mark(f"{score:.0f}", score == top))
        lines.append(f"| {fmt} | " + " | ".join(cells) + " |")
    lines.append("")
    return lines


def metrics_section():
    path = OUT / "metrics.json"
    if not path.exists():
        return []
    data = json.loads(path.read_text(encoding="utf-8"))
    lines = ["## Content recall vs anydoc (word-trigram containment)", "",
             "| tool | docs | anydoc content found in tool | tool content found in anydoc |",
             "| --- | --- | --- | --- |"]
    for tool, docs in sorted(data.items()):
        pairs = [(d["anydoc_recall_here"], d["here_recall_anydoc"])
                 for d in docs.values() if "anydoc_recall_here" in d]
        if not pairs:
            continue
        a = statistics.fmean(p[0] for p in pairs)
        b = statistics.fmean(p[1] for p in pairs)
        lines.append(f"| {tool} | {len(pairs)} | {a:.2f} | {b:.2f} |")
    lines.append("")
    return lines


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    rows = load_timings()
    lines = ["# anydoc benchmark report", ""]
    lines += speed_section(rows)
    lines += score_section(rows)
    lines += judge_section()
    lines += metrics_section()
    report = "\n".join(lines)
    (OUT / "report.md").write_text(report, encoding="utf-8")
    print(report)


if __name__ == "__main__":
    main()
