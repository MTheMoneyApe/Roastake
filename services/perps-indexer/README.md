# ROASTAKE Persistent NEAR Testnet Market History Indexer

**Copyright © 2026 @MTheMoneyApe. All rights reserved.**  
**License:** [ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0](../../LICENSE-ROASTAKE-PERPS.md).

Read-only indexer for `perps-v1.roastake-dev-mta.testnet`. It tracks finalized NEAR Testnet blocks and verified NEP-297 events for Arabica, Robusta and Gesha, persists market history to JSONL files, and serves the Perps frontend chart API.

## API

| Endpoint | Purpose |
| --- | --- |
| `GET /health` | Service health, lag and coverage |
| `GET /v1/perps/markets` | Status for all indexed markets |
| `GET /v1/perps/{arabica,robusta,gesha}/history` | Observations, executions, candles, coverage and volume status |
| `GET /v1/perps/{arabica,robusta,gesha}/observations` | Verified oracle observations |
| `GET /v1/perps/{arabica,robusta,gesha}/executions` | Verified event-derived executions |
| `GET /v1/perps/{arabica,robusta,gesha}/candles` | Oracle-sample OHLC where supported by observed samples |
| `GET /v1/perps/{arabica,robusta,gesha}/volume` | Volume availability and evidence |

The API is read-only. It does not accept transactions, private keys or trading instructions. It does not invent prices, candles or gap fills; missing coverage and unavailable volume remain explicit.

## Run locally

Requirements: Node.js 20.15 or newer.

```bash
cd services/perps-indexer
node --test test/*.test.mjs
node src/indexer.mjs
```

## Public hosting configuration

| Variable | Default | Description |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind address; use `0.0.0.0` on container hosts or localhost for local development |
| `PORT` | `8787` | HTTP listen port; platform-injected `PORT` takes priority |
| `CORS_ORIGIN` | Local development origins | Comma-separated exact frontend origins including scheme; wildcard rejected |
| `FASTNEAR_API_KEY` | Empty | Server-only FastNear bearer key for NEAR Data and Transactions API history recovery |
| `PERPS_INDEXER_ACCOUNT_HISTORY_BACKFILL` | Enabled when the key is present | Set `false` to disable transaction-history recovery |
| `PERPS_INDEXER_ACCOUNT_HISTORY_FROM_HEIGHT` | `277174473` | Inclusive starting transaction block height for historical account recovery |
| `PERPS_INDEXER_ACCOUNT_HISTORY_TXS_PER_CYCLE` | `3` | Maximum historical transactions independently verified per poll cycle (1–20) |
| `INDEXER_DATA_DIR` | `data/perps-indexer` | Persistent writable directory; overrides legacy `PERPS_INDEXER_DATA_DIR` |
| `PERPS_INDEXER_POLL_MS` | `2500` | Poll interval |

Example for the ROASTAKE Preview and production origins:

```env
HOST=0.0.0.0
PORT=8787
CORS_ORIGIN=https://roastake-ex387b8yd-astrym.vercel.app,https://roastake-git-feat-degen-mode-easter-egg-astrym.vercel.app,https://roastake.vercel.app
```

Set the public API URL in the frontend build as `VITE_PERPS_HISTORY_API_URL`. Never expose `FASTNEAR_API_KEY` or wallet credentials to Vite or the browser.

**Persistence requirement:** mount a persistent disk at `INDEXER_DATA_DIR` (for example, `/data` on Railway). Legacy `PERPS_INDEXER_DATA_DIR` remains a lower-priority fallback. The indexer writes append-only JSONL events, cursors and health files. Ephemeral serverless filesystems are not a durable deployment target. Run a single writer per data directory because the store uses an exclusive process lock.

## Historical event recovery

When `FASTNEAR_API_KEY` is configured, the indexer can page Testnet account history for the Perps contract from `PERPS_INDEXER_ACCOUNT_HISTORY_FROM_HEIGHT`. Each discovered transaction is fetched by hash from FastNear, then independently checked using finalized NEAR RPC transaction status before its contract receipts are parsed and appended to the per-market stores. Pagination and per-transaction progress are checkpointed in the existing persistent volume, so interrupted runs can resume safely.

This recovers verified contract events; it does **not** mark the intervening blocks as scanned. The API continues to report transaction-history recovery progress separately from contiguous block coverage. The existing forward scanner remains responsible for continuous block coverage.

## Coverage and security

The indexer separates transaction-hash seed backfill, FastNear account-history event recovery and contiguous finalized-block scans, and tracks gaps, lag, rejected events and provider backoff. It uses approved NEAR Testnet endpoints, accepts exact CORS origins only, rejects wildcard CORS and never receives wallet private keys. Restrict public access at the hosting layer if the API is not intended to be anonymous.
