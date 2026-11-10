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
Write-Host " Building NEAR Oracle Adapter Contract           " -ForegroundColor Cyan
Write-Host " Toolchain: cargo-near 0.22.0 (wasm-opt -O)      " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

$env:CARGO_TARGET_DIR = Join-Path $repoRoot "integration-tests\near-sandbox-js\target-adapter"
$tempOutDir = Join-Path $env:TEMP "roastake-rc-m03376-out"
New-Item -ItemType Directory -Force -Path $tempOutDir | Out-Null

& cargo near build non-reproducible-wasm --locked --no-abi --manifest-path "$repoRoot\contracts\oracle-adapter\Cargo.toml" --out-dir $tempOutDir
if ($LASTEXITCODE -ne 0) {
    Write-Host "`n[FAIL] cargo near build failed with exit code $LASTEXITCODE" -ForegroundColor Red
    exit $LASTEXITCODE
}

$builtWasm = Join-Path $tempOutDir "roastake_oracle_adapter.wasm"
$artifact = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_release_candidate_m03376.wasm"
Copy-Item -Force $builtWasm $artifact

if (Test-Path $artifact) {
    $fileItem = Get-Item $artifact
    $hash = (Get-FileHash -Algorithm SHA256 $artifact).Hash.ToLower()
    Write-Host "`n[OK] Release-Candidate M03C.3.7.6 Contract built successfully via cargo-near 0.22.0!" -ForegroundColor Green
    Write-Host "Path:   $artifact"
    Write-Host "Size:   $($fileItem.Length) bytes"
    Write-Host "SHA256: $hash"
} else {
    Write-Host "`n[FAIL] Artifact not found at $artifact" -ForegroundColor Red
    exit 1
}
