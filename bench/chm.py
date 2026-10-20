"""Measure release CHM conversion time and per-process peak RSS on macOS/Linux.

Reads an external CHM without saving its converted contents. Writes one JSONL
record to stdout; see README.md for measurement boundaries.
"""

import argparse
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def peak_rss_bytes(maxrss, platform_name):
    # wait4's ru_maxrss is bytes on macOS, KiB on Linux.
    if platform_name == "darwin":
        return int(maxrss)
    if platform_name == "linux":
        return int(maxrss) * 1024
    raise ValueError("peak RSS measurement requires macOS or Linux")


def conversion_ms(stderr):
    match = re.search(r"^converted .* in (\d+(?:\.\d+)?)ms$", stderr, re.MULTILINE)
    if not match:
        raise ValueError("anydoc did not report a conversion time")
    return float(match.group(1))


def measure(command):
    # A file avoids pipe backpressure without retaining Markdown or stderr in
    # the harness's memory while the converter runs.
    with tempfile.TemporaryFile() as stderr:
        start = time.perf_counter()
        with subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=stderr) as process:
            try:
                # Reap this exact process once; Popen.wait() would discard rusage.
                _, status, usage = os.wait4(process.pid, 0)
                process.returncode = os.waitstatus_to_exitcode(status)
            except BaseException:
                process.kill()
                raise
            wall_ms = (time.perf_counter() - start) * 1000
        stderr.seek(0)
        message = stderr.read().decode("utf-8", "replace").strip()
    if process.returncode:
        raise RuntimeError(message or f"converter exited with status {process.returncode}")
    return {
        "wall_ms": wall_ms,
        "conversion_ms": conversion_ms(message),
        "peak_rss_bytes": peak_rss_bytes(usage.ru_maxrss, sys.platform),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="path to an external CHM; never copied")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/examples/convert",
                        help="release convert binary, including a saved baseline build")
    parser.add_argument("--iters", type=int, default=3)
    parser.add_argument("--warmup", type=int, default=1)
    args = parser.parse_args()
    if sys.platform not in {"darwin", "linux"} or not hasattr(os, "wait4"):
        parser.error("peak RSS measurement requires macOS or Linux")
    if args.iters < 1 or args.warmup < 0:
        parser.error("--iters must be at least 1 and --warmup must be nonnegative")
    if not args.input.is_file() or args.input.suffix.lower() != ".chm":
        parser.error("input must be an existing CHM file")
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("release binary missing; run: cargo build --release --example convert")

    command = [str(args.binary.resolve()), str(args.input.resolve())]
    try:
        for _ in range(args.warmup):
            measure(command)
        runs = [measure(command) for _ in range(args.iters)]
    except (OSError, RuntimeError, ValueError) as error:
        parser.exit(1, f"error: {error}\n")
    row = {
        "tool": "anydoc", "format": "chm", "mode": "markdown",
        "file": str(args.input.resolve()), "bytes": args.input.stat().st_size,
        "binary": str(args.binary.resolve()), "platform": platform.platform(),
        "machine": platform.machine(), "ts": time.time(),
        "warmup": args.warmup, "iters": args.iters, "runs": runs,
    }
    for key in ("wall_ms", "conversion_ms", "peak_rss_bytes"):
        row[f"median_{key}"] = statistics.median(run[key] for run in runs)
    print(json.dumps(row))


if __name__ == "__main__":
    main()
