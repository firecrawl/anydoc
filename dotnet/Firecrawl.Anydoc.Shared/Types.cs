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