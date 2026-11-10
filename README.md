# Roastake

**Roastake is a commodity-finance platform focused initially on coffee markets.** The project starts with a NEAR Protocol Testnet perpetuals prototype and is designed to grow into the wider Roastake ecosystem.

> **Current development status:** Mission 03C.3.7.6 Final Solvency Liveness, Fixture Accounting, Storage Safety & Documentation Consolidation completed. The OutLayer NEAR Testnet foundation is implemented and verified against OutLayer's `outlayer.testnet` ABI schema and local `nearcore 2.14.0` (protocol 88) Sandbox: Rust Oracle Adapter smart contract (`contracts/oracle-adapter`), deterministic WASI worker (`workers/coffee-price-index`), shared schemas (`packages/shared`), NEAR Sandbox integration harness (`integration-tests/near-sandbox-js`), and PowerShell build/verification scripts (`scripts/`). `103` Rust unit tests pass across the workspace (`88` in `roastake-oracle-adapter`, `5` in `roastake-coffee-price-index`, `10` in `roastake-shared`), and `23` distinct local NEAR Sandbox leaf tests (`21` release-candidate leaf tests + `1` blank smoke test + `1` legacy M03C.3.2 compatibility test; `25` Node test-runner entries including `2` outer suite wrappers) pass against the M03C.3.7.6 release-candidate adapter WASM (`roastake_oracle_adapter_release_candidate_m03376.wasm`, `378,335` bytes, SHA-256 `a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8`, built via `cargo-near 0.22.0`). Mock OutLayer Sandbox tests do **not** prove live `outlayer.testnet` integration. All coffee prices and balances are demonstration data with no monetary value.

## Mission 01–03C.3.7.6 — OutLayer NEAR Testnet Foundation & Solvency Hardening (Verified)

The technical foundation of ROASTAKE is implemented, verified against local `nearcore 2.14.0` Sandbox with `Mock OutLayer` (live `outlayer.testnet` execution remains pending a separate Testnet authorization mission), and tested:

- **Smart Contract (`contracts/oracle-adapter`):** NEAR Testnet contract that interfaces with OutLayer (`outlayer.testnet::request_execution` via canonical 7-parameter ABI), enforces a verified provisional deposit policy with dynamic OutLayer refund routing to `payer_account_id`, tracks monotonic nonces with replay protection, handles OutLayer worker failure (`b"null"`) safely, enforces the exact 1,024-byte direct yield payload boundary (and rejects 1,025-byte oversized responses), enforces UTF-8 byte caps on `PriceObservation` metadata (`source <= 128 B`, `data_class <= 32 B`), reserves dedicated `STATE` storage margins (`STATE_STORAGE_MARGIN_BYTES = 512`, `STATE_GROWTH_MARGIN_BYTES = 128`), and implements a reserve-backed refund & insurance solvency model:
  - Explicit `UnreconciledFailure` quarantine on `PromiseError::Failed` (preventing unbacked liabilities when `outlayer.testnet` retains deposits) and an explicit external-resolution state machine (`promote_unreconciled_refund` rejects active external-resolution proposals; requesters may acknowledge via `acknowledge_unreconciled_resolved_external` or reject via `reject_unreconciled_external_resolution`, and admins may cancel proposals via `admin_cancel_unreconciled_external_resolution`).
  - Explicit `flush_storage()` across all **7** cached collections (of **8** total persistent `near_sdk::store` collections, including the uncached `LookupSet` `authorized_requesters`) before solvency checks so `env::storage_usage()` always reflects persisted trie bytes.
  - Segregated reserves (`minimum_operating_reserve` via `fund_operating_reserve()`, `refund_reserve_balance` via `fund_refund_reserve()` / `admin_designate_refund_reserve()`, and `insurance_reserve_balance` via `fund_insurance_reserve()`), plus explicit `total_in_flight_insurance_liability` and `total_unresolved_insurance_liability` tracking (`max_active_insurance_compensations` default `32`, hard cap `100`) with missing-callback recovery (`mark_insurance_compensation_disputed`, `acknowledge_insurance_compensation_received`, and late `on_insurance_transfer` acceptance). Unacknowledged `Disputed` liabilities conservatively encumber `encumbered_backing_floor` (`M-01` accepted liveness limitation, operationally mitigated via `fund_refund_reserve()`).
  - Non-destructive administrative settlement/resolution proposals (`admin_reconcile_stuck_claim(..., ConfirmedSuccess)`, `admin_propose_disputed_settlement`, `admin_resolve_unreconciled_external`) that preserve user claims and liabilities until claimant acknowledgement (`acknowledge_disputed_transfer_received`, `acknowledge_unreconciled_resolved_external`), plus two-step admin transfer (`propose_admin` + `accept_admin`) and decoupled `Disputed` claim liveness so historical disputed claims never block new available refunds.
  - Release-candidate WASM artifact: `integration-tests/near-sandbox-js/artifacts/roastake_oracle_adapter_release_candidate_m03376.wasm` (`378,335 bytes`, SHA-256 `a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8`).
- **WASI Worker (`workers/coffee-price-index`):** Rust WASI application (`wasm32-wasip2`) executing inside Intel TDX enclaves on OutLayer. Evaluates `GESHA-DEMO/USD` synthetic demo fixtures, reading from stdin and writing strict JSON to stdout with diagnostic logs on stderr. Pinned SHA-256: `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6`.
- **Shared Schemas (`packages/shared`):** Canonical types, OutLayer `WasmUrl` / `ResourceLimits` / `RequestParams` specifications, extraction wrappers, and fixed-point math routines.
- **Reproducible Tooling (`scripts/`):** PowerShell scripts for environment verification, target management, compilation, SHA-256 calculation, artifact inspection, and automated test execution on Windows.

## Vision

Build a unified platform connecting coffee price discovery, trading, procurement and real-world coffee commerce.

The long-term platform may include:

- **Roastake Perps:** perpetual markets for coffee varieties and other commodities.
- **Coffee Price Indices:** transparent, methodology-driven reference prices by variety, origin, process and quality.
- **Roastake Spot:** a marketplace for coffee assets and, subject to verification and legal design, inventory-backed tokenized coffee.
- **Roastake Trade:** B2B procurement, supplier quotations and commercial settlement.
- **Roastake Treasury & Finance:** future financing, collateral and yield products for coffee-related businesses.

These modules will be developed in stages. The initial repository foundation avoids assumptions that would prevent the broader platform from being added later.

## Phase 1 — NEAR Testnet Perpetuals

The first deliverable is a working perpetual-market prototype on **NEAR Testnet only**.

Initial market: `GESHA-DEMO/USD`

Planned MVP components:

- OutLayer integration for verifiable off-chain computation and oracle data delivery.
- A Roastake Oracle Adapter and on-chain price registry.
- A clearly labelled demonstration coffee index. Synthetic/demo values must never be presented as live or official market quotations.
- A demo collateral token with no monetary value.
- Long and short positions, position accounting, margin, PnL, funding and test liquidations.
- A trading interface, transaction history and Testnet Explorer links.
- Automated tests, security checks and reproducible deployment instructions.

All token balances, prices and trading activity in this phase are for testing only. No real funds, mainnet contracts or mainnet transaction flows are part of the MVP.

## Oracle and Price-Data Principles

OutLayer is the initial oracle/computing integration target for NEAR Testnet. The application keeps the oracle integration behind a provider adapter so that other providers can be evaluated later without rewriting the trading engine.

An execution attestation does not, by itself, prove that an external market quotation is accurate. Price methodology, source quality, freshness, outlier handling, signer authorization and failure behaviour must be documented and tested.

Initial demonstration prices are synthetic unless explicitly sourced and labelled otherwise. ICO, ICE and commercial supplier integrations must not be claimed until access, source terms and actual data ingestion have been verified.

## Development Principles

1. **Testnet-only by default:** reject mainnet configuration and transaction destinations in the MVP.
2. **Security before leverage:** use fixed-point arithmetic, explicit margin rules, stale-price protection and fail-closed oracle handling.
3. **Small, modular components:** separate the oracle adapter, price registry, collateral token, perpetual engine and frontend.
4. **Verifiable claims:** document what is implemented, simulated, planned and independently verified.
5. **Preserve the wider vision:** build the perpetuals module as the first part of Roastake, not as a separate product that has to be replaced later.

## Planned Repository Structure

```text
apps/
  web/                  # Roastake application and trading terminal
contracts/
  oracle-adapter/       # OutLayer integration and price registry
  perpetuals/           # Positions, margin, funding and liquidations
  demo-token/           # Test collateral token
workers/
  coffee-price-index/   # External data collection and index calculation
packages/
  shared/               # Shared types, schemas and utilities
  oracle-client/        # Oracle request and response client
docs/
scripts/
```

## License

Licensed under the [Apache License 2.0](LICENSE).
