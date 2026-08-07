using System.Text.Json;
using System.Text.Json.Serialization;

namespace Firecrawl.Anydoc.Model;

/// <summary>A parsed document: its body, its notes, and the bytes of everything
/// it embedded.</summary>
public sealed class Document
{
    public List<Block> Blocks { get; set; } = new();
    /// <summary>Footnote and endnote bodies, referenced from text by a
    /// <see cref="Inline.NoteId"/> inline.</summary>
    public List<Note> Notes { get; set; } = new();
    public List<Asset> Assets { get; set; } = new();

    internal static Document FromJson(string json) =>
        JsonSerializer.Deserialize<Document>(json, Json.Options)!;
}

/// <summary>One block-level piece of a document body.</summary>
public sealed class Block
{
    /// <summary><c>heading</c>, <c>paragraph</c>, <c>list</c>, <c>table</c>,
    /// <c>block_quote</c>, <c>code_block</c>, or <c>rule</c>.</summary>
    public string Kind { get; set; } = "";
    /// <summary>heading: 1-6.</summary>
    public int? Level { get; set; }
    /// <summary>heading: stable anchor id when the source document targets this
    /// heading (bookmark, chapter fragment, ...).</summary>
    public string? Anchor { get; set; }
    /// <summary>heading, paragraph: <see cref="Inline"/></summary>
    public List<Inline>? Content { get; set; }
    public Firecrawl.Anydoc.Model.List? List { get; set; }
    public Table? Table { get; set; }
    /// <summary>block_quote: nested blocks.</summary>
    public List<Block>? Blocks { get; set; }
    /// <summary>code_block: language hint when the source names one.</summary>
    public string? Lang { get; set; }
    /// <summary>code_block: the literal text, newlines intact.</summary>
    public string? Text { get; set; }
}

/// <summary>One span of inline content.</summary>
public sealed class Inline
{
    /// <summary><c>text</c>, <c>link</c>, <c>image</c>, <c>anchor</c> (a
    /// zero-width marker for an internal link target), <c>note_ref</c>, or
    /// <c>line_break</c>.</summary>
    public string Kind { get; set; } = "";
    /// <summary>text.</summary>
    public string? Text { get; set; }
    /// <summary>text: fully resolved character style.</summary>
    public Style? Style { get; set; }
    /// <summary>link: nested inline content.</summary>
    public List<Inline>? Content { get; set; }
    /// <summary>link.</summary>
    public LinkTarget? Target { get; set; }
    /// <summary>image: alt text, empty when the source gives none.</summary>
    public string? Alt { get; set; }
    /// <summary>image.</summary>
    public ImageSource? Source { get; set; }
    /// <summary>anchor: the anchor id.</summary>
    public string? Anchor { get; set; }
    /// <summary>note_ref: the id of the note in <see cref="Document.Notes"/>.</summary>
    public string? NoteId { get; set; }
}

/// <summary>Fully resolved character style.</summary>
public sealed class Style
{
    public bool Bold { get; set; }
    public bool Italic { get; set; }
    public bool Strike { get; set; }
    public bool Code { get; set; }
}

/// <summary>Where a link points.</summary>
public sealed class LinkTarget
{
    /// <summary><c>external</c> (absolute URL with a scheme), <c>relative</c>
    /// (scheme-less relative reference), or <c>anchor</c> (internal target: a
    /// heading anchor or an <c>anchor</c> inline).</summary>
    public string Kind { get; set; } = "";
    /// <summary>The URL, relative reference, or anchor id.</summary>
    public string Value { get; set; } = "";
}

/// <summary>Where an image's bytes live.</summary>
public sealed class ImageSource
{
    /// <summary><c>external</c> (absolute URL), <c>asset</c> (embedded image,
    /// carried in <see cref="Document.Assets"/>), or <c>unavailable</c> (no
    /// usable source: only the alt text remains).</summary>
    public string Kind { get; set; } = "";
    /// <summary>external.</summary>
    public string? Url { get; set; }
    /// <summary>asset: index into <see cref="Document.Assets"/>.</summary>
    public int? AssetId { get; set; }
}

/// <summary>The marker family a list uses in the source document.</summary>
public sealed class List
{
    /// <summary><c>bullet</c>, <c>decimal</c>, <c>lower_alpha</c>,
    /// <c>upper_alpha</c>, <c>lower_roman</c>, or <c>upper_roman</c>.</summary>
    public string Marker { get; set; } = "";
    /// <summary>Ordinal the first item counts from.</summary>
    public ulong Start { get; set; }
    public List<ListItem> Items { get; set; } = new();
}

/// <summary>One item of a <see cref="List"/>, which may hold nested blocks
/// including further lists.</summary>
public sealed class ListItem
{
    public List<Block> Blocks { get; set; } = new();
    /// <summary>Task-list state, when the item carries a checkbox.</summary>
    public bool? Checked { get; set; }
    /// <summary>Literal marker text that overrides the list marker when the
    /// source number text cannot be reproduced (composite number text such as
    /// <c>1-a)</c>).</summary>
    public string? MarkerLabel { get; set; }
}

/// <summary>Canonical table grid: every logical grid position appears exactly
/// once. Content and spans live on the origin slot, and each position a span
/// covers holds a <c>covered</c> slot pointing back at that origin.</summary>
public sealed class Table
{
    public List<List<CellSlot>> Grid { get; set; } = new();
    /// <summary>Number of leading rows that are header rows (0 = no header).</summary>
    public int HeaderRows { get; set; }
    /// <summary><c>data</c> (a real data table) or <c>layout</c> (layout
    /// scaffolding: text boxes, positioning tables).</summary>
    public string Kind { get; set; } = "";
}

/// <summary>One position in a <see cref="Table.Grid"/>: either a cell or the
/// shadow of one.</summary>
public sealed class CellSlot
{
    /// <summary><c>origin</c> or <c>covered</c>.</summary>
    public string Kind { get; set; } = "";
    /// <summary>origin.</summary>
    public Cell? Cell { get; set; }
    /// <summary>covered: row of the origin this position belongs to.</summary>
    public int? OriginRow { get; set; }
    /// <summary>covered: column of the origin this position belongs to.</summary>
    public int? OriginCol { get; set; }
}

/// <summary>A table cell and the extent it spans.</summary>
public sealed class Cell
{
    public List<Block> Blocks { get; set; } = new();
    /// <summary>Columns covered, at least 1.</summary>
    public uint ColSpan { get; set; }
    /// <summary>Rows covered, at least 1.</summary>
    public uint RowSpan { get; set; }
}

/// <summary>Footnote or endnote body, referenced from text by an
/// <see cref="Inline.NoteId"/>. <c>id</c> is document-scoped.</summary>
public sealed class Note
{
    public string Id { get; set; } = "";
    /// <summary><c>footnote</c> or <c>endnote</c>.</summary>
    public string Kind { get; set; } = "";
    public List<Block> Blocks { get; set; } = new();
}

/// <summary>An embedded binary asset (image, object payload). Bytes are always
/// retained, so a document stays self-contained.</summary>
public sealed class Asset
{
    /// <summary>Index into <see cref="Document.Assets"/>, as referenced by an
    /// image source.</summary>
    public int Id { get; set; }
    /// <summary>MIME type, e.g. <c>image/png</c>.</summary>
    public string MediaType { get; set; } = "";
    /// <summary>Package part or stream the asset came from, for provenance.</summary>
    public string OriginPart { get; set; } = "";
    /// <summary>The payload, exactly as stored in the source.</summary>
    public byte[] Data { get; set; } = Array.Empty<byte>();
}

internal static class Json
{
    public static readonly JsonSerializerOptions Options = new()
    {
        // The Rust serializers emit snake_case fields (col_span, media_type, ...).
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        PropertyNameCaseInsensitive = false,
        // Optional fields are omited when unset.
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };
}