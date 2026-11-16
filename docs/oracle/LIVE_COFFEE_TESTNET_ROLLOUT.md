# ROASTAKE — Live Coffee Oracle on NEAR Testnet

Status: implementation workstream opened on `feat/live-coffee-oracle-testnet`.
Scope: NEAR Testnet only. No Mainnet accounts, contracts, RPCs, deployments, or economic-value claims.

## Objective

Connect real external coffee futures quotations to the existing OutLayer WASI worker and Oracle Adapter, then expose them through separate Perps markets using `ExternalXcc`. Existing `*-TESTNET/USD` operator-sandbox markets and the IMF monthly benchmark remain unchanged.

Target flow:

`ICE futures data vendor(s) -> OutLayer WASI worker -> typed validation/aggregation -> roastake-oracle-v1.testnet -> Perps ExternalXcc markets`

## Selected instruments and unit conversion

| ROASTAKE market family | Reference instrument | Provider symbols | Exchange quote unit | Normalized output |
| --- | --- | --- | --- | --- |
| Arabica | ICE Coffee C | Commodities-API `COFFEE`; Barchart `KC*0` | US cents/lb | USD/kg |
| Robusta | ICE Robusta | Commodities-API `ROBUSTA`; Barchart `RM*0` (Barchart root maps to ICE contract RC) | USD/metric tonne | USD/kg |

The ICE Coffee C contract is quoted in cents per pound. ICE Robusta is quoted in dollars per metric tonne. These must never share a unit conversion.

Commodities-API documents `COFFEE` as an ICE Arabica futures price per pound (shown in US cents/lb) and `ROBUSTA` as an ICE Robusta futures price per tonne. With `base=USD`, its `rates.<symbol>` is the amount of commodity purchased by one USD; invert it to get the quote. Its `timestamp` is the provider's collection time. Subscription plans update the latest endpoint at 60m, 10m, or 60s, depending on tier.

Barchart OnDemand `getQuote` supports futures symbols using active-contract notation `<root>*0`; its response exposes `lastPrice`, `tradeTimestamp`, and `mode` (real-time, delayed, or end-of-day). The worker must reject end-of-day values for an intraday-configured market and must enforce freshness using the exchange trade timestamp, not the server response timestamp.

ICE product references:
- Coffee C: https://www.ice.com/products/15/data
- Robusta: https://www.ice.com/products/37089079

Provider documentation:
- Commodities-API coffee: https://commodities-api.com/symbols/COFFEE
- Commodities-API Robusta: https://commodities-api.com/symbols/ROBUSTA
- Barchart Quote API: https://www.barchart.com/ondemand/api/getQuote

## Important aggregation semantics

Two providers quoting the same ICE contract are two vendor observations, not two independent underlying markets. The output may use `LIVE_AGGREGATED` only to mean that more than one independently retrieved vendor observation participated in the calculation. UI and grant materials must not claim that this creates two independent exchange prices or independent market venues.

For each market:
1. Fetch every configured provider exactly once.
2. Parse the original source timestamp and explicit quote unit.
3. Convert each quote independently to USD/kg using integer fixed-point arithmetic.
4. Reject future, stale, non-positive, malformed, unsupported-mode, or wrong-instrument quotes.
5. Require the configured minimum provider count; no synthetic fallback and no single-provider fallback when the minimum is two.
6. Calculate source spread using the lowest accepted quote as denominator. Reject if spread exceeds the configured maximum.
7. Publish the arithmetic midpoint for two sources or the median for three or more sources.
8. Set `observed_at` to the oldest timestamp among participating quotes, so the aggregate is never presented as fresher than its stalest input.
9. Use a bounded source label and source count; retain provider-level quote details in structured worker diagnostics / off-chain evidence.

The current `WorkerOutput` has only `source` and `source_count`; detailed per-provider provenance should be persisted in off-chain execution evidence until a versioned on-chain schema extension is designed.

## Required environment secrets

Store these only in OutLayer's encrypted secrets system, bound to the exact WASM hash or a project version. Never commit them, paste them into GitHub, or print them in logs:

- `COMMODITIES_API_KEY`
- `BARCHART_API_KEY`

The current provider APIs use query-string keys, so HTTP diagnostics must redact complete query strings and never log raw request URLs. OutLayer network manifest must grant exact hosts only:
- `api.imf.org` (existing monthly benchmark)
- `commodities-api.com`
- `ondemand.websol.barchart.com`

Do not enable broad wildcards or arbitrary outbound network access.

## Testnet market wiring

Create new Perps market IDs distinct from current sandbox markets:
- `COFFEE-ARABICA-REALDATA-TESTNET/USD`
- `COFFEE-ROBUSTA-REALDATA-TESTNET/USD`

Register each with `OracleSourceConfig::ExternalXcc`, pointing to `roastake-oracle-v1.testnet` and the matching new Oracle Adapter market symbol. Never mutate the existing sandbox markets into external markets.

The new Oracle Adapter markets should use:
- `worker_mode = live_index`
- `allow_synthetic = false`
- `min_source_count = 2` for the two-vendor test
- bounded `max_source_spread_bps` (initial test target: 500 bps; tune only from evidence)
- explicit min/max price and upstream age limits
- immutable/pinned WASM URL, SHA-256, and OutLayer secrets reference

Perps execution freshness must be configured against the actual vendor delivery tier and timestamp semantics. Do not set a 300-second execution age if the selected data plan only updates every 10 or 60 minutes. The testnet market should fail closed while stale; do not silently fall back to the operator sandbox.

## Rollout gates

### Gate 1 — Source access (requires owner action)
- Create Commodities-API account and verify that the selected tier exposes both `COFFEE` and `ROBUSTA` at a known update interval.
- Create Barchart OnDemand account and confirm the account can query ICE/US Coffee C (`KC*0`) and ICE/EU Robusta (`RM*0`) with `lastPrice`, `tradeTimestamp`, and `mode`.
- Confirm each account's data-use and redistribution terms. A Testnet-only chain does not waive market-data licensing conditions.
- Keep both API keys private; add them to OutLayer Secrets, not this repository.

### Gate 2 — Worker implementation and deterministic tests
- Implement real provider parsers and exact integer conversions.
- Implement freshness, source-count, spread, and aggregate-price validation.
- Test valid quotes, malformed data, missing keys, HTTP failures, stale/future timestamps, end-of-day mode, source divergence, and one-source outage.
- Confirm live mode can never return a fixture value.

### Gate 3 — OutLayer Testnet execution
- Build the supported WASI target.
- Record artifact SHA-256 and publish only to Testnet FastFS.
- Bind secrets to the exact worker hash.
- Invoke from the existing Testnet Oracle Adapter and verify TEE attestation hashes.

### Gate 4 — Oracle Adapter and Perps wiring
- Register new Oracle Adapter markets and authorize Testnet requester.
- Verify `get_price` returns the expected `LIVE_AGGREGATED` observation.
- Register separate Perps `ExternalXcc` markets.
- Confirm Perps execution uses the on-chain observation and never the old sandbox keeper.

### Gate 5 — Runtime acceptance
- Confirm successful finalized updates over multiple cycles.
- Simulate each source failing independently; two-source markets must stop updating.
- Simulate excessive spread and stale data; Perps must reject opening/increasing/liquidating against stale data according to its policy.
- Verify UI labels identify the source, quote time, delay, and `TESTNET REAL-DATA` status.

## Current blockers requiring account-owner action

1. API credentials and paid/delayed-data entitlements are not available to the coding environment.
2. OutLayer secret provisioning is account-bound and must be completed by an authorized Testnet account owner.
3. Data redistribution permissions must be confirmed with each vendor before public display.

Until all gates pass, the feature is a Testnet engineering prototype, not a production oracle or independently verified physical-coffee price.
