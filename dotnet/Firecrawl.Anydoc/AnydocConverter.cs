using System.Runtime.InteropServices;
using System.Text;
using Firecrawl.Anydoc.Native;
using Firecrawl.Anydoc.Model;

namespace Firecrawl.Anydoc;

/// <summary>Input format, named after the extension that identifies it.
/// Container variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map
/// onto these via <see cref="AnydocConverter.DetectFormat"/> or
/// <see cref="AnydocConverter.DetectFormatByExtension"/>.</summary>
public enum Format
{
    Doc = 0,
    Docx = 1,
    Odt = 2,
    /// <summary>Converted with pdf-inspector, which emits Markdown directly;
    /// <see cref="AnydocConverter.ToDocument"/> is unsupported for PDFs. Scanned or
    /// image-only PDFs (needing OCR) error as unsupported.</summary>
    Pdf = 3,
    Ppt = 4,
    Pptx = 5,
    Rtf = 6,
    Epub = 7,
    Excel = 8,
    Ods = 9,
    Odp = 10,
    Csv = 11,
}

/// <summary>The kind of a failed conversion, matching the stable
/// <c>error.code()</c> strings the Rust engine and the other bindings
/// publish.</summary>
public enum ConvertErrorKind
{
    /// <summary>The format is unknown, or cannot be converted at all: a scanned
    /// or image-only PDF needs OCR, which anydoc does not do.</summary>
    Unsupported,
    /// <summary>The document is structurally unusable; no meaningful content
    /// could be extracted.</summary>
    Malformed,
    /// <summary>The document is encrypted or password-protected.</summary>
    Encrypted,
    /// <summary>A fixed safety limit was crossed (decompression, nesting depth,
    /// node count, repeat expansion, or retained asset bytes).</summary>
    ResourceLimit,
    /// <summary>A part required for any meaningful output is absent.</summary>
    MissingPart,
    /// <summary>The input could not be read.</summary>
    Io,
    /// <summary>An error kind this binding version does not know yet.</summary>
    Unknown,
}

/// <summary>Thrown when meaningful conversion is impossible. <see cref="Kind"/>
/// names the failure the same way callers of the Node and Python bindings
/// branch on.</summary>
public sealed class AnydocException : Exception
{
    public AnydocException(ConvertErrorKind kind, string? message) : base(message)
    {
        Kind = kind;
        Code = kind switch
        {
            ConvertErrorKind.Unsupported => "unsupported",
            ConvertErrorKind.Malformed => "malformed",
            ConvertErrorKind.Encrypted => "encrypted",
            ConvertErrorKind.ResourceLimit => "resourceLimit",
            ConvertErrorKind.MissingPart => "missingPart",
            ConvertErrorKind.Io => "io",
            _ => "unknown",
        };
    }

    /// <summary>The kind of failure.</summary>
    public ConvertErrorKind Kind { get; }

    /// <summary>Stable, machine-readable name for the kind: what callers branch
    /// on, identical to the `code` the Node bindings put on their errors.</summary>
    public string Code { get; }

    internal static AnydocException From(string? code, string? message)
    {
        ConvertErrorKind kind = code switch
        {
            "unsupported" => ConvertErrorKind.Unsupported,
            "malformed" => ConvertErrorKind.Malformed,
            "encrypted" => ConvertErrorKind.Encrypted,
            "resourceLimit" => ConvertErrorKind.ResourceLimit,
            "missingPart" => ConvertErrorKind.MissingPart,
            "io" => ConvertErrorKind.Io,
            _ => ConvertErrorKind.Unknown,
        };
        return new AnydocException(kind, message);
    }
}

/// <summary>Converts documents to GitHub-Flavored Markdown, backed by the anydoc
/// Rust engine loaded as a native library. Safe to share across threads and
/// reuse: the underlying native calls are stateless, so instances hold no
/// process-lifetime resources and do not need to be disposed.</summary>
public sealed class AnydocConverter
{
    private const int NoFormat = -1;

    /// <summary>Detect the format from the content itself: the signature and
    /// identity each container specification designates (PDF header, RTF open
    /// group, OLE stream names, ZIP package mimetype/content types). Plain-text
    /// formats (CSV) carry no signature and return <see langword="null"/>; so
    /// does anything unrecognized.</summary>
    public unsafe Format? DetectFormat(ReadOnlySpan<byte> bytes)
    {
        unsafe
        {
            fixed (byte* p = bytes)
            {
                return FormatFromCode(AnydocNative.anydoc_format_from_bytes(p, (nuint)bytes.Length));
            }
        }
    }

    /// <summary>The format an extension names, with or without a leading
    /// dot.</summary>
    public unsafe Format? DetectFormatByExtension(string extension)
    {
        byte[] text = Encoding.UTF8.GetBytes(extension);
        unsafe
        {
            fixed (byte* p = text)
            {
                return FormatFromCode(AnydocNative.anydoc_format_from_extension(p, (nuint)text.Length));
            }
        }
    }

    /// <summary>The format a path's extension names.</summary>
    public unsafe Format? DetectFormatByPath(string path)
    {
        byte[] text = Encoding.UTF8.GetBytes(path);
        unsafe
        {
            fixed (byte* p = text)
            {
                return FormatFromCode(AnydocNative.anydoc_format_from_path(p, (nuint)text.Length));
            }
        }
    }

    /// <summary>Convert a document file to Markdown. The format is detected from
    /// the file content; the extension is the fallback for signature-less
    /// formats (CSV) and unrecognizable containers.</summary>
    /// <exception cref="AnydocException">when conversion is impossible; a file
    /// that cannot be read has <see cref="ConvertErrorKind.Io"/>.</exception>
    public unsafe string ToMarkdown(string path)
    {
        byte[] text = Encoding.UTF8.GetBytes(path);
        AnyResult result = default;
        unsafe
        {
            fixed (byte* p = text)
            {
                AnydocNative.anydoc_to_markdown_path(p, (nuint)text.Length, &result);
            }
        }
        byte[] data = TakeResult(&result);
        return Encoding.UTF8.GetString(data);
    }

    /// <summary>Asynchronous <see cref="ToMarkdown(string)"/>, offloaded to the
    /// thread pool so the calling thread is not blocked by the native
    /// conversion.</summary>
    public Task<string> ToMarkdownAsync(string path) => Task.Run(() => ToMarkdown(path));

    /// <summary>Convert an in-memory document to Markdown. Without a format, it is
    /// detected from the content, which signature-less formats (CSV) have to
    /// name explicitly.</summary>
    public string ToMarkdownBytes(ReadOnlySpan<byte> data) => ToMarkdownBytesCore(data, null);

    /// <summary>Convert an in-memory document to Markdown, naming the format
    /// explicitly.</summary>
    public string ToMarkdownBytes(ReadOnlySpan<byte> data, Format format) =>
        ToMarkdownBytesCore(data, format);

    /// <summary>Asynchronous <see cref="ToMarkdownBytes(ReadOnlySpan{byte})"/>.</summary>
    public Task<string> ToMarkdownBytesAsync(ReadOnlySpan<byte> data)
    {
        byte[] copy = data.ToArray();
        return Task.Run(() => ToMarkdownBytes(copy));
    }

    /// <summary>Asynchronous <see cref="ToMarkdownBytes(ReadOnlySpan{byte}, Format)"/>.</summary>
    public Task<string> ToMarkdownBytesAsync(ReadOnlySpan<byte> data, Format format)
    {
        byte[] copy = data.ToArray();
        return Task.Run(() => ToMarkdownBytes(copy, format));
    }

    /// <summary>Parse an in-memory document into the document model, which also
    /// carries the embedded assets. The format is detected from the content.
    /// Unsupported for <see cref="Format.Pdf"/>: PDF conversion produces
    /// Markdown directly and has no document-model form; use
    /// <see cref="ToMarkdownBytes(ReadOnlySpan{byte})"/>.</summary>
    public unsafe Document ToDocument(ReadOnlySpan<byte> bytes)
    {
        AnyResult result = default;
        unsafe
        {
            fixed (byte* p = bytes)
            {
                AnydocNative.anydoc_to_document(p, (nuint)bytes.Length, NoFormat, &result);
            }
        }
        byte[] data = TakeResult(&result);
        return Document.FromJson(Encoding.UTF8.GetString(data));
    }

    /// <summary>Asynchronous <see cref="ToDocument(ReadOnlySpan{byte})"/>.</summary>
    public Task<Document> ToDocumentAsync(ReadOnlySpan<byte> bytes)
    {
        byte[] copy = bytes.ToArray();
        return Task.Run(() => ToDocument(copy));
    }

    private static unsafe string ToMarkdownBytesCore(ReadOnlySpan<byte> data, Format? format)
    {
        AnyResult result = default;
        unsafe
        {
            fixed (byte* p = data)
            {
                AnydocNative.anydoc_to_markdown_bytes(p, (nuint)data.Length, CodeFor(format), &result);
            }
        }
        byte[] bytes = TakeResult(&result);
        return Encoding.UTF8.GetString(bytes);
    }

    private static int CodeFor(Format? format) => format is null ? NoFormat : (int)format;

    private static Format? FormatFromCode(int code) => code < 0 ? null : (Format)code;

    /// <summary>Turn a filled <see cref="AnyResult"/> into managed
    /// bytes, throwing the matching <see cref="AnydocException"/> when the
    /// conversion failed, and always releasing the Rust-side allocation.</summary>
    private static unsafe byte[] TakeResult(AnyResult* result)
    {
        try
        {
            if (!result->ok)
            {
                string? code = Marshal.PtrToStringUTF8((nint)result->error_code);
                string? message = Marshal.PtrToStringUTF8((nint)result->error_message);
                throw AnydocException.From(code, message);
            }
            byte[] data = new byte[(int)result->data.len];
            if (data.Length > 0)
            {
                Marshal.Copy((nint)result->data.ptr, data, 0, data.Length);
            }
            return data;
        }
        finally
        {
            AnydocNative.anydoc_free_result(result);
        }
    }
}
