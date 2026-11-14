# ROASTAKE indexer deployment notes

**Scope:** read-only market-history service. Perps execution remains direct from the frontend wallet to NEAR Testnet RPC.

## Required environment

```env
HOST=0.0.0.0
PORT=8787
CORS_ORIGIN=https://roastake-ex387b8yd-astrym.vercel.app,https://roastake-git-feat-degen-mode-easter-egg-astrym.vercel.app,https://roastake.vercel.app
```

The service also accepts legacy `PERPS_INDEXER_HOST`, `PERPS_INDEXER_PORT` and `PERPS_INDEXER_CORS_ORIGINS` values as fallbacks. Standard `HOST`, `PORT` and `CORS_ORIGIN` take priority.

Use a persistent Node.js host with a writable disk mounted at `PERPS_INDEXER_DATA_DIR`. The store writes JSONL events and cursor/health state; one process may own a data directory at a time. Vercel serves the frontend and is not the recommended host for this persistent Node service.

After deployment, set the frontend build variable `VITE_PERPS_HISTORY_API_URL` to the service's HTTPS base URL and verify `/health`, `/v1/perps/markets` and all three `/history` endpoints. Do not expose server credentials in frontend environment variables.
