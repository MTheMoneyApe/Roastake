[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path "$PSScriptRoot\.."
$artifact = Join-Path $repoRoot "target\wasm32-wasip2\release\roastake-coffee-price-index.wasm"

if (-not (Test-Path $artifact)) {
    Write-Host "[WARN] Worker artifact not found at $artifact. Building worker first..." -ForegroundColor Yellow
    & "$PSScriptRoot\build-worker.ps1"
}

$hash = (Get-FileHash -Algorithm SHA256 $artifact).Hash.ToLower()

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " OutLayer WASI Worker Cryptographic SHA-256 Hash " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan
Write-Host "Artifact: $artifact"
Write-Host "SHA-256:  $hash" -ForegroundColor Green
Write-Host "=================================================" -ForegroundColor Cyan

return $hash
