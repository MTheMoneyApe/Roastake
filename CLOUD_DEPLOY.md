# ROASTAKE — 24/7 Railway deployment

This branch prepares two independent Node services from this repository. Deploy each as a separate Railway service from the same GitHub repository and the `hackathon-submission` branch. Keep one replica of each service.

> This guide prepares the deployment configuration; it does not create Railway services, deploy the indexer, start the keeper, or sign NEAR transactions for you. Both services operate against NEAR Testnet.

## 0. Shared service settings

For each Railway service:

- Source repository: `MTheMoneyApe/Roastake`
- Branch: `hackathon-submission`
- Root directory: repository root (`/`)
- Build strategy: the service-specific Dockerfile selected with `RAILWAY_DOCKERFILE_PATH` (this repository also contains Rust contracts, so do not let Railpack infer the runtime from the root `Cargo.toml`).
- Node runtime: Node.js 22 or newer

The Indexer Dockerfile runs the repository's `start:indexer` script. The Keeper Dockerfile installs the pinned `near-api-js@7.4.0` dependency and runs `start:keeper`. Railway injects `PORT` into the web service; do not hard-code a second port. Set secrets in Railway's Variables panel, never in Git.

## 1. Service 1 — Web Service: persistent Perps Indexer

Create a Railway service from the repository and set:

**Dockerfile selection**

Set this Railway service variable:

```env
RAILWAY_DOCKERFILE_PATH=Dockerfile.indexer
```

The Dockerfile runs `npm run start:indexer` from the repository root.

**Start command**

```bash
npm run start:indexer
```

**Networking**

- Generate a public HTTPS domain.
- Expose the service on Railway's injected `PORT`.
- Health-check path: `/health`.
- Keep exactly one replica per data volume.

**Persistent volume**

- Add a Railway Volume mounted at `/data`.
- Set `INDEXER_DATA_DIR=/data`.
- This directory contains `events.jsonl`, `cursor.json`, `health.json` and the writer lock. Do not run two writers against the same volume.

**Required variables**

| Variable | Value |
| --- | --- |
| `HOST` | `0.0.0.0` |
| `INDEXER_DATA_DIR` | `/data` |
| `CORS_ORIGIN` | Exact frontend origins, comma-separated; example below |
| `PORT` | Leave unset; Railway injects it |

Example `CORS_ORIGIN` for the current Preview alias and reserved Production origin:

```env
CORS_ORIGIN=https://roastake-git-feat-degen-mode-easter-egg-astrym.vercel.app,https://roastake.vercel.app
```

**Optional variables**

| Variable | Purpose |
| --- | --- |
| `FASTNEAR_API_KEY` | Optional server-only NEAR Data API key |
| `PERPS_INDEXER_NEARDATA_URL` | Defaults to `https://testnet.neardata.xyz` |
| `PERPS_INDEXER_RPC_URLS` | Comma-separated approved Testnet RPC URLs; defaults are FastNear and NEAR |
| `PERPS_INDEXER_ARCHIVAL_RPC_URLS` | Optional approved archival Testnet RPC URLs |
| `PERPS_INDEXER_CONTRACT_ID` | Defaults to `perps-v1.roastake-dev-mta.testnet` |
| `PERPS_INDEXER_POLL_MS` | Defaults to `2500` |
| `PERPS_INDEXER_MAX_RPM` | Optional request-rate override |

Leave `PERPS_INDEXER_MARKET_ID` unset to index Arabica, Robusta and Gesha together. The service is read-only and does not need a wallet key.

After the first deploy, verify:

```text
https://<your-indexer-domain>/health
https://<your-indexer-domain>/v1/perps/markets
https://<your-indexer-domain>/v1/perps/arabica/history
https://<your-indexer-domain>/v1/perps/robusta/history
https://<your-indexer-domain>/v1/perps/gesha/history
```

## 2. Service 2 — Background Worker: Perps Price Keeper

Create a second Railway service from the same repository/branch. Do not expose a public HTTP port and do not attach the indexer's volume.

**Dockerfile selection**

Set this Railway service variable:

```env
RAILWAY_DOCKERFILE_PATH=Dockerfile.keeper
```

The Dockerfile installs the pinned root dependency and runs `npm run start:keeper`.

**Start command**

```bash
npm run start:keeper
```

The process performs one immediate sandbox-price refresh, then updates Arabica ($10), Robusta ($5) and Gesha ($25) every 120 seconds. Run one replica only: multiple keepers using the same signer can contend for NEAR transaction nonces.

**Required variables**

| Variable | Value |
| --- | --- |
| `NEAR_OPERATOR_ACCOUNT_ID` | Authorized Perps Testnet operator, normally `roastake-dev-mta.testnet` |
| `NEAR_OPERATOR_PRIVATE_KEY` | Private key for that Testnet account; enter as a Railway secret |
| `NEAR_TESTNET_RPC_URL` | `https://rpc.testnet.fastnear.com` (or approved `https://rpc.testnet.near.org`) |
| `PERPS_CONTRACT_ID` | `perps-v1.roastake-dev-mta.testnet` |

The helper validates that the operator is a `.testnet` account and checks each market's sandbox classification, operator, mode, bounds and next sequence before signing. It does not unpause markets or change risk configuration.

## 3. Optional Oracle Adapter / OutLayer request

The Oracle Adapter is a separate pipeline from Perps' sandbox feed. Its contract method is `request_price_update(market)`; this is a payable OutLayer execution request, not a zero-cost health ping. The keeper leaves this feature **disabled by default**.

To opt in, create and fund a second NEAR Testnet account for Oracle requests. Authorize it on the adapter first (it must be the adapter admin or an authorized requester). The separate signer avoids transaction-nonce contention with the Perps keeper.

Set these variables on the **Background Worker** service:

| Variable | Value |
| --- | --- |
| `ORACLE_ADAPTER_ENABLED` | `true` |
| `ORACLE_ADAPTER_CONTRACT_ID` | `roastake-oracle-v1.testnet` (confirm this account currently exposes the expected adapter methods) |
| `ORACLE_ADAPTER_MARKET` | `GESHA-DEMO/USD` (must exist and be active on the deployed adapter) |
| `ORACLE_OPERATOR_ACCOUNT_ID` | Dedicated authorized `.testnet` account |
| `ORACLE_OPERATOR_PRIVATE_KEY` | Private key for the dedicated Oracle signer; Railway secret |
| `ORACLE_ADAPTER_DEPOSIT_YOCTO` | Explicit positive yoctoNEAR amount; suggested initial budget: `10000000000000000000000` (0.01 NEAR) |

Before each request, the helper checks the on-chain admin/requester status, confirms the market is active and compares the configured deposit against the contract's `estimate_execution_cost` view (using a conservative 1 KiB input bound). It then sends `request_price_update` with a 200 TGas limit and waits for finality. The adapter forwards execution funds to OutLayer and specifies the requester as the settlement-refund recipient; spent gas/execution fees are not guaranteed to be refunded.

When enabled, one request is attempted at worker startup and then every 900 seconds. Failures and 120-second local timeouts are caught independently and logged; they do not throw into or pause the 120-second Perps price loop. An ambiguous timeout keeps the Oracle in-flight guard active until its original RPC promise settles, preventing duplicate paid requests.

Do not enable this until the adapter account, current deployed ABI, active market, requester permission and Testnet deposit have been checked. Historical transaction links are not proof that the deployed account currently runs the latest source in this repository.

## 4. Connect the frontend through Vercel

After the Indexer Web Service has a working public HTTPS domain, open the ROASTAKE frontend project in Vercel and add:

```env
VITE_PERPS_HISTORY_API_URL=https://<your-indexer-domain>
```

Set the variable for the intended Vercel environments (Preview and/or Production), then redeploy the frontend so the Vite build includes it. Use the base HTTPS URL only; do not append `/health` or an API route. The backend CORS allowlist must include the exact frontend origin(s), with no trailing path and no wildcard.

## 5. Operational checks

- Indexer: `/health` reports service status, lag and coverage; check `/v1/perps/markets` and the history endpoints.
- Keeper: logs should show a finalized transaction for each Perps market approximately every 120 seconds.
- Oracle (only if enabled): logs show the market request transaction at startup and every 15 minutes.
- Restart policy: use automatic restart on failure for both services; retain exactly one active worker/indexer replica.
- Secrets: never place either private key, `.env`, or `FASTNEAR_API_KEY` in source control or Vercel frontend variables.

These services keep Testnet reference prices and event history available while their hosts are running. They do not turn the Perps feed into a live market oracle, and all Testnet assets/prices have no monetary value.
