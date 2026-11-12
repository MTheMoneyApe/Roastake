# ROASTAKE — Coffee Commodity Finance on NEAR

**ROASTAKE** is a commodity-finance platform built on **NEAR Protocol**, focused initially on coffee markets. Its hackathon MVP delivers **NEAR Testnet Coffee Perpetuals**: an isolated-margin, peer-to-pool perpetuals exchange settled in **Circle Native Testnet USDC**, paired with deterministic `10^8` / `10^12` Rust accounting and a verifiable on-chain solvency invariant.

> [!IMPORTANT]
> **Hackathon MVP Scope & Honesty Notice (`Testnet Prototype`)**
> - **Live Testnet Markets (`3` Independent Registered Markets):** `COFFEE-ARABICA-TESTNET/USD` (`$10.00/kg`, `M04.6-G`), `COFFEE-ROBUSTA-TESTNET/USD` (`COFFEE-ROBUSTA`, `$5.00/kg`, `M04.14`), and `COFFEE-GESHA-TESTNET/USD` (`COFFEE-GESHA`, `$25.00/kg`, `M04.15`) on [`perps-v1.roastake-dev-mta.testnet`](https://explorer.testnet.near.org/accounts/perps-v1.roastake-dev-mta.testnet) settled in non-monetary Circle Testnet USDC.
> - **Oracle Source:** Current trading uses independent per-market `TESTNET_SANDBOX_FEED` channels (`$10.00/kg` Arabica, `$5.00/kg` Robusta, `$25.00/kg` Gesha)—explicitly synthetic Testnet fixtures, **not** live exchange quotes, real commercial coffee prices, or a decentralized spot oracle.
> - **OutLayer Workstream Separation:** A separate OutLayer Oracle workstream (`contracts/oracle-adapter` at `roastake-oracle-v1.testnet` + WASI worker) exists in this repository and is verified on historical IMF monthly benchmarks, **not** wired into live Perps trading today.
> - **Security & Network:** Testnet-only prototype; **not externally audited** by a third party and **no Mainnet version exists**.
> - **Hackathon Track Alignment:** Primary track `defi` (*Unstoppable DeFi*, per [NEAR HACKATHON](https://nearhackathon.com/HACKATHON.md) `Q-010` Testnet eligibility and `Q-011` no mandatory integrations). ROASTAKE does **not** claim NEAR Intents, Chain Signatures, or cross-chain execution as implemented features.

---

## 1. Problem & Solution

### Problem
- **Hedging Barriers for Coffee Participants:** Traditional benchmark futures (e.g., ICE Coffee "C" at 37,500 lbs per contract) exclude smallholder producers, cooperatives, and specialty roasters behind oversized contracts and legacy brokerage rails.
- **Fragmented Coffee Infrastructure:** Price indexing, risk hedging, spot purchasing, and B2B commercial settlement operate in disconnected systems.
- **Asynchronous Collateral Accounting Hazards:** Sharded chains break synchronous ERC-20 margin assumptions if deposit rejections (`ft_resolve_transfer`) or outbound token transfers (`ft_transfer`) fail mid-flight without deterministic escrow accounting.

### Solution
ROASTAKE modularizes coffee commodity-finance starting with fractionalized **Peer-to-Pool Perpetuals on NEAR Testnet** (`0.10 kg` position verified on `$1.00` notional):
- Isolated per-position margin (`<= 5x` leverage) backed by Circle USDC liquidity with utilization hard caps (`75%`), Insurance Fund waterfall, and lossless `< 100 e8` sub-micro-USDC residue retention.
- Complete multi-step NEAR callback reconciliation ensuring deposits, closes, trader withdrawals, and LP redemptions always balance `tracked_collateral_e8` to exact `10^-8` USD precision.

---

## 2. What the MVP Does Today

- **Live Testnet Contract (`perps-v1.roastake-dev-mta.testnet`):** Deployed on NEAR Testnet with `COFFEE-ARABICA-TESTNET/USD` (`M04.6-G`), `COFFEE-ROBUSTA-TESTNET/USD` (`M04.14`), and `COFFEE-GESHA-TESTNET/USD` (`M04.15`) active on independent `TESTNET_SANDBOX_FEED` oracle sequences.
- **Collateral Settlement (`Circle USDC Testnet`):** Official 6-decimal NEP-141/145/148 USDC scaled deterministically (`×100`) to internal `USD_SCALE = 10^8`.
- **Separated Roles:** Operator/LP account (`roastake-dev-mta.testnet`) and independent Trader account (`majorblog5570.testnet` via HOT Connect / Meteor Wallet frontend).
- **Full Trading Lifecycle:** Account storage registration (`0.05 NEAR`), Circle NEP-145 storage checks (`0.00125 NEAR`), margin deposits (`ft_transfer_call`), leverage/margin/PnL/funding/10 bps fee math, `open_position`, `close_position`, async `withdraw_collateral` + `on_withdraw_transfer` callbacks, LP deposits & `withdraw_lp` (retaining `10,000` locked seed shares), and public `verify_solvency_invariant()`.
- **Non-Goals Today:** No live commercial coffee liquidity, no real-money prices, and no Mainnet trading.

---

## 3. Why NEAR Provides Concrete Value

In ROASTAKE, NEAR Protocol specifically provides:
1. **Deterministic Smart Contract Execution:** Zero-floating-point 256-bit `mul_div` fixed-point Rust execution handling isolated margin, funding accrual, utilization caps, and liquidations on-chain.
2. **Low-Cost Storage & State Settlement:** Position ledgers, locked LP seed accounting, and Circle USDC vault buckets settle on-chain with ~1–2s finality (`~0.00024–0.00086 NEAR` observed per operation).
3. **Public Transaction & Async Receipt Verification:** Every `ft_on_transfer`, `ft_resolve_transfer`, position settlement, and `collateral_withdrawn(settled)` callback emits auditable NEP-297 events verifiable via JSON-RPC `EXPERIMENTAL_tx_status` and NEAR Explorer.
4. **Separated Account & Permission Boundaries:** Explicit caller/operator checks (`admin`, `sandbox_operator`, `owner`, `predecessor_account_id`, private `#[private]` callbacks, two-step `propose_admin` / `accept_admin`) isolate admin/LP operations from trader wallets.

*(Note: NEAR Intents, Chain Signatures, cross-chain bridging, and autonomous AI trading agents are **not** implemented or claimed here.)*

---

## 4. Implemented Perpetuals Capabilities

- **Zero Floating-Point Rust Math (`packages/shared/src/perps.rs`):** Deterministic integer `u128`/`i128` 256-bit arithmetic (`PRICE_SCALE = 10^8`, `SIZE_SCALE = 10^8`, `USD_SCALE = 10^8`, `FUNDING_SCALE = 10^12`), rounding charges/margin up (`ceil`) and user payouts/shares down (`floor`).
- **Two-Phase Async Withdrawal Escrow:** Eagerly moves user credit into `total_in_flight_withdrawals_e8`, dispatches outbound Circle `ft_transfer`, and reconciles cleanly in `on_withdraw_transfer` (restoring user balance if an outbound transfer fails).
- **Sub-Micro-USDC Residue Retention:** Transfers whole `10^-6` micro-USDC units while retaining fractional `10^-8` residues (`< 100 e8`) in `free_collateral_e8` rather than dropping dust.
- **Anti-Inflation LP Seed Lock:** First LP deposit permanently locks `10,000` seed shares (`MINIMUM_LP_SEED_SHARES`).
- **Circuit Breakers & Outage Protection:** Monotonic oracle sequences, staleness guards (`300s`), future clock-drift caps (`60s`), single-tick jump circuit breakers (`500 bps` -> `ReduceOnly`), and uniform emergency freeze/settlement (`3600s` outage fallback).

---

## 5. Architecture & Components

| Component | Path | Deployment / Test Status | Role |
| --- | --- | --- | --- |
| **Perpetuals Smart Contract** | `contracts/perpetuals/` | **Live on NEAR Testnet** (`perps-v1.roastake-dev-mta.testnet`) + `33` unit tests + multi-contract Sandbox tests | Core isolated-margin Perps engine & Circle USDC vault *(Evaluation-Only)* |
| **Perpetuals Fixed-Point Core** | `packages/perps-core/` | **`15` Rust unit tests PASS** (`roastake-perps-core`) | Isolated deterministic margin, PnL, funding, LP NAV & solvency math *(Evaluation-Only)* |
| **Shared OutLayer Schemas & Fixed-Point** | `packages/shared/` | **`22` Rust unit tests PASS** (`roastake-shared`, Apache-2.0) | Shared OutLayer schemas, decimal parser, CEPEA/ICE unit math & Testnet account guards |
| **Persistent Market History Indexer** | `services/perps-indexer/` | **Verified (`node --test`: `21` tests across `7` suites PASS + persisted Arabica/Robusta/Gesha stores)** | Standalone 3-market Node.js service polling FastNear finalized Testnet blocks (`testnet.neardata.xyz`), isolating per-market `HistoryStore`s (`data/perps-indexer/` for Arabica, `markets/robusta/`, and `markets/gesha/`), separating `seedBackfill` from contiguous `forwardScan`/`range_backfill`, surfacing explicit `coverageGaps` & `lagBlocks`, bounded catch-up + HTTP `429`/`Retry-After`/jitter backoff, non-interpolated `ORACLE SAMPLE OHLC`, honest volume checks, and read-only HTTP JSON at `127.0.0.1:8787` (`/v1/perps/markets`, `/v1/perps/arabica/*`, `/v1/perps/robusta/*`, `/v1/perps/gesha/*`) *(Evaluation-Only)* |
| **Companion Trading UI** | `MTheMoneyApe/pixel-perfect-replica` (`hackathon-submission`) | Verified locally (`114/114` Vitest across `15` files, `tsc`, ESLint on Perps/Oracle/Wallet files `0` errors, Vite/Nitro build PASS) | React 19 / TanStack Start 3-market terminal (`/app/perps`, `/app/perps/COFFEE-ARABICA-TESTNET-USD`, `/app/perps/COFFEE-ROBUSTA`, `/app/perps/COFFEE-GESHA`) + HOT Connect / Meteor Wallet + Lightweight Charts `5.3.0` (`127.0.0.1:8787` indexer client showing `PARTIAL COVERAGE` vs `CONTIGUOUS COVERAGE`, `FINALIZED BLOCK`, `INDEXER LAG`, and verified RPC fallback) |
| **Oracle Adapter *(Independent Workstream)*** | `contracts/oracle-adapter/` | **Deployed separately** (`roastake-oracle-v1.testnet`) + `96` unit tests + Sandbox tests | Separate `outlayer.testnet` requester storing historical `ARABICA-MONTHLY-IMF/USD` |
| **WASI Worker *(Independent Workstream)*** | `workers/coffee-price-index/` | Compiled `wasm32-wasip2` + `16` unit tests | Off-chain OutLayer TEE worker fetching monthly IMF PCPS data (`api.imf.org`) |
| **Sandbox Harness** | `integration-tests/near-sandbox-js/` | Local `near-workspaces 5.0.0` suites (`sandbox-perpetuals.test.mjs`, `mock-ft`, `mock-outlayer`) | Automated multi-contract adversarial & integration tests |

> **OutLayer Workstream Separation:** `contracts/oracle-adapter` (`roastake-oracle-v1.testnet`) and `workers/coffee-price-index` are implemented and verified independently on historical monthly IMF benchmarks (`PUBLIC_BENCHMARK_MONTHLY`). They do **not** feed the active `COFFEE-ARABICA-TESTNET/USD` perpetual market today; Perpetuals intentionally rejects 90-day monthly IMF data and executes against `TESTNET_SANDBOX_FEED` until Gate 4 multi-source commercial spot feeds exist.

---

## 6. Deployed Testnet Contract & Circle USDC

- **Network:** NEAR Testnet Only (`testnet` — Mainnet `.near` accounts and Mainnet USDC are rejected fail-closed)
- **RPC Endpoint:** `https://rpc.testnet.fastnear.com`
- **Explorer:** `https://explorer.testnet.near.org`
- **Perpetuals Contract:** `perps-v1.roastake-dev-mta.testnet` ([Explorer](https://explorer.testnet.near.org/accounts/perps-v1.roastake-dev-mta.testnet))
- **On-Chain Code Hash:** `GFVwdDtzy4BfxB4zXBhoX3ANfAaFLAyo7EuHAjq8mUsG`
- **Contract WASM SHA-256 (`roastake_perpetuals.wasm`, `453,898` B):** `e29628ea1dba7410d29c84c49baf28cd5683f29c1d08abd1837e6ed2f9b4377f`
- **Circle Testnet USDC (`NEP-141/145/148`, 6 dec):** `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`
- **Separate OutLayer Oracle Adapter:** `roastake-oracle-v1.testnet` ([Explorer](https://explorer.testnet.near.org/accounts/roastake-oracle-v1.testnet))

---

## 7. Reproducible Verification Guide (`Do X -> See Y`)

Judges can verify tests, UI, and live Testnet solvency **with zero Testnet tokens or private keys**.

### Prerequisites
- **Backend tests:** Rust toolchain (`rustc` / `cargo`, plus `NASM` & `cargo-near` `0.22.0` if rebuilding contract WASM; `scripts/verify-env.ps1` validates setup on Windows).
- **Frontend & RPC CLI checks:** Node.js `>= 20.15.0` and `npm` (or `bun`).

### A. Verify Live Deployed Contract & Solvency (`0` Funds Required)
1. **Do:** Query `view_account`, `verify_solvency_invariant`, `get_accounting_snapshot`, `list_markets`, and `get_market` on `perps-v1.roastake-dev-mta.testnet`:
   ```bash
   node -e "
   const RPC='https://rpc.testnet.fastnear.com', PERPS='perps-v1.roastake-dev-mta.testnet', USDC='3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af';
   const q = (id, m, a={}) => fetch(RPC,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:m,method:'query',params:{request_type:'call_function',finality:'final',account_id:id,method_name:m,args_base64:Buffer.from(JSON.stringify(a)).toString('base64')}})}).then(r=>r.json()).then(d=>JSON.parse(Buffer.from(d.result.result).toString()));
   const va = (id) => fetch(RPC,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:'va',method:'query',params:{request_type:'view_account',finality:'final',account_id:id}})}).then(r=>r.json()).then(d=>d.result);
   Promise.all([va(PERPS), q(PERPS,'verify_solvency_invariant'), q(PERPS,'get_accounting_snapshot'), q(PERPS,'list_markets'), q(PERPS,'get_market',{market_id:'COFFEE-ARABICA-TESTNET/USD'}), q(PERPS,'get_market',{market_id:'COFFEE-ROBUSTA-TESTNET/USD'}), q(PERPS,'get_market',{market_id:'COFFEE-GESHA-TESTNET/USD'}), q(USDC,'ft_balance_of',{account_id:PERPS})]).then(x=>console.log(JSON.stringify(x,null,2)));
   "
   ```
   **See:**
   - `code_hash`: `"GFVwdDtzy4BfxB4zXBhoX3ANfAaFLAyo7EuHAjq8mUsG"`
   - `verify_solvency_invariant`: `true`
   - `list_markets`: `["COFFEE-ARABICA-TESTNET/USD", "COFFEE-ROBUSTA-TESTNET/USD", "COFFEE-GESHA-TESTNET/USD"]`
   - `get_accounting_snapshot`: `solvency_ok: true`, `total_open_positions: 0`, `total_in_flight_withdrawals_e8: "0"`, `tracked_collateral_e8: "250400"` matching bucket sum `368 + 0 + 0 + 10032 + 80000 + 160000 = 250400`
   - `ft_balance_of`: `"2504"` (`2504 × 100 = 250400`)
   - `get_market` (Arabica `seq=5` @ `$10.00/kg`, Robusta `seq=3` @ `$5.00/kg`, Gesha `seq=4` @ `$25.00/kg`): `feed_classification: "TESTNET_SANDBOX_FEED"`, `max_leverage: 5`, `initial_margin_bps: 2000`, `maintenance_margin_bps: 800`, `trade_fee_bps: 10`, `max_utilization_bps: 7500`.

### B. Clone & Run Backend Unit Tests (`0` Funds Required)
1. **Do:** Clone backend & run full Rust workspace tests:
   ```bash
   git clone https://github.com/MTheMoneyApe/Roastake.git
   cd Roastake
   git checkout hackathon-submission
   cargo test --workspace
   ```
   *(On Windows, `powershell -ExecutionPolicy Bypass -File scripts/run-tests.ps1` configures Cargo/NASM PATH automatically and executes `cargo test --workspace`.)*
   **See:** The workspace test command must complete with all tests passing (`0` failed). Cargo prints the authoritative test totals for the exact checkout; this README intentionally avoids freezing a historical workspace count as crates and fixtures evolve.

2. **Do *(Optional NEAR Sandbox Multi-Contract Suite on Linux/WSL2)*:**
   ```bash
   powershell -ExecutionPolicy Bypass -File scripts/build-perpetuals.ps1
   cd integration-tests/near-sandbox-js && pnpm install && NEAR_WORKSPACES_NETWORK=sandbox node --test test/sandbox-perpetuals.test.mjs
   ```
   **See:** Builds `roastake_perpetuals.wasm` (`e29628ea...4377f`) and runs local multi-contract collateral, XCC oracle, liquidation, and emergency settlement tests.

### C. Run Persistent NEAR Testnet Market History Indexer (`0` Funds Required)
1. **Do:** Test and run the standalone backend indexer service (pure Node.js standard library, no extra dependencies or API key required for public Testnet rate limits):
   ```bash
   cd services/perps-indexer
   node --test test/*.test.mjs
   node src/indexer.mjs --once   # one-shot Arabica M04.6-G seed + finalized anchor sync
   node src/indexer.mjs --once --catch-up --max-blocks=100  # bounded catch-up
   node src/indexer.mjs --market=robusta --backfill-range=277194418..277194421  # isolated market range backfill
   node src/indexer.mjs          # continuous multi-market polling + API on http://127.0.0.1:8787
   ```
   *(Optional server-only `FASTNEAR_API_KEY` env var adds `Authorization: Bearer`; never pass keys in URLs or frontend.)*
   **See:**
   - All `21` indexer tests across `7` suites pass (`0` failed): finalized block scanning (chunk transactions + receipts), strict 3-market store isolation (`arabica` vs `robusta` vs `gesha`, unscoped LP/collateral events skipped for `robusta` & `gesha`), separate `seedBackfill` vs `forwardScan` / `range_backfill` coverage segments, explicit `coverageGaps` (`coverageComplete: false` until gap-free), bounded catch-up (`> 4` blocks), HTTP `429` + `Retry-After` + jitter backoff, temporary vs irreversible `404` differentiation, exclusive `writer.lock`, non-interpolated `ORACLE SAMPLE OHLC`, schema-honest `Volume unavailable`, and strict CORS (`http://127.0.0.1:3000`).
   - Live `http://127.0.0.1:8787/health` + `/v1/perps/markets` + `/v1/perps/{arabica,robusta,gesha}/history` report per-market `status`, `providerStatus`, `indexedThroughHeight`, `lastFinalizedHeight`, `lagBlocks`, `coverageSegments`, `coverageGaps`, `coverageComplete`, and `rejectedCount = 0`. `data/perps-indexer/` persists Arabica (`events.jsonl` + `cursor.json`), Robusta (`markets/robusta/`), and Gesha (`markets/gesha/`) cleanly outside git (`.gitignore`).

### D. Clone, Test & Launch Companion Frontend (`0` Funds Required)
1. **Do:** Clone frontend, verify tests/build, and run dev server:
   ```bash
   git clone https://github.com/MTheMoneyApe/pixel-perfect-replica.git Roastake-Frontend
   cd Roastake-Frontend
   git checkout hackathon-submission
   npm install
   npm run test
   npm run build
   npm run dev
   ```
   Open `http://127.0.0.1:3000/app/perps`, `http://127.0.0.1:3000/app/perps/COFFEE-ARABICA-TESTNET-USD`, `http://127.0.0.1:3000/app/perps/COFFEE-ROBUSTA`, `http://127.0.0.1:3000/app/perps/COFFEE-GESHA`, and `http://127.0.0.1:3000/app/activity`.
   **See:**
   - `npm run test` reports **`15 passed (15)`** files / **`114 passed (114)`** tests in the latest recorded release verification.
   - All three live markets — Arabica (`$10.00/kg`, seq `#5`), Robusta (`$5.00/kg`, seq `#3`), and Gesha (`$25.00/kg`, seq `#4`) — display live on-chain `5x · 20.00% IM · 8.00% MM`, `Synthetic Testnet Oracle Notice`, `LP Vault`, isolated TradingView Lightweight Charts (`INDEXER CONNECTED`, `PARTIAL COVERAGE` vs `CONTIGUOUS COVERAGE`, `FINALIZED BLOCK`, `INDEXER LAG`, or verified RPC fallback when offline, `ORACLE SAMPLE OHLC` disabled on insufficient buckets, honest `Volume unavailable`), and verified NEAR Explorer links (`M04.6-G`, `M04.14`, `M04.15`).
   - Admin operator panel remains hidden unless connected as `roastake-dev-mta.testnet`.

### E. Actions Requiring a Testnet Wallet & Tokens
- Connecting a personal `.testnet` account (e.g., Meteor Wallet), depositing Circle Testnet USDC (`https://faucet.circle.com/`, `NEAR Testnet`), registering storage (`0.00125 NEAR` Circle + `0.05 NEAR` Perps), and signing new opens/closes/withdrawals require Testnet NEAR + USDC.
- Opening new positions on any of the three markets (`COFFEE-ARABICA-TESTNET/USD`, `COFFEE-ROBUSTA-TESTNET/USD`, `COFFEE-GESHA-TESTNET/USD`) fails closed (`ERR_ORACLE_STALE`) if that market's latest `TESTNET_SANDBOX_FEED` tick is `> 300s` old (refreshable only by `roastake-dev-mta.testnet`); closing open positions and withdrawing free collateral work directly for position/collateral owners.

---

## 8. Live Hybrid Cycle Evidence (`M04.6-G`, NEAR Testnet)

Executed end-to-end across **two independent accounts**: Admin/LP `roastake-dev-mta.testnet` (NEAR CLI `0.31.1`) and Trader `majorblog5570.testnet` (**Chrome + Meteor Wallet**). *(Preceded by trader's `1.000000 USDC` Meteor deposit verified via free collateral balance transition `0 -> 100000000 e8` at block `#277173730`.)*

| Step | Action | Signer | Verified Transaction Hash & Explorer Link |
| --- | --- | --- | --- |
| 1 | Oracle Tick #3 (`$10.00/kg`, seq `3`) | `roastake-dev-mta.testnet` | [`8KGqwWiHmFs2TQpDWWPEe8jRHWuBrfpjbe2EDBt47men`](https://explorer.testnet.near.org/transactions/8KGqwWiHmFs2TQpDWWPEe8jRHWuBrfpjbe2EDBt47men) |
| 2 | LP Deposit (`2.000000 USDC` -> `199,840,127` shares) | `roastake-dev-mta.testnet` | [`4b5cqULXF984ouYTUc5REX2vJbVKE3iYuTxAU1eZASd2`](https://explorer.testnet.near.org/transactions/4b5cqULXF984ouYTUc5REX2vJbVKE3iYuTxAU1eZASd2) |
| 3 | Oracle Tick #4 (`$10.00/kg`, seq `4`) | `roastake-dev-mta.testnet` | [`BNzMzWmVUBpz8nvMNnHvJjajYsnMuDqbVTr7iyQqVqUZ`](https://explorer.testnet.near.org/transactions/BNzMzWmVUBpz8nvMNnHvJjajYsnMuDqbVTr7iyQqVqUZ) |
| 4 | Open LONG Position #2 (`0.10 kg` @ `$10/kg`, `$1.00` notional, `$0.25` margin, `4x`) | `majorblog5570.testnet` *(Meteor)* | [`CNgQkfrRpMrU8ywgzFaBdniPfivRDsgEnXcfoFX6gWAp`](https://explorer.testnet.near.org/transactions/CNgQkfrRpMrU8ywgzFaBdniPfivRDsgEnXcfoFX6gWAp) |
| 5 | Oracle Tick #5 (`$10.00/kg`, seq `5`) | `roastake-dev-mta.testnet` | [`Jz7uJnm213s2qLNyBsnkwDPpkNYcuH255wFxPrFeemR`](https://explorer.testnet.near.org/transactions/Jz7uJnm213s2qLNyBsnkwDPpkNYcuH255wFxPrFeemR) |
| 6 | Close Position #2 (PnL `$0`, fee `$0.001`, funding `-1805 e8`) | `majorblog5570.testnet` *(Meteor)* | [`2QWybbu1rwxC9in5y8cxAzXzH7PR1bbH6x5WKDJHREnC`](https://explorer.testnet.near.org/transactions/2QWybbu1rwxC9in5y8cxAzXzH7PR1bbH6x5WKDJHREnC) |
| 7 | Trader Withdrawal (`0.997981 USDC` + `collateral_withdrawn(settled)`) | `majorblog5570.testnet` *(Meteor)* | [`DCAfqd5CSi2fk7ezsvo9J657yvX8M7NGEAqS6qvhZmFc`](https://explorer.testnet.near.org/transactions/DCAfqd5CSi2fk7ezsvo9J657yvX8M7NGEAqS6qvhZmFc) |
| 8 | Admin LP Withdrawal (`199,840,127` shares burned -> `2.001417 USDC`) | `roastake-dev-mta.testnet` | [`2PJWPxDTP4zt8H5EfiaMd3Tu34aM21k4x7FjUpcR6nVP`](https://explorer.testnet.near.org/transactions/2PJWPxDTP4zt8H5EfiaMd3Tu34aM21k4x7FjUpcR6nVP) |

### Final Verified Cycle Totals & Exact Solvency (`Block #277175208`)
- **Cash Flows:** LP deposited `2.000000 USDC` / received `2.001417 USDC` (reflecting observed fees & funding from this specific test trade; **not** a guaranteed or projected yield). Trader deposited `1.000000 USDC` / received `0.997981 USDC`.
- **Final Circle USDC Balances:** Admin `19.000716 USDC` · Trader `0.997981 USDC` · Contract `0.001303 USDC`.
- **Open Positions & In-Flight Withdrawals:** `0` open positions, `0` in-flight withdrawals, `10,000` locked LP seed shares preserved.
- **Exact Ledger Reconciliation (Post-M04.6-G Historical Snapshot):** `tracked_collateral_e8 = 130300` (`284 + 10016 + 40000 + 80000 = 130300 e8`), **`verify_solvency_invariant() = true`**.

---

## 8B. Live Multi-Market Robusta Cycle Evidence (`M04.14`, NEAR Testnet)

Executed independently on **`COFFEE-ROBUSTA-TESTNET/USD`** (frontend/indexer slug `COFFEE-ROBUSTA`) without redeploying `perps-v1.roastake-dev-mta.testnet` and leaving `COFFEE-ARABICA-TESTNET/USD` (`sandbox_sequence = 5`, `$10.00/kg`) 100% unchanged:

| Step | Action | Signer | Verified Transaction Hash & Explorer Link |
| --- | --- | --- | --- |
| 1 | `register_market` (`COFFEE-ROBUSTA-TESTNET/USD`) | `roastake-dev-mta.testnet` | [`6yY3GdDY1fkTZcVXpizTv2r1eDsTLs8sHzsMV6rALaf6`](https://explorer.testnet.near.org/transactions/6yY3GdDY1fkTZcVXpizTv2r1eDsTLs8sHzsMV6rALaf6) |
| 2 | Robusta Oracle Tick #1 (`$5.00/kg`, seq `1`) | `roastake-dev-mta.testnet` | [`99425WCWbxpGdbhZCUUfnCvPeyJurrmaYJacCneYPVj4`](https://explorer.testnet.near.org/transactions/99425WCWbxpGdbhZCUUfnCvPeyJurrmaYJacCneYPVj4) |
| 3 | LP Deposit (`2.000000 USDC` -> `199,680,511` shares) | `roastake-dev-mta.testnet` | [`Bjztjs2Knj1PTCZnq62aUK148iM5muusjK7bq5M1LZ1`](https://explorer.testnet.near.org/transactions/Bjztjs2Knj1PTCZnq62aUK148iM5muusjK7bq5M1LZ1) |
| 4 | Robusta Oracle Tick #2 (`$5.00/kg`, seq `2`) | `roastake-dev-mta.testnet` | [`3VMnYs7AzyzUbVcdy4VH6QJAhWNyyqPf9rNUxF5xhvcH`](https://explorer.testnet.near.org/transactions/3VMnYs7AzyzUbVcdy4VH6QJAhWNyyqPf9rNUxF5xhvcH) |
| 5 | Trader Deposit (`0.500000 USDC`, `free_collateral_e8 -> 50000095`) | `majorblog5570.testnet` *(Meteor)* | [`9MuxVRqtrhPsTXgNmiFdxeEdtQkK1KBWGvkvsGAUWfxm`](https://explorer.testnet.near.org/transactions/9MuxVRqtrhPsTXgNmiFdxeEdtQkK1KBWGvkvsGAUWfxm) |
| 6 | Open LONG Position #3 (`0.20 kg` @ `$5.00/kg`, `$1.00` notional, `$0.25` margin, `4x`) | `majorblog5570.testnet` *(Meteor)* | [`Dmd9uY1YmjVwR1mdBnzVQysjUjh8K6xNJi8mrHv9VJXw`](https://explorer.testnet.near.org/transactions/Dmd9uY1YmjVwR1mdBnzVQysjUjh8K6xNJi8mrHv9VJXw) |
| 7 | Robusta Oracle Tick #3 (`$5.00/kg`, seq `3`) | `roastake-dev-mta.testnet` | [`2W1ZYFXrhHxH2DJgiS16cM2KMa3uWPysiKh9thQz2WNR`](https://explorer.testnet.near.org/transactions/2W1ZYFXrhHxH2DJgiS16cM2KMa3uWPysiKh9thQz2WNR) |
| 8 | Close Position #3 (PnL `$0`, fee `$0.001`, funding `-2666 e8`, payout `24897334 e8`) | `majorblog5570.testnet` *(Meteor)* | [`ACKxYjULWt5AunYdLpJWmi9bCKgB69SmrDoCruBiHJnP`](https://explorer.testnet.near.org/transactions/ACKxYjULWt5AunYdLpJWmi9bCKgB69SmrDoCruBiHJnP) |
| 9 | Trader Withdrawal (`0.497974 USDC`, `withdrawal_id = 5` `settled`) | `majorblog5570.testnet` *(Meteor)* | [`8S6CVXMp7TeyE9MqYSbAZxME4xcDUhWhcZcb4M8puXuz`](https://explorer.testnet.near.org/transactions/8S6CVXMp7TeyE9MqYSbAZxME4xcDUhWhcZcb4M8puXuz) |
| 10 | Admin LP Withdrawal (`199,680,511` shares burned -> `2.001426 USDC` + `58 e8` residue, `withdrawal_id = 6` `settled`) | `roastake-dev-mta.testnet` | [`2DQ39iN2zg5iPZATt2ipvCNT2H6sjWodsiSR5Z6sQd6d`](https://explorer.testnet.near.org/transactions/2DQ39iN2zg5iPZATt2ipvCNT2H6sjWodsiSR5Z6sQd6d) |

### Final Verified Post-M04.14 Contract State (`Block #277196089`)
- **Final Circle USDC Balances:** Admin `19.002142 USDC` (`19002142`) · Trader `0.995955 USDC` (`995955`) · Contract `0.001903 USDC` (`1903`).
- **Exact Ledger Reconciliation (Post-M04.14 Historical Snapshot):** `tracked_collateral_e8 = 190300` (`276 + 0 + 0 + 10024 + 60000 + 120000 = 190300 e8`), `verify_solvency_invariant() = true`.

---

## 8C. Live Multi-Market Gesha Cycle Evidence (`M04.15`, NEAR Testnet)

Executed independently on **`COFFEE-GESHA-TESTNET/USD`** (frontend/indexer slug `COFFEE-GESHA`) without redeploying `perps-v1.roastake-dev-mta.testnet` and leaving Arabica (`seq = 5`, `$10.00/kg`) and Robusta (`seq = 3`, `$5.00/kg`) 100% unchanged:

| Step | Action | Signer | Verified Transaction Hash & Explorer Link |
| --- | --- | --- | --- |
| 1 | `register_market` (`COFFEE-GESHA-TESTNET/USD`) | `roastake-dev-mta.testnet` | [`AAEy7eWTAyitTLpdg8Zew9F11a3wFd49zZDYXXFeWjjc`](https://explorer.testnet.near.org/transactions/AAEy7eWTAyitTLpdg8Zew9F11a3wFd49zZDYXXFeWjjc) |
| 2 | Gesha Oracle Tick #1 (`$25.00/kg`, seq `1`) | `roastake-dev-mta.testnet` | [`8qz9deZxMYge5tXx19zSr9kkpNq7wSMd7Gh246yVHinb`](https://explorer.testnet.near.org/transactions/8qz9deZxMYge5tXx19zSr9kkpNq7wSMd7Gh246yVHinb) |
| 3 | LP Deposit (`2.000000 USDC` -> `199,521,149` shares, `nav_e8 = 10024`) | `roastake-dev-mta.testnet` | [`FgfBypxTMvdM8k7DqEfZny5QhgkCMumwLvJtY6SC7KhU`](https://explorer.testnet.near.org/transactions/FgfBypxTMvdM8k7DqEfZny5QhgkCMumwLvJtY6SC7KhU) |
| 4 | Gesha Oracle Tick #2 (`$25.00/kg`, seq `2`) | `roastake-dev-mta.testnet` | [`8jnhk3YuR24hZYAZk9hDj5qVjo1Va943VdBoGzrzp1FD`](https://explorer.testnet.near.org/transactions/8jnhk3YuR24hZYAZk9hDj5qVjo1Va943VdBoGzrzp1FD) |
| 5 | Gesha Oracle Tick #3 (`$25.00/kg`, seq `3`, refresh after `341s` STALE block; trader `0.500000 USDC` deposit verified at `#277199273`) | `roastake-dev-mta.testnet` | [`7KizejXJqqVVPExRBgmrPMM5RkQ6Sr4NkRuA6QyGtQEW`](https://explorer.testnet.near.org/transactions/7KizejXJqqVVPExRBgmrPMM5RkQ6Sr4NkRuA6QyGtQEW) |
| 6 | Open LONG Position #4 (`0.04 kg` @ `$25.00/kg`, `$1.00` notional, `$0.25` margin, `4x`) | `majorblog5570.testnet` *(Meteor)* | [`A2cKFvi6YiL15EUjJXGejFbcuviEPmp8JRLCK5LmPwgh`](https://explorer.testnet.near.org/transactions/A2cKFvi6YiL15EUjJXGejFbcuviEPmp8JRLCK5LmPwgh) |
| 7 | Gesha Oracle Tick #4 (`$25.00/kg`, seq `4`) | `roastake-dev-mta.testnet` | [`9CFB5JAUB94i2gr6oBgQhNfeP513bLvoKuZfsyuhY6ik`](https://explorer.testnet.near.org/transactions/9CFB5JAUB94i2gr6oBgQhNfeP513bLvoKuZfsyuhY6ik) |
| 8 | Close Position #4 (PnL `$0`, fee `$0.001`, funding `-2041 e8`, payout `24897959 e8`) | `majorblog5570.testnet` *(Meteor)* | [`JxF78YRwXUYwZwR6ZFo8QXXHxCv9sLD63yBTh3qNgKv`](https://explorer.testnet.near.org/transactions/JxF78YRwXUYwZwR6ZFo8QXXHxCv9sLD63yBTh3qNgKv) |
| 9 | Trader Withdrawal (`0.497979 USDC`, `withdrawal_id = 7` `settled`, `88 e8` residue) | `majorblog5570.testnet` *(Meteor)* | [`EmG7y5zqRJMcpVUhcj1d8kGS12dNwWmEpABqa5bM6WW2`](https://explorer.testnet.near.org/transactions/EmG7y5zqRJMcpVUhcj1d8kGS12dNwWmEpABqa5bM6WW2) |
| 10 | Admin LP Withdrawal (`199,521,149` shares burned -> `2.001420 USDC` + `33 e8` residue, `withdrawal_id = 8` `settled`) | `roastake-dev-mta.testnet` | [`3ueUEtCNsLY4HG542HwYtgdvN3CHu74xsfpg6YKDRPD3`](https://explorer.testnet.near.org/transactions/3ueUEtCNsLY4HG542HwYtgdvN3CHu74xsfpg6YKDRPD3) |

### Final Verified Post-M04.15 Contract State (`Block #277200674`)
- **Final Circle USDC Balances:** Admin `19.003562 USDC` (`19003562`) · Trader `0.993934 USDC` (`993934`) · Contract `0.002504 USDC` (`2504`).
- **Exact Ledger Reconciliation:** `tracked_collateral_e8 = 250400` (`total_trader_free_collateral_e8: 368` [admin `280` + trader `88`] + `total_trader_locked_margin_e8: 0` + `total_in_flight_withdrawals_e8: 0` + `lp_vault_realized_equity_e8: 10032` [`10,000` locked seed shares] + `insurance_fund_e8: 80000` + `protocol_fees_e8: 160000` = **`250400 e8`**), **`verify_solvency_invariant() = true`**.

---

## 9. Verified Market Parameters (`COFFEE-ARABICA-TESTNET/USD`, `COFFEE-ROBUSTA-TESTNET/USD` & `COFFEE-GESHA-TESTNET/USD`)

Verified identically in `packages/perps-core/src/perps.rs` and live on-chain `get_market` across all three registered markets:

| Parameter | Verified Value |
| --- | --- |
| Maximum Leverage (`max_leverage`) | `5x` |
| Initial Margin (`initial_margin_bps`) | `20%` (`2000 bps`) |
| Maintenance Margin (`maintenance_margin_bps`) | `8%` (`800 bps`) |
| Trading Fee (`trade_fee_bps`) | `10 bps` (`0.10%`, split `70%` LP / `10%` Insurance / `20%` Protocol) |
| Maximum LP Pool Utilization (`max_utilization_bps`) | `75%` (`7500 bps`, plus `2500 bps` matched-OI gap buffer) |
| Oracle Stale Threshold (`max_execution_age_sec`) | `300 seconds` (`3600s` emergency close threshold) |
| Oracle Future Clock Drift (`max_clock_drift_sec`) | `60 seconds` |
| Single-Update Circuit Breaker (`max_jump_bps`) | `500 bps` (`5.00%`) |

---

## 10. Limitations & Roadmap

- **Synthetic Oracle:** `TESTNET_SANDBOX_FEED` is operator-controlled for Testnet validation. Multi-source commercial spot feeds via OutLayer (`ExternalXcc`) remain post-hackathon Gate 4 work.
- **Testnet Only & No External Audit:** Built strictly for NEAR Testnet (`Circle Testnet USDC`); independent third-party smart contract security audit and DAO/multisig governance transition are required before Mainnet.
- **Ecosystem Modules (Post-Perps Roadmap):** Spot Marketplace, B2B Procurement, legally structured Coffee Tokenization, and Treasury portals remain preview/roadmap work. *(All three coffee perpetual markets — Arabica, Robusta, and Gesha — are registered and verified live on `perps-v1.roastake-dev-mta.testnet`.)*

---

## 11. Companion Frontend Repository

- **Frontend Repository (`MTheMoneyApe/pixel-perfect-replica`):** https://github.com/MTheMoneyApe/pixel-perfect-replica (branch `main`)

---

## 12. Baseline, Hackathon Chronology & Boilerplate Disclosure

Per [NEAR HACKATHON rules (`HACKATHON.md`)](https://nearhackathon.com/HACKATHON.md):
- **Event Window:** Announced `2026-10-19`, official hackathon start `2026-11-01T00:00:00Z`, submission deadline `2026-11-14T23:59:00Z` (`Q-001` covers work from `2026-10-19` through `2026-11-14`).
- **Backend (`Roastake`):** Initial commit `0d130ff` (`2026-11-09 21:36:44 +0100`). All Rust contracts (`oracle-adapter`, `perpetuals`), shared fixed-point crates, WASI worker, Sandbox tests, and deploy scripts were built inside the hackathon window.
- **Companion Frontend (`pixel-perfect-replica`):** Initial commit `4c3b15c` (`2026-11-02 13:30:43 +0000`, `template: tanstack_start_ts_current`) started from a **TanStack Start / Vite / Lovable React scaffold** (`shadcn/ui` primitives, Tailwind CSS, simulated coffee marketplace/UI demo views), which count cleanly as UI boilerplate/starter components (`Q-005`). All NEAR integrations (`@hot-labs/near-connect` 0.12.0 wallet hub, `roastake-oracle-v1.testnet` dashboard, `contracts/perpetuals` three-market BigInt order terminal, `M04.6-G` / `M04.14` / `M04.15` verifiers, and `96`-test Vitest suite across `13` files) were built during the hackathon window.
- **Publication Note:** The public hackathon submission snapshot is published on branch `hackathon-submission` in both repositories. Use the branch links above when reviewing the submitted source; historical local working-tree notes are no longer applicable.

---

## 13. Licensing & Provenance

**Copyright © 2026 @MTheMoneyApe. All rights reserved.** *(solely for the original ROASTAKE materials owned by `@MTheMoneyApe` documented in `LICENSES.md`)*  
**Licensor (Evaluation-Only Perpetuals Scope):** `@MTheMoneyApe`

This repository uses a **mixed licensing model** that preserves all prior Open Source commitments:

- **Previously Distributed Workspace & OutLayer Components ([`LICENSE`](./LICENSE)):** All files previously distributed on GitHub (and modified extensions inside existing Apache-2.0 crates/contracts) remain licensed under the **[Apache License 2.0](./LICENSE)** (`SHA-256: 1EB85FC9...`), without revocation or narrowing of existing Apache-2.0 rights.
- **Eligible Original ROASTAKE Perpetuals & Indexer Modules ([`LICENSE-ROASTAKE-PERPS.md`](./LICENSE-ROASTAKE-PERPS.md)):** Newly authored, previously unreleased ROASTAKE Perpetuals core math (`packages/perps-core/**`, `roastake-perps-core`), Perpetuals contract code (`contracts/perpetuals/**`, `roastake-perpetuals`), standalone persistent NEAR Testnet history indexer (`services/perps-indexer/**`), Perpetuals design specifications (`docs/perpetuals/**`), Perps build/deploy scripts, and Perps integration test (`sandbox-perpetuals.test.mjs`) are isolated cleanly and licensed solely for non-commercial inspection, compilation, testing, and hackathon/security evaluation under **`ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0`** (`SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only`). Commercial exploitation, production deployment, or hosted use requires a separate written agreement with **@MTheMoneyApe**.
- **Shared Utilities & Deployed Contract Notice:** `roastake-shared` (`packages/shared/src/lib.rs` and `fixed_point.rs`) remains purely under **[Apache License 2.0](./LICENSE)** (`roastake-perps-core` consumes `roastake-shared` one-way without restricting Apache-2.0 code). Extracting `roastake-perps-core` was verified locally without modifying or redeploying `perps-v1.roastake-dev-mta.testnet`.
- **Exact File Scope & Third-Party Notices:** See **[`LICENSES.md`](./LICENSES.md)** for the per-directory scope table (including documented mixed-history preservation and `mock-ft` unattributed test fixture status) and **[`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md)** for Rust/JavaScript third-party notices.

---

## Hackathon disclosure (NEAR HACKATHON 2026)

ROASTAKE is a coffee-commodity finance platform on NEAR. This submission covers three NEAR Testnet coffee perpetual markets (Arabica, Robusta, Gesha), three NEP-141 coffee tokens (rARABICA, rROBUSTA, rGESHA) with three RHEA USDC pools, and a Mini Swap in the frontend.

**Honest disclosures**

- Oracle prices are synthetic reference prices from TESTNET_SANDBOX_FEED; they are NOT decentralized live coffee-market prices.
- Token backing is SIMULATED_TESTNET_INVENTORY; there is no verified physical custody, warehouse backing or physical coffee redemption.
- Perps Sandbox tests pass on Linux/WSL2. The 
ear-sandbox binary is incompatible with Windows x64; use WSL2 or CI on Windows.
- ESLint whole-repo reports pre-existing CRLF/prettier noise (9093 errors) unrelated to M04.18; focused scope on new modules is clean.

**Reproducibility**

- **Frontend:** Clone the public frontend repository, check out `hackathon-submission`, then run `npm install && npm run dev` (development server: `http://127.0.0.1:3000`). Routes: `/app/swap` (Mini Swap) and `/app/perps` (Perps).
- **Backend Coffee Token tests:** `cargo test -p roastake-coffee-token` (latest recorded focused result: 9 tests).
- **Frontend release verification:** `npm run test` (114/114 tests across 15 files), `npx tsc --noEmit`, and `npm run build`.
- **Sandbox test (Linux/WSL2 only):** `cd integration-tests/near-sandbox-js && pnpm install --frozen-lockfile && NEAR_WORKSPACES_NETWORK=sandbox node --test test/sandbox-perpetuals.test.mjs` (4/4 pass in the recorded run).

**AI, libraries and templates**

- Lovable-assisted frontend scaffolding (TanStack Start starter).
- Antigravity-assisted mock-ft/ development.
- NEAR SDK, RHEA (Ref Finance) SDK, TanStack Router, shadcn/ui.

**Pre-hackathon baseline**

Pre-hackathon baseline is `main` (commit `1d38885`). The public hackathon submission is maintained on branch `hackathon-submission`; see the branch history for the exact release commit.
