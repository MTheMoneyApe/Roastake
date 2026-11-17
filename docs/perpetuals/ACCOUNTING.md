<!-- SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only -->
<!-- Copyright © 2026 @MTheMoneyApe. All rights reserved. -->

> **License:** [ROASTAKE EVALUATION-ONLY PROPRIETARY LICENSE v1.0](../../LICENSE-ROASTAKE-PERPS.md) — Copyright © 2026 @MTheMoneyApe. All rights reserved. **Licensor:** @MTheMoneyApe

# ROASTAKE Perpetuals — Double-Entry Accounting, LP Vault & Solvency Specification (M04.5B.1)

- **Status:** Pre-Deployment Validated Accounting Specification (`contracts/perpetuals` + `packages/shared/src/perps.rs`)
- **Date:** 2026-11-11
- **Version:** `1.3.0-m045b1`
- **Target Network:** NEAR Testnet Only

---

## 1. Collateral Asset Specification (M04.5B.1): Official Circle NEAR Testnet USDC (`3e2210e...af`)

For the NEAR Testnet MVP (**M04.5B.1**), ROASTAKE configures official Circle native NEAR Testnet USDC (`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`; Intents asset URI `nep141:3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`) as the default collateral allowlist entry, while rejecting Mainnet Circle USDC (`17208628...36133a1`), keeping `usdc.tidepool-sandbox.testnet` strictly as an alternative test fixture, keeping `mock-ft` strictly as a local Sandbox fixture, and deferring bespoke `rUSD` (`contracts/demo-token`) to the future roadmap.

> **MANDATORY LEGAL & PRODUCT DISCLOSURE (Testnet Collateral & Asset Boundaries):**
> - **Circle Testnet USDC (`3e2210e...af`):** Validated directly on NEAR Testnet (`name: "USDC"`, `symbol: "USDC"`, `decimals: 6`, `0.00125 NEAR` storage bound, WASM SHA-256 `ccfb5ad61c10b5a364d405eb404926f31b37bf636897ab9c7cfc3e74ffbf12ab`). Carries **zero monetary/Mainnet value**, is governed on Testnet by Circle multisig/pauser/blocklister roles (`roastake-dev-mta.testnet` has `is_minter = false`, `is_blocklisted = false`), and `1 USDC = 1 USD` is an accounting-only parity convention.
> - **Tidepool Testnet USDC (`usdc.tidepool-sandbox.testnet`):** External third-party non-monetary token retained ONLY as an alternative integration fixture; rejected unless explicitly allowlisted via `add_collateral_token`.
> - **Local Sandbox Fixture (`mock-ft`):** Compiled test helper WASM (`roastake_mock_tidepool_ft.wasm`) used exclusively inside local `near-sandbox` tests — never deployed to public Testnet.
> - **Future `rUSD` (`contracts/demo-token`):** Not implemented or deployed in M04.5B / M04.5B.1; deferred to future roadmap.

### Token Precision Alignment & Lossless 6 <-> 8 Scaling (`scale_to_usd_e8 = 100`)
- Verified on-chain NEP-148 metadata: `spec = "ft-1.0.0"`, `name = "USDC"`, `symbol = "USDC"`, `decimals = 6` (`1 USDC = 1_000_000` base units), plus `1,661`-byte SVG `icon` (`1,772`-byte total JSON response handled cleanly via `MAX_METADATA_PAYLOAD_BYTES = 16_384`), and NEP-145 storage bound `0.00125 NEAR` (`1_250_000_000_000_000_000_000 yoctoNEAR`).
- All internal ledger balances, realized/unrealized PnL, skew funding, fees, LP shares, and `ContractCashBuckets` execute strictly at `USD_SCALE = 10^8` (`roastake_shared::perps`).
- **Deposit scaling:** Validated 6-decimal base units multiply exactly via checked `amount_usdc_e6 * 100 = amount_usd_e8` (`scale_to_usd_e8 = 100`) upon verified `ft_on_transfer`. (Tokens with `decimals > 8` fail closed.)
- **Withdrawal rounding & lossless `< 100 e8` residue:** Outbound `ft_transfer` transfers whole `10^6` base units (`token_units = amount_e8 / 100` — partial withdrawals checked against `free_collateral_e8 >= units * 100` and rejecting `units == 0` or over-withdrawal), while preserving any sub-micro-USDC remainder (`amount_e8 % 100` in `[0, 99] e8`) inside `free_collateral_e8` (`sub_micro_unit_residue_e8`) and `total_trader_free_collateral_e8` — conserving `tracked_collateral_e8 == sum(buckets) == sum(tracked_balance_units * 100)` bit-for-bit.

---

## 2. Separation of All 10 Protocol Accounting Concepts

The contract separates realized on-chain cash claims from mark-to-market (unrealized) valuation adjustments so the same underlying Testnet USDC custody (`USD_SCALE = 10^8` internal units) is **never counted twice** across users, LPs, or reserves.

### 2.1 Realized Cash-Backed Ledger (`ContractCashBuckets`, `USD_SCALE = 10^8`)
Every tracked collateral unit inside `perps_contract` (`tracked_collateral_e8 = sum(tracked_balance_units * 100)`, excluding unallocated bare external `ft_transfer`s) belongs to **one and only one** of these six disjoint accounts:

| # | Bucket Name | Ownership / Role | Balance Sign | Extractable Directly? |
| --- | --- | --- | --- | --- |
| 1 | `total_trader_free_collateral` ($\sum C_{\text{free},u}$) | Sum of unassigned trader deposits, settled PnL & `<100 e8` residues | $\ge 0$ | Whole `10^6` units via `withdraw_collateral` (unless async-locked) |
| 2 | `total_trader_locked_margin` ($\sum M_p$) | Isolated collateral locked inside active positions | $\ge 0$ | No (only via `remove_margin` if health stays $\ge \text{IM}$, or `close`) |
| 3 | `total_pending_withdrawals` ($W_{\text{in\_flight}}$) | User/LP/Treasury withdrawals awaiting outbound `ft_transfer` callback | $\ge 0$ | In flight on NEAR; restores to #1/#4 on transfer failure |
| 4 | `lp_vault_realized_equity` ($E_{\text{LP,realized}}$) | Net realized LP cash (deposits + LP fees + losing margin - winning payouts) | $\ge 0$ | Only up to `lp_available_liquidity` (never touches reserved funds) |
| 5 | `insurance_fund` ($F_{\text{ins}}$) | Segregated first-loss bad-debt reserve | $\ge 0$ | No (solely absorbs bad debt or admin-governed safety rules) |
| 6 | `protocol_fees` ($F_{\text{proto}}$) | Accumulated Treasury fee split | $\ge 0$ | Only by Protocol Treasury address |

### 2.2 Valuation, Encumbrance & Insolvency Metrics (Non-Cash / Derived)
These four quantities govern limits, pricing, and loss absorption without inflating cash balances:

| # | Metric | Definition & Purpose |
| --- | --- | --- |
| 7 | `trader_unrealized_pnl` ($\text{PnL}_p + \text{FundingNet}_p$) | Open-position MTM gain/loss before close fee (positive profit bounded by `max_pnl_bps = 10_000`). Never adds cash until closed; loss bounded against LP equity via $\tilde{V}_p = \max(-M_p, V_p)$, while $-\min(0, V_p + M_p)$ tracks `unrealized_bad_debt`. |
| 8 | `lp_vault_equity` ($E_{\text{LP,MTM}}$) | Mark-to-Market LP NAV via `compute_lp_mtm_equity_with_insurance_offset_e8`: $E_{\text{LP,realized}} + \min(F_{\text{ins}}, \sum \text{bad\_debt}_{\text{unrealized}}) - \sum \tilde{V}_p$. Governs LP share mint/burn prices ONLY; defeats pre-liquidation insurance arbitrage while never creating withdrawable cash. |
| 9 | `lp_reserved_liquidity` ($R_{\text{LP}}$) | Encumbered capital backing open OI: $\min(E_{\text{LP,realized}},\; \max(R_{\text{OI}}, \max(0, \Delta_{\text{MTM}})))$. |
| 10a | `lp_available_liquidity` ($A_{\text{LP}}$) | Realized LP cash freely withdrawable right now: $E_{\text{LP,realized}} - \max(R_{\text{OI}}, \max(0, \Delta_{\text{MTM}}))$. |
| 10b | `bad_debt` ($\text{BadDebt}$) | Realized shortfall when closing equity $< 0$: $-\min(0, M_p + \text{PnL}_p + \text{Funding}_p)$. Flows through Insurance $\to$ LP Vault $\to$ Pause cascade. |

---

## 3. Formal Double-Entry Model & Solvency Invariant

### 3.1 Master Realized Solvency Equation
Let $B_{\text{tracked}}$ be the contract's internal `USD_SCALE = 10^8` record of physically settled collateral custody ($\sum \text{tracked\_balance\_units} \times \text{scale\_to\_usd\_e8}$, updated ONLY in `ft_on_transfer` deposits and settled `on_withdraw_transfer(Ok)` callbacks), and let $B_{\text{chain}} = \text{ft\_balance\_of}(\text{perps\_contract}) \times 100$ on Circle NEAR Testnet USDC (`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`).

$$\textbf{Accounted Liabilities \& Equity} \equiv C_{\text{free}} + M_{\text{locked}} + W_{\text{in\_flight}} + E_{\text{LP,realized}} + F_{\text{ins}} + F_{\text{proto}}$$

$$\textbf{Hard Solvency Invariant:} \quad B_{\text{chain}} \ge B_{\text{tracked}} \equiv \text{Accounted Liabilities \& Equity}$$

- **Why `unallocated_surplus = B_chain - B_tracked` stays segregated:** If anyone calls bare `ft_transfer(perps_contract)` (instead of `ft_transfer_call`), $B_{\text{chain}}$ rises while $B_{\text{tracked}}$ stays constant. By measuring LP equity strictly from $E_{\text{LP,realized}}$ rather than raw $B_{\text{chain}}$, **external token donations CANNOT inflate LP share value or steal rounding precision**.
- **Async Receipt Precision During Withdrawals:** When a withdrawal starts, Testnet USDC tokens are **still physically in `perps_contract`** until `ft_transfer` executes in the next receipt. Keeping the converted `USD_SCALE` amount in `W_in_flight` guarantees $\Delta(C_{\text{free}} + W_{\text{in\_flight}}) = 0$ prior to transfer settlement, and $\Delta(B_{\text{tracked}} - W_{\text{in\_flight}}) = 0$ on `Ok(_)` callback!

### 3.2 Algebraic Transition Table (Zero Double-Counting Proofs)

Let $\Delta$ track changes across `(Physical/Tracked B, Free C_free, Locked M, InFlight W, LP E_LP, Ins F_ins, Proto F_proto)`:

| Operation | $\Delta B_{\text{tracked}}$ | $\Delta C_{\text{free}}$ | $\Delta M$ | $\Delta W$ | $\Delta E_{\text{LP}}$ | $\Delta F_{\text{ins}}$ | $\Delta F_{\text{proto}}$ | $\sum \Delta$ Check |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **1. Trader Deposit (`ft_on_transfer` $x$)** | $+x$ | $+x$ | $0$ | $0$ | $0$ | $0$ | $0$ | $\Delta B = +x = \sum \text{Buckets}$ |
| **2a. Withdraw Start ($w$)** | $0$ | $-w$ | $0$ | $+w$ | $0$ | $0$ | $0$ | $0 = -w + w$ |
| **2b. Withdraw Callback `Ok`** | $-w$ | $0$ | $0$ | $-w$ | $0$ | $0$ | $0$ | $\Delta B = -w = -w$ |
| **2c. Withdraw Callback `Err`** | $0$ | $+w$ | $0$ | $-w$ | $0$ | $0$ | $0$ | $0 = +w - w$ |
| **3. LP Deposit ($d$)** | $+d$ | $0$ | $0$ | $0$ | $+d$ | $0$ | $0$ | $\Delta B = +d = +d$ |
| **4a. LP Withdraw Start ($a$)** | $0$ | $0$ | $0$ | $+a$ | $-a$ | $0$ | $0$ | $0 = +a - a$ |
| **5. Open Position (Margin $m$, Fee $f$)** | $0$ | $-(m+f)$ | $+m$ | $0$ | $+0.7f$ | $+0.1f$ | $+0.2f$ | $-(m+f) + m + f = 0$ |
| **6. Close Profit ($m$, Gain $+g>0$, Fee $f$)** | $0$ | $+(m+g-f)$ | $-m$ | $0$ | $-g+0.7f$ | $+0.1f$ | $+0.2f$ | $+g - g = 0$ |
| **7. Close Loss ($m$, Loss $-\ell$, $m \ge \ell+f$)** | $0$ | $+(m-\ell-f)$ | $-m$ | $0$ | $+\ell+0.7f$ | $+0.1f$ | $+0.2f$ | $-\ell + \ell = 0$ |
| **8. Liquidate Bad Debt ($m$ exhausted, Shortfall $s>0$, Fees $0$)** | $0$ | $0$ | $-m$ | $0$ | $+m + \min(F_{\text{ins}}, s)$ | $-\min(F_{\text{ins}}, s)$ | $0$ | $-m + m + c - c = 0$* |
| **9. Uniform Emergency Settlement (`fee=0`, Frozen $P_{\text{snap}}$)** | $0$ | $+\text{payout}$ | $-m$ | $0$ | $\text{vault\_delta} + \min(F_{\text{ins}}, s_{\text{em}})$ | $-\min(F_{\text{ins}}, s_{\text{em}})$ | $0$ | $\text{payout} - m + (m - \text{payout}) = 0$** |

*\*Note on Row 8 (`Bad Debt`) & Settlement Ordering:* Notice the shortfall $s$ represents an opposing trader's MTM win (which leaves `E_LP` via Row 6). Meanwhile the underwater position contributes `+m` into `E_LP` instead of `+(m+s)`. Insurance reimburses `LP Vault` by $+\min(F_{\text{ins}}, s)$, conserving contract physical tokens **bit-for-bit**! Furthermore, whenever closing an extreme winner would leave $E_{\text{LP,realized}} < \text{PnL}_{\text{win}}$ while opposing liquidatable/underwater positions hold unrecovered collateral or Insurance coverage, **M04.5A.1 enforces Liquidation-Before-Winner Ordering** (`liquidate` / bad-debt sweep executes prior to extreme winner cash-out so `E_LP_realized` never dips artificially below `0`). Any true residual uncovered gap ($s > F_{\text{ins}} + E_{\text{LP}}$) leaves `E_LP = 0` and triggers **Market Insolvency Pause** — **never touching other traders' `C_free` or unrelated `M`!**

*\*\*Note on Row 9 (`Uniform Emergency Settlement`):* Both winners (bounded by `max_pnl_bps = 10_000`) and losers unwind symmetrically via `compute_uniform_emergency_settlement_usd_e8` (`compute_position_settlement_e8`) with `close_fee_bps = 0`. LP withdrawals remain locked while any open position remains unclaimed (`open_positions > 0`), ensuring losing margins & Insurance reimbursements finalize before LP redemption.*

---

## 4. LP Vault Economic Cycle & Edge Cases

### 4.1 Should Shares Track LP NAV or a Fixed Unit?
**Decision: Shares represent pro-rata ownership of Mark-to-Market LP NAV (`lp_mtm_equity_e8`), NOT a fixed nominal unit.**
- **Why:** In a Peer-to-Pool engine, traders win/lose directly against the pool while fees and funding accumulate continuously into pool equity. Fixing shares to `$1.00` without floating NAV makes the first exit steal reserves when underwater or miss accrued fees when healthy. Using dynamic `LP NAV / Total Shares` keeps all active LPs exposed symmetrically pro-rata.

### 4.2 Share Minting, Redemption & Rounding Rules (`perps.rs`)
- **LP MTM Equity Basis (`compute_lp_mtm_equity_with_insurance_offset_e8`):**
  $$E_{\text{LP,MTM}} = E_{\text{LP,realized}} + \min\Big(F_{\text{ins}},\; \sum_{p \in \text{Open}} \text{unrealized\_bad\_debt}_p\Big) - \sum_{p \in \text{Open}} \max\Big(-M_p,\; \text{bounded\_PnL}_p(P) + \text{FundingNet}_p\Big)$$
  > **Why Insurance Offset Defeats Pre-Liquidation Arbitrage (`M04.5A.1` Finding H-1):** When a sharp market gap drives an opposing position into unrealized bad debt, omitting $+\min(F_{\text{ins}}, \text{bad\_debt}_{\text{unrealized}})$ temporarily depresses LP share price right before keeper liquidation transfers $F_{\text{ins}} \to E_{\text{LP,realized}}$, allowing opportunistic LP entrants to dilute existing LPs for guaranteed profit. Including the conservative insurance offset fixes NAV fairly **while keeping `lp_available_liquidity` strictly cash-backed** (never treating uncollected `F_ins` as withdrawable LP cash before liquidation executes).
- **Requires Fresh Oracle Price:** LP deposits and share redemptions **require fresh non-emergency oracle prices** (`ValidForExecution`) whenever ANY position is open (`Open Positions > 0`) so neither entrants nor exiters can front-run unpriced off-chain coffee moves! (When `Open Positions == 0`, $E_{\text{LP,MTM}} \equiv E_{\text{LP,realized}}$, allowing LP deposits/redemptions without external oracle dependency!).
- **First Depositor (Anti-Inflation Attack):**
  - Requires `deposit >= MIN_FIRST_LP_DEPOSIT_USD_E8` (`1 USDC = 10^8` internal units = `1_000_000` raw Circle Testnet USDC base units).
  - Permanently locks `MINIMUM_LP_SEED_SHARES = 10_000` (`0.0001` shares) inside protocol non-redeemable seed balance. User receives `deposit - 10_000` shares (`99.99%` on `$1`, `99.999999%` on `$10,000`).
- **Incremental Deposits:**
  - Reject if $E_{\text{LP,MTM}} \le 0$.
  - $\text{Shares}_{\text{mint}} = \left\lfloor \frac{\text{deposit} \times \text{TotalShares}}{E_{\text{LP,MTM}}} \right\rfloor$ (rounds **DOWN**, rejects `Shares_mint == 0`).
- **Simultaneous Deposits:** NEAR processes receipts sequentially per block, each computing against updated `(TotalShares, E_LP_MTM)`. Equal concurrent deposits in the same block with zero intervening price change mint **identical** share counts (`test_lp_simultaneous_deposits_...` verified).
- **Redemption / Withdrawal:**
  - Reject if $E_{\text{LP,MTM}} \le 0$ or `shares_burned > total_shares - MINIMUM_LP_SEED_SHARES`.
  - $\text{Assets}_{\text{out}} = \left\lfloor \frac{\text{shares} \times E_{\text{LP,MTM}}}{\text{TotalShares}} \right\rfloor$ (rounds **DOWN**, rejects `Assets_out == 0`).
  - Enforce Liquidity Guard: `Assets_out <= lp_available_liquidity`.

### 4.3 Edge-Case Matrix

| Edge Condition | Exact Behavior |
| --- | --- |
| **Pool has unrealized losses ($\Delta_{\text{MTM}} > 0$)** | Share price falls proportionally ($E_{\text{LP,MTM}} < E_{\text{LP,realized}}$). Existing LPs cannot dump losses onto newcomers or escape at `1.00`. |
| **Pool has unrealized gains ($\Delta_{\text{MTM}} < 0$)** | Share price rises proportionally, preventing newcomers from diluting LPs before losing trades close. However, `lp_available_liquidity` caps cash exit strictly to `E_LP_realized - R_OI` (no paying unrealized gains out of trader margin!). |
| **Insufficient free liquidity (`Assets_out > A_LP`)** | Call rejects cleanly with `ERR_INSUFFICIENT_LIQUIDITY`; open trader payouts and reserved liquidity remain 100% protected. |
| **Last External Withdrawer** | Last LP redeems up to `100%` of user-held shares; `MINIMUM_LP_SEED_SHARES` stays locked permanently so denominator `TotalShares >= 10_000` never drops to `0` (`no 0/0` reset). |
| **Pool enters Insolvency ($E_{\text{LP,MTM}} \le 0$ or $E_{\text{LP,realized}} + F_{\text{ins}} < \text{Shortfall}$)** | Both LP deposits and LP redemptions freeze (`ERR_INSUFFICIENT_LIQUIDITY`), new OI freezes, market switches to `PAUSED`, and other traders' `free_collateral` stays untouched. |

---

## 5. Explicit Assumptions, Open Decisions & Pending Risks

- **Assumptions:** Single multi-market pool settled internally in `USD_SCALE = 10^8` backed by allowlisted Circle NEAR Testnet USDC (`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`, `decimals = 6`, `scale_to_usd_e8 = 100` with lossless `<100 e8` residue retention); callbacks restore failed withdrawals cleanly without panicking. (`usdc.tidepool-sandbox.testnet` alternative fixture only; `rUSD` deferred to future roadmap.)
- **Open Decisions:** Whether LP exits require an optional cooldown epoch (`cooldown_sec`) if front-running latency becomes observable during M04.5G tests.
- **Pending Risks:** Extremely small residual bad debt if an LP withdraws up to `A_LP` just prior to a >`25%` instantaneous coffee price jump on a matched book.
