# Coffee Index — GESHA-DEMO/USD

## Initial market

- Symbol: `GESHA-DEMO/USD`
- Classification: synthetic demonstration data until a real source is explicitly verified.
- Unit: USD per kg of green coffee.
- Storage: integer fixed-point; no floating-point financial arithmetic.

## Worker input contract

The WASM worker reads one JSON object from stdin. Initial schema:

```json
{
  "schema_version": 1,
  "market": "GESHA-DEMO/USD",
  "mode": "fixture"
}
```

## Worker output contract

The WASM worker writes exactly one JSON object to stdout:

```json
{
  "schema_version": 1,
  "market": "GESHA-DEMO/USD",
  "price": "1850000000",
  "decimals": 8,
  "unit": "USD/kg",
  "source": "roastake-fixture",
  "source_count": 1,
  "observed_at": 0,
  "data_class": "SYNTHETIC_DEMO"
}
```

The price above is an illustrative fixture only, not a market quote. Production code must supply a real observation timestamp and verified source metadata. A fixture must never be labelled LIVE, OFFICIAL, ICO or ICE.

## Future real-data methodology

Before adding external quotes, document:
- Provider identity, source URL and access terms.
- Instrument and grade equivalence (e.g. exchange futures are not specialty Gesha spot quotations).
- Currency and unit normalization.
- Quote timestamp, timezone and maximum age.
- Outlier and missing-source policy.
- Aggregation formula and minimum independent source count.
- Market closure and stale-data behaviour.

The index adapter must fail closed when required sources are missing or stale. A TEE cannot establish the truth or independence of source data.

---

## Mission 01 Status — Implemented

**Branch:** `feat/outlayer-near-testnet-foundation`

### Worker implemented

Location: [`workers/coffee-price-index/`](../workers/coffee-price-index/)

| File | Purpose |
|------|---------|
| `src/main.rs` | WASI entry point; reads stdin, writes JSON to stdout, diagnostics to stderr |
| `src/engine.rs` | `process_request()` / `process_raw_json()` — input validation, dispatch, output serialization |
| `src/provider.rs` | `PriceProvider` trait; `FixtureProvider` (active); `IcoProvider`, `IceProvider`, `VerifiedSupplierProvider` (fail-closed stubs for future integration) |

### Build target

```
wasm32-wasip2
```

### Current binary

| Property | Value |
|----------|-------|
| File | `target/wasm32-wasip2/release/roastake-coffee-price-index.wasm` |
| Size | 143,075 bytes (139.72 KB) |
| SHA-256 | `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6` |

> [!IMPORTANT]
> This SHA-256 must be pinned in the on-chain `MarketConfig` and in every OutLayer `WasmUrl` request. Any binary whose SHA-256 does not match is rejected.

### Supported markets and modes

| Market | Mode | `data_class` |
|--------|------|-------------|
| `GESHA-DEMO/USD` | `fixture` | `SYNTHETIC_DEMO` |

All other markets and modes return an error. The contract callback rejects any response that lacks `data_class`.

### Extension points

The `PriceProvider` trait is defined and ready for future integrations. Each provider is a separate struct that implements `fn fetch(&self, market: &str) -> Result<WorkerOutput, String>`. Current stubs:

- `IcoProvider` — International Coffee Organization data feed (not yet implemented; fails closed)
- `IceProvider` — ICE Futures exchange adapter (not yet implemented; fails closed)
- `VerifiedSupplierProvider` — Verified green-coffee supplier quotes (not yet implemented; fails closed)

No unverified external price source will be activated without full provider documentation, source verification and an updated methodology section in this file.

### Data classification rules

| `data_class` | Description | `observed_at` policy |
|-------------|-------------|----------------------|
| `SYNTHETIC_DEMO` | Deterministic fixture; no monetary value | `0` accepted by contract (special case) |
| `LIVE_VERIFIED` | Real quotation from verified source | Must be within `max_price_age_seconds` of current time |
| `LIVE_AGGREGATED` | Aggregated from multiple verified sources | Must be within `max_price_age_seconds` of current time |

The contract callback enforces these rules: a `SYNTHETIC_DEMO` response with `observed_at = 0` is accepted; any other class with `observed_at = 0` is rejected as stale.

### Worker test results (Mission 01 Final Alignment)

```
cargo test --workspace --features near-sdk/unit-testing

running 5 tests (roastake-coffee-price-index)
test engine::tests::test_valid_fixture_request          ... ok
test engine::tests::test_process_raw_json_invalid_syntax ... ok
test engine::tests::test_unknown_market                  ... ok
test engine::tests::test_unsupported_mode               ... ok
test engine::tests::test_process_raw_json_exact         ... ok

running 22 tests (roastake-oracle-adapter)
... all 22 passed

running 10 tests (roastake-shared)
... all 10 passed

test result: ok. 37 passed; 0 failed
```
