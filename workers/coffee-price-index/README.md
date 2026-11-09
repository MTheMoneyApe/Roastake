# Coffee Price Index Worker

Custom WASI program executed by OutLayer. It reads JSON from stdin and writes one JSON response to stdout.

## First milestone

Implement a deterministic `fixture` mode for `GESHA-DEMO/USD`. This proves the WASM build, hash-pinned upload, OutLayer execution and NEAR callback without claiming access to real coffee-market data.

## Later milestone

Add controlled HTTPS retrieval only after confirming the WASI HTTP capability, network allowlist and source terms for each provider. Record provider, observation timestamp, unit, currency and source count. Normalize and aggregate only comparable instruments.

## Reproducibility

- Build a release WASM for the build target supported by the selected OutLayer runtime.
- Record the artifact SHA-256.
- Publish to Testnet FastFS.
- Call OutLayer with the exact URL and matching hash.
- Inspect the TEE attestation and compare its WASM/input/output hashes with the release artifact and transaction.

Never commit compiled WASM binaries, API keys or private credentials.
