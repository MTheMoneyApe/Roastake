# ROASTAKE — MISSION 02: SOLVENCY & REFUND RECOVERY HARDENING REPORT

**Repository:** `C:\Users\M\Documents\Roastake`  
**Branch:** `feat/outlayer-near-testnet-foundation`  
**Date:** 2026-11-10  
**Status:** IMPLEMENTED & VERIFIED  
**Auditor / Engineer:** Senior NEAR Protocol & Rust Security Engineer

---

## 1. Executive Summary

Mission 02 delivers a comprehensive security and accounting hardening pass on the ROASTAKE NEAR Testnet Oracle Adapter (`roastake-oracle-adapter`), resolving all findings identified in the Mission 01 independent audit:

1. **Global Solvency Guarantee:** Replaced local balance checks with a mathematically sound global solvency guard requiring `spendable_balance >= total_refund_liability` before initiating any refund claim transfer.
2. **Correct NEAR Runtime Balance Semantics:** Verified the exact semantics of `near-sdk` 5.30.1 and NEAR runtime: `env::account_balance()` represents liquid/unlocked account amount only; `env::account_locked_balance()` represents validator staking and does *not* reduce `account_balance()`. The spendable balance formula correctly deducts only required storage staking:
   $$\text{spendable\_balance} = \text{env::account\_balance()} - (\text{env::storage\_usage()} \times \text{env::storage\_byte\_cost()})$$
3. **Explicit Claim Identity & State Machine:** Introduced monotonic `claim_id` counter and persistent `RefundClaim` records binding claimant account, exact amount, originating request nonce, timestamp, and five explicit lifecycle statuses: `InFlight`, `Settled`, `RestoredOnFailure`, `ReconciledSuccess`, and `ReconciledFailure`.
4. **Callback Hardening & Replay Protection:** Hardened `on_refund_transfer(account_id, amount, claim_id)` to validate claim record existence, matching claimant, exact amount, and `InFlight` status. Replayed or unauthorized callbacks fail closed immediately.
5. **Safe Stuck-Claim Recovery (`admin_reconcile_stuck_claim`):** Provided a restricted, idempotent administrative recovery path for claims where the transfer receipt committed on NEAR but the confirmation callback ran out of gas or failed. Documented administrative attestation as an explicit trust assumption.
6. **Exact Response Boundary Verification:** Added dedicated tests verifying exact acceptance at 1,024 bytes and rejection with safe state preservation at 1,025 bytes.
7. **Comprehensive Test Suite:** Expanded the contract test suite from 35 to 54 unit tests (69 workspace tests total), covering all 15 required solvency and recovery scenarios.

---

## 2. Accounting Formula & NEAR Runtime Evidence

### 2.1 NEAR Balance Semantics Verification

In NEAR Protocol (`near-sdk` 5.30.1 / `nearcore` runtime):
- `env::account_balance()` queries the host's `current_account.amount`. This is strictly the **unlocked liquid balance** held by the contract.
- `env::account_locked_balance()` queries `current_account.locked`. This is balance locked in validator staking. It is completely separate from `amount` and is **not** available for storage staking.
- `env::storage_usage()` returns current storage consumed in bytes.
- `env::storage_byte_cost()` returns the protocol storage price (default $10^{19}$ yoctoNEAR per byte, or $0.00001$ NEAR/byte).
- The protocol invariant enforced by NEAR runtime is:
  $$\text{current\_account.amount} \ge \text{current\_account.storage\_usage} \times \text{storage\_byte\_cost}$$

**Deduction:** Subtracting `account_locked_balance()` from `account_balance()` was an accounting bug because locked balance was never part of `account_balance()`. The correct spendable balance formula is:
```rust
pub fn compute_spendable_balance() -> u128 {
    let total_unlocked = env::account_balance().as_yoctonear();
    let storage_locked = (env::storage_usage() as u128)
        .saturating_mul(env::storage_byte_cost().as_yoctonear());
    total_unlocked.saturating_sub(storage_locked)
}
```

### 2.2 Global Solvency Invariant

In Mission 01, `claim_refund()` checked `spendable >= claim_amount`. If multiple users had outstanding credits (e.g., Alice 5 NEAR, Bob 5 NEAR) and the contract held only 6 NEAR spendable, Alice could claim 5 NEAR, leaving the contract undercollateralized (1 NEAR balance against Bob's 5 NEAR entitlement).

Mission 02 strictly enforces **global solvency**:
```rust
let spendable = compute_spendable_balance();
require!(
    spendable >= self.total_refund_liability,
    format!(
        "Insufficient spendable contract balance to cover aggregate refund liabilities (spendable: {} yocto, total liability: {} yocto).",
        spendable, self.total_refund_liability
    )
);
```
No refund transfer can be dispatched unless the contract possesses sufficient liquid funds to honor **100% of all outstanding Available and InFlight refund liabilities**.

### 2.3 Protection from Ordinary Outgoing Operations

The only two methods creating outgoing NEAR value are:
1. `request_price_update(market)`: Outbound execution deposit to `outlayer.testnet`. The contract enforces `attached_deposit >= required_deposit`. The execution deposit is 100% funded upfront by the caller's transaction attachment. It never draws from contract balances or refund reserves.
2. `claim_refund()`: Transfer to user, strictly guarded by `spendable >= total_refund_liability`.

No administrative drain or unbacked outgoing transfer paths exist in the contract.

---

## 3. Refund Claim State Machine

### 3.1 State Diagram

```mermaid
stateDiagram-v2
    [*] --> Available: Cross-Contract Call Fails (PromiseError::Failed)\n[record_refund_credit]
    Available --> InFlight: User calls claim_refund()\n[spendable >= liability, allocates claim_id]
    InFlight --> Settled: on_refund_transfer (Successful)\n[liability decremented, credit removed]
    InFlight --> RestoredOnFailure: on_refund_transfer (Failed)\n[liability preserved, available restored]
    RestoredOnFailure --> InFlight: User retries claim_refund()\n[allocates new claim_id]
    InFlight --> ReconciledSuccess: admin_reconcile_stuck_claim (ConfirmedSuccess)\n[receipt confirmed on-chain, liability decremented]
    InFlight --> ReconciledFailure: admin_reconcile_stuck_claim (ConfirmedFailure)\n[receipt failed on-chain, available restored]
    ReconciledFailure --> InFlight: User retries claim_refund()\n[allocates new claim_id]
    Settled --> [*]
    ReconciledSuccess --> [*]
```

### 3.2 Data Structures

```rust
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub enum ClaimStatus {
    InFlight,
    Settled,
    RestoredOnFailure,
    ReconciledSuccess,
    ReconciledFailure,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub struct RefundClaim {
    pub claim_id: u64,
    pub claimant: AccountId,
    pub amount: NearToken,
    pub originating_nonce: u64,
    pub initiated_at_sec: u64,
    pub status: ClaimStatus,
}
```

---

## 4. Callback Hardening & Manual Reconciliation

### 4.1 `on_refund_transfer` Validations

The private transfer callback enforces six defensive checks in sequence:
1. **Private Access:** `require!(env::predecessor_account_id() == env::current_account_id())`.
2. **Claim Record Existence:** `claims.get(&claim_id)` must exist (`CRITICAL_ACCOUNTING_ERROR: missing refund claim record`).
3. **Claimant Binding:** `claim.claimant == account_id` (`CRITICAL_ACCOUNTING_ERROR: claimant mismatch`).
4. **Amount Binding:** `claim.amount.as_yoctonear() == amount_yocto` (`CRITICAL_ACCOUNTING_ERROR: claim amount mismatch`).
5. **State Invariant:** `claim.status == ClaimStatus::InFlight` (`CRITICAL_ACCOUNTING_ERROR: claim is not in-flight`).
6. **Promise Result Check:** `promise_result_checked(0, 64)`.
   - On success: marks claim `Settled`, clears credit `in_flight_amount`, checked-subtracts `total_refund_liability`, removes credit record if fully settled, clears `active_claim_id`.
   - On failure: marks claim `RestoredOnFailure`, restores `in_flight_amount` into `available_amount`, status becomes `Available`, clears `active_claim_id`, preserves `total_refund_liability`.

### 4.2 Administrative Recovery (`admin_reconcile_stuck_claim`)

If NEAR executes a transfer receipt but the confirmation callback runs out of gas or panics:
- The receipt execution result cannot be queried on-chain by the smart contract.
- `admin_reconcile_stuck_claim(claim_id, outcome)` permits the authorized contract administrator to resolve the stuck claim.
- **Rules:**
  1. Only admin (`assert_admin()`).
  2. Identifies exact `claim_id`.
  3. Fails closed if claim status is not `InFlight` (guarantees idempotency).
  4. Requires operator inspection of NEAR Explorer for the transfer receipt hash.
  5. `ReconcileOutcome::ConfirmedSuccess`: decrements liability, marks `ReconciledSuccess`.
  6. `ReconcileOutcome::ConfirmedFailure`: restores credit to Available, marks `ReconciledFailure`.
  7. Emits structured `refund_claim_reconciled` event.

> **Trust Assumption:** Administrative attestation of off-chain receipt execution is a trust assumption, not cryptographic on-chain verification. The contract relies on the administrator's integrity to truthfully report the off-chain receipt status.

---

## 5. Exact Response Boundary Verification

OutLayer direct yield/resume limit is **1,024 bytes**.

- **Accepted Boundary (1,024 bytes):** Serialized `WorkerOutput` with padding in `source` to exactly 1,024 bytes is accepted by `env::promise_result_checked(0, 1024)`. Price observation is stored, pending request cleaned up.
- **Rejected Boundary (1,025 bytes):** Payload of 1,025 bytes triggers `PromiseError::TooLong(1025)`. The callback returns `None`, cleans up pending request #2, preserves previous price observation (nonce 1), and records zero refund liability (since OutLayer accepted and executed the call).

---

## 6. Verification Results & Exit Codes

All verification commands executed cleanly on `feat/outlayer-near-testnet-foundation`:

```powershell
$env:PATH = "C:\Users\M\AppData\Local\bin\NASM;C:\Users\M\.cargo\bin;$env:PATH"

cargo fmt --check
# FMT_EXIT=0

cargo test --workspace --features near-sdk/unit-testing
# TEST_EXIT=0 (54 contract tests, 5 worker tests, 10 shared tests = 69 total tests)

cargo clippy --workspace --all-targets --features near-sdk/unit-testing
# CLIPPY_EXIT=0 (0 warnings)

powershell -ExecutionPolicy Bypass -File scripts\build-contract.ps1
# CONTRACT_BUILD_EXIT=0

powershell -ExecutionPolicy Bypass -File scripts\build-worker.ps1
# WORKER_BUILD_EXIT=0
```

### Artifact Checksums & Sizes

| Artifact | Target | Size (bytes) | SHA-256 Checksum |
| :--- | :--- | :--- | :--- |
| `roastake_oracle_adapter.wasm` | `wasm32-unknown-unknown` | 624,170 | `ad83a42a634a33f4a1163744b09eec2b6f33118c925ef2aec294ac74421df93a` |
| `roastake-coffee-price-index.wasm` | `wasm32-wasip2` | 143,075 | `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6` |

*Worker hash is perfectly preserved matching the pinned specification.*

---

## 7. Complete Test Coverage Mapping (15 Required Solvency Tests)

| Requirement | Test Function | Result |
| :--- | :--- | :--- |
| 1. Multiple users with aggregate liabilities exceeding individual refund | `test_solvency_1_multiple_users_aggregate_liabilities_exceed_single_refund` | PASS |
| 2. Under-collateralized contract rejects claim even if single amount affordable | `test_solvency_2_undercollateralized_contract_rejects_claim_when_single_amount_affordable` | PASS |
| 3. Fully collateralized contract permits sequential claims | `test_solvency_3_fully_collateralized_permits_sequential_claims` | PASS |
| 4. Failed claim transfer restores exact outstanding liability | `test_solvency_4_failed_claim_transfer_restores_exact_liability` | PASS |
| 5. Successful transfer reduces liability exactly once | `test_solvency_5_successful_transfer_reduces_liability_exactly_once` | PASS |
| 6. InFlight credits remain included in aggregate liabilities | `test_solvency_6_inflight_credits_remain_included_in_aggregate_liabilities` | PASS |
| 7. Two claims cannot collectively exceed available funds | `test_solvency_7_two_claims_cannot_collectively_exceed_available_funds` | PASS |
| 8. New refund credits cannot silently overflow aggregate liability | `test_solvency_8_new_refund_credits_cannot_silently_overflow` | PASS |
| 9. Storage requirements correctly incorporated into spendable balance | `test_solvency_9_storage_requirements_correctly_incorporated` | PASS |
| 10. Nonzero locked balance handled according to verified NEAR semantics | `test_solvency_10_nonzero_locked_balance_does_not_reduce_spendable` | PASS |
| 11. Storage underfunding fails closed | `test_solvency_11_storage_underfunding_fails_closed` | PASS |
| 12. Ordinary outgoing operations cannot spend refund-reserved funds | `test_solvency_12_ordinary_operations_cannot_spend_refund_reserved_funds` | PASS |
| 13. Multiple accounts remain strictly isolated | `test_solvency_13_multiple_accounts_remain_isolated` | PASS |
| 14. Replayed or mismatched claim IDs rejected | `test_solvency_14a_nonexistent_claim_id_rejected`<br>`test_solvency_14b_mismatched_claimant_rejected` | PASS |
| 15. Manual reconciliation is authorized, idempotent, and auditable | `test_solvency_15a_unauthorized_reconciliation_panics`<br>`test_solvency_15b_reconcile_nonexistent_claim_panics`<br>`test_solvency_15c_manual_reconciliation_lifecycle`<br>`test_solvency_15d_repeated_reconciliation_panics` | PASS |
| Exact 1,024-byte acceptance | `test_valid_response_at_boundary_accepted` | PASS |
| Exact 1,025-byte rejection | `test_too_long_response_handled_safely` | PASS |

---

## 8. Residual Risks & Technical Limitations

1. **NEAR Mock Testing Boundary:** Unit tests use `near-sdk::testing_env!`. Asynchronous cross-contract receipt execution and promise chaining are simulated via mock promise vectors. Actual receipt scheduling and gas consumption across multi-block transactions must be validated in Gate 3 on NEAR Testnet.
2. **Administrative Trust Assumption:** The manual reconciliation method (`admin_reconcile_stuck_claim`) relies on the contract owner accurately verifying receipt execution on NEAR Explorer. While the method cannot create new money or double-spend, an operator error could misclassify a dropped receipt.
3. **OutLayer Ed25519 Enclave Signature:** Per-response enclave signature verification remains deferred pending upstream OutLayer publication of their live verification key and signed receipt format.

---

## 9. Final Git Integrity Status

```
Branch: feat/outlayer-near-testnet-foundation
Staging index: Preserved without alterations
Destructive operations: None
No commits, pushes, merges, or Testnet transactions executed.
```

---

## 10. Final Verdict

- **Global Refund Solvency:** **PASS**
- **Verified Runtime Balance Semantics:** **PASS**
- **Claim Identity & InFlight Tracking:** **PASS**
- **Callback Hardening & Replay Protection:** **PASS**
- **Administrative Recovery Path:** **PASS** (with documented trust assumption)
- **Exact Response Boundary Tests:** **PASS**
- **Full Workspace Test Suite (69 tests):** **PASS**
- **Deterministic WASM Compilation:** **PASS**
- **Readiness for Independent Audit:** **READY**
