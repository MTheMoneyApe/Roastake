> **Status notice (2026-11-14): superseded for current project status.** This file is retained as historical design/operations documentation and may describe earlier implementation gates. For the authoritative current implementation, deployed contract registry, transaction evidence and evaluation instructions, use the [current ROASTAKE README](../../README.md). Do not use this document alone to infer present deployment state.

---

<!-- SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only -->
<!-- Copyright © 2026 @MTheMoneyApe. All rights reserved. -->

> **License:** [ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0](../../LICENSE-ROASTAKE-PERPS.md) — Copyright © 2026 @MTheMoneyApe. All rights reserved. **Licensor:** @MTheMoneyApe

# ROASTAKE Perpetuals — NEAR Testnet MVP Operations & Pre-Deploy Runbook (M04.5B.1)

- **Status:** Pre-Deployment Validated (Compiled Locally & Sandbox-Verified — **NOT Yet Deployed to Public Testnet**)
- **Date:** 2026-11-11
- **Version:** `1.1.0-m045b1`
- **Target Network:** **NEAR Testnet ONLY** (`.near` Mainnet accounts, `nep141:` URIs as account IDs, and Mainnet Circle USDC `17208628...36133a1` rejected fail-closed)

---

## 1. Environment, Asset & Contract Status Boundary Matrix

Every component below is strictly separated so compiled local artifacts, synthetic feeds, alternative fixtures, and live deployed contracts are never conflated:

| Component / Identifier | Category | Public Deployment Status | Live Purpose & Safeguards |
| --- | --- | --- | --- |
| **`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`** | **Default Testnet Collateral (Circle Testnet USDC)** | **Deployed on NEAR Testnet** (`code_hash: EoAUFWELjD9gYzsjKDXYRmHDHf1RJ95jaKNSYE6v9b2z`, WASM SHA-256 `ccfb5ad61c10b5a364d405eb404926f31b37bf636897ab9c7cfc3e74ffbf12ab`) | Intents URI: `nep141:3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`. `NEP-141/145/148`, `decimals = 6`, `0.00125 NEAR` storage bound. Scaled `x100 -> USD_SCALE = 10^8` with `<100 e8` residue retention. Zero real monetary value. |
| **`usdc.tidepool-sandbox.testnet`** | **Alternative Test Token (Tidepool)** | Deployed on NEAR Testnet (third-party `privacei.testnet`) | **NOT default collateral.** Rejected by Perpetuals allowlist by default unless explicitly added by Admin as an alternative integration fixture. |
| **`mock-ft` (`roastake_mock_tidepool_ft.wasm`)** & **`mock-outlayer`** | **Local Sandbox Fixtures** | **Local Compiled WASM Only** (`near-sandbox`) | Used strictly inside automated local WSL2 Sandbox integration tests; never deployed to public Testnet. |
| **`TESTNET_SANDBOX_FEED` (`TestnetSandboxOperator`)** | **Synthetic Testnet Oracle Feed** | Compiled inside `contracts/perpetuals` (Not Yet Deployed) | Explicit operator-signed synthetic price source (`monotonic sequence`, bounds, `300s` staleness, `500 bps` jump circuit breaker, visible `is_synthetic_testnet_sandbox = true` on `get_market`). |
| **`roastake-oracle-v1.testnet`** | **External On-Chain Oracle (`contracts/oracle-adapter`)** | **Deployed Live on NEAR Testnet** (`admin: roastake-dev-mta.testnet`, untouched) | Live RPC confirms only `ARABICA-MONTHLY-IMF/USD` (`PUBLIC_BENCHMARK_MONTHLY`) and `GESHA-DEMO/USD` (`SYNTHETIC_DEMO`) exist today — both strictly blocked (`ERR_ORACLE_CLASS_REJECTED`) for Perpetuals execution until Gate 4 live feeds arrive. |
| **`rUSD` (`contracts/demo-token`)** | **Future Roadmap Asset** | **Not Created / Not Compiled / Not Deployed** | Deferred to future post-hackathon roadmap. Zero `rUSD` code exists. |
| **`roastake-perpetuals` (`roastake_perpetuals.wasm`)** | **Perpetuals Core Contract (`contracts/perpetuals`)** | **Compiled & Sandbox-Verified Locally Only — NOT Yet Deployed** | Byte-deterministic host `cargo-near` WASM build ready in `target/m045b-perps-out/roastake_perpetuals.wasm`; awaiting explicit user deployment authorization. |

---

## 2. Local Build, Unit & NEAR Sandbox Verification Commands

```powershell
# 1. Rust formatting, strict clippy & full workspace tests
$env:PATH = "$env:LOCALAPPDATA\bin\NASM;$env:PATH"
cargo fmt --check
cargo fmt --manifest-path integration-tests/near-sandbox-js/mock-ft/Cargo.toml --check
cargo clippy -p roastake-shared --all-targets -- -D warnings
cargo clippy -p roastake-perpetuals --all-targets -- -D warnings
cargo clippy --manifest-path integration-tests/near-sandbox-js/mock-ft/Cargo.toml --all-targets -- -D warnings
cargo test --workspace

# 2. Build isolated WASM release outputs & inspect baselines
powershell -ExecutionPolicy Bypass -File scripts/build-perpetuals.ps1
powershell -ExecutionPolicy Bypass -File scripts/check-artifacts.ps1

# 3. Preview dry-run Testnet deployment & verify fail-closed Mainnet rejection (zero broadcasts)
powershell -ExecutionPolicy Bypass -File scripts/deploy-perpetuals-testnet.ps1
powershell -ExecutionPolicy Bypass -File scripts/deploy-perpetuals-testnet.ps1 -PerpsAccountId evil.near
```

```bash
# 4. Run local multi-contract NEAR Sandbox suite (Circle metadata profile + XCC Oracle + Liquidation + Emergency)
cd /mnt/c/Users/M/Documents/Roastake/integration-tests/near-sandbox-js
pnpm test:smoke
NEAR_WORKSPACES_NETWORK=sandbox node --test test/sandbox-perpetuals.test.mjs
```

---

## 3. Manual Circle Testnet Faucet Instructions (User Action Required)

Because Circle's official faucet enforces CAPTCHA verification and per-address rate limiting (**20 USDC per address per asset/network every 2 hours**), automated agents never request tokens on a user's behalf:

1. Open **`https://faucet.circle.com/`** directly in your browser (never use lookalike third-party domains).
2. Under **Token**, select **USDC**.
3. Under **Network**, select **NEAR Testnet**.
4. Enter your NEAR Testnet account ID (`<your-account>.testnet`).
5. Complete the manual CAPTCHA and click **Send 20 USDC**.
6. Verify your on-chain balance via Testnet RPC:
   ```powershell
   near tokens <your-account>.testnet send-ft 3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af ... # or view ft_balance_of via RPC
   ```
   > **Note on NEP-145 Storage:** Before receiving/transferring Circle Testnet USDC on NEAR, ensure both your account AND the deployed Perpetuals contract hold **`0.00125 NEAR`** storage deposit (`storage_deposit`) on `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`.

---

## 4. Authorized Testnet Deployment Sequence (M04.5G — NOT Executed in M04.5B.1)

Run [`scripts/deploy-perpetuals-testnet.ps1`](file:///C:/Users/M/Documents/Roastake/scripts/deploy-perpetuals-testnet.ps1) in dry-run mode first, then execute in exact order only with explicit user sign-off:

1. **Deploy `roastake_perpetuals.wasm`** to `perps-v1.roastake-dev-mta.testnet`.
2. **Initialize `init(...)`** setting `primary_collateral_token = "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af"`, `collateral_symbol = "USDC"`, `collateral_decimals = 6`, and explicit Testnet non-monetary disclosure.
3. **Register Perps on Circle USDC (`0.00125 NEAR`):** Call `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af::storage_deposit({"account_id":"perps-v1.roastake-dev-mta.testnet","registration_only":true})`.
4. **Verify On-Chain Circle Metadata via XCC:** Call `verify_collateral_metadata({"token_id":"3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af"})` (`100 TGas`, confirms `spec: "ft-1.0.0"`, `symbol: "USDC"`, `decimals: 6` across Circle's `1,772`-byte inline SVG JSON response).
5. **Register Initial `TESTNET_SANDBOX_FEED` Market (`COFFEE-ARABICA-TESTNET/USD`):** Explicit `TestnetSandboxOperator`, monotonic sequence, bounds `["100000000", "50000000000"]` (`$1.00–$500.00/kg`), `max_execution_age_sec: 300`, `emergency_close_age_sec: 3600`, `max_jump_bps: 500`.
6. **User Setup & Operations:** Register user/LP wallets on Perps (`register_account`, `0.05 NEAR`), push fresh synthetic tick (`push_sandbox_oracle_price`), deposit via `3e2210e...af::ft_transfer_call`, trade/liquidate/withdraw (`<100 e8` residue preserved cleanly in `free_collateral_e8`), and monitor `verify_solvency_invariant() == true`.
