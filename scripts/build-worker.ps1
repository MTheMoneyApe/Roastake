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
Write-Host " Building Coffee Price Index WASI Worker         " -ForegroundColor Cyan
Write-Host " Target: wasm32-wasip2                           " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

& cargo build --package roastake-coffee-price-index --target wasm32-wasip2 --release --features wasi-http

$artifact = Join-Path $repoRoot "target\wasm32-wasip2\release\roastake-coffee-price-index.wasm"
if (Test-Path $artifact) {
    $fileItem = Get-Item $artifact
    $hash = (Get-FileHash -Algorithm SHA256 $artifact).Hash.ToLower()
    Write-Host "`n[OK] Worker built successfully!" -ForegroundColor Green
    Write-Host "Path:   $artifact"
    Write-Host "Size:   $($fileItem.Length) bytes"
    Write-Host "SHA256: $hash"
} else {
    Write-Host "`n[FAIL] Artifact not found at $artifact" -ForegroundColor Red
    exit 1
}
