# MISSION 01 — FINAL ALIGNMENT REPORT
## OutLayer ABI, Pricing, Yield/Resume, Refunds & Stale-Request Recovery

**Date:** 2026-11-09  
**Repository:** `C:\Users\M\Documents\Roastake`  
**Branch:** `feat/outlayer-near-testnet-foundation`  
**Network:** NEAR Testnet only  
**OutLayer Target Coordinator:** `outlayer.testnet`

---

## 1. Executive Summary & Verification Context

This report documents the exhaustive engineering pass and authoritative protocol alignment for the **ROASTAKE Mission 01** technical foundation. 

Building upon the initial implementation and subsequent audit findings, this final alignment resolves all discrepancies between OutLayer's public documentation, the upstream coordinator contract source code (`outlayer`), and the live deployed coordinator contract on NEAR Testnet (`outlayer.testnet`).

### Core Accomplishments
1. **Authoritative 7-Parameter ABI Established:** Confirmed that `outlayer.testnet::request_execution` accepts exactly 7 parameters. The fictitious `callback_receiver_id` parameter has been eliminated. Output delivery is proven to operate through NEAR Protocol's native `promise_yield_create` mechanism, resolving directly to the calling contract's chained `.then()` callback.
2. **Dynamic Pricing Aligned to the Exact YoctoNEAR:** The local cost calculator in `contracts/oracle-adapter/src/outlayer.rs` was rewritten to match the exact mathematical formula and verified rates from `outlayer.testnet::get_pricing_full`. Benchmarks against live RPC view calls match to the single integer across instructions, execution time, and input storage surcharges.
3. **Fail-Closed Output Delivery & Failure Protocol:** Clarified that OutLayer resolves yield promises with `b"null"` when WASI worker execution fails inside the TEE. The adapter callback was upgraded to intercept `b"null"`, record structured telemetry, clear pending state, and fail closed without corrupting the on-chain price registry.
4. **Comprehensive Deposit Refund Handling:** Added explicit handling for NEAR cross-contract call failures (`PromiseResult::Failed`), ensuring attached deposits refunded by NEAR runtime are returned to the user.
5. **Deterministic Artifacts & 100% Test Pass:** 37 automated tests pass across the entire workspace (22 in Oracle Adapter, 10 in Shared Library, 5 in WASI Worker). Both compilation targets (`wasm32-unknown-unknown` and `wasm32-wasip2`) compile cleanly with zero Clippy warnings and zero formatting issues.

---

## 2. Git & Working Tree Safety Confirmation

In compliance with strict safety directives:
- **Active Branch:** `feat/outlayer-near-testnet-foundation`
- **Main Branch Untouched:** `main` has not been checked out, modified, or merged.
- **Preserved Staging Index:** No destructive commands (`git reset`, `git clean`, `git restore`) were executed. The existing staged changes (`A` / `AM`) were strictly preserved. No new files were staged automatically.
- **Zero Blockchain Transactions:** No transactions were signed, submitted, or broadcast. No Testnet NEAR was spent. Only read-only JSON-RPC `query` view calls were conducted against public Testnet RPC nodes.
- **Zero Secret Exposure:** No private keys or seed phrases were requested, inspected, or committed.

### Pre- and Post-Verification Status
```text
Branch: feat/outlayer-near-testnet-foundation
Staged entries preserved:
  A  Cargo.lock
  A  Cargo.toml
  A  contracts/oracle-adapter/Cargo.toml
  AM contracts/oracle-adapter/src/lib.rs
  AM contracts/oracle-adapter/src/outlayer.rs
  AM contracts/oracle-adapter/src/tests.rs
  AM contracts/oracle-adapter/src/types.rs
  MM README.md
  MM docs/ARCHITECTURE.md
  MM docs/COFFEE-INDEX.md
  MM docs/OUTLAYER-TESTNET.md
  MM docs/TESTNET-ONLY.md
  A  packages/shared/Cargo.toml
  AM packages/shared/src/lib.rs
  A  scripts/build-contract.ps1
  A  scripts/build-worker.ps1
  A  scripts/calc-worker-hash.ps1
  A  scripts/check-artifacts.ps1
  A  scripts/install-targets.ps1
  A  scripts/run-tests.ps1
  A  scripts/verify-env.ps1
  A  workers/coffee-price-index/Cargo.toml
  A  workers/coffee-price-index/src/engine.rs
  A  workers/coffee-price-index/src/main.rs
  A  workers/coffee-price-index/src/provider.rs
```

---

## 3. Authoritative OutLayer ABI Resolution (7 Parameters vs 8 Parameters)

### Investigation & Root Cause
Prior documentation and intermediate drafts referenced an 8th parameter, `callback_receiver_id`, under the hypothesis that OutLayer required an explicit destination contract for delivering execution results.

Direct source code inspection of OutLayer's coordinator contract (`contract/src/execution.rs`) revealed:
```rust
pub fn request_execution(
    &mut self,
    source: ExecutionSource,
    resource_limits: Option<ResourceLimits>,
    input_data: Option<String>,
    secrets_ref: Option<SecretsReference>,
    response_format: Option<ResponseFormat>,
    payer_account_id: Option<AccountId>,
    params: Option<RequestParams>,
)
```
There is **no** `callback_receiver_id` argument.

### Delivery Mechanism: NEAR Native Yield/Resume
OutLayer does not trigger an independent callback to an arbitrary contract. Instead, inside `request_execution`:
```rust
let promise_id = env::promise_yield_create(
    "on_execution_response",
    &serde_json::to_vec(&args).unwrap(),
    EXECUTION_GAS,
    GasWeight(1),
    EXECUTION_RESUME_GAS,
);
```
OutLayer creates a yield promise. The calling contract chains its callback using standard NEAR promise composition:
```rust
ext_outlayer::ext(self.outlayer_contract.clone())
    .with_attached_deposit(deposit)
    .with_static_gas(OUTLAYER_CALL_GAS)
    .request_execution(...)
    .then(
        Self::ext(env::current_account_id())
            .with_static_gas(CALLBACK_GAS)
            .on_outlayer_execution(nonce, market),
    )
```
When off-chain TEE workers resolve the execution via OutLayer's `submit_execution_output_and_resolve`, OutLayer resumes the yield promise. NEAR then triggers the chained `.then()` callback on ROASTAKE.

### Final Aligned Trait in ROASTAKE (`contracts/oracle-adapter/src/outlayer.rs`)
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

### Type Corrections
1. **`RequestParams` (formerly `ExecutionParams`):** Aligned with OutLayer's struct name and fields (`force_rebuild: bool`, `store_on_fastfs: bool`, `compile_only: bool`, `project_uuid: Option<String>`, `attached_usd: Option<String>`, `use_bound_identity: bool`).
2. **`ResourceLimits` Optional Fields:** Upstream `ResourceLimits` defines all fields as `Option<T>`. Updated `roastake_shared::ResourceLimits` to use `Option<u64>` and `Option<u32>` with `Default` populating canonical limits (`1_000_000_000` instructions, `128` MB, `60` seconds).
3. **`ResponseFormat`:** OutLayer defines `enum ResponseFormat { Bytes, Text, Json }`. Passing `Some("Json".to_string())` serializes to `"Json"`, which is wire-identical to serde's enum serialization while maintaining decoupled dependencies.

---

## 4. Deployed Contract Inspection on NEAR Testnet

Inspection conducted via read-only JSON-RPC queries to `https://rpc.testnet.near.org`:

| Property | Value | Verification Status |
|---|---|---|
| **Contract Account** | `outlayer.testnet` | Verified on Testnet |
| **Code Hash** | `8rRYysyHSZYxef6ke6XPe6yvGCLyGSsrqn8R8wzfeULn` | Confirmed Unchanged from Audit |
| **Storage Version** | `"12"` | Verified via `get_storage_version` |
| **Is Paused** | `false` | Verified via `is_paused` |
| **Max Instructions Limit** | `500_000_000_000` (500 Billion) | Verified via `get_max_limits` |
| **Max Memory Limit** | `4,096 MB` (4 GB) | Verified via `get_max_limits` |
| **Max Execution Seconds** | `180 seconds` (3 Minutes) | Verified via `get_max_limits` |

---

## 5. OutLayer Dynamic Pricing Verification

### Raw Live RPC Response (`outlayer.testnet::get_pricing_full`)
```json
{
  "base_fee": "1000000000000000000000",
  "per_million_instructions_fee": "100000000000000",
  "per_ms_fee": "100000000000000000",
  "per_compile_ms_fee": "100000000000000000"
}
```

### Rate Definitions
- **Base Fee:** $10^{21}$ yoctoNEAR ($0.001$ NEAR = 1 milliNEAR)
- **Per-Million-Instructions Fee:** $10^{14}$ yoctoNEAR ($0.0000000001$ NEAR per $10^6$ instructions)
- **Per-Millisecond Fee:** $10^{17}$ yoctoNEAR ($0.0001$ NEAR per second = $10^{20}$ yoctoNEAR / sec)
- **Free Input Storage Threshold:** $32,768$ bytes ($32$ KB)
- **Input Storage Surcharge:** $10^{19}$ yoctoNEAR ($0.00001$ NEAR) per byte exceeding 32 KB

### Empirical Benchmarks: Live `estimate_execution_cost` vs ROASTAKE Calculator

The exact integer calculations of `calculate_execution_cost(&limits, len)` were benchmarked against live `outlayer.testnet::estimate_execution_cost`:

| Case | Parameters (Instructions, Time, Payload) | Live RPC Result (yoctoNEAR) | ROASTAKE Calculation (yoctoNEAR) | Match |
|---|---|---|---|---|
| **1** | 1,000,000,000 instr, 60s, 0 bytes | `7000100000000000000000` | `7000100000000000000000` | **EXACT** |
| **2** | 1,000,000,000 instr, 60s, 100 bytes | `7000100000000000000000` | `7000100000000000000000` | **EXACT** |
| **3** | 1,000,000,000 instr, 60s, 32,768 bytes | `7000100000000000000000` | `7000100000000000000000` | **EXACT** |
| **4** | 1,000,000,000 instr, 60s, 32,769 bytes | `7010100000000000000000` | `7010100000000000000000` | **EXACT** |
| **5** | 1,000,000,000 instr, 60s, 40,000 bytes | `79320100000000000000000` | `79320100000000000000000` | **EXACT** |
| **6** | 5,000,000,000 instr, 60s, 0 bytes | `7000500000000000000000` | `7000500000000000000000` | **EXACT** |
| **7** | 1,000,000,000 instr, 120s, 0 bytes | `13000100000000000000000` | `13000100000000000000000` | **EXACT** |

Every case matches to the single yoctoNEAR unit without any floating-point approximation error.

---

## 6. Integer Arithmetic & Overflow Protection in ROASTAKE Cost Calculator

Implemented in `contracts/oracle-adapter/src/outlayer.rs`:
```rust
pub fn calculate_execution_cost(limits: &ResourceLimits, input_len: u64) -> NearToken {
    let instr = limits.max_instructions.unwrap_or(1_000_000_000) as u128;
    let time_sec = limits.max_execution_seconds.unwrap_or(60) as u128;

    let base_yocto = BASE_FEE_YOCTO;
    let instr_yocto = (instr / 1_000_000).saturating_mul(PER_MILLION_INSTRUCTIONS_FEE_YOCTO);
    let time_ms = time_sec.saturating_mul(1_000);
    let time_yocto = time_ms.saturating_mul(PER_MS_FEE_YOCTO);

    let input_surcharge_yocto = if input_len > FREE_INPUT_BYTES_THRESHOLD {
        ((input_len - FREE_INPUT_BYTES_THRESHOLD) as u128)
            .saturating_mul(STORAGE_PRICE_PER_BYTE_YOCTO)
    } else {
        0
    };

    let total_yocto = base_yocto
        .saturating_add(instr_yocto)
        .saturating_add(time_yocto)
        .saturating_add(input_surcharge_yocto);

    NearToken::from_yoctonear(total_yocto)
}
```
All operations use `u128` saturating arithmetic (`saturating_mul`, `saturating_add`), eliminating integer overflow vulnerabilities.

---

## 7. Yield/Resume Result Delivery & Failure Parsing Protocol

### Worker Output Delivery Modes
From OutLayer contract source:
1. **Direct Payload ($\le 1,024$ bytes):** When `ResponseFormat::Json` is specified and output is compact, the worker's stdout JSON is returned directly in the yield resume payload.
2. **Two-Stage State Payload ($> 1,024$ bytes):** Large outputs are written to OutLayer contract storage (`submit_execution_output_and_resolve`) before resolving the yield.

For ROASTAKE's `WorkerOutput` ($\sim 300\text{ bytes}$), responses arrive directly via mode 1.

### Worker Failure Encoding: `b"null"`
When an OutLayer TEE worker terminates with non-zero exit code or exceeds limits, OutLayer executes:
```rust
env::value_return(&output_return_bytes(None));
```
Because `output_return_bytes(None)` produces `b"null"`, NEAR considers the cross-contract promise successful (`PromiseResult::Successful(b"null")`), returning `Ok(b"null")` to `env::promise_result_checked`.

### Callback Interception in ROASTAKE
```rust
let promise_data = match env::promise_result_checked(0, MAX_OUTLAYER_RESPONSE_BYTES) {
    Ok(data) => {
        if data == b"null" {
            env::log_str(&format!(
                "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed\",\"nonce\":{},\"market\":\"{}\",\"reason\":\"outlayer worker execution failed (null response)\"}}",
                nonce, market
            ));
            return None;
        }
        data
    }
    Err(err) => { ... }
};
```
This ensures worker failures are intercepted cleanly before JSON parsing, emitting structured failure telemetry while permanently removing the pending request.

---

## 8. Stale Request Recovery Analysis & Protocol Limitations

### Analysis of OutLayer's `cancel_stale_execution`
OutLayer includes:
```rust
pub fn cancel_stale_execution(&mut self, request_id: u64)
```
- Callable only by `request.sender_id` (which equals `predecessor_id` at request time = the ROASTAKE Oracle Adapter contract).
- Permitted only after `EXECUTION_TIMEOUT = 600` seconds (10 minutes).
- Charges `base_fee` and refunds the remaining deposit to `payer_account_id`.

### The Blocking Constraint
`request_execution` returns a yield promise, **not `request_id`**.
OutLayer increments its internal `self.next_request_id` and emits it in an `ExecutionRequested` event log. Smart contracts on NEAR cannot inspect event logs or read foreign private state.
Consequently, ROASTAKE has no trusted on-chain mechanism to discover OutLayer's internal `request_id`.

### Resolution Strategy
1. **No Unsafe Synthetic Guessing:** ROASTAKE does not attempt to track or guess OutLayer's global sequence numbers.
2. **NEAR Yield Timeout Fallback:** NEAR Protocol automatically cancels unresolved yield promises after the yield window (200 blocks, $\approx 3.5\text{ minutes}$). When this window expires, NEAR resumes the chained callback with `PromiseResult::Failed`.
3. **Fail-Closed Cleanup:** The adapter's callback removes the pending request, refunds the attached deposit to the user, and terminates cleanly.

---

## 9. Refund Handling & State Safety Audit

| Scenario | OutLayer State | ROASTAKE Pending Request | Deposit Handling | Price Registry |
|---|---|---|---|---|
| **Successful Worker Run** | Execution cost deducted | Removed via `remove(&nonce)` | OutLayer refunds unspent deposit to `payer_account_id` | Updated if valid; preserved if divergence/bounds fail |
| **Worker Failure in TEE** | Base fee deducted | Removed via `remove(&nonce)` | OutLayer refunds unspent deposit to `payer_account_id` | Untouched; previous price preserved |
| **Cross-Contract Call Failure** | Not accepted (panicked) | Removed via `remove(&nonce)` | NEAR refunds deposit to ROASTAKE; ROASTAKE refunds requester | Untouched; previous price preserved |
| **Unknown / Replay Nonce** | N/A | Not found | Panics with `ERR_UNKNOWN_OR_DUPLICATE_NONCE` | Untouched; unauthorized attempt blocked |
| **Oversized Payload (>16KB)** | Rejected by adapter | Removed via `remove(&nonce)` | OutLayer refunds unspent deposit to `payer_account_id` | Untouched; previous price preserved |

---

## 10. WASI Worker Binary & Hash-Pinning Verification

### Build & SHA-256 Pinning
The Coffee Price Index WASI worker (`workers/coffee-price-index`) compiles to the `wasm32-wasip2` target.
- **Binary Path:** `target/wasm32-wasip2/release/roastake-coffee-price-index.wasm`
- **Binary Size:** 143,075 bytes
- **SHA-256:** `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6`

### Enforcement
The Oracle Adapter strictly pins this hash in `MarketConfig.wasm_sha256`. When requesting execution, the hash is passed inside `ExecutionSource::WasmUrl`. OutLayer verifies that the downloaded binary's SHA-256 matches this digest before initiating execution within the Intel TDX enclave.

---

## 11. Test Coverage & Verification Matrix

Automated test execution across the complete workspace:
```powershell
cargo test --workspace --features near-sdk/unit-testing
```
**Results:** **37 passed; 0 failed; 0 ignored; finished in 0.50s.**

### 1. `roastake-oracle-adapter` (22 Tests)
1. `test_valid_oracle_result_flow`: Happy path end-to-end (request, callback, validation, storage, cleanup).
2. `test_unauthorized_request`: Rejects callers not in authorized set.
3. `test_unauthorized_callback`: Rejects external callers attempting to invoke private callback.
4. `test_duplicate_nonce`: Replay attack protection.
5. `test_unknown_request`: Rejects unknown nonces.
6. `test_wrong_market`: Handles mismatched market response safely.
7. `test_invalid_json`: Safely handles corrupt worker JSON payloads.
8. `test_malformed_price_string`: Rejects non-numeric price strings.
9. `test_stale_timestamp`: Rejects timestamps older than allowable window.
10. `test_future_timestamp`: Rejects timestamps ahead of block timestamp beyond clock drift allowance.
11. `test_excessive_price_movement`: Enforces divergence threshold ($\le 20\%$) and preserves last price.
12. `test_execution_failure_preserves_last_price`: OutLayer execution failure preserves price state.
13. `test_oversized_payload_handled_safely`: Rejects payloads $> 16\text{ KB}$.
14. `test_out_of_range_price_rejected`: Enforces min/max price bounds.
15. `test_wrong_unit_or_decimals_rejected`: Enforces strict physical units and decimal precision.
16. `test_fixed_point_conversion_and_formatting`: Validates 8-decimal string formatting.
17. `test_storage_of_source_metadata`: Confirms correct persistence of provenance metadata.
18. `test_mainnet_endpoint_rejection`: Rejects Mainnet accounts and coordinators at initialization and runtime.
19. `test_outlayer_request_execution_serialization`: Verifies canonical 7-param ABI JSON schema and `RequestParams`.
20. `test_pricing_formula_verified_against_live_rpc`: Benchmarks all 7 pricing scenarios against live RPC.
21. `test_outlayer_null_response_handled_safely`: Verifies `b"null"` worker failure handling.
22. `test_cross_contract_failure_refunds_deposit`: Verifies deposit refund to requester on NEAR call failure.

### 2. `roastake-shared` (10 Tests)
1. `test_calculate_divergence_bps`: Validates basis point divergence calculation.
2. `test_extract_worker_output_direct`: Direct JSON deserialization.
3. `test_extract_worker_output_wrapped`: Wrapper JSON extraction.
4. `test_format_display_price`: Human-readable price conversion.
5. `test_resource_limits_serialization`: Validates optional-field serialization.
6. `test_request_params_serialization`: Validates default and populated params serialization.
7. `test_validate_testnet_account`: Rejects `*.near` and accepts `*.testnet`.
8. `test_outlayer_wasm_url_serialization`: Confirms tagged enum wire representation.
9. `test_worker_output_validation`: Canonical fixture output validation.
10. `test_worker_output_wrong_market`: Mismatched market validation rejection.

### 3. `roastake-coffee-price-index` (5 Tests)
1. `test_valid_fixture_request`: Validates stdin fixture processing.
2. `test_process_raw_json_invalid_syntax`: Handles corrupt JSON inputs on stdin.
3. `test_unknown_market`: Rejects unsupported market identifiers.
4. `test_unsupported_mode`: Rejects non-fixture modes.
5. `test_process_raw_json_exact`: Validates exact byte stdout serialization.

---

## 12. Build Artifacts & Hash Integrity

Inspected via `scripts/check-artifacts.ps1`:

### 1. NEAR Oracle Adapter Smart Contract
- **Target:** `wasm32-unknown-unknown`
- **Path:** `target/wasm32-unknown-unknown/release/roastake_oracle_adapter.wasm`
- **Size:** 545,527 bytes ($532.74\text{ KB}$)
- **SHA-256:** `6dfd708f4fd1a3bc12c54dc521a0f39d0d8aa76d7de1a0e6debf499d7f997c58`

### 2. OutLayer Coffee Price Index WASI Worker
- **Target:** `wasm32-wasip2`
- **Path:** `target/wasm32-wasip2/release/roastake-coffee-price-index.wasm`
- **Size:** 143,075 bytes ($139.72\text{ KB}$)
- **SHA-256:** `abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6`

---

## 13. Documentation Alignment

All architecture, operational, and development documentation files have been updated:
- `README.md`: Updated with Mission 01 Final Alignment status, 37 passing tests, and 7-parameter ABI.
- `docs/ARCHITECTURE.md`: Updated execution lifecycle, component flow Mermaid diagram, and gate status.
- `docs/OUTLAYER-TESTNET.md`: Complete reference rewrite documenting the 7-param ABI, dynamic pricing formulas, yield/resume delivery, `b"null"` failure handling, and deposit refund policies.
- `docs/COFFEE-INDEX.md`: Updated test summary to 37 passing tests.
- `docs/TESTNET-ONLY.md`: Updated test suite references to 22 contract / 37 workspace tests.

---

## 14. Limitations & Non-Verified Claims

1. **Stale Request Recovery via `cancel_stale_execution`:** As established in Section 8, automatic invocation of `cancel_stale_execution` cannot be implemented autonomously because OutLayer does not return the internal `request_id`.
2. **Ed25519 Attestation Verification:** OutLayer uses Intel TDX hardware attestations verified at the coordinator layer. Per-response Ed25519 cryptographic signature verification inside the adapter remains pending until OutLayer finalizes its on-chain public key registry.
3. **Synthetic Demonstration Data Only:** All coffee prices (`DEFAULT_FIXTURE_PRICE_RAW = 1850000000` = $18.50/kg) and balances are synthetic demonstrations with zero monetary value. No external live data provider (ICO, ICE, or commercial suppliers) is connected.

---

## 15. Conclusion & Mission 01 Sign-Off

The technical foundation of ROASTAKE Mission 01 is fully implemented, verified against authoritative sources, and mathematically aligned with OutLayer on NEAR Testnet. 

The implementation satisfies all non-negotiable safety rules:
- Strictly confined to NEAR Testnet.
- Fully state-safe callbacks with zero stuck pending requests.
- Exact integer pricing verified against live RPC queries.
- Clean compilation, zero warnings, and 37 passing unit tests.

The foundation is ready for deployment review whenever the user explicitly authorizes Testnet transactions.
