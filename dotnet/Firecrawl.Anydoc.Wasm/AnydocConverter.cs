using Firecrawl.Anydoc.Model;
using Microsoft.JSInterop;

namespace Firecrawl.Anydoc;

/// <summary>Converts documents to GitHub-Flavored Markdown inside a Blazor
/// WebAssembly app. Backed by the anydoc Rust engine compiled to WebAssembly
/// and reached through JavaScript interop, so no native platform library is
/// loaded.</summary>
///
/// <remarks>Blazor WASM has no synchronous JS interop, so every entry point is
/// async (`DetectFormatAsync`, `ToMarkdownBytesAsync`, ...). The sync-named
/// members exist for API parity with the native <c>Firecrawl.Anydoc</c> binding
/// but throw <see cref="PlatformNotSupportedException"/>; a browser has no
/// filesystem either, so path-based conversion is unsupported.</remarks>
public sealed class AnydocConverter : IAsyncDisposable
{
    private const string ModulePath = "./_content/Firecrawl.Anydoc.Wasm/anydoc-wasm.js";

    private readonly IJSRuntime _js;
    private IJSObjectReference? _module;

    /// <summary>Create a converter for the app's JS runtime. Safe to reuse
    /// across conversions; the WASM module is instantiated once, lazily.</summary>
    public AnydocConverter(IJSRuntime js) => _js = js;

    /// <summary>Detect the format from the content itself: the signature and
    /// identity each container specification designates (PDF header, RTF open
    /// group, OLE stream names, ZIP package mimetype/content types). Plain-text
    /// formats (CSV) carry no signature and return <see langword="null"/>; so
    /// does anything unrecognized.</summary>
    public async Task<Format?> DetectFormatAsync(byte[] bytes)
    {
        IJSObjectReference module = await ModuleAsync();
        string? name = await module.InvokeAsync<string?>("formatFromBytes", bytes);
        return FormatFromWasmName(name);
    }

    /// <summary>The format an extension names, with or without a leading
    /// dot.</summary>
    public async Task<Format?> DetectFormatByExtensionAsync(string extension)
    {
        IJSObjectReference module = await ModuleAsync();
        string? name = await module.InvokeAsync<string?>("formatFromExtension", extension);
        return FormatFromWasmName(name);
    }

    /// <summary>The format a path's extension names.</summary>
    public async Task<Format?> DetectFormatByPathAsync(string path)
    {
        IJSObjectReference module = await ModuleAsync();
        string? name = await module.InvokeAsync<string?>("formatFromPath", path);
        return FormatFromWasmName(name);
    }

    /// <summary>Convert an in-memory document to Markdown. Without a format, it
    /// is detected from the content, which signature-less formats (CSV) have to
    /// name explicitly.</summary>
    public async Task<string> ToMarkdownBytesAsync(byte[] data) =>
        await ToMarkdownBytesCoreAsync(data, format: null);

    /// <summary>Convert an in-memory document to Markdown, naming the format
    /// explicitly.</summary>
    public async Task<string> ToMarkdownBytesAsync(byte[] data, Format format) =>
        await ToMarkdownBytesCoreAsync(data, format);

    /// <summary>Parse an in-memory document into the document model, which also
    /// carries the embedded assets. The format is detected from the content.
    /// Unsupported for <see cref="Format.Pdf"/>: PDF conversion produces
    /// Markdown directly and has no document-model form; use
    /// <see cref="ToMarkdownBytesAsync(byte[])"/>.</summary>
    public async Task<Document> ToDocumentAsync(byte[] bytes)
    {
        IJSObjectReference module = await ModuleAsync();
        try
        {
            return await module.InvokeAsync<Document>("toDocument", bytes, (string?)null);
        }
        catch (JSException e)
        {
            throw FromJsException(e);
        }
    }

    /// <summary>Not supported in the browser: a WebAssembly host has no
    /// filesystem to read the file from. Fetch the bytes first and use
    /// <see cref="ToMarkdownBytesAsync(byte[])"/>.</summary>
    public Task<string> ToMarkdownAsync(string path) =>
        throw new PlatformNotSupportedException(
            "The WebAssembly binding has no filesystem; fetch the file's bytes and call ToMarkdownBytesAsync instead.");

    /// <inheritdoc cref="DetectFormatAsync(byte[])"/>
    /// <exception cref="PlatformNotSupportedException">always: Blazor WASM has
    /// no synchronous JS interop; use <see cref="DetectFormatAsync(byte[])"/>.</exception>
    public Format? DetectFormat(ReadOnlySpan<byte> bytes) =>
        throw SyncUnsupported();

    /// <inheritdoc cref="DetectFormatByExtensionAsync(string)"/>
    public Format? DetectFormatByExtension(string extension) =>
        throw SyncUnsupported();

    /// <inheritdoc cref="DetectFormatByPathAsync(string)"/>
    public Format? DetectFormatByPath(string path) =>
        throw SyncUnsupported();

    /// <inheritdoc cref="ToMarkdownBytesAsync(byte[])"/>
    public string ToMarkdownBytes(ReadOnlySpan<byte> data) =>
        throw SyncUnsupported();

    /// <inheritdoc cref="ToMarkdownBytesAsync(byte[], Format)"/>
    public string ToMarkdownBytes(ReadOnlySpan<byte> data, Format format) =>
        throw SyncUnsupported();

    /// <inheritdoc cref="ToDocumentAsync(byte[])"/>
    public Document ToDocument(ReadOnlySpan<byte> bytes) =>
        throw SyncUnsupported();

    private async Task<string> ToMarkdownBytesCoreAsync(byte[] data, Format? format)
    {
        IJSObjectReference module = await ModuleAsync();
        string? name = FormatToWasmName(format);
        try
        {
            return await module.InvokeAsync<string>("toMarkdownBytes", data, name);
        }
        catch (JSException e)
        {
            throw FromJsException(e);
        }
    }

    private async Task<IJSObjectReference> ModuleAsync()
    {
        if (_module is null)
        {
            _module = await _js.InvokeAsync<IJSObjectReference>("import", ModulePath);
        }
        return _module;
    }

    private static PlatformNotSupportedException SyncUnsupported() =>
        new(
            "Blazor WebAssembly has no synchronous JS interop; use the Async variant "
            + "(for example ToMarkdownBytesAsync instead of ToMarkdownBytes).");

    private static AnydocException FromJsException(JSException e)
    {
        // The shim re-throws engine errors as `anydoc:<code>:<message>`.
        const string Prefix = "anydoc:";
        string text = e.Message;
        if (text.StartsWith(Prefix, StringComparison.Ordinal))
        {
            string rest = text[Prefix.Length..];
            int colon = rest.IndexOf(':');
            if (colon > 0)
            {
                return AnydocException.From(rest[..colon], rest[(colon + 1)..]);
            }
        }
        return AnydocException.From("unknown", text);
    }

    /// <summary>The name the wasm-bindgen enum uses for a format (the JS
    /// string enum is what crosses the boundary, not the ordinal).</summary>
    private static string? FormatToWasmName(Format? format) => format switch
    {
        null => null,
        Format.Doc => "doc",
        Format.Docx => "docx",
        Format.Odt => "odt",
        Format.Pdf => "pdf",
        Format.Ppt => "ppt",
        Format.Pptx => "pptx",
        Format.Rtf => "rtf",
        Format.Epub => "epub",
        Format.Excel => "xlsx",
        Format.Ods => "ods",
        Format.Odp => "odp",
        Format.Csv => "csv",
        _ => null,
    };

    private static Format? FormatFromWasmName(string? name) => name switch
    {
        "doc" => Format.Doc,
        "docx" => Format.Docx,
        "odt" => Format.Odt,
        "pdf" => Format.Pdf,
        "ppt" => Format.Ppt,
        "pptx" => Format.Pptx,
        "rtf" => Format.Rtf,
        "epub" => Format.Epub,
        "xlsx" => Format.Excel,
        "ods" => Format.Ods,
        "odp" => Format.Odp,
        "csv" => Format.Csv,
        _ => null,
    };

    /// <summary>Release the JavaScript module reference. Idempotent; the
    /// converter is unusable afterwards.</summary>
    public async ValueTask DisposeAsync()
    {
        if (_module is not null)
        {
            await _module.DisposeAsync();
            _module = null;
        }
    }
}