[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " ROASTAKE - Install Required Rust WASM Targets   " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = $cargoBin + ";" + $env:PATH
}

Write-Host "Adding target: wasm32-unknown-unknown (NEAR smart contract)..." -ForegroundColor Yellow
& rustup target add wasm32-unknown-unknown

Write-Host "Adding target: wasm32-wasip2 (OutLayer WASI worker)..." -ForegroundColor Yellow
& rustup target add wasm32-wasip2

Write-Host "`n[OK] Targets installed successfully:" -ForegroundColor Green
& rustup target list --installed | Select-String "wasm32"
