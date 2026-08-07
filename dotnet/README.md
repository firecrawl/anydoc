# Firecrawl.Anydoc

.NET bindings for **anydoc**, the fast Rust library that converts documents
(Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF) into clean
GitHub-Flavored Markdown. This package bundles the native engine for every
supported platform, so it installs and runs with no extra setup.

```
dotnet add package Firecrawl.Anydoc
```

Supported platforms: **macOS, Windows, and Linux** on both **x64** and **arm64**.

## Usage

```csharp
using Firecrawl.Anydoc;

// Construct once and reuse for every conversion.
// Instances are thread-safe and hold no disposable state.
var anydoc = new AnydocConverter();

// From a file path (format detected from the content):
string markdown = await anydoc.ToMarkdownAsync("report.docx");

// From bytes, with the format detected from the content:
string fromBytes = await anydoc.ToMarkdownBytesAsync(bytes);

// Or name it, which signature-less formats (CSV) need:
string fromCsv = await anydoc.ToMarkdownBytesAsync(bytes, Format.Csv);

// Or stop at the document model, which also carries embedded assets:
Document document = await anydoc.ToDocumentAsync(bytes);
```

Synchronous variants (`ToMarkdown`, `ToMarkdownBytes`, `ToDocument`) behave the
same way when you are not in an `async` context.

### Format detection

```csharp
Format? byBytes  = anydoc.DetectFormat(bytes);              // content signature
Format? byExt    = anydoc.DetectFormatByExtension(".pptm"); // => Format.Pptx
Format? byPath   = anydoc.DetectFormatByPath("sheet.csv");  // => Format.Csv
```

### Errors

Failures throw `AnydocException`, whose `Kind` (a `ConvertErrorKind`) names the
reason the same way the Node and Python bindings do:

```csharp
try
{
    string md = anydoc.ToMarkdown(bytes, Format.Docx);
}
catch (AnydocException e)
{
    switch (e.Kind)
    {
        case ConvertErrorKind.Unsupported:  // scanned PDF, unknown format
        case ConvertErrorKind.Malformed:    // structurally unusable document
        case ConvertErrorKind.Encrypted:    // password-protected
        case ConvertErrorKind.ResourceLimit:// a fixed safety limit was crossed
        case ConvertErrorKind.MissingPart:  // a required part is absent
        case ConvertErrorKind.Io:           // the file could not be read
    }
}
```

The document model (`Firecrawl.Anydoc.Model`) encodes every block, inline,
list, table grid, note, and embedded asset, mirroring the Node and Python
bindings exactly (`Kind` string discriminants, `NoteId`, `MediaType`, etc.).

## Building

The C# wrapper is generated from the Rust FFI with
[csbindgen](https://github.com/Cysharp/csbindgen): `cargo build -p anydoc-dotnet`
regenerates `src/NativeMethods.g.cs`.

To rebuild the native binaries and produce the NuGet package:

```sh
sh dotnet/build.sh            # build the host's native library and pack
sh dotnet/build.sh --all      # build every supported platform's library
```

Each architecture's library is laid out under
`src/runtimes/{rid}/native/` — NuGet's native-library convention — so a single
package carries all six targets. `dotnet pack` bundles them with the managed
wrapper.

Run the tests with:

```sh
dotnet test dotnet/Firecrawl.Anydoc.slnx
```