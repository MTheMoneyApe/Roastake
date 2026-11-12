# ROASTAKE Persistent NEAR Testnet Market History Indexer

**Copyright © 2026 @MTheMoneyApe. All rights reserved.**  
**License:** [`ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0`](../../LICENSE-ROASTAKE-PERPS.md) (`SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only`)

Standalone, read-only NEAR Testnet indexing service for `perps-v1.roastake-dev-mta.testnet` covering all three registered coffee perpetual markets (`COFFEE-ARABICA-TESTNET/USD`, `COFFEE-ROBUSTA-TESTNET/USD`, `COFFEE-GESHA-TESTNET/USD`), persisting verified NEP-297 events into isolated market stores, tracking contiguous coverage vs. explicit historical gaps (`seedBackfill` vs. `forwardScan` / `range_backfill`), enforcing bounded catch-up + adaptive rate limiting, and serving HTTP JSON endpoints for `lightweight-charts@5.3.0`.

---

## 1. Architecture & Data Sources

| Component | Module | Responsibility |
| --- | --- | --- |
| **Config & Policy Guard** | `src/config.mjs` | Fail-closed Testnet enforcement (blocks `.near` Mainnet hosts/accounts, blocks CORS `*`, binds HTTP strictly to loopback, configures safe RPM & bounded catch-up limits). |
| **Coverage Algebra** | `src/coverage.mjs` | Separates discrete `seedBackfill` (tx-by-hash M04.6-G) from contiguous `block_scan` segments (`forward_scan`, `range_backfill`), upgrades legacy `v1` cursors without data loss, computes exact `coverageGaps`, and keeps `coverageComplete: false` whenever any interval remains unverified. |
| **Adaptive Rate Limiter** | `src/rate-limiter.mjs` | Single-flight RPM pacing, rolling 60s request counter, and exponential backoff with jitter respecting `Retry-After` headers strictly. |
| **Finalized Block & RPC Reader** | `src/block-source.mjs` | Polls FastNear NEAR Data (`https://testnet.neardata.xyz/v0/last_block/final` & `/v0/block/{height}`) with optional `Authorization: Bearer` server-side `FASTNEAR_API_KEY`, parses `Retry-After` (seconds/HTTP-date), separates `429` (`RATE_LIMIT`) from `404` (`BLOCK_NOT_FOUND`), plus NEAR Testnet RPC (`EXPERIMENTAL_tx_status` + `block` + optional archival verification). |
| **Unified Event Parser** | `src/event-parser.mjs` | Validates NEP-297 `EVENT_JSON` (`standard: "roastake-perps"`, `version: "1.0.0"`), scans both `shards[].chunk.transactions[].outcome` (local) and `shards[].receipt_execution_outcomes` (cross-shard), checks finality, market ID, Testnet accounts, price bounds, and records rejections separately. |
| **Seed & Resumable Range Backfill** | `src/backfill.mjs` | `importM046GBackfill` imports the 7 verified M04.6-G txs; `reconcileHistoricalRange` scans `[fromHeight..toHeight]` in bounded resumable chunks persisting events + cursor segments block-by-block. |
| **Durable JSONL Store** | `src/history-store.mjs` | Append-only isolated stores (`data/perps-indexer/events.jsonl` for Arabica, `markets/robusta/`, and `markets/gesha/`), `writer.lock` exclusive process lock, trailing partial-line truncation crash recovery, atomic `cursor.json` / `health.json`, and deduplicated in-memory indices (`eventId` & oracle sequence). |
| **OHLC & Volume Builder** | `src/ohlc-builder.mjs` | Builds `ORACLE SAMPLE OHLC` strictly from non-empty oracle intervals without interpolation, and keeps execution volume `unavailable` unless exact close size/notional fields exist on-chain (`contracts/perpetuals/src/events.rs`). |
| **Read-Only HTTP API** | `src/http-server.mjs` | Serves `/health`, `/v1/perps/markets`, and `/v1/perps/{arabica,robusta,gesha}/{observations,executions,candles,volume,history}` on `127.0.0.1:8787` with strict CORS (`http://127.0.0.1:3000`, `http://localhost:3000`). |
| **Service Orchestrator** | `src/indexer.mjs` | Captures anchor `H0`, runs `H0` + M04.6-G backfill, executes bounded sequential catch-up (`catchUp`) and `--backfill-range` batches, differentiates temporary near-tip/archival-verified `404` propagation lag from permanent missing blocks, and persists error state without skipping heights. |

---

## 2. Running Locally

```bash
cd services/perps-indexer

# Run unit, coverage-reconciliation, backoff & API integration tests (0 live RPC txs)
node --test test/*.test.mjs

# One-shot initialization + sync, then exit cleanly
node src/indexer.mjs --once

# Bounded sequential forward catch-up (default up to 500 blocks/run with safe pacing)
node src/indexer.mjs --once --catch-up --max-blocks=100

# Resumable bounded historical range backfill (persists every inspected block)
node src/indexer.mjs --backfill-range=277174473..277186960 --max-blocks=50

# Continuous local polling daemon + read-only API on http://127.0.0.1:8787
node src/indexer.mjs
```

Runtime files live in `../../data/perps-indexer/` (`events.jsonl`, `cursor.json`, `health.json`, `writer.lock`), excluded by `.gitignore`. Stop cleanly with `Ctrl+C` (`SIGINT`) / `SIGTERM` to flush cursor and remove `writer.lock`.

### Optional Environment Variables (Server Process Only)

- `FASTNEAR_API_KEY`: Optional server-only FastNear bearer key (`Authorization: Bearer ...`). Live measurement of unauthenticated public `testnet.neardata.xyz` showed HTTP `429` (`Retry-After: 0`) enforcement after **~30 requests** per sliding window (~`100 ms` avg RTT). Never expose `FASTNEAR_API_KEY` to `Roastake-Frontend`, URLs, logs, or version control.
- `PERPS_INDEXER_POLL_MS` (default `2500`), `PERPS_INDEXER_MAX_BLOCKS_PER_CYCLE` (default `25`), `PERPS_INDEXER_MAX_CATCHUP_BLOCKS` (default `500`), `PERPS_INDEXER_MAX_RPM` (default `120`), `PERPS_INDEXER_BLOCK_PACING_MS` (default `500`), `PERPS_INDEXER_PORT` (default `8787`), `PERPS_INDEXER_HOST` (must remain loopback `127.0.0.1` or `localhost`), `PERPS_INDEXER_DATA_DIR`, `PERPS_INDEXER_ARCHIVAL_RPC_URLS`.

---

## 3. Honesty & Schema Guarantees

- **Empirical FastNear Public Limits:** Measured unauthenticated `testnet.neardata.xyz` trips HTTP `429 (Retry-After: 0)` after 30 consecutive block fetches; recovering full unindexed multi-hour spans (such as `277174473..277186960`) without a paid server key requires bounded chunked execution and remains explicitly surfaced in `coverageGaps` (`coverageComplete: false`) until verified.
- **Gap Safety:** Discrete `seedBackfill` M04.6-G txs are never disguised as contiguous block scans. Unscanned inter-segment gaps stay in `coverageGaps` (with `/health.status: "degraded"` while `coverageComplete === false` even when `providerStatus === "healthy"`). Temporary near-tip or archival-verified `404`s retry with backoff; confirmed missing blocks halt forward progress (`halted_historical_gap`) without skipping heights.
- **No Fake Candles or Volume:** `ORACLE SAMPLE OHLC` only enables when `>= 6` non-empty buckets and `>= 2` multi-sample buckets exist without gap fills. `PositionClosedEvent` and `PositionLiquidatedEvent` omit `size_e8` and `notional_e8` on-chain (`contracts/perpetuals/src/events.rs`), so aggregated trading volume stays honestly `unavailable`.
