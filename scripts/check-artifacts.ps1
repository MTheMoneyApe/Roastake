[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path "$PSScriptRoot\.."

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " ROASTAKE Build Artifacts Inspection             " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan

$rcContractPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_release_candidate_m03376.wasm"
$expectedRcHash = "a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8"
$rcM03374Path = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_release_candidate.wasm"
$adminHardenedPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_admin_hardened.wasm"
$securityFixedPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter_security_fixed.wasm"
$baselineAdapterPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_oracle_adapter.wasm"
$mockAbiFixedPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\mock_outlayer_abi_fixed.wasm"
$mockOutlayerPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\mock_outlayer.wasm"
$roastakeMockPath = Join-Path $repoRoot "integration-tests\near-sandbox-js\artifacts\roastake_mock_outlayer.wasm"
$rawContractPath = Join-Path $repoRoot "target\wasm32-unknown-unknown\release\roastake_oracle_adapter.wasm"
$workerPath = Join-Path $repoRoot "target\wasm32-wasip2\release\roastake-coffee-price-index.wasm"

# 1. Release-Candidate Contract inspection (M03C.3.7.6)
Write-Host "`n1. NEAR Oracle Adapter Release-Candidate Smart Contract (M03C.3.7.6, cargo-near 0.22.0):" -ForegroundColor Yellow
if (Test-Path $rcContractPath) {
    $item = Get-Item $rcContractPath
    $hash = (Get-FileHash -Algorithm SHA256 $rcContractPath).Hash.ToLower()
    Write-Host "   Path:    $rcContractPath"
    Write-Host "   Size:    $($item.Length) bytes ($([math]::Round($item.Length / 1KB, 2)) KB)"
    Write-Host "   SHA-256: $hash" -ForegroundColor Green
    Write-Host "   Target:  wasm32-unknown-unknown (cargo-near 0.22.0 non-reproducible-wasm --locked --no-abi)"
    if ($hash -ne $expectedRcHash) {
        Write-Host "   [ERROR] Release-Candidate SHA-256 mismatch! Expected $expectedRcHash" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "   [MISSING] Release-candidate artifact not found. Run scripts/build-contract.ps1" -ForegroundColor Red
    exit 1
}

# 2. Protected Historical Baselines & Mock OutLayer Artifacts
Write-Host "`n2. Protected Historical Adapter Baselines & Mock OutLayer Artifacts:" -ForegroundColor Yellow
foreach ($entry in @(
    @{ Name = "M03C.3.7.4 Release Candidate"; Path = $rcM03374Path; Expected = "8ef00c67059b888ddff5de60829b2ffa7c26e066c82befb208fff41d3cebd00f" },
    @{ Name = "M03C.3.7.2 Admin-Hardened"; Path = $adminHardenedPath; Expected = "bc5fe10d77fe8cca112a5174f52568fb8b9de662c30d16516cffc45813a843c7" },
    @{ Name = "M03C.3.6 Security-Fixed"; Path = $securityFixedPath; Expected = "f4ae912d18f8d219f2dd643a22a75ccb6a5681c3670c1bef1b4a3a87420b4186" },
    @{ Name = "M03C.3.2 Baseline"; Path = $baselineAdapterPath; Expected = "ebd76a9346ce5c978381b2d06689fdca67b37a4feed7d5b6a9154375b04dea2f" },
    @{ Name = "Mock OutLayer ABI Fixed"; Path = $mockAbiFixedPath; Expected = "a37bd40d21a7d85e0de6e6bfb369bebe41313222dd938b6d6a5361d1d99883e4" },
    @{ Name = "Mock OutLayer Baseline"; Path = $mockOutlayerPath; Expected = "cf060259e3d2936e5ed4754e22ec8bb5e7d32ec2141d1ad9cfe9e44cf71b69cc" },
    @{ Name = "Roastake Mock OutLayer"; Path = $roastakeMockPath; Expected = "cf060259e3d2936e5ed4754e22ec8bb5e7d32ec2141d1ad9cfe9e44cf71b69cc" },
    @{ Name = "Raw Unoptimized Baseline (DO NOT DEPLOY)"; Path = $rawContractPath; Expected = "ad83a42a634a33f4a1163744b09eec2b6f33118c925ef2aec294ac74421df93a" }
)) {
    if (Test-Path $entry.Path) {
        $item = Get-Item $entry.Path
        $hash = (Get-FileHash -Algorithm SHA256 $entry.Path).Hash.ToLower()
        Write-Host "   [$($entry.Name)] $($item.Length) bytes | SHA-256: $hash"
        if ($hash -ne $entry.Expected) {
            Write-Host "   [ERROR] Protected baseline hash mismatch for $($entry.Name)!" -ForegroundColor Red
            exit 1
        }
    } else {
        Write-Host "   [ERROR] Protected baseline missing for $($entry.Name): $($entry.Path)" -ForegroundColor Red
        exit 1
    }
}

# 3. Worker inspection
Write-Host "`n3. OutLayer Coffee Price Index WASI Worker:" -ForegroundColor Yellow
if (Test-Path $workerPath) {
    $item = Get-Item $workerPath
    $hash = (Get-FileHash -Algorithm SHA256 $workerPath).Hash.ToLower()
    Write-Host "   Path:    $workerPath"
    Write-Host "   Size:    $($item.Length) bytes ($([math]::Round($item.Length / 1KB, 2)) KB)"
    Write-Host "   SHA-256: $hash" -ForegroundColor Green
    Write-Host "   Target:  wasm32-wasip2"
    if ($hash -ne "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6") {
        Write-Host "   [ERROR] Worker SHA-256 mismatch!" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "   [MISSING] Worker artifact not found. Run scripts/build-worker.ps1" -ForegroundColor Red
    exit 1
}

Write-Host "`nArtifact inspection complete." -ForegroundColor Cyan
