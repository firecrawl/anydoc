using System.Text;
using Firecrawl.Anydoc.Model;
using Xunit;

namespace Firecrawl.Anydoc.Tests;

/// <summary>Smoke test: the bindings load and every entry point round-trips a
/// fixture, mirroring the Node and Python test suites.</summary>
public class AnydocTests
{
    private readonly AnydocConverter converter = new();

    private static string Fixture(params string[] path) =>
        Path.Combine(new[] { AppContext.BaseDirectory, "fixtures" }.Concat(path).ToArray());

    private static readonly string Outline = Fixture("docx", "handmade-outline.docx");
    private static readonly string Rich = Fixture("docx", "handmade-rich.docx");
    private static readonly string Csv = Fixture("csv", "sheet.csv");
    private static readonly string Encrypted = Fixture("malformed", "encrypted--errors.odt");
    private static readonly string Zipbomb = Fixture("abuse", "zipbomb--errors.docx");

    [Fact]
    public void ToMarkdown_detects_the_format_from_the_file_content()
    {
        string markdown = converter.ToMarkdown(Outline);
        Assert.Matches("(?m)^# ", markdown);
    }

    [Fact]
    public void ToMarkdownBytes_converts_in_memory_and_round_trips_unicode()
    {
        string markdown = converter.ToMarkdownBytes(File.ReadAllBytes(Rich), Format.Docx);
        Assert.Contains("| Quarter | Widgets |", markdown);
    }

    [Fact]
    public void ToMarkdownBytes_detects_the_format_when_none_is_named()
    {
        string markdown = converter.ToMarkdownBytes(File.ReadAllBytes(Rich));
        Assert.Contains("| Quarter | Widgets |", markdown);
        // CSV carries no signature, so it has to be named.
        var csvBytes = File.ReadAllBytes(Csv);
        var exception = Assert.Throws<AnydocException>(() => converter.ToMarkdownBytes(csvBytes));
        Assert.Equal(ConvertErrorKind.Unsupported, exception.Kind);
        Assert.Matches("unrecognized file content", exception.Message);
        Assert.Contains("| --- |", converter.ToMarkdownBytes(csvBytes, Format.Csv));
    }

    [Fact]
    public void ToDocument_exposes_the_document_model()
    {
        Document document = converter.ToDocument(File.ReadAllBytes(Outline));
        Block heading = document.Blocks.First(b => b.Kind == "heading");
        Assert.InRange(heading.Level!.Value, 1, 6);
        Assert.IsType<string>(heading.Content![0].Text);
        Assert.Equal("text", heading.Content[0].Kind);
        Assert.IsType<bool>(heading.Content[0].Style!.Bold);
    }

    [Fact]
    public void ToDocument_carries_embedded_assets_as_bytes()
    {
        Document document = converter.ToDocument(File.ReadAllBytes(Rich));
        Asset image = document.Assets.Single(a => a.MediaType == "image/png");
        Assert.NotEmpty(image.Data);
        Assert.Equal(image.Id, document.Assets.IndexOf(image));
    }

    [Theory]
    [InlineData(".pptm", Format.Pptx)]
    [InlineData("xls", Format.Excel)]
    public void DetectFormatByExtension_maps_container_variants(string extension, Format expected)
    {
        Assert.Equal(expected, converter.DetectFormatByExtension(extension));
    }

    [Fact]
    public void DetectFormat_reads_content_extension_and_path()
    {
        Assert.Equal(Format.Docx, converter.DetectFormat(File.ReadAllBytes(Rich)));
        // CSV carries no signature: only the extension names it.
        Assert.Null(converter.DetectFormat(File.ReadAllBytes(Csv)));
        Assert.Equal(Format.Odt, converter.DetectFormatByPath("report.odt"));
        Assert.Null(converter.DetectFormatByPath("report.unknown"));
    }

    [Fact]
    public void Conversion_errors_throw_the_kind_that_names_the_failure()
    {
        // Nothing about these bytes is a package part (Malformed).
        var malformed = Assert.Throws<AnydocException>(() =>
            converter.ToMarkdownBytes(Encoding.UTF8.GetBytes("not a document"), Format.Docx));
        Assert.Equal(ConvertErrorKind.Malformed, malformed.Kind);

        var unsupported = Assert.Throws<AnydocException>(() =>
            converter.ToMarkdownBytes(File.ReadAllBytes(Csv)));
        Assert.Equal(ConvertErrorKind.Unsupported, unsupported.Kind);

        var encrypted = Assert.Throws<AnydocException>(() =>
            converter.ToMarkdownBytes(File.ReadAllBytes(Encrypted), Format.Odt));
        Assert.Equal(ConvertErrorKind.Encrypted, encrypted.Kind);

        var limit = Assert.Throws<AnydocException>(() =>
            converter.ToMarkdownBytes(File.ReadAllBytes(Zipbomb), Format.Docx));
        Assert.Equal(ConvertErrorKind.ResourceLimit, limit.Kind);
        // The message carries the limit name, matching the Rust detail.
        Assert.Contains("max_entry_bytes", limit.Message);

        // A readable package carrying none of the parts a docx is made of.
        Assert.Equal(ConvertErrorKind.MissingPart,
            Assert.Throws<AnydocException>(() => ToMarkdownOfEmptyDocx()).Kind);
    }

    [Fact]
    public async Task Async_variants_match_the_sync_results()
    {
        byte[] outline = File.ReadAllBytes(Outline);
        byte[] rich = File.ReadAllBytes(Rich);

        Assert.Equal(converter.ToMarkdown(Outline), await converter.ToMarkdownAsync(Outline));
        Assert.Equal(converter.ToMarkdownBytes(outline), await converter.ToMarkdownBytesAsync(outline));
        Assert.Equal(
            converter.ToMarkdownBytes(rich, Format.Docx),
            await converter.ToMarkdownBytesAsync(rich, Format.Docx));

        Document document = await converter.ToDocumentAsync(outline);
        Assert.Contains(document.Blocks, b => b.Kind == "heading");
    }

    [Fact]
    public void An_unreadable_file_raises_the_io_kind()
    {
        var exception = Assert.Throws<AnydocException>(() => converter.ToMarkdown("no-such-file.docx"));
        Assert.Equal(ConvertErrorKind.Io, exception.Kind);
    }

    /// <summary>A ZIP package without the parts a docx needs -> missingPart.</summary>
    private static string ToMarkdownOfEmptyDocx()
    {
        using var package = new MemoryStream();
        using (var archive = new System.IO.Compression.ZipArchive(package, System.IO.Compression.ZipArchiveMode.Create, true))
        {
            var entry = archive.CreateEntry("[Content_Types].xml");
            using var writer = new StreamWriter(entry.Open());
            writer.Write("<Types/>");
        }
        return new AnydocConverter().ToMarkdownBytes(package.ToArray(), Format.Docx);
    }
}