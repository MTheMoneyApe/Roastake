[CmdletBinding()]
param(
    [string]$OutFile = "",
    [string]$TargetDir = "",
    [switch]$ApproveProtectedOverwrite = $false
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
    $TargetDir = Join-Path $repoRoot "target\m042d2a-adapter"
} elseif (-not [System.IO.Path]::IsPathRooted($TargetDir)) {
    $TargetDir = Join-Path $repoRoot $TargetDir
}

if ([string]::IsNullOrWhiteSpace($OutFile)) {
    $OutFile = Join-Path $repoRoot "target\m042d2a-adapter-out\roastake_oracle_adapter_m042d2a.wasm"
} elseif (-not [System.IO.Path]::IsPathRooted($OutFile)) {
    $OutFile = Join-Path $repoRoot $OutFile
}

# Protected historical baselines and Gate 3 release artifacts that normal builds must NEVER overwrite.
$protectedPaths = @(
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_security_fixed.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_admin_hardened.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_release_candidate.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_release_candidate_m03376.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\mock_outlayer.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\mock_outlayer_abi_fixed.wasm"),
    (Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_mock_outlayer.wasm"),
    (Join-Path $repoRoot "target\wasm32-wasip2\release\roastake-coffee-price-index.wasm")
) | ForEach-Object { [System.IO.Path]::GetFullPath($_).ToLowerInvariant() }

$resolvedOutFile = [System.IO.Path]::GetFullPath($OutFile)
if (($protectedPaths -contains $resolvedOutFile.ToLowerInvariant()) -and (-not $ApproveProtectedOverwrite)) {
    Write-Error "[FAIL-CLOSED] Refusing to overwrite protected historical artifact '$resolvedOutFile' without -ApproveProtectedOverwrite."
    exit 1
}

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " Building NEAR Oracle Adapter Contract           " -ForegroundColor Cyan
Write-Host " Toolchain: cargo-near 0.22.0 (wasm-opt -O)      " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

$env:CARGO_TARGET_DIR = $TargetDir
$outDir = Split-Path -Parent $resolvedOutFile
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$tempOutDir = Join-Path $env:TEMP ("roastake-adapter-build-" + [System.Diagnostics.Process]::GetCurrentProcess().Id)
New-Item -ItemType Directory -Force -Path $tempOutDir | Out-Null

try {
    & cargo near build non-reproducible-wasm --locked --no-abi --manifest-path "$repoRoot\contracts\oracle-adapter\Cargo.toml" --out-dir $tempOutDir
    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n[FAIL] cargo near build failed with exit code $LASTEXITCODE" -ForegroundColor Red
        exit $LASTEXITCODE
    }

    $builtWasm = Join-Path $tempOutDir "roastake_oracle_adapter.wasm"
    if (-not (Test-Path $builtWasm)) {
        Write-Host "`n[FAIL] Expected build output missing at $builtWasm" -ForegroundColor Red
        exit 1
    }

    Copy-Item -Force $builtWasm $resolvedOutFile
    $fileItem = Get-Item $resolvedOutFile
    $hash = (Get-FileHash -Algorithm SHA256 $resolvedOutFile).Hash.ToLower()
    Write-Host "`n[OK] Adapter Contract built cleanly via cargo-near!" -ForegroundColor Green
    Write-Host "Path:   $resolvedOutFile"
    Write-Host "Size:   $($fileItem.Length) bytes"
    Write-Host "SHA256: $hash"
} finally {
    if (Test-Path $tempOutDir) {
        Remove-Item -Recurse -Force $tempOutDir -ErrorAction SilentlyContinue
    }
}
