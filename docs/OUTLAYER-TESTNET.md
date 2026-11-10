# OutLayer on NEAR Testnet

## Official References

- NEAR integration: https://app.outlayer.ai/docs/near-integration
- Dynamic pricing: https://app.outlayer.ai/docs/pricing
- Getting started: https://app.outlayer.ai/docs/getting-started
- WASI app guide: https://app.outlayer.ai/docs/wasi
- Example projects: https://app.outlayer.ai/docs/examples
- TEE attestation: https://app.outlayer.ai/docs/tee-attestation
- Official coordinator repository: https://github.com/out-layer/outlayer
- Official oracle example: https://github.com/out-layer/oracle-example

## Network Configuration

- OutLayer coordinator contract: `outlayer.testnet`
- FastFS Testnet: `https://test.fastfs.io`
- NEAR network: `testnet` only
- Build Target: `wasm32-wasip2`

Mainnet identifiers (`*.near`, `outlayer.near`) and hosts are strictly rejected at the contract and shared validation layers.

## Authoritative OutLayer Execution Interface (`request_execution`)

The contract calls `outlayer.testnet::request_execution` via an asynchronous cross-contract promise.
The exact 7-parameter interface has been verified against OutLayer contract source code and empirical RPC queries to the deployed `outlayer.testnet` contract:

```rust
#[ext_contract(ext_outlayer)]
pub trait OutLayer {
    fn request_execution(
        &mut self,
        source: ExecutionSource,
        resource_limits: Option<ResourceLimits>,
        input_data: Option<String>,
        secrets_ref: Option<SecretsReference>,
        response_format: Option<String>,
        payer_account_id: Option<AccountId>,
        params: Option<RequestParams>,
    );
}
```

### Parameter Specification

1. **`source` (`ExecutionSource::WasmUrl`)**:
   Direct WASM execution specification pinning the HTTPS FastFS URL, cryptographic SHA-256 hash, and WASI build target (`wasm32-wasip2`). Serializes to:
   ```json
   {
     "WasmUrl": {
       "url": "https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm",
       "hash": "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
       "build_target": "wasm32-wasip2"
     }
   }
   ```
2. **`resource_limits` (`Option<ResourceLimits>`)**:
   Enforced inside the TEE enclave with optional fields matching upstream:
   - `max_instructions`: `Some(1_000_000_000)` (u64, 1 billion instructions)
   - `max_memory_mb`: `Some(128)` (u32, 128 MB)
   - `max_execution_seconds`: `Some(60)` (u64, 60 seconds)
3. **`input_data` (`Option<String>`)**:
   Payload passed to the WASI worker's `stdin` as a JSON string (`WorkerInput`).
4. **`secrets_ref` (`Option<SecretsReference>`)**:
   Encrypted TEE profile reference (`None` for open public synthetic fixtures).
5. **`response_format` (`Option<String>`)**:
   Explicit output encoding; set to `Some("Json".to_string())`.
6. **`payer_account_id` (`Option<AccountId>`)**:
   Destination account for automatic refund of unspent execution deposits. Passed as `Some(caller)` so OutLayer returns excess funds directly to the user who requested and funded the execution.
7. **`params` (`Option<RequestParams>`)**:
   Optional execution control parameters (`force_rebuild`, `store_on_fastfs`, `compile_only`, `attached_usd`, `use_bound_identity`). Passed as `None` for standard oracle price updates.

> **Yield/Resume Result Delivery:**
> Result delivery is performed by OutLayer via NEAR's native `promise_yield_create` mechanism. The calling contract chains its callback using `.then(Self::ext(...).on_outlayer_execution(...))`. OutLayer's internal `on_execution_response` resolves the yield promise, delivering raw output bytes directly to the callback. There is no `callback_receiver_id` parameter in the contract ABI.

## Gas Allocation

- `OUTLAYER_CALL_GAS`: 100 TGas (for cross-contract invocation, parameter parsing, and TEE coordinator handoff)
- `CALLBACK_GAS`: 50 TGas (for on-chain structural validation, bounds checking, and state storage)

## Authoritative Cost Estimation & Provisional Deposit Policy

### Verified Dynamic Pricing Rates

OutLayer implements dynamic execution pricing verified against `outlayer.testnet::get_pricing_full` and empirical `estimate_execution_cost` queries:
- **Base Fee:** `0.001 NEAR` (1 milliNEAR = $10^{21}$ yoctoNEAR)
- **Instruction Rate:** `10^14 yoctoNEAR` per 1,000,000 instructions
- **Execution Time Rate:** `10^17 yoctoNEAR` per millisecond ($0.0001$ NEAR / sec = $10^{20}$ yoctoNEAR / sec)
- **Free Input Threshold:** `32,768` bytes (32 KB)
- **Input Surcharge:** `0.00001 NEAR` ($10^{19}$ yoctoNEAR) per byte for payloads exceeding 32 KB

Empirical verification querying `outlayer.testnet::estimate_execution_cost`:
- At $1,000,000,000$ instructions, $60$ seconds, and $\le 32,768$ bytes input:
  $$\text{Cost} = 10^{21} + \left(\frac{10^9}{10^6} \times 10^{14}\right) + (60 \times 1000 \times 10^{17}) = 10^{21} + 10^{17} + 6 \times 10^{21} = 7,000,100,000,000,000,000,000\text{ yoctoNEAR}$$
  Exact match with live RPC result: `"7000100000000000000000"`.

### On-Chain Provisional Deposit Policy

On NEAR, smart contracts cannot execute synchronous view calls to other contracts. Chaining an asynchronous cross-contract call to `estimate_execution_cost` before `request_execution` introduces additional receipt latency and gas costs without preventing execution-time slippage.

Furthermore, OutLayer's architecture natively handles dynamic pricing: callers attach a deposit to `request_execution`, and OutLayer's coordinator calculates exact resource consumption and automatically refunds unused funds to `payer_account_id`.

Therefore, ROASTAKE enforces an explicit **provisional deposit policy**:
1. When requesting a price update, the caller must attach a deposit meeting or exceeding the estimated minimum cost (`calculate_execution_cost(&limits, input_len)` $\approx 7.0001$ milliNEAR; recommended minimum: $10$ milliNEAR / $0.01$ NEAR).
2. The contract passes `payer_account_id = Some(caller)` to OutLayer.
3. OutLayer automatically credits all unspent compute deposit directly back to `caller`.
4. If `on_outlayer_execution` observes `PromiseError::Failed`, NEAR runtime does **not** guarantee whether the deposit returned to ROASTAKE (`InitialPanic`) or was retained by OutLayer (`DownstreamPromiseFailure`). ROASTAKE therefore records an `UnreconciledFailure` (with zero payable refund liability) rather than automatically crediting a refund; once backing is verified (`admin_designate_refund_reserve`) or explicitly funded (`fund_refund_reserve`), `promote_unreconciled_refund(nonce)` creates a 1:1 backed `RefundCredit`.
5. Off-chain clients and frontends can query `estimate_execution_cost` view method prior to submitting transactions.

## Callback Failure Handling & State Safety

In NEAR Protocol, transaction receipts that panic trigger a complete state rollback.
The aligned implementation ensures complete state safety across all callback execution paths:

1. **Private Access Control:** Callback enforces `env::predecessor_account_id() == env::current_account_id()` (panics if unauthorized).
2. **Terminal Nonce Removal:** `self.pending_requests.remove(&nonce)` is executed immediately. If the nonce is unknown or already consumed, it panics with `"ERR_UNKNOWN_OR_DUPLICATE_NONCE"`, preventing replay attacks.
3. **OutLayer Worker Failure Detection:** OutLayer's yield-resume returns `b"null"` when worker execution fails. The callback detects this, logs `execution_failed`, and terminates cleanly with `None`.
4. **Cross-Contract Call Failure Refund & Solvency-Backed Lifecycle (Mission 03C.3.6):** Two distinct refund domains exist and must never be conflated:
   - **OutLayer settlement refund (normal path):** When the initial `request_execution` receipt succeeds on `outlayer.testnet`, OutLayer records `payer_account_id = Some(caller)` (`RefundReceiver::Near(caller)`). On normal execution completion or normal worker failure (`Ok(b"null")`), OutLayer sends unspent or full deposit refunds directly to `caller`. ROASTAKE must never create a duplicate local refund credit for these cases.
   - **Ambiguity of `PromiseError::Failed` in `on_outlayer_execution`:** On NEAR, `PromiseError::Failed` in `on_outlayer_execution` can arise from two financially opposite scenarios:
     1. *Initial receipt failure (`InitialPanic` / pre-acceptance rejection):* `outlayer.testnet::request_execution` panics before accepting the deposit, so NEAR's protocol sends the attached deposit back to `RoastakeOracleAdapter`.
     2. *Downstream promise failure (`DownstreamPromiseFailure` / post-acceptance yield timeout):* `outlayer.testnet::request_execution` succeeds and retains the attached deposit in `outlayer.testnet`, while the downstream resumed promise fails. NEAR does **not** return the deposit to `RoastakeOracleAdapter`.
   - **UnreconciledFailure & Reserve-Backed Promotion:** Because `on_outlayer_execution` cannot assume that `PromiseError::Failed` returned funds to the adapter:
     1. `on_outlayer_execution` records an `UnreconciledFailure` (`unreconciled_failures` indexed by `nonce`) and emits `execution_failed_unreconciled`. It creates **zero** immediate `total_refund_liability` and **zero** payable `RefundCredit`.
     2. **Verified Spendable Balance Formula:**
        $$\text{spendable\_balance} = \text{env::account\_balance()} - (\text{storage\_usage()} \times \text{storage\_byte\_cost()})$$
     3. **Formal Global Solvency Invariant & Complete Collection Flush (`F-04`, `L-03`):** `RoastakeOracleAdapter` maintains **8** persistent `near_sdk::store` collections total (**1** uncached `LookupSet` `authorized_requesters` + **7** cached collections flushed by `flush_storage()`). Every state-mutating entrypoint and callback (`on_outlayer_execution`, `on_refund_transfer` success, `on_insurance_transfer` success) enforces:
        $$\text{spendable\_balance} \ge \text{minimum\_operating\_reserve} + \text{refund\_reserve\_balance} + \text{insurance\_reserve\_balance} + \text{insurance\_return\_pending} + \text{backed\_refund\_liability}$$
        where `backed_refund_liability = total_refund_liability - total_in_flight_liability - total_disputed_liability`, and `minimum_operating_reserve` is an explicit positive floor configured at `init` (and only monotonically increasable via `increase_minimum_operating_reserve` or `fund_operating_reserve`). Note that `disputed` liabilities remain non-payable and `in_flight` liabilities already deducted the outbound `Transfer` balance. Initial `init()` reserves `STATE_STORAGE_MARGIN_BYTES = 512`, `propose_admin` and `set_outlayer_contract` enforce `STATE_GROWTH_MARGIN_BYTES = 128`, and `WorkerOutput.source` is capped at `MAX_WORKER_SOURCE_BYTES = 128` UTF-8 bytes so new `PriceObservation` insertions stay within `OPEN_REQUEST_STORAGE_BUDGET_BYTES = 512`.
     4. **Reserve Funding & Designation (`F-07`) and Accepted Liveness Limitation (`M-01`):**
        - `fund_operating_reserve()` (`#[payable]`): adds attached NEAR directly to `minimum_operating_reserve` so contributed operating funds can never be reclassified into `refund_reserve_balance`.
        - `fund_refund_reserve()` (`#[payable]`): adds attached NEAR directly to `refund_reserve_balance`.
        - `admin_designate_refund_reserve(amount, evidence_sha256)`: designates unencumbered spendable funds above `compute_encumbered_backing_floor()` (`required_backing + total_in_flight_liability + total_disputed_liability + total_in_flight_insurance_liability + total_unresolved_insurance_liability`) into `refund_reserve_balance` after off-chain receipt verification. (`evidence_sha256` is an administrative audit attestation, not on-chain cryptographic proof.)
        - **Accepted `M-01` Liveness Limitation:** Because NEAR contracts cannot inspect historical receipt outcomes on-chain to distinguish a returning transfer refund (`Callback missing`) from an unrelated requester's `InitialPanic` deposit return, unacknowledged `Disputed` claims/compensations keep `encumbered_backing_floor` raised and block `admin_designate_refund_reserve` unless extra surplus exists. **Operational mitigation:** operators can call `fund_refund_reserve()` (`#[payable]`) with fresh NEAR and immediately promote the unrelated user's `UnreconciledFailure` via `promote_unreconciled_refund(nonce)` without waiting on or altering disputed claims.
     5. **Promotion or External Resolution State Machine (`F-01`, `L-02`):**
        - `promote_unreconciled_refund(nonce)`: requires `!failure.external_resolution_proposed` (`ERR_EXTERNAL_RESOLUTION_PROPOSED`) and `refund_reserve_balance >= deposit_at_risk`, atomically deducts `deposit_at_risk` from `refund_reserve_balance`, removes the `UnreconciledFailure`, and credits `pending_refunds` + `total_refund_liability` (`refund_credit_backed`).
        - `admin_resolve_unreconciled_external(nonce, evidence_sha256)` sets `external_resolution_proposed = true` without deleting the record; the original requester can either accept (`acknowledge_unreconciled_resolved_external(nonce)` -> terminal deletion) or reject (`reject_unreconciled_external_resolution(nonce)` -> clears proposal so `promote_unreconciled_refund` may proceed); admins can also cancel via `admin_cancel_unreconciled_external_resolution(nonce)` without auto-creating a refund.
     6. **Claim Settlement, Terminal Storage Cleanup, `ReturnPending` Safety & Disputed Liveness (`F-03`, `F-04`):**
        - Before every `assert_solvency_invariant()` check, `flush_storage()` explicitly flushes all **7** cached `near_sdk::store` collections (`LookupMap`, `IterableMap`) so `env::storage_usage()` reflects persisted trie storage bytes (`authorized_requesters: LookupSet` is uncached).
        - `claim_refund()` flushes storage, verifies `assert_solvency_invariant_with_outbound_transfer(amount_yocto)`, allocates a monotonic `claim_id`, transitions `available_amount` to `in_flight_amount`, and chains `Promise::transfer()` to `on_refund_transfer(caller, amount, claim_id)`. Historical `Disputed` claims clear `credit.active_claim_id = None` so new independent `available_amount` credits remain claimable without touching disputed amounts.
        - On transfer success: `in_flight_amount` and `total_refund_liability` are decremented once, the terminal `claim_id` is **removed** from `self.claims` (`claims.remove(&claim_id)`), and empty `RefundCredit` records are removed from `self.pending_refunds`.
        - On transfer failure: because NEAR's system-refund receipt for a failed `Transfer` may arrive in a subsequent block after `on_refund_transfer` executes, the claim transitions to `ClaimStatus::ReturnPending` and `RefundStatus::ReturnPending` (`return_pending_amount`), keeping the full amount inside `total_refund_liability`. Once the system refund settles, `reconcile_returned_refund(claim_id)` verifies the formal solvency invariant, restores `available_amount`, and removes the terminal `claim_id` from `self.claims`.
     7. **Restricted Manual Reconciliation, Non-Destructive Proposals & Insurance Solvency (`F-01`, `F-02`, Mission 03C.3.7.4):**
        - An `InFlight` claim whose callback did not settle can **never** be restored directly to `Available` or `ReturnPending` by an administrative assertion, elapsed timeout, aggregate solvency check, or `evidence_sha256` hash.
        - `admin_reconcile_stuck_claim(claim_id, ConfirmedFailure)` (or `mark_claim_disputed`, after `STUCK_CLAIM_MIN_AGE_BLOCKS = 100`) moves `InFlight` $\rightarrow$ `ClaimStatus::Disputed` (**NON-PAYABLE**), clearing `credit.active_claim_id` while preserving `disputed_amount` and `total_disputed_liability`.
        - `admin_reconcile_stuck_claim(claim_id, ConfirmedSuccess)` and `admin_propose_disputed_settlement(claim_id, evidence_sha256)` (after `DISPUTE_RESOLUTION_DELAY_BLOCKS = 1,000`) record `admin_settlement_proposed = true` without deleting the `RefundClaim` or reducing `total_refund_liability`. Terminal zero-payout closure requires claimant acknowledgement (`acknowledge_disputed_transfer_received(claim_id)`) or a late objective `on_refund_transfer` callback.
        - Discretionary insurance payouts (`set_insurance_enabled(true)`, `fund_insurance_reserve()`, `compensate_disputed_claim(claim_id, evidence_sha256)`) move `amount` from `insurance_reserve_balance` into `total_in_flight_insurance_liability` (bounded by `max_active_insurance_compensations`, default **`32`**, hard cap `100`). If `on_insurance_transfer` is lost after `STUCK_CLAIM_MIN_AGE_BLOCKS = 100`, `mark_insurance_compensation_disputed(compensation_id)` moves the record to `InsuranceCompensationStatus::Disputed` (`total_unresolved_insurance_liability`), which can be resolved via a late `on_insurance_transfer` callback or recipient acknowledgement (`acknowledge_insurance_compensation_received(compensation_id)`).
     8. **Deployment & Governance Policy:** Single-step `set_admin` is disabled in favor of two-step `propose_admin` + `accept_admin`. Because no legacy ROASTAKE Oracle Adapter has been deployed to Testnet, the contract uses **fresh deployment (`init`) on a new Testnet account** with `minimum_operating_reserve` (and optional `max_open_requests`). In-place WASM replacement over existing state is **forbidden** until an explicit Borsh migration entry point is implemented and verified. For production/mainnet value, configure `admin` as a $\ge 2$-of-$3$ or $3$-of-$5$ multisig/DAO governance contract rather than a single-key EOA. Note that local NEAR Sandbox integration tests use `Mock OutLayer` and do **not** prove live `outlayer.testnet` end-to-end execution.
5. **Pure Typed Validation:** Payload is validated via `validate_worker_output()` which returns `Result<WorkerOutput, String>`.
6. **Non-Panicking Failure Exit:** If OutLayer execution fails, the payload exceeds the **1,024-byte** direct yield/resume limit (`PromiseError::TooLong`), JSON is malformed, market mismatches, unit/decimals mismatch, price is out of bounds, divergence exceeds threshold ($> 20\%$), or timestamp is stale/future/non-monotonic:
   - A structured event is emitted:
     ```json
     {"standard":"roastake-oracle","event":"execution_failed","nonce":1,"market":"GESHA-DEMO/USD","reason":"..."}
     ```
   - The callback logs the event and returns `None` normally without panicking.
   - Because execution terminates normally, the removal of the pending request is committed to state. **No stuck pending requests are left behind.**
   - Existing price observations in `self.prices` are **never altered or replaced**.

## Payload Size Limits & Large Output Storage

OutLayer specifies a direct yield/resume payload limit of **1,024 bytes**. Payloads within 1,024 bytes are returned inline in the yield/resume completion.
- The ROASTAKE coffee price worker produces ~300 bytes of JSON output, safely operating well within this 1,024-byte boundary.
- If an OutLayer response exceeds 1,024 bytes, NEAR's `promise_result_checked` returns `PromiseError::TooLong(actual_length)`. The oracle adapter catches this variant explicitly, emits an `execution_failed` event indicating oversized response, and exits safely without mutating prices or leaving stuck requests.
- OutLayer supports a two-stage retrieval mechanism for large outputs (storing data via coordinator and retrieving by `request_id`). Because Mission 01 workers strictly output ~300 bytes, two-stage large-output retrieval is safely deferred as future work.

## Stale-Request Recovery Analysis

OutLayer (`outlayer.testnet`, storage version `"12"`) provides `cancel_stale_execution(request_id: u64)` allowing `request.sender_id` (`RoastakeOracleAdapter`) to cancel an execution after `EXECUTION_TIMEOUT = 600` seconds (10 minutes), which triggers `refund_deposit` back to `payer_account_id` (`RefundReceiver::Near(caller)`).
However, OutLayer's `request_execution` returns a yield promise (`Promise::new`) and **does not return `request_id` synchronously** to the calling contract; `request_id` is only emitted in OutLayer's event logs. Because smart contracts on NEAR cannot inspect event logs, `RoastakeOracleAdapter` cannot autonomously capture `request_id` during `request_price_update`.
Instead, if OutLayer's yield promise times out or fails with `PromiseError::Failed`, `on_outlayer_execution` removes the pending request and records an `UnreconciledFailure` (with `outlayer_request_id: None`), which can be promoted once backing is designated/funded or resolved externally via `admin_resolve_unreconciled_external` after off-chain verification.

## Deployment Preparation (Testnet)

### 1. Build WASI Worker
```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-worker.ps1
```

### 2. Verify Cryptographic SHA-256
```powershell
powershell -ExecutionPolicy Bypass -File scripts\calc-worker-hash.ps1
# Pinned release hash: abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6
```

### 3. Upload to FastFS Testnet (Requires user approval)
```bash
curl -X POST https://test.fastfs.io/upload \
  -F "file=@target/wasm32-wasip2/release/roastake-coffee-price-index.wasm"
```

### 4. Build NEAR Smart Contract (`cargo-near 0.22.0`)
```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-contract.ps1
# Release-Candidate WASM: integration-tests/near-sandbox-js/artifacts/roastake_oracle_adapter_release_candidate_m03376.wasm
# Pinned release-candidate hash: a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8 (378,335 bytes)
# NOTE: Never deploy unoptimized raw output from target/wasm32-unknown-unknown/release/roastake_oracle_adapter.wasm.
```

### 5. Fresh Deploy & Initialize on a New Testnet Account (Requires user approval)
```bash
near deploy <new-adapter-account>.testnet integration-tests/near-sandbox-js/artifacts/roastake_oracle_adapter_release_candidate_m03376.wasm
near call <new-adapter-account>.testnet init '{
  "admin": "<admin-or-multisig>.testnet",
  "outlayer_contract": "outlayer.testnet",
  "initial_wasm_url": "https://test.fastfs.io/roastake/worker.wasm",
  "initial_wasm_sha256": "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
  "minimum_operating_reserve": "10000000000000000000000",
  "max_open_requests": 100
}' --accountId <new-adapter-account>.testnet
```

### 6. Trigger Testnet Execution (Requires user approval)
```bash
near call <your-account>.testnet request_price_update '{"market": "GESHA-DEMO/USD"}' --accountId <your-account>.testnet --deposit 0.02 --gas 250000000000000
```
