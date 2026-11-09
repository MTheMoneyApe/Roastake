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
