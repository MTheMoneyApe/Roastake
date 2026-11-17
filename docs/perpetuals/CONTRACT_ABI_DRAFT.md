<!-- SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only -->
<!-- Copyright © 2026 @MTheMoneyApe. All rights reserved. -->

> **License:** [ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0](../../LICENSE-ROASTAKE-PERPS.md) — Copyright © 2026 @MTheMoneyApe. All rights reserved. **Licensor:** @MTheMoneyApe

# ROASTAKE Perpetuals — NEAR Async Architecture, Circle Testnet Collateral & Contract ABI Specification (M04.5B.1)

- **Status:** Pre-Deployment Validated ABI Specification (`contracts/perpetuals/src/lib.rs`, Compiled Only)
- **Date:** 2026-11-11
- **Version:** `1.3.0-m045b1`
- **NEAR SDK Target:** `near-sdk = 5.30.1`, NEP-141 / NEP-145 / NEP-148 / NEP-297

---

## 1. NEAR Asynchronous Cross-Contract Calls & Oracle Reality

### 1.1 Why Contracts Cannot Synchronously Read `roastake-oracle-v1.testnet`
Off-chain clients and frontends query NEAR contracts synchronously via JSON-RPC `view` calls (`call_function`). Inside the NEAR WebAssembly runtime, however, **smart contracts are strictly prohibited from synchronously reading another account's trie storage (`env::storage_read` reads `self` only)**. Every cross-contract read (`roastake-perps -> roastake-oracle-v1.testnet`) MUST execute asynchronously across **two receipts** (`ext_oracle::get_price` Promise + `#[private]` callback), never confused with synchronous off-chain RPC views!

### 1.2 Chosen Oracle Consumption Mechanism & Fallback Evaluation

| Option | Flow | Security & Latency Trade-off | M04.5A Verdict |
| --- | --- | --- | --- |
| **A. Direct Async Pull per Mutation (`ext_oracle::get_price`)** | Mutation acquires lock + reserves collateral $\to$ `ext_oracle::get_price(market)` (`10 TGas`) $\to$ `#[private]` callback (`35 TGas`) validates & settles | Reads the latest committed Oracle block state with **zero stale cache lag**; costs `~45 TGas` + ~1-2 block finality per action | **Selected Canonical Pattern** |
| **B. Verifiable Pull Snapshot Sync (`sync_oracle_price`) + Push Rejection** | Keeper pushes arbitrary raw price (`Push`) **OR** calls `sync_oracle_price` which runs `ext_oracle::get_price` and stores cached `PriceObservation` locally | Arbitrary unauthenticated `Push` creates catastrophic price-spoofing risk (**FORBIDDEN**). Contract-verified `sync_oracle_price` cache introduces multi-block lag between `oracle-adapter` update and `sync` tx, opening back-running windows if cached too long | **Rejected for unverified Push; `sync_oracle_price` kept ONLY as optional read-cache optimization if strict `observed_at` age checks still hold** |

### 1.3 Locking, Pre-Reservation & Async Safety Protocol
Because ~1–2 blocks elapse between initiating an async call (`open`, `increase`, `remove_margin`, `close`, `liquidate`, `lp_deposit`, `lp_withdraw`) and its `#[private]` callback, concurrent receipts from the same trader or keeper could double-spend `free_collateral` or double-liquidate the same position unless locked atomically:

1. **Pre-Action Validation & Locks (`Step 1 — Synchronous`):**
   - Acquire `pending_account_locks: LookupSet<AccountId>` on user actions affecting `free_collateral` (blocks overlapping withdrawals/opens by same user while in-flight), OR deduct `max_required_amount` from `free_collateral -> in_flight_action_escrow` atomically!
   - Acquire `pending_position_locks: LookupMap<u64, PendingActionKind>` on `position_id` (prevents double liquidation, concurrent `close` + `remove_margin`, or concurrent `liquidate` + `close`!).
   - Verify conservative storage budget (`>= 512 bytes` matching `oracle-adapter` `STATE_STORAGE_MARGIN_BYTES` / `RECORD_STORAGE_MARGIN_BYTES` patterns).
2. **External Call Execution (`Step 2 — Promise`):**
   - Invoke `ext_oracle::ext(self.oracle_contract).with_static_gas(ORACLE_VIEW_GAS).get_price(oracle_market)` and chain `.then(Self::ext(env::current_account_id()).with_static_gas(CALLBACK_GAS).on_oracle_price_*(...))`.
3. **Private Callback Settlement (`Step 3 — #[private]` MUST NEVER PANIC):**
   - Enforce `#[private]` (`env::predecessor_account_id() == env::current_account_id()`).
   - Inspect `env::promise_result_checked(0, MAX_ORACLE_CALLBACK_BYTES)` (caps payload size, rejects `PromiseError::Failed` / `TooLong` gracefully!).
   - Re-verify **all state + risk + slippage + freshness invariants** using fresh `PriceObservation` and `env::block_timestamp_ms() / 1000`.
   - **On Success:** Execute atomic accounting transition inside `ContractCashBuckets`, **release locks**, check `assert_solvency()`, emit NEP-297 event.
   - **On Failure (`PromiseError`, stale price, slippage exceeded, utilization exceeded):** Refund any `in_flight_action_escrow` 1:1 back into `free_collateral`, **release locks cleanly**, assert solvency, log structured failure event, and exit **without panicking** (guaranteeing lock release commits on chain!).

---

## 2. Collateral Integration (`Circle NEAR Testnet USDC`, M04.5B.1) & Transfer Recovery

### 2.1 Default Collateral: Official Circle Testnet USDC (`3e2210e...af`); Tidepool & `rUSD` Boundaries
- **Default Account ID (On-Chain NEAR):** `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af` (Defuse/Intents URI: `nep141:3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`). Mainnet Circle USDC `17208628...36133a1` and `nep141:` URIs are strictly rejected at allowlist boundaries.
- **Verified Live RPC Standards (`315,265` WASM bytes, `37` exports):** `NEP-141` (`ft_transfer`, `ft_transfer_call`, `ft_resolve_transfer`, `ft_balance_of`, `ft_total_supply`), `NEP-145` (`storage_deposit` min/max `0.00125 NEAR`), `NEP-148` (`"spec": "ft-1.0.0"`, `name: "USDC"`, `symbol: "USDC"`, **`decimals: 6`**, `1,661`-byte inline SVG `icon`). `on_verify_collateral_metadata` supports `MAX_METADATA_PAYLOAD_BYTES = 16_384` and validates `spec == "ft-1.0.0"`, `symbol`, and `decimals`.
- **Conversion & Residual Handling:** `scale_to_usd_e8 = 100` converts 6-decimal base units (`amount_usdc_e6 * 100 = amount_usd_e8`) cleanly into internal `USD_SCALE = 10^8`. Outbound withdrawals floor to whole base units (`amount_e8 / 100`), support exact partial `amount_token_units` while rejecting over-withdrawals (`ERR_INSUFFICIENT_FREE_COLLATERAL`), and retain `<100 e8` sub-micro-USDC dust losslessly in `free_collateral_e8` (`sub_micro_unit_residue_e8`).
- **Asset Distinction:**
  - `usdc.tidepool-sandbox.testnet` is kept strictly as an **alternative integration fixture** (rejected unless explicitly allowlisted).
  - `integration-tests/near-sandbox-js/mock-ft` is a **local Sandbox fixture only**.
  - `contracts/demo-token` (`rUSD`) was **not** created and remains deferred to the future roadmap.
- **Prerequisites:** Deployed Perps contract + user/LP recipients must hold `0.00125 NEAR` `storage_deposit` on `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`, plus `0.05 NEAR` via `register_account` on Perps. Testnet USDC may be acquired manually (up to `20 USDC / 2h`) via `https://faucet.circle.com/`.

### 2.2 Perps Receiver & Async Transfer Lifecycle
- **Inbound Deposits (`ft_on_transfer`):**
  - **Strict Token Allowlist (`collateral_tokens.get(&token_id)`):** Unknown NEP-141 tokens (even named `USDC`) or deactivated tokens fail/reject without mutating state, causing standard `ft_resolve_transfer` refund to sender.
  - Parses strict typed `DepositMemo` JSON (`"trader_collateral"` | `"lp_deposit"` (+ optional `min_shares`) | `"insurance_fund"`).
  - Returns `PromiseOrValue::Value(U128(0))` when synchronous (`TestnetSandboxOperator` or `open_positions == 0` LP deposit), or dispatches async XCC `get_price` -> `#[private] on_oracle_lp_deposit` with safe rollback if an LP deposit arrives while XCC markets have open positions.
- **Outbound Withdrawals (`ft_transfer` + `on_withdraw_transfer` Callback):**
  - Deducts `amount_e8` from `free_collateral_e8` (or LP equity) into `in_flight_withdrawals` and marks `has_pending_withdrawal = true` **before** dispatching `ext_ft::ft_transfer` (`1 yoctoNEAR`, `FT_TRANSFER_GAS = 25 TGas`).
  - Callback `#[private] on_withdraw_transfer(withdrawal_id)` (plus explicit `require!(predecessor == current_account_id)` + anti-replay `pending_withdrawals.remove`):
    - `Ok(_)`: Decrements `in_flight_withdrawals`, `tracked_collateral_e8`, and `tracked_balance_units`; emits settled event.
    - `Err(_)`: Decrements `in_flight_withdrawals`, restores `amount_e8` cleanly back to `free_collateral_e8` (even on failed LP or treasury claim transfers without panicking), preserves `tracked_collateral_e8`, and emits `"restored"`.

---

## 3. Implemented Contract State & Methods (`contracts/perpetuals/src/lib.rs`)

### 3.1 Core Data Structures
- `PerpsContract`: `admin`, `pending_admin`, `treasury`, `paused_global`, `tracked_collateral_e8`, `buckets: StoredCashBuckets`, `lp_total_shares`, `lp_locked_seed_shares`, `collateral_tokens: IterableMap<AccountId, CollateralTokenConfig>`, `markets: IterableMap<String, PerpsMarketState>`, `accounts: IterableMap<AccountId, AccountRecord>`, `positions: IterableMap<u64, PositionRecord>`, `pending_withdrawals`, `pending_oracle_actions`.
- `PerpsMarketState`: `market_id`, `unit`, `mode: StoredMarketMode`, `paused_for_insolvency`, `risk: StoredMarketRiskParams` (`max_pnl_bps: Some(10_000)` required), `oracle_policy: StoredOraclePolicy`, `oracle_source: OracleSourceConfig` (`ExternalXcc` | `TestnetSandboxOperator` with `U128` bounds), `exec_spread_bps`, `long_oi_e8`, `short_oi_e8`, `long_size_e8`, `short_size_e8`, `open_positions`, `cum_funding_index_e12`, `last_funding_sec`, `last_valid_price_e8`, `last_valid_observed_sec`, `sandbox_observation`, `attested_emergency_price_e8`, `frozen_settlement_price_e8`, `frozen_funding_index_e12`.
- `PositionRecord`: `id: u64`, `owner`, `market_id`, `side`, `size_e8`, `notional_e8`, `entry_price_e8`, `locked_margin_e8`, `entry_funding_index_e12`, `opened_at_sec`, `locked: bool`.
- `AccountRecord`: `registered: bool`, `storage_paid_yocto`, `free_collateral_e8`, `lp_shares`, `pending_action_lock`, `has_pending_withdrawal`.
- `MarketView` (JSON view): adds explicit `feed_classification`, `is_synthetic_testnet_sandbox`, `sandbox_operator`, `sandbox_min_price_e8`, `sandbox_max_price_e8`, and `sandbox_sequence` alongside `last_valid_price_e8`, `last_valid_observed_sec`, and `oracle_policy`.

### 3.2 Method Catalog (Implemented Public ABI)

| Role | Implemented Method Signatures |
| --- | --- |
| **Init** | `init(admin, treasury, primary_collateral_token, collateral_symbol, collateral_decimals, non_monetary_testnet_disclosure)` |
| **Account Storage & FT Hook** | `register_account(account_id?)`, `storage_deposit(account_id?, registration_only?)`, `ft_on_transfer(sender_id, amount, msg) -> PromiseOrValue<U128>` |
| **Trader** | `withdraw_collateral(token_id, amount_token_units?)`, `open_position(market_id, side, size_e8, collateral_e8, price_limit_e8?)`, `increase_position(position_id, add_size_e8, add_collateral_e8, price_limit_e8?)`, `reduce_position(position_id, close_size_e8, price_limit_e8?)`, `close_position(position_id, price_limit_e8?)`, `add_margin(position_id, amount_e8)`, `remove_margin(position_id, amount_e8)`, `claim_emergency_settlement(position_id)` |
| **LP** | `withdraw_lp(token_id, shares_to_burn, min_assets_out_e8?)` *(LP deposit via `ft_transfer_call`; locked in `Emergency` until `open_positions == 0`)* |
| **Keeper / Oracle Operator** | `accrue_funding(market_id)`, `liquidate_position(position_id)`, `trigger_emergency_settlement(market_id)`, `push_sandbox_oracle_price(market_id, price_e8, observed_at_sec, sequence)` *(operator-restricted `TESTNET_SANDBOX_FEED`)* |
| **Admin / Treasury** | `propose_admin`, `accept_admin`, `cancel_admin_proposal`, `set_global_pause`, `add_collateral_token`, `set_collateral_token_active`, `verify_collateral_metadata`, `register_market`, `update_risk_params`, `set_market_mode`, `set_emergency_recovery_price`, `claim_protocol_fees(token_id, amount_token_units?)` |
| **Private Callbacks** | `on_verify_collateral_metadata`, `on_withdraw_transfer`, `on_oracle_lp_deposit`, `on_oracle_action` |
| **Views (`&self`)** | `get_admin`, `get_pending_admin`, `get_treasury`, `is_paused_global`, `get_collateral_token`, `list_collateral_tokens`, `storage_balance_bounds`, `storage_balance_of`, `is_account_registered`, `get_account`, `get_market`, `list_markets`, `get_position`, `get_positions_by_owner`, `get_position_health`, `preview_emergency_settlement`, `preview_position_settlement_at`, `get_lp_snapshot`, `get_accounting_snapshot`, `verify_solvency_invariant`, `get_internal_usd_scale` |

---

## 4. NEP-297 Events Catalog (Implemented Events)

All events conform strictly to **NEP-297** (`EVENT_JSON:{"standard":"roastake-perps","version":"1.0.0","event":"...","data":[{...}]}`):

| # | Event Name (`event`) | Key `data[0]` Payload Fields |
| --- | --- | --- |
| 1 | `collateral_deposited` | `account_id`, `token_id`, `amount_token_units`, `amount_e8`, `new_free_collateral_e8` |
| 2 | `collateral_withdrawn` | `account_id`, `token_id`, `withdrawal_id`, `amount_token_units`, `amount_e8`, `status` (`"initiated" \| "settled" \| "restored"`) |
| 3 | `lp_deposited` | `account_id`, `token_id`, `amount_e8`, `shares_minted`, `seed_shares_locked`, `nav_e8` |
| 4 | `lp_withdrawn` | `account_id`, `token_id`, `withdrawal_id`, `shares_burned`, `assets_out_e8`, `transfer_units`, `residue_to_free_e8`, `nav_e8` |
| 5 | `position_opened` | `position_id`, `owner`, `market_id`, `side`, `size_e8`, `notional_e8`, `exec_price_e8`, `margin_e8`, `fee_e8`, `funding_index_e12` |
| 6 | `position_increased` | `position_id`, `owner`, `added_size_e8`, `new_size_e8`, `new_entry_price_e8`, `added_margin_e8`, `fee_e8` |
| 7 | `position_reduced` | `position_id`, `owner`, `closed_size_e8`, `remaining_size_e8`, `exec_price_e8`, `realized_pnl_e8`, `funding_settled_e8`, `fee_e8`, `trader_payout_e8` |
| 8 | `position_closed` | `position_id`, `owner`, `market_id`, `exec_price_e8`, `realized_pnl_e8`, `funding_settled_e8`, `fee_e8`, `payout_e8`, `bad_debt_e8`, `emergency` (`bool`) |
| 9 | `position_liquidated` | `position_id`, `owner`, `keeper`, `market_id`, `mark_price_e8`, `keeper_fee_e8`, `insurance_fee_e8`, `close_fee_e8`, `trader_refund_e8`, `bad_debt_e8` |
| 10 | `margin_added` | `position_id`, `owner`, `delta_margin_e8`, `new_locked_margin_e8`, `mark_price_e8` |
| 11 | `margin_removed` | `position_id`, `owner`, `delta_margin_e8`, `new_locked_margin_e8`, `mark_price_e8` |
| 12 | `funding_accrued` | `market_id`, `elapsed_sec`, `index_delta_e12`, `new_cum_funding_index_e12` |
| 13 | `insurance_updated` | `reason` (`"funded" \| "bad_debt_absorbed"`), `delta_e8`, `new_balance_e8`, `uncovered_insolvency_e8` |
| 14 | `market_paused` | `market_id`, `triggered_by`, `previous_mode`, `new_mode` (`"ReduceOnly" \| "Paused" \| "Emergency"`), `reason`, `frozen_settlement_price_e8`, `frozen_funding_index_e12` |
| 15 | `market_resumed` | `market_id`, `triggered_by`, `previous_mode`, `new_mode` (`"Normal"`), `reason`, `frozen_settlement_price_e8`, `frozen_funding_index_e12` |
| 16 | `oracle_sandbox_tick` | `market_id`, `publisher`, `sequence`, `price_e8`, `observed_at_sec`, `data_class` (`"TESTNET_SANDBOX_FEED"`) |
| 17 | `security_anomaly` | `kind`, `actor`, `detail` |

---

## 5. Error Code Registry (`contracts/perpetuals/src/*`)

- `ERR_UNAUTHORIZED`, `ERR_MAINNET_REJECTED`, `ERR_INVALID_COLLATERAL_TOKEN`, `ERR_ACCOUNT_LOCKED`, `ERR_POSITION_LOCKED`, `ERR_INSUFFICIENT_FREE_COLLATERAL`, `ERR_INSUFFICIENT_MARGIN`, `ERR_BELOW_MAINTENANCE_MARGIN`, `ERR_INSUFFICIENT_LIQUIDITY`, `ERR_INSUFFICIENT_LIQUIDITY_SETTLEMENT_ORDER`, `ERR_EMERGENCY_LP_LOCKED`, `ERR_MAX_UTILIZATION_EXCEEDED`, `ERR_MAX_OI_SKEW_EXCEEDED`, `ERR_SLIPPAGE_EXCEEDED`, `ERR_ORACLE_STALE`, `ERR_ORACLE_CLASS_REJECTED`, `ERR_PRICE_JUMP_CIRCUIT_BREAKER`, `ERR_MARKET_PAUSED`, `ERR_MARKET_REDUCE_ONLY`, `ERR_EMERGENCY_NOT_ELIGIBLE`, `ERR_POSITION_NOT_LIQUIDATABLE`, `ERR_SOLVENCY_INVARIANT_VIOLATED`.

---

## 6. Assumptions, Open Decisions & Pending Risks

- **Assumptions:** Outbound `ft_transfer` requires the recipient to hold NEP-145 `storage_deposit` (`0.00125 NEAR`) on Circle NEAR Testnet USDC (`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`) and not be Circle-blocklisted/paused; failed transfers restore 100% credit in `on_withdraw_transfer` cleanly. (`usdc.tidepool-sandbox.testnet` alternative fixture only; `rUSD` deferred to future roadmap.)
- **Open Decisions:** Whether `open_position` combines deposit + open inside one `ft_transfer_call` memo in M04.5D or keeps `deposit` (`ft_transfer_call`) + `open_position` two explicit calls for simpler rollback isolation (**recommended MVP default: separate deposit & open**).
- **Pending Risks:** OutLayer gas spikes or high-concurrency NEAR congestion delaying `get_price` callbacks; mitigated via per-account locks + generous callback gas allocation (`35–50 TGas`) + never panicking inside callbacks.
