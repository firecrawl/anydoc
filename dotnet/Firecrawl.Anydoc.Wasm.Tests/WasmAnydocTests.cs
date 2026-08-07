using System.Collections.Concurrent;
using Firecrawl.Anydoc;
using Firecrawl.Anydoc.Model;
using Microsoft.JSInterop;
using Xunit;

namespace Firecrawl.Anydoc.Wasm.Tests;

/// <summary>Exercises the managed Wasm binding against a fake JS runtime: a
/// browser cannot run under `dotnet test`, so the interop layer is stubbed with
/// a module returning canned results. This validates the C# glue (arg
/// marshalling, enum-name mapping, error mapping) independent of the Engine.</summary>
public class AnydocWasmTests
{
    private static readonly byte[] Docx = new byte[] { 0x50, 0x4B, 0x03 };

    [Fact]
    public async Task ToMarkdownBytesAsync_pass_bytes_and_returns_the_markdown()
    {
        var js = FakeJs.Empty();
        string? seenFormat = null;
        js.Module.On("toMarkdownBytes", args =>
        {
            seenFormat = args[1] as string;
            return "<converted>";
        });
        await using var converter = new AnydocConverter(js);

        string markdown = await converter.ToMarkdownBytesAsync(Docx, Format.Docx);

        Assert.Equal("<converted>", markdown);
        Assert.Equal("docx", seenFormat); // the enum is marshalled by its wasm name
    }

    [Fact]
    public async Task ToMarkdownBytesAsync_leaves_the_format_undefined_when_detecting()
    {
        var js = FakeJs.Empty();
        js.Module.On("toMarkdownBytes", args => "<converted>");
        await using var converter = new AnydocConverter(js);

        string markdown = await converter.ToMarkdownBytesAsync(Docx);

        Assert.Equal("<converted>", markdown);
    }

    [Fact]
    public async Task Detecting_the_format_maps_the_wasm_name_back_to_the_enum()
    {
        foreach ((string name, Format expected) in new[]
                 {
                     ("docx", Format.Docx),
                     ("odt", Format.Odt),
                     ("pdf", Format.Pdf),
                     ("xlsx", Format.Excel),
                     ("csv", Format.Csv),
                 })
        {
            var js = FakeJs.Returning("formatFromBytes", name);
            await using var converter = new AnydocConverter(js);
            Assert.Equal(expected, await converter.DetectFormatAsync(Docx));
        }
    }

    [Fact]
    public async Task Detect_the_format_returns_null_when_nothing_matches()
    {
        var js = FakeJs.Returning("formatFromBytes", null);
        await using var converter = new AnydocConverter(js);
        Assert.Null(await converter.DetectFormatAsync(Docx));
    }

    [Fact]
    public async Task Detecting_by_extension_and_path_use_their_own_entry_points()
    {
        var ext = FakeJs.Returning("formatFromExtension", "xlsx");
        await using var cExt = new AnydocConverter(ext);
        Assert.Equal(Format.Excel, await cExt.DetectFormatByExtensionAsync(".xlsx"));

        var path = FakeJs.Returning("formatFromPath", "pdf");
        await using var cPath = new AnydocConverter(path);
        Assert.Equal(Format.Pdf, await cPath.DetectFormatByPathAsync("report.pdf"));
    }

    [Fact]
    public async Task ToDocumentAsync_returns_the_deserialized_model()
    {
        var js = FakeJs.Returning("toDocument", new Document
        {
            Blocks =
            {
                new Block
                {
                    Kind = "heading",
                    Level = 1,
                    Content = new List<Inline> { new Inline { Kind = "text" } },
                },
            },
        });
        await using var converter = new AnydocConverter(js);

        Document document = await converter.ToDocumentAsync(Docx);

        Assert.Equal("heading", Assert.Single(document.Blocks).Kind);
        Assert.Equal("text", Assert.Single(Assert.Single(document.Blocks).Content!).Kind);
    }

    [Fact]
    public async Task A_js_engine_error_becomes_an_AnydocException_with_the_right_kind()
    {
        var js = FakeJs.Throwing("toMarkdownBytes", new JSException("anydoc:encrypted:the file is password-protected"));
        await using var converter = new AnydocConverter(js);

        var exception = await Assert.ThrowsAsync<AnydocException>(() => converter.ToMarkdownBytesAsync(Docx));

        Assert.Equal(ConvertErrorKind.Encrypted, exception.Kind);
        Assert.Equal("encrypted", exception.Code);
    }

    [Fact]
    public async Task A_js_error_with_a_future_code_maps_to_unknown()
    {
        var js = FakeJs.Throwing("toMarkdownBytes", new JSException("anydoc:negotiator hatchback:surprise!"));
        await using var converter = new AnydocConverter(js);

        var exception = await Assert.ThrowsAsync<AnydocException>(() => converter.ToMarkdownBytesAsync(Docx));

        Assert.Equal(ConvertErrorKind.Unknown, exception.Kind);
    }

    [Fact]
    public void The_sync_members_require_the_async_variant()
    {
        var js = FakeJs.Empty();
        var converter = new AnydocConverter(js);

        foreach (Action act in new Action[]
                 {
                     () => converter.DetectFormat(Docx),
                     () => converter.DetectFormatByExtension(".docx"),
                     () => converter.DetectFormatByPath("a.docx"),
                     () => converter.ToMarkdownBytes(Docx),
                     () => converter.ToMarkdownBytes(Docx, Format.Docx),
                     () => converter.ToDocument(Docx),
                 })
        {
            Assert.Throws<PlatformNotSupportedException>(act);
        }
    }

    [Fact]
    public async Task Path_conversion_is_unsupported_and_its_message_points_at_the_byte_api()
    {
        await using var converter = new AnydocConverter(FakeJs.Empty());

        var exception = await Assert.ThrowsAsync<PlatformNotSupportedException>(
            () => converter.ToMarkdownAsync("a.docx"));

        Assert.Contains("ToMarkdownBytesAsync", exception.Message);
    }

    [Fact]
    public async Task Reusing_one_converter_reuses_a_single_imported_module()
    {
        var js = FakeJs.Empty();
        js.Module.On("toMarkdownBytes", args => "<converted>");
        await using var converter = new AnydocConverter(js);

        await converter.ToMarkdownBytesAsync(Docx);
        await converter.ToMarkdownBytesAsync(Docx);

        Assert.Equal(1, js.ImportCount);
    }
}

/// <summary>A minimal IJSRuntime that returns a hand-driven module for the
/// `import` call and routes every module invocation to a registered handler.</summary>
internal sealed class FakeJs : IJSRuntime
{
    public FakeModule Module { get; } = new();
    public int ImportCount { get; private set; }

    public ValueTask<TValue> InvokeAsync<TValue>(string identifier, object?[]? args) =>
        InvokeAsync<TValue>(identifier, CancellationToken.None, args);

    public ValueTask<TValue> InvokeAsync<TValue>(
        string identifier, CancellationToken cancellationToken, object?[]? args)
    {
        if (identifier == "import")
        {
            ImportCount++;
            var task = Task.FromResult<TValue>((TValue)(object)Module);
            return new ValueTask<TValue>(task);
        }
        throw new InvalidOperationException($"unexpected js identifier {identifier}");
    }

    public static FakeJs Empty() => new();

    public static FakeJs Returning(string identifier, object? result)
    {
        var js = new FakeJs();
        js.Module.On(identifier, _ => result);
        return js;
    }

    public static FakeJs Throwing(string identifier, JSException exception)
    {
        var js = new FakeJs();
        js.Module.ThrowOn(identifier, exception);
        return js;
    }
}

internal sealed class FakeModule : IJSObjectReference
{
    private readonly ConcurrentDictionary<string, Func<object?[], object?>> _handlers = new();
    private readonly ConcurrentDictionary<string, JSException> _throwing = new();

    public void On(string identifier, Func<object?[], object?> handler) => _handlers[identifier] = handler;
    public void ThrowOn(string identifier, JSException exception) => _throwing[identifier] = exception;

    public ValueTask<TValue> InvokeAsync<TValue>(string identifier, object?[]? args) =>
        InvokeAsync<TValue>(identifier, CancellationToken.None, args);

    public ValueTask<TValue> InvokeAsync<TValue>(
        string identifier, CancellationToken cancellationToken, object?[]? args)
    {
        try
        {
            if (_throwing.TryGetValue(identifier, out JSException? thrown))
            {
                throw thrown;
            }
            if (_handlers.TryGetValue(identifier, out var handler))
            {
                object? result = handler(args ?? Array.Empty<object?>());
                var task = Task.FromResult<TValue>((TValue)result!);
                return new ValueTask<TValue>(task);
            }
        }
        catch (JSException)
        {
            throw;
        }
        throw new InvalidOperationException($"no handler registered for {identifier}");
    }

    public ValueTask DisposeAsync() => ValueTask.CompletedTask;
}