# Roastake Architecture

## Product direction

Roastake is a coffee-focused commodity-finance platform. The first deliverable is Roastake Perps on NEAR Testnet; later modules may include Coffee Price Indices, Spot, Trade, and Treasury & Finance.

## Phase 1 system boundaries

- `apps/web`: trading terminal and wallet connection.
- `contracts/oracle-adapter`: requests verifiable off-chain computation and stores validated index observations.
- `workers/coffee-price-index`: WASI program that accepts a JSON request on stdin and writes a JSON result to stdout.
- `contracts/perpetuals`: future position, collateral, margin, PnL, funding and liquidation logic. It must not be implemented until the oracle path is proven.
- `contracts/demo-token`: future test collateral only.
- `packages/shared`: shared schemas and fixed-point utilities.

## Oracle execution flow

1. An authorized account requests a refresh from the Roastake oracle adapter.
2. The adapter calls OutLayer's NEAR Testnet contract using `request_execution`.
3. OutLayer fetches the hash-pinned WASM and executes it in its TEE worker.
4. The WASM reads its JSON input from stdin, performs its configured data operation, and writes a structured JSON response to stdout.
5. OutLayer resumes the NEAR transaction through the contract callback.
6. The callback accepts only the expected promise result, validates the request nonce, market, timestamp, schema and price bounds, then stores the observation using integer fixed-point arithmetic.
7. Perpetuals may consume only a fresh, valid observation.

## Trust boundaries

TEE attestation establishes the integrity of the specified WASM, its input and output, and the worker identity. It does not prove that an external quotation is accurate, independent, licensed, or representative. Those properties belong to source selection and index methodology.

## Testnet boundary

The initial release must reject mainnet configuration. All prices, balances, collateral and trades are demonstrations with no monetary value. Never use real funds or production market claims.

## Implementation gates

- Gate 1: verify OutLayer Testnet request and callback contract interface.
- Gate 2: build a minimal deterministic WASI worker and publish its SHA-256-pinned artifact.
- Gate 3: complete one real Testnet request/callback and inspect its transaction and attestation.
- Gate 4: add external coffee data retrieval with source and freshness checks.
- Gate 5: implement the Perps engine only after Gates 1–4 pass.
