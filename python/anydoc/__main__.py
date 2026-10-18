"""The ``anydoc`` command line: convert one document to Markdown.

Runs as ``anydoc`` (a console script) and as ``python -m anydoc``. It hands
straight off to the same Rust command the ``cargo install anydoc`` binary
runs, so every front end takes the same options and returns the same exit
codes (``0`` success, ``1`` conversion error, ``2`` usage error).
"""

import sys

from anydoc._anydoc import _cli


def main() -> int:
    """Run the command line and return its exit code."""
    return _cli(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
