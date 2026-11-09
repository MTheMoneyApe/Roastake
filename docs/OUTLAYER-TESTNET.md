# OutLayer on NEAR Testnet

## Official references

- NEAR integration: https://app.outlayer.ai/docs/near-integration
- Getting started: https://app.outlayer.ai/docs/getting-started
- WASI app guide: https://app.outlayer.ai/docs/wasi
- Example projects: https://app.outlayer.ai/docs/examples
- TEE attestation: https://app.outlayer.ai/docs/tee-attestation
- Official examples: https://github.com/out-layer/oracle-example

## Network

- OutLayer contract: `outlayer.testnet`
- FastFS Testnet: `test.fastfs.io`
- NEAR network: `testnet` only

Do not copy Mainnet identifiers, RPC endpoints, secrets or deployment commands into this project.

## Direct execution

The current OutLayer NEAR integration documents `request_execution`. It accepts:

- `source`: a registered `Project` or direct `WasmUrl` source.
- `resource_limits`: required resource limits.
- `input_data`: JSON serialized as a string and passed to WASM stdin.
- Optional `secrets_ref`, `response_format`, `payer_account_id`, and `params`.

For direct WASM execution, pin both the HTTPS URL and SHA-256 hash. The build target must match the artifact, such as `wasm32-wasip1` or `wasm32-wasip2`.

## Callback rules

The contract must treat the OutLayer response as an asynchronous promise result. It must:

- Validate that the callback is a private self-callback.
- Match the callback to a pending request nonce and market.
- Handle promise failure without changing the last valid price.
- Reject malformed, stale, replayed, wrong-market and out-of-range results.
- Use fixed-point integer values for financial state.

Do not implement callback arguments from memory. Before coding, inspect the current official NEAR integration example and match the exact `near-sdk` version and response type.

## Payment and resource estimates

OutLayer charges based on execution resources and refunds unused funds. Query the documented cost-estimation method and set explicit conservative resource limits. Do not hardcode an unverified deposit as a guaranteed price. Any Testnet transaction or paid operation requires the user's explicit approval.

## Security and data quality

TEE attestation proves the integrity of the pinned code and execution inputs/outputs; it does not prove that a third-party coffee quote is true. Keep source provenance, timestamp, data classification and methodology in the worker response.

## First acceptance test

Use a deterministic fixture, not a claimed live coffee quotation. The acceptance test is complete only when a real Testnet transaction has invoked `outlayer.testnet`, the callback has completed, the stored observation can be queried on-chain, and the corresponding execution attestation can be inspected.
