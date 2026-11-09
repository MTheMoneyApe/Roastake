# Testnet-only policy

Roastake Phase 1 is restricted to NEAR Testnet.

## Allowed

- NEAR Testnet RPC and explorer.
- `*.testnet` account identifiers verified against the current network.
- OutLayer `outlayer.testnet`.
- Testnet-only demo token balances and synthetic coffee prices.
- Local unit tests and offline builds.

## Forbidden

- NEAR Mainnet RPC, accounts, contracts or transaction signing.
- OutLayer `outlayer.near`.
- Real funds, real collateral or production trading.
- Presenting synthetic fixtures as official or live coffee prices.
- Deploying, initializing contracts, funding accounts or sending transactions without explicit user approval.

## Required safeguards

- Centralize network configuration and default to `testnet`.
- Validate account IDs and RPC host before creating transactions.
- Add tests that reject mainnet network identifiers and endpoints.
- Keep secrets out of source control and logs.
- Document any exception before implementation; do not silently weaken this policy.
