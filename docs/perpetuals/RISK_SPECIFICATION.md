<!-- SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only -->
<!-- Copyright © 2026 @MTheMoneyApe. All rights reserved. -->

> **License:** [ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0](../../LICENSE-ROASTAKE-PERPS.md) — Copyright © 2026 @MTheMoneyApe. All rights reserved. **Licensor:** @MTheMoneyApe

# ROASTAKE Perpetuals — Mathematical Formulas, Risk Policy & Threat Model (M04.5B.1)

- **Status:** Pre-Deployment Validated for Circle NEAR Testnet USDC (`contracts/perpetuals` + `packages/shared/src/perps.rs`)
- **Date:** 2026-11-11
- **Version:** `1.3.0-m045b1`
- **Target Network:** NEAR Testnet Only

---

## 1. Fixed-Point Scales & Overflow Proof (`u128` / `i128` vs `U256`)

All on-chain and shared calculations execute deterministically in integer fixed-point with **zero IEEE-754 floating-point instructions**.

| Constant (`perps.rs`) | Value | Scale Semantics | Unit Tag |
| --- | --- | --- | --- |
| `PRICE_SCALE` | $10^8$ | `1 USD/kg = 100_000_000` | `D8{USD/kg}` |
| `SIZE_SCALE` | $10^8$ | `1 kg = 100_000_000` ($10^{-8}\text{ kg}$ step) | `D8{kg}` |
| `USD_SCALE` | $10^8$ | Internal 8-decimal ledger (`100` internal units per `10^-6` Circle Testnet USDC base unit; lossless `<100 e8` residue kept in `free_collateral_e8`; Tidepool kept as alternative test fixture only) | `D8{USD}` |
| `BPS_SCALE` | $10^4$ | `10_000 = 100.00%`, `1 = 0.01%` | `{bps}` |
| `FUNDING_SCALE` | $10^{12}$ | Rate `D12{1/s}` & Cumulative Index `D12{USD/kg}` | `D12{...}` |
| `LP_SHARE_SCALE` | $10^8$ | Matches `USD_SCALE` 1:1 | `D8{share}` |

### 1.1 Bit-Width & Zero-External-Dependency `U256` Proof
Can plain single-word `u128` / `i128` suffice everywhere, and should an external `U256` dependency be added?
1. **Why we reject adding an external `U256` crate:** Adding third-party `uint`/`primitive-types` bloat violates dependency stability (`Cargo.lock` minimal) when pure arithmetic bounds already show headroom — EXCEPT in **LP Share Mint/Redeem** and **arbitrary `mul_div`** where multiplying two `D8{USD}` (`u128`) capital figures (`deposit * total_shares` at `$10^{10}+`) could exceed `u128::MAX` ($\approx 3.4 \times 10^{38}$) before division!
2. **Market Bounds Proof:**
   - Under configured coffee price bounds ($P \le \$500\text{/kg} = 5 \times 10^{10}$ in `oracle-adapter`, stress ceiling $\$10,000\text{/kg} = 10^{12}$) and extreme position ceiling ($Q \le 10^8\text{ kg} = 10^{16}$, i.e., `100,000 metric tons`):
     - Notional product: $p \cdot q \le 10^{12} \times 10^{16} = 10^{28} \ll u128::MAX (3.4 \times 10^{38})$.
     - Margin/Fee product: $\text{notional} \cdot \text{bps} \le 10^{20} \times 10^4 = 10^{24} \ll u128::MAX$.
     - Funding payment product: $q \cdot \Delta I_{f,\text{e12}} \le 10^{16} \times 10^{16} = 10^{32} \ll i128::MAX (1.7 \times 10^{38})$.
3. **Internal Dependency-Free 256-bit `mul_div` (`mul_wide_u128` + `div_rem_256_by_128`):**
   - Implemented cleanly in ~45 lines of pure Rust inside `packages/shared/src/perps.rs` with zero external crates. Eliminates spurious intermediate `a * b` overflow for LP share ratios and `mul_div` while still returning `Err(PerpMathError::Overflow)` whenever the **mathematical quotient** exceeds `u128::MAX` / `i128` bounds (or when `ceil` pushes exact `floor == u128::MAX` past bounds), and safely handling `i128::MIN.unsigned_abs()`. Proven across a 640-point `(a, b, d)` grid against an independent 4x64 schoolbook + bit-by-bit 256-bit long-division oracle (`ref_mul_div_256`).

---

## 2. Validated Financial Formulas

### 2.1 Price Model: Index, Mark, Execution & Spread
Because Phase 1 uses oracle-indexed settlement rather than an active orderbook:
- **Never simulate fake orderbook depth.** Execution price is computed deterministically on-chain from the fresh validated Oracle price:
  1. **Index Price ($P_{\text{index}}$):** Directly from verified `roastake-oracle-adapter` `PriceObservation` (`D8{USD/kg}`).
  2. **Mark Price ($P_{\text{mark}}$):** Equals $P_{\text{index}}$ in Phase 1 (after passing staleness + jump circuit-breaker policy).
  3. **Execution Price ($P_{\text{exec}}$):** Applies a deterministic configurable half-spread (`exec_spread_bps`, e.g., `5–10 bps` configurable alongside `trade_fee_bps = 10 bps`) **against the taker direction** (Long opens buy slightly higher / closes sell slightly lower; Short opens sell slightly lower / closes buy slightly higher) OR explicit `slippage_guard` via user-supplied `[min_acceptable_price, max_acceptable_price]` checked in the async callback!
  - **Anti-Oracle-Latency Arbitrage:** Combining `trade_fee_bps` (`10 bps` open + `10 bps` close = `20 bps` round-trip) + `exec_spread_bps` makes exploiting tiny intra-cadence (`<= 300s`) coffee drift unprofitable, and circuit-breaking $> 500\text{ bps}$ jumps blocks trading on known large external moves!

### 2.2 Core Equations & Rounding Directions

| Calculation | Formula (Real Units) | Fixed-Point Formula (`perps.rs`) | Rounding |
| --- | --- | --- | --- |
| **Notional** (`D8{USD}`) | $P \cdot Q$ | `floor((p * q) / 10^8)` or `ceil((p * q) / 10^8)` | `ceil` for IM/MM/Fee; `floor` for Margin Ratio |
| **Long PnL** (`D8{USD}`) | $Q \cdot (P_1 - P_0)$ | `floor` if $P_1 \ge P_0$, else `-ceil(Q*(P0-P1)/10^8)` | Toward $-\infty$ ($\text{PnL}_L + \text{PnL}_S \le 0$) |
| **Short PnL** (`D8{USD}`) | $Q \cdot (P_0 - P_1)$ | `floor` if $P_0 \ge P_1$, else `-ceil(Q*(P1-P0)/10^8)` | Toward $-\infty$ |
| **Profit Cap (`max_pnl_bps`)** | $\min(\text{PnL}_p,\; N_0 \cdot \text{cap} / 10^4)$ | `apply_profit_cap_usd_e8` (`10_000 bps = 100%` of $N_0$) | Down (`floor`) on cap ceiling; guarantees LP vault solvency |
| **Initial Margin** | $N \cdot \text{im\_bps} / 10^4$ | `ceil((notional_ceil * im_bps) / 10_000)` | Up (`ceil`) |
| **Maintenance Margin** | $N \cdot \text{mm\_bps} / 10^4$ | `ceil((notional_ceil * mm_bps) / 10_000)` | Up (`ceil`) |
| **Trade Fee** | $N \cdot \text{fee\_bps} / 10^4$ | `ceil((notional_ceil * fee_bps) / 10_000)` | Up (`ceil`) |
| **Position Equity** | $M_p + \text{PnL}_p + \text{Funding}_p$ | `m as i128 + pnl + funding_net` | Exact integer sum |
| **Margin Ratio (`bps`)** | $\max(0, \text{Eq}) / N$ | `floor(max(0, eq) * 10_000 / mark_notional)` | Down (`floor`) |
| **Utilization (`bps`)** | $R_{\text{LP}} / E_{\text{LP}}$ | `compute_utilization_bps`: `ceil(reserved * 10_000 / equity)` | Up (`ceil` — conservative utilization limit) |
| **Available Margin** | Excess safe margin | $\min(M_p - \text{IM}_0,\; \max(0, \text{Eq} - \text{IM}_{\text{mark}}))$ | Protects unrealized profits |
| **Indicative Liq. Price** | Price where $\text{Eq}(P) = \text{MM}(P)$ | `compute_indicative_liquidation_price_e8` *(includes `funding_net_e8`)* | UI indicator ONLY — real `liquidate` strictly verifies $\text{Eq} \le \text{MM}$ |
| **Liquidation Trigger** | $\text{Eq}_p \le \text{MM}(P_{\text{mark}})$ | Recomputed **strictly on-chain** inside XCC callback | Never trusts frontend est. liq price |

### 2.3 Funding Accrual & Non-Creation Proof
- **Margin Mode Decision:** Strictly **Isolated Margin (`MarginMode::Isolated`)** per position for MVP, isolating per-position insolvency and async NEAR locks.
- **High-Precision Interval Funding Delta (`M04.5A.1`, zero `/ 3600` pre-truncation loss):**
  - Instead of pre-truncating tiny hourly rates via $\lfloor r_{\max,\text{hr}} \cdot |OI_L - OI_S| / (\text{denom} \cdot 3600) \rfloor$ (which dropped modest Testnet skews to `0`), `compute_funding_delta_over_interval_e12` multiplies numerator terms first across 256-bit arithmetic (`mul_div_floor_u128`):
    $$|\Delta I_f| = \left\lfloor \frac{\min\!\big(|OI_L - OI_S| \cdot r_{\max,\text{hr}},\; \text{denom} \cdot r_{\max,\text{hr}}\big) \cdot P_{\text{index}} \cdot \Delta t}{\text{denom} \cdot 3600 \cdot \text{PRICE\_SCALE}} \right\rfloor, \quad \Delta I_f = \operatorname{sign}(OI_L - OI_S) \cdot |\Delta I_f|$$
    with $\text{denom} = \max(E_{\text{LP}}, B_{\min})$ and $\Delta t = \min(\text{now} - t_{\text{last}}, \Delta t_{\max})$ (rejects `now < t_last` with `RegressiveTimestamp`).
  - Magnitude-symmetric sign application (`compute_funding_index_delta_e12` also uses `sign * floor(|num| / PRICE_SCALE)`) ensures positive and negative skews accrue identically without 1-wei sign asymmetry.
- **Position Funding Settlement (`D8{USD}`):**
  $$\text{FundingNet}_L = \left\lfloor \frac{-Q_L \cdot (I_{\text{now}} - I_{\text{entry}})}{\text{FUNDING\_SCALE}} \right\rfloor, \quad \text{FundingNet}_S = \left\lfloor \frac{+Q_S \cdot (I_{\text{now}} - I_{\text{entry}})}{\text{FUNDING\_SCALE}} \right\rfloor$$
  Because floor toward $-\infty$ rounds positive credits down and negative debits more negative:
  - Matched Long + Short ALWAYS satisfies $\text{FundingNet}_L + \text{FundingNet}_S \in \{-1, 0\} \le 0$ (**zero value creation**).
  - Unmatched net funding is collected/paid strictly upon **realized margin settlement** (`close` / `liquidate`), NEVER prematurely added as cash to $E_{\text{LP,realized}}$ while uncollected from underwater accounts!
  - Skew zero ($OI_L = OI_S \implies r_{\text{sec}} = 0$); direction flip smoothly crosses zero and reverses payer/receiver signs.

---

## 3. Loss Absorption Waterfall & Scope of ADL

### 3.1 Strict Loss Waterfall Cascade
When an underwater position closes or liquidates with `Equity < 0` (`shortfall = -Equity > 0`):
1. **Tier 1 — Position Locked Margin (`M_p`):** Confiscated first to zero (`rem = 0`).
2. **Tier 2 — Open Interest Liquidity & Fee Buffer:** LP Vault collects any positive maintenance/liquidation surplus from healthy closes; ongoing `10%` fee split continuously feeds Tier 3.
3. **Tier 3 — Insurance Fund (`F_ins`):** Reimburses `absorbed_ins = min(F_ins, shortfall)` directly to `lp_vault_realized_equity`.
4. **Tier 4 — LP Vault Realized Equity (`E_LP,realized`):** Bears residual `min(E_LP_realized, shortfall - absorbed_ins)` via lower LP share NAV (`lp_vault_mtm_equity`).
5. **Tier 5 — Insolvency Pause:** If `shortfall > F_ins + E_LP_realized`, `uncovered_insolvency > 0` immediately switches the market to **`MarketMode::Paused`** and freezes withdrawals. **Unrelated traders' `free_collateral` is NEVER reduced or socialized.**

### 3.2 Auto-Deleveraging (ADL) Verdict for Phase 1
**Decision: ADL is OUT OF SCOPE (deferred past MVP).**
- **Justification:** Strict $5\times$ max leverage, $8\%$ MM ($12\%$ distance from IM), $75\%$ utilization limit, $100\%$ skew reserve coverage, and isolated margin prevent unbounded multi-market cascade failures on Testnet. Introducing automated partial closure of profitable traders adds complex priority-queue UX and edge-case async callback conflicts on NEAR. If utilization exceeds safe bounds in extreme scenarios, the market cleanly uses `ReduceOnly` / `Paused` instead of force-deleveraging profitable users.

---

## 4. Parameter Audit & Stress-Test Verdict (M04.5 Prior Values)

Section 17 requires evaluating—never blindly copying—every parameter proposed in M04.5 under stress (`5%`, `12%`, `20%`, and `50%` coffee shocks):

| Parameter | M04.5 Proposal | M04.5A Stress Analysis | M04.5A Classification & Value |
| --- | --- | --- | --- |
| **Max Leverage** | `5x` | $1 / 5 = 20\%$ IM; gives $12\%$ price buffer before $8\%$ MM. Survives typical $2\text{--}5\%$ daily Arabica swings easily. | **Approved for Testnet Spec (`5x`)** |
| **Initial Margin (IM)** | `20%` (`2000 bps`) | Exact inverse of `5x`; sufficient buffer over `8%` MM. | **Approved for Testnet Spec (`2000 bps`)** |
| **Maintenance Margin (MM)** | `8%` (`800 bps`) | Leaves $8\%$ equity above `0`; strictly enforces `liq_keeper_fee + liq_ins_fee + trade_fee < mm_bps` (`50 + 50 + 10 = 110 bps < 800 bps`) leaving `690 bps` (`> 500 bps` jump) cushion against bad debt. | **Approved for Testnet Spec (`800 bps`)** |
| **Max Positive PnL Cap (`max_pnl_bps`)** | *(Unbounded in M04.5)* | **M04.5A.1 Stress finding:** An unbounded `>100%` rally on skewed `OI` would allow a Long to exceed `1:1` locked OI reserve! Explicit `Some(10_000)` caps gain at `100%` of entry notional (`+500%` ROI at `5x`), matching 1:1 OI backing & guaranteeing Vault solvency at `U <= 75%`. | **Mandatory MVP Control (`Some(10_000)`)** |
| **Trading Fee** | `10 bps` | `20 bps` round-trip (`1%` of margin at `5x`) deters micro-scalping inside `300s` windows; competitive viability depends on live feed jitter. | **Provisional (`10 bps`)** |
| **Liquidation Fee** | `200 bps` | **Stress finding:** Unsplit `200 bps` on *notional* leaves only `8% - 2% = 6%` gap cushion (`600 bps` vs `500 bps` jump -> only `100 bps` margin of safety!). On *remaining margin*, incentive shrinks to `~$0` near zero equity. **Refinement:** Split `100 bps` total notional (`50 bps` keeper + `50 bps` insurance), leaving **`7%` (`700 bps`) gap buffer** (`> 500 bps` jump!). | **Refined / Pending Economic Validation (`50 bps` keeper + `50 bps` insurance = `100 bps`; hard cap `< MM`)** |
| **Max Utilization** | `75%` (`7500 bps`) | Retains `25%` pool buffer for modest LP liquidity flexibility & payout growth under normal volatility. | **Approved for Testnet Spec (`7500 bps`)** |
| **Oracle Freshness** | `300 s` | Rejects IMF monthly (`90d`) and synthetic (`0`) correctly. Requires live tick feed (`<= 5 min`) not yet integrated until Gate 4 or `TestnetSandbox` keeper. | **Pending Data Source (`300s` target; sandbox `300–900s`)** |
| **Max Price Jump** | `500 bps` (`5%`) | **Risk finding:** Silent rejection holding stale `P_prev` allows free-roll arbitrage on an `8%` frost shock! Refined to trigger **Circuit Breaker (`REDUCE_ONLY` / `PAUSED`)**, NOT stale execution! | **Provisional / Redefined (`500 bps` Jump Circuit Breaker)** |

---

## 5. Comprehensive Threat Model

| Attack / Failure Vector | Affected Component | Mitigation Specified in M04.5A |
| --- | --- | --- |
| **1. Stale Oracle Arbitrage** | `open` / `close` / `remove_margin` | Reject execution if `now - observed_at > 300s`; transition to `REDUCE_ONLY`. Never trade on stale prices. |
| **2. >5% Shock Front-Running** | Stale price retained on jump reject | Trigger `JumpCircuitBreaker` $\to$ `REDUCE_ONLY`/`PAUSED` instead of falling back to `previous_price`. |
| **3. LP Share Inflation / Donation Attack** | LP Vault `deposit` | Burn `MINIMUM_LP_SEED_SHARES = 10_000` on first seed; price NAV off `lp_vault_realized_equity` (ignore unsolicited bare `ft_transfer` surplus). |
| **4. LP Front-Running / Insurance Arbitrage** | LP Vault `deposit` / `withdraw` | Require fresh Oracle price whenever `open_positions > 0`; price NAV via `compute_lp_mtm_equity_with_insurance_offset_e8` (`+min(F_ins, unrealized_bad_debt)`) defeating pre-liquidation dilution; keep `lp_available_liquidity` strictly cash-backed. |
| **5. Async Callback Double-Spend / Race** | NEAR 2-block XCC `get_price` / `ft_transfer` | Acquire per-account/per-position locks & deduct collateral (`free -> reserved/in_flight`) **before** emitting Promise; release cleanly in private callbacks. |
| **6. Fake NEP-141 Collateral Spoofing** | `ft_on_transfer` | Enforce `env::predecessor_account_id() == self.collateral_token_id` strictly; refund any unknown tokens cleanly (`PromiseOrValue::Value(amount)`). |
| **7. Underwater Liquidation Stale Drain** | `liquidate` | **Never** use frontend `liquidation_price` or stale prices; require fresh price + `MM` check on-chain; cap liquidation fees strictly by remaining equity. |
| **8. Outage Trap / Voluntary Put Arbitrage** | Oracle downtime (`>= 3600s`) | `MarketMode::Emergency` executes irreversible **Uniform Global Settlement** (`compute_uniform_emergency_settlement_usd_e8`) freezing global `P_last_valid` + `I_f` across ALL Longs & Shorts (`close_fee = 0`) and locking LP exit until `OI == 0` — zero cherry-picking, zero free put option, zero trapped margin. |
| **9. Phantom Bad Debt Double-Counting** | LP Vault valuation / Solvency | Bounded trader MTM $\tilde{V}_p = \max(-M_p, V_p)$ stops insolvent margin deficits from inflating vault assets. |
| **10. Mainnet / Malicious Account Spoof** | Admin / Contract init / Collateral allowlist | Enforce `.testnet`/`.test.near`/64-hex Testnet guards (`assert_testnet_or_sandbox_account`) rejecting `.near`, `nep141:` URIs, and Mainnet Circle USDC `17208628...36133a1`. |

---

## 6. Assumptions, Open Decisions & Pending Risks

- **Assumptions:** OutLayer `oracle-adapter` `validate_worker_output` guarantees non-negative price within `MarketConfig` bounds before `roastake-perps` ever reads an observation.
- **Open Decisions:** Exact `exec_spread_bps` (0 vs 5 bps symmetric spread) calibration using live commodity feed data in Gate 4; whether an optional minimum hold delay (`min_hold_sec`, e.g., 10s–30s) is added alongside fees.
- **Pending Risks:** Extreme >`20%` overnight weekend coffee gap when feeds are offline, requiring explicit weekend/holiday schedule policy in `ORACLE_POLICY.md` once commercial feeds are selected.
