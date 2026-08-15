# anydoc benchmark harness

Benchmarks anydoc against well-known document-to-markdown converters on the
`samples/` corpus, for both speed and quality (deterministic metrics + LLM judge).
Office, text, and presentation documents; PDFs are out of scope.

The corpus is not redistributable and is not in the repo. The harness reads
whatever documents are in `samples/`.

## Competitors

| tool                                 | formats benchmarked                                                   |
| ------------------------------------ | --------------------------------------------------------------------- |
| markitdown                           | docx, xlsx, xls, epub, csv, pptx                                      |
| pandoc                               | docx, odt, rtf, epub, csv                                             |
| docling                              | docx, xlsx, csv, pptx                                                 |
| unstructured                         | doc, docx, odt, rtf, epub, xls, xlsx, csv, ppt, pptx                  |
| mammoth + turndown                   | docx                                                                  |
| LibreOffice headless → HTML → pandoc | doc, docx, docm, odt, rtf, xls, xlsx, xlsm, xlsb, ods, ppt, pptx, odp |

Missing tools are skipped with a warning, so a partial setup still runs.

## Setup

```
cargo build --release --example convert          # anydoc bench binary
cd bench
python -m venv .venv && .venv\Scripts\activate
pip install -r requirements.txt                  # markitdown, pymupdf, requests, markdownify
pip install -r requirements-heavy.txt            # optional: docling (~2 GB torch), unstructured
npm install                                      # mammoth pipeline
```

Also needed on PATH: `pandoc`, `node`, and LibreOffice (`soffice`; set the
`SOFFICE` env var if it's not in a standard location). Close any running
LibreOffice GUI before benchmarking: it silently swallows headless calls.

## Running

```
python convert.py --iters 1            # convert everything with every tool
python render_truth.py                 # ground-truth page images via soffice + pymupdf
python metrics.py                      # structure counts + trigram containment
python judge.py --limit 25            # pairwise LLM judging (see cost note)
python report.py                       # aggregates everything into out/report.md
```

All artifacts land in `out/` (gitignored). `convert.py` appends to
`out/timings.jsonl`; `report.py` keeps only the latest row per (tool, file).

## Methodology notes

- **Speed**: min of `--iters` warm runs, and the published numbers use `--iters 1`,
  one warm conversion per document. markitdown, docling, unstructured, and mammoth
  are timed in-process. anydoc is timed from the conversion time its own CLI reports,
  which likewise leaves out process spawn. pandoc and LibreOffice include process
  spawn (that's how they're used in practice, and it's called out in the report).
- **Quality, deterministic**: heading/table/list/link/footnote counts per output,
  plus word-trigram containment between each tool and anydoc (a cheap
  dropped-content detector, not a truth measure).
- **Quality, LLM judge**: for each doc, anydoc vs one opponent, judged twice with
  A/B positions swapped. Ground truth is the LibreOffice-rendered pages (first 6)
  attached as images; for EPUB it's text extracted from the source XHTML. CSV is
  excluded (rendering is meaningless). Position-inconsistent verdicts count as
  ties. Model: `claude-sonnet-5` via the Anthropic Message Batches API (50%
  batch discount; override with `JUDGE_MODEL`, key in `bench/.env` as
  `ANTHROPIC_API_KEY`). Verdicts journal to `out/judge.jsonl` and submitted
  batches to `out/batches.jsonl`; reruns harvest open batches first and only
  submit never-judged pairs, so it's safe to interrupt.

## Cost control

`judge.py --dry-run` prints how many judge calls would be made.
`--limit N` caps docs per (format, opponent) bucket; `--formats docx` and
`--opponents pandoc,markitdown` narrow the matrix. Each call sends up to 6 page
images and two ~40k-char markdown outputs.
