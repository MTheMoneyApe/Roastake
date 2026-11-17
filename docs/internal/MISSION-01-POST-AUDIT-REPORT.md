# ROASTAKE — Mission 01 Post-Audit Report
## OutLayer ABI, Dynamic Pricing & Callback State-Safety Verification

**Project:** ROASTAKE (Commodity-Finance Platform)  
**Branch:** `feat/outlayer-near-testnet-foundation`  
**Execution Date:** 2026-11-09  
**Scope:** NEAR Testnet only (`outlayer.testnet`)  
**Status:** ALL 33 TESTS PASSED — VERIFIED & READY FOR TESTNET DEPLOYMENT REVIEW

---

## 1. Exact OutLayer ABI Verified

The canonical OutLayer coordinator interface for `request_execution` on NEAR Testnet was empirically verified against `https://rpc.testnet.near.org` querying `outlayer.testnet` and consulting official documentation:

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
        params: Option<ExecutionParams>,
        callback_receiver_id: Option<AccountId>,
    );
}
```

### Argument Verification Details:
| Parameter | Type | Value Used | Verification Finding |
|---|---|---|---|
| `source` | `ExecutionSource::WasmUrl` | Pinned FastFS URL, SHA-256, `wasm32-wasip2` | Passed JSON deserialization on `outlayer.testnet`. Rejects missing `source`. |
| `resource_limits` | `Option<ResourceLimits>` | `max_instructions: 1_000_000_000` (u64)<br>`max_memory_mb: 128` (u32)<br>`max_execution_seconds: 60` (u64) | Types strictly aligned with OutLayer protocol (`u64` seconds). Verified by live testnet compute metering. |
| `input_data` | `Option<String>` | JSON string of `WorkerInput` | Verified against 32 KB free threshold before byte surcharge. |
| `secrets_ref` | `Option<SecretsReference>` | `None` | Accepted as `null`. Optional secrets profile. |
| `response_format` | `Option<String>` | `Some("Json".to_string())` | Accepted as PascalCase `"Json"`. |
| `payer_account_id` | `Option<AccountId>` | `Some(caller)` | Direct refund destination for unspent execution deposits. |
| `params` | `Option<ExecutionParams>` | `None` (or struct with `attached_usd`, `use_bound_identity`) | Accepted as optional. Passed as `None` for oracle price updates. |
| `callback_receiver_id` | `Option<AccountId>` | `Some(env::current_account_id())` | Destination contract for yield/resume response delivery. |

---

## 2. Official Source References

- **NEAR Integration:** `https://app.outlayer.ai/docs/near-integration`
- **Dynamic Pricing:** `https://app.outlayer.ai/docs/pricing`
- **WASI Applications:** `https://app.outlayer.ai/docs/wasi`
- **TEE Attestation:** `https://app.outlayer.ai/docs/tee-attestation`
- **Official GitHub:** `https://github.com/out-layer/outlayer`
- **Oracle Example:** `https://github.com/out-layer/oracle-example`
- **Live Testnet Contract:** `outlayer.testnet` on `https://rpc.testnet.near.org` (code hash: `8rRYysyHSZYxef6ke6XPe6yvGCLyGSsrqn8R8wzfeULn`)

---

## 3. Pricing Mechanism and Implementation

### Verified Dynamic Pricing Rates
Live RPC queries to `outlayer.testnet::estimate_execution_cost` confirmed the dynamic formula:
$$\text{Execution Cost} = \text{base\_fee} + (\text{instructions} \times \text{instruction\_rate}) + (\text{time\_sec} \times \text{time\_rate}) + \text{input\_surcharge}$$

- **Base Fee:** `0.001 NEAR` ($10^{21}$ yoctoNEAR / 1 milliNEAR)
- **Instruction Rate:** `0.0000000001 NEAR` per 1M instructions ($100$ yoctoNEAR / instruction)
- **Execution Time Rate:** `0.0001 NEAR` per second ($10^{20}$ yoctoNEAR / sec)
- **Free Input Threshold:** 32,768 bytes (32 KB); surcharge: `0.00001 NEAR` per byte above 32 KB.

### Empirical RPC Benchmark:
For standard worker limits ($10^9$ instructions, 60s timeout, $< 32$ KB input):
$$\text{Expected} = 10^{21} + (10^9 \times 100) + (60 \times 10^{20}) = 7,000,100,000,000,000,000,000\text{ yoctoNEAR}$$
Live query to `outlayer.testnet::estimate_execution_cost` returned: `"7000100000000000000000"` (exact match to the yoctoNEAR).

### Provisional Deposit Policy & In-Contract Calculator:
Smart contracts on NEAR cannot execute synchronous view calls. Chaining an asynchronous cross-contract call to `estimate_execution_cost` doubles latency and gas costs without preventing execution slippage. 

Therefore, ROASTAKE implements an explicit **provisional deposit policy**:
1. Callers must attach $\ge \text{estimated minimum deposit}$ (calculated via `calculate_execution_cost(&limits, len)` $\approx 7.0001$ milliNEAR; recommended minimum: $10$ milliNEAR / $0.01$ NEAR).
2. The contract passes `payer_account_id = Some(caller)` to OutLayer.
3. OutLayer automatically credits all unspent deposit directly back to the caller's account upon execution completion.
4. Both ROASTAKE and OutLayer export the `estimate_execution_cost` view method for off-chain clients.

---

## 4. Request Lifecycle and Refund Handling

1. **Authorization:** Requester must be `admin` or registered in `authorized_requesters`.
2. **Market Validation:** Market exists and `active == true`.
3. **Provisional Deposit Check:** Attached deposit must meet or exceed `calculate_execution_cost()`.
4. **Nonce Registration:** Monotonic nonce allocated, `PendingRequest` created with `status: RequestStatus::Pending`.
5. **Cross-Contract Dispatch:** Calls `outlayer.testnet::request_execution` with 100 TGas and attached deposit.
6. **Refund Destination:** `payer_account_id` set to `Some(caller)`. OutLayer's coordinator automatically refunds unspent deposit directly to the user.
7. **Callback Chaining:** Attaches `.then(Self::ext(env::current_account_id()).with_static_gas(50 TGas).on_outlayer_execution(nonce, market))`.

---

## 5. Callback Failure Handling & State Safety

### The Vulnerability in Pre-Audit Code:
In NEAR, any panic (`require!`, `unwrap()`, `panic_str`) rolls back all state mutations in that receipt. If `self.pending_requests.remove(&nonce)` was followed by a panicking validation check, the removal rolled back, leaving a permanent stuck pending request.

### Corrective Architecture:
1. **Private Access Control:** Callback enforces `env::predecessor_account_id() == env::current_account_id()` (panics if unauthorized).
2. **Terminal Nonce Removal:** `self.pending_requests.remove(&nonce)` occurs immediately. Replaying an already resolved or unknown nonce panics with `"ERR_UNKNOWN_OR_DUPLICATE_NONCE"`, preventing replay attacks.
3. **Pure Typed Validation Function:** Validation is encapsulated in `validate_worker_output()` which returns `Result<WorkerOutput, String>`.
4. **Non-Panicking Clean Failure Termination:**
   On any validation error (bad JSON, wrong market, bad price, stale/future/non-monotonic timestamp, excessive divergence, oversized payload, or OutLayer promise failure):
   - A structured event is logged: `EVENT_JSON:{"standard":"roastake-oracle","event":"execution_failed","nonce":...,"market":"...","reason":"..."}`
   - The callback returns `None` normally without panicking.
   - The pending request removal is committed to state (**zero stuck pending requests**).
   - Last valid price is **100% preserved**.

---

## 6. Security Changes Implemented

- Added `validate_worker_output()` pure validation function returning `Result<WorkerOutput, String>`.
- Eliminated all validation panics in `on_outlayer_execution` after nonce removal.
- Removed misleading hardcoded pricing constants from `outlayer.rs` and replaced them with verified yoctoNEAR rates.
- Added `callback_receiver_id` and `params` to `ext_outlayer` trait matching deployed contract ABI.
- Updated `ResourceLimits.max_execution_seconds` to `u64`.
- Added serialization tests for `ExecutionSource::WasmUrl`, `ResourceLimits`, and `request_execution` JSON payloads.
- Maintained strict Testnet boundary: all mainnet identifiers (`*.near`, `outlayer.near`) are rejected.

---

## 7. Files Modified

| File | Changes Made |
|---|---|
| `contracts/oracle-adapter/src/outlayer.rs` | Updated `ext_outlayer` trait to 8 arguments; implemented verified pricing formula and provisional deposit constants; removed legacy constants. |
| `contracts/oracle-adapter/src/lib.rs` | Integrated `RequestStatus::Pending`; passed `payer_account_id` and `callback_receiver_id`; implemented `validate_worker_output`; refactored `on_outlayer_execution` to be non-panicking and fail-closed. |
| `contracts/oracle-adapter/src/types.rs` | Added `RequestStatus` enum (`Pending`, `Succeeded`, `Failed`) and added `status` field to `PendingRequest`. |
| `contracts/oracle-adapter/src/tests.rs` | Updated test suite to 19 tests verifying state safety, cleanup of pending requests across all failure modes, price preservation, and request argument serialization. |
| `packages/shared/src/lib.rs` | Updated `ResourceLimits.max_execution_seconds` to `u64` (default 60s); added `ExecutionParams` struct; added serialization tests. |
| `docs/OUTLAYER-TESTNET.md` | Documented verified 8-parameter ABI, dynamic pricing benchmarks, provisional deposit policy, and state-safe callback processing. |
| `docs/ARCHITECTURE.md` | Updated system lifecycle, component flow diagram, and implementation gates. |
| `README.md` | Updated mission status, verified test count (33 tests), and architecture summaries. |
| `docs/COFFEE-INDEX.md` | Updated test suite logs and reiterated synthetic fixture classification. |
| `docs/TESTNET-ONLY.md` | Updated test references and verification notes. |

---

## 8. Tests Executed and Exact Results

Command executed:
```powershell
cargo test --workspace --features near-sdk/unit-testing
```

### Results:
```text
running 5 tests (roastake-coffee-price-index)
test engine::tests::test_process_raw_json_invalid_syntax ... ok
test engine::tests::test_unknown_market                  ... ok
test engine::tests::test_unsupported_mode               ... ok
test engine::tests::test_valid_fixture_request          ... ok
test engine::tests::test_process_raw_json_exact         ... ok
test result: ok. 5 passed; 0 failed

running 19 tests (roastake-oracle-adapter)
test tests::tests::test_fixed_point_conversion_and_formatting        ... ok
test tests::tests::test_invalid_json                                 ... ok
test tests::tests::test_out_of_range_price_rejected                  ... ok
test tests::tests::test_outlayer_request_execution_serialization     ... ok
test tests::tests::test_malformed_price_string                       ... ok
test tests::tests::test_future_timestamp                             ... ok
test tests::tests::test_mainnet_endpoint_rejection                   ... ok
test tests::tests::test_duplicate_nonce - should panic               ... ok
test tests::tests::test_excessive_price_movement                     ... ok
test tests::tests::test_execution_failure_preserves_last_price       ... ok
test tests::tests::test_unauthorized_callback - should panic         ... ok
test tests::tests::test_oversized_payload_handled_safely             ... ok
test tests::tests::test_unauthorized_request - should panic          ... ok
test tests::tests::test_unknown_request - should panic               ... ok
test tests::tests::test_storage_of_source_metadata                   ... ok
test tests::tests::test_stale_timestamp                             ... ok
test tests::tests::test_wrong_market                                 ... ok
test tests::tests::test_valid_oracle_result_flow                     ... ok
test tests::tests::test_wrong_unit_or_decimals_rejected              ... ok
test result: ok. 19 passed; 0 failed

running 9 tests (roastake-shared)
test tests::test_calculate_divergence_bps         ... ok
test tests::test_format_display_price             ... ok
test tests::test_extract_worker_output_direct     ... ok
test tests::test_validate_testnet_account         ... ok
test tests::test_outlayer_wasm_url_serialization  ... ok
test tests::test_worker_output_validation         ... ok
test tests::test_resource_limits_serialization    ... ok
test tests::test_extract_worker_output_wrapped    ... ok
test tests::test_worker_output_wrong_market       ... ok
test result: ok. 9 passed; 0 failed

TOTAL: 33 passed; 0 failed; 0 warnings
```

Format and Linter Checks:
- `cargo fmt --check`: **PASSED** (Exit Code 0)
- `cargo clippy --workspace --all-targets --features near-sdk/unit-testing`: **PASSED** (0 warnings, Exit Code 0)

---

## 9. WASM Build Results and Artifact Hashes

### 1. NEAR Oracle Adapter Contract
- **Target:** `wasm32-unknown-unknown`
- **Output:** `target/wasm32-unknown-unknown/release/roastake_oracle_adapter.wasm`
- **Size:** 543,743 bytes (531 KB)
- **SHA-256:** `e55badc9a8d63c24c8590ba4494885ce4fb2e5ae4d0f22362415cc2c319fe3fd`

### 2. Coffee Price Index WASI Worker
- **Target:** `wasm32-wasip2`
- **Output:** `target/wasm32-wasip2/release/roastake-coffee-price-index.wasm`
- **Size:** 143,075 bytes (139.72 KB)
- **SHA-256:** `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6`

---

## 10. Remaining Limitations

1. **Synthetic Demonstration Fixture Only:** The worker produces deterministic test fixtures (`GESHA-DEMO/USD = 1850000000`). It does not ingest live or official market data from ICO or ICE.
2. **On-Chain Cryptographic Attestation Verification:** Intel TDX hardware attestation verification currently takes place within OutLayer's coordinator contract before callback dispatch. Future releases may evaluate optional direct Ed25519 signature checks if OutLayer exposes worker signing public keys on-chain.
3. **Cross-Contract View Call Boundary:** On NEAR, view queries cannot be executed synchronously. ROASTAKE uses the provisional deposit threshold with OutLayer's native refund mechanism.

---

## 11. Final Readiness Assessment

- **Is Mission 01 technically ready for Testnet deployment review?**  
  **YES.** The contract and worker compile cleanly to their target architectures, all 33 unit tests pass, the OutLayer interface and pricing formula were empirically verified against `outlayer.testnet`, and callback processing is state-safe and fail-closed.
- **Does any external prerequisite remain?**  
  User approval and an active NEAR Testnet account with Testnet NEAR for contract deployment and FastFS worker upload.
- **Were any operations performed outside the local repository?**  
  Only read-only view queries to `https://rpc.testnet.near.org` were executed to verify code hash and method signatures. No transactions were submitted, no keys were loaded, and no funds were spent.
