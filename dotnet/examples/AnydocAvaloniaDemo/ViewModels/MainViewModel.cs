using System;
using System.IO;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Firecrawl.Anydoc;

namespace AnydocAvaloniaDemo.ViewModels;

public partial class MainViewModel : ViewModelBase
{
    private readonly AnydocConverter _converter = new();

    [ObservableProperty]
    public partial string SelectedFile { get; set; } = "";

    [ObservableProperty]
    public partial string Status { get; set; } = "Select a document to convert.";

    [ObservableProperty]
    public partial string Markdown { get; set; } = "";

    [ObservableProperty]
    public partial bool IsBusy { get; set; }

    [ObservableProperty]
    public partial bool HasError { get; set; }

    [ObservableProperty]
    public partial string? Format { get; set; }

    [ObservableProperty]
    public partial bool HasResult { get; set; }

    [RelayCommand]
    public void SetFile(string path)
    {
        SelectedFile = path;
        Status = Path.GetFileName(path);
        Markdown = "";
        HasError = false;
        HasResult = false;
        Format = null;
    }

    [RelayCommand]
    public async Task ConvertAsync()
    {
        if (string.IsNullOrWhiteSpace(SelectedFile))
        {
            return;
        }

        IsBusy = true;
        HasError = false;
        HasResult = false;
        Markdown = "";
        Status = "Converting…";

        try
        {
            byte[] bytes = await File.ReadAllBytesAsync(SelectedFile);

            Format? format = _converter.DetectFormatByExtension(Path.GetExtension(SelectedFile))
                             ?? _converter.DetectFormat(bytes);

            string markdown = format is null
                ? await _converter.ToMarkdownBytesAsync(bytes)
                : await _converter.ToMarkdownBytesAsync(bytes, format.Value);

            Format = format?.ToString();
            Markdown = markdown;
            HasResult = true;
            Status = $"Converted {bytes.Length:N0} bytes to Markdown.";
        }
        catch (AnydocException ex)
        {
            HasError = true;
            Status = $"{ex.Code}: {ex.Message}";
        }
        catch (Exception ex)
        {
            HasError = true;
            Status = ex.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }
}
