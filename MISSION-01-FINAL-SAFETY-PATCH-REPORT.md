# ROASTAKE — MISSION 01 FINAL SAFETY PATCH REPORT
## Direct Yield Payload Limit & Native NEAR Deposit Refund Routing

**Date:** 2026-11-10  
**Repository:** `C:\Users\M\Documents\Roastake`  
**Branch:** `feat/outlayer-near-testnet-foundation`  
**Network:** NEAR Testnet only (`outlayer.testnet`)  
**Status:** PASSED (All 40 Workspace Unit Tests Pass; Contract & WASI Worker Binaries Built and Verified)

---

## 1. Executive Summary

This final safety pass addresses the two remaining protocol-level audit findings without altering the verified 7-parameter OutLayer ABI, pricing formula, or WASI worker implementation:

1. **Finding 1 (Payload Limit Correction):** Corrected `MAX_OUTLAYER_RESPONSE_BYTES` from `16 * 1024` (16 KB) to exactly `1_024` bytes, matching OutLayer's documented direct yield/resume return path limit. Updated callback handling to use `env::promise_result_checked` and explicitly distinguish `PromiseError::Failed`, `PromiseError::TooLong(len)`, and future `#[non_exhaustive]` variants. Oversized payloads are rejected cleanly without mutating prices or leaving stuck pending requests.
2. **Finding 2 (Native NEAR Deposit Refund Routing):** Investigated `near-sdk` v5.30.1 source and confirmed that `Promise::refund_to()` and `MockAction::SetRefundTo` are strictly gated behind `#[cfg(feature = "deterministic-account-ids")]`, which is disabled in standard `near-sdk` builds. Implemented a pull-based **credit fallback mechanism**: when a cross-contract call fails at the NEAR protocol level (`PromiseError::Failed`), the returned deposit is credited to the caller in state (`pending_refunds: LookupMap<AccountId, RefundCredit>`), and can be safely claimed at any time via `claim_refund()`.

---

## 2. Modified Files

| File | Type | Changes |
|------|------|---------|
| `contracts/oracle-adapter/src/types.rs` | Modified | Added `RefundCredit` struct (borsh + serde) with comprehensive architectural documentation regarding `Promise::refund_to()` gating and credit-based pull safety. |
| `contracts/oracle-adapter/src/lib.rs` | Modified | Updated `MAX_OUTLAYER_RESPONSE_BYTES` to `1_024`. Added `pending_refunds` and `total_refund_liability` to contract state. Explicitly matched `PromiseError::Failed`, `PromiseError::TooLong`, and `Err(_)`. Added `claim_refund()`, `get_pending_refund()`, `get_total_refund_liability()`, and internal `record_refund_credit()`. |
| `contracts/oracle-adapter/src/tests.rs` | Modified | Replaced outdated refund test with four comprehensive unit tests (Tests 22–25) covering credit recording, claim enforcement, oversized payload rejection, and exact 1,024-byte boundary validation. |
| `docs/OUTLAYER-TESTNET.md` | Modified | Documented the 1,024-byte limit, `PromiseError` variants, credit-based refund claim model, and deferred two-stage large output retrieval. Updated contract SHA-256. |
| `docs/ARCHITECTURE.md` | Modified | Updated lifecycle step 7 and Gate 1 verification status to reference 25 contract tests (40 workspace tests). |
| `README.md` | Modified | Updated workspace test count (40 tests) and project status description. |
| `docs/TESTNET-ONLY.md` | Modified | Updated test count in Testnet verification section. |

---

## 3. Finding 1: Direct Yield/Resume Payload Limit (1,024 Bytes)

### 3.1 Context & Rationale
OutLayer's execution coordinator on NEAR utilizes `promise_yield_create` and `promise_yield_resume` to deliver off-chain execution results back to calling contracts. The direct yield payload path supports payloads up to **1,024 bytes**. For larger outputs, OutLayer defines a two-stage coordinator storage mechanism requiring retrieval by `request_id`.

Because the ROASTAKE coffee price index worker produces approximately 300 bytes of JSON (`WorkerOutput`), Mission 01 operates safely within the direct response path. Setting the contract response limit to 16 KB created an inconsistency with OutLayer's direct path constraints.

### 3.2 Implementation Details
- `MAX_OUTLAYER_RESPONSE_BYTES` is defined as:
  ```rust
  pub const MAX_OUTLAYER_RESPONSE_BYTES: usize = 1_024;
  ```
- The callback uses `env::promise_result_checked(0, MAX_OUTLAYER_RESPONSE_BYTES)` and unpacks via an exhaustive pattern match on `near_sdk::PromiseError`:
  ```rust
  let raw_bytes = match env::promise_result_checked(0, MAX_OUTLAYER_RESPONSE_BYTES) {
      Ok(bytes) => bytes,
      Err(PromiseError::Failed) => {
          // Cross-contract call failed at NEAR level.
          // Deposit was returned to this contract. Record refund credit.
          self.record_refund_credit(&pending.requester, pending.deposit_attached, nonce);
          env::log_str(&serde_json::to_string(&serde_json::json!({
              "standard": "roastake-oracle",
              "event": "execution_failed",
              "nonce": nonce,
              "market": market,
              "reason": "OutLayer cross-contract call failed; refund credit recorded"
          })).unwrap_or_default());
          return None;
      }
      Err(PromiseError::TooLong(actual_len)) => {
          // Response exceeded the 1,024-byte direct yield limit.
          env::log_str(&serde_json::to_string(&serde_json::json!({
              "standard": "roastake-oracle",
              "event": "execution_failed",
              "nonce": nonce,
              "market": market,
              "reason": format!("OutLayer response exceeded 1,024-byte direct yield limit: {} bytes", actual_len)
          })).unwrap_or_default());
          return None;
      }
      #[allow(unreachable_patterns)]
      Err(_) => {
          // Future non-exhaustive PromiseError variants.
          env::log_str(&serde_json::to_string(&serde_json::json!({
              "standard": "roastake-oracle",
              "event": "execution_failed",
              "nonce": nonce,
              "market": market,
              "reason": "Unknown PromiseError variant in on_outlayer_execution callback"
          })).unwrap_or_default());
          return None;
      }
  };
  ```
- If the worker returns `b"null"` (an `Ok` result where the worker failed inside the TEE enclave), the callback detects this separately, emits an event, and terminates without altering prices.

---

## 4. Finding 2: Native NEAR Deposit Refund Routing & Credit Fallback

### 4.1 Root Cause Analysis: `Promise::refund_to()` Gating
Inspection of the `near-sdk` v5.30.1 crate source (`src/promise.rs` and `src/mock/mocked_blockchain.rs`) revealed:
```rust
#[cfg(feature = "deterministic-account-ids")]
pub fn refund_to(mut self, account_id: AccountId) -> Self { ... }
```
- In `near-sdk` 5.30.1, native receipt refund redirection via `refund_to()` is feature-gated under `deterministic-account-ids`.
- `MockAction::SetRefundTo` in test mocks is identically gated.
- Enabling this feature in `Cargo.toml` would modify dependency flags and alter the existing Git staging index, violating the non-negotiable safety rules.

### 4.2 Two Distinct Refund Mechanisms
It is critical to distinguish between the two separate refund pathways:
1. **OutLayer Compute Settlement Refund (Normal Path):** When `request_execution` succeeds at the NEAR level and executes on OutLayer, OutLayer computes actual gas/compute costs and automatically refunds unused funds directly to `payer_account_id` (passed as `Some(caller)`). This is managed off-chain/on-contract by OutLayer.
2. **NEAR Cross-Contract Failure Refund (Failure Path):** When the initial transaction receipt to `outlayer.testnet` cannot be processed (e.g. OutLayer contract unavailable, out of gas, yield failure, or invalid parameters), NEAR's protocol automatically refunds attached tokens back to the immediate caller: `contracts/oracle-adapter`. Because `refund_to()` is unavailable, the tokens cannot be natively forwarded at receipt routing time.

### 4.3 Pull-Based Credit Architecture
To eliminate all risk of trapped user funds, ROASTAKE implements a pull-based credit system:
- **State Storage:**
  - `pending_refunds: LookupMap<AccountId, RefundCredit>` (keyed by storage prefix `b"f"`)
  - `total_refund_liability: u128` tracks aggregate unwithdrawn refund balances
- **Recording:** On `PromiseError::Failed`, `self.record_refund_credit(&requester, deposit, nonce)` adds or accumulates the refund credit and increases total liability.
- **Claiming:** The user calls `claim_refund()`. The contract zeroes the credit, decrements `total_refund_liability`, and issues a fresh transfer: `Promise::new(caller).transfer(credit_amount)`.
- **View Methods:**
  - `get_pending_refund(account_id: AccountId) -> Option<RefundCredit>`
  - `get_total_refund_liability() -> U128`

### 4.4 Ordering Safety: Pull vs. Push
Executing a direct `Promise::new(requester).transfer(deposit)` inside the `on_outlayer_execution` callback is hazardous because the refund receipt from the failed outbound call arrives on NEAR asynchronously with non-deterministic ordering relative to the callback promise. If the transfer executed before the refund receipt was credited to the contract account, the contract could temporarily lack sufficient unlocked balance.
The credit-based pull pattern guarantees that the contract's balance has settled before the user claims, preventing balance depletion and transaction receipt failures.

---

## 5. Test Suite Verification

All 40 unit tests across the workspace pass without errors:
- **`roastake-coffee-price-index` (WASI worker):** 5 tests passed
- **`roastake-shared` (shared library):** 10 tests passed
- **`roastake-oracle-adapter` (NEAR smart contract):** 25 tests passed

### 5.1 New & Corrected Tests in `contracts/oracle-adapter/src/tests.rs`

1. **Test 22: `test_cross_contract_failure_records_refund_credit`**
   - Simulates `PromiseResult::Failed` from OutLayer.
   - Verifies that pending request is terminally cleared.
   - Verifies that `RefundCredit` is created for `alice.testnet` with the exact attached deposit (0.02 NEAR) and nonce.
   - Verifies that `total_refund_liability` matches the deposit.
   - Invokes `claim_refund()` as `alice.testnet`.
   - Verifies that `RefundCredit` is consumed and liability returns to 0.
   - Documents why `MockAction::SetRefundTo` cannot be asserted in tests due to SDK feature gating.
2. **Test 23: `test_claim_refund_no_credit_panics`**
   - Asserts `#[should_panic(expected = "No pending refund credit")]` when an account with zero balance (`bob.testnet`) calls `claim_refund()`.
3. **Test 24: `test_too_long_response_handled_safely`**
   - Constructs a payload of `MAX_OUTLAYER_RESPONSE_BYTES + 1` (1,025 bytes).
   - Passes it to `on_outlayer_execution`.
   - Verifies that `env::promise_result_checked` yields `PromiseError::TooLong(1025)`.
   - Verifies that the pending request is terminally cleared.
   - Verifies that the previous valid market observation is completely preserved.
   - Verifies that no refund credit is erroneously recorded (since OutLayer accepted the execution).
4. **Test 25: `test_valid_response_at_boundary_accepted`**
   - Submits a valid fixture payload (~300 bytes) within the 1,024-byte boundary.
   - Verifies successful validation, observation storage, and clean removal of pending state.

---

## 6. Deterministic Artifact Verification

Both binaries build cleanly with zero warnings:

| Artifact | Build Target | File Path | Size (Bytes) | SHA-256 Checksum |
|---|---|---|---|---|
| NEAR Oracle Adapter Contract | `wasm32-unknown-unknown` | `target/wasm32-unknown-unknown/release/roastake_oracle_adapter.wasm` | 579,665 | `aee5a9b4402f988367e1e1bab8b5a2c51ee72855c4db038e5d4c9280241360c2` |
| Coffee Price Index Worker | `wasm32-wasip2` | `target/wasm32-wasip2/release/roastake-coffee-price-index.wasm` | 143,075 | `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6` |

---

## 7. Deferred Items & Future Work

1. **Two-Stage Large-Output Retrieval:** OutLayer supports storing large results (> 1,024 bytes) in the coordinator for separate retrieval. Because ROASTAKE's WASI worker outputs strict JSON under 350 bytes, two-stage retrieval is not needed for Mission 01 and is deferred to Phase 2.
2. **Stale-Request Cancellation:** `cancel_stale_execution(request_id)` on OutLayer requires the coordinator `request_id`, which is emitted in receipt logs rather than returned in promise values. Yield expiration and timeout fallback via `PromiseError::Failed` safely handle stalled requests without stuck state.
3. **Ed25519 Hardware Attestation Signatures:** TEE enclave hardware attestation signature validation on-chain requires coordinator public key pinning and signature serialization specifications, deferred until verified with OutLayer engineering.

---

## 8. Git & Branch Compliance

- **Current Branch:** `feat/outlayer-near-testnet-foundation`
- **Protected Branch:** `main` untouched.
- **Git Index:** Existing staging index (`A`, `AM`, `MM` files) strictly preserved.
- **Transactions:** Zero Testnet NEAR spent; zero blockchain transactions executed; no private keys loaded or logged.
- **All 40 workspace tests passing.**
