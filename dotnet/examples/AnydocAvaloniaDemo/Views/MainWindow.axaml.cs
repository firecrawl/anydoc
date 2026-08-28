using System;
using System.Threading.Tasks;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using AnydocAvaloniaDemo.ViewModels;

namespace AnydocAvaloniaDemo.Views;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
    }

    protected override async void OnOpened(EventArgs e)
    {
        base.OnOpened(e);

        if (DataContext is MainViewModel vm)
        {
            var button = this.FindControl<Button>("SelectButton");
            if (button is not null)
            {
                button.Click += async (_, _) => await PickFileAsync(vm);
            }
        }
    }

    private async Task PickFileAsync(MainViewModel vm)
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "Select a document to convert",
            AllowMultiple = false,
            FileTypeFilter = new[]
            {
                new FilePickerFileType("Documents")
                {
                    Patterns = new[]
                    {
                        "*.doc", "*.docx", "*.odt", "*.pdf", "*.ppt", "*.pptx",
                        "*.rtf", "*.epub", "*.xls", "*.xlsx", "*.ods", "*.odp", "*.csv",
                    },
                },
                FilePickerFileTypes.All,
            },
        });

        if (files.Count > 0 && files[0].TryGetLocalPath() is string path)
        {
            vm.SetFileCommand.Execute(path);
        }
    }
}
