[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path "$PSScriptRoot\.."
Set-Location $repoRoot

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = $cargoBin + ";" + $env:PATH
}
$nasmBin = Join-Path $env:LOCALAPPDATA "bin\NASM"
if ((Test-Path $nasmBin) -and ($env:PATH -notlike "*$nasmBin*")) {
    $env:PATH = $nasmBin + ";" + $env:PATH
}

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " Running ROASTAKE Automated Test Suite           " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

& cargo test --workspace

if ($LASTEXITCODE -eq 0) {
    Write-Host "`n[OK] All test suites passed successfully!" -ForegroundColor Green
} else {
    Write-Host "`n[FAIL] One or more tests failed." -ForegroundColor Red
    exit $LASTEXITCODE
}
