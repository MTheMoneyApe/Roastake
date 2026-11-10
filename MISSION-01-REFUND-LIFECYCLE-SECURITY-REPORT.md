# ROASTAKE — MISSION 01 REFUND LIFECYCLE SECURITY REPORT
## Failure-Safe Pull Refund Lifecycle, State Machine & Solvency Verification

**Date:** 2026-11-10  
**Repository:** `MTheMoneyApe/Roastake`  
**Branch:** `feat/outlayer-near-testnet-foundation`  
**Network:** NEAR Testnet only (`outlayer.testnet`)  
**Status:** PASSED (All 50 Workspace Unit Tests Pass; Formatting and Clippy Clean; WASM Targets Verified)

---

## 1. Executive Summary

This security audit and correction resolves the critical vulnerability in the deposit refund lifecycle of the ROASTAKE Oracle Adapter (`contracts/oracle-adapter`).

### Critical Issue Addressed
The previous `claim_refund()` implementation prematurely removed the user's `RefundCredit` from `pending_refunds` and decremented `total_refund_liability` before initiating the NEAR transfer promise (`Promise::new(caller).transfer(credit.amount)`). Because NEAR cross-contract actions are asynchronous, any failure during transfer (e.g. gas exhaustion, receiver account constraints, or temporary contract balance depletion) would permanently destroy the user's refund entitlement without recourse.

### Security Resolution
A robust, failure-safe state machine has been implemented:
1. **Atomic InFlight Tracking:** Calling `claim_refund()` transitions the entitlement from `RefundStatus::Available` to `RefundStatus::InFlight`. Aggregate liabilities remain completely intact until transfer delivery is confirmed.
2. **Solvency & Receipt Settlement Guard:** Before dispatching a transfer, the contract calculates its available liquid balance (`compute_available_liquid_balance() = account_balance - (storage_usage * storage_byte_cost + account_locked_balance)`). If the returned deposit receipt from a failed OutLayer call has not yet settled, the claim is safely rejected with a clear error without corrupting state.
3. **Private Confirmation Callback (`on_refund_transfer`):** Chained to the transfer action, the callback checks `promise_result_checked(0, 64)`.
   - On **Success**: clears in-flight balance, decreases `total_refund_liability` via checked subtraction, and purges the record if settled.
   - On **Failure**: restores in-flight balance back to `available_amount` via checked addition, resets status to `Available`, and preserves `total_refund_liability`. The user's entitlement is 100% preserved and claimable again.
4. **Checked Arithmetic Invariant Enforcement:** Eliminated saturating arithmetic for liability accounting. Any underflow, overflow, or replay violation panics immediately, ensuring accounting errors can never be silently hidden.

---

## 2. State Machine & Accounting Invariants

### 2.1 State Model (`RefundStatus`)

```
   [ OutLayer Call Fails (PromiseError::Failed) ]
                       │
                       ▼
             ┌───────────────────┐
             │     Available     │ ◄──────────────────────┐
             └───────────────────┘                        │
                       │                                  │
      claim_refund()   │ (Atomic transition;              │ on_refund_transfer
     [checks solvency] │  liability intact)               │ [PromiseError::Failed]
                       ▼                                  │ (Entitlement restored)
             ┌───────────────────┐                        │
             │     InFlight      │ ───────────────────────┘
             └───────────────────┘
                       │
                       │ on_refund_transfer [Ok(_)]
                       │ (Liability decremented)
                       ▼
             ┌───────────────────┐
             │ Fully Settled /   │
             │ Record Removed    │
             └───────────────────┘
```

### 2.2 Data Structures (`types.rs`)

```rust
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundStatus {
    Available,
    InFlight,
}

#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundCredit {
    pub requester: AccountId,
    pub available_amount: NearToken,
    pub in_flight_amount: NearToken,
    pub status: RefundStatus,
    pub from_nonce: u64,
}
```

### 2.3 Mathematical Invariants Enforced in Code

1. **Entitlement Preservation:**
   $$\text{Total User Entitlement} = \text{available\_amount} + \text{in\_flight\_amount}$$
2. **Aggregate Liability Exactness:**
   $$\text{total\_refund\_liability} = \sum_{\text{all users}} (\text{available\_amount}_i + \text{in\_flight\_amount}_i)$$
   At no point during execution (including mid-flight transfers) does `total_refund_liability` deviate from the sum of outstanding obligations.
3. **No Double Claims:** A user with $\text{in\_flight\_amount} > 0$ is barred from calling `claim_refund()` until the in-flight transfer completes.
4. **Replay & Over-Resolution Protection:** `on_refund_transfer` checks `credit.in_flight_amount >= amount_yocto`. If replayed or invoked with excess amounts, it panics with `CRITICAL_ACCOUNTING_ERROR`.
5. **No Silent Truncation:** All balance mutations use `.checked_add()` and `.checked_sub()`, failing closed on invariant breaches.

---

## 3. NEAR Protocol Receipt Ordering & Solvency Analysis

### 3.1 Asynchronous Receipt Flow

When ROASTAKE calls `outlayer.testnet::request_execution` with an attached deposit:
1. **Outbound Receipt ($R_1$):** Dispatched to `outlayer.testnet` carrying the attached deposit.
2. **Callback Receipt ($C_1$):** Chained as `.then(on_outlayer_execution)`.
3. If $R_1$ fails at the protocol level, NEAR runtime generates a refund receipt ($R_{\text{refund}}$) routing the attached deposit back to the predecessor (`oracle-adapter`).
4. **Receipt Ordering Hazard:** On NEAR's sharded DAG architecture, $C_1$ (carrying `PromiseError::Failed`) and $R_{\text{refund}}$ may arrive in different blocks or in non-deterministic sequence:
   - If $C_1$ executes in Block $N$, $R_{\text{refund}}$ might only be processed and credited in Block $N+1$.
   - Attempting to push funds back to the user inside $C_1$ would risk executing before $R_{\text{refund}}$ settles, causing contract insolvency or storage staking violations.

### 3.2 Solvency Guard Implementation

The pull model solves this race condition, augmented by a real-time solvency check:
```rust
pub fn compute_available_liquid_balance() -> u128 {
    let total_balance = env::account_balance().as_yoctonear();
    let storage_locked =
        (env::storage_usage() as u128).saturating_mul(env::storage_byte_cost().as_yoctonear());
    let locked_balance = env::account_locked_balance().as_yoctonear();
    let reserved = storage_locked.saturating_add(locked_balance);
    total_balance.saturating_sub(reserved)
}
```
When `claim_refund()` is called:
- If `compute_available_liquid_balance() < claim_amount`, the contract panics with:
  `"Insufficient liquid contract balance ... Deposit refund receipt may still be settling; please retry shortly."`
- The user's entitlement remains completely undisturbed in `RefundStatus::Available`. Once $R_{\text{refund}}$ settles, the subsequent claim succeeds.

---

## 4. Verification & Test Results

### 4.1 Verification Commands and Exit Codes

| Step | Command | Exit Code | Status |
|------|---------|-----------|--------|
| Formatting Check | `cargo fmt --check` | 0 | PASSED |
| Static Analysis | `cargo clippy --workspace --all-targets --features near-sdk/unit-testing` | 0 | PASSED (0 warnings) |
| Workspace Unit Tests | `cargo test --workspace --features near-sdk/unit-testing` | 0 | PASSED (50/50 tests pass) |
| Contract WASM Build | `powershell scripts\build-contract.ps1` | 0 | PASSED |
| Worker WASM Build | `powershell scripts\build-worker.ps1` | 0 | PASSED |

### 4.2 Unit Test Suite Breakdown (50 Tests Total)

- **`roastake-coffee-price-index` (WASI worker):** 5 tests passed
- **`roastake-shared` (shared library):** 10 tests passed
- **`roastake-oracle-adapter` (NEAR smart contract):** 35 tests passed

#### Dedicated Refund Lifecycle Test Coverage:
1. `test_cross_contract_failure_records_refund_credit` (Test 22): Verifies failure recording, initial Available state, InFlight claim transition, and Successful callback finalization.
2. `test_claim_refund_no_credit_panics` (Test 23): Verifies accounts without credit cannot claim.
3. `test_failed_refund_transfer_restores_entitlement` (Test 26): Verifies transfer failure does NOT erase entitlement; restores balance to Available and permits retry.
4. `test_duplicate_claim_attempt_while_in_flight_panics` (Test 27): Verifies double claims during active transfer are rejected.
5. `test_multiple_failures_accumulate_safely_for_same_requester` (Test 28): Verifies safe accumulation across multiple failed requests via checked addition.
6. `test_multiple_requesters_independent_credits_and_settlement` (Test 29): Verifies independent credit tracking and failure isolation across multiple accounts.
7. `test_aggregate_liability_exactness_across_all_transitions` (Test 30): Exercises complex interleaving (concurrent mid-flight failures and settlements), verifying `total_refund_liability` matches at every stage.
8. `test_insufficient_contract_balance_prevents_claim` (Test 31): Verifies that low liquid contract balance panics and preserves entitlement without losing funds.
9. `test_refund_callback_unauthorized_caller_panics` (Test 32): Verifies third parties cannot invoke `on_refund_transfer`.
10. `test_repeated_callback_replay_attempt_panics` (Test 33): Verifies replayed callbacks are detected and rejected.
11. `test_callback_replay_with_excess_amount_panics` (Test 34): Verifies callback cannot settle more than what is currently in-flight.
12. `test_failed_outlayer_call_preserves_valid_price` (Test 35): Verifies that cross-contract call failures never alter or clear previously stored price observations.

---

## 5. Artifact Sizes & Cryptographic Checksums

| Artifact | Target | Size (Bytes) | SHA-256 Hash |
|----------|--------|--------------|--------------|
| **NEAR Oracle Adapter Contract** | `wasm32-unknown-unknown` | 594,262 | `da9a348008123c83692ce572e42a212775be550f4e3abd3891598735ff2bd615` |
| **Coffee Price Index Worker** | `wasm32-wasip2` | 143,075 | `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6` |

> **Worker Hash Alignment:** The worker SHA-256 matches the pinned release hash (`abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6`) exactly. No worker code was altered.

---

## 6. Remaining Limitations & Future Work

1. **Two-Stage Large-Output Storage:** The direct yield/resume path is restricted to 1,024 bytes. The worker outputs ~300 bytes, operating safely within limits. Two-stage coordinator retrieval for larger payloads is deferred to Phase 2.
2. **OutLayer `cancel_stale_execution`:** Because OutLayer emits `request_id` only in execution logs and does not return it in the yield promise, on-chain contracts cannot automatically invoke `cancel_stale_execution`. Yield timeout handling via `PromiseError::Failed` serves as the fail-safe recovery mechanism.
3. **Ed25519 Hardware Attestation Signatures:** TEE hardware attestation signature validation on-chain remains deferred until OutLayer publishes authoritative verification specifications.

---

## 7. Git & Branch Compliance

- **Current Branch:** `feat/outlayer-near-testnet-foundation`
- **Protected Branch:** `main` untouched.
- **Git Operations:** Zero reset, clean, restore, rebase, add -A, commit, push, or merge commands executed. Existing staging index strictly preserved.
- **Transactions:** Zero blockchain transactions executed; zero Testnet NEAR spent; no keys exposed.
