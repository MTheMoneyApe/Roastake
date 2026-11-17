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
| `SYNTHETIC_DEMO` | Deterministic fixture (`GESHA-DEMO/USD`); no monetary value | `0` accepted by contract (special case) |
| `PUBLIC_BENCHMARK_MONTHLY` | Historical monthly public benchmark (`ARABICA-MONTHLY-IMF/USD` only, source `imf-pcps-pcoffotm`, `source_count = 1`); not live pricing | Final UTC second (`23:59:59Z`) of `TIME_PERIOD` month; market-specific `max_price_age_sec = 7,776,000` (90 days) |
| `LIVE_VERIFIED` | Real quotation from verified commercial source (`live_index` mode only; forbidden for `GESHA-DEMO/USD` and `ARABICA-MONTHLY-IMF/USD`) | Must be within live market `max_price_age_sec` of current time |
| `LIVE_AGGREGATED` | Aggregated from multiple verified sources (`live_index` mode only; `source_count >= 2`) | Must be within live market `max_price_age_sec` of current time |

The contract callback enforces these rules fail-closed: unknown `data_class` values, `data_class > 32` UTF-8 bytes, synthetic fixtures outside `GESHA-DEMO/USD`, `PUBLIC_BENCHMARK_MONTHLY` outside `ARABICA-MONTHLY-IMF/USD`, and zero timestamps on non-fixture classes are rejected.

---

## IMF PCPS Historical Monthly Benchmark (`ARABICA-MONTHLY-IMF/USD`)

Enabled for **NEAR Testnet only** (`budget: €0`, no API key, no `secrets_ref` required; `MarketSecretsRef` support remains intact on `MarketConfig` for future authenticated feeds):

- **Market symbol:** `ARABICA-MONTHLY-IMF/USD` (`mode = "public_benchmark"`)
- **Provider source tag:** `imf-pcps-pcoffotm` (`source_count = 1`)
- **Data classification:** `PUBLIC_BENCHMARK_MONTHLY` (historical monthly publication, ~1–2 month lag)
- **Endpoint (HTTP GET, allowlisted host `api.imf.org` only):**
  `https://api.imf.org/external/sdmx/3.0/data/dataflow/IMF.RES/PCPS/+/G001.PCOFFOTM.USD.M?lastNObservations=1`
- **Units & deterministic math (`10^8` output `USD/kg`):**
  1. Upstream series `PCOFFOTM` reports US cents per pound (`USc/lb`, unscaled `SCALE = 0`) with 13+ fractional digits in IMF JSON (e.g. `"330.1219047619048"`).
  2. `parse_decimal_e12_round_half_up()` parses up to 24 fractional digits into `10^-12` `USc/lb` using exact decimal round-half-up at the 13th fractional digit (`>= '5'` rounds up).
  3. `convert_usc_per_lb_e12_to_usd_per_kg_e8()` converts `USc/lb` to `USD/kg` (`1 lb = 0.45359237 kg` exact SI) via checked `u128` arithmetic and half-up division (zero floating-point arithmetic).
- **Timestamp convention:** Derived strictly from SDMX `TIME_PERIOD` (`YYYY-Mmm`, e.g. `"2026-M09"`) as the **final UTC second of that calendar month** (`YYYY-MM-last_dayT23:59:59Z` = next month day 1 `00:00:00Z - 1 s`), handling leap years cleanly. The worker never substitutes current wall-clock time for missing or unfinalized observation periods.
- **Initialization & staleness policy:** `RoastakeOracleAdapter::init()` immediately registers **both** `GESHA-DEMO/USD` (`fixture`, `3600 s`) and `ARABICA-MONTHLY-IMF/USD` (`public_benchmark`, `7,776,000 s` = 90 days) with an explicit, non-zero 64-char hex `initial_wasm_sha256` (all-zero placeholders and missing hashes are rejected fail-closed). Freshness rules for `GESHA-DEMO/USD` (`3600 s`) and `live_index` markets (`<= 86,400 s`) remain strictly unrelaxed.
- **Fail-closed error handling:** Missing/malformed JSON, empty series/observations, `SCALE != 0`, non-positive prices, HTTP errors, timeouts, or oversized payloads (`> 512 KiB`) return an error immediately and never fall back to synthetic fixture data.

### Mandatory Attribution & Disclosure

> Source: International Monetary Fund (IMF), Primary Commodity Price System (PCPS), PCOFFOTM — ICO Other Mild Arabicas New York cash monthly average.
>
> **Disclosure:** This benchmark reflects historical monthly cash-average statistics, **not live ICE Coffee C futures** and **not real-time pricing**. Display or redistribution of IMF PCPS statistics on NEAR Testnet is for public hackathon/demonstration use only, requires preservation of IMF source attribution, and **does not imply IMF endorsement** (underlying ICO cash averages are subject to IMF/third-party upstream terms).

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
