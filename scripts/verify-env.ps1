[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " ROASTAKE - Rust Environment Verification        " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

# Ensure Cargo bin and NASM are on PATH for current session if installed in default locations
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = $cargoBin + ";" + $env:PATH
}

$nasmBin = Join-Path $env:LOCALAPPDATA "bin\NASM"
if ((Test-Path $nasmBin) -and ($env:PATH -notlike "*$nasmBin*")) {
    $env:PATH = $nasmBin + ";" + $env:PATH
}

# 1. Rust Compiler
try {
    $rustcVer = & rustc --version
    Write-Host "[OK] rustc: $rustcVer" -ForegroundColor Green
} catch {
    Write-Host "[FAIL] rustc is not in PATH. Please install Rust via https://rustup.rs" -ForegroundColor Red
    exit 1
}

# 2. Cargo
try {
    $cargoVer = & cargo --version
    Write-Host "[OK] cargo: $cargoVer" -ForegroundColor Green
} catch {
    Write-Host "[FAIL] cargo is not in PATH" -ForegroundColor Red
    exit 1
}

# 3. Rustup
try {
    $rustupVer = & rustup --version
    Write-Host "[OK] rustup: $rustupVer" -ForegroundColor Green
} catch {
    Write-Host "[FAIL] rustup is not in PATH" -ForegroundColor Red
    exit 1
}

# 4. Check targets
$installedTargets = & rustup target list --installed
Write-Host "`nInstalled Rust targets:" -ForegroundColor Yellow
$installedTargets | ForEach-Object { Write-Host " - $_" }

$hasNearTarget = $installedTargets -contains "wasm32-unknown-unknown"
$hasWasiTarget = $installedTargets -contains "wasm32-wasip2"

if ($hasNearTarget) {
    Write-Host "[OK] Target wasm32-unknown-unknown (NEAR Contract) is installed." -ForegroundColor Green
} else {
    Write-Host "[MISSING] Target wasm32-unknown-unknown is NOT installed. Run scripts/install-targets.ps1" -ForegroundColor Yellow
}

if ($hasWasiTarget) {
    Write-Host "[OK] Target wasm32-wasip2 (OutLayer WASI Worker) is installed." -ForegroundColor Green
} else {
    Write-Host "[MISSING] Target wasm32-wasip2 is NOT installed. Run scripts/install-targets.ps1" -ForegroundColor Yellow
}

Write-Host "`nEnvironment verification completed successfully." -ForegroundColor Cyan
