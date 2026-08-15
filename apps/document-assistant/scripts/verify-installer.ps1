$ErrorActionPreference = 'Stop'

$appRoot = Split-Path -Parent $PSScriptRoot
$required = @(
  'src-tauri\resources\licenses\anydoc-MIT.txt',
  'src-tauri\resources\licenses\libreoffice-MPL-2.0.txt',
  'src-tauri\resources\licenses\pdfium-BSD.txt',
  'src-tauri\resources\THIRD_PARTY_NOTICES.md',
  'src-tauri\resources\pdfium.dll'
)

foreach ($relative in $required) {
  $resolved = Join-Path $appRoot $relative
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
    throw "Missing packaged notice or runtime: $relative"
  }
}

$bundleRoot = Join-Path $appRoot 'src-tauri\target\release\bundle\nsis'
$installers = @(Get-ChildItem -LiteralPath $bundleRoot -Filter '*.exe' -File -ErrorAction SilentlyContinue)
if ($installers.Count -eq 0) {
  throw "No NSIS installer found under $bundleRoot"
}

$installer = $installers | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
if ($installer.Length -lt 1MB) {
  throw "Installer is unexpectedly small: $($installer.FullName)"
}

$forbidden = @(('.' + 'env'), ('sk-' + 'test'), ('sk-regression-' + 'secret'))
$releaseExecutable = Join-Path $appRoot 'src-tauri\target\release\anydoc-assistant.exe'
$resourceRoot = Join-Path $appRoot 'src-tauri\target\release\resources'
$scanTargets = @($releaseExecutable, $installer.FullName)
if (Test-Path -LiteralPath $resourceRoot) {
  $scanTargets += @(Get-ChildItem -LiteralPath $resourceRoot -File -Recurse | Select-Object -ExpandProperty FullName)
}
foreach ($needle in $forbidden) {
  $hits = rg -a -l --fixed-strings $needle $scanTargets 2>$null
  if ($LASTEXITCODE -eq 0 -and $hits) {
    throw "Forbidden release content '$needle' found in: $($hits -join ', ')"
  }
}

Write-Host "Installer verified: $($installer.FullName) ($([Math]::Round($installer.Length / 1MB, 2)) MB)"
