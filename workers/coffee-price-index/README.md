# Coffee Price Index Worker

Custom WASI program executed by OutLayer. It reads a typed JSON request from stdin and writes one JSON result to stdout.

## Existing non-live modes

### Gesha fixture

- Mode: `fixture`
- Market: `GESHA-DEMO/USD`
- Data class: `SYNTHETIC_DEMO`
- Fixed sample price: USD 18.50/kg
- Smoke-test only; never eligible for Perps execution.

### IMF Arabica monthly benchmark

- Mode: `public_benchmark`
- Market: `ARABICA-MONTHLY-IMF/USD`
- Source: IMF PCPS `PCOFFOTM`
- Data class: `PUBLIC_BENCHMARK_MONTHLY`
- Monthly historical cash-price benchmark, not live ICE futures and not eligible for Perps execution.

## Live Testnet connector implementation

The `live_index` mode has provider adapters for two real ICE coffee futures data vendors:

- Commodities-API: exact dated symbols such as `KC...` (Arabica) and `RM...` (Robusta).
- Barchart OnDemand: the exact same dated ICE contract symbol.

The worker requires both vendors' observations when the on-chain market configuration sets `min_source_count = 2`. It verifies that both vendors returned the same explicitly configured expiry, normalizes their different exchange quote units into USD/kg with integer fixed-point arithmetic, rejects stale/future quotes and non-intraday Barchart mode, checks source spread, and aggregates the accepted values. The aggregate timestamp is the oldest timestamp among included quotes.

**No fixture or sandbox fallback exists in `live_index`.** If a credential, quote, timestamp, contract, freshness check, or source-count requirement fails, the worker rejects the execution and the Oracle Adapter retains the previous observation.

### Required OutLayer Testnet environment

Configure these variables in the encrypted OutLayer Testnet secrets profile referenced by the Oracle Adapter market. Never place API credentials in Git, deployment output, or chat.

Secrets:
- `COMMODITIES_API_KEY`
- `BARCHART_API_KEY`

Dated contract settings (both providers query the same expiry):
- `ARABICA_FUTURES_SYMBOL` — explicit Coffee C ICE contract, e.g. a `KC...` code.
- `ROBUSTA_FUTURES_SYMBOL` — explicit ICE Robusta contract, e.g. an `RM...` code.

The worker intentionally has no implicit front-month default. The operator must select a liquid, non-expired contract and verify its first-notice/last-trade calendar before configuring or rolling it. Do not use a continuous contract at one vendor and a dated contract at the other.

### Network policy

The embedded manifest allows only exact HTTPS hostnames:
- `api.imf.org`
- `commodities-api.com`
- `ondemand.websol.barchart.com`

No wildcard hosts are enabled.

### Market-data semantics and limitations

- Commodities-API's `latest` cadence depends on its subscription tier (documented at 60 minutes, 10 minutes, or 60 seconds).
- Barchart `mode` must indicate real-time (`R`) or delayed intraday (`I)); end-of-day (`D`) results are rejected. The Barchart `tradeTimestamp` is used for freshness.
- The two services are separate vendors distributing quotations for the same ICE contract. They are not two independent exchanges or two independent underlying price-discovery venues. `source_count` means the number of vendor observations accepted by the worker.
- Data redistribution and vendor licensing terms still apply on Testnet.
- A successful TEE attestation proves the worker execution and its input/output hashes; it does not prove that an upstream vendor's market data is economically correct.
- These futures prices are not a spot price for every coffee grade, physical lot, origin, or quality.

## Reproducibility and rollout

1. Run the worker's native unit tests.
2. Build the release WASI target supported by OutLayer.
3. Record the artifact SHA-256 and publish to Testnet FastFS.
4. Bind the two vendor keys and dated contract settings in OutLayer Testnet secrets for that exact worker hash.
5. Register separate `live_index` markets in `roastake-oracle-v1.testnet` with `allow_synthetic = false`, `min_source_count = 2`, explicit freshness/spread/price limits, and the pinned WASM URL/hash.
6. Verify the finalized OutLayer attestation and on-chain `get_price` observation.
7. Register new Perps markets using `OracleSourceConfig::ExternalXcc`. Do not mutate the existing operator-sandbox markets.

Never commit compiled WASM binaries, API keys, private credentials, or paid data payloads.
