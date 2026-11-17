# Coffee Price Index Worker

Custom WASI program executed by OutLayer. It reads JSON from stdin and writes one JSON response to stdout.

## First milestone

Implement a deterministic `fixture` mode for `GESHA-DEMO/USD`. This proves the WASM build, hash-pinned upload, OutLayer execution and NEAR callback without claiming access to real coffee-market data.

## Implemented external benchmark (`ARABICA-MONTHLY-IMF/USD`)

Controlled HTTPS retrieval via WASI Preview 2 (`wasi:http/outgoing-handler`) is implemented for IMF's free, keyless Primary Commodity Price System (PCPS) endpoint (`outlayer.manifest.json` pins strictly `["api.imf.org"]`):

- **Mode / Market:** `"public_benchmark"` for `ARABICA-MONTHLY-IMF/USD` (`data_class = "PUBLIC_BENCHMARK_MONTHLY"`, `source = "imf-pcps-pcoffotm"`, `source_count = 1`).
- **Methodology:** Deterministic fixed-point conversion from `USc/lb` (`10^-12` half-up) to `USD/kg` (`10^8`), deterministic month-end UTC timestamp (`YYYY-Mmm` -> month's final UTC second), no fixture fallback on failure.
- **Attribution:** Source: International Monetary Fund (IMF), Primary Commodity Price System (PCPS), PCOFFOTM — ICO Other Mild Arabicas New York cash monthly average. Historical monthly data, not live ICE Coffee C futures, not real-time pricing, and does not imply IMF endorsement.

## Later commercial milestones

Verified commercial feeds (`IcoProvider`, `IceProvider`, `VerifiedSupplierProvider` under `live_index`) remain fail-closed until commercial redistribution licenses, secrets provisioning, and quorum agreements are approved.

## Reproducibility

- Build a release WASM for the build target supported by the selected OutLayer runtime.
- Record the artifact SHA-256.
- Publish to Testnet FastFS.
- Call OutLayer with the exact URL and matching hash.
- Inspect the TEE attestation and compare its WASM/input/output hashes with the release artifact and transaction.

Never commit compiled WASM binaries, API keys or private credentials.
