$ErrorActionPreference = 'Stop'

$appRoot = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $appRoot 'src-tauri\target\release\anydoc-assistant.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
  throw "Release executable not found: $executable"
}

$process = Start-Process -FilePath $executable -PassThru -WindowStyle Hidden
try {
  Start-Sleep -Seconds 5
  if ($process.HasExited) {
    throw "Release application exited early with code $($process.ExitCode)"
  }
  Write-Host "Release launch smoke test passed (PID $($process.Id))"
}
finally {
  if (-not $process.HasExited) {
    Stop-Process -Id $process.Id
    $process.WaitForExit()
  }
}
