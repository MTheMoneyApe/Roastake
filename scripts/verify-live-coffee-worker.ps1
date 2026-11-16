[CmdletBinding()]
param(
    [switch]$InstallWasiTarget
)

$ErrorActionPreference = "Stop"
$targetTriple = "wasm32-wasip2"
$packageName = "roastake-coffee-price-index"
$wasmRelativePath = "target/$targetTriple/release/roastake_coffee_price_index.wasm"
$repoRoot = Split-Path -Parent $PSScriptRoot

function Require-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command '$Name' was not found. Install Rust with rustup, then reopen PowerShell."
    }
}

function Invoke-Checked([string]$Name, [string[]]$Arguments) {
    Write-Host ""
    Write-Host "> $Name $($Arguments -join ' ')" -ForegroundColor Cyan
    & $Name @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw ("Command failed with exit code " + $LASTEXITCODE + ": " + $Name + " " + ($Arguments -join ' '))
    }
}

Require-Command "cargo"
Require-Command "rustup"

Push-Location $repoRoot
try {
    Write-Host "ROASTAKE live coffee Oracle worker verification" -ForegroundColor Green
    Write-Host "Repository: $repoRoot"
    Write-Host "Scope: local tests/build only; this script does not upload or deploy."

    Invoke-Checked "cargo" @("fmt", "--all", "--", "--check")
    Invoke-Checked "cargo" @("test", "-p", $packageName, "-p", "roastake-perpetuals", "-p", "roastake-oracle-adapter")

    $installedTargets = & rustup target list --installed
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to read installed Rust targets."
    }

    if ($installedTargets -notcontains $targetTriple) {
        if (-not $InstallWasiTarget) {
            throw "Rust target '$targetTriple' is not installed. Re-run with -InstallWasiTarget to install it."
        }
        Invoke-Checked "rustup" @("target", "add", $targetTriple)
    }

    Invoke-Checked "cargo" @(
        "build",
        "--release",
        "-p", $packageName,
        "--features", "wasi-http",
        "--target", $targetTriple
    )

    $wasmPath = Join-Path $repoRoot $wasmRelativePath
    if (-not (Test-Path -LiteralPath $wasmPath -PathType Leaf)) {
        throw "Expected WASM artifact was not produced: $wasmPath"
    }

    $artifact = Get-Item -LiteralPath $wasmPath
    $sha256 = (Get-FileHash -LiteralPath $wasmPath -Algorithm SHA256).Hash.ToLowerInvariant()

    [pscustomobject]@{
        Result        = "PASS"
        Artifact      = $wasmPath
        Bytes         = $artifact.Length
        SHA256        = $sha256
        Target        = $targetTriple
        Package       = $packageName
        Uploaded      = $false
        Deployed      = $false
    } | ConvertTo-Json -Depth 3
}
finally {
    Pop-Location
}
