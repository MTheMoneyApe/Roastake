# ROASTAKE — Finance infrastructure for the coffee economy

ROASTAKE explores how NEAR can connect coffee price risk management with a wider coffee-finance ecosystem: perpetual markets, test-token swaps, traceability concepts and, in later phases, verified real-world inventory and commercial settlement.

## Current evaluation release

The active hackathon implementation is maintained on the **`hackathon-submission`** branch. That branch's [README](https://github.com/MTheMoneyApe/Roastake/blob/hackathon-submission/README.md) is the authoritative guide to current Testnet contracts, transaction evidence, architecture, reproduction steps and limitations.

The implementation includes three coffee perpetual markets on NEAR Testnet, RHEA Mini Swap using non-monetary test tokens, and a separate Oracle Adapter / OutLayer workstream. Perps prices are operator-controlled synthetic references, not live market quotations. The Marketplace, Coffee Passport, Roast and Redeem flows remain browser-only illustrations; physical coffee custody and redemption are not verified.

## Problem and direction

Coffee participants face disconnected tools for hedging, purchasing and following a lot from producer to roaster. ROASTAKE's long-term goal is an accessible financial layer for that economy, starting with a small, verifiable NEAR Testnet perpetuals prototype and expanding only when data, custody, legal and commercial integrations are ready.

## Source-of-truth boundary

- `main` remains the repository baseline.
- `hackathon-submission` contains the current hackathon code and its verified implementation documentation.
- All Testnet assets and transactions have no monetary value.
- No Mainnet release or independent third-party security audit is claimed.

For the current Preview, frontend source and evaluation instructions, see [MTheMoneyApe/Roastake-frontend](https://github.com/MTheMoneyApe/Roastake-frontend).
