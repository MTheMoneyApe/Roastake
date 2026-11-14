# ROASTAKE indexer deployment notes

**Scope:** read-only market-history service. Perps execution remains direct from the frontend wallet to NEAR Testnet RPC.

## Required environment

```env
HOST=0.0.0.0
# PORT is injected by Railway/Fly.io; locally the default is 8787.
CORS_ORIGIN=https://roastake-ex387b8yd-astrym.vercel.app,https://roastake-git-feat-degen-mode-easter-egg-astrym.vercel.app,https://roastake.vercel.app
INDEXER_DATA_DIR=/data
```

The service binds to `HOST` and the platform-provided `PORT`. For local development, omit `INDEXER_DATA_DIR` to retain the repository default `data/perps-indexer`. The previous `PERPS_INDEXER_DATA_DIR` key remains a lower-priority compatibility fallback.

Mount a persistent writable volume at the configured directory (for example, `/data`). The store keeps append-only `events.jsonl`, `cursor.json`, `health.json` and a single-writer lock beneath that directory. Run exactly one indexer replica per data directory; do not attach the same volume to multiple writers.

The indexer defaults to `perps-v1.roastake-dev-mta.testnet` and all three supported markets. Do not set `PERPS_INDEXER_MARKET_ID` unless intentionally switching to a single-market instance. Optional tuning/upstream variables are `PERPS_INDEXER_NEARDATA_URL`, `PERPS_INDEXER_RPC_URLS`, `PERPS_INDEXER_ARCHIVAL_RPC_URLS`, `FASTNEAR_API_KEY`, `PERPS_INDEXER_MAX_RPM`, `PERPS_INDEXER_POLL_MS`, `PERPS_INDEXER_TIMEOUT_MS`, `PERPS_INDEXER_MAX_BLOCKS_PER_CYCLE`, `PERPS_INDEXER_MAX_CATCHUP_BLOCKS`, `PERPS_INDEXER_MAX_CYCLE_MS`, `PERPS_INDEXER_BLOCK_PACING_MS`, `PERPS_INDEXER_NEAR_TIP_404_TOLERANCE` and `PERPS_INDEXER_MAX_TRANSIENT_404_RETRIES`.

After deployment, set the frontend build variable `VITE_PERPS_HISTORY_API_URL` to the service's HTTPS base URL (no trailing path required) and verify `/health`, `/v1/perps/markets` and all three `/history` endpoints. Do not expose server credentials in frontend environment variables.

For the complete Railway setup, including the independent Price Keeper and optional OutLayer ping, see [`CLOUD_DEPLOY.md`](../../CLOUD_DEPLOY.md).
