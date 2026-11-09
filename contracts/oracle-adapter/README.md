# Roastake Oracle Adapter

NEAR Rust contract responsible for requesting OutLayer execution and storing validated coffee-index observations.

## Responsibilities

- Maintain an owner/authorized requester policy.
- Initiate `request_execution` on `outlayer.testnet`.
- Track pending requests with unique nonces and market symbols.
- Receive a private callback and handle promise success/failure.
- Validate schema, timestamp, market, price bounds and replay protection.
- Store the latest valid fixed-point observation.

## Not in scope yet

Do not add perpetual positions, leverage, collateral custody or liquidation logic here. Do not accept arbitrary callback callers. Do not trust a price solely because it came from an HTTP endpoint.

## Implementation prerequisite

Read the current OutLayer NEAR integration documentation and example before choosing the exact request and callback response types. Pin compatible `near-sdk` and Rust toolchain versions and add unit tests for every failure path.
