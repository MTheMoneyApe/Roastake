# SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
# Copyright © 2026 @MTheMoneyApe. All rights reserved.

[CmdletBinding()]
param(
    [string]$OutFile = "",
    [string]$TargetDir = "",
    [switch]$SkipMockFt = $false
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $repoRoot

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = $cargoBin + ";" + $env:PATH
}
$nasmBin = Join-Path $env:LOCALAPPDATA "bin\NASM"
if ((Test-Path $nasmBin) -and ($env:PATH -notlike "*$nasmBin*")) {
    $env:PATH = $nasmBin + ";" + $env:PATH
}

if ([string]::IsNullOrWhiteSpace($TargetDir)) {
    $TargetDir = Join-Path $repoRoot "target\m045b-perps"
} elseif (-not [System.IO.Path]::IsPathRooted($TargetDir)) {
    $TargetDir = Join-Path $repoRoot $TargetDir
}

if ([string]::IsNullOrWhiteSpace($OutFile)) {
    $OutFile = Join-Path $repoRoot "target\m045b-perps-out\roastake_perpetuals.wasm"
} elseif (-not [System.IO.Path]::IsPathRooted($OutFile)) {
    $OutFile = Join-Path $repoRoot $OutFile
}
$resolvedOutFile = [System.IO.Path]::GetFullPath($OutFile)
$outDir = Split-Path -Parent $resolvedOutFile
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " Building ROASTAKE Perpetuals Contract           " -ForegroundColor Cyan
Write-Host " Toolchain: cargo-near 0.22.0 (wasm-opt -O)      " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

$env:CARGO_TARGET_DIR = $TargetDir
$tempOutDir = Join-Path $env:TEMP ("roastake-perps-build-" + [System.Diagnostics.Process]::GetCurrentProcess().Id)
New-Item -ItemType Directory -Force -Path $tempOutDir | Out-Null

try {
    & cargo near build non-reproducible-wasm --locked --no-abi --manifest-path "$repoRoot\contracts\perpetuals\Cargo.toml" --out-dir $tempOutDir
    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n[FAIL] cargo near build for perpetuals failed with exit code $LASTEXITCODE" -ForegroundColor Red
        exit $LASTEXITCODE
    }

    $builtWasm = Join-Path $tempOutDir "roastake_perpetuals.wasm"
    if (-not (Test-Path $builtWasm)) {
        Write-Host "`n[FAIL] Expected build output missing at $builtWasm" -ForegroundColor Red
        exit 1
    }

    Copy-Item -Force $builtWasm $resolvedOutFile
    $fileItem = Get-Item $resolvedOutFile
    $hash = (Get-FileHash -Algorithm SHA256 $resolvedOutFile).Hash.ToLower()
    Write-Host "`n[OK] Perpetuals Contract built cleanly via cargo-near!" -ForegroundColor Green
    Write-Host "Path:   $resolvedOutFile"
    Write-Host "Size:   $($fileItem.Length) bytes"
    Write-Host "SHA256: $hash"
} finally {
    if (Test-Path $tempOutDir) {
        Remove-Item -Recurse -Force $tempOutDir -ErrorAction SilentlyContinue
    }
}

if (-not $SkipMockFt) {
    $mockOutDir = Join-Path $repoRoot "target\m045b-mock-ft-out"
    New-Item -ItemType Directory -Force -Path $mockOutDir | Out-Null
    $mockOutFile = Join-Path $mockOutDir "roastake_mock_tidepool_ft.wasm"
    $env:CARGO_TARGET_DIR = Join-Path $repoRoot "target\m045b-mock-ft"
    $tempMockDir = Join-Path $env:TEMP ("roastake-mock-ft-build-" + [System.Diagnostics.Process]::GetCurrentProcess().Id)
    New-Item -ItemType Directory -Force -Path $tempMockDir | Out-Null

    try {
        & cargo near build non-reproducible-wasm --no-abi --manifest-path "$repoRoot\integration-tests\near-sandbox-js\mock-ft\Cargo.toml" --out-dir $tempMockDir
        if ($LASTEXITCODE -ne 0) {
            Write-Host "`n[FAIL] cargo near build for mock-ft failed with exit code $LASTEXITCODE" -ForegroundColor Red
            exit $LASTEXITCODE
        }
        $builtMock = Join-Path $tempMockDir "roastake_mock_tidepool_ft.wasm"
        Copy-Item -Force $builtMock $mockOutFile
        $mockItem = Get-Item $mockOutFile
        $mockHash = (Get-FileHash -Algorithm SHA256 $mockOutFile).Hash.ToLower()
        Write-Host "`n[OK] Sandbox Tidepool FT Fixture built cleanly!" -ForegroundColor Green
        Write-Host "Path:   $mockOutFile"
        Write-Host "Size:   $($mockItem.Length) bytes"
        Write-Host "SHA256: $mockHash"
    } finally {
        if (Test-Path $tempMockDir) {
            Remove-Item -Recurse -Force $tempMockDir -ErrorAction SilentlyContinue
        }
    }
}
