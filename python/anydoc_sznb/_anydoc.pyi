# Hand-written stubs for the compiled module. `tests/test_anydoc.py` checks
# they stay in step with what the module actually exports.
import os
from typing import Literal, final

Format = Literal[
    "doc", "docx", "odt", "pdf", "ppt", "pptx", "rtf", "epub", "xlsx", "ods", "odp", "csv"
]

class ConvertError(Exception):
    """A complete conversion was impossible. Catch this to handle every kind
    of failure, or one of the subclasses below to single one out. An
    unreadable file raises `OSError` instead."""

class UnsupportedError(ConvertError):
    """The format is unknown, or cannot be converted at all."""

class NeedsOcrError(ConvertError):
    """Pages of a PDF are scanned or image-only and need OCR, which anydoc
    does not do."""

    pages: list[int]
    """1-indexed pages that need OCR."""
    page_count: int
    """Pages in the document."""

class MalformedError(ConvertError):
    """The document is structurally unusable: no meaningful content could be
    extracted."""

    part: str | None
    """The package part or stream at fault, or `None` when no single part
    is."""

class EncryptedError(ConvertError):
    """The document is encrypted or password-protected."""

class ResourceLimitError(ConvertError):
    """A fixed safety limit was crossed: decompression, nesting depth, node
    count, repeat expansion, or retained asset bytes."""

    limit: str
    """The limit that was crossed, e.g. `max_entry_bytes`."""

class MissingPartError(ConvertError):
    """A part required for any meaningful output is absent."""

    part: str
    """The part or stream that is missing."""

def format_from_bytes(data: bytes | bytearray) -> Format | None:
    """Detect the format from the content itself: the signature and identity
    each container specification designates (PDF header, RTF open group, OLE
    stream names, ZIP package mimetype/content types). Plain-text formats
    (CSV) carry no signature and return `None`; so does anything
    unrecognized."""

def format_from_extension(extension: str) -> Format | None:
    """The format an extension names, with or without a leading dot."""

def format_from_path(path: str | os.PathLike[str]) -> Format | None:
    """The format a path's extension names."""

def to_markdown(path: str | os.PathLike[str]) -> str:
    """Convert a document file to Markdown. The format is detected from the
    file content; the extension is the fallback for signature-less formats
    (CSV) and unrecognizable containers."""

def to_markdown_bytes(
    data: bytes | bytearray,
    format: Format | None = None,
    *,
    html_tables: bool = True,
    dedup_cells: bool = True,
    include_toc: bool = True,
) -> str:
    """Convert an in-memory document to Markdown. Without a format, it is
    detected from the content, which signature-less formats (CSV) have to
    name explicitly.

    Fork (sznb): `html_tables` selects the table form — HTML `<table>` with
    merged-cell colspan/rowspan (default) or GFM pipe tables. `dedup_cells`
    shows merged content once on the origin cell (default) or repeats it in
    every covered position (spans collapse). `include_toc=False` drops the
    Word table of contents (`Table of Contents` SDTs and bare `TOC` field
    results, with the 目录 title before them; docx / doc / wps only)."""

def to_text_bytes(
    data: bytes | bytearray,
    format: Format | None = None,
    *,
    dedup_cells: bool = True,
    include_toc: bool = True,
) -> str:
    """Convert an in-memory document to plain text (no Markdown syntax;
    tables render as tab-joined rows carrying merged content once, or in
    every covered cell with `dedup_cells=False`). `include_toc` as for
    `to_markdown_bytes`. Format handling as for `to_markdown_bytes`.

    Unsupported for `pdf`: PDF conversion produces Markdown directly and has
    no document-model form."""

def to_markdown_with_images(
    data: bytes | bytearray,
    format: Format | None = None,
    *,
    html_tables: bool = True,
    dedup_cells: bool = True,
    include_toc: bool = True,
    min_width: int = 50,
    min_height: int = 50,
    placeholder: str = "[IMG::{name}]",
    name_prefix: str = "img_",
) -> ImagesResult:
    """Fork (sznb): `to_markdown_bytes` plus image extraction. Embedded
    images become placeholder tokens in place (`{name}` = the file name);
    images under `min_width` x `min_height` pixels are dropped (unknown size
    is kept); names are `{name_prefix}{sha256[:16]}{ext}`, content-addressed
    so duplicates collapse. PDF yields its Markdown and no images."""

def to_text_with_images(
    data: bytes | bytearray,
    format: Format | None = None,
    *,
    dedup_cells: bool = True,
    include_toc: bool = True,
    min_width: int = 50,
    min_height: int = 50,
    placeholder: str = "[IMG::{name}]",
    name_prefix: str = "img_",
) -> ImagesResult:
    """Fork (sznb): `to_text_bytes` plus image extraction (as for
    `to_markdown_with_images`). Unsupported for `pdf`."""

@final
class ExtractedImage:
    name: str
    """Content-addressed file name, e.g. `img_3f2a9c0d1b2e4f56.png`."""
    placeholder: str
    """The token standing in for the image in the rendered text."""
    media_type: str
    width: int | None
    height: int | None
    data: bytes
    occurrences: int
    """How many places in the document reference it."""
    origin_part: str

@final
class ImagesResult:
    text: str
    images: list[ExtractedImage]
    """In order of first appearance."""
    skipped: int
    """Image references dropped for being under the size threshold."""

def to_document(data: bytes | bytearray, format: Format | None = None) -> Document:
    """Parse an in-memory document into the document model, which also
    carries the embedded assets. Without a format, it is detected from the
    content.

    Unsupported for `pdf`: PDF conversion produces Markdown directly and has
    no document-model form; use `to_markdown_bytes`."""

@final
class Document:
    blocks: list[Block]
    notes: list[Note]
    """Footnote and endnote bodies, referenced from text by a `note_ref`
    inline."""
    assets: list[Asset]

@final
class Block:
    kind: Literal["heading", "paragraph", "list", "table", "block_quote", "code_block", "rule", "math"]
    level: int | None
    """heading: 1-6."""
    anchor: str | None
    """heading: stable anchor id when the document targets this heading."""
    content: list[Inline] | None
    """heading, paragraph."""
    list: List | None
    table: Table | None
    blocks: list[Block] | None
    """block_quote."""
    lang: str | None
    """code_block."""
    text: str | None
    """code_block, math (LaTeX source without delimiters)."""

@final
class Inline:
    kind: Literal["text", "link", "image", "anchor", "note_ref", "line_break", "math", "checkbox"]
    """`anchor` is a zero-width marker for an internal link target at this
    position. `checkbox` is a checkbox control."""
    text: str | None
    style: Style | None
    """text."""
    content: list[Inline] | None
    """link."""
    target: LinkTarget | None
    """link."""
    alt: str | None
    """image."""
    source: ImageSource | None
    """image."""
    anchor: str | None
    """anchor: the anchor id."""
    note_id: str | None
    """note_ref: the id of the note in `Document.notes`."""
    checked: bool | None
    """checkbox: its state."""

@final
class Style:
    """Fully resolved character style."""

    bold: bool
    italic: bool
    strike: bool
    code: bool

@final
class LinkTarget:
    kind: Literal["external", "relative", "anchor"]
    """external: absolute URL with a scheme. relative: scheme-less relative
    reference, preserved as written. anchor: internal target, a heading
    anchor or an `anchor` inline."""
    value: str
    """The URL, relative reference, or anchor id."""

@final
class ImageSource:
    kind: Literal["external", "asset", "unavailable", "placeholder"]
    """external: absolute URL with a scheme. asset: embedded image, carried
    in `Document.assets`. unavailable: no usable source, only the alt text
    remains. placeholder (fork): extracted, `url` holds the token."""
    url: str | None
    """external; placeholder: the token."""
    asset_id: int | None
    """asset: index into `Document.assets`."""

@final
class List:
    marker: Literal["bullet", "decimal", "lower_alpha", "upper_alpha", "lower_roman", "upper_roman", "chinese_counting", "chinese_legal"]
    """The marker family the list uses in the source document."""
    start: int
    """Ordinal the first item counts from."""
    items: list[ListItem]

@final
class ListItem:
    blocks: list[Block]
    marker_label: str | None
    """Literal marker text that overrides the list marker when the source
    number text cannot be reproduced from the marker and position alone
    (composite number text such as `1-a)`)."""

@final
class Table:
    """Canonical table grid: every logical grid position appears exactly
    once. Content and spans live on the origin slot, and each position a span
    covers holds a `covered` slot pointing back at that origin."""

    grid: list[list[CellSlot]]
    header_rows: int
    """Number of leading rows that are header rows (0 = no header)."""
    kind: Literal["data", "layout"]
    """data: a real data table. layout: layout scaffolding (text boxes,
    positioning tables)."""

@final
class CellSlot:
    kind: Literal["origin", "covered"]
    cell: Cell | None
    """origin."""
    origin_row: int | None
    """covered: row of the origin this position belongs to."""
    origin_col: int | None
    """covered: column of the origin this position belongs to."""

@final
class Cell:
    blocks: list[Block]
    col_span: int
    row_span: int

@final
class Note:
    id: str
    kind: Literal["footnote", "endnote"]
    blocks: list[Block]

@final
class Asset:
    """An embedded binary asset (image, object payload). Bytes are always
    retained, so a document stays self-contained."""

    id: int
    """Index into `Document.assets`, as referenced by an image source."""
    media_type: str
    """MIME type, e.g. `image/png`."""
    origin_part: str
    """Package part or stream the asset came from, for provenance."""
    data: bytes
