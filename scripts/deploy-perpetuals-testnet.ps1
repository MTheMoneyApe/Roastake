# SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
# Copyright © 2026 @MTheMoneyApe. All rights reserved.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$PerpsAccountId = "perps-v1.roastake-dev-mta.testnet",

    [Parameter(Mandatory = $false)]
    [string]$AdminAccountId = "roastake-dev-mta.testnet",

    [Parameter(Mandatory = $false)]
    [string]$TreasuryAccountId = "roastake-dev-mta.testnet",

    # Official Circle NEAR Testnet USDC (validated on-chain NEP-141/145/148, 6 decimals, 0.00125 NEAR storage bound)
    [Parameter(Mandatory = $false)]
    [string]$CollateralTokenId = "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",

    [Parameter(Mandatory = $false)]
    [string]$SandboxFeedOperatorId = "roastake-dev-mta.testnet",

    [Parameter(Mandatory = $false)]
    [string]$WasmPath = "",

    [switch]$Execute = $false
)

$ErrorActionPreference = "Stop"

$CIRCLE_TESTNET_USDC_ID = "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af"
$CIRCLE_MAINNET_USDC_ID = "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"

$repoRoot = (Resolve-Path "$PSScriptRoot\..").Path
if ([string]::IsNullOrWhiteSpace($WasmPath)) {
    $WasmPath = Join-Path $repoRoot "target\m045b-perps-out\roastake_perpetuals.wasm"
}

foreach ($entry in @(
    @{ Name = "PerpsAccountId"; Value = $PerpsAccountId },
    @{ Name = "AdminAccountId"; Value = $AdminAccountId },
    @{ Name = "TreasuryAccountId"; Value = $TreasuryAccountId },
    @{ Name = "SandboxFeedOperatorId"; Value = $SandboxFeedOperatorId }
)) {
    if (-not $entry.Value.EndsWith(".testnet")) {
        Write-Error "[FAIL-CLOSED] $($entry.Name) ('$($entry.Value)') must be a valid NEAR '.testnet' account. Mainnet accounts are forbidden."
        exit 1
    }
}

$colLower = $CollateralTokenId.ToLowerInvariant()
if ($colLower.StartsWith("nep141:") -or $colLower -eq $CIRCLE_MAINNET_USDC_ID -or $colLower.EndsWith(".near") -or $colLower.Contains("mainnet")) {
    Write-Error "[FAIL-CLOSED] CollateralTokenId ('$CollateralTokenId') is rejected (Mainnet account / Intents URI / non-Testnet token forbidden)."
    exit 1
}
$isValidTestnetCollateral = $colLower.EndsWith(".testnet") -or ($colLower -match '^[0-9a-f]{64}$')
if (-not $isValidTestnetCollateral) {
    Write-Error "[FAIL-CLOSED] CollateralTokenId ('$CollateralTokenId') must be a valid NEAR Testnet '.testnet' account or verified 64-hex implicit account ID."
    exit 1
}

if (-not (Test-Path $WasmPath)) {
    Write-Error "[MISSING ARTIFACT] Expected WASM missing at '$WasmPath'. Run scripts/build-perpetuals.ps1 first."
    exit 1
}

$item = Get-Item $WasmPath
$sha256 = (Get-FileHash -Algorithm SHA256 $WasmPath).Hash.ToLower()

$disclosure = if ($colLower -eq $CIRCLE_TESTNET_USDC_ID) {
    "Circle Native Testnet USDC (no monetary value): official 6-decimal NEAR Testnet USDC (3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af); parity 1 USDC = 1 USD is accounting-only."
} else {
    "NEAR Testnet alternative collateral fixture (no monetary value): 6-decimal non-monetary testnet token; parity 1 USDC = 1 USD is accounting-only."
}

$initArgs = @{
    admin = $AdminAccountId
    treasury = $TreasuryAccountId
    primary_collateral_token = $CollateralTokenId
    collateral_symbol = "USDC"
    collateral_decimals = 6
    non_monetary_testnet_disclosure = $disclosure
} | ConvertTo-Json -Compress

$registerMarketArgs = @{
    market_id = "COFFEE-ARABICA-TESTNET/USD"
    oracle_source = @{
        TestnetSandboxOperator = @{
            oracle_operator = $SandboxFeedOperatorId
            min_price_e8 = "100000000"
            max_price_e8 = "50000000000"
        }
    }
    risk = $null
    oracle_policy = @{
        allow_testnet_sandbox = $true
        max_execution_age_sec = 300
        emergency_close_age_sec = 3600
        max_jump_bps = 500
        max_clock_drift_sec = 60
    }
    exec_spread_bps = 0
} | ConvertTo-Json -Compress -Depth 6

Write-Host "=================================================" -ForegroundColor Cyan
Write-Host " ROASTAKE Perpetuals - Testnet Deploy Preparation " -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan
Write-Host "WASM Artifact: $WasmPath"
Write-Host "Size:          $($item.Length) bytes"
Write-Host "SHA-256:       $sha256"
Write-Host "Target Net:    NEAR Testnet ONLY"
Write-Host "Collateral:    $CollateralTokenId (decimals: 6, scaled x100 -> USD_SCALE = 10^8)"
Write-Host "Oracle Mode:   Isolated TESTNET_SANDBOX_FEED (roastake-oracle-v1.testnet holds only IMF/GESHA which Perps blocks)"
Write-Host "`nPlanned CLI Initialization Sequence (NOT executed unless -Execute passed with user sign-off):" -ForegroundColor Yellow
Write-Host "1. Deploy contract:      near deploy $PerpsAccountId $WasmPath --networkId testnet"
Write-Host "2. Initialize perps:     near call $PerpsAccountId init '$initArgs' --accountId $AdminAccountId --networkId testnet"
Write-Host "3. Register perps on FT: near call $CollateralTokenId storage_deposit '{`"account_id`":`"$PerpsAccountId`",`"registration_only`":true}' --accountId $AdminAccountId --deposit 0.00125 --networkId testnet"
Write-Host "4. Verify NEP-148 XCC:   near call $PerpsAccountId verify_collateral_metadata '{`"token_id`":`"$CollateralTokenId`"}' --accountId $AdminAccountId --gas 100000000000000 --networkId testnet"
Write-Host "5. Register market:      near call $PerpsAccountId register_market '$registerMarketArgs' --accountId $AdminAccountId --networkId testnet"
Write-Host "6. User setup & Faucet:  Acquire up to 20 USDC/2h manually at https://faucet.circle.com/ (NEAR Testnet), ensure user holds 0.00125 NEAR storage_deposit on $CollateralTokenId and 0.05 NEAR via register_account on $PerpsAccountId."

if (-not $Execute) {
    Write-Host "`n[DRY-RUN COMPLETE] Zero transactions broadcast during M04.5B.1." -ForegroundColor Green
    exit 0
}

Write-Error "[SAFETY BLOCK] Interactive execution must be authorized explicitly outside M04.5B.1."
exit 1
