param(
    [Parameter(Mandatory = $true)]
    [string]$SourcePath,
    [Parameter(Mandatory = $true)]
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$source = [System.IO.Path]::GetFullPath($SourcePath)
$output = [System.IO.Path]::GetFullPath($OutputPath)
$extension = [System.IO.Path]::GetExtension($source).ToLowerInvariant()
$application = $null
$document = $null

try {
    switch ($extension) {
        { $_ -in '.doc', '.docx' } {
            $application = New-Object -ComObject Word.Application
            $application.Visible = $false
            $application.DisplayAlerts = 0
            $document = $application.Documents.Open($source, $false, $true)
            $document.ExportAsFixedFormat($output, 17)
            break
        }
        { $_ -in '.ppt', '.pptx' } {
            $application = New-Object -ComObject PowerPoint.Application
            $document = $application.Presentations.Open($source, $true, $false, $false)
            $document.SaveAs($output, 32)
            break
        }
        default {
            throw "Unsupported Office extension: $extension"
        }
    }
}
finally {
    if ($null -ne $document) {
        try { $document.Close() } catch { }
        [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($document)
    }
    if ($null -ne $application) {
        try { $application.Quit() } catch { }
        [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($application)
    }
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
