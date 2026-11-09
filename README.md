# Roastake

**Roastake is a commodity-finance platform focused initially on coffee markets.** The project starts with a NEAR Protocol Testnet perpetuals prototype and is designed to grow into the wider Roastake ecosystem.

> **Current development status:** early-stage, Testnet-only prototype. Features described as future phases are roadmap items, not implemented functionality.

## Vision

Build a unified platform connecting coffee price discovery, trading, procurement and real-world coffee commerce.

The long-term platform may include:

- **Roastake Perps:** perpetual markets for coffee varieties and other commodities.
- **Coffee Price Indices:** transparent, methodology-driven reference prices by variety, origin, process and quality.
- **Roastake Spot:** a marketplace for coffee assets and, subject to verification and legal design, inventory-backed tokenized coffee.
- **Roastake Trade:** B2B procurement, supplier quotations and commercial settlement.
- **Roastake Treasury & Finance:** future financing, collateral and yield products for coffee-related businesses.

These modules will be developed in stages. The initial repository foundation should avoid assumptions that would prevent the broader platform from being added later.

## Phase 1 — NEAR Testnet Perpetuals

The first deliverable is a working perpetual-market prototype on **NEAR Testnet only**.

Initial market: `GESHA-DEMO/USD`

Planned MVP components:

- OutLayer integration for verifiable off-chain computation and oracle data delivery.
- A Roastake Oracle Adapter and on-chain price registry.
- A clearly labelled demonstration coffee index. Synthetic/demo values must never be presented as live or official market quotations.
- A demo collateral token with no monetary value.
- Long and short positions, position accounting, margin, PnL, funding and test liquidations.
- A trading interface, transaction history and Testnet Explorer links.
- Automated tests, security checks and reproducible deployment instructions.

All token balances, prices and trading activity in this phase are for testing only. No real funds, mainnet contracts or mainnet transaction flows are part of the MVP.

## Oracle and price-data principles

OutLayer is the initial oracle/computing integration target for NEAR Testnet. The application must keep the oracle integration behind a provider adapter so that other providers can be evaluated later without rewriting the trading engine.

An execution attestation does not, by itself, prove that an external market quotation is accurate. Price methodology, source quality, freshness, outlier handling, signer authorization and failure behaviour must be documented and tested.

Initial demonstration prices are synthetic unless explicitly sourced and labelled otherwise. ICO, ICE and commercial supplier integrations must not be claimed until access, source terms and actual data ingestion have been verified.

## Development principles

1. **Testnet-only by default:** reject mainnet configuration and transaction destinations in the MVP.
2. **Security before leverage:** use fixed-point arithmetic, explicit margin rules, stale-price protection and fail-closed oracle handling.
3. **Small, modular components:** separate the oracle adapter, price registry, collateral token, perpetual engine and frontend.
4. **Verifiable claims:** document what is implemented, simulated, planned and independently verified.
5. **Preserve the wider vision:** build the perpetuals module as the first part of Roastake, not as a separate product that has to be replaced later.

## Planned repository structure

The repository may evolve toward the following modular layout:

```text
apps/
  web/                  # Roastake application and trading terminal
contracts/
  oracle-adapter/       # OutLayer integration and price registry
  perpetuals/           # Positions, margin, funding and liquidations
  demo-token/           # Test collateral token
workers/
  coffee-price-index/   # External data collection and index calculation
packages/
  shared/               # Shared types, schemas and utilities
  oracle-client/        # Oracle request and response client
docs/
tests/
```

The structure is a target architecture; directories and components will be created only when needed.

## License

Licensed under the [Apache License 2.0](LICENSE).
