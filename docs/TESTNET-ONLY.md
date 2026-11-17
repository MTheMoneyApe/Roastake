# Testnet-only policy

Roastake Phase 1 is restricted to NEAR Testnet.

## Allowed

- NEAR Testnet RPC and explorer.
- `*.testnet` account identifiers verified against the current network.
- OutLayer `outlayer.testnet`.
- Testnet-only demo token balances and synthetic coffee prices.
- Local unit tests and offline builds.

## Forbidden

- NEAR Mainnet RPC, accounts, contracts or transaction signing.
- OutLayer `outlayer.near`.
- Real funds, real collateral or production trading.
- Presenting synthetic fixtures as official or live coffee prices.
- Deploying, initializing contracts, funding accounts or sending transactions without explicit user approval.

## Required safeguards

- Centralize network configuration and default to `testnet`.
- Validate account IDs and RPC host before creating transactions.
- Add tests that reject mainnet network identifiers and endpoints.
- Keep secrets out of source control and logs.
- Document any exception before implementation; do not silently weaken this policy.

---

## Mission 01 Verification

**Branch:** `feat/outlayer-near-testnet-foundation`

### Enforcement points

The Testnet-only policy is enforced at multiple layers in the implemented code:

| Layer | Location | Mechanism |
|-------|----------|-----------|
| Contract init | [`contracts/oracle-adapter/src/lib.rs`](../contracts/oracle-adapter/src/lib.rs) — `init()` | Calls `validate_testnet_account_id()` on `outlayer_contract`; panics if account does not end in `.testnet` |
| Admin update | `set_outlayer_contract()` | Same validation — cannot change to a mainnet account at runtime (`set_admin` is disabled in favor of two-step `propose_admin` + `accept_admin`) |
| Market config | `MarketConfig::validate()` / `add_market()` / `update_market_wasm()` | Enforces `wasm_url` uses `https://` protocol (`<= 256` UTF-8 bytes) and valid 64-char hex `wasm_sha256`; `init()` defaults `wasm_url` to `https://test.fastfs.io/...` |
| Shared validation | [`packages/shared/src/lib.rs`](../packages/shared/src/lib.rs) — `validate_testnet_account_id()` | Rejects any account ID not ending in `.testnet` |

### Automated test coverage

The test `test_mainnet_endpoint_rejection` in `contracts/oracle-adapter/src/tests.rs` verifies:

- Calling `init()` with `outlayer_contract = "outlayer.near"` panics.
- The panic message confirms rejection is due to the mainnet account identifier.

```
test tests::tests::test_mainnet_endpoint_rejection ... ok
```

This test is part of the `88`-test oracle adapter suite (`103` workspace Rust unit tests total across `88` adapter, `5` worker, and `10` shared library tests), complemented by `23` distinct NEAR Sandbox integration leaf tests (`21` release-candidate leaf tests + `1` blank smoke test + `1` legacy compatibility test; `25` entries reported by Node's runner including `2` outer suite wrappers). Note that local NEAR Sandbox integration tests use `Mock OutLayer` and do not prove live `outlayer.testnet` integration.

### Worker URL / FastFS Policy

All `WasmUrl` sources must use `https://` (`wasm_url.len() <= 256`), `init()` defaults `initial_wasm_url` to `https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm`, and worker execution requires exact SHA-256 verification. Mainnet `.near` accounts and endpoints remain blocked by `validate_testnet_account_id()`.

### Secrets policy

- No private keys or seed phrases appear in source files, test fixtures, documentation or build scripts.
- Build scripts output only public artifact hashes (SHA-256).
- Tests use `near_sdk::testing_env!` sandbox — no real accounts or funds involved.

### What is not yet enforced (future work)

- RPC endpoint validation is not yet enforced at the contract level (RPCs are used off-chain only; on-chain code does not select an RPC).
- Ed25519 attestation signature verification has not been implemented because OutLayer's documented per-response signing key and exact payload format were not confirmed at Mission 01 time. This must be resolved before promoting the contract to Mainnet.
