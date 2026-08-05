"""Convert documents to GitHub-Flavored Markdown."""

from typing import Literal

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
    Note,
    OutlineEntry,
    ResourceLimitError,
    Style,
    Table,
    UnsupportedError,
    format_from_bytes,
    format_from_extension,
    format_from_path,
    to_document,
    to_markdown,
    to_markdown_bytes,
)

Format = Literal[
    "doc", "docx", "odt", "pdf", "ppt", "pptx", "rtf", "epub", "xlsx", "ods", "odp", "csv"
]
"""Input format, named after the extension that identifies it. Container
variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto these
via `format_from_bytes` or `format_from_extension`."""

__all__ = [
    "Asset",
    "Block",
    "Cell",
    "CellSlot",
    "ConvertError",
    "Document",
    "EncryptedError",
    "Format",
    "ImageSource",
    "Inline",
    "LinkTarget",
    "List",
    "ListItem",
    "MalformedError",
    "MissingPartError",
    "Note",
    "OutlineEntry",
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
