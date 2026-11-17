# SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
# Copyright © 2026 @MTheMoneyApe. All rights reserved.

[CmdletBinding()]
param(
    [string]$OutFile = "",
    [string]$TargetDir = ""
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
    $TargetDir = Join-Path $repoRoot "target\m0418-coffee-token"
} elseif (-not [System.IO.Path]::IsPathRooted($TargetDir)) {
    $TargetDir = Join-Path $repoRoot $TargetDir
}

if ([string]::IsNullOrWhiteSpace($OutFile)) {
    $OutFile = Join-Path $repoRoot "target\m0418-coffee-token-out\roastake_coffee_token.wasm"
} elseif (-not [System.IO.Path]::IsPathRooted($OutFile)) {
    $OutFile = Join-Path $repoRoot $OutFile
}

$resolvedOutFile = [System.IO.Path]::GetFullPath($OutFile)
$outDir = Split-Path -Parent $resolvedOutFile
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$env:CARGO_TARGET_DIR = $TargetDir
$tempOutDir = Join-Path $env:TEMP ("roastake-coffee-token-build-" + [System.Diagnostics.Process]::GetCurrentProcess().Id)
New-Item -ItemType Directory -Force -Path $tempOutDir | Out-Null

try {
    & cargo near build non-reproducible-wasm --locked --no-abi --manifest-path "$repoRoot\contracts\coffee-token\Cargo.toml" --out-dir $tempOutDir
    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n[FAIL] cargo near build for coffee-token failed with exit code $LASTEXITCODE" -ForegroundColor Red
        exit $LASTEXITCODE
    }

    $builtWasm = Join-Path $tempOutDir "roastake_coffee_token.wasm"
    if (-not (Test-Path $builtWasm)) {
        Write-Host "`n[FAIL] Expected build output missing at $builtWasm" -ForegroundColor Red
        exit 1
    }

    Copy-Item -Force $builtWasm $resolvedOutFile
    $fileItem = Get-Item $resolvedOutFile
    $hash = (Get-FileHash -Algorithm SHA256 $resolvedOutFile).Hash.ToLower()
    Write-Host "`n[OK] Coffee Asset Token Contract built cleanly!" -ForegroundColor Green
    Write-Host "Path:   $resolvedOutFile"
    Write-Host "Size:   $($fileItem.Length) bytes"
    Write-Host "SHA256: $hash"
} finally {
    if (Test-Path $tempOutDir) {
        Remove-Item -Recurse -Force $tempOutDir -ErrorAction SilentlyContinue
    }
}
