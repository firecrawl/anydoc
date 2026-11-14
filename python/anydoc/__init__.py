"""Convert documents to GitHub-Flavored Markdown."""

import json
import os
import urllib.error
import urllib.request
import uuid
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path
from typing import Literal, get_args

from anydoc._anydoc import (
    Asset,
    Block,
    Cell,
    CellSlot,
    ConvertError,
    Document,
    EncryptedError,
    ImageSource,
    Inline,
    LinkTarget,
    List,
    ListItem,
    MalformedError,
    MissingPartError,
    NeedsOcrError,
    Note,
    ResourceLimitError,
    Style,
    Table,
    UnsupportedError,
    format_from_bytes,
    format_from_extension,
    format_from_path,
    to_document,
)
from anydoc._anydoc import to_markdown as _to_markdown
from anydoc._anydoc import to_markdown_bytes as _to_markdown_bytes

Format = Literal[
    "doc", "docx", "odt", "pdf", "ofd", "ppt", "pptx", "rtf", "epub", "xlsx", "ods", "odp", "csv"
]
"""Input format, named after the extension that identifies it. Container
variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto these
via `format_from_bytes` or `format_from_extension`."""

Ocr = Literal["reject", "hosted"]
"""What happens when PDF or OFD pages need OCR. `reject` (the default)
raises `NeedsOcrError` naming the pages. `hosted` sends PDFs to Firecrawl
Parse, keyless unless a key is given. OFD still raises `NeedsOcrError`.
Documents anydoc converts itself never leave the machine."""

ParseMode = Literal["auto", "fast", "ocr"]
"""Firecrawl Parse PDF mode for `ocr="hosted"`. `auto` (the default) extracts
text and falls back to OCR, `fast` extracts text only, `ocr` OCRs every
page."""


class HostedError(ConvertError):
    """`ocr="hosted"` could not get the document through Firecrawl Parse."""


def to_markdown(
    path: "str | os.PathLike[str]",
    *,
    ocr: Ocr = "reject",
    parse_mode: ParseMode = "auto",
    api_key: "str | None" = None,
    api_url: "str | None" = None,
) -> str:
    """Convert a document file to Markdown. The format is detected from the
    file content; the extension is the fallback for signature-less formats
    (CSV) and unrecognizable containers.

    For `ocr="hosted"`, `parse_mode` picks the Parse PDF mode, and any other
    value raises `HostedError` before anything is sent; `api_key` falls back
    to `FIRECRAWL_API_KEY`, then keyless; `api_url` to `FIRECRAWL_API_URL`,
    then `https://api.firecrawl.dev`."""
    _check_parse_mode(parse_mode)
    try:
        return _to_markdown(path)
    except NeedsOcrError:
        if ocr != "hosted":
            raise
        path = Path(path)
        data = path.read_bytes()
        if (format_from_bytes(data) or format_from_path(path)) != "pdf":
            raise
    return _parse_hosted(data, path.name, parse_mode, api_key, api_url)


def to_markdown_bytes(
    data: "bytes | bytearray",
    format: "Format | None" = None,
    *,
    ocr: Ocr = "reject",
    parse_mode: ParseMode = "auto",
    api_key: "str | None" = None,
    api_url: "str | None" = None,
) -> str:
    """Convert an in-memory document to Markdown. Without a format, it is
    detected from the content, which signature-less formats (CSV) have to
    name explicitly. `ocr`, `parse_mode`, `api_key` and `api_url` are as
    for `to_markdown`."""
    _check_parse_mode(parse_mode)
    try:
        return _to_markdown_bytes(data, format)
    except NeedsOcrError:
        if ocr != "hosted" or (format or format_from_bytes(bytes(data))) != "pdf":
            raise
    return _parse_hosted(bytes(data), "document.pdf", parse_mode, api_key, api_url)


_API_URL = "https://api.firecrawl.dev"
_TIMEOUT_SECONDS = 300


def _check_parse_mode(parse_mode: str) -> None:
    if parse_mode not in get_args(ParseMode):
        expected = ", ".join(get_args(ParseMode))
        raise HostedError(f"invalid parse_mode {parse_mode!r}; expected one of: {expected}")


# The whole document goes, not only the pages that need OCR: Parse has no
# page selection.
def _parse_hosted(
    data: bytes, filename: str, parse_mode: str, api_key: "str | None", api_url: "str | None"
) -> str:
    if api_key is None:
        api_key = os.environ.get("FIRECRAWL_API_KEY")
    api_url = api_url or os.environ.get("FIRECRAWL_API_URL") or _API_URL
    url = api_url.rstrip("/") + "/v2/parse"
    options = {"parsers": [{"type": "pdf", "mode": parse_mode}], "origin": f"anydoc@{_version()}"}
    boundary = uuid.uuid4().hex
    request = urllib.request.Request(
        url,
        data=_multipart(boundary, json.dumps(options), filename, data),
        method="POST",
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    if api_key:
        request.add_header("Authorization", f"Bearer {api_key}")
    try:
        with urllib.request.urlopen(request, timeout=_TIMEOUT_SECONDS) as response:
            status, reply = response.status, _json(response.read())
    except urllib.error.HTTPError as error:
        status, reply = error.code, _json(error.read())
    except OSError as error:
        raise HostedError(f"Firecrawl Parse: {error}") from error
    if status != 200 or not reply.get("success"):
        detail = reply.get("error") or f"HTTP {status}"
        raise HostedError(_describe(status, detail, bool(api_key)))
    data = reply.get("data")
    markdown = data.get("markdown") if isinstance(data, dict) else None
    if not isinstance(markdown, str) or not markdown:
        raise HostedError("Firecrawl Parse returned no Markdown")
    return markdown if markdown.endswith("\n") else markdown + "\n"


def _multipart(boundary: str, options: str, filename: str, data: bytes) -> bytes:
    filename = filename.replace('"', "_").replace("\r", "_").replace("\n", "_")
    return b"".join(
        [
            f"--{boundary}\r\n".encode(),
            b'Content-Disposition: form-data; name="options"\r\n\r\n',
            options.encode(),
            f"\r\n--{boundary}\r\n".encode(),
            f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'.encode(),
            b"Content-Type: application/pdf\r\n\r\n",
            data,
            f"\r\n--{boundary}--\r\n".encode(),
        ]
    )


def _json(body: bytes) -> dict:
    try:
        reply = json.loads(body)
    except ValueError:
        return {}
    return reply if isinstance(reply, dict) else {}


def _describe(status: int, detail: str, keyed: bool) -> str:
    if status == 401:
        return f"Firecrawl Parse rejected the API key: {detail}"
    if status == 402:
        return f"Firecrawl Parse is out of credits: {detail}"
    if status == 429 and keyed:
        return f"Firecrawl Parse rate limit reached: {detail}"
    if status == 429:
        return f"Firecrawl Parse keyless limit reached, set FIRECRAWL_API_KEY: {detail}"
    return f"Firecrawl Parse: {detail}"


def _version() -> str:
    try:
        return version("firecrawl-anydoc")
    except PackageNotFoundError:
        return "unknown"


__all__ = [
    "Asset",
    "Block",
    "Cell",
    "CellSlot",
    "ConvertError",
    "Document",
    "EncryptedError",
    "Format",
    "HostedError",
    "ImageSource",
    "Inline",
    "LinkTarget",
    "List",
    "ListItem",
    "MalformedError",
    "MissingPartError",
    "NeedsOcrError",
    "Note",
    "Ocr",
    "ParseMode",
    "ResourceLimitError",
    "Style",
    "Table",
    "UnsupportedError",
    "format_from_bytes",
    "format_from_extension",
    "format_from_path",
    "to_document",
    "to_markdown",
    "to_markdown_bytes",
]
